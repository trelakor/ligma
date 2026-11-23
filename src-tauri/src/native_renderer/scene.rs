//! The mirrored scene: the current page's nodes as `page.js` reads them,
//! parsed once into paths and paints ready to draw, filled in as the page
//! streams them, nearest the view first, and kept up to date with its
//! `changes()`

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;
use skia_safe::{Matrix, Path, PathFillType, Rect};

/// A node as `page.js` sends it
#[derive(Deserialize)]
pub struct RawNode {
    pub id: String,
    pub t: String,
    pub p: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub tf: Option<[f64; 6]>,
    pub w: Option<f64>,
    pub h: Option<f64>,
    #[serde(default, deserialize_with = "lenient")]
    pub rb: Option<[f64; 4]>,
    pub o: Option<f64>,
    pub bm: Option<String>,
    #[serde(default)]
    pub clip: bool,
    pub mask: Option<String>,
    pub fills: Option<Value>,
    pub strokes: Option<Value>,
    pub sa: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub fx: Option<Vec<Effect>>,
    pub fg: Option<Vec<(String, String)>>,
    pub sg: Option<Vec<(String, String)>>,
    #[serde(default, deserialize_with = "lenient")]
    pub cr: Option<[f64; 4]>,
    /// Corner smoothing, 0 to 1
    pub cs: Option<f64>,
    /// Text with more than one fill: each style's fills, in the order of
    /// its outlines in `fg`
    pub segs: Option<Vec<Value>>,
}

/// `T`, or None where it's malformed rather than failing the whole page
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> Result<Option<T>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

/// The start of reading a page: what's needed before its nodes come
#[derive(Deserialize)]
pub struct Begin {
    /// The page's batches are kept under this and their number
    #[serde(default)]
    pub stream: String,
    pub page: String,
    #[serde(default)]
    pub bg: Option<Rgba>,
    /// The page's background is hidden, so Figma shows a checkerboard
    #[serde(default)]
    pub transparent: bool,
    /// Figma's dark theme, which darkens its checkerboard
    #[serde(default)]
    pub dark: bool,
    /// The file's colors are Display P3 rather than sRGB
    #[serde(default)]
    pub p3: bool,
    /// Figma's noise octaves as its GPU hashes them, see `NOISE_OCTAVES`
    #[serde(default)]
    pub octaves: Option<Vec<f32>>,
}

/// A part of the page as it streams in
#[derive(Deserialize)]
pub struct Batch {
    pub items: Vec<Update>,
}

#[derive(Deserialize)]
pub struct Update {
    pub parent: String,
    /// Place among the parent's children
    pub index: i64,
    pub nodes: Vec<RawNode>,
}

/// Parent of nodes kept only to be drawn as pattern tiles, from other pages
pub const EXTERNAL: &str = "__ext";

#[derive(Deserialize)]
pub struct Changes {
    #[serde(default)]
    pub resync: bool,
    #[serde(default)]
    pub updates: Vec<Update>,
    #[serde(default)]
    pub removed: Vec<String>,
}

/// What `Scene::apply` changed: subtrees sent whole, and nodes whose
/// children came or went
#[derive(Default)]
pub struct Changed {
    pub roots: HashSet<String>,
    pub parents: HashSet<String>,
}

/// Where a subtree goes among its parent's children
#[derive(Clone, Copy)]
pub enum Place {
    /// At this index, or last: a change, with its siblings all there
    At(Option<usize>),
    /// By its index in Figma, among siblings that may not have come yet
    Rank(usize),
}

/// `Changes` or a `Batch` with their paths parsed, off the render thread
pub struct Prepared {
    pub removed: Vec<String>,
    /// Parent, place among its children, and the subtree
    pub updates: Vec<(String, Place, Vec<Node>)>,
}

impl Changes {
    pub fn prepare(self) -> Prepared {
        let updates = self.updates.into_iter().map(|update| (update.parent, Place::At(usize::try_from(update.index).ok()), update.nodes));
        Prepared { removed: self.removed, updates: parse_updates(updates.collect()) }
    }
}

