use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::Value;

use crate::ui;

// ── Display items ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub enum DisplayItem {
    Message { role: Role, text: String },
    ToolCall { name: String, label: String, state: ToolState },
    SystemNote(String),
}

// ── API events ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ApiEvent {
    TextChunk(String),
    ToolStart { name: String, label: String },
    ToolResult { name: String, output: String, is_error: bool },
    Done { input_tokens: u32, output_tokens: u32 },
    Err(String),
}

// ── App status ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Idle,
    Streaming,
    Error(String),
}

// ── Model picker ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub provider: &'static str,
}

pub const KNOWN_MODELS: &[ModelEntry] = &[
    // Anthropic — Claude
    ModelEntry { id: "claude-opus-4-6",          label: "Claude Opus 4.6",       provider: "Anthropic" },
    ModelEntry { id: "claude-sonnet-4-6",         label: "Claude Sonnet 4.6",     provider: "Anthropic" },
    ModelEntry { id: "claude-haiku-4-5-20251001", label: "Claude Haiku 4.5",      provider: "Anthropic" },
    // OpenAI — GPT / reasoning / Codex
    ModelEntry { id: "gpt-4.1",                  label: "GPT-4.1",               provider: "OpenAI"    },
    ModelEntry { id: "gpt-4.1-mini",              label: "GPT-4.1 Mini",          provider: "OpenAI"    },
    ModelEntry { id: "gpt-4o",                    label: "GPT-4o",                provider: "OpenAI"    },
    ModelEntry { id: "gpt-4o-mini",               label: "GPT-4o Mini",           provider: "OpenAI"    },
    ModelEntry { id: "o3",                        label: "o3",                    provider: "OpenAI"    },
    ModelEntry { id: "o3-mini",                   label: "o3 Mini",               provider: "OpenAI"    },
    ModelEntry { id: "o4-mini",                   label: "o4 Mini",               provider: "OpenAI"    },
    ModelEntry { id: "codex-mini-latest",         label: "Codex Mini",            provider: "OpenAI"    },
    // xAI — Grok
    ModelEntry { id: "grok-3",                    label: "Grok 3",                provider: "xAI"       },
    ModelEntry { id: "grok-3-mini",               label: "Grok 3 Mini",           provider: "xAI"       },
    // GitHub Copilot
    ModelEntry { id: "github-copilot/gpt-4o",     label: "GPT-4o (Copilot)",      provider: "Copilot"   },
    ModelEntry { id: "github-copilot/gpt-4.1",    label: "GPT-4.1 (Copilot)",     provider: "Copilot"   },
    // Google — Gemini
    ModelEntry { id: "gemini-2.5-pro",            label: "Gemini 2.5 Pro",         provider: "Google"    },
    ModelEntry { id: "gemini-2.5-flash",          label: "Gemini 2.5 Flash",       provider: "Google"    },
    ModelEntry { id: "gemini-2.0-flash",          label: "Gemini 2.0 Flash",       provider: "Google"    },
    ModelEntry { id: "gemini-1.5-pro",            label: "Gemini 1.5 Pro",         provider: "Google"    },
    ModelEntry { id: "gemini-1.5-flash",          label: "Gemini 1.5 Flash",       provider: "Google"    },
];

#[derive(Debug, Clone)]
pub struct ModelPicker {
    pub cursor: usize,
    /// Input field for typing a custom model ID.
    pub custom_input: String,
    /// Whether cursor is on the custom input row (below the list).
    pub in_custom: bool,
}

impl ModelPicker {
    #[must_use]
    pub fn new(current_model: &str) -> Self {
        let cursor = KNOWN_MODELS
            .iter()
            .position(|m| m.id == current_model)
            .unwrap_or(0);
        Self { cursor, custom_input: String::new(), in_custom: false }
    }

    #[must_use]
    pub fn selected_model(&self) -> &str {
        if self.in_custom && !self.custom_input.trim().is_empty() {
            self.custom_input.trim()
        } else {
            KNOWN_MODELS[self.cursor].id
        }
    }
}

// ── Onboard wizard ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardStep {
    PickProvider,
    /// Only shown for providers that support OAuth (Anthropic).
    ChooseAuthMethod { provider: usize },
    EnterKey { provider: usize },
    /// After an AI provider key is saved — offer to also configure Telegram.
    TelegramOffer,
    /// Enter the Telegram bot token obtained from @BotFather.
    TelegramToken,
    Done,
}

#[derive(Debug, Clone)]
pub struct OnboardPopup {
    pub step: OnboardStep,
    pub provider_cursor: usize,
    pub key_input: String,
    pub error: Option<String>,
}

