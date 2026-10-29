import type { Elements, EngineInterface, Register } from 'claude-code'

type Els = Elements['terminal']

// Pictures and video inline in the chat. Claude calls show_image or
// show_video (or you run /show <path>) and the picture is drawn under the
// call with Claude Code's Image element: kitty graphics, as kitty, Ghostty
// and punkterm draw it. Pixels go as bytes, not a file name, so it works
// when Claude runs over ssh too.
//
// Anything not a PNG is converted first with what the host has: ImageMagick
// or ffmpeg for photos, pdftoppm for PDF pages, rsvg-convert for SVG, mmdc
// for Mermaid. Video is cut into small frames with ffmpeg and played in
// place by swapping the keyed Image's source on a timer.

const IMAGE_TOOL = 'mcp__show-image__show_image'
const VIDEO_TOOL = 'mcp__show-image__show_video'
/** The Image element takes PNGs up to 2 MiB. */
const MAX_BYTES = 2 * 1024 * 1024
/** Longest side a still is converted at. */
const STILL_SIDE = 1600
/** Video frames: their width (small, as each one crosses ssh), rate, count. */
const VIDEO_WIDTH = 480
const VIDEO_FPS = 10
const VIDEO_MAX_FRAMES = 600

type Pic = { path: string; png: string; width: number; height: number }
type Video = {
  path: string
  dir: string
  frames: number
  fps: number
  width: number
  height: number
  first: string
  frame: number
  timer?: { cancel: () => void }
  isPlaying: boolean
  hasStarted?: boolean
}

// Module state: what each row showed. A reload forgets it; a still is then
// read again from its path, a video shows its alt until shown again.
const shown = new Map<string, Pic>()
const videos = new Map<string, Video>()
const tempDirs: string[] = []
// The rows drawn, oldest first (the row's requestId and its kind), for
// /media, which acts on the latest; and the rows rolled up to one line.
const rows: { id: string; kind: 'image' | 'video'; path: string; key: string }[] = []
const hidden = new Set<string>()

function noteRow(id: string, kind: 'image' | 'video', path: string, key: string) {
  if (!rows.some(r => r.id === id)) rows.push({ id, kind, path, key })
  if (rows.length > 200) rows.splice(0, rows.length - 200)
}

const VIDEO_EXT = /\.(mp4|m4v|mov|webm|mkv|avi|gif|apng)$/i
const VIDEO_WORDS = 'mp4, mov, webm, mkv, avi or an animated GIF'

/** Bytes from (whole groups of) base64. */
function unbase64(text: string): number[] {
  const abc = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
  const out: number[] = []
  let acc = 0
  let bits = 0
  for (const ch of text) {
    const v = abc.indexOf(ch)
    if (v < 0) continue
    acc = ((acc << 6) | v) & 0xffffff
    bits += 6
    if (bits >= 8) {
      bits -= 8
      out.push((acc >> bits) & 0xff)
    }
  }
  return out
}

/** Width and height from a PNG's IHDR, or undefined when it is no PNG. */
function pngSize(base64: string): { width: number; height: number } | undefined {
  const head = unbase64(base64.slice(0, 44))
  const sig = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]
  if (head.length < 24 || sig.some((b, i) => head[i] !== b)) return undefined
  const u32 = (o: number) =>
    (((head[o] ?? 0) << 24) | ((head[o + 1] ?? 0) << 16) | ((head[o + 2] ?? 0) << 8) | (head[o + 3] ?? 0)) >>> 0
  return { width: u32(16), height: u32(20) }
}

/** Runs a converter; false when it is not installed or failed. */
async function run($: EngineInterface, argv: string[], timeoutMs = 60_000): Promise<boolean> {
  try {
    const r = await $.process.run(argv, { timeoutMs })
    return r.exitCode === 0
  } catch {
    return false
  }
}

async function tempDir($: EngineInterface): Promise<string | undefined> {
  try {
    const r = await $.process.run(['mktemp', '-d', '-t', 'show-image.XXXXXX'])
    const dir = r.stdout.trim()
    if (r.exitCode !== 0 || !dir) return undefined
    tempDirs.push(dir)
    return dir
  } catch {
    return undefined
  }
}

async function readPng($: EngineInterface, file: string): Promise<{ png: string; width: number; height: number } | undefined> {
  try {
    const { base64 } = await $.fs.read(file, { as: 'bytes' })
    const size = pngSize(base64)
    return size && { png: base64, ...size }
  } catch {
    return undefined
  }
}

