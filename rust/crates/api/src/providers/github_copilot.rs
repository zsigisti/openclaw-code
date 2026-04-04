//! GitHub Copilot provider.
//!
//! Uses a GitHub OAuth token to obtain short-lived Copilot session tokens, then
//! calls the Copilot chat completions endpoint (OpenAI-compatible format).
//!
//! Authentication flow:
//! 1. `run_device_flow()` — GitHub device authorization → saves a GitHub OAuth token.
//! 2. At runtime, the GitHub token is exchanged for a Copilot session token (TTL ~30 min).
//! 3. Each `send_message` / `stream_message` reuses the cached session token, refreshing
//!    it automatically 60 s before expiry.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::error::ApiError;
use crate::providers::openai_compat::{
    ExtraHeaders, MessageStream, OpenAiCompatClient, OpenAiCompatConfig,
};
use crate::providers::{Provider, ProviderFuture};
use crate::types::{MessageRequest, MessageResponse};

// ── Constants ─────────────────────────────────────────────────────────────────

/// GitHub's public OAuth app client ID for the Copilot device flow.
/// This is the same client ID used by the GitHub Copilot Neovim/Vim plugin.
const DEVICE_FLOW_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// Headers required by the Copilot chat completions API.
/// Without `Editor-Version`, the API returns 400 "missing Editor-Version header for IDE auth".
const COPILOT_EXTRA_HEADERS: &[(&str, &str)] = &[
    ("Editor-Version", "vscode/1.85.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.12.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
    ("User-Agent", "GithubCopilot/1.155.0"),
];

const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const COPILOT_SESSION_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// Refresh the Copilot session token this many seconds before it actually expires.
const TOKEN_REFRESH_BUFFER_SECS: u64 = 60;

// ── Session token cache ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: u64, // unix timestamp (seconds)
}

impl CachedToken {
    fn is_valid(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.expires_at > now + TOKEN_REFRESH_BUFFER_SECS
    }
}

#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: u64,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// A provider client for GitHub Copilot.
///
/// Holds a GitHub OAuth token and caches the short-lived Copilot session token.
/// Uses the Copilot chat completions endpoint (`https://api.githubcopilot.com`)
/// with an OpenAI-compatible request format.
#[derive(Debug, Clone)]
pub struct GithubCopilotClient {
    http: reqwest::Client,
    github_token: String,
    cached: Arc<Mutex<Option<CachedToken>>>,
}

impl GithubCopilotClient {
    /// Create a client from a GitHub OAuth token (the long-lived token saved by
    /// `run_device_flow`).
    #[must_use]
    pub fn new(github_token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            github_token: github_token.into(),
            cached: Arc::new(Mutex::new(None)),
        }
    }

    /// Obtain a valid Copilot session token, refreshing if necessary.
    async fn session_token(&self) -> Result<String, ApiError> {
        // Check cache — release lock before any await.
        {
            let cache = self.cached.lock().expect("cache lock");
            if let Some(cached) = cache.as_ref() {
                if cached.is_valid() {
                    return Ok(cached.token.clone());
                }
            }
        }

        // Fetch a fresh session token from the GitHub API.
        let response = self
            .http
            .get(COPILOT_SESSION_TOKEN_URL)
            .header("Authorization", format!("token {}", self.github_token))
            .header("Accept", "application/json")
            .header("User-Agent", "claw-tui")
            .send()
            .await
            .map_err(ApiError::from)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::Api {
                status,
                error_type: None,
                message: Some(
                    "GitHub Copilot: failed to obtain session token — check your GitHub token"
                        .to_string(),
                ),
                body,
                retryable: status.as_u16() >= 500,
            });
        }

        let data: CopilotTokenResponse = response.json().await.map_err(ApiError::from)?;
        let token = data.token.clone();

        {
            let mut cache = self.cached.lock().expect("cache lock");
            *cache = Some(CachedToken {
                token: data.token,
                expires_at: data.expires_at,
            });
        }

        Ok(token)
    }

    /// Build a temporary OpenAiCompatClient with the given session token and
    /// the required Copilot-specific headers.
    fn compat_client(&self, session_token: String) -> OpenAiCompatClient {
        let headers = ExtraHeaders(
            COPILOT_EXTRA_HEADERS
                .iter()
                .map(|(k, v)| (*k, (*v).to_string()))
                .collect(),
        );
        OpenAiCompatClient::from_client(
            self.http.clone(),
            session_token,
            OpenAiCompatConfig::github_copilot(),
        )
        .with_extra_headers(headers)
    }

    /// Strip the `github-copilot/` prefix from the model name before sending.
    fn normalize_request(request: &MessageRequest) -> MessageRequest {
        let model = request
            .model
            .strip_prefix("github-copilot/")
            .unwrap_or(&request.model)
            .to_string();
        MessageRequest {
            model,
            ..request.clone()
        }
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        let token = self.session_token().await?;
        let request = Self::normalize_request(request);
        self.compat_client(token).send_message(&request).await
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        let token = self.session_token().await?;
        let request = Self::normalize_request(request);
        self.compat_client(token).stream_message(&request).await
    }
}