pub const ONBOARD_PROVIDERS: &[(&str, &str, &str)] = &[
    ("Anthropic", "Claude models (Opus, Sonnet, Haiku)", "ANTHROPIC_API_KEY"),
    ("OpenAI",    "GPT / o-series / Codex models",       "OPENAI_API_KEY"),
    ("xAI",       "Grok models",                          "XAI_API_KEY"),
    ("Copilot",   "GitHub Copilot models",                "GITHUB_COPILOT_TOKEN"),
    ("Google",    "Gemini models (2.5 Pro, 2.5 Flash, …)", "GOOGLE_API_KEY"),
];

impl OnboardPopup {
    pub fn new() -> Self {
        Self {
            step: OnboardStep::PickProvider,
            provider_cursor: 0,
            key_input: String::new(),
            error: None,
        }
    }
}

// ── App ────────────────────────────────────────────────────────────────────

pub struct App {
    pub model: String,
    pub display: Vec<DisplayItem>,
    pub input: String,
    pub cursor: usize,
    pub scroll_up: usize,
    pub status: Status,
    pub streaming_buf: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub tick: u64,
    pub model_picker: Option<ModelPicker>,
    pub onboard: Option<OnboardPopup>,
    /// Set when the onboard wizard needs to suspend the TUI and run OAuth stdio flow.
    pub pending_oauth: bool,
    /// Set when the onboard wizard needs to suspend the TUI and run the Copilot device flow.
    pub pending_copilot_flow: bool,

    /// Full message history sent to the API.
    api_history: Vec<api::InputMessage>,

    event_rx: mpsc::Receiver<ApiEvent>,
    event_tx: mpsc::Sender<ApiEvent>,
}

