//! The HUD: background, session rail, panel frame and header. Visual language:
//! chamfered geometry, hairlines, tracked mono meta-text, light that follows
//! session state. No post-processing — every pixel here is a crisp shape.

use std::time::Instant;

use alacritty_terminal::grid::Dimensions;

use super::{App, HostState, Layout, Rect, NO_CLIP};
use crate::anim::ease_out_cubic;
use crate::claude::Status;
use crate::gfx::{kind, Quad};
use crate::glyphs::{Font, TextLayout};
use crate::theme::{mix, with_alpha, Rgba};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SCRAMBLE: &[char] = &['<', '>', '/', '\\', '_', '-', '=', '+', '*', '#', '[', ']', '%', '0', '1', '?'];
const DECODE_SECS: f32 = 0.34;

/// Text that resolves left to right behind a short frontier of noise.
fn decode(text: &str, elapsed: f32) -> (String, bool) {
    let p = elapsed / DECODE_SECS;
    if p >= 1.0 {
        return (text.to_string(), false);
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len() as f32;
    let resolved = (p * (n + 6.0)) as usize;
    let tick = (elapsed * 40.0) as usize;
    let out = chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if i < resolved.saturating_sub(6) || c == ' ' {
                c
            } else if i < resolved {
                SCRAMBLE[(i * 7 + tick * 13) % SCRAMBLE.len()]
            } else {
                ' '
            }
        })
        .collect();
    (out, true)
}

fn status_word(s: Status) -> &'static str {
    match s {
        Status::Working => "RUN",
        Status::Blocked => "INPUT",
        Status::YourTurn => "READY",
        Status::Idle => "IDLE",
        Status::Shell => "SHELL",
    }
}

impl App {
    /// Build one frame. Returns the quads and whether an animation is still running.
    pub fn frame(&mut self) -> (Vec<Quad>, bool) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        self.dt = dt;
        self.sys.tick();
        if let Some((from, t0)) = &self.theme_from {
            let p = t0.elapsed().as_secs_f32() / 0.45;
            if p >= 1.0 {
                self.theme = self.theme_target.clone();
                self.theme_from = None;
            } else {
                self.theme = from.lerp(&self.theme_target, ease_out_cubic(p));
            }
        }
        if self.glyphs.run_cache_len() > 4096 {
            self.glyphs.clear_runs();
        }
        let mut animating = false;
        let mut q = Vec::with_capacity(8192);
        let l = self.layout();

        self.background(&mut q, dt, &mut animating);
        self.rail(&mut q, &l, dt, &mut animating);
        self.panel(&mut q, &l);
        self.header(&mut q, &l);
        self.timeline_rail(&mut q, &l);
        animating |= self.ui.iter().any(|u| u.status == Status::Working);

        // Terminal. Switching sessions wipes the new one in behind a hairline.
        let pad = self.px(14.0);
        let (x0, x1) = (l.term.x - pad, l.term.x + l.term.w + pad);
        if let Some(tr) = &self.transition {
            let p = (now - tr.started).as_secs_f32() / 0.26;
            if p >= 1.0 {
                self.transition = None;
            } else {
                animating = true;
                let e = ease_out_cubic(p);
                let (from, dir) = (tr.from, tr.dir);
                let wx = if dir > 0.0 { x0 + (x1 - x0) * e } else { x1 - (x1 - x0) * e };
                let (new_clip, old_clip) = if dir > 0.0 { ((x0, wx), (wx, x1)) } else { ((wx, x1), (x0, wx)) };
                if let Some(fi) = self.panes.iter().position(|p| p.id == from) {
                    self.draw_view(&mut q, fi, l.term, 1.0 - 0.6 * e, false, Some(old_clip));
                }
                self.draw_view(&mut q, self.active, l.term, 1.0, true, Some(new_clip));
                let t = self.theme.clone();
                let (ty, th) = (l.header.y + l.header.h + self.px(1.0), l.panel.y + l.panel.h - l.header.y - l.header.h - self.px(2.0));
                q.push(Quad {
                    rect: [wx - self.px(28.0), ty, self.px(56.0), th],
                    color: with_alpha(t.accent2, 0.22 * (1.0 - e)),
                    params: [kind::GLOW, 0.0, 0.0, 0.0],
                    clip: NO_CLIP,
                    ..Default::default()
                });
                self.rect(&mut q, Rect { x: wx.round(), y: ty, w: self.px(1.0).max(1.0), h: th }, with_alpha(t.accent2, 1.0 - e * 0.5), 0.0, NO_CLIP);
            }
        }
        if self.transition.is_none() && !self.panes.is_empty() {
            animating |= self.draw_view(&mut q, self.active, l.term, 1.0, true, None);
            self.scroll_badge(&mut q, &l);
        }
        animating |= self.theme_from.is_some();

