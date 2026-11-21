# Session groups

Code: `src/ui.rs` (`Group`, `group_picked`, `toggle_group`, `shown`,
`select_nth`, `regroup`, `head_drop`), `src/ui/chrome.rs` (headers in the rail),
`src/state.rs` (saved groups).

With many sessions on a host, put the ones you're working on in one group
and the rest in another, then roll the second up out of the way.

| Do | How |
|---|---|
| Pick sessions | **Shift+click** their cards (again to unpick; a plain click clears the picks) |
| Group them | **Ctrl+Space g**, then type a name (↵ with nothing keeps `GROUP n`). With nothing picked, groups the active session |
| Take out of a group | **Ctrl+Space G** (the picked sessions, else the active one) |
| Rename | **Ctrl+Space ,** renames the active session's group |
| Roll up / out | Click the group's header |
| Reorder groups | Drag a group's header up or down: a line shows where it will go |
| Move a session | Drag its card onto another group's header or cards; onto an ungrouped card to take it out |
| Full or brief cards | The **FULL / BRIEF** switch on the header |
| All of the above | Right-click a card or header (see [menu.md](menu.md)) |

In the rail each host lists its ungrouped sessions first, then each group
under a header: `▾`/`▸`, the name, how many sessions, and dots counting the
ones needing input, done, or working, so a rolled-up group still shows when
something in it wants you. A split window's panes always go into a group
together, and a new shell opened from a grouped session (a split, Ctrl+Shift+T,
the `+`, or Ctrl+Space c) joins that session's group.

## Full and brief cards

A group's cards are shown in full by default, with their activity line
(what Claude is doing, its agents and workflows), however crowded the rail
gets. The header's **FULL / BRIEF** switch turns that group's cards to one
line each, a summary. Ungrouped cards go brief on their own when full ones
would not fit.

## Order

Groups are listed in the order they were made until you drag one: hold a
group's header and move it up or down, and a line shows where it will land;
let go to move it there. A header let go without moving still rolls the
group up. The order is the same on every host, and session numbers follow
it. It is saved with the groups.

## Colour marks

Right-click a card and pick a colour on its **Colour** row to mark it: the
card is tinted and outlined in that colour, with a bar at its right edge.
Red, orange, yellow, green, blue, purple and gray look the same in every
theme; gray also mutes the card's name, for sessions set aside as inactive;
the struck-through swatch clears the mark. Shift+click several cards first
to mark them all at once. A session's mark is saved with the groups (by host
and tmux pane id, so a tmux server restart can move it to a new session that
reuses the id) and dropped when you close the session from punkterm.

## Rolled up

A rolled-up group's sessions are hidden from the rail and skipped by every
shortcut: Ctrl+] / Ctrl+[, Ctrl+Tab, Ctrl+Shift+Space (needs you) and the
session numbers. Rolling up the group the active session is in moves to the
next session in sight.

## Session numbers

Cards are numbered by their place among the sessions in sight, and the
number keys follow the rail: **Ctrl+Shift+1–9**, or **Ctrl+Space** and the
number, where a second digit within a second makes it two-digit
(**Ctrl+Space 1 2** is session 12).

## Saved

Groups, their order, and whether each is rolled up or brief, are kept in
`~/.local/state/punkterm/state.toml`, each session by its host's name and
tmux pane id (`%12`), which hold while the tmux server runs, through
punkterm restarts and joins. After a tmux server restart pane ids are
reused, so regroup then.

## A long list

The session list scrolls in the room above the rail's footer: the wheel over
it scrolls it, ↑ MORE / ↓ MORE say there is more above or below, and moving
to a session (Ctrl+], a number, a click elsewhere) scrolls its card into
view. Only whole cards are drawn.

## The rail's width

Drag the rail's right edge (the gap between it and the terminal panel; the
pointer turns into a resize arrow there) to make it wider or narrower,
220 to 600 px; the panel and its terminals, and tmux with them, take the
rest. Double-click the edge for the default width again. The width is kept
in punkterm's state file across restarts.
