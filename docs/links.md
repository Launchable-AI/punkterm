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

URLs that a program breaks with hard newlines (rather than letting the
terminal wrap them) are only matched up to the break.
