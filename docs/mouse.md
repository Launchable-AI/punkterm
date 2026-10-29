# Mouse

Code: `App::mouse_report`, `mouse_report_motion`, `report_cell` and `scroll`
in `src/ui.rs`; `pane_modes` and `PaneInfo::mode_sequences` (`src/tmuxcc.rs`).

By default a left drag selects text (double-click a word, triple-click a
line), middle-click pastes the selection, the wheel scrolls punkterm's
history, Ctrl+click opens links and paths, and right-click on a session card
or group header opens its menu.

A program that asks for the mouse (Claude Code's fullscreen layout, `/tui
fullscreen`; vim with `mouse=a`; htop; tmux-aware TUIs) gets it instead:
presses and releases of all three buttons, drags, motion when it asks for
any-motion tracking, and the wheel, each at the cell under the pointer, in
SGR encoding. **Shift** keeps the click for punkterm (Shift+drag selects, as
in other terminals); Ctrl+click still opens links.

## Modes set before punkterm attached

A pane's history comes from `capture-pane`, which brings the screen but not
the modes its program set. punkterm reads them from tmux's own record of the
pane (`mouse_standard_flag`, `mouse_button_flag`, `mouse_all_flag`,
`mouse_sgr_flag`, `mouse_utf8_flag`, `keypad_cursor_flag`) and sets them
after each capture. tmux keeps no record of bracketed paste, so it is set for
panes running `claude`, which always asks for it: a multi-line paste then
stays one paste.