impl Batch {
    pub fn prepare(self) -> Prepared {
        let updates = self.items.into_iter().map(|item| (item.parent, Place::Rank(item.index.max(0) as usize), item.nodes));
        Prepared { removed: Vec::new(), updates: parse_updates(updates.collect()) }
    }
}

/// Parses all the updates' nodes together, to spread them across cores
fn parse_updates(updates: Vec<(String, Place, Vec<RawNode>)>) -> Vec<(String, Place, Vec<Node>)> {
    let mut heads = Vec::new();
    let mut all = Vec::new();
    for (parent, place, nodes) in updates {
        heads.push((parent, place, nodes.len()));
        all.extend(nodes);
    }
    let mut parsed = parse_all(all).into_iter();
    heads.into_iter().map(|(parent, place, size)| (parent, place, parsed.by_ref().take(size).collect())).collect()
}

#[derive(Clone, Copy, Deserialize, Debug)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    #[serde(default = "one")]
    pub a: f32,
}

impl Rgba {
    /// A Display P3 color in sRGB, as Figma's renderer shows it on an sRGB
    /// canvas: converted in linear light, and what's outside sRGB clipped
    pub fn from_p3(self) -> Rgba {
        let linear = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        let encode = |v: f32| {
            let v = v.clamp(0.0, 1.0);
            if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
        };
        let (r, g, b) = (linear(self.r), linear(self.g), linear(self.b));
        Rgba {
            r: encode(1.2249401 * r - 0.2249404 * g),
            g: encode(-0.0420569 * r + 1.0420571 * g),
            b: encode(-0.0196376 * r - 0.0786361 * g + 1.0982735 * b),
            a: self.a,
        }
    }
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

#[derive(Clone, Deserialize, Debug)]
pub struct Stop {
    pub position: f32,
    pub color: Rgba,
}

#[derive(Clone, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Paint {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default = "one")]
    pub opacity: f32,
    pub blend_mode: Option<String>,
    pub color: Option<Rgba>,
    pub gradient_transform: Option<[[f64; 3]; 2]>,
    pub gradient_stops: Option<Vec<Stop>>,
    pub image_hash: Option<String>,
    pub scale_mode: Option<String>,
    pub image_transform: Option<[[f64; 3]; 2]>,
    pub scaling_factor: Option<f64>,
    #[serde(default)]
    pub rotation: f64,
    pub filters: Option<HashMap<String, f64>>,
    /// Pattern fills: the node tiled, how, and how far apart, as a share of
    /// the tile
    pub source_node_id: Option<String>,
    pub tile_type: Option<String>,
    pub spacing: Option<Vec2>,
    pub horizontal_alignment: Option<String>,
    pub vertical_alignment: Option<String>,
}

impl Paint {
    /// Image adjustments (exposure, contrast and so on) other than none
    pub fn has_filters(&self) -> bool {
        self.filters.as_ref().is_some_and(|filters| filters.values().any(|value| value.abs() > 1e-6))
    }
}

#[derive(Clone, Copy, Deserialize, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    #[serde(rename = "type")]
    pub kind: String,
    pub color: Option<Rgba>,
    #[serde(default)]
    pub offset: Vec2,
    #[serde(default)]
    pub radius: f64,
    #[serde(default)]
    pub spread: f64,
    pub blend_mode: Option<String>,
    #[serde(default)]
    pub show_shadow_behind_node: bool,
    /// PROGRESSIVE blurs go from `start_radius` at `start_offset` to
    /// `radius` at `end_offset`, as shares of the node's box
    pub blur_type: Option<String>,
    #[serde(default)]
    pub start_radius: f64,
    pub start_offset: Option<Vec2>,
    pub end_offset: Option<Vec2>,
    /// Noise: grain size and how much of it shows
    pub noise_size: Option<f64>,
    pub density: Option<f64>,
    /// Grain size along each axis, where it differs
    pub noise_size_vector: Option<Vec2>,
    /// MONOTONE, DUOTONE or MULTITONE
    pub noise_type: Option<String>,
    /// Duotone noise's second color
    pub secondary_color: Option<Rgba>,
    /// A texture effect keeps to the node's shape
    pub clip_to_shape: Option<bool>,
    /// A custom effect's code, by the file's name for it
    pub id: Option<String>,
    /// A custom effect's settings, by key, and each key's name and default
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub property_metadata: serde_json::Map<String, serde_json::Value>,
}

