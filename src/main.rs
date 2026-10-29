mod anim;
mod claude;
mod clipboard;
mod gfx;
mod glyphs;
mod resume;
mod graphics;
mod hosts;
mod input;
mod links;
mod term;
mod sound;
mod state;
mod sysstat;
mod timeline;
mod theme;
mod tmuxcc;
mod usage;
mod ui;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, StartCause, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{CursorIcon, Window, WindowId};

use crate::term::Spawn;
use crate::theme::Theme;
use crate::ui::App;

struct Args {
    theme: Option<String>,
    snapshot: Option<String>,
    size: (u32, u32),
    scale: f32,
    cmds: Vec<String>,
    attach: bool,
    select: Option<usize>,
    scroll: f32,
    typed: Option<String>,
    show: Option<String>,
    drag: Option<String>,
    hosts: Vec<String>,
    /// Snapshots never resize tmux windows unless asked (for tests against
    /// a throwaway tmux server).
    resize_tmux: bool,
}

fn parse_args() -> Args {
    let mut a = Args { theme: None, snapshot: None, size: (1600, 960), scale: 1.0, cmds: Vec::new(), attach: false, select: None, scroll: 0.0, typed: None, show: None, drag: None, hosts: Vec::new(), resize_tmux: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--theme" => a.theme = it.next(),
            "--config" => {
                if let Some(p) = it.next() {
                    // SAFETY: set before any other thread starts.
                    unsafe { std::env::set_var("PUNKTERM_CONFIG", p) };
                }
            }
            "--snapshot" => a.snapshot = it.next(),
            "--size" => {
                if let Some((w, h)) = it.next().as_deref().and_then(|s| s.split_once('x')) {
                    a.size = (w.parse().unwrap_or(1600), h.parse().unwrap_or(960));
                }
            }
            "--attach" => a.attach = true,
            "--select" => a.select = it.next().and_then(|s| s.parse().ok()),
            "--scroll" => a.scroll = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0),
            "--type" => a.typed = it.next(),
            "--show" => a.show = it.next(),
            "--drag" => a.drag = it.next(),
            "--scale" => a.scale = it.next().and_then(|s| s.parse().ok()).unwrap_or(1.0),
            "-e" | "--cmd" => a.cmds.extend(it.next()),
            "--host" => a.hosts.extend(it.next()),
            "--resize-tmux" => a.resize_tmux = true,
            "-h" | "--help" => {
                println!(
                    "punkterm [--theme NAME] [-e CMD]...\n\n  \
                     --theme NAME     a themer theme (~/.config/themer/themes/NAME.toml, read-only)\n  \
                     --config PATH    hosts config (default ~/.config/punkterm/config.toml)\n  \
                     --host SSH       also join tmux on this host (repeatable), e.g.\n                   \
                       --host \"-i ~/.ssh/key.pem ubuntu@203.0.113.7\"\n  \
                     -e, --cmd CMD    open a session running CMD (repeatable; default: $SHELL)\n  \
                     --snapshot PNG   render headlessly to PNG and exit\n  \
                     --size WxH       snapshot size (default 1600x960)\n  \
                     --scale F        snapshot scale factor"
                );
                std::process::exit(0);
            }
            _ => eprintln!("punkterm: unknown argument {arg}"),
        }
    }
    a
}

fn spawn_spec(cmd: Option<&str>, keep_alive: bool) -> Spawn {
    match cmd {
        None => Spawn { program: None, args: vec![], cwd: std::env::current_dir().ok() },
        Some(c) => {
            let shell = std::env::var("SHELL").unwrap_or("/bin/sh".into());
            let c = if keep_alive { format!("{c}; sleep 600") } else { c.to_string() };
            Spawn { program: Some(shell), args: vec!["-c".into(), c], cwd: std::env::current_dir().ok() }
        }
    }
}

/// With Ctrl held over a link: underline it and show a hand.
fn hover_link(app: &mut App, gpu: &Gpu, mouse: (f64, f64), mods: ModifiersState) {
    if app.hover_link(mouse.0 as f32, mouse.1 as f32, mods.control_key()) {
        gpu.window.set_cursor(if app.hovering_link() { CursorIcon::Pointer } else { CursorIcon::Default });
        gpu.window.request_redraw();
    }
}

/// Paste text into the active session, bracketed when the program asked.
fn paste_into(app: &mut App, text: Option<String>) {
    if let Some(t) = text {
        let i = app.active;
        app.paste_text(i, &t);
    }
}

