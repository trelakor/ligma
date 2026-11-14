//! Settings' Show stats: frame times and GPU memory drawn over the corner
//! of the canvas in each present, with graphs of the last few seconds

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use skia_safe::{Canvas, Color, Font, FontMgr, FontStyle, Paint, PaintStyle, PathBuilder, Rect, RRect};

/// Presents in the frame graph
const FRAMES: usize = 200;
/// Memory samples, one a tick: the last minute
const SAMPLES: usize = 240;
/// How often the stats are drawn again while nothing else changes
pub const TICK: Duration = Duration::from_millis(250);
/// Gaps between presents longer than this are the view at rest, not a
/// slow frame
const IDLE_MS: f32 = 250.0;
/// The frame graph's scale, in ms
const SCALE_MS: f32 = 20.0;

const WIDTH: f32 = 300.0;
const PAD: f32 = 10.0;
/// Room on the right of the graphs for their guides' labels
const GUTTER: f32 = 44.0;

const SURFACE: Color = Color::from_rgb(0x1a, 0x1a, 0x19);
const INK: Color = Color::from_rgb(0xf0, 0xef, 0xec);
const SECONDARY: Color = Color::from_rgb(0xc3, 0xc2, 0xb7);
const MUTED: Color = Color::from_rgb(0x89, 0x87, 0x81);
const GRID: Color = Color::from_rgb(0x38, 0x38, 0x35);
const GOOD: Color = Color::from_rgb(0x0c, 0xa3, 0x0c);
const WARNING: Color = Color::from_rgb(0xfa, 0xb2, 0x19);
const CRITICAL: Color = Color::from_rgb(0xd0, 0x3b, 0x3b);
const LIGMA: Color = Color::from_rgb(0x39, 0x87, 0xe5);
const FIGMA: Color = Color::from_rgb(0xd9, 0x59, 0x26);

struct Frame {
    at: Instant,
    /// Since the present before, in ms, or none after the view was at rest
    interval: Option<f32>,
}

/// What the stats show besides frames, read from `stats()` each draw
#[derive(Default)]
pub struct Numbers {
    pub present_ms: f64,
    pub vello_tiles_ms: f64,
    pub tile_ms: f64,
    pub whole_ms: f64,
    pub vram_ligma: u64,
    pub vram_webview: u64,
    pub native: u32,
    pub approximate: u32,
    pub tiles: usize,
    pub zoom: f64,
}

pub struct Hud {
    font: Option<(Font, Font)>,
    frames: VecDeque<Frame>,
    last: Option<Instant>,
    memory: VecDeque<(f32, f32)>,
    sampled: Option<Instant>,
}

impl Hud {
    pub fn new() -> Self {
        let face = FontMgr::new().match_family_style("Segoe UI", FontStyle::normal());
        let bold = FontMgr::new().match_family_style("Segoe UI", FontStyle::bold());
        let font = face.map(|face| {
            let make = |face, size| {
                let mut font = Font::from_typeface(face, size);
                font.set_edging(skia_safe::font::Edging::AntiAlias);
                font.set_subpixel(true);
                font
            };
            (make(face.clone(), 11.0), make(bold.unwrap_or(face), 15.0))
        });
        Hud { font, frames: VecDeque::new(), last: None, memory: VecDeque::new(), sampled: None }
    }

    /// A frame presented for the view, not only to draw the stats again
    pub fn presented(&mut self, at: Instant) {
        let interval = self.last.map(|last| (at - last).as_secs_f32() * 1000.0).filter(|ms| *ms < IDLE_MS);
        self.last = Some(at);
        self.frames.push_back(Frame { at, interval });
        if self.frames.len() > FRAMES {
            self.frames.pop_front();
        }
    }

