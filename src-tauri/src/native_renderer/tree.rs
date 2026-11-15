//! Draws Figma's own scene: the render tree its main thread sends its render
//! worker every frame (see rt.rs), handed over by page_hook.js as each frame
//! goes. Unlike the plugin API's scene, it's every file Figma can show, the
//! moment it changes, already laid out: text as outlines, strokes widened,
//! effects as the steps Figma's renderer takes
//!
//! `Tree` keeps the scene and what's worked out from it once, as nodes never
//! change: a new node replaces one that would, and its parents up to a new
//! root. `Tree::units` splits the scene into the units tiles are drawn from,
//! `Drawer` draws a node and what's under it

use std::sync::{Mutex, RwLock};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde::Deserialize;
use skia_safe::canvas::{SaveLayerFlags, SaveLayerRec};
use skia_safe::gradient::{shaders as gradients, Colors, Gradient, Interpolation};
use skia_safe::{
    color_filters, image_filters, BlendMode, Canvas, ClipOp, Color4f, ColorChannel, Data, FilterMode, Matrix, MipmapMode, Paint, Path, Picture,
    PictureRecorder, Point, Rect, SamplingOptions, TileMode,
};

use super::adjust::Adjust;
use super::draw::{self, Counts, Img, Kit, MAX_SIGMA};
use super::rt::{self, Forest, IdMap, MaskOpType, Node, Op, PaintType, Schema};
use super::scene::Rgba;

/// Dev tools (see `Painter::probe`): what the drawer leaves out, to time
/// what each kind of thing costs. 1 masks drawn as their content alone, 2
/// blends and opacity without layers, 4 blurs left out, 8 silhouettes left
/// out, 16 paths drawn without smoothed edges, 32 no paths drawn
pub static SKIP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Dev tools: masks drawn as layers and as clips, blend layers, blur
/// layers, silhouette layers, paths drawn and their points
pub static COUNTED: [std::sync::atomic::AtomicU64; 7] = [const { std::sync::atomic::AtomicU64::new(0) }; 7];

/// `path` with no more detail than `tolerance` shows: segments shorter
/// than it, and curves within it of a line, as lines, or left out
fn coarse(path: &Path, tolerance: f32) -> Path {
    use skia_safe::PathVerb;
    let mut out = skia_safe::PathBuilder::new();
    out.set_fill_type(path.fill_type());
    let mut last = Point::default();
    let far = |a: Point, b: Point| (a.x - b.x).abs().max((a.y - b.y).abs()) >= tolerance;
    for rec in path.iter() {
        let points = rec.points();
        match rec.verb() {
            PathVerb::Move => {
                last = points[0];
                out.move_to(last);
            }
            PathVerb::Close => {
                out.close();
            }
            verb => {
                let end = *points.last().unwrap_or(&last);
                let (mut left, mut top, mut right, mut bottom) = (end.x, end.y, end.x, end.y);
                for point in points {
                    left = left.min(point.x);
                    top = top.min(point.y);
                    right = right.max(point.x);
                    bottom = bottom.max(point.y);
                }
                let small = right - left < tolerance * 2.0 && bottom - top < tolerance * 2.0;
                if verb == PathVerb::Line || small {
                    if far(end, last) {
                        out.line_to(end);
                        last = end;
                    }
                    continue;
                }
                match verb {
                    PathVerb::Quad => out.quad_to(points[1], points[2]),
                    PathVerb::Conic => out.conic_to(points[1], points[2], rec.conic_weight()),
                    _ => out.cubic_to(points[1], points[2], points[3]),
                };
                last = end;
            }
        }
    }
    out.detach()
}

/// Dev tools: nodes under this many device pixels left out, and layers
/// under this many drawn without their layer, as f32 bits
pub static SMALL: [std::sync::atomic::AtomicU32; 2] = [const { std::sync::atomic::AtomicU32::new(0) }; 2];

fn small(which: usize) -> f32 {
    f32::from_bits(SMALL[which].load(std::sync::atomic::Ordering::Relaxed))
}

fn skip(bit: u32) -> bool {
    SKIP.load(std::sync::atomic::Ordering::Relaxed) & bit != 0
}

fn count(what: usize, n: u64) {
    COUNTED[what].fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

/// What page_hook.js sends with each frame, ahead of its forest
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Header {
    pub session: String,
    pub seq: u64,
    /// 0 for a whole scene, 1 for what changed
    pub mode: u32,
    pub root: u32,
    #[serde(default)]
    pub released: Vec<u32>,
    /// Figma's canvas to device pixels: `pixel = point · scale + offset`
    pub ox: f64,
    pub oy: f64,
    pub scale: f64,
    pub dpr: f64,
    /// What Figma clears its canvas to, straight RGBA, 0 to 1
    #[serde(default)]
    pub bg: [f32; 4],
    /// 0 none, 1 Figma's light checkerboard, 2 its dot grid, 3 its dark one
    #[serde(default)]
    pub checker: u32,
    #[serde(default)]
    pub grid: bool,
    /// Pixels a unit Figma's pixel preview shows, or 0
    #[serde(default)]
    pub preview: f64,
    /// Outline mode's background, 0xRRGGBB, or -1
    #[serde(default = "no_outline")]
    pub outline: f64,
    /// Figma's canvas in the tab, in CSS pixels
    pub cx: f64,
    pub cy: f64,
    pub cw: f64,
    pub ch: f64,
    #[serde(default)]
    pub visible: bool,
    /// When the page sent it, in ms since 1970
    #[serde(default)]
    pub at: f64,
    /// Bytes of what Figma draws over the canvas, after the forest
    #[serde(default)]
    pub ui: usize,
}

fn no_outline() -> f64 {
    -1.0
}

/// A frame as the page sends it: a little-endian u32 length, that much
/// JSON (`Header`), the forest, then what Figma draws over the canvas
/// (`uiCommands`, see overlay.rs)
pub struct Frame {
    pub header: Header,
    body: Vec<u8>,
    start: usize,
}

impl Frame {
    pub fn parse(body: Vec<u8>) -> Result<Frame, String> {
        let length = u32::from_le_bytes(body.get(..4).ok_or("no header")?.try_into().unwrap()) as usize;
        let end = 4usize.checked_add(length).filter(|&end| end <= body.len()).ok_or("header past the end")?;
        let header: Header = serde_json::from_slice(&body[4..end]).map_err(|error| error.to_string())?;
        if header.ui > body.len() - end {
            return Err("overlay past the end".into());
        }
        Ok(Frame { header, body, start: end })
    }

    pub fn forest(&self) -> &[u8] {
        &self.body[self.start..self.body.len() - self.header.ui]
    }

    pub fn ui(&self) -> &[u8] {
        &self.body[self.body.len() - self.header.ui..]
    }

    /// Whether the frame changes the scene, rather than only what's over
    /// it or the view: a whole scene, nodes let go, or more than a reference
    /// to the root as it was
    pub fn edits(&self) -> bool {
        self.header.mode == 0 || !self.header.released.is_empty() || self.forest().len() > 16
    }

    /// The view Figma drew this frame at
    pub fn view(&self) -> super::render::View {
        let h = &self.header;
        let scale = h.scale.max(1e-9);
        let mut view = super::render::View {
            x: -h.ox / scale,
            y: -h.oy / scale,
            w: 0.0,
            h: 0.0,
            zoom: scale / h.dpr.max(1e-9),
            cx: h.cx,
            cy: h.cy,
            cw: h.cw,
            ch: h.ch,
            dpr: h.dpr,
            page: String::new(),
            visible: h.visible,
            preview: h.preview,
            grid: h.grid,
            outline: h.outline,
        };
        // Over the canvas's pixels, so a pixel is `scale` a unit exactly
        let (w, h) = view.canvas();
        view.w = w as f64 / scale;
        view.h = h as f64 / scale;
        view
    }
}

/// What's worked out once for a node and what's under it
#[derive(Clone, Copy)]
pub(super) struct Info {
    /// Where it draws, in the space it's drawn in
    pub(super) bounds: Option<Rect>,
    /// Nodes under it, itself included, counting shared ones each time
    count: u32,
    /// Draws with a blend mode that reaches what's drawn before it
    pub(super) blends: bool,
    /// Has a custom effect, drawn as Figma's canvas draws it at its zoom
    custom: bool,
    /// Has something only drawn close to Figma's look
    approximate: Option<&'static str>,
}

/// A unit tiles are drawn from: node `id` drawn with `at`, inside `clips`
#[derive(Clone)]
pub struct Place {
    pub id: u32,
    pub at: Matrix,
    /// Fill nodes whose outline clips it, each with where it's drawn
    pub clips: Vec<(u32, Matrix)>,
}

pub struct TreeUnit {
    /// Stays the same while the unit does, as its node never changes
    pub key: usize,
    pub place: Place,
    pub bounds: Rect,
    pub blends: bool,
    pub custom: bool,
}

/// A custom effect node's effects, as the effect host runs them
pub struct CustomEffect {
    /// What they draw goes by this, which stays the same while the node does
    pub key: String,
    /// The Figma node they're on
    pub node: String,
    /// Each effect's code by name, and its settings
    pub chain: Vec<(String, serde_json::Value)>,
    /// The box they run over and draw, in the node's own space
    pub area: Rect,
    /// From the node's own space to the space the custom effect node is in
    pub to: Matrix,
    /// What they run on
    pub child: u32,
}

/// Subtrees with more nodes than this are split into units of their own
/// children where that draws the same, so a change redraws less
const SPLIT: u32 = 1500;

/// Feeds what's written to a hasher
struct HashWrite<'a>(&'a mut std::collections::hash_map::DefaultHasher);