fn main() {
    let args = parse_args();
    let prefs = state::load();
    let theme = Theme::load(args.theme.as_deref().or(prefs.theme.as_deref()));
    if let Some(path) = args.snapshot.clone() {
        snapshot(theme, &args, &path);
        return;
    }
    let el = EventLoop::<()>::with_user_event().build().expect("event loop");
    el.set_control_flow(ControlFlow::Wait);
    let proxy = el.create_proxy();
    let mut win = Win { theme: Some(theme), theme_flag: args.theme.is_some(), cmds: args.cmds, hosts: args.hosts, proxy, state: None, mods: ModifiersState::empty(), mouse: (0.0, 0.0) };
    el.run_app(&mut win).expect("run");
}

// ---------------------------------------------------------------- windowed

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: gfx::Renderer,
}

struct Win {
    theme: Option<Theme>,
    theme_flag: bool,
    cmds: Vec<String>,
    hosts: Vec<String>,
    proxy: EventLoopProxy<()>,
    state: Option<(Gpu, App)>,
    mods: ModifiersState,
    mouse: (f64, f64),
}

impl ApplicationHandler for Win {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("punkterm")
            .with_name("punkterm", "punkterm")
            .with_inner_size(LogicalSize::new(1440.0, 900.0))
            .with_transparent(true);
        let window = Arc::new(el.create_window(attrs).expect("window"));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())));
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("device");
        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("surface config");
        // Blend in the same space the palette's hex values are written in:
        // a plain 8-bit UNORM surface. Not merely "any non-sRGB" format: a
        // float (scRGB/HDR) surface is read as linear light by the
        // compositor, which lifts every dark colour to a washed-out gray.
        let caps = surface.get_capabilities(&adapter);
        let preferred = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Rgb10a2Unorm];
        if let Some(f) = preferred.iter().find(|f| caps.formats.contains(f)) {
            config.format = *f;
        }
        if std::env::var_os("PUNKTERM_DEBUG").is_some() {
            eprintln!("punkterm: surface formats {:?}, using {:?}", caps.formats, config.format);
        }
        config.present_mode = wgpu::PresentMode::Fifo;
        // Transparency needs a premultiplied surface; without one, stay opaque.
        let alpha_ok = caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied);
        if alpha_ok {
            config.alpha_mode = wgpu::CompositeAlphaMode::PreMultiplied;
        }
        surface.configure(&device, &config);
        let renderer = gfx::Renderer::new(device, queue, config.format);

        let woke = Arc::new(AtomicBool::new(false));
        let proxy = self.proxy.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if !woke.swap(true, Ordering::AcqRel) {
                let _ = proxy.send_event(());
            }
            woke.store(false, Ordering::Release);
        });
        let mut app = App::new(self.theme.take().unwrap(), window.scale_factor() as f32, (size.width, size.height), wake);
        app.alpha_ok = alpha_ok;
        let prefs = state::load();
        app.apply_prefs(&prefs, self.theme_flag);
        if !alpha_ok {
            app.opacity = 1.0;
        }
        if self.cmds.is_empty() {
            app.start(hosts::load(&[prefs.hosts, std::mem::take(&mut self.hosts)].concat()));
        }
        for c in std::mem::take(&mut self.cmds) {
            app.spawn(spawn_spec(Some(&c), false));
        }
        window.request_redraw();
        self.state = Some((Gpu { window, surface, config, renderer }, app));
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        // The HUD clock ticks once a second even when nothing else moves.
        if let (StartCause::ResumeTimeReached { .. }, Some((gpu, _))) = (cause, &self.state) {
            gpu.window.request_redraw();
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, _: ()) {
        if let Some((gpu, app)) = &mut self.state {
            if app.pump() {
                gpu.window.request_redraw();
            }
            if app.quit {
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some((gpu, app)) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                gpu.config.width = s.width.max(1);
                gpu.config.height = s.height.max(1);
                gpu.surface.configure(&gpu.renderer.device, &gpu.config);
                app.resize((gpu.config.width, gpu.config.height), gpu.window.scale_factor() as f32);
                if std::env::var_os("PUNKTERM_DEBUG").is_some() {
                    eprintln!("punkterm: resized {}x{} scale {} grid {:?}", s.width, s.height, gpu.window.scale_factor(), app.grid_size());
                }
                gpu.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                app.resize(app.size, scale_factor as f32);
                gpu.window.request_redraw();
            }
            WindowEvent::Focused(f) => {
                app.focused = f;
                if let Some(p) = app.active_pane() {
                    if p.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
                        p.write(if f { b"\x1b[I".as_slice() } else { b"\x1b[O".as_slice() });
                    }
                }
                gpu.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                hover_link(app, gpu, self.mouse, self.mods);
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } if event.state == ElementState::Pressed => {
                let m = self.mods;
                let code = match event.physical_key {
                    PhysicalKey::Code(c) => Some(c),
                    _ => None,
                };
                // Overlays take the keyboard while they are open.
                if app.menu.is_some() {
                    match code {
                        Some(KeyCode::ArrowDown) => app.menu_step(1),
                        Some(KeyCode::ArrowUp) => app.menu_step(-1),
                        Some(KeyCode::Enter | KeyCode::NumpadEnter) => app.menu_enter(),
                        Some(KeyCode::ShiftLeft | KeyCode::ShiftRight | KeyCode::ControlLeft | KeyCode::ControlRight) => {}
                        _ => app.menu = None,
                    }
                    gpu.window.request_redraw();
                    return;
                }
                if app.prompt.is_some() {
                    use ui::PromptKey as K;
                    let k = match code {
                        Some(KeyCode::Escape) => Some(K::Esc),
                        Some(KeyCode::Enter | KeyCode::NumpadEnter) => Some(K::Enter),
                        Some(KeyCode::Backspace) => Some(K::Back),
                        Some(KeyCode::ArrowUp) => Some(K::Up),
                        Some(KeyCode::ArrowDown) => Some(K::Down),
                        Some(KeyCode::Tab) => Some(K::Tab),
                        Some(KeyCode::KeyV) if m.control_key() => clipboard::paste().map(|t| K::Char(t.lines().next().unwrap_or("").to_string())),
                        _ if !m.control_key() && !m.alt_key() => event.text.as_ref().filter(|t| !t.chars().any(char::is_control)).map(|t| K::Char(t.to_string())),
                        _ => None,
                    };
                    if let Some(k) = k {
                        app.prompt_key(k);
                    }
                    gpu.window.request_redraw();
                    return;
                }
                if app.help {
                    if matches!(code, Some(KeyCode::Escape | KeyCode::Slash | KeyCode::Enter | KeyCode::KeyQ)) {
                        app.toggle_help();
                    }
                    gpu.window.request_redraw();
                    return;
                }
                if app.resume.is_some() {
                    match code {
                        Some(KeyCode::ArrowDown | KeyCode::KeyJ) => app.resume_step(1),
                        Some(KeyCode::ArrowUp | KeyCode::KeyK) => app.resume_step(-1),
                        Some(KeyCode::PageDown) => app.resume_step(8),
                        Some(KeyCode::PageUp) => app.resume_step(-8),
                        Some(KeyCode::Enter | KeyCode::NumpadEnter) => app.resume_pick(),
                        Some(KeyCode::Escape | KeyCode::KeyQ) => app.resume = None,
                        _ => {}
                    }
                    gpu.window.request_redraw();
                    return;
                }
                if app.usage_open {
                    if matches!(code, Some(KeyCode::Escape | KeyCode::KeyU | KeyCode::Enter | KeyCode::KeyQ)) {
                        app.toggle_usage();
                    }
                    gpu.window.request_redraw();
                    return;
                }
                if app.overview && !(m.control_key() && m.shift_key() && code != Some(KeyCode::KeyD)) {
                    match code {
                        Some(KeyCode::Escape) | Some(KeyCode::KeyD) if code == Some(KeyCode::Escape) || (m.control_key() && m.shift_key()) => {
                            app.toggle_overview()
                        }
                        Some(KeyCode::ArrowLeft) => app.overview_move(-1, 0),
                        Some(KeyCode::ArrowRight) => app.overview_move(1, 0),
                        Some(KeyCode::ArrowUp) => app.overview_move(0, -1),
                        Some(KeyCode::ArrowDown) => app.overview_move(0, 1),
                        Some(KeyCode::Enter) | Some(KeyCode::NumpadEnter) => app.overview_open(app.overview_sel),
                        _ => {}
                    }
                    gpu.window.request_redraw();
                    return;
                }
                // The second digit of a session number begun with the leader.
                if let Some((d, t)) = app.digits.take().filter(|(_, t)| t.elapsed().as_millis() < 1000) {
                    let digit = match &event.logical_key {
                        Key::Character(s) => s.chars().next().and_then(|c| c.to_digit(10)),
                        _ => None,
                    };
                    if let Some(e) = digit.filter(|_| !m.control_key() && !m.alt_key()) {
                        app.select_nth(d * 10 + e as usize);
                        gpu.window.request_redraw();
                        return;
                    }
                    if matches!(code, Some(KeyCode::ShiftLeft | KeyCode::ShiftRight)) {
                        app.digits = Some((d, t));
                    }
                }
                // tmux-style leader: Ctrl+Space, then a key within 3 s.
                if let Some(t) = app.leader.take().filter(|t| t.elapsed().as_secs() < 3) {
                    if matches!(code, Some(KeyCode::ShiftLeft | KeyCode::ShiftRight | KeyCode::ControlLeft | KeyCode::ControlRight | KeyCode::AltLeft | KeyCode::AltRight)) {
                        app.leader = Some(t);
                        return;
                    }
                    let ch = match &event.logical_key {
                        Key::Character(s) => s.chars().next(),
                        _ => None,
                    };
                    match (ch, code) {
                        (Some('v' | '%'), _) => app.split(true),
                        (Some('s' | '"'), _) => app.split(false),
                        (Some('z'), _) => app.toggle_zoom(),
                        (Some('m'), _) => app.mark(),
                        (Some('g'), _) => app.group_picked(),
                        (Some('G'), _) => app.ungroup_picked(),
                        (Some(','), _) => app.rename_group(None),
                        // Session by number; a second digit within a second
                        // makes it two-digit (⌃␣ 1 2 is session 12).
                        (Some(d @ '1'..='9'), _) => {
                            let d = d as usize - '0' as usize;
                            app.select_nth(d);
                            app.digits = Some((d, std::time::Instant::now()));
                        }
                        (Some('J'), _) => app.join_marked(),
                        (Some('!'), _) => app.break_out(),
                        (Some('c'), _) => app.new_session(app.active_host()),
                        (Some('n'), _) => app.cycle(1),
                        (Some('p'), _) => app.cycle(-1),
                        (Some('h'), _) | (_, Some(KeyCode::ArrowLeft)) => {
                            app.focus_dir(-1, 0);
                        }
                        (Some('j'), _) | (_, Some(KeyCode::ArrowDown)) => {
                            app.focus_dir(0, 1);
                        }
                        (Some('k'), _) | (_, Some(KeyCode::ArrowUp)) => {
                            app.focus_dir(0, -1);
                        }
                        (Some('l'), _) | (_, Some(KeyCode::ArrowRight)) => {
                            app.focus_dir(1, 0);
                        }
                        // Twice: a literal Ctrl+Space for the program.
                        (_, Some(KeyCode::Space)) if m.control_key() => {
                            if let Some(p) = app.active_pane() {
                                p.write(vec![0u8]);
                            }
                        }
                        _ => {}
                    }
                    gpu.window.request_redraw();
                    return;
                }
                if m.control_key() && !m.shift_key() && !m.alt_key() && code == Some(KeyCode::Space) {
                    app.leader = Some(std::time::Instant::now());
                    gpu.window.request_redraw();
                    return;
                }
                // Alt+hjkl moves between split panes; at an edge, or with no
                // split, the key goes to the program as usual.
                if m.alt_key() && !m.control_key() && !m.shift_key() {
                    let dir = match code {
                        Some(KeyCode::KeyH) => Some((-1, 0)),
                        Some(KeyCode::KeyJ) => Some((0, 1)),
                        Some(KeyCode::KeyK) => Some((0, -1)),
                        Some(KeyCode::KeyL) => Some((1, 0)),
                        _ => None,
                    };
                    if let Some((dx, dy)) = dir {
                        if app.focus_dir(dx, dy) {
                            gpu.window.request_redraw();
                            return;
                        }
                    }
                }
                // Ctrl+] / Ctrl+[ step through sessions, as in kitty.
                if m.control_key() && !m.shift_key() && !m.alt_key() && matches!(code, Some(KeyCode::BracketRight | KeyCode::BracketLeft)) {
                    app.cycle(if code == Some(KeyCode::BracketRight) { 1 } else { -1 });
                    gpu.window.request_redraw();
                    return;
                }
                // Shift+Insert pastes the clipboard (Ctrl+Shift+V splits);
                // middle-click pastes the primary selection.
                if m.shift_key() && !m.control_key() && !m.alt_key() && code == Some(KeyCode::Insert) {
                    // An image (no text): pasted as a file path, as with Ctrl+V.
                    let text = clipboard::paste().filter(|t| !t.is_empty());
                    if text.is_some() || !app.paste_image() {
                        paste_into(app, text);
                    }
                    gpu.window.request_redraw();
                    return;
                }
                // Text size: Ctrl(+Shift) with = / + / - / 0, as in kitty and browsers.
                if m.control_key() && !m.alt_key() {
                    let px = match code {
                        Some(KeyCode::Equal | KeyCode::NumpadAdd) => Some(app.mono_px + 1.0),
                        Some(KeyCode::Minus | KeyCode::NumpadSubtract) => Some(app.mono_px - 1.0),
                        Some(KeyCode::Digit0 | KeyCode::Numpad0) => Some(Theme::load(None).mono_px),
                        _ => None,
                    };
                    if let Some(px) = px {
                        app.set_font_px(px);
                        gpu.window.request_redraw();
                        return;
                    }
                }
                // punkterm's own bindings: Ctrl+Shift for app actions, Ctrl+Tab to cycle.
                if m.control_key() && m.shift_key() {
                    let handled = match code {
                        Some(KeyCode::KeyT) => {
                            app.new_session(app.active_host());
                            true
                        }
                        Some(KeyCode::KeyP) => {
                            app.cycle_theme(1);
                            true
                        }
                        Some(KeyCode::KeyO) => {
                            app.cycle_opacity();
                            true
                        }
                        Some(KeyCode::KeyS) => {
                            app.cycle_sound();
                            true
                        }
                        Some(KeyCode::KeyD) => {
                            app.toggle_overview();
                            true
                        }
                        Some(KeyCode::KeyE) => {
                            app.split(true);
                            true
                        }
                        Some(KeyCode::KeyN) => {
                            app.open_prompt();
                            true
                        }
                        Some(KeyCode::KeyR) => {
                            app.reconnect();
                            true
                        }
                        Some(KeyCode::Slash) => {
                            app.toggle_help();
                            true
                        }
                        Some(KeyCode::KeyH) => {
                            app.toggle_timeline();
                            true
                        }
                        Some(KeyCode::KeyU) => {
                            app.toggle_usage();
                            true
                        }
                        Some(KeyCode::KeyW) => {
                            app.close_active();
                            true
                        }
                        Some(KeyCode::KeyC) => {
                            if let Some(t) = app.selected_text() {
                                clipboard::copy(&t);
                                app.toast("COPIED");
                            }
                            true
                        }
                        // Splits, as the leader's v and ": side by side, stacked.
                        Some(KeyCode::KeyV) => {
                            app.split(true);
                            true
                        }
                        Some(KeyCode::Quote) => {
                            app.split(false);
                            true
                        }
                        Some(c @ (KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 | KeyCode::Digit5
                        | KeyCode::Digit6 | KeyCode::Digit7 | KeyCode::Digit8 | KeyCode::Digit9)) => {
                            app.select_nth(c as usize - KeyCode::Digit1 as usize + 1);
                            true
                        }
                        Some(KeyCode::Tab) => {
                            app.cycle(-1);
                            true
                        }
                        Some(KeyCode::Space) => {
                            app.jump_attention();
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        gpu.window.request_redraw();
                        return;
                    }
                }
                if m.control_key() && !m.shift_key() && code == Some(KeyCode::Tab) {
                    app.cycle(1);
                    gpu.window.request_redraw();
                    return;
                }
                // Ctrl+V in Claude on a remote host: Claude would read that
                // host's clipboard, so bring this one's image over instead.
                if m.control_key() && !m.shift_key() && !m.alt_key() && code == Some(KeyCode::KeyV) && app.remote_claude() && app.paste_image() {
                    gpu.window.request_redraw();
                    return;
                }
                let mut typed = false;
                if let Some(p) = app.active_pane() {
                    let mode = *p.term.lock().mode();
                    if let Some(bytes) = input::encode(&event, m, mode) {
                        p.term.lock().scroll_display(Scroll::Bottom);
                        p.write(bytes);
                        typed = true;
                    }
                }
                if typed {
                    app.clear_selection();
                    app.note_input();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x, position.y);
                if app.menu.is_some() {
                    if app.menu_hover(position.x as f32, position.y as f32) {
                        gpu.window.request_redraw();
                    }
                    return;
                }
                if app.divider_drag(position.x as f32, position.y as f32) {
                    gpu.window.request_redraw();
                    return;
                }
                // Over a split divider: a resize pointer.
                let over = app.divider_at(position.x as f32, position.y as f32).map(|(_, v)| v);
                if over != app.divider_hover {
                    app.divider_hover = over;
                    gpu.window.set_cursor(match over {
                        Some(true) => CursorIcon::ColResize,
                        Some(false) => CursorIcon::RowResize,
                        None => CursorIcon::Default,
                    });
                }
                if app.card_drag_to(position.x as f32, position.y as f32) {
                    gpu.window.set_cursor(CursorIcon::Grabbing);
                    gpu.window.request_redraw();
                    return;
                }
                app.pointer = (position.x as f32, position.y as f32);
                // A program that takes the mouse gets the motion it asked for.
                if app.mouse_report_motion(position.x as f32, position.y as f32) {
                    return;
                }
                if app.mouse_drag(position.x as f32, position.y as f32) {
                    gpu.window.request_redraw();
                }
                hover_link(app, gpu, self.mouse, self.mods);
                let (changed, over) = app.hover_timeline(position.x as f32, position.y as f32);
                if changed {
                    gpu.window.set_cursor(if over { CursorIcon::Pointer } else { CursorIcon::Default });
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                if app.divider_release() {
                    gpu.window.request_redraw();
                }
                if app.card_drop(self.mouse.0 as f32, self.mouse.1 as f32) {
                    gpu.window.set_cursor(CursorIcon::Default);
                    gpu.window.request_redraw();
                }
                app.mouse_report(self.mouse.0 as f32, self.mouse.1 as f32, 0, false);
                app.mouse_up();
            }
            WindowEvent::MouseInput { state, button: button @ (MouseButton::Middle | MouseButton::Right), .. }
                if state == ElementState::Released =>
            {
                let b = if button == MouseButton::Middle { 1 } else { 2 };
                app.mouse_report(self.mouse.0 as f32, self.mouse.1 as f32, b, false);
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Middle, .. } => {
                let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
                if app.help || app.overview || app.card_at(mx, my).is_some() {
                } else if !self.mods.shift_key() && app.mouse_report(mx, my, 1, true) {
                } else {
                    paste_into(app, clipboard::paste_primary());
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right, .. } => {
                let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
                app.menu = None;
                if !app.help && !app.overview && app.prompt.is_none() && !app.open_menu(mx, my) && !self.mods.shift_key() {
                    app.mouse_report(mx, my, 2, true);
                }
                app.menu_hover(mx, my);
                gpu.window.request_redraw();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
                if app.menu.is_some() {
                    app.menu_click(mx, my);
                    gpu.window.request_redraw();
                } else if app.help {
                    app.toggle_help();
                    gpu.window.request_redraw();
                } else if app.resume.is_some() {
                    app.resume_click(mx, my);
                    gpu.window.request_redraw();
                } else if app.usage_open {
                    app.toggle_usage();
                    gpu.window.request_redraw();
                } else if app.footer_click(mx, my) {
                    gpu.window.request_redraw();
                } else if app.overview {
                    if let Some(i) = app.overview_tile_at(mx, my) {
                        app.overview_open(i);
                    }
                    gpu.window.request_redraw();
                } else if app.timeline_at(mx, my) {
                    // The collapsed strip opens it; the open rail's header closes it.
                    if !app.timeline_open || app.timeline_header_at(my) {
                        app.toggle_timeline();
                        gpu.window.request_redraw();
                    } else if app.timeline_click(mx, my) {
                        gpu.window.request_redraw();
                    }
                } else if let Some(h) = app.resume_at(mx, my) {
                    app.open_resume(h);
                    gpu.window.request_redraw();
                } else if let Some(h) = app.add_at(mx, my) {
                    app.new_session(h);
                    gpu.window.request_redraw();
                } else if app.head_click(mx, my) {
                    gpu.window.request_redraw();
                } else if let Some(i) = app.card_at(mx, my) {
                    // Shift+click picks sessions to group; Ctrl+click brings
                    // that session beside this one (tmux's join-pane).
                    if self.mods.shift_key() {
                        app.pick(i);
                    } else if self.mods.control_key() && i != app.active {
                        app.join(i);
                    } else {
                        app.picked.clear();
                        app.select(i);
                        app.card_press(i, mx, my);
                    }
                    gpu.window.request_redraw();
                } else if self.mods.control_key() && app.open_link(mx, my) {
                } else if app.divider_press(mx, my) {
                    gpu.window.request_redraw();
                } else if !self.mods.shift_key() && app.mouse_report(mx, my, 0, true) {
                    // The program takes the click (Shift+click still selects).
                } else if app.mouse_down(mx, my) {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 3.0,
                    MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => y as f32 / app.glyphs.cell_h,
                };
                if app.resume.is_some() {
                    app.resume_step(if lines > 0.0 { -1 } else { 1 });
                } else if app.rail_list_at(self.mouse.0 as f32, self.mouse.1 as f32) {
                    app.scroll_rail(lines);
                } else if app.timeline_open && app.timeline_at(self.mouse.0 as f32, self.mouse.1 as f32) {
                    app.scroll_timeline(lines);
                } else {
                    app.scroll(lines, self.mods.shift_key());
                }
                gpu.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                app.pump();
                let (quads, animating) = app.frame();
                gpu.renderer.upload(&mut app.glyphs.uploads);
                gpu.renderer.upload_images(&mut app.image_atlas.uploads);
                let frame = match gpu.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                        gpu.surface.configure(&gpu.renderer.device, &gpu.config);
                        gpu.window.request_redraw();
                        return;
                    }
                    _ => return,
                };
                let view = frame.texture.create_view(&Default::default());
                // Premultiplied clear: the gaps between panels show through.
                let a = app.opacity * if app.opacity < 1.0 { 0.85 } else { 1.0 };
                let b = app.theme.bg;
                let bg = [b[0] * a, b[1] * a, b[2] * a, a];
                gpu.renderer.draw(&view, (gpu.config.width, gpu.config.height), bg, &quads);
                gpu.window.pre_present_notify();
                gpu.renderer.queue.present(frame);
                if animating {
                    gpu.window.request_redraw();
                    el.set_control_flow(ControlFlow::Wait);
                } else {
                    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_millis()).unwrap_or(0);
                    el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(1000 - ms as u64 + 5)));
                }
                if app.quit {
                    el.exit();
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- headless

/// Render the UI offscreen to a PNG: used to check visuals without a display.
fn snapshot(theme: Theme, args: &Args, path: &str) {
    let (w, h) = args.size;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("device");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = gfx::Renderer::new(device, queue, format);
    let mut app = App::new(theme, args.scale, (w, h), Arc::new(|| {}));
    app.persist = false;
    app.apply_prefs(&state::load(), true);
    app.report_size = args.resize_tmux;
    app.set_hosts(hosts::load(&args.hosts));
    if args.attach {
        app.start(hosts::load(&args.hosts));
    } else if args.cmds.is_empty() {
        app.spawn(spawn_spec(Some("echo punkterm"), true));
    }
    // "@N cmd" places the session on host N (for previewing multi-host layouts).
    for c in &args.cmds {
        let (host, cmd) = match c.strip_prefix('@').and_then(|r| r.split_once(' ')) {
            Some((n, rest)) => (n.parse().unwrap_or(0), rest),
            None => (0, c.as_str()),
        };
        app.spawn_on(spawn_spec(Some(cmd), true), host);
    }
    // Let the programs draw, then let animations settle.
    let step = |app: &mut App, ms: u64| {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            app.pump();
            let _ = app.frame();
            std::thread::sleep(Duration::from_millis(16));
        }
    };
    step(&mut app, 2000);
    if let Some(i) = args.select {
        app.select(i.saturating_sub(1));
    }
    if let Some(t) = args.typed.as_ref().filter(|_| args.show.as_deref() != Some("connect")) {
        if let Some(p) = app.active_pane() {
            p.write(t.replace("\\n", "\r").into_bytes());
        }
        app.note_input();
    }
    // "x0,y0,x1,y1": select by dragging, then print what was selected.
    if let Some(d) = &args.drag {
        let v: Vec<f32> = d.split(',').filter_map(|n| n.parse().ok()).collect();
        if let [x0, y0, x1, y1] = v[..] {
            step(&mut app, 600);
            app.mouse_down(x0, y0);
            app.mouse_drag(x1, y1);
            println!("selected: {:?}", app.selected_text());
        }
    }
    match args.show.as_deref() {
        Some("help") => app.toggle_help(),
        Some("usage") => {
            app.toggle_usage();
            step(&mut app, 2500);
        }
        // "cdrag:N:G": session N's card held over group G's header.
        Some(s) if s.starts_with("cdrag:") => {
            let v: Vec<usize> = s["cdrag:".len()..].split(':').filter_map(|x| x.parse().ok()).collect();
            if let [n, g] = v[..] {
                app.drag_card_over(n, g);
            }
        }
        // "divider:DX": drag the first side-by-side divider DX px.
        Some(s) if s.starts_with("divider:") => {
            let dx = s["divider:".len()..].parse().unwrap_or(0.0);
            app.drag_divider_by(dx);
            step(&mut app, 1200);
        }
        // "click:X,Y": a left click, reported to the program if it takes the mouse.
        Some(s) if s.starts_with("click:") => {
            let v: Vec<f32> = s["click:".len()..].split(',').filter_map(|n| n.parse().ok()).collect();
            if let [x, y] = v[..] {
                app.mouse_report(x, y, 0, true);
                app.mouse_report(x, y, 0, false);
                step(&mut app, 1000);
            }
        }
        // "dclick:X,Y": a double-click there; prints what it selected.
        Some(s) if s.starts_with("dclick:") => {
            let v: Vec<f32> = s["dclick:".len()..].split(',').filter_map(|n| n.parse().ok()).collect();
            if let [x, y] = v[..] {
                app.mouse_down(x, y);
                app.mouse_up();
                app.mouse_down(x, y);
                println!("selected: {:?}", app.selected_text());
            }
        }
        // "hover:X,Y": Ctrl held over a point (the link underline).
        Some(s) if s.starts_with("hover:") => {
            let v: Vec<f32> = s["hover:".len()..].split(',').filter_map(|n| n.parse().ok()).collect();
            if let [x, y] = v[..] {
                app.hover_link(x, y, true);
            }
        }
        // "link:X,Y": print the link at a point (what Ctrl+click would open).
        Some(s) if s.starts_with("link:") => {
            let v: Vec<f32> = s["link:".len()..].split(',').filter_map(|n| n.parse().ok()).collect();
            if let [x, y] = v[..] {
                println!("link: {:?}", app.link_uri_at(x, y));
            }
        }
        // "accounts[:folded]": three account configs on this machine.
        Some(s) if s.starts_with("accounts") => {
            app.fake_accounts();
            if s.ends_with(":folded") {
                app.plan_collapsed = true;
                app.load_collapsed = true;
            }
        }
        // "resume[:N]": this machine's resume menu, row N picked.
        Some(s) if s.starts_with("resume") => {
            app.open_resume(0);
            step(&mut app, 2500);
            for _ in 0..s.trim_end_matches(":pick").strip_prefix("resume:").and_then(|n| n.parse::<usize>().ok()).unwrap_or(0) {
                app.resume_step(1);
            }
            if s.ends_with(":pick") {
                app.resume_pick();
                step(&mut app, 1500);
            }
        }
        Some("stats") => app.fake_host_stats("app.example.com", sysstat::Load { cpu: 0.62, mem: 0.71, disk: Some(0.86) }),
        // "menu:N": right-click the Nth session's card; "menu:gN" group N's header.
        Some(s) if s.starts_with("menu:") => {
            app.menu_target(&s["menu:".len()..]);
        }
        Some("grid") => app.toggle_overview(),
        // "relaunch": the local tmux session goes away (its last shell
        // closed), then a new shell is asked for, as Ctrl+Shift+T would.
        Some("relaunch") => {
            let _ = std::process::Command::new("tmux").args(["kill-server"]).status();
            step(&mut app, 1500);
            println!("after close: {} panes, local {:?}", app.panes.len(), app.host_state.first());
            app.new_session(0);
            step(&mut app, 3000);
            println!("after new: {} panes, local {:?}", app.panes.len(), app.host_state.first());
        }
        Some(s) if s.starts_with("timeline") => {
            // Not toggle_timeline: a snapshot must not save prefs.
            app.timeline_open = true;
            app.resize(app.size, app.scale);
            step(&mut app, 2500);
            // "timeline:N" clicks the Nth item from the newest.
            if let Some(n) = s.strip_prefix("timeline:").and_then(|n| n.parse::<usize>().ok()) {
                step(&mut app, 5000);
                if let Some((x, y)) = app.timeline_item_center(n) {
                    println!("clicked: {}", app.timeline_click(x, y));
                    step(&mut app, 300);
                }
            }
        }
        Some("connect") => {
            app.open_prompt();
            for k in args.typed.as_deref().unwrap_or("").split('|') {
                app.prompt_key(match k {
                    "↓" => ui::PromptKey::Down,
                    k => ui::PromptKey::Char(k.to_string()),
                });
            }
        }
        _ => {}
    }
    step(&mut app, 800);
    if args.scroll != 0.0 {
        app.scroll(args.scroll, false);
    }
    app.pump();
    let (quads, _) = app.frame();
    renderer.upload(&mut app.glyphs.uploads);
    renderer.upload_images(&mut app.image_atlas.uploads);

    let tex = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    renderer.draw(&view, (w, h), app.theme.bg, &quads);

    let row = (w * 4).div_ceil(256) * 256;
    let buf = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = renderer.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    renderer.queue.submit([enc.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = renderer.device.poll(wgpu::PollType::wait_indefinitely());
    let data = buf.slice(..).get_mapped_range().expect("map");
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let s = (y * row) as usize;
        pixels.extend_from_slice(&data[s..s + (w * 4) as usize]);
    }
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().expect("png header").write_image_data(&pixels).expect("png data");
    println!("wrote {path}");
}
