//! Draws the mirrored scene with Skia
//!
//! Each node is drawn from its outlines as Figma computed them: fills from
//! `fillGeometry`, strokes, already expanded, from `strokeGeometry`, and
//! text from its glyph outlines, which are its `fillGeometry` too. Effects
//! are Skia image filters on the node's layer: drop and inner shadows and
//! layer blur in one filter graph, background blur as a backdrop filter.
//! Pattern fills are their source node recorded once and tiled as a
//! picture shader. Noise and LINEAR_BURN, which Skia has no built-in for,
//! are small SkSL programs

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use skia_safe::canvas::{SaveLayerFlags, SaveLayerRec};
use skia_safe::image_filters::CropRect;
use skia_safe::gradient::{shaders as gradients, Colors, Gradient, Interpolation};
use skia_safe::{
    color_filters, image_filters, luma_color_filter, BlendMode, Blender, BlurStyle, Canvas, ClipOp, Color4f, ColorChannel, Data,
    FilterMode, Image, ImageFilter, MaskFilter, Matrix, MipmapMode, Paint, Path, PictureRecorder, Point, RRect, Rect,
    RuntimeEffect, SamplingOptions, Shader, TileMode, Vector,
};

use super::adjust::Adjust;
use super::scene::{Effect, Fills, Kind, Node, Paint as FigmaPaint, Rgba, Scene, StrokeAlign, Vec2};

/// Figma's blend modes Skia has built in
pub fn blend(mode: &str) -> Option<BlendMode> {
    Some(match mode {
        "PASS_THROUGH" | "NORMAL" => BlendMode::SrcOver,
        "DARKEN" => BlendMode::Darken,
        "MULTIPLY" => BlendMode::Multiply,
        "COLOR_BURN" => BlendMode::ColorBurn,
        "LIGHTEN" => BlendMode::Lighten,
        "SCREEN" => BlendMode::Screen,
        "COLOR_DODGE" => BlendMode::ColorDodge,
        "OVERLAY" => BlendMode::Overlay,
        "SOFT_LIGHT" => BlendMode::SoftLight,
        "HARD_LIGHT" => BlendMode::HardLight,
        "DIFFERENCE" => BlendMode::Difference,
        "EXCLUSION" => BlendMode::Exclusion,
        "HUE" => BlendMode::Hue,
        "SATURATION" => BlendMode::Saturation,
        "COLOR" => BlendMode::Color,
        "LUMINOSITY" => BlendMode::Luminosity,
        "LINEAR_DODGE" => BlendMode::Plus,
        _ => return None,
    })
}

/// Whether blend mode `mode` is drawn as Figma draws it
pub fn supported(mode: &str) -> bool {
    blend(mode).is_some() || mode == "LINEAR_BURN"
}

/// Blend modes drawn with Figma's own formulas, on premultiplied colors,
/// where Skia's differ from them. Color burn and dodge differ only in how
/// they guard against dividing by nearly nothing, and Skia's built-in ones
/// match Figma's canvas better under a layer's opacity
const BLENDERS: [(&str, &str); 3] = [
    ("LINEAR_BURN", LINEAR_BURN),
    ("SOFT_LIGHT", SOFT_LIGHT),
    ("LINEAR_DODGE", LINEAR_DODGE),
];

/// LINEAR_DODGE: the sum, clamped, with alpha as in source over. Skia's
/// plus adds alpha up as well
const LINEAR_DODGE: &str = r#"
half4 main(half4 src, half4 dst) {
    return half4(min(half3(1), src.rgb + dst.rgb), src.a + dst.a - src.a * dst.a);
}
"#;



/// A diamond gradient: the ramp, along x, by the distance from the middle
/// summed across both axes
const DIAMOND: &str = r#"
uniform shader ramp;
half4 main(float2 p) {
    float2 d = abs(p - 0.5);
    return ramp.eval(float2(2.0 * (d.x + d.y), 0.5));
}
"#;

/// Figma's luminance mask: the root of the weighted squares of the color as
/// kept, premultiplied, times its alpha
const LUMINANCE: &str = r#"
half4 main(half4 color) {
    float3 c = float3(color.rgb);
    return half4(0, 0, 0, half(sqrt(dot(c * c, float3(0.299, 0.587, 0.114))) * float(color.a)));
}
"#;

fn is_normal(mode: &Option<String>) -> bool {
    mode.as_deref().is_none_or(|mode| mode == "NORMAL" || mode == "PASS_THROUGH")
}

fn color(rgba: Rgba, alpha: f32) -> Color4f {
    Color4f::new(rgba.r, rgba.g, rgba.b, (rgba.a * alpha).clamp(0.0, 1.0))
}

/// Whether node `slot` draws with a blend mode that reaches what's behind
/// it: its own, its fills' or strokes', or a child's through groups that
/// don't keep their children to themselves
pub fn blends_through(scene: &Scene, slot: usize) -> bool {
    let Some(node) = scene.node(slot) else {
        return false;
    };
    if node.hidden {
        return false;
    }
    let paints_blend = node.visible_fills().chain(node.strokes.iter().filter(|paint| paint.visible)).any(|paint| !is_normal(&paint.blend_mode));
    if !is_normal(&node.blend) || paints_blend {
        return true;
    }
    let isolated = node.opacity < 0.999 || !node.effects.is_empty() || node.blend.as_deref() == Some("NORMAL");
    !isolated && node.children.iter().any(|&child| blends_through(scene, child))
}

/// How far past node `slot`'s bounds a background blur in it reads what's
/// behind it, if it has one
pub fn blurs_behind(scene: &Scene, slot: usize) -> Option<f32> {
    let node = scene.node(slot).filter(|node| !node.hidden)?;
    let own = node.effects.iter().filter(|effect| effect.kind == "BACKGROUND_BLUR" || effect.kind == "GLASS").map(|effect| effect.radius as f32 * 1.5).reduce(f32::max);
    node.children.iter().filter_map(|&child| blurs_behind(scene, child)).chain(own).reduce(f32::max)
}

/// LINEAR_BURN on premultiplied colors: the separable blend formula with
/// B(s, d) = max(0, s + d - 1)
const LINEAR_BURN: &str = r#"
half4 main(half4 src, half4 dst) {
    half3 rgb = max(half3(0), src.rgb + dst.rgb - src.a * dst.a);
    return half4(rgb, src.a + dst.a - src.a * dst.a);
}
"#;

/// Figma's soft light, which differs from the W3C's: a light source
/// brightens by `cb (1 - cb)`, as in Pegtop's, and a dark one darkens by the
/// W3C's `D(cb) - cb`, so it isn't Skia's
const SOFT_LIGHT: &str = r#"
half4 main(half4 src, half4 dst) {
    float4 s = float4(src);
    float4 d = float4(dst);
    if (s.a == 0.0) {
        return dst;
    }
    float3 cs = s.rgb / s.a;
    float3 cb = d.a > 0.0 ? d.rgb / d.a : float3(0);
    float3 dark = mix((16.0 * cb - 12.0) * cb + 3.0, inversesqrt(cb + 1e-6) - 1.0, step(0.25, cb));
    float3 e = mix(dark, 1.0 - cb, step(0.5, cs));
    float3 mixed = cb * (1.0 + e * (2.0 * cs - 1.0));
    float3 rgb = s.rgb * (1.0 - d.a) + d.rgb * (1.0 - s.a) + s.a * d.a * mixed;
    return half4(half3(rgb), half(s.a + d.a - s.a * d.a));
}
"#;

/// The source over the destination's alpha, which undoes a blur's fading at
/// the edge of what it blurred
const DIVIDE: &str = r#"
half4 main(half4 src, half4 dst) {
    return dst.a > 0.0 ? half4(float4(src) / float(dst.a)) : half4(0);
}
"#;

/// Every pixel with any coverage made opaque, as Figma's silhouettes are
fn silhouette() -> skia_safe::ColorFilter {
    let mut alpha = [255u8; 256];
    alpha[0] = 0;
    color_filters::table_argb(Some(&alpha), None, None, None).expect("table color filter")
}