impl Effect {
    /// A custom effect's settings by name, as its code reads them
    pub fn params(&self) -> serde_json::Value {
        let mut params = serde_json::Map::new();
        for (key, meta) in &self.property_metadata {
            let Some(name) = meta.get("name").and_then(|name| name.as_str()) else {
                continue;
            };
            let value = self.properties.get(key).or_else(|| meta.get("defaultValue")).cloned().unwrap_or(serde_json::Value::Null);
            params.insert(name.to_string(), value);
        }
        serde_json::Value::Object(params)
    }

    pub fn progressive(&self) -> bool {
        self.blur_type.as_deref() == Some("PROGRESSIVE")
    }
}

/// Paints drawn natively. Others are custom paints, drawn as Figma renders
/// them
const KNOWN_PAINTS: [&str; 8] =
    ["SOLID", "GRADIENT_LINEAR", "GRADIENT_RADIAL", "GRADIENT_ANGULAR", "GRADIENT_DIAMOND", "IMAGE", "PATTERN", "VIDEO"];

/// Fills of a node: text with more than one style has several, which its
/// outline doesn't tell apart
#[derive(Clone, Debug)]
pub enum Fills {
    Paints(Vec<Paint>),
    Mixed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StrokeAlign {
    Inside,
    Outside,
    Center,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Frame,
    Group,
    Section,
    Boolean,
    Text,
    Shape,
    Other,
}

impl Kind {
    fn from(name: &str) -> Self {
        match name {
            "FRAME" | "COMPONENT" | "COMPONENT_SET" | "INSTANCE" => Self::Frame,
            "GROUP" => Self::Group,
            "SECTION" => Self::Section,
            "BOOLEAN_OPERATION" => Self::Boolean,
            "TEXT" => Self::Text,
            "RECTANGLE" | "ELLIPSE" | "POLYGON" | "STAR" | "VECTOR" | "LINE" => Self::Shape,
            _ => Self::Other,
        }
    }
}

pub struct Node {
    pub id: String,
    pub kind: Kind,
    pub type_name: String,
    /// Parent's id as sent, the page's for top-level nodes
    pub parent_id: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub hidden: bool,
    pub transform: Matrix,
    pub size: (f64, f64),
    /// Absolute bounds of everything it draws, effects and children included
    pub bounds: Option<Rect>,
    pub opacity: f32,
    pub blend: Option<String>,
    pub clip: bool,
    pub mask: Option<String>,
    pub fills: Fills,
    pub strokes: Vec<Paint>,
    /// INSIDE, OUTSIDE or CENTER. Figma's stroke outlines for INSIDE and
    /// OUTSIDE are of a centered stroke twice as wide, which Figma's own
    /// renderer cuts down with the fill's outline
    pub stroke_align: StrokeAlign,
    pub effects: Vec<Effect>,
    pub fill_geometry: Vec<Path>,
    pub stroke_geometry: Vec<Path>,
    pub radii: Option<[f64; 4]>,
    /// Corner smoothing, 0 to 1, which makes corners squircles
    pub smoothing: f64,
    /// Text with more than one fill: the fills of each outline in
    /// `fill_geometry`
    pub segment_fills: Vec<Vec<Paint>>,
    /// Index among its parent's children in Figma
    pub rank: usize,
    /// What's only drawn close to Figma's look, if anything
    pub approximate: Option<&'static str>,
}

fn paints(value: Option<Value>) -> Fills {
    match value {
        Some(Value::String(_)) => Fills::Mixed,
        Some(value) => Fills::Paints(serde_json::from_value(value).unwrap_or_default()),
        None => Fills::Paints(Vec::new()),
    }
}

fn geometry(raw: Option<Vec<(String, String)>>) -> Vec<Path> {
    raw.unwrap_or_default()
        .into_iter()
        .filter_map(|(rule, data)| {
            let fill = if rule == "EVENODD" { PathFillType::EvenOdd } else { PathFillType::Winding };
            skia_safe::utils::parse_path::from_svg(&data).map(|path| path.with_fill_type(fill))
        })
        .collect()
}

impl Node {
    fn parse(raw: RawNode) -> Self {
        let kind = Kind::from(&raw.t);
        // Figma's [a, b, c, d, e, f] maps (x, y) to (ax + cy + e, bx + dy + f)
        let transform = raw.tf.map_or(Matrix::new_identity(), |[a, b, c, d, e, f]| {
            Matrix::new_all(a as f32, c as f32, e as f32, b as f32, d as f32, f as f32, 0.0, 0.0, 1.0)
        });
        let bounds = raw.rb.map(|[x, y, w, h]| Rect::from_xywh(x as f32, y as f32, w as f32, h as f32));
        let fills = paints(raw.fills);
        let strokes = match paints(raw.strokes) {
            Fills::Paints(paints) => paints,
            Fills::Mixed => Vec::new(),
        };
        let mut node = Self {
            id: raw.id,
            kind,
            type_name: raw.t,
            parent_id: raw.p,
            parent: None,
            children: Vec::new(),
            hidden: raw.hidden,
            transform,
            size: (raw.w.unwrap_or(0.0), raw.h.unwrap_or(0.0)),
            bounds,
            opacity: raw.o.unwrap_or(1.0) as f32,
            blend: raw.bm,
            clip: raw.clip,
            mask: raw.mask,
            fills,
            strokes,
            stroke_align: match raw.sa.as_deref() {
                Some("INSIDE") => StrokeAlign::Inside,
                Some("OUTSIDE") => StrokeAlign::Outside,
                _ => StrokeAlign::Center,
            },
            effects: raw.fx.unwrap_or_default(),
            fill_geometry: geometry(raw.fg),
            stroke_geometry: geometry(raw.sg),
            radii: raw.cr,
            smoothing: raw.cs.unwrap_or(0.0),
            segment_fills: raw
                .segs
                .unwrap_or_default()
                .into_iter()
                .map(|fills| match paints(Some(fills)) {
                    Fills::Paints(paints) => paints,
                    Fills::Mixed => Vec::new(),
                })
                .collect(),
            rank: 0,
            approximate: None,
        };
        node.approximate = node.check();
        node
    }

