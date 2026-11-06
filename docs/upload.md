# Uploading files into a session

Code: `src/upload.rs` (picker, ssh copy, what gets pasted),
`App::upload_file` / `open_upload` in `src/ui.rs`, `DroppedFile` in
`src/main.rs`, the header's UPLOAD chip in `src/ui/chrome.rs`.

Send a file from this machine into the active session, on any host, so
Claude can read it:

- **Drag and drop** files onto the punkterm window (X11; winit doesn't yet
  deliver file drops on Wayland). Several at once work.
- **UPLOAD** in the session header, **Ctrl+Shift+F**, or **Upload file…** in
  a session card's right-click menu: opens the desktop's file picker
  (`zenity`, else `kdialog`; one of them must be installed). Pick one or more.

Each file goes to the active session (the clicked card's, from the menu):

| Session | Host | What happens |
|---|---|---|
| Claude | remote | Copied to `/tmp/punkterm-upload/<time>/<name>` over the host's shared ssh connection, then `@<path> ` is pasted: Claude Code reads the file into context when you send the prompt |
| Claude | local | `@<path> ` to the file where it is (copied to the temp dir first if its path has spaces or other characters an `@` mention can't hold) |
| Claude, an image (png, jpg, gif, webp) | either | The bare path is pasted, which Claude Code attaches as an image |
| Shell | either | The path, quoted if needed, so you can use it in a command |

Nothing is sent to Claude until you press ↵, so add your question after the
mention. On the host, each upload gets its own folder, made with `umask 077`
(readable only by you); file names are reduced to letters, digits and `._-+`.
Folders can't be uploaded. A toast says when a file is sent, or why not.

For clipboard images (Ctrl+V / Shift+Insert) see [paste.md](paste.md).
