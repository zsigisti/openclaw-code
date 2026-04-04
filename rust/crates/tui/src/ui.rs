use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;
use runtime::{format_usd, pricing_for_model, TokenUsage};

use crate::app::{
    App, DisplayItem, OnboardPopup, OnboardStep, Role, Status, ToolState, KNOWN_MODELS,
    ONBOARD_PROVIDERS,
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// ── Palette ────────────────────────────────────────────────────────────────
const C_ACCENT: Color    = Color::Rgb(0, 210, 190);   // teal
const C_USER:   Color    = Color::Rgb(100, 160, 255);  // blue
const C_ASST:   Color    = Color::Rgb(180, 220, 140);  // soft green
const C_TOOL:   Color    = Color::Rgb(195, 155, 255);  // lavender
const C_DIM:    Color    = Color::Rgb(100, 100, 110);  // muted
const C_GREEN:  Color    = Color::Rgb(100, 220, 120);
const C_RED:    Color    = Color::Rgb(255, 100, 100);
const C_YELLOW: Color    = Color::Rgb(255, 210, 80);
const C_BORDER: Color    = Color::Rgb(55, 55, 65);

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    let input_height = u16::try_from(input_lines(app).max(1)).unwrap_or(10) + 2;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(f, app, chunks[0]);
    render_messages(f, app, chunks[1]);
    render_input(f, app, chunks[2]);
    render_statusbar(f, app, chunks[3]);

    if app.model_picker.is_some() {
        render_model_picker(f, app, area);
    }
    if app.onboard.is_some() {
        render_onboard(f, app, area);
    }
}

// ── Header ─────────────────────────────────────────────────────────────────

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let title = " ◈ openclaw-code ";

    let usage = TokenUsage {
        input_tokens: app.input_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        output_tokens: app.output_tokens,
    };
    let cost_text = if let Some(pricing) = pricing_for_model(&app.model) {
        let est = usage.estimate_cost_usd_with_pricing(pricing);
        format!(" {} ", format_usd(est.total_cost_usd()))
    } else {
        format!(" ↑{} ↓{} ", app.input_tokens, app.output_tokens)
    };
    let model_text = format!(" {} ", app.model);

    let used = title.chars().count() + model_text.chars().count() + cost_text.chars().count();
    let fill = "─".repeat((area.width as usize).saturating_sub(used));

    let line = Line::from(vec![
        Span::styled(title,      Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(fill,       Style::default().fg(C_BORDER)),
        Span::styled(model_text, Style::default().fg(C_ASST)),
        Span::styled(cost_text,  Style::default().fg(C_YELLOW)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

// ── Messages ───────────────────────────────────────────────────────────────

fn render_messages(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line<'_>> = Vec::new();

    // Empty state.
    if app.display.is_empty() && !matches!(app.status, Status::Streaming | Status::Error(_)) {
        let welcome = vec![
            Line::default(),
            Line::from(Span::styled(
                "  ◈  openclaw-code",
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            )),
            Line::default(),
            Line::from(Span::styled(
                "  Type a message and press Enter to start.",
                Style::default().fg(C_DIM),
            )),
            Line::from(Span::styled(
                "  Run /onboard to configure an API key.",
                Style::default().fg(C_DIM),
            )),
            Line::from(Span::styled(
                "  Run /help for keyboard shortcuts.",
                Style::default().fg(C_DIM),
            )),
        ];
        let block = styled_block(" chat ", &app.status);
        f.render_widget(
            Paragraph::new(Text::from(welcome)).block(block),
            area,
        );
        return;
    }

    for item in &app.display {
        match item {
            DisplayItem::Message { role, text } => push_message_lines(&mut lines, *role, text),
            DisplayItem::ToolCall { name, label, state } => push_tool_lines(&mut lines, name, label, state),
            DisplayItem::SystemNote(text) => push_note_lines(&mut lines, text),
        }
        lines.push(Line::default());
    }

    // Live streaming.
    match &app.status {
        Status::Streaming => {
            let frame = SPINNER[(app.tick / 2) as usize % SPINNER.len()];
            if app.streaming_buf.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("OC    ", Style::default().fg(C_ASST).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{frame} thinking…"), Style::default().fg(C_DIM)),
                ]));
            } else {
                push_message_lines(&mut lines, Role::Assistant, &app.streaming_buf);
                lines.push(Line::from(Span::styled(
                    format!("      {frame}"),
                    Style::default().fg(C_DIM),
                )));
            }
            lines.push(Line::default());
        }
        Status::Error(msg) => {
            lines.push(Line::from(vec![
                Span::styled("  ✘ ", Style::default().fg(C_RED).add_modifier(Modifier::BOLD)),
                Span::styled(msg.clone(), Style::default().fg(C_RED)),
            ]));
            lines.push(Line::default());
        }
        Status::Idle => {}
    }

    let inner_h = area.height.saturating_sub(2) as usize;
    let total = lines.len();
    let bottom = total.saturating_sub(inner_h);
    let scroll = u16::try_from(bottom.saturating_sub(app.scroll_up)).unwrap_or(u16::MAX);

    let block = styled_block(" chat ", &app.status);
    f.render_widget(
        Paragraph::new(Text::from(lines.clone()))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );

    if total > inner_h {
        let mut sb = ScrollbarState::new(total.saturating_sub(inner_h))
            .position(bottom.saturating_sub(app.scroll_up));
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"))
                .style(Style::default().fg(C_DIM)),
            area.inner(Margin { horizontal: 0, vertical: 1 }),
            &mut sb,
        );
    }
}