    /// Its colors, given in Display P3, in sRGB
    fn colors_from_p3(&mut self) {
        let paint = |paint: &mut Paint| {
            paint.color = paint.color.map(Rgba::from_p3);
            for stop in paint.gradient_stops.iter_mut().flatten() {
                stop.color = stop.color.from_p3();
            }
        };
        if let Fills::Paints(fills) = &mut self.fills {
            fills.iter_mut().for_each(paint);
        }
        self.strokes.iter_mut().for_each(paint);
        self.segment_fills.iter_mut().flatten().for_each(paint);
        for effect in &mut self.effects {
            effect.color = effect.color.map(Rgba::from_p3);
            effect.secondary_color = effect.secondary_color.map(Rgba::from_p3);
        }
    }

    /// Bounds for a layer its effects filter. Its render bounds can be cut
    /// by a mask or a frame around it, which would cut what its blurs and
    /// shadows read near that edge, so they're grown by as far as those read
    pub fn layer_bounds(&self) -> Option<Rect> {
        let bounds = self.bounds?;
        let scale = self.transform.scale_x().hypot(self.transform.skew_y()).max(self.transform.skew_x().hypot(self.transform.scale_y()));
        let reach: f64 = self
            .effects
            .iter()
            .map(|effect| effect.radius.max(effect.start_radius) + effect.offset.x.abs().max(effect.offset.y.abs()) + effect.spread.abs())
            .sum();
        let reach = reach as f32 * scale + 1.0;
        Some(bounds.with_outset((reach, reach)))
    }

    /// Every fill, all styles' for text with several
    pub fn visible_fills(&self) -> Box<dyn Iterator<Item = &Paint> + '_> {
        match &self.fills {
            Fills::Paints(paints) => Box::new(paints.iter().filter(|paint| paint.visible && paint.opacity > 0.0)),
            Fills::Mixed => Box::new(self.segment_fills.iter().flatten().filter(|paint| paint.visible && paint.opacity > 0.0)),
        }
    }

