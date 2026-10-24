#!/usr/bin/env bash
# Build and install punkterm for the current user (Ubuntu 22.04+ or Arch).
#
#   scripts/install.sh            build, install to ~/.local/bin, add a launcher
#   scripts/install.sh --no-deps  skip system packages
#
# Touches only: system packages (after asking), ~/.rustup + ~/.cargo (only if
# no new-enough Rust is present; shell rc files are left alone),
# ~/.local/bin/punkterm and ~/.local/share/applications/punkterm.desktop
# (never replacing a file punkterm did not put there).
set -euo pipefail

cd "$(dirname "$0")/.."
NEED_RUST="1.90"
DEPS=1
[[ "${1:-}" == "--no-deps" ]] && DEPS=0

say() { printf '\033[1;35m::\033[0m %s\n' "$*"; }
ask() { read -r -p "$1 [y/N] " a; [[ "$a" == [yY]* ]]; }

# ---------------------------------------------------------------- packages
. /etc/os-release 2>/dev/null || true
if [[ $DEPS == 1 ]]; then
  case "${ID:-}${ID_LIKE:-}" in
    *ubuntu*|*debian*)
      # Runtime: Vulkan or GL drivers, clipboard and sound helpers, tmux, a
      # monospace font. Display libraries are loaded at runtime, so building
      # needs only a C toolchain.
      PKGS=(build-essential pkg-config curl tmux libxkbcommon0 libxkbcommon-x11-0 libvulkan1
            mesa-vulkan-drivers libegl1 libgl1 wl-clipboard xclip pulseaudio-utils fonts-jetbrains-mono)
      say "packages: ${PKGS[*]}"
      if ask "install them with apt (sudo)?"; then
        sudo apt-get update && sudo apt-get install -y "${PKGS[@]}"
      fi
      ;;
    *arch*)
      PKGS=(base-devel tmux vulkan-icd-loader wl-clipboard ttf-jetbrains-mono-nerd)
      say "packages: ${PKGS[*]}"
      if ask "install missing ones with pacman (sudo)?"; then
        sudo pacman -S --needed "${PKGS[@]}"
      fi
      ;;
    *) say "unknown distro; skipping system packages" ;;
  esac
fi

# ---------------------------------------------------------------- rust
CARGO="$(command -v cargo || true)"
ver_ok() { [[ "$(printf '%s\n%s\n' "$NEED_RUST" "$1" | sort -V | head -1)" == "$NEED_RUST" ]]; }
have=""
[[ -n "$CARGO" ]] && have="$("$CARGO" --version | awk '{print $2}')"
if [[ -z "$have" ]] || ! ver_ok "$have"; then
  if [[ -x "$HOME/.cargo/bin/rustup" ]]; then
    say "updating Rust via rustup"
    "$HOME/.cargo/bin/rustup" update stable
  else
    say "Rust ${have:-not found}; punkterm needs >= $NEED_RUST"
    ask "install Rust with rustup (no changes to your shell rc files)?" || exit 1
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
  fi
  CARGO="$HOME/.cargo/bin/cargo"
fi

# ---------------------------------------------------------------- build
say "building (release)"
"$CARGO" build --release

# ---------------------------------------------------------------- install
BIN="$HOME/.local/bin/punkterm"
mkdir -p "$HOME/.local/bin"
if [[ -e "$BIN" ]] && ! "$BIN" --help 2>/dev/null | grep -q '^punkterm '; then
  say "$BIN exists and is not punkterm; leaving it alone"
else
  install -m 755 target/release/punkterm "$BIN"
  say "installed $BIN"
fi

DESK="$HOME/.local/share/applications/punkterm.desktop"
mkdir -p "$(dirname "$DESK")"
if [[ -e "$DESK" ]] && ! grep -q 'X-Punkterm=1' "$DESK"; then
  say "$DESK exists and is not ours; leaving it alone"
else
  cat > "$DESK" <<EOF
[Desktop Entry]
Type=Application
Name=punkterm
Comment=Cyberpunk terminal for Claude Code sessions
Exec=$BIN
Icon=utilities-terminal
Terminal=false
Categories=System;TerminalEmulator;
StartupWMClass=punkterm
X-Punkterm=1
EOF
  say "launcher: $DESK"
fi

case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) say "note: ~/.local/bin is not on PATH; run $BIN directly" ;; esac
say "done — run: punkterm   (Ctrl+Shift+/ for keys)"
