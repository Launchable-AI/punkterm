//! A tmux control-mode client (`tmux -C attach`), locally or over ssh.
//!
//! punkterm joins the user's existing tmux as one more client: it never
//! changes options (beyond a set-and-clear nudge, see `report_colours`) and
//! never resizes windows, so a kitty+tmux setup on the same server keeps
//! working exactly as before. One connection per (host,
//! tmux session) carries every pane; punkterm renders each pane itself.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// What a command's reply block is for.
#[derive(Clone, Debug)]
pub enum Want {
    Ignore,
    Panes,
    Capture(String),
    /// A pane's cursor, asked right after its capture.
    Cursor(String),
}

#[derive(Debug)]
pub enum CcEvent {
    Output { conn: u64, pane: String, data: Vec<u8> },
    Reply { conn: u64, want: Want, lines: Vec<String> },
    /// Windows/panes/layout changed: re-list panes.
    Changed { conn: u64 },
    Exit { conn: u64 },
}

/// The writing half, shared by every pane on the connection.
pub struct Writer {
    stdin: Mutex<ChildStdin>,
    queue: Arc<Mutex<VecDeque<Want>>>,
    size: Mutex<(usize, usize)>,
    window_sizes: Mutex<std::collections::HashMap<String, (usize, usize)>>,
    pinned: Mutex<std::collections::HashMap<String, (usize, usize)>>,
    colours: Mutex<std::collections::HashMap<String, ([u8; 3], [u8; 3])>>,
}

impl Writer {
    pub fn command(&self, want: Want, cmd: &str) {
        let mut q = self.queue.lock().unwrap();
        let mut w = self.stdin.lock().unwrap();
        if w.write_all(cmd.as_bytes()).and_then(|_| w.write_all(b"\n")).and_then(|_| w.flush()).is_ok() {
            q.push_back(want);
        }
    }

    /// Type bytes into a pane, hex-encoded so any byte survives.
    pub fn send_bytes(&self, pane: &str, bytes: &[u8]) {
        for chunk in bytes.chunks(256) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            self.command(Want::Ignore, &format!("send-keys -t {pane} -H {}", hex.join(" ")));
        }
    }

    /// This client's size; unchanged sizes are not re-sent (window drags
    /// resize every frame).
    pub fn set_size(&self, cols: usize, lines: usize) {
        let mut last = self.size.lock().unwrap();
        if *last != (cols, lines) {
            *last = (cols, lines);
            self.command(Want::Ignore, &format!("refresh-client -C {cols}x{lines}"));
        }
    }

    /// This client's size for one window, overriding its overall size there.
    /// True when it asked tmux for a new size.
    pub fn set_window_size(&self, window: &str, cols: usize, lines: usize) -> bool {
        let mut sizes = self.window_sizes.lock().unwrap();
        if sizes.get(window) == Some(&(cols, lines)) {
            return false;
        }
        sizes.insert(window.to_string(), (cols, lines));
        self.command(Want::Ignore, &format!("refresh-client -C {window}:{cols}x{lines}"));
        true
    }

    /// Hold a pane at `width` columns. Asked once per change of its width:
    /// a resize tmux clamps is not retried on every listing.
    pub fn pin_width(&self, pane: &str, width: usize, now: usize) {
        let mut pinned = self.pinned.lock().unwrap();
        if now != width && pinned.get(pane) != Some(&(width, now)) {
            pinned.insert(pane.to_string(), (width, now));
            self.command(Want::Ignore, &format!("resize-pane -t {pane} -x {width}"));
        }
    }

    /// Tell tmux the colours behind each pane, as a terminal answers OSC
    /// 10/11. tmux (3.6+) answers the pane's own colour queries with them and
    /// derives the pane's light/dark from the background, which it reports to
    /// apps that asked for theme updates (mode 2031: Claude Code's "auto",
    /// cc-rail). tmux only re-sends those reports when a pane's style
    /// changes, so after any new colours, set and clear a pane-level
    /// `window-style` on one pane: that marks every pane, and leaves the
    /// options as they were. Older tmux rejects `-r`; nothing changes there.
    pub fn report_colours(&self, panes: &[&str], fg: [u8; 3], bg: [u8; 3]) {
        let osc = |n: u8, c: [u8; 3]| format!("\\033]{n};rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}\\007", c[0], c[1], c[2]);
        let mut sent = self.colours.lock().unwrap();
        let mut last = None;
        for &pane in panes {
            if sent.get(pane) == Some(&(fg, bg)) {
                continue;
            }
            sent.insert(pane.to_string(), (fg, bg));
            self.command(Want::Ignore, &format!("refresh-client -r \"{pane}:{}\"", osc(10, fg)));
            self.command(Want::Ignore, &format!("refresh-client -r \"{pane}:{}\"", osc(11, bg)));
            last = Some(pane);
        }
        if let Some(pane) = last {
            self.command(Want::Ignore, &format!("set -p -t {pane} window-style default"));
            self.command(Want::Ignore, &format!("set -pu -t {pane} window-style"));
        }
    }

    pub fn list_panes(&self) {
        self.command(Want::Panes, &format!("list-panes -s -F \"{PANE_FORMAT}\""));
    }

    pub fn capture(&self, pane: &str) {
        self.command(Want::Capture(pane.to_string()), &format!("capture-pane -p -e -t {pane} -S -{}", crate::term::SCROLLBACK));
        // The cursor as of the capture: the one from the last pane listing
        // can be older (a new shell's, before it printed its prompt).
        self.command(Want::Cursor(pane.to_string()), &format!("display -p -t {pane} \"#{{cursor_x}} #{{cursor_y}}\""));
    }
}

