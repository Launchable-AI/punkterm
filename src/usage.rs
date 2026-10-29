//! Claude plan usage: the bars behind /usage (5h, weekly, scoped weekly,
//! credits), per host, plus the width cc-rail wants its rails. Read the way cc-rail reads them, and only read: Claude
//! Code's cache in ~/.claude.json and cc-rail's own poll file, whichever is
//! newer. A tiny Python script does the reading on the host (fed on stdin,
//! so nothing needs quoting over ssh) and prints one line per window.

use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
import json, os, datetime, glob
home = os.path.expanduser('~')
def load(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return None
default = os.path.join(home, '.claude.json')
rail_home = os.environ.get('CC_RAIL_HOME', os.path.join(home, '.cc-rail'))
rail = load(os.path.join(rail_home, 'state', 'usage.json')) or {}
if os.path.isdir(rail_home):
    w = (load(os.path.join(rail_home, 'state', 'ui.json')) or {}).get('width') or (load(os.path.join(rail_home, 'config.json')) or {}).get('railWidth') or 34
    if isinstance(w, int): print('railw', w)
# Each account is a config: ~/.claude.json, or $CLAUDE_CONFIG_DIR/.claude.json
# for a Claude started with one (several accounts at once on one host).
cfgs = [default] + sorted(p for p in glob.glob(os.path.join(home, '.claude*', '.claude.json')))
def cfg_index(path):
    if path not in cfgs: cfgs.append(path)
    return cfgs.index(path)
# Which pane runs which config; and panes running `claude attach <job>`,
# which never set a terminal title, named after the background job.
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
            if not argv or 'claude' not in os.path.basename(argv[0]): continue
            p = pid
            for _ in range(5):
                if p in panes: break
                p = ppid(p)
            if p not in panes: continue
            with open('/proc/%s/environ' % pid, 'rb') as f:
                env = dict(e.split('=', 1) for e in f.read().decode(errors='replace').split('\0') if '=' in e)
            d = env.get('CLAUDE_CONFIG_DIR')
            cfg = os.path.join(os.path.expanduser(d), '.claude.json') if d else default
            print('pane', panes[p], cfg_index(cfg))
            if len(argv) >= 3 and argv[1] == 'attach':
                jobs = os.path.join(os.path.expanduser(d) if d else os.path.join(home, '.claude'), 'jobs')
                name = (load(os.path.join(jobs, os.path.basename(argv[2]), 'state.json')) or {}).get('name')
                if name: print('name', panes[p], ' '.join(str(name).split()))
        except Exception: pass
except Exception: pass
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
def account(i, cfg):
    # The config's own name: dev1 for ~/.claude-dev1, default for ~/.claude.
    base = os.path.basename(os.path.dirname(cfg)) if cfg != default else '.claude'
    print(i, 'cfg', base[len('.claude-'):] if base.startswith('.claude-') else 'default')
    conf = load(cfg) or {}
    oauth = conf.get('oauthAccount') or {}
    email = oauth.get('emailAddress')
    if not email: return
    print(i, 'acct', str(email).replace(' ', ''))
    # Claude Code's cache names the account it read; right after a /login it
    # still holds the last account's numbers, so it counts only once it names
    # this one. cc-rail's copy names none: it counts only where it is newer
    # than a cache that matches (it reads with whoever is signed in then).
    mine = conf.get('cachedUsageUtilization')
    uuid = oauth.get('accountUuid')
    if isinstance(mine, dict) and mine.get('accountUuid') and uuid and mine.get('accountUuid') != uuid:
        print(i, 'stale')
        return
    cands = [mine, (rail.get('accounts') or {}).get(cfg)]
    cands = [c for c in cands if isinstance(c, dict) and c.get('utilization')]
    if not cands: return
    best = max(cands, key=lambda c: c.get('fetchedAtMs') or 0)
    u = best['utilization']
    print(i, 'at', int(best.get('fetchedAtMs') or 0))
    lim = u.get('limits')
    if isinstance(lim, list) and lim:
        for l in lim:
            if isinstance(l, dict) and isinstance(l.get('percent'), num):
                print(i, 'row', label(l).replace(' ', '_'), round(l['percent']), ms(l.get('resets_at')), l.get('severity') or 'normal')
    else:
        for k, lab in (('five_hour', '5h'), ('seven_day', 'wk')):
            w = u.get(k)
            if isinstance(w, dict) and isinstance(w.get('utilization'), num):
                print(i, 'row', lab, round(w['utilization']), ms(w.get('resets_at')), 'normal')
    x = u.get('extra_usage')
    if isinstance(x, dict) and x.get('is_enabled') and isinstance(x.get('utilization'), num):
        print(i, 'row credit', round(x['utilization']), 0, x.get('severity') or 'normal')
for i, cfg in enumerate(cfgs):
    try: account(i, cfg)
    except Exception: pass
"#;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub label: String,
    pub pct: f32,
    /// Unix ms; 0 when the window has no reset time.
    pub resets_ms: u64,
    pub severity: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
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
    /// The host's cached usage is another account's (just after a /login):
    /// no rows until Claude Code reads this one's.
    pub is_stale: bool,
    /// Every account config on the host: the default (first) and each
    /// CLAUDE_CONFIG_DIR one, logged in or not.
    pub accounts: Vec<Acct>,
    /// Which config (index into `accounts`) each tmux pane's Claude runs.
    pub pane_account: std::collections::HashMap<String, usize>,
}

