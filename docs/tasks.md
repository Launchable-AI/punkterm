# Tasks rail

Code: `src/tasks.rs` (reading a host's sessions' background work),
`App::poll_tasks` / `tasks_of` in `src/ui.rs`, `tasks_rail` in
`src/ui/chrome.rs`.

**Ctrl+Shift+K** (or a click on the strip) opens the tasks rail, left of
the timeline: what the selected session's **sub-agents** and **background
shells** are doing. A card in the session rail says "2 agents · 3 shells";
this says which, and what each is up to.

Each task shows:

- **AGENT** (round dot) or **SHELL** (square dot), filled while running.
- What it is for: the description Claude gave it when it started it
  ("Run full verify in background", "Evaluate the router options").
- What it is doing now, after `›`: an agent's last tool call
  (`Bash npm run verify`, `Read router.ts`) or a shell's last line of
  output.
- How long it has been running; or, once ended, **DONE**, **FAILED** (red,
  with its last line) or **STOPPED**, and how long ago. Ended tasks stay for
  30 minutes; Claude Code's own `← N agents` count keeps finished agents
  for longer, so it can be higher than what runs.

**Detail.** Click a task (its tag shows `+`) to open its recent steps
under it, live: an agent's last dozen tool calls (`▸ Bash Check progress`,
the newest lit) and what it said, oldest first; a shell's last lines of
output (a failed one's last line in red). Click again (`−`) to close it. The
**BRIEF / DETAIL** toggle in the header opens every task at once (clicking
one then closes just that one); it is remembered. The list scrolls with the
wheel when it runs long (`↑ MORE` / `↓ MORE`).

**Width.** Drag the gap on the rail's left edge (the pointer turns into a
resize arrow) to make it wider or narrower, from 240 px up to what leaves
the terminal about 560 px; double-click there for the default (330). The
width is remembered.

Running tasks come first, newest first. Below them, **ELSEWHERE** lists the
other sessions with work running (`1 agent · 2 shells`); click one to switch
to it. Collapsed, the rail is a strip with the selected session's running
count and a tick per task (agents long, shells short).

## Where it comes from

Nothing is guessed or summarised by a model: it is what Claude Code writes
anyway, read (only read) on each host by a Python script over the host's
ssh connection, every 4 s while the rail is open and every 20 s while it is
a strip:

- **Which session a pane runs**: the Claude process under the tmux pane and
  its `sessions/<pid>.json`, in any account config (`~/.claude`,
  `~/.claude-dev1`, …), or the job a `claude attach` pane shows.
- **Sub-agents**: `projects/<project>/<session>/subagents/agent-*.meta.json`
  (description, `finished`) and the agent's own `agent-*.jsonl` (its last
  dozen tool calls and messages, from its last 192 KB).
- **Background shells**: the session's transcript — each `Bash` call run in
  the background (or sent there with Ctrl+B), its task id, and the
  `<task-notification>` that says how it ended (`TaskStop` marks it
  stopped).
- **Shells an agent started**: the agent's own transcript, read the same
  way. They can outlive the agent.
- **The command running in the foreground**: its output file is written
  before the transcript records the call, so an output file a live process
  still writes, that no background call names, is shown as running, with
  the description of the Bash call still waiting on its result.
- **A shell's output**: `/tmp/claude-<uid>/<project>/<session>/tasks/<id>.output`.
  Its last line is what it is doing (the detail view shows the last dozen); `[exited with code N]` or `[killed]`
  there also says it ended. A shell whose output file no process holds open
  any more has ended, even with no notice.

Transcripts are read incrementally: how far the script read and what it
learned are kept per session in `/tmp/punkterm-<uid>/tasks2-<session>.json`
on the host, so a long transcript is not re-read every few seconds.

Not shown yet: Claude Code's dynamic workflows (they are counted on the
session's card).