    /// The fills of outline `index` of `fill_geometry`
    pub fn fills_of(&self, index: usize) -> Box<dyn Iterator<Item = &Paint> + '_> {
        match &self.fills {
            Fills::Paints(paints) => Box::new(paints.iter().filter(|paint| paint.visible && paint.opacity > 0.0)),
            Fills::Mixed => {
                let paints = self.segment_fills.get(index).or(self.segment_fills.first()).map_or(&[][..], Vec::as_slice);
                Box::new(paints.iter().filter(|paint| paint.visible && paint.opacity > 0.0))
            }
        }
    }

    pub fn visible_strokes(&self) -> impl Iterator<Item = &Paint> {
        self.strokes.iter().filter(|paint| paint.visible && paint.opacity > 0.0)
    }

    /// A box with one opaque solid fill: what a drop shadow can be drawn
    /// natively for, as a blurred rounded rectangle
    pub fn is_solid_box(&self) -> bool {
        matches!(self.kind, Kind::Frame | Kind::Shape)
            && (self.type_name != "VECTOR" && self.type_name != "ELLIPSE" && self.type_name != "STAR" && self.type_name != "POLYGON" && self.type_name != "LINE")
            && self.smoothing == 0.0
            && self.visible_fills().any(|paint| paint.kind == "SOLID" && paint.opacity >= 0.999 && paint.color.is_some_and(|color| color.a >= 0.999))
    }

    /// Has a custom effect or paint: code the file carries, which Figma runs,
    /// so it's drawn as Figma renders it
    pub fn custom(&self) -> bool {
        self.shaded() || self.custom_paint()
    }

    /// Whether it has custom effects, which run natively
    pub fn shaded(&self) -> bool {
        self.effects.iter().any(|effect| effect.kind == "SHADER" && effect.id.is_some())
    }

    /// Whether it has paints of kinds not drawn natively
    pub fn custom_paint(&self) -> bool {
        self.visible_fills().chain(self.visible_strokes()).any(|paint| !KNOWN_PAINTS.contains(&paint.kind.as_str()))
    }

    /// Its custom effects in order, each its code's name and settings
    pub fn shaders(&self) -> Vec<(String, serde_json::Value)> {
        self.effects.iter().filter(|effect| effect.kind == "SHADER").filter_map(|effect| Some((effect.id.clone()?, effect.params()))).collect()
    }

    /// The layer blur, if it has one
    pub fn layer_blur(&self) -> Option<&Effect> {
        self.effects.iter().find(|effect| effect.kind == "LAYER_BLUR" && effect.radius > 0.0)
    }

    /// What's only drawn close to Figma's look, checked in order of how
    /// often it comes up
    fn check(&self) -> Option<&'static str> {
        if self.hidden {
            return None;
        }
        for effect in &self.effects {
            match effect.kind.as_str() {
                "INNER_SHADOW" if effect.spread != 0.0 => return Some("inner shadow spread"),
                "DROP_SHADOW" | "INNER_SHADOW" | "LAYER_BLUR" => {}
                "BACKGROUND_BLUR" if effect.progressive() => return Some("progressive background blur"),
                "BACKGROUND_BLUR" => {}
                "NOISE" => {}
                // Kept to the shape, Figma only moves what's inside it about
                "TEXTURE" if effect.clip_to_shape == Some(true) => return Some("texture kept to the shape"),
                "TEXTURE" => {}
                "GLASS" => return Some("glass effect"),
                "SHADER" => {}
                _ => return Some("other effect"),
            }
        }
        if matches!(self.fills, Fills::Mixed) && self.segment_fills.len() != self.fill_geometry.len() {
            return Some("mixed fills");
        }
        if self.kind == Kind::Text && self.fill_geometry.is_empty() && self.visible_fills().next().is_some() {
            return Some("text without outlines");
        }
        for paint in self.visible_fills().chain(self.visible_strokes()) {
            match paint.kind.as_str() {
                "SOLID" | "GRADIENT_LINEAR" | "GRADIENT_RADIAL" | "GRADIENT_ANGULAR" | "PATTERN" => {}
                "IMAGE" | "GRADIENT_DIAMOND" => {}
                "VIDEO" => return Some("video fill"),
                _ => {}
            }
            if let Some(mode) = &paint.blend_mode {
                if !super::draw::supported(mode) {
                    return Some("blend mode");
                }
            }
        }
        if let Some(mode) = &self.blend {
            if !super::draw::supported(mode) {
                return Some("blend mode");
            }
        }
        None
    }
}