fn styled_block<'a>(title: &'a str, status: &Status) -> Block<'a> {
    let border_color = match status {
        Status::Error(_)   => C_RED,
        Status::Streaming  => C_ACCENT,
        Status::Idle       => C_BORDER,
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(border_color).add_modifier(Modifier::DIM),
        ))
}

fn push_message_lines(lines: &mut Vec<Line<'_>>, role: Role, text: &str) {
    let (label, label_style, content_style) = match role {
        Role::User => (
            "You   ",
            Style::default().fg(C_USER).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::White),
        ),
        Role::Assistant => (
            "OC    ",
            Style::default().fg(C_ASST).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Rgb(210, 215, 210)),
        ),
    };

    let mut in_code_block = false;
    for (i, line_text) in text.lines().enumerate() {
        // Toggle code-block state.
        if line_text.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
        }

        let prefix = if i == 0 {
            Span::styled(label, label_style)
        } else {
            Span::raw("      ")
        };

        let content = if in_code_block || line_text.trim_start().starts_with("```") {
            Span::styled(line_text.to_string(), Style::default().fg(Color::Rgb(255, 215, 135)))
        } else {
            Span::styled(line_text.to_string(), content_style)
        };

        lines.push(Line::from(vec![prefix, content]));
    }
}

fn push_tool_lines(lines: &mut Vec<Line<'_>>, name: &str, label: &str, state: &ToolState) {
    let is_diff = matches!(name, "edit_file" | "write_file");
    let is_bash = name == "bash";

    let (icon, header_text) = match name {
        "bash"                    => ("⬢ ", if label.is_empty() { "Bash".into() } else { format!("Bash({label})") }),
        "edit_file"               => ("✎ ", if label.is_empty() { "edit_file".into() } else { format!("Edit({label})") }),
        "write_file"              => ("✎ ", if label.is_empty() { "write_file".into() } else { format!("Write({label})") }),
        "read_file"               => ("◎ ", if label.is_empty() { "read_file".into() } else { format!("Read({label})") }),
        "glob_search"             => ("⌕ ", "Glob".to_string()),
        "grep_search"             => ("⌕ ", "Grep".to_string()),
        _                         => ("◆ ", format!("{name}")),
    };

    match state {
        ToolState::Running => {
            let frame = "⠙";
            lines.push(Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled(frame, Style::default().fg(C_YELLOW)),
                Span::raw(" "),
                Span::styled(icon, Style::default().fg(C_TOOL)),
                Span::styled(header_text, Style::default().fg(C_TOOL).add_modifier(Modifier::BOLD)),
            ]));
        }

        ToolState::Done(out) if is_diff && is_diff_output(out) => {
            push_diff_lines(lines, out, icon);
        }

        ToolState::Done(out) if is_bash && out.starts_with("Bash(") => {
            push_bash_lines(lines, out);
        }

        ToolState::Done(out) => {
            let preview = first_line_preview(out, 55);
            let mut spans = vec![
                Span::styled("      ", Style::default()),
                Span::styled("● ", Style::default().fg(C_GREEN)),
                Span::styled(icon, Style::default().fg(C_TOOL)),
                Span::styled(header_text, Style::default().fg(C_TOOL).add_modifier(Modifier::BOLD)),
            ];
            if !preview.is_empty() {
                spans.push(Span::styled("  ", Style::default()));
                spans.push(Span::styled(preview, Style::default().fg(C_DIM)));
            }
            lines.push(Line::from(spans));
        }

        ToolState::Error(msg) => {
            let preview = first_line_preview(msg, 55);
            let mut spans = vec![
                Span::styled("      ", Style::default()),
                Span::styled("✘ ", Style::default().fg(C_RED)),
                Span::styled(icon, Style::default().fg(C_TOOL)),
                Span::styled(header_text, Style::default().fg(C_TOOL).add_modifier(Modifier::BOLD)),
            ];
            if !preview.is_empty() {
                spans.push(Span::styled("  ", Style::default()));
                spans.push(Span::styled(preview, Style::default().fg(C_RED)));
            }
            lines.push(Line::from(spans));
        }
    }
}

