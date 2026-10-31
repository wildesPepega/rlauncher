#!/bin/sh
# rlauncher installer
#
#   curl -fsSL https://raw.githubusercontent.com/wildesPepega/rlauncher/main/install.sh | sh
#
# Options (pass after `sh -s --` when piping):
#   --source         build from source with cargo instead of downloading a release binary
#   --version vX.Y.Z install a specific release (default: latest)
#   --no-autostart   don't start rlauncher automatically on login
#   --no-start       don't start the daemon after installing
#   --uninstall      remove rlauncher (your config in ~/.config/rlauncher is kept)
#
# Environment: PREFIX (default ~/.local)
set -eu

REPO="wildesPepega/rlauncher"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"

MODE=binary
VERSION=latest
AUTOSTART=1
START=1
UNINSTALL=0

info() { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --source) MODE=source ;;
        --version) shift; VERSION="${1:?--version needs a value}" ;;
        --no-autostart) AUTOSTART=0 ;;
        --no-start) START=0 ;;
        --uninstall) UNINSTALL=1 ;;
        -h|--help) sed -n '2,15p' "$0" 2>/dev/null || true; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
    shift
done

if [ "$UNINSTALL" = 1 ]; then
    info "Stopping rlauncher"
    "$BIN_DIR/rlauncher" quit 2>/dev/null || true
    rm -f "$BIN_DIR/rlauncher" "$BIN_DIR/rlauncher-toggle" \
        "$APP_DIR/io.github.rlauncher.desktop" "$AUTOSTART_DIR/rlauncher.desktop"
    info "rlauncher removed. Your settings are still in ~/.config/rlauncher (delete it to reset)."
    exit 0
fi

[ "$(uname -s)" = Linux ] || die "rlauncher only runs on Linux"

case "${XDG_SESSION_TYPE:-}" in
    wayland) ;;
    "") ;;
    *) warn "rlauncher needs a Wayland session (current: $XDG_SESSION_TYPE)" ;;
esac
case "${XDG_CURRENT_DESKTOP:-}" in
    *GNOME*) warn "GNOME does not support wlr-layer-shell, rlauncher will not be able to show its window" ;;
esac

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

download() {
    arch="$(uname -m)"
    case "$arch" in
        x86_64|aarch64) ;;
        *) warn "no prebuilt binary for $arch"; return 1 ;;
    esac
    if [ "$VERSION" = latest ]; then
        url="https://github.com/$REPO/releases/latest/download/rlauncher-$arch-linux.tar.gz"
    else
        url="https://github.com/$REPO/releases/download/$VERSION/rlauncher-$arch-linux.tar.gz"
    fi
    info "Downloading $url"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$TMP/rlauncher.tar.gz" || return 1
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$TMP/rlauncher.tar.gz" "$url" || return 1
    else
        die "curl or wget is required"
    fi
    tar -xzf "$TMP/rlauncher.tar.gz" -C "$TMP" || return 1
    [ -x "$TMP/rlauncher/rlauncher" ] || return 1
    # The binary must be able to run on this system (glibc version, libxkbcommon).
    if ! "$TMP/rlauncher/rlauncher" --help >/dev/null 2>&1; then
        warn "the prebuilt binary does not run on this system"
        return 1
    fi
    cp "$TMP/rlauncher/rlauncher" "$TMP/rlauncher/rlauncher-toggle" "$TMP/"
}

build() {
    command -v cargo >/dev/null 2>&1 || die "cargo not found. Install Rust from https://rustup.rs (1.88 or newer) and run this again."
    if [ -f Cargo.toml ] && grep -q '^name = "rlauncher"' Cargo.toml; then
        info "Building from the local checkout"
        cargo build --release --locked
        cp target/release/rlauncher target/release/rlauncher-toggle "$TMP/"
    else
        info "Building from https://github.com/$REPO (this takes a few minutes)"
        if [ "$VERSION" = latest ]; then
            cargo install --locked --git "https://github.com/$REPO" --root "$TMP/root" rlauncher
        else
            cargo install --locked --git "https://github.com/$REPO" --tag "$VERSION" --root "$TMP/root" rlauncher
        fi
        cp "$TMP/root/bin/rlauncher" "$TMP/root/bin/rlauncher-toggle" "$TMP/"
    fi
}

if [ "$MODE" = binary ]; then
    download || { warn "falling back to building from source"; build; }
else
    build
fi

if command -v ldconfig >/dev/null 2>&1 && ! ldconfig -p 2>/dev/null | grep -q 'libxkbcommon.so.0'; then
    warn "libxkbcommon was not found, install it with your package manager (libxkbcommon / libxkbcommon0)"
fi

info "Installing to $BIN_DIR"
"$BIN_DIR/rlauncher" quit 2>/dev/null || true
mkdir -p "$BIN_DIR" "$APP_DIR"
install -m755 "$TMP/rlauncher" "$BIN_DIR/rlauncher"
install -m755 "$TMP/rlauncher-toggle" "$BIN_DIR/rlauncher-toggle"

# Gives rlauncher an app id, needed by the XDG GlobalShortcuts portal.
cat > "$APP_DIR/io.github.rlauncher.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=rlauncher
GenericName=Application Launcher
Comment=Fast application launcher for Wayland
Exec=$BIN_DIR/rlauncher-toggle
Icon=system-search
Terminal=false
Categories=Utility;
NoDisplay=true
EOF

if [ "$AUTOSTART" = 1 ]; then
    mkdir -p "$AUTOSTART_DIR"
    cat > "$AUTOSTART_DIR/rlauncher.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=rlauncher
Comment=Start the rlauncher daemon
Exec=$BIN_DIR/rlauncher
Icon=system-search
Terminal=false
NoDisplay=true
X-GNOME-Autostart-enabled=true
EOF
    info "Autostart enabled"
fi

if [ "$START" = 1 ] && [ -n "${WAYLAND_DISPLAY:-}" ]; then
    info "Starting rlauncher"
    nohup "$BIN_DIR/rlauncher" >/dev/null 2>&1 &
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not in your PATH, add it to use the rlauncher commands" ;;
esac

cat <<EOF

rlauncher is installed.

  Open it:      your hotkey (KDE: Meta+Space by default) or run rlauncher-toggle
  Settings:     click the gear in the search field, or run: rlauncher settings
  Other WMs:    bind "rlauncher-toggle" to a key in your compositor config
  Uninstall:    curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- --uninstall

EOF