/// Parses nodes' paths on all cores: text outlines make up most of a page
fn parse_all(raw: Vec<RawNode>) -> Vec<Node> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
    let chunk = raw.len().div_ceil(threads).max(1);
    let mut raw = raw;
    let mut chunks = Vec::new();
    while !raw.is_empty() {
        let rest = raw.split_off(chunk.min(raw.len()));
        chunks.push(std::mem::replace(&mut raw, rest));
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| scope.spawn(move || chunk.into_iter().map(Node::parse).collect::<Vec<_>>()))
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap_or_default()).collect()
    })
}

const NOISE_OCTAVES: usize = 36;

/// Figma's noise seed, which a file can't change
const NOISE_SEED: f32 = 5277.0;

/// Noise octaves as Figma hashes them, where the tab couldn't say how its
/// GPU rounds them. Any GPU's grain differs from another's, this is a likely one
fn noise_octaves() -> Vec<f32> {
    let hash = |x: f32| (((x * 91.3458 + 12.345).sin() * 43758.547) as f32).rem_euclid(1.0);
    let mut out = vec![0.0; NOISE_OCTAVES];
    for field in 0..4 {
        for octave in 0..3 {
            let seed = (((NOISE_SEED + field as f32) * 91.3458 + octave as f32 * 57.123 + 12.345).sin() * 43758.547).rem_euclid(1.0);
            let at = (field * 3 + octave) * 3;
            out[at] = seed;
            out[at + 1] = hash(seed + 2.6457);
            out[at + 2] = hash(seed + 3.6055);
        }
    }
    out
}

#[derive(Default)]
pub struct Scene {
    pub page: String,
    /// The tab it's read from
    pub tab: Option<std::sync::Arc<str>>,
    pub background: Option<Rgba>,
    /// The page's background is hidden: Figma shows a checkerboard
    pub transparent: bool,
    pub dark: bool,
    /// Colors come in Display P3, and are kept in sRGB
    pub p3: bool,
    /// For each of noise's four fields, each of its three octaves' seed and
    /// two gradient scales
    pub noise_octaves: Vec<f32>,
    /// Nodes by slot. Removed ones leave an empty slot
    pub nodes: Vec<Option<Node>>,
    pub by_id: HashMap<String, usize>,
    /// The page's children, bottom first
    pub roots: Vec<usize>,
}

impl Scene {
    pub fn new(begin: Begin) -> Self {
        let noise_octaves = begin.octaves.filter(|octaves| octaves.len() == NOISE_OCTAVES).unwrap_or_else(noise_octaves);
        let background = if begin.p3 { begin.bg.map(Rgba::from_p3) } else { begin.bg };
        Scene { page: begin.page, background, transparent: begin.transparent, dark: begin.dark, p3: begin.p3, noise_octaves, ..Default::default() }
    }

