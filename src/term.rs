//! A pane: one PTY + one terminal state machine (alacritty_terminal).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty::{self, Options, Shell};
use alacritty_terminal::vte::ansi::Processor;

use crate::tmuxcc::Writer;

/// Lines of history kept per pane.
pub const SCROLLBACK: usize = 50_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub cols: usize,
    pub lines: usize,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

impl Size {
    fn window(&self) -> WindowSize {
        WindowSize {
            num_lines: self.lines as u16,
            num_cols: self.cols as u16,
            cell_width: self.cell_w,
            cell_height: self.cell_h,
        }
    }
}

/// Terminal events are funnelled to the UI thread tagged with their pane id;
/// `wake` nudges the UI loop so it drains them promptly.
#[derive(Clone)]
pub struct Listener {
    id: u64,
    tx: Sender<(u64, Event)>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl EventListener for Listener {
    fn send_event(&self, ev: Event) {
        let _ = self.tx.send((self.id, ev));
        (self.wake)();
    }
}

/// Where a pane's bytes come from: our own PTY, or a pane in the user's
/// tmux reached through a control-mode connection.
enum Backend {
    Pty(EventLoopSender),
    Tmux { writer: Arc<Writer>, pane: String, parser: Processor },
}

pub struct Pane {
    pub id: u64,
    pub term: Arc<FairMutex<Term<Listener>>>,
    backend: Backend,
    /// The control connection and tmux pane id, for tmux panes.
    pub conn: Option<u64>,
    pub tmux_pane: Option<String>,
    /// Output is dropped until the history capture lands (it already contains it).
    pub awaiting_capture: bool,
    pub tmux_cursor: (usize, usize),
    pub tmux_alt: bool,
    /// The tmux window this pane is in, where in it (cells), and whether
    /// that window is zoomed onto one pane.
    pub window: Option<String>,
    pub pos: (usize, usize),
    pub zoomed: bool,
    pub size: Size,
    pub title: String,
    pub subtitle: String,
    pub exited: bool,
    /// Index into the app's host list.
    pub host: usize,
    /// The tmux session backing this pane, if it persists.
    pub session: Option<String>,
}

pub struct Spawn {
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
}

impl Pane {
    pub fn spawn(
        id: u64,
        spec: Spawn,
        size: Size,
        tx: Sender<(u64, Event)>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<Pane> {
        let listener = Listener { id, tx, wake };
        let config = Config { scrolling_history: SCROLLBACK, ..Default::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let mut env = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "punkterm".into());
        let subtitle = spec.program.clone().unwrap_or_else(|| {
            std::env::var("SHELL").ok().and_then(|s| s.rsplit('/').next().map(str::to_string)).unwrap_or("sh".into())
        });
        let opts = Options {
            shell: spec.program.map(|p| Shell::new(p, spec.args)),
            working_directory: spec.cwd,
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&opts, size.window(), id)?;
        let ev = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = ev.channel();
        ev.spawn();
        Ok(Pane {
            id,
            term,
            backend: Backend::Pty(sender),
            conn: None,
            tmux_pane: None,
            awaiting_capture: false,
            tmux_cursor: (0, 0),
            tmux_alt: false,
            window: None,
            pos: (0, 0),
            zoomed: false,
            size,
            title: String::new(),
            subtitle: subtitle.rsplit('/').next().unwrap_or("").to_string(),
            exited: false,
            host: 0,
            session: None,
        })
    }

    /// A view of a pane in the user's tmux. tmux owns its size.
    #[allow(clippy::too_many_arguments)]
    pub fn tmux(
        id: u64,
        conn: u64,
        writer: Arc<Writer>,
        pane: &str,
        size: Size,
        tx: Sender<(u64, Event)>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Pane {
        let listener = Listener { id, tx, wake };
        let config = Config { scrolling_history: SCROLLBACK, ..Default::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener)));
        Pane {
            id,
            term,
            backend: Backend::Tmux { writer, pane: pane.to_string(), parser: Processor::new() },
            conn: Some(conn),
            tmux_pane: Some(pane.to_string()),
            awaiting_capture: true,
            tmux_cursor: (0, 0),
            tmux_alt: false,
            window: None,
            pos: (0, 0),
            zoomed: false,
            size,
            title: String::new(),
            subtitle: String::new(),
            exited: false,
            host: 0,
            session: None,
        }
    }

    pub fn is_tmux(&self) -> bool {
        matches!(self.backend, Backend::Tmux { .. })
    }

    /// Feed bytes from tmux into the terminal state.
    pub fn feed(&mut self, bytes: &[u8]) {
        if let Backend::Tmux { parser, .. } = &mut self.backend {
            parser.advance(&mut *self.term.lock(), bytes);
        }
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let b = bytes.into();
        if b.is_empty() {
            return;
        }
        match &self.backend {
            Backend::Pty(sender) => {
                let _ = sender.send(Msg::Input(b));
            }
            Backend::Tmux { writer, pane, .. } => writer.send_bytes(pane, &b),
        }
    }

    /// Fit a PTY pane to the panel. tmux panes keep tmux's size.
    pub fn resize(&mut self, size: Size) {
        if let Backend::Pty(sender) = &self.backend {
            if size == self.size {
                return;
            }
            self.size = size;
            self.term.lock().resize(size);
            let _ = sender.send(Msg::Resize(size.window()));
        }
    }

    /// Adopt the size tmux reports for this pane.
    pub fn set_tmux_size(&mut self, cols: usize, lines: usize) {
        // tmux can squeeze a pane to one column; alacritty's grid panics
        // writing to one.
        let size = Size { cols: cols.max(2), lines: lines.max(1), ..self.size };
        if size != self.size {
            self.size = size;
            self.term.lock().resize(size);
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let Backend::Pty(sender) = &self.backend {
            let _ = sender.send(Msg::Shutdown);
        }
    }
}
