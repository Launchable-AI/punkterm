//! jotbus-local, the message bus coding agents on one machine use to talk
//! to each other: who is on it and what they have said, for the comms view.
//! Read (only read) from the files jotbus-local keeps on each host:
//! `~/.config/jotbus-local/agents/<pid>.json` (one per running agent
//! session) and `channels/<name>.jsonl` (append-only message logs).
//!
//! A small Python script does the reading on the host, fed on stdin like the
//! other readers, and also matches each agent to the tmux pane it runs in
//! (an agent's server lists its parent processes; the pane's shell is one).

use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
import glob, json, os, subprocess
from datetime import datetime
home = os.environ.get('JOTBUS_LOCAL_HOME') or os.path.join(os.path.expanduser('~'), '.config', 'jotbus-local')
if not os.path.isdir(home):
    print('N')
    raise SystemExit
PER_CHANNEL = 200
MAX_TEXT = 4000
def esc(s): return str(s).replace('\\', '\\\\').replace('\t', '\\t').replace('\r', '').replace('\n', '\\n')
def when(t):
    try:
        d = datetime.fromisoformat(str(t).replace('Z', '+00:00'))
        return int(d.timestamp() * 1000), d.astimezone().strftime('%H:%M')
    except Exception: return 0, ''
panes = {}
try:
    out = subprocess.run(['tmux', 'list-panes', '-a', '-F', '#{pane_pid} #{pane_id}'], capture_output=True, text=True, timeout=5).stdout
    panes = dict(l.split() for l in out.splitlines() if len(l.split()) == 2)
except Exception: pass
print('Y')
for p in glob.glob(os.path.join(home, 'agents', '*.json')):
    try:
        with open(p) as f: a = json.load(f)
        pid = int(a.get('pid'))
    except Exception: continue
    try: os.kill(pid, 0)
    except PermissionError: pass
    except Exception: continue
    pane = next((panes[str(x)] for x in [pid] + list(a.get('ancestors') or []) if str(x) in panes), '')
    print('\t'.join(['A', esc(a.get('name', '')), esc(a.get('agent', '')), esc(a.get('cwd', '')),
                     str(when(a.get('last_active'))[0]), str(when(a.get('started_at'))[0]), pane]))
for p in glob.glob(os.path.join(home, 'channels', '*.jsonl')):
    ch = os.path.basename(p)[:-6]
    try:
        with open(p, 'rb') as f:
            size = os.path.getsize(p)
            f.seek(max(0, size - 1024 * 1024))
            data = f.read()
    except Exception: continue
    msgs = []
    for l in data.split(b'\n'):
        try:
            m = json.loads(l)
            if isinstance(m, dict) and isinstance(m.get('cursor'), int): msgs.append(m)
        except Exception: pass
    if not msgs: continue
    print('\t'.join(['C', esc(ch), str(msgs[-1]['cursor'])]))
    for m in msgs[-PER_CHANNEL:]:
        at, hm = when(m.get('created_at'))
        print('\t'.join(['M', esc(ch), str(m['cursor']), str(at), hm, esc(m.get('author', '')), esc(str(m.get('content', ''))[:MAX_TEXT])]))
"#;

/// A running agent session.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub name: String,
    /// claude, codex, …
    pub kind: String,
    /// Its project directory.
    pub cwd: String,
    pub active_ms: u64,
    pub started_ms: u64,
    /// The tmux pane it runs in, when it runs in one.
    pub pane: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub channel: String,
    pub cursor: u64,
    pub at_ms: u64,
    /// Local time on the host, "14:05".
    pub hm: String,
    pub author: String,
    pub content: String,
}

/// One host's bus. `present` is false where jotbus-local isn't set up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bus {
    pub present: bool,
    pub agents: Vec<Agent>,
    /// Every channel and its newest cursor (its message count, roughly).
    pub channels: Vec<(String, u64)>,
    /// The recent messages of every channel, oldest first.
    pub messages: Vec<Msg>,
}

/// The most messages kept for the view, across channels.
const KEEP: usize = 400;

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