        self.overview_layer(&mut q, dt, &mut animating);
        self.toast_layer(&mut q, &l, &mut animating);
        self.help_layer(&mut q, dt, &mut animating);
        self.prompt_layer(&mut q, dt, &mut animating);
        (q, animating)
    }

    // ------------------------------------------------------------ primitives

    pub(super) fn rect(&self, q: &mut Vec<Quad>, r: Rect, c: Rgba, radius: f32, clip: [f32; 4]) {
        q.push(Quad { rect: [r.x, r.y, r.w, r.h], color: c, params: [kind::RECT, radius, 0.0, 0.0], clip, ..Default::default() });
    }

    /// A chamfered shape: `cuts` are the tl, tr, br, bl corner cuts in px.
    fn shape(&self, q: &mut Vec<Quad>, r: Rect, fill: Rgba, stroke: Option<Rgba>, cuts: [f32; 4]) {
        q.push(Quad {
            rect: [r.x, r.y, r.w, r.h],
            uv: cuts,
            color: fill,
            color2: stroke.unwrap_or([0.0; 4]),
            params: [kind::RECT, 0.0, if stroke.is_some() { self.px(1.0).max(1.0) } else { 0.0 }, 0.0],
            clip: NO_CLIP,
        });
    }

    fn text(&mut self, q: &mut Vec<Quad>, l: &TextLayout, x: f32, y: f32, c: Rgba, clip: [f32; 4]) {
        for g in &l.glyphs {
            let Some(a) = self.glyphs.atlas(g.key) else { continue };
            q.push(Quad {
                rect: [(x + g.x + a.left as f32).round(), (y + g.y - a.top as f32).round(), a.w as f32, a.h as f32],
                uv: [a.ax as f32, a.ay as f32, 0.0, 0.0],
                color: c,
                params: [if a.color { kind::COLOR_GLYPH } else { kind::GLYPH }, 0.0, 0.0, 0.0],
                clip,
                ..Default::default()
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn label(&mut self, q: &mut Vec<Quad>, s: &str, font: Font, size: f32, weight: u16, tracking: f32, x: f32, y: f32, c: Rgba) -> f32 {
        let l = self.glyphs.text(s, font, self.px(size), weight, self.px(tracking));
        self.text(q, &l, x, y, c, NO_CLIP);
        l.width
    }

    fn measure(&mut self, s: &str, font: Font, size: f32, weight: u16, tracking: f32) -> f32 {
        self.glyphs.text(s, font, self.px(size), weight, self.px(tracking)).width
    }

    /// Shorten `s` with an ellipsis until it fits `max_w` px.
    fn fit(&mut self, s: &str, font: Font, size: f32, weight: u16, max_w: f32) -> String {
        if self.measure(s, font, size, weight, 0.0) <= max_w {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let cand: String = chars[..mid].iter().collect::<String>().trim_end().to_string() + "…";
            if self.measure(&cand, font, size, weight, 0.0) <= max_w { lo = mid } else { hi = mid - 1 }
        }
        chars[..lo].iter().collect::<String>().trim_end().to_string() + "…"
    }

    /// Meta-text: segments joined by an accent `//`.
    fn meta(&mut self, q: &mut Vec<Quad>, parts: &[&str], x: f32, y: f32, col: Rgba) -> f32 {
        let sep = with_alpha(self.theme.accent, 0.8);
        let mut cx = x;
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                cx += self.px(7.0);
                cx += self.label(q, "//", Font::Mono, 9.5, 600, 0.0, cx, y, sep);
                cx += self.px(7.0);
            }
            cx += self.label(q, p, Font::Mono, 9.5, 500, 1.2, cx, y, col);
        }
        cx - x
    }

    /// A slanted status chip, right-aligned at `rx`. Returns its width.
    fn chip(&mut self, q: &mut Vec<Quad>, word: &str, col: Rgba, rx: f32, cy: f32, solid: bool) -> f32 {
        let k = self.glyphs.text(word, Font::Ui, self.px(8.5), 800, self.px(1.4));
        let h = self.px(16.0);
        let w = k.width + self.px(16.0);
        let cut = (h * 0.45).round();
        let r = Rect { x: rx - w, y: (cy - h / 2.0).round(), w, h };
        if solid {
            self.shape(q, r, col, None, [cut, 0.0, cut, 0.0]);
        } else {
            self.shape(q, r, with_alpha(col, 0.13), Some(with_alpha(col, 0.65)), [cut, 0.0, cut, 0.0]);
        }
        let tc = if solid { self.theme.bg } else { col };
        self.text(q, &k, r.x + self.px(8.0), cy - k.height / 2.0, tc, NO_CLIP);
        w
    }

    /// Pane `i`, or, when its tmux window is split, every pane of that
    /// window in its place: the others dimmed, hairlines in the gaps tmux
    /// leaves between them.
    fn draw_view(&mut self, q: &mut Vec<Quad>, i: usize, term: Rect, alpha: f32, primary: bool, clip_x: Option<(f32, f32)>) -> bool {
        let g = self.group(i);
        if g.len() < 2 {
            return self.draw_term(q, i, term, 0.0, alpha, primary, clip_x);
        }
        let t = self.theme.clone();
        let (cw, ch) = (self.glyphs.cell_w, self.glyphs.cell_h);
        let clip = clip_x.map_or([term.x - self.px(6.0), term.y, term.x + term.w + self.px(6.0), term.y + term.h], |(x0, x1)| [x0, term.y, x1, term.y + term.h]);
        let hair = self.px(1.0).max(1.0);
        let mut moving = false;
        for &j in &g {
            let a = self.pane_area(j, term);
            let focused = j == i;
            // Keep each pane inside its own cells, so wide neighbours don't bleed.
            let own = (a.x.max(clip[0]), (a.x + a.w + cw * 0.5).min(clip[2]));
            moving |= self.draw_term(q, j, a, 0.0, if focused { alpha } else { alpha * 0.55 }, primary && focused, Some(own));
            if a.x + a.w + cw < term.x + term.w {
                self.rect(q, Rect { x: (a.x + a.w + cw / 2.0).round(), y: a.y, w: hair, h: a.h }, t.border, 0.0, clip);
            }
            if a.y + a.h + ch < term.y + term.h {
                self.rect(q, Rect { x: a.x, y: (a.y + a.h + ch / 2.0).round(), w: a.w, h: hair }, t.border, 0.0, clip);
            }
        }
        moving
    }

    /// tmux's copy-mode position, `[lines up/history]`, top-right of the
    /// terminal while it is scrolled back.
    fn scroll_badge(&mut self, q: &mut Vec<Quad>, l: &Layout) {
        let Some(p) = self.panes.get(self.active) else { return };
        let (up, total) = {
            let term = p.term.lock();
            (term.grid().display_offset(), term.grid().history_size())
        };
        if up == 0 {
            return;
        }
        let t = self.theme.clone();
        let k = self.glyphs.text(&format!("[{up}/{total}]"), Font::Mono, self.px(11.0), 700, 0.0);
        let (px, py) = (self.px(7.0), self.px(3.0));
        let a = self.pane_area(self.active, l.term);
        let r = Rect { x: a.x + a.w - k.width - 2.0 * px, y: a.y, w: k.width + 2.0 * px, h: k.height + 2.0 * py };
        self.shape(q, r, t.accent, None, [0.0, 0.0, 0.0, (r.h * 0.45).round()]);
        self.text(q, &k, r.x + px, r.y + py, t.bg, NO_CLIP);
    }

    /// Segmented meter, slanted segments; colour climbs with the value.
    fn meter(&mut self, q: &mut Vec<Quad>, x: f32, y: f32, w: f32, v: f32) {
        let t = self.theme.clone();
        let n = 24;
        let gap = self.px(2.0);
        let sw = ((w - gap * (n - 1) as f32) / n as f32).floor();
        let h = self.px(6.0);
        let lit = (v.clamp(0.0, 1.0) * n as f32).round() as usize;
        let col = if v > 0.9 { t.accent } else if v > 0.7 { t.warning } else { t.accent2 };
        for i in 0..n {
            let c = if i < lit { col } else { with_alpha(t.border, 0.8) };
            let r = Rect { x: x + i as f32 * (sw + gap), y, w: sw, h };
            self.shape(q, r, c, None, [0.0, (sw * 0.5).floor(), 0.0, (sw * 0.5).floor()]);
        }
    }

    // ------------------------------------------------------------ regions

    fn background(&mut self, q: &mut Vec<Quad>, dt: f32, animating: &mut bool) {
        let t = self.theme.clone();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);

        // Ambient light follows the most urgent state across all sessions.
        let has = |s: Status| self.ui.iter().any(|u| u.status == s);
        let target = if has(Status::Blocked) {
            with_alpha(t.alert, 0.20)
        } else if has(Status::Working) {
            with_alpha(t.busy, 0.13)
        } else if has(Status::YourTurn) {
            with_alpha(t.ready, 0.09)
        } else {
            with_alpha(t.ansi[5], 0.07)
        };
        let k = 1.0 - (-dt * 2.2).exp();
        let prev = self.ambient;
        self.ambient = mix(self.ambient, target, k);
        if prev.iter().zip(target.iter()).any(|(a, b)| (a - b).abs() > 0.002) {
            *animating = true;
        }

        q.push(Quad {
            rect: [-0.55 * w, 0.25 * h, 1.3 * w, 1.5 * h],
            color: self.ambient,
            params: [kind::GLOW, 0.0, 0.0, 0.0],
            clip: NO_CLIP,
            ..Default::default()
        });
        q.push(Quad {
            rect: [0.55 * w, -0.6 * h, 0.9 * w, 1.2 * h],
            color: with_alpha(t.ansi[5], 0.06),
            params: [kind::GLOW, 0.0, 0.0, 0.0],
            clip: NO_CLIP,
            ..Default::default()
        });
        q.push(Quad {
            rect: [0.0, 0.0, w, h],
            color: with_alpha(t.muted, 0.16),
            params: [kind::DOTS, self.px(24.0), self.px(1.5), 0.0],
            clip: NO_CLIP,
            ..Default::default()
        });
    }

    fn panel(&mut self, q: &mut Vec<Quad>, l: &Layout) {
        let t = self.theme.clone();
        let r = l.panel;
        let c = self.px(18.0);
        let cuts = [0.0, c, 0.0, c];
        q.push(Quad {
            rect: [r.x, r.y + self.px(8.0), r.w, r.h],
            uv: cuts,
            color: [0.0, 0.0, 0.0, 0.55],
            params: [kind::SHADOW, 0.0, 0.0, self.px(18.0)],
            clip: NO_CLIP,
            ..Default::default()
        });
        self.shape(q, r, with_alpha(t.surface, self.opacity), Some(t.border), cuts);
        if self.focused {
            q.push(Quad {
                rect: [r.x, r.y, r.w, r.h],
                uv: cuts,
                color: with_alpha(t.accent, 0.85),
                color2: with_alpha(t.accent2, 0.85),
                params: [kind::GRADIENT_STROKE, 0.0, self.px(1.0).max(1.0), 0.0],
                clip: NO_CLIP,
                ..Default::default()
            });
            // Registration brackets on the square corners.
            let (a, b, o) = (self.px(14.0), self.px(2.0), self.px(5.0));
            self.rect(q, Rect { x: r.x - o, y: r.y - o, w: a, h: b }, t.accent, 0.0, NO_CLIP);
            self.rect(q, Rect { x: r.x - o, y: r.y - o, w: b, h: a }, t.accent, 0.0, NO_CLIP);
            let (rx, ry) = (r.x + r.w + o, r.y + r.h + o);
            self.rect(q, Rect { x: rx - a, y: ry - b, w: a, h: b }, t.accent2, 0.0, NO_CLIP);
            self.rect(q, Rect { x: rx - b, y: ry - a, w: b, h: a }, t.accent2, 0.0, NO_CLIP);
        }
    }

    fn header(&mut self, q: &mut Vec<Quad>, l: &Layout) {
        let t = self.theme.clone();
        let h = l.header;
        let Some(p) = self.panes.get(self.active) else { return };
        let status = self.ui[self.active].status;
        let title = if p.title.is_empty() { p.subtitle.clone() } else { super::clean_title(&p.title) };
        let dims = format!("{}×{}", p.size.cols, p.size.lines);
        let prog = p.subtitle.to_uppercase();
        let host = p.host;
        let host_name = self.hosts.get(host).map(|h| h.name.to_uppercase()).unwrap_or_default();
        let host_col = self.host_color(host);
        let idx = format!("{:02}", self.active + 1);

        // Index tab, flush in the square corner, filled with the session's state.
        let tab_w = self.px(54.0);
        let tab_fill = match status {
            Status::Shell | Status::Idle => t.surface_alt,
            s => self.status_color(s),
        };
        let tab_fg = if matches!(status, Status::Shell | Status::Idle) { t.fg } else { t.bg };
        self.shape(q, Rect { x: h.x, y: h.y, w: tab_w, h: h.h }, tab_fill, None, [0.0, 0.0, self.px(14.0), 0.0]);
        let il = self.glyphs.text(&idx, Font::Mono, self.px(15.0), 800, 0.0);
        self.text(q, &il, h.x + (tab_w - self.px(8.0) - il.width) / 2.0, h.y + (h.h - il.height) / 2.0, tab_fg, NO_CLIP);

        let x = h.x + tab_w + self.px(16.0);
        let right_reserve = self.px(220.0);
        let title = self.fit(&title, Font::Ui, 14.0, 650, h.w - tab_w - right_reserve);
        let tl = self.glyphs.text(&title, Font::Ui, self.px(14.0), 650, 0.0);
        let clip = [x, h.y, h.x + h.w - right_reserve, h.y + h.h];
        self.text(q, &tl, x, h.y + self.px(7.0), t.fg, clip);
        self.meta(q, &[&host_name, &prog, &dims], x, h.y + self.px(28.0), t.muted);

        // Right: state chip, then host chip.
        let cy = h.y + h.h / 2.0;
        let mut rx = h.x + h.w - self.px(40.0);
        rx -= self.chip(q, &host_name, host_col, rx, cy, false) + self.px(8.0);
        if self.leader.is_some_and(|t| t.elapsed().as_secs() < 3) {
            rx -= self.chip(q, "LEADER", t.accent, rx, cy, true) + self.px(8.0);
        }
        if status != Status::Shell {
            let col = self.status_color(status);
            let w = self.chip(q, status_word(status), col, rx, cy, true);
            if status == Status::Working {
                let time = self.epoch.elapsed().as_secs_f32();
                let f = SPINNER[((time * 12.0) as usize) % SPINNER.len()];
                let k = self.glyphs.text(f, Font::Mono, self.px(14.0), 400, 0.0);
                self.text(q, &k, rx - w - self.px(6.0) - k.width, cy - k.height / 2.0, col, NO_CLIP);
            }
        }

        // Base hairline.
        let by = h.y + h.h;
        self.rect(q, Rect { x: h.x, y: by, w: h.w - self.px(18.0), h: self.px(1.0).max(1.0) }, t.border, 0.0, NO_CLIP);
    }

    fn rail(&mut self, q: &mut Vec<Quad>, l: &Layout, dt: f32, animating: &mut bool) {
        let t = self.theme.clone();
        let r = l.rail;
        let c = self.px(18.0);
        let cuts = [c, 0.0, c, 0.0];
        q.push(Quad {
            rect: [r.x, r.y + self.px(8.0), r.w, r.h],
            uv: cuts,
            color: [0.0, 0.0, 0.0, 0.45],
            params: [kind::SHADOW, 0.0, 0.0, self.px(16.0)],
            clip: NO_CLIP,
            ..Default::default()
        });
        self.shape(q, r, with_alpha(t.surface, 0.78 * self.opacity), Some(t.border), cuts);
        let x = r.x + self.px(20.0);
        let right = r.x + r.w - self.px(18.0);

        // Wordmark.
        let pw = self.label(q, "PUNK", Font::Ui, 14.0, 800, 3.4, x, r.y + self.px(16.0), t.accent);
        self.label(q, "TERM", Font::Ui, 14.0, 800, 3.4, x + pw + self.px(3.4), r.y + self.px(16.0), t.fg);

        // Status tally, right-aligned.
        let mut rx = right;
        for st in [Status::Working, Status::YourTurn, Status::Blocked] {
            let k = self.ui.iter().filter(|u| u.status == st).count();
            if k == 0 {
                continue;
            }
            let s = format!("{k}");
            rx -= self.measure(&s, Font::Mono, 12.0, 700, 0.0);
            let col = self.status_color(st);
            self.label(q, &s, Font::Mono, 12.0, 700, 0.0, rx, r.y + self.px(17.0), col);
            let d = self.px(6.0);
            rx -= d + self.px(5.0);
            self.shape(q, Rect { x: rx, y: r.y + self.px(24.0) - d / 2.0, w: d, h: d }, col, None, [d / 2.0, 0.0, d / 2.0, 0.0]);
            rx -= self.px(12.0);
        }

        // System line + clock.
        let my = r.y + self.px(40.0);
        self.meta(q, &["SYS", "ONLINE"], x, my, t.muted);
        let clock = self.sys.clock();
        let cw = self.measure(&clock, Font::Mono, 10.0, 500, 0.8);
        self.label(q, &clock, Font::Mono, 10.0, 500, 0.8, right - cw, my, mix(t.fg, t.muted, 0.4));
        let hy = r.y + self.px(62.0);
        self.rect(q, Rect { x, y: hy, w: right - x, h: self.px(1.0).max(1.0) }, t.border, 0.0, NO_CLIP);
        q.push(Quad {
            rect: [x, hy - self.px(0.5), self.px(44.0), self.px(2.0)],
            color: t.accent,
            color2: t.accent2,
            params: [kind::GRADIENT, 0.0, 0.0, 0.0],
            clip: NO_CLIP,
            ..Default::default()
        });

        // Host sections.
        let n = self.panes.len();
        for &(h, sy) in &l.sections {
            let hcol = self.host_color(h);
            let num = format!("{:02}", h + 1);
            let name = self.hosts[h].name.to_uppercase();
            let k = self.panes.iter().filter(|p| p.host == h).count();
            let w1 = self.label(q, &num, Font::Mono, 9.5, 700, 0.0, x, sy, hcol);
            let hx = x + w1 + self.px(8.0);
            let hw = self.label(q, &name, Font::Ui, 10.0, 700, 2.4, hx, sy, t.fg);
            // "+": a new shell on this host.
            let a = l.adds[h];
            let cut = self.px(5.0);
            self.shape(q, a, with_alpha(hcol, 0.13), Some(with_alpha(hcol, 0.65)), [cut, 0.0, cut, 0.0]);
            let arm = self.px(4.0);
            let th = self.px(1.5).max(1.0);
            let (cx, cy) = ((a.x + a.w / 2.0).round(), (a.y + a.h / 2.0).round());
            self.rect(q, Rect { x: cx - arm, y: cy - th / 2.0, w: 2.0 * arm, h: th }, hcol, 0.0, NO_CLIP);
            self.rect(q, Rect { x: cx - th / 2.0, y: cy - arm, w: th, h: 2.0 * arm }, hcol, 0.0, NO_CLIP);
            let count = format!("{k:02} SES");
            let cw = self.measure(&count, Font::Mono, 9.5, 500, 1.0);
            let count_r = a.x - self.px(8.0);
            self.label(q, &count, Font::Mono, 9.5, 500, 1.0, count_r - cw, sy, t.muted);
            let lx = hx + hw + self.px(10.0);
            let lw = count_r - cw - self.px(10.0) - lx;
            if lw > 0.0 {
                self.rect(q, Rect { x: lx, y: sy + self.px(7.0), w: lw, h: self.px(1.0).max(1.0) }, t.border, 0.0, NO_CLIP);
            }
            // A remote host with nothing to show says why.
            if h > 0 && k == 0 {
                let (msg, col) = match self.host_state.get(h).cloned().unwrap_or(HostState::Connecting) {
                    HostState::Connecting | HostState::Up => ("CONNECTING…".to_string(), t.busy),
                    HostState::Empty => ("NO TMUX SESSIONS // ⌃⇧T STARTS ONE".to_string(), t.muted),
                    HostState::Failed(e) => (format!("✕ {}", e.to_uppercase()), t.alert),
                };
                let msg = self.fit(&msg, Font::Mono, 9.0, 600, right - hx);
                self.label(q, &msg, Font::Mono, 9.0, 600, 0.0, hx, sy + self.px(18.0), col);
            }
        }

        // Active indicator glides between cards.
        if let Some(card) = l.cards.get(self.active) {
            if self.indicator.target == 0.0 && self.indicator.value == 0.0 {
                self.indicator.snap(card.y);
            }
            self.indicator.set(card.y);
        }
        *animating |= self.indicator.step(dt);
        let time = self.epoch.elapsed().as_secs_f32();

        for i in 0..n {
            let card = l.cards[i];
            let active = i == self.active;
            let ui = &mut self.ui[i];
            if ui.appear < 1.0 {
                ui.appear = (ui.appear + dt / 0.3).min(1.0);
                *animating = true;
            }
            if ui.bell > 0.0 {
                ui.bell = (ui.bell - dt / 0.9).max(0.0);
                *animating = true;
            }
            let (appear, bell, unseen, status, since) = (ease_out_cubic(ui.appear), ui.bell, ui.unseen, ui.status, ui.since);
            let activity = ui.activity.clone();
            let bg = ui.background.as_ref().map(|b| format!(" · {b}")).unwrap_or_default();
            let c = Rect { x: card.x + (1.0 - appear) * self.px(-14.0), ..card };
            let scol = self.status_color(status);
            let cut = self.px(10.0);
            if active {
                self.shape(q, c, t.surface_alt, Some(with_alpha(t.border, 0.95)), [cut, 0.0, 0.0, 0.0]);
            } else {
                self.shape(q, c, with_alpha(t.surface_alt, 0.35 * appear), None, [cut, 0.0, 0.0, 0.0]);
            }
            if status == Status::Blocked {
                let breathe = 0.5 + 0.5 * (time * std::f32::consts::TAU / 1.8).sin();
                self.shape(q, c, with_alpha(t.alert, 0.05 + 0.06 * breathe), Some(with_alpha(t.alert, 0.35 + 0.45 * breathe)), [cut, 0.0, 0.0, 0.0]);
                *animating = true;
            }
            if bell > 0.0 {
                self.shape(q, c, with_alpha(t.accent, 0.22 * bell), None, [cut, 0.0, 0.0, 0.0]);
            }

            let p = &self.panes[i];
            let title = if p.title.is_empty() { p.subtitle.clone() } else { super::clean_title(&p.title) };
            let sub = match status {
                Status::Working => activity.unwrap_or_else(|| "working".into()),
                Status::Blocked => "awaiting permission".into(),
                Status::YourTurn => format!("turn complete · {}{bg}", super::ago(since)),
                Status::Idle => format!("idle · {}{bg}", super::ago(since)),
                Status::Shell => format!("{} · {}×{}", p.subtitle, p.size.cols, p.size.lines),
            };
            let sub_col = match status {
                Status::Shell | Status::Idle => t.muted,
                s => with_alpha(self.status_color(s), 0.85),
            };
            // Re-decode the subtitle whenever it changes.
            let ui = &mut self.ui[i];
            if ui.sub_shown != sub {
                ui.sub_shown = sub.clone();
                ui.sub_since = Instant::now();
            }
            let (sub_txt, decoding) = decode(&super::truncate(&sub, 34), ui.sub_since.elapsed().as_secs_f32());
            *animating |= decoding;

            let emphasis = active || unseen || matches!(status, Status::Blocked | Status::YourTurn);
            let fg = with_alpha(if emphasis { t.fg } else { mix(t.fg, t.muted, 0.35) }, appear);
            let num = format!("{:02}", i + 1);
            let ncol = if active { t.accent } else { with_alpha(t.muted, 0.8) };
            let compact = c.h < self.px(40.0);
            let row_y = if compact { c.y + self.px(7.0) } else { c.y + self.px(11.0) };
            self.label(q, &num, Font::Mono, 10.5, 700, 0.0, c.x + self.px(14.0), row_y, with_alpha(ncol, appear));

            let tx = c.x + self.px(42.0);
            let chip_w = if status == Status::Shell { self.px(16.0) } else { self.px(66.0) };
            let clip = [tx, c.y, c.x + c.w - chip_w - self.px(10.0), c.y + c.h];
            let weight = if emphasis { 600 } else { 500 };
            let title = self.fit(&title, Font::Ui, 13.0, weight, clip[2] - tx);
            let title_l = self.glyphs.text(&title, Font::Ui, self.px(13.0), weight, 0.0);
            self.text(q, &title_l, tx, if compact { c.y + self.px(6.0) } else { c.y + self.px(9.0) }, fg, clip);
            if !compact {
                let sub_l = self.glyphs.text(&sub_txt, Font::Mono, self.px(10.5), 400, 0.0);
                let sub_clip = [tx, c.y, c.x + c.w - self.px(12.0), c.y + c.h];
                self.text(q, &sub_l, tx, c.y + self.px(31.0), with_alpha(sub_col, appear), sub_clip);
            }

            let rx = c.x + c.w - self.px(12.0);
            let cy = if compact { c.y + c.h / 2.0 } else { c.y + self.px(19.0) };
            match status {
                Status::Shell => {
                    if unseen {
                        let d = self.px(6.0);
                        self.shape(q, Rect { x: rx - d, y: cy - d / 2.0, w: d, h: d }, t.accent2, None, [d / 2.0, 0.0, d / 2.0, 0.0]);
                    }
                }
                s => {
                    let w = self.chip(q, status_word(s), scol, rx, cy, s == Status::Blocked);
                    if s == Status::Working {
                        let f = SPINNER[((time * 12.0) as usize) % SPINNER.len()];
                        let k = self.glyphs.text(f, Font::Mono, self.px(13.0), 400, 0.0);
                        self.text(q, &k, rx - w - self.px(5.0) - k.width, cy - k.height / 2.0, scol, NO_CLIP);
                    }
                }
            }
        }

        // A bracket down the left of each split window's cards; in the
        // accent when that window is the one on screen.
        let mut i = 0;
        while i < n {
            let Some(key) = self.split_key(i) else {
                i += 1;
                continue;
            };
            let mut j = i;
            while j + 1 < n && self.split_key(j + 1) == Some(key) {
                j += 1;
            }
            let (top, bot) = (l.cards[i].y + self.px(6.0), l.cards[j].y + l.cards[j].h - self.px(6.0));
            let on = (i..=j).contains(&self.active);
            let col = if on { with_alpha(t.accent, 0.8) } else { with_alpha(t.muted, 0.6) };
            let bx = l.cards[i].x - self.px(6.0);
            let th = self.px(1.5).max(1.0);
            let tick = self.px(4.0);
            self.rect(q, Rect { x: bx, y: top, w: th, h: bot - top }, col, 0.0, NO_CLIP);
            self.rect(q, Rect { x: bx, y: top, w: tick, h: th }, col, 0.0, NO_CLIP);
            self.rect(q, Rect { x: bx, y: bot - th, w: tick, h: th }, col, 0.0, NO_CLIP);
            i = j + 1;
        }

        if !l.cards.is_empty() {
            let c = l.cards[self.active.min(l.cards.len() - 1)];
            let inset = if c.h < self.px(40.0) { self.px(8.0) } else { self.px(14.0) };
            let ih = c.h - inset - self.px(10.0);
            self.rect(q, Rect { x: c.x, y: self.indicator.value + inset, w: self.px(2.0), h: ih }, t.accent, 0.0, NO_CLIP);
        }

        // Footer: live meters, then key hints.
        let fy = r.y + r.h - self.px(108.0);
        // Claude plan usage for the active host's account, as /usage shows it.
        if let Some((host, u)) = self.plan() {
            let py = fy - self.plan_h();
            self.rect(q, Rect { x, y: py, w: right - x, h: self.px(1.0).max(1.0) }, t.border, 0.0, NO_CLIP);
            let _ = host;
            let age = crate::usage::now_ms().saturating_sub(u.at_ms) / 60_000;
            // Claude Code refreshes these only while sessions make requests.
            let stale = match age {
                0..15 => String::new(),
                15..120 => format!("{age}M AGO"),
                120..2880 => format!("{}H AGO", age / 60),
                _ => format!("{}D AGO", age / 1440),
            };
            // PLAN // account email, then the reading's age when it is stale.
            let mut hx = x + self.meta(q, &["PLAN"], x, py + self.px(12.0), t.muted);
            let tail_w = if stale.is_empty() { 0.0 } else { self.measure(&stale, Font::Mono, 9.5, 500, 1.2) + self.px(28.0) };
            if let Some(email) = &u.account {
                hx += self.px(7.0);
                hx += self.label(q, "//", Font::Mono, 9.5, 600, 0.0, hx, py + self.px(12.0), with_alpha(t.accent, 0.8)) + self.px(7.0);
                let e = self.fit(email, Font::Mono, 9.5, 500, right - hx - tail_w);
                hx += self.label(q, &e, Font::Mono, 9.5, 500, 0.0, hx, py + self.px(12.0), mix(t.fg, t.muted, 0.35));
            }
            if !stale.is_empty() {
                let sw = self.measure(&stale, Font::Mono, 9.5, 500, 1.2);
                let _ = hx;
                self.label(q, &stale, Font::Mono, 9.5, 500, 1.2, right - sw, py + self.px(12.0), t.muted);
            }
            for (k, row) in u.rows.iter().enumerate() {
                let y = py + self.px(32.0) + k as f32 * self.px(20.0);
                let v = row.pct / 100.0;
                let name = self.fit(&row.label.to_uppercase(), Font::Mono, 9.5, 600, self.px(40.0));
                self.label(q, &name, Font::Mono, 9.5, 600, 0.0, x, y, t.muted);
                // A window that has reset since this reading started over;
                // its old percentage no longer means anything.
                let reset = row.resets_ms != 0 && row.resets_ms <= crate::usage::now_ms();
                let v = if reset { 0.0 } else { v };
                let left = crate::usage::until(row.resets_ms);
                let tail = if reset {
                    "RESET".to_string()
                } else if left.is_empty() {
                    format!("{:>3}%", row.pct.round() as i32)
                } else {
                    format!("{:>3}% {left:>4}", row.pct.round() as i32)
                };
                let pw = self.measure(&tail, Font::Mono, 9.5, 500, 0.0);
                let col = if row.severity != "normal" || v > 0.9 { t.alert } else { mix(t.fg, t.muted, 0.4) };
                self.label(q, &tail, Font::Mono, 9.5, 500, 0.0, right - pw, y, col);
                let mx = x + self.px(46.0);
                self.meter(q, mx, y + self.px(3.0), right - pw - self.px(10.0) - mx, v);
            }
        }
        self.rect(q, Rect { x, y: fy, w: right - x, h: self.px(1.0).max(1.0) }, t.border, 0.0, NO_CLIP);
        let (cpu, mem) = (self.sys.cpu, self.sys.mem);
        for (k, (name, v)) in [("CPU", cpu), ("MEM", mem)].into_iter().enumerate() {
            let y = fy + self.px(14.0) + k as f32 * self.px(20.0);
            self.label(q, name, Font::Mono, 9.5, 600, 1.2, x, y, t.muted);
            let pct = format!("{:>3}%", (v * 100.0).round() as i32);
            let pw = self.measure(&pct, Font::Mono, 9.5, 500, 0.0);
            self.label(q, &pct, Font::Mono, 9.5, 500, 0.0, right - pw, y, mix(t.fg, t.muted, 0.4));
            let mx = x + self.px(46.0);
            self.meter(q, mx, y + self.px(3.0), right - pw - self.px(10.0) - mx, v);
        }
        let ky = r.y + r.h - self.px(36.0);
        self.rect(q, Rect { x, y: ky - self.px(12.0), w: right - x, h: self.px(1.0).max(1.0) }, t.border, 0.0, NO_CLIP);
        let mut kx = x;
        for (k, v) in [("⌃⇧D", "GRID"), ("⌃⇧␣", "NEEDS YOU"), ("⌃?", "HELP")] {
            kx += self.label(q, k, Font::Mono, 10.5, 700, 0.0, kx, ky, t.fg) + self.px(5.0);
            kx += self.label(q, v, Font::Ui, 9.0, 600, 1.2, kx, ky + self.px(1.0), t.muted) + self.px(12.0);
        }
    }
}