/// One account config on a host and its usage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Acct {
    /// The config's name: `default`, or `dev1` for `~/.claude-dev1`.
    pub config: String,
    /// The signed-in account's email (none: not logged in there).
    pub account: Option<String>,
    pub at_ms: u64,
    pub rows: Vec<Row>,
    pub is_stale: bool,
}

impl Acct {
    /// How a card or the footer names it: the email's name, else the config's.
    pub fn short(&self) -> String {
        match &self.account {
            Some(a) => a.split('@').next().unwrap_or(a).to_string(),
            None => self.config.clone(),
        }
    }
}

impl Usage {
    /// The account config pane `pane` runs, if known.
    pub fn account_of(&self, pane: &str) -> Option<&Acct> {
        self.pane_account.get(pane).and_then(|&i| self.accounts.get(i))
    }
}

/// An account punkterm has seen signed in on a host, with its last reading:
/// a host keeps only the account signed in now, so the others are known
/// from here. Saved in punkterm's state folder, as accounts.toml.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Known {
    pub account: String,
    /// When its reading was fetched (Unix ms), and the reading.
    #[serde(default)]
    pub at_ms: u64,
    #[serde(default)]
    pub rows: Vec<Row>,
    /// The hosts it was last seen signed in on, by name.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// When it was last seen signed in anywhere (Unix ms).
    #[serde(default)]
    pub seen_ms: u64,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct KnownFile {
    #[serde(default)]
    account: Vec<Known>,
}

fn known_path() -> Option<std::path::PathBuf> {
    dirs::state_dir().map(|d| d.join("punkterm").join("accounts.toml"))
}

pub fn load_known() -> Vec<Known> {
    let text = known_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    toml::from_str::<KnownFile>(&text).map(|f| f.account).unwrap_or_default()
}

pub fn save_known(known: &[Known]) {
    let Some(path) = known_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = toml::to_string(&KnownFile { account: known.to_vec() }) {
        let _ = std::fs::write(path, text);
    }
}

impl Usage {
    /// Every account config the host reported (the default alone, from an
    /// older report that names no configs).
    pub fn configs(&self) -> Vec<Acct> {
        if !self.accounts.is_empty() {
            return self.accounts.clone();
        }
        if self.account.is_none() && self.rows.is_empty() {
            return Vec::new();
        }
        vec![Acct { config: "default".into(), account: self.account.clone(), at_ms: self.at_ms, rows: self.rows.clone(), is_stale: self.is_stale }]
    }
}

/// Note what host `host` reported: each signed-in account seen now, and its
/// reading when newer than the one kept. True when anything changed.
pub fn remember(known: &mut Vec<Known>, host: &str, u: &Usage) -> bool {
    let mut changed = false;
    for a in u.configs() {
        changed |= remember_one(known, host, &a);
    }
    changed
}

fn remember_one(known: &mut Vec<Known>, host: &str, u: &Acct) -> bool {
    let Some(account) = u.account.clone() else { return false };
    let now = now_ms();
    let i = match known.iter().position(|k| k.account == account) {
        Some(i) => i,
        None => {
            known.push(Known { account, at_ms: 0, rows: Vec::new(), hosts: Vec::new(), seen_ms: now });
            known.len() - 1
        }
    };
    let k = &mut known[i];
    let mut changed = false;
    if !k.hosts.iter().any(|h| h == host) {
        k.hosts.push(host.to_string());
        changed = true;
    }
    if !u.is_stale && !u.rows.is_empty() && u.at_ms >= k.at_ms && (u.at_ms != k.at_ms || u.rows != k.rows) {
        k.at_ms = u.at_ms;
        k.rows = u.rows.clone();
        changed = true;
    }
    // Seen-now is refreshed on each change, or when it is an hour old.
    if changed || now.saturating_sub(k.seen_ms) > 3_600_000 {
        k.seen_ms = now;
        changed = true;
    }
    changed
}

