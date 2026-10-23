use crate::Args;
use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::TermMode,
};
use anyhow::{Context, Result};
use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind, SetExtLinux};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};
use vibeterm::{
    config::Config,
    input,
    render::{Renderer, Snapshot},
    session::{Session, engine_config},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

enum Event {
    Wake,
    Paste(std::result::Result<String, String>),
    Notice(String),
    Config(std::result::Result<Config, String>),
}
enum ClipboardCommand {
    Copy(String, bool),
    Paste(bool),
}

fn clipboard_worker(proxy: EventLoopProxy<Event>) -> SyncSender<ClipboardCommand> {
    let (tx, rx) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        let mut clipboard = Clipboard::new();
        for request in rx {
            if clipboard.is_err() {
                clipboard = Clipboard::new();
            }
            let clipboard = match clipboard.as_mut() {
                Ok(clipboard) => clipboard,
                Err(error) => {
                    let _ =
                        proxy.send_event(Event::Notice(format!("Clipboard unavailable: {error}")));
                    continue;
                }
            };
            match request {
                ClipboardCommand::Copy(text, primary) => {
                    let kind = if primary {
                        LinuxClipboardKind::Primary
                    } else {
                        LinuxClipboardKind::Clipboard
                    };
                    if let Err(error) = clipboard.set().clipboard(kind).text(text) {
                        let _ = proxy.send_event(Event::Notice(format!("Copy failed: {error}")));
                    }
                }
                ClipboardCommand::Paste(primary) => {
                    let kind = if primary {
                        LinuxClipboardKind::Primary
                    } else {
                        LinuxClipboardKind::Clipboard
                    };
                    let result = clipboard
                        .get()
                        .clipboard(kind)
                        .text()
                        .map_err(|e| format!("Paste failed: {e}"));
                    let _ = proxy.send_event(Event::Paste(result));
                }
            }
        }
    });
    tx
}

struct State {
    renderer: Renderer,
    session: Session,
    clipboard: SyncSender<ClipboardCommand>,
    modifiers: ModifiersState,
    mouse: PhysicalPosition<f64>,
    pressed: Option<MouseButton>,
    last_click: Option<(Instant, (usize, usize), u8)>,
    selecting: bool,
    preedit: String,
    composing: bool,
    focused: bool,
    occluded: bool,
    blink_on: bool,
    next_blink: Instant,
    retry: Option<Instant>,
    notice: Option<String>,
    reload_pending: bool,
    wheel_pixels: f64,
    title: String,
    ime_position: Option<(usize, usize)>,
}
impl State {
    fn redraw(&self) {
        if !self.occluded {
            self.renderer.window.request_redraw();
        }
    }
    fn notice(&mut self, message: impl Into<String>) {
        self.notice = Some(message.into());
        self.redraw();
    }
    fn send(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Err(error) = self.session.input(bytes) {
            self.notice(error.to_string());
            return;
        }
        let mut term = self.session.terminal.lock();
        term.scroll_display(Scroll::Bottom);
        term.selection = None;
        drop(term);
        self.blink_on = true;
        self.next_blink = Instant::now() + Duration::from_millis(600);
        self.redraw();
    }
    fn report(&mut self, bytes: Vec<u8>) {
        if let Err(error) = self.session.input(bytes) {
            self.notice(error.to_string());
        }
    }
    fn mode(&self) -> TermMode {
        *self.session.terminal.lock().mode()
    }
    fn cell(&self) -> (usize, usize) {
        let term = self.session.terminal.lock();
        let col = ((self.mouse.x as f32 - self.renderer.padding).max(0.0)
            / self.renderer.cell_width) as usize;
        let row = ((self.mouse.y as f32 - self.renderer.padding).max(0.0)
            / self.renderer.cell_height) as usize;
        (
            col.min(term.columns() - 1),
            row.min(term.screen_lines() - 1),
        )
    }
    fn point(&self) -> (Point, Side) {
        let (col, row) = self.cell();
        let offset = self.session.terminal.lock().grid().display_offset();
        let fraction =
            ((self.mouse.x as f32 - self.renderer.padding) / self.renderer.cell_width).fract();
        (
            Point::new(Line(row as i32 - offset as i32), Column(col)),
            if fraction < 0.5 {
                Side::Left
            } else {
                Side::Right
            },
        )
    }
    fn clipboard(&mut self, command: ClipboardCommand) {
        if self.clipboard.try_send(command).is_err() {
            self.notice("Clipboard busy; try again");
        }
    }
    fn copy(&mut self, primary: bool) {
        let selection = self.session.terminal.lock().selection_to_string();
        if let Some(text) = selection {
            self.clipboard(ClipboardCommand::Copy(text, primary));
        }
    }
    fn scroll(&mut self, scroll: Scroll) {
        self.session.terminal.lock().scroll_display(scroll);
        self.redraw();
    }
    fn reporting(&self) -> bool {
        !self.modifiers.shift_key() && self.mode().intersects(TermMode::MOUSE_MODE)
    }
    fn resize(&mut self) {
        let size = self.renderer.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.ime_position = None;
        self.renderer.resize();
        if let Err(error) = self.session.resize(self.renderer.size()) {
            self.notice(error.to_string());
        }
        self.redraw();
    }
}