fn push_bash_lines(lines: &mut Vec<Line<'_>>, out: &str) {
    for (i, raw) in out.lines().enumerate() {
        let line = if i == 0 {
            Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled("● ", Style::default().fg(C_GREEN)),
                Span::styled("⬢ ", Style::default().fg(C_TOOL)),
                Span::styled(raw.to_string(), Style::default().fg(C_TOOL).add_modifier(Modifier::BOLD)),
            ])
        } else if raw.starts_with('\u{23bf}') {
            Line::from(vec![
                Span::styled("        ", Style::default()),
                Span::styled(raw.to_string(), Style::default().fg(C_DIM)),
            ])
        } else {
            Line::from(vec![
                Span::styled("        ", Style::default()),
                Span::styled(raw.to_string(), Style::default().fg(C_DIM)),
            ])
        };
        lines.push(line);
    }
}

fn is_diff_output(out: &str) -> bool {
    out.starts_with("Update(") || out.starts_with("Create(")
}

fn push_diff_lines<'a>(lines: &mut Vec<Line<'a>>, out: &str, icon: &'a str) {
    let mut iter = out.lines().enumerate();
    for (i, raw) in &mut iter {
        let line = if i == 0 {
            Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled("● ", Style::default().fg(C_GREEN)),
                Span::styled(icon, Style::default().fg(C_TOOL)),
                Span::styled(raw.to_string(), Style::default().fg(C_TOOL).add_modifier(Modifier::BOLD)),
            ])
        } else if raw.starts_with('\u{23bf}') {
            Line::from(vec![
                Span::styled("        ", Style::default()),
                Span::styled(raw.to_string(), Style::default().fg(C_DIM)),
            ])
        } else if let Some(rest) = raw.strip_prefix('+') {
            Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled(format!("+{rest}"), Style::default().fg(C_GREEN)),
            ])
        } else if let Some(rest) = raw.strip_prefix('-') {
            Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled(format!("-{rest}"), Style::default().fg(C_RED)),
            ])
        } else {
            Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled(raw.to_string(), Style::default().fg(C_DIM)),
            ])
        };
        lines.push(line);
    }
}

fn push_note_lines(lines: &mut Vec<Line<'_>>, text: &str) {
    for line_text in text.lines() {
        lines.push(Line::from(vec![
            Span::styled("  ╌ ", Style::default().fg(C_DIM)),
            Span::styled(line_text.to_string(), Style::default().fg(Color::Rgb(160, 165, 175))),
        ]));
    }
}

fn first_line_preview(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    if first.len() > max { format!("{}…", &first[..max]) } else { first.to_string() }
}

// ── Input box ──────────────────────────────────────────────────────────────

pub fn input_lines(app: &App) -> usize {
    if app.input.is_empty() { 1 } else { app.input.lines().count().max(1) }
}

fn render_input(f: &mut Frame, app: &App, area: Rect) {
    let is_streaming = matches!(app.status, Status::Streaming);

    let (border_color, title_text) = match &app.status {
        Status::Error(_)  => (C_RED,    " ✘ error "),
        Status::Streaming => (C_ACCENT, " ⠙ streaming "),
        Status::Idle      => (C_BORDER, " message "),
    };

    let prompt = if is_streaming { "" } else { "› " };
    let display = format!("{prompt}{}", app.input);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(title_text, Style::default().fg(border_color)));

    f.render_widget(Paragraph::new(display).block(block), area);

    if !is_streaming {
        let before_cursor = format!("{prompt}{}", &app.input[..app.cursor]);
        let cursor_row = u16::try_from(before_cursor.lines().count().saturating_sub(1))
            .unwrap_or(u16::MAX);
        let last_cols = before_cursor.lines().last().map_or(0, |l| l.chars().count());
        let cx = (area.x + 1 + last_cols as u16).min(area.x + area.width.saturating_sub(2));
        let cy = (area.y + 1 + cursor_row).min(area.y + area.height.saturating_sub(2));
        f.set_cursor_position((cx, cy));
    }
}

