# Links

Code: `src/links.rs` (finding and opening), `src/ui.rs` (`hover_link`, `open_link`).

**Ctrl+click** a link to open it with the desktop's opener (`xdg-open`; `open`
on macOS). Holding Ctrl over a link underlines it in the accent colour and
shows a hand cursor. A plain click still starts a selection.

Two kinds of link are recognised:

- **OSC 8 hyperlinks**, which programs embed explicitly (Claude Code, `ls
  --hyperlink`, many CLIs). The whole run of cells carrying the link opens
  its target. Only `http(s)://`, `mailto:` and `file://` targets are opened.
- **URLs in plain text** starting with `http://` or `https://`, followed
  across rows the terminal soft-wrapped. Trailing punctuation is dropped, and
  a closing `)`/`]` is kept only when the URL opened one, so
  `(see https://example.com/x).` opens `https://example.com/x`.

- **File paths** in plain text, absolute (`/tmp/x/shot.png`) or under the
  home folder (`~/notes.md`), as Claude prints them. A trailing `:12:5` line
  number and sentence punctuation are dropped, so `src=/home/me/a.rs:12.`
  opens `/home/me/a.rs`.

A path opens where the session runs. In a local session it opens with the
desktop's opener ("NOT FOUND" if it isn't there). In a session on an ssh
host, punkterm first copies the file over the host's ssh connection into
`~/Downloads/punkterm/` (as `name-1.ext` and so on when the name is taken),
then opens the copy; a toast says where it was saved. Directories aren't
copied.

An OSC 8 link that a program broke across rows (Claude Code does, for its
sign-in URL) is followed across them too, so a double-click selects all of
it.

A long URL or path is followed onto the next row both where the terminal
wrapped it and where the program broke the line itself, as Claude Code does
at the pane's width: a row that runs to its last column into a row that
starts at its first. The link is read from the pane's text, so columns cut
off on screen (a pane wider than the panel) still count.

## Selecting

A **double-click** on a URL or path selects all of it, across rows; elsewhere
it selects a word, and a triple-click a line. Copying a selection (drag,
then Ctrl+Shift+C, or the primary selection) joins rows a program broke at
the pane's width back together, so a URL copied off several rows pastes as
one line; other line breaks are kept.