impl std::fmt::Write for HashWrite<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

/// A hash of a path's outline: Figma's own, when it sends one
fn path_hash(path: &rt::Path) -> u64 {
    if let Some(hash) = path.content_hash {
        return hash;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let _ = std::fmt::Write::write_fmt(&mut HashWrite(&mut hasher), format_args!("{path:?}"));
    hasher.finish()
}

/// Nested nodes drawn at most, against a tree that refers to itself
pub(super) const DEPTH: u8 = 96;

pub struct Tree {
    pub forest: Forest,
    schema: &'static Schema,
    pub root: Option<u32>,
    pub header: Option<Header>,
    info: RwLock<IdMap<u32, Info>>,
    /// A hash of what each node draws, with all under it, by which units
    /// are known: Figma sends a node again under another id, unchanged, and
    /// an id again for another node
    contents: IdMap<u32, u64>,
    /// Skia's copy of each path, by the path's address, kept with it
    paths: Mutex<HashMap<usize, (Arc<rt::Path>, Path)>>,
    /// Pattern tiles: by source, tiling and zoom level
    patterns: Mutex<HashMap<(u32, u64, i32), (Picture, Vec<String>)>>,
    /// Figma's noise octaves, see `scene::Begin`
    pub octaves: Vec<f32>,
}

impl Tree {
    pub fn new(schema: &'static Schema) -> Self {
        Tree {
            forest: Forest::new(),
            schema,
            root: None,
            header: None,
            info: RwLock::default(),
            contents: IdMap::default(),
            paths: Mutex::default(),
            patterns: Mutex::default(),
            octaves: Vec::new(),
        }
    }

    /// Applies a frame. On an error the scene is as it was, and Figma has to
    /// send it whole again
    pub fn apply(&mut self, frame: &Frame) -> Result<rt::Applied, String> {
        let header = &frame.header;
        if header.mode != 0 && self.root.is_none() {
            return Err("a change before the whole scene".into());
        }
        // What a whole scene's nodes were, to know those sent again the same
        let previous = (header.mode == 0).then(|| self.forest.nodes.clone());
        let applied = self.forest.apply(frame.forest(), header.mode, &header.released, self.schema)?;
        if !self.forest.nodes.contains_key(&header.root) {
            self.root = None;
            return Err(format!("root {} isn't there", header.root));
        }
        let mut info = self.info.write().unwrap();
        if header.mode == 0 {
            info.clear();
        } else {
            for id in &header.released {
                info.remove(id);
            }
        }
        drop(info);
        self.renew(&applied, previous.as_ref(), &header.released);
        if header.mode == 0 || !header.released.is_empty() {
            self.paths.lock().unwrap().retain(|_, (path, _)| Arc::strong_count(path) > 1);
            self.patterns.lock().unwrap().clear();
        } else if !applied.new_nodes.is_empty() {
            // A pattern's source can change
            self.patterns.lock().unwrap().clear();
        }
        self.root = Some(header.root);
        self.header = Some(header.clone());
        Ok(applied)
    }

    /// Hashes what the frame's new nodes draw, each after those it refers
    /// to, as they come. Of a whole scene, a node the same as the one its
    /// id was, over nodes all the same too, keeps its hash, which is most
    /// of them and far quicker than hashing them again
    fn renew(&mut self, applied: &rt::Applied, previous: Option<&IdMap<u32, Arc<Node>>>, released: &[u32]) {
        let before = match previous {
            Some(_) => std::mem::take(&mut self.contents),
            None => {
                for id in released {
                    self.contents.remove(id);
                }
                IdMap::default()
            }
        };
        for &id in &applied.new_nodes {
            let Some(node) = self.forest.get(id) else { continue };
            let kept = previous
                .and_then(|previous| previous.get(&id))
                .filter(|old| ***old == **node)
                .and_then(|_| before.get(&id))
                .filter(|_| node.refs().all(|r| before.get(&r) == self.contents.get(&r)));
            let content = match kept {
                Some(&content) => content,
                None => self.hash_node(node),
            };
            self.contents.insert(id, content);
        }
    }

    /// A hash of what node `id` draws, see `contents`
    pub fn content(&self, id: u32) -> u64 {
        self.contents.get(&id).copied().unwrap_or(0)
    }

    fn hash_node(&self, node: &Node) -> u64 {
        use std::fmt::Write;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // Each field, so one added to `Node` has to be thought of here
        let Node {
            id,
            op,
            children,
            child,
            transform,
            paint_size,
            geometry,
            fill_geometries,
            stroke_geometries,
            stroke_mode,
            bg_blur_expansion_for_frame_fill,
            is_created_from_brush,
            blend_mode,
            opacity,
            alpha_node,
            color_node,
            mask_op,
            color,
            radius,
            ext,
            extra,
        } = node;
        // Custom effects are run and kept by node
        if matches!(op, Op::CustomEffect) {
            id.hash(&mut hasher);
        }
        let _ = write!(
            HashWrite(&mut hasher),
            "{op:?}{transform:?}{paint_size:?}{stroke_mode:?}{bg_blur_expansion_for_frame_fill:?}{is_created_from_brush:?}{blend_mode:?}{opacity:?}{mask_op:?}{color:?}{radius:?}{ext:?}{extra:?}"
        );
        for geometry in geometry.iter().chain(fill_geometries).chain(stroke_geometries) {
            let rt::Geometry { path, paints, glyph_batch, glyph_metadata, extra } = geometry;
            path.as_deref().map(path_hash).hash(&mut hasher);
            let _ = write!(HashWrite(&mut hasher), "{paints:?}{glyph_metadata:?}{extra:?}");
            if let Some(batch) = glyph_batch {
                let rt::GlyphBatch { glyphs, decoration_paths, should_round_y, extra } = &**batch;
                let _ = write!(HashWrite(&mut hasher), "{glyphs:?}{should_round_y:?}{extra:?}");
                for path in decoration_paths {
                    path_hash(path).hash(&mut hasher);
                }
            }
            0xfeu8.hash(&mut hasher);
        }
        (children.len(), child.is_some(), alpha_node.is_some(), color_node.is_some()).hash(&mut hasher);
        for id in node.refs() {
            self.content(id).hash(&mut hasher);
        }
        hasher.finish()
    }

    pub fn node(&self, id: u32) -> Option<&Arc<Node>> {
        self.forest.get(id)
    }

    /// Dev tools: how many of each op node `id` and what's under it have,
    /// and the points of their paths
    pub fn census(&self, id: u32) -> String {
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        let mut points = 0;
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let Some(node) = self.node(id) else { continue };
            *counts.entry(format!("{:?}", node.op)).or_default() += 1;
            for geometry in node.geometry.iter().chain(&node.fill_geometries).chain(&node.stroke_geometries) {
                points += geometry.path.as_ref().map_or(0, |path| path.points.len());
            }
            stack.extend(node.children.iter().copied().chain(node.child).chain(node.alpha_node).chain(node.color_node));
        }
        format!("{counts:?} points={points}")
    }

    /// Pattern tiles drawn without image `key`, or all of them
    pub fn forget_patterns(&self, key: Option<&str>) {
        match key {
            Some(key) => self.patterns.lock().unwrap().retain(|_, (_, missing)| !missing.iter().any(|missing| missing == key)),
            None => self.patterns.lock().unwrap().clear(),
        }
    }

    pub(super) fn info(&self, id: u32) -> Info {
        self.info_at(id, 0)
    }

    fn info_at(&self, id: u32, depth: u8) -> Info {
        if let Some(info) = self.info.read().unwrap().get(&id) {
            return *info;
        }
        let none = Info { bounds: None, count: 1, blends: false, custom: false, approximate: None };
        let Some(node) = self.forest.get(id).cloned() else {
            return none;
        };
        if depth > DEPTH {
            return none;
        }
        let of = |id: Option<u32>| id.map(|id| self.info_at(id, depth + 1));
        let union = |a: Option<Rect>, b: Option<Rect>| match (a, b) {
            (Some(a), Some(b)) => Some(Rect::join2(a, b)),
            (a, b) => a.or(b),
        };
        let intersect = |a: Option<Rect>, b: Option<Rect>| match (a, b) {
            (Some(a), Some(b)) => Rect::intersects2(&a, &b).then(|| {
                let mut a = a;
                a.intersect(b);
                a
            }),
            _ => None,
        };
        let mut info = none;
        let refs: Vec<u32> = node.refs().collect();
        let below: Vec<Info> = refs.iter().map(|&child| self.info_at(child, depth + 1)).collect();
        info.count = 1 + below.iter().map(|info| info.count).sum::<u32>();
        info.custom = below.iter().any(|info| info.custom);
        info.approximate = below.iter().find_map(|info| info.approximate);
        let children_bounds = || below.iter().map(|info| info.bounds).fold(None, union);
        let transform = node.transform.map(|t| rt::to_matrix(&t)).unwrap_or_default();
        let ext = node.ext();
        match node.op {
            Op::Fill => {
                let geometry = node.geometry.as_ref();
                let painted = geometry.is_some_and(|g| !g.paints.is_empty());
                info.bounds = geometry.filter(|_| painted).and_then(|g| g.path.as_ref()).map(|path| transform.map_rect(*self.skia_path(path).bounds()).0);
                info.blends = geometry.is_some_and(|g| g.paints.iter().any(|paint| blends(paint.blend_mode)));
            }
            Op::FillStroke | Op::ScatterBrushFill | Op::ScatterBrushFillStroke => {
                let mut bounds = None;
                for g in node.fill_geometries.iter().chain(&node.stroke_geometries).filter(|g| !g.paints.is_empty()) {
                    if let Some(path) = &g.path {
                        bounds = union(bounds, Some(*self.skia_path(path).bounds()));
                    }
                    info.blends |= g.paints.iter().any(|paint| blends(paint.blend_mode));
                }
                let instances: Vec<Matrix> = if ext.instance_transforms.is_empty() {
                    vec![Matrix::default()]
                } else {
                    ext.instance_transforms.iter().map(rt::to_matrix).collect()
                };
                info.bounds = bounds.map(|bounds| {
                    instances.iter().map(|instance| Matrix::concat(&transform, instance).map_rect(bounds).0).reduce(Rect::join2).unwrap_or(bounds)
                });
            }
            Op::FillGlyphs => {
                let size = node.paint_size.unwrap_or([0.0, 0.0]);
                // Glyphs reach past the box by their ascent and descent
                let margin = size[1].max(1.0);
                info.bounds = Some(transform.map_rect(Rect::from_wh(size[0], size[1]).with_outset((margin, margin))).0);
            }
            Op::StaticTransformBinding => {
                info.bounds = of(node.child).and_then(|child| child.bounds).map(|bounds| transform.map_rect(bounds).0);
                info.blends = below.iter().any(|info| info.blends);
            }
            Op::Blend => {
                info.bounds = children_bounds();
                let child = below.iter().any(|info| info.blends);
                info.blends = blends(node.blend_mode) || (pass_through(node.blend_mode) && node.opacity.unwrap_or(1.0) >= 0.999 && child);
            }
            Op::Blur => {
                let radius = finite(node.radius).max(finite(ext.start_radius));
                let reach = radius * 1.5;
                info.bounds = children_bounds().map(|bounds| bounds.with_outset((reach, reach)));
                if let Some(clamp) = ext.clamp {
                    info.bounds = intersect(info.bounds, Some(clamp.to_skia().with_outset((1.0, 1.0))));
                }
            }
            Op::HardMaskWithColor => {
                info.bounds = of(node.alpha_node).and_then(|alpha| alpha.bounds);
            }
            Op::Mask => {
                let alpha = of(node.alpha_node).and_then(|alpha| alpha.bounds);
                let color = of(node.color_node);
                info.bounds = match node.mask_op {
                    Some(MaskOpType::KnockoutShadow) => color.and_then(|color| color.bounds),
                    Some(MaskOpType::InnerShadow) => alpha,
                    _ => intersect(alpha, color.and_then(|color| color.bounds)),
                };
                info.blends = color.is_some_and(|color| color.blends);
                if matches!(node.mask_op, Some(MaskOpType::Glass) | Some(MaskOpType::Unknown(_))) {
                    info.approximate = Some("glass");
                }
            }
            Op::CustomEffect | Op::CustomEffectFill => {
                info.custom = true;
                info.approximate = Some("custom effect");
                info.bounds = children_bounds();
                if let (Some(target), Some(to)) = (ext.custom_effect_render_target_local_bounds, ext.node_to_canvas_space_transform.filter(|t| t.iter().all(|v| v.is_finite()))) {
                    info.bounds = union(info.bounds, Some(rt::to_matrix(&to).map_rect(target.to_skia()).0));
                }
            }
            Op::Glass => {
                info.approximate = Some("glass");
                info.bounds = children_bounds();
            }
            Op::SpatialPlane | Op::SpatialTransform | Op::SpatialComposition => {
                info.approximate = Some("3D layer");
                info.bounds = children_bounds();
            }
            _ => {
                info.bounds = children_bounds();
                info.blends = below.iter().any(|info| info.blends);
            }
        }
        if let Some(bounds) = info.bounds {
            if !(bounds.left.is_finite() && bounds.top.is_finite() && bounds.right.is_finite() && bounds.bottom.is_finite()) {
                info.bounds = None;
            }
        }
        self.info.write().unwrap().insert(id, info);
        info
    }

    /// Skia's copy of `path`, made once
    pub fn skia_path(&self, path: &Arc<rt::Path>) -> Path {
        let key = Arc::as_ptr(path) as usize;
        if let Some((_, skia)) = self.paths.lock().unwrap().get(&key) {
            return skia.clone();
        }
        let skia = path.to_skia();
        self.paths.lock().unwrap().insert(key, (path.clone(), skia.clone()));
        skia
    }

    /// The units to draw tiles from, bottom first
    pub fn units(&self) -> Vec<TreeUnit> {
        let mut out = Vec::new();
        if let Some(root) = self.root {
            self.expand(root, Matrix::default(), &mut Vec::new(), 0, &mut out);
        }
        // The same drawn twice in the same place is two units, for either
        // to go on its own
        let mut seen: HashMap<usize, usize> = HashMap::with_capacity(out.len());
        for unit in &mut out {
            let count = seen.entry(unit.key).or_insert(0);
            if *count > 0 {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                (unit.key, *count).hash(&mut hasher);
                unit.key = hasher.finish() as usize;
            }
            *count += 1;
        }
        out
    }

    fn expand(&self, id: u32, at: Matrix, clips: &mut Vec<(u32, Matrix)>, depth: u8, out: &mut Vec<TreeUnit>) {
        let Some(node) = self.forest.get(id) else {
            return;
        };
        let info = self.info(id);
        let big = info.count > SPLIT && depth < 32;
        match node.op {
            Op::OcclusionScope | Op::Metadata | Op::CompositeTileGrid if big => {
                if let Some(child) = node.child {
                    return self.expand(child, at, clips, depth + 1, out);
                }
            }
            Op::Group if big => {
                for &child in &node.children {
                    self.expand(child, at, clips, depth + 1, out);
                }
                return;
            }
            Op::StaticTransformBinding if big => {
                if let (Some(child), Some(t)) = (node.child, node.transform) {
                    return self.expand(child, Matrix::concat(&at, &rt::to_matrix(&t)), clips, depth + 1, out);
                }
            }
            Op::Mask if big && matches!(node.mask_op, None | Some(MaskOpType::Normal)) => {
                if let (Some(color), Some((fill, to))) = (node.color_node, node.alpha_node.and_then(|alpha| self.clip_of(alpha))) {
                    clips.push((fill, Matrix::concat(&at, &to)));
                    self.expand(color, at, clips, depth + 1, out);
                    clips.pop();
                    return;
                }
            }
            _ => {}
        }
        let Some(bounds) = info.bounds.map(|bounds| at.map_rect(bounds).0) else {
            return;
        };
        let mut bounds = bounds;
        for (fill, to) in clips.iter() {
            let Some(clip) = self.info(*fill).bounds.map(|clip| to.map_rect(clip).0) else {
                return;
            };
            if !bounds.intersect(clip) {
                return;
            }
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.content(id).hash(&mut hasher);
        let mut values = [0.0f32; 9];
        for m in std::iter::once(&at).chain(clips.iter().map(|(_, m)| m)) {
            m.get_9(&mut values);
            for value in values {
                value.to_bits().hash(&mut hasher);
            }
        }
        for (fill, _) in clips.iter() {
            self.content(*fill).hash(&mut hasher);
        }
        out.push(TreeUnit {
            key: hasher.finish() as usize,
            place: Place { id, at, clips: clips.clone() },
            bounds,
            blends: info.blends,
            custom: info.custom,
        });
    }

    /// When node `id` is no more than an opaque shape, the fill node it is
    /// and where that's drawn, for a mask of it to be a clip instead
    pub(super) fn clip_of(&self, id: u32) -> Option<(u32, Matrix)> {
        let mut at = Matrix::default();
        let mut id = id;
        for _ in 0..8 {
            let node = self.forest.get(id)?;
            match node.op {
                Op::StaticTransformBinding => {
                    at = Matrix::concat(&at, &rt::to_matrix(&node.transform?));
                    id = node.child?;
                }
                Op::Group if node.children.len() == 1 => id = node.children[0],
                Op::OcclusionScope => id = node.child?,
                Op::Fill => {
                    let geometry = node.geometry.as_ref()?;
                    geometry.path.as_ref()?;
                    let opaque = !geometry.paints.is_empty()
                        && geometry.paints.iter().all(|paint| {
                            paint.kind == Some(PaintType::Solid)
                                && !blends(paint.blend_mode)
                                && paint.opacity.unwrap_or(1.0) >= 0.999
                                && paint.color.is_some_and(|color| color.a >= 0.999)
                        });
                    return opaque.then(|| (id, Matrix::concat(&at, &rt::to_matrix(&node.transform.unwrap_or(rt::IDENTITY)))));
                }
                _ => return None,
            }
        }
        None
    }

    /// Custom effect node `id`'s effects, when they're of a kind that runs
    pub fn custom_effect(&self, id: u32) -> Option<CustomEffect> {
        let node = self.forest.get(id)?;
        if node.op != Op::CustomEffect {
            return None;
        }
        let ext = node.ext();
        // One run of effects over the node is all that's taken
        let [texture] = ext.custom_effect_textures.as_slice() else {
            return None;
        };
        let chain: Vec<(String, serde_json::Value)> = texture
            .intra_layer_chain
            .iter()
            .map(|entry| {
                let id = entry.code_component_id.as_deref()?;
                let id = id.strip_prefix("CodeComponentId:").unwrap_or(id).to_string();
                let params = serde_json::from_str(entry.params_json.as_deref().unwrap_or("{}")).ok()?;
                Some((id, params))
            })
            .collect::<Option<_>>()?;
        let area = ext.custom_effect_render_target_local_bounds?.to_skia();
        if chain.is_empty() || !area.is_finite() || area.is_empty() {
            return None;
        }
        let guid = texture.node_guid.unwrap_or_default();
        Some(CustomEffect {
            key: format!("{}t{id}.{}", draw::EFFECT, self.content(id)),
            node: format!("{}:{}", guid.session_id, guid.local_id),
            chain,
            area,
            to: Drawer::effect_space(node),
            child: node.child?,
        })
    }

    /// The outline of fill node `id`, in its own space
    pub(super) fn outline(&self, id: u32) -> Option<Path> {
        let node = self.forest.get(id)?;
        node.geometry.as_ref()?.path.as_ref().map(|path| self.skia_path(path))
    }

    /// Ids of nodes under the absolute `point`, from the top-level one down
    pub fn hit(&self, point: (f32, f32)) -> Vec<String> {
        let mut out = Vec::new();
        let Some(root) = self.root else {
            return out;
        };
        let mut id = root;
        let mut at = Matrix::default();
        for _ in 0..64 {
            let Some(node) = self.forest.get(id) else { break };
            let inside = |child: u32, at: &Matrix| {
                self.info(child).bounds.is_some_and(|bounds| {
                    let bounds = at.map_rect(bounds).0;
                    point.0 >= bounds.left && point.0 <= bounds.right && point.1 >= bounds.top && point.1 <= bounds.bottom
                })
            };
            let next = match node.op {
                Op::Group => node.children.iter().rev().copied().find(|&child| inside(child, &at)),
                Op::StaticTransformBinding => {
                    at = Matrix::concat(&at, &rt::to_matrix(&node.transform.unwrap_or(rt::IDENTITY)));
                    node.child
                }
                Op::Mask => node.color_node,
                _ => node.child,
            };
            out.push(format!("{id} {:?}", node.op));
            match next {
                Some(next) => id = next,
                None => break,
            }
        }
        out
    }

    /// What Figma clears its canvas to, if opaque, whether the page shows a
    /// checkerboard, and whether that's the dark one
    pub fn background(&self) -> Option<(Option<Color4f>, bool, bool)> {
        let header = self.header.as_ref()?;
        let [r, g, b, a] = header.bg;
        let opaque = (a >= 0.999).then(|| Color4f::new(r, g, b, 1.0));
        Some((opaque, opaque.is_none() && header.checker != 0, header.checker == 3))
    }
}

fn finite(value: Option<f32>) -> f32 {
    value.filter(|value| value.is_finite()).unwrap_or(0.0).max(0.0)
}

pub(super) fn pass_through(mode: Option<rt::BlendMode>) -> bool {
    matches!(mode, None | Some(rt::BlendMode::None) | Some(rt::BlendMode::PassThrough))
}

/// A blend mode that reaches what's drawn before
pub(super) fn blends(mode: Option<rt::BlendMode>) -> bool {
    !matches!(mode, None | Some(rt::BlendMode::None) | Some(rt::BlendMode::PassThrough) | Some(rt::BlendMode::Normal))
}

/// The blend mode's name as the plugin API gives it, for draw.rs
fn blend_name(mode: rt::BlendMode) -> Option<&'static str> {
    use rt::BlendMode::*;
    Some(match mode {
        None | PassThrough | Normal => return Option::None,
        Darken => "DARKEN",
        Multiply => "MULTIPLY",
        LinearBurn => "LINEAR_BURN",
        ColorBurn => "COLOR_BURN",
        Lighten => "LIGHTEN",
        Screen => "SCREEN",
        LinearDodge => "LINEAR_DODGE",
        ColorDodge => "COLOR_DODGE",
        Overlay => "OVERLAY",
        SoftLight => "SOFT_LIGHT",
        HardLight => "HARD_LIGHT",
        Difference => "DIFFERENCE",
        Exclusion => "EXCLUSION",
        Hue => "HUE",
        Saturation => "SATURATION",
        Color => "COLOR",
        Luminosity => "LUMINOSITY",
        Unknown(_) => return Option::None,
    })
}

