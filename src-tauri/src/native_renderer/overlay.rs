//! What Figma draws over its canvas, drawn by Ligma: selection, handles,
//! hover outlines, size labels, frame and section names, measurements,
//! scrollbars and the rest. Figma's main thread sends it each frame along
//! with the render tree, as the frame's `uiCommands`, which its render worker
//! draws over the scene. With Ligma's renderer on, Figma's worker gets no
//! frames, so Ligma draws both, in the same frame, as Figma's worker does
//!
//! `uiCommands` is a short header, then a run of commands: a `UICommandType`
//! varint, then that command's Kiwi message, `UI<Name>Command` in
//! `render.bkiwi` (`UICommand` has a field for each). Positions are in pane
//! units: a device pixel is `(point - viewPaneOffset) · zoomScale`, as the
//! last `SET_VIEW_PANE_OFFSET` and `SET_ZOOM_SCALE` set. Colors are a `Color`
//! of floats or a u32 of bytes R, G, B, A from the low byte up
//!
//! Each frame's commands are recorded into a picture once, as they come, and
//! the present thread draws that picture over the scene

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use skia_safe::{
    font_arguments::{variation_position::Coordinate, VariationPosition},
    paint, path_effect::PathEffect, Canvas, Color4f, Font, FontArguments, FontHinting, FontMgr, FourByteTag, Matrix, Paint, Path,
    PathBuilder, PathFillType, Picture, PictureRecorder, Point, RRect, Rect,
};

use super::rt::{Raw, Schema, Value};

/// A frame's overlay, ready to draw over the scene in device pixels
pub type Overlay = Option<Picture>;

/// Turns frames' commands into pictures, keeping what later frames refer
/// back to
pub struct Decoder {
    schema: &'static Schema,
    /// The message definition of each command type, by type
    messages: Vec<Option<u32>>,
    names: Vec<Arc<str>>,
    /// Section names, kept by snapshot, as a frame whose names haven't
    /// changed sends only the snapshot's hash
    sections: Option<(u64, Value)>,
    /// Commands and icons seen that aren't drawn yet, logged once each
    skipped: std::collections::HashSet<String>,
    failed: bool,
    /// The last frame's commands and its overlay: Figma sends the same again
    /// in frames where only what's under it changed, which is shown as is
    last: Option<(Vec<u8>, Picture)>,
}

impl Decoder {
    pub fn new() -> Option<Self> {
        let schema = Schema::bundled();
        let types = schema.def("UICommandType")?;
        let command = schema.def("UICommand")?;
        let count = types.fields.iter().map(|f| f.value as usize + 1).max().unwrap_or(0);
        let mut messages = vec![None; count];
        let mut names: Vec<Arc<str>> = vec![Arc::from(""); count];
        for t in &types.fields {
            let want = format!("{}command", t.name.replace('_', "").to_lowercase());
            names[t.value as usize] = t.name.clone();
            messages[t.value as usize] = command.fields.iter().find(|f| f.name.to_lowercase() == want).and_then(|f| match f.ty {
                super::rt::Type::Def(d) => Some(d),
                _ => None,
            });
        }
        Some(Decoder { schema, messages, names, sections: None, skipped: Default::default(), failed: false, last: None })
    }

    /// The overlay of a frame, from its `uiCommands`
    pub fn decode(&mut self, bytes: &[u8]) -> Overlay {
        if bytes.is_empty() {
            return None;
        }
        if let Some((_, picture)) = self.last.as_ref().filter(|(last, _)| last == bytes) {
            return Some(picture.clone());
        }
        let mut recorder = PictureRecorder::new();
        let canvas = recorder.begin_recording(Rect::from_xywh(-1e5, -1e5, 2e5, 2e5), false);
        let result = self.draw(bytes, canvas);
        if let Err(error) = result {
            if !self.failed {
                self.failed = true;
                log::warn!("native renderer: Figma's overlay couldn't be read: {error}");
            }
        }
        let picture = recorder.finish_recording_as_picture(None)?;
        self.last = Some((bytes.to_vec(), picture.clone()));
        Some(picture)
    }

