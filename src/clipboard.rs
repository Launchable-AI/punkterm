//! Clipboard via wl-clipboard (Wayland) or xclip, whichever is installed.

use std::io::Write;
use std::process::{Command, Stdio};

pub fn copy(text: &str) {
    write(text, false)
}

/// Set the primary selection (what middle-click pastes).
pub fn copy_primary(text: &str) {
    write(text, true)
}

fn write(text: &str, primary: bool) {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (cmd, args): (&str, &[&str]) = match (wayland, primary) {
        (true, false) => ("wl-copy", &[]),
        (true, true) => ("wl-copy", &["--primary"]),
        (false, false) => ("xclip", &["-selection", "clipboard"]),
        (false, true) => ("xclip", &["-selection", "primary"]),
    };
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

/// The primary selection (what middle-click pastes),
/// falling back to the clipboard when nothing is selected.
pub fn paste_primary() -> Option<String> {
    read(true).filter(|t| !t.is_empty()).or_else(paste)
}

fn read(primary: bool) -> Option<String> {
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

/// An image on the clipboard, as PNG (or JPEG) bytes and its extension.
pub fn image() -> Option<(Vec<u8>, &'static str)> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let types = if wayland {
        Command::new("wl-paste").arg("--list-types").stderr(Stdio::null()).output().ok()?
    } else {
        Command::new("xclip").args(["-selection", "clipboard", "-t", "TARGETS", "-o"]).stderr(Stdio::null()).output().ok()?
    };
    let types = String::from_utf8_lossy(&types.stdout);
    let (mime, ext) = [("image/png", "png"), ("image/jpeg", "jpg")].into_iter().find(|(m, _)| types.lines().any(|l| l.trim() == *m))?;
    let out = if wayland {
        Command::new("wl-paste").args(["--type", mime]).stderr(Stdio::null()).output().ok()?
    } else {
        Command::new("xclip").args(["-selection", "clipboard", "-t", mime, "-o"]).stderr(Stdio::null()).output().ok()?
    };
    (out.status.success() && !out.stdout.is_empty()).then_some((out.stdout, ext))
}