// ---------------------------------------------------------------- overlays

const KEYS_SESSIONS: &[(&str, &str)] = &[
    ("⌃⇧T", "New shell · this host (tmux window)"),
    ("⌃⇧E", "Split: shell beside this pane"),
    ("⌃⇧N", "Connect to a host (ssh)"),
    ("⌃⇧R", "Reconnect hosts"),
    ("⌃⇧W", "Detach session (keeps running)"),
    ("⌃]  ⌃[", "Next / previous session"),
    ("⌃⇥  ⌃⇧⇥", "Next / previous session"),
    ("⌃⇧1–9", "Jump to session"),
    ("⌃⇧␣", "Next session that needs you"),
    ("⌃⇧D", "Session grid (dashboard)"),
    ("⌃⇧H", "Conversation timeline (right rail)"),
];

const KEYS_VIEW: &[(&str, &str)] = &[
    ("⌃⇧P", "Cycle theme"),
    ("⌃⇧O", "Cycle opacity"),
    ("⌃⇧S", "Sound: off / alerts / all"),
    ("⌃=  ⌃-  ⌃0", "Font size"),
    ("wheel  ⇧wheel", "Scroll · punkterm history"),
    ("Drag  ⌃⇧C", "Select · copy"),
    ("⌃click", "Open link"),
    ("⌃⇧V  ⇧Ins", "Paste"),
    ("⌃?", "This help"),
    ("⌃␣ v  ⌃␣ s", "Split side by side · stacked"),
    ("⌃␣ hjkl", "Move between split panes"),
    ("⌥ hjkl", "Move between split panes"),
    ("⌃␣ z", "Zoom pane · ⌃␣ c n p"),
];