pub fn parse(out: &str) -> Bus {
    let mut bus = Bus::default();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f[..] {
            ["Y"] => bus.present = true,
            ["A", name, kind, cwd, active, started, pane] => bus.agents.push(Agent {
                name: unesc(name),
                kind: unesc(kind),
                cwd: unesc(cwd),
                active_ms: active.parse().unwrap_or(0),
                started_ms: started.parse().unwrap_or(0),
                pane: (!pane.is_empty()).then(|| pane.to_string()),
            }),
            ["C", ch, latest] => bus.channels.push((unesc(ch), latest.parse().unwrap_or(0))),
            ["M", ch, cursor, at, hm, author, content] => bus.messages.push(Msg {
                channel: unesc(ch),
                cursor: cursor.parse().unwrap_or(0),
                at_ms: at.parse().unwrap_or(0),
                hm: hm.to_string(),
                author: unesc(author),
                content: unesc(content),
            }),
            _ => {}
        }
    }
    bus.agents.sort_by(|a, b| a.started_ms.cmp(&b.started_ms).then_with(|| a.name.cmp(&b.name)));
    bus.messages.sort_by(|a, b| a.at_ms.cmp(&b.at_ms).then_with(|| a.channel.cmp(&b.channel)).then(a.cursor.cmp(&b.cursor)));
    if bus.messages.len() > KEEP {
        bus.messages.drain(..bus.messages.len() - KEEP);
    }
    // Busiest-recently first: the channel of the newest message on top.
    let last = |ch: &str| bus.messages.iter().rev().position(|m| m.channel == ch).unwrap_or(usize::MAX);
    let mut chans = std::mem::take(&mut bus.channels);
    chans.sort_by_key(|(c, _)| last(c));
    bus.channels = chans;
    bus
}

/// Read a host's bus (`None` for `ssh`: this machine). `None` back: the host
/// couldn't be read.
pub fn fetch(ssh: Option<&str>) -> Option<Bus> {
    let mut c = match ssh {
        None => {
            let mut c = Command::new("python3");
            c.arg("-");
            c
        }
        Some(spec) => {
            let mut c = Command::new("ssh");
            c.args(crate::tmuxcc::ssh_args(spec)).arg("python3 -");
            c
        }
    };
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    child.stdin.take()?.write_all(SCRIPT.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    out.status.success().then(|| parse(&String::from_utf8_lossy(&out.stdout)))
}

/// Names @mentioned in a message, lowercased (`all` included).
pub fn mentions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let cs: Vec<char> = text.chars().collect();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.';
    for i in 0..cs.len() {
        if cs[i] != '@' || (i > 0 && word(cs[i - 1])) {
            continue;
        }
        let name: String = cs[i + 1..].iter().take_while(|&&c| word(c)).collect();
        let name = name.trim_end_matches(['.', '-', '_']).to_lowercase();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_bus() {
        let b = parse(
            "Y\n\
             A\tclaude-api\tclaude\t/home/k/api\t200\t100\t%3\n\
             A\tcodex-web\tcodex\t/home/k/web\t150\t50\t\n\
             C\tgeneral\t2\n\
             C\tbuild\t1\n\
             M\tgeneral\t1\t1000\t09:00\tclaude-api\t@codex-web is it green?\\nline two\n\
             M\tbuild\t1\t1500\t09:01\tci\ttab\\there \\\\ok\n\
             M\tgeneral\t2\t1200\t09:00\tcodex-web\tyes\n",
        );
        assert!(b.present);
        assert_eq!(b.agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["codex-web", "claude-api"]);
        assert_eq!(b.agents[1].pane.as_deref(), Some("%3"));
        assert_eq!(b.agents[0].pane, None);
        assert_eq!(b.messages.iter().map(|m| (m.channel.as_str(), m.cursor)).collect::<Vec<_>>(), [("general", 1), ("general", 2), ("build", 1)]);
        assert_eq!(b.messages[0].content, "@codex-web is it green?\nline two");
        assert_eq!(b.messages[2].content, "tab\there \\ok");
        assert_eq!(b.channels[0].0, "build", "most recent channel first");
        assert!(!parse("N\n").present);
    }

    #[test]
    fn finds_mentions() {
        assert_eq!(mentions("hi @Claude-API, cc @all. mail me@x.dev"), ["claude-api", "all"]);
    }
}
