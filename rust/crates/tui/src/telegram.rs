use std::collections::HashMap;
use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::ChatAction;
use tokio::sync::Mutex;

use api::{InputContentBlock, InputMessage, MessageRequest, OutputContentBlock, ProviderClient};

const TELEGRAM_MSG_LIMIT: usize = 4096;

#[derive(Clone)]
struct BotState {
    model: String,
    history: Arc<Mutex<HashMap<i64, Vec<InputMessage>>>>,
    google_key: Option<String>,
}

pub async fn run(model: String) {
    // Inject stored credentials into env so ProviderClient::from_model works.
    // This also covers the Telegram bot token saved via `openclaw-code setup`.
    if let Some(creds) = crate::auth::load_openclaw_credentials() {
        if std::env::var("TELEGRAM_BOT_TOKEN").is_err() {
            if let Some(t) = creds.telegram_bot_token {
                std::env::set_var("TELEGRAM_BOT_TOKEN", t);
            }
        }
        if std::env::var("ANTHROPIC_API_KEY").is_err() {
            if let Some(k) = creds.anthropic_api_key {
                std::env::set_var("ANTHROPIC_API_KEY", k);
            }
        }
        if std::env::var("OPENAI_API_KEY").is_err() {
            if let Some(k) = creds.openai_key {
                std::env::set_var("OPENAI_API_KEY", k);
            }
        }
        if std::env::var("XAI_API_KEY").is_err() {
            if let Some(k) = creds.xai_key {
                std::env::set_var("XAI_API_KEY", k);
            }
        }
        if std::env::var("GOOGLE_API_KEY").is_err() {
            if let Some(k) = creds.google_key {
                std::env::set_var("GOOGLE_API_KEY", k);
            }
        }
    }

    let token = std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
    if token.is_empty() {
        eprintln!(
            "No Telegram bot token found.\n\
             Run 'openclaw-code setup' to configure one, or set:\n\
             \n\
             export TELEGRAM_BOT_TOKEN=<token-from-@BotFather>\n\
             openclaw-code telegram [model]"
        );
        std::process::exit(1);
    }

    let google_key = std::env::var("GOOGLE_API_KEY").ok().filter(|k| !k.is_empty());

    let state = BotState {
        model: model.clone(),
        history: Arc::new(Mutex::new(HashMap::new())),
        google_key,
    };

    eprintln!("Telegram bot running (model: {model}) — press Ctrl+C to stop.");

    let bot = Bot::new(token);
    let handler = Update::filter_message().endpoint(handle_message);

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn handle_message(bot: Bot, msg: Message, state: BotState) -> ResponseResult<()> {
    let Some(text) = msg.text() else {
        return Ok(());
    };

    let chat_id_raw = msg.chat.id.0;
    let _ = bot.send_chat_action(msg.chat.id, ChatAction::Typing).await;

    // Append user message to per-chat history.
    {
        let mut h = state.history.lock().await;
        h.entry(chat_id_raw)
            .or_insert_with(Vec::new)
            .push(InputMessage::user_text(text));
    }

    let messages: Vec<InputMessage> = state
        .history
        .lock()
        .await
        .get(&chat_id_raw)
        .cloned()
        .unwrap_or_default();

    let client = match build_client(&state.model, state.google_key.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Provider error: {e}")).await?;
            return Ok(());
        }
    };

    let request = MessageRequest {
        model: state.model.clone(),
        max_tokens: 4096,
        messages,
        system: Some("You are a helpful AI assistant.".to_string()),
        tools: None,
        tool_choice: None,
        stream: false,
    };

    match client.send_message(&request).await {
        Ok(response) => {
            let reply: String = response
                .content
                .iter()
                .filter_map(|b| {
                    if let OutputContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("");

            if reply.is_empty() {
                return Ok(());
            }

            // Store assistant turn before sending (idempotent on error).
            {
                let mut h: tokio::sync::MutexGuard<'_, HashMap<i64, Vec<InputMessage>>> =
                    state.history.lock().await;
                if let Some(msgs) = h.get_mut(&chat_id_raw) {
                    msgs.push(InputMessage {
                        role: "assistant".to_string(),
                        content: vec![InputContentBlock::Text { text: reply.clone() }],
                    });
                }
            }

            // Telegram hard-limits messages to 4096 chars; chunk if needed.
            for chunk in split_text(&reply, TELEGRAM_MSG_LIMIT) {
                bot.send_message(msg.chat.id, chunk).await?;
            }
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Error: {e}")).await?;
        }
    }

    Ok(())
}

fn build_client(model: &str, google_key: Option<&str>) -> Result<ProviderClient, api::ApiError> {
    use api::{OpenAiCompatClient, OpenAiCompatConfig, ProviderKind};

    if api::detect_provider_kind(model) == ProviderKind::Google {
        if let Some(key) = google_key {
            return Ok(ProviderClient::Google(OpenAiCompatClient::new(
                key,
                OpenAiCompatConfig::google(),
            )));
        }
    }
    ProviderClient::from_model(model)
}

fn split_text(text: &str, max_bytes: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + max_bytes).min(text.len());
        // Walk back to a valid char boundary.
        let end = (0..=end)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(end);
        if end <= start {
            break;
        }
        parts.push(text[start..end].to_string());
        start = end;
    }
    parts
}