/**
 * Any picture as a PNG the Image element takes: as it is, or converted by
 * the first converter that works. A string says why not.
 */
async function load($: EngineInterface, path: string, page = 1): Promise<Pic | string> {
  const isPng = /\.png$/i.test(path)
  if (isPng) {
    const got = await readPng($, path).catch(() => undefined)
    if (got && got.png.length * 0.75 <= MAX_BYTES) return { path, ...got }
    if (!got) {
      try {
        await $.fs.read(path, { as: 'bytes' })
      } catch {
        return `cannot read ${path}`
      }
    }
  }
  const dir = await tempDir($)
  if (!dir) return 'cannot make a temporary folder to convert the image in'
  const out = `${dir}/image.png`
  const fit = `${STILL_SIDE}x${STILL_SIDE}>`
  const p = Math.max(1, Math.floor(page))
  let tries: string[][]
  let needs: string
  if (/\.pdf$/i.test(path)) {
    tries = [
      ['pdftoppm', '-png', '-r', '110', '-f', `${p}`, '-l', `${p}`, '-singlefile', path, `${dir}/image`],
      ['magick', '-density', '110', `${path}[${p - 1}]`, '-resize', fit, out],
    ]
    needs = 'poppler-utils (pdftoppm) or ImageMagick'
  } else if (/\.svg$/i.test(path)) {
    tries = [
      ['rsvg-convert', '-w', `${STILL_SIDE}`, '-o', out, path],
      ['magick', '-background', 'none', path, '-resize', fit, out],
    ]
    needs = 'librsvg (rsvg-convert) or ImageMagick'
  } else if (/\.(mmd|mermaid)$/i.test(path)) {
    tries = [['mmdc', '-i', path, '-o', out, '-w', `${STILL_SIDE}`, '-b', 'white']]
    needs = 'the Mermaid CLI (npm install -g @mermaid-js/mermaid-cli)'
  } else {
    // Photos and the rest; a GIF's or video's first frame.
    tries = [
      ['magick', `${path}[0]`, '-auto-orient', '-resize', fit, out],
      ['convert', `${path}[0]`, '-auto-orient', '-resize', fit, out],
      ['ffmpeg', '-v', 'error', '-y', '-i', path, '-frames:v', '1', '-vf', `scale='min(${STILL_SIDE},iw)':-2`, out],
    ]
    needs = 'ImageMagick or ffmpeg'
  }
  for (const argv of tries) {
    if (await run($, argv)) {
      const got = await readPng($, out)
      if (got && got.png.length * 0.75 <= MAX_BYTES) return { path, ...got }
    }
  }
  return `could not turn ${path} into a picture: needs ${needs} on this machine`
}

/** Cut a video into frames; a string says why not. */
async function loadVideo($: EngineInterface, path: string, start?: number, seconds?: number): Promise<Video | string> {
  const dir = await tempDir($)
  if (!dir) return 'cannot make a temporary folder for the frames'
  const fps = VIDEO_FPS
  const argv = ['ffmpeg', '-v', 'error', '-y']
  if (start) argv.push('-ss', `${start}`)
  argv.push('-i', path)
  if (seconds) argv.push('-t', `${seconds}`)
  argv.push('-vf', `fps=${fps},scale=${VIDEO_WIDTH}:-2`, '-frames:v', `${VIDEO_MAX_FRAMES}`, `${dir}/f%04d.png`)
  if (!(await run($, argv, 180_000))) return `could not read ${path} as video: needs ffmpeg on this machine (and a ${VIDEO_WORDS})`
  let frames = 0
  try {
    frames = (await $.fs.list(dir)).filter(f => /^f\d+\.png$/.test(f.name)).length
  } catch {
    frames = 0
  }
  const first = frames > 0 ? await readPng($, `${dir}/f0001.png`) : undefined
  if (!first) return `${path} has no frames ffmpeg could read`
  return { path, dir, frames, fps, width: first.width, height: first.height, first: first.png, frame: 1, isPlaying: false }
}

