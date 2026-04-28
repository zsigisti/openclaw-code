//! `openclaw-code login` — interactive provider authentication flow.
//!
//! Writes credentials to `~/.openclaw/agents/main/agent/auth-profiles.json`
//! in the same format that the `OpenClaw` app uses, so both tools share them.

use std::io::{self, BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;

use api::ClawApiClient;
use runtime::{
    generate_pkce_pair, generate_state, loopback_redirect_uri,
    OAuthAuthorizationRequest, OAuthConfig, OAuthTokenExchangeRequest,
};
use serde_json::{json, Map, Value};

// ── Anthropic OAuth config (same endpoints as Claude Code) ────────────────────

const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_AUTHORIZE_URL: &str = "https://platform.claude.com/oauth/authorize";
const ANTHROPIC_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const ANTHROPIC_SCOPES: &[&str] = &["user:inference", "user:profile", "org:create_api_key"];

// Port 0 = OS picks a free port; we read the actual port after binding.
const CALLBACK_PORT_HINT: u16 = 54_545;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run the interactive login wizard and save credentials.
/// Returns `Ok(())` on success or `Err(message)` on failure.
pub fn run_login() -> Result<(), String> {
    println!();
    println!("  openclaw-code login");
    println!("  ──────────────────────────────────────────");
    println!();

    loop {
        let provider = prompt_select(
            "Select provider",
            &[
                ("1", "Anthropic (Claude)  — OAuth browser login (free tier: Haiku only)"),
                ("2", "Anthropic (Claude)  — API key (console.anthropic.com)"),
                ("3", "OpenAI              — API key"),
                ("4", "xAI (Grok)          — API key"),
                ("5", "GitHub Copilot      — device login"),
                ("q", "Cancel"),
            ],
        )?;

        match provider.as_str() {
            "1" => {
                anthropic_oauth_flow()?;
                println!("\n  Anthropic OAuth login complete.");
                println!("  Note: OAuth access is free-tier — only Haiku is available.");
                println!("  For Opus/Sonnet, set ANTHROPIC_API_KEY from console.anthropic.com.\n");
                return Ok(());
            }
            "2" => {
                let key = prompt_secret("Anthropic API key (sk-ant-…)")?;
                save_profile("anthropic:claude-cli-key", json!({
                    "type": "api_key",
                    "provider": "anthropic",
                    "key": key
                }))?;
                println!("\n  Anthropic API key saved.\n");
                return Ok(());
            }
            "3" => {
                let key = prompt_secret("OpenAI API key (sk-…)")?;
                save_profile("openai:default", json!({
                    "type": "api_key",
                    "provider": "openai",
                    "key": key
                }))?;
                println!("\n  OpenAI API key saved.\n");
                return Ok(());
            }
            "4" => {
                let key = prompt_secret("xAI API key (xai-…)")?;
                save_profile("xai:default", json!({
                    "type": "api_key",
                    "provider": "xai",
                    "key": key
                }))?;
                println!("\n  xAI API key saved.\n");
                return Ok(());
            }
            "5" => {
                github_copilot_device_flow()?;
                println!("\n  GitHub Copilot login complete.\n");
                return Ok(());
            }
            "q" | "Q" => {
                println!("\n  Cancelled.\n");
                return Ok(());
            }
            _ => {
                println!("  Unknown choice — try again.");
            }
        }
    }
}

// ── GitHub Copilot device flow ────────────────────────────────────────────────

pub fn github_copilot_device_flow() -> Result<(), String> {
    println!("\n  Starting GitHub Copilot device login…");
    println!("  (Requires a GitHub account with an active Copilot subscription.)");

    let token = tokio_block(async { api::github_copilot_device_flow().await })
        .map_err(|e| format!("GitHub Copilot login failed: {e}"))?;

    save_profile(
        "github-copilot:default",
        json!({
            "type": "github_token",
            "provider": "github-copilot",
            "token": token,
        }),
    )?;
    Ok(())
}

// ── Anthropic OAuth (PKCE) ────────────────────────────────────────────────────

pub fn anthropic_oauth_flow() -> Result<(), String> {
    let pkce = generate_pkce_pair().map_err(|e| format!("PKCE generation failed: {e}"))?;
    let state = generate_state().map_err(|e| format!("State generation failed: {e}"))?;

    // Bind listener first so we know the actual port.
    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT_HINT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0u16)))
        .map_err(|e| format!("Cannot open callback listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();

    let redirect_uri = loopback_redirect_uri(port);
    let config = oauth_config();

    let auth_url = OAuthAuthorizationRequest::from_config(&config, &redirect_uri, &state, &pkce)
        .build_url();

    println!();
    println!("  Opening browser for Anthropic login…");
    println!("  If the browser does not open, visit:");
    println!();
    println!("    {auth_url}");
    println!();

    open_browser(&auth_url);

    println!("  Waiting for callback on http://localhost:{port}/callback …");

    let (stream, _) = listener
        .accept()
        .map_err(|e| format!("Callback server error: {e}"))?;

    let params = read_callback(&stream)?;

    // Verify state
    if params.state.as_deref() != Some(state.as_str()) {
        return Err("OAuth state mismatch — possible CSRF attack".to_string());
    }
    if let Some(err) = &params.error {
        let desc = params.error_description.as_deref().unwrap_or("");
        return Err(format!("OAuth error: {err}: {desc}"));
    }
    let code = params.code.ok_or("OAuth callback missing code")?;

    // Exchange code for tokens.
    let exchange = OAuthTokenExchangeRequest::from_config(
        &config,
        code,
        state,
        pkce.verifier,
        &redirect_uri,
    );
    let client = ClawApiClient::from_auth(api::AuthSource::None);
    let token_set = tokio_block(async move { client.exchange_oauth_code(&config, &exchange).await })
        .map_err(|e| format!("Token exchange failed: {e}"))?;

    // Create a long-lived workspace API key from the OAuth token.
    // The Messages API requires x-api-key (workspace key) + Authorization: Bearer (OAuth token).
    println!("  Creating workspace API key…");
    let access_token = token_set.access_token.clone();
    let client_for_key = ClawApiClient::from_auth(api::AuthSource::None);
    let api_key = tokio_block(async move {
        client_for_key.create_api_key_from_oauth(&access_token).await
    })
    .map_err(|e| format!("API key creation failed: {e}"))?;

    // Save the workspace API key (sent as x-api-key header).
    save_profile("anthropic:claude-cli-key", json!({
        "type": "api_key",
        "provider": "anthropic",
        "key": api_key
    }))?;

    // Save the OAuth access token (sent as Authorization: Bearer).
    // Both headers together are required for the Messages API when using OAuth login.
    let mut oauth_cred = json!({
        "type": "oauth",
        "provider": "anthropic",
        "access": token_set.access_token
    });
    if let Some(refresh) = token_set.refresh_token {
        oauth_cred["refresh"] = json!(refresh);
    }
    if let Some(expires_at) = token_set.expires_at {
        oauth_cred["expiresAt"] = json!(expires_at);
    }
    save_profile("anthropic:claude-cli", oauth_cred)?;

    Ok(())
}

fn oauth_config() -> OAuthConfig {
    OAuthConfig {
        client_id: ANTHROPIC_CLIENT_ID.to_string(),
        authorize_url: ANTHROPIC_AUTHORIZE_URL.to_string(),
        token_url: ANTHROPIC_TOKEN_URL.to_string(),
        callback_port: Some(CALLBACK_PORT_HINT),
        manual_redirect_url: None,
        scopes: ANTHROPIC_SCOPES.iter().map(|s| (*s).to_string()).collect(),
    }
}

// ── Callback HTTP server (minimal) ────────────────────────────────────────────

struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

fn read_callback(stream: &TcpStream) -> Result<CallbackParams, String> {
    let mut reader = BufReader::new(stream);
    // Read the HTTP request line: "GET /callback?code=…&state=… HTTP/1.1"
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| format!("Failed to read callback: {e}"))?;

    // Send a minimal success response so the browser shows something.
    let body = "<html><body><h2>Login complete</h2><p>You can close this tab.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    // Write response back (best-effort; ignore errors).
    let _ = (&*stream).write_all(response.as_bytes());

    // Parse the request target from the request line.
    // Format: "GET /callback?code=…&state=… HTTP/1.1\r\n"
    let target = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/callback");

    let params = runtime::parse_oauth_callback_request_target(target)
        .map_err(|e| format!("Bad callback URL: {e}"))?;

    Ok(CallbackParams {
        code: params.code,
        state: params.state,
        error: params.error,
        error_description: params.error_description,
    })
}

