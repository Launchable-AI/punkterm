//! What each Claude session's background work is doing, for the tasks rail:
//! its sub-agents and background shells, each with the description Claude
//! gave it, what it is doing now (an agent's last tool call, a shell's last
//! line of output), and whether it is running or finished. Read (only read)
//! from what Claude Code writes anyway: the session's transcript (shells
//! started, ids, completion notices), `subagents/agent-*.{jsonl,meta.json}`
//! beside it, and the shells' output files under /tmp/claude-<uid>.
//!
//! One Python script per host covers every tmux pane running Claude. Long
//! transcripts are read incrementally: what it has learned and how far it
//! read are kept in /tmp/punkterm-<uid>/tasks2-<session>.json on the host.

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
import glob, json, os, re, subprocess, time
home = os.path.expanduser('~')
now = int(time.time() * 1000)
KEEP_DONE = 30 * 60 * 1000
def load(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return None
dirs = [os.path.join(home, '.claude')] + sorted(d for d in glob.glob(os.path.join(home, '.claude-*')) if os.path.isdir(d))
out = subprocess.run(['tmux', 'list-panes', '-a', '-F', '#{pane_pid} #{pane_id}'], capture_output=True, text=True, timeout=5).stdout
panes = dict(l.split() for l in out.splitlines() if len(l.split()) == 2)
if not panes: raise SystemExit
kids = {}
for pid in os.listdir('/proc'):
    if not pid.isdigit(): continue
    try:
        with open('/proc/%s/stat' % pid) as f: kids.setdefault(f.read().rsplit(')', 1)[1].split()[1], []).append(pid)
    except Exception: pass
# Live sessions: sessions/<pid>.json of a running pid, in any account config.
live = {}
for d in dirs:
    for p in glob.glob(os.path.join(d, 'sessions', '*.json')):
        pid = os.path.basename(p)[:-5]
        if pid.isdigit() and os.path.exists('/proc/' + pid):
            s = load(p) or {}
            if s.get('sessionId'): live[pid] = s
def session_of(root):
    todo = [root]
    for _ in range(6):
        nxt = []
        for pid in todo:
            if pid in live: return live[pid]
            try:
                with open('/proc/%s/cmdline' % pid, 'rb') as f: argv = f.read().decode(errors='replace').split('\0')
            except Exception: argv = []
            if len(argv) >= 3 and 'claude' in os.path.basename(argv[0]) and argv[1] == 'attach':
                for d in dirs:
                    j = load(os.path.join(d, 'jobs', os.path.basename(argv[2]), 'state.json')) or {}
                    if j.get('sessionId'): return {'sessionId': j['sessionId'], 'name': j.get('name'), 'startedAt': 0}
            nxt += kids.get(pid, [])
        todo = nxt
    return None
from datetime import datetime
def ms(t):
    try: return int(datetime.fromisoformat(str(t).replace('Z', '+00:00')).timestamp() * 1000)
    except Exception: return 0
def flat(s, n=160): return ' '.join(str(s).split())[:n]
def arg(name, i):
    if not isinstance(i, dict): return ''
    pick = lambda *ks: next((str(i[k]) for k in ks if i.get(k)), '')
    if name in ('Read', 'Edit', 'Write', 'NotebookEdit'): return os.path.basename(pick('file_path'))
    if name == 'Bash': return pick('description', 'command')
    if name in ('WebFetch',): return pick('url')
    return pick('description', 'query', 'pattern', 'skill', 'command', 'prompt')
NOTE = re.compile(r'<task-id>([^<]+)</task-id>.*?<status>([^<]+)</status>', re.S)
ANSI = re.compile(r'\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\x1b.')
cache_dir = '/tmp/punkterm-%d' % os.getuid()
try: os.makedirs(cache_dir, mode=0o700, exist_ok=True)
except Exception: pass
def read_new(path, files, bash, shells, status, pending):
    """Fold what a transcript gained since the last run into what is known
    of the shells: Bash calls, background task ids, how they ended, and
    calls still waiting on their result (running in the foreground)."""
    try: size = os.path.getsize(path)
    except Exception: return
    off = files.get(path, 0)
    if off > size: off = 0
    with open(path, 'rb') as f:
        f.seek(off)
        data = f.read()
    end = data.rfind(b'\n') + 1
    files[path] = off + end
    for l in data[:end].split(b'\n'):
        if not (b'"Bash"' in l or b'tool_use_id' in l or b'task-notification' in l or b'TaskStop' in l or b'KillShell' in l): continue
        try: o = json.loads(l)
        except Exception: continue
        at = ms(o.get('timestamp'))
        # Completion notices come as a queued message, an attachment or a
        # user turn: read them from wherever they are on the line.
        if b'task-notification' in l:
            for tid, st in NOTE.findall(l.decode(errors='replace').replace('\\n', '\n')):
                status[tid.strip()] = [st.strip(), at]
        cont = (o.get('message') or {}).get('content')
        if o.get('type') == 'assistant' and isinstance(cont, list):
            for b in cont:
                if not isinstance(b, dict) or b.get('type') != 'tool_use': continue
                i = b.get('input') or {}
                if b.get('name') == 'Bash':
                    bash[b.get('id')] = [flat(i.get('description') or i.get('command') or 'shell'), at]
                    pending[b.get('id')] = [flat(i.get('description') or i.get('command') or 'shell'), at]
                elif b.get('name') in ('TaskStop', 'KillShell'):
                    tid = i.get('task_id') or i.get('shell_id')
                    if tid: status[tid] = ['stopped', at]
        elif o.get('type') == 'user' and isinstance(cont, list):
            r = o.get('toolUseResult')
            for b in cont:
                if not isinstance(b, dict) or b.get('type') != 'tool_result': continue
                pending.pop(b.get('tool_use_id'), None)
                if isinstance(r, dict) and r.get('backgroundTaskId') and b.get('tool_use_id') in bash:
                    shells[r['backgroundTaskId']] = bash[b['tool_use_id']]
def scan(sid):
    """What the session's transcript, and its agents' transcripts, say of
    its shells, read from where the last run stopped."""
    paths = {}
    for d in dirs:
        for p in glob.glob(os.path.join(d, 'projects', '*', sid + '.jsonl')):
            r = os.path.realpath(p)
            paths[r] = os.path.dirname(p)
    if not paths: return None, None
    path = max(paths, key=os.path.getmtime)
    sdir = os.path.join(paths[path], sid)
    cpath = os.path.join(cache_dir, 'tasks2-%s.json' % sid)
    c = load(cpath) or {}
    if c.get('path') != path or 'files' not in c: c = {}
    files = c.get('files', {})      # transcript -> how far read
    bash = c.get('bash', {})        # tool_use id -> [description, at]
    shells = c.get('shells', {})    # task id -> [description, at]
    status = c.get('status', {})    # task id -> [status, at]
    pending = c.get('pending', {})  # tool_use id -> [description, at]
    # Agents start shells of their own, recorded in their transcripts.
    for p in [path] + sorted(glob.glob(os.path.join(sdir, 'subagents', 'agent-*.jsonl'))):
        read_new(p, files, bash, shells, status, pending)
    # Keep the cache small: recent Bash calls only.
    if len(bash) > 400: bash = dict(sorted(bash.items(), key=lambda kv: kv[1][1])[-200:])
    pending = {k: v for k, v in pending.items() if now - (v[1] or now) < 86400000}
    c = {'path': path, 'files': files, 'bash': bash, 'shells': shells, 'status': status, 'pending': pending}
    try:
        tmp = cpath + '.tmp'
        with open(tmp, 'w') as f: json.dump(c, f)
        os.replace(tmp, cpath)
    except Exception: pass
    return c, sdir
EXIT = re.compile(r'^\[(exited with code (\d+)|killed)\]$')
LOG = 12
def tail_lines(p, n=LOG):
    """A shell's last lines of output, as a terminal would leave them."""
    try:
        with open(p, 'rb') as f:
            f.seek(0, 2); size = f.tell(); f.seek(max(0, size - 8192))
            s = f.read().decode(errors='replace')
    except Exception: return []
    out = []
    for l in s.split('\n'):
        l = ANSI.sub('', l.split('\r')[-1] if l.rstrip('\r') else l).rstrip()
        if l.strip(): out.append(flat(l, 240))
    return out[-n:]
def last_line(p):
    ls = tail_lines(p, 1)
    return ls[0] if ls else ''
def agent_log(p, n=LOG):
    """An agent's last steps, oldest first: (tool|say, at, text)."""
    try:
        with open(p, 'rb') as f:
            f.seek(0, 2); size = f.tell(); f.seek(max(0, size - 196608))
            ls = f.read().decode(errors='replace').split('\n')
    except Exception: return []
    ev = []
    for l in reversed(ls):
        if len(ev) >= n: break
        try: o = json.loads(l)
        except Exception: continue
        cont = (o.get('message') or {}).get('content')
        if o.get('type') != 'assistant' or not isinstance(cont, list): continue
        at = ms(o.get('timestamp'))
        for b in reversed(cont):
            if not isinstance(b, dict): continue
            if b.get('type') == 'tool_use': ev.append(('tool', at, flat(b.get('name', '?') + ' ' + arg(b.get('name'), b.get('input')), 240)))
            elif b.get('type') == 'text' and b.get('text', '').strip(): ev.append(('say', at, flat(b['text'], 400)))
    return ev[:n][::-1]
def agent_now(log):
    return log[-1][2] if log else ''
def log_out(pane, tid, ev):
    for k, at, t in ev: print('\t'.join(['D', pane, tid, k, str(at), t.replace('\t', ' ')]))
def first_at(p):
    try:
        with open(p) as f:
            for l in f:
                t = ms((json.loads(l) or {}).get('timestamp'))
                if t: return t
    except Exception: pass
    return 0
def emit(pane, kind, tid, st, start, end, desc, doing):
    print('\t'.join(['T', pane, kind, tid, st, str(start), str(end), flat(desc).replace('\t', ' '), flat(doing, 200).replace('\t', ' ')]))
# Output files a live process still writes: those shells are running.
writing = set()
for pid in os.listdir('/proc'):
    if not pid.isdigit(): continue
    try: fds = os.listdir('/proc/%s/fd' % pid)
    except Exception: continue
    for fd in fds:
        try: t = os.readlink('/proc/%s/fd/%s' % (pid, fd))
        except Exception: continue
        if t.endswith('.output') and '/tasks/' in t: writing.add(t)
seen = set()
for root, pane in panes.items():
    s = session_of(root)
    if not s: continue
    sid = s['sessionId']
    if sid in seen: continue
    seen.add(sid)
    started = int(s.get('startedAt') or 0)
    print('\t'.join(['P', pane, sid, flat(s.get('name') or '', 80)]))
    try: c, sdir = scan(sid)
    except Exception: c, sdir = None, None
    if not c: continue
    status = c['status']
    # Sub-agents, from their own transcripts beside the session's.
    for meta in glob.glob(os.path.join(sdir, 'subagents', 'agent-*.meta.json')):
        aid = os.path.basename(meta)[len('agent-'):-len('.meta.json')]
        m = load(meta) or {}
        jl = meta[:-len('.meta.json')] + '.jsonl'
        try: mt = int(os.path.getmtime(jl) * 1000)
        except Exception: mt = 0
        st = status.get(aid)
        if st: state, end = st[0], st[1] or mt
        elif m.get('finished'): state, end = 'completed', mt
        else: state, end = 'running', 0
        start = first_at(jl)
        # Left running by an earlier run of this session: not running now.
        if state == 'running' and started and start and start < started: state, end = 'stopped', mt
        if state != 'running' and now - end > KEEP_DONE: continue
        log = agent_log(jl)
        emit(pane, 'agent', aid, state, start, end, m.get('description') or m.get('agentType') or 'agent', agent_now(log) if state == 'running' else '')
        log_out(pane, aid, log)
    tdirs = glob.glob('/tmp/claude-*/*/%s/tasks' % sid)
    for tid, (desc, start) in c['shells'].items():
        st = status.get(tid)
        state, end = (st[0], st[1]) if st else ('running', 0)
        if state == 'running' and started and start and start < started: state, end = 'stopped', start
        if state != 'running' and now - end > KEEP_DONE: continue
        out = next((os.path.join(d, tid + '.output') for d in tdirs if os.path.exists(os.path.join(d, tid + '.output'))), None)
        line = last_line(out) if out else ''
        # The shell's own last words, where no notice was recorded.
        x = EXIT.match(line)
        if x and state == 'running':
            state = 'stopped' if x.group(1) == 'killed' else ('completed' if x.group(2) == '0' else 'failed')
            try: end = int(os.path.getmtime(out) * 1000)
            except Exception: end = now
        # Nothing writes its output any more: it ended, unannounced.
        elif state == 'running' and out and os.path.realpath(out) not in writing:
            state = 'stopped'
            try: end = int(os.path.getmtime(out) * 1000)
            except Exception: end = now
        if state != 'running' and now - end > KEEP_DONE: continue
        emit(pane, 'shell', tid, state, start, end, desc, line)
        if out: log_out(pane, tid, [('out', 0, x) for x in tail_lines(out)])
    # Output a live process still writes that no background call names: the
    # command running in the foreground (its call still waiting on a result).
    waiting = sorted(c['pending'].values(), key=lambda v: v[1], reverse=True)
    for d in tdirs:
        for out in glob.glob(os.path.join(d, '*.output')):
            tid = os.path.basename(out)[:-len('.output')]
            if tid in c['shells'] or os.path.realpath(out) not in writing: continue
            desc, start = waiting.pop(0) if waiting else ('Shell', 0)
            if not start:
                try: start = int(os.path.getctime(out) * 1000)
                except Exception: start = now
            emit(pane, 'shell', tid, 'running', start, 0, desc, last_line(out))
            log_out(pane, tid, [('out', 0, x) for x in tail_lines(out)])
"#;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Agent,
    Shell,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed,
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub kind: Kind,
    pub id: String,
    pub state: State,
    /// Unix ms; 0 when unknown.
    pub start_ms: u64,
    pub end_ms: u64,
    /// What Claude said it is for.
    pub desc: String,
    /// What it is doing now: an agent's last tool call, a shell's last line.
    pub doing: String,
    /// Its recent steps, oldest first, for the detail view: an agent's tool
    /// calls and messages, a shell's last lines of output.
    pub log: Vec<Step>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StepKind {
    Tool,
    Say,
    Out,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub kind: StepKind,
    /// Unix ms; 0 for a line of output.
    pub at_ms: u64,
    pub text: String,
}

/// One session's background work.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub session_id: String,
    pub tasks: Vec<Task>,
}