impl App {
    #[must_use]
    pub fn new(model: String) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            model,
            display: Vec::new(),
            input: String::new(),
            cursor: 0,
            scroll_up: 0,
            status: Status::Idle,
            streaming_buf: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            tick: 0,
            model_picker: None,
            onboard: None,
            pending_oauth: false,
            pending_copilot_flow: false,
            api_history: Vec::new(),
            event_rx: rx,
            event_tx: tx,
        }
    }

    /// Same as `new` but opens the onboard setup wizard immediately.
    #[must_use]
    pub fn new_with_setup(model: String) -> Self {
        let mut app = Self::new(model);
        app.onboard = Some(OnboardPopup::new());
        app
    }

    pub fn run(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;

        // Enable the Kitty keyboard protocol on terminals that support it.
        // This lets us distinguish Shift+Enter from Enter and Ctrl+M from Enter.
        let enhanced = supports_keyboard_enhancement().unwrap_or(false);
        if enhanced {
            execute!(
                stdout,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
                )
            )?;
        }

        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;

        let result = self.event_loop(&mut terminal);

        if enhanced {
            execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
        }
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        result
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> io::Result<()> {
        loop {
            self.tick = self.tick.wrapping_add(1);
            terminal.draw(|f| ui::render(f, self))?;

            // If the onboard wizard requested an OAuth flow, suspend the TUI,
            // run the stdio-based OAuth, then restore the TUI.
            if self.pending_oauth {
                self.pending_oauth = false;
                execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                disable_raw_mode()?;
                terminal.show_cursor()?;

                let result = crate::login::anthropic_oauth_flow();

                enable_raw_mode()?;
                execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                terminal.hide_cursor()?;
                terminal.clear()?;

                match result {
                    Ok(()) => {
                        self.onboard = None;
                        self.display.push(DisplayItem::SystemNote(
                            "OAuth login complete! Credentials saved.".to_string(),
                        ));
                    }
                    Err(e) => {
                        if let Some(ob) = self.onboard.as_mut() {
                            ob.error = Some(e);
                        }
                    }
                }
                continue;
            }

            if self.pending_copilot_flow {
                self.pending_copilot_flow = false;
                execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                disable_raw_mode()?;
                terminal.show_cursor()?;

                let result = crate::login::github_copilot_device_flow();

                enable_raw_mode()?;
                execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                terminal.hide_cursor()?;
                terminal.clear()?;

                match result {
                    Ok(()) => {
                        self.onboard = None;
                        self.display.push(DisplayItem::SystemNote(
                            "GitHub Copilot login complete! Credentials saved.".to_string(),
                        ));
                    }
                    Err(e) => {
                        if let Some(ob) = self.onboard.as_mut() {
                            ob.error = Some(e);
                        }
                    }
                }
                continue;
            }

            if event::poll(Duration::from_millis(80))? {
                if let Event::Key(key) = event::read()? {
                    if self.handle_key(key) {
                        break;
                    }
                }
            }

            self.drain_api_events();
        }
        Ok(())
    }

    fn drain_api_events(&mut self) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                ApiEvent::TextChunk(text) => {
                    self.streaming_buf.push_str(&text);
                }
                ApiEvent::ToolStart { name, label } => {
                    self.display.push(DisplayItem::ToolCall {
                        name,
                        label,
                        state: ToolState::Running,
                    });
                }
                ApiEvent::ToolResult { name, output, is_error } => {
                    // Update the Running entry for this tool.
                    if let Some(entry) = self.display.iter_mut().rev().find(|item| {
                        matches!(item, DisplayItem::ToolCall { name: n, state: ToolState::Running, .. } if *n == name)
                    }) {
                        if let DisplayItem::ToolCall { state, .. } = entry {
                            *state = if is_error {
                                ToolState::Error(output)
                            } else {
                                ToolState::Done(output)
                            };
                        }
                    }
                }
                ApiEvent::Done { input_tokens, output_tokens } => {
                    let text = std::mem::take(&mut self.streaming_buf);
                    if !text.is_empty() {
                        self.display.push(DisplayItem::Message {
                            role: Role::Assistant,
                            text,
                        });
                    }
                    self.input_tokens = self.input_tokens.saturating_add(input_tokens);
                    self.output_tokens = self.output_tokens.saturating_add(output_tokens);
                    self.status = Status::Idle;
                    self.scroll_up = 0;
                }
                ApiEvent::Err(msg) => {
                    self.streaming_buf.clear();
                    self.status = Status::Error(msg);
                }
            }
        }
    }

    /// Returns `true` when the app should quit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Ignore key-release events (only sent by enhanced terminals).
        if key.kind == KeyEventKind::Release {
            return false;
        }

        // Model picker is active — delegate all keys to it.
        if self.model_picker.is_some() {
            return self.handle_model_picker_key(key);
        }

        // Onboard wizard is active — delegate all keys to it.
        if self.onboard.is_some() {
            return self.handle_onboard_key(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt  = key.modifiers.contains(KeyModifiers::ALT);

        if key.code == KeyCode::Char('c') && ctrl {
            return true;
        }

        // Open model picker:
        //   Ctrl+P  — works on all terminals (0x10, no clash)
        //   Ctrl+M  — works on Kitty-protocol terminals (disambiguated from Enter)
        let open_picker = (key.code == KeyCode::Char('p') && ctrl)
            || (key.code == KeyCode::Char('m') && key.modifiers == KeyModifiers::CONTROL);
        if open_picker {
            self.model_picker = Some(ModelPicker::new(&self.model));
            return false;
        }

        // ── Ctrl editing shortcuts ──────────────────────────────────────────
        if ctrl {
            match key.code {
                // Ctrl+A — beginning of line
                KeyCode::Char('a') => {
                    let line_start = self.input[..self.cursor]
                        .rfind('\n')
                        .map_or(0, |p| p + 1);
                    self.cursor = line_start;
                    return false;
                }
                // Ctrl+E — end of line
                KeyCode::Char('e') => {
                    let line_end = self.input[self.cursor..]
                        .find('\n')
                        .map_or(self.input.len(), |p| self.cursor + p);
                    self.cursor = line_end;
                    return false;
                }
                // Ctrl+U — clear entire input
                KeyCode::Char('u') => {
                    self.input.clear();
                    self.cursor = 0;
                    return false;
                }
                // Ctrl+K — delete to end of line
                KeyCode::Char('k') => {
                    let end = self.input[self.cursor..]
                        .find('\n')
                        .map_or(self.input.len(), |p| self.cursor + p);
                    self.input.drain(self.cursor..end);
                    return false;
                }
                // Ctrl+W — delete word backward
                KeyCode::Char('w') => {
                    let new_cursor = word_start_before(&self.input, self.cursor);
                    self.input.drain(new_cursor..self.cursor);
                    self.cursor = new_cursor;
                    return false;
                }
                // Ctrl+Left — word backward
                KeyCode::Left => {
                    self.cursor = word_start_before(&self.input, self.cursor);
                    return false;
                }
                // Ctrl+Right — word forward
                KeyCode::Right => {
                    self.cursor = word_end_after(&self.input, self.cursor);
                    return false;
                }
                _ => {}
            }
        }

        // ── Alt editing shortcuts ───────────────────────────────────────────
        if alt {
            match key.code {
                // Alt+Left — word backward
                KeyCode::Left => {
                    self.cursor = word_start_before(&self.input, self.cursor);
                    return false;
                }
                // Alt+Right — word forward
                KeyCode::Right => {
                    self.cursor = word_end_after(&self.input, self.cursor);
                    return false;
                }
                // Alt+Backspace — delete word backward
                KeyCode::Backspace => {
                    let new_cursor = word_start_before(&self.input, self.cursor);
                    self.input.drain(new_cursor..self.cursor);
                    self.cursor = new_cursor;
                    return false;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Enter => {
                // Shift+Enter (Kitty protocol) or Alt+Enter (works everywhere via ESC+CR)
                // → insert newline.
                let want_newline = key.modifiers.contains(KeyModifiers::SHIFT) || alt;
                if want_newline {
                    self.input.insert(self.cursor, '\n');
                    self.cursor += 1;
                    return false;
                }
                if matches!(self.status, Status::Streaming) {
                    return false;
                }
                let input = self.input.trim().to_string();
                if input.is_empty() {
                    return false;
                }
                if self.dispatch_slash(&input) {
                    self.input.clear();
                    self.cursor = 0;
                } else {
                    self.submit(&input);
                }
            }

            KeyCode::Char(c) => {
                self.input.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }

            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = prev_char_boundary(&self.input, self.cursor);
                    self.input.drain(prev..self.cursor);
                    self.cursor = prev;
                }
            }

            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    let next = next_char_boundary(&self.input, self.cursor);
                    self.input.drain(self.cursor..next);
                }
            }

            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor = prev_char_boundary(&self.input, self.cursor);
                }
            }

            KeyCode::Right => {
                if self.cursor < self.input.len() {
                    self.cursor = next_char_boundary(&self.input, self.cursor);
                }
            }

            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),

            // Scroll message pane.
            KeyCode::Up => self.scroll_up = self.scroll_up.saturating_add(1),
            KeyCode::Down => self.scroll_up = self.scroll_up.saturating_sub(1),
            KeyCode::PageUp => self.scroll_up = self.scroll_up.saturating_add(10),
            KeyCode::PageDown => self.scroll_up = self.scroll_up.saturating_sub(10),

            _ => {}
        }
        false
    }

    fn handle_model_picker_key(&mut self, key: KeyEvent) -> bool {
        let Some(picker) = self.model_picker.as_mut() else {
            return false;
        };

        match key.code {
            KeyCode::Esc => {
                self.model_picker = None;
            }
            KeyCode::Up => {
                if picker.in_custom {
                    picker.in_custom = false;
                } else {
                    picker.cursor = picker.cursor.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if picker.in_custom {
                    // already at bottom
                } else if picker.cursor + 1 < KNOWN_MODELS.len() {
                    picker.cursor += 1;
                } else {
                    picker.in_custom = true;
                }
            }
            KeyCode::Char(c) if picker.in_custom => {
                picker.custom_input.push(c);
            }
            KeyCode::Backspace if picker.in_custom => {
                picker.custom_input.pop();
            }
            KeyCode::Enter => {
                let chosen = picker.selected_model().to_string();
                self.model_picker = None;
                if !chosen.is_empty() {
                    self.model = api::resolve_model_alias(&chosen);
                    crate::config::save_last_model(&self.model);
                    let note = format!("Switched to model: {}", self.model);
                    self.display.push(DisplayItem::SystemNote(note));
                }
            }
            _ => {}
        }
        false
    }

    fn handle_onboard_key(&mut self, key: KeyEvent) -> bool {
        let Some(ob) = self.onboard.as_mut() else { return false };

        match &ob.step.clone() {
            OnboardStep::PickProvider => match key.code {
                KeyCode::Esc => { self.onboard = None; }
                KeyCode::Up => { ob.provider_cursor = ob.provider_cursor.saturating_sub(1); }
                KeyCode::Down => {
                    if ob.provider_cursor + 1 < ONBOARD_PROVIDERS.len() {
                        ob.provider_cursor += 1;
                    }
                }
                KeyCode::Enter => {
                    let idx = ob.provider_cursor;
                    if idx == 0 {
                        // Anthropic offers both API key and OAuth — show method picker.
                        ob.step = OnboardStep::ChooseAuthMethod { provider: idx };
                    } else if idx == 3 {
                        // Copilot always uses the device flow — skip straight to it.
                        self.pending_copilot_flow = true;
                    } else {
                        ob.step = OnboardStep::EnterKey { provider: idx };
                    }
                    ob.key_input.clear();
                    ob.error = None;
                }
                _ => {}
            },

            OnboardStep::ChooseAuthMethod { provider } => {
                let provider = *provider;
                match key.code {
                    KeyCode::Esc => { ob.step = OnboardStep::PickProvider; ob.error = None; }
                    // 1 = paste key (Anthropic only) / device flow (Copilot goes straight here)
                    KeyCode::Char('1') => {
                        if provider == 3 {
                            self.pending_copilot_flow = true;
                        } else {
                            ob.step = OnboardStep::EnterKey { provider };
                            ob.key_input.clear();
                            ob.error = None;
                        }
                    }
                    // 2 = OAuth browser flow (Anthropic only)
                    KeyCode::Char('2') => {
                        if provider != 3 {
                            self.pending_oauth = true;
                        }
                    }
                    _ => {}
                }
            }

            OnboardStep::EnterKey { provider } => {
                let provider = *provider;
                match key.code {
                    KeyCode::Esc => {
                        let ob = self.onboard.as_mut().unwrap();
                        if provider == 0 {
                            ob.step = OnboardStep::ChooseAuthMethod { provider };
                        } else {
                            ob.step = OnboardStep::PickProvider;
                        }
                        ob.error = None;
                    }
                    KeyCode::Backspace => { ob.key_input.pop(); }
                    KeyCode::Char(c) => { ob.key_input.push(c); }
                    KeyCode::Enter => {
                        let key_val = ob.key_input.trim().to_string();
                        if key_val.is_empty() {
                            ob.error = Some("Key cannot be empty.".to_string());
                            return false;
                        }
                        let (prov_name, _, env_var) = ONBOARD_PROVIDERS[provider];
                        let profile_id = format!("{}:default", prov_name.to_lowercase());
                        let cred = serde_json::json!({
                            "type": "api_key",
                            "provider": prov_name.to_lowercase(),
                            "key": key_val,
                        });
                        match save_onboard_profile(&profile_id, cred) {
                            Ok(()) => {
                                std::env::set_var(env_var, &key_val);
                                let ob = self.onboard.as_mut().unwrap();
                                // Offer Telegram setup after any AI provider key is saved.
                                ob.step = OnboardStep::TelegramOffer;
                                ob.key_input.clear();
                                ob.error = None;
                            }
                            Err(e) => { ob.error = Some(format!("Save failed: {e}")); }
                        }
                    }
                    _ => {}
                }
            }

            OnboardStep::TelegramOffer => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let ob = self.onboard.as_mut().unwrap();
                    ob.step = OnboardStep::TelegramToken;
                    ob.key_input.clear();
                    ob.error = None;
                }
                _ => {
                    // Any other key (N, Esc, Enter…) skips Telegram setup.
                    self.onboard.as_mut().unwrap().step = OnboardStep::Done;
                }
            },

            OnboardStep::TelegramToken => {
                match key.code {
                    KeyCode::Esc => {
                        ob.step = OnboardStep::TelegramOffer;
                        ob.error = None;
                    }
                    KeyCode::Backspace => { ob.key_input.pop(); }
                    KeyCode::Char(c) => { ob.key_input.push(c); }
                    KeyCode::Enter => {
                        let token_val = ob.key_input.trim().to_string();
                        if token_val.is_empty() {
                            ob.error = Some("Token cannot be empty.".to_string());
                            return false;
                        }
                        let cred = serde_json::json!({
                            "type": "bot_token",
                            "provider": "telegram",
                            "token": token_val,
                        });
                        match save_onboard_profile("telegram:default", cred) {
                            Ok(()) => {
                                std::env::set_var("TELEGRAM_BOT_TOKEN", &token_val);
                                let ob = self.onboard.as_mut().unwrap();
                                ob.step = OnboardStep::Done;
                                ob.error = None;
                            }
                            Err(e) => { ob.error = Some(format!("Save failed: {e}")); }
                        }
                    }
                    _ => {}
                }
            }

            OnboardStep::Done => {
                self.onboard = None;
                self.display.push(DisplayItem::SystemNote(
                    "Setup complete! Your credentials have been saved.".to_string(),
                ));
            }
        }
        false
    }

    fn dispatch_slash(&mut self, input: &str) -> bool {
        let (cmd, rest) = input
            .split_once(char::is_whitespace)
            .map_or((input, ""), |(c, r)| (c, r.trim()));

        match cmd {
            "/help" => {
                self.display.push(DisplayItem::SystemNote(
                    "Keyboard shortcuts:\n\
                     \n\
                     Enter              send message\n\
                     Shift+Enter        insert newline\n\
                     Ctrl+C             quit\n\
                     Ctrl+P             open model picker\n\
                     Ctrl+A / Ctrl+E    start / end of line\n\
                     Ctrl+U             clear input\n\
                     Ctrl+K             delete to end of line\n\
                     Ctrl+W             delete word backward\n\
                     Alt+← / Alt+→      word navigation\n\
                     ↑ / ↓              scroll chat\n\
                     PgUp / PgDn        scroll 10 lines\n\
                     \n\
                     Slash commands:\n\
                     /help              this message\n\
                     /model [id]        show or switch model\n\
                     /clear             wipe chat history\n\
                     /compact           keep only the last two turns\n\
                     \n\
                     Run `openclaw-code setup` to configure credentials."
                        .to_string(),
                ));
                true
            }
            "/model" => {
                if rest.is_empty() {
                    let msg = format!("Current model: {}", self.model);
                    self.display.push(DisplayItem::SystemNote(msg));
                } else {
                    self.model = api::resolve_model_alias(rest);
                    crate::config::save_last_model(&self.model);
                    let msg = format!("Switched to model: {}", self.model);
                    self.display.push(DisplayItem::SystemNote(msg));
                }
                true
            }
            "/clear" => {
                self.display.clear();
                self.api_history.clear();
                self.streaming_buf.clear();
                self.status = Status::Idle;
                self.scroll_up = 0;
                true
            }
            "/compact" => {
                let keep = 4; // last two turns (user+assistant each)
                if self.api_history.len() > keep {
                    self.api_history.drain(..self.api_history.len() - keep);
                }
                if self.display.len() > keep {
                    self.display.drain(..self.display.len() - keep);
                }
                self.display
                    .push(DisplayItem::SystemNote("History compacted to last two turns.".to_string()));
                true
            }
            other if other.starts_with('/') => {
                let msg = format!("Unknown command: {other}  (try /help)");
                self.display.push(DisplayItem::SystemNote(msg));
                true
            }
            _ => false,
        }
    }

    fn submit(&mut self, input: &str) {
        self.display.push(DisplayItem::Message {
            role: Role::User,
            text: input.to_string(),
        });
        self.api_history.push(api::InputMessage::user_text(input.to_string()));
        self.input.clear();
        self.cursor = 0;
        self.scroll_up = 0;
        self.status = Status::Streaming;

        let tx = self.event_tx.clone();
        let model = self.model.clone();
        let history = self.api_history.clone();

        thread::spawn(move || {
            // Reload credentials fresh from disk on every API call so changes
            // from `openclaw-code login` take effect without restarting the TUI.
            let creds = crate::auth::load_openclaw_credentials();
            let anthropic_api_key = creds.as_ref().and_then(|c| c.anthropic_api_key.clone());
            let anthropic_token = creds.as_ref().and_then(|c| c.anthropic_token.clone());
            let openai_key = creds.as_ref().and_then(|c| c.openai_key.clone());
            let xai_key = creds.as_ref().and_then(|c| c.xai_key.clone());
            let github_copilot_token =
                creds.as_ref().and_then(|c| c.github_copilot_token.clone());
            let google_key = creds.as_ref().and_then(|c| c.google_key.clone());
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(call_api(
                tx,
                model,
                history,
                anthropic_api_key,
                anthropic_token,
                openai_key,
                xai_key,
                github_copilot_token,
                google_key,
            ));
        });
    }

}

