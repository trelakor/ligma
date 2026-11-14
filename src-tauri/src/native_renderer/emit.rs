//! Dev tools (see `Painter::probe`): what a view draws, its paths, layers
//! and clips in device pixels, written out to time other renderers on

use skia_safe::{Canvas, Color4f, Matrix, Path, PathFillType, PathVerb, Point, Rect};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ON: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<State>> = Mutex::new(None);

struct State {
    out: Vec<u8>,
    /// The save count each open layer or clip ends below
    open: Vec<usize>,
    post: Matrix,
}

pub fn start() {
    *STATE.lock().unwrap() = Some(State { out: Vec::new(), open: Vec::new(), post: Matrix::default() });
    ON.store(true, Ordering::Relaxed);
}

/// What follows is a unit's, drawn with `post` after its own matrix
pub fn unit(post: Matrix) {
    if let Some(state) = STATE.lock().unwrap().as_mut() {
        close(state, usize::MAX);
        state.post = post;
    }
}

pub fn finish() -> Vec<u8> {
    ON.store(false, Ordering::Relaxed);
    let mut state = STATE.lock().unwrap().take();
    state.as_mut().map(|state| {
        close(state, usize::MAX);
        std::mem::take(&mut state.out)
    }).unwrap_or_default()
}

fn close(state: &mut State, count: usize) {
    while state.open.last().is_some_and(|&open| count < open || count == usize::MAX) {
        state.open.pop();
        state.out.push(b'X');
    }
}

fn with(canvas: &Canvas, f: impl FnOnce(&mut State, Matrix)) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    if let Some(state) = STATE.lock().unwrap().as_mut() {
        close(state, canvas.save_count());
        let m = Matrix::concat(&state.post, &canvas.local_to_device_as_3x3());
        f(state, m);
    }
}

fn rect(out: &mut Vec<u8>, r: Rect) {
    for v in [r.left, r.top, r.right, r.bottom] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn path_out(out: &mut Vec<u8>, path: &Path, m: &Matrix) {
    out.push((path.fill_type() == PathFillType::EvenOdd) as u8);
    let at = out.len();
    out.extend_from_slice(&0u32.to_le_bytes());
    let mut verbs = 0u32;
    let point = |out: &mut Vec<u8>, p: Point| {
        let p = m.map_point(p);
        out.extend_from_slice(&p.x.to_le_bytes());
        out.extend_from_slice(&p.y.to_le_bytes());
    };
    for rec in path.iter() {
        let points = rec.points();
        verbs += 1;
        match rec.verb() {
            PathVerb::Move => {
                out.push(0);
                point(out, points[0]);
            }
            PathVerb::Line => {
                out.push(1);
                point(out, points[1]);
            }
            PathVerb::Quad | PathVerb::Conic => {
                out.push(2);
                point(out, points[1]);
                point(out, points[2]);
            }
            PathVerb::Cubic => {
                out.push(3);
                point(out, points[1]);
                point(out, points[2]);
                point(out, points[3]);
            }
            PathVerb::Close => out.push(4),
        }
    }
    out[at..at + 4].copy_from_slice(&verbs.to_le_bytes());
}

fn color_out(out: &mut Vec<u8>, color: Color4f) {
    for v in [color.r, color.g, color.b, color.a] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// A layer just saved, over `bounds` where it has them
pub fn layer(canvas: &Canvas, bounds: Option<&Rect>) {
    with(canvas, |state, m| {
        let device = match bounds {
            Some(bounds) => m.map_rect(bounds).0,
            None => Matrix::concat(&state.post, &Matrix::default()).map_rect(Rect::from(canvas.device_clip_bounds().unwrap_or_default())).0,
        };
        state.out.push(b'L');
        rect(&mut state.out, device);
        state.open.push(canvas.save_count());
    });
}

/// A clip just made, as its bounds
pub fn clip(canvas: &Canvas) {
    with(canvas, |state, _| {
        let device = state.post.map_rect(Rect::from(canvas.device_clip_bounds().unwrap_or_default())).0;
        state.out.push(b'C');
        rect(&mut state.out, device);
        state.open.push(canvas.save_count());
    });
}

pub fn path(canvas: &Canvas, path: &Path, color: Color4f) {
    with(canvas, |state, m| {
        state.out.push(b'P');
        color_out(&mut state.out, color);
        path_out(&mut state.out, path, &m);
    });
}

pub fn fill_rect(canvas: &Canvas, r: &Rect, color: Color4f) {
    path(canvas, &Path::rect(*r, None), color);
}
