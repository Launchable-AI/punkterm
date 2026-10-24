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