const LEGEND: &[(Status, &str)] = &[
    (Status::Working, "Claude is working"),
    (Status::Blocked, "Waiting on a permission prompt"),
    (Status::YourTurn, "Turn complete — your move"),
    (Status::Idle, "Claude open, nothing pending"),
];

impl App {
    /// The conversation in the active pane, newest at the bottom: your
    /// prompts, Claude's replies, and runs of tool calls folded into one
    /// line. Collapsed, a strip of ticks, one per event.
    fn timeline_rail(&mut self, q: &mut Vec<Quad>, l: &Layout) {
        use crate::timeline::Kind;
        let t = self.theme.clone();
        let r = l.timeline;
        let c = self.px(18.0);
        let cuts = [0.0, c, 0.0, c];
        q.push(Quad {
            rect: [r.x, r.y + self.px(8.0), r.w, r.h],
            uv: cuts,
            color: [0.0, 0.0, 0.0, 0.45],
            params: [kind::SHADOW, 0.0, 0.0, self.px(16.0)],
            clip: NO_CLIP,
            ..Default::default()
        });
        self.shape(q, r, with_alpha(t.surface, 0.78 * self.opacity), Some(t.border), cuts);
        let tl = self.active_timeline().cloned();
        // Consecutive tool calls fold into one item: (kind, at, text, tools).
        let mut items: Vec<(Kind, u64, String, usize)> = Vec::new();
        for e in tl.iter().flat_map(|t| &t.entries) {
            match items.last_mut() {
                Some((Kind::Tool, at, text, n)) if e.kind == Kind::Tool => {
                    *at = e.at_ms;
                    *text = e.text.clone();
                    *n += 1;
                }
                _ => items.push((e.kind, e.at_ms, e.text.clone(), 1)),
            }
        }
        let tone = |k: Kind| match k {
            Kind::Ask => t.accent,
            Kind::Say => t.accent2,
            Kind::Tool => t.muted,
        };

        if !self.timeline_open {
            let label = self.measure("‹", Font::Ui, 14.0, 700, 0.0);
            self.label(q, "‹", Font::Ui, 14.0, 700, 0.0, r.x + (r.w - label) / 2.0, r.y + self.px(14.0), t.muted);
            let (step, top) = (self.px(6.0), r.y + self.px(44.0));
            let mut y = r.y + r.h - self.px(20.0);
            for (k, ..) in items.iter().rev() {
                if y < top {
                    break;
                }
                let w = self.px(match k { Kind::Ask => 14.0, Kind::Say => 9.0, Kind::Tool => 4.0 });
                self.rect(q, Rect { x: r.x + (r.w - w) / 2.0, y, w, h: self.px(2.0).max(1.0) }, with_alpha(tone(*k), 0.85), 0.0, NO_CLIP);
                y -= step;
            }
            return;
        }

        let x = r.x + self.px(18.0);
        let right = r.x + r.w - self.px(18.0);
        let pw = self.label(q, "TIME", Font::Ui, 12.0, 800, 3.0, x, r.y + self.px(17.0), t.accent);
        self.label(q, "LINE", Font::Ui, 12.0, 800, 3.0, x + pw + self.px(3.0), r.y + self.px(17.0), t.fg);
        let keys = "⌃⇧H";
        let kw = self.measure(keys, Font::Mono, 9.5, 500, 0.0);
        self.label(q, keys, Font::Mono, 9.5, 500, 0.0, right - kw, r.y + self.px(19.0), t.muted);
        let asks = items.iter().filter(|i| i.0 == Kind::Ask).count();
        let title = tl.as_ref().map(|t| t.title.to_uppercase()).filter(|s| !s.is_empty()).unwrap_or_else(|| "SESSION".into());
        let count = format!("{asks} ASKED");
        let room = right - x - self.measure(&count, Font::Mono, 9.5, 500, 1.2) - self.px(30.0);
        let title = self.fit(&title, Font::Mono, 9.5, 500, room);
        self.meta(q, &[&title, &count], x, r.y + self.px(40.0), t.muted);
        let hy = r.y + self.px(62.0);
        self.rect(q, Rect { x, y: hy, w: right - x, h: self.px(1.0).max(1.0) }, t.border, 0.0, NO_CLIP);

        let body = Rect { x, y: hy + self.px(12.0), w: right - x, h: (r.y + r.h - self.px(18.0) - hy - self.px(12.0)).max(0.0) };
        if items.is_empty() {
            let msg = if self.active_pane().is_some_and(|p| p.tmux_pane.is_some()) { "NO CLAUDE SESSION HERE" } else { "NOT A TMUX PANE" };
            self.label(q, msg, Font::Mono, 9.5, 500, 1.2, x, body.y + self.px(4.0), t.muted);
            return;
        }
        let tx = x + self.px(16.0);
        let size = 11.0;
        let cw = self.measure("0", Font::Mono, size, 400, 0.0).max(1.0);
        let per = (((right - tx) / cw) as usize).max(8);
        let (tag_h, line_h, gap) = (self.px(15.0), self.px(16.0), self.px(12.0));
        let laid: Vec<Vec<String>> = items
            .iter()
            .map(|(k, _, text, _)| {
                let text = text.replace("**", "").replace('`', "");
                match k {
                    Kind::Ask => wrap(&text, per, 4),
                    Kind::Say => wrap(&text, per, 3),
                    Kind::Tool if text.chars().count() > per => vec![text.chars().take(per - 1).chain(['…']).collect()],
                    Kind::Tool => vec![text],
                }
            })
            .collect();
        let total: f32 = laid.iter().map(|ls| tag_h + ls.len() as f32 * line_h + gap).sum();
        self.tl_scroll = self.tl_scroll.min((total - body.h).max(0.0));
        let clip = [body.x - self.px(8.0), body.y, right + self.px(2.0), body.y + body.h];
        self.tl_hits.clear();
        self.rect(q, Rect { x: x + self.px(3.0), y: body.y, w: self.px(1.0).max(1.0), h: body.h }, with_alpha(t.border, 0.9), 0.0, clip);
        let mut bottom = body.y + body.h + self.tl_scroll;
        for (i, (k, at, _, n)) in items.iter().enumerate().rev() {
            let h = tag_h + laid[i].len() as f32 * line_h + gap;
            let top = bottom - h;
            bottom = top;
            if top > body.y + body.h {
                continue;
            }
            if top + h < body.y {
                break;
            }
            let col = tone(*k);
            let (y0, y1) = (top.max(body.y), (top + h - gap / 2.0).min(body.y + body.h));
            self.tl_hits.push((Rect { x: r.x, y: y0, w: r.w, h: y1 - y0 }, *k, items[i].2.clone()));
            if self.hover_tl.is_some_and(|(hx, hy)| hx >= r.x && hy >= y0 && hy < y1) {
                self.rect(q, Rect { x: x + self.px(10.0), y: y0 - self.px(3.0), w: right - x - self.px(6.0), h: y1 - y0 }, with_alpha(t.accent, 0.07), self.px(3.0), clip);
            }
            let d = self.px(if *k == Kind::Ask { 9.0 } else { 7.0 });
            let dy = top + tag_h / 2.0 - d / 2.0;
            q.push(Quad {
                rect: [x + self.px(3.5) - d / 2.0, dy, d, d],
                uv: [d / 2.0, 0.0, d / 2.0, 0.0],
                color: if *k == Kind::Tool { t.surface } else { col },
                color2: col,
                params: [kind::RECT, 0.0, self.px(1.0).max(1.0), 0.0],
                clip,
                ..Default::default()
            });
            let tag = match k {
                Kind::Ask => "YOU".to_string(),
                Kind::Say => "CLAUDE".to_string(),
                Kind::Tool if *n > 1 => format!("{n} TOOLS"),
                Kind::Tool => "TOOL".to_string(),
            };
            let g = self.glyphs.text(&tag, Font::Ui, self.px(8.5), 800, self.px(1.4));
            self.text(q, &g, tx, top + tag_h / 2.0 - g.height / 2.0, col, clip);
            if *at > 0 {
                let when = self.sys.stamp(*at);
                let g = self.glyphs.text(&when, Font::Mono, self.px(9.0), 500, self.px(0.6));
                self.text(q, &g, right - g.width, top + tag_h / 2.0 - g.height / 2.0, t.muted, clip);
            }
            let fg = match k {
                Kind::Ask => t.fg,
                Kind::Say => mix(t.fg, t.muted, 0.3),
                Kind::Tool => t.muted,
            };
            for (j, line) in laid[i].iter().enumerate() {
                let g = self.glyphs.text(line, Font::Mono, self.px(size), 400, 0.0);
                self.text(q, &g, tx, top + tag_h + j as f32 * line_h + (line_h - g.height) / 2.0, fg, clip);
            }
        }
    }