    fn draw(&mut self, bytes: &[u8], canvas: &Canvas) -> Result<(), String> {
        let mut raw = Raw::new(self.schema, bytes);
        // Header: 3, 0, the count of commands, 0, 0, 2, 0, 3, the count again
        let (a, b) = (raw.byte()?, raw.byte()?);
        let count = raw.uint()?;
        let mut rest = [0u8; 5];
        for byte in &mut rest {
            *byte = raw.byte()?;
        }
        let again = raw.uint()?;
        if (a, b) != (3, 0) || rest != [0, 0, 2, 0, 3] || again != count {
            return Err(format!("unexpected header {:02x?}", &bytes[..bytes.len().min(12)]));
        }
        let mut state = State { zoom: 1.0, offset: (0.0, 0.0) };
        for _ in 0..count {
            if raw.done() {
                break;
            }
            let t = raw.uint()?;
            let Some(&message) = self.messages.get(t as usize) else {
                return Err(format!("unknown command {t} at byte {}", raw.at()));
            };
            let Some(message) = message else {
                continue;
            };
            let value = raw.value(message)?;
            let name = self.names[t as usize].clone();
            if !self.command(&name, &value, &mut state, canvas) {
                let what = match value.get("image").or(value.get("handles")) {
                    Some(which) => format!("{name} {}", which.str()),
                    None => name.to_string(),
                };
                if self.skipped.insert(what.clone()) {
                    log::info!("native renderer: Figma's overlay command {what} isn't drawn yet");
                }
            }
        }
        Ok(())
    }