    pub fn draw(&mut self, canvas: &Canvas, scale: f32, numbers: &Numbers) {
        let Some((font, big)) = self.font.clone() else {
            return;
        };
        let now = Instant::now();
        if self.sampled.is_none_or(|at| now - at >= TICK) {
            self.sampled = Some(now);
            self.memory.push_back((numbers.vram_ligma as f32 / 1e6, numbers.vram_webview as f32 / 1e6));
            if self.memory.len() > SAMPLES {
                self.memory.pop_front();
            }
        }

        canvas.save();
        canvas.translate((8.0 * scale, 8.0 * scale));
        canvas.scale((scale, scale));
        let inner = WIDTH - 2.0 * PAD;
        let height = 272.0;
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_color(SURFACE);
        canvas.draw_rrect(RRect::new_rect_xy(Rect::from_wh(WIDTH, height), 8.0, 8.0), &fill);

        // The last second of presents, while the view moves
        let moving = self.last.is_some_and(|last| (now - last).as_secs_f32() * 1000.0 < IDLE_MS);
        let recent: Vec<f32> = self.frames.iter().filter(|frame| now - frame.at <= Duration::from_secs(1)).filter_map(|frame| frame.interval).collect();
        let mut y = PAD + 11.0;
        text(canvas, PAD, y, &[("Active Renderer: ", MUTED), ("Ligma", INK), (" (experimental)", MUTED)], &font);
        y += 22.0;
        if moving && !recent.is_empty() {
            let mut sorted = recent.clone();
            sorted.sort_by(f32::total_cmp);
            let at = |p: f32| sorted[((sorted.len() - 1) as f32 * p).round() as usize];
            let mean = recent.iter().sum::<f32>() / recent.len() as f32;
            let x = text(canvas, PAD, y, &[(&format!("{:.0}", 1000.0 / mean), INK)], &big);
            text(canvas, PAD + x + 4.0, y, &[("fps", SECONDARY)], &font);
            let right = [("p50 ", MUTED), (&*format!("{:.1}", at(0.5)), INK), ("  p95 ", MUTED), (&*format!("{:.1}", at(0.95)), INK), ("  max ", MUTED), (&*format!("{:.1} ms", at(1.0)), INK)];
            text_right(canvas, PAD + inner, y, &right, &font);
        } else {
            let x = text(canvas, PAD, y, &[("At rest", INK)], &big);
            text(canvas, PAD + x + 6.0, y, &[("frames show while the view moves", MUTED)], &font);
        }

        // Frame graph: a bar a present, its time since the one before
        y += 8.0;
        let graph = Rect::from_xywh(PAD, y, inner - GUTTER, 64.0);
        guide(canvas, graph, 1000.0 / 120.0, SCALE_MS, "120 fps", &font);
        guide(canvas, graph, 1000.0 / 200.0, SCALE_MS, "200 fps", &font);
        let step = graph.width() / FRAMES as f32;
        let mut bars = [(PathBuilder::new(), GOOD), (PathBuilder::new(), WARNING), (PathBuilder::new(), CRITICAL)];
        let start = FRAMES - self.frames.len();
        for (i, frame) in self.frames.iter().enumerate() {
            let Some(ms) = frame.interval else { continue };
            let h = (ms / SCALE_MS).min(1.0) * graph.height();
            let x = graph.left + (start + i) as f32 * step;
            let kind = if ms <= 1000.0 / 120.0 + 0.5 { 0 } else if ms <= 1000.0 / 60.0 + 0.5 { 1 } else { 2 };
            bars[kind].0.add_rect(Rect::from_xywh(x, graph.bottom - h, (step - 0.5).max(0.5), h), None, None);
        }
        for (mut path, color) in bars {
            fill.set_color(color);
            canvas.draw_path(&path.detach(), &fill);
        }
        line(canvas, graph.left, graph.bottom, graph.right, graph.bottom, GRID);
        y = graph.bottom + 16.0;
        text(
            canvas,
            PAD,
            y,
            &[
                ("Present ", MUTED),
                (&format!("{:.1}", numbers.present_ms), INK),
                ("  Tiles ", MUTED),
                (&format!("{:.1}", numbers.vello_tiles_ms.max(numbers.tile_ms)), INK),
                ("  Whole view ", MUTED),
                (&format!("{:.1} ms", numbers.whole_ms), INK),
            ],
            &font,
        );

        // GPU memory over the last minute, both on one scale
        y += 20.0;
        let mb = |value: f32| if value >= 1000.0 { format!("{:.2} GB", value / 1000.0) } else { format!("{value:.0} MB") };
        let (ligma, figma) = self.memory.back().copied().unwrap_or_default();
        text(canvas, PAD, y, &[("GPU memory", MUTED)], &font);
        y += 16.0;
        let mut at = PAD;
        for (name, value, color) in [("Renderer ", ligma, LIGMA), ("Figma page ", figma, FIGMA)] {
            fill.set_color(color);
            canvas.draw_circle((at + 3.5, y - 3.5), 3.5, &fill);
            at += 11.0 + text(canvas, at + 10.0, y, &[(name, SECONDARY), (&mb(value), INK)], &font) + 6.0;
        }
        y += 8.0;
        let graph = Rect::from_xywh(PAD, y, inner - GUTTER, 44.0);
        let top = self.memory.iter().fold(500.0f32, |top, (a, b)| top.max(*a).max(*b)) * 1.1;
        guide(canvas, graph, 400.0, top, "400 MB", &font);
        let step = graph.width() / (SAMPLES - 1) as f32;
        let start = SAMPLES - self.memory.len();
        let mut stroke = Paint::default();
        stroke.set_anti_alias(true);
        stroke.set_style(PaintStyle::Stroke);
        stroke.set_stroke_width(1.5);
        for (pick, color) in [(0, FIGMA), (1, LIGMA)] {
            let mut path = PathBuilder::new();
            for (i, sample) in self.memory.iter().enumerate() {
                let value = if pick == 0 { sample.1 } else { sample.0 };
                let point = (graph.left + (start + i) as f32 * step, graph.bottom - (value / top).min(1.0) * graph.height());
                if i == 0 {
                    path.move_to(point);
                } else {
                    path.line_to(point);
                }
            }
            stroke.set_color(color);
            canvas.draw_path(&path.detach(), &stroke);
        }
        line(canvas, graph.left, graph.bottom, graph.right, graph.bottom, GRID);

        y = graph.bottom + 18.0;
        let count = |value: u64| {
            let digits = value.to_string();
            let mut out = String::new();
            for (i, digit) in digits.chars().enumerate() {
                if i > 0 && (digits.len() - i) % 3 == 0 {
                    out.push(',');
                }
                out.push(digit);
            }
            out
        };
        text(
            canvas,
            PAD,
            y,
            &[
                (&count(numbers.native as u64), INK),
                (" drawn  ", MUTED),
                (&count(numbers.approximate as u64), INK),
                (" approximate  ", MUTED),
                (&count(numbers.tiles as u64), INK),
                (" tiles  ", MUTED),
                (&format!("{:.0}%", numbers.zoom * 100.0), INK),
            ],
            &font,
        );
        canvas.restore();
    }
}