    fn fade(v: &mut f32, open: bool, dt: f32, secs: f32, animating: &mut bool) -> f32 {
        let target = if open { 1.0 } else { 0.0 };
        if (*v - target).abs() > 0.0001 {
            *v = if open { (*v + dt / secs).min(1.0) } else { (*v - dt / secs).max(0.0) };
            *animating = true;
        }
        ease_out_cubic(*v)
    }

    fn help_layer(&mut self, q: &mut Vec<Quad>, dt: f32, animating: &mut bool) {
        let mut ht = self.help_t;
        let e = Self::fade(&mut ht, self.help, dt, 0.18, animating);
        self.help_t = ht;
        if e <= 0.0 {
            return;
        }
        let t = self.theme.clone();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        self.rect(q, Rect { x: 0.0, y: 0.0, w, h }, with_alpha(t.bg, 0.72 * e), 0.0, NO_CLIP);

        let pw = self.px(760.0).min(w - self.px(48.0));
        let ph = self.px(540.0).min(h - self.px(48.0));
        let r = Rect { x: ((w - pw) / 2.0).round(), y: ((h - ph) / 2.0 + (1.0 - e) * self.px(14.0)).round(), w: pw, h: ph };
        let c = self.px(18.0);
        let cuts = [c, 0.0, c, 0.0];
        q.push(Quad {
            rect: [r.x, r.y + self.px(10.0), r.w, r.h],
            uv: cuts,
            color: [0.0, 0.0, 0.0, 0.6 * e],
            params: [kind::SHADOW, 0.0, 0.0, self.px(24.0)],
            clip: NO_CLIP,
            ..Default::default()
        });
        self.shape(q, r, with_alpha(t.surface, 0.98 * e), None, cuts);
        q.push(Quad {
            rect: [r.x, r.y, r.w, r.h],
            uv: cuts,
            color: with_alpha(t.accent, e),
            color2: with_alpha(t.accent2, e),
            params: [kind::GRADIENT_STROKE, 0.0, self.px(1.0).max(1.0), 0.0],
            clip: NO_CLIP,
            ..Default::default()
        });

        let x = r.x + self.px(32.0);
        let right = r.x + r.w - self.px(32.0);
        let mut y = r.y + self.px(26.0);
        let pw2 = self.label(q, "PUNK", Font::Ui, 15.0, 800, 3.4, x, y, with_alpha(t.accent, e));
        let tw = self.label(q, "TERM", Font::Ui, 15.0, 800, 3.4, x + pw2 + self.px(3.4), y, with_alpha(t.fg, e));
        self.meta(q, &["KEYMAP"], x + pw2 + tw + self.px(18.0), y + self.px(4.0), with_alpha(t.muted, e));
        let esc = "ESC  CLOSE";
        let ew = self.measure(esc, Font::Mono, 9.5, 600, 1.2);
        self.label(q, esc, Font::Mono, 9.5, 600, 1.2, right - ew, y + self.px(4.0), with_alpha(t.muted, e));
        y += self.px(36.0);
        self.rect(q, Rect { x, y, w: right - x, h: self.px(1.0).max(1.0) }, with_alpha(t.border, e), 0.0, NO_CLIP);
        y += self.px(22.0);

        let col_w = (right - x - self.px(32.0)) / 2.0;
        let key_w = self.px(112.0);
        for (ci, (title, keys)) in [("SESSIONS", KEYS_SESSIONS), ("VIEW", KEYS_VIEW)].into_iter().enumerate() {
            let cx = x + ci as f32 * (col_w + self.px(32.0));
            let mut cy = y;
            self.meta(q, &[title], cx, cy, with_alpha(t.accent2, e));
            cy += self.px(24.0);
            for (k, v) in keys {
                self.label(q, k, Font::Mono, 12.0, 700, 0.0, cx, cy, with_alpha(t.fg, e));
                self.label(q, v, Font::Ui, 12.0, 400, 0.0, cx + key_w, cy, with_alpha(mix(t.fg, t.muted, 0.35), e));
                cy += self.px(23.0);
            }
        }

        // Status legend along the bottom.
        let ly = r.y + r.h - self.px(150.0);
        self.rect(q, Rect { x, y: ly, w: right - x, h: self.px(1.0).max(1.0) }, with_alpha(t.border, e), 0.0, NO_CLIP);
        self.meta(q, &["STATUS"], x, ly + self.px(16.0), with_alpha(t.accent2, e));
        for (i, (s, text)) in LEGEND.iter().enumerate() {
            let cx = x + (i % 2) as f32 * (col_w + self.px(32.0));
            let cy = ly + self.px(48.0) + (i / 2) as f32 * self.px(26.0);
            let col = with_alpha(self.status_color(*s), e);
            let cw = self.chip(q, status_word(*s), col, cx + self.px(64.0), cy, *s == Status::Blocked);
            let _ = cw;
            self.label(q, text, Font::Ui, 12.0, 400, 0.0, cx + key_w, cy - self.px(8.0), with_alpha(mix(t.fg, t.muted, 0.35), e));
        }

        // Current settings.
        let opacity = format!("{}%", (self.opacity * 100.0).round());
        let theme = self.theme_target.name.to_uppercase();
        let sound = self.sound.mode.name().to_uppercase();
        let parts = ["THEME", &theme, "OPACITY", &opacity, "SOUND", &sound];
        self.meta(q, &parts, x, r.y + r.h - self.px(28.0), with_alpha(t.muted, e));
    }