/// The standard deviation of Figma's blur of `radius`. It blurs with a
/// triangle as wide as the radius each side, which a Gaussian of the same
/// spread matches to within a level or two
pub fn tent_sigma(radius: f64) -> f32 {
    (radius / 6f64.sqrt()) as f32
}

/// Skia blurs no more than this many pixels wide, however wide it's asked
const MAX_SIGMA: f32 = 500.0;

/// A blur of `sigma` in the canvas's units, where one is `device` pixels.
/// Wider than Skia goes, it's blurred at a lower resolution instead, as
/// Figma's big blurs show zoomed in
fn wide_blur(sigma: f32, device: f32, input: Option<ImageFilter>, crop: Option<Rect>) -> Option<ImageFilter> {
    let crop = crop.map(CropRect::from).unwrap_or_default();
    let k = sigma * device / MAX_SIGMA;
    if k <= 1.0 {
        return image_filters::blur((sigma, sigma), TileMode::Decal, input, crop);
    }
    let linear = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
    let down = image_filters::matrix_transform(&Matrix::scale((1.0 / k, 1.0 / k)), linear, input);
    let blurred = image_filters::blur((sigma / k, sigma / k), TileMode::Decal, down, None);
    let up = image_filters::matrix_transform(&Matrix::scale((k, k)), linear, blurred);
    image_filters::offset((0.0, 0.0), up, crop)
}

/// Figma's fractal noise, which its noise and texture effects are made of:
/// three octaves of gradient noise on a grid hashed the way Figma does
const FBM: &str = r#"
float cellHash(float2 cell) {
    float t = dot(cell, float2(12.9898, 78.233));
    t = t - floor(t / 3.14159) * 3.14159;
    return fract(sin(t * 2.71828) * 438.5453 + 1.73205);
}

float component(float2 cell, float2 k, float s) {
    return 2.0 * (fract(sin(dot(k, cell) * 0.0625) * (s + 312.0)) - 0.5);
}

float2 slope(float2 cell, float3 octave) {
    float a = cellHash(cell);
    float s1 = octave.x * a * 1.61803 + 18.345;
    float s2 = octave.x * a * 0.61803 + 21.543;
    float gx = component(cell, float2(12.42, 78.23) * octave.y, s1);
    float gy = component(cell, float2(-19.98, 31.21) * octave.z, s2);
    return float2(gx, gy) / length(float2(gx, gy));
}

float fade(float t) {
    return t * t * (3.0 - 2.0 * t);
}

float gradientNoise(float3 octave, float2 p) {
    float2 c = floor(p);
    float2 f = fract(p);
    float2 u = float2(fade(f.x), fade(f.y));
    float a = dot(f, slope(c, octave));
    float b = dot(f - float2(1, 0), slope(c + float2(1, 0), octave));
    float d = dot(f - float2(0, 1), slope(c + float2(0, 1), octave));
    float e = dot(f - float2(1, 1), slope(c + float2(1, 1), octave));
    return mix(mix(a, b, u.x), mix(d, e, u.x), u.y);
}

// Three octaves at `q`, from -1.75 to 1.75
float fbm(float3 a, float3 b, float3 c, float2 q) {
    return gradientNoise(a, q) + gradientNoise(b, q * 2.0) / 2.0 + gradientNoise(c, q * 4.0) / 4.0;
}
"#;

/// Figma's noise effect: three octaves of hashed gradient noise in node
/// space, one field a channel, of which values in a band as wide as the
/// density show. Monotone shows `color` where the fields' luma is in the low
/// band, duotone `secondary` there and `color` in the high band, multitone
/// the fields as they are where the fourth one is in the low band. The
/// hashes are Figma's, so the grain lands where Figma's does
const NOISE: &str = r#"
uniform float2 frequency;
uniform float2 center;
uniform float kind;
uniform float density;
uniform half4 color;
uniform half4 secondary;
// Each field's octaves: the octave's seed and its two gradient scales
uniform float3 octaves[12];

float band(float v, bool upper) {
    float low = 10.0 * ((upper ? 0.75 : 0.25) - density / 4.0);
    float high = low + 10.0 * (density / 2.0);
    float x = v * 10.0;
    return step(low, x) * (1.0 - step(high, x));
}

// From 0 to 1
float field(float3 a, float3 b, float3 c, float2 q) {
    return (fbm(a, b, c, q) + 1.0) / 2.0;
}

half4 main(float2 p) {
    float2 at = (p - center) * frequency;
    float r = field(octaves[0], octaves[1], octaves[2], at);
    float g = field(octaves[3], octaves[4], octaves[5], at);
    float b = field(octaves[6], octaves[7], octaves[8], at);
    if (kind == 0.0) {
        float a = band(field(octaves[9], octaves[10], octaves[11], at), false) * color.a;
        return half4(half3(float3(2.0 * r - 0.5, 2.0 * g - 0.5, 2.0 * b - 0.5) * a), half(a));
    }
    float luma = 0.213 * r + 0.715 * g + 0.072 * b;
    float low = band(luma, false);
    if (kind == 1.0) {
        float a = low * color.a;
        return half4(color.rgb * half(a), half(a));
    }
    if (low == 1.0) {
        return half4(secondary.rgb * secondary.a, secondary.a);
    }
    float a = band(luma, true) * color.a;
    return half4(color.rgb * half(a), half(a));
}
"#;

/// Figma's texture effect moves what a node drew by two noise fields, as
/// far as its radius. This is where to, a quarter of it in red and green
/// about the middle, for a displacement map
const TEXTURE: &str = r#"
uniform float2 frequency;
uniform float2 center;
uniform float3 octaves[6];

half4 main(float2 p) {
    float2 at = (p - center) * frequency;
    float x = fbm(octaves[0], octaves[1], octaves[2], at);
    float y = fbm(octaves[3], octaves[4], octaves[5], at);
    return half4(half(x / 4.0 + 0.5), half(y / 4.0 + 0.5), 0.0, 1.0);
}
"#;

/// Puts the noise over what the node drew, only where it drew, by how much
const NOISE_OVER: &str = r#"
half4 main(half4 src, half4 dst) {
    half4 s = src * dst.a;
    return s + dst * (1.0 - s.a);
}
"#;

/// An image fill, decoded at about the size it shows
#[derive(Clone)]
pub struct Img {
    pub image: Image,
    /// The original's size in pixels, which image fills are laid out by
    pub original: (u32, u32),
    /// For a node as Figma renders it, the absolute area it covers
    pub area: Option<Rect>,
    /// Out of date, shown until the new one comes
    pub stale: bool,
}

impl Img {
    /// On the GPU, with its mipmaps
    pub fn bytes(&self) -> usize {
        (self.image.width() * self.image.height() * 4) as usize * 4 / 3
    }
}

type PatternKey = (usize, u64, i32);

/// What drawing shares across nodes and frames: compiled SkSL and pattern
/// tiles
/// An image as 4 by 4 nearest samples a pixel average, which is how Figma's
/// canvas shows images and how exact tiles draw them: `footprint` takes a
/// pixel's offsets to the image's texels
const BOX: &str = r#"
uniform shader image;
uniform float2x2 footprint;
half4 main(float2 p) {
    half4 sum = half4(0);
    for (int i = 0; i < 4; i++) {
        for (int j = 0; j < 4; j++) {
            float2 o = float2((float(i) + 0.5) / 4.0 - 0.5, (float(j) + 0.5) / 4.0 - 0.5);
            sum += image.eval(p + footprint * o);
        }
    }
    return sum / 16.0;
}
"#;

pub struct Kit {
    blenders: HashMap<&'static str, Blender>,
    diamond: Option<RuntimeEffect>,
    luminance: Option<skia_safe::ColorFilter>,
    /// Divides the source by the destination's alpha
    divide: Option<Blender>,
    noise: Option<RuntimeEffect>,
    noise_over: Option<Blender>,
    texture: Option<RuntimeEffect>,
    boxed: Option<RuntimeEffect>,
    /// Recorded pattern tiles, and the images they were waiting for
    patterns: RefCell<HashMap<PatternKey, (skia_safe::Picture, Vec<String>, Vec<(String, bool)>)>>,
}

