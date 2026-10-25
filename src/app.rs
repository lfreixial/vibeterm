use crate::Args;
use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::TermMode,
};
use anyhow::{Context, Result};
use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind, SetExtLinux};
use glyphon::FontSystem;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};
use vibeterm::{
    actions::{self, ACTIONS, Action},
    config::Config,
    input,
    render::{
        CellMetrics, Overlay, Palette, PaneFrame, Renderer, Snapshot, TabLabel, measure_cell,
        window_cells,
    },
    search::{self, SearchWorker},
    session::Session,
    watch::ConfigWatcher,
    workspace::{Axis, Divider, MAX_PANES, MAX_TABS, PaneId, Rect, Workspace},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    platform::modifier_supplement::KeyEventExtModifierSupplement,
    window::{CursorIcon, Window, WindowId},
};

enum Event {
    Wake(PaneId),
    Spawned {
        id: PaneId,
        origin: PaneId,
        axis: Option<Axis>,
        size: vibeterm::session::Size,
        result: std::result::Result<Session, String>,
    },
    Paste(PaneId, std::result::Result<String, String>),
    Notice(String),
    Config(std::result::Result<Config, String>),
    Search(search::Response),
}
enum ClipboardCommand {
    Copy(String, bool),
    Paste(PaneId, bool),
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
                ClipboardCommand::Paste(id, primary) => {
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
                    let _ = proxy.send_event(Event::Paste(id, result));
                }
            }
        }
    });
    tx
}

struct SearchUi {
    query: String,
    status: String,
    token: u64,
    matched: Option<search::Match>,
}

struct Pane {
    session: Session,
    size: vibeterm::session::Size,
    /// Points added to the window-wide font size for this pane only.
    zoom: f32,
    metrics: CellMetrics,
}

