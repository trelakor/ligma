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
    color_filters, image_filters, luma_color_filter, BlendMode, Blender, BlurStyle, Canvas, ClipOp, Color4f, Data,
    FilterMode, Image, ImageFilter, MaskFilter, Matrix, MipmapMode, Paint, Path, PictureRecorder, Point, RRect, Rect,
    RuntimeEffect, SamplingOptions, Shader, TileMode, Vector,
};

use super::adjust::Adjust;
use super::scene::{Effect, Fills, Kind, Node, Paint as FigmaPaint, Rgba, Scene, StrokeAlign};

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

fn is_normal(mode: &Option<String>) -> bool {
    mode.as_deref().is_none_or(|mode| mode == "NORMAL" || mode == "PASS_THROUGH")
}

fn color(rgba: Rgba, alpha: f32) -> Color4f {
    Color4f::new(rgba.r, rgba.g, rgba.b, (rgba.a * alpha).clamp(0.0, 1.0))
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

/// Noise: square grains `grain` units wide, `density` of them in `color`,
/// the same every time. Figma doesn't say how it makes its grain, so this
/// only looks alike
const NOISE: &str = r#"
uniform float grain;
uniform float density;
uniform half4 color;
half4 main(float2 p) {
    float2 cell = floor(p / grain);
    float h = fract(sin(dot(cell, float2(12.9898, 78.233))) * 43758.5453);
    return h < density ? half4(color.rgb * color.a, color.a) : half4(0);
}
"#;

/// An image fill, decoded at about the size it shows
#[derive(Clone)]
pub struct Img {
    pub image: Image,
    /// The original's size in pixels, which image fills are laid out by
    pub original: (u32, u32),
}

impl Img {
    pub fn bytes(&self) -> usize {
        (self.image.width() * self.image.height() * 4) as usize
    }
}

type PatternKey = (usize, u64, i32);

/// What drawing shares across nodes and frames: compiled SkSL and pattern
/// tiles
pub struct Kit {
    linear_burn: Option<Blender>,
    soft_light: Option<Blender>,
    /// Divides the source by the destination's alpha
    divide: Option<Blender>,
    noise: Option<RuntimeEffect>,
    /// Recorded pattern tiles, and the images they were waiting for
    patterns: RefCell<HashMap<PatternKey, (skia_safe::Picture, Vec<String>)>>,
}

impl Kit {
    pub fn new() -> Self {
        let linear_burn = RuntimeEffect::make_for_blender(LINEAR_BURN, None)
            .map_err(|error| log::warn!("native renderer: LINEAR_BURN: {error}"))
            .ok()
            .and_then(|effect| effect.make_blender(Data::new_copy(&[]), None));
        let soft_light = RuntimeEffect::make_for_blender(SOFT_LIGHT, None)
            .map_err(|error| log::warn!("native renderer: SOFT_LIGHT: {error}"))
            .ok()
            .and_then(|effect| effect.make_blender(Data::new_copy(&[]), None));
        let divide = RuntimeEffect::make_for_blender(DIVIDE, None)
            .map_err(|error| log::warn!("native renderer: divide: {error}"))
            .ok()
            .and_then(|effect| effect.make_blender(Data::new_copy(&[]), None));
        let noise = RuntimeEffect::make_for_shader(NOISE, None).map_err(|error| log::warn!("native renderer: noise: {error}")).ok();
        Self { linear_burn, soft_light, divide, noise, patterns: RefCell::new(HashMap::new()) }
    }

    /// Forgets pattern tiles drawn without image `key`, or all of them
    pub fn forget_patterns(&self, key: Option<&str>) {
        let mut patterns = self.patterns.borrow_mut();
        match key {
            Some(key) => patterns.retain(|_, (_, missing)| !missing.iter().any(|missing| missing == key)),
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
    /// Why the first approximate node is
    pub approximate_reason: Option<String>,
    /// Marks approximate nodes
    pub tint: bool,
    /// Edges are anti-aliased. Off when tiles are supersampled, as Figma
    /// covers each pixel with 16 samples, each in or out, which leaves no
    /// seam where shapes meet
    pub aa: bool,
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
            approximate_reason: None,
            tint: false,
            aa: true,
            depth: 0,
        }
    }

    /// Draws node `slot` and everything under it
    pub fn node(&mut self, slot: usize) {
        self.draw(slot, false);
    }

    /// `as_mask` draws a mask itself, for the siblings it masks
    fn draw(&mut self, slot: usize, as_mask: bool) {
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
        // rectangles, which is much cheaper than filtering a layer
        let box_shadows = node.is_solid_box() && node.effects.iter().all(|effect| effect.kind == "DROP_SHADOW");
        let progressive = node.layer_blur().filter(|effect| effect.progressive());
        let filter = if box_shadows { None } else { self.filter(node, progressive.is_none()) };
        let layer = node.opacity < 0.999 || !is_normal(&node.blend) || filter.is_some();
        if layer {
            let mut paint = Paint::default();
            paint.set_alpha_f(node.opacity);
            self.set_blend(&mut paint, node.blend.as_deref());
            paint.set_image_filter(filter);
            let bounds = node.bounds;
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

    fn set_blend(&self, paint: &mut Paint, mode: Option<&str>) {
        match mode {
            None | Some("NORMAL") | Some("PASS_THROUGH") => {}
            Some("LINEAR_BURN") => {
                if let Some(blender) = &self.kit.linear_burn {
                    paint.set_blender(blender.clone());
                }
            }
            Some("SOFT_LIGHT") => {
                if let Some(blender) = &self.kit.soft_light {
                    paint.set_blender(blender.clone());
                }
            }
            Some(mode) => {
                if let Some(mode) = blend(mode) {
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
        let solid = || image_filters::color_filter(silhouette(), None, None);
        for effect in &node.effects {
            let sigma = tent_sigma(effect.radius);
            let offset = Vector::new(effect.offset.x as f32, effect.offset.y as f32);
            match effect.kind.as_str() {
                "DROP_SHADOW" => {
                    let Some(rgba) = effect.color else { continue };
                    let spread = effect.spread as f32;
                    let input = if spread > 0.0 {
                        image_filters::dilate((spread, spread), solid(), None)
                    } else if spread < 0.0 {
                        image_filters::erode((-spread, -spread), solid(), None)
                    } else {
                        solid()
                    };
                    let mut shadow = image_filters::drop_shadow_only(offset, (sigma, sigma), color(rgba, 1.0), None, input, None);
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
                        shadow = image_filters::blur((sigma, sigma), TileMode::Decal, shadow, crop);
                    }
                    over.push(image_filters::blend(BlendMode::SrcIn, None, shadow, None));
                }
                "LAYER_BLUR" if blur && effect.radius > 0.0 => blur_sigma = Some(sigma),
                _ => {}
            }
        }
        let mut filter = if behind.is_empty() && over.is_empty() {
            None
        } else {
            let layers = behind.into_iter().chain([None]).chain(over);
            image_filters::merge(layers, None)
        };
        if let Some(sigma) = blur_sigma {
            filter = image_filters::blur((sigma, sigma), TileMode::Decal, filter, None);
        }
        filter
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
    fn children(&mut self, node: &Node) {
        let scene = self.scene;
        let children = &node.children;
        let mut i = 0;
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
            let bounds = node.bounds;
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
                paint.set_color_filter(luma_color_filter::new());
            }
            self.canvas.save_layer(&SaveLayerRec::default().paint(&paint));
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

    /// Whether node `slot` draws with a blend mode that reaches what's
    /// behind it: its own, its fills', or a child's through groups that
    /// don't keep their children to themselves
    fn blends_through(&self, slot: usize) -> bool {
        let Some(node) = self.scene.node(slot) else {
            return false;
        };
        if node.hidden {
            return false;
        }
        if !is_normal(&node.blend) || node.visible_fills().any(|paint| !is_normal(&paint.blend_mode)) {
            return true;
        }
        let isolated = node.opacity < 0.999 || !node.effects.is_empty() || node.blend.as_deref() == Some("NORMAL");
        !isolated && node.children.iter().any(|&child| self.blends_through(child))
    }

    /// Fills (`fills`) or strokes. Strokes aligned inside are clipped to
    /// the fill's outline, and those outside have it cut out
    fn paints(&mut self, node: &Node, fills: bool) {
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
        if !fills {
            for paint in node.visible_strokes() {
                for path in geometry {
                    self.paint(node, paint, path);
                }
            }
        } else if matches!(node.fills, Fills::Mixed) {
            // Text in several styles: each outline in its own
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
                let Some(shader) = gradient(paint, w, h) else { return };
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
                self.missing_images.push((key, (w.max(h) * node_scale * self.px) as f64));
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
        if kept * 1.4 < shown && img.original.0 as f32 > kept && kept < 4096.0 {
            self.sharper_images.push((key, shown as f64));
        }
        let mut local = to_node;
        local.pre_scale((iw / kept, ih / img.image.height() as f32), None);
        // Figma resamples images with a sharp cubic (Keys, a = -1), so does
        // this, unless the image is shrunk well past the samples drawn,
        // where mipmaps keep it from shimmering
        let samples = if self.aa { 1.0 } else { 4.0 };
        let sampling = if kept > shown * samples * 2.0 {
            SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear)
        } else {
            SamplingOptions::from(skia_safe::CubicResampler { b: 0.0, c: 1.0 })
        };
        img.image.to_shader((tile, tile), sampling, &local)
    }

    /// A pattern fill: its source node recorded into a tile, which repeats
    fn pattern(&mut self, node: &Node, paint: &FigmaPaint, w: f32, h: f32) -> Option<Shader> {
        if self.depth >= PATTERN_DEPTH {
            return None;
        }
        let scene = self.scene;
        let &source_slot = scene.by_id.get(paint.source_node_id.as_ref()?)?;
        let source = scene.node(source_slot)?;
        let (sw, sh) = (source.size.0.max(1e-3) as f32, source.size.1.max(1e-3) as f32);
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
        let cached = self.kit.patterns.borrow().get(&key).map(|(picture, missing)| (picture.clone(), missing.clone()));
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
                let inverse = source.transform.invert()?;
                let mut inner = Encoder::new(scene, self.images, self.kit, canvas, 2f32.powi(level));
                inner.depth = self.depth + 1;
                for (x, y) in placements {
                    canvas.save();
                    canvas.translate((x, y));
                    canvas.concat(&inverse);
                    inner.node(source_slot);
                    canvas.restore();
                }
                let missing: Vec<String> = inner.missing_images.iter().map(|(hash, _)| hash.clone()).collect();
                self.missing_images.extend(inner.missing_images);
                self.sharper_images.extend(inner.sharper_images);
                let picture = recorder.finish_recording_as_picture(Some(&tile))?;
                self.kit.patterns.borrow_mut().insert(key, (picture.clone(), missing));
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
        let radius = node.radii.map_or(0.0, |r| r.iter().sum::<f64>() as f32 / 4.0) + spread.max(0.0);
        let mut paint = Paint::new(color(rgba, 1.0), None);
        paint.set_anti_alias(self.aa);
        self.set_blend(&mut paint, effect.blend_mode.as_deref());
        if effect.radius > 0.0 {
            paint.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, tent_sigma(effect.radius), true));
        }
        self.canvas.save();
        self.canvas.translate((effect.offset.x as f32, effect.offset.y as f32));
        self.canvas.concat(&node.transform);
        self.canvas.draw_rrect(RRect::new_rect_xy(rect, radius, radius), &paint);
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
        let inside = image_filters::blur((sigma, sigma), TileMode::Decal, behind, None);
        let box_ = image_filters::shader(skia_safe::shaders::color(skia_safe::Color::WHITE), CropRect::from(area));
        let weight = image_filters::blur((sigma, sigma), TileMode::Decal, box_, None);
        let Some(blur) = self.kit.divide.clone().and_then(|divide| image_filters::blend(divide, weight, inside, None)) else {
            return;
        };
        self.canvas.save();
        self.canvas.clip_path(&self.outline(node).with_transform(&node.transform), ClipOp::Intersect, self.aa);
        self.canvas.save_layer(&SaveLayerRec::default().backdrop(&blur));
        self.canvas.restore();
        self.canvas.restore();
    }

    /// A progressive layer blur, blurring more from its start to its end:
    /// the sharp node fading out as the blurred one fades in
    fn progressive(&mut self, node: &Node, effect: &Effect) {
        let (w, h) = (node.size.0 as f32, node.size.1 as f32);
        let start = effect.start_offset.unwrap_or_default();
        let end = effect.end_offset.unwrap_or(super::scene::Vec2 { x: 0.5, y: 1.0 });
        let p0 = node.transform.map_point((start.x as f32 * w, start.y as f32 * h));
        let p1 = node.transform.map_point((end.x as f32 * w, end.y as f32 * h));
        let ramp = |from: f32, to: f32| {
            let colors = [Color4f::new(0.0, 0.0, 0.0, from), Color4f::new(0.0, 0.0, 0.0, to)];
            let ramp = Gradient::new(Colors::new(&colors, None, TileMode::Clamp, None), Interpolation::default());
            gradients::linear_gradient((p0, p1), &ramp, None)
        };
        let Some(bounds) = node.bounds else {
            return self.content(node);
        };
        let sigma = tent_sigma(effect.radius);
        let mut dest_in = Paint::default();
        dest_in.set_blend_mode(BlendMode::DstIn);
        self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds));
        // Sharp, or as blurred as it starts, fading out
        let start_sigma = tent_sigma(effect.start_radius);
        let mut starting = Paint::default();
        if start_sigma > 0.0 {
            starting.set_image_filter(image_filters::blur((start_sigma, start_sigma), TileMode::Decal, None, None));
        }
        self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds));
        self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&starting));
        self.content(node);
        self.canvas.restore();
        dest_in.set_shader(ramp(1.0, 0.0));
        self.canvas.draw_rect(bounds, &dest_in);
        self.canvas.restore();
        // Blurred, fading in, added
        let mut plus = Paint::default();
        plus.set_blend_mode(BlendMode::Plus);
        self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&plus));
        let mut blurred = Paint::default();
        blurred.set_image_filter(image_filters::blur((sigma, sigma), TileMode::Decal, None, None));
        self.canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&blurred));
        self.content(node);
        self.canvas.restore();
        dest_in.set_shader(ramp(0.0, 1.0));
        self.canvas.draw_rect(bounds, &dest_in);
        self.canvas.restore();
        self.canvas.restore();
    }

    /// Noise: grain in the effect's color over the node's outline
    fn noise(&mut self, node: &Node, effect: &Effect) {
        let (Some(rgba), Some(noise)) = (effect.color, &self.kit.noise) else {
            return;
        };
        let grain = (effect.noise_size.unwrap_or(100.0) / 100.0).max(0.01) as f32;
        let density = effect.density.unwrap_or(0.5).clamp(0.0, 1.0) as f32;
        let mut uniforms = Vec::with_capacity(24);
        for value in [grain, density, rgba.r, rgba.g, rgba.b, rgba.a] {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        let Some(shader) = noise.make_shader(Data::new_copy(&uniforms), &[], None) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_anti_alias(self.aa);
        paint.set_shader(shader);
        self.canvas.save();
        self.canvas.concat(&node.transform);
        self.canvas.draw_path(&self.outline(node), &paint);
        self.canvas.restore();
    }
}

/// A gradient fill as a shader in the node's own coordinates. Figma's
/// `gradientTransform` maps the node's box, as 0 to 1 both ways, to the
/// gradient's space, where a linear gradient runs from (0, 0.5) to (1, 0.5)
/// and the others are centered on (0.5, 0.5). Diamond gradients are drawn
/// as radial ones
fn gradient(paint: &FigmaPaint, w: f32, h: f32) -> Option<Shader> {
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
        "GRADIENT_RADIAL" | "GRADIENT_DIAMOND" => gradients::radial_gradient((center, 0.5), &spec, &local),
        _ => gradients::sweep_gradient(center, (0.0, 360.0), &spec, &local),
    }
}