pub(super) fn rgba(color: rt::Color) -> Rgba {
    Rgba { r: color.r, g: color.g, b: color.b, a: color.a }
}

/// Draws the tree onto a canvas whose matrix maps absolute coordinates to
/// pixels, `px` of them a unit
pub struct Drawer<'a> {
    pub tree: &'a Tree,
    pub images: &'a HashMap<String, Img>,
    pub kit: &'a Kit,
    pub canvas: &'a Canvas,
    pub px: f32,
    pub counts: Counts,
    pub missing_images: Vec<(String, f64)>,
    pub sharper_images: Vec<(String, f64)>,
    pub used_images: Vec<(String, f64)>,
    pub approximate_reason: Option<String>,
    pub aa: bool,
    pub draft: bool,
    pub tint: bool,
    /// Draws what casts a silhouette: every paint solid wherever it's at
    /// all, an image's texels before they're smoothed, as Figma casts
    /// shadows
    cast: bool,
    /// The color a silhouette's drawn in, rather than in a layer of it
    shade: Option<skia_safe::ColorFilter>,
    depth: u8,
    patterns: u8,
}

impl<'a> Drawer<'a> {
    pub fn new(tree: &'a Tree, images: &'a HashMap<String, Img>, kit: &'a Kit, canvas: &'a Canvas, px: f32) -> Self {
        Drawer {
            tree,
            images,
            kit,
            canvas,
            px,
            counts: Counts::default(),
            missing_images: Vec::new(),
            sharper_images: Vec::new(),
            used_images: Vec::new(),
            approximate_reason: None,
            aa: true,
            draft: false,
            tint: false,
            cast: false,
            shade: None,
            depth: 0,
            patterns: 0,
        }
    }