/** Fit a picture to the row: cells are about twice as tall as wide. */
function cells(width: number, height: number, room: number, want?: number) {
  let columns = Math.max(4, Math.min(want ?? 72, room, 255))
  let rows = Math.round((columns * height) / width / 2)
  if (rows > 36) {
    rows = 36
    columns = Math.max(4, Math.min(Math.round((rows * 2 * width) / height), room))
  }
  return { columns, rows: Math.max(1, Math.min(rows, 255)) }
}

function stop(v: Video) {
  v.timer?.cancel()
  v.timer = undefined
  v.isPlaying = false
}

/** Play from the current frame, swapping the Image's source each tick. */
function play($: EngineInterface, requestId: string, v: Video) {
  if (v.isPlaying) return
  if (v.frame >= v.frames) v.frame = 1
  v.isPlaying = true
  let busy = false
  v.timer = $.clock.every(Math.round(1000 / v.fps), () => {
    if (busy) return
    busy = true
    void (async () => {
      try {
        if (v.frame >= v.frames) {
          stop(v)
          $.ui.invalidate('ui.render')
          return
        }
        v.frame += 1
        const f = await readPng($, `${v.dir}/f${String(v.frame).padStart(4, '0')}.png`)
        if (!f) return
        const r = await $.ui.blit({ requestId, key: 'video', source: { png: f.png } })
        // Scrolled away or redrawn elsewhere: stop rather than spin.
        if ('deny' in r && r.deny) {
          stop(v)
          $.ui.invalidate('ui.render')
        }
      } finally {
        busy = false
      }
    })()
  })
}

/** Open a file with the desktop's own program, where Claude runs. */
async function openExternally($: EngineInterface, path: string): Promise<string> {
  for (const opener of ['xdg-open', 'open']) {
    if (await run($, [opener, path], 15_000)) return `Opened ${path}`
  }
  return `No desktop opener here (xdg-open, open). In punkterm, Ctrl+click the path to download and open it.`
}

/** The controls under a picture: buttons (where the surface takes clicks)
 * and the /media words that do the same anywhere. */
function controls($: EngineInterface, { Box, Text, Button }: Els, id: string, path: string, v?: Video) {
  const redraw = () => $.ui.invalidate('ui.render')
  const toggle = () => {
    if (!v) return
    if (v.isPlaying) stop(v)
    else play($, id, v)
    redraw()
  }
  const replay = () => {
    if (!v) return
    stop(v)
    v.frame = 1
    play($, id, v)
    redraw()
  }
  const open = () => void openExternally($, path).then(t => $.ui.toast(t))
  const hide = () => {
    if (v) stop(v)
    hidden.add(id)
    redraw()
  }
  return (
    <Box flexDirection="row" gap={1}>
      {v && (
        <Button key="toggle" onPress={toggle}>
          {v.isPlaying ? 'Pause' : 'Play'}
        </Button>
      )}
      {v && (
        <Button key="replay" onPress={replay}>
          Replay
        </Button>
      )}
      <Button key="open" onPress={open}>
        Open
      </Button>
      <Button key="hide" onPress={hide}>
        Hide
      </Button>
      <Text dimColor>/media {v ? 'pause · play · replay · ' : ''}open · hide</Text>
    </Box>
  )
}

/** A rolled-up row: one line, with a button (and the command) to bring it back. */
function drawHidden($: EngineInterface, { Box, Text, Button }: Els, id: string, kind: string, path: string) {
  const show = () => {
    hidden.delete(id)
    $.ui.invalidate('ui.render')
  }
  return (
    <Box flexDirection="row" gap={1}>
      <Button key="show" onPress={show}>
        Show
      </Button>
      <Text dimColor>
        ▸ {kind} {path} · hidden (/media show)
      </Text>
    </Box>
  )
}

function drawStill($: EngineInterface, els: Els, room: number, id: string, pic: Pic) {
  if (hidden.has(id)) return drawHidden($, els, id, 'image', pic.path)
  const { Box, Text, Image } = els
  const { columns, rows } = cells(pic.width, pic.height, room - 6)
  return (
    <Box flexDirection="column">
      <Text dimColor>{pic.path}</Text>
      <Image source={{ png: pic.png }} columns={columns} rows={rows} alt={`image: ${pic.path}`} />
      {controls($, els, id, pic.path)}
    </Box>
  )
}

