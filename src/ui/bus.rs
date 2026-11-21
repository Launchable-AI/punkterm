//! The BUS view (Ctrl+Shift+J): the jotbus-local traffic between agents on
//! a host, read by `crate::comms`. Who is on the bus (and the session each
//! runs in, to jump to it), its channels, and the messages back and forth.
//! Drawn by `bus_layer` in chrome.rs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use super::{App, HostState, Rect};
use crate::comms::Bus;

/// How often an open view reads the bus again.
const POLL: Duration = Duration::from_millis(2500);

/// Something clickable in the view, placed by `bus_layer`.
#[derive(Clone, Debug)]
pub enum BusHit {
    /// A host's tab.
    Host(usize),
    /// An agent's row: the pane index of its session, when punkterm has it.
    Agent(Option<usize>),
    /// A channel's row: show only it (again: all channels).
    Channel(String),
}

pub struct BusView {
    pub open: bool,
    pub t: f32,
    /// Each host's bus, as last read.
    pub data: HashMap<usize, Bus>,
    tx: Sender<(usize, u64, Option<Bus>)>,
    rx: Receiver<(usize, u64, Option<Bus>)>,
    /// Bumped on every open and close: readers from an older opening stop.
    epoch: Arc<AtomicU64>,
    /// The host shown; `None` picks one (the active session's, if it has a bus).
    pub host: Option<usize>,
    /// The channel the feed is narrowed to.
    pub filter: Option<String>,
    /// How far the feed is scrolled up from its newest message (px), and
    /// the most it can be.
    pub scroll: f32,
    pub scroll_max: f32,
    /// The view's panel and its clickable parts, from the last frame.
    pub panel: Rect,
    pub hits: Vec<(Rect, BusHit)>,
}

impl Default for BusView {
    fn default() -> Self {
        let (tx, rx) = channel();
        BusView {
            open: false,
            t: 0.0,
            data: HashMap::new(),
            tx,
            rx,
            epoch: Arc::new(AtomicU64::new(0)),
            host: None,
            filter: None,
            scroll: 0.0,
            scroll_max: 0.0,
            panel: Rect::default(),
            hits: Vec::new(),
        }
    }
}

impl App {
    /// Open or close the BUS view. While open, each connected host's bus is
    /// read every few seconds.
    pub fn toggle_bus(&mut self) {
        self.bus.open = !self.bus.open;
        let epoch = self.bus.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.sound.play(crate::sound::Sfx::Blip);
        if !self.bus.open {
            return;
        }
        self.bus.scroll = 0.0;
        for (h, host) in self.hosts.iter().enumerate() {
            if h != 0 && self.host_state.get(h) != Some(&HostState::Up) {
                continue;
            }
            let (ssh, tx, wake, live) = (host.ssh.clone(), self.bus.tx.clone(), self.wake.clone(), self.bus.epoch.clone());
            std::thread::spawn(move || {
                while live.load(Ordering::SeqCst) == epoch {
                    let bus = crate::comms::fetch(ssh.as_deref());
                    if tx.send((h, epoch, bus)).is_err() {
                        return;
                    }
                    wake();
                    std::thread::sleep(POLL);
                }
            });
        }
    }

    /// Take in the buses read since the last frame. True when one changed.
    pub(super) fn bus_pump(&mut self) -> bool {
        let mut dirty = false;
        let epoch = self.bus.epoch.load(Ordering::SeqCst);
        while let Ok((h, g, bus)) = self.bus.rx.try_recv() {
            if g != epoch {
                continue;
            }
            match bus {
                Some(b) if self.bus.data.get(&h) != Some(&b) => {
                    self.bus.data.insert(h, b);
                    dirty = true;
                }
                Some(_) => {}
                None => dirty |= self.bus.data.remove(&h).is_some(),
            }
        }
        dirty
    }

    /// The hosts with a bus, in host order.
    pub(super) fn bus_hosts(&self) -> Vec<usize> {
        let mut hs: Vec<usize> = self.bus.data.iter().filter(|(_, b)| b.present).map(|(&h, _)| h).collect();
        hs.sort();
        hs
    }

    /// The host the view shows: the one picked, else the active session's
    /// if it has a bus, else the first that does.
    pub(super) fn bus_shown(&self) -> Option<usize> {
        let hs = self.bus_hosts();
        self.bus
            .host
            .filter(|h| hs.contains(h))
            .or_else(|| Some(self.active_host()).filter(|h| hs.contains(h)))
            .or_else(|| hs.first().copied())
    }

    /// Tab: the next host with a bus.
    pub fn bus_next_host(&mut self) {
        let hs = self.bus_hosts();
        if let Some(cur) = self.bus_shown() {
            let k = hs.iter().position(|&h| h == cur).unwrap_or(0);
            self.bus.host = hs.get((k + 1) % hs.len().max(1)).copied();
            self.bus.filter = None;
            self.bus.scroll = 0.0;
        }
    }

    /// The punkterm session (pane index) an agent's tmux pane is shown in.
    pub(super) fn bus_pane(&self, host: usize, tmux_pane: Option<&str>) -> Option<usize> {
        let tp = tmux_pane?;
        self.panes.iter().position(|p| p.host == host && p.tmux_pane.as_deref() == Some(tp))
    }

    /// Scroll the feed by `lines` (wheel steps; up is positive: older).
    pub fn bus_scroll(&mut self, lines: f32) {
        let step = self.px(18.0);
        self.bus.scroll = (self.bus.scroll + lines * step).clamp(0.0, self.bus.scroll_max);
    }

    /// A click while the view is open: a host tab, an agent (jump to its
    /// session), a channel (narrow the feed to it); outside the panel closes.
    pub fn bus_click(&mut self, x: f32, y: f32) {
        let inside = |r: &Rect| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
        let hit = self.bus.hits.iter().find(|(r, _)| inside(r)).map(|(_, h)| h.clone());
        match hit {
            Some(BusHit::Host(h)) => {
                if self.bus_shown() != Some(h) {
                    self.bus.host = Some(h);
                    self.bus.filter = None;
                    self.bus.scroll = 0.0;
                }
            }
            Some(BusHit::Agent(Some(i))) => {
                self.toggle_bus();
                if self.overview {
                    self.toggle_overview();
                }
                self.select(i);
            }
            Some(BusHit::Agent(None)) => self.toast("BUS // NOT IN A SESSION PUNKTERM SHOWS"),
            Some(BusHit::Channel(c)) => {
                self.bus.filter = if self.bus.filter.as_deref() == Some(&c) { None } else { Some(c) };
                self.bus.scroll = 0.0;
            }
            None if !inside(&self.bus.panel) => self.toggle_bus(),
            None => {}
        }
    }

    /// Whether (x, y) is over the view's panel (for the wheel).
    pub fn bus_at(&self, x: f32, y: f32) -> bool {
        let r = self.bus.panel;
        self.bus.open && x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
    }
}