/// Fields of a pane listing, separated by a glyph that never occurs in titles.
pub const SEP: char = '\u{241F}';
const PANE_FORMAT: &str = "#{pane_id}\u{241F}#{window_id}\u{241F}#{window_index}\u{241F}#{window_name}\u{241F}#{pane_width}\u{241F}#{pane_height}\u{241F}#{pane_current_command}\u{241F}#{pane_title}\u{241F}#{@cc_rail_pane}\u{241F}#{alternate_on}\u{241F}#{cursor_x}\u{241F}#{cursor_y}\u{241F}#{session_name}\u{241F}#{window_width}\u{241F}#{window_height}\u{241F}#{pane_left}\u{241F}#{pane_top}\u{241F}#{window_zoomed_flag}\u{241F}#{mouse_standard_flag}#{mouse_button_flag}#{mouse_all_flag}#{mouse_sgr_flag}#{mouse_utf8_flag}#{keypad_cursor_flag}";

#[derive(Clone, Debug, PartialEq)]
pub struct PaneInfo {
    pub id: String,
    pub window_id: String,
    pub window_index: u32,
    pub window_name: String,
    pub cols: usize,
    pub lines: usize,
    pub command: String,
    pub title: String,
    pub rail: bool,
    pub alt: bool,
    pub cursor: (usize, usize),
    pub session: String,
    pub window_size: (usize, usize),
    /// Where the pane sits in its window, in cells.
    pub pos: (usize, usize),
    pub zoomed: bool,
    /// Modes the pane's program set, as tmux holds them: a capture brings
    /// the screen but not these (`mode_sequences`).
    pub modes: [bool; 6],
}

pub fn parse_pane(line: &str) -> Option<PaneInfo> {
    let f: Vec<&str> = line.split(SEP).collect();
    if f.len() < 13 {
        return None;
    }
    let num = |i: usize| f.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(PaneInfo {
        id: f[0].to_string(),
        window_id: f[1].to_string(),
        window_index: f[2].parse().ok()?,
        window_name: f[3].to_string(),
        cols: f[4].parse().ok()?,
        lines: f[5].parse().ok()?,
        command: f[6].to_string(),
        title: f[7].to_string(),
        rail: !f[8].is_empty(),
        alt: f[9] == "1",
        cursor: (f[10].parse().unwrap_or(0), f[11].parse().unwrap_or(0)),
        session: f[12].to_string(),
        window_size: (num(13), num(14)),
        pos: (num(15), num(16)),
        zoomed: f.get(17) == Some(&"1"),
        modes: {
            let m = f.get(18).unwrap_or(&"").as_bytes();
            std::array::from_fn(|k| m.get(k) == Some(&b'1'))
        },
    })
}

impl PaneInfo {
    /// The sequences that set the modes `modes` records: mouse tracking
    /// (click, drag, any motion), its SGR / UTF-8 encoding, cursor keys.
    pub fn mode_sequences(&self) -> String {
        let [standard, button, all, sgr, utf8, cursor_keys] = self.modes;
        let mut s = String::new();
        let mut on = |code: u16, set: bool| {
            if set {
                s.push_str(&format!("\x1b[?{code}h"));
            }
        };
        on(1003, all);
        on(1002, button && !all);
        on(1000, standard && !button && !all);
        on(1006, sgr);
        on(1005, utf8);
        on(1, cursor_keys);
        s
    }
}