pub fn parse(out: &str) -> Option<Usage> {
    let mut u = Usage {
        at_ms: 0,
        rows: Vec::new(),
        rail_width: None,
        account: None,
        names: Default::default(),
        is_stale: false,
        accounts: Vec::new(),
        pane_account: Default::default(),
    };
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        // "<n> <key> …": account config n's line.
        if let Some(i) = f.first().and_then(|n| n.parse::<usize>().ok()).filter(|&i| i < 64) {
            if u.accounts.len() <= i {
                u.accounts.resize(i + 1, Acct::default());
            }
            let a = &mut u.accounts[i];
            match &f[1..] {
                ["cfg", name] => a.config = name.to_string(),
                ["acct", e] => a.account = Some(e.to_string()),
                ["at", ms] => a.at_ms = ms.parse().unwrap_or(0),
                ["stale"] => a.is_stale = true,
                ["row", label, pct, resets, sev] => a.rows.push(Row {
                    label: label.replace('_', " "),
                    pct: pct.parse().unwrap_or(0.0),
                    resets_ms: resets.parse().unwrap_or(0),
                    severity: sev.to_string(),
                }),
                _ => {}
            }
            continue;
        }
        match f.as_slice() {
            ["pane", id, i] => {
                if let Ok(i) = i.parse() {
                    u.pane_account.insert(id.to_string(), i);
                }
            }
            ["at", ms] => u.at_ms = ms.parse().unwrap_or(0),
            ["acct", e] => u.account = Some(e.to_string()),
            ["name", pane, rest @ ..] if !rest.is_empty() => {
                u.names.insert(pane.to_string(), rest.join(" "));
            }
            ["stale"] => u.is_stale = true,
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
    // The default config is the host's account, as before.
    if let Some(d) = u.accounts.first().cloned() {
        u.account = u.account.take().or(d.account);
        u.at_ms = u.at_ms.max(d.at_ms);
        if u.rows.is_empty() {
            u.rows = d.rows;
        }
        u.is_stale |= d.is_stale;
    }
    (!u.rows.is_empty() || u.rail_width.is_some() || !u.names.is_empty() || u.account.is_some() || !u.accounts.is_empty()).then_some(u)
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
        // Account configs, numbered; the default's is the host's account.
        let u = parse("pane %4 1\n0 cfg default\n0 acct k@x\n0 at 5\n0 row 5h 30 0 normal\n1 cfg dev1\n1 acct d@x\n2 cfg dev2\n").unwrap();
        assert_eq!(u.account.as_deref(), Some("k@x"));
        assert_eq!(u.rows.len(), 1);
        assert_eq!(u.accounts.len(), 3);
        assert_eq!(u.account_of("%4").map(|a| a.short()), Some("d".into()));
        assert_eq!(u.accounts[2].short(), "dev2");
        let stale = parse("acct new@example.com\nstale\n").unwrap();
        assert!(stale.is_stale && stale.rows.is_empty());
        assert_eq!(stale.account.as_deref(), Some("new@example.com"));
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

    #[test]
    fn remembers_accounts() {
        let mut known = Vec::new();
        let row = |pct: f32| Row { label: "5h".into(), pct, resets_ms: 0, severity: "normal".into() };
        let a = Usage { at_ms: 10, rows: vec![row(40.0)], account: Some("a@x".into()), ..Default::default() };
        assert!(remember(&mut known, "server", &a));
        // A stale report (just after a /login) keeps the old reading.
        let stale = Usage { at_ms: 20, rows: vec![], is_stale: true, ..a.clone() };
        remember(&mut known, "server", &stale);
        assert_eq!(known[0].rows[0].pct, 40.0);
        // An older reading doesn't replace a newer one.
        let old = Usage { at_ms: 5, rows: vec![row(99.0)], ..a.clone() };
        remember(&mut known, "laptop", &old);
        assert_eq!(known[0].rows[0].pct, 40.0);
        assert_eq!(known[0].hosts, vec!["server".to_string(), "laptop".to_string()]);
        let b = Usage { account: Some("b@x".into()), ..a.clone() };
        remember(&mut known, "server", &b);
        assert_eq!(known.len(), 2);
    }
}
