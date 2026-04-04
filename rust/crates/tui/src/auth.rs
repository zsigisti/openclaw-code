//! Read credentials from the `OpenClaw` auth-profiles store.
//!
//! Auth-profiles path: `~/.openclaw/agents/main/agent/auth-profiles.json`
//! Format: `{ "version": 1, "profiles": { "<provider>:<name>": { "type": "...", ... } } }`
//!
//! Returned credentials supplement (but never override) values already present
//! in env vars or .env — the caller decides priority.

use std::env;
use std::path::PathBuf;

use serde_json::Value;

/// Credentials loaded from the `OpenClaw` auth-profiles store.
#[derive(Debug, Default)]
pub struct LoadedCredentials {
    /// Anthropic API key (`sk-ant-…`) if found.
    pub anthropic_api_key: Option<String>,
    /// Anthropic bearer token (OAuth access token or `ANTHROPIC_AUTH_TOKEN`).
    pub anthropic_token: Option<String>,
    /// `OpenAI` / Codex API key or OAuth access token.
    pub openai_key: Option<String>,
    /// xAI API key.
    pub xai_key: Option<String>,
    /// GitHub OAuth token used to obtain Copilot session tokens at runtime.
    pub github_copilot_token: Option<String>,
}

impl LoadedCredentials {
    /// Returns `true` if at least one Anthropic credential is present.
    #[must_use]
    pub fn has_anthropic(&self) -> bool {
        self.anthropic_api_key.is_some() || self.anthropic_token.is_some()
    }
}

fn store_path() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".openclaw/agents/main/agent/auth-profiles.json"))
}

/// `~/.config/openclaw-code/auth.json` — written by the in-TUI setup wizard.
fn oc_auth_path() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("openclaw-code/auth.json"))
}

/// Load one auth-profiles store file and merge into `creds`.
fn load_store_file(path: &PathBuf, creds: &mut LoadedCredentials) {
    let Ok(bytes) = std::fs::read(path) else { return };
    let Ok(store) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
    let Some(profiles) = store.get("profiles").and_then(serde_json::Value::as_object) else { return };
    for (profile_id, cred) in profiles {
        let cred_type = cred.get("type").and_then(serde_json::Value::as_str).unwrap_or("");
        let provider  = cred.get("provider").and_then(serde_json::Value::as_str).unwrap_or("");
        match (cred_type, provider) {
            ("api_key", "anthropic") => {
                let is_oauth_key = profile_id.contains("cli-key");
                if creds.anthropic_api_key.is_none() || is_oauth_key {
                    if let Some(key) = non_empty_str(cred.get("key")) {
                        creds.anthropic_api_key = Some(key);
                    }
                }
            }
            ("token", "anthropic") => {
                if creds.anthropic_token.is_none() {
                    if let Some(token) = non_empty_str(cred.get("token")) {
                        creds.anthropic_token = Some(token);
                    }
                }
            }
            ("oauth", "anthropic") => {
                if creds.anthropic_token.is_none() {
                    if let Some(access) = non_empty_str(cred.get("access")) {
                        creds.anthropic_token = Some(access);
                    }
                }
            }
            ("api_key", "openai") => {
                if creds.openai_key.is_none() {
                    if let Some(key) = non_empty_str(cred.get("key")) { creds.openai_key = Some(key); }
                }
            }
            ("oauth", "openai" | "openai-codex") => {
                if creds.openai_key.is_none() {
                    if let Some(a) = non_empty_str(cred.get("access")) { creds.openai_key = Some(a); }
                }
            }
            ("api_key", "xai") => {
                if creds.xai_key.is_none() {
                    if let Some(key) = non_empty_str(cred.get("key")) { creds.xai_key = Some(key); }
                }
            }
            ("github_token", "github-copilot") => {
                if creds.github_copilot_token.is_none() {
                    if let Some(t) = non_empty_str(cred.get("token")) { creds.github_copilot_token = Some(t); }
                }
            }
            _ => {}
        }
    }
}

fn claude_code_creds_path() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".claude/.credentials.json"))
}

/// Load credentials from all known stores, with a fallback to Claude Code.
#[must_use]
pub fn load_openclaw_credentials() -> Option<LoadedCredentials> {
    let mut creds = LoadedCredentials::default();

    // ── Primary: ~/.openclaw auth-profiles.json (legacy / `login` command) ──
    if let Some(path) = store_path() {
        load_store_file(&path, &mut creds);
    }

    // ── Secondary: ~/.config/openclaw-code/auth.json (setup wizard) ─────────
    if let Some(path) = oc_auth_path() {
        load_store_file(&path, &mut creds);
    }

    // ── Fallback: Claude Code's ~/.claude/.credentials.json ──────────────
    // If no Anthropic token found yet, try Claude Code's stored OAuth token.
    // This lets users who are already logged in via `claude login` skip a
    // separate `openclaw-code login` step.
    if creds.anthropic_token.is_none() {
        if let Some(path) = claude_code_creds_path() {
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(store) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(access) =
                        non_empty_str(store.get("claudeAiOauth").and_then(|o| o.get("accessToken")))
                    {
                        creds.anthropic_token = Some(access);
                    }
                }
            }
        }
    }

    if creds.has_anthropic()
        || creds.openai_key.is_some()
        || creds.xai_key.is_some()
        || creds.github_copilot_token.is_some()
    {
        Some(creds)
    } else {
        None
    }
}

fn non_empty_str(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?;
    if s.is_empty() { None } else { Some(s.to_string()) }
}
