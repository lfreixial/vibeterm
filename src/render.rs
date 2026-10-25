use crate::{
    config::{Config, parse_color},
    session::{Size, Terminal},
    workspace::Rect,
};
use alacritty_terminal::{
    grid::Dimensions,
    term::{cell::Flags, color::Colors},
    vte::ansi::{Color as AnsiColor, CursorShape, NamedColor},
};
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use lru::LruCache;
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};
use wgpu::util::DeviceExt;
use winit::window::Window;

const ANSI: [[u8; 3]; 16] = [
    [39, 45, 57],
    [235, 112, 123],
    [161, 206, 142],
    [235, 202, 137],
    [130, 170, 230],
    [190, 153, 225],
    [128, 203, 196],
    [216, 222, 233],
    [95, 105, 125],
    [245, 142, 150],
    [184, 220, 162],
    [245, 218, 164],
    [158, 192, 245],
    [211, 178, 242],
    [158, 224, 216],
    [242, 245, 250],
];

/// Configured colors, parsed once per frame rather than once per cell.
pub struct Palette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub accent: [u8; 3],
}
impl Palette {
    pub fn new(config: &Config) -> Self {
        let color = |value: &str| parse_color(value).unwrap_or([255, 0, 255]);
        Self {
            foreground: color(&config.foreground),
            background: color(&config.background),
            accent: color(&config.accent),
        }
    }
}

pub fn indexed(index: usize, palette: &Palette) -> [u8; 3] {
    match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let n = index - 16;
            let level = |n: usize| if n == 0 { 0 } else { 55 + 40 * n as u8 };
            [level(n / 36), level(n / 6 % 6), level(n % 6)]
        }
        232..=255 => [8 + 10 * (index - 232) as u8; 3],
        257 => palette.background,
        258 => palette.accent,
        259..=266 => ANSI[index - 259].map(|c| (f32::from(c) * 0.66) as u8),
        _ => palette.foreground,
    }
}

fn resolve(color: AnsiColor, colors: &Colors, palette: &Palette) -> [u8; 3] {
    let index = match color {
        AnsiColor::Spec(rgb) => return [rgb.r, rgb.g, rgb.b],
        AnsiColor::Named(named) => named as usize,
        AnsiColor::Indexed(index) => index as usize,
    };
    colors[index]
        .map(|rgb| [rgb.r, rgb.g, rgb.b])
        .unwrap_or_else(|| indexed(index, palette))
}

pub struct Cell {
    pub column: usize,
    pub row: usize,
    /// Glyph to draw, or `None` for blanks and spacers.
    pub glyph: Option<Glyph>,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub flags: Flags,
}
pub struct Snapshot {
    pub cells: Vec<Cell>,
    pub cursor: Option<(usize, usize, CursorShape)>,
    pub background: [u8; 3],
    pub offset: usize,
    pub columns: usize,
    pub rows: usize,
}
impl Snapshot {
    /// `focused` is window focus: an unfocused window shows a hollow block cursor.
    pub fn capture(
        term: &Terminal,
        palette: &Palette,
        cursor_visible: bool,
        focused: bool,
    ) -> Self {
        let content = term.renderable_content();
        let background = resolve(
            AnsiColor::Named(NamedColor::Background),
            content.colors,
            palette,
        );
        let cursor_row = content.cursor.point.line.0 + content.display_offset as i32;
        let cursor = (cursor_visible
            && cursor_row >= 0
            && cursor_row < term.screen_lines() as i32
            && content.cursor.shape != CursorShape::Hidden)
            .then_some((
                content.cursor.point.column.0,
                cursor_row.max(0) as usize,
                match content.cursor.shape {
                    CursorShape::Block if !focused => CursorShape::HollowBlock,
                    shape => shape,
                },
            ));
        let mut cells = Vec::with_capacity(term.columns() * term.screen_lines());
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + content.display_offset as i32;
            if row < 0 || row >= term.screen_lines() as i32 {
                continue;
            }
            let cell = indexed.cell;
            let mut fg = resolve(cell.fg, content.colors, palette);
            let mut bg = resolve(cell.bg, content.colors, palette);
            if cell.flags.contains(Flags::DIM) {
                fg = fg.map(|c| (f32::from(c) * 0.66) as u8);
            }
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if content.selection.as_ref().is_some_and(|s| {
                s.contains_cell(&indexed, content.cursor.point, content.cursor.shape)
            }) {
                bg = [57, 78, 93];
                fg = [242, 245, 250];
            }
            if cursor.is_some_and(|(col, line, shape)| {
                shape == CursorShape::Block && col == indexed.point.column.0 && line == row as usize
            }) {
                bg = palette.accent;
                fg = background;
            }
            let glyph = (!cell.flags.intersects(
                Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN,
            ) && cell.c != ' ')
                .then(|| Glyph {
                    base: cell.c,
                    // Combining marks are rare; only those cells allocate.
                    marks: cell.zerowidth().map(Box::from),
                });
            // Blank cells on the default background draw nothing; skip copying them.
            if glyph.is_none()
                && bg == background
                && !cell
                    .flags
                    .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
            {
                continue;
            }
            cells.push(Cell {
                column: indexed.point.column.0,
                row: row as usize,
                glyph,
                fg,
                bg,
                flags: cell.flags,
            });
        }
        Self {
            cells,
            cursor,
            background,
            offset: content.display_offset,
            columns: term.columns(),
            rows: term.screen_lines(),
        }
    }
}