// ── API call loop (background thread) ─────────────────────────────────────

/// Build a `ProviderClient` for `model`, preferring OpenClaw-loaded credentials
/// over env-var lookup only when the relevant env var is absent.
fn make_client(
    model: &str,
    anthropic_api_key: Option<String>,
    anthropic_token: Option<String>,
    openai_key: Option<String>,
    xai_key: Option<String>,
    github_copilot_token: Option<String>,
    google_key: Option<String>,
) -> Result<api::ProviderClient, api::ApiError> {
    use api::{
        AuthSource, ClawApiClient, GithubCopilotClient, OpenAiCompatClient, OpenAiCompatConfig,
        ProviderClient, ProviderKind,
    };
    use std::env;

    match api::detect_provider_kind(model) {
        ProviderKind::ClawApi => {
            // Prefer env vars; fall back to OpenClaw store.
            let auth = if env::var("ANTHROPIC_API_KEY").is_ok()
                || env::var("ANTHROPIC_AUTH_TOKEN").is_ok()
            {
                AuthSource::from_env()?
            } else if let Some(key) = anthropic_api_key {
                if let Some(token) = anthropic_token {
                    // OAuth login: both x-api-key (workspace key) + Bearer token required.
                    AuthSource::ApiKeyAndBearer { api_key: key, bearer_token: token }
                } else {
                    AuthSource::ApiKey(key)
                }
            } else if let Some(token) = anthropic_token {
                AuthSource::BearerToken(token)
            } else {
                // Fall back to saved OAuth (Claude Code / system login).
                AuthSource::from_env_or_saved()?
            };
            Ok(ProviderClient::ClawApi(ClawApiClient::from_auth(auth)))
        }

        ProviderKind::OpenAi => {
            if env::var("OPENAI_API_KEY").is_ok() {
                Ok(ProviderClient::OpenAi(OpenAiCompatClient::from_env(
                    OpenAiCompatConfig::openai(),
                )?))
            } else if let Some(key) = openai_key {
                Ok(ProviderClient::OpenAi(OpenAiCompatClient::new(
                    key,
                    OpenAiCompatConfig::openai(),
                )))
            } else {
                Err(api::ApiError::missing_credentials(
                    "OpenAI",
                    &["OPENAI_API_KEY"],
                ))
            }
        }

        ProviderKind::Xai => {
            if env::var("XAI_API_KEY").is_ok() {
                Ok(ProviderClient::Xai(OpenAiCompatClient::from_env(
                    OpenAiCompatConfig::xai(),
                )?))
            } else if let Some(key) = xai_key {
                Ok(ProviderClient::Xai(OpenAiCompatClient::new(
                    key,
                    OpenAiCompatConfig::xai(),
                )))
            } else {
                Err(api::ApiError::missing_credentials(
                    "xAI",
                    &["XAI_API_KEY"],
                ))
            }
        }

        ProviderKind::GithubCopilot => {
            if let Ok(token) = env::var("GITHUB_COPILOT_TOKEN") {
                if !token.is_empty() {
                    return Ok(ProviderClient::GithubCopilot(GithubCopilotClient::new(token)));
                }
            }
            if let Some(token) = github_copilot_token {
                Ok(ProviderClient::GithubCopilot(GithubCopilotClient::new(token)))
            } else {
                Err(api::ApiError::missing_credentials(
                    "GitHub Copilot",
                    &["GITHUB_COPILOT_TOKEN"],
                ))
            }
        }

        ProviderKind::Google => {
            if env::var("GOOGLE_API_KEY").is_ok() {
                Ok(ProviderClient::Google(OpenAiCompatClient::from_env(
                    OpenAiCompatConfig::google(),
                )?))
            } else if let Some(key) = google_key {
                Ok(ProviderClient::Google(OpenAiCompatClient::new(
                    key,
                    OpenAiCompatConfig::google(),
                )))
            } else {
                Err(api::ApiError::missing_credentials("Google", &["GOOGLE_API_KEY"]))
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn call_api(
    tx: mpsc::Sender<ApiEvent>,
    model: String,
    mut history: Vec<api::InputMessage>,
    anthropic_api_key: Option<String>,
    anthropic_token: Option<String>,
    openai_key: Option<String>,
    xai_key: Option<String>,
    github_copilot_token: Option<String>,
    google_key: Option<String>,
) {
    use api::{
        ContentBlockDelta, InputContentBlock, MessageRequest, OutputContentBlock, ProviderKind,
        StreamEvent, ToolResultContentBlock,
    };
    use tools::GlobalToolRegistry;

    let registry = GlobalToolRegistry::builtin();
    let tool_defs = registry.definitions(None);

    // Build the provider client, preferring OpenClaw-loaded credentials over env
    // vars only when the latter are absent.
    let client = match make_client(
        &model,
        anthropic_api_key,
        anthropic_token,
        openai_key,
        xai_key,
        github_copilot_token,
        google_key,
    ) {
        Ok(c) => c,
        Err(e) => {
            let hint = match api::detect_provider_kind(&model) {
                ProviderKind::ClawApi =>
                    " — set ANTHROPIC_API_KEY or log in via Claude Code / OpenClaw".to_string(),
                ProviderKind::OpenAi =>
                    " — set OPENAI_API_KEY".to_string(),
                ProviderKind::Xai =>
                    " — set XAI_API_KEY".to_string(),
                ProviderKind::GithubCopilot =>
                    " — run `openclaw-code login` and select GitHub Copilot".to_string(),
                ProviderKind::Google =>
                    " — set GOOGLE_API_KEY".to_string(),
            };
            let _ = tx.send(ApiEvent::Err(format!("{e}{hint}")));
            return;
        }
    };

    let mut total_input = 0u32;
    let mut total_output = 0u32;

    // Agentic loop: keep going while there are tool calls.
    loop {
        let max_tokens = api::max_tokens_for_model(&model);
        let request = MessageRequest {
            model: model.clone(),
            max_tokens,
            messages: history.clone(),
            system: None,
            tools: Some(tool_defs.clone()),
            tool_choice: None,
            stream: true,
        };

        let mut stream = match client.stream_message(&request).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(ApiEvent::Err(e.to_string()));
                return;
            }
        };

        // Accumulate one full assistant turn.
        let mut text_buf = String::new();
        let mut tool_calls: Vec<PendingTool> = Vec::new();
        let mut current_tool: Option<PendingTool> = None;

        loop {
            match stream.next_event().await {
                Ok(None) => break,
                Ok(Some(event)) => match event {
                    StreamEvent::ContentBlockStart(ev) => {
                        if let OutputContentBlock::ToolUse { id, name, .. } = ev.content_block {
                            current_tool = Some(PendingTool {
                                id,
                                name,
                                input_json: String::new(),
                            });
                        }
                    }
                    StreamEvent::ContentBlockDelta(ev) => match ev.delta {
                        ContentBlockDelta::TextDelta { text } => {
                            text_buf.push_str(&text);
                            let _ = tx.send(ApiEvent::TextChunk(text));
                        }
                        ContentBlockDelta::InputJsonDelta { partial_json } => {
                            if let Some(t) = current_tool.as_mut() {
                                t.input_json.push_str(&partial_json);
                            }
                        }
                        _ => {}
                    },
                    StreamEvent::ContentBlockStop(_) => {
                        if let Some(tool) = current_tool.take() {
                            tool_calls.push(tool);
                        }
                    }
                    StreamEvent::MessageStart(ev) => {
                        total_input =
                            total_input.saturating_add(ev.message.usage.input_tokens);
                    }
                    StreamEvent::MessageDelta(ev) => {
                        total_output = ev.usage.output_tokens;
                    }
                    StreamEvent::MessageStop(_) => break,
                },
                Err(e) => {
                    let _ = tx.send(ApiEvent::Err(e.to_string()));
                    return;
                }
            }
        }

        if tool_calls.is_empty() {
            // No tools — we are done.
            break;
        }

        // Build the assistant content block (text + tool_use blocks).
        let mut assistant_content = Vec::new();
        if !text_buf.is_empty() {
            assistant_content.push(InputContentBlock::Text { text: text_buf.clone() });
        }
        for tc in &tool_calls {
            let input_val: Value = serde_json::from_str(&tc.input_json).unwrap_or(Value::Null);
            assistant_content.push(InputContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: input_val,
            });
        }
        history.push(api::InputMessage {
            role: "assistant".to_string(),
            content: assistant_content,
        });

        // Execute tools and collect results.
        let mut result_blocks = Vec::new();
        for tc in &tool_calls {
            let input_val: Value =
                serde_json::from_str(&tc.input_json).unwrap_or(Value::Null);
            let label = tool_label(&tc.name, &input_val);
            let _ = tx.send(ApiEvent::ToolStart { name: tc.name.clone(), label });
            let (output, is_error) = match registry.execute(&tc.name, &input_val) {
                Ok(out) => (out, false),
                Err(e) => (e, true),
            };
            let _ = tx.send(ApiEvent::ToolResult {
                name: tc.name.clone(),
                output: output.clone(),
                is_error,
            });
            result_blocks.push(InputContentBlock::ToolResult {
                tool_use_id: tc.id.clone(),
                content: vec![ToolResultContentBlock::Text { text: output }],
                is_error,
            });
        }

        // Add tool results and loop.
        history.push(api::InputMessage {
            role: "user".to_string(),
            content: result_blocks,
        });

        // Reset text buffer before next iteration.
        text_buf.clear();
    }

    let _ = tx.send(ApiEvent::Done {
        input_tokens: total_input,
        output_tokens: total_output,
    });
}

/// Save a credential profile to `~/.config/openclaw-code/auth.json`.
fn save_onboard_profile(profile_id: &str, cred: serde_json::Value) -> Result<(), String> {
    use serde_json::{Map, Value};
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config"))
        })
        .ok_or("HOME not set")?;
    let path = base.join("openclaw-code/auth.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut store: Map<String, Value> = if path.exists() {
        std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    } else {
        Map::new()
    };
    store.entry("version").or_insert(Value::Number(1.into()));
    let profiles = store
        .entry("profiles")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(ref mut map) = profiles {
        map.insert(profile_id.to_string(), cred);
    }
    let rendered = serde_json::to_string_pretty(&Value::Object(store)).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{rendered}\n")).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Move cursor back to the start of the previous word.
