//! Ligma adjustments (Settings > Images > Adjustments): Vibrance, Hue and
//! Hue/Saturation for image fills and strokes, which Figma's picker doesn't
//! have, in a Ligma tab beside its own seven sliders. On by default
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
//! the full size one follows a moment after it stops. Each new image is
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
  // fully within 15º of its hue and fading out to 45º
  const RANGES = [
    ["reds", "Reds", 0],
    ["yellows", "Yellows", 60],
    ["greens", "Greens", 120],
    ["cyans", "Cyans", 180],
    ["blues", "Blues", 240],
    ["magentas", "Magentas", 300],
  ];
  const blank = () => ({ vibrance: 0, hue: 0, ...Object.fromEntries(RANGES.map(([id]) => [id, { hue: 0, saturation: 0, lightness: 0 }])) });
  // Any shape saved or copied, as one with every value
  const normal = (s) => {
    const out = blank();
    if (!s || typeof s !== "object") return out;
    const number = (value, limit) => (Number.isFinite(value) ? Math.max(-limit, Math.min(limit, Math.round(value))) : 0);
    out.vibrance = number(s.vibrance, 100);
    out.hue = number(s.hue, 180);
    for (const [id] of RANGES) {
      out[id].hue = number(s[id]?.hue, 180);
      out[id].saturation = number(s[id]?.saturation, 100);
      out[id].lightness = number(s[id]?.lightness, 100);
    }
    return out;
  };
  const isBlank = (s) => !s.vibrance && !s.hue && RANGES.every(([id]) => !s[id].hue && !s[id].saturation && !s[id].lightness);

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

  // ---------- Making the image ----------

  // The picture's top row at the canvas's top, which WebGL counts from the
  // bottom. ImageBitmaps aren't flipped by UNPACK_FLIP_Y_WEBGL
  const VERTEX = `attribute vec2 p; varying vec2 uv; void main() { uv = vec2(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5); gl_Position = vec4(p, 0.0, 1.0); }`;
  const FRAGMENT = `
    precision highp float;
    uniform sampler2D image;
    uniform float hue;
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
      c.x = fract(c.x + hue + shift.x);
      c.y = shift.y < 0.0 ? c.y * (1.0 + shift.y) : 1.0 - pow(1.0 - c.y, 1.0 + 2.0 * shift.y);
      c.z = shift.z < 0.0 ? c.z * (1.0 + shift.z) : c.z + (1.0 - c.z) * shift.z;
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
    gl.uniforms = Object.fromEntries(["hue", "vibrance", "ranges"].map((name) => [name, gl.getUniformLocation(program, name)]));
    gl.drawn = null;
    return gl;
  };

  // Each original as pictures to make from, full size and small, the last
  // few kept
  const PREVIEW = 1024;
  const sources = new Map();
  const sourceOf = (figma, hash) => {
    if (!sources.has(hash)) {
      const made = (async () => {
        const bytes = await figma.getImageByHash(hash).getBytesAsync();
        const blob = new Blob([bytes]);
        const options = { premultiplyAlpha: "none", colorSpaceConversion: "none" };
        const full = await createImageBitmap(blob, options);
        const scale = Math.min(1, PREVIEW / Math.max(full.width, full.height));
        const small = scale < 1 ? await createImageBitmap(blob, { ...options, resizeWidth: Math.round(full.width * scale), resizeHeight: Math.round(full.height * scale), resizeQuality: "high" }) : full;
        // JPEGs stay JPEGs, quicker to make and as small as they were
        const jpeg = bytes[0] === 0xff && bytes[1] === 0xd8;
        return { full, small, jpeg };
      })();
      made.catch(() => sources.delete(hash));
      sources.set(hash, made);
      while (sources.size > 4) {
        const [oldest, picture] = sources.entries().next().value;
        sources.delete(oldest);
        picture.then(({ full, small }) => (full.close(), small.close())).catch(() => {});
      }
    }
    return sources.get(hash);
  };

  // The original `hash` with `s`, as image bytes, full size or small
  const make = async (figma, hash, s, full) => {
    const source = await sourceOf(figma, hash);
    const picture = full ? source.full : source.small;
    const gl = gpu();
    if (!gl) throw new Error("no WebGL");
    gl.canvas.width = picture.width;
    gl.canvas.height = picture.height;
    gl.viewport(0, 0, picture.width, picture.height);
    if (gl.drawn !== picture) {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, picture);
      gl.drawn = picture;
    }
    gl.uniform1f(gl.uniforms.hue, s.hue / 360);
    gl.uniform1f(gl.uniforms.vibrance, s.vibrance / 100);
    gl.uniform3fv(gl.uniforms.ranges, RANGES.flatMap(([id]) => [s[id].hue / 360, s[id].saturation / 100, s[id].lightness / 100]));
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    const blob = await new Promise((resolve) => gl.canvas.toBlob(resolve, source.jpeg ? "image/jpeg" : "image/png", 0.95));
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

  // Puts `s` on the paint `index` of `list` that showed `was`: made from
  // its original, or the original back with none. False if the paint's
  // changed since
  const put = async (figma, node, list, index, was, s, full) => {
    const made = madeOn(node);
    const original = made[was]?.original ?? was;
    let hash = original;
    if (!isBlank(s)) {
      hash = figma.createImage(await make(figma, original, s, full)).hash;
      await loaded(figma, hash);
    }
    const paints = paintsOf(node, list);
    if (node.removed || paints[index]?.imageHash !== was) return false;
    node[list] = paints.map((paint, i) => (i === index ? { ...paint, imageHash: hash } : paint));
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

  // The edit under way: the paint whose picker is open in each selected
  // layer, and the settings the sliders show
  let editing = null;
  const begin = () => {
    const figma = api();
    const at = openPaint();
    if (!figma || !at) return null;
    const targets = figma.currentPage.selection.filter((node) => image(paintsOf(node, at.list)[at.index])).map((node) => ({ node, hash: paintsOf(node, at.list)[at.index].imageHash }));
    if (!targets.length) return null;
    return { figma, ...at, targets, settings: settingsOf(targets[0].node, paintsOf(targets[0].node, at.list)[at.index]), key: key(figma, at) };
  };
  const key = (figma, at) => `${figma.currentPage.selection.map((node) => node.id).join()}:${at.list}:${at.index}`;

  // Moving: small images, one at a time, the latest settings next; then,
  // once it's still, the full size ones
  const change = (edit) => {
    edit.want = edit.want === "full" ? "full" : "small";
    clearTimeout(edit.idle);
    edit.idle = setTimeout(() => {
      edit.want = "full";
      run(edit);
    }, 350);
    run(edit);
  };
  const run = async (edit) => {
    if (edit.running) return;
    edit.running = true;
    if (!edit.started) {
      edit.started = true;
      try {
        edit.figma.commitUndo();
      } catch {}
    }
    while (edit.want) {
      const full = edit.want === "full";
      edit.want = null;
      const s = structuredClone(edit.settings);
      for (const target of edit.targets) {
        try {
          const hash = await put(edit.figma, target.node, edit.list, edit.index, target.hash, s, full);
          if (hash) target.hash = hash;
        } catch {
          edit.figma.notify("Couldn't adjust the image", { timeout: 2000 });
          edit.want = null;
          break;
        }
      }
      edit.full = full;
    }
    edit.running = false;
    if (edit.ended) finish(edit);
  };
  // Once the picker's closed: full size images in, the originals kept below
  // and one undo step for it all
  const finish = (edit) => {
    edit.ended = true;
    clearTimeout(edit.idle);
    if (edit.running) return;
    if (edit.started && !edit.full) {
      edit.want = "full";
      return run(edit);
    }
    if (!edit.started) return;
    for (const { node } of edit.targets) settle(node);
    try {
      edit.figma.commitUndo();
    } catch {}
  };

  // Pasted or reset from adjustments.rs, with the picker closed: straight
  // to full size, the original kept below. `at` is `{ list, index }`
  const apply = async (node, list, index, settings) => {
    const figma = api();
    const paint = paintsOf(node, list)[index];
    if (!figma || !image(paint)) return;
    const s = normal(settings);
    if (isBlank(s) && !madeOn(node)[paint.imageHash]) return;
    await put(figma, node, list, index, paint.imageHash, s, true);
    if (!openPaint()) settle(node);
    else pending.add(node);
  };
  const pending = new Set();

  // ---------- The Ligma tab in the image picker ----------

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
  // Figma's seven sliders, as adjustments.rs finds them
  const figmaSliders = (picker) => {
    const label = [...picker.querySelectorAll(LABEL)].find(visible);
    const box = label && around(label, (node) => node.querySelectorAll(LABEL).length >= 7);
    if (box) return box;
    for (const slider of picker.querySelectorAll(SLIDER)) {
      if (!slider.closest("[data-ligma-color]") && visible(slider)) {
        const box = around(slider, (node) => node.querySelectorAll(SLIDER).length >= 7);
        if (box) return box;
      }
    }
    const walker = document.createTreeWalker(picker, NodeFilter.SHOW_TEXT);
    for (let text = walker.nextNode(); text; text = walker.nextNode()) {
      const label = text.parentElement;
      if (text.data.trim() !== LABELS[0] || !label || !visible(label)) continue;
      const box = around(label, (node) => node.textContent.includes(LABELS[1]));
      if (box) return box;
    }
    return null;
  };

  const style = document.createElement("style");
  style.textContent = `
    [data-ligma-color] { font-family: var(--font-family-default, Inter, sans-serif); font-size: 11px; line-height: 16px; letter-spacing: 0.005em; color: var(--color-text); -webkit-font-smoothing: antialiased; }
    [data-ligma-color="tabs"] { display: flex; align-items: center; gap: 2px; padding: 0 16px 8px; }
    [data-ligma-color="tabs"] button { height: 24px; padding: 0 8px; border: 0; border-radius: 5px; background: none; color: var(--color-text-secondary); font: inherit; font-weight: 450; }
    [data-ligma-color="tabs"] button:hover { color: var(--color-text); }
    [data-ligma-color="tabs"] button[aria-selected="true"] { background: var(--color-bg-secondary); color: var(--color-text); font-weight: 550; }
    [data-ligma-color="tabs"] .reset { margin-left: auto; font-weight: 450; }
    [data-ligma-color="pane"] { padding: 0 16px 8px; }
    [data-ligma-color] .row { display: grid; grid-template-columns: 88px 1fr; align-items: center; height: 32px; }
    [data-ligma-color] .label { color: var(--color-text-secondary); white-space: nowrap; }
    [data-ligma-color] .caption { display: flex; align-items: center; height: 32px; margin-top: 4px; font-weight: 550; }
    [data-ligma-color] .track { position: relative; height: 16px; border-radius: 8px; background: var(--color-bg-secondary); box-shadow: inset 0 0 0 1px var(--color-border); cursor: default; touch-action: none; outline: 0; }
    [data-ligma-color] .track .fill { position: absolute; top: 0; bottom: 0; background: var(--color-bg-brand, #0d99ff); }
    [data-ligma-color] .track .middle { position: absolute; top: 6px; left: calc(50% - 2px); width: 4px; height: 4px; border-radius: 2px; background: var(--color-icon-tertiary, #ffffff66); }
    [data-ligma-color] .track.changed .middle { background: var(--color-text-onbrand, #fff); }
    [data-ligma-color] .track .knob { position: absolute; top: 0; width: 16px; height: 16px; margin-left: -8px; border-radius: 8px; background: #fff; box-shadow: 0 0 0 1px rgb(0 0 0 / 0.2), 0 1px 3px rgb(0 0 0 / 0.3); }
    [data-ligma-color] .track.changed .knob { box-shadow: inset 0 0 0 3px var(--color-bg-brand, #0d99ff), 0 1px 3px rgb(0 0 0 / 0.3); }
    [data-ligma-color] .track:focus-visible .knob { outline: 1px solid var(--color-border-selected, #0d99ff); outline-offset: 1px; }
    [data-ligma-color] .swatches { display: flex; justify-content: space-between; height: 32px; align-items: center; margin: 0 -6px; }
    [data-ligma-color] .swatch { position: relative; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 5px; background: none; display: grid; place-items: center; }
    [data-ligma-color] .swatch:hover { background: var(--color-bg-hover); }
    [data-ligma-color] .swatch[aria-checked="true"] { box-shadow: inset 0 0 0 1px var(--color-border-selected, #0d99ff); }
    [data-ligma-color] .swatch span { width: 12px; height: 12px; border-radius: 6px; box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.15); }
    [data-ligma-color] .swatch.changed::after { content: ""; position: absolute; top: 2px; right: 2px; width: 4px; height: 4px; border-radius: 2px; background: var(--color-bg-brand, #0d99ff); }
    [data-ligma-color] :focus-visible { outline: 1px solid var(--color-border-selected, #0d99ff); outline-offset: -1px; }`;

  const element = (tag, props = {}, ...children) => {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children);
    return node;
  };

  // A slider as Figma's in the picker: centered at 0, filled from the
  // middle, double-click to reset. `change(value, done)`
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
      track.addEventListener("lostpointercapture", () => {
        track.removeEventListener("pointermove", move);
        change(get(), true);
      }, { once: true });
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
      const [from, to] = [Math.min(place, 0.5), Math.max(place, 0.5)];
      fill.style.left = `calc(8px + (100% - 16px) * ${from})`;
      fill.style.width = `calc((100% - 16px) * ${to - from})`;
      track.classList.toggle("changed", value !== 0);
      track.setAttribute("aria-valuenow", String(value));
    };
    return row;
  };

  // Which tab shows, kept for every picker
  const TAB = "ligma-color-tab";
  let tab = "figma";
  try {
    tab = localStorage.getItem(TAB) === "ligma" ? "ligma" : "figma";
  } catch {}
  let range = "reds";

  // The tabs above the sliders and the Ligma ones, made once each time a
  // picker opens on an image
  let ui = null;
  const build = () => {
    const tabs = element("div");
    tabs.dataset.ligmaColor = "tabs";
    tabs.setAttribute("role", "tablist");
    const tabButton = (id, text) => {
      const button = tabs.appendChild(element("button", { type: "button", textContent: text }));
      button.setAttribute("role", "tab");
      button.dataset.tab = id;
      button.addEventListener("click", () => {
        tab = id;
        try {
          localStorage.setItem(TAB, id);
        } catch {}
        decorate();
      });
    };
    tabButton("figma", "Figma");
    tabButton("ligma", "Ligma");
    const reset = tabs.appendChild(element("button", { type: "button", className: "reset", textContent: "Reset" }));
    reset.addEventListener("click", () => {
      ui.edit ??= begin();
      if (!ui.edit) return;
      ui.edit.settings = blank();
      change(ui.edit);
      show();
    });

    const pane = element("div");
    pane.dataset.ligmaColor = "pane";
    pane.setAttribute("role", "tabpanel");
    const rows = [];
    const set = (path, value, done) => {
      const edit = ui.edit ?? (ui.edit = begin());
      if (!edit) return;
      const [a, b] = path;
      const before = b ? edit.settings[a][b] : edit.settings[a];
      if (before === value && !done) return;
      if (b) edit.settings[a][b] = value;
      else edit.settings[a] = value;
      if (before !== value) change(edit);
      show();
    };
    const get = (a, b) => {
      const s = ui.edit?.settings ?? ui.shown;
      return b ? s[a][b] : s[a];
    };
    // `a` a key, or for a range's sliders, the range shown
    const add = (label, limit, a, b) => {
      const path = () => (b ? [a(), b] : [a]);
      const row = slider(label, limit, () => get(...path()), (value, done) => set(path(), value, done));
      rows.push(row);
      return row;
    };
    const current = () => range;
    const swatches = element("div", { className: "swatches" });
    swatches.setAttribute("role", "radiogroup");
    swatches.setAttribute("aria-label", "Color range");
    for (const [id, name, hue] of RANGES) {
      const swatch = swatches.appendChild(element("button", { type: "button", className: "swatch", title: name }, element("span")));
      swatch.firstChild.style.background = `hsl(${hue} 85% 55%)`;
      swatch.setAttribute("role", "radio");
      swatch.setAttribute("aria-label", name);
      swatch.dataset.range = id;
      swatch.addEventListener("click", () => {
        range = id;
        show();
      });
    }
    const caption = element("div", { className: "caption" });
    pane.append(
      add("Vibrance", 100, "vibrance"),
      add("Hue", 180, "hue"),
      caption,
      swatches,
      add("Hue", 180, current, "hue"),
      add("Saturation", 100, current, "saturation"),
      add("Lightness", 100, current, "lightness"),
    );
    const show = () => {
      for (const button of tabs.querySelectorAll("[data-tab]")) button.setAttribute("aria-selected", String(button.dataset.tab === tab));
      const s = ui.edit?.settings ?? ui.shown;
      reset.hidden = tab !== "ligma" || isBlank(s);
      caption.textContent = `Hue/Saturation · ${RANGES.find(([id]) => id === range)[1]}`;
      for (const swatch of swatches.children) {
        const id = swatch.dataset.range;
        swatch.setAttribute("aria-checked", String(id === range));
        swatch.classList.toggle("changed", !!(s[id].hue || s[id].saturation || s[id].lightness));
      }
      for (const row of rows) row.show();
    };
    return { tabs, pane, show, edit: null, shown: blank() };
  };

  const decorate = () => {
    const picker = document.querySelector(PICKER);
    const figma = state.on && api();
    const onImage = !!figma && !!picker?.querySelector('fieldset[value="IMAGE"]');
    // Figma's, once hidden for the Ligma tab, can't be found by what shows
    const box = !onImage ? null : ui?.box?.isConnected && picker.contains(ui.box) ? ui.box : figmaSliders(picker);
    const at = box ? openPaint() : null;
    const nowKey = at ? key(figma, at) : null;
    // The picker closed, or moved to another paint or layer
    if (ui && (!at || ui.key !== nowKey)) {
      if (ui.edit) finish(ui.edit);
      ui.tabs.remove();
      ui.pane.remove();
      if (ui.box) ui.box.style.display = "";
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
      ui = build();
      ui.key = nowKey;
      const node = figma.currentPage.selection.find((node) => image(paintsOf(node, at.list)[at.index]));
      ui.shown = node ? settingsOf(node, paintsOf(node, at.list)[at.index]) : blank();
    }
    ui.box = box;
    if (box.previousElementSibling !== ui.tabs) box.before(ui.tabs);
    if (box.nextElementSibling !== ui.pane) box.after(ui.pane);
    const ligma = tab === "ligma";
    if (ligma && box.style.display !== "none") {
      // As tall as Figma's, so the picker doesn't jump
      ui.pane.style.minHeight = `${box.offsetHeight}px`;
      box.style.display = "none";
    } else if (!ligma && box.style.display === "none") box.style.display = "";
    ui.pane.hidden = !ligma;
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
