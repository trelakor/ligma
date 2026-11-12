//! Figma's render tree, the "forest": the scene Figma's main thread sends
//! its render worker every frame, as the `renderTreeForest` bytes of
//! `sendCanvasFrameToRenderWorker`. It's one Kiwi `Resources` message in the
//! schema `render.bkiwi`, which Figma's wasm carries
//!
//! - `Schema` parses a binary Kiwi schema and binds its fields to the types
//!   below by name, so a schema taken from a newer Figma build still decodes
//!   when fields are added, renumbered or reordered. Fields this module
//!   doesn't know are kept as raw `Value`s in each type's `extra`
//! - `Forest` keeps the scene resident across frames. Figma sends a full
//!   scene, then only new nodes, referring to untouched subtrees by id
//! - `schema_from_wasm` takes the schema out of Figma's wasm, as
//!   `rt_schema.js` does in the page
//!
//! Transforms are `[m00, m01, m02, m10, m11, m12]`, row-major 2×3 matrices
//! that map (x, y) to (m00·x + m01·y + m02, m10·x + m11·y + m12), as Figma's
//! plugin API writes them. A drawable's absolute transform is the product of
//! its STATIC_TRANSFORM_BINDING ancestors' transforms, outermost first, then
//! its own: `concat(&concat(&stb1, &stb2), &node)`
//!
//! Optional fields are `None` when the frame doesn't carry them. Figma's
//! defaults for those: paint and node opacity 1, blend mode NORMAL, paint
//! transform identity, NaN floats for unset effect parameters

// Not wired into the renderer yet
#![allow(dead_code)]

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, LazyLock};

type R<T> = Result<T, String>;

// ---------------------------------------------------------------- schema

/// A field's wire type
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Bool,
    Byte,
    Int,
    Uint,
    Float,
    String,
    Int64,
    Uint64,
    /// An enum, struct or message: index into `Schema::defs`
    Def(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    Enum,
    Struct,
    Message,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: Arc<str>,
    pub ty: Type,
    pub array: bool,
    /// Field id for messages, value for enums, unused for structs
    pub value: u32,
}

#[derive(Clone, Debug)]
pub struct Def {
    pub name: Arc<str>,
    pub kind: DefKind,
    pub fields: Vec<Field>,
    index: Index,
}

impl Def {
    /// The message field with this id, or the enum member with this value
    pub fn field(&self, value: u32) -> Option<&Field> {
        self.index.get(value).map(|i| &self.fields[i])
    }

    pub fn field_named(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| &*f.name == name)
    }
}

const NO: u16 = u16::MAX;

#[derive(Clone, Debug)]
enum Index {
    Dense(Vec<u16>),
    Sparse(HashMap<u32, u16>),
}

impl Index {
    fn new(fields: &[Field]) -> Self {
        let max = fields.iter().map(|f| f.value).max().unwrap_or(0);
        if max < 4096 {
            let mut v = vec![NO; max as usize + 1];
            for (i, f) in fields.iter().enumerate() {
                v[f.value as usize] = i as u16;
            }
            Index::Dense(v)
        } else {
            Index::Sparse(fields.iter().enumerate().map(|(i, f)| (f.value, i as u16)).collect())
        }
    }

    #[inline]
    fn get(&self, value: u32) -> Option<usize> {
        match self {
            Index::Dense(v) => match v.get(value as usize) {
                Some(&i) if i != NO => Some(i as usize),
                _ => None,
            },
            Index::Sparse(m) => m.get(&value).map(|&i| i as usize),
        }
    }
}

/// A binary Kiwi schema, with its fields bound to this module's types
pub struct Schema {
    pub defs: Vec<Def>,
    names: HashMap<Arc<str>, u32>,
    bind: Bind,
}

impl Schema {
    pub fn parse(bytes: &[u8]) -> Result<Schema, String> {
        let defs = parse_defs(bytes)?;
        let names = defs.iter().enumerate().map(|(i, d)| (d.name.clone(), i as u32)).collect();
        let bind = Bind::new(&defs, &names)?;
        Ok(Schema { defs, names, bind })
    }

    /// The schema of the Figma build these decoders were written against
    pub fn bundled() -> &'static Schema {
        static BUNDLED: LazyLock<Schema> =
            LazyLock::new(|| Schema::parse(include_bytes!("render.bkiwi")).expect("the bundled render.bkiwi parses"));
        &BUNDLED
    }

    pub fn def(&self, name: &str) -> Option<&Def> {
        self.names.get(name).map(|&i| &self.defs[i as usize])
    }

    /// Fields of the definitions the forest uses that no type here takes,
    /// as `Def.field`. They're decoded into `extra`
    pub fn unbound_fields(&self) -> Vec<String> {
        let mut out = Vec::new();
        for plan in &self.bind.plans {
            let def = &self.defs[plan.def as usize];
            for m in &plan.members {
                if m.slot == NONE {
                    out.push(format!("{}.{}", def.name, def.fields[m.field as usize].name));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

fn parse_defs(bytes: &[u8]) -> R<Vec<Def>> {
    let mut rd = Reader::new(bytes);
    let count = rd.uint()? as usize;
    let mut defs = Vec::with_capacity(count.min(bytes.len()));
    for _ in 0..count {
        let name = schema_string(&mut rd)?;
        let kind = match rd.byte()? {
            0 => DefKind::Enum,
            1 => DefKind::Struct,
            2 => DefKind::Message,
            k => return Err(format!("definition {name} has unknown kind {k}")),
        };
        let field_count = rd.uint()? as usize;
        if field_count >= NO as usize {
            return Err(format!("definition {name} has too many fields"));
        }
        let mut fields = Vec::with_capacity(field_count.min(rd.remaining()));
        for _ in 0..field_count {
            let field_name = schema_string(&mut rd)?;
            let ty = match rd.int()? {
                -1 => Type::Bool,
                -2 => Type::Byte,
                -3 => Type::Int,
                -4 => Type::Uint,
                -5 => Type::Float,
                -6 => Type::String,
                -7 => Type::Int64,
                -8 => Type::Uint64,
                t if t >= 0 => Type::Def(t as u32),
                t => return Err(format!("field {name}.{field_name} has unknown type {t}")),
            };
            let array = rd.byte()? & 1 != 0;
            let value = rd.uint()?;
            fields.push(Field { name: field_name, ty, array, value });
        }
        defs.push(Def { name, kind, index: Index::new(&fields), fields });
    }
    if rd.p != bytes.len() {
        return Err(format!("the schema has {} trailing bytes", bytes.len() - rd.p));
    }
    for def in &defs {
        if def.kind == DefKind::Enum {
            continue;
        }
        for f in &def.fields {
            if let Type::Def(t) = f.ty {
                if t as usize >= defs.len() {
                    return Err(format!("field {}.{} refers to definition {t}, which doesn't exist", def.name, f.name));
                }
            }
            if def.kind == DefKind::Message && f.value == 0 {
                return Err(format!("field {}.{} has id 0", def.name, f.name));
            }
        }
    }
    Ok(defs)
}

fn schema_string(rd: &mut Reader) -> R<Arc<str>> {
    let at = rd.p;
    std::str::from_utf8(rd.string_bytes()?).map(Arc::from).map_err(|_| format!("the schema has invalid UTF-8 at byte {at}"))
}

// ---------------------------------------------------------------- wire reading

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b, p: 0 }
    }

    fn remaining(&self) -> usize {
        self.b.len() - self.p
    }

    #[cold]
    fn eof(&self) -> String {
        format!("the data ends early, at byte {}", self.p)
    }

    #[inline]
    fn byte(&mut self) -> R<u8> {
        match self.b.get(self.p) {
            Some(&x) => {
                self.p += 1;
                Ok(x)
            }
            None => Err(self.eof()),
        }
    }

    #[inline]
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        if n > self.remaining() {
            return Err(self.eof());
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }

    /// Kiwi's varuint: at most 5 bytes, the last one's high bits dropped
    #[inline]
    fn uint(&mut self) -> R<u32> {
        let first = self.byte()?;
        if first < 128 {
            return Ok(first as u32);
        }
        let mut v = (first & 127) as u32;
        let mut shift = 7;
        loop {
            let b = self.byte()?;
            v |= ((b & 127) as u32).wrapping_shl(shift);
            shift += 7;
            if b & 128 == 0 || shift >= 35 {
                return Ok(v);
            }
        }
    }

    #[inline]
    fn int(&mut self) -> R<i32> {
        let v = self.uint()?;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }

    /// Kiwi's uint64: LEB128 for 8 bytes, then a 9th byte, if reached,
    /// carries 8 full bits
    fn uint64(&mut self) -> R<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            if shift == 56 {
                return Ok(v | (b as u64) << 56);
            }
            v |= ((b & 127) as u64) << shift;
            if b & 128 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }

    fn int64(&mut self) -> R<i64> {
        let v = self.uint64()?;
        Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
    }

    /// Kiwi's varfloat: one zero byte for 0, otherwise the f32 bits rotated
    /// so the exponent comes first, in 4 little-endian bytes
    #[inline]
    fn float(&mut self) -> R<f32> {
        match self.b.get(self.p) {
            Some(0) => {
                self.p += 1;
                Ok(0.0)
            }
            Some(_) => {
                let b = self.take(4)?;
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                Ok(f32::from_bits(bits.rotate_left(23)))
            }
            None => Err(self.eof()),
        }
    }

    fn string_bytes(&mut self) -> R<&'a [u8]> {
        let rest = &self.b[self.p..];
        let end = rest.iter().position(|&c| c == 0).ok_or_else(|| format!("unterminated string at byte {}", self.p))?;
        self.p += end + 1;
        Ok(&rest[..end])
    }
}

/// A value of a field no type here takes, kept as decoded
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Byte(u8),
    Int(i32),
    Uint(u32),
    Float(f32),
    Int64(i64),
    Uint64(u64),
    String(String),
    Enum { value: u32, name: Option<Arc<str>> },
    /// A struct or message, as (field name, value) pairs in wire order
    Record(Vec<(Arc<str>, Value)>),
    Array(Vec<Value>),
    Bytes(Vec<u8>),
}

/// Fields a type here doesn't know, by name
pub type Extra = Vec<(Arc<str>, Value)>;

// ---------------------------------------------------------------- binding fields by name

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Rec {
    Resources,
    Resource,
    Path,
    PathMetadata,
    Paint,
    Image,
    EmojiId,
    EffectTexture,
    ChainEntry,
    Glyph,
    GlyphMetadata,
    GlyphBatch,
    Geometry,
    GlyphGeometry,
    StrokeRecipe,
    Node,
    Gradient,
    ColorStop,
    GlassCorner,
    Affine,
    Color,
    Vector,
    Rect,
    RoundedRect,
    ColorAdjust,
    Guid,
}

/// What a field must look like on the wire to be read into its slot
#[derive(Clone, Copy)]
enum Expect {
    /// Any number or bool, converted
    Num,
    Str,
    Bytes,
    Enum(&'static [&'static str]),
    /// A struct or message, read by name with that record's slots
    Rec(Rec),
}

#[derive(Clone, Copy)]
struct Spec {
    name: &'static str,
    array: bool,
    expect: Expect,
}

const NONE: u8 = u8::MAX;

#[derive(Clone, Copy, Debug)]
struct Member {
    /// Index into the record's slots, or NONE
    slot: u8,
    array: bool,
    field: u16,
    def: u32,
    ty: Type,
    /// The plan of a record field, or the value table of an enum field
    sub: u32,
}

struct Plan {
    def: u32,
    /// In the definition's field order
    members: Vec<Member>,
}

struct Bind {
    plans: Vec<Plan>,
    /// Wire value to index in the Rust enum's names, NO when unknown
    enums: Vec<Vec<u16>>,
    root: u32,
}

impl Bind {
    fn new(defs: &[Def], names: &HashMap<Arc<str>, u32>) -> R<Bind> {
        let root = *names.get("Resources").ok_or("the schema has no Resources message")?;
        if defs[root as usize].kind == DefKind::Enum {
            return Err("Resources is an enum".into());
        }
        let mut b = Binder { defs, plans: Vec::new(), enums: Vec::new(), plan_ids: HashMap::new(), enum_ids: HashMap::new() };
        let root = b.plan(root, Rec::Resources);
        Ok(Bind { plans: b.plans, enums: b.enums, root })
    }
}

struct Binder<'a> {
    defs: &'a [Def],
    plans: Vec<Plan>,
    enums: Vec<Vec<u16>>,
    plan_ids: HashMap<(u32, Rec), u32>,
    enum_ids: HashMap<(u32, usize), u32>,
}

impl Binder<'_> {
    fn plan(&mut self, def: u32, rec: Rec) -> u32 {
        if let Some(&i) = self.plan_ids.get(&(def, rec)) {
            return i;
        }
        let id = self.plans.len() as u32;
        self.plan_ids.insert((def, rec), id);
        self.plans.push(Plan { def, members: Vec::new() });
        let defs = self.defs;
        let specs = rec.specs();
        let mut members = Vec::with_capacity(defs[def as usize].fields.len());
        for (i, f) in defs[def as usize].fields.iter().enumerate() {
            let mut m = Member { slot: NONE, array: f.array, field: i as u16, def, ty: f.ty, sub: 0 };
            if let Some(slot) = specs.iter().position(|s| s.name == &*f.name) {
                if let Some(sub) = self.compatible(specs[slot], f) {
                    m.slot = slot as u8;
                    m.sub = sub;
                }
            }
            members.push(m);
        }
        self.plans[id as usize].members = members;
        id
    }

    fn compatible(&mut self, spec: Spec, f: &Field) -> Option<u32> {
        if spec.array != f.array {
            return None;
        }
        let defs = self.defs;
        let kind = |t: u32| defs[t as usize].kind;
        match (spec.expect, f.ty) {
            (Expect::Num, Type::Bool | Type::Byte | Type::Int | Type::Uint | Type::Float | Type::Int64 | Type::Uint64) => Some(0),
            (Expect::Str, Type::String) | (Expect::Bytes, Type::Byte) => Some(0),
            (Expect::Enum(names), Type::Def(t)) if kind(t) == DefKind::Enum => Some(self.enum_table(t, names)),
            (Expect::Rec(rec), Type::Def(t)) if kind(t) != DefKind::Enum => Some(self.plan(t, rec)),
            _ => None,
        }
    }

    fn enum_table(&mut self, def: u32, names: &'static [&'static str]) -> u32 {
        let key = (def, names.as_ptr() as usize);
        if let Some(&i) = self.enum_ids.get(&key) {
            return i;
        }
        let fields = &self.defs[def as usize].fields;
        let max = fields.iter().map(|f| f.value).max().unwrap_or(0).min(4095);
        let mut table = vec![NO; max as usize + 1];
        for f in fields.iter().filter(|f| f.value <= max) {
            if let Some(i) = names.iter().position(|n| *n == &*f.name) {
                table[f.value as usize] = i as u16;
            }
        }
        let id = self.enums.len() as u32;
        self.enums.push(table);
        self.enum_ids.insert(key, id);
        id
    }
}