function drawVideo($: EngineInterface, els: Els, room: number, id: string, v: Video) {
  if (hidden.has(id)) return drawHidden($, els, id, 'video', v.path)
  const { Box, Text, Image } = els
  const { columns, rows } = cells(v.width, v.height, room - 6)
  // Start on its first drawing.
  if (!v.isPlaying && v.frame === 1 && !v.hasStarted) {
    v.hasStarted = true
    $.clock.after(50, () => play($, id, v))
  }
  return (
    <Box flexDirection="column">
      <Text dimColor>
        {v.path} · {(v.frames / v.fps).toFixed(1)} s{v.isPlaying ? '' : v.frame >= v.frames ? ' · ended' : ' · paused'}
      </Text>
      <Image key="video" source={{ png: v.first }} columns={columns} rows={rows} alt={`video: ${v.path}`} />
      {controls($, els, id, v.path, v)}
    </Box>
  )
}

const RESULT_IMAGE = /^Shown inline to the user: (.+) \(\d+x\d+ px\)/

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    await $.tool.register({
      name: 'show_image',
      description:
        'Show a picture to the user inline in the chat: a screenshot, a photo, a chart, a diagram, a PDF page. ' +
        'Use it whenever the user wants to see an image instead of opening it in another app. ' +
        'Takes PNG, JPEG, WebP, GIF, BMP, TIFF, SVG, PDF (one page) and Mermaid (.mmd) files; others are converted with ImageMagick or ffmpeg on this machine. ' +
        'To show a chart or plot, draw it to a PNG first (for example matplotlib `plt.savefig("chart.png", dpi=150)`) and show that file.',
      inputSchema: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Path of the file, absolute or relative to the working directory' },
          alt: { type: 'string', description: 'What the picture shows, in a few words' },
          page: { type: 'number', description: 'For a PDF: which page (from 1)' },
          columns: { type: 'number', description: 'Width in terminal columns (default 72, at most the chat width)' },
        },
        required: ['path'],
      },
      isDeferred: false,
    })
    await $.tool.register({
      name: 'show_video',
      description:
        `Play a video or animation to the user inline in the chat, with play and pause: ${VIDEO_WORDS}. ` +
        `Shown at ${VIDEO_FPS} frames a second, ${VIDEO_WIDTH} px wide, up to ${VIDEO_MAX_FRAMES / VIDEO_FPS} s (pick a part with start and seconds). Needs ffmpeg on this machine.`,
      inputSchema: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Path of the video file' },
          alt: { type: 'string', description: 'What the video shows, in a few words' },
          start: { type: 'number', description: 'Start this many seconds in' },
          seconds: { type: 'number', description: 'How many seconds to show' },
          columns: { type: 'number', description: 'Width in terminal columns (default 72)' },
        },
        required: ['path'],
      },
      isDeferred: false,
    })
    await $.command.register({
      name: 'media',
      description: 'Control the latest inline picture or video: pause, play, replay, open, hide, show',
      argumentHint: 'pause | play | replay | open | hide | show',
    })
    await $.command.register({
      name: 'show',
      description: 'Show a picture or play a video inline: /show <path>',
      argumentHint: '<path to image, PDF, SVG or video>',
    })
    return next(e)
  })

  on('session.end', async ($, e, next) => {
    for (const v of videos.values()) stop(v)
    for (const dir of tempDirs.splice(0)) await run($, ['rm', '-rf', '--', dir])
    return next(e)
  })

  on('tool.call', { tool: IMAGE_TOOL }, async ($, e) => {
    const input = e as unknown as { tool_use_id: string; path: string; page?: number }
    const pic = await load($, input.path, input.page)
    if (typeof pic === 'string') return { deny: pic }
    shown.set(input.tool_use_id, pic)
    return { result: `Shown inline to the user: ${pic.path} (${pic.width}x${pic.height} px)` }
  }).catch(() => ({ deny: 'show_image could not read the picture.' }))

  on('tool.call', { tool: VIDEO_TOOL }, async ($, e) => {
    const input = e as unknown as { tool_use_id: string; path: string; start?: number; seconds?: number }
    const v = await loadVideo($, input.path, input.start, input.seconds)
    if (typeof v === 'string') return { deny: v }
    videos.set(input.tool_use_id, v)
    return { result: `Playing inline for the user: ${v.path} (${v.frames} frames, ${(v.frames / v.fps).toFixed(1)} s)` }
  }).catch(() => ({ deny: 'show_video could not read the video.' }))

  on('command.run', { command: 'media' }, async ($, e) => {
    const act = e.args.trim().toLowerCase() || 'help'
    const acts = ['pause', 'play', 'replay', 'open', 'hide', 'show']
    if (!acts.includes(act)) return { text: `Usage: /media ${acts.join(' | ')} (acts on the latest picture or video)` }
    // show: the latest hidden one; the rest: the latest shown one.
    const row = [...rows].reverse().find(r => (act === 'show' ? hidden.has(r.id) : !hidden.has(r.id)))
    if (!row) return { text: act === 'show' ? 'Nothing is hidden.' : 'No picture or video in this session yet.' }
    const v = row.kind === 'video' ? videos.get(row.key) : undefined
    let text: string
    switch (act) {
      case 'pause':
        if (!v) return { text: `${row.path} is a picture, not a video.` }
        stop(v)
        text = `Paused ${row.path}`
        break
      case 'play':
      case 'replay':
        if (!v) return { text: `${row.path} is a picture, not a video.` }
        stop(v)
        if (act === 'replay' || v.frame >= v.frames) v.frame = 1
        play($, row.id, v)
        text = `Playing ${row.path}`
        break
      case 'open':
        text = await openExternally($, row.path)
        break
      case 'hide':
        if (v) stop(v)
        hidden.add(row.id)
        text = `Hid ${row.path}`
        break
      default:
        hidden.delete(row.id)
        text = `Showing ${row.path} again`
    }
    $.ui.invalidate('ui.render')
    return { text }
  })

  on('command.run', { command: 'show' }, async ($, e) => {
    const path = e.args.trim()
    if (!path) return { text: 'Usage: /show <path to image, PDF, SVG or video>' }
    if (VIDEO_EXT.test(path)) {
      const v = await loadVideo($, path)
      if (typeof v === 'string') return { text: v }
      videos.set(`show:${path}`, v)
      return { text: `Playing ${v.path} (${v.frames} frames)` }
    }
    const pic = await load($, path)
    if (typeof pic === 'string') return { text: pic }
    shown.set(`show:${path}`, pic)
    return { text: `Showing ${pic.path} (${pic.width}x${pic.height} px)` }
  })

  // Under a call's row: the picture or the video, sized to the chat.
  on('ui.render', { component: 'ToolResult', props: { tool: IMAGE_TOOL } }, async ($, e, next) => {
    if (e.surface !== 'terminal' || e.props.isErrored) return next(e)
    let pic = shown.get(e.requestId)
    if (!pic) {
      const m = typeof e.props.output === 'string' ? RESULT_IMAGE.exec(e.props.output) : null
      const got = m?.[1] ? await load($, m[1]) : undefined
      if (!got || typeof got === 'string') return next(e)
      pic = got
      shown.set(e.requestId, pic)
    }
    noteRow(e.requestId, 'image', pic.path, e.requestId)
    return drawStill($, $.ui.resolve(e), e.viewport?.columns ?? 80, e.requestId, pic)
  })

  on('ui.render', { component: 'ToolResult', props: { tool: VIDEO_TOOL } }, async ($, e, next) => {
    if (e.surface !== 'terminal' || e.props.isErrored) return next(e)
    const v = videos.get(e.requestId)
    if (!v) return next(e)
    noteRow(e.requestId, 'video', v.path, e.requestId)
    return drawVideo($, $.ui.resolve(e), e.viewport?.columns ?? 80, e.requestId, v)
  })

  // /show's own row.
  on('ui.render', { component: 'CommandOutput', props: { command: 'show' } }, async ($, e, next) => {
    if (e.surface !== 'terminal' || e.props.isErrored) return next(e)
    const key = `show:${e.props.args.trim()}`
    const v = videos.get(key)
    if (v && e.props.text.startsWith('Playing ')) {
      noteRow(e.requestId, 'video', v.path, key)
      return drawVideo($, $.ui.resolve(e), e.viewport?.columns ?? 80, e.requestId, v)
    }
    if (!e.props.text.startsWith('Showing ')) return next(e)
    let pic = shown.get(key)
    if (!pic) {
      const got = await load($, e.props.args.trim())
      if (typeof got === 'string') return next(e)
      pic = got
      shown.set(key, pic)
    }
    noteRow(e.requestId, 'image', pic.path, key)
    return drawStill($, $.ui.resolve(e), e.viewport?.columns ?? 80, e.requestId, pic)
  })
}
