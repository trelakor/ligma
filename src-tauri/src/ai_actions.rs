//! AI actions in free projects (Settings > Features > Images): in files
//! from a free (Starter) project, the bar Figma shows above the toolbar
//! for a selected image keeps only Crop, and Ligma's Copy and Paste
//! adjustments (see adjustments.rs). Select area, Remove background, Edit
//! with prompt and the More menu, all AI actions, are hidden with their
//! divider. On by default
//!
//! In their place goes Ligma's own Remove background, which works on this
//! device (see background.rs), where it can. It hides the top image fill
//! of each selected layer and adds its cut-out as a new fill above it,
//! through Figma's console plugin API, `window.figma`, as one undo step.
//! On by default
//!
//! After it goes Ligma's own Vectorize, in place of the one in Figma's
//! More menu, which traces each selected layer on this device (see
//! vectorize.rs) into a frame of vectors right under it, as one undo step,
//! hiding the layer, as Figma's does. A bar like Figma's then recolors the
//! vectors: in full color, grayscale or black and white, keeping fewer
//! colors. On by default
//!
//! First of them goes Ligma's own Select object, in place of Select area,
//! which picks out objects in the selected image as Photoshop's Object
//! Selection does, with SAM on this device (see objects.rs): the object
//! under the pointer shows as it moves, a click picks it, Shift adds,
//! Alt (Option) takes away and a box picks the object in it. A brush
//! paints in what it missed or takes away what it overshot. Isolate puts
//! the picked objects in a new layer above, cropped to them, with their
//! edge moved and feathered as the bar says, as one undo step. On by
//! default, where the model runs
//!
//! A file's plan is `INITIAL_OPTIONS.plan_tier` ("starter" when free) while
//! the page is still on the file it loaded with, and otherwise the "Free"
//! badge Figma shows by the file's name. The bar is told by Crop's icon,
//! not by its text. Names as seen on 2026-10-29

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_FREE_AI_ACTIONS__` is `{ on, local,
/// vectorize, objects, rev }`: the settings when the tab was made, `local`
/// and `objects` only where this device can remove backgrounds and select
/// objects. A page loaded later in the same tab may find a newer one in
/// `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-free-ai-actions";
  let state = __LIGMA_FREE_AI_ACTIONS__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  const BADGE = '[class*="paid_status_badge_base--badge"]';
  const fileKey = () => /^\/(?:design|file|board|slides|proto)\/([^/]+)/.exec(location.pathname)?.[1];
  const free = () => {
    const file = window.INITIAL_OPTIONS?.editing_file;
    if (file?.key && file.key === fileKey()) return window.INITIAL_OPTIONS.plan_tier === "starter";
    return /^free$/i.test(document.querySelector(BADGE)?.textContent.trim() ?? "");
  };

  // The plugin API, if the file can be edited
  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.editorType === "figma" ? figma : null;
    } catch {
      return null;
    }
  };
  // Text with mixed fills has none to change
  const fillsOf = (node) => {
    try {
      return Array.isArray(node.fills) ? node.fills : [];
    } catch {
      return [];
    }
  };
  const plural = (count) => `${count} image${count === 1 ? "" : "s"}`;
  // Icons drawn in Figma's icon color
  const STROKE = 'stroke="var(--fpl-icon-color, var(--color-icon))"';
  const FILL = 'fill="var(--fpl-icon-color, var(--color-icon))"';

  // Hands an image to Ligma as a download, for `kind` ("background" or
  // "vectorize"), with `suffix` after the job in its name, and resolves to
  // what Ligma sends back: the cut-out as a PNG in base64, or the traced
  // shapes as JSON. Rejects with the reason there's none, such
  // as "subject" when there's nothing to keep, "cancelled" when the model
  // wasn't downloaded or "failed". `onProgress` gets what Ligma says while
  // it downloads the model (see handoff.rs). Job numbers start from the
  // time, so a result meant for the page before a reload can't land on a
  // new job
  const jobs = new Map();
  let lastJob = Date.now() * 1000;
  const handOff = (kind, bytes, onProgress, suffix = "") =>
    new Promise((resolve, reject) => {
      const job = ++lastJob;
      // Typed, or WebView2 sniffs it and names it as text
      const url = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
      let timer;
      const finish = () => {
        clearTimeout(timer);
        jobs.delete(job);
        URL.revokeObjectURL(url);
      };
      // A tab frozen in the meantime never hears back. On Windows the
      // model takes a while on the CPU, and longer the first time
      const wait = () => {
        clearTimeout(timer);
        timer = setTimeout(() => {
          finish();
          reject("failed");
        }, 300000);
      };
      wait();
      jobs.set(job, {
        progress: (text) => {
          wait();
          onProgress(text);
        },
        done: (result) => {
          finish();
          resolve(result);
        },
        failed: (reason) => {
          finish();
          reject(reason);
        },
      });
      const link = Object.assign(document.createElement("a"), { href: url, download: `ligma-${kind}-${job}${suffix}`, hidden: true });
      document.documentElement.append(link);
      link.click();
      link.remove();
    });
  // From Ligma, with a job's progress and result
  window.__ligmaJob = {
    progress: (job, text) => jobs.get(job)?.progress(text),
    done: (job, result) => jobs.get(job)?.done(result),
    failed: (job, reason) => jobs.get(job)?.failed(reason),
  };

  // Figma's canvas, the largest
  const mainCanvas = () =>
    [...document.querySelectorAll("canvas")].reduce(
      (best, canvas) => (canvas.offsetWidth * canvas.offsetHeight > (best ? best.offsetWidth * best.offsetHeight : 0) ? canvas : best),
      null,
    );

  // A shimmer over each of `nodes` on the canvas, as Figma shows while its
  // own AI actions work, white for Remove background and blue for
  // Vectorize and Select object, kept on them as the canvas pans and zooms.
  // Returns what stops it
  const SHIMMER = "data-ligma-shimmer";
  const shimmer = (figma, nodes, kind) => {
    const canvas = mainCanvas();
    if (!canvas) return () => {};
    const layer = document.createElement("div");
    layer.setAttribute(SHIMMER, kind);
    canvas.after(layer);
    const boxes = nodes.map(() => layer.appendChild(document.createElement("div")));
    let frame;
    const place = () => {
      Object.assign(layer.style, {
        left: `${canvas.offsetLeft}px`,
        top: `${canvas.offsetTop}px`,
        width: `${canvas.offsetWidth}px`,
        height: `${canvas.offsetHeight}px`,
      });
      try {
        const { bounds } = figma.viewport;
        const zoom = figma.viewport.zoom;
        nodes.forEach((node, i) => {
          const box = boxes[i];
          if (node.removed) {
            box.hidden = true;
            return;
          }
          const [[a, c, e], [b, d, f]] = node.absoluteTransform;
          const radius = typeof node.cornerRadius === "number" ? node.cornerRadius * zoom : 0;
          box.hidden = false;
          Object.assign(box.style, {
            width: `${node.width * zoom}px`,
            height: `${node.height * zoom}px`,
            borderRadius: `${radius}px`,
            transform: `matrix(${a}, ${b}, ${c}, ${d}, ${(e - bounds.x) * zoom}, ${(f - bounds.y) * zoom})`,
          });
        });
      } catch {}
      frame = requestAnimationFrame(place);
    };
    place();
    return () => {
      cancelAnimationFrame(frame);
      layer.classList.add("done");
      setTimeout(() => layer.remove(), 200);
    };
  };

  // Figma's toast, kept up while an action works, with what it's doing
  const toast = (figma) => {
    let current;
    const hide = () => {
      try {
        current?.cancel();
      } catch {}
    };
    const show = (text) => {
      hide();
      current = figma.notify(text, { timeout: Infinity });
    };
    return { show, hide };
  };

  // Hides the top image fill of each selected layer and adds its cut-out
  // above it, as one undo step. Each image is cut out once, however many
  // layers show it
  let busy = false;
  const removeBackground = async () => {
    const figma = api();
    if (!figma || busy) return;
    const targets = [];
    for (const node of figma.currentPage.selection) {
      const fills = fillsOf(node);
      const image = (paint) => paint?.type === "IMAGE" && !!paint.imageHash;
      let index = fills.findLastIndex((paint) => image(paint) && paint.visible !== false);
      if (index < 0) index = fills.findLastIndex(image);
      if (index >= 0) targets.push({ node, index, hash: fills[index].imageHash });
    }
    if (!targets.length) {
      figma.notify("No images selected", { timeout: 2000 });
      return;
    }
    busy = true;
    tidy();
    const stopShimmer = shimmer(figma, targets.map((target) => target.node), "background");
    const { show, hide } = toast(figma);
    show("Removing background…");
    const cut = new Map();
    let missing = 0;
    let failed = 0;
    let cancelled = false;
    for (const hash of new Set(targets.map((target) => target.hash))) {
      try {
        const bytes = await figma.getImageByHash(hash).getBytesAsync();
        const base64 = await handOff("background", bytes, show);
        cut.set(hash, figma.createImage(Uint8Array.from(atob(base64), (char) => char.charCodeAt(0))).hash);
      } catch (reason) {
        if (reason === "cancelled") {
          cancelled = true;
          break;
        }
        if (reason === "subject") missing++;
        else failed++;
      }
    }
    let images = 0;
    for (const { node, index, hash } of targets) {
      if (!cut.has(hash)) continue;
      try {
        // Unless the layer changed in the meantime
        const fills = fillsOf(node);
        if (fills[index]?.imageHash !== hash) continue;
        // The original stays, hidden, under the cut-out
        const original = fills[index];
        node.fills = [
          ...fills.slice(0, index),
          { ...original, visible: false },
          { ...original, imageHash: cut.get(hash), visible: true },
          ...fills.slice(index + 1),
        ];
        images++;
      } catch {
        failed++;
      }
    }
    if (images) {
      try {
        figma.commitUndo();
      } catch {}
    }
    busy = false;
    stopShimmer();
    tidy();
    hide();
    if (cancelled && !images) return;
    const message = images
      ? `Background removed${targets.length > 1 ? ` from ${plural(images)}` : ""}${missing ? `, no subject in ${missing}` : ""}${failed ? `, ${failed} failed` : ""}`
      : missing && !failed
        ? "No subject found"
        : "Couldn't remove the background";
    figma.notify(message, { timeout: 3000 });
  };

  // How wide a pixel of the layer's top image is, drawn `scale` times as
  // large: more than 1 for a small image drawn large, whose steps along
  // edges Ligma then smooths as such
  const pixelSize = async (figma, node, scale) => {
    const paint = fillsOf(node).findLast((fill) => fill.type === "IMAGE" && fill.imageHash && fill.visible !== false);
    try {
      const size = figma.getImageByHash(paint.imageHash).getSizeAsync();
      const { width, height } = await Promise.race([size, new Promise((_, reject) => setTimeout(reject, 2000))]);
      if (paint.scaleMode === "TILE") return Math.max(1, scale * (paint.scalingFactor ?? 1));
      // Fitted, the image fits inside the layer, and otherwise covers it
      const fit = paint.scaleMode === "FIT" ? Math.min : Math.max;
      return Math.max(1, scale * fit(node.width / width, node.height / height));
    } catch {
      return 1;
    }
  };

  // The layer as Figma draws it, unturned, `scale` times as large, as a PNG
  const drawn = async (figma, node, scale) => {
    const settings = { format: "PNG", constraint: { type: "SCALE", value: scale }, useAbsoluteBounds: true };
    const [[a, c], [b, d]] = node.absoluteTransform;
    if (Math.abs(b) < 1e-6 && Math.abs(c) < 1e-6 && a > 0 && d > 0) return node.exportAsync(settings);
    // Turned or flipped, here or by a parent: a copy that isn't
    const copy = node.clone();
    try {
      figma.currentPage.appendChild(copy);
      copy.relativeTransform = [
        [1, 0, 0],
        [0, 1, 0],
      ];
      return await copy.exportAsync(settings);
    } finally {
      copy.remove();
    }
  };

  // How large to draw a layer to trace it or find objects in it: up to 4x
  // for small layers, down to 1024 pixels on the longer side for large ones
  const workingScale = (node) => Math.min(4, 1024 / Math.max(node.width, node.height, 1));

  // The layer drawn to trace it, with how wide a pixel of its image is in it
  const render = async (figma, node) => {
    const scale = workingScale(node);
    const pixel = await pixelSize(figma, node, scale);
    return { bytes: await drawn(figma, node, scale), pixel };
  };

  // The SVG Ligma traced from `node`, as a frame of vectors in its place,
  // as Figma's Vectorize leaves it: right under the layer, which is
  // hidden, named after it, with a hidden white fill
  const SVG_SIZE = /^<svg([^>]*?) width="[\d.]+" height="[\d.]+"/;
  const place = (figma, node, svg) => {
    const frame = figma.createNodeFromSvg(svg.replace(SVG_SIZE, `<svg$1 width="${node.width}" height="${node.height}"`));
    try {
      const parent = node.parent;
      parent.insertChild(parent.children.indexOf(node), frame);
      frame.name = `${node.name} [Vectorized]`;
      frame.fills = [{ type: "SOLID", color: { r: 1, g: 1, b: 1 }, visible: false }];
      frame.clipsContent = true;
      if (node.layoutPositioning === "ABSOLUTE") frame.layoutPositioning = "ABSOLUTE";
      frame.relativeTransform = node.relativeTransform;
      if (node.constraints) frame.constraints = node.constraints;
      for (const vector of frame.findAll((child) => child.type === "VECTOR")) {
        vector.constraints = { horizontal: "SCALE", vertical: "SCALE" };
      }
      node.visible = false;
      return frame;
    } catch (error) {
      frame.remove();
      throw error;
    }
  };

  // Traces each selected layer with an image into vectors, as one undo
  // step, and opens the recolor bar for them
  const vectorize = async () => {
    const figma = api();
    if (!figma || busy) return;
    const image = (paint) => paint?.type === "IMAGE" && !!paint.imageHash && paint.visible !== false;
    const nodes = figma.currentPage.selection.filter((node) => fillsOf(node).some(image));
    if (!nodes.length) {
      figma.notify("No images selected", { timeout: 2000 });
      return;
    }
    busy = true;
    closeRecolor();
    tidy();
    const stopShimmer = shimmer(figma, nodes, "vectorize");
    const { show, hide } = toast(figma);
    show("Vectorizing…");
    const frames = [];
    // How many pixels of each fill show, for the recolor bar
    const areas = new Map();
    let empty = 0;
    let failed = 0;
    for (const node of nodes) {
      try {
        const { bytes, pixel } = await render(figma, node);
        const traced = JSON.parse(await handOff("vectorize", bytes, show, `@${Math.round(pixel * 100)}`));
        // Unless it was deleted in the meantime
        if (node.removed) continue;
        frames.push(place(figma, node, traced.svg));
        for (const [fill, area] of Object.entries(traced.areas)) areas.set(fill, (areas.get(fill) ?? 0) + area);
      } catch (reason) {
        if (reason === "empty") empty++;
        else failed++;
      }
    }
    if (frames.length) {
      try {
        figma.commitUndo();
      } catch {}
      figma.currentPage.selection = frames;
    }
    busy = false;
    stopShimmer();
    tidy();
    hide();
    // The recolor bar says it worked, where Figma's toast would cover it
    if (frames.length) openRecolor(figma, frames, areas);
    if (frames.length === nodes.length) return;
    const message = frames.length
      ? `Vectorized ${plural(frames.length)}${empty ? `, nothing to trace in ${empty}` : ""}${failed ? `, ${failed} failed` : ""}`
      : empty && !failed
        ? "Nothing to vectorize"
        : "Couldn't vectorize the image";
    figma.notify(message, { timeout: 3000 });
  };

  // An sRGB color, channels 0 to 1, in CIELAB, where how far apart two
  // colors are matches how different they look
  const lab = ({ r, g, b }) => {
    const linear = (value) => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4);
    const [lr, lg, lb] = [r, g, b].map(linear);
    const f = (t) => (t > 216 / 24389 ? Math.cbrt(t) : ((24389 / 27) * t + 16) / 116);
    const x = f((0.4124 * lr + 0.3576 * lg + 0.1805 * lb) / 0.95047);
    const y = f(0.2126 * lr + 0.7152 * lg + 0.0722 * lb);
    const z = f((0.0193 * lr + 0.1192 * lg + 0.9505 * lb) / 1.08883);
    return [116 * y - 16, 500 * (x - y), 200 * (y - z)];
  };

  // The colors of `palette` ({ key, color, area } for each) merged down to
  // each count from all of them to one: for each count, a map from each
  // key to the color it's shown in. The two groups that cost least to
  // merge go first, by Ward's method in CIELAB: those that look most alike
  // and cover least. Area counts by its square root, so a small accent
  // that stands out isn't merged away before large areas that look alike.
  // A group shows its color that covers most, as it is
  const reductions = (palette) => {
    const maps = [];
    let groups = palette.map((entry) => ({ members: [entry], weight: Math.sqrt(Math.max(entry.area, 1)), lab: lab(entry.color) }));
    for (;;) {
      const map = new Map();
      for (const { members } of groups) {
        const top = members.reduce((a, b) => (b.area > a.area ? b : a));
        for (const member of members) map.set(member.key, top.color);
      }
      maps[groups.length] = map;
      if (groups.length === 1) return maps;
      let best = null;
      for (let i = 0; i < groups.length; i++) {
        for (let j = i + 1; j < groups.length; j++) {
          const [a, b] = [groups[i], groups[j]];
          const distance = a.lab.reduce((sum, value, c) => sum + (value - b.lab[c]) ** 2, 0);
          const cost = ((a.weight * b.weight) / (a.weight + b.weight)) * distance;
          if (!best || cost < best.cost) best = { i, j, cost };
        }
      }
      const [a, b] = [groups[best.i], groups[best.j]];
      const weight = a.weight + b.weight;
      groups = groups.filter((_, index) => index !== best.j);
      groups[best.i] = { members: [...a.members, ...b.members], weight, lab: a.lab.map((value, c) => (value * a.weight + b.lab[c] * b.weight) / weight) };
    }
  };

  // Figma leaves a bar after its Vectorize to recolor the result: its
  // colors in full, in grays or in black and white, and how many of them
  // to keep. Ligma's is the same, in the place Figma shows its bars, with
  // Figma's own classes for it. Each change is an undo step, and the
  // original colors are kept, so they come back in full
  const RECOLOR = "data-ligma-recolor";
  // On <html> while it shows, to hide Figma's own bars
  const RECOLORING = "data-ligma-recoloring";
  const MODES = [
    ["COLOR", "Full color"],
    ["GRAYSCALE", "Grayscale"],
    ["BLACK_AND_WHITE", "Black and white"],
  ];
  const CHECK = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" d="m7.5 12.5 3 3 6-7"/></svg>`;
  // The image bar's classes, from the last time its Vectorize was used
  let barClasses = null;
  let recolor = null;
  const luma = ({ r, g, b }) => 0.299 * r + 0.587 * g + 0.114 * b;
  // A color as hex, as Ligma names fills
  const colorKey = ({ r, g, b }) =>
    [r, g, b]
      .map((value) => Math.round(value * 255).toString(16).padStart(2, "0"))
      .join("")
      .toUpperCase();

  const closeRecolor = () => {
    if (!recolor) return;
    cancelAnimationFrame(recolor.frame);
    recolor.root.remove();
    document.documentElement.removeAttribute(RECOLORING);
    recolor = null;
  };

  // `areas` is how many pixels of each fill show, by hex
  const openRecolor = (figma, frames, areas) => {
    closeRecolor();
    const paints = [];
    const palette = new Map();
    for (const frame of frames) {
      for (const node of frame.findAll((child) => child.type === "VECTOR")) {
        const paint = fillsOf(node).find((fill) => fill.type === "SOLID");
        if (!paint) continue;
        const key = colorKey(paint.color);
        paints.push({ node, paint, key, shown: key });
        if (!palette.has(key)) palette.set(key, { key, color: paint.color, area: areas.get(`#${key}`) ?? 0 });
      }
    }
    if (!paints.length) return;
    const colors = palette.size;
    // What the bar shows, and the colors each count maps to, once worked out
    const state = { palette: [...palette.values()], count: colors, mode: "COLOR", maps: null };

    const root = document.createElement("div");
    root.setAttribute(RECOLOR, "");
    root.setAttribute("data-fullscreen-prevent-event-capture", "true");
    const bar = root.appendChild(document.createElement("div"));
    if (barClasses) {
      root.className = barClasses.root;
      bar.className = barClasses.bar;
    }
    const title = bar.appendChild(Object.assign(document.createElement("span"), { className: "title", textContent: "Vectorize" }));
    title.setAttribute("aria-hidden", "true");
    bar.append(Object.assign(document.createElement("span"), { className: "divider" }));
    const modes = bar.appendChild(Object.assign(document.createElement("div"), { className: "modes" }));
    modes.setAttribute("role", "radiogroup");
    modes.setAttribute("aria-label", "Color mapping mode");
    for (const [mode, label] of MODES) {
      const button = modes.appendChild(Object.assign(document.createElement("button"), { type: "button", className: `mode ${mode.toLowerCase()}` }));
      button.setAttribute("role", "radio");
      button.setAttribute("aria-label", label);
      Object.assign(button.dataset, { mode, tooltip: label, tooltipType: "text", tooltipShowAbove: "true" });
      button.append(document.createElement("span"));
      button.addEventListener("click", () => {
        state.mode = mode;
        update(true);
      });
    }
    bar.append(Object.assign(document.createElement("span"), { className: "divider" }));
    const number = bar.appendChild(Object.assign(document.createElement("input"), { type: "text", className: "count", inputMode: "numeric", spellcheck: false }));
    number.setAttribute("aria-label", "Number of colors");
    Object.assign(number.dataset, { tooltip: "Number of colors", tooltipType: "text", tooltipShowAbove: "true" });
    const slider = bar.appendChild(Object.assign(document.createElement("input"), { type: "range", className: "slider", min: 1, max: colors, step: 1 }));
    slider.setAttribute("aria-label", "Number of colors");
    slider.disabled = colors < 2;
    bar.append(Object.assign(document.createElement("span"), { className: "divider" }));
    const done = bar.appendChild(Object.assign(document.createElement("button"), { type: "button", className: "done", innerHTML: CHECK }));
    done.setAttribute("aria-label", "Done");
    Object.assign(done.dataset, { tooltip: "Done", tooltipType: "text", tooltipShowAbove: "true" });
    done.addEventListener("click", closeRecolor);

    // Keys typed in the bar, kept from Figma (see below)
    const key = (event) => {
      if (event.type !== "keydown" || event.target !== number) return;
      if (event.key === "Enter") number.blur();
      if (event.key === "Escape") {
        number.value = state.count;
        number.blur();
      }
    };
    number.addEventListener("change", () => {
      const value = Math.round(Number(number.value));
      if (Number.isFinite(value)) state.count = Math.min(colors, Math.max(1, value));
      update(true);
    });
    slider.addEventListener("input", () => {
      state.count = Number(slider.value);
      update(false);
    });
    slider.addEventListener("change", () => update(true));

    // Shows the state in the bar and recolors the vectors, at most once a
    // frame. `commit` ends the undo step
    let pending = false;
    const update = (commit) => {
      number.value = state.count;
      slider.value = state.count;
      for (const button of modes.children) button.setAttribute("aria-checked", String(button.dataset.mode === state.mode));
      if (commit) {
        apply();
        try {
          figma.commitUndo();
        } catch {}
        return;
      }
      if (pending) return;
      pending = true;
      recolor.frame = requestAnimationFrame(() => {
        pending = false;
        apply();
      });
    };
    const apply = () => {
      state.maps ??= reductions(state.palette);
      const map = state.maps[state.count];
      for (const entry of paints) {
        let color = map.get(entry.key);
        if (state.mode !== "COLOR") {
          const gray = state.mode === "GRAYSCALE" ? luma(color) : luma(color) < 0.5 ? 0 : 1;
          color = { r: gray, g: gray, b: gray };
        }
        const key = colorKey(color);
        if (key === entry.shown || entry.node.removed) continue;
        try {
          entry.node.fills = [{ ...entry.paint, color }];
          entry.shown = key;
        } catch {}
      }
    };

    recolor = { root, frames, ids: new Set(frames.map((frame) => frame.id)), frame: 0, key };
    document.documentElement.setAttribute(RECOLORING, "");
    update(false);
    placeRecolor();
  };

  // Keeps the recolor bar above the toolbar, where Figma shows its bars,
  // and closes it once its vectors aren't what's selected
  const placeRecolor = () => {
    if (!recolor) return;
    const figma = api();
    const selection = figma?.currentPage.selection ?? [];
    if (!selection.length || recolor.frames.some((frame) => frame.removed) || !selection.every((node) => recolor.ids.has(node.id))) {
      closeRecolor();
      return;
    }
    const wrapper = document.querySelector('[data-testid="design-toolbelt-wrapper"]');
    recolor.root.classList.toggle("floating", !wrapper || !barClasses);
    const parent = wrapper && barClasses ? wrapper : document.body;
    if (recolor.root.parentElement !== parent) parent.append(recolor.root);
  };

  // Figma takes keys on the window as they go down to their target, so
  // keys typed in the recolor bar stop here, before it sees them: this
  // runs before Figma's own scripts, so it comes first. Escape elsewhere
  // closes the bar
  for (const type of ["keydown", "keypress", "keyup"]) {
    addEventListener(
      type,
      (event) => {
        if (!recolor) return;
        if (event.target.closest?.(`[${RECOLOR}]`)) {
          event.stopImmediatePropagation();
          recolor.key(event);
        } else if (type === "keydown" && event.key === "Escape") closeRecolor();
      },
      true,
    );
  }

  // Select object picks out objects in one image as Photoshop's Object
  // Selection does, in place of Figma's Select area, whose bar its own
  // follows: the object under the pointer shows as it moves, a click picks
  // it, Shift adds, Alt (Option) takes away and dragging a box picks the
  // object in it. The bar's brush paints picks in or out instead, and its
  // modes do what Shift and Alt do. Isolate, or Enter, puts what's picked
  // in a new layer above, cropped to it, its edge moved and feathered as
  // the bar says; Cancel, Escape or a click on the canvas off the image
  // leaves. Ligma's SAM finds the objects (see objects.rs): it reads the
  // image once when the mode opens, then gives outlines as SVG paths in
  // the pixels of the image it read
  const OBJECTS = "data-ligma-objects";
  // On <html> while it's open, to hide Figma's own bars, and while the
  // pointer is over the image, with the cursor for what a click does
  const PICKING = "data-ligma-picking";
  const POINTER = "data-ligma-pick-pointer";
  const MAC = navigator.userAgent.includes("Mac");
  let picking = null;
  // The bar's choices, kept while the tab is open: the tool, what a click
  // or stroke does without Shift or Alt, and the brush's size (once
  // chosen), how far out the edge moves and how far it's feathered, in
  // the image's own pixels
  const options = { tool: "object", mode: "new", size: null, shift: 0, feather: 0 };

  // A request for objects.rs: a line of JSON, then the image if any
  const request = (header, bytes) => {
    const line = new TextEncoder().encode(`${JSON.stringify(header)}\n`);
    if (!bytes) return line;
    const all = new Uint8Array(line.length + bytes.length);
    all.set(line);
    all.set(bytes, line.length);
    return all;
  };

  // How many of its image's pixels each of the layer's is, and how large
  // to draw the layer to isolate objects in it: as large as its image, up
  // to 4096 pixels across, and no smaller than what was read
  const imageScales = async (figma, node) => {
    let native = 1;
    const paint = fillsOf(node).findLast((fill) => fill.type === "IMAGE" && fill.imageHash && fill.visible !== false);
    try {
      const size = figma.getImageByHash(paint.imageHash).getSizeAsync();
      const { width, height } = await Promise.race([size, new Promise((_, reject) => setTimeout(reject, 2000))]);
      const fit = paint.scaleMode === "FIT" ? Math.min : Math.max;
      native = paint.scaleMode === "TILE" ? 1 / (paint.scalingFactor ?? 1) : 1 / fit(node.width / width, node.height / height);
    } catch {}
    const largest = 4096 / Math.max(node.width, node.height, 1);
    return { native, full: Math.min(largest, Math.max(native, workingScale(node))) };
  };

  // The cut-out objects.rs sent, as a new image layer right above `node`,
  // where they were in it
  const placeIsolated = (figma, node, { png, x, y, width, height }) => {
    const hash = figma.createImage(Uint8Array.from(atob(png), (char) => char.charCodeAt(0))).hash;
    const layer = figma.createRectangle();
    try {
      const parent = node.parent;
      parent.insertChild(parent.children.indexOf(node) + 1, layer);
      layer.name = `${node.name} [Isolated]`;
      layer.resize(Math.max(width * node.width, 0.01), Math.max(height * node.height, 0.01));
      layer.fills = [{ type: "IMAGE", imageHash: hash, scaleMode: "FILL" }];
      if (parent.layoutMode && parent.layoutMode !== "NONE") layer.layoutPositioning = "ABSOLUTE";
      const [[a, c, e], [b, d, f]] = node.relativeTransform;
      const [left, top] = [x * node.width, y * node.height];
      layer.relativeTransform = [
        [a, c, a * left + c * top + e],
        [b, d, b * left + d * top + f],
      ];
      if (node.constraints) layer.constraints = node.constraints;
      return layer;
    } catch (error) {
      layer.remove();
      throw error;
    }
  };

  const SVG_NS = "http://www.w3.org/2000/svg";
  const ALT = MAC ? "Option" : "Alt";
  const icon = (paths) => `<svg width="24" height="24" fill="none" viewBox="0 0 24 24">${paths}</svg>`;
  const ISOLATE_ICON = icon(`<rect ${STROKE} x="4.5" y="7.5" width="12" height="12" rx="1.5" stroke-dasharray="2 2"/><rect ${STROKE} x="8.5" y="4.5" width="11" height="11" rx="1.5"/>`);
  // An arrow on a dashed box, as Photoshop's Object Selection has it
  const OBJECT_ICON = icon(`<path ${STROKE} stroke-dasharray="2 2" d="M10.5 16.5h-3a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v3"/><path ${STROKE} stroke-linejoin="round" d="m12.5 12.5 6.5 2.4-2.9 1.2-1.2 2.9z"/>`);
  const TOOLS = [
    ["object", "Click an object, or drag a box around it", OBJECT_ICON],
    ["brush", "Paint over what to pick, [ and ] to resize", icon(`<path ${STROKE} stroke-linecap="round" d="m18.5 5.5-6.5 7.5"/><path ${STROKE} stroke-linejoin="round" d="M11.5 12.5c1 .7 1.4 1.7 1 2.8-.6 1.8-2.8 3-6.5 2.7 1.2-.8 1-2 1.4-3.2.5-1.6 2.6-3.1 4.1-2.3z"/>`)],
  ];
  // Photoshop's selection modes: a new selection, added to it and taken
  // away from it
  const PICK_MODES = [
    ["new", "New selection", icon(`<rect ${STROKE} x="6.5" y="6.5" width="11" height="11" rx="1"/>`)],
    ["add", `Add to selection, or hold Shift`, icon(`<path ${STROKE} stroke-linejoin="round" d="M5.5 5.5h9v4h4v9h-9v-4h-4z"/>`)],
    ["subtract", `Take away from selection, or hold ${ALT}`, icon(`<path ${STROKE} stroke-linejoin="round" d="M5.5 5.5h9v4h-5v5h-4z"/><path ${STROKE} stroke-dasharray="2 2" d="M9.5 9.5h9v9h-9z"/>`)],
  ];
  // The bar's number fields: the brush's size, and the edge, in the
  // image's own pixels
  const FIELDS = [
    ["size", "Size", "Brush size, in pixels", 1, 2000],
    ["shift", "Expand", "Expand, or contract when negative, in pixels", -200, 200],
    ["feather", "Feather", "Feather, in pixels", 0, 200],
  ];

  const selectObject = async (bar, crop) => {
    const figma = api();
    if (!figma || busy || picking) return;
    const image = (paint) => paint?.type === "IMAGE" && !!paint.imageHash && paint.visible !== false;
    const selection = figma.currentPage.selection;
    if (selection.length !== 1 || !fillsOf(selection[0]).some(image)) {
      figma.notify(selection.length > 1 ? "Select one image to pick objects in" : "No images selected", { timeout: 2000 });
      return;
    }
    const node = selection[0];
    const canvas = mainCanvas();
    if (!canvas) return;
    closeRecolor();
    barClasses = { root: bar.closest("[data-secondary-toolbelt]")?.className ?? "", bar: bar.className };

    // The bar, as Figma's Select area has it, Isolate then Cancel, after
    // the tools, modes and fields
    const root = document.createElement("div");
    root.setAttribute(OBJECTS, "");
    root.setAttribute("data-fullscreen-prevent-event-capture", "true");
    root.className = barClasses.root;
    const strip = root.appendChild(Object.assign(document.createElement("div"), { className: barClasses.bar }));
    // Only while the image is read, so the bar keeps its width after
    const hint = strip.appendChild(Object.assign(document.createElement("span"), { className: "hint", textContent: "Reading the image…" }));
    hint.setAttribute("aria-live", "polite");
    const labelled = (label, icon) => {
      const button = Object.assign(document.createElement("button"), { type: "button", className: crop.className });
      const text = crop.querySelector("span")?.cloneNode(false) ?? document.createElement("span");
      text.textContent = label;
      if (icon) button.innerHTML = icon;
      button.append(text);
      return button;
    };
    // Icon buttons, one of which is checked, as Figma's own radio groups
    const radios = (label, choices, key) => {
      const group = strip.appendChild(Object.assign(document.createElement("div"), { className: "group" }));
      group.setAttribute("role", "radiogroup");
      group.setAttribute("aria-label", label);
      for (const [value, name, svg] of choices) {
        const button = group.appendChild(Object.assign(document.createElement("button"), { type: "button", className: "icon", innerHTML: svg }));
        button.setAttribute("role", "radio");
        button.setAttribute("aria-label", name);
        Object.assign(button.dataset, { value, tooltip: name, tooltipType: "text", tooltipShowAbove: "true" });
        button.addEventListener("click", () => choose(key, value));
      }
      return group;
    };
    const tools = radios("Tool", TOOLS, "tool");
    strip.append(Object.assign(document.createElement("span"), { className: "divider" }));
    const modes = radios("Selection mode", PICK_MODES, "mode");
    strip.append(Object.assign(document.createElement("span"), { className: "divider" }));
    // Fields, whose labels drag to change them, as Figma's do
    const fields = {};
    for (const [key, label, tip, min, max] of FIELDS) {
      const field = strip.appendChild(Object.assign(document.createElement("label"), { className: `field ${key}` }));
      Object.assign(field.dataset, { tooltip: tip, tooltipType: "text", tooltipShowAbove: "true" });
      const name = field.appendChild(Object.assign(document.createElement("span"), { textContent: label }));
      const input = field.appendChild(Object.assign(document.createElement("input"), { type: "text", inputMode: "numeric", spellcheck: false }));
      input.setAttribute("aria-label", tip);
      const set = (value) => {
        if (!Number.isFinite(value)) return;
        choose(key, Math.min(max, Math.max(min, Math.round(value))));
      };
      input.addEventListener("change", () => set(Number(input.value)));
      name.addEventListener("pointerdown", (event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        name.setPointerCapture(event.pointerId);
        const [from, start] = [event.clientX, picking.values[key]];
        const move = (event) => set(start + (event.clientX - from) / (key === "size" ? 1 : 4));
        name.addEventListener("pointermove", move);
        name.addEventListener("lostpointercapture", () => name.removeEventListener("pointermove", move), { once: true });
      });
      fields[key] = { field, input };
    }
    strip.append(Object.assign(document.createElement("span"), { className: "divider" }));
    const isolateButton = strip.appendChild(labelled("Isolate", ISOLATE_ICON));
    Object.assign(isolateButton.dataset, { tooltip: "Isolate in a new layer", tooltipType: "text", tooltipShowAbove: "true" });
    isolateButton.disabled = true;
    isolateButton.addEventListener("click", () => isolate());
    strip.append(Object.assign(document.createElement("span"), { className: "divider" }));
    const cancel = strip.appendChild(labelled("Cancel"));
    cancel.addEventListener("click", () => closePicking());

    // The outlines over the image, in the pixels of the image Ligma read
    const layer = document.createElement("div");
    layer.setAttribute(OBJECTS, "overlay");
    canvas.after(layer);
    const svg = layer.appendChild(document.createElementNS(SVG_NS, "svg"));
    const shape = (className) => {
      const element = svg.appendChild(document.createElementNS(SVG_NS, className === "band" ? "rect" : "path"));
      element.setAttribute("class", className);
      return element;
    };
    const paths = { picked: shape("picked"), ants: shape("ants"), hover: shape("hover"), band: shape("band"), stroke: shape("stroke") };
    const brush = svg.appendChild(document.createElementNS(SVG_NS, "circle"));
    brush.setAttribute("class", "brush");

    picking = {
      figma,
      node,
      canvas,
      root,
      layer,
      svg,
      paths,
      brush,
      hint,
      tools,
      modes,
      fields,
      isolateButton,
      session: null,
      width: 0,
      height: 0,
      // How many of the image's pixels each of the layer's is, and how
      // large it's drawn to isolate objects (see imageScales)
      scales: { native: 1, full: 1 },
      // What the bar shows, the brush's size in the image's pixels once
      // the image is read
      values: { ...options, size: options.size ?? 0 },
      picks: [],
      // The picks before each change, and those undone, for Undo and Redo
      undone: [],
      redone: [],
      // The point to find the object at, once the last request is back,
      // and the one shown
      wanted: null,
      asking: false,
      shown: null,
      // How many times the picks changed, so a late outline is dropped
      version: 0,
      drag: null,
      working: false,
      frame: 0,
    };
    document.documentElement.setAttribute(PICKING, "");
    placePicking();
    placeOverlay();

    const current = picking;
    showOptions();
    const stopShimmer = shimmer(figma, [node], "objects");
    const { show, hide } = toast(figma);
    try {
      const scales = imageScales(figma, node);
      const bytes = await drawn(figma, node, workingScale(node));
      const opened = JSON.parse(await handOff("object", request({ op: "open" }, bytes), show));
      if (picking !== current) {
        handOff("object", request({ op: "close", session: opened.session })).catch(() => {});
        return;
      }
      Object.assign(current, { session: opened.session, width: opened.width, height: opened.height, scales: await scales });
      // A brush a 40th of the image across, until one is chosen
      const { native } = current.scales;
      current.values.size = options.size ?? Math.max(2, Math.round((Math.max(node.width, node.height) * native) / 40));
      svg.setAttribute("viewBox", `0 0 ${opened.width} ${opened.height}`);
      showOptions();
    } catch (reason) {
      if (picking !== current) return;
      closePicking();
      if (reason !== "cancelled") figma.notify("Couldn't read the image", { timeout: 3000 });
    } finally {
      stopShimmer();
      hide();
    }
  };

  const closePicking = () => {
    if (!picking) return;
    const { root, layer, session, frame } = picking;
    cancelAnimationFrame(frame);
    root.remove();
    layer.remove();
    if (session) handOff("object", request({ op: "close", session })).catch(() => {});
    document.documentElement.removeAttribute(PICKING);
    document.documentElement.removeAttribute(POINTER);
    picking = null;
  };

  // Keeps the bar above the toolbar, and closes it once the image isn't
  // what's selected
  const placePicking = () => {
    if (!picking) return;
    const selection = api()?.currentPage.selection ?? [];
    if (!state.objects || picking.node.removed || selection.length !== 1 || selection[0] !== picking.node) {
      if (!picking.working) closePicking();
      return;
    }
    const wrapper = document.querySelector('[data-testid="design-toolbelt-wrapper"]');
    picking.root.classList.toggle("floating", !wrapper);
    const parent = wrapper ?? document.body;
    if (picking.root.parentElement !== parent) parent.append(picking.root);
  };

  // Keeps the outlines on the image as the canvas pans and zooms
  const placeOverlay = () => {
    if (!picking) return;
    const { figma, node, canvas, layer, svg } = picking;
    Object.assign(layer.style, {
      left: `${canvas.offsetLeft}px`,
      top: `${canvas.offsetTop}px`,
      width: `${canvas.offsetWidth}px`,
      height: `${canvas.offsetHeight}px`,
    });
    try {
      const { bounds } = figma.viewport;
      const zoom = figma.viewport.zoom;
      const [[a, c, e], [b, d, f]] = node.absoluteTransform;
      svg.setAttribute("width", node.width * zoom);
      svg.setAttribute("height", node.height * zoom);
      svg.style.transform = `matrix(${a}, ${b}, ${c}, ${d}, ${(e - bounds.x) * zoom}, ${(f - bounds.y) * zoom})`;
    } catch {}
    picking.frame = requestAnimationFrame(placeOverlay);
  };

  // Where `event` is on the image, in fractions of it, or null when it's
  // not over the canvas
  const onImage = (event) => {
    const { figma, node, canvas } = picking;
    const box = canvas.getBoundingClientRect();
    const [x, y] = [event.clientX - box.left, event.clientY - box.top];
    if (x < 0 || y < 0 || x > box.width || y > box.height) return null;
    const target = event.target;
    if (target instanceof Element && target !== canvas && target.closest(`[${OBJECTS}], [data-secondary-toolbelt], [data-testid="design-toolbelt-wrapper"], [role="toolbar"], [role="dialog"], [role="menu"]`)) return null;
    try {
      const { bounds } = figma.viewport;
      const zoom = figma.viewport.zoom;
      const [[a, c, e], [b, d, f]] = node.absoluteTransform;
      const [dx, dy] = [bounds.x + x / zoom - e, bounds.y + y / zoom - f];
      const det = a * d - b * c;
      const u = (d * dx - c * dy) / det / node.width;
      const v = (a * dy - b * dx) / det / node.height;
      return { u, v, inside: u >= 0 && u <= 1 && v >= 0 && v <= 1, x: event.clientX, y: event.clientY };
    } catch {
      return null;
    }
  };

  const clamp = (value) => Math.min(1, Math.max(0, value));

  // A choice in the bar, kept for next time
  const choose = (key, value) => {
    if (!picking) return;
    if (picking.values[key] === value) {
      showOptions();
      return;
    }
    picking.values[key] = value;
    options[key] = value;
    showOptions();
    if (key === "shift") showPicks();
    if (key === "tool") hoverAt(null);
  };

  // The bar as the choices are
  const showOptions = () => {
    const { values, tools, modes, fields, hint, session, brush } = picking;
    for (const button of tools.children) button.setAttribute("aria-checked", String(button.dataset.value === values.tool));
    for (const button of modes.children) button.setAttribute("aria-checked", String(button.dataset.value === values.mode));
    for (const [key, { field, input }] of Object.entries(fields)) {
      if (document.activeElement !== input) input.value = values[key];
      const off = key === "size" && values.tool !== "brush";
      field.classList.toggle("off", off);
      input.disabled = off;
    }
    hint.hidden = !!session;
    brush.setAttribute("r", brushRadius() * picking.width);
  };

  // The brush's radius, as a fraction of the image's width
  const brushRadius = () => {
    const { values, scales, node } = picking;
    return values.size / 2 / Math.max(node.width * scales.native, 1);
  };

  // The edge, in pixels of the image drawn `scale` times as large as the
  // layer
  const edgeAt = (scale) => {
    const { values, scales } = picking;
    const ratio = scale / scales.native;
    return { shift: values.shift * ratio, feather: values.feather * ratio };
  };

  // The outline of the object at `wanted`, one request at a time, the
  // latest point winning
  const askHover = async () => {
    const current = picking;
    if (!current?.session || current.asking || !current.wanted) return;
    const point = current.wanted;
    current.wanted = null;
    current.asking = true;
    try {
      const { d } = JSON.parse(await handOff("object", request({ op: "outline", session: current.session, picks: [{ point }] })));
      if (picking === current && current.shown === point && !current.drag) current.paths.hover.setAttribute("d", d);
    } catch {}
    current.asking = false;
    if (picking === current) askHover();
  };

  const hoverAt = (at) => {
    if (!picking.session || picking.working) return;
    if (!at) {
      picking.shown = null;
      picking.wanted = null;
      picking.paths.hover.removeAttribute("d");
      return;
    }
    // Pixels apart in the image read, so small moves don't ask again
    const last = picking.shown;
    if (last && Math.hypot((at.u - last[0]) * picking.width, (at.v - last[1]) * picking.height) < 3) return;
    const point = [clamp(at.u), clamp(at.v)];
    picking.shown = point;
    picking.wanted = point;
    askHover();
  };

  // The picks outlined, and the bar told
  const showPicks = async () => {
    const current = picking;
    const version = ++current.version;
    const { picks, paths, node } = current;
    current.isolateButton.disabled = !picks.length || current.working;
    showOptions();
    if (!picks.length) {
      paths.picked.removeAttribute("d");
      paths.ants.removeAttribute("d");
      paths.stroke.removeAttribute("d");
      return;
    }
    try {
      const edge = edgeAt(workingScale(node));
      const { d } = JSON.parse(await handOff("object", request({ op: "outline", session: current.session, picks, edge })));
      if (picking !== current || current.version !== version) return;
      paths.picked.setAttribute("d", d);
      paths.ants.setAttribute("d", d);
      if (!current.drag) paths.stroke.removeAttribute("d");
      // Taking away everything leaves nothing to isolate, though
      // contracting the edge only hides it
      if (!d && edge.shift >= 0) {
        current.picks = [];
        showPicks();
      }
    } catch {}
  };

  const pick = (pick, mode) => {
    const { picks } = picking;
    // A stroke of the brush adds to the picks, unless it takes away
    if (pick.stroke && mode === "replace") mode = "add";
    if (mode === "subtract" && !picks.length) {
      picking.paths.stroke.removeAttribute("d");
      return;
    }
    picking.undone.push(picks);
    picking.redone = [];
    if (mode === "add") picking.picks = [...picks, pick];
    else if (mode === "subtract") picking.picks = [...picks, { ...pick, subtract: true }];
    else picking.picks = [pick];
    showPicks();
  };

  // Steps back through the picks, or forward again after that, as
  // Photoshop's undo does while selecting
  const undoPick = (redo) => {
    const [from, to] = redo ? [picking.redone, picking.undone] : [picking.undone, picking.redone];
    if (!from.length || picking.working) return;
    to.push(picking.picks);
    picking.picks = from.pop();
    showPicks();
  };

  const isolate = async () => {
    const current = picking;
    if (!current?.session || !current.picks.length || current.working) return;
    const { figma, node, session, picks } = current;
    current.working = true;
    current.isolateButton.disabled = true;
    hoverAt(null);
    const stopShimmer = shimmer(figma, [node], "objects");
    let layer = null;
    try {
      const { full } = current.scales;
      const bytes = await drawn(figma, node, full);
      const result = JSON.parse(await handOff("object", request({ op: "isolate", session, picks, edge: edgeAt(full) }, bytes)));
      if (node.removed) throw "failed";
      layer = placeIsolated(figma, node, result);
      try {
        figma.commitUndo();
      } catch {}
    } catch (reason) {
      figma.notify(reason === "empty" ? "Nothing picked to isolate" : "Couldn't isolate the object", { timeout: 3000 });
    }
    stopShimmer();
    current.working = false;
    if (picking !== current) return;
    if (layer) {
      closePicking();
      figma.currentPage.selection = [layer];
    } else showPicks();
  };

  // The cursor for what a click on the image does: Ligma's arrow with a
  // dashed box, as Photoshop's Object Selection has, and + or − with Shift
  // or Alt
  const cursor = (mark) => {
    const extra =
      mark === "add" ? '<path d="M19 3v6M16 6h6" stroke="#fff" stroke-width="3.5"/><path d="M19 3v6M16 6h6" stroke="#000" stroke-width="1.5"/>'
      : mark === "subtract" ? '<path d="M16 6h6" stroke="#fff" stroke-width="3.5"/><path d="M16 6h6" stroke="#000" stroke-width="1.5"/>'
      : "";
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" fill="none"><path d="M4.5 3.5v12.6l3.2-3 2.2 4.9 2.1-.9-2.2-4.8h4.4z" fill="#000" stroke="#fff" stroke-linejoin="round"/><rect x="13.5" y="13.5" width="8" height="8" rx="1" stroke="#fff" stroke-width="2.5"/><rect x="13.5" y="13.5" width="8" height="8" rx="1" stroke="#000" stroke-dasharray="2 1.5"/>${extra}</svg>`;
    return `url("data:image/svg+xml,${encodeURIComponent(svg)}") 4 3, crosshair`;
  };
  // Shift and Alt add and take away, whatever the bar's mode
  const modeOf = (event) => (event.shiftKey ? "add" : event.altKey ? "subtract" : picking.values.mode === "new" ? "replace" : picking.values.mode);
  const showPointer = (event, over) => {
    const root = document.documentElement;
    const value = !over ? null : !picking.session || picking.working ? "wait" : picking.values.tool === "brush" ? "brush" : modeOf(event);
    if (value) root.setAttribute(POINTER, value);
    else root.removeAttribute(POINTER);
    // The brush shows as a circle its size, dashed where it takes away
    const shown = over && value === "brush";
    picking.brush.style.display = shown ? "block" : "";
    if (shown) picking.brush.classList.toggle("subtract", modeOf(event) === "subtract");
  };

  // The stroke being painted, as wide as the brush, in the image's pixels
  const showStroke = () => {
    const { drag, paths, width, height } = picking;
    const points = drag.points.map(([u, v]) => `${u * width} ${v * height}`);
    paths.stroke.setAttribute("d", `M${points.join("L")}`);
    paths.stroke.setAttribute("stroke-width", brushRadius() * 2 * width);
    paths.stroke.classList.toggle("subtract", drag.mode === "subtract");
  };

  // Pointer events over the image are Select object's while it's open:
  // this runs before Figma's own scripts, so it comes first. Space held, or
  // a button other than the main one, leaves them to Figma to pan
  let space = false;
  const POINTER_TYPES = ["pointerdown", "pointermove", "pointerup", "mousedown", "mousemove", "mouseup", "click", "dblclick", "auxclick"];
  const swallow = (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  for (const type of POINTER_TYPES) {
    addEventListener(
      type,
      (event) => {
        if (!picking) return;
        const drag = picking.drag;
        if (drag) {
          swallow(event);
          if (drag.points && type === "pointermove") {
            const at = onImage(event);
            if (!at) return;
            placeBrush(at);
            // A point every quarter of the brush across, in the image read
            const [u, v] = drag.points.at(-1);
            const step = Math.max(1, (brushRadius() * picking.width) / 2);
            if (Math.hypot((at.u - u) * picking.width, (at.v - v) * picking.height) < step) return;
            drag.points.push([at.u, at.v]);
            showStroke();
          } else if (drag.points && type === "pointerup") {
            picking.drag = null;
            const at = onImage(event);
            if (at) drag.points.push([at.u, at.v]);
            pick({ stroke: drag.points, radius: brushRadius() }, drag.mode);
          } else if (type === "pointermove") {
            const at = onImage(event);
            if (!at) return;
            drag.end = at;
            drag.moved ||= Math.hypot(at.x - drag.start.x, at.y - drag.start.y) > 4;
            if (drag.moved) {
              const [left, top] = [clamp(Math.min(drag.start.u, at.u)), clamp(Math.min(drag.start.v, at.v))];
              const [right, bottom] = [clamp(Math.max(drag.start.u, at.u)), clamp(Math.max(drag.start.v, at.v))];
              const band = picking.paths.band;
              band.setAttribute("x", left * picking.width);
              band.setAttribute("y", top * picking.height);
              band.setAttribute("width", (right - left) * picking.width);
              band.setAttribute("height", (bottom - top) * picking.height);
              band.style.display = "block";
            }
          } else if (type === "pointerup") {
            picking.drag = null;
            picking.paths.band.style.display = "";
            const { start, end = start, moved, mode } = drag;
            if (!moved) pick({ point: [clamp(start.u), clamp(start.v)] }, mode);
            else {
              const rect = [Math.min(start.u, end.u), Math.min(start.v, end.v), Math.max(start.u, end.u), Math.max(start.v, end.v)].map(clamp);
              if (rect[2] - rect[0] > 0.005 && rect[3] - rect[1] > 0.005) pick({ box: rect }, mode);
            }
          }
          return;
        }
        const at = onImage(event);
        const painting = picking.values.tool === "brush";
        if (type === "pointermove") {
          showPointer(event, at?.inside);
          if (painting && at) placeBrush(at);
          hoverAt(at?.inside && !space && !event.buttons && !painting ? at : null);
        }
        if (!at?.inside) {
          // A click on the canvas off the image leaves, and goes to Figma
          if (type === "pointerdown" && at && event.button === 0 && !space) closePicking();
          return;
        }
        // A button other than the main one, pressed or held, pans
        if (space || event.button > 0 || (event.buttons & ~1) !== 0) return;
        swallow(event);
        if (type === "pointerdown" && picking.session && !picking.working) {
          picking.drag = { start: at, end: at, moved: false, mode: modeOf(event) };
          picking.paths.hover.removeAttribute("d");
          if (painting) {
            picking.drag.points = [[at.u, at.v]];
            showStroke();
          }
        }
      },
      true,
    );
  }

  addEventListener("blur", () => (space = false));

  // Keeps the brush's circle under the pointer
  const placeBrush = (at) => {
    picking.brush.setAttribute("cx", at.u * picking.width);
    picking.brush.setAttribute("cy", at.v * picking.height);
  };

  // While it's open, Escape leaves, Enter isolates, Undo and Redo step
  // through the picks rather than the file, [ and ] resize the brush, and
  // Delete does nothing rather than delete the image. Shift and Alt change
  // the cursor, and Space is left to Figma, to pan. Keys typed in the
  // bar's fields stop here, before Figma sees them
  for (const type of ["keydown", "keypress", "keyup"]) {
    addEventListener(
      type,
      (event) => {
        const input = event.target;
        if (picking && input instanceof HTMLInputElement && input.closest(`[${OBJECTS}=""]`)) {
          event.stopImmediatePropagation();
          if (type !== "keydown") return;
          if (event.key === "Enter") input.blur();
          if (event.key === "Escape") {
            input.value = picking.values[input.closest(".field").classList[1]];
            input.blur();
          }
          return;
        }
        if (type === "keypress") return;
        if (event.key === " ") space = type === "keydown";
        // Figma keeps focus in a read-only input of its own, which isn't typing
        if (!picking || event.target.closest?.(':is(input, textarea):not([readonly]), [contenteditable]:not([contenteditable="false"])')) return;
        if (document.documentElement.hasAttribute(POINTER)) showPointer(event, true);
        if (type !== "keydown") return;
        if (event.key === "Escape") {
          swallow(event);
          closePicking();
        } else if (event.key === "Enter") {
          swallow(event);
          isolate();
        } else if ((MAC ? event.metaKey : event.ctrlKey) && !event.altKey && /^[zy]$/i.test(event.key)) {
          swallow(event);
          undoPick(event.key.toLowerCase() === "y" || event.shiftKey);
        } else if ((event.key === "[" || event.key === "]") && picking.values.tool === "brush" && !event.ctrlKey && !event.metaKey) {
          swallow(event);
          const size = picking.values.size;
          choose("size", Math.max(1, Math.round(event.key === "]" ? Math.max(size + 1, size * 1.2) : Math.min(size - 1, size / 1.2))));
        } else if (event.key === "Delete" || event.key === "Backspace") swallow(event);
      },
      true,
    );
  }

  // Ligma's Select object, Remove background and Vectorize, after Crop and
  // Ligma's adjustments, each where it's on
  const BUTTONS = [
    {
      attribute: "data-ligma-select-object",
      on: () => state.objects,
      label: "Select object",
      icon: OBJECT_ICON,
      run: (bar, crop) => selectObject(bar, crop),
    },
    {
      attribute: "data-ligma-background",
      on: () => state.local,
      label: "Remove background",
      icon: `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M5.5 8.5v-2a1 1 0 0 1 1-1h2m7 0h2a1 1 0 0 1 1 1v2m0 7v2a1 1 0 0 1-1 1h-2m-7 0h-2a1 1 0 0 1-1-1v-2"/><circle ${STROKE} cx="12" cy="10" r="2.5"/><path ${STROKE} d="M8.5 17a3.5 3.5 0 0 1 7 0"/></svg>`,
      run: () => removeBackground(),
    },
    {
      attribute: "data-ligma-vectorize",
      on: () => state.vectorize,
      label: "Vectorize",
      // Figma's, from its More menu
      icon: `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${FILL} d="M6.5 5.001c.654 0 1.208.42 1.413 1.004L8 6h5.5a.5.5 0 0 1 0 1H8q-.045 0-.089.004a1.5 1.5 0 0 1-.907.907L7 8.001v8q0 .045.004.088c.423.15.756.485.907.907q.045.005.09.005h8q.044 0 .087-.005c.151-.422.486-.756.907-.907Q17 16.046 17 16v-5.5a.5.5 0 0 1 1 0V16l-.005.085a1.5 1.5 0 1 1-1.91 1.91L16 18H8l-.086-.005a1.5 1.5 0 1 1-1.91-1.91L6 16V8q.001-.044.004-.086A1.5 1.5 0 0 1 6.5 5.001m0 12a.5.5 0 1 0 0 1 .5.5 0 0 0 0-1m11 0a.5.5 0 1 0 0 1 .5.5 0 0 0 0-1m-7.776-5.418a.5.5 0 0 1 .629.065l3.5 3.5a.5.5 0 0 1-.707.707l-3.112-3.112a.05.05 0 0 0-.07 0l-1.111 1.111a.5.5 0 0 1-.707-.706l1.5-1.5zm3.775-2.582a1.5 1.5 0 1 1 0 3 1.5 1.5 0 0 1 0-3m0 1a.5.5 0 1 0 .001 1 .5.5 0 0 0 0-1m3.516-5.563c.126-.505.843-.505.97 0l.315 1.26 1.26.314c.505.126.505.844 0 .97l-1.26.315-.315 1.26c-.127.504-.844.504-.97 0l-.316-1.26-1.26-.315c-.504-.126-.503-.844 0-.97l1.26-.315zM6.499 6A.5.5 0 1 0 6.5 7.002a.5.5 0 0 0 0-1.001"/></svg>`,
      run: (bar) => {
        barClasses = { root: bar.closest("[data-secondary-toolbelt]")?.className ?? "", bar: bar.className };
        vectorize();
      },
    },
  ];
  const OURS = ["[data-ligma-adjustments-bar]", ...BUTTONS.map(({ attribute }) => `[${attribute}]`)].join(", ");
  const placeButtons = (bar, crop) => {
    let last = [...bar.querySelectorAll("[data-ligma-adjustments-bar]")].pop() ?? crop;
    for (const { attribute, on, label, icon, run } of BUTTONS) {
      let button = bar.querySelector(`[${attribute}]`);
      if (!on()) {
        button?.remove();
        continue;
      }
      if (!button) {
        button = Object.assign(document.createElement("button"), { type: "button", className: crop.className });
        button.setAttribute(attribute, "");
        const text = crop.querySelector("span")?.cloneNode(false) ?? document.createElement("span");
        text.textContent = label;
        button.innerHTML = icon;
        button.append(text);
        button.addEventListener("click", (event) => {
          event.preventDefault();
          event.stopImmediatePropagation();
          run(bar, crop);
        });
      }
      if (last.nextElementSibling !== button) last.after(button);
      button.disabled = busy;
      last = button;
    }
  };

  // The image bar's Crop, by its icon. Everything else in the bar but
  // Ligma's buttons is an AI action, its divider or the More menu
  const BAR = '[data-secondary-toolbelt="true"] [class*="secondary_toolbelt--secondaryToolbeltContainer"]';
  const CROP = 'path[d^="M8.5 5a.5.5 0 0 1 .5.5V15h6V9"]';
  const HIDDEN = "data-ligma-ai-hidden";
  const tidy = () => {
    placeRecolor();
    placePicking();
    const hide = state.on && free();
    if (!hide) {
      if (!picking?.working) closePicking();
      for (const node of document.querySelectorAll(`[${HIDDEN}]`)) node.removeAttribute(HIDDEN);
      for (const { attribute } of BUTTONS) for (const node of document.querySelectorAll(`[${attribute}]`)) node.remove();
      return;
    }
    for (const bar of document.querySelectorAll(BAR)) {
      const crop = bar.querySelector(CROP)?.closest("button");
      if (!crop || crop.parentElement !== bar) continue;
      for (const child of bar.children) {
        if (child === crop || child.matches(`${OURS}, [data-editor-theme]`)) continue;
        if (!child.hasAttribute(HIDDEN)) child.setAttribute(HIDDEN, "");
      }
      placeButtons(bar, crop);
    }
  };

  const style = document.createElement("style");
  style.textContent = `[${HIDDEN}] { display: none !important; }
    [${SHIMMER}] { position: absolute; overflow: hidden; pointer-events: none; transition: opacity 200ms ease-out; }
    [${SHIMMER}].done { opacity: 0; }
    [${SHIMMER}] > div { position: absolute; left: 0; top: 0; overflow: hidden; transform-origin: 0 0; background: rgb(255 255 255 / 0.12); }
    :is([${SHIMMER}="vectorize"], [${SHIMMER}="objects"]) > div { background: rgb(24 160 251 / 0.16); }
    [${SHIMMER}] > div::after {
      content: ""; position: absolute; inset: 0;
      background: linear-gradient(100deg, transparent 20%, rgb(255 255 255 / 0.45) 50%, transparent 80%);
      animation: ligma-shimmer 1.6s ease-in-out infinite;
    }
    :is([${SHIMMER}="vectorize"], [${SHIMMER}="objects"]) > div::after { background: linear-gradient(100deg, transparent 20%, rgb(190 226 255 / 0.4) 50%, transparent 80%); }
    @keyframes ligma-shimmer { from { transform: translateX(-100%); } to { transform: translateX(100%); } }
    @media (prefers-reduced-motion: reduce) { [${SHIMMER}] > div::after { animation: none; opacity: 0; } }
    :is(html[${RECOLORING}], html[${PICKING}]) [data-secondary-toolbelt] { display: none !important; }
    :is([${RECOLOR}], [${OBJECTS}=""]).floating { position: fixed; left: 50%; bottom: 72px; transform: translateX(-50%); z-index: 10; }
    :is([${RECOLOR}], [${OBJECTS}=""]) > div {
      display: flex; align-items: center; gap: 4px; height: 40px; box-sizing: border-box; padding: 8px; border-radius: 13px;
      background: var(--color-bg-toolbar, var(--color-bg, #2c2c2c)); color: var(--color-text, #fff);
      box-shadow: var(--elevation-200-canvas, 0 3px 8px rgb(0 0 0 / 0.35)); font: 500 11px/16px Inter, system-ui, sans-serif;
    }
    [${RECOLOR}] .title { padding: 0 4px; white-space: nowrap; }
    [${OBJECTS}=""] .hint { padding: 0 8px 0 4px; color: var(--color-text-secondary, #b3b3b3); white-space: nowrap; }
    [${OBJECTS}=""] button:disabled { opacity: 0.4; pointer-events: none; }
    [${OBJECTS}=""] [hidden] { display: none !important; }
    [${OBJECTS}=""] .group { display: flex; gap: 2px; }
    [${OBJECTS}=""] .icon { display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 5px; background: none; color: var(--color-icon, #fff); }
    [${OBJECTS}=""] .icon:hover { background: var(--color-bg-hover, #383838); }
    [${OBJECTS}=""] .icon[aria-checked="true"] { background: var(--color-bg-selected, #4a4a4a); }
    [${OBJECTS}=""] .field { display: flex; align-items: center; gap: 6px; padding-left: 4px; color: var(--color-text-secondary, #b3b3b3); white-space: nowrap; }
    [${OBJECTS}=""] .field span { cursor: ew-resize; user-select: none; }
    [${OBJECTS}=""] .field.off { opacity: 0.4; pointer-events: none; }
    [${OBJECTS}=""] .field input {
      width: 40px; height: 24px; box-sizing: border-box; padding: 0 6px; border: 1px solid transparent; border-radius: 5px; outline: none;
      background: var(--color-bg-secondary, #383838); color: var(--color-text, #fff); font: inherit;
    }
    [${OBJECTS}=""] .field input:focus { border-color: var(--color-border-selected, #0d99ff); }
    [${OBJECTS}="overlay"] { position: absolute; overflow: hidden; pointer-events: none; }
    [${OBJECTS}="overlay"] svg { position: absolute; left: 0; top: 0; overflow: visible; transform-origin: 0 0; }
    [${OBJECTS}="overlay"] :is(path, rect) { vector-effect: non-scaling-stroke; fill-rule: evenodd; stroke-linejoin: round; }
    [${OBJECTS}="overlay"] .hover { fill: color-mix(in srgb, var(--color-border-selected, #0d99ff) 18%, transparent); stroke: var(--color-border-selected, #0d99ff); stroke-width: 1.5px; }
    [${OBJECTS}="overlay"] .picked { fill: color-mix(in srgb, var(--color-border-selected, #0d99ff) 30%, transparent); stroke: #fff; stroke-width: 1.5px; }
    [${OBJECTS}="overlay"] .ants { fill: none; stroke: var(--color-border-selected, #0d99ff); stroke-width: 1.5px; stroke-dasharray: 4 4; animation: ligma-ants 400ms linear infinite; }
    [${OBJECTS}="overlay"] .stroke { vector-effect: none; fill: none; stroke: color-mix(in srgb, var(--color-border-selected, #0d99ff) 40%, transparent); stroke-linecap: round; }
    [${OBJECTS}="overlay"] .stroke.subtract { stroke: rgb(0 0 0 / 0.4); }
    [${OBJECTS}="overlay"] .brush { display: none; vector-effect: non-scaling-stroke; fill: none; stroke: #fff; stroke-width: 1.5px; filter: drop-shadow(0 0 1px rgb(0 0 0 / 0.8)); }
    [${OBJECTS}="overlay"] .brush.subtract { stroke-dasharray: 4 3; }
    [${OBJECTS}="overlay"] .band { display: none; fill: color-mix(in srgb, var(--color-border-selected, #0d99ff) 8%, transparent); stroke: var(--color-border-selected, #0d99ff); stroke-width: 1px; stroke-dasharray: 4 3; }
    @keyframes ligma-ants { to { stroke-dashoffset: -8; } }
    @media (prefers-reduced-motion: reduce) { [${OBJECTS}="overlay"] .ants { animation: none; } }
    html[${POINTER}="replace"], html[${POINTER}="replace"] * { cursor: ${cursor("replace")} !important; }
    html[${POINTER}="add"], html[${POINTER}="add"] * { cursor: ${cursor("add")} !important; }
    html[${POINTER}="subtract"], html[${POINTER}="subtract"] * { cursor: ${cursor("subtract")} !important; }
    html[${POINTER}="wait"], html[${POINTER}="wait"] * { cursor: progress !important; }
    html[${POINTER}="brush"], html[${POINTER}="brush"] * { cursor: crosshair !important; }
    :is([${RECOLOR}], [${OBJECTS}=""]) .divider { align-self: stretch; width: 1px; margin: 0 4px; background: var(--color-border, #444); }
    [${RECOLOR}] .modes { display: flex; gap: 2px; }
    [${RECOLOR}] button { display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 5px; background: none; color: var(--color-icon, #fff); }
    [${RECOLOR}] button:hover { background: var(--color-bg-hover, #383838); }
    [${RECOLOR}] .mode[aria-checked="true"] { background: var(--color-bg-selected, #4a4a4a); }
    [${RECOLOR}] .mode span { width: 14px; height: 14px; border-radius: 3px; box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.2); }
    [${RECOLOR}] .color span { background: conic-gradient(#ff3b30, #ffcc00, #34c759, #0a84ff, #af52de, #ff3b30); }
    [${RECOLOR}] .grayscale span { background: linear-gradient(135deg, #fff, #000); }
    [${RECOLOR}] .black_and_white span { background: linear-gradient(135deg, #fff 50%, #000 50%); }
    [${RECOLOR}] .count {
      width: 40px; height: 24px; box-sizing: border-box; padding: 0 8px; border: 1px solid transparent; border-radius: 5px; outline: none;
      background: var(--color-bg-secondary, #383838); color: inherit; font: inherit;
    }
    [${RECOLOR}] .count:focus { border-color: var(--color-border-selected, #0d99ff); }
    [${RECOLOR}] .slider { width: 96px; margin: 0 4px; accent-color: var(--color-bg-brand, #0d99ff); }
    [${RECOLOR}] .done { background: var(--color-bg-brand, #0d99ff); color: var(--color-icon-onbrand, #fff); }
    [${RECOLOR}] .done:hover { background: var(--color-bg-brand-hover, var(--color-bg-brand, #0d99ff)); }`;
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  // `document`, not its <html>: WebView2 runs this before there is one
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      tidy();
    });
  }).observe(document, { childList: true, subtree: true });

  // From Ligma, when the setting changes
  window.__ligmaFreeAiActions = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    state = next;
    tidy();
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        tidy();
      }
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
})()"##;

