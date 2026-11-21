# Agent bus (jotbus-local)

Code: `src/comms.rs` (reading a host's bus), `src/ui/bus.rs` (the view's
state, polling and clicks), `src/ui/chrome.rs` (`bus_layer`, drawing).

[jotbus-local](https://jotbus.com) lets coding agents on one machine message
each other: Claude Code sessions (and Codex) @mention one another, hand off
work and wait for replies. **Ctrl+Shift+J** opens the **BUS** view to watch
that traffic.

- **Agents** (left): everyone on the bus now, each in its own colour, with
  its project folder, when it was last active (green dot: in the last two
  minutes), and the punkterm session it runs in (`→ ml-lab`). Click one to
  jump to its session.
- **Channels** (below): each channel and how many messages it has had.
  Click one to show only its messages; click it again for all of them.
- **Messages** (right): every channel's recent messages, oldest to newest,
  the newest at the bottom. Each shows the time, the channel, who wrote it
  and whom it @mentions (`@claude-ubuntu-2 → @claude-ubuntu`), each name in
  that agent's colour, so two sessions talking read like a conversation.
  The text keeps its line breaks; long paths wrap rather than being cut.

| Keys | |
|---|---|
| Ctrl+Shift+J | Open or close (also Esc, Enter, q, J) |
| Wheel, ↑ ↓, PageUp/PageDown | Scroll back through older messages; Home / End: oldest / newest |
| Tab | Next host with a bus |
| Click outside | Close |

## Where it reads from

Each host's bus is read where it lives, locally or over the host's shared
ssh connection, every 2.5 s while the view is open (nothing runs while it
is closed). A small Python script (`python3` on the host) reads
`~/.config/jotbus-local/` (or `$JOTBUS_LOCAL_HOME`):

- `agents/<pid>.json`: one per running agent session; agents whose process
  has exited are left out.
- `channels/<name>.jsonl`: each channel's messages; the view keeps the last
  200 per channel and 400 overall.

punkterm only reads these files; it never posts to the bus.

An agent is matched to its tmux pane through the process tree: the
jotbus-local server records its parent processes, and one of them is the
pane's shell. Agents outside tmux, or in a pane punkterm doesn't show, have
no session to jump to.

The view shows the active session's host when it has a bus, else the first
host that does. A host without jotbus-local just has no tab; set it up there
with `npx jotbus-local`.
