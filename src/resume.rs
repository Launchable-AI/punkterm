//! Recent Claude Code sessions on a host, for the resume menu: every account
//! config's (~/.claude, ~/.claude-dev1, ...) transcripts in projects/, newest
//! first, each with its title, folder, branch, first and last prompt, and
//! whether a Claude still has it open. Read (only read) by a Python script
//! over the host's ssh connection, a little from each file's start and end.

use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
import glob, json, os
home = os.path.expanduser('~')
N = 40
def load(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return None
dirs = [os.path.join(home, '.claude')] + sorted(d for d in glob.glob(os.path.join(home, '.claude-*')) if os.path.isdir(d))
# Sessions a running Claude has open (sessions/<pid>.json of a live pid),
# and under which config: the only way to tell, where the configs share
# their projects/ (symlinked) and so the transcripts.
open_cfg = {}
for d in dirs:
    for p in glob.glob(os.path.join(d, 'sessions', '*.json')):
        pid = os.path.basename(p)[:-5]
        if pid.isdigit() and os.path.exists('/proc/' + pid):
            s = load(p) or {}
            if s.get('sessionId'): open_cfg[s['sessionId']] = d
files, seen = [], set()
for d in dirs:
    for f in glob.glob(os.path.join(d, 'projects', '*', '*.jsonl')):
        real = os.path.realpath(f)
        if real in seen: continue
        seen.add(real)
        try: files.append((os.path.getmtime(real), real))
        except OSError: pass
files.sort(reverse=True)
def text(m):
    c = (m or {}).get('content')
    if isinstance(c, str): return c
    if isinstance(c, list):
        return ' '.join(b.get('text', '') for b in c if isinstance(b, dict) and b.get('type') == 'text')
    return ''
def prompt(d):
    if d.get('type') != 'user' or d.get('isMeta') or d.get('isSidechain'): return None
    t = ' '.join(text(d.get('message')).split())
    # Not a command's echo, a tool result or a reminder.
    if not t or t.startswith('<') or t.startswith('Caveat:') or t.startswith('[Request interrupted'): return None
    return t[:240]
def lines(f, start, size):
    with open(f, 'rb') as fh:
        fh.seek(start)
        chunk = fh.read(size).decode(errors='replace').split('\n')
    if start: chunk = chunk[1:]
    for l in chunk:
        try: yield json.loads(l)
        except Exception: pass
out = 0
for mtime, f in files:
    if out >= N: break
    try:
        size = os.path.getsize(f)
        sid = os.path.basename(f)[:-6]
        d = open_cfg.get(sid, '')
        r = {'id': sid, 'mtime': int(mtime * 1000),
             'config': '' if not d or d.endswith('/.claude') else d, 'cwd': '', 'branch': '', 'title': '', 'first': '', 'last': ''}
        for e in lines(f, 0, 262144):
            r['cwd'] = r['cwd'] or e.get('cwd') or ''
            r['branch'] = r['branch'] or e.get('gitBranch') or ''
            if not r['first']: r['first'] = prompt(e) or ''
        for e in lines(f, max(0, size - 262144), 262144):
            t = e.get('type')
            if t in ('custom-title', 'ai-title', 'agent-name'):
                v = e.get('customTitle') or e.get('aiTitle') or e.get('agentName')
                # A title you gave beats one Claude made up.
                if v and (t == 'custom-title' or not r.get('custom')):
                    r['title'] = ' '.join(str(v).split())[:120]
                    r['custom'] = t == 'custom-title'
            p = prompt(e)
            if p: r['last'] = p
            if e.get('cwd'): r['cwd'] = e['cwd']
            if e.get('gitBranch'): r['branch'] = e['gitBranch']
        if not (r['first'] or r['last'] or r['title']): continue
        r['open'] = '1' if sid in open_cfg else ''
        print('\t'.join(str(r[k]).replace('\t', ' ') for k in ('id', 'mtime', 'config', 'cwd', 'branch', 'title', 'first', 'last', 'open')))
        out += 1
    except Exception: pass
"#;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recent {
    pub id: String,
    /// When the transcript was last written, Unix ms.
    pub mtime: u64,
    /// The account config folder the Claude that has it open runs (empty:
    /// the default ~/.claude, or not open).
    pub config: String,
    pub cwd: String,
    pub branch: String,
    /// Its name (`/rename`), or the one Claude gave it.
    pub title: String,
    pub first: String,
    pub last: String,
    /// A Claude has it open right now.
    pub open: bool,
}

impl Recent {
    /// The account config's short name: `dev1` for ~/.claude-dev1.
    pub fn config_name(&self) -> Option<&str> {
        let base = self.config.rsplit('/').next().filter(|b| !b.is_empty())?;
        Some(base.strip_prefix(".claude-").unwrap_or(base))
    }

    /// What to type in a shell on the host to resume it.
    pub fn command(&self) -> String {
        let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let mut cmd = String::new();
        if !self.cwd.is_empty() {
            cmd.push_str(&format!("cd {} && ", q(&self.cwd)));
        }
        if !self.config.is_empty() {
            cmd.push_str(&format!("CLAUDE_CONFIG_DIR={} ", q(&self.config)));
        }
        cmd.push_str(&format!("claude --resume {}", self.id));
        cmd
    }
}

/// The host's recent sessions (`None`: local), newest first.
pub fn fetch(ssh: Option<&str>) -> Result<Vec<Recent>, String> {
    let mut c = match ssh {
        None => Command::new("python3"),
        Some(spec) => {
            let mut c = Command::new("ssh");
            c.args(crate::tmuxcc::ssh_args(spec)).arg("python3 -");
            c
        }
    };
    if ssh.is_none() {
        c.arg("-");
    }
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    child.stdin.take().ok_or("no stdin")?.write_all(SCRIPT.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("could not read the host's sessions".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().filter_map(parse).collect())
}

/// One tab-separated line of the script's: id, mtime, config, cwd, branch,
/// title, first prompt, last prompt, open.
fn parse(line: &str) -> Option<Recent> {
    let f: Vec<&str> = line.split('\t').collect();
    let [id, mtime, config, cwd, branch, title, first, last, open] = f[..] else { return None };
    Some(Recent {
        id: id.to_string(),
        mtime: mtime.parse().ok()?,
        config: config.to_string(),
        cwd: cwd.to_string(),
        branch: branch.to_string(),
        title: title.to_string(),
        first: first.to_string(),
        last: last.to_string(),
        open: open == "1",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines() {
        let r = parse("abc\t1700\t/h/.claude-dev1\t/h/x\tmain\tFix it\tfirst\tlast\t1").unwrap();
        assert_eq!((r.id.as_str(), r.mtime, r.open, r.config_name()), ("abc", 1700, true, Some("dev1")));
        assert!(parse("too\tfew").is_none());
    }

    #[test]
    fn resume_command() {
        let r = Recent { id: "abc".into(), cwd: "/home/u/it's".into(), config: "/home/u/.claude-dev1".into(), ..Default::default() };
        assert_eq!(r.config_name(), Some("dev1"));
        assert_eq!(r.command(), "cd '/home/u/it'\\''s' && CLAUDE_CONFIG_DIR='/home/u/.claude-dev1' claude --resume abc");
        let d = Recent { id: "x".into(), ..Default::default() };
        assert_eq!(d.config_name(), None);
        assert_eq!(d.command(), "claude --resume x");
    }
}
