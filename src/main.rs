mod anim;
mod claude;
mod clipboard;
mod gfx;
mod glyphs;
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
#[cfg(target_os = "linux")]
use winit::platform::wayland::WindowAttributesExtWayland;
#[cfg(target_os = "macos")]
use winit::platform::macos::{OptionAsAlt, WindowAttributesExtMacOS};
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

/// An app opened from Finder or the Dock gets launchd's bare environment:
/// no Homebrew on PATH (so no tmux) and possibly no SHELL.
fn mac_env() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let mut dirs: Vec<&str> = ["/opt/homebrew/bin", "/usr/local/bin"]
        .into_iter()
        .filter(|d| std::path::Path::new(d).is_dir() && !path.split(':').any(|p| p == *d))
        .collect();
    // Safety: runs first thing in main, before any thread exists.
    unsafe {
        if !dirs.is_empty() {
            dirs.push(&path);
            std::env::set_var("PATH", dirs.join(":"));
        }
        if std::env::var_os("SHELL").is_none() {
            std::env::set_var("SHELL", "/bin/zsh");
        }
    }
}

/// On macOS, Cmd does what Ctrl+Shift does elsewhere (Cmd+C copies, Cmd+T
/// opens a shell, Cmd+= grows the text); Ctrl keeps going to programs.
fn mac_command(m: ModifiersState) -> ModifiersState {
    if cfg!(target_os = "macos") && m.super_key() {
        (m - ModifiersState::SUPER) | ModifiersState::CONTROL | ModifiersState::SHIFT
    } else {
        m
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
    let (Some(t), Some(p)) = (text, app.active_pane()) else { return };
    let bracketed = p.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
    let body = t.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        p.write(format!("\x1b[200~{}\x1b[201~", body.replace('\x1b', "")).into_bytes());
    } else {
        p.write(body.into_bytes());
    }
}

fn main() {
    mac_env();
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
            .with_inner_size(LogicalSize::new(1440.0, 900.0))
            .with_transparent(true);
        // The app id compositors group and match windows by.
        #[cfg(target_os = "linux")]
        let attrs = attrs.with_name("punkterm", "punkterm");
        // Left Option is Alt (Alt+hjkl, Meta in shells); right Option still
        // types accented characters.
        #[cfg(target_os = "macos")]
        let attrs = attrs.with_option_as_alt(OptionAsAlt::OnlyLeft);
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
                let m = mac_command(self.mods);
                let code = match event.physical_key {
                    PhysicalKey::Code(c) => Some(c),
                    _ => None,
                };
                // Overlays take the keyboard while they are open.
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
                if m.shift_key() && !m.control_key() && !m.alt_key() && code == Some(KeyCode::Insert) {
                    paste_into(app, clipboard::paste_primary());
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
                        Some(KeyCode::KeyV) => {
                            paste_into(app, clipboard::paste());
                            true
                        }
                        Some(c @ (KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 | KeyCode::Digit5
                        | KeyCode::Digit6 | KeyCode::Digit7 | KeyCode::Digit8 | KeyCode::Digit9)) => {
                            app.select(c as usize - KeyCode::Digit1 as usize);
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
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => app.mouse_up(),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Middle, .. } => {
                if !app.help && !app.overview && app.card_at(self.mouse.0 as f32, self.mouse.1 as f32).is_none() {
                    paste_into(app, clipboard::paste_primary());
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
                if app.help {
                    app.toggle_help();
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
                } else if let Some(h) = app.add_at(mx, my) {
                    app.new_session(h);
                    gpu.window.request_redraw();
                } else if let Some(i) = app.card_at(mx, my) {
                    app.select(i);
                    gpu.window.request_redraw();
                } else if self.mods.control_key() && app.open_link(mx, my) {
                } else if app.mouse_down(mx, my) {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 3.0,
                    MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => y as f32 / app.glyphs.cell_h,
                };
                if app.timeline_open && app.timeline_at(self.mouse.0 as f32, self.mouse.1 as f32) {
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
        Some("grid") => app.toggle_overview(),
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