// ── OpenClaw auth-profiles.json ───────────────────────────────────────────────

fn auth_profiles_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".openclaw/agents/main/agent/auth-profiles.json"))
}

fn save_profile(profile_id: &str, cred: Value) -> Result<(), String> {
    let path = auth_profiles_path().ok_or("HOME not set")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create dir: {e}"))?;
    }

    let mut store: Map<String, Value> = if path.exists() {
        let bytes = std::fs::read(&path).map_err(|e| format!("Cannot read store: {e}"))?;
        serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    } else {
        Map::new()
    };

    let version = store
        .entry("version")
        .or_insert(Value::Number(1.into()))
        .clone();
    store.insert("version".to_string(), version);

    let profiles = store
        .entry("profiles")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(ref mut map) = profiles {
        map.insert(profile_id.to_string(), cred);
    }

    let rendered = serde_json::to_string_pretty(&Value::Object(store))
        .map_err(|e| format!("JSON error: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{rendered}\n"))
        .map_err(|e| format!("Cannot write profile: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("Cannot rename profile: {e}"))?;
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn prompt_select(label: &str, options: &[(&str, &str)]) -> Result<String, String> {
    println!("  {label}:");
    for (key, desc) in options {
        println!("    [{key}]  {desc}");
    }
    print!("\n  Choice: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("Input error: {e}"))?;
    Ok(line.trim().to_string())
}

fn prompt_secret(label: &str) -> Result<String, String> {
    print!("  {label}: ");
    io::stdout().flush().ok();
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|e| format!("Input error: {e}"))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err("Empty input — cancelled".to_string());
    }
    Ok(value)
}

fn open_browser(url: &str) {
    // Try common openers in order.
    for cmd in &["xdg-open", "open", "start"] {
        if Command::new(cmd).arg(url).spawn().is_ok() {
            return;
        }
    }
}

fn tokio_block<F, T>(future: F) -> Result<T, api::ApiError>
where
    F: std::future::Future<Output = Result<T, api::ApiError>>,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(future)
}
