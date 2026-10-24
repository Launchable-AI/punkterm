//! The app model and the frame builder: layout, session rail, terminal panel.
//! Everything here is window-system agnostic so headless snapshots share it.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::Event;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

mod chrome;

use crate::anim::Spring;
use crate::claude::{self, Status};
use crate::gfx::{kind, Quad};
use crate::glyphs::Glyphs;
use crate::hosts::Host;
use crate::tmuxcc::{self, CcEvent};
use crate::sound::{self, Sfx, Sound};
use crate::state::{self, Prefs};
use crate::sysstat::SysStat;
use crate::term::{Pane, Size, Spawn};
use crate::theme::{mix, with_alpha, Rgba, Theme};

const NO_CLIP: [f32; 4] = [-1e9, -1e9, 1e9, 1e9];

#[derive(Clone, Copy, Default, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

struct Layout {
    rail: Rect,
    panel: Rect,
    header: Rect,
    term: Rect,
    cards: Vec<Rect>,
    /// (host index, y) for each host heading in the rail.
    sections: Vec<(usize, f32)>,
    /// The conversation timeline on the right: a strip when collapsed.
    timeline: Rect,
    /// Each heading's "+" button (new shell on that host), by host index.
    adds: Vec<Rect>,
}

/// Per-session UI state that outlives a single frame.
struct SessionUi {
    /// Background work the session reports ("1 shell"), for the card.
    background: Option<String>,
    unseen: bool,
    bell: f32,
    appear: f32,
    status: Status,
    activity: Option<String>,
    was_working: bool,
    since: Instant,
    sub_shown: String,
    sub_since: Instant,
    /// False until the first look, so startup states don't chime.
    detected: bool,
    /// Whether this Claude session was last told the theme is light.
    told_light: Option<bool>,
}

impl SessionUi {
    fn new() -> Self {
        SessionUi {
            background: None,
            unseen: false,
            bell: 0.0,
            appear: 0.0,
            status: Status::Shell,
            activity: None,
            was_working: false,
            since: Instant::now(),
            sub_shown: String::new(),
            sub_since: Instant::now(),
            detected: false,
            told_light: None,
        }
    }
}

/// What a host's connection is doing, for the rail.
#[derive(Clone, Debug, PartialEq)]
pub enum HostState {
    Connecting,
    Up,
    /// Reachable, but no tmux sessions to join.
    Empty,
    Failed(String),
}

/// The connect prompt (⌃⇧N): an ssh destination or command, with
/// suggestions from shell history.
pub struct Prompt {
    pub text: String,
    /// Highlighted suggestion; `None` uses the typed text.
    pub sel: Option<usize>,
    history: Vec<String>,
}

impl Prompt {
    pub fn matches(&self) -> Vec<&str> {
        let needle = self.text.trim().to_lowercase();
        self.history.iter().map(String::as_str).filter(|h| h.to_lowercase().contains(&needle)).take(7).collect()
    }
}

pub enum PromptKey {
    Char(String),
    Back,
    Up,
    Down,
    Tab,
    Enter,
    Esc,
}

struct Transition {
    from: u64,
    started: Instant,
    dir: f32,
}

pub struct App {
    pub theme: Theme,
    pub glyphs: Glyphs,
    pub scale: f32,
    pub size: (u32, u32),
    pub panes: Vec<Pane>,
    ui: Vec<SessionUi>,
    pub active: usize,
    pub focused: bool,
    next_id: u64,
    tx: Sender<(u64, Event)>,
    rx: Receiver<(u64, Event)>,
    wake: Arc<dyn Fn() + Send + Sync>,
    cursor_x: Spring,
    cursor_y: Spring,
    cursor_pane: u64,
    indicator: Spring,
    transition: Option<Transition>,
    last_frame: Instant,
    last_detect: Instant,
    epoch: Instant,
    dt: f32,
    ambient: Rgba,
    sys: SysStat,
    pub hosts: Vec<Host>,
    pub host_state: Vec<HostState>,
    disc_tx: Sender<(usize, Result<Vec<String>, String>)>,
    disc_rx: Receiver<(usize, Result<Vec<String>, String>)>,
    /// Hosts added from the prompt, remembered across runs.
    saved_hosts: Vec<String>,
    pub prompt: Option<Prompt>,
    prompt_t: f32,
    /// Report punkterm's size to tmux. Off for headless snapshots, which
    /// must never resize anyone's windows.
    pub report_size: bool,
    /// Plan usage per host, refreshed in the background.
    usage: std::collections::HashMap<usize, crate::usage::Usage>,
    usage_tx: Sender<(usize, Option<crate::usage::Usage>)>,
    usage_rx: Receiver<(usize, Option<crate::usage::Usage>)>,
    last_usage: Option<Instant>,
    /// The timeline rail is expanded (collapsed, it is a strip of ticks).
    pub timeline_open: bool,
    /// The conversation in each (host, tmux pane), refreshed while active.
    timelines: std::collections::HashMap<(usize, String), crate::timeline::Timeline>,
    tl_tx: Sender<((usize, String), Option<crate::timeline::Timeline>)>,
    tl_rx: Receiver<((usize, String), Option<crate::timeline::Timeline>)>,
    /// The pane last fetched, when, and whether a fetch is still running.
    tl_last: Option<((usize, String), Instant)>,
    tl_busy: bool,
    /// Timeline scroll, in px up from the newest entry.
    pub(super) tl_scroll: f32,
    /// Where each timeline item was drawn last frame, and its kind and text.
    pub(super) tl_hits: Vec<(Rect, crate::timeline::Kind, String)>,
    /// The mouse, while it is over the open timeline.
    pub(super) hover_tl: Option<(f32, f32)>,
    conns: Vec<tmuxcc::Conn>,
    cc_tx: Sender<CcEvent>,
    cc_rx: Receiver<CcEvent>,
    /// Focus the next pane that appears on this connection (after ⌃⇧T).
    focus_new: Option<u64>,
    /// tmux panes closed with ⌃⇧W: hidden here, untouched in tmux.
    hidden: std::collections::HashSet<(u64, String)>,
    hostnames: std::collections::HashMap<usize, String>,
    last_list: Instant,
    theme_target: Theme,
    theme_from: Option<(Theme, Instant)>,
    pub mono_px: f32,
    pub opacity: f32,
    /// Whether the surface can show through (premultiplied alpha).
    pub alpha_ok: bool,
    pub sound: Sound,
    pub help: bool,
    /// When Ctrl+Space (the tmux-style leader) was pressed, while it waits
    /// for its key.
    pub leader: Option<Instant>,
    help_t: f32,
    toast: Option<(String, Instant)>,
    pub overview: bool,
    overview_t: f32,
    overview_opened: Instant,
    pub overview_sel: usize,
    scroll_acc: f32,
    /// A mouse drag is extending the active pane's selection.
    selecting: bool,
    /// Last left click (when, cell, count) for double/triple clicks.
    last_click: Option<(Instant, (i32, usize), u8)>,
    /// The link under the pointer while Ctrl is held, with its pane id.
    hover: Option<(u64, crate::links::Link)>,
    overview_rects: Vec<Rect>,
    pub quit: bool,
}