impl Kit {
    pub fn new() -> Self {
        let blenders = BLENDERS
            .iter()
            .filter_map(|&(name, sksl)| {
                let effect = RuntimeEffect::make_for_blender(sksl, None).map_err(|error| log::warn!("native renderer: {name}: {error}")).ok()?;
                Some((name, effect.make_blender(Data::new_copy(&[]), None)?))
            })
            .collect();
        let diamond = RuntimeEffect::make_for_shader(DIAMOND, None).map_err(|error| log::warn!("native renderer: diamond: {error}")).ok();
        let luminance = RuntimeEffect::make_for_color_filter(LUMINANCE, None)
            .map_err(|error| log::warn!("native renderer: luminance: {error}"))
            .ok()
            .and_then(|effect| effect.make_color_filter(Data::new_copy(&[]), None));
        let divide = RuntimeEffect::make_for_blender(DIVIDE, None)
            .map_err(|error| log::warn!("native renderer: divide: {error}"))
            .ok()
            .and_then(|effect| effect.make_blender(Data::new_copy(&[]), None));
        let noise = RuntimeEffect::make_for_shader(format!("{FBM}{NOISE}"), None).map_err(|error| log::warn!("native renderer: noise: {error}")).ok();
        let texture = RuntimeEffect::make_for_shader(format!("{FBM}{TEXTURE}"), None).map_err(|error| log::warn!("native renderer: texture: {error}")).ok();
        let noise_over = RuntimeEffect::make_for_blender(NOISE_OVER, None)
            .map_err(|error| log::warn!("native renderer: noise over: {error}"))
            .ok()
            .and_then(|effect| effect.make_blender(Data::new_copy(&[]), None));
        let boxed = RuntimeEffect::make_for_shader(BOX, None).map_err(|error| log::warn!("native renderer: box: {error}")).ok();
        Self { blenders, diamond, luminance, divide, noise, noise_over, texture, boxed, patterns: RefCell::new(HashMap::new()) }
    }

    /// Forgets pattern tiles drawn without image `key`, or all of them
    pub fn forget_patterns(&self, key: Option<&str>) {
        let mut patterns = self.patterns.borrow_mut();
        match key {
            Some(key) => patterns.retain(|_, (_, missing, _)| !missing.iter().any(|missing| missing == key)),
            None => patterns.clear(),
        }
    }
}

#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub native: u32,
    /// Of those, drawn only close to Figma's look
    pub approximate: u32,
    /// Too small to see at this zoom, so not drawn
    pub skipped: u32,
}

impl std::ops::AddAssign for Counts {
    fn add_assign(&mut self, other: Self) {
        self.native += other.native;
        self.approximate += other.approximate;
        self.skipped += other.skipped;
    }
}

/// Text shorter than this on screen, in pixels, is drawn as a faint bar,
/// as its letters would be a blur anyway
const GREEK_BELOW: f32 = 3.0;


/// Image keys of nodes as Figma renders them start with this
pub const RENDER: &str = "node:";

/// Key of a node with its custom effects run on it, in its own space
pub const EFFECT: &str = "fx:";

/// Largest side a node is rendered at by Figma, in pixels
pub const RENDER_LIMIT: u32 = 4096;

/// How deep pattern fills may nest, patterns in patterns
const PATTERN_DEPTH: u8 = 3;

/// Draws nodes onto `canvas`, whose matrix maps absolute coordinates to
/// pixels, `px` of them per unit
pub struct Encoder<'a> {
    pub scene: &'a Scene,
    pub images: &'a HashMap<String, Img>,
    pub kit: &'a Kit,
    pub canvas: &'a Canvas,
    pub px: f32,
    pub counts: Counts,
    /// Images not here yet, and about how wide they show, in pixels
    pub missing_images: Vec<(String, f64)>,
    /// Images that show bigger than they were kept at, and how wide
    pub sharper_images: Vec<(String, f64)>,
    /// Every image drawn, and how wide it needs keeping for that
    pub used_images: Vec<(String, f64)>,
    /// Nodes pattern fills drew from, and whether each was there yet
    pub sources: Vec<(String, bool)>,
    /// Nodes as Figma renders them are wanted at exactly this zoom
    pub exact_renders: bool,
    /// Those not here at exactly this zoom, and how wide they'd be
    pub exact_images: Vec<(String, f64)>,
    /// Why the first approximate node is
    pub approximate_reason: Option<String>,
    /// Marks approximate nodes
    pub tint: bool,
    /// Edges are anti-aliased. Off when tiles are supersampled, as Figma
    /// covers each pixel with 16 samples, each in or out, which leaves no
    /// seam where shapes meet
    pub aa: bool,
    /// Nothing is drawn after this node, for drawing it over what's behind it
    pub stop_after: Option<usize>,
    /// A node drawn as its custom effects take it: without them, its opacity
    /// or its blend mode
    pub bare: Option<usize>,
    stopped: bool,
    depth: u8,
}

impl<'a> Encoder<'a> {
    pub fn new(scene: &'a Scene, images: &'a HashMap<String, Img>, kit: &'a Kit, canvas: &'a Canvas, px: f32) -> Self {
        Self {
            scene,
            images,
            kit,
            canvas,
            px,
            counts: Counts::default(),
            missing_images: Vec::new(),
            sharper_images: Vec::new(),
            used_images: Vec::new(),
            sources: Vec::new(),
            exact_renders: false,
            exact_images: Vec::new(),
            approximate_reason: None,
            tint: false,
            aa: true,
            stop_after: None,
            bare: None,
            stopped: false,
            depth: 0,
        }
    }

    /// Pixels a unit of the canvas's current coordinates covers
    fn device_scale(&self) -> f32 {
        let m = self.canvas.local_to_device_as_3x3();
        m.scale_x().hypot(m.skew_y()).max(m.skew_x().hypot(m.scale_y()))
    }

    /// Draws node `slot` and everything under it
    pub fn node(&mut self, slot: usize) {
        self.draw(slot, false);
    }

    /// `as_mask` draws a mask itself, for the siblings it masks
    fn draw(&mut self, slot: usize, as_mask: bool) {
        if self.stopped {
            return;
        }
        if self.stop_after == Some(slot) {
            self.draw_node(slot, as_mask);
            self.stopped = true;
            return;
        }
        self.draw_node(slot, as_mask);
    }