/// `%output` data: octal escapes (`\ooo`) for control bytes and backslash.
pub fn unescape(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// A host's ssh spec, split like a shell would: a destination
/// (`ubuntu@1.2.3.4`, an alias) or a whole command
/// (`ssh -i ~/.ssh/key.pem ubuntu@1.2.3.4`). A leading `ssh` is dropped and
/// `~/` expanded, so lines can be pasted straight from shell history.
pub fn ssh_words(spec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let (mut cur, mut quote, mut any) = (String::new(), None::<char>, false);
    let mut chars = spec.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, '\\') => cur.extend(chars.next()),
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        words.push(cur);
    }
    if words.first().is_some_and(|w| w == "ssh" || w.ends_with("/ssh")) {
        words.remove(0);
    }
    let home = dirs::home_dir();
    for w in &mut words {
        if let (Some(rest), Some(h)) = (w.strip_prefix("~/"), &home) {
            *w = h.join(rest).display().to_string();
        }
    }
    words
}

/// The destination in an ssh spec (its last word), without the user: a
/// short name for the rail.
pub fn ssh_host_name(spec: &str) -> String {
    let words = ssh_words(spec);
    let dest = words.last().map(String::as_str).unwrap_or(spec);
    dest.rsplit('@').next().unwrap_or(dest).to_string()
}

/// ssh connection sharing: every connection to a host rides one TCP session.
/// The control socket lives in punkterm's cache dir; the user's ssh config is
/// only read, never changed.
pub fn ssh_args(spec: &str) -> Vec<String> {
    let mut v = vec!["-o".into(), "BatchMode=yes".into(), "-o".into(), "ConnectTimeout=8".into(), "-o".into(), "ServerAliveInterval=15".into()];
    if let Some(dir) = dirs::cache_dir().map(|d| d.join("punkterm").join("ssh")) {
        if std::fs::create_dir_all(&dir).is_ok() {
            v.extend([
                "-o".into(),
                "ControlMaster=auto".into(),
                "-o".into(),
                format!("ControlPath={}/%C", dir.display()),
                "-o".into(),
                "ControlPersist=120".into(),
            ]);
        }
    }
    v.extend(ssh_words(spec));
    v
}