    /// Unit `place`: its node inside its clips
    pub fn place(&mut self, place: &Place) {
        self.canvas.save();
        for (fill, at) in &place.clips {
            if let Some(outline) = self.tree.outline(*fill) {
                self.canvas.clip_path(&self.volatile(outline.with_transform(at)), ClipOp::Intersect, self.aa);
                super::emit::clip(self.canvas);
            }
        }
        self.canvas.concat(&place.at);
        self.draw(place.id);
        self.canvas.restore();
    }

    fn flat(&self, bounds: Rect) -> bool {
        bounds.width().max(bounds.height()) * self.device_scale() < small(1)
    }

    fn device_scale(&self) -> f32 {
        let m = self.canvas.local_to_device_as_3x3();
        m.scale_x().hypot(m.skew_y()).max(m.skew_x().hypot(m.scale_y())) * self.px
    }

    fn volatile(&self, mut path: Path) -> Path {
        if !self.aa {
            path.set_is_volatile(true);
        }
        path
    }

    fn approximate(&mut self, reason: &str) {
        self.counts.approximate += 1;
        if self.approximate_reason.is_none() {
            self.approximate_reason = Some(reason.to_string());
        }
    }

    pub fn draw(&mut self, id: u32) {
        if self.depth >= DEPTH {
            return;
        }
        let Some(node) = self.tree.node(id).cloned() else {
            return;
        };
        let info = self.tree.info(id);
        let Some(bounds) = info.bounds else {
            return;
        };
        let device = self.device_scale();
        if bounds.width().max(bounds.height()) * device < if self.draft { 2.0 } else { small(0).max(0.5) } {
            self.counts.skipped += 1;
            return;
        }
        self.depth += 1;
        self.op(&node, bounds);
        self.depth -= 1;
    }