    /// Draws one command, false if it's one not drawn
    fn command(&mut self, name: &str, v: &Value, state: &mut State, canvas: &Canvas) -> bool {
        canvas.save();
        canvas.concat(&state.matrix());
        let drawn = match name {
            "SET_ZOOM_SCALE" => {
                state.zoom = num(v, "zoomScale", 1.0);
                true
            }
            "SET_VIEW_PANE_OFFSET" => {
                state.offset = vector(v.get("viewPaneOffset"));
                true
            }
            "FLUSH" | "CLEAR" | "SET_VIEWPORT" | "FLUSH_SHIMMER" | "FLUSH_PROCESSING_SHIMMER_BAND" => true,
            "STROKE" | "STROKE_WITH_SHIMMER" => {
                let path = network(v.get("network"), false);
                canvas.draw_path(&path, &stroke(color(v.get("color")), num(v, "thickness", 1.0), true));
                true
            }
            "FILL" => {
                let path = network(v.get("network"), true);
                canvas.draw_path(&path, &fill(color(v.get("color")), true));
                true
            }
            "FILL_CONVEX_POLYGON" | "FILL_CONVEX_POLYGON_WITH_SHIMMER" => {
                canvas.draw_path(&polygon(v.get("points"), true), &fill(color(v.get("color")), true));
                true
            }
            "FILL_CONVEX_POLYGON_IGNORING_SKIRT" => {
                canvas.draw_path(&polygon(v.get("points"), true), &fill(color(v.get("color")), false));
                true
            }
            "STROKE_NON_OVERLAPPING_POLYLINE" | "STROKE_POLYLINE_WITH_SHIMMER" => {
                let closed = v.get("isClosed").and_then(Value::num).unwrap_or(0.0) != 0.0;
                let width = v.get("lineWidth").or(v.get("thickness")).and_then(Value::num).unwrap_or(1.0) as f32;
                let mut paint = stroke(color(v.get("color")), width, true);
                paint.set_stroke_join(paint::Join::Miter);
                paint.set_stroke_miter(num(v, "miterLimit", 4.0));
                canvas.draw_path(&polygon(v.get("points"), closed), &paint);
                true
            }
            "FILL_CIRCLE" | "FILL_CIRCLE_IGNORING_SKIRT" | "FILL_CIRCLE_WITH_SHIMMER" => {
                let aa = name != "FILL_CIRCLE_IGNORING_SKIRT";
                canvas.draw_circle(vector(v.get("center")), num(v, "radius", 0.0), &fill(color(v.get("color")), aa));
                true
            }
            "STROKE_CIRCLE" => {
                canvas.draw_circle(vector(v.get("center")), num(v, "radius", 0.0), &stroke(color(v.get("color")), num(v, "thickness", 1.0), true));
                true
            }
            "BATCH_FILL_AND_STROKE_CIRCLES" => {
                let radius = num(v, "radius", 0.0);
                let inside = fill(color(v.get("fillColor")), true);
                let outside = stroke(color(v.get("strokeColor")), num(v, "lineWidth", 1.0), true);
                for center in v.get("centers").map(Value::items).unwrap_or_default() {
                    let center = vector(Some(center));
                    canvas.draw_circle(center, radius, &inside);
                    canvas.draw_circle(center, radius, &outside);
                }
                true
            }
            "FILL_DIAMOND_IGNORING_SKIRT" | "STROKE_DIAMOND" => {
                let (x, y) = vector(v.get("center"));
                let r = num(v, "radius", 0.0);
                let mut path = PathBuilder::new();
                path.move_to((x, y - r)).line_to((x + r, y)).line_to((x, y + r)).line_to((x - r, y)).close();
                let path = path.detach();
                let paint = if name == "STROKE_DIAMOND" { stroke(color(v.get("color")), num(v, "lineWidth", 1.0), true) } else { fill(color(v.get("color")), false) };
                canvas.draw_path(&path, &paint);
                true
            }
            "STROKE_LINE_WITHOUT_CAPS" => {
                let paint = stroke(color(v.get("color")), num(v, "lineWidth", 1.0), true);
                canvas.draw_line(vector(v.get("start")), vector(v.get("end")), &paint);
                true
            }
            "STROKE_CUBIC" => {
                let mut path = PathBuilder::new();
                path.move_to(vector(v.get("p0"))).cubic_to(vector(v.get("p1")), vector(v.get("p2")), vector(v.get("p3")));
                canvas.draw_path(&path.detach(), &stroke(color(v.get("color")), num(v, "thickness", 1.0), true));
                true
            }
            "FILL_TEXT" => {
                let font = font_of(v, "font");
                let paint = fill(color(v.get("color")), true);
                if let Some(matrix) = v.get("matrix").map(Value::items).and_then(|m| m.first()) {
                    canvas.concat(&affine(Some(matrix)));
                }
                if let Some(font) = font {
                    // `point` is the top of the text, its em box fit between
                    // the font's ascent and descent, as frame names sit
                    let (_, metrics) = font.metrics();
                    let extent = -metrics.ascent + metrics.descent;
                    let drop = if extent > 0.0 { font.size() * -metrics.ascent / extent } else { font.size() * 0.8 };
                    let (x, y) = vector(v.get("point"));
                    canvas.draw_str(v.get("text").map(Value::str).unwrap_or_default(), (x, y + drop), &font, &paint);
                }
                true
            }
            "FILL_TEXT_WITH_BOX" => {
                text_with_box(v, canvas);
                true
            }
            "RENDER_FILL_AND_STROKE_ROUNDED_RECT" => {
                let r = rect(v.get("r"));
                let radius = num(v, "radius", 0.0);
                let angle = num(v, "angleInRadians", 0.0);
                if angle != 0.0 {
                    canvas.rotate(angle.to_degrees(), Some(Point::new(r.center_x(), r.center_y())));
                }
                canvas.draw_rrect(RRect::new_rect_xy(r, radius, radius), &fill(color(v.get("fillColor")), true));
                let thickness = num(v, "thickness", 0.0);
                if thickness > 0.0 {
                    // Inside, center or outside the rectangle
                    let out = match v.get("align").and_then(Value::num).unwrap_or(0.0) as u32 {
                        1 => -thickness / 2.0,
                        2 => thickness / 2.0,
                        _ => 0.0,
                    };
                    let r = r.with_outset((out, out));
                    let radius = (radius + out).max(0.0);
                    canvas.draw_rrect(RRect::new_rect_xy(r, radius, radius), &stroke(color(v.get("strokeColor")), thickness, true));
                }
                true
            }
            "RENDER_DASHED_LINES" => {
                let points = v.get("points").map(Value::items).unwrap_or_default();
                let thickness = num(v, "thickness", 1.0);
                let effect = PathEffect::dash(&[num(v, "dash", 4.0), num(v, "gap", 4.0)], 0.0);
                for pair in points.chunks(2) {
                    let [from, to] = pair else { continue };
                    let mut paint = stroke(color(from.get("color")), thickness, true);
                    paint.set_path_effect(effect.clone());
                    canvas.draw_line(xy(from), xy(to), &paint);
                }
                true
            }
            "RENDER_SQUIGGLE_LINES" => {
                let points = v.get("points").map(Value::items).unwrap_or_default();
                let size = num(v, "fontSize", 12.0) * num(v, "scale", 1.0).max(1e-3);
                let paint = stroke(color(v.get("color")), (size / 14.0).max(1.0), true);
                for pair in points.chunks(2) {
                    let [from, to] = pair else { continue };
                    canvas.draw_path(&squiggle(xy(from), xy(to), size / 8.0), &paint);
                }
                true
            }
            "RENDER_SOLID_LOOP_BLINN_NETWORK" => {
                canvas.concat(&affine(v.get("transform")));
                canvas.draw_path(&network(v.get("network"), true), &fill(color(v.get("color")), true));
                true
            }
            "RENDER_INVERSE_FILL_REGION" => {
                canvas.concat(&affine(v.get("transform")));
                let mut path = PathBuilder::new_path(&network(v.get("network"), true));
                path.set_fill_type(PathFillType::EvenOdd);
                path.add_rect(rect(v.get("viewportBounds")), None, None);
                canvas.draw_path(&path.detach(), &fill(color(v.get("color")), true));
                true
            }
            "RENDER_SHADED_RECTANGLE" => {
                canvas.concat(&affine(v.get("transform")));
                shade(canvas, &Path::rect(rect(v.get("rectangle")), None), v);
                true
            }
            "RENDER_SHADED_REGIONS" | "RENDER_SHADING_NETWORK" => {
                canvas.concat(&affine(v.get("transform")));
                let path = network(v.get("network").or(v.get("vectorNetwork")), true);
                shade(canvas, &path, v);
                true
            }
            "RENDER_TLF_CANVAS_HIGHLIGHT_BOX" | "RENDER_CANVAS_GRID_CHILD_HOVER_BOX" => {
                let path = polygon(v.get("offsetPoints"), true);
                canvas.draw_path(&path, &fill(color(v.get("fillColor")), true));
                canvas.draw_path(&path, &stroke(color(v.get("strokeColor")), num(v, "lineWidth", 1.0).max(1.0), true));
                true
            }
            "RENDER_GRADIENT_COLOR_STOP" => {
                let at = v.get("result").map(|r| vector(r.get("location"))).unwrap_or_default();
                canvas.draw_circle(at, 7.0, &fill(Color4f::new(1.0, 1.0, 1.0, 1.0), true));
                canvas.draw_circle(at, 5.0, &fill(color(v.get("color")), true));
                canvas.draw_circle(at, 7.0, &stroke(Color4f::new(0.0, 0.0, 0.0, 0.2), 1.0, true));
                true
            }
            "RENDER_SECTION_NAMES" => {
                let hash = v.get("snapshotHash").and_then(Value::num).map(|h| h as u64).unwrap_or(0);
                let elided = v.get("elided").and_then(Value::num).unwrap_or(0.0) != 0.0;
                if !elided {
                    self.sections = Some((hash, v.clone()));
                }
                if let Some((_, sections)) = self.sections.as_ref().filter(|(kept, _)| !elided || *kept == hash) {
                    section_names(sections, canvas);
                }
                true
            }
            "RENDER_ORIENTED_IMAGE" | "RENDER_ORIENTED_TINTED_IMAGE" | "RENDER_CIRCULAR_IMAGE" => {
                let image = v.get("image").map(Value::str).unwrap_or_default();
                canvas.concat(&affine(v.get("transform")));
                let tint = v.get("tint").map(|tint| color(Some(tint)));
                let opacity = v.get("opacity").and_then(Value::num).unwrap_or(1.0) as f32;
                super::overlay_icons::draw(canvas, image, tint, opacity)
            }
            _ => false,
        };
        canvas.restore();
        drawn
    }
}