    fn draw_node(&mut self, slot: usize, as_mask: bool) {
        let scene = self.scene;
        let Some(node) = scene.node(slot) else {
            return;
        };
        if node.hidden || (node.mask.is_some() && !as_mask) {
            return;
        }
        if let Some(bounds) = node.bounds {
            if bounds.width().max(bounds.height()) * self.px < 0.5 {
                self.counts.skipped += 1;
                return;
            }
            if node.kind == Kind::Text && bounds.height() * self.px < GREEK_BELOW {
                self.greek(node, bounds);
                return;
            }
        }
        let bare = self.bare == Some(slot);
        if !as_mask && !bare && node.shaded() && !node.custom_paint() && self.effected(node) {
            return;
        }
        if !as_mask && !bare && node.custom_paint() && self.rendered(node) {
            return;
        }
        self.counts.native += 1;
        if let Some(reason) = node.approximate {
            self.counts.approximate += 1;
            if self.approximate_reason.is_none() {
                self.approximate_reason = Some(format!("{}: {reason}", node.type_name));
            }
        }
        if let Some(effect) = node.effects.iter().find(|effect| effect.kind == "BACKGROUND_BLUR" && effect.radius > 0.0) {
            self.backdrop(node, effect);
        }
        // Drop shadows of a plain box are drawn as blurred rounded
        // rectangles, which is much cheaper than filtering a layer, unless
        // they're wider than Skia blurs those
        let device = self.device_scale();
        let box_shadows = node.is_solid_box()
            && node.effects.iter().all(|effect| effect.kind == "DROP_SHADOW" && tent_sigma(effect.radius) * device <= MAX_SIGMA);
        let progressive = node.layer_blur().filter(|effect| effect.progressive());
        let filter = if box_shadows { None } else { self.filter(node, progressive.is_none()) };
        // Noise goes over what the node drew alone, so it needs its own layer
        let noisy = node.effects.iter().any(|effect| effect.kind == "NOISE");
        let (opacity, blend) = if bare { (1.0, None) } else { (node.opacity, node.blend.as_deref()) };
        let layer = opacity < 0.999 || blend.is_some_and(|mode| mode != "NORMAL" && mode != "PASS_THROUGH") || filter.is_some() || noisy;
        if layer {
            let mut paint = Paint::default();
            paint.set_alpha_f(opacity);
            self.set_blend(&mut paint, blend);
            let bounds = if filter.is_some() { node.layer_bounds() } else { node.bounds };
            paint.set_image_filter(filter);
            let mut rec = SaveLayerRec::default().paint(&paint);
            if let Some(bounds) = &bounds {
                rec = rec.bounds(bounds);
            }
            self.canvas.save_layer(&rec);
        }
        if box_shadows {
            for effect in &node.effects {
                self.box_shadow(node, effect);
            }
        }
        match progressive {
            Some(effect) => self.progressive(node, effect),
            None => self.content(node),
        }
        for effect in node.effects.iter().filter(|effect| effect.kind == "NOISE") {
            self.noise(node, effect);
        }
        if layer {
            self.canvas.restore();
        }
        if self.tint && node.approximate.is_some() {
            if let Some(bounds) = node.bounds {
                let mut paint = Paint::new(Color4f::new(1.0, 0.0, 0.6, 0.35), None);
                paint.set_anti_alias(self.aa);
                self.canvas.draw_rect(bounds, &paint);
            }
        }
    }