fn word_start_before(s: &str, cursor: usize) -> usize {
    let before = &s[..cursor];
    let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric());
    let word_start = trimmed.rfind(|c: char| !c.is_alphanumeric())
        .map_or(0, |p| p + 1);
    word_start
}

/// Move cursor forward to the end of the next word.
fn word_end_after(s: &str, cursor: usize) -> usize {
    let after = &s[cursor..];
    let skipped = after.trim_start_matches(|c: char| !c.is_alphanumeric());
    let skip_len = after.len() - skipped.len();
    let word_end = skipped.find(|c: char| !c.is_alphanumeric())
        .map_or(s.len(), |p| cursor + skip_len + p);
    word_end
}

/// Build a short human-readable label for a tool call.
fn tool_label(name: &str, input: &Value) -> String {
    match name {
        "bash" => input
            .get("command")
            .and_then(Value::as_str)
            .map(|c| {
                // Truncate very long commands.
                if c.len() > 80 { format!("{}…", &c[..80]) } else { c.to_string() }
            })
            .unwrap_or_default(),
        "read_file" | "write_file" | "edit_file" => input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

struct PendingTool {
    id: String,
    name: String,
    input_json: String,
}

// ── Cursor helpers ─────────────────────────────────────────────────────────

fn prev_char_boundary(s: &str, from: usize) -> usize {
    s[..from].char_indices().last().map_or(0, |(i, _)| i)
}

fn next_char_boundary(s: &str, from: usize) -> usize {
    s[from..]
        .char_indices()
        .nth(1)
        .map_or(s.len(), |(i, _)| from + i)
}
