//! REFLOW: quit the Claude Code in a tmux pane and resume the same session
//! in its place, so it prints its transcript afresh at the pane's width
//! (Claude hard-wraps what it prints, and a width change leaves its older
//! output squashed in the scrollback). A Python script on the pane's host
//! finds the Claude process under the pane, its session
//! (`<config>/sessions/<pid>.json`), account config, folder and the flags
//! worth keeping; stops it (SIGTERM); clears the pane's tmux history; and
//! types `claude --resume <id>` into the pane's shell (or, with no shell
//! there, respawns the pane with it).

use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
import glob, json, os, shlex, signal, subprocess, sys, time
pane = sys.argv[1]
home = os.path.expanduser('~')
def out(*a):
    print('\t'.join(a)); raise SystemExit
def tmux(*a): return subprocess.run(['tmux'] + list(a), capture_output=True, text=True, timeout=5).stdout.strip()
def load(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return None
dirs = [os.path.join(home, '.claude')] + sorted(d for d in glob.glob(os.path.join(home, '.claude-*')) if os.path.isdir(d))
root = tmux('display', '-p', '-t', pane, '#{pane_pid}')
if not root.isdigit(): out('err', 'pane is gone')
kids = {}
for pid in os.listdir('/proc'):
    if not pid.isdigit(): continue
    try:
        with open('/proc/%s/stat' % pid) as f: kids.setdefault(f.read().rsplit(')', 1)[1].split()[1], []).append(pid)
    except Exception: pass
found, todo = None, [root]
for _ in range(6):
    nxt = []
    for pid in todo:
        for d in dirs:
            s = load(os.path.join(d, 'sessions', pid + '.json'))
            if s and s.get('sessionId'): found = (pid, s, d); break
        if found: break
        nxt += kids.get(pid, [])
    if found or not nxt: break
    todo = nxt
if not found: out('err', 'no claude session in this pane')
pid, s, cfg = found
sid = s['sessionId']
# Nothing said yet: no transcript to resume, so leave it running.
if not any(glob.glob(os.path.join(d, 'projects', '*', sid + '.jsonl')) for d in dirs): out('err', 'nothing to resume yet')
try: cwd = os.readlink('/proc/%s/cwd' % pid)
except Exception: cwd = s.get('cwd') or ''
try:
    with open('/proc/%s/environ' % pid, 'rb') as f:
        env = dict(kv.split('=', 1) for kv in f.read().decode(errors='replace').split('\0') if '=' in kv)
except Exception: env = {}
try:
    with open('/proc/%s/cmdline' % pid, 'rb') as f: argv = f.read().decode(errors='replace').split('\0')
except Exception: argv = []
# Flags worth keeping across the restart; never a prompt, --resume, --continue.
BARE = {'--dangerously-skip-permissions', '--allow-dangerously-skip-permissions', '--verbose', '--chrome', '--ide'}
VALUED = {'--model', '--permission-mode', '--add-dir', '--settings', '--mcp-config', '--plugin-dir', '--agent', '--fallback-model'}
keep, i = [], 0
while i < len(argv):
    a = argv[i]
    if a in BARE: keep.append(a)
    elif a.split('=', 1)[0] in VALUED:
        if '=' in a: keep.append(a)
        elif i + 1 < len(argv): keep += [a, argv[i + 1]]; i += 1
    i += 1
q = shlex.quote
cmd = ''
if cwd: cmd += 'cd %s && ' % q(cwd)
if env.get('CLAUDE_CONFIG_DIR') or cfg != dirs[0]: cmd += 'CLAUDE_CONFIG_DIR=%s ' % q(env.get('CLAUDE_CONFIG_DIR') or cfg)
cmd += 'claude --resume %s' % q(sid) + ''.join(' ' + q(k) for k in keep)
if pid == root:
    # Claude is the pane's own process: no shell to type into.
    shell = env.get('SHELL') or os.environ.get('SHELL') or 'sh'
    tmux('respawn-pane', '-k', '-t', pane, shell, '-ic', cmd)
    tmux('clear-history', '-t', pane)
    out('ok', sid)
os.kill(int(pid), signal.SIGTERM)
for _ in range(80):
    if not os.path.exists('/proc/' + pid): break
    time.sleep(0.1)
else: out('err', 'claude did not quit')
time.sleep(0.4)
tmux('send-keys', '-t', pane, '-l', 'clear && ' + cmd)
tmux('clear-history', '-t', pane)
tmux('send-keys', '-t', pane, 'Enter')
out('ok', sid)
"#;

/// Quit and resume the Claude in tmux pane `pane` on the host behind `ssh`
/// (`None`: local). Ok: the session id. Blocks while Claude quits.
pub fn run(ssh: Option<&str>, pane: &str) -> Result<String, String> {
    let mut c = match ssh {
        None => {
            let mut c = Command::new("python3");
            c.args(["-", pane]);
            c
        }
        Some(spec) => {
            let mut c = Command::new("ssh");
            c.args(crate::tmuxcc::ssh_args(spec)).arg(format!("python3 - {pane}"));
            c
        }
    };
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| "python3 did not start".to_string())?;
    child.stdin.take().ok_or("no stdin")?.write_all(SCRIPT.as_bytes()).map_err(|_| "could not reach host".to_string())?;
    let out = child.wait_with_output().map_err(|_| "could not reach host".to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    match text.trim().split_once('\t') {
        Some(("ok", sid)) => Ok(sid.to_string()),
        Some(("err", why)) => Err(why.to_string()),
        _ => Err("could not reach host".into()),
    }
}