/// The settings: hide the AI actions,
static ON: AtomicBool = AtomicBool::new(true);
/// put Ligma's Remove background in their place, where it works,
static LOCAL: AtomicBool = AtomicBool::new(true);
/// and its Vectorize,
static VECTORIZE: AtomicBool = AtomicBool::new(true);
/// and its Select object, where it works,
static OBJECTS: AtomicBool = AtomicBool::new(true);
/// and when they last changed, for `PAGE_JS` to tell it from older ones in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    let (on, local, vectorize) = (ON.load(Ordering::Relaxed), LOCAL.load(Ordering::Relaxed), VECTORIZE.load(Ordering::Relaxed));
    let objects = OBJECTS.load(Ordering::Relaxed);
    format!(r#"{{"on":{on},"local":{local},"vectorize":{vectorize},"objects":{objects},"rev":{}}}"#, REV.load(Ordering::Relaxed))
}

fn local(settings: &Settings) -> bool {
    settings.local_remove_background && crate::background::available()
}

fn objects(settings: &Settings) -> bool {
    settings.local_select_object && crate::models::supported()
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.hide_free_ai_actions, Ordering::Relaxed);
    LOCAL.store(local(settings), Ordering::Relaxed);
    VECTORIZE.store(settings.local_vectorize, Ordering::Relaxed);
    OBJECTS.store(objects(settings), Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_FREE_AI_ACTIONS__", &state())
}

/// Sends changed settings to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    let on = ON.swap(settings.hide_free_ai_actions, Ordering::Relaxed) != settings.hide_free_ai_actions;
    let local = LOCAL.swap(local(settings), Ordering::Relaxed) != local(settings);
    let vectorize = VECTORIZE.swap(settings.local_vectorize, Ordering::Relaxed) != settings.local_vectorize;
    let objects = OBJECTS.swap(objects(settings), Ordering::Relaxed) != objects(settings);
    if !on && !local && !vectorize && !objects {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaFreeAiActions?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