/// Draws `parts` one after another from `x` on the baseline `y`, and gives
/// their width
fn text(canvas: &Canvas, x: f32, y: f32, parts: &[(&str, Color)], font: &Font) -> f32 {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    let mut at = x;
    for (part, color) in parts {
        paint.set_color(*color);
        canvas.draw_str(part, (at, y), font, &paint);
        at += font.measure_str(part, None).0;
    }
    at - x
}

fn text_right(canvas: &Canvas, right: f32, y: f32, parts: &[(&str, Color)], font: &Font) {
    let width: f32 = parts.iter().map(|(part, _)| font.measure_str(part, None).0).sum();
    text(canvas, right - width, y, parts, font);
}

fn line(canvas: &Canvas, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.set_stroke_width(1.0);
    canvas.draw_line((x0, y0), (x1, y1), &paint);
}

/// A dashed line across `graph` at `value` of `top`, labelled to its right
fn guide(canvas: &Canvas, graph: Rect, value: f32, top: f32, label: &str, font: &Font) {
    if value >= top {
        return;
    }
    let y = (graph.bottom - value / top * graph.height()).round() + 0.5;
    let mut paint = Paint::default();
    paint.set_color(GRID);
    paint.set_stroke_width(1.0);
    paint.set_path_effect(skia_safe::PathEffect::dash(&[3.0, 3.0], 0.0));
    canvas.draw_line((graph.left, y), (graph.right, y), &paint);
    text(canvas, graph.right + 6.0, y + 3.5, &[(label, MUTED)], font);
}