impl App {
    pub fn new(theme: Theme, scale: f32, size: (u32, u32), wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let glyphs = Glyphs::new(&theme.mono_font, &theme.ui_font, theme.mono_px * scale);
        let mono_px = theme.mono_px;
        let theme_target = theme.clone();
        let (tx, rx) = channel();
        let (disc_tx, disc_rx) = channel();
        let (cc_tx, cc_rx) = channel();
        let (usage_tx, usage_rx) = channel();
        let (tl_tx, tl_rx) = channel();
        let mut hostnames = std::collections::HashMap::new();
        if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
            hostnames.insert(0usize, h.trim().to_string());
        }
        App {
            theme,
            glyphs,
            scale,
            size,
            panes: Vec::new(),
            ui: Vec::new(),
            active: 0,
            focused: true,
            next_id: 1,
            tx,
            rx,
            wake,
            cursor_x: Spring::new(0.0, 45.0),
            cursor_y: Spring::new(0.0, 45.0),
            cursor_pane: 0,
            indicator: Spring::new(0.0, 26.0),
            transition: None,
            last_frame: Instant::now(),
            last_detect: Instant::now(),
            epoch: Instant::now(),
            dt: 0.0,
            ambient: [0.0; 4],
            sys: SysStat::new(),
            hosts: vec![Host { name: "local".into(), ssh: None, color: None }],
            host_state: vec![HostState::Up],
            disc_tx,
            disc_rx,
            saved_hosts: Vec::new(),
            prompt: None,
            prompt_t: 0.0,
            report_size: true,
            usage: Default::default(),
            usage_tx,
            usage_rx,
            last_usage: None,
            timeline_open: false,
            timelines: Default::default(),
            tl_tx,
            tl_rx,
            tl_last: None,
            tl_busy: false,
            tl_scroll: 0.0,
            tl_hits: Vec::new(),
            hover_tl: None,
            conns: Vec::new(),
            cc_tx,
            cc_rx,
            focus_new: None,
            hidden: Default::default(),
            hostnames,
            last_list: Instant::now(),
            theme_target,
            theme_from: None,
            mono_px,
            opacity: 1.0,
            alpha_ok: false,
            sound: Sound::new(sound::Mode::Off),
            help: false,
            leader: None,
            help_t: 0.0,
            toast: None,
            overview: false,
            overview_t: 0.0,
            overview_opened: Instant::now(),
            overview_sel: 0,
            scroll_acc: 0.0,
            selecting: false,
            last_click: None,
            hover: None,
            overview_rects: Vec::new(),
            quit: false,
        }
    }

    fn px(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }

    fn layout(&self) -> Layout {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let m = self.px(14.0);
        let rail_w = self.px(292.0);
        let rail = Rect { x: m, y: m, w: rail_w, h: h - 2.0 * m };
        let tl_w = if self.timeline_open { self.px(340.0) } else { self.px(30.0) };
        let panel = Rect { x: rail.x + rail_w + m, y: m, w: (w - rail_w - tl_w - 4.0 * m).max(1.0), h: h - 2.0 * m };
        let timeline = Rect { x: panel.x + panel.w + m, y: m, w: tl_w, h: h - 2.0 * m };
        let header_h = self.px(50.0);
        let header = Rect { x: panel.x, y: panel.y, w: panel.w, h: header_h };
        let pad = self.px(14.0);
        let term = Rect {
            x: panel.x + pad,
            y: panel.y + header_h + self.px(10.0),
            w: (panel.w - 2.0 * pad).max(1.0),
            h: (panel.h - header_h - self.px(10.0) - pad).max(1.0),
        };
        // Host sections in host order; panes are kept grouped by host.
        // Cards collapse to one line when the full ones would not fit above
        // the footer.
        let avail = rail.h - self.px(76.0) - self.px(130.0) - self.plan_h();
        let need = |ch: f32, g: f32| self.hosts.len() as f32 * self.px(50.0) + self.panes.len() as f32 * (ch + g);
        let (card_h, gap) = if need(self.px(56.0), self.px(6.0)) <= avail {
            (self.px(56.0), self.px(6.0))
        } else {
            (self.px(32.0), self.px(4.0))
        };
        let mut y = rail.y + self.px(76.0);
        let mut cards = vec![Rect::default(); self.panes.len()];
        let mut sections = Vec::new();
        let mut adds = Vec::new();
        let add = self.px(16.0);
        for h in 0..self.hosts.len() {
            sections.push((h, y));
            adds.push(Rect { x: rail.x + rail.w - self.px(18.0) - add, y: y - self.px(3.0), w: add, h: add });
            y += self.px(22.0);
            if h > 0 && !self.panes.iter().any(|p| p.host == h) {
                y += self.px(16.0);
            }
            for (i, p) in self.panes.iter().enumerate() {
                if p.host == h {
                    // Panes of one split window sit close, as one group.
                    let mate = i + 1 < self.panes.len() && self.split_key(i).is_some() && self.split_key(i) == self.split_key(i + 1);
                    cards[i] = Rect { x: rail.x + self.px(10.0), y, w: rail.w - self.px(20.0), h: card_h };
                    y += card_h + if mate { self.px(2.0) } else { gap };
                }
            }
            y += self.px(12.0);
        }
        Layout { rail, panel, header, term, cards, sections, adds, timeline }
    }

    pub fn grid_size(&self) -> Size {
        let t = self.layout().term;
        Size {
            cols: ((t.w / self.glyphs.cell_w) as usize).max(2),
            lines: ((t.h / self.glyphs.cell_h) as usize).max(1),
            cell_w: self.glyphs.cell_w as u16,
            cell_h: self.glyphs.cell_h as u16,
        }
    }

    pub fn spawn(&mut self, spec: Spawn) {
        self.spawn_pane(spec, 0, None, true);
    }

    pub fn spawn_on(&mut self, spec: Spawn, host: usize) {
        self.spawn_pane(spec, host.min(self.hosts.len().saturating_sub(1)), None, true);
    }

    fn spawn_pane(&mut self, spec: Spawn, host: usize, session: Option<String>, focus: bool) {
        let id = self.next_id;
        self.next_id += 1;
        let size = self.grid_size();
        match Pane::spawn(id, spec, size, self.tx.clone(), self.wake.clone()) {
            Ok(mut p) => {
                p.host = host;
                if let Some(s) = &session {
                    p.subtitle = s.clone();
                }
                p.session = session;
                // Keep panes grouped by host, in host order.
                let at = self.panes.iter().rposition(|q| q.host <= host).map(|i| i + 1).unwrap_or(0);
                let active_id = self.panes.get(self.active).map(|p| p.id);
                self.panes.insert(at, p);
                self.ui.insert(at, SessionUi::new());
                if let Some(aid) = active_id {
                    self.active = self.panes.iter().position(|p| p.id == aid).unwrap_or(0);
                }
                if focus || self.panes.len() == 1 {
                    self.select(at);
                }
            }
            Err(e) => eprintln!("punkterm: spawn failed: {e}"),
        }
    }

    /// Start up: join the tmux sessions already running on every host
    /// (local now, remote as each host answers). With no local tmux, open
    /// a plain shell so there is always something to type into.
    fn fetch_usage(&self, host: usize, ssh: Option<String>) {
        let (tx, wake) = (self.usage_tx.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send((host, crate::usage::fetch(ssh.as_deref())));
            wake();
        });
    }

    /// Plan usage for the host of the active session (at most three windows).
    fn plan(&self) -> Option<(usize, crate::usage::Usage)> {
        let h = self.active_host();
        let u = self.usage.get(&h).filter(|u| !u.rows.is_empty())?;
        Some((h, crate::usage::Usage { at_ms: u.at_ms, rows: u.rows.iter().take(3).cloned().collect(), rail_width: u.rail_width, account: u.account.clone(), names: Default::default() }))
    }

    /// Height of the plan block in the rail footer.
    fn plan_h(&self) -> f32 {
        self.plan().map_or(0.0, |(_, u)| self.px(26.0) + u.rows.len() as f32 * self.px(20.0))
    }

    pub fn set_hosts(&mut self, hosts: Vec<Host>) {
        self.hosts = hosts;
        self.host_state = vec![HostState::Up; self.hosts.len()];
    }

    pub fn start(&mut self, hosts: Vec<Host>) {
        self.set_hosts(hosts);
        let local = tmuxcc::list_sessions(None).unwrap_or_default();
        for name in &local {
            self.open_conn(0, name);
        }
        if local.is_empty() {
            self.spawn(Spawn { program: None, args: vec![], cwd: std::env::current_dir().ok() });
        }
        for i in 1..self.hosts.len() {
            self.discover(i);
        }
    }

    /// Look for tmux sessions on a remote host, in the background.
    fn discover(&mut self, i: usize) {
        let Some(h) = self.hosts.get(i) else { return };
        self.host_state[i] = HostState::Connecting;
        let (dest, tx, wake) = (h.ssh.clone(), self.disc_tx.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send((i, tmuxcc::list_sessions(dest.as_deref())));
            wake();
        });
    }

    /// Retry every remote host that is down or has nothing attached.
    pub fn reconnect(&mut self) {
        let mut n = 0;
        for i in 1..self.hosts.len() {
            if !self.conns.iter().any(|c| c.host == i) && self.host_state[i] != HostState::Connecting {
                self.discover(i);
                n += 1;
            }
        }
        self.toast(if n == 0 { "HOSTS // ALL CONNECTED".to_string() } else { format!("RECONNECT // {n} HOST{}", if n == 1 { "" } else { "S" }) });
    }

    /// Connect to a host from an ssh spec, and remember it.
    pub fn add_host(&mut self, spec: &str) {
        let h = Host::from_spec(spec);
        if h.ssh.as_deref().is_none_or(str::is_empty) {
            return;
        }
        if let Some(i) = self.hosts.iter().position(|o| o.ssh == h.ssh) {
            if let Some(p) = self.panes.iter().position(|p| p.host == i) {
                self.select(p);
            } else {
                self.discover(i);
            }
            return;
        }
        self.toast(format!("CONNECT // {}", h.name.to_uppercase()));
        self.hosts.push(h.clone());
        self.host_state.push(HostState::Connecting);
        self.discover(self.hosts.len() - 1);
        let spec = h.ssh.unwrap_or_default();
        if !self.saved_hosts.contains(&spec) {
            self.saved_hosts.push(spec);
            self.save_prefs();
        }
    }

    pub fn open_prompt(&mut self) {
        self.prompt = Some(Prompt { text: String::new(), sel: None, history: crate::hosts::ssh_history() });
        self.sound.play(Sfx::Blip);
    }

    /// A key while the connect prompt is open.
    pub fn prompt_key(&mut self, key: PromptKey) {
        let Some(p) = self.prompt.as_mut() else { return };
        let n = p.matches().len();
        match key {
            PromptKey::Char(s) => {
                p.text.push_str(&s);
                p.sel = None;
            }
            PromptKey::Back => {
                p.text.pop();
                p.sel = None;
            }
            PromptKey::Down if n > 0 => p.sel = Some(p.sel.map_or(0, |i| (i + 1).min(n - 1))),
            PromptKey::Up => p.sel = p.sel.and_then(|i| i.checked_sub(1)),
            PromptKey::Tab => {
                if let Some(m) = p.matches().get(p.sel.unwrap_or(0)) {
                    p.text = m.to_string();
                    p.sel = None;
                }
            }
            PromptKey::Enter => {
                let spec = match p.sel {
                    Some(i) => p.matches().get(i).map(|s| s.to_string()).unwrap_or_default(),
                    None => p.text.trim().to_string(),
                };
                self.prompt = None;
                if !spec.is_empty() {
                    self.add_host(&spec);
                }
            }
            PromptKey::Esc => self.prompt = None,
            _ => {}
        }
    }

    fn open_conn(&mut self, host: usize, session: &str) {
        if self.conns.iter().any(|c| c.host == host && c.session == session) {
            return;
        }
        let ssh = self.hosts.get(host).and_then(|h| h.ssh.clone());
        let id = self.next_id;
        self.next_id += 1;
        match tmuxcc::Conn::open(id, host, ssh.as_deref(), session, self.cc_tx.clone(), self.wake.clone()) {
            Ok(c) => {
                let g = self.grid_size();
                if self.report_size {
                    c.writer.set_size(g.cols, g.lines);
                }
                self.conns.push(c);
            }
            Err(e) => eprintln!("punkterm: tmux attach {session} failed: {e}"),
        }
    }

    /// A new session on `host`: a new window in the tmux session already
    /// there (cc-rail adopts it like any other), else a new tmux session —
    /// or, locally without tmux, a plain shell.
    pub fn new_session(&mut self, host: usize) {
        self.sound.play(Sfx::Open);
        let Some(h) = self.hosts.get(host).cloned() else { return };
        let session = self.panes.get(self.active).filter(|p| p.host == host && p.conn.is_some()).and_then(|p| p.session.clone());
        let conn = self
            .conns
            .iter()
            .find(|c| c.host == host && session.as_ref().is_none_or(|s| *s == c.session))
            .map(|c| (c.id, c.writer.clone()));
        if let Some((id, w)) = conn {
            self.focus_new = Some(id);
            w.command(tmuxcc::Want::Ignore, "new-window");
            return;
        }
        if h.ssh.is_none() && !tmux_installed() {
            self.spawn_pane(Spawn { program: None, args: vec![], cwd: std::env::current_dir().ok() }, host, None, true);
            return;
        }
        let (dest, tx, wake) = (h.ssh.clone(), self.disc_tx.clone(), self.wake.clone());
        std::thread::spawn(move || {
            tmuxcc::new_detached_session(dest.as_deref(), "main");
            let _ = tx.send((host, tmuxcc::list_sessions(dest.as_deref())));
            wake();
        });
    }

    /// Split the active tmux pane, side by side or stacked, as tmux's
    /// `split-window -h` / `-v` do; the panel shows the window's panes
    /// together. Plain shells open a new one.
    pub fn split(&mut self, side_by_side: bool) {
        let Some(p) = self.panes.get(self.active) else { return };
        let (Some(conn), Some(pane)) = (p.conn, p.tmux_pane.clone()) else {
            let host = p.host;
            self.new_session(host);
            return;
        };
        if let Some(c) = self.conns.iter().find(|c| c.id == conn) {
            self.sound.play(Sfx::Open);
            self.focus_new = Some(conn);
            let dir = if side_by_side { "-h" } else { "-v" };
            c.writer.command(tmuxcc::Want::Ignore, &format!("split-window {dir} -t {pane} -c \"#{{pane_current_path}}\""));
        }
    }

    /// Zoom the active tmux pane to its whole window, or back (tmux's
    /// `resize-pane -Z`).
    pub fn toggle_zoom(&mut self) {
        let Some(p) = self.panes.get(self.active) else { return };
        let (Some(conn), Some(pane)) = (p.conn, p.tmux_pane.clone()) else { return };
        if let Some(c) = self.conns.iter().find(|c| c.id == conn) {
            c.writer.command(tmuxcc::Want::Ignore, &format!("resize-pane -Z -t {pane}"));
            c.writer.list_panes();
        }
    }

    /// The panes shown together with pane `i`: all of its tmux window's
    /// (rails aside), unless the window is zoomed or holds just that one.
    fn group(&self, i: usize) -> Vec<usize> {
        let Some(p) = self.panes.get(i) else { return Vec::new() };
        let (Some(conn), Some(w)) = (p.conn, p.window.as_ref()) else { return vec![i] };
        if p.zoomed {
            return vec![i];
        }
        let g: Vec<usize> = (0..self.panes.len()).filter(|&j| self.panes[j].conn == Some(conn) && self.panes[j].window.as_ref() == Some(w)).collect();
        if g.len() > 1 { g } else { vec![i] }
    }

    /// Pane `i`'s tmux window, when it shares that window with other shown
    /// panes (zoomed or not): the rail groups such panes.
    pub(crate) fn split_key(&self, i: usize) -> Option<(u64, &str)> {
        let p = self.panes.get(i)?;
        let key = (p.conn?, p.window.as_deref()?);
        let n = self.panes.iter().filter(|q| q.conn == Some(key.0) && q.window.as_deref() == Some(key.1)).count();
        (n > 1).then_some(key)
    }

    /// Reorder the panes so each split window's panes are adjacent (at its
    /// first pane's place), in reading order: top to bottom, left to right.
    fn regroup(&mut self) {
        let n = self.panes.len();
        let mut order: Vec<usize> = Vec::with_capacity(n);
        for i in 0..n {
            if order.contains(&i) {
                continue;
            }
            let Some(key) = self.split_key(i) else {
                order.push(i);
                continue;
            };
            let mut mates: Vec<usize> = (i..n).filter(|&j| self.split_key(j) == Some(key)).collect();
            mates.sort_by_key(|&j| (self.panes[j].pos.1, self.panes[j].pos.0));
            order.extend(mates);
        }
        if order.iter().enumerate().all(|(k, &i)| k == i) {
            return;
        }
        let active_id = self.panes.get(self.active).map(|p| p.id);
        let mut slots: Vec<Option<(Pane, SessionUi)>> = self.panes.drain(..).zip(self.ui.drain(..)).map(Some).collect();
        for i in order {
            let (p, u) = slots[i].take().expect("each pane once");
            self.panes.push(p);
            self.ui.push(u);
        }
        self.active = active_id.and_then(|a| self.panes.iter().position(|p| p.id == a)).unwrap_or(0);
    }

    /// Where pane `i` is drawn within `term`: all of it, or the pane's place
    /// in its split tmux window (cut to `term` when the window is bigger).
    fn pane_area(&self, i: usize, term: Rect) -> Rect {
        let g = self.group(i);
        if g.len() < 2 {
            return term;
        }
        let ox = g.iter().map(|&j| self.panes[j].pos.0).min().unwrap_or(0);
        let oy = g.iter().map(|&j| self.panes[j].pos.1).min().unwrap_or(0);
        let p = &self.panes[i];
        let (cw, ch) = (self.glyphs.cell_w, self.glyphs.cell_h);
        let x = term.x + (p.pos.0 - ox) as f32 * cw;
        let y = term.y + (p.pos.1 - oy) as f32 * ch;
        let w = (p.size.cols as f32 * cw).min(term.x + term.w - x).max(0.0);
        let h = (p.size.lines as f32 * ch).min(term.y + term.h - y).max(0.0);
        Rect { x, y, w, h }
    }

    /// The pane of the active view under (x, y).
    fn pane_at(&self, x: f32, y: f32) -> Option<usize> {
        let term = self.layout().term;
        self.group(self.active).into_iter().find(|&j| {
            let a = self.pane_area(j, term);
            x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
        })
    }

    /// Move to the neighbouring pane of a split window, like tmux's
    /// `select-pane -L/-R/-U/-D`.
    /// Focus the nearest pane in a direction; false when there is none.
    pub fn focus_dir(&mut self, dx: i32, dy: i32) -> bool {
        let term = self.layout().term;
        let cur = self.pane_area(self.active, term);
        let (cx, cy) = (cur.x + cur.w / 2.0, cur.y + cur.h / 2.0);
        let best = self
            .group(self.active)
            .into_iter()
            .filter(|&j| j != self.active)
            .filter_map(|j| {
                let a = self.pane_area(j, term);
                let ahead = match (dx, dy) {
                    (1, _) => a.x >= cur.x + cur.w - 1.0,
                    (-1, _) => a.x + a.w <= cur.x + 1.0,
                    (_, 1) => a.y >= cur.y + cur.h - 1.0,
                    _ => a.y + a.h <= cur.y + 1.0,
                };
                let overlaps = if dx != 0 { a.y < cur.y + cur.h && a.y + a.h > cur.y } else { a.x < cur.x + cur.w && a.x + a.w > cur.x };
                (ahead && overlaps).then(|| (j, (a.x + a.w / 2.0 - cx).abs() + (a.y + a.h / 2.0 - cy).abs()))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((j, _)) = best {
            self.select(j);
        }
        best.is_some()
    }

    /// Apply a pane listing from one connection: add, update, drop.
    fn sync_panes(&mut self, conn: u64, lines: &[String]) {
        let Some(c) = self.conns.iter().find(|c| c.id == conn) else { return };
        let (host, writer) = (c.host, c.writer.clone());
        let all: Vec<tmuxcc::PaneInfo> = lines.iter().filter_map(|l| tmuxcc::parse_pane(l)).collect();
        // cc-rail's rail sits beside a session in the same window, and
        // punkterm does not show it: make such windows wider by the rail, so
        // the session itself fills punkterm's panel. Window resizes scale
        // the rail too and cc-rail does not restore it, so keep each rail
        // at the width cc-rail is set to.
        if self.report_size {
            let g = self.grid_size();
            // cc-rail's default until the host's setting is read.
            let rw = self.usage.get(&host).and_then(|u| u.rail_width).unwrap_or(34);
            // Panes side by side next to a window's rails: each needs
            // MIN_COLS, or holding a rail at width in a narrow window
            // squeezes its neighbours to a column.
            let beside = |w: &str| all.iter().filter(|p| p.window_id == w && !p.rail).map(|p| p.pos.0).collect::<std::collections::BTreeSet<_>>().len().max(1);
            for r in all.iter().filter(|p| p.rail) {
                if r.window_size.0 >= rw + 1 + MIN_COLS * beside(&r.window_id) {
                    writer.pin_width(&r.id, rw, r.cols);
                }
            }
            let mut windows: Vec<&str> = all.iter().map(|p| p.window_id.as_str()).collect();
            windows.dedup();
            for w in windows {
                let panes: Vec<&tmuxcc::PaneInfo> = all.iter().filter(|p| p.window_id == w).collect();
                let shown: Vec<&&tmuxcc::PaneInfo> = panes.iter().filter(|p| !p.rail).collect();
                let (Some(p), true) = (shown.first(), panes.len() > shown.len()) else { continue };
                let (ww, wh) = p.window_size;
                if ww == 0 {
                    continue;
                }
                // The shown panes' extent: one pane, or a split window's.
                let left = shown.iter().map(|p| p.pos.0).min().unwrap_or(0);
                let top = shown.iter().map(|p| p.pos.1).min().unwrap_or(0);
                let cols = shown.iter().map(|p| p.pos.0 + p.cols).max().unwrap_or(0) - left;
                let lines = shown.iter().map(|p| p.pos.1 + p.lines).max().unwrap_or(0) - top;
                let extra_w = (panes.len() - shown.len()) * (rw + 1);
                if cols.abs_diff(g.cols) > 1 || lines.abs_diff(g.lines) > 1 || ww != g.cols + extra_w {
                    if writer.set_window_size(w, g.cols + extra_w, g.lines + wh.saturating_sub(lines)) {
                        // tmux spreads a resize over every pane; give the
                        // rails their width straight back.
                        if g.cols >= MIN_COLS * beside(w) {
                            for r in panes.iter().filter(|p| p.rail) {
                                writer.command(tmuxcc::Want::Ignore, &format!("resize-pane -t {} -x {rw}", r.id));
                            }
                        }
                    }
                }
            }
        }
        // Rails included: they follow the terminal's theme too.
        let rgb = |c: Rgba| [c[0], c[1], c[2]].map(|v| (v * 255.0).round() as u8);
        let ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        writer.report_colours(&ids, rgb(self.theme_target.fg), rgb(self.theme_target.surface));
        let infos: Vec<tmuxcc::PaneInfo> = all.into_iter().filter(|p| !p.rail).collect();
        let hostname = self.hostnames.get(&host).cloned().unwrap_or_default();
        let mut focus = None;
        for info in &infos {
            if self.hidden.contains(&(conn, info.id.clone())) {
                continue;
            }
            let job = self.usage.get(&host).and_then(|u| u.names.get(&info.id));
            let title = if let Some(name) = job {
                name.clone()
            } else if info.title.is_empty() || info.title == hostname || is_hostname_like(&info.title) {
                info.window_name.clone()
            } else {
                info.title.clone()
            };
            let sub = format!("{}:{} · {}", info.session, info.window_index, info.command);
            if let Some(p) = self.panes.iter_mut().find(|p| p.conn == Some(conn) && p.tmux_pane.as_deref() == Some(info.id.as_str())) {
                p.set_tmux_size(info.cols, info.lines);
                p.title = title;
                p.subtitle = sub;
                p.tmux_cursor = info.cursor;
                p.tmux_alt = info.alt;
                p.window = Some(info.window_id.clone());
                p.pos = info.pos;
                p.zoomed = info.zoomed;
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            let size = Size { cols: info.cols.max(2), lines: info.lines.max(1), cell_w: self.glyphs.cell_w as u16, cell_h: self.glyphs.cell_h as u16 };
            let mut pane = Pane::tmux(id, conn, writer.clone(), &info.id, size, self.tx.clone(), self.wake.clone());
            pane.host = host;
            pane.title = title;
            pane.subtitle = sub;
            pane.session = Some(info.session.clone());
            pane.tmux_cursor = info.cursor;
            pane.tmux_alt = info.alt;
            pane.window = Some(info.window_id.clone());
            pane.pos = info.pos;
            pane.zoomed = info.zoomed;
            writer.capture(&info.id);
            let at = self.panes.iter().rposition(|q| q.host <= host).map(|i| i + 1).unwrap_or(0);
            let active_id = self.panes.get(self.active).map(|p| p.id);
            self.panes.insert(at, pane);
            self.ui.insert(at, SessionUi::new());
            if let Some(aid) = active_id {
                self.active = self.panes.iter().position(|p| p.id == aid).unwrap_or(0);
            }
            if self.focus_new == Some(conn) || self.panes.len() == 1 {
                focus = Some(id);
            }
        }
        // Panes that went away in tmux.
        let alive: Vec<&str> = infos.iter().map(|i| i.id.as_str()).collect();
        let active_id = self.panes.get(self.active).map(|p| p.id);
        let mut k = 0;
        while k < self.panes.len() {
            let p = &self.panes[k];
            if p.conn == Some(conn) && !p.tmux_pane.as_deref().is_some_and(|id| alive.contains(&id)) {
                self.panes.remove(k);
                self.ui.remove(k);
            } else {
                k += 1;
            }
        }
        self.active = active_id.and_then(|a| self.panes.iter().position(|p| p.id == a)).unwrap_or(0).min(self.panes.len().saturating_sub(1));
        self.regroup();
        if let Some(id) = focus {
            self.focus_new = None;
            if let Some(i) = self.panes.iter().position(|p| p.id == id) {
                self.select(i);
            }
        }
    }

    /// Lay a pane's captured history and screen into its terminal.
    fn apply_capture(&mut self, conn: u64, pane: &str, lines: &[String]) {
        let Some(p) = self.panes.iter_mut().find(|p| p.conn == Some(conn) && p.tmux_pane.as_deref() == Some(pane)) else { return };
        let rows = p.size.lines;
        let mut out = Vec::new();
        let lines: &[String] = if p.tmux_alt {
            out.extend_from_slice(b"\x1b[?1049h\x1b[H");
            &lines[lines.len().saturating_sub(rows)..]
        } else {
            lines
        };
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(b"\x1b[0m\r\n");
            }
            out.extend_from_slice(l.as_bytes());
        }
        let (cx, cy) = p.tmux_cursor;
        out.extend_from_slice(format!("\x1b[0m\x1b[{};{}H", cy + 1, cx + 1).as_bytes());
        p.feed(&out);
        p.awaiting_capture = false;
    }

    // ------------------------------------------------------------ preferences

    pub fn apply_prefs(&mut self, p: &Prefs, theme_flag: bool) {
        if let Some(o) = p.opacity {
            self.opacity = o.clamp(0.3, 1.0);
        }
        if let Some(s) = &p.sound {
            self.sound.set_mode(sound::Mode::parse(s));
        }
        self.saved_hosts = p.hosts.clone();
        self.timeline_open = p.timeline.unwrap_or(false);
        if !theme_flag {
            if let Some(name) = &p.theme {
                let t = Theme::load(Some(name));
                self.theme = t.clone();
                self.theme_target = t;
            }
        }
    }

    fn save_prefs(&self) {
        state::save(&Prefs {
            theme: Some(self.theme_target.name.clone()),
            opacity: Some(self.opacity),
            sound: Some(self.sound.mode.name().to_string()),
            hosts: self.saved_hosts.clone(),
            timeline: Some(self.timeline_open),
        });
    }

    /// Expand or collapse the timeline rail; the terminal takes up the rest.
    pub fn toggle_timeline(&mut self) {
        self.timeline_open = !self.timeline_open;
        self.tl_scroll = 0.0;
        self.save_prefs();
        self.resize(self.size, self.scale);
    }

    pub fn timeline_at(&self, x: f32, y: f32) -> bool {
        let r = self.layout().timeline;
        x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
    }

    pub fn timeline_header_at(&self, y: f32) -> bool {
        y < self.layout().timeline.y + self.px(62.0)
    }

    /// Track the mouse over the open timeline. Returns true when the hover
    /// changed (redraw), and whether it is over an item (pointer cursor).
    pub fn hover_timeline(&mut self, x: f32, y: f32) -> (bool, bool) {
        let next = (self.timeline_open && self.timeline_at(x, y)).then_some((x, y));
        let over = |h: Option<(f32, f32)>| h.and_then(|(x, y)| self.tl_hits.iter().position(|(r, ..)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h));
        let changed = over(next) != over(self.hover_tl);
        self.hover_tl = next;
        (changed, over(next).is_some())
    }

    /// The middle of the Nth drawn timeline item counting from the newest
    /// (1-based), for snapshots.
    pub fn timeline_item_center(&self, n: usize) -> Option<(f32, f32)> {
        let (r, ..) = self.tl_hits.get(n.checked_sub(1)?)?;
        Some((r.x + r.w / 2.0, r.y + r.h / 2.0))
    }

    /// Click on a timeline item: scroll the main view back to where that
    /// message is in the pane's scrollback, and mark its first line.
    pub fn timeline_click(&mut self, x: f32, y: f32) -> bool {
        use crate::timeline::Kind;
        let Some(i) = self.tl_hits.iter().position(|(r, ..)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) else { return false };
        let (kind, text) = (self.tl_hits[i].1, self.tl_hits[i].2.clone());
        let needle = |kind: Kind, text: &str| -> String {
            match kind {
                // Claude Code shows a tool call as `Name(…)`, not its description.
                // and an MCP tool `mcp__server__tool` as `server - tool (MCP)`.
                Kind::Tool => {
                    let name = text.split_whitespace().next().unwrap_or("");
                    match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
                        Some((server, tool)) => format!("{server} - {tool}"),
                        None => format!("{name}("),
                    }
                }
                _ => squash(text).chars().take(28).collect(),
            }
        };
        let want = needle(kind, &text);
        if want.chars().count() < 3 {
            return false;
        }
        let Some(p) = self.panes.get(self.active) else { return false };
        let mut term = p.term.lock();
        let (top, bottom) = (term.topmost_line().0, term.bottommost_line().0);
        let cols = term.columns();
        let rows: Vec<String> = (top..=bottom)
            .map(|l| {
                let row = &term.grid()[Line(l)];
                squash(&(0..cols).map(|c| row[Column(c)].c).collect::<String>())
            })
            .collect();
        let last = |n: &str, below: usize| rows[..below].iter().rposition(|r| r.contains(n));
        // Claude Code reprints the conversation when the pane resizes, so
        // a message can be on screen several times: take the copy just above
        // where the next message is, when that can be found.
        let next = i.checked_sub(1).and_then(|n| self.tl_hits.get(n)).map(|(_, k, t)| needle(*k, t));
        let below = next.and_then(|n| last(&n, rows.len())).unwrap_or(rows.len());
        // Some calls never show (MCP tools, for one): land where the
        // message after them starts, which is where they ran.
        let fallback = (kind == Kind::Tool && below < rows.len()).then_some(below);
        let found = last(&want, below).or_else(|| last(&want, rows.len())).or(fallback).map(|r| top + r as i32);
        let Some(l) = found else {
            drop(term);
            self.toast("NOT IN THIS PANE'S SCROLLBACK");
            return true;
        };
        // Viewport row r shows Line(r - offset): put the match near the top.
        let want = (2 - l).clamp(0, term.history_size() as i32);
        let delta = want - term.grid().display_offset() as i32;
        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        term.selection = Some(Selection::new(SelectionType::Lines, Point::new(Line(l), Column(0)), Side::Left));
        true
    }

    /// Wheel over the timeline: `lines` > 0 goes back in time.
    pub fn scroll_timeline(&mut self, lines: f32) {
        self.tl_scroll = (self.tl_scroll + lines * self.px(18.0)).max(0.0);
    }

    /// The (host, tmux pane) of the active session, if it is a tmux pane.
    fn timeline_key(&self) -> Option<(usize, String)> {
        let p = self.panes.get(self.active)?;
        Some((p.host, p.tmux_pane.clone()?))
    }

    pub(super) fn active_timeline(&self) -> Option<&crate::timeline::Timeline> {
        self.timelines.get(&self.timeline_key()?)
    }

    /// Re-read the active pane's conversation: at once on switching panes,
    /// then every few seconds (more often while the rail is open).
    fn poll_timeline(&mut self) {
        let Some(key) = self.timeline_key() else { return };
        let every = if self.timeline_open { 3 } else { 10 };
        let due = match &self.tl_last {
            Some((k, t)) => *k != key || t.elapsed().as_secs() >= every,
            None => true,
        };
        if self.tl_busy || !due {
            return;
        }
        if self.tl_last.as_ref().is_some_and(|(k, _)| *k != key) {
            self.tl_scroll = 0.0;
        }
        self.tl_busy = true;
        self.tl_last = Some((key.clone(), Instant::now()));
        let ssh = self.hosts.get(key.0).and_then(|h| h.ssh.clone());
        let (tx, wake) = (self.tl_tx.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let t = crate::timeline::fetch(ssh.as_deref(), &key.1);
            let _ = tx.send((key, t));
            wake();
        });
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    /// Crossfade to the next (or previous) theme.
    pub fn cycle_theme(&mut self, d: isize) {
        let names = Theme::names();
        let cur = names.iter().position(|n| *n == self.theme_target.name).unwrap_or(0) as isize;
        let next = names[((cur + d).rem_euclid(names.len() as isize)) as usize].clone();
        let mut t = Theme::load(Some(&next));
        // Fonts stay as they are; only colours change.
        t.mono_font = self.theme.mono_font.clone();
        t.ui_font = self.theme.ui_font.clone();
        self.theme_from = Some((self.theme.clone(), Instant::now()));
        self.theme_target = t;
        // Re-list now so tmux hears the new colours without waiting.
        for c in &self.conns {
            c.writer.list_panes();
        }
        self.sound.play(Sfx::Theme);
        self.toast(format!("THEME // {}", next.to_uppercase()));
        self.save_prefs();
    }

    pub fn cycle_opacity(&mut self) {
        if !self.alpha_ok {
            self.toast("OPACITY // NOT SUPPORTED HERE");
            return;
        }
        const STEPS: [f32; 4] = [1.0, 0.9, 0.8, 0.7];
        let i = STEPS.iter().position(|s| (s - self.opacity).abs() < 0.02).unwrap_or(0);
        self.opacity = STEPS[(i + 1) % STEPS.len()];
        self.sound.play(Sfx::Blip);
        self.toast(format!("OPACITY // {}%", (self.opacity * 100.0).round()));
        self.save_prefs();
    }

    pub fn cycle_sound(&mut self) {
        if !self.sound.available() {
            self.toast("SOUND // NO PLAYER (pw-play / paplay)");
            return;
        }
        let m = self.sound.mode.next();
        self.sound.set_mode(m);
        self.sound.play(if m == sound::Mode::Alerts { Sfx::Ready } else { Sfx::Blip });
        self.toast(format!("SOUND // {}", m.name().to_uppercase()));
        self.save_prefs();
    }

    pub fn toggle_help(&mut self) {
        self.help = !self.help;
        self.sound.play(Sfx::Blip);
    }

    pub fn toggle_overview(&mut self) {
        self.overview = !self.overview;
        if self.overview {
            self.overview_sel = self.active;
            self.overview_opened = Instant::now();
        }
        self.sound.play(Sfx::Blip);
    }

    /// Move the dashboard selection by whole tiles.
    pub fn overview_move(&mut self, dx: isize, dy: isize) {
        let n = self.panes.len();
        if n == 0 {
            return;
        }
        let cols = self.overview_cols() as isize;
        let i = self.overview_sel as isize + dx + dy * cols;
        self.overview_sel = i.clamp(0, n as isize - 1) as usize;
        self.sound.play(Sfx::Tick);
    }

    pub fn overview_open(&mut self, i: usize) {
        self.overview = false;
        self.select(i);
    }

    pub fn active_host(&self) -> usize {
        self.panes.get(self.active).map(|p| p.host).unwrap_or(0)
    }

    pub fn host_color(&self, h: usize) -> Rgba {
        let t = &self.theme;
        self.hosts.get(h).and_then(|h| h.color).unwrap_or_else(|| {
            let palette = [t.accent2, t.warning, t.ansi[5], t.ok, t.ansi[4]];
            palette[h % palette.len()]
        })
    }

    pub fn select(&mut self, i: usize) {
        if i >= self.panes.len() {
            return;
        }
        let same_view = self.group(self.active).contains(&i);
        if same_view && i != self.active {
            // Same window: just move the focus, and tell tmux, as clicking
            // a pane there would.
            if let (Some(conn), Some(pane)) = (self.panes[i].conn, self.panes[i].tmux_pane.as_deref()) {
                if let Some(c) = self.conns.iter().find(|c| c.id == conn) {
                    c.writer.command(tmuxcc::Want::Ignore, &format!("select-pane -t {pane}"));
                }
            }
        } else if i != self.active && self.active < self.panes.len() {
            let dir = if i > self.active { 1.0 } else { -1.0 };
            self.transition = Some(Transition { from: self.panes[self.active].id, started: Instant::now(), dir });
        }
        if i != self.active {
            self.sound.play(Sfx::Tick);
        }
        self.active = i;
        self.ui[i].unseen = false;
    }

    pub fn cycle(&mut self, d: isize) {
        let n = self.panes.len() as isize;
        if n > 0 {
            self.select(((self.active as isize + d).rem_euclid(n)) as usize);
        }
    }

    /// Close the active session's view. A tmux pane is only hidden here —
    /// it keeps running in tmux, and shows again on the next launch.
    pub fn close_active(&mut self) {
        let Some(p) = self.panes.get(self.active) else { return };
        if let (Some(conn), Some(tp)) = (p.conn, p.tmux_pane.clone()) {
            self.hidden.insert((conn, tp));
            self.panes.remove(self.active);
            self.ui.remove(self.active);
            self.active = self.active.min(self.panes.len().saturating_sub(1));
            self.transition = None;
            if self.panes.is_empty() {
                self.quit = true;
            }
        } else {
            p.term.lock().exit();
        }
    }

    /// The last `k` non-blank lines of a pane's screen, for previews.
    fn preview(&self, idx: usize, k: usize) -> Vec<String> {
        use alacritty_terminal::index::{Column, Line};
        let term = self.panes[idx].term.lock();
        let grid = term.grid();
        let (cols, lines) = (term.columns(), term.screen_lines());
        let mut out: Vec<String> = Vec::new();
        for l in (0..lines).rev() {
            let row = &grid[Line(l as i32)];
            let s: String = (0..cols).map(|c| row[Column(c)].c).collect::<String>().trim_end().to_string();
            if s.trim().is_empty() || s.chars().all(|c| "─━│╭╮╯╰ ".contains(c)) {
                continue;
            }
            out.push(s);
            if out.len() == k {
                break;
            }
        }
        out.reverse();
        out
    }

    /// The host whose "+" button is under (x, y).
    pub fn add_at(&self, x: f32, y: f32) -> Option<usize> {
        let pad = self.px(3.0);
        self.layout().adds.iter().position(|r| x >= r.x - pad && x <= r.x + r.w + pad && y >= r.y - pad && y <= r.y + r.h + pad)
    }

    pub fn card_at(&self, x: f32, y: f32) -> Option<usize> {
        self.layout().cards.iter().position(|c| x >= c.x && x <= c.x + c.w && y >= c.y && y <= c.y + c.h)
    }

    /// Wheel scrolling goes to the app when it wants it (mouse reporting,
    /// or arrows in a full-screen app); `own` (Shift held) always scrolls
    /// punkterm's own history.
    pub fn scroll(&mut self, lines: f32, own: bool) {
        // Touchpads deliver fractions of a line; keep the remainder.
        self.scroll_acc += lines;
        let n = self.scroll_acc.trunc() as i32;
        if n == 0 {
            return;
        }
        self.scroll_acc -= n as f32;
        let Some(p) = self.panes.get(self.active) else { return };
        let mode = *p.term.lock().mode();
        if own {
            p.term.lock().scroll_display(alacritty_terminal::grid::Scroll::Delta(n));
        } else if mode.intersects(TermMode::MOUSE_MODE) && mode.contains(TermMode::SGR_MOUSE) {
            let b = if n > 0 { 64 } else { 65 };
            let ev = format!("\x1b[<{b};1;1M").repeat(n.unsigned_abs() as usize);
            p.write(ev.into_bytes());
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let k = if n > 0 { "\x1bOA" } else { "\x1bOB" };
            p.write(k.repeat(n.unsigned_abs() as usize).into_bytes());
        } else {
            p.term.lock().scroll_display(alacritty_terminal::grid::Scroll::Delta(n));
        }
    }

    /// The cell of the active pane under (x, y), clamped to the terminal,
    /// and which half of it. `None` when the point is outside the panel.
    fn term_point(&self, x: f32, y: f32, clamp: bool) -> Option<(Point, Side)> {
        self.term_point_in(self.active, x, y, clamp)
    }

    fn term_point_in(&self, i: usize, x: f32, y: f32, clamp: bool) -> Option<(Point, Side)> {
        let area = self.pane_area(i, self.layout().term);
        if !clamp && !(x >= area.x && x < area.x + area.w && y >= area.y && y < area.y + area.h) {
            return None;
        }
        let p = self.panes.get(i)?;
        let term = p.term.lock();
        let (cw, ch) = (self.glyphs.cell_w, self.glyphs.cell_h);
        let lines = term.screen_lines();
        let overflow = ((lines as f32 * ch - area.h) / ch).ceil().max(0.0);
        let oy = area.y - overflow * ch;
        let fx = ((x - area.x) / cw).max(0.0);
        let col = (fx as usize).min(term.columns().saturating_sub(1));
        let row = (((y - oy) / ch).floor().max(overflow) as i32).min(lines as i32 - 1);
        let side = if fx.fract() < 0.5 { Side::Left } else { Side::Right };
        Some((Point::new(Line(row - term.grid().display_offset() as i32), Column(col)), side))
    }

    /// Left press: start a selection in the active pane (two clicks select a
    /// word, three a line). Returns false when the press was not on it.
    pub fn mouse_down(&mut self, x: f32, y: f32) -> bool {
        if let Some(j) = self.pane_at(x, y).filter(|&j| j != self.active) {
            self.clear_selection();
            self.select(j);
        }
        let Some((point, side)) = self.term_point(x, y, false) else { return false };
        let cell = (point.line.0, point.column.0);
        let n = match self.last_click {
            Some((t, c, n)) if c == cell && t.elapsed().as_millis() < 400 => n % 3 + 1,
            _ => 1,
        };
        self.last_click = Some((Instant::now(), cell, n));
        let ty = [SelectionType::Simple, SelectionType::Semantic, SelectionType::Lines][n as usize - 1];
        self.panes[self.active].term.lock().selection = Some(Selection::new(ty, point, side));
        self.selecting = true;
        true
    }

    /// Drag: extend the selection, scrolling back when dragged past an edge.
    pub fn mouse_drag(&mut self, x: f32, y: f32) -> bool {
        if !self.selecting {
            return false;
        }
        let area = self.layout().term;
        let edge = if y < area.y { 1 } else if y >= area.y + area.h { -1 } else { 0 };
        if edge != 0 {
            self.panes[self.active].term.lock().scroll_display(alacritty_terminal::grid::Scroll::Delta(edge));
        }
        let Some((point, side)) = self.term_point(x, y, true) else { return false };
        if let Some(sel) = self.panes[self.active].term.lock().selection.as_mut() {
            sel.update(point, side);
        }
        true
    }

    /// The link at (x, y) in the active pane.
    fn link_at(&self, x: f32, y: f32) -> Option<(u64, crate::links::Link)> {
        let i = self.pane_at(x, y).unwrap_or(self.active);
        let (point, _) = self.term_point_in(i, x, y, false)?;
        let p = self.panes.get(i)?;
        crate::links::at(&p.term.lock(), point).map(|l| (p.id, l))
    }

    /// Track the link under the pointer (only while `armed`, i.e. Ctrl is
    /// held). Returns true when that changed.
    pub fn hover_link(&mut self, x: f32, y: f32, armed: bool) -> bool {
        let next = if armed { self.link_at(x, y) } else { None };
        let key = |h: &Option<(u64, crate::links::Link)>| h.as_ref().map(|(id, l)| (*id, l.uri.clone(), l.cells.first().copied()));
        let changed = key(&self.hover) != key(&next);
        self.hover = next;
        changed
    }

    pub fn hovering_link(&self) -> bool {
        self.hover.is_some()
    }

    /// Ctrl+click: open the link at (x, y). False when there is none.
    pub fn open_link(&mut self, x: f32, y: f32) -> bool {
        let Some((_, link)) = self.link_at(x, y) else { return false };
        crate::links::open(&link.uri);
        self.sound.play(Sfx::Open);
        self.toast(format!("OPEN // {}", truncate(&link.uri, 60)));
        true
    }

    /// Release: the selection becomes the primary selection, like any X/Wayland
    /// terminal; Ctrl+Shift+C puts it on the clipboard.
    pub fn mouse_up(&mut self) {
        if std::mem::take(&mut self.selecting) {
            if let Some(t) = self.selected_text() {
                crate::clipboard::copy_primary(&t);
            }
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        self.panes.get(self.active)?.term.lock().selection_to_string().filter(|t| !t.is_empty())
    }

    pub fn clear_selection(&mut self) {
        if let Some(p) = self.panes.get(self.active) {
            p.term.lock().selection = None;
        }
    }

    pub fn active_pane(&self) -> Option<&Pane> {
        self.panes.get(self.active)
    }

    pub fn set_font_px(&mut self, px: f32) {
        self.mono_px = px.clamp(8.0, 32.0);
        self.glyphs.set_size(self.mono_px * self.scale);
        self.resize(self.size, self.scale);
        self.toast(format!("TEXT // {}PX", self.mono_px));
    }

    pub fn resize(&mut self, size: (u32, u32), scale: f32) {
        if (scale - self.scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.glyphs.set_size(self.mono_px * scale);
        }
        self.size = size;
        let g = self.grid_size();
        for p in &mut self.panes {
            p.resize(g);
        }
        // Tell tmux how big punkterm is, as any attached terminal would:
        // with tmux's default `window-size latest`, windows fit punkterm
        // while it is the client in use, and kitty's size when kitty is.
        for c in self.conns.iter().filter(|_| self.report_size) {
            c.writer.set_size(g.cols, g.lines);
            c.writer.list_panes();
        }
    }

    /// Apply terminal events. Returns true if anything needs redrawing.
    pub fn pump(&mut self) -> bool {
        let mut dirty = false;
        while let Ok((id, ev)) = self.rx.try_recv() {
            dirty = true;
            let Some(i) = self.panes.iter().position(|p| p.id == id) else { continue };
            match ev {
                Event::Wakeup => {
                    if i != self.active {
                        self.ui[i].unseen = true;
                    }
                }
                Event::Title(t) => self.panes[i].title = t,
                Event::ResetTitle => self.panes[i].title.clear(),
                // Answers to terminal queries. For tmux panes tmux already
                // answered; echoing ours back would type into the pane.
                Event::PtyWrite(_) | Event::ColorRequest(..) | Event::TextAreaSizeRequest(_) if self.panes[i].is_tmux() => {}
                Event::PtyWrite(s) => self.panes[i].write(s.into_bytes()),
                Event::ColorRequest(idx, f) => {
                    let c = self.color_index(idx);
                    let rgb = Rgb { r: (c[0] * 255.0) as u8, g: (c[1] * 255.0) as u8, b: (c[2] * 255.0) as u8 };
                    self.panes[i].write(f(rgb).into_bytes());
                }
                Event::TextAreaSizeRequest(f) => {
                    let s = self.panes[i].size;
                    let ws = alacritty_terminal::event::WindowSize {
                        num_lines: s.lines as u16,
                        num_cols: s.cols as u16,
                        cell_width: s.cell_w,
                        cell_height: s.cell_h,
                    };
                    self.panes[i].write(f(ws).into_bytes());
                }
                Event::ClipboardStore(_, text) => crate::clipboard::copy(&text),
                Event::Bell => self.ui[i].bell = 1.0,
                Event::ChildExit(_) | Event::Exit => self.panes[i].exited = true,
                _ => {}
            }
        }
        // Plan usage: every minute, for every reachable host.
        if self.last_usage.is_none_or(|t| t.elapsed().as_secs() >= 60) {
            self.last_usage = Some(Instant::now());
            for (i, h) in self.hosts.iter().enumerate() {
                if i == 0 || self.host_state.get(i) == Some(&HostState::Up) {
                    self.fetch_usage(i, h.ssh.clone());
                }
            }
        }
        self.poll_timeline();
        while let Ok((key, t)) = self.tl_rx.try_recv() {
            self.tl_busy = false;
            match t {
                Some(t) => {
                    if self.timelines.get(&key) != Some(&t) {
                        dirty = true;
                    }
                    self.timelines.insert(key, t);
                }
                None => dirty |= self.timelines.remove(&key).is_some(),
            }
        }
        while let Ok((host, u)) = self.usage_rx.try_recv() {
            let old = self.usage.get(&host);
            let rail_changed = old.and_then(|u| u.rail_width) != u.as_ref().and_then(|u| u.rail_width)
                || old.map(|u| &u.names) != u.as_ref().map(|u| &u.names);
            match u {
                Some(u) => self.usage.insert(host, u),
                None => self.usage.remove(&host),
            };
            // Size this host's windows now that the rail width is known, and
            // retitle panes whose background job got a name.
            if rail_changed {
                for c in self.conns.iter().filter(|c| c.host == host) {
                    c.writer.list_panes();
                }
            }
            dirty = true;
        }
        while let Ok((host, res)) = self.disc_rx.try_recv() {
            let name = self.hosts.get(host).map(|h| h.name.to_uppercase()).unwrap_or_default();
            match res {
                Ok(names) if names.is_empty() => {
                    self.host_state[host] = HostState::Empty;
                    self.toast(format!("{name} // NO TMUX SESSIONS"));
                }
                Ok(names) => {
                    self.host_state[host] = HostState::Up;
                    self.fetch_usage(host, self.hosts[host].ssh.clone());
                    for n in names {
                        self.open_conn(host, &n);
                    }
                }
                Err(e) => {
                    self.toast(format!("{name} // {}", e.to_uppercase()));
                    self.host_state[host] = HostState::Failed(e);
                }
            }
            dirty = true;
        }
        while let Ok(ev) = self.cc_rx.try_recv() {
            dirty = true;
            match ev {
                CcEvent::Output { conn, pane, data } => {
                    let active = self.active;
                    if let Some(i) = self.panes.iter().position(|p| p.conn == Some(conn) && p.tmux_pane.as_deref() == Some(pane.as_str())) {
                        if !self.panes[i].awaiting_capture {
                            self.panes[i].feed(&data);
                            if i != active {
                                self.ui[i].unseen = true;
                            }
                        }
                    }
                }
                CcEvent::Reply { conn, want, lines } => match want {
                    tmuxcc::Want::Panes => self.sync_panes(conn, &lines),
                    tmuxcc::Want::Capture(pane) => self.apply_capture(conn, &pane, &lines),
                    tmuxcc::Want::Ignore => {}
                },
                CcEvent::Changed { conn } => {
                    if let Some(c) = self.conns.iter().find(|c| c.id == conn) {
                        c.writer.list_panes();
                    }
                }
                CcEvent::Exit { conn } => {
                    self.conns.retain(|c| c.id != conn);
                    let active_id = self.panes.get(self.active).map(|p| p.id);
                    let keep: Vec<bool> = self.panes.iter().map(|p| p.conn != Some(conn)).collect();
                    let mut k = 0;
                    self.panes.retain(|_| { k += 1; keep[k - 1] });
                    let mut k = 0;
                    self.ui.retain(|_| { k += 1; keep[k - 1] });
                    self.active = active_id.and_then(|a| self.panes.iter().position(|p| p.id == a)).unwrap_or(0);
                }
            }
        }
        // Titles change without notifications: re-list now and then.
        if self.last_list.elapsed().as_millis() >= 1500 {
            self.last_list = Instant::now();
            for c in &self.conns {
                c.writer.list_panes();
            }
        }
        if self.last_detect.elapsed().as_millis() >= 150 {
            self.last_detect = Instant::now();
            dirty |= self.detect();
        }
        // Drop exited panes, keeping the selection on a neighbour.
        while let Some(i) = self.panes.iter().position(|p| p.exited) {
            self.panes.remove(i);
            self.ui.remove(i);
            if self.active >= i && self.active > 0 {
                self.active -= 1;
            }
            self.transition = None;
            if self.panes.is_empty() {
                self.quit = true;
            }
        }
        dirty
    }

    /// Re-read every session's Claude state. Returns true if any changed.
    fn detect(&mut self) -> bool {
        let mut changed = false;
        let light = self.theme_target.is_light();
        for (p, ui) in self.panes.iter().zip(self.ui.iter_mut()) {
            let r = claude::read(&p.term.lock(), &p.title);
            let status = if !r.claude {
                ui.was_working = false;
                Status::Shell
            } else if r.blocked {
                Status::Blocked
            } else if r.working || r.background_busy {
                ui.was_working = true;
                Status::Working
            } else if ui.was_working {
                Status::YourTurn
            } else {
                Status::Idle
            };
            if status != ui.status {
                if ui.detected {
                    match status {
                        Status::Blocked => self.sound.play(Sfx::Blocked),
                        Status::YourTurn => self.sound.play(Sfx::Ready),
                        _ => {}
                    }
                }
                ui.status = status;
                ui.since = Instant::now();
                changed = true;
            }
            ui.detected = true;
            // Claude's "auto" theme follows mode 2031 reports, which it only
            // gets from the terminal: tell each session when it appears and
            // on every light/dark switch. Typed as input, this also reaches
            // Claude inside tmux, which answers its startup colour query itself.
            if status == Status::Shell {
                ui.told_light = None;
            } else if ui.told_light != Some(light) {
                ui.told_light = Some(light);
                p.write(if light { &b"\x1b[?997;2n"[..] } else { &b"\x1b[?997;1n"[..] });
            }
            // Background work: beside the turn's own verb, or as the
            // activity when it alone keeps the session busy.
            ui.background = r.background.clone();
            let activity = match (&r.background, r.working, r.background_busy) {
                (Some(bg), false, true) => Some(bg.clone()),
                (Some(bg), true, _) => r.activity.map(|a| format!("{a} · {bg}")),
                _ => r.activity,
            };
            if activity.is_some() || !r.working {
                changed |= ui.activity != activity;
                ui.activity = activity;
            }
        }
        changed
    }

    /// Typing into a session answers it: "your turn" is over.
    pub fn note_input(&mut self) {
        if let Some(ui) = self.ui.get_mut(self.active) {
            if ui.status == Status::YourTurn {
                ui.status = Status::Idle;
                ui.since = Instant::now();
            }
            ui.was_working = false;
        }
    }

    /// Jump to the next session that needs you: blocked first, then your-turn.
    pub fn jump_attention(&mut self) {
        let n = self.ui.len();
        for want in [Status::Blocked, Status::YourTurn] {
            for k in 1..=n {
                let i = (self.active + k) % n;
                if self.ui[i].status == want && i != self.active {
                    self.select(i);
                    return;
                }
            }
        }
    }

    fn status_color(&self, s: Status) -> Rgba {
        let t = &self.theme;
        match s {
            Status::Blocked => t.alert,
            Status::Working => t.busy,
            Status::YourTurn => t.ready,
            Status::Idle => t.muted,
            Status::Shell => mix(t.muted, t.surface, 0.3),
        }
    }

    fn color_index(&self, idx: usize) -> Rgba {
        let t = &self.theme;
        match idx {
            0..=15 => t.ansi[idx],
            16..=231 => {
                let i = idx - 16;
                let lvl = |v: usize| if v == 0 { 0.0 } else { (55.0 + v as f32 * 40.0) / 255.0 };
                [lvl(i / 36), lvl((i / 6) % 6), lvl(i % 6), 1.0]
            }
            232..=255 => {
                let v = (8.0 + (idx - 232) as f32 * 10.0) / 255.0;
                [v, v, v, 1.0]
            }
            256 => t.fg,
            257 => t.surface,
            258 => t.cursor,
            _ => t.fg,
        }
    }

    /// `None` means "the panel's own background".
    fn resolve(&self, c: Color, overrides: &alacritty_terminal::term::color::Colors, bg: bool) -> Option<Rgba> {
        match c {
            Color::Spec(rgb) => Some([rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0, 1.0]),
            Color::Indexed(i) => Some(self.lookup(i as usize, overrides)),
            Color::Named(n) => {
                if bg && n == NamedColor::Background {
                    return None;
                }
                let idx = n as usize;
                let base = match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.theme.fg,
                    NamedColor::DimForeground => with_alpha(self.theme.fg, 0.6),
                    NamedColor::Background => self.theme.surface,
                    NamedColor::Cursor => self.theme.cursor,
                    _ if idx >= 259 && idx <= 266 => with_alpha(self.theme.ansi[idx - 259], 0.65),
                    _ => return Some(self.lookup(idx, overrides)),
                };
                Some(overrides[idx].map(rgb_to).unwrap_or(base))
            }
        }
    }

    fn lookup(&self, i: usize, overrides: &alacritty_terminal::term::color::Colors) -> Rgba {
        overrides[i].map(rgb_to).unwrap_or_else(|| self.color_index(i))
    }

    /// Draw a pane's visible grid. Returns true while the cursor is moving.
    #[allow(clippy::too_many_arguments)]
    fn draw_term(
        &mut self,
        q: &mut Vec<Quad>,
        idx: usize,
        area: Rect,
        dx: f32,
        alpha: f32,
        primary: bool,
        clip_x: Option<(f32, f32)>,
    ) -> bool {
        let t = self.theme.clone();
        let (cw, ch) = (self.glyphs.cell_w, self.glyphs.cell_h);
        let mut clip = [area.x - self.px(6.0), area.y - self.px(2.0), area.x + area.w + self.px(6.0), area.y + area.h + self.px(2.0)];
        if let Some((x0, x1)) = clip_x {
            clip[0] = x0;
            clip[2] = x1;
        }
        let ox = area.x + dx;
        let term = self.panes[idx].term.clone();
        let pane_id = self.panes[idx].id;
        let term = term.lock();
        // A tmux window taller than the panel (sized by another client)
        // shows its bottom rows: that is where prompts and input live.
        let overflow = ((term.screen_lines() as f32 * ch - area.h) / ch).ceil().max(0.0);
        let oy = area.y - overflow * ch;
        let content = term.renderable_content();
        let selection = content.selection;
        let colors = content.colors;
        let offset = content.display_offset as i32;
        let mode = content.mode;
        let cursor = content.cursor;
        let lines = term.screen_lines();

        let mut cells: Vec<(f32, f32, char, Option<String>, Rgba, Option<Rgba>, Flags)> = Vec::with_capacity(term.columns() * lines);
        for cell in content.display_iter {
            let row = cell.point.line.0 + offset;
            if row < 0 || row as usize >= lines {
                continue;
            }
            let x = ox + cell.point.column.0 as f32 * cw;
            let y = oy + row as f32 * ch;
            let mut fg = self.resolve(cell.fg, colors, false).unwrap_or(t.fg);
            let mut bg = self.resolve(cell.bg, colors, true);
            if cell.flags.contains(Flags::INVERSE) {
                let f = fg;
                fg = bg.unwrap_or(t.surface);
                bg = Some(f);
            }
            if cell.flags.contains(Flags::DIM) {
                fg = with_alpha(fg, 0.62);
            }
            if selection.is_some_and(|r| r.contains(cell.point)) {
                bg = Some(t.selection_bg);
            }
            let zw = cell.zerowidth().map(|z| {
                let mut s = String::from(cell.c);
                s.extend(z);
                s
            });
            cells.push((x, y, cell.c, zw, fg, bg, cell.flags));
        }
        drop(term);

        // Backgrounds first, merged into horizontal runs.
        let mut run: Option<(f32, f32, f32, Rgba)> = None;
        for &(x, y, _, _, _, bg, flags) in &cells {
            let w = if flags.contains(Flags::WIDE_CHAR) { 2.0 * cw } else { cw };
            match (bg, &mut run) {
                (Some(b), Some((rx, ry, rw, rc))) if *ry == y && *rc == b && (*rx + *rw - x).abs() < 0.5 => *rw += w,
                _ => {
                    if let Some((rx, ry, rw, rc)) = run.take() {
                        self.rect(q, Rect { x: rx, y: ry, w: rw, h: ch }, with_alpha(rc, alpha), 0.0, clip);
                    }
                    if let Some(b) = bg {
                        run = Some((x, y, w, b));
                    }
                }
            }
        }
        if let Some((rx, ry, rw, rc)) = run {
            self.rect(q, Rect { x: rx, y: ry, w: rw, h: ch }, with_alpha(rc, alpha), 0.0, clip);
        }

        // Cursor (block/beam/underline), springing between cells.
        let mut moving = false;
        let show = mode.contains(TermMode::SHOW_CURSOR) && cursor.shape != CursorShape::Hidden && offset == 0;
        let crow = cursor.point.line.0 + offset;
        let (cx, cy) = (area.x + cursor.point.column.0 as f32 * cw, area.y + crow as f32 * ch);
        let mut block_at = None;
        if primary {
            if self.cursor_pane != pane_id {
                self.cursor_pane = pane_id;
                self.cursor_x.snap(cx);
                self.cursor_y.snap(cy);
            }
            self.cursor_x.set(cx);
            self.cursor_y.set(cy);
            let dt = self.dt;
            moving = self.cursor_x.step(dt) | self.cursor_y.step(dt);
        }
        if show && primary {
            let (x, y) = (self.cursor_x.value.round() + dx, self.cursor_y.value.round());
            let col = if self.focused { t.cursor } else { with_alpha(t.cursor, 0.0) };
            match cursor.shape {
                CursorShape::Beam => self.rect(q, Rect { x, y, w: self.px(2.0), h: ch }, with_alpha(t.cursor, alpha), 0.0, clip),
                CursorShape::Underline => {
                    self.rect(q, Rect { x, y: y + ch - self.px(2.0), w: cw, h: self.px(2.0) }, with_alpha(t.cursor, alpha), 0.0, clip)
                }
                _ if !self.focused => q.push(Quad {
                    rect: [x, y, cw, ch],
                    color: col,
                    color2: with_alpha(t.cursor, 0.8 * alpha),
                    params: [kind::RECT, self.px(2.0), self.px(1.0).max(1.0), 0.0],
                    clip,
                    ..Default::default()
                }),
                _ => {
                    self.rect(q, Rect { x, y, w: cw, h: ch }, with_alpha(t.cursor, alpha), self.px(2.0), clip);
                    if !moving {
                        block_at = Some((cx + dx, cy));
                    }
                }
            }
        }

        // Glyphs and decorations.
        for (x, y, c, zw, fg, _bg, flags) in cells {
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN) {
                continue;
            }
            let fg = if block_at == Some((x, y)) { t.surface } else { fg };
            let fg = with_alpha(fg, alpha);
            if flags.intersects(Flags::ALL_UNDERLINES) {
                self.rect(q, Rect { x, y: y + ch - self.px(2.0), w: cw, h: self.px(1.0).max(1.0) }, fg, 0.0, clip);
            }
            if flags.contains(Flags::STRIKEOUT) {
                self.rect(q, Rect { x, y: y + (ch / 2.0).round(), w: cw, h: self.px(1.0).max(1.0) }, fg, 0.0, clip);
            }
            if c == ' ' || c == '\t' {
                continue;
            }
            if zw.is_none() && self.box_glyph(q, c, x, y, cw, ch, fg, clip) {
                continue;
            }
            let text = zw.unwrap_or_else(|| c.to_string());
            let bold = flags.contains(Flags::BOLD);
            let italic = flags.contains(Flags::ITALIC);
            let Some(p) = self.glyphs.cell(&text, bold, italic) else { continue };
            let Some(a) = self.glyphs.atlas(p.key) else { continue };
            q.push(Quad {
                rect: [(x + p.x + a.left as f32).round(), (y + p.y - a.top as f32).round(), a.w as f32, a.h as f32],
                uv: [a.ax as f32, a.ay as f32, 0.0, 0.0],
                color: fg,
                params: [if a.color { kind::COLOR_GLYPH } else { kind::GLYPH }, 0.0, 0.0, 0.0],
                clip,
                ..Default::default()
            });
        }
        // The Ctrl-hovered link, underlined in the accent.
        if let Some((_, link)) = self.hover.as_ref().filter(|(id, _)| *id == pane_id) {
            for pt in &link.cells {
                let row = pt.line.0 + offset;
                if row >= 0 && (row as usize) < lines {
                    let y = oy + row as f32 * ch + ch - self.px(2.0);
                    self.rect(q, Rect { x: ox + pt.column.0 as f32 * cw, y, w: cw, h: self.px(1.5).max(1.0) }, t.accent, 0.0, clip);
                }
            }
        }
        moving
    }

    /// Box drawing and block elements are drawn as geometry so they join
    /// seamlessly across cells at any line height.
    fn box_glyph(&self, q: &mut Vec<Quad>, c: char, x: f32, y: f32, w: f32, h: f32, col: Rgba, clip: [f32; 4]) -> bool {
        let lt = (h / 16.0).round().max(1.0);
        // Block elements.
        let block = |q: &mut Vec<Quad>, fx0: f32, fy0: f32, fx1: f32, fy1: f32, a: f32| {
            let x0 = (x + w * fx0).round();
            let x1 = (x + w * fx1).round();
            let y0 = (y + h * fy0).round();
            let y1 = (y + h * fy1).round();
            q.push(Quad {
                rect: [x0, y0, x1 - x0, y1 - y0],
                color: with_alpha(col, a),
                params: [kind::RECT, 0.0, 0.0, 0.0],
                clip,
                ..Default::default()
            });
        };
        match c {
            '█' => { block(q, 0.0, 0.0, 1.0, 1.0, 1.0); return true; }
            '▀' => { block(q, 0.0, 0.0, 1.0, 0.5, 1.0); return true; }
            '▄' => { block(q, 0.0, 0.5, 1.0, 1.0, 1.0); return true; }
            '▌' => { block(q, 0.0, 0.0, 0.5, 1.0, 1.0); return true; }
            '▐' => { block(q, 0.5, 0.0, 1.0, 1.0, 1.0); return true; }
            '░' => { block(q, 0.0, 0.0, 1.0, 1.0, 0.25); return true; }
            '▒' => { block(q, 0.0, 0.0, 1.0, 1.0, 0.5); return true; }
            '▓' => { block(q, 0.0, 0.0, 1.0, 1.0, 0.75); return true; }
            '▁'..='▇' => {
                let n = (c as u32 - '▁' as u32 + 1) as f32;
                block(q, 0.0, 1.0 - n / 8.0, 1.0, 1.0, 1.0);
                return true;
            }
            '▘' => { block(q, 0.0, 0.0, 0.5, 0.5, 1.0); return true; }
            '▝' => { block(q, 0.5, 0.0, 1.0, 0.5, 1.0); return true; }
            '▖' => { block(q, 0.0, 0.5, 0.5, 1.0, 1.0); return true; }
            '▗' => { block(q, 0.5, 0.5, 1.0, 1.0, 1.0); return true; }
            _ => {}
        }
        // Lines: (up, right, down, left) weights; 1 light, 2 heavy.
        let arms: [u8; 4] = match c {
            '─' => [0, 1, 0, 1],
            '━' => [0, 2, 0, 2],
            '│' => [1, 0, 1, 0],
            '┃' => [2, 0, 2, 0],
            '┌' => [0, 1, 1, 0],
            '┐' => [0, 0, 1, 1],
            '└' => [1, 1, 0, 0],
            '┘' => [1, 0, 0, 1],
            '├' => [1, 1, 1, 0],
            '┤' => [1, 0, 1, 1],
            '┬' => [0, 1, 1, 1],
            '┴' => [1, 1, 0, 1],
            '┼' => [1, 1, 1, 1],
            '┏' => [0, 2, 2, 0],
            '┓' => [0, 0, 2, 2],
            '┗' => [2, 2, 0, 0],
            '┛' => [2, 0, 0, 2],
            '╴' => [0, 0, 0, 1],
            '╵' => [1, 0, 0, 0],
            '╶' => [0, 1, 0, 0],
            '╷' => [0, 0, 1, 0],
            '╭' | '╮' | '╯' | '╰' => {
                let (sx, sy) = match c {
                    '╭' => (1.0, 1.0),
                    '╮' => (-1.0, 1.0),
                    '╯' => (-1.0, -1.0),
                    _ => (1.0, -1.0),
                };
                let t = lt;
                let lcx = (x + w / 2.0 - t / 2.0).round() + t / 2.0;
                let lcy = (y + h / 2.0 - t / 2.0).round() + t / 2.0;
                let r = (w / 2.0).min(h / 2.0);
                let (ccx, ccy) = (lcx + sx * r, lcy + sy * r);
                let x0 = ccx.min(ccx - sx * (r + t));
                let y0 = ccy.min(ccy - sy * (r + t));
                q.push(Quad {
                    rect: [x0, y0, r + t, r + t],
                    uv: [ccx - x0, ccy - y0, r, 0.0],
                    color: col,
                    params: [kind::ARC, 0.0, t, 0.0],
                    clip,
                    ..Default::default()
                });
                // Straight runs from the arc's ends to the cell edges.
                let hx0 = if sx > 0.0 { ccx } else { x };
                let hx1 = if sx > 0.0 { x + w } else { ccx };
                if hx1 > hx0 {
                    q.push(Quad { rect: [hx0, lcy - t / 2.0, hx1 - hx0, t], color: col, clip, ..Default::default() });
                }
                let vy0 = if sy > 0.0 { ccy } else { y };
                let vy1 = if sy > 0.0 { y + h } else { ccy };
                if vy1 > vy0 {
                    q.push(Quad { rect: [lcx - t / 2.0, vy0, t, vy1 - vy0], color: col, clip, ..Default::default() });
                }
                return true;
            }
            _ => return false,
        };
        let cxl = (x + w / 2.0 - lt / 2.0).round();
        let cyl = (y + h / 2.0 - lt / 2.0).round();
        let thick = |wgt: u8| if wgt == 2 { lt * 2.0 } else { lt };
        let maxv = arms[0].max(arms[2]);
        let maxh = arms[1].max(arms[3]);
        let mut push = |rx: f32, ry: f32, rw: f32, rh: f32| {
            q.push(Quad { rect: [rx, ry, rw, rh], color: col, clip, ..Default::default() });
        };
        // Vertical arms overlap the horizontal stroke so joins are solid.
        let hv = thick(maxh);
        let vv = thick(maxv);
        if arms[0] > 0 {
            let t = thick(arms[0]);
            push(cxl + ((lt - t) / 2.0).floor(), y, t, cyl - y + hv);
        }
        if arms[2] > 0 {
            let t = thick(arms[2]);
            push(cxl + ((lt - t) / 2.0).floor(), cyl, t, y + h - cyl);
        }
        if arms[3] > 0 {
            let t = thick(arms[3]);
            push(x, cyl + ((lt - t) / 2.0).floor(), cxl - x + vv, t);
        }
        if arms[1] > 0 {
            let t = thick(arms[1]);
            push(cxl, cyl + ((lt - t) / 2.0).floor(), x + w - cxl, t);
        }
        true
    }
}

fn tmux_installed() -> bool {
    std::process::Command::new("tmux").arg("-V").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

/// tmux's default pane title is the host name; that is no title at all.
fn is_hostname_like(t: &str) -> bool {
    !t.contains(' ') && (t.starts_with("ip-") || t.contains(".internal") || t.contains(".local"))
}

fn rgb_to(c: Rgb) -> Rgba {
    [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, 1.0]
}

/// Claude Code (and others) prefix the title with a spinner glyph.
/// Text as compared for timeline jumps: markdown marks dropped, spaces
/// collapsed (a transcript holds `**bold**`, the screen shows it bold).
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").replace(['*', '`'], "")
}

/// The narrowest a pane beside a cc-rail rail may be left.
const MIN_COLS: usize = 20;

fn clean_title(t: &str) -> String {
    t.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '~' && c != '/' && c != '.').trim().to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(n - 1).collect();
        o.push('…');
        o
    }
}

fn ago(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}
