# Pasting

Code: `src/clipboard.rs`, `App::paste_text` / `App::paste_image` in `src/ui.rs`.

| Keys | Pastes |
|---|---|
| Shift+Insert | The clipboard (text, or an image as below) |
| Middle-click | The primary selection (what you last selected) |
| Ctrl+V | Goes to the program, except in Claude on a remote host (below) |

Text is pasted bracketed when the program asks for that (as Claude Code does).
Ctrl+Shift+V is a split, not paste (see [splits.md](splits.md)).

## Images into Claude, on any host

Claude Code's own Ctrl+V reads the clipboard of the machine it runs on, so
over ssh it can't see yours. punkterm brings the image over instead:

- **Ctrl+V** in a Claude session on a remote host, with an image on the
  clipboard: punkterm copies it to the host over the shared ssh connection
  (`/tmp/punkterm-paste/paste-<time>.png`), then pastes that path, which
  Claude Code attaches as an image. Without an image on the clipboard, Ctrl+V
  goes to Claude as usual. A toast says when it's sent, or why not.
- **Shift+Insert** with only an image on the clipboard does the same, in any
  session; locally the file goes in the system temp directory.

Locally, Claude's own Ctrl+V already works and is left alone. The clipboard
is read with `xclip` (X11) or `wl-paste` (Wayland); PNG and JPEG.
