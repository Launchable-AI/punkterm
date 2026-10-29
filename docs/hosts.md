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
Claude Code sessions, newest first (up to 40), from every account config
(`~/.claude`, `~/.claude-dev1`, …). Each shows its title (from `/rename`, else
the one Claude gave it, else its first prompt), its folder and git branch,
how long ago it was last written, and your last prompt; **OPEN** marks one a
Claude has open right now (resuming it starts a second copy).

↑/↓ (or j/k, the wheel), then ↵ or a click, resumes it: a new tmux window on
that host, in the session's folder, with `claude --resume <id>` typed into
the shell there, so its PATH and aliases apply. A session open under another
account config resumes under that one (`CLAUDE_CONFIG_DIR=…`). Configs that
share their `projects/` folder share transcripts, so a closed session can't
say which account it last ran under; it resumes under the default one. Esc
closes the menu.

Code: `src/resume.rs` (reading the host's transcripts: a Python script over
the host's ssh connection, a little from each file's start and end),
`App::open_resume` / `resume_pick` in `src/ui.rs`.