    /// The connect prompt: an input line over suggestions from shell history.
    fn prompt_layer(&mut self, q: &mut Vec<Quad>, dt: f32, animating: &mut bool) {
        let mut pt = self.prompt_t;
        let e = Self::fade(&mut pt, self.prompt.is_some(), dt, 0.16, animating);
        self.prompt_t = pt;
        let Some(p) = self.prompt.as_ref() else { return };
        let (text, sel) = (p.text.clone(), p.sel);
        let matches: Vec<String> = p.matches().into_iter().map(str::to_string).collect();
        let t = self.theme.clone();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        self.rect(q, Rect { x: 0.0, y: 0.0, w, h }, with_alpha(t.bg, 0.72 * e), 0.0, NO_CLIP);

        let row = self.px(30.0);
        let pw = self.px(720.0).min(w - self.px(48.0));
        let ph = self.px(150.0) + matches.len().max(1) as f32 * row;
        let r = Rect { x: ((w - pw) / 2.0).round(), y: (h * 0.18 + (1.0 - e) * self.px(14.0)).round(), w: pw, h: ph };
        let c = self.px(16.0);
        let cuts = [c, 0.0, c, 0.0];
        q.push(Quad {
            rect: [r.x, r.y + self.px(10.0), r.w, r.h],
            uv: cuts,
            color: [0.0, 0.0, 0.0, 0.6 * e],
            params: [kind::SHADOW, 0.0, 0.0, self.px(24.0)],
            clip: NO_CLIP,
            ..Default::default()
        });
        self.shape(q, r, with_alpha(t.surface, 0.98 * e), None, cuts);
        q.push(Quad {
            rect: [r.x, r.y, r.w, r.h],
            uv: cuts,
            color: with_alpha(t.accent, e),
            color2: with_alpha(t.accent2, e),
            params: [kind::GRADIENT_STROKE, 0.0, self.px(1.0).max(1.0), 0.0],
            clip: NO_CLIP,
            ..Default::default()
        });

        let x = r.x + self.px(28.0);
        let right = r.x + r.w - self.px(28.0);
        let mut y = r.y + self.px(24.0);
        self.meta(q, &["CONNECT", "JOIN TMUX ON A HOST"], x, y, with_alpha(t.accent2, e));
        let esc = "ESC  CANCEL";
        let ew = self.measure(esc, Font::Mono, 9.5, 600, 1.2);
        self.label(q, esc, Font::Mono, 9.5, 600, 1.2, right - ew, y, with_alpha(t.muted, e));
        y += self.px(24.0);

        // Input line.
        let ib = Rect { x, y, w: right - x, h: self.px(40.0) };
        self.shape(q, ib, with_alpha(t.surface_alt, e), Some(with_alpha(if sel.is_none() { t.accent } else { t.border }, e)), [self.px(8.0), 0.0, 0.0, 0.0]);
        let gx = ib.x + self.px(14.0);
        let gy = ib.y + self.px(12.0);
        let gw = self.label(q, "ssh", Font::Mono, 13.0, 700, 0.0, gx, gy, with_alpha(t.accent, e));
        let tx = gx + gw + self.px(10.0);
        let shown = if text.is_empty() { "ubuntu@203.0.113.7  or  -i ~/.ssh/key.pem ubuntu@host".to_string() } else { text.clone() };
        let shown = self.fit(&shown, Font::Mono, 13.0, 500, ib.x + ib.w - tx - self.px(20.0));
        let tw = self.label(q, &shown, Font::Mono, 13.0, 500, 0.0, tx, gy, with_alpha(if text.is_empty() { t.muted } else { t.fg }, e));
        let cx = if text.is_empty() { tx } else { tx + tw + self.px(2.0) };
        self.rect(q, Rect { x: cx, y: ib.y + self.px(10.0), w: self.px(8.0), h: self.px(20.0) }, with_alpha(t.cursor, 0.85 * e), 0.0, NO_CLIP);
        y += ib.h + self.px(18.0);

        // Suggestions.
        self.meta(q, &["RECENT", "FROM SHELL HISTORY"], x, y, with_alpha(t.muted, e));
        y += self.px(18.0);
        if matches.is_empty() {
            self.label(q, "no matches // ↵ connects to what you typed", Font::Mono, 11.0, 500, 0.0, x, y + self.px(8.0), with_alpha(t.muted, e));
        }
        for (i, m) in matches.iter().enumerate() {
            let rr = Rect { x, y, w: right - x, h: row - self.px(4.0) };
            let on = sel == Some(i);
            if on {
                self.shape(q, rr, with_alpha(t.surface_alt, e), Some(with_alpha(t.accent2, 0.8 * e)), [self.px(6.0), 0.0, 0.0, 0.0]);
                self.rect(q, Rect { x: rr.x, y: rr.y + self.px(6.0), w: self.px(2.0), h: rr.h - self.px(12.0) }, with_alpha(t.accent2, e), 0.0, NO_CLIP);
            }
            let host = crate::tmuxcc::ssh_host_name(m).to_uppercase();
            let hw = self.px(190.0);
            let hs = self.fit(&host, Font::Ui, 11.0, 700, hw - self.px(12.0));
            self.label(q, &hs, Font::Ui, 11.0, 700, 1.0, rr.x + self.px(12.0), rr.y + self.px(7.0), with_alpha(if on { t.fg } else { mix(t.fg, t.muted, 0.3) }, e));
            let ms = self.fit(m, Font::Mono, 11.0, 400, rr.w - hw - self.px(16.0));
            self.label(q, &ms, Font::Mono, 11.0, 400, 0.0, rr.x + hw, rr.y + self.px(7.0), with_alpha(t.muted, e));
            y += row;
        }
        let hint = "↑↓ PICK  ·  TAB FILL  ·  ↵ CONNECT  ·  KEY-BASED SSH ONLY";
        self.label(q, hint, Font::Mono, 9.0, 600, 1.0, x, r.y + r.h - self.px(24.0), with_alpha(t.muted, e));
    }

