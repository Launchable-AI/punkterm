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

## Closing a pane

When the pane on screen goes away (its program exits, Ctrl+Shift+W, or it is
killed in tmux), punkterm moves to the nearest pane of the same split window,
as tmux does; with none left there, to the nearest session in the rail on the
same host, then any session (`App::successor`). Sessions in rolled-up groups
come last.

## Resizing

Drag the line between two panes to resize them (the pointer turns into a
resize arrow over it). The pane left of or above the line takes the
pointer's distance from its own edge as its width or height, a cell at a
time, through tmux's `resize-pane -x` / `-y`; tmux resizes the rest of the
window to match.

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

## Joining sessions: join-pane

**Ctrl+click** a session's card in the rail to bring that session beside the
one on screen, side by side: tmux's `join-pane -h` moves its pane into this
window. Or mark a pane with **Ctrl+Space m**, go to another, and **Ctrl+Space
J** joins the marked one here. Both panes must be tmux panes on the same host
(any of its sessions).

A session whose only pane is joined away closes, as in tmux. punkterm
remembers where a joined pane came from: **Ctrl+Space !** (`break-pane`)
moves it back out to a window of its own in that session, recreating the
session if it closed. A pane split here rather than joined breaks out into
the current session.

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
| `1`–`9`, then another digit | Session by its rail number: `1` `2` is session 12 (the second digit within a second) |
| `m` | Mark this pane to join elsewhere (again: clear the mark) |
| `J` | Join the marked pane beside this one |
| `!` | Break this pane out to its own window |
| `g` / `G` / `,` | Group the picked sessions / ungroup / rename the group (see [groups.md](groups.md)) |
| Ctrl+Space | Send a literal Ctrl+Space to the program |

Any other key cancels.

Without the leader: **Ctrl+Shift+V** (or Ctrl+Shift+E) splits side by side and
**Ctrl+Shift+"** splits stacked, each with a new shell in this pane's directory.
A plain shell (not tmux) opens a new session instead.

Ctrl+Shift+V doesn't paste: **Shift+Insert** pastes the clipboard, and
middle-click the primary selection.

**Alt+h/j/k/l** moves between split panes without the leader. When there is
no pane in that direction (or the window isn't split) the key goes to the
program as usual, so Alt+h etc. still work in a single pane.
