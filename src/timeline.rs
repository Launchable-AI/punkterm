//! The conversation behind a tmux pane, for the timeline rail: your
//! prompts, Claude's replies and its tool calls, read (only read) from the
//! session's transcript on the pane's host. A Python script finds the
//! Claude process under the pane, its session (~/.claude/sessions/<pid>.json,
//! or the background job a `claude attach` pane shows), and the transcript
//! in ~/.claude/projects, and prints one tab-separated line per event.

use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
import glob, json, os, re, subprocess, sys
home = os.path.expanduser('~')
cdir = os.path.join(home, '.claude')
def load(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return None
root = subprocess.run(['tmux', 'display', '-p', '-t', sys.argv[1], '#{pane_pid}'], capture_output=True, text=True, timeout=5).stdout.strip()
if not root.isdigit(): raise SystemExit
# The process tree from ps (there is no /proc on macOS): children, argv.
kids, args = {}, {}
for l in subprocess.run(['ps', '-A', '-o', 'pid=', '-o', 'ppid=', '-o', 'command='], capture_output=True, text=True, timeout=5).stdout.splitlines():
    f = l.split(None, 2)
    if len(f) == 3:
        kids.setdefault(f[1], []).append(f[0])
        args[f[0]] = f[2].split()
sid, name, todo = None, None, [root]
for _ in range(5):
    nxt = []
    for pid in todo:
        s = load(os.path.join(cdir, 'sessions', pid + '.json'))
        if s and s.get('sessionId'):
            sid, name = s['sessionId'], s.get('name'); break
        argv = args.get(pid, [])
        if len(argv) >= 3 and 'claude' in os.path.basename(argv[0]) and argv[1] == 'attach':
            j = load(os.path.join(cdir, 'jobs', os.path.basename(argv[2]), 'state.json')) or {}
            if j.get('sessionId'): sid, name = j['sessionId'], j.get('name'); break
        nxt += kids.get(pid, [])
    if sid: break
    todo = nxt
if not sid: raise SystemExit
paths = sorted(glob.glob(os.path.join(cdir, 'projects', '*', sid + '.jsonl')), key=os.path.getmtime)
if not paths: raise SystemExit
with open(paths[-1], 'rb') as f:
    f.seek(0, 2); size = f.tell(); f.seek(max(0, size - 6 * 1024 * 1024))
    data = f.read().decode(errors='replace')
lines = data.split('\n')
if size > 6 * 1024 * 1024: lines = lines[1:]
SYN = re.compile(r'^<(local-command|command-name|command-message|command-args|system-reminder|user-prompt-submit-hook|task-notification|fork-source|bash-input|bash-stdout|bash-stderr)')
def flat(s, n=700): return ' '.join(str(s).split())[:n]
def arg(name, i):
    if not isinstance(i, dict): return ''
    pick = lambda *ks: next((str(i[k]) for k in ks if i.get(k)), '')
    if name in ('Read', 'Edit', 'Write', 'NotebookEdit'): return os.path.basename(pick('file_path'))
    if name == 'Bash': return pick('description', 'command')
    if name in ('WebFetch',): return pick('url')
    return pick('description', 'query', 'pattern', 'skill', 'command', 'prompt')
ev = []
from datetime import datetime
def ms(t):
    try: return int(datetime.fromisoformat(str(t).replace('Z', '+00:00')).timestamp() * 1000)
    except Exception: return 0
for l in lines:
    try: o = json.loads(l)
    except Exception: continue
    t = o.get('type')
    if t in ('custom-title', 'agent-name') and not name: name = o.get('customTitle') or o.get('agentName')
    if t == 'ai-title' and o.get('aiTitle'): ai = o['aiTitle']
    if t not in ('user', 'assistant') or o.get('isSidechain') or o.get('isMeta'): continue
    c = (o.get('message') or {}).get('content')
    at = ms(o.get('timestamp'))
    if t == 'user':
        if isinstance(c, list):
            if any(b.get('type') == 'tool_result' for b in c if isinstance(b, dict)): continue
            c = '\n'.join(b.get('text', '') for b in c if isinstance(b, dict) and b.get('type') == 'text')
        if isinstance(c, str) and c.strip() and not SYN.match(c.lstrip()): ev.append(('ask', at, flat(c)))
    elif isinstance(c, list):
        for b in c:
            if not isinstance(b, dict): continue
            if b.get('type') == 'text' and b.get('text', '').strip(): ev.append(('say', at, flat(b['text'])))
            elif b.get('type') == 'tool_use': ev.append(('tool', at, flat(b.get('name', '?') + ' ' + arg(b.get('name'), b.get('input')), 120)))
title = name or (locals().get('ai')) or ''
if title: print('title\t0\t' + flat(title, 120))
for k, at, s in ev[-400:]: print('%s\t%d\t%s' % (k, at, s))
"#;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Ask,
    Say,
    Tool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub kind: Kind,
    /// Unix ms; 0 when the transcript did not say.
    pub at_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    pub title: String,
    pub entries: Vec<Entry>,
}

pub fn parse(out: &str) -> Option<Timeline> {
    let mut t = Timeline::default();
    for line in out.lines() {
        let mut f = line.splitn(3, '\t');
        let (Some(k), Some(at), Some(text)) = (f.next(), f.next(), f.next()) else { continue };
        let kind = match k {
            "title" => {
                t.title = text.to_string();
                continue;
            }
            "ask" => Kind::Ask,
            "say" => Kind::Say,
            "tool" => Kind::Tool,
            _ => continue,
        };
        t.entries.push(Entry { kind, at_ms: at.parse().unwrap_or(0), text: text.to_string() });
    }
    (!t.entries.is_empty() || !t.title.is_empty()).then_some(t)
}

/// Read the conversation in tmux pane `pane` on a host (`None`: local).
pub fn fetch(ssh: Option<&str>, pane: &str) -> Option<Timeline> {
    let mut c = match ssh {
        None => Command::new("python3"),
        Some(spec) => {
            let mut c = Command::new("ssh");
            c.args(crate::tmuxcc::ssh_args(spec)).arg(format!("python3 - {pane}"));
            c
        }
    };
    if ssh.is_none() {
        c.args(["-", pane]);
    }
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    child.stdin.take()?.write_all(SCRIPT.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    parse(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries() {
        let t = parse("title\t0\tFix rail\nask\t1792530373253\tdo the thing\ntool\t1792530373300\tBash ls\nsay\t0\tdone\tok\n").unwrap();
        assert_eq!(t.title, "Fix rail");
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.entries[0].kind, Kind::Ask);
        assert_eq!(t.entries[2].text, "done\tok");
        assert!(parse("").is_none());
    }
}
