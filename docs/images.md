# Inline images

Code: `src/graphics.rs` (protocol, decoding, image store, atlas packing),
`src/term.rs` (`Pane::feed`), `draw_term` in `src/ui.rs` (placeholder cells),
`src/gfx.rs` + `src/shader.wgsl` (the image atlas, quad kind `IMAGE`).

punkterm shows images sent with the **kitty graphics protocol** the way Claude
Code sends them: the image is transmitted once, then drawn with **Unicode
placeholders**, text cells holding U+10EEEE whose foreground colour names the
image and whose combining marks give the row and column of the image each
shows. Because placeholders are ordinary text, they survive tmux, scrolling
and redraws; punkterm draws the matching slice of the image in each one,
fitted to the placement's cells (aspect ratio kept, centred).

## Through tmux

Inside tmux, Claude wraps each command in tmux passthrough
(`ESC P tmux; … ESC \`). tmux's control mode gives punkterm a pane's raw
output, so punkterm reads the images there itself: no `allow-passthrough`
needed, and tmux 3.2 is fine.

Claude Code keeps images off inside tmux unless `CLAUDE_CODE_FORCE_TERMINAL_IMAGES=1`
is in its environment. punkterm sets it in each tmux server's global
environment when it connects (`set-environment -g`), so **Claude sessions
started afterwards** have it; restart older ones to get images. A session
punkterm creates itself (`main`, when a host has none) has it from its first
shell (`new-session -e`). A shell that was already running before punkterm
connected doesn't: open a new window (Ctrl+Shift+T) or
`export CLAUDE_CODE_FORCE_TERMINAL_IMAGES=1` there before starting Claude. To undo on a
host: `tmux set-environment -gu CLAUDE_CODE_FORCE_TERMINAL_IMAGES`. (A Claude
started with it, viewed from a terminal without kitty graphics, shows
placeholder boxes instead of images.)

tmux reads any APC sequence as a pane title, so an image command sent
outside passthrough becomes the pane's title in tmux. punkterm ignores such a
title (`is_graphics_title`) and keeps the card's last one.

## Supported

- Transmit (`a=t`, `a=T`), chunked (`m=1`), PNG (`f=100`), RGBA (`f=32`),
  RGB (`f=24`), zlib (`o=z`); queries (`a=q`) answered; delete by id or all.
- Sources: inline data (`t=d`), and files (`t=f`, `t=t` deleted after
  reading). Files are read where the program runs: locally, or over the
  host's ssh connection for a remote session. Not shared memory (`t=s`).
- Images over 1536 px a side are scaled down on arrival.

Not yet: direct placements (an image drawn at the cursor without
placeholders, which Claude's markdown renderer can use), animation, and
images in panes that aren't tmux panes. Images sent before punkterm attached
to a pane are not known to it.

## Where Claude Code shows images

Only through its image component, which plugins use: Claude has no way of its
own to put a picture in the chat. This repository ships one,
[`claude-plugin/show-image`](../claude-plugin/show-image/README.md): a
`show_image` tool Claude calls when you ask to see an image, and `/show <path>`.
The repository is its marketplace:

```sh
claude plugin marketplace add /path/to/punkterm
claude plugin install show-image@punkterm
```

Install it on each host Claude runs on (an ssh host too). Others:
[claude-show-your-work](https://github.com/iamumeransari/claude-show-your-work).
