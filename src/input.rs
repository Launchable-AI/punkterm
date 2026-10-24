//! Keyboard → bytes for the PTY (xterm encoding).

use alacritty_terminal::term::TermMode;
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn csi_mod(mods: ModifiersState) -> u8 {
    1 + mods.shift_key() as u8 + 2 * mods.alt_key() as u8 + 4 * mods.control_key() as u8
}

pub fn encode(ev: &KeyEvent, mods: ModifiersState, mode: TermMode) -> Option<Vec<u8>> {
    let m = csi_mod(mods);
    let app = mode.contains(TermMode::APP_CURSOR);
    // Cursor-style keys: SS3 in application mode without modifiers, CSI otherwise.
    let cursor = |c: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if app {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let alt = |b: Vec<u8>| if mods.alt_key() { [vec![0x1b], b].concat() } else { b };

    if let Key::Named(named) = &ev.logical_key {
        let out = match named {
            NamedKey::Enter => alt(if mods.shift_key() { b"\x1b\r".to_vec() } else { b"\r".to_vec() }),
            NamedKey::Backspace => alt(if mods.control_key() { vec![0x08] } else { vec![0x7f] }),
            NamedKey::Tab => if mods.shift_key() { b"\x1b[Z".to_vec() } else { alt(b"\t".to_vec()) },
            NamedKey::Escape => alt(vec![0x1b]),
            NamedKey::Space => {
                if mods.control_key() { vec![0] } else { alt(b" ".to_vec()) }
            }
            NamedKey::ArrowUp => cursor('A'),
            NamedKey::ArrowDown => cursor('B'),
            NamedKey::ArrowRight => cursor('C'),
            NamedKey::ArrowLeft => cursor('D'),
            NamedKey::Home => cursor('H'),
            NamedKey::End => cursor('F'),
            NamedKey::Insert => tilde(2),
            NamedKey::Delete => tilde(3),
            NamedKey::PageUp => tilde(5),
            NamedKey::PageDown => tilde(6),
            NamedKey::F1 => cursor_ss3('P', m),
            NamedKey::F2 => cursor_ss3('Q', m),
            NamedKey::F3 => cursor_ss3('R', m),
            NamedKey::F4 => cursor_ss3('S', m),
            NamedKey::F5 => tilde(15),
            NamedKey::F6 => tilde(17),
            NamedKey::F7 => tilde(18),
            NamedKey::F8 => tilde(19),
            NamedKey::F9 => tilde(20),
            NamedKey::F10 => tilde(21),
            NamedKey::F11 => tilde(23),
            NamedKey::F12 => tilde(24),
            _ => return None,
        };
        return Some(out);
    }

    if mods.control_key() {
        if let Key::Character(s) = &ev.logical_key {
            let c = s.chars().next()?.to_ascii_lowercase();
            let b = match c {
                'a'..='z' => c as u8 - b'a' + 1,
                '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '7' => 0x1f,
                '/' => 0x1f,
                '8' => 0x7f,
                _ => return ev.text.as_ref().map(|t| alt(t.as_bytes().to_vec())),
            };
            return Some(alt(vec![b]));
        }
    }

    let text = ev.text.as_ref()?;
    if text.is_empty() {
        return None;
    }
    Some(alt(text.as_bytes().to_vec()))
}

fn cursor_ss3(c: char, m: u8) -> Vec<u8> {
    if m > 1 { format!("\x1b[1;{m}{c}").into_bytes() } else { format!("\x1bO{c}").into_bytes() }
}