macro_rules! slots {
    ($name:ident { $($var:ident: $field:literal, $array:literal, $expect:expr;)* }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum $name { $($var),* }
        impl $name {
            const SPECS: &'static [Spec] = &[$(Spec { name: $field, array: $array, expect: $expect }),*];
            const ALL: &'static [$name] = &[$($name::$var),*];
            #[inline]
            fn of(m: &Member) -> Option<$name> {
                Self::ALL.get(m.slot as usize).copied()
            }
        }
    };
}

macro_rules! nums {
    ($($field:literal),*) => { &[$(Spec { name: $field, array: false, expect: Expect::Num }),*] };
}

use Expect::{Bytes as B, Num as N, Rec as Rc, Str as St};

slots!(ResourcesS {
    Paths: "paths", true, Rc(Rec::Path);
    Resources: "resources", true, Rc(Rec::Resource);
});

slots!(ResourceS {
    Paint: "paint", false, Rc(Rec::Paint);
    Node: "node", false, Rc(Rec::Node);
});

slots!(PathS {
    WindingRule: "windingRule", false, Expect::Enum(WindingRule::NAMES);
    Commands: "commands", true, B;
    Metadata: "metadata", false, Rc(Rec::PathMetadata);
    ContentHash: "contentHash", false, N;
});

slots!(PathMetadataS {
    Type: "type", false, Expect::Enum(PathType::NAMES);
    Values: "values", true, N;
    Bounds: "bounds", false, Rc(Rec::Rect);
});

slots!(PaintS {
    Type: "type", false, Expect::Enum(PaintType::NAMES);
    BlendMode: "blendMode", false, Expect::Enum(BlendMode::NAMES);
    Opacity: "opacity", false, N;
    Color: "color", false, Rc(Rec::Color);
    Transform: "transform", false, Rc(Rec::Affine);
    Gradient: "gradient", false, Rc(Rec::Gradient);
    Thumbnail: "thumbnail", false, Rc(Rec::Image);
    Image: "image", false, Rc(Rec::Image);
    FilterColorAdjust: "filterColorAdjust", false, Rc(Rec::ColorAdjust);
    ImageScaleMode: "imageScaleMode", false, Expect::Enum(ImageScaleMode::NAMES);
    ImageRotationDeg: "imageRotationDeg", false, N;
    ImageScalingFactor: "imageScalingFactor", false, N;
    OriginalImageWidth: "originalImageWidth", false, N;
    OriginalImageHeight: "originalImageHeight", false, N;
    ColorspaceConversion: "colorspaceConversion", false, Expect::Enum(ColorspaceConversion::NAMES);
    EmojiId: "emojiId", false, Rc(Rec::EmojiId);
    PatternSourceNodeId: "patternSourceNodeId", false, N;
    PatternTileType: "patternTileType", false, Expect::Enum(PatternTileType::NAMES);
    PatternHorizontalAlignment: "patternHorizontalAlignment", false, Expect::Enum(PatternAlignment::NAMES);
    PatternVerticalAlignment: "patternVerticalAlignment", false, Expect::Enum(PatternAlignment::NAMES);
    PatternScale: "patternScale", false, N;
    PatternSpacing: "patternSpacing", false, Rc(Rec::Vector);
    PatternSourceAbsoluteBounds: "patternSourceAbsoluteBounds", false, Rc(Rec::Rect);
    CustomEffectTexture: "customEffectTexture", false, Rc(Rec::EffectTexture);
    VideoNodeId: "videoNodeId", false, Rc(Rec::Guid);
    InterpolationColorSpace: "interpolationColorSpace", false, Expect::Enum(InterpolationColorSpace::NAMES);
    InterpolationHueMethod: "interpolationHueMethod", false, Expect::Enum(InterpolationHueMethod::NAMES);
    GradientColorProfile: "gradientColorProfile", false, Expect::Enum(ColorProfile::NAMES);
    MediaGeneration: "mediaGeneration", false, N;
    VideoFillPaintId: "videoFillPaintId", false, Rc(Rec::Guid);
});

slots!(ImageS {
    Hash: "hash", true, B;
});

slots!(EmojiIdS {
    CodePoints: "codePoints", true, N;
    ImageSet: "imageSet", false, Expect::Enum(EmojiImageSet::NAMES);
});

slots!(ChainEntryS {
    CodeComponentId: "codeComponentId", false, St;
    ParamsJson: "paramsJson", false, St;
    OccurrenceId: "occurrenceId", false, St;
});

slots!(EffectTextureS {
    InstanceId: "instanceId", false, N;
    IntraLayerChain: "intraLayerChain", true, Rc(Rec::ChainEntry);
    DispatchRevision: "dispatchRevision", false, N;
    NodeGuid: "nodeGuid", false, Rc(Rec::Guid);
    InitialTextureFallbackInstanceId: "initialTextureFallbackInstanceId", false, N;
});

slots!(GlyphS {
    PathIndex: "pathIndex", false, N;
    Position: "position", false, Rc(Rec::Vector);
    FontSize: "fontSize", false, N;
    Rotation: "rotation", false, N;
    StyleId: "styleID", false, N;
    EmojiPaintIndices: "emojiPaintIndices", true, N;
});

slots!(GlyphMetadataS {
    Advance: "advance", false, N;
    Rotation: "rotation", false, N;
});

slots!(GlyphBatchS {
    Glyphs: "glyphs", true, Rc(Rec::Glyph);
    DecorationGeometryPathIndices: "decorationGeometryPathIndices", true, N;
    ShouldRoundY: "shouldRoundY", false, N;
});

slots!(GeometryS {
    PathIndex: "pathIndex", false, N;
    PaintIndices: "paintIndices", true, N;
    GlyphBatch: "glyphBatch", false, Rc(Rec::GlyphBatch);
    GlyphMetadata: "glyphMetadata", true, Rc(Rec::GlyphMetadata);
});

slots!(GlyphGeometryS {
    Glyphs: "glyphs", true, Rc(Rec::Glyph);
    PaintIndices: "paintIndices", true, N;
});

slots!(StrokeRecipeS {
    Tags: "tags", true, B;
    Data: "data", true, N;
    HalfWidth: "halfWidth", false, N;
    MiterLimit: "miterLimit", false, N;
    CapExtent: "capExtent", false, N;
    CapBreadth: "capBreadth", false, N;
    Cap: "cap", false, N;
    Join: "join", false, N;
});

slots!(GradientS {
    Type: "type", false, Expect::Enum(GradientType::NAMES);
    Stops: "stops", true, Rc(Rec::ColorStop);
});

slots!(ColorStopS {
    Color: "color", false, Rc(Rec::Color);
    Position: "position", false, N;
});

slots!(GlassCornerS {
    RectSize: "rectSize", false, Rc(Rec::Vector);
    CornerRadii: "cornerRadii", true, N;
    CornerSmoothings: "cornerSmoothings", true, N;
    CornerRadiiIndependent: "cornerRadiiIndependent", false, N;
});

slots!(NodeS {
    Op: "op", false, Expect::Enum(Op::NAMES);
    ChildIndices: "childIndices", true, N;
    ChildIndex: "childIndex", false, N;
    Radius: "radius", false, N;
    Clamp: "clamp", false, Rc(Rec::Rect);
    BlendMode: "blendMode", false, Expect::Enum(BlendMode::NAMES);
    Opacity: "opacity", false, N;
    Offset: "offset", false, Rc(Rec::Vector);
    Transform: "transform", false, Rc(Rec::Affine);
    PaintSize: "paintSize", false, Rc(Rec::Vector);
    Geometry: "geometry", false, Rc(Rec::Geometry);
    RoundedRectInfo: "roundedRectInfo", false, Rc(Rec::RoundedRect);
    BgBlurExpansionForFrameFill: "bgBlurExpansionForFrameFill", false, N;
    IsEllipse: "isEllipse", false, N;
    IsCreatedFromBrush: "isCreatedFromBrush", false, N;
    FillGeometries: "fillGeometries", true, Rc(Rec::Geometry);
    StrokeGeometries: "strokeGeometries", true, Rc(Rec::Geometry);
    StrokeMode: "strokeMode", false, Expect::Enum(StrokeMask::NAMES);
    PaintedGlyphGeometry: "paintedGlyphGeometry", false, Rc(Rec::GlyphGeometry);
    AlphaNodeIndex: "alphaNodeIndex", false, N;
    ColorNodeIndex: "colorNodeIndex", false, N;
    MaskOpType: "maskOpType", false, Expect::Enum(MaskOpType::NAMES);
    InnerShadowFill: "innerShadowFill", false, Rc(Rec::Color);
    InnerShadowAlphaRequiresHardMask: "innerShadowAlphaRequiresHardMask", false, N;
    Color: "color", false, Rc(Rec::Color);
    BlurOpType: "blurOpType", false, Expect::Enum(BlurOpType::NAMES);
    StartOffset: "startOffset", false, Rc(Rec::Vector);
    EndOffset: "endOffset", false, Rc(Rec::Vector);
    StartRadius: "startRadius", false, N;
    InstanceTransforms: "instanceTransforms", true, Rc(Rec::Affine);
    NoiseSize: "noiseSize", false, Rc(Rec::Vector);
    ClipToShape: "clipToShape", false, N;
    CenterPoint: "centerPoint", false, Rc(Rec::Vector);
    Seed: "seed", false, N;
    NoiseType: "noiseType", false, Expect::Enum(NoiseType::NAMES);
    Density: "density", false, N;
    NoiseColor: "noiseColor", false, Rc(Rec::Color);
    SecondaryNoiseColor: "secondaryNoiseColor", false, Rc(Rec::Color);
    BlurRadius: "blurRadius", false, N;
    Splay: "splay", false, N;
    RefractionIntensity: "refractionIntensity", false, N;
    RefractionDistance: "refractionDistance", false, N;
    TintColor: "tintColor", false, Rc(Rec::Color);
    LightIntensity: "lightIntensity", false, N;
    LightAngle: "lightAngle", false, N;
    BevelSize: "bevelSize", false, N;
    ChromaticAberration: "chromaticAberration", false, N;
    GlassRectCornerInfo: "glassRectCornerInfo", false, Rc(Rec::GlassCorner);
    RectCenterPoint: "rectCenterPoint", false, Rc(Rec::Vector);
    NodeToCanvasSpaceTransform: "nodeToCanvasSpaceTransform", false, Rc(Rec::Affine);
    FillIndex: "fillIndex", false, N;
    MetadataType: "metadataType", false, Expect::Enum(MetadataType::NAMES);
    ExpandedBounds: "expandedBounds", false, Rc(Rec::Rect);
    PatternSourceNodeId: "patternSourceNodeId", false, N;
    CustomEffectTextures: "customEffectTextures", true, Rc(Rec::EffectTexture);
    CustomEffectRenderTargetLocalBounds: "customEffectRenderTargetLocalBounds", false, Rc(Rec::Rect);
    NodeId: "nodeId", false, N;
    IsExternalRef: "isExternalRef", false, N;
    UsesCompositedBackdrop: "usesCompositedBackdrop", false, N;
    CompositeTileGridTransformHandling: "compositeTileGridTransformHandling", false, Expect::Enum(TransformHandling::NAMES);
    CompositeTileGridEnableSpatialReuseGate: "compositeTileGridEnableSpatialReuseGate", false, N;
    CompositeTileGridExpandBounds: "compositeTileGridExpandBounds", false, N;
    GpuStrokeRecipe: "gpuStrokeRecipe", false, Rc(Rec::StrokeRecipe);
    SpatialPlaneContentId: "spatialPlaneContentId", false, N;
    SpatialPlaneResetsFacing: "spatialPlaneResetsFacing", false, N;
    SpatialPlaneBlendMode: "spatialPlaneBlendMode", false, Expect::Enum(BlendMode::NAMES);
    SpatialPlaneUsesParentDensity: "spatialPlaneUsesParentDensity", false, N;
    SpatialTransformChildId: "spatialTransformChildId", false, N;
    SpatialTransform: "spatialTransform", true, N;
    SpatialCompositionChildIds: "spatialCompositionChildIds", true, N;
    VolatileMaxTransformScale: "volatileMaxTransformScale", false, N;
    LocalBackdropIndex: "localBackdropIndex", false, N;
});

const AFFINE: &[Spec] = nums!("m00", "m01", "m02", "m10", "m11", "m12");
const COLOR: &[Spec] = nums!("r", "g", "b", "a");
const VECTOR: &[Spec] = nums!("x", "y");
const RECT: &[Spec] = nums!("x", "y", "w", "h");
const ROUNDED_RECT: &[Spec] =
    nums!("topLeftCornerRadius", "topRightCornerRadius", "bottomLeftCornerRadius", "bottomRightCornerRadius", "cornerSmoothing");
const COLOR_ADJUST: &[Spec] = nums!("tint", "vibrance", "shadows", "highlights", "exposure", "temperature", "contrast");
const GUID: &[Spec] = nums!("sessionID", "localID");

impl Rec {
    fn specs(self) -> &'static [Spec] {
        match self {
            Rec::Resources => ResourcesS::SPECS,
            Rec::Resource => ResourceS::SPECS,
            Rec::Path => PathS::SPECS,
            Rec::PathMetadata => PathMetadataS::SPECS,
            Rec::Paint => PaintS::SPECS,
            Rec::Image => ImageS::SPECS,
            Rec::EmojiId => EmojiIdS::SPECS,
            Rec::EffectTexture => EffectTextureS::SPECS,
            Rec::ChainEntry => ChainEntryS::SPECS,
            Rec::Glyph => GlyphS::SPECS,
            Rec::GlyphMetadata => GlyphMetadataS::SPECS,
            Rec::GlyphBatch => GlyphBatchS::SPECS,
            Rec::Geometry => GeometryS::SPECS,
            Rec::GlyphGeometry => GlyphGeometryS::SPECS,
            Rec::StrokeRecipe => StrokeRecipeS::SPECS,
            Rec::Node => NodeS::SPECS,
            Rec::Gradient => GradientS::SPECS,
            Rec::ColorStop => ColorStopS::SPECS,
            Rec::GlassCorner => GlassCornerS::SPECS,
            Rec::Affine => AFFINE,
            Rec::Color => COLOR,
            Rec::Vector => VECTOR,
            Rec::Rect => RECT,
            Rec::RoundedRect => ROUNDED_RECT,
            Rec::ColorAdjust => COLOR_ADJUST,
            Rec::Guid => GUID,
        }
    }
}

// ---------------------------------------------------------------- enums

trait KiwiEnum: Copy {
    /// Member names as the schema spells them
    const NAMES: &'static [&'static str];
    fn from_index(i: usize) -> Self;
    fn unknown(value: u32) -> Self;
}

macro_rules! kiwi_enum {
    ($(#[$meta:meta])* $name:ident { $($var:ident = $wire:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($var,)*
            /// A member this module doesn't know, by wire value
            Unknown(u32),
        }
        impl KiwiEnum for $name {
            const NAMES: &'static [&'static str] = &[$($wire),*];
            fn from_index(i: usize) -> Self {
                const ALL: &[$name] = &[$($name::$var),*];
                ALL[i]
            }
            fn unknown(value: u32) -> Self {
                $name::Unknown(value)
            }
        }
    };
}