struct State {
    zoom: f32,
    offset: (f32, f32),
}

impl State {
    fn matrix(&self) -> Matrix {
        let mut m = Matrix::scale((self.zoom, self.zoom));
        m.pre_translate((-self.offset.0, -self.offset.1));
        m
    }
}

fn num(v: &Value, name: &str, default: f32) -> f32 {
    v.get(name).and_then(Value::num).map_or(default, |n| n as f32)
}

fn xy(v: &Value) -> (f32, f32) {
    (num(v, "x", 0.0), num(v, "y", 0.0))
}

fn vector(v: Option<&Value>) -> (f32, f32) {
    v.map(xy).unwrap_or_default()
}

fn rect(v: Option<&Value>) -> Rect {
    let Some(v) = v else { return Rect::new_empty() };
    Rect::from_xywh(num(v, "x", 0.0), num(v, "y", 0.0), num(v, "w", 0.0), num(v, "h", 0.0))
}

fn affine(v: Option<&Value>) -> Matrix {
    let Some(v) = v else { return Matrix::new_identity() };
    Matrix::new_all(num(v, "m00", 1.0), num(v, "m01", 0.0), num(v, "m02", 0.0), num(v, "m10", 0.0), num(v, "m11", 1.0), num(v, "m12", 0.0), 0.0, 0.0, 1.0)
}

