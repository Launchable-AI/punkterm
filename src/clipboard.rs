//! Clipboard via wl-clipboard (Wayland), xclip (X11) or pbcopy/pbpaste (macOS).

use std::io::Write;
use std::process::{Command, Stdio};

pub fn copy(text: &str) {
    write(text, false)
}

/// Set the primary selection (what middle-click and Shift+Insert paste).
pub fn copy_primary(text: &str) {
    write(text, true)
}

fn write(text: &str, primary: bool) {
    // macOS has one pasteboard and no primary selection.
    if cfg!(target_os = "macos") {
        if !primary {
            pipe("pbcopy", &[], text);
        }
        return;
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (cmd, args): (&str, &[&str]) = match (wayland, primary) {
        (true, false) => ("wl-copy", &[]),
        (true, true) => ("wl-copy", &["--primary"]),
        (false, false) => ("xclip", &["-selection", "clipboard"]),
        (false, true) => ("xclip", &["-selection", "primary"]),
    };
    pipe(cmd, args, text);
}

fn pipe(cmd: &str, args: &[&str], text: &str) {
    if let Ok(mut child) = Command::new(cmd).args(args).stdin(Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

pub fn paste() -> Option<String> {
    read(false)
}

/// The primary selection (what Shift+Insert pastes in kitty and xterm),
/// falling back to the clipboard when nothing is selected.
pub fn paste_primary() -> Option<String> {
    read(true).filter(|t| !t.is_empty()).or_else(paste)
}

fn read(primary: bool) -> Option<String> {
    if cfg!(target_os = "macos") {
        // No primary selection: paste_primary falls back to the pasteboard.
        if primary {
            return None;
        }
        let out = Command::new("pbpaste").stderr(Stdio::null()).output().ok()?;
        return out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let sel = if primary { "primary" } else { "clipboard" };
    let out = if wayland {
        let mut c = Command::new("wl-paste");
        c.arg("--no-newline");
        if primary {
            c.arg("--primary");
        }
        c.stderr(Stdio::null()).output().ok()?
    } else {
        Command::new("xclip").args(["-selection", sel, "-o"]).stderr(Stdio::null()).output().ok()?
    };
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
