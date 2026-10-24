# Themes

Code: `src/theme.rs` (palettes, themer loading), `src/ui.rs` (`cycle_theme`, `detect`).

## Choosing a theme

- **Ctrl+Shift+P** cycles themes with a crossfade; the choice is saved to punkterm's prefs.
- `punkterm --theme NAME` picks one for this run (not saved).

The cycle is punkterm's built-in palettes in order, then every dark theme in
`~/.config/themer/themes/*.toml` (read-only; light themer themes are skipped).

## Built-in palettes

| Name | Look |
|---|---|
| `neon` | default; magenta/cyan on near-black |
| `matrix` | green on black |
| `tron` | blue/cyan on navy-black |
| `green-black` | phosphor green on pure black |
| `blue-black` | electric blue on pure black |
| `amber` | amber CRT |
| `ghost` | monochrome white/gray |
| `burnt-orange-light` | light: warm cream and brown, burnt-orange accent (after the wezterm `burnt_orange_light` palette) |

A palette is a `Spec` in `theme.rs`. `surface` is the terminal background and
`bg` the window around it (a shade deeper than `surface`); `alert`/`busy`/`ready`
color session states and stay distinct from the accents.

## Light/dark and Claude Code

Claude Code's `auto` theme only changes when the terminal sends a mode 2031
theme report (`ESC[?997;1n` dark, `ESC[?997;2n` light). punkterm sends one to
every pane it detects as a Claude session: once when the session is first
detected, and again whenever the theme crosses between light and dark.
A theme counts as light when its `surface` luminance is above 0.5
(`Theme::is_light`).

Panes not detected as Claude get nothing, so shells never see stray text.

### Inside tmux

With tmux in between, Claude asks tmux, not punkterm, for the background and
theme, and tmux 3.6+ sends its own mode 2031 reports. Left alone, tmux decides
from whatever other clients it knows (or guesses dark) and overrides
punkterm's report, so long-running sessions would flip back.

So punkterm also tells tmux its colours (`Writer::report_colours` in
`src/tmuxcc.rs`), as a terminal answers OSC 10/11:

- on every pane listing (every 1.5 s, and at once after a theme change), each
  pane whose reported colours are stale gets
  `refresh-client -r '%N:ESC]10;rgb:…'` and `…]11;…` (cc-rail rails included);
- then one pane gets `set -p window-style default` followed by `set -pu
  window-style`. tmux re-sends theme reports to every pane whenever
  `window-style` changes; the pair leaves the options as they were.

tmux then derives each pane's light/dark from punkterm's background, answers
the pane's own colour queries with it, and reports it to Claude and cc-rail
itself. This is what kept sessions on the right theme regardless of which
machine you attach from: each punkterm reports its own theme on attach.

Caveats: the colours are per pane, so two punkterm instances attached to the
same session at once (e.g. desktop and laptop) take turns: the last to report
wins. tmux older than 3.4 rejects `-r` (punkterm ignores the error), and older
than 3.6 sends no theme reports, so there only the direct report above works.