/// A `Color` of floats, or a u32 of bytes R, G, B, A from the low byte up
fn color(v: Option<&Value>) -> Color4f {
    match v {
        Some(v @ Value::Record(_)) => Color4f::new(num(v, "r", 0.0), num(v, "g", 0.0), num(v, "b", 0.0), num(v, "a", 1.0)),
        Some(v) => {
            let c = v.num().unwrap_or(0.0) as u32;
            let byte = |shift: u32| ((c >> shift) & 0xff) as f32 / 255.0;
            Color4f::new(byte(0), byte(8), byte(16), byte(24))
        }
        None => Color4f::new(0.0, 0.0, 0.0, 1.0),
    }
}

fn fill(color: Color4f, aa: bool) -> Paint {
    let mut paint = Paint::new(color, None);
    paint.set_anti_alias(aa);
    paint
}

fn stroke(color: Color4f, width: f32, aa: bool) -> Paint {
    let mut paint = fill(color, aa);
    paint.set_style(paint::Style::Stroke);
    paint.set_stroke_width(width);
    paint
}

fn polygon(points: Option<&Value>, close: bool) -> Path {
    let points = points.map(Value::items).unwrap_or_default();
    let mut path = PathBuilder::new();
    for (i, point) in points.iter().enumerate() {
        if i == 0 {
            path.move_to(xy(point));
        } else {
            path.line_to(xy(point));
        }
    }
    if close && points.len() > 2 {
        path.close();
    }
    path.detach()
}

