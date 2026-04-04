# openclaw-code

A terminal-based AI coding assistant supporting multiple providers — Claude, GPT, Grok, and GitHub Copilot. Inspired by Claude Code, built from scratch in Rust.

![Platform](https://img.shields.io/badge/platform-Linux-blue)
![License](https://img.shields.io/badge/license-GPL--3.0-green)
![Language](https://img.shields.io/badge/language-Rust-orange)

---

## Features

- **Multi-provider** — Anthropic (Claude), OpenAI (GPT/o-series), xAI (Grok), GitHub Copilot
- **Agentic tool loop** — reads, writes, edits files; runs bash; searches with glob/grep
- **Diff display** — shows compact summaries for new files, full diffs for edits
- **OAuth login** — browser-based Anthropic OAuth (free tier) or API key
- **Setup wizard** — guided first-run onboarding via `openclaw-code setup`
- **Keyboard shortcuts** — Ctrl+A/E/U/K/W, Alt+arrows, word navigation
- **Slash commands** — `/help`, `/model`, `/clear`, `/compact`
- **Persistent config** — model and settings stored in `~/.config/openclaw-code/`

---

## Install

> **Linux x86\_64 only.** macOS and other platforms are not yet packaged — build from source instead.

### One-line install (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/zsigisti/openclaw-code/main/install.sh | bash
```

The script downloads the binary, installs it to `/usr/local/bin`, and prints setup instructions. Uses `sudo` only if needed.

### Manual download

```bash
curl -Lo openclaw-code https://github.com/zsigisti/openclaw-code/releases/download/beta/openclaw-code
chmod +x openclaw-code
sudo mv openclaw-code /usr/local/bin/
```

### Build from source

Requires Rust stable (1.75+):

```bash
git clone https://github.com/zsigisti/openclaw-code
cd openclaw-code/rust
cargo build --release -p openclaw-code
# Binary at: ./target/release/openclaw-code
```

---

## Setup

Run the interactive setup wizard on first use:

```bash
openclaw-code setup
```

This walks you through choosing a provider and entering credentials. Credentials are saved to `~/.config/openclaw-code/auth.json`.

You can also set credentials via environment variables:

| Variable               | Provider                      |
|------------------------|-------------------------------|
| `ANTHROPIC_API_KEY`    | Claude models                 |
| `ANTHROPIC_AUTH_TOKEN` | Claude (OAuth bearer token)   |
| `OPENAI_API_KEY`       | GPT / o-series / Codex        |
| `XAI_API_KEY`          | Grok models                   |
| `GITHUB_COPILOT_TOKEN` | GitHub Copilot                |

---

## Usage

```bash
openclaw-code                          # start with last-used model
openclaw-code claude-sonnet-4-6        # start with a specific model
openclaw-code setup                    # run the setup wizard
openclaw-code login                    # re-run the CLI login flow
```

### Keyboard shortcuts

| Shortcut         | Action                        |
|------------------|-------------------------------|
| `Enter`          | Send message                  |
| `Ctrl+C` / `Esc` | Quit / cancel                 |
| `Ctrl+A`         | Move to start of line         |
| `Ctrl+E`         | Move to end of line           |
| `Ctrl+U`         | Delete to start of line       |
| `Ctrl+K`         | Delete to end of line         |
| `Ctrl+W`         | Delete word before cursor     |
| `Alt+←` / `Alt+→`| Move word left / right        |
| `Page Up/Down`   | Scroll conversation           |

### Slash commands

| Command    | Description                        |
|------------|------------------------------------|
| `/help`    | Show available commands            |
| `/model`   | Switch AI model                    |
| `/clear`   | Clear conversation history         |
| `/compact` | Summarise and compact the context  |

---

## Supported models

| Provider         | Example models                                          |
|------------------|---------------------------------------------------------|
| Anthropic        | `claude-haiku-4-5-20251001`, `claude-sonnet-4-6`, `claude-opus-4-6` |
| OpenAI           | `gpt-4o`, `gpt-4o-mini`, `o1`, `o3-mini`               |
| xAI              | `grok-2`, `grok-2-mini`                                 |
| GitHub Copilot   | `gpt-4o` (via Copilot endpoint)                         |

---

## Configuration

Config and credentials live under `~/.config/openclaw-code/`:

```
~/.config/openclaw-code/
├── config.json      # last-used model, preferences
└── auth.json        # saved credentials (from setup wizard)
```

Legacy credentials from `~/.openclaw/agents/main/agent/auth-profiles.json` are read automatically for backward compatibility.

---

## License

Copyright (C) 2024 mmzs

This program is free software: you can redistribute it and/or modify it under the terms of the **GNU General Public License** as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