    fn op(&mut self, node: &Node, bounds: Rect) {
        let ext = node.ext();
        match node.op {
            Op::Group => {
                for &child in &node.children {
                    self.draw(child);
                }
            }
            Op::StaticTransformBinding => {
                let Some(child) = node.child else { return };
                self.canvas.save();
                self.canvas.concat(&rt::to_matrix(&node.transform.unwrap_or(rt::IDENTITY)));
                self.draw(child);
                self.canvas.restore();
            }
            Op::Fill => {
                self.counts.native += 1;
                let Some(geometry) = &node.geometry else { return };
                let Some(path) = &geometry.path else { return };
                let path = self.tree.skia_path(path);
                self.canvas.save();
                self.canvas.concat(&rt::to_matrix(&node.transform.unwrap_or(rt::IDENTITY)));
                for paint in &geometry.paints {
                    self.paint(node, paint, &path);
                }
                self.canvas.restore();
            }
            Op::FillStroke | Op::ScatterBrushFill | Op::ScatterBrushFillStroke => {
                self.counts.native += 1;
                let instances: Vec<Matrix> =
                    if ext.instance_transforms.is_empty() { vec![Matrix::default()] } else { ext.instance_transforms.iter().map(rt::to_matrix).collect() };
                for instance in instances {
                    self.canvas.save();
                    self.canvas.concat(&rt::to_matrix(&node.transform.unwrap_or(rt::IDENTITY)));
                    self.canvas.concat(&instance);
                    self.fill_stroke(node);
                    self.canvas.restore();
                }
            }
            Op::FillGlyphs => {
                self.counts.native += 1;
                let Some(glyphs) = &ext.painted_glyph_geometry else { return };
                let mut outline = skia_safe::PathBuilder::new();
                for glyph in &glyphs.glyphs {
                    let Some(path) = &glyph.path else { continue };
                    let [x, y] = glyph.position.unwrap_or([0.0, 0.0]);
                    let size = glyph.font_size.unwrap_or(1.0);
                    let mut m = Matrix::translate((x, y));
                    m.pre_scale((size, -size), None);
                    m.pre_rotate(glyph.rotation.unwrap_or(0.0).to_degrees(), None);
                    outline.add_path(&self.tree.skia_path(path).with_transform(&m), None);
                }
                let outline = outline.detach();
                self.canvas.save();
                self.canvas.concat(&rt::to_matrix(&node.transform.unwrap_or(rt::IDENTITY)));
                for paint in &glyphs.paints {
                    self.paint(node, paint, &outline);
                }
                self.canvas.restore();
            }
            Op::Blend => {
                let Some(child) = node.child else { return };
                let opacity = node.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
                if self.cast {
                    // A silhouette is solid however see-through what casts it
                    self.draw(child);
                    return;
                }
                if opacity <= 0.0 {
                    return;
                }
                let isolate = !pass_through(node.blend_mode) || self.tree.info(child).blends;
                if (opacity >= 0.999 && !blends(node.blend_mode) && !isolate) || skip(2) || self.flat(bounds) {
                    self.draw(child);
                    return;
                }
                count(2, 1);
                let mut paint = Paint::default();
                paint.set_alpha_f(opacity);
                self.set_blend(&mut paint, node.blend_mode);
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&paint));
                super::emit::layer(self.canvas, Some(&bounds));
                self.draw(child);
                self.canvas.restore();
            }
            Op::Blur => {
                let Some(child) = node.child else { return };
                if ext.blur_op_type == Some(rt::BlurOpType::Progressive) {
                    return self.progressive(node, child, bounds);
                }
                let sigma = draw::tent_sigma(finite(node.radius) as f64);
                if sigma <= 0.0 || skip(4) || self.flat(bounds) {
                    return self.draw(child);
                }
                count(3, 1);
                let device = self.device_scale();
                let filter = match ext.clamp {
                    // Figma blurs only what's inside, each pixel the
                    // weighted mean of the part of the kernel inside it: the
                    // blur of what's inside over the blur of the area itself
                    Some(clamp) => {
                        let area = clamp.to_skia();
                        let inside = draw::wide_blur(sigma, device, image_filters::crop(area, TileMode::Decal, None), None);
                        let weight = draw::wide_blur_box(area, sigma, device);
                        self.kit.divide.clone().and_then(|divide| image_filters::blend(divide, weight, inside, None))
                    }
                    None => draw::wide_blur(sigma, device, None, None),
                };
                let mut paint = Paint::default();
                paint.set_image_filter(filter);
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&paint));
                super::emit::layer(self.canvas, Some(&bounds));
                self.draw(child);
                self.canvas.restore();
            }
            Op::HardMaskWithColor => {
                let Some(alpha) = node.alpha_node else { return };
                if skip(8) || self.flat(bounds) {
                    return;
                }
                let color = node.color.unwrap_or_default();
                let shade = color_filters::blend(draw::color(rgba(color), 1.0).to_color(), BlendMode::SrcIn);
                // A silhouette drawn at once comes out the same colored as
                // it's drawn, as a text's shadow is, without a layer each
                if !self.cast && self.single_draw(alpha) {
                    let was = std::mem::replace(&mut self.shade, shade);
                    self.cast_with(alpha);
                    self.shade = was;
                    return;
                }
                let mut paint = Paint::default();
                if !self.cast {
                    paint.set_color_filter(shade);
                }
                count(4, 1);
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&paint));
                super::emit::layer(self.canvas, Some(&bounds));
                self.cast_with(alpha);
                self.canvas.restore();
            }
            Op::Mask if skip(1) || self.flat(bounds) => {
                if let Some(color) = node.color_node {
                    self.draw(color);
                }
            }
            Op::Mask => self.mask(node, bounds),
            Op::Noise => {
                let Some(child) = node.child else { return };
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds));
                super::emit::layer(self.canvas, Some(&bounds));
                self.draw(child);
                if !self.cast {
                    self.noise(node, bounds);
                }
                self.canvas.restore();
            }
            Op::Grain => {
                let Some(child) = node.child else { return };
                let filter = if self.cast { None } else { self.grain(node) };
                let mut paint = Paint::default();
                paint.set_image_filter(filter);
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&paint));
                super::emit::layer(self.canvas, Some(&bounds));
                self.draw(child);
                self.canvas.restore();
            }
            Op::CustomEffect if !self.cast && self.custom_effect(node.id) => {}
            Op::CustomEffect | Op::CustomEffectFill | Op::Glass | Op::SpatialPlane | Op::SpatialTransform | Op::SpatialComposition => {
                // Drawn without the effect until it's run, or where it can't
                // be, as Figma would if it couldn't run it
                self.approximate(match node.op {
                    Op::Glass => "glass",
                    Op::CustomEffect | Op::CustomEffectFill => "custom effect",
                    _ => "3D layer",
                });
                let children: Vec<u32> = node
                    .child
                    .into_iter()
                    .chain(node.children.iter().copied())
                    .chain(ext.spatial_plane_content_id)
                    .chain(ext.spatial_transform_child_id)
                    .chain(ext.spatial_composition_child_ids.iter().copied())
                    .collect();
                for child in children {
                    self.draw(child);
                }
            }
            _ => {
                for child in node.child.into_iter().chain(node.children.iter().copied()) {
                    self.draw(child);
                }
            }
        }
    }

    /// Draws custom effect node `id` as its effects drew it, run by the
    /// effect host on the node's content, if that's here. As on Figma's
    /// canvas, they run once, at a pixel a unit whatever the zoom, and what
    /// they draw covers the box they ran over
    fn custom_effect(&mut self, id: u32) -> bool {
        let Some(effect) = self.tree.custom_effect(id) else {
            return false;
        };
        self.canvas.save();
        self.canvas.concat(&effect.to);
        let shown = (effect.area.width().max(effect.area.height()) * self.device_scale() * 1.42) as f64;
        let drawn = match self.images.get(&effect.key) {
            Some(img) => {
                // Kept about as big as it shows, in steps of halves of what
                // the effects drew, as the effects thread sizes it
                let (fw, fh) = img.original;
                let longest = fw.max(fh);
                let fits = longest >> super::run::halvings(longest, longest as f64 * shown * 1.25 / fw.max(1) as f64);
                if img.stale {
                    self.missing_images.push((effect.key.clone(), shown));
                } else {
                    self.used_images.push((effect.key.clone(), shown));
                    if (img.image.width().max(img.image.height()) as u32) < fits {
                        self.sharper_images.push((effect.key.clone(), shown));
                    }
                }
                img.area.is_some_and(|area| {
                    let scale = img.image.width() as f32 / area.width().max(1e-6);
                    let sampling = if scale > self.device_scale() * 1.5 {
                        draw::sampled(&img.image, FilterMode::Linear, MipmapMode::Linear)
                    } else {
                        SamplingOptions::new(FilterMode::Linear, MipmapMode::None)
                    };
                    let at = Rect::from_xywh(effect.area.left, effect.area.top, area.width(), area.height());
                    self.canvas.draw_image_rect_with_sampling_options(&img.image, None, at, sampling, &Paint::default());
                    super::emit::fill_rect(self.canvas, &at, Color4f::new(0.5, 0.5, 0.5, 1.0));
                    true
                })
            }
            None => {
                self.missing_images.push((effect.key.clone(), shown));
                false
            }
        };
        self.canvas.restore();
        if drawn {
            self.counts.native += 1;
        }
        drawn
    }

    /// Draws `id` as what casts a silhouette
    /// Node `id` is one shape with one paint, drawn in one go
    fn single_draw(&self, id: u32) -> bool {
        single_draw(self.tree, id)
    }

    /// Draws what casts a silhouette, in `shade` if there's one
    pub(super) fn casting(&mut self, shade: Option<Color4f>) {
        self.cast = true;
        self.shade = shade.and_then(|shade| color_filters::blend(shade.to_color(), BlendMode::SrcIn));
    }

    fn cast_with(&mut self, id: u32) {
        let was = std::mem::replace(&mut self.cast, true);
        self.draw(id);
        self.cast = was;
    }

    fn fill_stroke(&mut self, node: &Node) {
        for geometry in &node.fill_geometries {
            let Some(path) = &geometry.path else { continue };
            let path = self.tree.skia_path(path);
            for paint in &geometry.paints {
                self.paint(node, paint, &path);
            }
        }
        if node.stroke_geometries.iter().all(|g| g.paints.is_empty()) {
            return;
        }
        let outlines: Vec<Path> = node.fill_geometries.iter().filter_map(|g| g.path.as_ref()).map(|path| self.tree.skia_path(path)).collect();
        let outline = match outlines.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [first, rest @ ..] => Some(rest.iter().fold(first.clone(), |all, path| all.op(path, skia_safe::PathOp::Union).unwrap_or(all))),
        };
        self.canvas.save();
        match (node.stroke_mode, outline) {
            (Some(rt::StrokeMask::Inside), Some(outline)) => {
                self.canvas.clip_path(&self.volatile(outline), ClipOp::Intersect, self.aa);
                super::emit::clip(self.canvas);
            }
            (Some(rt::StrokeMask::Outside), Some(outline)) => {
                self.canvas.clip_path(&self.volatile(outline), ClipOp::Difference, self.aa);
                super::emit::clip(self.canvas);
            }
            _ => {}
        }
        for geometry in &node.stroke_geometries {
            let Some(path) = &geometry.path else { continue };
            let path = self.tree.skia_path(path);
            for paint in &geometry.paints {
                self.paint(node, paint, &path);
            }
        }
        self.canvas.restore();
    }

    fn mask(&mut self, node: &Node, bounds: Rect) {
        let (Some(alpha), Some(color)) = (node.alpha_node, node.color_node) else {
            return;
        };
        let ext = node.ext();
        let op = node.mask_op.unwrap_or(MaskOpType::Normal);
        match op {
            MaskOpType::KnockoutShadow => {
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds));
                super::emit::layer(self.canvas, Some(&bounds));
                self.draw(color);
                let mut out = Paint::default();
                out.set_blend_mode(BlendMode::DstOut);
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&out));
                super::emit::layer(self.canvas, Some(&bounds));
                self.cast_with(alpha);
                self.canvas.restore();
                self.canvas.restore();
            }
            MaskOpType::InnerShadow => {
                // The shadow's color where the shape, moved and blurred,
                // isn't, kept inside the shape
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds));
                super::emit::layer(self.canvas, Some(&bounds));
                if !self.cast {
                    let fill = ext.inner_shadow_fill.unwrap_or_default();
                    let mut paint = Paint::new(draw::color(rgba(fill), 1.0), None);
                    paint.set_anti_alias(false);
                    self.canvas.draw_rect(bounds, &paint);
                    super::emit::fill_rect(self.canvas, &bounds, Color4f::new(0.5, 0.5, 0.5, 1.0));
                } else {
                    self.canvas.draw_rect(bounds, &Paint::new(Color4f::new(0.0, 0.0, 0.0, 1.0), None));
                    super::emit::fill_rect(self.canvas, &bounds, Color4f::new(0.5, 0.5, 0.5, 1.0));
                }
                let mut out = Paint::default();
                out.set_blend_mode(BlendMode::DstOut);
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&out));
                super::emit::layer(self.canvas, Some(&bounds));
                self.draw(color);
                self.canvas.restore();
                let mut keep = Paint::default();
                keep.set_blend_mode(BlendMode::DstIn);
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&keep));
                super::emit::layer(self.canvas, Some(&bounds));
                if ext.inner_shadow_alpha_requires_hard_mask {
                    self.cast_with(alpha);
                } else {
                    self.draw(alpha);
                }
                self.canvas.restore();
                self.canvas.restore();
            }
            _ => {
                if matches!(op, MaskOpType::Glass | MaskOpType::Unknown(_)) {
                    self.approximate("glass");
                }
                // A mask that's an opaque shape is a clip, where that comes
                // out the same: clipped with smoothed edges, each shape
                // under it is cut on its own, so at the edge those beneath
                // show through those over them, and blends add to what's
                // already mixed with what's outside. Masked as Figma does,
                // what's under the mask is cut once, as one
                // A background blur has to be over what it blurs, which in a
                // layer of its own it isn't
                let one = !self.aa || op == MaskOpType::BackgroundBlur || (self.single_draw(color) && !self.tree.info(color).blends);
                if one && matches!(op, MaskOpType::Normal | MaskOpType::Pattern | MaskOpType::BackgroundBlur) {
                    if let Some((fill, at)) = self.tree.clip_of(alpha) {
                        if let Some(outline) = self.tree.outline(fill) {
                            count(1, 1);
                            self.canvas.save();
                            self.canvas.clip_path(&self.volatile(outline.with_transform(&at)), ClipOp::Intersect, self.aa);
                            super::emit::clip(self.canvas);
                            self.draw(color);
                            self.canvas.restore();
                            return;
                        }
                    }
                }
                // What's masked blends with what's under the mask, as
                // masks don't isolate it in Figma: it's drawn over a copy of
                // that, then the copy is kept only where the mask is
                count(0, 1);
                let mut rec = SaveLayerRec::default().bounds(&bounds);
                if !self.cast && self.tree.info(color).blends {
                    rec = rec.flags(SaveLayerFlags::INIT_WITH_PREVIOUS);
                }
                self.canvas.save_layer(&rec);
                super::emit::layer(self.canvas, None);
                self.draw(color);
                let mut keep = Paint::default();
                keep.set_blend_mode(BlendMode::DstIn);
                if op == MaskOpType::Luminance {
                    keep.set_color_filter(self.kit.luminance.clone().unwrap_or_else(skia_safe::luma_color_filter::new));
                }
                self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&keep));
                super::emit::layer(self.canvas, Some(&bounds));
                // What a background blur shows is opaque, so it shows
                // wherever its shape is at all
                if op == MaskOpType::BackgroundBlur {
                    self.cast_with(alpha);
                } else {
                    self.draw(alpha);
                }
                self.canvas.restore();
                self.canvas.restore();
            }
        }
    }

    fn set_blend(&self, paint: &mut Paint, mode: Option<rt::BlendMode>) {
        let Some(name) = mode.and_then(blend_name) else {
            return;
        };
        if let Some(blender) = self.kit.blenders.get(name) {
            paint.set_blender(blender.clone());
        } else if let Some(mode) = draw::blend(name) {
            paint.set_blend_mode(mode);
        }
    }

    /// One paint over `path`, in the node's own coordinates
    pub(super) fn paint(&mut self, node: &Node, paint: &rt::Paint, path: &Path) {
        let size = node.paint_size.unwrap_or([1.0, 1.0]);
        let (w, h) = (size[0].max(1e-3), size[1].max(1e-3));
        let opacity = paint.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
        let mut out = Paint::default();
        out.set_anti_alias(self.aa);
        let kind = paint.kind.unwrap_or(PaintType::Solid);
        if self.cast {
            if kind != PaintType::Image {
                out.set_color_filter(draw::silhouette());
            }
            if let Some(shade) = &self.shade {
                let filter = match out.color_filter() {
                    Some(inner) => shade.composed(inner),
                    None => Some(shade.clone()),
                };
                out.set_color_filter(filter);
            }
        } else {
            self.set_blend(&mut out, paint.blend_mode);
        }
        match kind {
            PaintType::Solid => {
                let color = paint.color.unwrap_or_default();
                out.set_color4f(draw::color(rgba(color), opacity), None);
            }
            PaintType::GradientLinear | PaintType::GradientRadial | PaintType::GradientAngular | PaintType::GradientDiamond => {
                let Some(shader) = gradient(self.kit, paint, kind, w, h) else { return };
                out.set_shader(shader);
            }
            PaintType::Image | PaintType::Video => {
                if kind == PaintType::Video {
                    self.approximate("video");
                }
                let Some(shader) = self.image(paint, w, h) else { return };
                out.set_shader(shader);
                out.set_alpha_f(if self.cast { 1.0 } else { opacity });
            }
            PaintType::Pattern => {
                let Some(shader) = self.pattern(paint, w, h) else { return };
                out.set_shader(shader);
                out.set_alpha_f(opacity);
            }
            _ => {
                self.approximate("emoji or effect paint");
                return;
            }
        }
        if skip(32) {
            return;
        }
        if skip(16) {
            out.set_anti_alias(false);
        }
        let path = if skip(64) {
            coarse(path, 0.2 / self.device_scale().max(1e-6))
        } else {
            path.clone()
        };
        count(5, 1);
        count(6, path.count_points() as u64);
        super::emit::path(self.canvas, &path, out.color4f());
        self.canvas.draw_path(&self.volatile(path), &out);
    }

    fn image(&mut self, paint: &rt::Paint, w: f32, h: f32) -> Option<skia_safe::Shader> {
        let key = image_key(paint)?;
        let ctm = self.canvas.local_to_device_as_3x3();
        let node_scale = ctm.scale_x().hypot(ctm.skew_y());
        let Some(img) = self.images.get(&key) else {
            if !self.missing_images.iter().any(|(missing, _)| *missing == key) {
                self.missing_images.push((key, (w.max(h) * node_scale * self.px * 1.42) as f64));
            }
            return None;
        };
        let (to_node, tile) = image_layout(paint, w, h, img)?;
        let iw = img.original.0 as f32;
        let shown = to_node.scale_x().hypot(to_node.skew_y()) * node_scale * iw * self.px;
        let kept = img.image.width() as f32;
        self.used_images.push((key.clone(), (shown * 1.42) as f64));
        if kept < shown * 1.42 && iw > kept && kept < 4096.0 {
            self.sharper_images.push((key, (shown * 1.42) as f64));
        }
        draw::image_shader(self.kit, img, to_node, tile, &ctm, self.px, shown, self.aa, self.cast)
    }

    /// A pattern fill: its source drawn into a tile, which repeats
    fn pattern(&mut self, paint: &rt::Paint, w: f32, h: f32) -> Option<skia_safe::Shader> {
        if self.patterns >= 3 {
            return None;
        }
        let source = paint.pattern_source_node_id?;
        let frame = paint.pattern_source_absolute_bounds?.to_skia();
        let (sw, sh) = (frame.width().max(1e-3), frame.height().max(1e-3));
        let factor = paint.pattern_scale.filter(|f| f.is_finite() && *f > 0.0).unwrap_or(1.0);
        let spacing = paint.pattern_spacing.unwrap_or([0.0, 0.0]);
        let (cw, ch) = (sw * (1.0 + spacing[0]).max(0.01), sh * (1.0 + spacing[1]).max(0.01));
        let tile_type = paint.pattern_tile_type.unwrap_or(rt::PatternTileType::Rectangular);
        let (tile, placements): (Rect, Vec<(f32, f32)>) = match tile_type {
            rt::PatternTileType::HorizontalHexagonal => (Rect::from_wh(cw, ch * 2.0), vec![(0.0, 0.0), (cw / 2.0, ch), (-cw / 2.0, ch)]),
            rt::PatternTileType::VerticalHexagonal => (Rect::from_wh(cw * 2.0, ch), vec![(0.0, 0.0), (cw, ch / 2.0), (cw, -ch / 2.0)]),
            _ => (Rect::from_wh(cw, ch), vec![(0.0, 0.0)]),
        };
        let ctm = self.canvas.local_to_device_as_3x3();
        let px = self.px * ctm.scale_x().hypot(ctm.skew_y()) * factor;
        let level = px.log2().round().clamp(-12.0, 8.0) as i32;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (format!("{tile_type:?}"), spacing[0].to_bits(), spacing[1].to_bits(), frame.left.to_bits(), frame.top.to_bits()).hash(&mut hasher);
        let key = (source, hasher.finish(), level);
        let cached = self.tree.patterns.lock().unwrap().get(&key).cloned();
        let picture = match cached {
            Some((picture, missing)) => {
                for hash in missing {
                    if !self.missing_images.iter().any(|(m, _)| *m == hash) {
                        self.missing_images.push((hash, 0.0));
                    }
                }
                picture
            }
            None => {
                let mut recorder = PictureRecorder::new();
                let canvas = recorder.begin_recording(tile, false);
                let mut inner = Drawer::new(self.tree, self.images, self.kit, canvas, 2f32.powi(level));
                inner.patterns = self.patterns + 1;
                inner.aa = self.aa;
                for (x, y) in placements {
                    canvas.save();
                    canvas.translate((x - frame.left, y - frame.top));
                    inner.draw(source);
                    canvas.restore();
                }
                let missing: Vec<String> = inner.missing_images.iter().map(|(hash, _)| hash.clone()).collect();
                self.missing_images.append(&mut inner.missing_images);
                self.sharper_images.append(&mut inner.sharper_images);
                self.used_images.append(&mut inner.used_images);
                self.counts += inner.counts;
                drop(inner);
                let picture = recorder.finish_recording_as_picture(Some(&tile))?;
                self.tree.patterns.lock().unwrap().insert(key, (picture.clone(), missing));
                picture
            }
        };
        let along = |alignment: Option<rt::PatternAlignment>, size: f32, tile: f32| match alignment {
            Some(rt::PatternAlignment::Center) => (size - tile) / 2.0,
            Some(rt::PatternAlignment::End) => size - tile,
            _ => 0.0,
        };
        let mut local = Matrix::translate((
            along(paint.pattern_horizontal_alignment, w, sw * factor),
            along(paint.pattern_vertical_alignment, h, sh * factor),
        ));
        local.pre_scale((factor, factor), None);
        Some(picture.to_shader((TileMode::Repeat, TileMode::Repeat), FilterMode::Linear, &local, &tile))
    }

    /// The node's effect space: where its source node is in the node's own
    pub fn effect_space(node: &Node) -> Matrix {
        node.ext()
            .node_to_canvas_space_transform
            .filter(|t| t.iter().all(|v| v.is_finite()))
            .map(|t| rt::to_matrix(&t))
            .unwrap_or_default()
    }

    /// Noise over what the layer drew, only where it drew
    fn noise(&mut self, node: &Node, bounds: Rect) {
        let ext = node.ext();
        let (Some(noise), Some(over)) = (&self.kit.noise, &self.kit.noise_over) else {
            return;
        };
        if self.tree.octaves.len() < 36 {
            self.approximate("noise");
            return;
        }
        let color = ext.noise_color.unwrap_or(rt::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.25 });
        let secondary = ext.secondary_noise_color.unwrap_or(rt::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.25 });
        let size = ext.noise_size.unwrap_or([1.0, 1.0]);
        let center = ext.center_point.unwrap_or([0.0, 0.0]);
        let kind = match ext.noise_type {
            Some(rt::NoiseType::Multitone) => 0.0,
            Some(rt::NoiseType::Duotone) => 2.0,
            _ => 1.0,
        };
        let values = [
            1.0 / size[0].max(1e-3),
            1.0 / size[1].max(1e-3),
            center[0],
            center[1],
            kind,
            ext.density.unwrap_or(0.5).clamp(0.0, 1.0),
            color.r,
            color.g,
            color.b,
            color.a,
            secondary.r,
            secondary.g,
            secondary.b,
            secondary.a,
        ];
        let mut uniforms = Vec::with_capacity((values.len() + 36) * 4);
        for value in values.into_iter().chain(self.tree.octaves.iter().take(36).copied()) {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        let Some(shader) = noise.make_shader(Data::new_copy(&uniforms), &[], None) else {
            return;
        };
        let space = Self::effect_space(node);
        let Some(inverse) = space.invert() else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_shader(shader);
        paint.set_blender(over.clone());
        self.canvas.save();
        self.canvas.concat(&space);
        self.canvas.draw_rect(inverse.map_rect(bounds.with_outset((1.0, 1.0))).0, &paint);
        super::emit::fill_rect(self.canvas, &inverse.map_rect(bounds.with_outset((1.0, 1.0))).0, Color4f::new(0.5, 0.5, 0.5, 1.0));
        self.canvas.restore();
    }

    /// Figma's texture effect: what the layer drew, moved about by noise
    fn grain(&mut self, node: &Node) -> Option<skia_safe::ImageFilter> {
        let ext = node.ext();
        let texture = self.kit.texture.as_ref()?;
        if self.tree.octaves.len() < 18 {
            self.approximate("texture");
            return None;
        }
        let size = ext.noise_size.unwrap_or([1.0, 1.0]);
        let center = ext.center_point.unwrap_or([0.0, 0.0]);
        let values = [1.0 / size[0].max(1e-3), 1.0 / size[1].max(1e-3), center[0], center[1]];
        let mut uniforms = Vec::with_capacity((values.len() + 18) * 4);
        for value in values.into_iter().chain(self.tree.octaves.iter().take(18).copied()) {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        let space = Self::effect_space(node);
        let shader = texture.make_shader(Data::new_copy(&uniforms), &[], Some(&space))?;
        let field = image_filters::shader(shader, None);
        let scale = space.scale_x().hypot(space.skew_y());
        image_filters::displacement_map((ColorChannel::R, ColorChannel::G), 4.0 * finite(node.radius) * scale, field, None, None)
    }

    /// A progressive blur, more from its start to its end: blurs of radii
    /// evenly apart, each shown about where the radius is its own
    fn progressive(&mut self, node: &Node, child: u32, bounds: Rect) {
        let ext = node.ext();
        let space = Self::effect_space(node);
        let point = |offset: Option<rt::Vector>, default: [f32; 2]| {
            let [x, y] = offset.filter(|p| p[0].is_finite() && p[1].is_finite()).unwrap_or(default);
            space.map_point((x, y))
        };
        let (p0, p1) = (point(ext.start_offset, [0.0, 0.0]), point(ext.end_offset, [0.0, 1.0]));
        let (start, end) = (finite(ext.start_radius), finite(node.radius));
        let device = self.device_scale();
        let span = (end - start).abs() * device;
        let levels = ((span / 6.0).ceil() as usize + 1).clamp(2, 8);
        let mut plus = Paint::default();
        plus.set_blend_mode(BlendMode::Plus);
        let mut dest_in = Paint::default();
        dest_in.set_blend_mode(BlendMode::DstIn);
        self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds));
        super::emit::layer(self.canvas, Some(&bounds));
        for level in 0..levels {
            let at = level as f32 / (levels - 1) as f32;
            let radius = start + (end - start) * at;
            let step = 1.0 / (levels - 1) as f32;
            let (colors, positions): (Vec<Color4f>, Vec<f32>) = [(at - step, 0.0), (at, 1.0), (at + step, 0.0)]
                .into_iter()
                .filter(|(position, _)| (-1e-6..=1.0 + 1e-6).contains(position))
                .map(|(position, alpha)| (Color4f::new(0.0, 0.0, 0.0, alpha), position.clamp(0.0, 1.0)))
                .unzip();
            let ramp = Gradient::new(Colors::new(&colors, Some(&positions), TileMode::Clamp, None), Interpolation::default());
            dest_in.set_shader(gradients::linear_gradient((p0, p1), &ramp, None));
            self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&plus));
            super::emit::layer(self.canvas, Some(&bounds));
            let mut blurred = Paint::default();
            let sigma = draw::tent_sigma(radius as f64);
            if sigma > 0.0 {
                blurred.set_image_filter(draw::wide_blur(sigma, device, None, None));
            }
            self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&blurred));
            super::emit::layer(self.canvas, Some(&bounds));
            self.draw(child);
            self.canvas.restore();
            self.canvas.draw_rect(bounds, &dest_in);
            super::emit::fill_rect(self.canvas, &bounds, Color4f::new(0.5, 0.5, 0.5, 1.0));
            self.canvas.restore();
        }
        self.canvas.restore();
        let _ = MAX_SIGMA;
    }
}