    fn toast_layer(&mut self, q: &mut Vec<Quad>, l: &Layout, animating: &mut bool) {
        let Some((msg, at)) = self.toast.clone() else { return };
        let age = at.elapsed().as_secs_f32();
        if age > 1.9 {
            self.toast = None;
            return;
        }
        *animating = true;
        let e = ease_out_cubic((age / 0.18).min(1.0)) * (1.0 - ((age - 1.5) / 0.4).clamp(0.0, 1.0));
        let t = self.theme.clone();
        let k = self.glyphs.text(&msg, Font::Mono, self.px(11.0), 700, self.px(1.4));
        let (w, h) = (k.width + self.px(36.0), self.px(34.0));
        let p = l.panel;
        let r = Rect { x: (p.x + (p.w - w) / 2.0).round(), y: (p.y + p.h - h - self.px(28.0) + (1.0 - e) * self.px(10.0)).round(), w, h };
        let c = (h * 0.4).round();
        self.shape(q, r, with_alpha(t.surface_alt, 0.97 * e), Some(with_alpha(t.accent2, 0.8 * e)), [c, 0.0, c, 0.0]);
        // Separators inside the message render in the accent.
        let mut x = r.x + self.px(18.0);
        let y = r.y + (h - k.height) / 2.0;
        for (i, part) in msg.split(" // ").enumerate() {
            if i > 0 {
                x += self.label(q, " // ", Font::Mono, 11.0, 700, 1.4, x, y, with_alpha(t.accent, e));
            }
            x += self.label(q, part, Font::Mono, 11.0, 700, 1.4, x, y, with_alpha(if i == 0 { t.muted } else { t.fg }, e));
        }
    }

    /// Columns of the dashboard grid at the current window size.
    pub fn overview_cols(&self) -> usize {
        let w = self.size.0 as f32 - self.px(28.0) - self.px(48.0);
        ((w + self.px(14.0)) / (self.px(360.0) + self.px(14.0))).floor().max(1.0) as usize
    }