kiwi_enum!(
    /// Wire `NONE` means NORMAL. Layer blend modes keep PASS_THROUGH
    BlendMode {
        None = "NONE", PassThrough = "PASS_THROUGH", Normal = "NORMAL", Darken = "DARKEN", Multiply = "MULTIPLY",
        LinearBurn = "LINEAR_BURN", ColorBurn = "COLOR_BURN", Lighten = "LIGHTEN", Screen = "SCREEN",
        LinearDodge = "LINEAR_DODGE", ColorDodge = "COLOR_DODGE", Overlay = "OVERLAY", SoftLight = "SOFT_LIGHT",
        HardLight = "HARD_LIGHT", Difference = "DIFFERENCE", Exclusion = "EXCLUSION", Hue = "HUE",
        Saturation = "SATURATION", Color = "COLOR", Luminosity = "LUMINOSITY",
    }
);
kiwi_enum!(BlurOpType { Normal = "NORMAL", Progressive = "PROGRESSIVE" });
kiwi_enum!(ColorspaceConversion {
    NoConversion = "NO_CONVERSION", SrgbToDisplayP3 = "SRGB_TO_DISPLAY_P3", DisplayP3ToSrgb = "DISPLAY_P3_TO_SRGB",
    HlgBt2020ToSrgb = "HLG_BT2020_TO_SRGB", HlgBt2020ToDisplayP3 = "HLG_BT2020_TO_DISPLAY_P3",
    PqBt2020ToSrgb = "PQ_BT2020_TO_SRGB", PqBt2020ToDisplayP3 = "PQ_BT2020_TO_DISPLAY_P3",
});
kiwi_enum!(EmojiImageSet { Apple = "APPLE", Noto = "NOTO" });
kiwi_enum!(
    /// STRETCH is the plugin API's CROP
    ImageScaleMode { Stretch = "STRETCH", Fit = "FIT", Fill = "FILL", Tile = "TILE" }
);
kiwi_enum!(WindingRule { NonZero = "NONZERO", Odd = "ODD" });
kiwi_enum!(PaintType {
    Solid = "SOLID", GradientLinear = "GRADIENT_LINEAR", GradientRadial = "GRADIENT_RADIAL",
    GradientAngular = "GRADIENT_ANGULAR", GradientDiamond = "GRADIENT_DIAMOND", Image = "IMAGE", Emoji = "EMOJI",
    Video = "VIDEO", Pattern = "PATTERN", CustomEffect = "CUSTOM_EFFECT",
});
kiwi_enum!(PathType { Bezier = "BEZIER", Rect = "RECT", RoundedRect = "ROUNDED_RECT", Ellipse = "ELLIPSE", ArcSweep = "ARC_SWEEP" });
kiwi_enum!(
    /// The schema's `RenderNodeOp`, which differs from the op enum in
    /// Figma's JS from TRANSLATE on
    Op {
        Metadata = "METADATA", Group = "GROUP", Fill = "FILL", FillStroke = "FILL_STROKE", FillGlyphs = "FILL_GLYPHS",
        Mask = "MASK", Blur = "BLUR", Blend = "BLEND", Translate = "TRANSLATE", CompositeTile = "COMPOSITE_TILE",
        HardMaskWithColor = "HARD_MASK_WITH_COLOR", StaticTransformBinding = "STATIC_TRANSFORM_BINDING",
        ScatterBrushFill = "SCATTER_BRUSH_FILL", ScatterBrushFillStroke = "SCATTER_BRUSH_FILL_STROKE",
        VolatilePropertyBinding = "VOLATILE_PROPERTY_BINDING", VolatileRenderEffect = "VOLATILE_RENDER_EFFECT",
        Dissolve = "DISSOLVE", CompositeTileGrid = "COMPOSITE_TILE_GRID", SupersampleGroup = "SUPERSAMPLE_GROUP",
        Grain = "GRAIN", Noise = "NOISE", Glass = "GLASS", OcclusionScope = "OCCLUSION_SCOPE",
        CustomEffect = "CUSTOM_EFFECT", CustomEffectFill = "CUSTOM_EFFECT_FILL", SpatialPlane = "SPATIAL_PLANE",
        SpatialTransform = "SPATIAL_TRANSFORM", SpatialComposition = "SPATIAL_COMPOSITION",
    }
);
kiwi_enum!(MaskOpType {
    Normal = "NORMAL", Luminance = "LUMINANCE", InnerShadow = "INNER_SHADOW", BackgroundBlur = "BACKGROUND_BLUR",
    KnockoutShadow = "KNOCKOUT_SHADOW", Glass = "GLASS", Pattern = "PATTERN",
});
kiwi_enum!(
    /// Which side of the fill path a FILL_STROKE's strokes are kept on
    StrokeMask { Inside = "INSIDE", Outside = "OUTSIDE" }
);
kiwi_enum!(MetadataType {
    BackgroundBlur = "BACKGROUND_BLUR", Glass = "GLASS", RadialRepetition = "RADIAL_REPETITION",
    BackfaceHidden = "BACKFACE_HIDDEN",
});
kiwi_enum!(TransformHandling { Deferred = "DEFERRED", Baked = "BAKED" });
kiwi_enum!(GradientType { Clamp = "CLAMP", Wrap = "WRAP" });
kiwi_enum!(PatternTileType {
    Rectangular = "RECTANGULAR", HorizontalHexagonal = "HORIZONTAL_HEXAGONAL", VerticalHexagonal = "VERTICAL_HEXAGONAL",
});
kiwi_enum!(PatternAlignment { Start = "START", Center = "CENTER", End = "END" });
kiwi_enum!(NoiseType { Multitone = "MULTITONE", Monotone = "MONOTONE", Duotone = "DUOTONE" });
kiwi_enum!(InterpolationColorSpace {
    Default = "DEFAULT", Oklab = "OKLAB", Oklch = "OKLCH", Hsl = "HSL", SrgbLinear = "SRGB_LINEAR",
});
kiwi_enum!(InterpolationHueMethod { Shorter = "SHORTER", Longer = "LONGER", Increasing = "INCREASING", Decreasing = "DECREASING" });
kiwi_enum!(ColorProfile { Srgb = "SRGB", DisplayP3 = "DISPLAY_P3" });

// ---------------------------------------------------------------- model

/// `[m00, m01, m02, m10, m11, m12]`, see the module docs
pub type Transform = [f32; 6];

pub const IDENTITY: Transform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// `a · b`: `b` applied first, then `a`
pub fn concat(a: &Transform, b: &Transform) -> Transform {
    [
        a[0] * b[0] + a[1] * b[3],
        a[0] * b[1] + a[1] * b[4],
        a[0] * b[2] + a[1] * b[5] + a[2],
        a[3] * b[0] + a[4] * b[3],
        a[3] * b[1] + a[4] * b[4],
        a[3] * b[2] + a[4] * b[5] + a[5],
    ]
}

pub fn to_matrix(t: &Transform) -> skia_safe::Matrix {
    skia_safe::Matrix::new_all(t[0], t[1], t[2], t[3], t[4], t[5], 0.0, 0.0, 1.0)
}

pub type Vector = [f32; 2];

/// Straight (not premultiplied) RGBA, 0 to 1, sRGB unless a color profile
/// says otherwise. Figma's own decoder clamps paint colors to 0..1 and
/// turns NaN into 0, these are as sent
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub fn to_skia(self) -> skia_safe::Color4f {
        skia_safe::Color4f::new(self.r, self.g, self.b, self.a)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn to_skia(self) -> skia_safe::Rect {
        skia_safe::Rect::from_xywh(self.x, self.y, self.w, self.h)
    }
}

/// A Figma node id, `session_id:local_id`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Guid {
    pub session_id: u32,
    pub local_id: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RoundedRectInfo {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_left: f32,
    pub bottom_right: f32,
    pub corner_smoothing: f32,
}

