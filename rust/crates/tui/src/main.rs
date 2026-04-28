mod app;
mod auth;
mod config;
mod login;
mod telegram;
mod ui;

use std::env;

fn main() {
    // Load .env from the current directory (silently ignore if absent).
    let _ = dotenvy::dotenv();

    // Check for subcommands before doing anything else.
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("help") | Some("--help") | Some("-h") => {
            println!("openclaw-code — multi-provider AI chat\n");
            println!("USAGE");
            println!("  openclaw-code [model]              Start chat TUI");
            println!("  openclaw-code setup [model]        Configure credentials (interactive)");
            println!("  openclaw-code login                CLI credential wizard");
            println!("  openclaw-code telegram [model]     Run 24/7 Telegram bot");
            println!("  openclaw-code help                 Show this message\n");
            println!("MODELS");
            println!("  claude-sonnet-4-6  claude-opus-4-6  claude-haiku-4-5-20251001");
            println!("  gemini-2.5-pro     gemini-2.5-flash gemini-2.0-flash");
            println!("  gpt-4.1            gpt-4o           o3  o4-mini");
            println!("  grok-3             grok-3-mini");
            println!("  github-copilot/gpt-4o  github-copilot/gpt-4.1\n");
            println!("  Aliases: opus  sonnet  haiku  grok  grok-mini\n");
            println!("CREDENTIALS");
            println!("  ANTHROPIC_API_KEY      Claude models");
            println!("  OPENAI_API_KEY         GPT / o-series / Codex models");
            println!("  XAI_API_KEY            Grok models");
            println!("  GITHUB_COPILOT_TOKEN   GitHub Copilot models");
            println!("  GOOGLE_API_KEY         Gemini models");
            println!("  TELEGRAM_BOT_TOKEN     Telegram bot (telegram subcommand)\n");
            println!("  Credentials can also be saved with: openclaw-code setup");
            std::process::exit(0);
        }
        Some("login") => {
            match login::run_login() {
                Ok(()) => std::process::exit(0),
                Err(e) => { eprintln!("Login failed: {e}"); std::process::exit(1); }
            }
        }
        Some("setup") => {
            // Launch TUI with the setup wizard pre-opened; skip credential check.
            let model = args.iter().skip(1)
                .find(|a| !a.starts_with('-'))
                .cloned()
                .or_else(config::load_last_model)
                .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string());
            let mut tui_app = app::App::new_with_setup(model);
            if let Err(e) = tui_app.run() {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        Some("telegram") => {
            let model = args.iter().skip(1)
                .find(|a| !a.starts_with('-'))
                .cloned()
                .or_else(config::load_last_model)
                .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string());
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(telegram::run(model));
            std::process::exit(0);
        }
        _ => {}
    }

    // Load credentials from the OpenClaw auth-profiles store (if present).
    // These supplement env vars — env vars always take priority.
    let openclaw_creds = auth::load_openclaw_credentials();

    let model = args
        .into_iter()
        .find(|a| !a.starts_with('-'))
        .or_else(config::load_last_model)
        .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string());

    // Validate that at least one provider has credentials.
    let has_anthropic = env::var("ANTHROPIC_API_KEY").is_ok()
        || env::var("ANTHROPIC_AUTH_TOKEN").is_ok()
        || api::has_auth_from_env_or_saved().unwrap_or(false)
        || openclaw_creds
            .as_ref()
            .is_some_and(auth::LoadedCredentials::has_anthropic);
    let has_openai = env::var("OPENAI_API_KEY").is_ok()
        || openclaw_creds
            .as_ref()
            .is_some_and(|c| c.openai_key.is_some());
    let has_xai = env::var("XAI_API_KEY").is_ok()
        || openclaw_creds
            .as_ref()
            .is_some_and(|c| c.xai_key.is_some());
    let has_copilot = env::var("GITHUB_COPILOT_TOKEN").is_ok()
        || openclaw_creds
            .as_ref()
            .is_some_and(|c| c.github_copilot_token.is_some());
    let has_google = env::var("GOOGLE_API_KEY").is_ok()
        || openclaw_creds
            .as_ref()
            .is_some_and(|c| c.google_key.is_some());
    if !has_anthropic && !has_openai && !has_xai && !has_copilot && !has_google {
        eprintln!("No API credentials found. Run 'openclaw-code setup' to set up, or set one of:");
        eprintln!("  ANTHROPIC_API_KEY      — Claude models");
        eprintln!("  OPENAI_API_KEY         — GPT / o-series / Codex models");
        eprintln!("  XAI_API_KEY            — Grok models");
        eprintln!("  GITHUB_COPILOT_TOKEN   — GitHub Copilot models");
        eprintln!("  GOOGLE_API_KEY         — Gemini models");
        std::process::exit(1);
    }

    let mut tui_app = app::App::new(model);
    if let Err(e) = tui_app.run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