// ── Status bar ─────────────────────────────────────────────────────────────

fn render_statusbar(f: &mut Frame, app: &App, area: Rect) {
    let extra = if matches!(app.status, Status::Streaming) { "  ⠙ streaming" } else { "" };
    let bar = format!(
        " Enter:send  Alt+Enter:↵  Ctrl+P:model  Ctrl+W:word  Ctrl+U:clear  /help  `setup` to configure{extra} "
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(bar, Style::default().fg(C_DIM)))),
        area,
    );
}

// ── Model picker popup ─────────────────────────────────────────────────────

fn render_model_picker(f: &mut Frame, app: &App, area: Rect) {
    let Some(picker) = &app.model_picker else { return };

    let popup_w = 56u16;
    let popup_h = u16::try_from(KNOWN_MODELS.len()).unwrap_or(10) + 6;
    let x = area.width.saturating_sub(popup_w) / 2;
    let y = area.height.saturating_sub(popup_h) / 2;
    let popup = Rect::new(x, y, popup_w.min(area.width), popup_h.min(area.height));

    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_ACCENT))
        .title(Span::styled(
            " ◈ Select Model  ↑↓ navigate  Enter confirm  Esc cancel ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(u16::try_from(KNOWN_MODELS.len()).unwrap_or(10)),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let items: Vec<ListItem<'_>> = KNOWN_MODELS
        .iter()
        .map(|m| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("  {:<34}", m.label), Style::default().fg(Color::White)),
                Span::styled(m.provider, Style::default().fg(C_DIM)),
            ]))
        })
        .collect();

    let hl = if picker.in_custom {
        Style::default()
    } else {
        Style::default().bg(Color::Rgb(40, 45, 55)).fg(C_ACCENT).add_modifier(Modifier::BOLD)
    };

    let mut list_state = ListState::default();
    list_state.select(Some(picker.cursor));
    f.render_stateful_widget(List::new(items).highlight_style(hl), split[0], &mut list_state);

    f.render_widget(
        Paragraph::new(Span::styled(
            "  ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌",
            Style::default().fg(C_BORDER),
        )),
        split[1],
    );

    let custom_style = if picker.in_custom {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_DIM)
    };
    let custom_text = if picker.custom_input.is_empty() {
        "  type a custom model id…".to_string()
    } else {
        format!("  {}", picker.custom_input)
    };
    f.render_widget(Paragraph::new(Span::styled(custom_text, custom_style)), split[2]);

    if picker.in_custom {
        let cx = split[2].x + 2 + u16::try_from(picker.custom_input.len()).unwrap_or(u16::MAX);
        f.set_cursor_position((cx.min(split[2].x + split[2].width - 1), split[2].y));
    }
}

// ── Onboard wizard popup ───────────────────────────────────────────────────

fn onboard_popup_rect(area: Rect) -> Rect {
    let w = 60u16;
    let h = 16u16;
    let x = area.width.saturating_sub(w) / 2;
    let y = area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w.min(area.width), h.min(area.height))
}

fn onboard_block(title: String, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title, Style::default().fg(color).add_modifier(Modifier::BOLD)))
}

fn render_onboard(f: &mut Frame, app: &App, area: Rect) {
    let Some(ob) = &app.onboard else { return };
    let popup = onboard_popup_rect(area);
    f.render_widget(Clear, popup);

    match &ob.step {
        OnboardStep::PickProvider => render_onboard_pick(f, ob, popup),
        OnboardStep::ChooseAuthMethod { provider } => render_onboard_auth_method(f, ob, *provider, popup),
        OnboardStep::EnterKey { provider } => render_onboard_key(f, ob, *provider, popup),
        OnboardStep::Done => render_onboard_done(f, popup),
    }
}