    pub fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index).and_then(Option::as_ref)
    }

    fn rank_of(&self, slot: usize) -> usize {
        self.node(slot).map_or(0, |node| node.rank)
    }

    /// Adds `nodes`, a subtree in paint order with its root first, under
    /// its parent at `place`. Returns the root's slot
    fn insert(&mut self, nodes: Vec<Node>, page: &str, place: Place) -> Option<usize> {
        let mut first = None;
        for mut node in nodes {
            if self.p3 {
                node.colors_from_p3();
            }
            let slot = self.nodes.len();
            let external = node.parent_id == EXTERNAL;
            let parent = if node.parent_id == page || external { None } else { self.by_id.get(&node.parent_id).copied() };
            node.parent = parent;
            self.by_id.insert(node.id.clone(), slot);
            let is_first = first.is_none();
            if is_first {
                first = Some(slot);
            }
            let siblings: &[usize] = match parent {
                Some(parent) => self.node(parent).map_or(&[][..], |parent| parent.children.as_slice()),
                None if external => &[],
                None => &self.roots,
            };
            let at = match (is_first, place) {
                (true, Place::At(Some(at))) => at.min(siblings.len()),
                (true, Place::Rank(rank)) => {
                    node.rank = rank;
                    siblings.partition_point(|&sibling| self.rank_of(sibling) <= rank)
                }
                _ => siblings.len(),
            };
            if !(is_first && matches!(place, Place::Rank(_))) {
                node.rank = at;
            }
            self.nodes.push(Some(node));
            match parent {
                Some(parent) => {
                    if let Some(Some(parent)) = self.nodes.get_mut(parent) {
                        parent.children.insert(at, slot);
                    }
                }
                None if external => {}
                None => self.roots.insert(at, slot),
            }
        }
        first
    }

    /// Takes node `id` and everything under it out
    pub fn remove(&mut self, id: &str) {
        let Some(&slot) = self.by_id.get(id) else {
            return;
        };
        let Some(parent) = self.node(slot).map(|node| node.parent) else {
            return;
        };
        match parent {
            Some(parent) => {
                if let Some(Some(parent)) = self.nodes.get_mut(parent) {
                    parent.children.retain(|&child| child != slot);
                }
            }
            None => self.roots.retain(|&root| root != slot),
        }
        let mut stack = vec![slot];
        while let Some(slot) = stack.pop() {
            if let Some(node) = self.nodes.get_mut(slot).and_then(Option::take) {
                if self.by_id.get(&node.id) == Some(&slot) {
                    self.by_id.remove(&node.id);
                }
                stack.extend(node.children);
            }
        }
    }

    /// The unit node `slot` is drawn in: the top-level layer, or the layer
    /// right in a section, it's under, or itself (see render.rs)
    pub fn unit_of(&self, mut slot: usize) -> usize {
        while let Some(parent) = self.node(slot).and_then(|node| node.parent) {
            if self.node(parent).is_some_and(|parent| parent.kind == Kind::Section) {
                return slot;
            }
            slot = parent;
        }
        slot
    }

    /// Applies `changes`. Returns the units whose drawing changed, before
    /// and after, and what changed in them (see `Changed`)
    pub fn apply(&mut self, changes: Prepared) -> (HashSet<usize>, Changed) {
        let mut touched = HashSet::new();
        let mut changed = Changed::default();
        for id in &changes.removed {
            if let Some(&slot) = self.by_id.get(id) {
                touched.insert(self.unit_of(slot));
                if let Some(parent) = self.node(slot).and_then(|node| node.parent).and_then(|parent| self.node(parent)) {
                    changed.parents.insert(parent.id.clone());
                }
            }
            self.remove(id);
        }
        let page = self.page.clone();
        for (parent, place, nodes) in changes.updates {
            let Some(root_id) = nodes.first().map(|node| node.id.clone()) else {
                continue;
            };
            if let Some(&slot) = self.by_id.get(&root_id) {
                touched.insert(self.unit_of(slot));
            }
            changed.roots.insert(root_id.clone());
            changed.parents.insert(parent.clone());
            self.remove(&root_id);
            if parent != page && parent != EXTERNAL && !self.by_id.contains_key(&parent) {
                continue;
            }
            if let Some(slot) = self.insert(nodes, &page, place) {
                touched.insert(self.unit_of(slot));
            }
        }
        (touched, changed)
    }

    /// Whether node `id` looks other than before `changed`: it or something
    /// in it changed, came or went
    pub fn changed(&self, id: &str, changed: &Changed) -> bool {
        let Some(&slot) = self.by_id.get(id) else {
            return true;
        };
        // Replaced along with something it's in
        let mut at = Some(slot);
        while let Some(slot) = at {
            let Some(node) = self.node(slot) else { break };
            if changed.roots.contains(&node.id) {
                return true;
            }
            at = node.parent;
        }
        // Something in it changed
        let target = slot;
        changed.roots.iter().chain(&changed.parents).any(|other| {
            let mut at = self.by_id.get(other).copied();
            while let Some(slot) = at {
                if slot == target {
                    return true;
                }
                at = self.node(slot).and_then(|node| node.parent);
            }
            false
        })
    }

    pub fn live_nodes(&self) -> usize {
        self.by_id.len()
    }
}