fn tmux_command(ssh: Option<&str>, args: &[&str]) -> Command {
    match ssh {
        None => {
            let mut c = Command::new("tmux");
            c.args(args);
            c
        }
        Some(dest) => {
            let mut c = Command::new("ssh");
            c.args(ssh_args(dest)).arg(format!("tmux {}", args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")));
            c
        }
    }
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:,@%+=".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// tmux sessions on a host's default server. No server, or no sessions,
/// is an empty list; an unreachable host or missing tmux is an error with
/// the reason (ssh's or tmux's last line of complaint).
pub fn list_sessions(ssh: Option<&str>) -> Result<Vec<String>, String> {
    let out = tmux_command(ssh, &["list-sessions", "-F", "#{session_name}"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", if ssh.is_some() { "ssh" } else { "tmux" }))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(str::to_string).collect());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("no server running") || err.contains("error connecting to") || err.contains("no sessions") {
        return Ok(Vec::new());
    }
    if out.status.code() == Some(127) || err.contains("command not found") {
        return Err("tmux is not installed there".into());
    }
    let last = err.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("Warning: Permanently added")).last();
    Err(last.unwrap_or("connection failed").to_string())
}

/// Create a detached session (used only when a host has none to join).
/// Its first shell has Claude Code's image switch from the start: the
/// server-wide one punkterm sets on connecting comes too late for it.
pub fn new_detached_session(ssh: Option<&str>, name: &str) -> bool {
    tmux_command(ssh, &["new-session", "-d", "-s", name, "-e", "CLAUDE_CODE_FORCE_TERMINAL_IMAGES=1"]).stdin(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub struct Conn {
    pub id: u64,
    pub host: usize,
    pub session: String,
    pub writer: Arc<Writer>,
    child: Child,
}

impl Drop for Conn {
    fn drop(&mut self) {
        // Detach politely; tmux keeps every session running.
        let _ = self.writer.stdin.lock().map(|mut w| w.write_all(b"\n"));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Conn {
    pub fn open(
        id: u64,
        host: usize,
        ssh: Option<&str>,
        session: &str,
        tx: Sender<CcEvent>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<Conn> {
        let target = format!("={session}");
        let mut child = tmux_command(ssh, &["-C", "attach", "-t", &target])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let writer = Arc::new(Writer { stdin: Mutex::new(stdin), queue: queue.clone(), size: Mutex::new((0, 0)), window_sizes: Default::default(), pinned: Default::default(), colours: Default::default() });
        std::thread::spawn(move || read_loop(id, BufReader::new(stdout), queue, tx, wake));
        writer.list_panes();
        Ok(Conn { id, host, session: session.to_string(), writer, child })
    }
}

fn read_loop(
    conn: u64,
    mut r: BufReader<std::process::ChildStdout>,
    queue: Arc<Mutex<VecDeque<Want>>>,
    tx: Sender<CcEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let mut block: Option<(Want, Vec<String>)> = None;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        let line = String::from_utf8_lossy(&buf).into_owned();
        if let Some((_, lines)) = block.as_mut() {
            if line.starts_with("%end ") || line.starts_with("%error ") {
                let (want, lines) = block.take().unwrap();
                let _ = tx.send(CcEvent::Reply { conn, want, lines });
                wake();
            } else {
                lines.push(line);
            }
            continue;
        }
        if line.starts_with("%begin ") {
            // `%begin TIME NUMBER FLAGS`: flags 1 marks a command this client
            // sent. Others (the attach itself, commands run by the server's
            // hooks, e.g. a client-attached `source-file`) take no queue slot.
            let ours = line.rsplit(' ').next() != Some("0");
            let want = if ours { queue.lock().unwrap().pop_front().unwrap_or(Want::Ignore) } else { Want::Ignore };
            block = Some((want, Vec::new()));
        } else if let Some(rest) = line.strip_prefix("%output ") {
            if let Some((pane, _)) = rest.split_once(' ') {
                // Unescape the raw bytes: a line of output need not be valid UTF-8.
                let data = unescape(&buf[b"%output ".len() + pane.len() + 1..]);
                let _ = tx.send(CcEvent::Output { conn, pane: pane.to_string(), data });
                wake();
            }
        } else if line.starts_with("%exit") {
            break;
        } else if ["%window-add", "%window-close", "%unlinked-window-close", "%layout-change", "%window-renamed", "%session-window-changed", "%window-pane-changed"]
            .iter()
            .any(|p| line.starts_with(p))
        {
            let _ = tx.send(CcEvent::Changed { conn });
            wake();
        }
    }
    let _ = tx.send(CcEvent::Exit { conn });
    wake();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_octal() {
        assert_eq!(unescape(b"hi\\015\\012"), b"hi\r\n");
        assert_eq!(unescape(b"a\\134b\\033[0m"), b"a\\b\x1b[0m");
        assert_eq!(unescape(b"tail\\01"), b"tail\\01");
    }

    #[test]
    fn splits_ssh_specs() {
        let home = dirs::home_dir().unwrap().display().to_string();
        assert_eq!(ssh_words("ssh -i ~/.ssh/k.pem ubuntu@1.2.3.4"), ["-i", &format!("{home}/.ssh/k.pem"), "ubuntu@1.2.3.4"]);
        assert_eq!(ssh_words("box"), ["box"]);
        assert_eq!(ssh_words("-p 2222 'me@a b'"), ["-p", "2222", "me@a b"]);
        assert_eq!(ssh_host_name("ssh -i k.pem ubuntu@app.example.com"), "app.example.com");
    }

    #[test]
    fn quotes_shell_comments() {
        assert_eq!(shell_quote("#{session_name}"), "'#{session_name}'");
        assert_eq!(shell_quote("=main"), "=main");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn parses_pane_line() {
        let l = ["%3", "@1", "2", "zsh", "120", "40", "claude", "✳ Fix tests", "", "0", "4", "17", "main", "177", "40", "57", "0", "0"].join("\u{241F}");
        let p = parse_pane(&l).unwrap();
        assert_eq!(p.id, "%3");
        assert_eq!((p.cols, p.lines, p.cursor), (120, 40, (4, 17)));
        assert!(!p.rail && !p.alt);
        assert_eq!(p.title, "✳ Fix tests");
        assert_eq!((p.window_id.as_str(), p.window_size), ("@1", (177, 40)));
        assert_eq!((p.pos, p.zoomed), ((57, 0), false));
    }
}
