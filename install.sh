#!/usr/bin/env bash
set -euo pipefail

BINARY_URL="https://github.com/zsigisti/openclaw-code/releases/download/beta/openclaw-code"
BINARY_NAME="openclaw-code"
INSTALL_DIR="/usr/local/bin"

# ── Colours ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'

info()    { echo -e "${CYAN}${BOLD}→${RESET} $*"; }
success() { echo -e "${GREEN}${BOLD}✓${RESET} $*"; }
error()   { echo -e "${RED}${BOLD}✗${RESET} $*" >&2; exit 1; }

# ── Platform check ────────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

if [[ "$OS" != "Linux" ]]; then
    error "openclaw-code binaries are currently only available for Linux. Build from source on $OS."
fi

if [[ "$ARCH" != "x86_64" ]]; then
    error "Only x86_64 is supported in this release (detected: $ARCH)."
fi

# ── Dependency check ──────────────────────────────────────────────────────────
for cmd in curl chmod; do
    command -v "$cmd" &>/dev/null || error "Required tool not found: $cmd"
done

# ── Download ──────────────────────────────────────────────────────────────────
TMP_FILE="$(mktemp /tmp/openclaw-code.XXXXXX)"
trap 'rm -f "$TMP_FILE"' EXIT

info "Downloading openclaw-code..."
if ! curl -fsSL --progress-bar "$BINARY_URL" -o "$TMP_FILE"; then
    error "Download failed. Check your internet connection or the release URL."
fi

chmod +x "$TMP_FILE"

# ── Install ───────────────────────────────────────────────────────────────────
INSTALL_PATH="$INSTALL_DIR/$BINARY_NAME"

if [[ -w "$INSTALL_DIR" ]]; then
    mv "$TMP_FILE" "$INSTALL_PATH"
else
    info "Installing to $INSTALL_PATH (sudo required)..."
    sudo mv "$TMP_FILE" "$INSTALL_PATH"
fi

success "Installed to $INSTALL_PATH"

# ── Verify ────────────────────────────────────────────────────────────────────
if command -v "$BINARY_NAME" &>/dev/null; then
    success "openclaw-code is ready."
else
    echo ""
    echo -e "  ${BOLD}Note:${RESET} $INSTALL_PATH is not on your PATH."
    echo -e "  Add this to your shell config (e.g. ~/.bashrc or ~/.zshrc):"
    echo -e "    ${CYAN}export PATH=\"\$PATH:$INSTALL_DIR\"${RESET}"
fi

# ── Done ──────────────────────────────────────────────────────────────────────
echo ""
echo -e "  ${BOLD}Get started:${RESET}"
echo -e "    ${CYAN}openclaw-code setup${RESET}   — configure your API credentials"
echo -e "    ${CYAN}openclaw-code${RESET}          — start chatting"
echo ""