struct State {
    renderer: Renderer,
    panes: BTreeMap<PaneId, Pane>,
    workspace: Workspace,
    next_id: PaneId,
    spawning: bool,
    proxy: EventLoopProxy<Event>,
    directory: PathBuf,
    config_path: PathBuf,
    palette: Option<(String, usize)>,
    search: Option<SearchUi>,
    search_worker: SearchWorker,
    _watcher: Option<ConfigWatcher>,
    clipboard: SyncSender<ClipboardCommand>,
    modifiers: ModifiersState,
    mouse: PhysicalPosition<f64>,
    pressed: Option<MouseButton>,
    last_click: Option<(Instant, (usize, usize), u8)>,
    selecting: bool,
    /// Next selection autoscroll step while dragging beyond the pane edge.
    autoscroll: Option<Instant>,
    drag: Option<Divider>,
    cursor_icon: CursorIcon,
    /// Font size from the configuration file; zoom changes `Config::font_size` only.
    configured_font_size: f32,
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
    fn session(&self) -> &Session {
        &self.panes[&self.workspace.active()].session
    }
    fn metrics(&self) -> CellMetrics {
        self.panes[&self.workspace.active()].metrics
    }
    /// Recompute every pane's cell size after a font, scale or pane-zoom change.
    fn remeasure(&mut self, config: &Config) {
        for pane in self.panes.values_mut() {
            let size = (config.font_size + pane.zoom).clamp(6.0, 72.0);
            pane.metrics = self.renderer.measure(config, size);
        }
        self.resize();
    }
    fn placements(&self) -> Vec<(PaneId, Rect)> {
        self.workspace.placements(
            self.renderer.content_rect(self.workspace.tabs.len()),
            self.gap(),
        )
    }
    fn gap(&self) -> f32 {
        6.0 * self.renderer.scale
    }
    fn divider(&self) -> Option<Divider> {
        self.workspace.divider_at(
            self.renderer.content_rect(self.workspace.tabs.len()),
            self.gap(),
            self.mouse.x as f32,
            self.mouse.y as f32,
            3.0 * self.renderer.scale,
        )
    }
    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if self.cursor_icon != icon {
            self.cursor_icon = icon;
            self.renderer.window.set_cursor(icon);
        }
    }
    /// Pixels to scroll the selection by while the pointer is above (positive) or
    /// below (negative) the active pane's text area.
    fn overscroll(&self) -> f32 {
        let rect = self.active_rect();
        let y = self.mouse.y as f32;
        let top = rect.y + self.renderer.padding;
        let bottom = rect.y + rect.height - self.renderer.padding;
        if y < top {
            top - y
        } else if y > bottom {
            bottom - y
        } else {
            0.0
        }
    }
    fn autoscroll_step(&mut self) {
        let distance = self.overscroll();
        if !self.selecting || distance == 0.0 {
            self.autoscroll = None;
            return;
        }
        // Speed up with distance from the edge, at most ten lines per step.
        let lines = (1.0 + distance.abs() / self.metrics().cell_height).min(10.0) as i32;
        self.session()
            .terminal
            .lock()
            .scroll_display(Scroll::Delta(lines * distance.signum() as i32));
        let (point, side) = self.point();
        if let Some(selection) = self.session().terminal.lock().selection.as_mut() {
            selection.update(point, side);
        }
        self.autoscroll = Some(Instant::now() + Duration::from_millis(50));
        self.redraw();
    }
    fn active_rect(&self) -> Rect {
        self.placements()
            .into_iter()
            .find(|(id, _)| *id == self.workspace.active())
            .map(|(_, r)| r)
            .unwrap_or_default()
    }
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
        if let Err(error) = self.session().input(bytes) {
            self.notice(error.to_string());
            return;
        }
        let mut term = self.session().terminal.lock();
        term.scroll_display(Scroll::Bottom);
        term.selection = None;
        drop(term);
        self.blink_on = true;
        self.next_blink = Instant::now() + Duration::from_millis(600);
        self.redraw();
    }
    fn report(&mut self, bytes: Vec<u8>) {
        if let Err(error) = self.session().input(bytes) {
            self.notice(error.to_string());
        }
    }
    fn mode(&self) -> TermMode {
        *self.session().terminal.lock().mode()
    }
    fn cell(&self) -> (usize, usize) {
        let term = self.session().terminal.lock();
        let rect = self.active_rect();
        let metrics = self.metrics();
        let col = ((self.mouse.x as f32 - rect.x - self.renderer.padding).max(0.0)
            / metrics.cell_width) as usize;
        let row = ((self.mouse.y as f32 - rect.y - self.renderer.padding).max(0.0)
            / metrics.cell_height) as usize;
        (
            col.min(term.columns() - 1),
            row.min(term.screen_lines() - 1),
        )
    }
    fn point(&self) -> (Point, Side) {
        let (col, row) = self.cell();
        let offset = self.session().terminal.lock().grid().display_offset();
        let fraction = ((self.mouse.x as f32 - self.active_rect().x - self.renderer.padding)
            / self.metrics().cell_width)
            .fract();
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
        let selection = self.session().terminal.lock().selection_to_string();
        if let Some(text) = selection {
            self.clipboard(ClipboardCommand::Copy(text, primary));
        }
    }
    fn scroll(&mut self, scroll: Scroll) {
        self.session().terminal.lock().scroll_display(scroll);
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
        for (id, rect) in self.placements() {
            let pane = self.panes.get_mut(&id).unwrap();
            let size = self.renderer.pane_size(rect, pane.metrics);
            if pane.size != size {
                if let Err(error) = pane.session.resize(size) {
                    self.notice = Some(error.to_string());
                } else {
                    pane.size = size;
                }
            }
        }
        self.redraw();
    }
    fn refocus(&mut self) {
        let active = self.workspace.active();
        for (id, pane) in &self.panes {
            let mut term = pane.session.terminal.lock();
            let focused = *id == active && self.focused;
            if term.is_focused != focused && term.mode().contains(TermMode::FOCUS_IN_OUT) {
                let _ = pane.session.input(if focused {
                    b"\x1b[I".to_vec()
                } else {
                    b"\x1b[O".to_vec()
                });
            }
            term.is_focused = focused;
        }
        self.selecting = false;
        self.autoscroll = None;
        self.drag = None;
        self.pressed = None;
        self.composing = false;
        self.preedit.clear();
        self.palette = None;
        self.search = None;
        self.search_worker.cancel();
        self.ime_position = None;
        self.blink_on = true;
        self.resize();
    }
    fn close_pane(&mut self, id: PaneId) {
        if let Some(pane) = self.panes.remove(&id) {
            pane.session.close();
            std::thread::spawn(move || drop(pane));
        }
        self.workspace.close(id);
        if !self.workspace.tabs.is_empty() {
            self.refocus();
        }
    }
    fn enabled(&self, action: Action) -> bool {
        match action {
            Action::NewTab => {
                !self.spawning
                    && self.workspace.tabs.len() < MAX_TABS
                    && self.panes.len() < MAX_PANES
            }
            Action::SplitColumns | Action::SplitRows => {
                !self.spawning && self.panes.len() < MAX_PANES && {
                    let r = self.active_rect();
                    let metrics = self.metrics();
                    if action == Action::SplitColumns {
                        r.width
                            >= 4.0 * self.renderer.padding
                                + 8.0 * metrics.cell_width
                                + 6.0 * self.renderer.scale
                    } else {
                        r.height
                            >= 4.0 * self.renderer.padding
                                + 4.0 * metrics.cell_height
                                + 6.0 * self.renderer.scale
                    }
                }
            }
            Action::NextTab | Action::PreviousTab => self.workspace.tabs.len() > 1,
            Action::NextPane | Action::PreviousPane => {
                self.workspace.tabs[self.workspace.active_tab].ids().len() > 1
            }
            Action::Copy => self
                .session()
                .terminal
                .lock()
                .selection_to_string()
                .is_some(),
            Action::Reload => !self.reload_pending,
            _ => true,
        }
    }
    fn palette_items(&self) -> Vec<(Action, &'static str)> {
        let query = self
            .palette
            .as_ref()
            .map(|(q, _)| q.to_lowercase())
            .unwrap_or_default();
        ACTIONS
            .iter()
            .filter(|(_, label, _)| label.to_lowercase().contains(&query))
            .map(|(a, l, _)| (*a, *l))
            .collect()
    }
    fn action(&mut self, action: Action, config: &mut Config) {
        if !self.enabled(action) {
            self.notice("Action unavailable in the current layout");
            return;
        }
        match action {
            Action::NewTab | Action::SplitColumns | Action::SplitRows => {
                self.spawning = true;
                let id = self.next_id;
                self.next_id += 1;
                let origin = self.workspace.active();
                let axis = match action {
                    Action::SplitColumns => Some(Axis::Columns),
                    Action::SplitRows => Some(Axis::Rows),
                    _ => None,
                };
                let config = config.clone();
                // Follow the focused shell's directory; fall back to the launch directory.
                let cwd = self
                    .session()
                    .cwd()
                    .unwrap_or_else(|| self.directory.clone());
                let proxy = self.proxy.clone();
                // New panes start at the window-wide font size.
                let size = self
                    .renderer
                    .pane_size(self.active_rect(), self.renderer.metrics());
                std::thread::spawn(move || {
                    let wake = proxy.clone();
                    let result = Session::spawn(
                        &config,
                        size,
                        &[],
                        Some(&cwd),
                        Arc::new(move || {
                            let _ = wake.send_event(Event::Wake(id));
                        }),
                    )
                    .map_err(|e| format!("{e:#}"));
                    let _ = proxy.send_event(Event::Spawned {
                        id,
                        origin,
                        axis,
                        size,
                        result,
                    });
                });
            }
            Action::ClosePane => self.close_pane(self.workspace.active()),
            Action::NextTab | Action::PreviousTab => {
                self.workspace.cycle_tab(action == Action::NextTab);
                self.refocus();
            }
            Action::NextPane | Action::PreviousPane => {
                self.workspace.cycle_pane(action == Action::NextPane);
                self.refocus();
            }
            Action::Copy => self.copy(false),
            Action::Paste => {
                self.clipboard(ClipboardCommand::Paste(self.workspace.active(), false))
            }
            Action::Reload => {
                self.reload_pending = true;
                let path = self.config_path.clone();
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let _ = proxy.send_event(Event::Config(
                        Config::load(&path).map_err(|e| format!("{e:#}")),
                    ));
                });
            }
            Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => {
                config.font_size = match action {
                    Action::ZoomIn => (config.font_size + 1.0).min(72.0),
                    Action::ZoomOut => (config.font_size - 1.0).max(6.0),
                    _ => {
                        // Resetting the window also clears every pane's own zoom.
                        for pane in self.panes.values_mut() {
                            pane.zoom = 0.0;
                        }
                        self.configured_font_size
                    }
                };
                self.renderer.configure_font(config);
                self.remeasure(config);
            }
            Action::PaneZoomIn | Action::PaneZoomOut | Action::PaneZoomReset => {
                let pane = self.panes.get_mut(&self.workspace.active()).unwrap();
                let current = (config.font_size + pane.zoom).clamp(6.0, 72.0);
                pane.zoom = match action {
                    Action::PaneZoomIn => (current + 1.0).min(72.0) - config.font_size,
                    Action::PaneZoomOut => (current - 1.0).max(6.0) - config.font_size,
                    _ => 0.0,
                };
                self.remeasure(config);
            }
            Action::Palette => {
                self.search = None;
                self.search_worker.cancel();
                self.palette = Some((String::new(), 0));
            }
            Action::Search => {
                self.palette = None;
                self.search = Some(SearchUi {
                    query: String::new(),
                    status: "Type to search · Enter next · Shift Enter previous · Esc close".into(),
                    token: 0,
                    matched: None,
                });
            }
        }
        self.redraw();
    }

    fn search_submit(&mut self, next: bool, backward: bool) {
        let Some(ui) = &self.search else {
            return;
        };
        let query = ui.query.clone();
        let matched = ui.matched.clone();
        let token = self.search_worker.cancel();
        self.search.as_mut().unwrap().token = token;
        if query.is_empty() {
            let ui = self.search.as_mut().unwrap();
            ui.status = "Type to search".into();
            ui.matched = None;
            self.session().terminal.lock().selection = None;
            return;
        }
        let pane = self.workspace.active();
        let session = self.session();
        let term = session.terminal.lock();
        let mut origin = Point::new(Line(-(term.grid().display_offset() as i32)), Column(0));
        if next && let Some(found) = matched {
            origin = if backward {
                *found.start()
            } else {
                *found.end()
            };
            if backward {
                if origin.column.0 > 0 {
                    origin.column.0 -= 1;
                } else {
                    origin.line.0 -= 1;
                    origin.column = term.last_column();
                }
            } else {
                origin.column.0 += 1;
                if origin.column.0 >= term.columns() {
                    origin.column = Column(0);
                    origin.line.0 += 1;
                }
            }
            if origin.line < term.topmost_line() {
                origin = Point::new(term.bottommost_line(), term.last_column());
            }
            if origin.line > term.bottommost_line() {
                origin = Point::new(term.topmost_line(), Column(0));
            }
        }
        drop(term);
        let request = search::Request {
            token,
            pane,
            query,
            origin,
            backward,
            terminal: session.terminal.clone(),
            revision: session.shared.revision.clone(),
        };
        let result = self.search_worker.submit(request);
        let ui = self.search.as_mut().unwrap();
        ui.token = token;
        ui.status = result.err().unwrap_or_else(|| "Searching…".into());
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
    /// System font discovery, started before the event loop so it overlaps window setup.
    fonts: Option<std::thread::JoinHandle<FontSystem>>,
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
        // Measure the font and start the shell while the GPU initialises on this thread.
        let fonts = self.fonts.take().context("Fonts already consumed")?;
        let (scale, inner) = (window.scale_factor() as f32, window.inner_size());
        let config = self.config.clone();
        let command = self.args.command.clone();
        let directory = self.args.directory.clone();
        let proxy = self.proxy.clone();
        let early = std::thread::spawn(move || {
            let mut fonts = fonts
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            let metrics = measure_cell(&mut fonts, &config, scale, config.font_size);
            let size = window_cells(inner.width, inner.height, scale, &config, metrics);
            let session = Session::spawn(
                &config,
                size,
                &command,
                directory.as_deref(),
                Arc::new(move || {
                    let _ = proxy.send_event(Event::Wake(1));
                }),
            );
            (fonts, session.map(|session| (session, size)))
        });
        let mut spawned = None;
        let renderer = pollster::block_on(Renderer::new(window, &self.config, || {
            let (fonts, session) = early
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            spawned = Some(session);
            fonts
        }));
        let (session, size) = match spawned {
            Some(result) => result?,
            // GPU setup failed before joining; the early thread finishes on its own.
            None => return Err(renderer.err().unwrap()),
        };
        let renderer = renderer?;
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
        let search_proxy = self.proxy.clone();
        let search_worker = SearchWorker::new(move |result| {
            let _ = search_proxy.send_event(Event::Search(result));
        });
        let watch_proxy = self.proxy.clone();
        let watcher = ConfigWatcher::start(self.config_path.clone(), move |result| {
            let _ = watch_proxy.send_event(Event::Config(result));
        });
        let watcher_error = watcher
            .as_ref()
            .err()
            .map(|e| format!("Automatic config reload unavailable: {e}"));
        Ok(State {
            search: None,
            search_worker,
            _watcher: watcher.ok(),
            panes: BTreeMap::from([(
                1,
                Pane {
                    session,
                    size,
                    zoom: 0.0,
                    metrics: renderer.metrics(),
                },
            )]),
            workspace: Workspace::new(1),
            next_id: 2,
            spawning: false,
            proxy: self.proxy.clone(),
            directory: self
                .args
                .directory
                .clone()
                .unwrap_or(std::env::current_dir()?),
            config_path: self.config_path.clone(),
            palette: None,
            renderer,
            clipboard: clipboard_worker(self.proxy.clone()),
            modifiers: ModifiersState::empty(),
            mouse: PhysicalPosition::new(0.0, 0.0),
            pressed: None,
            last_click: None,
            selecting: false,
            autoscroll: None,
            drag: None,
            cursor_icon: CursorIcon::Default,
            configured_font_size: self.config.font_size,
            preedit: String::new(),
            composing: false,
            focused: true,
            occluded: false,
            blink_on: true,
            next_blink: Instant::now() + Duration::from_millis(600),
            retry: None,
            notice: watcher_error,
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
        if state.workspace.tabs.is_empty() {
            event_loop.exit();
            return;
        }
        match event {
            Event::Wake(id) => {
                let Some(pane) = state.panes.get(&id) else {
                    return;
                };
                let error = pane.session.shared.error.lock().unwrap().take();
                let exited = pane.session.shared.exited.load(Ordering::Acquire);
                if let Some(error) = error {
                    state.notice(error);
                }
                if exited && !self.args.hold {
                    state.close_pane(id);
                } else if exited && state.notice.is_none() {
                    state.notice("Process exited · Ctrl Shift W closes the pane");
                }
                if state.workspace.tabs.is_empty() {
                    event_loop.exit();
                    return;
                }
                if state.workspace.tabs[state.workspace.active_tab]
                    .ids()
                    .contains(&id)
                {
                    state.redraw();
                }
            }
            Event::Spawned {
                id,
                origin,
                axis,
                size,
                result,
            } => {
                state.spawning = false;
                match result {
                    Ok(session) if state.panes.contains_key(&origin) => {
                        let previous = state.workspace.active();
                        state.workspace.focus(origin);
                        state.panes.insert(
                            id,
                            Pane {
                                session,
                                size,
                                zoom: 0.0,
                                metrics: state.renderer.metrics(),
                            },
                        );
                        if let Some(axis) = axis {
                            state.workspace.split(id, axis);
                        } else {
                            state.workspace.add_tab(id);
                        }
                        if previous != origin {
                            state.workspace.focus(previous);
                        }
                        state.refocus();
                        // A short-lived child can exit before its Spawned event is delivered.
                        if state.panes[&id]
                            .session
                            .shared
                            .exited
                            .load(Ordering::Acquire)
                            && !self.args.hold
                        {
                            state.close_pane(id);
                        }
                    }
                    Ok(session) => {
                        session.close();
                        std::thread::spawn(move || drop(session));
                    }
                    Err(error) => state.notice(error),
                }
            }
            Event::Paste(id, Ok(text)) => {
                if let Some(pane) = state.panes.get(&id) {
                    let bracketed = pane
                        .session
                        .terminal
                        .lock()
                        .mode()
                        .contains(TermMode::BRACKETED_PASTE);
                    if let Err(e) = pane.session.input(input::paste(&text, bracketed)) {
                        state.notice(e.to_string());
                    } else {
                        let mut term = pane.session.terminal.lock();
                        term.scroll_display(Scroll::Bottom);
                        term.selection = None;
                        drop(term);
                        state.redraw();
                    }
                }
            }
            Event::Paste(_, Err(error)) | Event::Notice(error) => state.notice(error),
            Event::Search(response) => {
                if let Some(ui) = &mut state.search
                    && ui.token == response.token
                    && state.workspace.active() == response.pane
                {
                    let pane = &state.panes[&response.pane];
                    let mut term = pane.session.terminal.lock();
                    if pane.session.shared.revision.load(Ordering::Acquire) != response.revision {
                        ui.status = "Output changed; press Enter to search again".into();
                    } else {
                        match response.result {
                            Ok(Some(found)) => {
                                let mut selection = Selection::new(
                                    SelectionType::Simple,
                                    *found.start(),
                                    Side::Left,
                                );
                                selection.update(*found.end(), Side::Right);
                                term.selection = Some(selection);
                                term.scroll_to_point(*found.start());
                                ui.matched = Some(found);
                                ui.status = "Enter next · Shift Enter previous · Esc close".into();
                            }
                            Ok(None) => {
                                ui.matched = None;
                                ui.status = "No match".into();
                                term.selection = None;
                            }
                            Err(e) => ui.status = e,
                        }
                    }
                    drop(term);
                    state.redraw();
                }
            }
            Event::Config(result) => {
                state.reload_pending = false;
                match result {
                    Ok(mut config) => {
                        let zoom = self.config.font_size - state.configured_font_size;
                        state.configured_font_size = config.font_size;
                        config.font_size = (config.font_size + zoom).clamp(6.0, 72.0);
                        state.blink_on = true;
                        for pane in state.panes.values() {
                            pane.session.configure(&config);
                        }
                        state.renderer.configure_font(&config);
                        state.remeasure(&config);
                        self.config = config;
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
        if state.workspace.tabs.is_empty() {
            event_loop.exit();
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                for pane in state.panes.values() {
                    pane.session.close();
                }
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
                state.remeasure(&self.config);
            }
            WindowEvent::Occluded(occluded) => {
                state.occluded = occluded;
                if !occluded {
                    state.redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                state.focused = focused;
                state.session().terminal.lock().is_focused = focused;
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
                    state.autoscroll = None;
                    state.drag = None;
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
                if let Some(ui) = &mut state.search {
                    if ui.query.len() + text.len() <= 256 {
                        ui.query.push_str(&text);
                        state.search_submit(false, false);
                    }
                    state.redraw();
                } else if let Some((query, selected)) = &mut state.palette {
                    if query.len() + text.len() <= 256 {
                        query.push_str(&text);
                        *selected = 0;
                    }
                    state.redraw();
                } else {
                    state.send(text.into_bytes());
                }
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
                let key = &input::latin(&event.logical_key, event.physical_key, mods);
                if state.search.is_some() {
                    if *key == Key::Named(NamedKey::Escape) {
                        state.search = None;
                        state.search_worker.cancel();
                        state.redraw();
                        return;
                    }
                    if *key == Key::Named(NamedKey::Enter) {
                        state.search_submit(true, mods.shift_key());
                        state.redraw();
                        return;
                    }
                    if *key == Key::Named(NamedKey::Backspace) {
                        state.search.as_mut().unwrap().query.pop();
                        state.search_submit(false, false);
                        state.redraw();
                        return;
                    }
                    if !mods.control_key() && !mods.alt_key() && !mods.super_key() {
                        if let Some(text) = event.text.as_deref() {
                            let ui = state.search.as_mut().unwrap();
                            if ui.query.len() + text.len() <= 256 {
                                ui.query.push_str(text);
                                state.search_submit(false, false);
                            }
                        }
                        state.redraw();
                        return;
                    }
                }
                if state.palette.is_some() {
                    let items = state.palette_items();
                    let (query, selected) = state.palette.as_mut().unwrap();
                    match key {
                        Key::Named(NamedKey::Escape) => state.palette = None,
                        Key::Named(NamedKey::ArrowDown) => {
                            *selected = (*selected + 1).min(items.len().saturating_sub(1))
                        }
                        Key::Named(NamedKey::ArrowUp) => *selected = selected.saturating_sub(1),
                        Key::Named(NamedKey::Backspace) => {
                            query.pop();
                            *selected = 0;
                        }
                        Key::Named(NamedKey::Enter) => {
                            let action = items.get(*selected).map(|(a, _)| *a);
                            state.palette = None;
                            if let Some(action) = action {
                                state.action(action, &mut self.config);
                            }
                        }
                        _ => {
                            if !mods.control_key()
                                && !mods.alt_key()
                                && !mods.super_key()
                                && let Some(text) = event.text.as_deref()
                                && query.len() + text.len() <= 256
                            {
                                query.push_str(text);
                                *selected = 0;
                            }
                        }
                    }
                    state.redraw();
                    if state.workspace.tabs.is_empty() {
                        event_loop.exit();
                    }
                    return;
                }
                if let Some(action) = actions::resolve(
                    key,
                    &input::latin(&event.key_without_modifiers(), event.physical_key, mods),
                    mods,
                    &self.config.keybindings,
                ) {
                    state.action(action, &mut self.config);
                    if state.workspace.tabs.is_empty() {
                        event_loop.exit();
                    }
                    return;
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
                        _ => (),
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
                if state.palette.is_some() {
                    return;
                }
                if let Some(divider) = state.drag.clone() {
                    let min = 2.0 * state.renderer.padding + 2.0 * state.renderer.cell_width;
                    let min = match divider.axis {
                        Axis::Columns => min,
                        Axis::Rows => 2.0 * state.renderer.padding + state.renderer.cell_height,
                    };
                    let gap = state.gap();
                    state
                        .workspace
                        .drag(&divider, gap, position.x as f32, position.y as f32, min);
                    state.resize();
                    return;
                }
                if state.pressed.is_none() {
                    let icon = match state.divider().map(|d| d.axis) {
                        Some(Axis::Columns) => CursorIcon::ColResize,
                        Some(Axis::Rows) => CursorIcon::RowResize,
                        None => CursorIcon::Default,
                    };
                    state.set_cursor_icon(icon);
                }
                let current = state.cell();
                if state.selecting {
                    let (point, side) = state.point();
                    if let Some(selection) = state.session().terminal.lock().selection.as_mut() {
                        selection.update(point, side);
                    }
                    if state.overscroll() != 0.0 && state.autoscroll.is_none() {
                        state.autoscroll = Some(Instant::now());
                    }
                    state.redraw();
                } else if current != previous
                    && state
                        .active_rect()
                        .contains(position.x as f32, position.y as f32)
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
                if state.palette.is_some() {
                    return;
                }
                if button == MouseButton::Left && !pressed && state.drag.take().is_some() {
                    state.redraw();
                    return;
                }
                if pressed
                    && button == MouseButton::Left
                    && let Some(divider) = state.divider()
                {
                    state.drag = Some(divider);
                    return;
                }
                if pressed && button == MouseButton::Left {
                    let header = state.renderer.header_height(state.workspace.tabs.len());
                    if state.mouse.y < (header as f64) {
                        let width = state.renderer.window.inner_size().width as f64
                            / state.workspace.tabs.len() as f64;
                        state.workspace.active_tab =
                            ((state.mouse.x / width) as usize).min(state.workspace.tabs.len() - 1);
                        state.refocus();
                        return;
                    }
                    if let Some((id, _)) = state
                        .placements()
                        .into_iter()
                        .find(|(_, r)| r.contains(state.mouse.x as f32, state.mouse.y as f32))
                        && id != state.workspace.active()
                    {
                        state.workspace.focus(id);
                        state.refocus();
                    }
                }
                if pressed {
                    if !state
                        .active_rect()
                        .contains(state.mouse.x as f32, state.mouse.y as f32)
                    {
                        return;
                    }
                    state.pressed = Some(button);
                } else if state.pressed != Some(button) {
                    return;
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
                    if !pressed {
                        state.autoscroll = None;
                    }
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
                        state.session().terminal.lock().selection = Some(Selection::new(
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
                    state.clipboard(ClipboardCommand::Paste(state.workspace.active(), true));
                }
                if !pressed {
                    state.pressed = None;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if state.palette.is_some()
                    || !state
                        .active_rect()
                        .contains(state.mouse.x as f32, state.mouse.y as f32)
                {
                    return;
                }
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as i32,
                    MouseScrollDelta::PixelDelta(delta) => {
                        state.wheel_pixels += delta.y;
                        let height = f64::from(state.metrics().cell_height);
                        let lines = (state.wheel_pixels / height) as i32;
                        state.wheel_pixels -= f64::from(lines) * height;
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
                } else if state
                    .mode()
                    .contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
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
                let active = state.workspace.active();
                let palette = Palette::new(&self.config);
                let frames: Vec<_> = state
                    .placements()
                    .into_iter()
                    .map(|(id, rect)| {
                        let pane = &state.panes[&id];
                        pane.session.shared.dirty.store(false, Ordering::Release);
                        PaneFrame {
                            rect,
                            metrics: pane.metrics,
                            snapshot: Snapshot::capture(
                                &pane.session.terminal.lock(),
                                &palette,
                                state.blink_on && id == active,
                                state.focused,
                            ),
                            active: id == active,
                            preedit: if id == active {
                                state.preedit.clone()
                            } else {
                                String::new()
                            },
                        }
                    })
                    .collect();
                let title = state.session().shared.title.lock().unwrap().clone();
                if state.title != title {
                    state
                        .renderer
                        .window
                        .set_title(&format!("{title} — vibeterm"));
                    state.title = title;
                }
                let current = frames.iter().find(|f| f.active).unwrap();
                let status = state
                    .search
                    .as_ref()
                    .map(|ui| format!("Find > {}{}  ·  {}", ui.query, state.preedit, ui.status))
                    .or_else(|| state.notice.clone())
                    .unwrap_or_else(|| {
                        if current.snapshot.offset > 0 {
                            format!(
                                "HISTORY ↑{} · Shift End returns to prompt",
                                current.snapshot.offset
                            )
                        } else {
                            // Mention the pane's font only when it differs from the window's.
                            let font = if current.metrics.font_size == self.config.font_size {
                                String::new()
                            } else {
                                format!(" · {}pt", current.metrics.font_size)
                            };
                            format!(
                                "vibeterm / pane {} · {} × {}{font} · Ctrl Shift P commands",
                                active, current.snapshot.columns, current.snapshot.rows
                            )
                        }
                    });
                if let Some((col, row, _)) = current.snapshot.cursor
                    && state.ime_position != Some((col, row))
                {
                    state.ime_position = Some((col, row));
                    state.renderer.window.set_ime_cursor_area(
                        PhysicalPosition::new(
                            current.rect.x
                                + state.renderer.padding
                                + col as f32 * current.metrics.cell_width,
                            current.rect.y
                                + state.renderer.padding
                                + (row + 1) as f32 * current.metrics.cell_height,
                        ),
                        PhysicalSize::new(current.metrics.cell_width, current.metrics.cell_height),
                    );
                }
                let tabs: Vec<_> = state
                    .workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| TabLabel {
                        text: format!(
                            "{}  {}",
                            index + 1,
                            state.panes[&tab.active]
                                .session
                                .shared
                                .title
                                .lock()
                                .unwrap()
                        ),
                        active: index == state.workspace.active_tab,
                    })
                    .collect();
                let overlay = state.palette.as_ref().map(|(query, selected)| Overlay {
                    title: format!("Commands > {query}{}", state.preedit),
                    selected: *selected,
                    rows: state
                        .palette_items()
                        .iter()
                        .enumerate()
                        .map(|(i, (action, label))| {
                            format!(
                                "{} {}   {}{}",
                                if i == *selected { ">" } else { " " },
                                label,
                                actions::label(*action, &self.config.keybindings),
                                if state.enabled(*action) {
                                    ""
                                } else {
                                    "  [unavailable]"
                                }
                            )
                        })
                        .chain(std::iter::once("↑↓ choose · Enter run · Esc close".into()))
                        .collect(),
                });
                match state
                    .renderer
                    .draw(&frames, &self.config, &status, &tabs, overlay.as_ref())
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
        let blink =
            self.config.cursor_blink && state.focused && !state.occluded && state.palette.is_none();
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
        if state
            .autoscroll
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            state.autoscroll_step();
        }
        let deadline = state
            .retry
            .into_iter()
            .chain(blink.then_some(state.next_blink))
            .chain(state.autoscroll)
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
    let fonts = std::thread::spawn(FontSystem::new);
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
        fonts: Some(fonts),
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