impl Provider for GithubCopilotClient {
    type Stream = MessageStream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse> {
        Box::pin(async move { self.send_message(request).await })
    }

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream> {
        Box::pin(async move { self.stream_message(request).await })
    }
}

// ── Device authorization flow ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct TokenPollResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    /// Increased polling interval sent by GitHub on `slow_down` errors.
    #[serde(default)]
    interval: Option<u64>,
}

/// Run the GitHub device authorization flow and return a GitHub OAuth token.
///
/// Prints the user code and verification URL to stdout. The caller is responsible
/// for saving the returned token (e.g., via `save_profile`).
pub async fn run_device_flow() -> Result<String, ApiError> {
    let http = reqwest::Client::new();

    // ── Step 1: request a device code ────────────────────────────────────
    let response = http
        .post(GITHUB_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", DEVICE_FLOW_CLIENT_ID), ("scope", "read:user")])
        .send()
        .await
        .map_err(ApiError::from)?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Api {
            status,
            error_type: None,
            message: Some("GitHub device code request failed".to_string()),
            body,
            retryable: false,
        });
    }

    let dc: DeviceCodeResponse = response.json().await.map_err(ApiError::from)?;

    // Print the user code so the caller can display it.
    println!();
    println!("  Visit:       {}", dc.verification_uri);
    println!("  Enter code:  {}", dc.user_code);
    println!();
    println!("  Waiting for authorization…");

    // ── Step 2: poll until authorized ────────────────────────────────────
    let mut poll_interval =
        std::time::Duration::from_secs(dc.interval.max(5));
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(dc.expires_in);

    loop {
        tokio::time::sleep(poll_interval).await;

        if tokio::time::Instant::now() > deadline {
            return Err(ApiError::Auth(
                "GitHub device code expired before authorization".to_string(),
            ));
        }

        let response = http
            .post(GITHUB_TOKEN_URL)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", DEVICE_FLOW_CLIENT_ID),
                ("device_code", dc.device_code.as_str()),
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:device_code",
                ),
            ])
            .send()
            .await
            .map_err(ApiError::from)?;

        let poll: TokenPollResponse = response.json().await.map_err(ApiError::from)?;

        if let Some(token) = poll.access_token.filter(|t| !t.is_empty()) {
            return Ok(token);
        }

        match poll.error.as_deref() {
            Some("authorization_pending") | None => {}
            Some("slow_down") => {
                let extra = poll.interval.unwrap_or(5);
                poll_interval += std::time::Duration::from_secs(extra);
            }
            Some("expired_token") => {
                return Err(ApiError::Auth("GitHub device code expired".to_string()));
            }
            Some("access_denied") => {
                return Err(ApiError::Auth("GitHub authorization denied by user".to_string()));
            }
            Some(other) => {
                return Err(ApiError::Auth(format!(
                    "GitHub authorization failed: {other}"
                )));
            }
        }
    }
}
