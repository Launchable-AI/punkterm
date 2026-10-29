# Right-click menu

Code: `src/ui/menu.rs` (items and actions), `src/ui/chrome.rs` (`menu_layer`).

Right-click a session's card in the rail, or a group's header. Pick with the
mouse, or ↑ ↓ and ↵; any other key or a click outside closes it.

**On a session** (items show only where they apply):

| Item | Does |
|---|---|
| Open | Switch to it |
| Join beside current | tmux `join-pane`: it moves beside the session on screen (same host) |
| Split side by side / stacked | A new shell beside or below it |
| Break out of split | tmux `break-pane`, back to a window (and session) of its own |
| Fork Claude session… | Switches to it and types `/fork `: Claude Code's fork, which carries the conversation into a new background session, wants a first prompt, so finish it and press ↵ |
| Pick / Unpick | For a new group (as Shift+click) |
| New group… | From the picked sessions, or just this one; asks for a name |
| Move to *group* | Into that group |
| Remove from group / Rename group… | |
| Detach (keeps running) | Drop it from punkterm; it keeps running in tmux |

**On a group header:** Roll up / Expand, Rename group…, Ungroup all.
