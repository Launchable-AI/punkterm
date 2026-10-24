# Scrollback

Code: `src/term.rs` (`SCROLLBACK`), `src/tmuxcc.rs` (`Writer::capture`),
`src/ui/chrome.rs` (`scroll_badge`).

Every pane keeps up to **50,000 lines** of history (`term::SCROLLBACK`).

- **Own panes** (a local PTY): history builds up as output scrolls off.
- **tmux panes**: on attach punkterm captures the pane's existing history
  (`capture-pane -S -50000`), then keeps everything tmux sends after that.
  The capture can only return what tmux kept, so it is capped by tmux's own
  `history-limit` (default 2000). For deep history from before attaching, raise it in
  the server's `~/.tmux.conf`, e.g. `set -g history-limit 50000`.

## Scrolling

The mouse wheel goes to the app when the app asks for it: apps with mouse
reporting on (Claude Code, tmux with `mouse on`, vim) get wheel events, and
full-screen apps get arrow keys. Only otherwise does the wheel scroll
punkterm's history. **Shift+wheel** always scrolls punkterm's history.

While scrolled back, a badge in the terminal's top-right shows the position
the way tmux's copy mode does: `[lines up/lines of history]`. It disappears
when you're back at the bottom (typing also jumps back to the bottom).