/// An image paint's filters. `vibrance` is the plugin API's saturation and
/// `contrast` is the plugin's contrast × 0.3
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorAdjust {
    pub tint: f32,
    pub vibrance: f32,
    pub shadows: f32,
    pub highlights: f32,
    pub exposure: f32,
    pub temperature: f32,
    pub contrast: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorStop {
    pub color: Color,
    pub position: f32,
}

/// Angular gradients already carry the two seam stops at 0 and 1
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gradient {
    pub kind: Option<GradientType>,
    pub stops: Vec<ColorStop>,
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Image {
    /// SHA-1, the plugin API's `imageHash`
    pub hash: Vec<u8>,
    pub extra: Extra,
}

impl Image {
    pub fn hex(&self) -> String {
        self.hash.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmojiId {
    pub code_points: Vec<u32>,
    pub image_set: Option<EmojiImageSet>,
    pub extra: Extra,
}

/// One custom effect in a layer's effect stack
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChainEntry {
    /// `CodeComponentId:<sha1>/<version>`
    pub code_component_id: Option<String>,
    pub params_json: Option<String>,
    pub occurrence_id: Option<String>,
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectTexture {
    pub instance_id: Option<u32>,
    pub intra_layer_chain: Vec<ChainEntry>,
    pub dispatch_revision: Option<u64>,
    /// The Figma node the effect is on
    pub node_guid: Option<Guid>,
    pub initial_texture_fallback_instance_id: Option<u32>,
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paint {
    pub kind: Option<PaintType>,
    pub blend_mode: Option<BlendMode>,
    /// Effective alpha is `color.a × opacity`
    pub opacity: Option<f32>,
    pub color: Option<Color>,
    /// Gradients: the plugin API's `gradientTransform`, applied to node
    /// space normalized by the node's `paint_size`. STRETCH images: the
    /// plugin's `imageTransform`
    pub transform: Option<Transform>,
    pub gradient: Option<Gradient>,
    pub thumbnail: Option<Image>,
    pub image: Option<Image>,
    pub filter_color_adjust: Option<ColorAdjust>,
    pub image_scale_mode: Option<ImageScaleMode>,
    pub image_rotation_deg: Option<f32>,
    pub image_scaling_factor: Option<f32>,
    pub original_image_width: Option<i32>,
    pub original_image_height: Option<i32>,
    pub colorspace_conversion: Option<ColorspaceConversion>,
    pub emoji_id: Option<EmojiId>,
    /// Node id of the pattern's source subtree
    pub pattern_source_node_id: Option<u32>,
    pub pattern_tile_type: Option<PatternTileType>,
    pub pattern_horizontal_alignment: Option<PatternAlignment>,
    pub pattern_vertical_alignment: Option<PatternAlignment>,
    pub pattern_scale: Option<f32>,
    pub pattern_spacing: Option<Vector>,
    pub pattern_source_absolute_bounds: Option<Rect>,
    pub custom_effect_texture: Option<EffectTexture>,
    pub video_node_id: Option<Guid>,
    pub interpolation_color_space: Option<InterpolationColorSpace>,
    pub interpolation_hue_method: Option<InterpolationHueMethod>,
    pub gradient_color_profile: Option<ColorProfile>,
    pub media_generation: Option<u64>,
    pub video_fill_paint_id: Option<Guid>,
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Verb {
    Close,
    Move,
    Line,
    Quad,
    Cubic,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    Move(Vector),
    Line(Vector),
    Quad(Vector, Vector),
    Cubic(Vector, Vector, Vector),
    Close,
}

/// The analytic shape Figma sends with some paths for fast paths
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathMetadata {
    pub kind: Option<PathType>,
    pub values: Vec<f32>,
    pub bounds: Option<Rect>,
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape<'a> {
    Bezier,
    Rect(Rect),
    /// Radii already clamped to half the shorter side, top left and top
    /// right first. The captures never had the two bottom radii differ, so
    /// whether they come as [bottom right, bottom left] (Skia's clockwise
    /// order, which `to_skia` assumes) or the other way round is unverified.
    /// `more` holds any values after the four radii
    RoundedRect { bounds: Rect, radii: [f32; 4], more: &'a [f32] },
    Ellipse { bounds: Rect, rx: f32, ry: f32 },
    /// Never seen in captures, its values are unknown
    ArcSweep { bounds: Option<Rect>, values: &'a [f32] },
    /// A kind this module doesn't know, or a known one without its bounds
    Unknown { kind: PathType, bounds: Option<Rect>, values: &'a [f32] },
}

impl PathMetadata {
    pub fn shape(&self) -> Shape<'_> {
        let v = &self.values[..];
        let at = |i: usize| v.get(i).copied().unwrap_or(0.0);
        match (self.kind, self.bounds) {
            (None | Some(PathType::Bezier), _) => Shape::Bezier,
            (Some(PathType::Rect), Some(bounds)) => Shape::Rect(bounds),
            (Some(PathType::RoundedRect), Some(bounds)) => {
                Shape::RoundedRect { bounds, radii: [at(0), at(1), at(2), at(3)], more: v.get(4..).unwrap_or(&[]) }
            }
            (Some(PathType::Ellipse), Some(bounds)) => Shape::Ellipse {
                bounds,
                rx: v.first().copied().unwrap_or(bounds.w / 2.0),
                ry: v.get(1).copied().unwrap_or(bounds.h / 2.0),
            },
            (Some(PathType::ArcSweep), bounds) => Shape::ArcSweep { bounds, values: v },
            (Some(kind), bounds) => Shape::Unknown { kind, bounds, values: v },
        }
    }
}

/// An outline in its node's local space, the shape within
/// `[0, paint_size]`. `verbs` and `points` hold the commands compactly,
/// `commands()` reads them back
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub fill_rule: Option<WindingRule>,
    pub verbs: Vec<Verb>,
    pub points: Vec<Vector>,
    pub metadata: Option<PathMetadata>,
    /// Key of the worker's path cache. Later frames send only the hash
    pub content_hash: Option<u64>,
    pub extra: Extra,
}

impl Path {
    pub fn commands(&self) -> Commands<'_> {
        Commands { verbs: self.verbs.iter(), points: &self.points }
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty() && self.metadata.as_ref().is_none_or(|m| matches!(m.shape(), Shape::Bezier))
    }

    pub fn fill_type(&self) -> skia_safe::PathFillType {
        match self.fill_rule {
            Some(WindingRule::Odd) => skia_safe::PathFillType::EvenOdd,
            _ => skia_safe::PathFillType::Winding,
        }
    }

    /// The commands as a Skia path. A path that came with only its
    /// analytic shape is built from that
    pub fn to_skia(&self) -> skia_safe::Path {
        let mut b = skia_safe::PathBuilder::new_with_fill_type(self.fill_type());
        if self.verbs.is_empty() {
            if let Some(m) = &self.metadata {
                match m.shape() {
                    Shape::Rect(r) => {
                        b.add_rect(r.to_skia(), None, None);
                    }
                    Shape::RoundedRect { bounds, radii, .. } => {
                        let radii = radii.map(|r| skia_safe::Vector::new(r, r));
                        b.add_rrect(skia_safe::RRect::new_rect_radii(bounds.to_skia(), &radii), None, None);
                    }
                    Shape::Ellipse { bounds, .. } => {
                        b.add_oval(bounds.to_skia(), None, None);
                    }
                    _ => {}
                }
            }
            return b.detach();
        }
        b.inc_reserve(self.points.len(), self.verbs.len(), 0);
        let pt = |p: Vector| skia_safe::Point::new(p[0], p[1]);
        for cmd in self.commands() {
            match cmd {
                PathCmd::Move(p) => b.move_to(pt(p)),
                PathCmd::Line(p) => b.line_to(pt(p)),
                PathCmd::Quad(c, p) => b.quad_to(pt(c), pt(p)),
                PathCmd::Cubic(c1, c2, p) => b.cubic_to(pt(c1), pt(c2), pt(p)),
                PathCmd::Close => b.close(),
            };
        }
        b.detach()
    }
}

pub struct Commands<'a> {
    verbs: std::slice::Iter<'a, Verb>,
    points: &'a [Vector],
}

impl Iterator for Commands<'_> {
    type Item = PathCmd;

    fn next(&mut self) -> Option<PathCmd> {
        let verb = *self.verbs.next()?;
        let p = self.points;
        Some(match verb {
            Verb::Close => PathCmd::Close,
            Verb::Move => {
                self.points = &p[1..];
                PathCmd::Move(p[0])
            }
            Verb::Line => {
                self.points = &p[1..];
                PathCmd::Line(p[0])
            }
            Verb::Quad => {
                self.points = &p[2..];
                PathCmd::Quad(p[0], p[1])
            }
            Verb::Cubic => {
                self.points = &p[3..];
                PathCmd::Cubic(p[0], p[1], p[2])
            }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.verbs.size_hint()
    }
}

/// Figma's path blob: an opcode byte, then little-endian f32 coordinates.
/// 0 close, 1 move, 2 line, 3 quad, 4 cubic
fn parse_commands(b: &[u8], verbs: &mut Vec<Verb>, points: &mut Vec<Vector>) -> R<()> {
    let (mut verb_count, mut point_count, mut q) = (0, 0, 0);
    while q < b.len() {
        let n = match b[q] {
            0 => 0,
            1 | 2 => 1,
            3 => 2,
            4 => 3,
            op => return Err(format!("unknown path opcode {op} at offset {q} of a path")),
        };
        q += 1 + n * 8;
        verb_count += 1;
        point_count += n;
    }
    if q != b.len() {
        return Err("a path's commands end in the middle of a command".into());
    }
    verbs.reserve_exact(verb_count);
    points.reserve_exact(point_count);
    let f = |at: usize| f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let mut q = 0;
    while q < b.len() {
        let (verb, n) = match b[q] {
            0 => (Verb::Close, 0),
            1 => (Verb::Move, 1),
            2 => (Verb::Line, 1),
            3 => (Verb::Quad, 2),
            _ => (Verb::Cubic, 3),
        };
        q += 1;
        for _ in 0..n {
            points.push([f(q), f(q + 4)]);
            q += 8;
        }
        verbs.push(verb);
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glyph {
    /// Unit-em outline, y up. Placed by `translate(position) ·
    /// scale(font_size, -font_size) · rotate(rotation)` in node space
    pub path: Option<Arc<Path>>,
    /// Baseline origin in node space
    pub position: Option<Vector>,
    pub font_size: Option<f32>,
    pub rotation: Option<f32>,
    pub style_id: Option<i32>,
    pub emoji_paints: Vec<Arc<Paint>>,
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphMetadata {
    /// In em
    pub advance: Option<f32>,
    pub rotation: Option<f32>,
    pub extra: Extra,
}

/// A text fill's glyphs. The fill's own path already holds the laid out
/// outline, so drawing can ignore these
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphBatch {
    pub glyphs: Vec<Glyph>,
    /// Underlines and strikethroughs
    pub decoration_paths: Vec<Arc<Path>>,
    pub should_round_y: bool,
    pub extra: Extra,
}

/// A path and the paints filling it, bottom first
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    pub path: Option<Arc<Path>>,
    pub paints: Vec<Arc<Paint>>,
    pub glyph_batch: Option<Box<GlyphBatch>>,
    pub glyph_metadata: Vec<GlyphMetadata>,
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphGeometry {
    pub glyphs: Vec<Glyph>,
    pub paints: Vec<Arc<Paint>>,
    pub extra: Extra,
}

/// A stroke for the GPU to expand, used instead of an outlined path when
/// `tags`, `data` and `half_width` are all set
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrokeRecipe {
    pub tags: Vec<u8>,
    pub data: Vec<f32>,
    pub half_width: Option<f32>,
    pub miter_limit: Option<f32>,
    pub cap_extent: Option<f32>,
    pub cap_breadth: Option<f32>,
    pub cap: Option<u8>,
    pub join: Option<u8>,
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlassCorner {
    pub rect_size: Vector,
    pub corner_radii: Vec<f32>,
    pub corner_smoothings: Vec<f32>,
    pub corner_radii_independent: bool,
    pub extra: Extra,
}

/// A render node. Nodes are immutable: a change to one arrives as new
/// nodes for it and its ancestors. References to other nodes are node ids,
/// looked up with `Forest::get`
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: u32,
    pub op: Op,
    /// GROUP: drawn in order, first at the bottom
    pub children: Vec<u32>,
    /// The single child of STATIC_TRANSFORM_BINDING, BLEND, BLUR, METADATA
    /// and the effect ops
    pub child: Option<u32>,
    /// STATIC_TRANSFORM_BINDING: concatenated for the child. Drawables: the
    /// node's own transform
    pub transform: Option<Transform>,
    /// The node size paints are normalized by
    pub paint_size: Option<Vector>,
    /// FILL: the path and its paints
    pub geometry: Option<Geometry>,
    /// FILL_STROKE and SCATTER_BRUSH_FILL
    pub fill_geometries: Vec<Geometry>,
    /// FILL_STROKE: widened outlines, kept inside or outside the fill path
    /// per `stroke_mode`
    pub stroke_geometries: Vec<Geometry>,
    pub stroke_mode: Option<StrokeMask>,
    pub bg_blur_expansion_for_frame_fill: Option<f32>,
    pub is_created_from_brush: bool,
    pub blend_mode: Option<BlendMode>,
    pub opacity: Option<f32>,
    /// MASK and HARD_MASK_WITH_COLOR: the coverage subtree
    pub alpha_node: Option<u32>,
    /// MASK: the content subtree
    pub color_node: Option<u32>,
    pub mask_op: Option<MaskOpType>,
    /// HARD_MASK_WITH_COLOR: the silhouette's color
    pub color: Option<Color>,
    /// BLUR and GRAIN
    pub radius: Option<f32>,
    /// The fields only effects and rarer ops use
    pub ext: Option<Box<NodeExt>>,
    pub extra: Extra,
}

impl Default for Node {
    fn default() -> Self {
        Node {
            id: 0,
            op: Op::Unknown(0),
            children: Vec::new(),
            child: None,
            transform: None,
            paint_size: None,
            geometry: None,
            fill_geometries: Vec::new(),
            stroke_geometries: Vec::new(),
            stroke_mode: None,
            bg_blur_expansion_for_frame_fill: None,
            is_created_from_brush: false,
            blend_mode: None,
            opacity: None,
            alpha_node: None,
            color_node: None,
            mask_op: None,
            color: None,
            radius: None,
            ext: None,
            extra: Vec::new(),
        }
    }
}

static NO_EXT: LazyLock<NodeExt> = LazyLock::new(NodeExt::default);

impl Node {
    /// `ext`, or all its fields unset
    pub fn ext(&self) -> &NodeExt {
        self.ext.as_deref().unwrap_or(&NO_EXT)
    }

    /// Every node id this node refers to
    pub fn refs(&self) -> impl Iterator<Item = u32> + '_ {
        let ext = self.ext.as_deref().into_iter().flat_map(|e| {
            e.pattern_source_node_id
                .into_iter()
                .chain(e.local_backdrop)
                .chain(e.fill_index)
                .chain(e.spatial_plane_content_id)
                .chain(e.spatial_transform_child_id)
                .chain(e.spatial_composition_child_ids.iter().copied())
        });
        self.children.iter().copied().chain(self.child).chain(self.alpha_node).chain(self.color_node).chain(ext)
    }

    fn ext_mut(&mut self) -> &mut NodeExt {
        self.ext.get_or_insert_with(Box::default)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeExt {
    /// BLUR: background blurs sample with edges clamped to this rect
    pub clamp: Option<Rect>,
    pub offset: Option<Vector>,
    pub rounded_rect_info: Option<RoundedRectInfo>,
    pub is_ellipse: bool,
    /// FILL_GLYPHS
    pub painted_glyph_geometry: Option<GlyphGeometry>,
    /// MASK(INNER_SHADOW): the shadow color
    pub inner_shadow_fill: Option<Color>,
    pub inner_shadow_alpha_requires_hard_mask: bool,
    pub blur_op_type: Option<BlurOpType>,
    /// PROGRESSIVE blurs ramp from `start_radius` at `start_offset` to
    /// `radius` at `end_offset`, in node space
    pub start_offset: Option<Vector>,
    pub end_offset: Option<Vector>,
    pub start_radius: Option<f32>,
    /// SCATTER_BRUSH_FILL and RADIAL_REPETITION metadata
    pub instance_transforms: Vec<Transform>,
    pub noise_size: Option<Vector>,
    pub clip_to_shape: bool,
    pub center_point: Option<Vector>,
    pub seed: Option<u32>,
    pub noise_type: Option<NoiseType>,
    pub density: Option<f32>,
    pub noise_color: Option<Color>,
    pub secondary_noise_color: Option<Color>,
    pub blur_radius: Option<f32>,
    pub splay: Option<f32>,
    pub refraction_intensity: Option<f32>,
    pub refraction_distance: Option<f32>,
    pub tint_color: Option<Color>,
    pub light_intensity: Option<f32>,
    pub light_angle: Option<f32>,
    pub bevel_size: Option<f32>,
    pub chromatic_aberration: Option<f32>,
    pub glass_rect_corner_info: Option<GlassCorner>,
    pub rect_center_point: Option<Vector>,
    /// Despite the name, the effect's source transform in the effect
    /// node's own space. All NaN for shadow blurs
    pub node_to_canvas_space_transform: Option<Transform>,
    /// GLASS: node id of its fill
    pub fill_index: Option<u32>,
    pub metadata_type: Option<MetadataType>,
    pub expanded_bounds: Option<Rect>,
    /// MASK(PATTERN): node id of the pattern's source
    pub pattern_source_node_id: Option<u32>,
    pub custom_effect_textures: Vec<EffectTexture>,
    pub custom_effect_render_target_local_bounds: Option<Rect>,
    pub uses_composited_backdrop: bool,
    pub composite_tile_grid_transform_handling: Option<TransformHandling>,
    pub composite_tile_grid_enable_spatial_reuse_gate: bool,
    pub composite_tile_grid_expand_bounds: bool,
    pub gpu_stroke_recipe: Option<StrokeRecipe>,
    pub spatial_plane_content_id: Option<u32>,
    pub spatial_plane_resets_facing: bool,
    pub spatial_plane_blend_mode: Option<BlendMode>,
    pub spatial_plane_uses_parent_density: bool,
    pub spatial_transform_child_id: Option<u32>,
    /// A 4×4 matrix, 16 values
    pub spatial_transform: Vec<f32>,
    pub spatial_composition_child_ids: Vec<u32>,
    pub volatile_max_transform_scale: Option<f32>,
    /// Node id, despite the name
    pub local_backdrop: Option<u32>,
}

// ---------------------------------------------------------------- resident scene

/// A map keyed by node ids or path hashes. They need no protection from
/// crafted keys, and SipHash was a fifth of a large frame's decode time
pub type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

#[derive(Clone, Copy, Default)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

/// The scene a render worker holds: every node it has been sent and not
/// told to release, and every path it has been sent, by content hash
#[derive(Default)]
pub struct Forest {
    pub nodes: IdMap<u32, Arc<Node>>,
    pub paths: IdMap<u64, Arc<Path>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Ids of the nodes this frame added, in the order they came
    pub new_nodes: Vec<u32>,
    pub consumed: usize,
    /// The last node in the frame, the root it draws from
    pub root: Option<u32>,
    pub paths: usize,
    /// Paths sent as only their hash and taken from the cache
    pub path_stubs: usize,
    pub paints: usize,
    pub external_refs: usize,
    /// Released ids that were resident
    pub released: usize,
}

impl Forest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: u32) -> Option<&Arc<Node>> {
        self.nodes.get(&id)
    }

    /// Drops everything, the path cache too, as for a new render worker
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.paths.clear();
    }

    /// Applies one frame's forest. `update_mode` 0 is a full scene and
    /// replaces the resident nodes. The path cache is kept, since keeping
    /// every path is what Figma's worker can rely on. `released` is the
    /// frame's `releasedNodeIds`, applied after the frame. On an error
    /// nothing changes but the path cache
    pub fn apply(&mut self, bytes: &[u8], update_mode: u32, released: &[u32], schema: &Schema) -> Result<Applied, String> {
        let previous = (update_mode == 0).then(|| std::mem::take(&mut self.nodes));
        if let Some(previous) = &previous {
            self.nodes.reserve(previous.len());
        }
        let mut frame = Frame {
            rd: Reader::new(bytes),
            s: schema,
            nodes: &mut self.nodes,
            cache: &mut self.paths,
            paths: Vec::new(),
            paints: Vec::new(),
            new_nodes: Vec::new(),
            pattern_refs: Vec::new(),
            root: None,
            stubs: 0,
            external: 0,
        };
        let result = frame.run();
        let mut applied = Applied {
            consumed: frame.rd.p,
            root: frame.root,
            paths: frame.paths.len(),
            path_stubs: frame.stubs,
            paints: frame.paints.len(),
            external_refs: frame.external,
            new_nodes: std::mem::take(&mut frame.new_nodes),
            released: 0,
        };
        drop(frame);
        if let Err(e) = result {
            match previous {
                Some(previous) => self.nodes = previous,
                None => {
                    for id in &applied.new_nodes {
                        self.nodes.remove(id);
                    }
                }
            }
            return Err(e);
        }
        for id in released {
            if self.nodes.remove(id).is_some() {
                applied.released += 1;
            }
        }
        Ok(applied)
    }
}

struct Frame<'a> {
    rd: Reader<'a>,
    s: &'a Schema,
    nodes: &'a mut IdMap<u32, Arc<Node>>,
    cache: &'a mut IdMap<u64, Arc<Path>>,
    paths: Vec<Arc<Path>>,
    paints: Vec<Arc<Paint>>,
    new_nodes: Vec<u32>,
    pattern_refs: Vec<u32>,
    root: Option<u32>,
    stubs: usize,
    external: usize,
}

impl<'a> Frame<'a> {
    fn run(&mut self) -> R<()> {
        self.resources(self.s.bind.root)?;
        if self.rd.p != self.rd.b.len() {
            return Err(format!("{} bytes left over after the forest", self.rd.b.len() - self.rd.p));
        }
        for id in &self.pattern_refs {
            if !self.nodes.contains_key(id) {
                return Err(format!("a paint's pattern source node {id} isn't resident"));
            }
        }
        Ok(())
    }

    // Generic Kiwi

    fn skip(&mut self, ty: Type, array: bool) -> R<()> {
        if array {
            let n = self.rd.uint()? as usize;
            if ty == Type::Byte {
                self.rd.take(n)?;
            } else {
                for _ in 0..n {
                    self.skip(ty, false)?;
                }
            }
            return Ok(());
        }
        match ty {
            Type::Bool | Type::Byte => {
                self.rd.take(1)?;
            }
            Type::Int | Type::Uint => {
                self.rd.uint()?;
            }
            Type::Float => {
                self.rd.float()?;
            }
            Type::String => {
                self.rd.string_bytes()?;
            }
            Type::Int64 | Type::Uint64 => {
                self.rd.uint64()?;
            }
            Type::Def(d) => {
                let s = self.s;
                let def = &s.defs[d as usize];
                match def.kind {
                    DefKind::Enum => {
                        self.rd.uint()?;
                    }
                    DefKind::Struct => {
                        for f in &def.fields {
                            self.skip(f.ty, f.array)?;
                        }
                    }
                    DefKind::Message => loop {
                        let at = self.rd.p;
                        let id = self.rd.uint()?;
                        if id == 0 {
                            break;
                        }
                        let f = def.field(id).ok_or_else(|| unknown_field(def, id, at))?;
                        self.skip(f.ty, f.array)?;
                    },
                }
            }
        }
        Ok(())
    }

    fn value(&mut self, ty: Type, array: bool) -> R<Value> {
        if array {
            let n = self.rd.uint()? as usize;
            if ty == Type::Byte {
                return Ok(Value::Bytes(self.rd.take(n)?.to_vec()));
            }
            let mut v = Vec::with_capacity(n.min(self.rd.remaining()));
            for _ in 0..n {
                v.push(self.value(ty, false)?);
            }
            return Ok(Value::Array(v));
        }
        Ok(match ty {
            Type::Bool => Value::Bool(self.rd.byte()? != 0),
            Type::Byte => Value::Byte(self.rd.byte()?),
            Type::Int => Value::Int(self.rd.int()?),
            Type::Uint => Value::Uint(self.rd.uint()?),
            Type::Float => Value::Float(self.rd.float()?),
            Type::String => Value::String(String::from_utf8_lossy(self.rd.string_bytes()?).into_owned()),
            Type::Int64 => Value::Int64(self.rd.int64()?),
            Type::Uint64 => Value::Uint64(self.rd.uint64()?),
            Type::Def(d) => {
                let s = self.s;
                let def = &s.defs[d as usize];
                match def.kind {
                    DefKind::Enum => {
                        let value = self.rd.uint()?;
                        Value::Enum { value, name: def.field(value).map(|f| f.name.clone()) }
                    }
                    DefKind::Struct => {
                        let mut fields = Vec::with_capacity(def.fields.len());
                        for f in &def.fields {
                            fields.push((f.name.clone(), self.value(f.ty, f.array)?));
                        }
                        Value::Record(fields)
                    }
                    DefKind::Message => {
                        let mut fields = Vec::new();
                        loop {
                            let at = self.rd.p;
                            let id = self.rd.uint()?;
                            if id == 0 {
                                break;
                            }
                            let f = def.field(id).ok_or_else(|| unknown_field(def, id, at))?;
                            fields.push((f.name.clone(), self.value(f.ty, f.array)?));
                        }
                        Value::Record(fields)
                    }
                }
            }
        })
    }

    /// Calls `f` for each field of a struct or message, with its binding
    #[inline]
    fn record(&mut self, plan: u32, mut f: impl FnMut(&mut Self, &'a Member) -> R<()>) -> R<()> {
        let s = self.s;
        let plan = &s.bind.plans[plan as usize];
        let def = &s.defs[plan.def as usize];
        if def.kind == DefKind::Message {
            loop {
                let at = self.rd.p;
                let id = self.rd.uint()?;
                if id == 0 {
                    return Ok(());
                }
                let i = def.index.get(id).ok_or_else(|| unknown_field(def, id, at))?;
                f(self, &plan.members[i])?;
            }
        }
        for m in &plan.members {
            f(self, m)?;
        }
        Ok(())
    }

    fn keep(&mut self, m: &Member, extra: &mut Extra) -> R<()> {
        let s = self.s;
        let name = s.defs[m.def as usize].fields[m.field as usize].name.clone();
        let v = self.value(m.ty, m.array)?;
        extra.push((name, v));
        Ok(())
    }

    fn array<T>(&mut self, mut f: impl FnMut(&mut Self) -> R<T>) -> R<Vec<T>> {
        let n = self.rd.uint()? as usize;
        let mut v = Vec::with_capacity(n.min(self.rd.remaining()));
        for _ in 0..n {
            v.push(f(self)?);
        }
        Ok(v)
    }

    // Numbers, converted from whatever numeric type the schema has

    fn u64_of(&mut self, ty: Type) -> R<u64> {
        Ok(match ty {
            Type::Uint => self.rd.uint()? as u64,
            Type::Uint64 => self.rd.uint64()?,
            Type::Int => self.rd.int()? as u64,
            Type::Int64 => self.rd.int64()? as u64,
            Type::Bool | Type::Byte => self.rd.byte()? as u64,
            Type::Float => self.rd.float()? as u64,
            _ => return Err(format!("{ty:?} isn't a number")),
        })
    }

    #[inline]
    fn u32_of(&mut self, ty: Type) -> R<u32> {
        match ty {
            Type::Uint => self.rd.uint(),
            _ => self.u64_of(ty).map(|v| v as u32),
        }
    }

    fn i32_of(&mut self, ty: Type) -> R<i32> {
        match ty {
            Type::Int => self.rd.int(),
            Type::Float => Ok(self.rd.float()? as i32),
            _ => self.u64_of(ty).map(|v| v as i32),
        }
    }

    #[inline]
    fn f32_of(&mut self, ty: Type) -> R<f32> {
        match ty {
            Type::Float => self.rd.float(),
            Type::Int => Ok(self.rd.int()? as f32),
            Type::Int64 => Ok(self.rd.int64()? as f32),
            _ => self.u64_of(ty).map(|v| v as f32),
        }
    }

    fn bool_of(&mut self, ty: Type) -> R<bool> {
        match ty {
            Type::Float => Ok(self.rd.float()? != 0.0),
            _ => self.u64_of(ty).map(|v| v != 0),
        }
    }

    fn en<E: KiwiEnum>(&mut self, m: &Member) -> R<E> {
        let v = self.rd.uint()?;
        Ok(enum_of(&self.s.bind.enums[m.sub as usize], v))
    }

    fn f32s(&mut self, ty: Type) -> R<Vec<f32>> {
        self.array(|fr| fr.f32_of(ty))
    }

    fn u32s(&mut self, ty: Type) -> R<Vec<u32>> {
        self.array(|fr| fr.u32_of(ty))
    }

    fn string(&mut self) -> R<String> {
        Ok(String::from_utf8_lossy(self.rd.string_bytes()?).into_owned())
    }

    /// A struct of numbers into an array, by member name, unset members
    /// keeping `out`'s values
    fn nums<T, const K: usize>(&mut self, plan: u32, mut out: [T; K], read: fn(&mut Self, Type) -> R<T>) -> R<[T; K]> {
        self.record(plan, |fr, m| match out.get_mut(m.slot as usize) {
            Some(slot) => {
                *slot = read(fr, m.ty)?;
                Ok(())
            }
            None => fr.skip(m.ty, m.array),
        })?;
        Ok(out)
    }

    fn affine(&mut self, m: &Member) -> R<Transform> {
        self.nums(m.sub, IDENTITY, Self::f32_of)
    }

    fn vector(&mut self, m: &Member) -> R<Vector> {
        self.nums(m.sub, [0.0; 2], Self::f32_of)
    }

    fn color(&mut self, m: &Member) -> R<Color> {
        let [r, g, b, a] = self.nums(m.sub, [0.0; 4], Self::f32_of)?;
        Ok(Color { r, g, b, a })
    }

    fn rect(&mut self, m: &Member) -> R<Rect> {
        let [x, y, w, h] = self.nums(m.sub, [0.0; 4], Self::f32_of)?;
        Ok(Rect { x, y, w, h })
    }

    fn guid(&mut self, m: &Member) -> R<Guid> {
        let [session_id, local_id] = self.nums(m.sub, [0; 2], Self::u32_of)?;
        Ok(Guid { session_id, local_id })
    }

    fn rounded_rect(&mut self, m: &Member) -> R<RoundedRectInfo> {
        let [top_left, top_right, bottom_left, bottom_right, corner_smoothing] = self.nums(m.sub, [0.0; 5], Self::f32_of)?;
        Ok(RoundedRectInfo { top_left, top_right, bottom_left, bottom_right, corner_smoothing })
    }

    fn color_adjust(&mut self, m: &Member) -> R<ColorAdjust> {
        let [tint, vibrance, shadows, highlights, exposure, temperature, contrast] = self.nums(m.sub, [0.0; 7], Self::f32_of)?;
        Ok(ColorAdjust { tint, vibrance, shadows, highlights, exposure, temperature, contrast })
    }

    fn path_ref(&self, i: u32) -> R<Arc<Path>> {
        self.paths.get(i as usize).cloned().ok_or_else(|| format!("path index {i} is out of range, the frame has {} paths", self.paths.len()))
    }

    fn paint_ref(&self, i: u32) -> R<Arc<Paint>> {
        self.paints.get(i as usize).cloned().ok_or_else(|| format!("paint index {i} isn't decoded yet, {} paints so far", self.paints.len()))
    }

    fn paths_of(&mut self, ty: Type) -> R<Vec<Arc<Path>>> {
        self.array(|fr| {
            let i = fr.u32_of(ty)?;
            fr.path_ref(i)
        })
    }

    fn paints_of(&mut self, ty: Type) -> R<Vec<Arc<Paint>>> {
        self.array(|fr| {
            let i = fr.u32_of(ty)?;
            fr.paint_ref(i)
        })
    }

    // The forest

    fn resources(&mut self, plan: u32) -> R<()> {
        // Nodes resolve path indices as they're decoded, so if resources
        // come before paths they're decoded once paths have been
        let mut paths_seen = false;
        let mut deferred: Option<(usize, &'a Member)> = None;
        self.record(plan, |fr, m| match ResourcesS::of(m) {
            Some(ResourcesS::Paths) => {
                fr.path_table(m.sub)?;
                paths_seen = true;
                Ok(())
            }
            Some(ResourcesS::Resources) if !paths_seen => {
                deferred = Some((fr.rd.p, m));
                fr.skip(m.ty, m.array)
            }
            Some(ResourcesS::Resources) => fr.resource_list(m.sub),
            None => fr.skip(m.ty, m.array),
        })?;
        if let Some((at, m)) = deferred {
            let end = self.rd.p;
            self.rd.p = at;
            self.resource_list(m.sub)?;
            self.rd.p = end;
        }
        Ok(())
    }

    fn path_table(&mut self, plan: u32) -> R<()> {
        let n = self.rd.uint()? as usize;
        self.paths.reserve(n.min(self.rd.remaining()));
        for _ in 0..n {
            let at = self.rd.p;
            let (path, has_commands) = self.path(plan)?;
            let stub = !has_commands && path.metadata.is_none() && path.fill_rule.is_none() && path.extra.is_empty();
            let path = match (stub, path.content_hash) {
                (true, Some(hash)) => {
                    self.stubs += 1;
                    self.cache.get(&hash).cloned().ok_or_else(|| format!("path {hash:016x} at byte {at} was sent as its hash only and isn't cached"))?
                }
                (_, hash) => {
                    let path = Arc::new(path);
                    if let Some(hash) = hash {
                        self.cache.insert(hash, path.clone());
                    }
                    path
                }
            };
            self.paths.push(path);
        }
        Ok(())
    }

    fn resource_list(&mut self, plan: u32) -> R<()> {
        let n = self.rd.uint()?;
        for _ in 0..n {
            self.record(plan, |fr, m| match ResourceS::of(m) {
                Some(ResourceS::Paint) => {
                    let paint = fr.paint(m.sub)?;
                    if let Some(id) = paint.pattern_source_node_id {
                        fr.pattern_refs.push(id);
                    }
                    fr.paints.push(Arc::new(paint));
                    Ok(())
                }
                Some(ResourceS::Node) => fr.node_resource(m.sub),
                None => fr.skip(m.ty, m.array),
            })?;
        }
        Ok(())
    }

    fn node_resource(&mut self, plan: u32) -> R<()> {
        let at = self.rd.p;
        let (node, external) = self.node(plan)?;
        let id = node.id;
        if external {
            if !self.nodes.contains_key(&id) {
                return Err(format!("node {id} at byte {at} refers to a resident node that isn't there"));
            }
            self.external += 1;
        } else {
            if self.nodes.contains_key(&id) {
                return Err(format!("node {id} at byte {at} reuses the id of a resident node"));
            }
            if let Some(r) = node.refs().find(|r| !self.nodes.contains_key(r)) {
                return Err(format!("node {id} at byte {at} refers to node {r}, which isn't resident or earlier in the frame"));
            }
            self.nodes.insert(id, Arc::new(node));
            self.new_nodes.push(id);
        }
        self.root = Some(id);
        Ok(())
    }

    fn path(&mut self, plan: u32) -> R<(Path, bool)> {
        let mut p = Path::default();
        let mut has_commands = false;
        self.record(plan, |fr, m| {
            match PathS::of(m) {
                Some(PathS::WindingRule) => p.fill_rule = Some(fr.en(m)?),
                Some(PathS::Commands) => {
                    let n = fr.rd.uint()? as usize;
                    let at = fr.rd.p;
                    let bytes = fr.rd.take(n)?;
                    parse_commands(bytes, &mut p.verbs, &mut p.points).map_err(|e| format!("{e}, at byte {at}"))?;
                    has_commands = true;
                }
                Some(PathS::Metadata) => p.metadata = Some(fr.path_metadata(m.sub)?),
                Some(PathS::ContentHash) => p.content_hash = Some(fr.u64_of(m.ty)?),
                None => fr.keep(m, &mut p.extra)?,
            }
            Ok(())
        })?;
        Ok((p, has_commands))
    }

    fn path_metadata(&mut self, plan: u32) -> R<PathMetadata> {
        let mut out = PathMetadata::default();
        self.record(plan, |fr, m| {
            match PathMetadataS::of(m) {
                Some(PathMetadataS::Type) => out.kind = Some(fr.en(m)?),
                Some(PathMetadataS::Values) => out.values = fr.f32s(m.ty)?,
                Some(PathMetadataS::Bounds) => out.bounds = Some(fr.rect(m)?),
                None => fr.keep(m, &mut out.extra)?,
            }
            Ok(())
        })?;
        Ok(out)
    }

    fn paint(&mut self, plan: u32) -> R<Paint> {
        let mut p = Paint::default();
        self.record(plan, |fr, m| {
            use PaintS as S;
            let Some(slot) = S::of(m) else { return fr.keep(m, &mut p.extra) };
            match slot {
                S::Type => p.kind = Some(fr.en(m)?),
                S::BlendMode => p.blend_mode = Some(fr.en(m)?),
                S::Opacity => p.opacity = Some(fr.f32_of(m.ty)?),
                S::Color => p.color = Some(fr.color(m)?),
                S::Transform => p.transform = Some(fr.affine(m)?),
                S::Gradient => p.gradient = Some(fr.gradient(m.sub)?),
                S::Thumbnail => p.thumbnail = Some(fr.image(m.sub)?),
                S::Image => p.image = Some(fr.image(m.sub)?),
                S::FilterColorAdjust => p.filter_color_adjust = Some(fr.color_adjust(m)?),
                S::ImageScaleMode => p.image_scale_mode = Some(fr.en(m)?),
                S::ImageRotationDeg => p.image_rotation_deg = Some(fr.f32_of(m.ty)?),
                S::ImageScalingFactor => p.image_scaling_factor = Some(fr.f32_of(m.ty)?),
                S::OriginalImageWidth => p.original_image_width = Some(fr.i32_of(m.ty)?),
                S::OriginalImageHeight => p.original_image_height = Some(fr.i32_of(m.ty)?),
                S::ColorspaceConversion => p.colorspace_conversion = Some(fr.en(m)?),
                S::EmojiId => p.emoji_id = Some(fr.emoji_id(m.sub)?),
                S::PatternSourceNodeId => p.pattern_source_node_id = Some(fr.u32_of(m.ty)?),
                S::PatternTileType => p.pattern_tile_type = Some(fr.en(m)?),
                S::PatternHorizontalAlignment => p.pattern_horizontal_alignment = Some(fr.en(m)?),
                S::PatternVerticalAlignment => p.pattern_vertical_alignment = Some(fr.en(m)?),
                S::PatternScale => p.pattern_scale = Some(fr.f32_of(m.ty)?),
                S::PatternSpacing => p.pattern_spacing = Some(fr.vector(m)?),
                S::PatternSourceAbsoluteBounds => p.pattern_source_absolute_bounds = Some(fr.rect(m)?),
                S::CustomEffectTexture => p.custom_effect_texture = Some(fr.effect_texture(m.sub)?),
                S::VideoNodeId => p.video_node_id = Some(fr.guid(m)?),
                S::InterpolationColorSpace => p.interpolation_color_space = Some(fr.en(m)?),
                S::InterpolationHueMethod => p.interpolation_hue_method = Some(fr.en(m)?),
                S::GradientColorProfile => p.gradient_color_profile = Some(fr.en(m)?),
                S::MediaGeneration => p.media_generation = Some(fr.u64_of(m.ty)?),
                S::VideoFillPaintId => p.video_fill_paint_id = Some(fr.guid(m)?),
            }
            Ok(())
        })?;
        Ok(p)
    }

    fn gradient(&mut self, plan: u32) -> R<Gradient> {
        let mut g = Gradient::default();
        self.record(plan, |fr, m| {
            match GradientS::of(m) {
                Some(GradientS::Type) => g.kind = Some(fr.en(m)?),
                Some(GradientS::Stops) => g.stops = fr.array(|fr| fr.color_stop(m.sub))?,
                None => fr.keep(m, &mut g.extra)?,
            }
            Ok(())
        })?;
        Ok(g)
    }

    fn color_stop(&mut self, plan: u32) -> R<ColorStop> {
        let mut stop = ColorStop::default();
        self.record(plan, |fr, m| {
            match ColorStopS::of(m) {
                Some(ColorStopS::Color) => stop.color = fr.color(m)?,
                Some(ColorStopS::Position) => stop.position = fr.f32_of(m.ty)?,
                None => fr.skip(m.ty, m.array)?,
            }
            Ok(())
        })?;
        Ok(stop)
    }

    fn image(&mut self, plan: u32) -> R<Image> {
        let mut image = Image::default();
        self.record(plan, |fr, m| {
            match ImageS::of(m) {
                Some(ImageS::Hash) => {
                    let n = fr.rd.uint()? as usize;
                    image.hash = fr.rd.take(n)?.to_vec();
                }
                None => fr.keep(m, &mut image.extra)?,
            }
            Ok(())
        })?;
        Ok(image)
    }

    fn emoji_id(&mut self, plan: u32) -> R<EmojiId> {
        let mut e = EmojiId::default();
        self.record(plan, |fr, m| {
            match EmojiIdS::of(m) {
                Some(EmojiIdS::CodePoints) => e.code_points = fr.u32s(m.ty)?,
                Some(EmojiIdS::ImageSet) => e.image_set = Some(fr.en(m)?),
                None => fr.keep(m, &mut e.extra)?,
            }
            Ok(())
        })?;
        Ok(e)
    }

    fn effect_texture(&mut self, plan: u32) -> R<EffectTexture> {
        let mut t = EffectTexture::default();
        self.record(plan, |fr, m| {
            match EffectTextureS::of(m) {
                Some(EffectTextureS::InstanceId) => t.instance_id = Some(fr.u32_of(m.ty)?),
                Some(EffectTextureS::IntraLayerChain) => t.intra_layer_chain = fr.array(|fr| fr.chain_entry(m.sub))?,
                Some(EffectTextureS::DispatchRevision) => t.dispatch_revision = Some(fr.u64_of(m.ty)?),
                Some(EffectTextureS::NodeGuid) => t.node_guid = Some(fr.guid(m)?),
                Some(EffectTextureS::InitialTextureFallbackInstanceId) => {
                    t.initial_texture_fallback_instance_id = Some(fr.u32_of(m.ty)?)
                }
                None => fr.keep(m, &mut t.extra)?,
            }
            Ok(())
        })?;
        Ok(t)
    }

    fn chain_entry(&mut self, plan: u32) -> R<ChainEntry> {
        let mut e = ChainEntry::default();
        self.record(plan, |fr, m| {
            match ChainEntryS::of(m) {
                Some(ChainEntryS::CodeComponentId) => e.code_component_id = Some(fr.string()?),
                Some(ChainEntryS::ParamsJson) => e.params_json = Some(fr.string()?),
                Some(ChainEntryS::OccurrenceId) => e.occurrence_id = Some(fr.string()?),
                None => fr.keep(m, &mut e.extra)?,
            }
            Ok(())
        })?;
        Ok(e)
    }

    fn glyph(&mut self, plan: u32) -> R<Glyph> {
        let mut g = Glyph::default();
        self.record(plan, |fr, m| {
            match GlyphS::of(m) {
                Some(GlyphS::PathIndex) => {
                    let i = fr.u32_of(m.ty)?;
                    g.path = Some(fr.path_ref(i)?);
                }
                Some(GlyphS::Position) => g.position = Some(fr.vector(m)?),
                Some(GlyphS::FontSize) => g.font_size = Some(fr.f32_of(m.ty)?),
                Some(GlyphS::Rotation) => g.rotation = Some(fr.f32_of(m.ty)?),
                Some(GlyphS::StyleId) => g.style_id = Some(fr.i32_of(m.ty)?),
                Some(GlyphS::EmojiPaintIndices) => g.emoji_paints = fr.paints_of(m.ty)?,
                None => fr.keep(m, &mut g.extra)?,
            }
            Ok(())
        })?;
        Ok(g)
    }

    fn glyph_metadata(&mut self, plan: u32) -> R<GlyphMetadata> {
        let mut g = GlyphMetadata::default();
        self.record(plan, |fr, m| {
            match GlyphMetadataS::of(m) {
                Some(GlyphMetadataS::Advance) => g.advance = Some(fr.f32_of(m.ty)?),
                Some(GlyphMetadataS::Rotation) => g.rotation = Some(fr.f32_of(m.ty)?),
                None => fr.keep(m, &mut g.extra)?,
            }
            Ok(())
        })?;
        Ok(g)
    }

    fn glyph_batch(&mut self, plan: u32) -> R<GlyphBatch> {
        let mut b = GlyphBatch::default();
        self.record(plan, |fr, m| {
            match GlyphBatchS::of(m) {
                Some(GlyphBatchS::Glyphs) => b.glyphs = fr.array(|fr| fr.glyph(m.sub))?,
                Some(GlyphBatchS::DecorationGeometryPathIndices) => b.decoration_paths = fr.paths_of(m.ty)?,
                Some(GlyphBatchS::ShouldRoundY) => b.should_round_y = fr.bool_of(m.ty)?,
                None => fr.keep(m, &mut b.extra)?,
            }
            Ok(())
        })?;
        Ok(b)
    }

    fn geometry(&mut self, plan: u32) -> R<Geometry> {
        let mut g = Geometry::default();
        self.record(plan, |fr, m| {
            match GeometryS::of(m) {
                Some(GeometryS::PathIndex) => {
                    let i = fr.u32_of(m.ty)?;
                    g.path = Some(fr.path_ref(i)?);
                }
                Some(GeometryS::PaintIndices) => g.paints = fr.paints_of(m.ty)?,
                Some(GeometryS::GlyphBatch) => g.glyph_batch = Some(Box::new(fr.glyph_batch(m.sub)?)),
                Some(GeometryS::GlyphMetadata) => g.glyph_metadata = fr.array(|fr| fr.glyph_metadata(m.sub))?,
                None => fr.keep(m, &mut g.extra)?,
            }
            Ok(())
        })?;
        Ok(g)
    }

    fn glyph_geometry(&mut self, plan: u32) -> R<GlyphGeometry> {
        let mut g = GlyphGeometry::default();
        self.record(plan, |fr, m| {
            match GlyphGeometryS::of(m) {
                Some(GlyphGeometryS::Glyphs) => g.glyphs = fr.array(|fr| fr.glyph(m.sub))?,
                Some(GlyphGeometryS::PaintIndices) => g.paints = fr.paints_of(m.ty)?,
                None => fr.keep(m, &mut g.extra)?,
            }
            Ok(())
        })?;
        Ok(g)
    }

    fn stroke_recipe(&mut self, plan: u32) -> R<StrokeRecipe> {
        let mut r = StrokeRecipe::default();
        self.record(plan, |fr, m| {
            use StrokeRecipeS as S;
            match S::of(m) {
                Some(S::Tags) => {
                    let n = fr.rd.uint()? as usize;
                    r.tags = fr.rd.take(n)?.to_vec();
                }
                Some(S::Data) => r.data = fr.f32s(m.ty)?,
                Some(S::HalfWidth) => r.half_width = Some(fr.f32_of(m.ty)?),
                Some(S::MiterLimit) => r.miter_limit = Some(fr.f32_of(m.ty)?),
                Some(S::CapExtent) => r.cap_extent = Some(fr.f32_of(m.ty)?),
                Some(S::CapBreadth) => r.cap_breadth = Some(fr.f32_of(m.ty)?),
                Some(S::Cap) => r.cap = Some(fr.u32_of(m.ty)? as u8),
                Some(S::Join) => r.join = Some(fr.u32_of(m.ty)? as u8),
                None => fr.keep(m, &mut r.extra)?,
            }
            Ok(())
        })?;
        Ok(r)
    }

    fn glass_corner(&mut self, plan: u32) -> R<GlassCorner> {
        let mut g = GlassCorner::default();
        self.record(plan, |fr, m| {
            match GlassCornerS::of(m) {
                Some(GlassCornerS::RectSize) => g.rect_size = fr.vector(m)?,
                Some(GlassCornerS::CornerRadii) => g.corner_radii = fr.f32s(m.ty)?,
                Some(GlassCornerS::CornerSmoothings) => g.corner_smoothings = fr.f32s(m.ty)?,
                Some(GlassCornerS::CornerRadiiIndependent) => g.corner_radii_independent = fr.bool_of(m.ty)?,
                None => fr.keep(m, &mut g.extra)?,
            }
            Ok(())
        })?;
        Ok(g)
    }

    fn node(&mut self, plan: u32) -> R<(Node, bool)> {
        let mut n = Node::default();
        let mut op: Option<Op> = None;
        let mut id = None;
        let mut external = false;
        self.record(plan, |fr, m| {
            use NodeS as S;
            let Some(slot) = S::of(m) else { return fr.keep(m, &mut n.extra) };
            match slot {
                S::Op => op = Some(fr.en(m)?),
                S::ChildIndices => n.children = fr.u32s(m.ty)?,
                S::ChildIndex => n.child = Some(fr.u32_of(m.ty)?),
                S::Radius => n.radius = Some(fr.f32_of(m.ty)?),
                S::Clamp => n.ext_mut().clamp = Some(fr.rect(m)?),
                S::BlendMode => n.blend_mode = Some(fr.en(m)?),
                S::Opacity => n.opacity = Some(fr.f32_of(m.ty)?),
                S::Offset => n.ext_mut().offset = Some(fr.vector(m)?),
                S::Transform => n.transform = Some(fr.affine(m)?),
                S::PaintSize => n.paint_size = Some(fr.vector(m)?),
                S::Geometry => n.geometry = Some(fr.geometry(m.sub)?),
                S::RoundedRectInfo => n.ext_mut().rounded_rect_info = Some(fr.rounded_rect(m)?),
                S::BgBlurExpansionForFrameFill => n.bg_blur_expansion_for_frame_fill = Some(fr.f32_of(m.ty)?),
                S::IsEllipse => n.ext_mut().is_ellipse = fr.bool_of(m.ty)?,
                S::IsCreatedFromBrush => n.is_created_from_brush = fr.bool_of(m.ty)?,
                S::FillGeometries => n.fill_geometries = fr.array(|fr| fr.geometry(m.sub))?,
                S::StrokeGeometries => n.stroke_geometries = fr.array(|fr| fr.geometry(m.sub))?,
                S::StrokeMode => n.stroke_mode = Some(fr.en(m)?),
                S::PaintedGlyphGeometry => n.ext_mut().painted_glyph_geometry = Some(fr.glyph_geometry(m.sub)?),
                S::AlphaNodeIndex => n.alpha_node = Some(fr.u32_of(m.ty)?),
                S::ColorNodeIndex => n.color_node = Some(fr.u32_of(m.ty)?),
                S::MaskOpType => n.mask_op = Some(fr.en(m)?),
                S::InnerShadowFill => n.ext_mut().inner_shadow_fill = Some(fr.color(m)?),
                S::InnerShadowAlphaRequiresHardMask => n.ext_mut().inner_shadow_alpha_requires_hard_mask = fr.bool_of(m.ty)?,
                S::Color => n.color = Some(fr.color(m)?),
                S::BlurOpType => n.ext_mut().blur_op_type = Some(fr.en(m)?),
                S::StartOffset => n.ext_mut().start_offset = Some(fr.vector(m)?),
                S::EndOffset => n.ext_mut().end_offset = Some(fr.vector(m)?),
                S::StartRadius => n.ext_mut().start_radius = Some(fr.f32_of(m.ty)?),
                S::InstanceTransforms => n.ext_mut().instance_transforms = fr.array(|fr| fr.affine(m))?,
                S::NoiseSize => n.ext_mut().noise_size = Some(fr.vector(m)?),
                S::ClipToShape => n.ext_mut().clip_to_shape = fr.bool_of(m.ty)?,
                S::CenterPoint => n.ext_mut().center_point = Some(fr.vector(m)?),
                S::Seed => n.ext_mut().seed = Some(fr.u32_of(m.ty)?),
                S::NoiseType => n.ext_mut().noise_type = Some(fr.en(m)?),
                S::Density => n.ext_mut().density = Some(fr.f32_of(m.ty)?),
                S::NoiseColor => n.ext_mut().noise_color = Some(fr.color(m)?),
                S::SecondaryNoiseColor => n.ext_mut().secondary_noise_color = Some(fr.color(m)?),
                S::BlurRadius => n.ext_mut().blur_radius = Some(fr.f32_of(m.ty)?),
                S::Splay => n.ext_mut().splay = Some(fr.f32_of(m.ty)?),
                S::RefractionIntensity => n.ext_mut().refraction_intensity = Some(fr.f32_of(m.ty)?),
                S::RefractionDistance => n.ext_mut().refraction_distance = Some(fr.f32_of(m.ty)?),
                S::TintColor => n.ext_mut().tint_color = Some(fr.color(m)?),
                S::LightIntensity => n.ext_mut().light_intensity = Some(fr.f32_of(m.ty)?),
                S::LightAngle => n.ext_mut().light_angle = Some(fr.f32_of(m.ty)?),
                S::BevelSize => n.ext_mut().bevel_size = Some(fr.f32_of(m.ty)?),
                S::ChromaticAberration => n.ext_mut().chromatic_aberration = Some(fr.f32_of(m.ty)?),
                S::GlassRectCornerInfo => n.ext_mut().glass_rect_corner_info = Some(fr.glass_corner(m.sub)?),
                S::RectCenterPoint => n.ext_mut().rect_center_point = Some(fr.vector(m)?),
                S::NodeToCanvasSpaceTransform => n.ext_mut().node_to_canvas_space_transform = Some(fr.affine(m)?),
                S::FillIndex => n.ext_mut().fill_index = Some(fr.u32_of(m.ty)?),
                S::MetadataType => n.ext_mut().metadata_type = Some(fr.en(m)?),
                S::ExpandedBounds => n.ext_mut().expanded_bounds = Some(fr.rect(m)?),
                S::PatternSourceNodeId => n.ext_mut().pattern_source_node_id = Some(fr.u32_of(m.ty)?),
                S::CustomEffectTextures => n.ext_mut().custom_effect_textures = fr.array(|fr| fr.effect_texture(m.sub))?,
                S::CustomEffectRenderTargetLocalBounds => {
                    n.ext_mut().custom_effect_render_target_local_bounds = Some(fr.rect(m)?)
                }
                S::NodeId => id = Some(fr.u32_of(m.ty)?),
                S::IsExternalRef => external = fr.bool_of(m.ty)?,
                S::UsesCompositedBackdrop => n.ext_mut().uses_composited_backdrop = fr.bool_of(m.ty)?,
                S::CompositeTileGridTransformHandling => {
                    n.ext_mut().composite_tile_grid_transform_handling = Some(fr.en(m)?)
                }
                S::CompositeTileGridEnableSpatialReuseGate => {
                    n.ext_mut().composite_tile_grid_enable_spatial_reuse_gate = fr.bool_of(m.ty)?
                }
                S::CompositeTileGridExpandBounds => n.ext_mut().composite_tile_grid_expand_bounds = fr.bool_of(m.ty)?,
                S::GpuStrokeRecipe => n.ext_mut().gpu_stroke_recipe = Some(fr.stroke_recipe(m.sub)?),
                S::SpatialPlaneContentId => n.ext_mut().spatial_plane_content_id = Some(fr.u32_of(m.ty)?),
                S::SpatialPlaneResetsFacing => n.ext_mut().spatial_plane_resets_facing = fr.bool_of(m.ty)?,
                S::SpatialPlaneBlendMode => n.ext_mut().spatial_plane_blend_mode = Some(fr.en(m)?),
                S::SpatialPlaneUsesParentDensity => n.ext_mut().spatial_plane_uses_parent_density = fr.bool_of(m.ty)?,
                S::SpatialTransformChildId => n.ext_mut().spatial_transform_child_id = Some(fr.u32_of(m.ty)?),
                S::SpatialTransform => n.ext_mut().spatial_transform = fr.f32s(m.ty)?,
                S::SpatialCompositionChildIds => n.ext_mut().spatial_composition_child_ids = fr.u32s(m.ty)?,
                S::VolatileMaxTransformScale => n.ext_mut().volatile_max_transform_scale = Some(fr.f32_of(m.ty)?),
                S::LocalBackdropIndex => n.ext_mut().local_backdrop = Some(fr.u32_of(m.ty)?),
            }
            Ok(())
        })?;
        n.id = id.ok_or("a node has no nodeId")?;
        n.op = match op {
            Some(op) => op,
            // Kiwi's default, wire value 0
            None => {
                let s = self.s;
                s.bind.plans[plan as usize]
                    .members
                    .iter()
                    .find(|m| NodeS::of(m) == Some(NodeS::Op))
                    .map_or(Op::Unknown(0), |m| enum_of(&s.bind.enums[m.sub as usize], 0))
            }
        };
        Ok((n, external))
    }
}

fn enum_of<E: KiwiEnum>(table: &[u16], v: u32) -> E {
    match table.get(v as usize) {
        Some(&i) if i != NO => E::from_index(i as usize),
        _ => E::unknown(v),
    }
}

#[cold]
fn unknown_field(def: &Def, id: u32, at: usize) -> String {
    format!("{} has no field {id}, at byte {at}: the schema doesn't match the data", def.name)
}

// ---------------------------------------------------------------- schema from Figma's wasm

/// The `render.bkiwi` asset of an Emscripten build of Figma, found by name
/// in its asset table (`{namePtr, dataPtr, size}`, u32 little-endian) in the
/// wasm's data segments
pub fn schema_from_wasm(wasm: &[u8]) -> Option<Vec<u8>> {
    let memory = Memory(wasm_data(wasm)?);
    const NAME: &[u8] = b"render.bkiwi\0";
    for &(addr, data) in &memory.0 {
        let mut from = 0;
        while let Some(i) = find(&data[from..], NAME) {
            let pos = from + i;
            from = pos + 1;
            if pos > 0 && data[pos - 1] != 0 {
                continue;
            }
            let Ok(name_ptr) = u32::try_from(addr + pos as u64) else { continue };
            if let Some(schema) = memory.asset(name_ptr) {
                return Some(schema);
            }
        }
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let first = needle[0];
    let mut at = 0;
    while let Some(i) = hay[at..].iter().position(|&b| b == first) {
        let start = at + i;
        if hay[start..].starts_with(needle) {
            return Some(start);
        }
        at = start + 1;
    }
    None
}

/// Active data segments: memory address and bytes
struct Memory<'a>(Vec<(u64, &'a [u8])>);

impl Memory<'_> {
    fn asset(&self, name_ptr: u32) -> Option<Vec<u8>> {
        let needle = name_ptr.to_le_bytes();
        for &(addr, data) in &self.0 {
            let mut off = ((4 - addr % 4) % 4) as usize;
            while off + 4 <= data.len() {
                if data[off..off + 4] == needle {
                    if let Some(entry) = self.read(addr + off as u64, 12) {
                        let data_ptr = u32::from_le_bytes(entry[4..8].try_into().unwrap());
                        let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
                        if size > 0 && size <= 64 << 20 {
                            if let Some(bytes) = self.read(data_ptr as u64, size) {
                                if parse_defs(&bytes).is_ok() {
                                    return Some(bytes);
                                }
                            }
                        }
                    }
                }
                off += 4;
            }
        }
        None
    }

    /// Memory as initialized, zeros between segments. None unless a segment
    /// holds the first byte
    fn read(&self, addr: u64, len: usize) -> Option<Vec<u8>> {
        let end = addr + len as u64;
        let mut out = vec![0; len];
        let mut starts = false;
        for &(a, d) in &self.0 {
            let (s, e) = (a.max(addr), (a + d.len() as u64).min(end));
            if s < e {
                out[(s - addr) as usize..(e - addr) as usize].copy_from_slice(&d[(s - a) as usize..(e - a) as usize]);
                starts |= s == addr;
            }
        }
        starts.then_some(out)
    }
}

fn wasm_data(w: &[u8]) -> Option<Vec<(u64, &[u8])>> {
    if w.get(0..4)? != b"\0asm" {
        return None;
    }
    let mut p = 8;
    let mut out = Vec::new();
    while p < w.len() {
        let id = w[p];
        p += 1;
        let size = leb(w, &mut p)? as usize;
        let end = p.checked_add(size)?;
        let section = w.get(p..end)?;
        if id == 11 {
            data_section(section, &mut out)?;
        }
        p = end;
    }
    Some(out)
}

fn data_section<'a>(s: &'a [u8], out: &mut Vec<(u64, &'a [u8])>) -> Option<()> {
    let mut p = 0;
    let count = leb(s, &mut p)?;
    for _ in 0..count {
        let addr = match leb(s, &mut p)? {
            0 => offset_expr(s, &mut p)?,
            1 => None,
            2 => {
                leb(s, &mut p)?;
                offset_expr(s, &mut p)?
            }
            _ => return None,
        };
        let size = leb(s, &mut p)? as usize;
        let bytes = s.get(p..p.checked_add(size)?)?;
        p += size;
        if let Some(addr) = addr {
            out.push((addr, bytes));
        }
    }
    Some(())
}

/// An active segment's offset: Some(None) when it isn't a constant
fn offset_expr(s: &[u8], p: &mut usize) -> Option<Option<u64>> {
    let op = *s.get(*p)?;
    *p += 1;
    let addr = match op {
        0x41 => Some(sleb(s, p)? as u32 as u64),
        0x42 => Some(sleb(s, p)? as u64),
        0x23 => {
            leb(s, p)?;
            None
        }
        _ => return None,
    };
    (*s.get(*p)? == 0x0b).then_some(())?;
    *p += 1;
    Some(addr)
}

fn leb(s: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *s.get(*p)?;
        *p += 1;
        v |= ((b & 127) as u64) << shift;
        if b & 128 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn sleb(s: &[u8], p: &mut usize) -> Option<i64> {
    let mut v = 0i64;
    let mut shift = 0;
    loop {
        let b = *s.get(*p)?;
        *p += 1;
        v |= ((b & 127) as i64) << shift;
        shift += 7;
        if b & 128 == 0 {
            if shift < 64 && b & 64 != 0 {
                v |= -1i64 << shift;
            }
            return Some(v);
        }
        if shift > 63 {
            return None;
        }
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const SCRATCH: &str =
        r"target\native-captures";

    fn capture(rel: &str) -> Option<Vec<u8>> {
        let path = format!(r"{SCRATCH}\{rel}");
        let bytes = std::fs::read(&path).ok();
        if bytes.is_none() {
            eprintln!("skipping, {path} is missing");
        }
        bytes
    }

    fn json(rel: &str) -> Option<serde_json::Value> {
        serde_json::from_slice(&capture(rel)?).ok()
    }

    fn ids(v: &serde_json::Value) -> Vec<u32> {
        v.as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect()).unwrap_or_default()
    }

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    /// Absolute transforms of every visit of each node from the root, and
    /// whether every reference resolved
    fn walk(forest: &Forest, root: u32) -> (HashMap<u32, Vec<Transform>>, usize, usize) {
        let mut abs: HashMap<u32, Vec<Transform>> = HashMap::new();
        let mut seen = std::collections::HashSet::new();
        let mut missing = 0;
        let mut stack = vec![(root, IDENTITY)];
        while let Some((id, t)) = stack.pop() {
            let Some(n) = forest.get(id) else {
                missing += 1;
                continue;
            };
            let here = match (n.op, n.transform) {
                (Op::StaticTransformBinding, Some(own)) => concat(&t, &own),
                _ => t,
            };
            let drawn = match (n.op, n.transform) {
                (Op::Fill | Op::FillStroke, Some(own)) => concat(&t, &own),
                _ => t,
            };
            let visits = abs.entry(id).or_default();
            if visits.len() < 8 {
                visits.push(drawn);
            }
            let first = seen.insert(id);
            if first || visits.len() < 8 {
                for r in n.refs() {
                    stack.push((r, here));
                }
            }
        }
        (abs, seen.len(), missing)
    }

    fn reachable(forest: &Forest, root: u32) -> (usize, usize) {
        let mut seen = std::collections::HashSet::new();
        let mut missing = 0;
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match forest.get(id) {
                Some(n) => stack.extend(n.refs()),
                None => missing += 1,
            }
        }
        (seen.len() - missing, missing)
    }

    #[test]
    fn kiwi_primitives() {
        let mut rd = Reader::new(&[0x7f, 0, 0, 0, 0, 0x80, 0, 0, 0]);
        assert_eq!(rd.float().unwrap(), 1.0);
        assert_eq!(rd.float().unwrap(), 0.0);
        assert_eq!(rd.float().unwrap(), 2.0);
        // 9 bytes: 8 LEB128 groups, then a full byte
        let mut rd = Reader::new(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(rd.uint64().unwrap(), u64::MAX);
        let mut rd = Reader::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert_eq!(rd.uint64().unwrap(), 1 << 56);
        let mut rd = Reader::new(&[3, 4, 0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert_eq!(rd.int().unwrap(), -2);
        assert_eq!(rd.int().unwrap(), 2);
        assert_eq!(rd.uint().unwrap(), u32::MAX);
        assert_eq!(rd.p, 7);
    }

    #[test]
    fn bundled_schema_is_fully_bound() {
        let s = Schema::bundled();
        assert_eq!(s.def("RenderNode").unwrap().fields.len(), 72);
        assert_eq!(s.def("Paint").unwrap().fields.len(), 30);
        // Every field the forest can carry has a typed home
        assert_eq!(s.unbound_fields(), Vec::<String>::new());
        assert_eq!(Schema::parse(include_bytes!("render.bkiwi")).unwrap().defs.len(), s.defs.len());
    }

    #[test]
    fn schema_from_figma_wasm() {
        let Some(wasm) = capture(r"wasm\fig.wasm") else { return };
        let t = Instant::now();
        let schema = schema_from_wasm(&wasm).expect("fig.wasm has render.bkiwi");
        eprintln!("schema_from_wasm(fig.wasm): {} bytes in {:?}", schema.len(), t.elapsed());
        assert_eq!(&schema[..], &include_bytes!("render.bkiwi")[..]);
        Schema::parse(&schema).unwrap();
        if let Some(worker) = capture(r"rww\compiled_wasm.wasm") {
            match schema_from_wasm(&worker) {
                Some(s) => eprintln!("render worker wasm: {} bytes, same as fig.wasm's: {}", s.len(), s == schema),
                None => eprintln!("render worker wasm: no render.bkiwi"),
            }
        }
        assert_eq!(schema_from_wasm(b"\0asm\x01\0\0\0"), None);
        assert_eq!(schema_from_wasm(b"not wasm"), None);
    }

    #[test]
    fn full_scene() {
        let Some(bytes) = capture(r"rt\f0_forest.bin") else { return };
        let schema = Schema::bundled();
        let mut forest = Forest::new();
        let t = Instant::now();
        let applied = forest.apply(&bytes, 0, &[], schema).unwrap();
        let took = t.elapsed();
        eprintln!("f0: {} bytes in {took:?}", bytes.len());
        assert_eq!(applied.consumed, bytes.len());
        assert_eq!((applied.paths, applied.paints, applied.new_nodes.len()), (3780, 1124, 23693));
        assert_eq!((applied.path_stubs, applied.external_refs), (0, 0));
        assert_eq!(applied.root, Some(64479));
        assert_eq!(forest.paths.len(), 3780);
        assert_eq!(reachable(&forest, 64479), (23693, 0));

        let t = Instant::now();
        let mut again = Forest::new();
        again.apply(&bytes, 0, &[], schema).unwrap();
        eprintln!("f0 again: {:?}", t.elapsed());

        let (abs, _, missing) = walk(&forest, 64479);
        assert_eq!(missing, 0);
        let at = |id: u32, want: Transform, tol: f32| {
            let got = abs.get(&id).unwrap_or_else(|| panic!("node {id} isn't reachable"));
            assert!(
                got.iter().any(|t| t.iter().zip(want).all(|(a, b)| close(*a, b, tol))),
                "node {id}: {got:?}, want {want:?}"
            );
        };
        let node = |id: u32| forest.get(id).unwrap_or_else(|| panic!("node {id} is missing")).clone();
        let first_paint = |id: u32| node(id).geometry.as_ref().unwrap().paints[0].clone();

        // Solid rectangle (validate.md, 5751:5769)
        at(1, [1.0, 0.0, 7332.0, 0.0, 1.0, 12762.0], 0.01);
        let n = node(1);
        assert_eq!(n.op, Op::Fill);
        assert_eq!(n.paint_size, Some([3370.0, 801.0]));
        let p = first_paint(1);
        assert_eq!(p.kind, Some(PaintType::Solid));
        let c = p.color.unwrap();
        assert!(close(c.r, 0.851, 0.001) && close(c.g, 0.851, 0.001) && close(c.a, 1.0, 0.0));
        let path = n.geometry.as_ref().unwrap().path.clone().unwrap();
        let meta = path.metadata.as_ref().unwrap();
        assert_eq!(meta.shape(), Shape::Rect(Rect { x: 0.0, y: 0.0, w: 3370.0, h: 801.0 }));
        let sk = path.to_skia();
        assert_eq!(sk.bounds(), &skia_safe::Rect::from_xywh(0.0, 0.0, 3370.0, 801.0));

        // Rotated, with a linear gradient (143:9)
        at(220, [0.975, -0.222, 3957.531, 0.222, 0.975, 600.968], 0.002);
        let p = first_paint(220);
        assert_eq!(p.kind, Some(PaintType::GradientLinear));
        let g = p.gradient.as_ref().unwrap();
        let want = [((0.063, 0.09, 0.176, 0.0), 0.0), ((0.063, 0.09, 0.176, 0.058), 0.317), ((0.062, 0.089, 0.178, 1.0), 1.0)];
        assert_eq!(g.stops.len(), want.len());
        for (s, ((r, gr, b, a), pos)) in g.stops.iter().zip(want) {
            let c = s.color;
            assert!(close(c.r, r, 0.001) && close(c.g, gr, 0.001) && close(c.b, b, 0.001) && close(c.a, a, 0.001), "{s:?}");
            assert!(close(s.position, pos, 0.001));
        }
        let gt = p.transform.unwrap();
        for (a, b) in gt.iter().zip([0.0, 1.031, -0.031, -20.141, 0.0, 10.57]) {
            assert!(close(*a, b, 0.001), "{gt:?}");
        }

        // Linear gradient (21:11)
        at(29, [1.0, 0.0, 627.0, 0.0, 1.0, -10.0], 0.01);
        let p = first_paint(29);
        let g = p.gradient.as_ref().unwrap();
        assert_eq!(g.kind, Some(GradientType::Clamp));
        assert_eq!(g.stops, vec![
            ColorStop { color: Color { r: 0.0, g: 0.0, b: 0.0, a: 0.5 }, position: 0.0 },
            ColorStop { color: Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, position: 1.0 },
        ]);
        assert_eq!(p.interpolation_hue_method, Some(InterpolationHueMethod::Increasing));
        assert_eq!(p.gradient_color_profile, Some(ColorProfile::Srgb));

        // Angular gradient with its seam stops (2343:653)
        at(4671, [1.0, 0.0, -3179.0, 0.0, 1.0, -348.0], 0.01);
        let p = first_paint(4671);
        assert_eq!(p.kind, Some(PaintType::GradientAngular));
        let stops = &p.gradient.as_ref().unwrap().stops;
        let pos: Vec<f32> = stops.iter().map(|s| s.position).collect();
        assert_eq!(pos.len(), 6);
        assert!(close(pos[0], 0.0, 1e-6) && close(pos[1], 0.016, 0.001) && close(pos[4], 0.997, 0.001) && close(pos[5], 1.0, 1e-6));
        assert!(close(stops[0].color.r, 0.994, 0.001) && close(stops[5].color.g, 0.996, 0.001));

        // Flipped (20:11)
        at(52, [-1.0, 0.0, 1201.5, 0.0, 1.0, 35.0], 0.01);

        // Tiled image (143:8)
        at(217, [0.975, -0.222, 4001.001, 0.222, 0.975, 642.0], 0.002);
        let p = first_paint(217);
        assert_eq!(p.kind, Some(PaintType::Image));
        assert_eq!(p.image.as_ref().unwrap().hex(), "7b01a39d30dcafb15e6a3027bd702f6d54e91fe1");
        assert_eq!(p.image_scale_mode, Some(ImageScaleMode::Tile));

        // Text (21:6): glyphs at its font size
        at(22, [1.0, 0.0, 89.0, 0.0, 1.0, 926.0], 0.01);
        let n = node(22);
        let batch = n.geometry.as_ref().unwrap().glyph_batch.as_ref().unwrap();
        assert!(!batch.glyphs.is_empty());
        assert_eq!(batch.glyphs[0].font_size, Some(53.0));
        assert!(batch.glyphs.iter().all(|g| g.path.is_some()));

        // Noise effect (2771:730)
        let n = node(12403);
        assert_eq!(n.op, Op::Noise);
        let e = n.ext();
        assert_eq!(e.noise_type, Some(NoiseType::Monotone));
        let size = e.noise_size.unwrap();
        assert!(close(size[0], 100.0, 0.01) && close(size[1], 99.9, 0.01));
        assert_eq!(e.density, Some(1.0));
        let c = e.noise_color.unwrap();
        assert!(close(c.r, 1.0, 0.001) && close(c.b, 0.517, 0.001) && close(c.a, 0.12, 0.001));

        // Op census matches the spec
        let mut ops: HashMap<Op, usize> = HashMap::new();
        for n in forest.nodes.values() {
            *ops.entry(n.op).or_default() += 1;
        }
        assert_eq!(ops[&Op::Fill], 10727);
        assert_eq!(ops[&Op::StaticTransformBinding], 3899);
        assert_eq!(ops[&Op::CustomEffect], 32);
        let extras = forest.nodes.values().filter(|n| !n.extra.is_empty()).count();
        assert_eq!(extras, 0);

        // Every path converts
        let t = Instant::now();
        let verbs: usize = forest.paths.values().map(|p| p.to_skia().count_verbs()).sum();
        eprintln!("to_skia on {} paths, {verbs} verbs: {:?}", forest.paths.len(), t.elapsed());
    }

    #[test]
    fn f_sequence_with_rwlog_releases() {
        // fN.json carry release counts only, rwlog.json the ids
        let Some(log) = json("rwlog.json") else { return };
        let mut released_by_revision = HashMap::new();
        for e in log.as_array().unwrap() {
            if e["m"]["method"] == "sendCanvasFrameToRenderWorker" {
                let f = &e["m"]["params"]["frame"];
                released_by_revision.insert(f["transportRevision"].as_str().unwrap().to_string(), ids(&f["releasedNodeIds"]));
            }
        }
        let schema = Schema::bundled();
        let mut forest = Forest::new();
        let want = [(23693, 23693), (23693, 23693), (23693, 23693), (23693, 23703), (23693, 24473), (23719, 24059), (23693, 26142), (23693, 30527)];
        for (i, (reach, resident)) in want.into_iter().enumerate() {
            let Some(bytes) = capture(&format!(r"rt\f{i}_forest.bin")) else { return };
            let frame = json(&format!(r"rt\f{i}.json")).unwrap();
            let revision = frame["transportRevision"].as_str().unwrap();
            let released = &released_by_revision[revision];
            assert_eq!(released.len() as u64, frame["releasedNodeIds"].as_u64().unwrap_or(released.len() as u64));
            let mode = frame["updateMode"].as_u64().unwrap() as u32;
            let root = frame["canvasRootNodeId"].as_u64().unwrap() as u32;
            let t = Instant::now();
            let applied = forest.apply(&bytes, mode, released, schema).unwrap_or_else(|e| panic!("f{i}: {e}"));
            let took = t.elapsed();
            eprintln!(
                "f{i}: {} bytes, {} new nodes, {} paths ({} stubs), released {} of {}, in {took:?}",
                bytes.len(),
                applied.new_nodes.len(),
                applied.paths,
                applied.path_stubs,
                applied.released,
                released.len()
            );
            assert_eq!(applied.consumed, bytes.len());
            assert_eq!(applied.root, Some(root));
            assert_eq!(reachable(&forest, root), (reach, 0), "f{i}");
            assert_eq!(forest.nodes.len(), resident, "f{i}");
        }
    }

    #[test]
    fn cap2_sequence() {
        let dir = format!(r"{SCRATCH}\rtspec\cap2");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("skipping, {dir} is missing");
            return;
        };
        let mut frames: Vec<u32> = entries
            .filter_map(|e| e.ok()?.file_name().to_str()?.strip_prefix('r')?.strip_suffix("_forest.bin")?.parse().ok())
            .collect();
        frames.sort();
        assert!(frames.len() >= 40);
        let schema = Schema::bundled();
        let mut forest = Forest::new();
        let mut slowest_incremental = std::time::Duration::ZERO;
        for r in frames {
            let bytes = capture(&format!(r"rtspec\cap2\r{r}_forest.bin")).unwrap();
            let frame = json(&format!(r"rtspec\cap2\r{r}.json")).unwrap();
            let mode = frame["updateMode"].as_u64().unwrap() as u32;
            let root = frame["canvasRootNodeId"].as_u64().unwrap() as u32;
            let released = ids(&frame["releasedNodeIds"]);
            let t = Instant::now();
            let applied = forest.apply(&bytes, mode, &released, schema).unwrap_or_else(|e| panic!("r{r}: {e}"));
            let took = t.elapsed();
            if mode != 0 {
                slowest_incremental = slowest_incremental.max(took);
            }
            assert_eq!(applied.consumed, bytes.len());
            assert_eq!(applied.root, Some(root), "r{r}");
            let (reach, missing) = reachable(&forest, root);
            assert_eq!(missing, 0, "r{r}");
            if r >= 128 {
                assert_eq!(reach, 23693, "r{r}");
            }
            if r > 128 && applied.paths > 0 {
                assert_eq!(applied.path_stubs, applied.paths, "r{r}: incremental frames send path stubs");
            }
        }
        eprintln!("cap2: slowest incremental frame {slowest_incremental:?}");
    }

    #[test]
    fn errors_leave_the_forest_as_it_was() {
        let Some(bytes) = capture(r"rt\f0_forest.bin") else { return };
        let schema = Schema::bundled();
        let mut forest = Forest::new();
        forest.apply(&bytes, 0, &[], schema).unwrap();
        // The same full scene again as an incremental frame reuses resident ids
        assert!(forest.apply(&bytes, 1, &[], schema).unwrap_err().contains("reuses"));
        assert_eq!(forest.nodes.len(), 23693);
        // Trailing bytes
        let mut long = bytes[..].to_vec();
        long.push(0);
        assert!(forest.apply(&long, 0, &[], schema).unwrap_err().contains("left over"));
        assert_eq!(forest.nodes.len(), 23693);
        // An external reference to a node that isn't resident
        let mut empty = Forest::new();
        assert!(empty.apply(&[2, 1, 2, 0x38, 0xdf, 0xf7, 3, 0x3a, 1, 0, 0, 0], 1, &[], schema).is_err());
        // Truncated data
        assert!(Forest::new().apply(&bytes[..bytes.len() / 2], 0, &[], schema).is_err());
        // A path stub the cache doesn't have
        if let Some(r129) = capture(r"rtspec\cap2\r129_forest.bin") {
            assert!(Forest::new().apply(&r129, 1, &[], schema).unwrap_err().contains("cached"));
        }
    }

    /// Binary Kiwi schema writer, to build schemas a future Figma might ship
    fn encode_schema(defs: &[Def]) -> Vec<u8> {
        fn uint(out: &mut Vec<u8>, mut v: u32) {
            loop {
                let b = (v & 127) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(b);
                    return;
                }
                out.push(b | 128);
            }
        }
        let mut out = Vec::new();
        uint(&mut out, defs.len() as u32);
        for d in defs {
            out.extend_from_slice(d.name.as_bytes());
            out.push(0);
            out.push(d.kind as u8);
            uint(&mut out, d.fields.len() as u32);
            for f in &d.fields {
                out.extend_from_slice(f.name.as_bytes());
                out.push(0);
                let t: i32 = match f.ty {
                    Type::Bool => -1,
                    Type::Byte => -2,
                    Type::Int => -3,
                    Type::Uint => -4,
                    Type::Float => -5,
                    Type::String => -6,
                    Type::Int64 => -7,
                    Type::Uint64 => -8,
                    Type::Def(i) => i as i32,
                };
                uint(&mut out, ((t << 1) ^ (t >> 31)) as u32);
                out.push(f.array as u8);
                uint(&mut out, f.value);
            }
        }
        out
    }

    #[test]
    fn fields_bind_by_name_not_number() {
        let mut defs = Schema::bundled().defs.clone();
        let node = defs.iter().position(|d| &*d.name == "RenderNode").unwrap();
        let op = defs.iter().position(|d| &*d.name == "RenderNodeOp").unwrap();
        // Renumber every node field, reverse their order, add a field and
        // move GROUP to a new value
        for f in &mut defs[node].fields {
            f.value += 100;
        }
        defs[node].fields.reverse();
        defs[node].fields.push(Field { name: "futureThing".into(), ty: Type::Uint, array: false, value: 7 });
        for f in &mut defs[op].fields {
            if &*f.name == "GROUP" {
                f.value = 99;
            }
        }
        defs[op].fields.push(Field { name: "HOLOGRAM".into(), ty: Type::Def(0), array: false, value: 98 });
        let schema = Schema::parse(&encode_schema(&defs)).unwrap();
        assert_eq!(schema.unbound_fields(), vec!["RenderNode.futureThing".to_string()]);

        // Resources { resources: [{ node: { nodeId 5, op GROUP, childIndices [], futureThing 7 } },
        //                         { node: { nodeId 6, op HOLOGRAM, childIndex 5 } }] }
        let bytes = [
            2, 2, // resources, 2 entries
            2, 156, 1, 5, 101, 99, 102, 0, 7, 7, 0, 0, // node 5
            2, 156, 1, 6, 101, 98, 103, 5, 0, 0, // node 6
            0,
        ];
        let mut forest = Forest::new();
        let applied = forest.apply(&bytes, 0, &[], &schema).unwrap();
        assert_eq!(applied.new_nodes, vec![5, 6]);
        let five = forest.get(5).unwrap();
        assert_eq!(five.op, Op::Group);
        assert_eq!(five.extra, vec![(Arc::from("futureThing"), Value::Uint(7))]);
        let six = forest.get(6).unwrap();
        assert_eq!(six.op, Op::Unknown(98));
        assert_eq!(six.child, Some(5));
    }
}