/// A vector network as a path: its regions' loops, closed, or for one
/// without regions, or to stroke, its segments in runs
fn network(v: Option<&Value>, filled: bool) -> Path {
    let mut path = PathBuilder::new();
    let Some(v) = v else { return path.detach() };
    let vertices: Vec<Point> = v.get("vertices").map(Value::items).unwrap_or_default().iter().map(|vertex| fields(vertex).first().map(|p| xy(p).into()).unwrap_or_default()).collect();
    // Each segment: its start and end, a handle each of the tangent and the
    // vertex's index
    let segments: Vec<(usize, Point, usize, Point)> = v
        .get("segments")
        .map(Value::items)
        .unwrap_or_default()
        .iter()
        .map(|segment| {
            let parts = fields(segment);
            let handle = |i: usize| {
                let h = parts.get(i).map(|h| fields(h)).unwrap_or_default();
                let tangent: Point = h.first().map(|t| xy(t).into()).unwrap_or_default();
                let index = h.get(1).and_then(|i| i.num()).unwrap_or(0.0) as usize;
                (index, tangent)
            };
            let ((a, ta), (b, tb)) = (handle(0), handle(1));
            (a, ta, b, tb)
        })
        .collect();
    let at = |i: usize| vertices.get(i).copied().unwrap_or_default();
    let segment = |path: &mut PathBuilder, (a, ta, b, tb): (usize, Point, usize, Point)| {
        if ta == Point::default() && tb == Point::default() {
            path.line_to(at(b));
        } else {
            path.cubic_to(at(a) + ta, at(b) + tb, at(b));
        }
    };
    let regions = v.get("regions").map(Value::items).unwrap_or_default();
    if !regions.is_empty() {
        for region in regions {
            if region.get("windingRule").and_then(Value::num) == Some(1.0) {
                path.set_fill_type(PathFillType::EvenOdd);
            }
            for lp in region.get("loops").map(Value::items).unwrap_or_default() {
                let indices: Vec<usize> = lp.get("segments").map(Value::items).unwrap_or_default().iter().filter_map(|i| i.num()).map(|i| i as usize).collect();
                let mut first = true;
                for &i in &indices {
                    let Some(&s) = segments.get(i) else { continue };
                    if first {
                        path.move_to(at(s.0));
                        first = false;
                    }
                    segment(&mut path, s);
                }
                if !first {
                    path.close();
                }
            }
        }
        return path.detach();
    }
    // Runs of segments that follow on from each other
    let mut last = usize::MAX;
    for &s in &segments {
        if s.0 != last {
            path.move_to(at(s.0));
        }
        segment(&mut path, s);
        last = s.2;
    }
    if filled {
        path.close();
    }
    path.detach()
}

/// The values of a record, in wire order
fn fields(v: &Value) -> Vec<&Value> {
    match v {
        Value::Record(fields) => fields.iter().map(|(_, v)| v).collect(),
        _ => Vec::new(),
    }
}

fn squiggle(from: (f32, f32), to: (f32, f32), amplitude: f32) -> Path {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    let mut path = PathBuilder::new();
    if length <= 0.0 {
        return path.detach();
    }
    let (ux, uy) = (dx / length, dy / length);
    let step = amplitude * 2.0;
    let mut t = 0.0;
    let mut up = true;
    path.move_to(from);
    while t < length {
        let next = (t + step).min(length);
        let mid = (t + next) / 2.0;
        let side = if up { -amplitude } else { amplitude };
        path.quad_to((from.0 + ux * mid - uy * side, from.1 + uy * mid + ux * side), (from.0 + ux * next, from.1 + uy * next));
        t = next;
        up = !up;
    }
    path.detach()
}

