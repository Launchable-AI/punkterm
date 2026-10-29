# show-image

A Claude Code plugin that shows pictures and plays video inline in the chat.
Claude gets two tools: `show_image`, which it uses when you ask to see a
screenshot it took, a chart it drew, a PDF page or any picture on disk, and
`show_video`, which plays a video or animated GIF in place with play and
pause. You can also run `/show <path>` yourself, for either. The picture is drawn under the call with Claude Code's own image
element, which uses the kitty graphics protocol: it shows in punkterm, kitty
and Ghostty, and as a one-line description elsewhere.

The pixels are sent as data, not a file name, so it works when Claude runs on
an ssh host too.

| Shows | Needs, on the machine Claude runs on |
|---|---|
| PNG | nothing |
| JPEG, WebP, GIF, BMP, TIFF, HEIC… | ImageMagick (`convert`/`magick`) or ffmpeg |
| PDF (a page) | poppler-utils (`pdftoppm`), or ImageMagick allowed to read PDFs |
| SVG | librsvg (`rsvg-convert`) or ImageMagick |
| Mermaid (`.mmd`) | the Mermaid CLI (`mmdc`) |
| Video (mp4, mov, webm, mkv, avi), animated GIF | ffmpeg |
| Charts | whatever draws them to PNG; Claude is told to save a PNG (e.g. matplotlib) and show that |

Stills are converted at up to 1600 px a side. Video is cut into frames at 10
a second, 480 px wide, up to 60 s (Claude can pick a part with `start` and
`seconds`), and played by swapping the picture in place; it starts on its
own and stops at the end or when scrolled out of view, with a Play / Pause /
Replay button under it.

## Controls

Under each picture: **Open** (in the desktop's own program, on the machine
Claude runs on) and **Hide** (rolled up to one line, with a **Show** button to bring it back); under a video also
**Pause / Play** and **Replay**. The buttons take clicks in Claude Code's
fullscreen layout (`/tui fullscreen`, where Claude asks the terminal for the
mouse; punkterm passes clicks on); anywhere, `/media` does the same for the
latest one:

| | |
|---|---|
| `/media pause`, `/media play`, `/media replay` | the latest video |
| `/media open` | the latest picture or video, in its own program |
| `/media hide`, `/media show` | roll the latest up to one line, or the latest hidden one back |

The path above each one is plain text: in punkterm, Ctrl+click it to open the
file, downloaded first when Claude runs on an ssh host.

## Install

From a shell, with the punkterm repository checked out:

```sh
claude plugin marketplace add /path/to/punkterm
claude plugin install show-image@punkterm
```

Or from GitHub (needs read access to the repository and a git credential for
github.com):

```sh
claude plugin install show-image --marketplace Launchable-AI/punkterm
```

## Inside tmux

Claude Code keeps images off inside tmux unless it starts with
`CLAUDE_CODE_FORCE_TERMINAL_IMAGES=1`. punkterm sets that in each tmux server
it connects to, so Claude sessions started from then on have it (see
punkterm's `docs/images.md`).
