#!/usr/bin/env bash
# Build and install punkterm on macOS (Apple Silicon or Intel).
#
#   scripts/install-macos.sh            build, install punkterm.app and a CLI link
#   scripts/install-macos.sh --no-deps  skip Homebrew packages
#
# Touches only: Homebrew packages (after asking), ~/.rustup + ~/.cargo (only
# if no new-enough Rust is present; shell rc files are left alone),
# ~/Applications/punkterm.app and ~/.local/bin/punkterm (never replacing
# anything punkterm did not put there).
set -euo pipefail

cd "$(dirname "$0")/.."
NEED_RUST="1.90"
DEPS=1
[[ "${1:-}" == "--no-deps" ]] && DEPS=0

say() { printf '\033[1;35m::\033[0m %s\n' "$*"; }
ask() { read -r -p "$1 [y/N] " a; [[ "$a" == [yY]* ]]; }

[[ "$(uname -s)" == Darwin ]] || { say "this is the macOS installer; use scripts/install.sh"; exit 1; }

# ---------------------------------------------------------------- toolchain
if ! xcode-select -p >/dev/null 2>&1; then
  say "the Xcode command line tools are needed (compiler, git, python3)"
  xcode-select --install || true
  say "re-run this script once they have finished installing"
  exit 1
fi

# ---------------------------------------------------------------- packages
if [[ $DEPS == 1 ]]; then
  if command -v brew >/dev/null; then
    missing=()
    command -v tmux >/dev/null || missing+=(tmux)
    [[ ${#missing[@]} -gt 0 ]] && { say "packages: ${missing[*]}"; ask "install with Homebrew?" && brew install "${missing[@]}"; }
    if ! ls ~/Library/Fonts/JetBrainsMono* /Library/Fonts/JetBrainsMono* >/dev/null 2>&1; then
      ask "install the JetBrains Mono font (Homebrew cask; otherwise Menlo is used)?" && brew install --cask font-jetbrains-mono
    fi
  else
    command -v tmux >/dev/null || say "tmux not found; install Homebrew (https://brew.sh) and run: brew install tmux"
  fi
fi

# ---------------------------------------------------------------- rust
CARGO=$(command -v cargo || true)
ver() { "$1" --version 2>/dev/null | awk '{print $2}'; }
# Is version $1 >= NEED_RUST? (no sort -V on older macOS)
newer() { awk -v a="$1" -v b="$NEED_RUST" 'BEGIN { split(a, x, "."); split(b, y, "."); for (i = 1; i <= 3; i++) { if (x[i] + 0 != y[i] + 0) exit !(x[i] + 0 > y[i] + 0) } exit 0 }'; }
have=$([[ -n "$CARGO" ]] && ver "$CARGO" || true)
if [[ -z "$have" ]] || ! newer "$have"; then
  if [[ -x "$HOME/.cargo/bin/rustup" ]]; then
    say "updating Rust via rustup"
    "$HOME/.cargo/bin/rustup" update stable
  else
    say "Rust ${have:-not found}; punkterm needs >= $NEED_RUST"
    ask "install Rust with rustup (no changes to your shell rc files)?" || exit 1
    curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal
  fi
  CARGO="$HOME/.cargo/bin/cargo"
fi

say "building (release)"
"$CARGO" build --release

# ---------------------------------------------------------------- app bundle
# A minimal .app so punkterm opens from Spotlight, Launchpad and the Dock.
APP="$HOME/Applications/punkterm.app"
MARK="$APP/Contents/Resources/.installed-by-punkterm"
if [[ -e "$APP" && ! -e "$MARK" ]]; then
  say "$APP exists and is not ours; leaving it alone"
else
  mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
  install -m 755 target/release/punkterm "$APP/Contents/MacOS/punkterm"
  VERSION=$(awk -F'"' '/^version/ {print $2; exit}' Cargo.toml)
  cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>punkterm</string>
  <key>CFBundleDisplayName</key><string>punkterm</string>
  <key>CFBundleIdentifier</key><string>ai.launchable.punkterm</string>
  <key>CFBundleExecutable</key><string>punkterm</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST
  touch "$MARK"
  # Built here, so Gatekeeper never quarantines it; an ad-hoc signature is
  # still required to run on Apple Silicon.
  codesign --force --sign - "$APP" >/dev/null 2>&1 || say "codesign failed; the app may not open"
  say "installed $APP"
fi

# ---------------------------------------------------------------- cli link
BIN="$HOME/.local/bin/punkterm"
mkdir -p "$HOME/.local/bin"
if [[ -L "$BIN" && "$(readlink "$BIN")" != "$APP/Contents/MacOS/punkterm" ]] || [[ -e "$BIN" && ! -L "$BIN" ]]; then
  say "$BIN exists and is not ours; leaving it alone"
else
  ln -sf "$APP/Contents/MacOS/punkterm" "$BIN"
  say "linked $BIN"
fi

case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) say "note: ~/.local/bin is not on PATH; open punkterm from Spotlight, or run $BIN" ;; esac
say "done: open punkterm from Spotlight or Launchpad (Cmd+? for keys)"
