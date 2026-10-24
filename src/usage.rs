//! Claude plan usage: the bars behind /usage (5h, weekly, scoped weekly,
//! credits), per host, plus the width cc-rail wants its rails. Read the way cc-rail reads them, and only read: Claude
//! Code's cache in ~/.claude.json and cc-rail's own poll file, whichever is
//! newer. A tiny Python script does the reading on the host (fed on stdin,
//! so nothing needs quoting over ssh) and prints one line per window.

use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
import json, os, datetime
home = os.path.expanduser('~')
def load(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return None
cfg = os.path.join(home, '.claude.json')
rail_home = os.environ.get('CC_RAIL_HOME', os.path.join(home, '.cc-rail'))
rail = os.path.join(rail_home, 'state', 'usage.json')
if os.path.isdir(rail_home):
    w = (load(os.path.join(rail_home, 'state', 'ui.json')) or {}).get('width') or (load(os.path.join(rail_home, 'config.json')) or {}).get('railWidth') or 34
    if isinstance(w, int): print('railw', w)
# Panes running `claude attach <job>` never set a terminal title; name them
# after the background job they show.
try:
    import subprocess
    out = subprocess.run(['tmux', 'list-panes', '-a', '-F', '#{pane_pid} #{pane_id}'], capture_output=True, text=True, timeout=5).stdout
    panes = dict(l.split() for l in out.splitlines() if len(l.split()) == 2)
    def ppid(pid):
        with open('/proc/%s/stat' % pid) as f: return f.read().rsplit(')', 1)[1].split()[1]
    for pid in os.listdir('/proc') if panes else []:
        if not pid.isdigit(): continue
        try:
            with open('/proc/%s/cmdline' % pid, 'rb') as f: argv = f.read().decode(errors='replace').split('\0')
            if len(argv) < 3 or 'claude' not in os.path.basename(argv[0]) or argv[1] != 'attach': continue
            job = os.path.basename(argv[2])
            name = (load(os.path.join(home, '.claude', 'jobs', job, 'state.json')) or {}).get('name')
            p = pid
            for _ in range(4):
                if p in panes: break
                p = ppid(p)
            if name and p in panes: print('name', panes[p], ' '.join(str(name).split()))
        except Exception: pass
except Exception: pass
conf = load(cfg) or {}
email = (conf.get('oauthAccount') or {}).get('emailAddress')
if email: print('acct', str(email).replace(' ', ''))
cands = [conf.get('cachedUsageUtilization'), ((load(rail) or {}).get('accounts') or {}).get(cfg)]
cands = [c for c in cands if isinstance(c, dict) and c.get('utilization')]
if not cands: raise SystemExit
best = max(cands, key=lambda c: c.get('fetchedAtMs') or 0)
u = best['utilization']
print('at', int(best.get('fetchedAtMs') or 0))
def ms(s):
    try: return int(datetime.datetime.fromisoformat(str(s).replace('Z', '+00:00')).timestamp() * 1000)
    except Exception: return 0
KIND = {'session': '5h', 'weekly_all': 'wk'}
def label(l):
    if l.get('kind') in KIND: return KIND[l['kind']]
    sc = l.get('scope') or {}
    for k in ('model', 'surface'):
        d = (sc.get(k) or {}).get('display_name')
        if d: return str(d).lower()
    return 'wk*' if l.get('kind') == 'weekly_scoped' else str(l.get('kind') or '')[:6]
num = (int, float)
lim = u.get('limits')
if isinstance(lim, list) and lim:
    for l in lim:
        if isinstance(l, dict) and isinstance(l.get('percent'), num):
            print('row', label(l).replace(' ', '_'), round(l['percent']), ms(l.get('resets_at')), l.get('severity') or 'normal')
else:
    for k, lab in (('five_hour', '5h'), ('seven_day', 'wk')):
        w = u.get(k)
        if isinstance(w, dict) and isinstance(w.get('utilization'), num):
            print('row', lab, round(w['utilization']), ms(w.get('resets_at')), 'normal')
x = u.get('extra_usage')
if isinstance(x, dict) and x.get('is_enabled') and isinstance(x.get('utilization'), num):
    print('row credit', round(x['utilization']), 0, x.get('severity') or 'normal')
"#;

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub pct: f32,
    /// Unix ms; 0 when the window has no reset time.
    pub resets_ms: u64,
    pub severity: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    /// When Claude Code (or cc-rail) fetched it, Unix ms.
    pub at_ms: u64,
    pub rows: Vec<Row>,
    /// The width cc-rail keeps its rails at on this host, if it is installed.
    pub rail_width: Option<usize>,
    /// The signed-in Claude account's email.
    pub account: Option<String>,
    /// Pane id -> session name, for panes attached to a background job.
    pub names: std::collections::HashMap<String, String>,
}

pub fn parse(out: &str) -> Option<Usage> {
    let mut u = Usage { at_ms: 0, rows: Vec::new(), rail_width: None, account: None, names: Default::default() };
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        match f.as_slice() {
            ["at", ms] => u.at_ms = ms.parse().unwrap_or(0),
            ["acct", e] => u.account = Some(e.to_string()),
            ["name", pane, rest @ ..] if !rest.is_empty() => {
                u.names.insert(pane.to_string(), rest.join(" "));
            }
            ["railw", w] => u.rail_width = w.parse().ok().filter(|w| (8..400).contains(w)),
            ["row", label, pct, resets, sev] => u.rows.push(Row {
                label: label.replace('_', " "),
                pct: pct.parse().unwrap_or(0.0),
                resets_ms: resets.parse().unwrap_or(0),
                severity: sev.to_string(),
            }),
            _ => {}
        }
    }
    (!u.rows.is_empty() || u.rail_width.is_some() || !u.names.is_empty()).then_some(u)
}

/// Read a host's plan usage (`None`: local). `None` if there is none to show.
pub fn fetch(ssh: Option<&str>) -> Option<Usage> {
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
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    child.stdin.take()?.write_all(SCRIPT.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    parse(&String::from_utf8_lossy(&out.stdout))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// "2H14" / "3D" / "41M": time left until `ms`.
pub fn until(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let m = ms.saturating_sub(now_ms()) / 60_000;
    if m >= 48 * 60 {
        format!("{}D", m / (24 * 60))
    } else if m >= 60 {
        format!("{}H{:02}", m / 60, m % 60)
    } else {
        format!("{m}M")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows() {
        let u = parse("at 1792530373253\nrow 5h 42 1792540000000 normal\nrow fable_5 7 0 warning\n").unwrap();
        assert_eq!(u.at_ms, 1792530373253);
        assert_eq!(u.rows.len(), 2);
        assert_eq!(u.rows[1].label, "fable 5");
        assert_eq!(u.rows[0].pct, 42.0);
        assert!(parse("").is_none());
        assert_eq!(parse("railw 56\n").unwrap().rail_width, Some(56));
        assert_eq!(parse("name %138 arch design\n").unwrap().names["%138"], "arch design");
        assert_eq!(parse("acct me@x.io\nrow 5h 1 0 normal\n").unwrap().account.as_deref(), Some("me@x.io"));
    }

    #[test]
    fn script_reads_local_files() {
        // Runs the real script; only checks it exits cleanly with valid lines.
        if let Some(u) = fetch(None) {
            assert!(u.rows.iter().all(|r| !r.label.is_empty()));
        }
    }
}