/// Diagonal lines `spacing` apart over `path`
fn shade(canvas: &Canvas, path: &Path, v: &Value) {
    let spacing = num(v, "spacing", 6.0).max(1.0);
    let flip = v.get("flipDirection").and_then(Value::num).unwrap_or(0.0) != 0.0;
    let bounds = path.bounds();
    canvas.save();
    canvas.clip_path(path, None, true);
    let paint = stroke(color(v.get("color")), 1.0, true);
    let size = bounds.width() + bounds.height();
    let mut d = -bounds.height();
    while d < bounds.width() + spacing {
        let (x0, x1) = (bounds.left + d, bounds.left + d + bounds.height());
        let (a, b) = if flip { ((x0, bounds.bottom), (x1, bounds.top)) } else { ((x0, bounds.top), (x1, bounds.bottom)) };
        canvas.draw_line(a, b, &paint);
        d += spacing;
        if d > size * 4.0 {
            break;
        }
    }
    canvas.restore();
}

/// Text with a rounded box behind it, as Figma's size labels: the box hugs
/// the text, padded, placed by `alignX` and `alignY` at `point`
fn text_with_box(v: &Value, canvas: &Canvas) {
    let Some(font) = font_of(v, "font") else { return };
    let text = v.get("text").map(Value::str).unwrap_or_default();
    let (x, y) = vector(v.get("point"));
    let (width, _) = font.measure_str(text, None);
    let (_, metrics) = font.metrics();
    let height = num(v, "fontPixelLineHeight", 16.0);
    let (px, py) = vector(v.get("padding"));
    let px = if px > 0.0 { px } else { 4.0 };
    let (bw, bh) = (width + px * 2.0, height + py * 2.0);
    let left = match v.get("alignX").and_then(Value::num).unwrap_or(0.0) as u32 {
        1 => x - bw / 2.0,
        2 => x - bw,
        _ => x,
    };
    let top = match v.get("alignY").and_then(Value::num).unwrap_or(0.0) as u32 {
        1 => y - bh / 2.0,
        2 => y - bh,
        _ => y,
    };
    let angle = num(v, "angleInRadians", 0.0);
    if angle != 0.0 {
        canvas.rotate(angle.to_degrees(), Some(Point::new(x, y)));
    }
    let r = Rect::from_xywh(left, top, bw, bh);
    let radius = num(v, "borderRadius", 2.0);
    canvas.draw_rrect(RRect::new_rect_xy(r, radius, radius), &fill(color(v.get("bkColor")), true));
    // Centered in the box by the font's ascent and descent
    let baseline = top + bh / 2.0 + (-metrics.ascent - metrics.descent) / 2.0;
    canvas.draw_str(text, (left + px, baseline), &font, &fill(color(v.get("color")), true));
}

/// Figma's section name pills, a level at a time
fn section_names(v: &Value, canvas: &Canvas) {
    for level in v.get("levels").map(Value::items).unwrap_or_default() {
        for s in level.get("sections").map(Value::items).unwrap_or_default() {
            let alpha = num(s, "alpha", 255.0) / 255.0;
            let with = |c: Color4f| Color4f::new(c.r, c.g, c.b, c.a * alpha);
            let pill = rect(s.get("pillRect"));
            let radius = num(s, "cornerRadius", 0.0);
            canvas.draw_rrect(RRect::new_rect_xy(pill, radius, radius), &fill(with(color(s.get("pillBackgroundColor"))), true));
            let border = num(s, "borderThickness", 0.0);
            if border > 0.0 {
                canvas.draw_rrect(RRect::new_rect_xy(pill, radius, radius), &stroke(with(color(s.get("pillBorderColor"))), border, true));
            }
            let font = font_named(
                s.get("fontName").map(Value::str).unwrap_or("Inter"),
                num(s, "fontPixelSize", 11.0),
                num(s, "fontAttributes", 0.0) as u32,
            );
            let Some(font) = font else { continue };
            let text_paint = fill(with(color(s.get("textColor"))), true);
            let runs = s.get("highlightNormalTextRuns").map(Value::items).unwrap_or_default();
            if num(s, "useSearchHighlights", 0.0) != 0.0 && !runs.is_empty() {
                for h in s.get("highlightRects").map(Value::items).unwrap_or_default() {
                    let r = num(h, "cornerRadius", 0.0);
                    canvas.draw_rrect(RRect::new_rect_xy(rect(h.get("rect")), r, r), &fill(with(color(h.get("fill"))), true));
                }
                for run in runs.iter().chain(s.get("highlightMatchedTextRuns").map(Value::items).unwrap_or_default()) {
                    canvas.draw_str(run.get("text").map(Value::str).unwrap_or_default(), vector(run.get("origin")), &font, &fill(with(color(run.get("color"))), true));
                }
            } else {
                canvas.draw_str(s.get("text").map(Value::str).unwrap_or_default(), vector(s.get("textOrigin")), &font, &text_paint);
            }
            if num(s, "renderStatus", 0.0) != 0.0 {
                let r = rect(s.get("statusPillRect"));
                let h = r.height() / 2.0;
                canvas.draw_rrect(RRect::new_rect_xy(r, h, h), &fill(with(color(s.get("statusPillBackgroundColor"))), true));
                canvas.draw_str(s.get("statusText").map(Value::str).unwrap_or_default(), vector(s.get("statusTextOrigin")), &font, &fill(with(color(s.get("statusTextColor"))), true));
            }
        }
    }
}

