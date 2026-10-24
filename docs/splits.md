# Split panes and the leader key

Code: `src/ui.rs` (`group`, `pane_area`, `split`, `focus_dir`, `toggle_zoom`),
`src/ui/chrome.rs` (`draw_view`), `src/main.rs` (leader handling),
`src/tmuxcc.rs` (`PANE_FORMAT`: `pane_left`, `pane_top`, `window_zoomed_flag`).

## Split view

When the active session's tmux window holds more than one pane (cc-rail
rails aside), the panel shows all of them in tmux's own layout: each pane at
its `pane_left`/`pane_top`, hairlines in the one-cell gaps tmux leaves, the
focused pane at full strength and the rest dimmed. Every pane still has its
own card in the rail.

- Click a pane to focus it; punkterm also runs `select-pane` so tmux agrees.
- A zoomed window (`resize-pane -Z`) shows just its zoomed pane.
- punkterm sizes tmux windows to its panel. A window sized by a larger
  client (another terminal attached to the same session) is cut to the
  panel from its top-left, so panes off to the right may not show until
  punkterm is the client in use again.

## Split windows with a cc-rail

punkterm doesn't draw cc-rail's rail pane. A window that holds one makes
the window wider by the rail (plus its divider), so the panes it does show
fill the panel; this applies to split windows too. The rail is held at the
width cc-rail is set to (`~/.cc-rail/state/ui.json`, else 34) and given that
width straight back after each window resize, since tmux spreads a resize
over every pane.

## In the rail

Each pane of a split window keeps its own card, and the cards of one window
stay together: adjacent, packed closer than other cards, in reading order
(top to bottom, left to right), with a bracket down their left side. The
bracket is in the accent colour for the window on screen. Panes created
later (a new split, or one moved in with `join-pane`) are moved next to
their window-mates on the next pane listing (`App::regroup`).

## Leader: Ctrl+Space

Ctrl+Space arms the leader for 3 seconds (a `LEADER` chip shows in the
header); the next key picks the action, as with a tmux prefix. tmux's own
bindings don't apply to punkterm, which talks to tmux in control mode, so
these mirror the usual ones:

| Key | Action |
|---|---|
| `v` or `%` | Split side by side (`split-window -h`) |
| `s` or `"` | Split stacked (`split-window -v`) |
| `h` `j` `k` `l` / arrows | Focus the pane left / down / up / right |
| `z` | Zoom the pane to the whole window, or back |
| `c` | New shell (tmux window) on this host |
| `n` / `p` | Next / previous session |
| Ctrl+Space | Send a literal Ctrl+Space to the program |

Any other key cancels. Ctrl+Shift+E still splits side by side.

**Alt+h/j/k/l** moves between split panes without the leader. When there is
no pane in that direction (or the window isn't split) the key goes to the
program as usual, so Alt+h etc. still work in a single pane.