impl Session {
    pub fn running(&self, kind: Kind) -> usize {
        self.tasks.iter().filter(|t| t.kind == kind && t.state == State::Running).count()
    }
}

/// Every Claude session on a host, by tmux pane id.
pub type Host = HashMap<String, Session>;

pub fn parse(out: &str) -> Host {
    let mut host = Host::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f[..] {
            ["P", pane, sid, _] => {
                host.insert(pane.to_string(), Session { session_id: sid.to_string(), tasks: Vec::new() });
            }
            ["T", pane, kind, id, state, start, end, desc, doing] => {
                let Some(s) = host.get_mut(pane) else { continue };
                let kind = if kind == "agent" { Kind::Agent } else { Kind::Shell };
                let state = match state {
                    "running" => State::Running,
                    "completed" | "done" => State::Done,
                    "stopped" | "killed" => State::Stopped,
                    _ => State::Failed,
                };
                s.tasks.push(Task {
                    kind,
                    id: id.to_string(),
                    state,
                    start_ms: start.parse().unwrap_or(0),
                    end_ms: end.parse().unwrap_or(0),
                    desc: desc.to_string(),
                    doing: doing.to_string(),
                    log: Vec::new(),
                });
            }
            ["D", pane, id, kind, at, text] => {
                let Some(t) = host.get_mut(pane).and_then(|s| s.tasks.iter_mut().rfind(|t| t.id == id)) else { continue };
                let kind = match kind {
                    "tool" => StepKind::Tool,
                    "say" => StepKind::Say,
                    _ => StepKind::Out,
                };
                t.log.push(Step { kind, at_ms: at.parse().unwrap_or(0), text: text.to_string() });
            }
            _ => {}
        }
    }
    // Running first (newest first), then the rest by when they ended.
    for s in host.values_mut() {
        s.tasks.sort_by_key(|t| (t.state != State::Running, std::cmp::Reverse(if t.state == State::Running { t.start_ms } else { t.end_ms })));
    }
    host
}

