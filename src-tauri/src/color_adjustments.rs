//! Ligma adjustments (Settings > Images > Adjustments): Vibrance and
//! Hue/Saturation for image fills and strokes, which Figma's picker doesn't
//! have, in a section of their own under its seven sliders, marked as
//! Ligma's, that opens and closes. On by default
//!
//! Figma can't run code on an image as it draws it, so the image is made
//! again with them, on the GPU, and put in the paint in place of the
//! original. The original is kept as a hidden paint right below it, so the
//! layer stays a plain image fill anyone can edit, with or without Ligma:
//! showing the original again, or changing Figma's own sliders, works as
//! it always does. Which image each one was made from, and with what, is
//! kept on the layer (shared plugin data `ligma`/`color`), keyed by the
//! made image's hash, so the sliders open where they were left and are
//! always applied to the original, never to an image already adjusted
//!
//! While a slider moves, a smaller image is made, quick to make and load;
//! the full size one follows once it's let go, and is given up on if the
//! slider moves again before it's in. Each new image is
//! loaded off the canvas before it's swapped in, so the fill never shows
//! empty. The hidden original is put below once the picker closes, as
//! adding a paint while it's open would move the picker to another one
//!
//! Copy and paste adjustments (adjustments.rs) carry these too, through
//! `window.__ligmaColor`

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_COLOR__` is `{ on, rev }`: the setting when
/// the tab was made. A page loaded later in the same tab may find a newer
/// one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-color-adjustments";
  let state = __LIGMA_COLOR__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  // The plugin API, if the file can be edited
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
  const image = (paint) => paint?.type === "IMAGE" && !!paint.imageHash;

  // ---------- Settings ----------

  // Hue/Saturation's ranges, as Photoshop's: each a sixth of the wheel,
  // fully within 15º of its hue and fading out to 45º. All, Photoshop's
  // Master, changes every color, and keeps its values at the top level
  const RANGES = [
    ["reds", "Reds", 0],
    ["yellows", "Yellows", 60],
    ["greens", "Greens", 120],
    ["cyans", "Cyans", 180],
    ["blues", "Blues", 240],
    ["magentas", "Magentas", 300],
  ];
  const blank = () => ({ vibrance: 0, hue: 0, saturation: 0, lightness: 0, ...Object.fromEntries(RANGES.map(([id]) => [id, { hue: 0, saturation: 0, lightness: 0 }])) });
  // Any shape saved or copied, as one with every value
  const normal = (s) => {
    const out = blank();
    if (!s || typeof s !== "object") return out;
    const number = (value, limit) => (Number.isFinite(value) ? Math.max(-limit, Math.min(limit, Math.round(value))) : 0);
    out.vibrance = number(s.vibrance, 100);
    out.hue = number(s.hue, 180);
    out.saturation = number(s.saturation, 100);
    out.lightness = number(s.lightness, 100);
    for (const [id] of RANGES) {
      out[id].hue = number(s[id]?.hue, 180);
      out[id].saturation = number(s[id]?.saturation, 100);
      out[id].lightness = number(s[id]?.lightness, 100);
    }
    return out;
  };
  const changed = (values) => !!(values.hue || values.saturation || values.lightness);
  const isBlank = (s) => !s.vibrance && !changed(s) && RANGES.every(([id]) => !changed(s[id]));

  // What each adjusted image on a layer was made from, `{ [hash]: {
  // original, settings } }`
  const PLUGIN = "ligma";
  const DATA = "color";
  const madeOn = (node) => {
    try {
      return JSON.parse(node.getSharedPluginData(PLUGIN, DATA) || "{}");
    } catch {
      return {};
    }
  };
  // Kept only for images the layer still shows
  const keepMade = (node, made) => {
    const hashes = new Set([...paintsOf(node, "fills"), ...paintsOf(node, "strokes")].map((paint) => paint.imageHash));
    for (const hash of Object.keys(made)) if (!hashes.has(hash)) delete made[hash];
    try {
      node.setSharedPluginData(PLUGIN, DATA, Object.keys(made).length ? JSON.stringify(made) : "");
    } catch {}
  };
  // A paint's settings, blank for one Ligma didn't adjust
  const settingsOf = (node, paint) => normal(image(paint) ? madeOn(node)[paint.imageHash]?.settings : null);
  const originalOf = (node, hash) => madeOn(node)[hash]?.original ?? hash;

  // ---------- Making the image ----------

  // The picture's top row at the canvas's top, which WebGL counts from the
  // bottom. ImageBitmaps aren't flipped by UNPACK_FLIP_Y_WEBGL
  const VERTEX = `attribute vec2 p; varying vec2 uv; void main() { uv = vec2(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5); gl_Position = vec4(p, 0.0, 1.0); }`;
  const FRAGMENT = `
    precision highp float;
    uniform sampler2D image;
    uniform vec3 master;
    uniform float vibrance;
    uniform vec3 ranges[6];
    varying vec2 uv;
    vec3 toHsl(vec3 c) {
      float hi = max(c.r, max(c.g, c.b));
      float lo = min(c.r, min(c.g, c.b));
      float l = (hi + lo) * 0.5;
      float d = hi - lo;
      if (d < 1e-5) return vec3(0.0, 0.0, l);
      float s = l > 0.5 ? d / (2.0 - hi - lo) : d / (hi + lo);
      float h = hi == c.r ? (c.g - c.b) / d + (c.g < c.b ? 6.0 : 0.0) : hi == c.g ? (c.b - c.r) / d + 2.0 : (c.r - c.g) / d + 4.0;
      return vec3(h / 6.0, s, l);
    }
    float channel(float p, float q, float t) {
      t = fract(t);
      if (t < 1.0 / 6.0) return p + (q - p) * 6.0 * t;
      if (t < 0.5) return q;
      if (t < 2.0 / 3.0) return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
      return p;
    }
    vec3 toRgb(vec3 c) {
      if (c.y <= 0.0) return vec3(c.z);
      float q = c.z < 0.5 ? c.z * (1.0 + c.y) : c.z + c.y - c.z * c.y;
      float p = 2.0 * c.z - q;
      return vec3(channel(p, q, c.x + 1.0 / 3.0), channel(p, q, c.x), channel(p, q, c.x - 1.0 / 3.0));
    }
    float saturate(float s, float by) { return by < 0.0 ? s * (1.0 + by) : 1.0 - pow(1.0 - s, 1.0 + 2.0 * by); }
    float lighten(float l, float by) { return by < 0.0 ? l * (1.0 + by) : l + (1.0 - l) * by; }
    void main() {
      vec4 pixel = texture2D(image, uv);
      vec3 c = toHsl(pixel.rgb);
      // Grays have no hue, so no range takes them
      float colorful = smoothstep(0.0, 0.06, c.y * (1.0 - abs(2.0 * c.z - 1.0)));
      vec3 shift = vec3(0.0);
      for (int i = 0; i < 6; i++) {
        float away = abs(fract(c.x - float(i) / 6.0 + 0.5) - 0.5) * 360.0;
        shift += ranges[i] * clamp((45.0 - away) / 30.0, 0.0, 1.0) * colorful;
      }
      // Skin's hues, oranges, kept from Vibrance's boost
      float skin = clamp(1.0 - abs(c.x * 360.0 - 25.0) / 30.0, 0.0, 1.0) * smoothstep(0.08, 0.25, c.y);
      c.x = fract(c.x + master.x + shift.x);
      c.y = saturate(saturate(c.y, shift.y), master.y);
      c.z = lighten(lighten(c.z, shift.z), master.z);
      // The less colorful a color, the more Vibrance changes it
      c.y = vibrance < 0.0 ? c.y * (1.0 + vibrance * (1.0 - 0.5 * c.y)) : c.y * (1.0 + vibrance * 2.0 * (1.0 - c.y) * (1.0 - c.y) * (1.0 - 0.7 * skin));
      c.y = clamp(c.y, 0.0, 1.0);
      gl_FragColor = vec4(toRgb(c), pixel.a);
    }`;
  let gl = null;
  const gpu = () => {
    if (gl && !gl.isContextLost()) return gl;
    const canvas = document.createElement("canvas");
    gl = canvas.getContext("webgl", { premultipliedAlpha: false, preserveDrawingBuffer: true, antialias: false });
    if (!gl) return null;
    const shader = (type, source) => {
      const made = gl.createShader(type);
      gl.shaderSource(made, source);
      gl.compileShader(made);
      return made;
    };
    const program = gl.createProgram();
    gl.attachShader(program, shader(gl.VERTEX_SHADER, VERTEX));
    gl.attachShader(program, shader(gl.FRAGMENT_SHADER, FRAGMENT));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      gl = null;
      return null;
    }
    gl.useProgram(program);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    const at = gl.getAttribLocation(program, "p");
    gl.enableVertexAttribArray(at);
    gl.vertexAttribPointer(at, 2, gl.FLOAT, false, 0, 0);
    gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
    for (const [key, value] of [
      [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE],
      [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE],
      [gl.TEXTURE_MIN_FILTER, gl.NEAREST],
      [gl.TEXTURE_MAG_FILTER, gl.NEAREST],
    ])
      gl.texParameteri(gl.TEXTURE_2D, key, value);
    gl.uniforms = Object.fromEntries(["master", "vibrance", "ranges"].map((name) => [name, gl.getUniformLocation(program, name)]));
    gl.drawn = null;
    return gl;
  };

  // Whether a picture has no see-through pixels, so its previews can be
  // JPEGs, far quicker to make than PNGs
  const opaque = (picture) => {
    try {
      const canvas = Object.assign(document.createElement("canvas"), { width: picture.width, height: picture.height });
      const context = canvas.getContext("2d", { willReadFrequently: true });
      context.drawImage(picture, 0, 0);
      const { data } = context.getImageData(0, 0, picture.width, picture.height);
      for (let i = 3; i < data.length; i += 4) if (data[i] < 255) return false;
      return true;
    } catch {
      return false;
    }
  };

  // Each original as pictures to make from, the last few kept: a small one
  // for previews, decoded first, and the full size one when it's needed
  const PREVIEW = 512;
  const sources = new Map();
  const sourceOf = (figma, hash) => {
    if (!sources.has(hash)) {
      const made = (async () => {
        const picture = figma.getImageByHash(hash);
        const bytes = await picture.getBytesAsync();
        const blob = new Blob([bytes]);
        const options = { premultiplyAlpha: "none", colorSpaceConversion: "none" };
        let full = null;
        let size = null;
        try {
          size = await picture.getSizeAsync();
        } catch {}
        if (!size?.width) size = full = await createImageBitmap(blob, options);
        const scale = Math.min(1, PREVIEW / Math.max(size.width, size.height));
        const small =
          scale < 1
            ? await createImageBitmap(blob, { ...options, resizeWidth: Math.max(1, Math.round(size.width * scale)), resizeHeight: Math.max(1, Math.round(size.height * scale)), resizeQuality: "medium" })
            : (full ??= await createImageBitmap(blob, options));
        // JPEGs stay JPEGs, quicker to make and as small as they were
        const jpeg = bytes[0] === 0xff && bytes[1] === 0xd8;
        return { blob, options, small, full: full && Promise.resolve(full), jpeg, opaque: jpeg || opaque(small) };
      })();
      made.catch(() => sources.delete(hash));
      sources.set(hash, made);
      while (sources.size > 4) {
        const [oldest, picture] = sources.entries().next().value;
        sources.delete(oldest);
        picture
          .then(async (source) => {
            source.small.close();
            (await source.full)?.close();
          })
          .catch(() => {});
      }
    }
    return sources.get(hash);
  };

  // The original `hash` with `s`, as image bytes, full size or small. Drawn
  // and read back with nothing awaited between, so two can be under way
  const make = async (figma, hash, s, full) => {
    const source = await sourceOf(figma, hash);
    const picture = full ? await (source.full ??= createImageBitmap(source.blob, source.options)) : source.small;
    const gl = gpu();
    if (!gl) throw new Error("no WebGL");
    gl.canvas.width = picture.width;
    gl.canvas.height = picture.height;
    gl.viewport(0, 0, picture.width, picture.height);
    if (gl.drawn !== picture) {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, picture);
      gl.drawn = picture;
    }
    gl.uniform3f(gl.uniforms.master, s.hue / 360, s.saturation / 100, s.lightness / 100);
    gl.uniform1f(gl.uniforms.vibrance, s.vibrance / 100);
    gl.uniform3fv(gl.uniforms.ranges, RANGES.flatMap(([id]) => [s[id].hue / 360, s[id].saturation / 100, s[id].lightness / 100]));
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    const jpeg = full ? source.jpeg : source.opaque;
    const blob = await new Promise((resolve) => gl.canvas.toBlob(resolve, jpeg ? "image/jpeg" : "image/png", full ? 0.95 : 0.9));
    if (!blob) throw new Error("not made");
    return new Uint8Array(await blob.arrayBuffer());
  };

  // A new image draws nothing in the file until Figma's loaded it, so it's
  // put on a small layer off the canvas first, until that draws something.
  // The layer goes again in the same undo step, so undo never brings it back
  const loaded = async (figma, hash) => {
    let loader = null;
    try {
      loader = figma.createRectangle();
      loader.name = "Ligma loading";
      figma.currentPage.insertChild(0, loader);
      loader.resizeWithoutConstraints(8, 8);
      loader.x = -200000;
      loader.y = -200000;
      loader.fills = [{ type: "IMAGE", imageHash: hash, scaleMode: "FILL" }];
      const canvas = Object.assign(document.createElement("canvas"), { width: 8, height: 8 });
      const context = canvas.getContext("2d", { willReadFrequently: true });
      const until = performance.now() + 2000;
      // A hidden tab doesn't draw, so exporting may wait as long as it is
      const late = new Promise((resolve) => setTimeout(resolve, 2000, null));
      while (performance.now() < until && !loader.removed) {
        const bytes = await Promise.race([loader.exportAsync({ format: "PNG", constraint: { type: "WIDTH", value: 8 } }), late]);
        if (!bytes) return;
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
        context.clearRect(0, 0, 8, 8);
        context.drawImage(bitmap, 0, 0, 8, 8);
        bitmap.close();
        const { data } = context.getImageData(0, 0, 8, 8);
        for (let i = 3; i < data.length; i += 4) if (data[i]) return;
      }
    } catch {
    } finally {
      try {
        if (loader && !loader.removed) loader.remove();
      } catch {}
    }
  };

  // Puts `s` on the paint `index` of `list` showing `target.hash`: made
  // from its original, or the original back with none. False if the paint
  // changed meanwhile, or once `stale`, and then nothing's changed
  const put = async (figma, target, list, index, s, full, stale = () => false) => {
    const { node } = target;
    const original = originalOf(node, target.hash);
    let hash = original;
    if (!isBlank(s)) {
      const bytes = await make(figma, original, s, full);
      if (stale()) return false;
      hash = figma.createImage(bytes).hash;
      await loaded(figma, hash);
      if (stale()) return false;
    }
    const paints = paintsOf(node, list);
    const was = target.hash;
    if (node.removed || paints[index]?.imageHash !== was) return false;
    node[list] = paints.map((paint, i) => (i === index ? { ...paint, imageHash: hash } : paint));
    target.hash = hash;
    const now = madeOn(node);
    delete now[was];
    if (hash !== original) now[hash] = { original, settings: s };
    keepMade(node, now);
    return hash;
  };

  // Each paint adjusted gets its original as a hidden paint right below it,
  // and one adjusted back to its original loses it. Not while its picker's
  // open, which would then show another paint
  const settle = (node) => {
    if (node.removed) return;
    const made = madeOn(node);
    for (const list of ["fills", "strokes"]) {
      const paints = [...paintsOf(node, list)];
      let changed = false;
      for (let i = 0; i < paints.length; i++) {
        const paint = paints[i];
        const entry = image(paint) && made[paint.imageHash];
        if (entry) {
          const below = paints[i - 1];
          if (!(image(below) && below.imageHash === entry.original && below.visible === false)) {
            paints.splice(i, 0, { ...paint, imageHash: entry.original, visible: false });
            i++;
            changed = true;
          }
        } else if (image(paint) && paint.visible === false && image(paints[i + 1]) && paints[i + 1].imageHash === paint.imageHash) {
          // The original below one now showing it again
          paints.splice(i, 1);
          i--;
          changed = true;
        }
      }
      if (changed) {
        try {
          node[list] = paints;
        } catch {}
      }
    }
  };

  // ---------- Changing them ----------

  // The paint whose picker is open, `{ list, index }`, as adjustments.rs
  // finds it: its row in the Fill or Stroke section is marked selected,
  // and names its section and paint in `data-onboarding-key`
  const ROW = '[data-testid="paint-panel-color-value-container"]';
  const KEY_PATTERN = /-(\d+)-(\d+)$/;
  const openPaint = () => {
    const row = document.querySelector(`${ROW}[class*="paintPanelValueSelected--"]`);
    const match = KEY_PATTERN.exec(row?.dataset.onboardingKey ?? "");
    if (!match) return null;
    const first = KEY_PATTERN.exec(document.querySelector(ROW)?.dataset.onboardingKey ?? "");
    const at = { list: first && first[1] !== match[1] ? "strokes" : "fills", index: Number(match[2]) };
    const nodes = api()?.currentPage.selection ?? [];
    const other = at.list === "fills" ? "strokes" : "fills";
    const has = (list) => nodes.some((node) => image(paintsOf(node, list)[at.index]));
    if (!has(at.list) && has(other)) at.list = other;
    return has(at.list) ? at : null;
  };
  const key = (figma, at) => `${figma.currentPage.selection.map((node) => node.id).join()}:${at.list}:${at.index}`;
  const targetsOf = (figma, at) => figma.currentPage.selection.filter((node) => image(paintsOf(node, at.list)[at.index])).map((node) => ({ node, hash: paintsOf(node, at.list)[at.index].imageHash }));

  // The edit under way: the paint whose picker is open in each selected
  // layer, and the settings the sliders show. `version` counts changes,
  // `fullVersion` the last one in at full size
  const begin = () => {
    const figma = api();
    const at = openPaint();
    if (!figma || !at) return null;
    const targets = targetsOf(figma, at);
    if (!targets.length) return null;
    const settings = settingsOf(targets[0].node, paintsOf(targets[0].node, at.list)[at.index]);
    return { figma, ...at, targets, settings, version: 0, fullVersion: 0 };
  };

  // Two lanes: small images while the sliders move, one at a time, always
  // of the latest settings; and full size ones, once a slider's let go or a
  // moment after a key, given up on as soon as the settings move again
  const change = (edit) => {
    edit.version++;
    edit.want = true;
    later(edit, edit.holding ? null : 250);
    run(edit);
  };
  const later = (edit, delay) => {
    clearTimeout(edit.idle);
    if (delay !== null) edit.idle = setTimeout(() => runFull(edit), delay);
  };
  const start = (edit) => {
    if (edit.started) return;
    edit.started = true;
    try {
      edit.figma.commitUndo();
    } catch {}
  };
  const fail = (edit) => {
    if (edit.failed) return;
    edit.failed = true;
    edit.figma.notify("Couldn't adjust the image", { timeout: 2000 });
  };
  const run = async (edit) => {
    if (edit.running || edit.failed) return;
    edit.running = true;
    busy(edit);
    start(edit);
    while (edit.want && !edit.failed) {
      edit.want = false;
      const version = edit.version;
      const s = structuredClone(edit.settings);
      for (const target of edit.targets) {
        try {
          await put(edit.figma, target, edit.list, edit.index, s, false, () => edit.fullVersion >= version);
        } catch {
          fail(edit);
          break;
        }
      }
    }
    edit.running = false;
    busy(edit);
    if (edit.ended) finish(edit);
  };
  const runFull = async (edit) => {
    if (edit.holding || edit.failed || edit.fullVersion === edit.version) return;
    if (edit.fullRunning) {
      edit.again = true;
      return;
    }
    edit.fullRunning = true;
    busy(edit);
    start(edit);
    const version = edit.version;
    const stale = () => edit.version !== version;
    const s = structuredClone(edit.settings);
    for (const target of edit.targets) {
      try {
        await put(edit.figma, target, edit.list, edit.index, s, true, stale);
      } catch {
        fail(edit);
      }
      if (stale() || edit.failed) break;
    }
    if (!stale() && !edit.failed) edit.fullVersion = version;
    edit.fullRunning = false;
    busy(edit);
    if (edit.ended) finish(edit);
    else if (edit.again) {
      edit.again = false;
      runFull(edit);
    }
  };
  // Once the picker's closed: full size images in, the originals kept below
  // and one undo step for it all
  const finish = (edit) => {
    edit.ended = true;
    edit.holding = false;
    clearTimeout(edit.idle);
    if (!edit.started || edit.running || edit.fullRunning || edit.settled) return;
    if (!edit.failed && edit.fullVersion !== edit.version) return runFull(edit);
    edit.settled = true;
    for (const { node } of edit.targets) settle(node);
    try {
      edit.figma.commitUndo();
    } catch {}
  };

  // Pasted or reset from adjustments.rs, with the picker closed: straight
  // to full size, the original kept below
  const apply = async (node, list, index, settings) => {
    const figma = api();
    const paint = paintsOf(node, list)[index];
    if (!figma || !image(paint)) return;
    const s = normal(settings);
    if (isBlank(s) && !madeOn(node)[paint.imageHash]) return;
    await put(figma, { node, hash: paint.imageHash }, list, index, s, true);
    if (!openPaint()) settle(node);
    else pending.add(node);
  };
  const pending = new Set();

  // ---------- The section under Figma's sliders ----------

  const PICKER = '[data-testid="paint-picker-v2-modal"]';
  const LABEL = '[class*="image_settings_modal--colorAdjustLabel"]';
  const SLIDER = '[role="slider"], input[type="range"]';
  const LABELS = ["Exposure", "Shadows"];
  const visible = (node) => node.getClientRects().length > 0;
  const around = (start, holds) => {
    for (let node = start.parentElement; node && node !== document.body; node = node.parentElement) {
      if (holds(node)) return node.getBoundingClientRect().height <= innerHeight * 0.8 ? node : null;
    }
    return null;
  };
  const ours = (node) => !!node.closest("[data-ligma-color]");
  // Figma's seven sliders, as adjustments.rs finds them
  const figmaSliders = (picker) => {
    const label = [...picker.querySelectorAll(LABEL)].find(visible);
    const box = label && around(label, (node) => node.querySelectorAll(LABEL).length >= 7);
    if (box) return box;
    for (const slider of picker.querySelectorAll(SLIDER)) {
      if (!ours(slider) && visible(slider)) {
        const box = around(slider, (node) => node.querySelectorAll(SLIDER).length >= 7);
        if (box) return box;
      }
    }
    const walker = document.createTreeWalker(picker, NodeFilter.SHOW_TEXT);
    for (let text = walker.nextNode(); text; text = walker.nextNode()) {
      const label = text.parentElement;
      if (text.data.trim() !== LABELS[0] || !label || !visible(label) || ours(label)) continue;
      const box = around(label, (node) => node.textContent.includes(LABELS[1]));
      if (box) return box;
    }
    return null;
  };
  // Where Figma's first label and its slider sit, so ours line up with
  // them: `{ left, column, right }` in pixels, or null if unsure
  const measure = (box, section) => {
    const label = [...box.querySelectorAll(LABEL)].find(visible);
    if (!label) return null;
    const row = label.getBoundingClientRect();
    const middle = row.top + row.height / 2;
    let track = [...box.querySelectorAll(SLIDER)].find((slider) => {
      const rect = slider.getBoundingClientRect();
      return rect.top <= middle && rect.bottom >= middle;
    });
    while (track && track !== box && track.getBoundingClientRect().width < 60) track = track.parentElement;
    if (!track || track === box) return null;
    const own = section.getBoundingClientRect();
    const rect = track.getBoundingClientRect();
    const sizes = { left: row.left - own.left, column: rect.left - row.left, right: own.right - rect.right };
    const fits = sizes.left >= 0 && sizes.left <= 32 && sizes.column >= 48 && sizes.column <= 120 && sizes.right >= 0 && sizes.right <= 32;
    return fits ? sizes : null;
  };

  const style = document.createElement("style");
  style.textContent = `
    [data-ligma-color] { position: relative; --left: 16px; --column: 88px; --right: 16px; margin-top: 8px; border-top: 1px solid var(--color-border); font-family: var(--font-family-default, Inter, sans-serif); font-size: 11px; line-height: 16px; letter-spacing: 0.005em; color: var(--color-text); -webkit-font-smoothing: antialiased; cursor: default; user-select: none; }
    [data-ligma-color] button { font: inherit; color: inherit; cursor: default; }
    [data-ligma-color] :focus-visible { outline: 1px solid var(--color-border-selected, #0d99ff); outline-offset: -1px; }
    [data-ligma-color] .head { display: flex; align-items: center; gap: 4px; height: 40px; padding: 0 var(--right) 0 calc(var(--left) - 4px); }
    [data-ligma-color] .toggle { display: flex; align-items: center; gap: 2px; flex: 1; min-width: 0; height: 24px; padding: 0 4px 0 0; border: 0; border-radius: 5px; background: none; font-weight: 550; text-align: left; }
    [data-ligma-color] .toggle span { flex: none; white-space: nowrap; }
    [data-ligma-color] .toggle .title { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
    [data-ligma-color] .toggle svg { flex: none; color: var(--color-icon-secondary); transition: transform 160ms cubic-bezier(0.2, 0, 0, 1); }
    [data-ligma-color] .toggle:hover svg { color: var(--color-icon); }
    [data-ligma-color] .toggle[aria-expanded="true"] svg { transform: rotate(90deg); }
    [data-ligma-color] .tag { margin-left: 4px; padding: 0 4px; border-radius: 4px; box-shadow: inset 0 0 0 1px var(--color-border); color: var(--color-text-secondary); font-weight: 450; }
    [data-ligma-color]::before { content: ""; position: absolute; top: -1px; left: 0; right: 0; height: 1px; background: linear-gradient(90deg, transparent, var(--color-bg-brand, #0d99ff), transparent) no-repeat; background-size: 40% 100%; opacity: 0; transition: opacity 120ms; animation: ligma-color-busy 900ms linear infinite paused; }
    [data-ligma-color].busy::before { opacity: 1; transition-delay: 160ms; animation-play-state: running; }
    @keyframes ligma-color-busy { from { background-position: -70% 0; } to { background-position: 170% 0; } }
    @media (prefers-reduced-motion: reduce) { [data-ligma-color]::before { animation: none; background: var(--color-bg-brand, #0d99ff); } [data-ligma-color] .toggle svg { transition: none; } }
    [data-ligma-color] .reset { flex: none; height: 24px; margin-right: -6px; padding: 0 6px; border: 0; border-radius: 5px; background: none; color: var(--color-text-secondary); }
    [data-ligma-color] .reset:hover { background: var(--color-bg-hover); color: var(--color-text); }
    [data-ligma-color] .body { padding: 0 var(--right) 8px var(--left); }
    [data-ligma-color] .row { display: grid; grid-template-columns: var(--column) 1fr; align-items: center; height: 32px; }
    [data-ligma-color] .label { color: var(--color-text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    [data-ligma-color] .caption { display: flex; align-items: center; justify-content: space-between; height: 32px; margin-top: 4px; }
    [data-ligma-color] .caption b { font-weight: 550; }
    [data-ligma-color] .caption span { color: var(--color-text-secondary); }
    [data-ligma-color] .track { position: relative; height: 16px; border-radius: 8px; background: var(--color-bg-secondary); box-shadow: inset 0 0 0 1px var(--color-border); touch-action: none; outline: 0; }
    [data-ligma-color] .track .fill { position: absolute; top: 0; bottom: 0; border-radius: 8px; background: var(--color-bg-brand, #0d99ff); }
    [data-ligma-color] .track .middle { position: absolute; top: 6px; left: calc(50% - 2px); width: 4px; height: 4px; border-radius: 2px; background: var(--color-icon-tertiary, #ffffff66); }
    [data-ligma-color] .track.changed .middle { background: var(--color-text-onbrand, #fff); }
    [data-ligma-color] .track .knob { position: absolute; top: 0; width: 16px; height: 16px; margin-left: -8px; border-radius: 8px; background: #fff; box-shadow: 0 0 0 1px rgb(0 0 0 / 0.2), 0 1px 3px rgb(0 0 0 / 0.3); }
    [data-ligma-color] .track.changed .knob { box-shadow: inset 0 0 0 3px var(--color-bg-brand, #0d99ff), 0 1px 3px rgb(0 0 0 / 0.3); }
    [data-ligma-color] .track:focus-visible { outline: 0; }
    [data-ligma-color] .track:focus-visible .knob { outline: 1px solid var(--color-border-selected, #0d99ff); outline-offset: 1px; }
    [data-ligma-color] .ranges { display: flex; justify-content: space-between; align-items: center; height: 32px; margin: 0 -4px; }
    [data-ligma-color] .range { position: relative; display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 5px; background: none; }
    [data-ligma-color] .range:hover { background: var(--color-bg-hover); }
    [data-ligma-color] .range[aria-checked="true"] { box-shadow: inset 0 0 0 1px var(--color-border-selected, #0d99ff); }
    [data-ligma-color] .range span { width: 12px; height: 12px; border-radius: 6px; box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.15); }
    [data-ligma-color] .range.changed::after { content: ""; position: absolute; top: 2px; right: 2px; width: 4px; height: 4px; border-radius: 2px; background: var(--color-bg-brand, #0d99ff); }
    [data-ligma-color] .note { margin: 4px 0 0; color: var(--color-text-secondary); }`;

  const element = (tag, props = {}, ...children) => {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children);
    return node;
  };
  const CHEVRON = '<svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M6.5 5L9.5 8L6.5 11" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/></svg>';

  // A slider as Figma's in the picker: centered at 0, filled from the
  // middle with round ends, double-click to reset. `change(value, done)`,
  // `done` once it's let go
  const slider = (label, limit, get, change) => {
    const track = element("div", { className: "track", tabIndex: 0 });
    track.setAttribute("role", "slider");
    track.setAttribute("aria-label", label);
    track.setAttribute("aria-valuemin", String(-limit));
    track.setAttribute("aria-valuemax", String(limit));
    const fill = track.appendChild(element("div", { className: "fill" }));
    track.appendChild(element("div", { className: "middle" }));
    const knob = track.appendChild(element("div", { className: "knob" }));
    const valueAt = (x) => {
      const box = track.getBoundingClientRect();
      const place = Math.min(1, Math.max(0, (x - box.left - 8) / (box.width - 16)));
      return Math.round((place * 2 - 1) * limit);
    };
    track.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      track.focus();
      track.setPointerCapture(event.pointerId);
      change(valueAt(event.clientX), false);
      const move = (event) => change(valueAt(event.clientX), false);
      track.addEventListener("pointermove", move);
      track.addEventListener(
        "lostpointercapture",
        () => {
          track.removeEventListener("pointermove", move);
          change(get(), true);
        },
        { once: true },
      );
    });
    track.addEventListener("dblclick", () => change(0, true));
    track.addEventListener("keydown", (event) => {
      const step = event.shiftKey ? 10 : 1;
      const by = { ArrowRight: step, ArrowUp: step, ArrowLeft: -step, ArrowDown: -step }[event.key];
      if (by === undefined) return;
      event.preventDefault();
      event.stopPropagation();
      change(Math.max(-limit, Math.min(limit, get() + by)), true);
    });
    const row = element("div", { className: "row" }, element("span", { className: "label", textContent: label }), track);
    row.show = () => {
      const value = get();
      const place = (value / limit + 1) / 2;
      knob.style.left = `calc(8px + (100% - 16px) * ${place})`;
      // Out to the knob's middle and round around the track's, as Figma's
      const [from, to] = [Math.min(place, 0.5), Math.max(place, 0.5)];
      fill.hidden = value === 0;
      fill.style.left = `calc((100% - 16px) * ${from})`;
      fill.style.width = `calc((100% - 16px) * ${to - from} + 16px)`;
      track.classList.toggle("changed", value !== 0);
      track.setAttribute("aria-valuenow", String(value));
    };
    return row;
  };

  // Whether the section's open, and the color range shown, kept for every
  // picker
  const OPEN = "ligma-color-open";
  const stored = (name) => {
    try {
      return localStorage.getItem(name);
    } catch {
      return null;
    }
  };
  let range = "all";

  // Made each time a picker opens on an image
  let ui = null;
  const busy = (edit) => {
    if (ui?.edit === edit) ui.section.classList.toggle("busy", !!(edit.running || edit.fullRunning));
  };
  const build = (shown) => {
    const section = element("div");
    section.dataset.ligmaColor = "section";
    const id = `ligma-color-${Date.now()}`;

    const toggle = element("button", { type: "button", className: "toggle", title: "Ligma's own adjustments, not Figma's" });
    toggle.setAttribute("aria-controls", id);
    toggle.insertAdjacentHTML("afterbegin", CHEVRON);
    toggle.append(element("span", { className: "title", textContent: "Vibrance and hue" }), element("span", { className: "tag", textContent: "Ligma" }));
    const reset = element("button", { type: "button", className: "reset", textContent: "Reset" });
    const head = element("div", { className: "head" }, toggle, reset);

    const body = element("div", { className: "body", id });
    body.setAttribute("role", "group");
    body.setAttribute("aria-label", "Vibrance and hue");
    section.append(head, body);

    const settings = () => ui.edit?.settings ?? ui.shown;
    // A value's place in the settings: a range's are in its own, All's at
    // the top level
    const path = (name) => (name === "vibrance" || range === "all" ? [name] : [range, name]);
    const read = (s, [a, b]) => (b ? s[a][b] : s[a]);
    const rows = [];
    const add = (label, limit, name) => {
      const row = slider(label, limit, () => read(settings(), path(name)), (value, done) => set(path(name), value, done));
      rows.push(row);
      return row;
    };
    const set = ([a, b], value, done) => {
      const edit = ui.edit ?? (ui.edit = begin());
      if (!edit) return;
      edit.holding = !done;
      if (read(edit.settings, [a, b]) !== value) {
        if (b) edit.settings[a][b] = value;
        else edit.settings[a] = value;
        change(edit);
      } else if (done) later(edit, 150);
      show();
    };

    const caption = element("span");
    const ranges = element("div", { className: "ranges" });
    ranges.setAttribute("role", "radiogroup");
    ranges.setAttribute("aria-label", "Colors");
    const choices = [["all", "All colors", null], ...RANGES];
    const pick = (id, focus) => {
      range = id;
      show();
      if (focus) ranges.querySelector(`[data-range="${id}"]`).focus();
    };
    for (const [id, name, hue] of choices) {
      const button = ranges.appendChild(element("button", { type: "button", className: "range", title: name }, element("span")));
      button.firstChild.style.background = hue === null ? "conic-gradient(from 90deg, #f33, #fd3, #3d5, #3de, #36f, #e3e, #f33)" : `hsl(${hue} 85% 55%)`;
      button.setAttribute("role", "radio");
      button.setAttribute("aria-label", name);
      button.dataset.range = id;
      button.addEventListener("click", () => pick(id));
      button.addEventListener("keydown", (event) => {
        const by = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
        if (by === undefined) return;
        event.preventDefault();
        event.stopPropagation();
        const at = choices.findIndex(([choice]) => choice === range);
        pick(choices[(at + by + choices.length) % choices.length][0], true);
      });
    }
    body.append(
      add("Vibrance", 100, "vibrance"),
      element("div", { className: "caption" }, element("b", { textContent: "Hue/Saturation" }), caption),
      ranges,
      add("Hue", 180, "hue"),
      add("Saturation", 100, "saturation"),
      add("Lightness", 100, "lightness"),
      element("p", { className: "note", textContent: "Made by Ligma as a new image, with the original kept hidden below it" }),
    );

    reset.addEventListener("click", () => {
      const edit = ui.edit ?? (ui.edit = begin());
      if (!edit) return;
      edit.holding = false;
      edit.settings = blank();
      change(edit);
      show();
    });
    toggle.addEventListener("click", () => {
      ui.open = !ui.open;
      try {
        localStorage.setItem(OPEN, ui.open ? "1" : "0");
      } catch {}
      show();
      if (ui.open) warm();
    });

    const show = () => {
      const s = settings();
      const blankNow = isBlank(s);
      toggle.setAttribute("aria-expanded", String(ui.open));
      body.hidden = !ui.open;
      reset.hidden = blankNow;
      caption.textContent = choices.find(([id]) => id === range)[1];
      for (const button of ranges.children) {
        const id = button.dataset.range;
        button.setAttribute("aria-checked", String(id === range));
        button.tabIndex = id === range ? 0 : -1;
        button.classList.toggle("changed", changed(id === "all" ? s : s[id]));
      }
      for (const row of rows) row.show();
    };
    return { section, show, edit: null, shown, open: stored(OPEN) === "1" || !isBlank(shown) };
  };

  // The original decoded and the GPU set up ahead of the first change
  const warm = () => {
    const figma = api();
    const at = openPaint();
    const target = figma && at && targetsOf(figma, at)[0];
    if (!target) return;
    setTimeout(() => {
      gpu();
      sourceOf(figma, originalOf(target.node, target.hash)).catch(() => {});
    });
  };

  const decorate = () => {
    const picker = document.querySelector(PICKER);
    const figma = state.on && api();
    const onImage = !!figma && !!picker?.querySelector('fieldset[value="IMAGE"]');
    const box = !onImage ? null : ui?.box?.isConnected && picker.contains(ui.box) ? ui.box : figmaSliders(picker);
    const at = box ? openPaint() : null;
    const nowKey = at ? key(figma, at) : null;
    // The picker closed, or moved to another paint or layer
    if (ui && (!at || ui.key !== nowKey)) {
      if (ui.edit) finish(ui.edit);
      ui.section.remove();
      ui = null;
    }
    if (!at) {
      for (const node of pending) {
        pending.delete(node);
        settle(node);
      }
      return;
    }
    if (!style.isConnected) document.head.append(style);
    if (!ui) {
      const target = targetsOf(figma, at)[0];
      ui = build(target ? settingsOf(target.node, paintsOf(target.node, at.list)[at.index]) : blank());
      ui.key = nowKey;
      if (ui.open) warm();
    }
    const moved = box.nextElementSibling !== ui.section;
    if (moved) box.after(ui.section);
    if (moved || ui.box !== box) {
      ui.box = box;
      const sizes = measure(box, ui.section);
      for (const name of ["left", "column", "right"]) ui.section.style.setProperty(`--${name}`, sizes ? `${sizes[name]}px` : "");
    }
    ui.show();
  };

  let queued = false;
  new MutationObserver((records) => {
    if (queued || records.every((record) => record.target.closest?.("[data-ligma-color]"))) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      try {
        decorate();
      } catch {}
    });
  }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] });

  window.__ligmaColor = {
    // For adjustments.rs: what a paint has, and putting it on another
    settingsOf: (node, paint) => (state.on ? settingsOf(node, paint) : null),
    apply: (node, list, index, settings) => (state.on ? apply(node, list, index, settings) : Promise.resolve()),
    set: (next) => {
      try {
        localStorage.setItem(KEY, JSON.stringify(next));
      } catch {}
      state = next;
      decorate();
    },
  };
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        decorate();
      }
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
})()"##;

/// The setting,
static ON: AtomicBool = AtomicBool::new(true);
/// and when it last changed, for `PAGE_JS` to tell it from an older one in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    let (on, rev) = (ON.load(Ordering::Relaxed), REV.load(Ordering::Relaxed));
    format!(r#"{{"on":{on},"rev":{rev}}}"#)
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.color_adjustments, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_COLOR__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.color_adjustments, Ordering::Relaxed) == settings.color_adjustments {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaColor?.set({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
