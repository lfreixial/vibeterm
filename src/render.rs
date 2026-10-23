use crate::{
    config::{Config, parse_color},
    session::{Size, Terminal},
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
    collections::VecDeque,
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

pub fn indexed(index: usize, config: &Config) -> [u8; 3] {
    match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let n = index - 16;
            let level = |n: usize| if n == 0 { 0 } else { 55 + 40 * n as u8 };
            [level(n / 36), level(n / 6 % 6), level(n % 6)]
        }
        232..=255 => [8 + 10 * (index - 232) as u8; 3],
        257 => parse_color(&config.background).unwrap(),
        258 => parse_color(&config.accent).unwrap(),
        259..=266 => ANSI[index - 259].map(|c| (f32::from(c) * 0.66) as u8),
        _ => parse_color(&config.foreground).unwrap(),
    }
}

fn resolve(color: AnsiColor, colors: &Colors, config: &Config) -> [u8; 3] {
    let index = match color {
        AnsiColor::Spec(rgb) => return [rgb.r, rgb.g, rgb.b],
        AnsiColor::Named(named) => named as usize,
        AnsiColor::Indexed(index) => index as usize,
    };
    colors[index]
        .map(|rgb| [rgb.r, rgb.g, rgb.b])
        .unwrap_or_else(|| indexed(index, config))
}