fn render_onboard_pick(f: &mut Frame, ob: &OnboardPopup, popup: Rect) {
    let block = onboard_block(
        " ◈ Setup — Choose Provider   ↑↓ navigate   Enter select   Esc close ".to_string(),
        C_ACCENT,
    );
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines: Vec<Line<'_>> = vec![
        Line::default(),
        Line::from(Span::styled(
            "  Which AI provider would you like to configure?",
            Style::default().fg(Color::White),
        )),
        Line::default(),
    ];

    for (i, (name, desc, _)) in ONBOARD_PROVIDERS.iter().enumerate() {
        let sel = i == ob.provider_cursor;
        let arrow = if sel { "▶ " } else { "  " };
        let name_style = if sel {
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_DIM)
        };
        let desc_style = if sel {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(C_DIM)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {arrow}"), name_style),
            Span::styled(format!("{name:<13}"), name_style),
            Span::styled(*desc, desc_style),
        ]));
    }

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn render_onboard_auth_method(f: &mut Frame, ob: &OnboardPopup, provider: usize, popup: Rect) {
    let (name, _, _) = ONBOARD_PROVIDERS[provider];
    let block = onboard_block(
        format!(" ◈ {name} — Auth Method   1/2 select   Esc back "),
        C_ACCENT,
    );
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines = vec![
        Line::default(),
        Line::from(Span::styled(
            "  How would you like to authenticate?",
            Style::default().fg(Color::White),
        )),
        Line::default(),
        Line::from(vec![
            Span::styled("  [1]  ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("API Key          ", Style::default().fg(Color::White)),
            Span::styled("from console.anthropic.com", Style::default().fg(C_DIM)),
        ]),
        Line::from(vec![
            Span::styled("  [2]  ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("OAuth (browser)  ", Style::default().fg(Color::White)),
            Span::styled("free tier — Haiku only", Style::default().fg(C_DIM)),
        ]),
    ];

    if let Some(err) = &ob.error {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("  ✘ ", Style::default().fg(C_RED)),
            Span::styled(err.clone(), Style::default().fg(C_RED)),
        ]));
    }

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn render_onboard_key(f: &mut Frame, ob: &OnboardPopup, provider: usize, popup: Rect) {
    let (name, _, env_var) = ONBOARD_PROVIDERS[provider];
    let block = onboard_block(
        format!(" ◈ {name} — API Key   Enter confirm   Esc back "),
        C_ACCENT,
    );

    // Split popup: block border, then inside we render a labelled input widget.
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Mask: show last 4 chars, rest as •
    let masked: String = if ob.key_input.len() > 4 {
        format!("{}{}", "•".repeat(ob.key_input.len() - 4), &ob.key_input[ob.key_input.len() - 4..])
    } else {
        ob.key_input.clone()
    };

    // Layout: label row + input block + optional error
    let input_area = Rect {
        x: inner.x + 2,
        y: inner.y + 3,
        width: inner.width.saturating_sub(4),
        height: 3,
    };

    let lines = vec![
        Line::default(),
        Line::default(),
        Line::from(Span::styled(
            format!("  {env_var}"),
            Style::default().fg(C_DIM),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)), inner);

    // Input field as a proper bordered block.
    let key_text = if ob.key_input.is_empty() {
        Span::styled("  paste or type key…", Style::default().fg(C_DIM))
    } else {
        Span::styled(format!("  {masked}"), Style::default().fg(Color::White))
    };
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_ACCENT));
    f.render_widget(Paragraph::new(key_text).block(input_block), input_area);

    // Cursor position inside the input block.
    let cx = (input_area.x + 3 + masked.len() as u16).min(input_area.x + input_area.width - 2);
    let cy = input_area.y + 1;
    f.set_cursor_position((cx, cy));

    if let Some(err) = &ob.error {
        let err_y = input_area.y + input_area.height + 1;
        if err_y < inner.y + inner.height {
            let err_area = Rect { x: inner.x, y: err_y, width: inner.width, height: 1 };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("  ✘ ", Style::default().fg(C_RED)),
                    Span::styled(err.clone(), Style::default().fg(C_RED)),
                ])),
                err_area,
            );
        }
    }
}

fn render_onboard_done(f: &mut Frame, popup: Rect) {
    let block = onboard_block(" ◈ Setup Complete ".to_string(), C_GREEN);
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let lines = vec![
        Line::default(),
        Line::from(Span::styled(
            "  ✔  Credentials saved successfully!",
            Style::default().fg(C_GREEN).add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        Line::from(Span::styled(
            "  Your key is active for this session and stored in",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "  ~/.config/openclaw-code/auth.json",
            Style::default().fg(C_ACCENT),
        )),
        Line::default(),
        Line::from(Span::styled(
            "  Press any key to continue.",
            Style::default().fg(C_DIM),
        )),
    ];

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}