/// A gradient as a shader in the node's own coordinates, as draw.rs's, from
/// the tree's paint: its transform is the plugin API's `gradientTransform`,
/// and an angular one comes with stops either side of its seam
fn gradient(kit: &Kit, paint: &rt::Paint, kind: PaintType, w: f32, h: f32) -> Option<skia_safe::Shader> {
    let t = rt::to_matrix(&paint.transform.unwrap_or(rt::IDENTITY));
    let mut local = Matrix::scale((w, h));
    local.pre_concat(&t.invert()?);
    let ramp = paint.gradient.as_ref()?;
    if ramp.stops.is_empty() {
        return None;
    }
    let opacity = paint.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
    let colors: Vec<Color4f> = ramp.stops.iter().map(|stop| draw::color(rgba(stop.color), opacity)).collect();
    let positions: Vec<f32> = ramp.stops.iter().map(|stop| stop.position).collect();
    let tile = if ramp.kind == Some(rt::GradientType::Wrap) { TileMode::Repeat } else { TileMode::Clamp };
    let spec = Gradient::new(Colors::new(&colors, Some(&positions), tile, None), interpolation(paint));
    let center = Point::new(0.5, 0.5);
    match kind {
        PaintType::GradientLinear => gradients::linear_gradient((Point::new(0.0, 0.5), Point::new(1.0, 0.5)), &spec, &local),
        PaintType::GradientRadial => gradients::radial_gradient((center, 0.5), &spec, &local),
        PaintType::GradientDiamond => {
            let ramp = gradients::linear_gradient((Point::new(0.0, 0.5), Point::new(1.0, 0.5)), &spec, None)?;
            kit.diamond.as_ref()?.make_shader(Data::new_copy(&[]), &[ramp.into()], Some(&local))
        }
        _ => gradients::sweep_gradient(center, (0.0, 360.0), &spec, &local),
    }
}