/// Read a host's sessions' background work (`None`: this machine).
pub fn fetch(ssh: Option<&str>) -> Option<Host> {
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
    out.status.success().then(|| parse(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sessions_and_tasks() {
        let h = parse(
            "P\t%1\tabc\tname\n\
             T\t%1\tshell\tb1\tcompleted\t100\t200\tRun tests\t42 passed\n\
             T\t%1\tagent\ta1\trunning\t150\t0\tResearch\tBash ls\n\
             D\t%1\ta1\ttool\t160\tBash ls\n\
             D\t%1\ta1\tsay\t170\tFound it\n\
             T\t%9\tagent\tx\trunning\t1\t0\tno session\t\n\
             P\t%2\tdef\t\n",
        );
        assert_eq!(h.len(), 2);
        let s = &h["%1"];
        assert_eq!(s.session_id, "abc");
        assert_eq!(s.tasks[0].id, "a1", "running first");
        assert_eq!(s.tasks[1].state, State::Done);
        assert_eq!((s.running(Kind::Agent), s.running(Kind::Shell)), (1, 0));
        assert!(h["%2"].tasks.is_empty());
        assert_eq!(s.tasks[0].log.len(), 2);
        assert_eq!((s.tasks[0].log[1].kind, s.tasks[0].log[1].text.as_str()), (StepKind::Say, "Found it"));
    }
}