    pub fn overview_tile_at(&self, x: f32, y: f32) -> Option<usize> {
        self.overview_rects.iter().position(|r| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
    }

    /// Mission control: every session, grouped by host, with a live tail.
    fn overview_layer(&mut self, q: &mut Vec<Quad>, dt: f32, animating: &mut bool) {
        let mut ot = self.overview_t;
        let e = Self::fade(&mut ot, self.overview, dt, 0.2, animating);
        self.overview_t = ot;
        self.overview_rects.clear();
        if e <= 0.0 {
            return;
        }
        let t = self.theme.clone();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        self.rect(q, Rect { x: 0.0, y: 0.0, w, h }, with_alpha(t.bg, (0.6 + 0.4 * self.opacity) * e), 0.0, NO_CLIP);
        q.push(Quad {
            rect: [0.0, 0.0, w, h],
            color: with_alpha(t.muted, 0.14 * e),
            params: [kind::DOTS, self.px(24.0), self.px(1.5), 0.0],
            clip: NO_CLIP,
            ..Default::default()
        });

        let pad = self.px(38.0);
        let x0 = pad;
        let right = w - pad;
        let mut y = self.px(28.0);

        // Top bar.
        let pw = self.label(q, "PUNK", Font::Ui, 15.0, 800, 3.4, x0, y, with_alpha(t.accent, e));
        let tw = self.label(q, "TERM", Font::Ui, 15.0, 800, 3.4, x0 + pw + self.px(3.4), y, with_alpha(t.fg, e));
        let n = self.panes.len();
        let hosts_used = (0..self.hosts.len()).filter(|h| self.panes.iter().any(|p| p.host == *h)).count();
        let ses = format!("{n} SESSIONS");
        let hs = format!("{hosts_used} HOSTS");
        self.meta(q, &["OVERVIEW", &ses, &hs], x0 + pw + tw + self.px(20.0), y + self.px(4.0), with_alpha(t.muted, e));
        let hint = "←↑↓→ SELECT   ⏎ OPEN   ESC CLOSE";
        let hw = self.measure(hint, Font::Mono, 9.5, 600, 1.0);
        self.label(q, hint, Font::Mono, 9.5, 600, 1.0, right - hw, y + self.px(4.0), with_alpha(t.muted, e));
        y += self.px(36.0);
        self.rect(q, Rect { x: x0, y, w: right - x0, h: self.px(1.0).max(1.0) }, with_alpha(t.border, e), 0.0, NO_CLIP);
        q.push(Quad {
            rect: [x0, y - self.px(0.5), self.px(64.0), self.px(2.0)],
            color: with_alpha(t.accent, e),
            color2: with_alpha(t.accent2, e),
            params: [kind::GRADIENT, 0.0, 0.0, 0.0],
            clip: NO_CLIP,
            ..Default::default()
        });
        y += self.px(22.0);

        // Size tiles so every host fits on screen.
        let cols = self.overview_cols();
        let gap = self.px(14.0);
        let tile_w = ((right - x0 - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        let rows: usize = (0..self.hosts.len())
            .map(|h| self.panes.iter().filter(|p| p.host == h).count().div_ceil(cols))
            .sum();
        let groups = hosts_used as f32;
        let avail = h - y - self.px(24.0) - groups * self.px(30.0);
        let tile_h = ((avail - gap * rows.saturating_sub(1) as f32) / rows.max(1) as f32).clamp(self.px(92.0), self.px(240.0)).floor();
        let line_h = (self.px(10.5) * 1.35).round();
        let preview_lines = (((tile_h - self.px(70.0)) / line_h).floor().max(0.0)) as usize;

        let since_open = self.overview_opened.elapsed().as_secs_f32();
        let time = self.epoch.elapsed().as_secs_f32();
        let mut rects = vec![Rect::default(); n];
        let mut order = 0usize;
        for host in 0..self.hosts.len() {
            let members: Vec<usize> = (0..n).filter(|&i| self.panes[i].host == host).collect();
            if members.is_empty() {
                continue;
            }
            let hcol = self.host_color(host);
            let num = format!("{:02}", host + 1);
            let name = self.hosts[host].name.to_uppercase();
            let w1 = self.label(q, &num, Font::Mono, 10.0, 700, 0.0, x0, y, with_alpha(hcol, e));
            let nw = self.label(q, &name, Font::Ui, 11.0, 700, 2.6, x0 + w1 + self.px(10.0), y, with_alpha(t.fg, e));
            let cnt = format!("{:02} SES", members.len());
            let cw = self.measure(&cnt, Font::Mono, 9.5, 500, 1.0);
            self.label(q, &cnt, Font::Mono, 9.5, 500, 1.0, right - cw, y, with_alpha(t.muted, e));
            let lx = x0 + w1 + nw + self.px(22.0);
            self.rect(q, Rect { x: lx, y: y + self.px(8.0), w: right - cw - self.px(12.0) - lx, h: self.px(1.0).max(1.0) }, with_alpha(t.border, e), 0.0, NO_CLIP);
            y += self.px(26.0);

            for (k, &i) in members.iter().enumerate() {
                let (col, row) = (k % cols, k / cols);
                let base = Rect { x: x0 + col as f32 * (tile_w + gap), y: y + row as f32 * (tile_h + gap), w: tile_w, h: tile_h };
                rects[i] = base;
                // Tiles cascade in.
                let a = ease_out_cubic(((since_open - order as f32 * 0.018) / 0.26).clamp(0.0, 1.0)) * e;
                if a < 1.0 {
                    *animating = true;
                }
                order += 1;
                let r = Rect { y: base.y + (1.0 - a) * self.px(12.0), ..base };
                self.tile(q, i, r, a, preview_lines, line_h, time, animating);
            }
            y += members.len().div_ceil(cols) as f32 * (tile_h + gap) + self.px(8.0);
        }
        self.overview_rects = rects;
    }

    #[allow(clippy::too_many_arguments)]
    fn tile(&mut self, q: &mut Vec<Quad>, i: usize, r: Rect, a: f32, lines: usize, line_h: f32, time: f32, animating: &mut bool) {
        let t = self.theme.clone();
        let selected = i == self.overview_sel;
        let status = self.ui[i].status;
        let scol = self.status_color(status);
        let cut = self.px(12.0);
        let cuts = [cut, 0.0, cut, 0.0];
        if selected {
            q.push(Quad {
                rect: [r.x - self.px(30.0), r.y - self.px(30.0), r.w + self.px(60.0), r.h + self.px(60.0)],
                color: with_alpha(if status == Status::Shell { t.accent2 } else { scol }, 0.16 * a),
                params: [kind::GLOW, 0.0, 0.0, 0.0],
                clip: NO_CLIP,
                ..Default::default()
            });
        }
        let stroke = if selected {
            t.fg
        } else if status == Status::Shell {
            t.border
        } else {
            with_alpha(scol, 0.5)
        };
        self.shape(q, r, with_alpha(t.surface, 0.96 * a), Some(with_alpha(stroke, a)), cuts);
        if status == Status::Blocked {
            let breathe = 0.5 + 0.5 * (time * std::f32::consts::TAU / 1.8).sin();
            self.shape(q, r, with_alpha(t.alert, (0.04 + 0.05 * breathe) * a), Some(with_alpha(t.alert, (0.4 + 0.5 * breathe) * a)), cuts);
            *animating = true;
        }

        let p = &self.panes[i];
        let title = if p.title.is_empty() { p.subtitle.clone() } else { super::clean_title(&p.title) };
        let x = r.x + self.px(16.0);
        let right = r.x + r.w - self.px(14.0);
        let top = r.y + self.px(12.0);
        let num = format!("{:02}", i + 1);
        self.label(q, &num, Font::Mono, 11.0, 800, 0.0, x, top + self.px(1.0), with_alpha(if selected { t.accent } else { t.muted }, a));

        let cy = top + self.px(9.0);
        let mut chip_w = 0.0;
        if status != Status::Shell {
            chip_w = self.chip(q, status_word(status), with_alpha(scol, a), right, cy, status == Status::Blocked);
            if status == Status::Working {
                let f = SPINNER[((time * 12.0) as usize) % SPINNER.len()];
                let k = self.glyphs.text(f, Font::Mono, self.px(13.0), 400, 0.0);
                self.text(q, &k, right - chip_w - self.px(6.0) - k.width, cy - k.height / 2.0, with_alpha(scol, a), NO_CLIP);
                chip_w += self.px(20.0);
            }
        }
        let tx = x + self.px(30.0);
        let title = self.fit(&title, Font::Ui, 13.5, 650, right - chip_w - self.px(12.0) - tx);
        self.label(q, &title, Font::Ui, 13.5, 650, 0.0, tx, top, with_alpha(t.fg, a));

        let sub = self.ui[i].sub_shown.clone();
        let sub_col = match status {
            Status::Shell | Status::Idle => t.muted,
            s => self.status_color(s),
        };
        let sub = self.fit(&sub, Font::Mono, 10.5, 500, right - tx);
        self.label(q, &sub, Font::Mono, 10.5, 500, 0.0, tx, top + self.px(22.0), with_alpha(sub_col, 0.9 * a));

        if lines == 0 {
            return;
        }
        let dy = top + self.px(46.0);
        self.rect(q, Rect { x, y: dy, w: right - x, h: self.px(1.0).max(1.0) }, with_alpha(t.border, a), 0.0, NO_CLIP);
        let tail = self.preview(i, lines);
        let clip = [x, dy, right, r.y + r.h - self.px(6.0)];
        let mut ly = dy + self.px(8.0);
        let count = tail.len();
        for (k, line) in tail.iter().enumerate() {
            // The newest line reads brightest.
            let c = if k + 1 == count { mix(t.fg, t.muted, 0.15) } else { mix(t.fg, t.muted, 0.55) };
            let l = self.glyphs.text(line, Font::Mono, self.px(10.5), 400, 0.0);
            self.text(q, &l, x, ly, with_alpha(c, a), clip);
            ly += line_h;
        }
    }
}

/// Greedy word wrap to `per` characters, at most `max` lines (the last
/// ending in an ellipsis when cut).
fn wrap(text: &str, per: usize, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut words = text.split_whitespace().peekable();
    while let Some(w) = words.next() {
        let w: String = if w.chars().count() > per { w.chars().take(per - 1).chain(['…']).collect() } else { w.to_string() };
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > per {
            out.push(std::mem::take(&mut cur));
            if out.len() == max {
                let last = out.last_mut().unwrap();
                if last.chars().count() >= per {
                    *last = last.chars().take(per - 1).collect();
                }
                last.push('…');
                return out;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&w);
        let _ = words.peek();
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