    /// Draws node `node` as Figma rendered it, if that's here. Custom effects
    /// are code the file carries, which Figma runs, so their look is taken
    /// from Figma. Until it comes the node is drawn without them
    fn rendered(&mut self, node: &Node) -> bool {
        let Some(bounds) = node.bounds else {
            return false;
        };
        let key = format!("{RENDER}{}", node.id);
        // Exact tiles want it at exactly their zoom, as custom effects often
        // work in whole pixels, like dithering. It can't be any bigger than
        // Figma renders it though
        let exact = self.exact_renders && bounds.width().max(bounds.height()) * self.px <= RENDER_LIMIT as f32;
        // Others a little sharper than shown, as tiles of a zoom level show
        // at up to 1.41 times its scale
        let width = (bounds.width() * self.px * if exact { 1.0 } else { 1.42 }) as f64;
        let Some(img) = self.images.get(&key) else {
            if !self.missing_images.iter().any(|(missing, _)| *missing == key) {
                self.missing_images.push((key, width));
            }
            return false;
        };
        let Some(area) = img.area else {
            return false;
        };
        let scale = img.image.width() as f32 / area.width();
        let at_scale = (scale / self.px - 1.0).abs() <= 0.002;
        if img.stale {
            self.missing_images.push((key, width));
        } else if exact {
            if !at_scale {
                self.exact_images.push((key, width));
            }
        } else if scale * 1.4 < self.px * 1.42 && img.image.width().max(img.image.height()) < RENDER_LIMIT as i32 {
            self.sharper_images.push((key, width));
        }
        self.counts.native += 1;
        let mut paint = Paint::default();
        self.set_blend(&mut paint, node.blend.as_deref());
        let sampling = if at_scale {
            SamplingOptions::new(FilterMode::Nearest, MipmapMode::None)
        } else if scale > self.px * 1.5 {
            SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear)
        } else {
            SamplingOptions::new(FilterMode::Linear, MipmapMode::None)
        };
        self.canvas.draw_image_rect_with_sampling_options(&img.image, None, area, sampling, &paint);
        true
    }

    /// Draws node `node` with its custom effects run on it, if that's here.
    /// As on Figma's canvas, they run once, at a pixel a unit whatever the
    /// zoom, on the node as drawn without them over its box, and what they
    /// draw covers the same box. A group's run on each child over its box
    fn effected(&mut self, node: &Node) -> bool {
        let scene = self.scene;
        let targets: Vec<(&Node, String)> = if node.kind == Kind::Group {
            node.children.iter().filter_map(|&child| scene.node(child)).filter(|child| !child.hidden).map(|child| (child, format!("{EFFECT}{}>{}", node.id, child.id))).collect()
        } else {
            vec![(node, format!("{EFFECT}{}", node.id))]
        };
        let mut images = Vec::new();
        let mut missing = false;
        for (target, key) in targets {
            if target.size.0 <= 0.0 || target.size.1 <= 0.0 {
                continue;
            }
            let width = target.size.0.ceil();
            self.used_images.push((key.clone(), width));
            match self.images.get(&key) {
                Some(img) if img.area.is_some() => {
                    if img.stale {
                        self.missing_images.push((key, width));
                    }
                    images.push((target, img));
                }
                _ => {
                    if !self.missing_images.iter().any(|(waiting, _)| *waiting == key) {
                        self.missing_images.push((key, width));
                    }
                    missing = true;
                }
            }
        }
        if missing || images.is_empty() {
            return false;
        }
        self.counts.native += 1;
        let mut paint = Paint::default();
        paint.set_alpha_f(node.opacity);
        self.set_blend(&mut paint, node.blend.as_deref());
        let layer = images.len() > 1 && (node.opacity < 0.999 || !is_normal(&node.blend));
        if layer {
            self.canvas.save_layer(&SaveLayerRec::default().paint(&paint));
            paint = Paint::default();
        }
        for (target, img) in images {
            let area = img.area.unwrap();
            let m = &target.transform;
            let px = self.px * m.scale_x().hypot(m.skew_y()).max(m.skew_x().hypot(m.scale_y()));
            let scale = img.image.width() as f32 / area.width();
            let sampling = if scale > px * 1.5 {
                SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear)
            } else {
                SamplingOptions::new(FilterMode::Linear, MipmapMode::None)
            };
            self.canvas.save();
            self.canvas.concat(m);
            self.canvas.draw_image_rect_with_sampling_options(&img.image, None, area, sampling, &paint);
            self.canvas.restore();
        }
        if layer {
            self.canvas.restore();
        }
        true
    }

    fn set_blend(&self, paint: &mut Paint, mode: Option<&str>) {
        match mode {
            None | Some("NORMAL") | Some("PASS_THROUGH") => {}
            Some(mode) => {
                if let Some(blender) = self.kit.blenders.get(mode) {
                    paint.set_blender(blender.clone());
                } else if let Some(mode) = blend(mode) {
                    paint.set_blend_mode(mode);
                }
            }
        }
    }

    /// The node's shadows, inner shadows and layer blur (unless `blur` is
    /// false) as one image filter for its layer: shadows behind it, inner
    /// shadows over it, then all of it blurred. Figma casts shadows from the
    /// node's silhouette, every pixel it covers at all counted as solid, so a
    /// faint stroke casts as dark a shadow as the fill. Where the shadow
    /// isn't shown behind the node, it's cut out wherever the node is at all
    fn filter(&self, node: &Node, blur: bool) -> Option<ImageFilter> {
        let mut behind = Vec::new();
        let mut over = Vec::new();
        let mut blur_sigma = None;
        // What the node drew, as its effects change it
        let mut content = None;
        let device = self.device_scale();
        let solid = || image_filters::color_filter(silhouette(), None, None);
        for effect in &node.effects {
            let sigma = tent_sigma(effect.radius);
            let offset = Vector::new(effect.offset.x as f32, effect.offset.y as f32);
            match effect.kind.as_str() {
                "DROP_SHADOW" => {
                    let Some(rgba) = effect.color else { continue };
                    let spread = effect.spread as f32;
                    // A spread shadow is cast by the node's outline grown by
                    // the spread, whatever its fills show
                    let input = if spread != 0.0 { self.spread_outline(node, spread).or_else(solid) } else { solid() };
                    let tint = color_filters::blend(color(rgba, 1.0).to_color(), BlendMode::SrcIn);
                    let shadow = image_filters::color_filter(tint?, input, None);
                    let mut shadow = image_filters::offset(offset, wide_blur(sigma, device, shadow, None), None);
                    if !effect.show_shadow_behind_node {
                        shadow = image_filters::blend(BlendMode::DstOut, shadow, solid(), None);
                    }
                    behind.push(shadow);
                }
                "INNER_SHADOW" => {
                    let Some(rgba) = effect.color else { continue };
                    let reach = sigma * 3.0 + offset.x.abs().max(offset.y.abs()) + effect.spread.abs() as f32 + 1.0;
                    let crop = CropRect::from(node.bounds.map(|bounds| CropRect::from(bounds.with_outset((reach, reach)))));
                    // The outside of the shape in the shadow's color...
                    let outside = color_filters::matrix_row_major(
                        &[0.0, 0.0, 0.0, 0.0, rgba.r, 0.0, 0.0, 0.0, 0.0, rgba.g, 0.0, 0.0, 0.0, 0.0, rgba.b, 0.0, 0.0, 0.0, -rgba.a, rgba.a],
                        None,
                    );
                    let mut shadow = image_filters::color_filter(outside, solid(), crop);
                    if effect.spread > 0.0 {
                        let spread = effect.spread as f32;
                        shadow = image_filters::dilate((spread, spread), shadow, crop);
                    }
                    // ...moved and blurred, kept inside the shape
                    shadow = image_filters::offset(offset, shadow, crop);
                    if sigma > 0.0 {
                        shadow = wide_blur(sigma, device, shadow, node.bounds.map(|bounds| bounds.with_outset((reach, reach))));
                    }
                    over.push(image_filters::blend(BlendMode::SrcIn, None, shadow, None));
                }
                "LAYER_BLUR" if blur && effect.radius > 0.0 => blur_sigma = Some(sigma),
                "TEXTURE" if effect.radius > 0.0 => content = self.texture(node, effect).or(content),
                _ => {}
            }
        }
        let mut filter = if behind.is_empty() && over.is_empty() {
            content
        } else {
            let layers = behind.into_iter().chain([content]).chain(over);
            image_filters::merge(layers, None)
        };
        if let Some(sigma) = blur_sigma {
            filter = wide_blur(sigma, device, filter, None);
        }
        filter
    }

    /// What the node drew moved about by Figma's texture `effect`. Unlike
    /// Figma's, it reads past the shape, so the edges roughen both ways
    fn texture(&self, node: &Node, effect: &Effect) -> Option<ImageFilter> {
        let texture = self.kit.texture.as_ref()?;
        let size = effect.noise_size_vector.unwrap_or(Vec2 { x: effect.noise_size.unwrap_or(1.0), y: effect.noise_size.unwrap_or(1.0) });
        let (w, h) = node.size;
        let values = [(1.0 / size.x.max(1e-3)) as f32, (1.0 / size.y.max(1e-3)) as f32, (w / 2.0) as f32, (h / 2.0) as f32];
        let mut uniforms = Vec::with_capacity((values.len() + 18) * 4);
        for value in values.into_iter().chain(self.scene.noise_octaves.iter().take(18).copied()) {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        let shader = texture.make_shader(Data::new_copy(&uniforms), &[], Some(&node.transform))?;
        let field = image_filters::shader(shader, None);
        let scale = node.transform.scale_x().hypot(node.transform.skew_y());
        image_filters::displacement_map((ColorChannel::R, ColorChannel::G), 4.0 * effect.radius as f32 * scale, field, None, None)
    }

    /// The node's outline grown (or shrunk) by `spread`, solid, as an image
    /// filter's input. Corners stay as sharp as they are
    fn spread_outline(&self, node: &Node, spread: f32) -> Option<ImageFilter> {
        let outline = self.outline(node);
        let bounds = node.transform.map_rect(outline.bounds().with_outset((spread.abs() + 1.0, spread.abs() + 1.0))).0;
        let mut recorder = PictureRecorder::new();
        let canvas = recorder.begin_recording(bounds, false);
        canvas.concat(&node.transform);
        let mut paint = Paint::new(Color4f::new(0.0, 0.0, 0.0, 1.0), None);
        paint.set_anti_alias(self.aa);
        canvas.draw_path(&outline, &paint);
        paint.set_style(skia_safe::PaintStyle::Stroke);
        paint.set_stroke_width(spread.abs() * 2.0);
        paint.set_stroke_join(skia_safe::PaintJoin::Miter);
        if spread < 0.0 {
            paint.set_blend_mode(BlendMode::Clear);
        }
        canvas.draw_path(&outline, &paint);
        let picture = recorder.finish_recording_as_picture(None)?;
        image_filters::picture(picture, None)
    }

    /// Fills, children and strokes
    fn content(&mut self, node: &Node) {
        self.paints(node, true);
        if !node.children.is_empty() {
            let clip = node.clip && node.kind != Kind::Group;
            if clip {
                self.canvas.save();
                self.canvas.clip_path(&self.box_shape(node).with_transform(&node.transform), ClipOp::Intersect, self.aa);
            }
            self.children(node);
            if clip {
                self.canvas.restore();
            }
        }
        self.paints(node, false);
    }

    /// Text too small to read, as a bar in its color
    fn greek(&mut self, node: &Node, bounds: Rect) {
        self.counts.skipped += 1;
        let Some((alpha, rgba)) = node.visible_fills().find_map(|paint| paint.color.map(|color| (color.a * paint.opacity, color))) else {
            return;
        };
        let bar = bounds.with_inset((0.0, bounds.height() * 0.2));
        let paint = Paint::new(color(rgba, alpha * 0.35 / rgba.a.max(1e-3)), None);
        self.canvas.draw_rect(bar, &paint);
    }

    /// A section's own fills and strokes, without what's in it, which is
    /// drawn (and culled) separately
    pub fn section(&mut self, slot: usize) {
        let scene = self.scene;
        let Some(node) = scene.node(slot) else {
            return;
        };
        if node.hidden {
            return;
        }
        self.counts.native += 1;
        self.paints(node, true);
        self.paints(node, false);
    }

    /// The node's box with its corners, in its own coordinates
    fn box_shape(&self, node: &Node) -> Path {
        if node.smoothing > 0.0 {
            if let [first, ..] = node.fill_geometry.as_slice() {
                return first.clone();
            }
        }
        let (w, h) = node.size;
        let rect = Rect::from_wh(w as f32, h as f32);
        match node.radii {
            Some([tl, tr, br, bl]) if tl + tr + br + bl > 0.0 => {
                let radii = [tl, tr, br, bl].map(|r| Vector::new(r as f32, r as f32));
                Path::rrect(RRect::new_rect_radii(rect, &radii), None)
            }
            _ => Path::rect(rect, None),
        }
    }

    /// The node's fill outline, or its box, in its own coordinates
    fn outline(&self, node: &Node) -> Path {
        match node.fill_geometry.as_slice() {
            [] => self.box_shape(node),
            [one] => one.clone(),
            [first, rest @ ..] => {
                let mut outline = first.clone();
                for path in rest {
                    outline = outline.op(path, skia_safe::PathOp::Union).unwrap_or(outline);
                }
                outline
            }
        }
    }

    /// Children in order. A mask masks the siblings above it, up to the
    /// next mask: by its alpha (ALPHA), its outline (VECTOR) or its
    /// brightness (LUMINANCE)
    /// A section's children from mask `slot` on, which it masks
    pub fn masked(&mut self, slot: usize) {
        let scene = self.scene;
        let Some(parent) = scene.node(slot).and_then(|mask| mask.parent).and_then(|parent| scene.node(parent)) else {
            return;
        };
        let Some(start) = parent.children.iter().position(|&child| child == slot) else {
            return;
        };
        self.children_from(parent, start);
    }

    fn children(&mut self, node: &Node) {
        self.children_from(node, 0);
    }

    fn children_from(&mut self, node: &Node, start: usize) {
        let scene = self.scene;
        let children = &node.children;
        let mut i = start;
        while i < children.len() {
            let child = children[i];
            let Some(mask) = scene.node(child).filter(|child| !child.hidden && child.mask.is_some()) else {
                self.node(child);
                i += 1;
                continue;
            };
            let end = (i + 1..children.len())
                .find(|&j| scene.node(children[j]).is_some_and(|other| !other.hidden && other.mask.is_some()))
                .unwrap_or(children.len());
            // Bounded to the mask, as nothing outside it shows. A blur inside
            // still reads past it, as its own layer is sized from what it
            // reaches. Unbounded layers would take the whole supersampled tile
            let bounds = mask.bounds;
            let mut rec = SaveLayerRec::default();
            if let Some(bounds) = &bounds {
                rec = rec.bounds(bounds);
            }
            // Figma blends masked layers with what's behind the mask group,
            // so their layer starts as a copy of it. Masking it then mixes
            // the blended result with what was there, by the mask
            if children[i + 1..end].iter().any(|&sibling| self.blends_through(sibling)) {
                rec = rec.flags(SaveLayerFlags::INIT_WITH_PREVIOUS);
            }
            self.canvas.save_layer(&rec);
            for &sibling in &children[i + 1..end] {
                self.node(sibling);
            }
            let mut paint = Paint::default();
            paint.set_blend_mode(BlendMode::DstIn);
            if mask.mask.as_deref() == Some("LUMINANCE") {
                paint.set_color_filter(self.kit.luminance.clone().unwrap_or_else(luma_color_filter::new));
            }
            let mut rec = SaveLayerRec::default().paint(&paint);
            if let Some(bounds) = &bounds {
                rec = rec.bounds(bounds);
            }
            self.canvas.save_layer(&rec);
            // A background blur fills the mask's shape with what's behind it,
            // which is opaque, so the mask is its whole shape
            let opaque = mask.effects.iter().any(|effect| effect.kind == "BACKGROUND_BLUR");
            if mask.mask.as_deref() == Some("VECTOR") || opaque {
                self.canvas.save();
                self.canvas.concat(&mask.transform);
                let mut black = Paint::new(Color4f::new(0.0, 0.0, 0.0, 1.0), None);
                black.set_anti_alias(self.aa);
                self.canvas.draw_path(&self.outline(mask), &black);
                self.canvas.restore();
            } else {
                self.draw(child, true);
            }
            self.canvas.restore();
            self.canvas.restore();
            i = end;
        }
    }

    fn blends_through(&self, slot: usize) -> bool {
        blends_through(self.scene, slot)
    }

    /// Fills (`fills`) or strokes. Strokes aligned inside are clipped to
    /// the fill's outline, and those outside have it cut out
    fn paints(&mut self, node: &Node, fills: bool) {
        if self.stopped {
            return;
        }
        let any = if fills { node.visible_fills().next().is_some() } else { node.visible_strokes().next().is_some() };
        if !any {
            return;
        }
        let geometry = if fills { &node.fill_geometry } else { &node.stroke_geometry };
        // Sections have no outline of their own
        let section;
        let geometry = if fills && geometry.is_empty() && node.kind == Kind::Section {
            section = vec![self.box_shape(node)];
            &section
        } else {
            geometry
        };
        let align = if fills || node.fill_geometry.is_empty() { StrokeAlign::Center } else { node.stroke_align };
        self.canvas.save();
        self.canvas.concat(&node.transform);
        match align {
            StrokeAlign::Inside => {
                self.canvas.clip_path(&self.outline(node), ClipOp::Intersect, self.aa);
            }
            StrokeAlign::Outside => {
                self.canvas.clip_path(&self.outline(node), ClipOp::Difference, self.aa);
            }
            StrokeAlign::Center => {}
        }
        // Figma writes all of a node's outlines to its stencil, then covers
        // them once, so where they overlap a see-through paint isn't doubled
        let joined;
        let geometry = match geometry.as_slice() {
            [first, rest @ ..] if !rest.is_empty() && !matches!(node.fills, Fills::Mixed) && rest.iter().all(|path| path.fill_type() == first.fill_type()) => {
                let mut builder = skia_safe::PathBuilder::new();
                builder.set_fill_type(first.fill_type());
                for path in geometry {
                    builder.add_path(path, None);
                }
                joined = vec![builder.detach()];
                &joined
            }
            _ => geometry,
        };
        if !fills {
            for paint in node.visible_strokes() {
                for path in geometry {
                    self.paint(node, paint, path);
                }
            }
        } else if matches!(node.fills, Fills::Mixed) {
            // Text in several styles, or a vector's regions: each outline in
            // its own
            for (i, path) in geometry.iter().enumerate() {
                for paint in node.fills_of(i) {
                    self.paint(node, paint, path);
                }
            }
        } else {
            for paint in node.visible_fills() {
                for path in geometry {
                    self.paint(node, paint, path);
                }
            }
        }
        self.canvas.restore();
    }

    /// One paint over `path`, in the node's own coordinates
    fn paint(&mut self, node: &Node, paint: &FigmaPaint, path: &Path) {
        let (w, h) = (node.size.0.max(1e-3) as f32, node.size.1.max(1e-3) as f32);
        let mut out = Paint::default();
        out.set_anti_alias(self.aa);
        self.set_blend(&mut out, paint.blend_mode.as_deref());
        match paint.kind.as_str() {
            "SOLID" => {
                let Some(rgba) = paint.color else { return };
                out.set_color4f(color(rgba, paint.opacity), None);
            }
            "GRADIENT_LINEAR" | "GRADIENT_RADIAL" | "GRADIENT_ANGULAR" | "GRADIENT_DIAMOND" => {
                let Some(shader) = gradient(self.kit, paint, w, h) else { return };
                out.set_shader(shader);
            }
            "IMAGE" => {
                let Some(shader) = self.image(node, paint, w, h) else { return };
                out.set_shader(shader);
                out.set_alpha_f(paint.opacity);
            }
            "PATTERN" => {
                let Some(shader) = self.pattern(node, paint, w, h) else { return };
                out.set_shader(shader);
                out.set_alpha_f(paint.opacity);
            }
            _ => return,
        }
        self.canvas.draw_path(path, &out);
    }

    fn image(&mut self, node: &Node, paint: &FigmaPaint, w: f32, h: f32) -> Option<Shader> {
        let hash = paint.image_hash.as_ref()?;
        // Adjusted images are kept apart, adjusted
        let key = match &paint.filters {
            Some(filters) if paint.has_filters() => format!("{hash}#{}", Adjust::from_filters(filters).key()),
            _ => hash.clone(),
        };
        let node_scale = node.transform.scale_x().hypot(node.transform.skew_y());
        let Some(img) = self.images.get(&key) else {
            if !self.missing_images.iter().any(|(missing, _)| *missing == key) {
                self.missing_images.push((key, (w.max(h) * node_scale * self.px * 1.42) as f64));
            }
            return None;
        };
        let (iw, ih) = (img.original.0 as f32, img.original.1 as f32);
        // Original image pixels to the node's own coordinates
        let mode = paint.scale_mode.as_deref().unwrap_or("FILL");
        let (to_node, tile) = match mode {
            "TILE" => (Matrix::scale((paint.scaling_factor.unwrap_or(1.0) as f32, paint.scaling_factor.unwrap_or(1.0) as f32)), TileMode::Repeat),
            "CROP" => {
                let [[a, c, e], [b, d, f]] = paint.image_transform?;
                let t = Matrix::new_all(a as f32, c as f32, e as f32, b as f32, d as f32, f as f32, 0.0, 0.0, 1.0);
                let mut m = Matrix::scale((w, h));
                m.pre_concat(&t.invert()?);
                m.pre_scale((1.0 / iw, 1.0 / ih), None);
                (m, TileMode::Decal)
            }
            _ => {
                // FILL covers the box, FIT fits in it, both centered, turned
                // by `rotation` in quarter turns
                let turns = ((paint.rotation / 90.0).round() as i64).rem_euclid(4);
                let (rw, rh) = if turns % 2 == 1 { (ih, iw) } else { (iw, ih) };
                let s = if mode == "FIT" { (w / rw).min(h / rh) } else { (w / rw).max(h / rh) };
                let mut m = Matrix::translate((w / 2.0, h / 2.0));
                m.pre_rotate(turns as f32 * 90.0, None);
                m.pre_scale((s, s), None);
                m.pre_translate((-iw / 2.0, -ih / 2.0));
                (m, TileMode::Decal)
            }
        };
        // How wide the whole image shows on screen, against how wide it was
        // decoded
        let shown = to_node.scale_x().hypot(to_node.skew_y()) * node_scale * iw * self.px;
        let kept = img.image.width() as f32;
        // Tiles of this zoom level show up to 1.42 times bigger than it, and
        // an image is never shown bigger than kept while there's more of it
        let shown = shown * 1.42;
        self.used_images.push((key.clone(), shown as f64));
        if kept < shown && img.original.0 as f32 > kept && kept < 4096.0 {
            self.sharper_images.push((key, shown as f64));
        }
        let mut local = to_node;
        local.pre_scale((iw / kept, ih / img.image.height() as f32), None);
        // Figma's canvas shows each pixel as the average of the image's texels
        // under it. Exact tiles take 4 by 4 samples a pixel, so nearest ones
        // average the same way, unless there are more texels under a pixel
        // than samples, where mipmaps keep it from shimmering. Quick tiles
        // take one sample and smooth it
        let texels = kept / (shown / 1.42).max(1e-3);
        let sampling = if self.aa {
            // One sample a pixel: the 16 nearest ones exact tiles take are
            // averaged here instead, over where the pixel falls on the image
            if texels <= 4.0 {
                if let Some(boxed) = &self.kit.boxed {
                    let mut to_device = node.transform;
                    to_device.post_scale((self.px, self.px), None);
                    to_device.pre_concat(&local);
                    if let Some(back) = to_device.invert() {
                        let nearest = SamplingOptions::new(FilterMode::Nearest, MipmapMode::None);
                        let child = img.image.to_shader((tile, tile), nearest, None)?;
                        // Column major
                        let footprint = [back.scale_x(), back.skew_y(), back.skew_x(), back.scale_y()];
                        let uniforms: Vec<u8> = footprint.iter().flat_map(|v| v.to_le_bytes()).collect();
                        return boxed.make_shader(Data::new_copy(&uniforms), &[child.into()], Some(&local));
                    }
                }
            }
            SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear)
        } else if texels > 4.0 {
            SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear)
        } else {
            SamplingOptions::new(FilterMode::Nearest, MipmapMode::None)
        };
        img.image.to_shader((tile, tile), sampling, &local)
    }

    fn note_source(&mut self, id: &str, there: bool) {
        if !self.sources.iter().any(|(known, _)| known == id) {
            self.sources.push((id.to_string(), there));
        }
    }

    /// A pattern fill: its source node recorded into a tile, which repeats
    fn pattern(&mut self, node: &Node, paint: &FigmaPaint, w: f32, h: f32) -> Option<Shader> {
        if self.depth >= PATTERN_DEPTH {
            return None;
        }
        let scene = self.scene;
        let source_id = paint.source_node_id.as_ref()?;
        let found = scene.by_id.get(source_id).copied();
        // The page can come in after what uses it
        self.note_source(source_id, found.is_some());
        let source_slot = found?;
        let source = scene.node(source_slot)?;
        // The source as it shows, turned if it is, in the box around it
        let frame = source.transform.map_rect(Rect::from_wh(source.size.0 as f32, source.size.1 as f32)).0;
        let (sw, sh) = (frame.width().max(1e-3), frame.height().max(1e-3));
        let factor = paint.scaling_factor.unwrap_or(1.0).max(1e-3) as f32;
        let spacing = paint.spacing.unwrap_or_default();
        // A cell is the tile and the space after it, in the source's units
        let (cw, ch) = (sw * (1.0 + spacing.x as f32).max(0.01), sh * (1.0 + spacing.y as f32).max(0.01));
        let tile_type = paint.tile_type.as_deref().unwrap_or("RECTANGULAR");
        let (tile, placements): (Rect, Vec<(f32, f32)>) = match tile_type {
            // Every other row half a cell along
            "HORIZONTAL_HEXAGONAL" => (Rect::from_wh(cw, ch * 2.0), vec![(0.0, 0.0), (cw / 2.0, ch), (-cw / 2.0, ch)]),
            "VERTICAL_HEXAGONAL" => (Rect::from_wh(cw * 2.0, ch), vec![(0.0, 0.0), (cw, ch / 2.0), (cw, -ch / 2.0)]),
            _ => (Rect::from_wh(cw, ch), vec![(0.0, 0.0)]),
        };
        // Recorded for the zoom it shows at, for its level of detail
        let node_scale = node.transform.scale_x().hypot(node.transform.skew_y());
        let px = self.px * node_scale * factor;
        let level = px.log2().round().clamp(-12.0, 8.0) as i32;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (tile_type, spacing.x.to_bits(), spacing.y.to_bits()).hash(&mut hasher);
        let key = (source_slot, hasher.finish(), level);
        let cached = self.kit.patterns.borrow().get(&key).map(|(picture, missing, sources)| (picture.clone(), missing.clone(), sources.clone()));
        let picture = match cached {
            Some((picture, missing, sources)) => {
                for (id, there) in sources {
                    self.note_source(&id, there);
                }
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
                let mut inner = Encoder::new(scene, self.images, self.kit, canvas, 2f32.powi(level));
                inner.depth = self.depth + 1;
                for (x, y) in placements {
                    canvas.save();
                    canvas.translate((x - frame.left, y - frame.top));
                    inner.node(source_slot);
                    canvas.restore();
                }
                let missing: Vec<String> = inner.missing_images.iter().map(|(hash, _)| hash.clone()).collect();
                let sources = std::mem::take(&mut inner.sources);
                for (id, there) in &sources {
                    self.note_source(id, *there);
                }
                self.missing_images.extend(inner.missing_images);
                self.sharper_images.extend(inner.sharper_images);
                self.used_images.extend(inner.used_images);
                let picture = recorder.finish_recording_as_picture(Some(&tile))?;
                self.kit.patterns.borrow_mut().insert(key, (picture.clone(), missing, sources));
                picture
            }
        };
        // Where the grid starts in the node's box
        let along = |alignment: Option<&str>, size: f32, tile: f32| match alignment {
            Some("CENTER") => (size - tile) / 2.0,
            Some("END") => size - tile,
            _ => 0.0,
        };
        let mut local = Matrix::translate((
            along(paint.horizontal_alignment.as_deref(), w, sw * factor),
            along(paint.vertical_alignment.as_deref(), h, sh * factor),
        ));
        local.pre_scale((factor, factor), None);
        Some(picture.to_shader((TileMode::Repeat, TileMode::Repeat), FilterMode::Linear, &local, &tile))
    }

    /// A drop shadow under a plain box, as a blurred rounded rectangle
    fn box_shadow(&mut self, node: &Node, effect: &Effect) {
        let Some(rgba) = effect.color else {
            return;
        };
        let (w, h) = node.size;
        let spread = effect.spread as f32;
        let rect = Rect::from_wh(w as f32, h as f32).with_outset((spread, spread));
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        // Rounded corners grow and shrink with the spread, sharp ones stay
        let radii = node.radii.unwrap_or_default().map(|r| {
            let r = if r > 0.0 { (r as f32 + spread).max(0.0) } else { 0.0 };
            Vector::new(r, r)
        });
        let mut paint = Paint::new(color(rgba, 1.0), None);
        paint.set_anti_alias(self.aa);
        self.set_blend(&mut paint, effect.blend_mode.as_deref());
        if effect.radius > 0.0 {
            paint.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, tent_sigma(effect.radius), true));
        }
        self.canvas.save();
        self.canvas.translate((effect.offset.x as f32, effect.offset.y as f32));
        self.canvas.concat(&node.transform);
        self.canvas.draw_rrect(RRect::new_rect_radii(rect, &radii), &paint);
        self.canvas.restore();
    }

    /// A background blur: what's behind the node blurred, inside its
    /// outline, under its fills
    fn backdrop(&mut self, node: &Node, effect: &Effect) {
        let sigma = tent_sigma(effect.radius);
        // Figma blurs only what's behind the layer's box, each pixel the
        // weighted mean of the part of the kernel inside it: the blur of
        // what's inside over the blur of the box itself
        let area = node.transform.map_rect(Rect::from_wh(node.size.0 as f32, node.size.1 as f32)).0;
        let behind = image_filters::crop(area, TileMode::Decal, None);
        let device = self.device_scale();
        let inside = wide_blur(sigma, device, behind, None);
        let box_ = image_filters::shader(skia_safe::shaders::color(skia_safe::Color::WHITE), CropRect::from(area));
        let weight = wide_blur(sigma, device, box_, None);
        let Some(blur) = self.kit.divide.clone().and_then(|divide| image_filters::blend(divide, weight, inside, None)) else {
            return;
        };
        self.canvas.save();
        self.canvas.clip_path(&self.outline(node).with_transform(&node.transform), ClipOp::Intersect, self.aa);
        self.canvas.save_layer(&SaveLayerRec::default().backdrop(&blur));
        self.canvas.restore();
        self.canvas.restore();
    }

    /// A progressive layer blur, blurring more from its start to its end
    fn progressive(&mut self, node: &Node, effect: &Effect) {
        let (w, h) = (node.size.0 as f32, node.size.1 as f32);
        let start = effect.start_offset.unwrap_or_default();
        let end = effect.end_offset.unwrap_or(super::scene::Vec2 { x: 0.5, y: 1.0 });
        let p0 = node.transform.map_point((start.x as f32 * w, start.y as f32 * h));
        let p1 = node.transform.map_point((end.x as f32 * w, end.y as f32 * h));
        let Some(bounds) = node.layer_bounds() else {
            return self.content(node);
        };
        // Figma's radius changes along the way. Here blurs of radii evenly
        // apart are each shown about where the radius is theirs, fading into
        // the next, so no radius is more than a few pixels off
        let device = self.px * node.transform.scale_x().hypot(node.transform.skew_y());
        let span = (effect.radius - effect.start_radius).abs() as f32 * device;
        let levels = ((span / 6.0).ceil() as usize + 1).clamp(2, 8);
        let device_px = self.device_scale();
        let mut plus = Paint::default();
        plus.set_blend_mode(BlendMode::Plus);
        let mut dest_in = Paint::default();
        dest_in.set_blend_mode(BlendMode::DstIn);
        self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds));
        for level in 0..levels {
            let at = level as f32 / (levels - 1) as f32;
            let radius = effect.start_radius + (effect.radius - effect.start_radius) * at as f64;
            let step = 1.0 / (levels - 1) as f32;
            // 1 where the radius is this level's, 0 a level away
            let (colors, positions): (Vec<Color4f>, Vec<f32>) = [(at - step, 0.0), (at, 1.0), (at + step, 0.0)]
                .into_iter()
                .filter(|(position, _)| (-1e-6..=1.0 + 1e-6).contains(position))
                .map(|(position, alpha)| (Color4f::new(0.0, 0.0, 0.0, alpha), position.clamp(0.0, 1.0)))
                .unzip();
            let ramp = Gradient::new(Colors::new(&colors, Some(&positions), TileMode::Clamp, None), Interpolation::default());
            dest_in.set_shader(gradients::linear_gradient((p0, p1), &ramp, None));
            self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&plus));
            let mut blurred = Paint::default();
            let sigma = tent_sigma(radius);
            if sigma > 0.0 {
                blurred.set_image_filter(wide_blur(sigma, device_px, None, None));
            }
            self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&blurred));
            self.content(node);
            self.canvas.restore();
            self.canvas.draw_rect(bounds, &dest_in);
            self.canvas.restore();
        }
        self.canvas.restore();
    }

    /// Noise: grain in the effect's color over the node's outline
    fn noise(&mut self, node: &Node, effect: &Effect) {
        let (Some(rgba), Some(noise), Some(over)) = (effect.color, &self.kit.noise, &self.kit.noise_over) else {
            return;
        };
        let size = effect.noise_size_vector.unwrap_or(Vec2 { x: effect.noise_size.unwrap_or(1.0), y: effect.noise_size.unwrap_or(1.0) });
        let kind = match effect.noise_type.as_deref() {
            Some("MULTITONE") => 0.0,
            Some("DUOTONE") => 2.0,
            _ => 1.0,
        };
        let secondary = effect.secondary_color.unwrap_or(Rgba { r: 1.0, g: 1.0, b: 1.0, a: 0.25 });
        let (w, h) = node.size;
        let values = [
            (1.0 / size.x.max(1e-3)) as f32,
            (1.0 / size.y.max(1e-3)) as f32,
            (w / 2.0) as f32,
            (h / 2.0) as f32,
            kind,
            effect.density.unwrap_or(0.5).clamp(0.0, 1.0) as f32,
            rgba.r,
            rgba.g,
            rgba.b,
            rgba.a,
            secondary.r,
            secondary.g,
            secondary.b,
            secondary.a,
        ];
        let mut uniforms = Vec::with_capacity((values.len() + 36) * 4);
        for value in values.into_iter().chain(self.scene.noise_octaves.iter().copied()) {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        let Some(shader) = noise.make_shader(Data::new_copy(&uniforms), &[], None) else {
            log::warn!("native renderer: noise uniforms are {} bytes, not {}", uniforms.len(), noise.uniform_size());
            return;
        };
        let Some(bounds) = node.bounds else {
            return;
        };
        let Some(inverse) = node.transform.invert() else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_shader(shader);
        paint.set_blender(over.clone());
        self.canvas.save();
        self.canvas.concat(&node.transform);
        self.canvas.draw_rect(inverse.map_rect(bounds.with_outset((1.0, 1.0))).0, &paint);
        self.canvas.restore();
    }
}