/// Font size (points, before display scaling) and the cell it produces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub font_size: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// A cell's text without per-frame allocation: one character plus any combining marks.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct Glyph {
    pub base: char,
    pub marks: Option<Box<[char]>>,
}
impl Glyph {
    fn text(&self) -> String {
        std::iter::once(self.base)
            .chain(self.marks.iter().flat_map(|marks| marks.iter().copied()))
            .collect()
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct GlyphKey {
    glyph: Glyph,
    size: u32,
    bold: bool,
    italic: bool,
    wide: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

fn shape_buffer(
    font_system: &mut FontSystem,
    text: &str,
    attrs: Attrs<'_>,
    metrics: Metrics,
    width: Option<f32>,
) -> Buffer {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(font_system, width, Some(metrics.line_height));
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_text(font_system, text, &attrs, Shaping::Advanced);
    buffer.shape_until_scroll(font_system, false);
    buffer
}

fn family(config: &Config) -> Family<'_> {
    if config.font_family == "monospace" {
        Family::Monospace
    } else {
        Family::Name(&config.font_family)
    }
}

/// Cell size of a font, measured from the advance of "M" (terminal fonts are monospace).
pub fn measure_cell(
    font_system: &mut FontSystem,
    config: &Config,
    scale: f32,
    font_size: f32,
) -> CellMetrics {
    let cell_height = (font_size * config.line_spacing * scale).ceil();
    let buffer = shape_buffer(
        font_system,
        "M",
        Attrs::new().family(family(config)),
        Metrics::new(font_size * scale, cell_height),
        None,
    );
    CellMetrics {
        font_size,
        cell_width: buffer
            .layout_runs()
            .next()
            .map(|run| run.line_w)
            .unwrap_or(font_size * scale * 0.6)
            .round()
            .max(1.0),
        cell_height,
    }
}

/// Grid that fits a pane rectangle; shared by startup and the renderer so both agree.
pub fn cells(rect: Rect, padding: f32, metrics: CellMetrics) -> Size {
    Size {
        columns: (((rect.width - 2.0 * padding).max(0.0) / metrics.cell_width) as usize)
            .clamp(2, 500),
        rows: (((rect.height - 2.0 * padding).max(0.0) / metrics.cell_height) as usize)
            .clamp(1, 200),
        cell_width: metrics.cell_width as u16,
        cell_height: metrics.cell_height as u16,
    }
}

fn footer_height(scale: f32) -> f32 {
    (28.0 * scale).round()
}

/// Grid of a single-pane window, computable before the GPU is ready.
pub fn window_cells(
    width: u32,
    height: u32,
    scale: f32,
    config: &Config,
    metrics: CellMetrics,
) -> Size {
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: width as f32,
        height: (height as f32 - footer_height(scale)).max(0.0),
    };
    cells(rect, (config.padding * scale).round(), metrics)
}

fn status_buffer(
    font_system: &mut FontSystem,
    text: &str,
    config: &Config,
    scale: f32,
    width: f32,
) -> Buffer {
    // UI text follows display scaling, independently of terminal zoom and line spacing.
    shape_buffer(
        font_system,
        text,
        Attrs::new().family(family(config)),
        Metrics::new(11.0 * scale, (16.0 * scale).ceil()),
        Some(width.max(0.0)),
    )
}

fn intersection(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    Rect {
        x,
        y,
        width: ((a.x + a.width).min(b.x + b.width) - x).max(0.0),
        height: ((a.y + a.height).min(b.y + b.height) - y).max(0.0),
    }
}
fn bounds(rect: Rect) -> TextBounds {
    TextBounds {
        left: rect.x.ceil() as i32,
        top: rect.y.ceil() as i32,
        right: (rect.x + rect.width).floor() as i32,
        bottom: (rect.y + rect.height).floor() as i32,
    }
}

pub struct PaneFrame {
    pub rect: Rect,
    pub metrics: CellMetrics,
    pub snapshot: Snapshot,
    pub active: bool,
    pub preedit: String,
}
pub struct TabLabel {
    pub text: String,
    pub active: bool,
}
pub struct Overlay {
    pub title: String,
    pub rows: Vec<String>,
    pub selected: usize,
}
struct PlacedText {
    buffer: Arc<Buffer>,
    x: f32,
    y: f32,
    bounds: TextBounds,
    color: [u8; 3],
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    font_system: FontSystem,
    swash: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text: TextRenderer,
    solid: wgpu::RenderPipeline,
    glyphs: LruCache<GlyphKey, Arc<Buffer>>,
    measured: HashMap<u32, CellMetrics>,
    ui_glyphs: LruCache<(String, u32), Arc<Buffer>>,
    /// Cell size at the window-wide font size.
    font_size: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub padding: f32,
    pub footer_height: f32,
    pub scale: f32,
    pub frames: u64,
    pub frame_times: VecDeque<Duration>,
    pub adapter_info: String,
    pub window: Arc<Window>,
}

impl Renderer {
    /// `fonts` is called once the GPU is ready, so font loading can run concurrently.
    pub async fn new(
        window: Arc<Window>,
        config: &Config,
        fonts: impl FnOnce() -> FontSystem,
    ) -> Result<Self> {
        // OpenGL/EGL initialisation costs ~20 ms and memory; only pay it without Vulkan.
        let mut found = None;
        for backends in [wgpu::Backends::VULKAN, wgpu::Backends::GL] {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends,
                ..Default::default()
            });
            let Ok(surface) = instance.create_surface(window.clone()) else {
                continue;
            };
            if let Ok(adapter) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&surface),
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: false,
                })
                .await
            {
                found = Some((surface, adapter));
                break;
            }
        }
        let (surface, adapter) = found.context(
            "No usable Vulkan/OpenGL adapter. Check your graphics driver and desktop session",
        )?;
        let info = adapter.get_info();
        let adapter_info = format!("{} / {:?} / {}", info.name, info.backend, info.driver);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .context("Initialize GPU device")?;
        let size = window.inner_size();
        let mut surface_config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("GPU surface has no supported format")?;
        // Non-sRGB keeps terminal RGB values consistent with the glyph shader's output.
        if let Some(format) = surface
            .get_capabilities(&adapter)
            .formats
            .iter()
            .find(|f| !f.is_srgb())
        {
            surface_config.format = *format;
        }
        surface_config.present_mode = wgpu::PresentMode::Fifo;
        // Frames are drawn only on change, so a deeper queue adds latency without
        // smoothing anything; Fifo still paces output floods to the display.
        surface_config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &surface_config);
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, surface_config.format);
        let text = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cell rectangles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("solid.wgsl").into()),
        });
        let solid = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cell rectangles"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let mut renderer = Self {
            surface,
            device,
            queue,
            surface_config,
            font_system: fonts(),
            swash: SwashCache::new(),
            viewport,
            atlas,
            text,
            solid,
            glyphs: LruCache::new(NonZeroUsize::new(4096).unwrap()),
            measured: HashMap::new(),
            ui_glyphs: LruCache::new(NonZeroUsize::new(128).unwrap()),
            font_size: config.font_size,
            cell_width: 9.0,
            cell_height: 20.0,
            padding: 14.0,
            footer_height: 28.0,
            scale: window.scale_factor() as f32,
            frames: 0,
            frame_times: VecDeque::with_capacity(4096),
            adapter_info,
            window,
        };
        renderer.configure_font(config);
        Ok(renderer)
    }

    pub fn configure_font(&mut self, config: &Config) {
        self.scale = self.window.scale_factor() as f32;
        self.padding = (config.padding * self.scale).round();
        self.footer_height = footer_height(self.scale);
        self.glyphs.clear();
        self.ui_glyphs.clear();
        self.measured.clear();
        self.swash = SwashCache::new();
        let metrics = self.measure(config, config.font_size);
        self.font_size = config.font_size;
        self.cell_width = metrics.cell_width;
        self.cell_height = metrics.cell_height;
    }

    /// Cell metrics for a font size, cached until the next `configure_font`.
    pub fn measure(&mut self, config: &Config, font_size: f32) -> CellMetrics {
        if let Some(metrics) = self.measured.get(&font_size.to_bits()) {
            return *metrics;
        }
        let metrics = measure_cell(&mut self.font_system, config, self.scale, font_size);
        self.measured.insert(font_size.to_bits(), metrics);
        metrics
    }

    /// Metrics at the window-wide font size.
    pub fn metrics(&self) -> CellMetrics {
        CellMetrics {
            font_size: self.font_size,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }

    pub fn header_height(&self, tabs: usize) -> f32 {
        if tabs > 1 {
            (32.0 * self.scale).round()
        } else {
            0.0
        }
    }
    pub fn content_rect(&self, tabs: usize) -> Rect {
        let size = self.window.inner_size();
        let y = self.header_height(tabs);
        Rect {
            x: 0.0,
            y,
            width: size.width as f32,
            height: (size.height as f32 - y - self.footer_height).max(0.0),
        }
    }
    pub fn size(&self) -> Size {
        self.pane_size(self.content_rect(1), self.metrics())
    }
    pub fn pane_size(&self, rect: Rect, metrics: CellMetrics) -> Size {
        cells(rect, self.padding, metrics)
    }

    pub fn resize(&mut self) {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);
    }

    pub fn surface_size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }

    fn buffer(
        &mut self,
        text: &str,
        config: &Config,
        metrics: CellMetrics,
        bold: bool,
        italic: bool,
        width: Option<f32>,
    ) -> Buffer {
        shape_buffer(
            &mut self.font_system,
            text,
            Attrs::new()
                .family(family(config))
                .weight(if bold { Weight::BOLD } else { Weight::NORMAL })
                .style(if italic { Style::Italic } else { Style::Normal }),
            Metrics::new(metrics.font_size * self.scale, metrics.cell_height),
            width,
        )
    }

    fn rectangle(&self, vertices: &mut Vec<Vertex>, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]) {
        let to_ndc = |x, y| {
            [
                2.0 * x / self.surface_config.width as f32 - 1.0,
                1.0 - 2.0 * y / self.surface_config.height as f32,
            ]
        };
        let color = [
            f32::from(rgb[0]) / 255.0,
            f32::from(rgb[1]) / 255.0,
            f32::from(rgb[2]) / 255.0,
            1.0,
        ];
        for (px, py) in [
            (x, y),
            (x + w, y),
            (x, y + h),
            (x, y + h),
            (x + w, y),
            (x + w, y + h),
        ] {
            vertices.push(Vertex {
                position: to_ndc(px, py),
                color,
            });
        }
    }

    fn ui_text(&mut self, text: &str, config: &Config, rect: Rect, color: [u8; 3]) -> PlacedText {
        // Status/tab/palette text rarely changes between frames; reshaping it every
        // frame used to cost more than drawing the whole terminal grid.
        let key = (text.to_owned(), rect.width.to_bits());
        let buffer = if let Some(buffer) = self.ui_glyphs.get(&key) {
            buffer.clone()
        } else {
            let buffer = Arc::new(status_buffer(
                &mut self.font_system,
                text,
                config,
                self.scale,
                rect.width,
            ));
            self.ui_glyphs.put(key, buffer.clone());
            buffer
        };
        let y = rect.y + (rect.height - buffer.metrics().line_height) / 2.0;
        PlacedText {
            buffer,
            x: rect.x,
            y,
            bounds: bounds(rect),
            color,
        }
    }

    pub fn draw(
        &mut self,
        panes: &[PaneFrame],
        config: &Config,
        status: &str,
        tabs: &[TabLabel],
        overlay: Option<&Overlay>,
    ) -> Result<bool> {
        let started = Instant::now();
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(false);
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.resize();
                return Ok(false);
            }
            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::Other) => return Ok(false),
            Err(error) => return Err(error).context("GPU surface failure"),
        };
        self.viewport.update(
            &self.queue,
            Resolution {
                width: size.width,
                height: size.height,
            },
        );
        let footer_y = (size.height as f32 - self.footer_height).max(0.0);
        let mut vertices = Vec::new();
        let mut glyphs = Vec::new();
        for pane in panes.iter().filter(|_| overlay.is_none()) {
            let snapshot = &pane.snapshot;
            let rect = pane.rect;
            let metrics = pane.metrics;
            let (cell_width, cell_height) = (metrics.cell_width, metrics.cell_height);
            self.rectangle(
                &mut vertices,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                snapshot.background,
            );
            if panes.len() > 1 {
                let color = if pane.active {
                    parse_color(&config.accent)?
                } else {
                    [57, 65, 79]
                };
                self.rectangle(
                    &mut vertices,
                    rect.x,
                    rect.y,
                    rect.width,
                    self.scale * if pane.active { 2.0 } else { 1.0 },
                    color,
                );
            }
            let origin_x = rect.x + self.padding;
            let origin_y = rect.y + self.padding;
            let clip = Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height.min(footer_y - rect.y).max(0.0),
            };
            let mut solids = Vec::new();
            for cell in &snapshot.cells {
                let x = origin_x + cell.column as f32 * cell_width;
                let y = origin_y + cell.row as f32 * cell_height;
                let width = cell_width
                    * if cell.flags.contains(Flags::WIDE_CHAR) {
                        2.0
                    } else {
                        1.0
                    };
                if cell.bg != snapshot.background {
                    solids.push((
                        Rect {
                            x,
                            y,
                            width: cell_width,
                            height: cell_height,
                        },
                        cell.bg,
                    ));
                }
                if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                    solids.push((
                        Rect {
                            x,
                            y: y + cell_height - 2.0 * self.scale,
                            width,
                            height: self.scale,
                        },
                        cell.fg,
                    ));
                }
                if cell.flags.contains(Flags::STRIKEOUT) {
                    solids.push((
                        Rect {
                            x,
                            y: y + cell_height * 0.55,
                            width,
                            height: self.scale,
                        },
                        cell.fg,
                    ));
                }
                let Some(glyph) = &cell.glyph else {
                    continue;
                };
                let key = GlyphKey {
                    glyph: glyph.clone(),
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    wide: cell.flags.contains(Flags::WIDE_CHAR),
                    size: metrics.font_size.to_bits(),
                };
                let buffer = if let Some(buffer) = self.glyphs.get(&key) {
                    buffer.clone()
                } else {
                    let buffer = Arc::new(self.buffer(
                        &key.glyph.text(),
                        config,
                        metrics,
                        key.bold,
                        key.italic,
                        Some(width),
                    ));
                    self.glyphs.put(key, buffer.clone());
                    buffer
                };
                glyphs.push(PlacedText {
                    buffer,
                    x,
                    y,
                    bounds: bounds(intersection(
                        clip,
                        Rect {
                            x,
                            y,
                            width,
                            height: cell_height,
                        },
                    )),
                    color: cell.fg,
                });
            }
            if let Some((col, row, shape)) = snapshot.cursor {
                let x = origin_x + col as f32 * cell_width;
                let y = origin_y + row as f32 * cell_height;
                let color = parse_color(&config.accent)?;
                match shape {
                    CursorShape::Beam => solids.push((
                        Rect {
                            x,
                            y,
                            width: 2.0 * self.scale,
                            height: cell_height,
                        },
                        color,
                    )),
                    CursorShape::HollowBlock => {
                        let t = self.scale.max(1.0);
                        let (w, h) = (cell_width, cell_height);
                        for (rx, ry, rw, rh) in [
                            (x, y, w, t),
                            (x, y + h - t, w, t),
                            (x, y, t, h),
                            (x + w - t, y, t, h),
                        ] {
                            solids.push((
                                Rect {
                                    x: rx,
                                    y: ry,
                                    width: rw,
                                    height: rh,
                                },
                                color,
                            ));
                        }
                    }
                    CursorShape::Underline => solids.push((
                        Rect {
                            x,
                            y: y + cell_height - 2.0 * self.scale,
                            width: cell_width,
                            height: 2.0 * self.scale,
                        },
                        color,
                    )),
                    _ => (),
                }
                if !pane.preedit.is_empty() {
                    let width = (rect.x + rect.width - x - self.padding).max(0.0);
                    solids.push((
                        Rect {
                            x,
                            y,
                            width: width.min(pane.preedit.chars().count() as f32 * cell_width),
                            height: cell_height,
                        },
                        [44, 57, 69],
                    ));
                    glyphs.push(PlacedText {
                        buffer: Arc::new(self.buffer(
                            &pane.preedit,
                            config,
                            metrics,
                            false,
                            false,
                            Some(width),
                        )),
                        x,
                        y,
                        bounds: bounds(intersection(
                            clip,
                            Rect {
                                x,
                                y,
                                width,
                                height: cell_height,
                            },
                        )),
                        color,
                    });
                }
            }
            for (r, color) in solids {
                let r = intersection(r, clip);
                self.rectangle(&mut vertices, r.x, r.y, r.width, r.height, color);
            }
        }
        let header = self.header_height(tabs.len());
        if header > 0.0 {
            let width = size.width as f32 / tabs.len() as f32;
            for (index, tab) in tabs.iter().enumerate() {
                let x = index as f32 * width;
                self.rectangle(
                    &mut vertices,
                    x,
                    0.0,
                    width,
                    header,
                    if tab.active {
                        [39, 47, 59]
                    } else {
                        [21, 25, 33]
                    },
                );
                if tab.active {
                    self.rectangle(
                        &mut vertices,
                        x,
                        header - 2.0 * self.scale,
                        width,
                        2.0 * self.scale,
                        parse_color(&config.accent)?,
                    );
                }
                let label = format!("{} {}", if tab.active { "●" } else { "○" }, tab.text);
                glyphs.push(self.ui_text(
                    &label,
                    config,
                    Rect {
                        x: x + 8.0 * self.scale,
                        y: 0.0,
                        width: (width - 16.0 * self.scale).max(0.0),
                        height: header,
                    },
                    [216, 222, 233],
                ));
            }
        }
        if let Some(overlay) = overlay {
            let width = (size.width as f32 - 24.0 * self.scale)
                .min(760.0 * self.scale)
                .max(0.0);
            let x = (size.width as f32 - width) / 2.0;
            let mut y = header + 12.0 * self.scale;
            let visible = ((footer_y - y) / (28.0 * self.scale)) as usize;
            let first = (overlay.selected + 1).saturating_sub(visible.saturating_sub(1));
            for (index, line) in std::iter::once(&overlay.title)
                .chain(overlay.rows.iter().skip(first))
                .enumerate()
            {
                let height = 28.0 * self.scale;
                if y + height > footer_y {
                    break;
                }
                self.rectangle(
                    &mut vertices,
                    x,
                    y,
                    width,
                    height,
                    if index == 0 {
                        [40, 54, 66]
                    } else {
                        [26, 33, 43]
                    },
                );
                glyphs.push(self.ui_text(
                    line,
                    config,
                    Rect {
                        x: x + 12.0 * self.scale,
                        y,
                        width: (width - 24.0 * self.scale).max(0.0),
                        height,
                    },
                    if index == 0 {
                        [128, 203, 196]
                    } else {
                        [216, 222, 233]
                    },
                ));
                y += height;
            }
        }
        self.rectangle(
            &mut vertices,
            0.0,
            footer_y,
            size.width as f32,
            self.footer_height,
            [21, 25, 33],
        );
        self.rectangle(
            &mut vertices,
            0.0,
            footer_y,
            size.width as f32,
            self.scale,
            [44, 51, 64],
        );
        glyphs.push(self.ui_text(
            status,
            config,
            Rect {
                x: self.padding,
                y: footer_y,
                width: (size.width as f32 - 2.0 * self.padding).max(0.0),
                height: self.footer_height,
            },
            [151, 166, 185],
        ));
        self.text
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                glyphs.iter().map(|t| TextArea {
                    buffer: &t.buffer,
                    left: t.x,
                    top: t.y,
                    scale: 1.0,
                    bounds: t.bounds,
                    default_color: Color::rgb(t.color[0], t.color[1], t.color[2]),
                    custom_glyphs: &[],
                }),
                &mut self.swash,
            )
            .context("Prepare glyph atlas")?;
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("visible cell rectangles"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let rgb = parse_color(&config.background)?.map(|c| f64::from(c) / 255.0);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: rgb[0],
                            g: rgb[1],
                            b: rgb[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.solid);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.draw(0..vertices.len() as u32, 0..1);
            self.text
                .render(&self.atlas, &self.viewport, &mut pass)
                .context("Render glyphs")?;
        }
        self.queue.submit(Some(encoder.finish()));
        self.window.pre_present_notify();
        frame.present();
        self.atlas.trim();
        // CPU raster cache is independent from the bounded GPU atlas and shaped-cell LRU.
        if self.swash.image_cache.len() > 8192 {
            self.swash = SwashCache::new();
        }
        self.frames += 1;
        if self.frame_times.len() == 4096 {
            self.frame_times.pop_front();
        }
        self.frame_times.push_back(started.elapsed());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_text_stays_inside_bar_at_extreme_terminal_zoom() {
        let mut fonts = FontSystem::new();
        for scale in [1.0_f32, 1.25, 2.0] {
            let bar_height = (28.0 * scale).round();
            for font_size in [6.0, 15.0, 72.0] {
                for line_spacing in [1.0, 1.3, 3.0] {
                    let config = Config {
                        font_size,
                        line_spacing,
                        ..Config::default()
                    };
                    let buffer = status_buffer(
                        &mut fonts,
                        "vibeterm / Ctrl Shift C copy",
                        &config,
                        scale,
                        800.0 * scale,
                    );
                    let offset = (bar_height - buffer.metrics().line_height) / 2.0;
                    assert!(offset >= 0.0);
                    assert!(offset + buffer.metrics().line_height <= bar_height);
                    let runs: Vec<_> = buffer.layout_runs().collect();
                    assert_eq!(runs.len(), 1);
                    assert!(!runs[0].glyphs.is_empty());
                    let baseline = offset + runs[0].line_y;
                    assert!(
                        baseline > 0.0 && baseline < bar_height,
                        "font={font_size}, spacing={line_spacing}, scale={scale}: baseline {baseline} outside {bar_height}px bar"
                    );
                }
            }
        }
    }

    #[test]
    fn xterm_cube_and_grayscale() {
        let palette = Palette::new(&Config::default());
        assert_eq!(indexed(16, &palette), [0, 0, 0]);
        assert_eq!(indexed(21, &palette), [0, 0, 255]);
        assert_eq!(indexed(231, &palette), [255, 255, 255]);
        assert_eq!(indexed(232, &palette), [8, 8, 8]);
        assert_eq!(indexed(255, &palette), [238, 238, 238]);
        assert_eq!(indexed(257, &palette), [0x17, 0x1b, 0x24]);
    }
}
