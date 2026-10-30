//! Color contrast (Settings > Features > Colors): while the color picker of
//! a solid fill or stroke of a text layer, shape or vector is open, it
//! shows the color's WCAG 2 contrast against what's behind the layer
//!
//! A row under the hex field has a swatch with "Aa" in the color on what
//! it's measured against, the ratio, and the levels: AA and AAA at 4.5:1
//! and 7:1 for text, 3:1 and 4.5:1 for large text (24px, or 18.66px bold),
//! and AA at 3:1 for shapes and vectors. Lines on the picker's color
//! square mark where the color starts to pass each level at the current
//! hue, solid for AA and dashed for AAA, like Chrome's color picker.
//! Hovering the row names the colors behind
//!
//! What's behind is worked out at points under the paint, so under the
//! glyphs for text, from every layer painted before it: fills and strokes
//! by their real shapes, inside clips and masks, with solid colors,
//! gradients, images and blend modes. Effects and image adjustments are
//! left out. The layer's own fills below the one being edited are laid over
//! that. Over more than one color, the row shows the lowest ratio and the
//! typical one, the levels go by the lowest and the lines by the darkest
//! and lightest colors behind, leaving out the outer 5% each way. It's
//! worked out again each time the picker opens
//!
//! Paints, shapes and images are read through Figma's console plugin API,
//! `window.figma`. The
//! paint being edited is the Fill or Stroke row whose picker is open, found
//! by Figma's test ids and class names as seen on 2026-10-29 (see
//! adjustments.rs). Inside the picker, the color square is told by its
//! shape and the hex field as the first inputs below it, so nothing is
//! drawn when they can't be found. The square is Figma's saturation (left
//! to right) by brightness (bottom to top)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_COLOR_CONTRAST__` is `{ on, rev }`: the
/// setting when the tab was made. A page loaded later in the same tab may
/// find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-color-contrast";
  let state = __LIGMA_COLOR_CONTRAST__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.editorType === "figma" ? figma : null;
    } catch {
      return null;
    }
  };
  const paintsOf = (node, list) => {
    try {
      return Array.isArray(node[list]) ? node[list] : [];
    } catch {
      return [];
    }
  };
  const shown = (paint) => paint.visible !== false && (paint.opacity ?? 1) > 0;

  // The Fill or Stroke row whose picker is open, as `{ list, index }`. Its
  // `data-onboarding-key` is `paint-panel-row-paint-<section>-<index>`, and
  // Fill comes first on the page
  const ROW = '[data-testid="paint-panel-color-value-container"]';
  const KEY_PATTERN = /-(\d+)-(\d+)$/;
  const openPaint = (node) => {
    const row = document.querySelector(`${ROW}[class*="paintPanelValueSelected--"]`);
    const match = KEY_PATTERN.exec(row?.dataset.onboardingKey ?? "");
    if (!match) return null;
    const first = KEY_PATTERN.exec(document.querySelector(ROW)?.dataset.onboardingKey ?? "");
    const at = { list: first && first[1] !== match[1] ? "strokes" : "fills", index: Number(match[2]) };
    // A layer with strokes but no fills: Stroke comes first
    if (at.list === "fills" && !paintsOf(node, "fills").length && paintsOf(node, "strokes").length) at.list = "strokes";
    return at;
  };

  // `{ r, g, b }` from 0 to 1, with `paint` laid over it at `opacity`.
  // Null when the paint isn't one color
  const over = (color, paint, opacity = 1) => {
    if (!color || !shown(paint)) return color;
    if (paint.type !== "SOLID") return null;
    const alpha = (paint.opacity ?? 1) * opacity;
    const mix = (key) => paint.color[key] * alpha + color[key] * (1 - alpha);
    return { r: mix("r"), g: mix("g"), b: mix("b") };
  };
  const overAll = (color, paints, opacity) => paints.reduce((color, paint) => over(color, paint, opacity), color);
  const opacityOf = (node) => {
    let opacity = 1;
    for (let at = node; at && at.type !== "PAGE"; at = at.parent) opacity *= at.opacity ?? 1;
    return opacity;
  };

  // What's behind a layer, worked out from the layers painted before it, at
  // points under the paint being edited (its glyphs, for text), or across
  // its box when there are too few. Each layer's fills and strokes count by
  // their real shapes, inside its parents' clips and masks, with solid
  // colors, gradients, images and blend modes. Effects and image
  // adjustments are left out. Rendering with `exportAsync` would also see
  // those, but holds up Figma for seconds. Colors are grouped in bins,
  // darkest first, and `dark` and `light` are the darkest and lightest left
  // once the outer 5% each way are dropped, so stray points don't decide
  const GRID = 1600;
  const MIN_POINTS = 24;
  const IMAGE_SIZE = 256;
  const context = document.createElement("canvas").getContext("2d");
  const apply = ([[a, c, e], [b, d, f]], x, y) => [a * x + c * y + e, b * x + d * y + f];
  const toLocal = ([[a, c, e], [b, d, f]], x, y) => {
    const det = a * d - b * c;
    return [(d * (x - e) - c * (y - f)) / det, (a * (y - f) - b * (x - e)) / det];
  };
  const geometryOf = (node, kind) => {
    try {
      const geometry = kind === "strokes" ? node.strokeGeometry : node.fillGeometry;
      if (geometry?.length) return geometry.map((path) => ({ path: new Path2D(path.data), rule: path.windingRule === "EVENODD" ? "evenodd" : "nonzero" }));
    } catch {}
    if (kind === "strokes") return [];
    const path = new Path2D();
    path.rect(0, 0, node.width, node.height);
    return [{ path, rule: "nonzero" }];
  };
  const inShape = (shape, x, y) => shape.some(({ path, rule }) => context.isPointInPath(path, x, y, rule));
  const bounds = (node) => {
    try {
      return node.absoluteRenderBounds ?? node.absoluteBoundingBox;
    } catch {
      return null;
    }
  };
  const meets = (a, b) => !!a && !!b && a.x <= b.x + b.width && b.x <= a.x + a.width && a.y <= b.y + b.height && b.y <= a.y + a.height;

  // Images, small, by hash, as loaded or loading
  const images = new Map();
  const loaded = new Map();
  const loadImage = (figma, hash) => {
    if (!images.has(hash)) {
      if (images.size >= 32) {
        const oldest = images.keys().next().value;
        images.delete(oldest);
        loaded.delete(oldest);
      }
      // Decoded and made small off the main thread. Figma's `getSizeAsync`
      // decodes on it, holding up the page for up to a second
      const load = async () => {
        const bytes = await figma.getImageByHash(hash).getBytesAsync();
        const full = await createImageBitmap(new Blob([bytes]));
        const scale = Math.min(1, IMAGE_SIZE / Math.max(full.width, full.height));
        const width = Math.max(1, Math.round(full.width * scale));
        const height = Math.max(1, Math.round(full.height * scale));
        const small = await createImageBitmap(full, { resizeWidth: width, resizeHeight: height, resizeQuality: "medium" });
        const natural = { naturalWidth: full.width, naturalHeight: full.height };
        full.close();
        const canvas = document.createElement("canvas");
        canvas.width = width;
        canvas.height = height;
        const pixels = canvas.getContext("2d", { willReadFrequently: true });
        pixels.drawImage(small, 0, 0);
        small.close();
        loaded.set(hash, { data: pixels.getImageData(0, 0, width, height).data, width, height, ...natural });
      };
      images.set(hash, load().catch(() => {}));
    }
    return images.get(hash);
  };

  const stopAt = (stops, t) => {
    t = Math.min(1, Math.max(0, t));
    const i = stops.findIndex((stop) => stop.position >= t);
    if (i === 0) return stops[0].color;
    if (i < 0) return stops[stops.length - 1].color;
    const [a, b] = [stops[i - 1], stops[i]];
    const k = (t - a.position) / (b.position - a.position || 1);
    const mix = (key) => a.color[key] + (b.color[key] - a.color[key]) * k;
    return { r: mix("r"), g: mix("g"), b: mix("b"), a: mix("a") };
  };
  const wrap = (n) => n - Math.floor(n);
  // `{ r, g, b, a }` of `paint` at `x`, `y` in the layer, or null when it
  // can't be told, like a video or an image that didn't load
  const paintAt = (paint, node, x, y) => {
    const width = node.width || 1;
    const height = node.height || 1;
    const u = x / width;
    const v = y / height;
    if (paint.type === "SOLID") return { ...paint.color, a: 1 };
    if (paint.type.startsWith("GRADIENT_")) {
      const [gx, gy] = apply(paint.gradientTransform, u, v);
      const dx = gx - 0.5;
      const dy = gy - 0.5;
      const t = {
        GRADIENT_LINEAR: gx,
        GRADIENT_RADIAL: Math.hypot(dx, dy) * 2,
        GRADIENT_DIAMOND: (Math.abs(dx) + Math.abs(dy)) * 2,
        GRADIENT_ANGULAR: wrap(Math.atan2(dy, dx) / (2 * Math.PI)),
      }[paint.type];
      return t === undefined ? null : stopAt(paint.gradientStops, t);
    }
    if (paint.type !== "IMAGE") return null;
    const image = loaded.get(paint.imageHash);
    if (!image) return null;
    let iu;
    let iv;
    if (paint.scaleMode === "TILE") {
      const size = paint.scalingFactor ?? 1;
      iu = wrap(x / (image.naturalWidth * size));
      iv = wrap(y / (image.naturalHeight * size));
    } else if (paint.scaleMode === "CROP" && paint.imageTransform) {
      [iu, iv] = apply(paint.imageTransform, u, v);
    } else {
      // Fill covers the layer and Fit shows all of the image, both centered
      // and turned by quarter turns
      const turns = Math.round((paint.rotation ?? 0) / 90) & 3;
      let [w, h, px, py] = [width, height, x - width / 2, y - height / 2];
      for (let i = 0; i < turns; i++) [w, h, px, py] = [h, w, py, -px];
      iu = px / w;
      iv = py / h;
      // How much wider the image is than the layer
      const k = image.naturalWidth / image.naturalHeight / (w / h);
      if ((k > 1) !== (paint.scaleMode === "FIT")) iu /= k;
      else iv *= k;
      iu += 0.5;
      iv += 0.5;
    }
    if (iu < 0 || iv < 0 || iu >= 1 || iv >= 1) return { r: 0, g: 0, b: 0, a: 0 };
    const i = (Math.floor(iv * image.height) * image.width + Math.floor(iu * image.width)) * 4;
    return { r: image.data[i] / 255, g: image.data[i + 1] / 255, b: image.data[i + 2] / 255, a: image.data[i + 3] / 255 };
  };

  // `source` laid on `backdrop` in Figma's blend `mode`, before opacity,
  // by the W3C compositing formulas
  const channel = {
    DARKEN: (b, s) => Math.min(b, s),
    MULTIPLY: (b, s) => b * s,
    LINEAR_BURN: (b, s) => Math.max(0, b + s - 1),
    COLOR_BURN: (b, s) => (b >= 1 ? 1 : s <= 0 ? 0 : 1 - Math.min(1, (1 - b) / s)),
    LIGHTEN: (b, s) => Math.max(b, s),
    SCREEN: (b, s) => b + s - b * s,
    LINEAR_DODGE: (b, s) => Math.min(1, b + s),
    COLOR_DODGE: (b, s) => (b <= 0 ? 0 : s >= 1 ? 1 : Math.min(1, b / (1 - s))),
    OVERLAY: (b, s) => channel.HARD_LIGHT(s, b),
    HARD_LIGHT: (b, s) => (s <= 0.5 ? b * 2 * s : channel.SCREEN(b, 2 * s - 1)),
    SOFT_LIGHT: (b, s) => {
      const d = b <= 0.25 ? ((16 * b - 12) * b + 4) * b : Math.sqrt(b);
      return s <= 0.5 ? b - (1 - 2 * s) * b * (1 - b) : b + (2 * s - 1) * (d - b);
    },
    DIFFERENCE: (b, s) => Math.abs(b - s),
    EXCLUSION: (b, s) => b + s - 2 * b * s,
  };
  const lum = ({ r, g, b }) => 0.3 * r + 0.59 * g + 0.11 * b;
  const setLum = (color, l) => {
    const d = l - lum(color);
    let [r, g, b] = [color.r + d, color.g + d, color.b + d];
    const L = lum({ r, g, b });
    const n = Math.min(r, g, b);
    const x = Math.max(r, g, b);
    if (n < 0) [r, g, b] = [r, g, b].map((c) => L + ((c - L) * L) / (L - n));
    if (x > 1) [r, g, b] = [r, g, b].map((c) => L + ((c - L) * (1 - L)) / (x - L));
    return { r, g, b };
  };
  const sat = ({ r, g, b }) => Math.max(r, g, b) - Math.min(r, g, b);
  const setSat = (color, s) => {
    const keys = ["r", "g", "b"].sort((a, b) => color[a] - color[b]);
    const [min, mid, max] = keys.map((key) => color[key]);
    const out = { r: 0, g: 0, b: 0 };
    if (max > min) {
      out[keys[1]] = ((mid - min) * s) / (max - min);
      out[keys[2]] = s;
    }
    return out;
  };
  const blended = (mode, backdrop, source) => {
    const pick = channel[mode];
    if (pick) return { r: pick(backdrop.r, source.r), g: pick(backdrop.g, source.g), b: pick(backdrop.b, source.b) };
    if (mode === "HUE") return setLum(setSat(source, sat(backdrop)), lum(backdrop));
    if (mode === "SATURATION") return setLum(setSat(backdrop, sat(source)), lum(backdrop));
    if (mode === "COLOR") return setLum(source, lum(backdrop));
    if (mode === "LUMINOSITY") return setLum(backdrop, lum(source));
    return source;
  };

  const read = async (figma, node, kind) => {
    const transform = node.absoluteTransform;
    const width = node.width;
    const height = node.height;
    // Points on a grid over the layer, kept where its paint is
    const columns = Math.max(1, Math.round(Math.sqrt((GRID * Math.max(width, 1)) / Math.max(height, 1))));
    const rows = Math.max(1, Math.round(GRID / columns));
    const own = geometryOf(node, kind);
    const all = [];
    let points = [];
    for (let row = 0; row < rows; row++) {
      for (let column = 0; column < columns; column++) {
        const x = ((column + 0.5) / columns) * width;
        const y = ((row + 0.5) / rows) * height;
        const [px, py] = apply(transform, x, y);
        const point = { x: px, y: py, known: true };
        all.push(point);
        if (inShape(own, x, y)) points.push(point);
      }
    }
    if (points.length < MIN_POINTS) points = all;

    // The layers painted before it, in order, each with its opacity and
    // the shapes it's clipped to
    const target = bounds(node);
    const ancestors = new Set();
    for (let at = node.parent; at && at.type !== "PAGE"; at = at.parent) ancestors.add(at);
    const layers = [];
    // The shapes the layer itself is clipped to, once reached
    let reached = null;
    const visit = (at, opacity, clips, blend) => {
      if (at === node) {
        reached = clips;
        return true;
      }
      if (!at.visible || (!ancestors.has(at) && !meets(bounds(at), target))) return false;
      if (at.type === "BOOLEAN_OPERATION" && ancestors.has(at)) throw new Error("Inside a boolean");
      opacity *= at.opacity ?? 1;
      // A group's or frame's mode is laid on what's in it, one by one
      if (at.blendMode && at.blendMode !== "PASS_THROUGH" && at.blendMode !== "NORMAL") blend = at.blendMode;
      const fills = paintsOf(at, "fills").filter(shown);
      if (fills.length) layers.push({ at, kind: "fills", paints: fills, opacity, clips, blend });
      const strokes = paintsOf(at, "strokes").filter(shown);
      if ("children" in at && at.type !== "BOOLEAN_OPERATION") {
        let inner = at.clipsContent ? [...clips, { at, kind: "fills" }] : clips;
        for (const child of at.children) {
          // A mask isn't drawn, nor anything in it, so neither is a layer
          // inside one
          if (child.isMask) {
            if (ancestors.has(child)) return true;
            if (child.visible) inner = [...inner, { at: child, kind: "fills" }];
            continue;
          }
          if (visit(child, opacity, inner, blend)) return true;
        }
      }
      if (strokes.length) layers.push({ at, kind: "strokes", paints: strokes, opacity, clips, blend });
      return false;
    };
    if (node.isMask) return null;
    for (const child of figma.currentPage.children) {
      if (visit(child, 1, [], "NORMAL")) break;
    }
    // Not reached when hidden or inside a mask
    if (!reached) return null;
    const shapes = new Map();
    const within = ({ at, kind }, point) => {
      const key = at.id + kind;
      if (!shapes.has(key)) shapes.set(key, { shape: geometryOf(at, kind), transform: at.absoluteTransform });
      const { shape, transform } = shapes.get(key);
      const [x, y] = toLocal(transform, point.x, point.y);
      return inShape(shape, x, y) ? [x, y] : null;
    };
    // Only where the layer shows
    points = points.filter((point) => reached.every((clip) => within(clip, point)));
    if (!points.length) return null;

    // The points each layer covers, in slices so Figma keeps drawing
    let slice = performance.now();
    for (const layer of layers) {
      if (performance.now() - slice > 8) {
        await new Promise((resolve) => setTimeout(resolve));
        slice = performance.now();
      }
      layer.hits = [];
      for (const point of points) {
        if (!layer.clips.every((clip) => within(clip, point))) continue;
        const local = within(layer, point);
        if (local) layer.hits.push({ point, local });
      }
    }
    // Images only for layers that cover any, since each can take Figma a
    // moment to fetch
    const covering = layers.filter((layer) => layer.hits.length);
    await Promise.all(covering.flatMap(({ paints }) => paints.filter((paint) => paint.type === "IMAGE" && paint.imageHash).map((paint) => loadImage(figma, paint.imageHash))));

    // Laid over the canvas, bottom first
    const page = overAll({ r: 1, g: 1, b: 1 }, figma.currentPage.backgrounds ?? []) ?? { r: 1, g: 1, b: 1 };
    for (const point of points) point.color = { ...page };
    for (const layer of covering) {
      for (const { point, local } of layer.hits) {
        if (!point.known) continue;
        for (const paint of layer.paints) {
          const color = paintAt(paint, layer.at, ...local);
          if (!color) {
            point.known = false;
            break;
          }
          const mode = paint.blendMode && paint.blendMode !== "NORMAL" ? paint.blendMode : layer.blend;
          point.color = over(point.color, { type: "SOLID", color: blended(mode, point.color, color) }, color.a * (paint.opacity ?? 1) * layer.opacity);
        }
      }
    }

    const bins = new Map();
    for (const { known, color } of points) {
      if (!known) continue;
      const key = ["r", "g", "b"].map((k) => Math.round(color[k] * 63)).join();
      const bin = bins.get(key) ?? { sum: [0, 0, 0], count: 0 };
      bin.sum[0] += color.r;
      bin.sum[1] += color.g;
      bin.sum[2] += color.b;
      bin.count++;
      bins.set(key, bin);
    }
    if (!bins.size) return null;
    const sorted = [...bins.values()]
      .map(({ sum, count }) => {
        const color = { r: sum[0] / count, g: sum[1] / count, b: sum[2] / count };
        return { color, count, lum: luminance(color) };
      })
      .sort((a, b) => a.lum - b.lum);
    const total = sorted.reduce((total, bin) => total + bin.count, 0);
    let seen = 0;
    for (const bin of sorted) {
      bin.kept = seen + bin.count > total * 0.05 && seen < total * 0.95;
      seen += bin.count;
    }
    const kept = sorted.filter((bin) => bin.kept);
    return { bins: sorted, dark: kept[0].color, light: kept[kept.length - 1].color };
  };
  // Read once each time the picker opens, and again for another paint list
  // or if the layer moves. Null until it's read
  let backdropKey = "";
  let backdrop = null;
  let pendingKey = "";
  const backdropOf = (figma, node, kind) => {
    const key = JSON.stringify([node.id, kind, node.absoluteBoundingBox]);
    if (key === backdropKey) return backdrop;
    if (key !== pendingKey) {
      pendingKey = key;
      read(figma, node, kind)
        .catch(() => null)
        .then((value) => {
          if (pendingKey !== key) return;
          pendingKey = "";
          backdropKey = key;
          backdrop = value;
        });
    }
    return null;
  };
  const forget = () => {
    backdropKey = pendingKey = "";
    backdrop = null;
  };

  const luminance = ({ r, g, b }) => {
    const linear = (c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
    return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
  };
  const contrast = (a, b) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  const hsv = ({ r, g, b }) => {
    const max = Math.max(r, g, b);
    const range = max - Math.min(r, g, b);
    let h = 0;
    if (range) {
      if (max === r) h = ((g - b) / range + 6) % 6;
      else if (max === g) h = (b - r) / range + 2;
      else h = (r - g) / range + 4;
    }
    return { h: h / 6, s: max ? range / max : 0, v: max };
  };
  const rgb = (h, s, v) => {
    const channel = (n) => {
      const k = (n + h * 6) % 6;
      return v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
    };
    return { r: channel(5), g: channel(3), b: channel(1) };
  };
  const css = ({ r, g, b }, alpha = 1) => `rgb(${r * 255} ${g * 255} ${b * 255} / ${alpha})`;
  const hex = (color) =>
    "#" + ["r", "g", "b"].map((key) => Math.round(color[key] * 255).toString(16).padStart(2, "0")).join("").toUpperCase();

  // The levels a layer is held to, lowest first. None for layers that
  // aren't text, shapes or vectors, like frames
  const GRAPHICS = new Set(["VECTOR", "BOOLEAN_OPERATION", "LINE", "STAR", "POLYGON", "RECTANGLE", "ELLIPSE"]);
  const levelsOf = (node) => {
    if (GRAPHICS.has(node.type)) return [{ name: "AA", min: 3 }];
    if (node.type !== "TEXT") return null;
    const size = typeof node.fontSize === "number" ? node.fontSize : 0;
    const weight = typeof node.fontWeight === "number" ? node.fontWeight : 400;
    const large = size >= 24 || (size >= 18.66 && weight >= 700);
    return [
      { name: "AA", min: large ? 3 : 4.5 },
      { name: "AAA", min: large ? 4.5 : 7 },
    ];
  };

  // What to show for the open picker, or null for nothing
  const measure = () => {
    const figma = api();
    const node = figma?.currentPage.selection[0];
    const levels = node && levelsOf(node);
    if (!levels) return null;
    const at = openPaint(node);
    const list = at && paintsOf(node, at.list);
    const paint = list?.[at.index];
    if (paint?.type !== "SOLID") return null;
    const back = backdropOf(figma, node, at.list);
    if (!back) return null;
    // The layer's own fills below this one lie over what's behind
    const below = at.list === "fills" ? list.slice(0, at.index) : [];
    const opacity = opacityOf(node);
    const alpha = (paint.opacity ?? 1) * opacity;
    const ratios = [];
    let worst = null;
    for (const bin of back.bins) {
      const bg = overAll(bin.color, below);
      if (!bg) return null;
      const ratio = contrast(luminance(over(bg, { ...paint, visible: true }, opacity)), luminance(bg));
      ratios.push({ ratio, count: bin.count });
      if (bin.kept && (!worst || ratio < worst.ratio)) worst = { ratio, bg };
    }
    // Weighted median
    ratios.sort((a, b) => a.ratio - b.ratio);
    let half = ratios.reduce((total, { count }) => total + count, 0) / 2;
    const typical = ratios.find(({ count }) => (half -= count) <= 0).ratio;
    return { paint, alpha, worst, typical, dark: overAll(back.dark, below), light: overAll(back.light, below), levels };
  };

  const PICKER = '[data-testid="paint-picker-v2-modal"]';
  const visible = (node) => node.getClientRects().length > 0;
  // The color square: the smallest squarish area nearly as wide as the
  // picker without inputs or buttons in it, so not a box around it and
  // the sliders
  const squareOf = (picker) => {
    const width = picker.getBoundingClientRect().width;
    let best = null;
    let area = Infinity;
    for (const node of picker.querySelectorAll("div, canvas")) {
      const rect = node.getBoundingClientRect();
      if (rect.width < width * 0.6 || rect.height < rect.width * 0.4 || rect.height > rect.width * 1.2) continue;
      if (node.tagName === "DIV" && node.querySelector("input, button, select")) continue;
      if (rect.width * rect.height < area) {
        best = node;
        area = rect.width * rect.height;
      }
    }
    return best;
  };
  // The hex field's row: the first input below the square, and the boxes
  // around it that are still one row high
  const hexRowOf = (picker, square) => {
    const bottom = square.getBoundingClientRect().bottom;
    const input = [...picker.querySelectorAll("input")].find((input) => visible(input) && input.getBoundingClientRect().top >= bottom);
    if (!input) return null;
    let row = input;
    while (row.parentElement && row.parentElement !== picker && row.parentElement.getBoundingClientRect().height <= 48) row = row.parentElement;
    return row === input ? null : row;
  };
  // The hue the square shows: its base color, if it's painted with one,
  // else the paint's own, kept while the paint is grey and has none
  let lastHue = 0;
  const hueOf = (square, paint) => {
    for (const node of [square, ...square.querySelectorAll("div")]) {
      const match = /^rgba?\((\d+), (\d+), (\d+)(?:, ([\d.]+))?\)$/.exec(getComputedStyle(node).backgroundColor);
      if (!match || match[4] === "0") continue;
      const color = hsv({ r: match[1] / 255, g: match[2] / 255, b: match[3] / 255 });
      if (color.s > 0.5 && color.v > 0.5) return (lastHue = color.h);
    }
    const color = hsv(paint.color);
    if (color.s > 0 && color.v > 0) lastHue = color.h;
    return lastHue;
  };

  // Where a level starts to pass on the square at hue `h`: one path for
  // darker colors, against the darkest of what's behind, and one for
  // lighter, against the lightest, as SVG path data in a unit box. A
  // color's luminance only grows with its brightness, so each column's
  // edge is found by halving
  const COLUMNS = 48;
  const edges = (h, dark, light, alpha, min) => {
    let path = "";
    for (const [bg, darker] of [
      [dark, true],
      [light, false],
    ]) {
      const back = luminance(bg);
      const target = darker ? (back + 0.05) / min - 0.05 : min * (back + 0.05) - 0.05;
      const lumAt = (s, v) => luminance(over(bg, { type: "SOLID", color: rgb(h, s, v) }, alpha));
      const solve = (s) => {
        if (target <= lumAt(s, 0) || target >= lumAt(s, 1)) return null;
        let [low, high] = [0, 1];
        for (let i = 0; i < 16; i++) {
          const mid = (low + high) / 2;
          if (lumAt(s, mid) < target) low = mid;
          else high = mid;
        }
        return (low + high) / 2;
      };
      let drawing = false;
      for (let i = 0; i <= COLUMNS; i++) {
        const s = i / COLUMNS;
        const v = solve(s);
        if (v === null) {
          drawing = false;
          continue;
        }
        path += `${drawing ? "L" : "M"}${s.toFixed(4)} ${(1 - v).toFixed(4)}`;
        drawing = true;
      }
    }
    return path;
  };

  const ROW_ATTR = "data-ligma-contrast";
  const LINES_ATTR = "data-ligma-contrast-lines";
  const SVG = "http://www.w3.org/2000/svg";
  const sample = (dashed) =>
    `<svg width="12" height="2" viewBox="0 0 12 2" aria-hidden="true"><path d="M0 1h12" stroke="currentColor" stroke-width="1.5"${dashed ? ' stroke-dasharray="3 2"' : ""}/></svg>`;
  const MARKS = {
    pass: '<svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="m3 6.2 2 2L9 4" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    fail: '<svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="m4 4 4 4m0-4L4 8" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>',
  };

  const clear = () => {
    for (const node of document.querySelectorAll(`[${ROW_ATTR}], [${LINES_ATTR}]`)) node.remove();
  };

  // The row under the hex field
  const showRow = (picker, anchor, result) => {
    let row = picker.querySelector(`[${ROW_ATTR}]`);
    if (!row || row.previousElementSibling !== anchor) {
      row?.remove();
      row = document.createElement("div");
      row.setAttribute(ROW_ATTR, "");
      row.dataset.tooltipType = "text";
      anchor.after(row);
    }
    // Lined up with the hex row's first control
    const inset = Math.max(0, (anchor.firstElementChild ?? anchor).getBoundingClientRect().left - anchor.getBoundingClientRect().left);
    row.style.paddingInline = `${inset}px`;
    // Over one color, its ratio. Over several, the lowest and the typical one
    const format = (ratio) => (Math.floor(ratio * 100) / 100).toFixed(2);
    const lowest = format(result.worst.ratio);
    const typical = format(result.typical);
    const varied = typical - lowest >= 0.1;
    const ratio = varied ? `${lowest}–${typical}` : lowest;
    row.dataset.tooltip = varied
      ? `Lowest against ${hex(result.worst.bg)}, typically ${typical}:1, with ${hex(result.dark)} to ${hex(result.light)} behind it`
      : `Against ${hex(result.worst.bg)}`;
    const levels = result.levels
      .map((level, i) => {
        const pass = result.worst.ratio >= level.min;
        const mark = pass ? "pass" : i === 0 ? "fail" : "short";
        return `<span data-level="${mark}">${sample(i > 0)}${level.name}${MARKS[pass ? "pass" : "fail"]}</span>`;
      })
      .join("");
    const html =
      `<span data-swatch style="background:${css(result.worst.bg)};color:${css(result.paint.color, result.alpha)}">Aa</span>` +
      `<span data-ratio>${ratio}:1</span><span data-levels>${levels}</span>`;
    if (row.innerHTML !== html) row.innerHTML = html;
  };

  // The lines on the square, laid over it inside the picker so they move
  // with it. Worked out again only when the hue, background, opacity or
  // levels change
  let drawn = "";
  const showLines = (picker, square, result) => {
    let lines = picker.querySelector(`[${LINES_ATTR}]`);
    if (!lines) {
      lines = document.createElement("div");
      lines.setAttribute(LINES_ATTR, "");
      lines.innerHTML = `<svg xmlns="${SVG}" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"></svg>`;
      picker.append(lines);
      drawn = "";
    }
    // Placed from what it's positioned against
    const host = lines.offsetParent ?? document.body;
    const origin = host.getBoundingClientRect();
    const rect = square.getBoundingClientRect();
    Object.assign(lines.style, {
      left: `${rect.left - origin.left - host.clientLeft + host.scrollLeft}px`,
      top: `${rect.top - origin.top - host.clientTop + host.scrollTop}px`,
      width: `${rect.width}px`,
      height: `${rect.height}px`,
      borderRadius: getComputedStyle(square).borderRadius,
    });
    const h = hueOf(square, result.paint);
    const key = [h.toFixed(3), hex(result.dark), hex(result.light), result.alpha.toFixed(3), ...result.levels.map((level) => level.min)].join();
    if (key === drawn) return;
    drawn = key;
    lines.firstElementChild.innerHTML = result.levels
      .map((level, i) => {
        const d = edges(h, result.dark, result.light, result.alpha, level.min);
        if (!d) return "";
        const dash = i > 0 ? ' stroke-dasharray="4 3"' : "";
        return `<path d="${d}" data-shadow${dash}/><path d="${d}"${dash}/>`;
      })
      .join("");
  };

  const render = () => {
    const picker = state.on && document.querySelector(PICKER);
    let result = null;
    let square = null;
    let anchor = null;
    if (picker) {
      try {
        result = measure();
        square = result && squareOf(picker);
        anchor = square && hexRowOf(picker, square);
      } catch {
        result = null;
      }
    }
    if (!result || !anchor) {
      clear();
      return;
    }
    showRow(picker, anchor, result);
    showLines(picker, square, result);
  };

  const style = document.createElement("style");
  style.textContent = `
    [${ROW_ATTR}] {
      display: flex; align-items: center; gap: 8px; height: 32px; box-sizing: border-box;
      font-size: 11px; color: var(--color-text-secondary, rgba(0, 0, 0, 0.5));
    }
    [${ROW_ATTR}] [data-swatch] {
      flex: none; display: grid; place-items: center; width: 24px; height: 24px; border-radius: 4px;
      box-shadow: inset 0 0 0 1px var(--color-border, rgba(0, 0, 0, 0.1));
      font-size: 11px; font-weight: 600; letter-spacing: -0.01em;
    }
    [${ROW_ATTR}] [data-ratio] { font-weight: 500; color: var(--color-text, rgba(0, 0, 0, 0.9)); font-variant-numeric: tabular-nums; }
    [${ROW_ATTR}] [data-levels] { display: flex; gap: 10px; margin-left: auto; }
    [${ROW_ATTR}] [data-level] { display: flex; align-items: center; gap: 4px; }
    [${ROW_ATTR}] [data-level] > svg:first-child { opacity: 0.6; }
    [${ROW_ATTR}] [data-level="pass"] > svg:last-child { color: var(--color-icon-success, #14ae5c); }
    [${ROW_ATTR}] [data-level="fail"] > svg:last-child { color: var(--color-icon-danger, #f24822); }
    [${ROW_ATTR}] [data-level="short"] > svg:last-child { color: var(--color-icon-tertiary, rgba(0, 0, 0, 0.3)); }
    [${LINES_ATTR}] { position: absolute; z-index: 1; overflow: hidden; pointer-events: none; }
    [${LINES_ATTR}] svg { display: block; width: 100%; height: 100%; overflow: visible; }
    [${LINES_ATTR}] path { fill: none; stroke: #fff; stroke-width: 1.25px; vector-effect: non-scaling-stroke; }
    [${LINES_ATTR}] path[data-shadow] { stroke: rgba(0, 0, 0, 0.35); stroke-width: 3px; }
  `;
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  // Dragging in the picker changes the paint without changing the page,
  // so while it's open it's checked every few frames
  let frame;
  let ticks = 0;
  const loop = () => {
    if (!(state.on && document.querySelector(PICKER))) {
      frame = undefined;
      clear();
      forget();
      return;
    }
    if (ticks++ % 4 === 0) render();
    frame = requestAnimationFrame(loop);
  };
  const watch = () => {
    if (!frame) loop();
  };
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      watch();
    });
  }).observe(document, { childList: true, subtree: true });

  // From Ligma, when the setting changes
  window.__ligmaColorContrast = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    state = next;
    watch();
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        watch();
      }
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
})()"##;


/// The setting,
static ON: AtomicBool = AtomicBool::new(false);
/// and when it last changed, for `PAGE_JS` to tell it from older ones in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    format!(r#"{{"on":{},"rev":{}}}"#, ON.load(Ordering::Relaxed), REV.load(Ordering::Relaxed))
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.color_contrast, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_COLOR_CONTRAST__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.color_contrast, Ordering::Relaxed) == settings.color_contrast {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaColorContrast?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