/// A gradient fill as a shader in the node's own coordinates. Figma's
/// `gradientTransform` maps the node's box, as 0 to 1 both ways, to the
/// gradient's space, where a linear gradient runs from (0, 0.5) to (1, 0.5)
/// and the others are centered on (0.5, 0.5). Colors are mixed as they
/// are, not premultiplied, so one fading out darkens toward a transparent
/// black, as in Figma
fn gradient(kit: &Kit, paint: &FigmaPaint, w: f32, h: f32) -> Option<Shader> {
    let [[a, c, e], [b, d, f]] = paint.gradient_transform?;
    let t = Matrix::new_all(a as f32, c as f32, e as f32, b as f32, d as f32, f as f32, 0.0, 0.0, 1.0);
    let mut local = Matrix::scale((w, h));
    local.pre_concat(&t.invert()?);
    let stops = paint.gradient_stops.as_ref()?;
    let colors: Vec<Color4f> = stops.iter().map(|stop| color(stop.color, paint.opacity)).collect();
    let positions: Vec<f32> = stops.iter().map(|stop| stop.position).collect();
    let spec = Gradient::new(Colors::new(&colors, Some(&positions), TileMode::Clamp, None), Interpolation::default());
    let center = Point::new(0.5, 0.5);
    match paint.kind.as_str() {
        "GRADIENT_LINEAR" => gradients::linear_gradient((Point::new(0.0, 0.5), Point::new(1.0, 0.5)), &spec, &local),
        "GRADIENT_RADIAL" => gradients::radial_gradient((center, 0.5), &spec, &local),
        "GRADIENT_DIAMOND" => {
            let ramp = gradients::linear_gradient((Point::new(0.0, 0.5), Point::new(1.0, 0.5)), &spec, None)?;
            kit.diamond.as_ref()?.make_shader(Data::new_copy(&[]), &[ramp.into()], Some(&local))
        }
        _ => gradients::sweep_gradient(center, (0.0, 360.0), &spec, &local),
    }
}