/// How the gradient mixes its colors. Figma's default mixes them as they
/// are, not premultiplied
fn interpolation(paint: &rt::Paint) -> Interpolation {
    use skia_safe::gradient::interpolation::{ColorSpace, HueMethod};
    let mut out = Interpolation::default();
    out.color_space = match paint.interpolation_color_space {
        Some(rt::InterpolationColorSpace::Oklab) => ColorSpace::OKLab,
        Some(rt::InterpolationColorSpace::Oklch) => ColorSpace::OKLCH,
        Some(rt::InterpolationColorSpace::Hsl) => ColorSpace::HSL,
        Some(rt::InterpolationColorSpace::SrgbLinear) => ColorSpace::SRGBLinear,
        _ => return out,
    };
    out.hue_method = match paint.interpolation_hue_method {
        Some(rt::InterpolationHueMethod::Longer) => HueMethod::Longer,
        Some(rt::InterpolationHueMethod::Increasing) => HueMethod::Increasing,
        Some(rt::InterpolationHueMethod::Decreasing) => HueMethod::Decreasing,
        _ => HueMethod::Shorter,
    };
    out
}

/// The key of the image `paint` fills with, adjusted ones kept apart
pub(super) fn image_key(paint: &rt::Paint) -> Option<String> {
    let image = paint.image.as_ref().or(paint.thumbnail.as_ref())?;
    let hash = image.hex();
    Some(match paint.filter_color_adjust {
        Some(adjust) => {
            let adjust = Adjust {
                exposure: adjust.exposure,
                contrast: adjust.contrast / 0.3,
                saturation: adjust.vibrance,
                temperature: adjust.temperature,
                tint: adjust.tint,
                highlights: adjust.highlights,
                shadows: adjust.shadows,
            };
            if adjust == Adjust::default() { hash } else { format!("{hash}#{}", adjust.key()) }
        }
        None => hash,
    })
}