struct App {
    args: Args,
    config: Config,
    config_path: PathBuf,
    proxy: EventLoopProxy<Event>,
    state: Option<State>,
    failure: Option<anyhow::Error>,
    started: Instant,
}
impl App {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<State> {
        let attributes = Window::default_attributes()
            .with_title("vibeterm")
            .with_inner_size(LogicalSize::new(1060, 700))
            .with_min_inner_size(LogicalSize::new(320, 180));
        let attributes = winit::platform::wayland::WindowAttributesExtWayland::with_name(
            attributes, "vibeterm", "vibeterm",
        );
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("Open native window")?,
        );
        window.set_ime_allowed(true);
        let renderer = pollster::block_on(Renderer::new(window, &self.config))?;
        if self.args.diagnostics {
            eprintln!("GPU: {}", renderer.adapter_info);
            eprintln!(
                "Window: {:?}, scale={}, font={} {}, grid={}x{}, scrollback={}",
                renderer.window.inner_size(),
                renderer.scale,
                self.config.font_family,
                self.config.font_size,
                renderer.size().columns,
                renderer.size().rows,
                self.config.scrollback
            );
            if let Some(monitor) = renderer.window.current_monitor() {
                eprintln!(
                    "Monitor: {:?}, refresh={:?} mHz",
                    monitor.size(),
                    monitor.refresh_rate_millihertz()
                );
            }
        }
        let proxy = self.proxy.clone();
        let session = Session::spawn(
            &self.config,
            renderer.size(),
            &self.args.command,
            self.args.directory.as_deref(),
            Arc::new(move || {
                let _ = proxy.send_event(Event::Wake);
            }),
        )?;
        Ok(State {
            renderer,
            session,
            clipboard: clipboard_worker(self.proxy.clone()),
            modifiers: ModifiersState::empty(),
            mouse: PhysicalPosition::new(0.0, 0.0),
            pressed: None,
            last_click: None,
            selecting: false,
            preedit: String::new(),
            composing: false,
            focused: true,
            occluded: false,
            blink_on: true,
            next_blink: Instant::now() + Duration::from_millis(600),
            retry: None,
            notice: None,
            reload_pending: false,
            wheel_pixels: 0.0,
            title: String::new(),
            ime_position: None,
        })
    }
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match self.initialize(event_loop) {
            Ok(state) => {
                state.redraw();
                self.state = Some(state);
            }
            Err(error) => {
                self.failure = Some(error);
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let Some(state) = &mut self.state else {
            return;
        };
        match event {
            Event::Wake => {
                if state.occluded {
                    state.session.shared.dirty.store(false, Ordering::Release);
                }
                let error = state.session.shared.error.lock().unwrap().take();
                if let Some(error) = error {
                    state.notice(error);
                }
                if state.session.shared.exited.load(Ordering::Acquire) && !self.args.hold {
                    event_loop.exit();
                }
                state.redraw();
            }
            Event::Paste(Ok(text)) => state.send(input::paste(
                &text,
                state.mode().contains(TermMode::BRACKETED_PASTE),
            )),
            Event::Paste(Err(error)) | Event::Notice(error) => state.notice(error),
            Event::Config(result) => {
                state.reload_pending = false;
                match result {
                    Ok(config) => {
                        state.blink_on = true;
                        state
                            .session
                            .terminal
                            .lock()
                            .set_options(engine_config(&config));
                        state.renderer.configure_font(&config);
                        self.config = config;
                        state.resize();
                        state.notice("Configuration reloaded");
                    }
                    Err(error) => state.notice(format!(
                        "Reload rejected; keeping previous settings: {error}"
                    )),
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                state.session.close();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if state.renderer.surface_size() != (size.width, size.height) {
                    state.ime_position = None;
                    state.resize();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                state.renderer.configure_font(&self.config);
                state.resize();
            }
            WindowEvent::Occluded(occluded) => {
                state.occluded = occluded;
                if !occluded {
                    state.redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                state.focused = focused;
                state.session.terminal.lock().is_focused = focused;
                if state.mode().contains(TermMode::FOCUS_IN_OUT) {
                    state.report(if focused {
                        b"\x1b[I".to_vec()
                    } else {
                        b"\x1b[O".to_vec()
                    });
                }
                if !focused {
                    state.pressed = None;
                    state.selecting = false;
                }
                state.blink_on = true;
                state.redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => state.modifiers = modifiers.state(),
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                state.preedit = text;
                state.composing = cursor.is_some();
                state.redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                state.preedit.clear();
                state.composing = false;
                state.send(text.into_bytes());
            }
            WindowEvent::Ime(Ime::Disabled) => {
                state.preedit.clear();
                state.composing = false;
                state.redraw();
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => {
                if state.composing {
                    return;
                }
                let mods = state.modifiers;
                let key = &event.logical_key;
                if mods.control_key() && mods.shift_key() && !mods.alt_key() && !mods.super_key() {
                    match key.as_ref() {
                        Key::Character("c" | "C") => {
                            state.copy(false);
                            return;
                        }
                        Key::Character("v" | "V") => {
                            state.clipboard(ClipboardCommand::Paste(false));
                            return;
                        }
                        Key::Character("r" | "R") => {
                            if !state.reload_pending {
                                state.reload_pending = true;
                                let path = self.config_path.clone();
                                let proxy = self.proxy.clone();
                                std::thread::spawn(move || {
                                    let result = Config::load(&path).map_err(|e| format!("{e:#}"));
                                    let _ = proxy.send_event(Event::Config(result));
                                });
                            }
                            return;
                        }
                        _ => (),
                    }
                }
                if mods.shift_key() && !mods.control_key() && !mods.alt_key() {
                    match key {
                        Key::Named(NamedKey::PageUp) => {
                            state.scroll(Scroll::PageUp);
                            return;
                        }
                        Key::Named(NamedKey::PageDown) => {
                            state.scroll(Scroll::PageDown);
                            return;
                        }
                        Key::Named(NamedKey::Home) => {
                            state.scroll(Scroll::Top);
                            return;
                        }
                        Key::Named(NamedKey::End) => {
                            state.scroll(Scroll::Bottom);
                            return;
                        }
                        Key::Named(NamedKey::Insert) => {
                            state.clipboard(ClipboardCommand::Paste(false));
                            return;
                        }
                        _ => (),
                    }
                }
                if mods.control_key() && !mods.alt_key() {
                    let new_size = match key.as_ref() {
                        Key::Character("+" | "=") => Some(self.config.font_size + 1.0),
                        Key::Character("-") => Some(self.config.font_size - 1.0),
                        Key::Character("0") => Some(Config::default().font_size),
                        _ => None,
                    };
                    if let Some(size) = new_size {
                        self.config.font_size = size.clamp(6.0, 72.0);
                        state.renderer.configure_font(&self.config);
                        state.resize();
                        return;
                    }
                }
                if *key == Key::Named(NamedKey::Escape) {
                    state.notice = None;
                }
                if !mods.super_key() {
                    state.send(input::encode(
                        key,
                        event.text.as_deref(),
                        mods,
                        state.mode(),
                    ));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let previous = state.cell();
                state.mouse = position;
                let current = state.cell();
                if state.selecting {
                    let (point, side) = state.point();
                    if let Some(selection) = state.session.terminal.lock().selection.as_mut() {
                        selection.update(point, side);
                    }
                    state.redraw();
                } else if current != previous
                    && state.reporting()
                    && (state.mode().contains(TermMode::MOUSE_MOTION)
                        || (state.mode().contains(TermMode::MOUSE_DRAG) && state.pressed.is_some()))
                {
                    let code = match state.pressed {
                        Some(MouseButton::Left) => 0,
                        Some(MouseButton::Middle) => 1,
                        Some(MouseButton::Right) => 2,
                        _ => 3,
                    } + 32;
                    state.report(input::mouse(
                        code,
                        current.0,
                        current.1,
                        false,
                        state.modifiers,
                        state.mode(),
                    ));
                }
            }
            WindowEvent::MouseInput {
                state: button_state,
                button,
                ..
            } => {
                let pressed = button_state == ElementState::Pressed;
                if pressed {
                    state.pressed = Some(button);
                }
                if state.reporting() {
                    let code = match button {
                        MouseButton::Left => 0,
                        MouseButton::Middle => 1,
                        MouseButton::Right => 2,
                        _ => return,
                    };
                    let (col, row) = state.cell();
                    state.report(input::mouse(
                        code,
                        col,
                        row,
                        !pressed,
                        state.modifiers,
                        state.mode(),
                    ));
                } else if button == MouseButton::Left {
                    state.selecting = pressed;
                    if pressed {
                        let cell = state.cell();
                        let now = Instant::now();
                        let clicks = state
                            .last_click
                            .filter(|(time, previous, _)| {
                                now.duration_since(*time) < Duration::from_millis(400)
                                    && *previous == cell
                            })
                            .map(|(_, _, n)| n % 3 + 1)
                            .unwrap_or(1);
                        state.last_click = Some((now, cell, clicks));
                        let (point, side) = state.point();
                        state.session.terminal.lock().selection = Some(Selection::new(
                            match clicks {
                                2 => SelectionType::Semantic,
                                3 => SelectionType::Lines,
                                _ => SelectionType::Simple,
                            },
                            point,
                            side,
                        ));
                    } else {
                        state.copy(true);
                        if self.config.copy_on_select {
                            state.copy(false);
                        }
                    }
                    state.redraw();
                } else if button == MouseButton::Middle && pressed {
                    state.clipboard(ClipboardCommand::Paste(true));
                }
                if !pressed {
                    state.pressed = None;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as i32,
                    MouseScrollDelta::PixelDelta(delta) => {
                        state.wheel_pixels += delta.y;
                        let lines =
                            (state.wheel_pixels / f64::from(state.renderer.cell_height)) as i32;
                        state.wheel_pixels -=
                            f64::from(lines) * f64::from(state.renderer.cell_height);
                        lines
                    }
                }
                .clamp(-100, 100);
                if state.reporting() {
                    let (col, row) = state.cell();
                    for _ in 0..lines.abs() {
                        state.report(input::mouse(
                            if lines > 0 { 64 } else { 65 },
                            col,
                            row,
                            false,
                            state.modifiers,
                            state.mode(),
                        ));
                    }
                } else if state.mode().contains(TermMode::ALT_SCREEN)
                    && !state.modifiers.shift_key()
                {
                    for _ in 0..lines.abs() {
                        state.send(input::encode(
                            &Key::Named(if lines > 0 {
                                NamedKey::ArrowUp
                            } else {
                                NamedKey::ArrowDown
                            }),
                            None,
                            ModifiersState::empty(),
                            state.mode(),
                        ));
                    }
                } else {
                    state.scroll(Scroll::Delta(lines));
                }
            }
            WindowEvent::RedrawRequested if !state.occluded => {
                state.session.shared.dirty.store(false, Ordering::Release);
                let exited = state.session.shared.exited.load(Ordering::Acquire);
                if exited && !self.args.hold {
                    event_loop.exit();
                    return;
                }
                let snapshot =
                    Snapshot::capture(&state.session.terminal.lock(), &self.config, state.blink_on);
                let title = state.session.shared.title.lock().unwrap().clone();
                if state.title != title {
                    state
                        .renderer
                        .window
                        .set_title(&format!("{title} — vibeterm"));
                    state.title = title;
                }
                let status = state.notice.clone().unwrap_or_else(|| if exited { "  process exited · close the window to finish".into() } else if snapshot.offset > 0 { format!("  HISTORY  ↑{}    ·    Shift End  return to prompt", snapshot.offset) } else { format!("  vibeterm   /   {} × {}      Ctrl Shift C  copy    Ctrl Shift V  paste    Ctrl +/−  font", snapshot.columns, snapshot.rows) });
                if let Some((col, row, _)) = snapshot.cursor
                    && state.ime_position != Some((col, row))
                {
                    state.ime_position = Some((col, row));
                    state.renderer.window.set_ime_cursor_area(
                        PhysicalPosition::new(
                            state.renderer.padding + col as f32 * state.renderer.cell_width,
                            state.renderer.padding + (row + 1) as f32 * state.renderer.cell_height,
                        ),
                        PhysicalSize::new(state.renderer.cell_width, state.renderer.cell_height),
                    );
                }
                match state
                    .renderer
                    .draw(&snapshot, &self.config, &status, &state.preedit)
                {
                    Ok(true) => {
                        state.retry = None;
                        if state.renderer.frames == 1 && self.args.diagnostics {
                            eprintln!(
                                "First frame: {:.2} ms (font/GPU setup included; shell readiness not measured)",
                                self.started.elapsed().as_secs_f64() * 1000.0
                            );
                        }
                    }
                    Ok(false) => {
                        if state.renderer.window.inner_size().width > 0
                            && state.renderer.window.inner_size().height > 0
                        {
                            state.retry = Some(Instant::now() + Duration::from_millis(100));
                        }
                    }
                    Err(error) => {
                        self.failure = Some(error);
                        event_loop.exit();
                    }
                }
            }
            _ => (),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = &mut self.state else {
            return;
        };
        let blink = self.config.cursor_blink && state.focused && !state.occluded;
        if blink && Instant::now() >= state.next_blink {
            state.blink_on = !state.blink_on;
            state.next_blink = Instant::now() + Duration::from_millis(600);
            state.redraw();
        }
        if state
            .retry
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            state.retry = None;
            state.redraw();
        }
        let deadline = state
            .retry
            .into_iter()
            .chain(blink.then_some(state.next_blink))
            .min();
        event_loop.set_control_flow(
            deadline
                .map(ControlFlow::WaitUntil)
                .unwrap_or(ControlFlow::Wait),
        );
    }
}

pub fn run(args: Args, config: Config, config_path: PathBuf) -> Result<()> {
    let started = Instant::now();
    let event_loop = EventLoop::<Event>::with_user_event()
        .build()
        .context("Connect to Wayland/X11 desktop")?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
        args,
        config,
        config_path,
        proxy,
        state: None,
        failure: None,
        started,
    };
    event_loop.run_app(&mut app)?;
    if app.args.diagnostics
        && let Some(state) = &app.state
    {
        eprintln!("Frames presented: {}", state.renderer.frames);
        let mut times: Vec<_> = state.renderer.frame_times.iter().copied().collect();
        times.sort_unstable();
        if !times.is_empty() {
            eprintln!(
                "Frame CPU + surface acquisition + submission: n={}, p50={:.3} ms, p95={:.3} ms (not GPU or key-to-photon latency)",
                times.len(),
                times[times.len() / 2].as_secs_f64() * 1000.0,
                times[(times.len() * 95).div_ceil(100) - 1].as_secs_f64() * 1000.0
            );
        }
    }
    if let Some(error) = app.failure {
        return Err(error);
    }
    Ok(())
}