// ---------------------------------------------------------------- fonts

/// Figma's interface fonts, as its render worker loads them (`fonts` of the
/// worker's `init`): Inter, a variable font
static FONTS: LazyLock<Mutex<HashMap<String, Faces>>> = LazyLock::new(Default::default);

struct Faces {
    base: skia_safe::Typeface,
    by_weight: HashMap<u32, skia_safe::Typeface>,
}

/// Takes a font file, WOFF2 or not, for `family`
pub fn add_font(family: &str, bytes: &[u8]) -> Result<(), String> {
    let bytes = if bytes.starts_with(b"wOF2") { wuff::decompress_woff2(bytes).map_err(|error| format!("{error:?}"))? } else { bytes.to_vec() };
    let typeface = FontMgr::new().new_from_data(skia_safe::Data::new_copy(&bytes), None).ok_or("not a font")?;
    FONTS.lock().unwrap().insert(family.to_string(), Faces { base: typeface, by_weight: HashMap::new() });
    Ok(())
}

/// Figma's text attributes: bold, italic, medium
fn weight(attributes: u32) -> u32 {
    if attributes & 4 != 0 {
        500
    } else if attributes & 1 != 0 {
        700
    } else {
        400
    }
}

fn font_named(family: &str, size: f32, attributes: u32) -> Option<Font> {
    let weight = weight(attributes);
    let mut fonts = FONTS.lock().unwrap();
    let faces = fonts.get_mut(family).or(None)?;
    let typeface = match faces.by_weight.get(&weight) {
        Some(typeface) => typeface.clone(),
        None => {
            let coordinates = [Coordinate { axis: FourByteTag::from_chars('w', 'g', 'h', 't'), value: weight as f32 }];
            let arguments = FontArguments::new().set_variation_design_position(VariationPosition { coordinates: &coordinates });
            let typeface = faces.base.clone_with_arguments(&arguments).unwrap_or_else(|| faces.base.clone());
            faces.by_weight.insert(weight, typeface.clone());
            typeface
        }
    };
    drop(fonts);
    let mut font = Font::from_typeface(typeface, size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font.set_subpixel(true);
    font.set_hinting(FontHinting::None);
    if attributes & 2 != 0 {
        font.set_skew_x(-0.2);
    }
    Some(font)
}

fn font_of(v: &Value, field: &str) -> Option<Font> {
    let family = v.get(field).map(Value::str).filter(|f| !f.is_empty()).unwrap_or("Inter");
    font_named(family, num(v, "fontPixelSize", 11.0), num(v, "fontAttributes", 0.0) as u32).or_else(|| font_named("Inter", num(v, "fontPixelSize", 11.0), num(v, "fontAttributes", 0.0) as u32))
}