/// How image fill `paint` lays `img` in a `w` by `h` node: what maps the
/// original's pixels to the node's coordinates, and how it tiles
pub(super) fn image_layout(paint: &rt::Paint, w: f32, h: f32, img: &Img) -> Option<(Matrix, TileMode)> {
    let (iw, ih) = (img.original.0 as f32, img.original.1 as f32);
    let mode = paint.image_scale_mode.unwrap_or(rt::ImageScaleMode::Fill);
    let (to_node, tile) = match mode {
        rt::ImageScaleMode::Tile => {
            let factor = paint.image_scaling_factor.filter(|f| f.is_finite() && *f > 0.0).unwrap_or(1.0);
            (Matrix::scale((factor, factor)), TileMode::Repeat)
        }
        rt::ImageScaleMode::Stretch => {
            let t = rt::to_matrix(&paint.transform.unwrap_or(rt::IDENTITY));
            let mut m = Matrix::scale((w, h));
            m.pre_concat(&t.invert()?);
            m.pre_scale((1.0 / iw, 1.0 / ih), None);
            (m, TileMode::Decal)
        }
        _ => {
            // FILL covers the box, FIT fits in it, both centered, turned
            // in quarter turns
            let turns = ((paint.image_rotation_deg.unwrap_or(0.0) / 90.0).round() as i64).rem_euclid(4);
            let (rw, rh) = if turns % 2 == 1 { (ih, iw) } else { (iw, ih) };
            let s = if mode == rt::ImageScaleMode::Fit { (w / rw).min(h / rh) } else { (w / rw).max(h / rh) };
            let mut m = Matrix::translate((w / 2.0, h / 2.0));
            m.pre_rotate(turns as f32 * 90.0, None);
            m.pre_scale((s, s), None);
            m.pre_translate((-iw / 2.0, -ih / 2.0));
            (m, TileMode::Decal)
        }
    };
    Some((to_node, tile))
}

/// Whether `id` draws one paint over one path, and nothing else
pub(super) fn single_draw(tree: &Tree, id: u32) -> bool {
    let mut id = id;
    for _ in 0..8 {
        let Some(node) = tree.node(id) else { return false };
        match node.op {
            Op::StaticTransformBinding => match node.child {
                Some(child) => id = child,
                None => return false,
            },
            Op::Group if node.children.len() == 1 => id = node.children[0],
            Op::Fill => return node.geometry.as_ref().is_some_and(|g| g.path.is_some() && g.paints.len() == 1),
            Op::FillGlyphs => return node.ext().painted_glyph_geometry.as_ref().is_some_and(|g| g.paints.len() == 1),
            _ => return false,
        }
    }
    false
}
