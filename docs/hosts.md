# Hosts

Code: `src/hosts.rs` (host list, ssh history), `src/ui.rs` (`new_session`, `add_at`),
`src/ui/chrome.rs` (rail).

The rail lists sessions grouped by host: `local` first, then hosts from
`~/.config/punkterm/config.toml`, `--host`, and ones added from the connect
prompt.

```toml
[[host]]
name = "dev-box"
ssh = "-i ~/.ssh/key.pem ubuntu@203.0.113.7"   # destination, or ssh args
color = "#00e5ff"                             # optional
```

## Disconnect or remove a host

Right-click a host's heading in the rail for its menu: **New shell**,
**Resume a Claude session…**, and, for a remote host:

- **Disconnect (sessions keep running)**: punkterm detaches from the host's
  tmux sessions and drops them from the rail; they keep running there. The
  heading stays, marked DISCONNECTED, and Ctrl+Shift+R (reconnect) leaves it
  alone. **Connect** (the same menu), its **+**, or adding it again from the
  connect prompt brings it back.
- **Remove host**: disconnects it and takes it out of the rail, and out of
  the hosts punkterm connects to at launch. A host added from the connect
  prompt or `--host` is forgotten; one from `config.toml` (which punkterm
  only reads) is remembered as removed in the state file
  (`removed_hosts`) until you add it again from the connect prompt.

`local` can't be disconnected or removed.

Code: `App::disconnect_host` / `remove_host` / `connect_host` in
`src/ui.rs`, `host_menu` in `src/ui/menu.rs`.

## New shell on a host

Each host heading has a **+** button at its right end; clicking it opens a
new shell on that host. **Ctrl+Shift+T** does the same for the active
session's host.

On a host with tmux, the new shell is a new tmux window (in the active
session's tmux session when it is on that host, else the first connected
one). With no tmux session yet, punkterm starts one named `main`; locally
without tmux installed, it opens a plain shell.

## Resume a Claude session

The **↺** beside a host's **+** opens the resume menu: that host's recent
Claude Code sessions, newest first (up to 150), from every account config
(`~/.claude`, `~/.claude-dev1`, …). Each shows its title (from `/rename`, else
the one Claude gave it, else its first prompt), its folder and git branch,
how long ago it was last written, and your last prompt; **OPEN** marks one a
Claude has open right now (resuming it starts a second copy).

Type to search: the list narrows to the sessions with every word you've
typed somewhere in their title, first or last prompt, folder, branch,
account or id (any case; `punk rail` finds a punkterm session about the
rail). Backspace deletes, Ctrl+V pastes, and Esc clears the search before it
closes the menu.

↑/↓ (or Ctrl+N/Ctrl+P, the wheel), then ↵ or a click, resumes it: a new tmux window on
that host, in the session's folder, with `claude --resume <id>` typed into
the shell there, so its PATH and aliases apply. A session open under another
account config resumes under that one (`CLAUDE_CONFIG_DIR=…`). Configs that
share their `projects/` folder share transcripts, so a closed session can't
say which account it last ran under; it resumes under the default one. Esc
(with nothing typed) closes the menu.

Code: `src/resume.rs` (reading the host's transcripts: a Python script over
the host's ssh connection, a little from each file's start and end),
`App::open_resume` / `resume_pick` in `src/ui.rs`.