pub struct Cell {
    pub column: usize,
    pub row: usize,
    pub text: String,
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
    pub fn capture(term: &Terminal, config: &Config, cursor_visible: bool) -> Self {
        let content = term.renderable_content();
        let background = resolve(
            AnsiColor::Named(NamedColor::Background),
            content.colors,
            config,
        );
        let cursor_row = content.cursor.point.line.0 + content.display_offset as i32;
        let cursor = (cursor_visible
            && cursor_row >= 0
            && cursor_row < term.screen_lines() as i32
            && content.cursor.shape != CursorShape::Hidden)
            .then_some((
                content.cursor.point.column.0,
                cursor_row.max(0) as usize,
                content.cursor.shape,
            ));
        let mut cells = Vec::with_capacity(term.columns() * term.screen_lines());
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + content.display_offset as i32;
            if row < 0 || row >= term.screen_lines() as i32 {
                continue;
            }
            let cell = indexed.cell;
            let mut fg = resolve(cell.fg, content.colors, config);
            let mut bg = resolve(cell.bg, content.colors, config);
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
                bg = parse_color(&config.accent).unwrap();
                fg = background;
            }
            let mut text = String::new();
            if !cell.flags.intersects(
                Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN,
            ) && cell.c != ' '
            {
                text.push(cell.c);
                if let Some(chars) = cell.zerowidth() {
                    text.extend(chars);
                }
            }
            cells.push(Cell {
                column: indexed.point.column.0,
                row: row as usize,
                text,
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

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct GlyphKey {
    text: String,
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

fn status_buffer(
    font_system: &mut FontSystem,
    text: &str,
    config: &Config,
    scale: f32,
    width: f32,
) -> Buffer {
    let family = if config.font_family == "monospace" {
        Family::Monospace
    } else {
        Family::Name(&config.font_family)
    };
    // UI text follows display scaling, independently of terminal zoom and line spacing.
    shape_buffer(
        font_system,
        text,
        Attrs::new().family(family),
        Metrics::new(11.0 * scale, (16.0 * scale).ceil()),
        Some(width.max(0.0)),
    )
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
    pub async fn new(window: Arc<Window>, config: &Config) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .context("Create native GPU surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: false,
            })
            .await
            .context(
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
        surface_config.desired_maximum_frame_latency = 2;
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
            font_system: FontSystem::new(),
            swash: SwashCache::new(),
            viewport,
            atlas,
            text,
            solid,
            glyphs: LruCache::new(NonZeroUsize::new(4096).unwrap()),
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
        self.cell_height = (config.font_size * config.line_spacing * self.scale).ceil();
        self.padding = (config.padding * self.scale).round();
        self.footer_height = (28.0 * self.scale).round();
        self.glyphs.clear();
        self.swash = SwashCache::new();
        let buffer = self.buffer("M", config, false, false, None);
        self.cell_width = buffer
            .layout_runs()
            .next()
            .map(|run| run.line_w)
            .unwrap_or(config.font_size * self.scale * 0.6)
            .round()
            .max(1.0);
    }

    pub fn size(&self) -> Size {
        let size = self.window.inner_size();
        Size {
            columns: (((size.width as f32 - 2.0 * self.padding) / self.cell_width) as usize)
                .clamp(2, 500),
            rows: (((size.height as f32 - 2.0 * self.padding - self.footer_height)
                / self.cell_height) as usize)
                .clamp(1, 200),
            cell_width: self.cell_width as u16,
            cell_height: self.cell_height as u16,
        }
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
        bold: bool,
        italic: bool,
        width: Option<f32>,
    ) -> Buffer {
        let family = if config.font_family == "monospace" {
            Family::Monospace
        } else {
            Family::Name(&config.font_family)
        };
        shape_buffer(
            &mut self.font_system,
            text,
            Attrs::new()
                .family(family)
                .weight(if bold { Weight::BOLD } else { Weight::NORMAL })
                .style(if italic { Style::Italic } else { Style::Normal }),
            Metrics::new(config.font_size * self.scale, self.cell_height),
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

    pub fn draw(
        &mut self,
        snapshot: &Snapshot,
        config: &Config,
        status: &str,
        preedit: &str,
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
        for cell in &snapshot.cells {
            let x = self.padding + cell.column as f32 * self.cell_width;
            let y = self.padding + cell.row as f32 * self.cell_height;
            if cell.bg != snapshot.background {
                self.rectangle(
                    &mut vertices,
                    x,
                    y,
                    self.cell_width,
                    self.cell_height,
                    cell.bg,
                );
            }
            let width = self.cell_width
                * if cell.flags.contains(Flags::WIDE_CHAR) {
                    2.0
                } else {
                    1.0
                };
            if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                self.rectangle(
                    &mut vertices,
                    x,
                    y + self.cell_height - 2.0 * self.scale,
                    width,
                    self.scale,
                    cell.fg,
                );
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                self.rectangle(
                    &mut vertices,
                    x,
                    y + self.cell_height * 0.55,
                    width,
                    self.scale,
                    cell.fg,
                );
            }
            if cell.text.is_empty() {
                continue;
            }
            let key = GlyphKey {
                text: cell.text.clone(),
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                wide: cell.flags.contains(Flags::WIDE_CHAR),
            };
            let buffer = if let Some(buffer) = self.glyphs.get(&key) {
                buffer.clone()
            } else {
                let buffer =
                    Arc::new(self.buffer(&key.text, config, key.bold, key.italic, Some(width)));
                self.glyphs.put(key, buffer.clone());
                buffer
            };
            glyphs.push((buffer, x, y, width, cell.fg));
        }
        if let Some((col, row, shape)) = snapshot.cursor {
            let x = self.padding + col as f32 * self.cell_width;
            let y = self.padding + row as f32 * self.cell_height;
            let color = parse_color(&config.accent)?;
            match shape {
                CursorShape::Beam => self.rectangle(
                    &mut vertices,
                    x,
                    y,
                    2.0 * self.scale,
                    self.cell_height,
                    color,
                ),
                CursorShape::Underline => self.rectangle(
                    &mut vertices,
                    x,
                    y + self.cell_height - 2.0 * self.scale,
                    self.cell_width,
                    2.0 * self.scale,
                    color,
                ),
                _ => (),
            }
            if !preedit.is_empty() {
                let width = (size.width as f32 - x - self.padding).max(self.cell_width);
                self.rectangle(
                    &mut vertices,
                    x,
                    y,
                    width.min(preedit.chars().count() as f32 * self.cell_width),
                    self.cell_height,
                    [44, 57, 69],
                );
                glyphs.push((
                    Arc::new(self.buffer(preedit, config, false, false, Some(width))),
                    x,
                    y,
                    width,
                    color,
                ));
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
        let footer_width = (size.width as f32 - 2.0 * self.padding).max(0.0);
        let footer = status_buffer(
            &mut self.font_system,
            status,
            config,
            self.scale,
            footer_width,
        );
        let footer_top = footer_y + (self.footer_height - footer.metrics().line_height) / 2.0;
        self.text
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                glyphs
                    .iter()
                    .map(|(buffer, x, y, width, rgb)| TextArea {
                        buffer,
                        left: *x,
                        top: *y,
                        scale: 1.0,
                        bounds: TextBounds {
                            left: *x as i32,
                            top: *y as i32,
                            right: (*x + width).ceil() as i32,
                            bottom: ((*y + self.cell_height).ceil() as i32).min(footer_y as i32),
                        },
                        default_color: Color::rgb(rgb[0], rgb[1], rgb[2]),
                        custom_glyphs: &[],
                    })
                    .chain(std::iter::once(TextArea {
                        buffer: &footer,
                        left: self.padding,
                        top: footer_top,
                        scale: 1.0,
                        bounds: TextBounds {
                            left: self.padding as i32,
                            top: footer_y.ceil() as i32,
                            right: (self.padding + footer_width).floor() as i32,
                            bottom: size.height as i32,
                        },
                        default_color: Color::rgb(151, 166, 185),
                        custom_glyphs: &[],
                    })),
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
            let rgb = snapshot.background.map(|c| f64::from(c) / 255.0);
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
        let config = Config::default();
        assert_eq!(indexed(16, &config), [0, 0, 0]);
        assert_eq!(indexed(21, &config), [0, 0, 255]);
        assert_eq!(indexed(231, &config), [255, 255, 255]);
        assert_eq!(indexed(232, &config), [8, 8, 8]);
        assert_eq!(indexed(255, &config), [238, 238, 238]);
    }
}
