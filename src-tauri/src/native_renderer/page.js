// Experimental native renderer, the page side (see native_renderer.rs). Sets
// up `window.__ligmaNative` in a Figma file tab, which reads the current
// page's scene through the plugin API, `window.figma`, for Ligma to draw
//
// `eval` can't wait for a promise or carry much at once, so bulk results
// (the scene, image bytes, exports) are kept here under a key, and handed
// to Ligma as each is done through an address on this site that Ligma
// answers itself (see `pushTo`). Where that can't be, Ligma asks for their
// `status`, reads them in `slice`s and `drop`s them
(() => {
  if (window.__ligmaNative) return "ready";
  if (location.hostname !== "www.figma.com" || window.top !== window) return "unavailable";

  // Figma's noise seed, which a file can't change
  const NOISE_SEED = 5277;

  // Figma's modules: `require` and each module's source, for those whose
  // source has `marker`
  let figmaRequire;
  const modulesWith = (marker) => {
    if (!figmaRequire) window.rspackChunk_figma_web_bundler?.push([[Symbol("ligma")], {}, (r) => (figmaRequire = r)]);
    if (!figmaRequire?.m) return [];
    const ids = [];
    for (const id of Object.keys(figmaRequire.m)) {
      try {
        if (Function.prototype.toString.call(figmaRequire.m[id]).includes(marker)) ids.push(id);
      } catch {}
    }
    return ids;
  };
  // The first of the exports of modules with `marker` that has a function
  // `name`. Only modules Figma has loaded already are asked for
  const exportWith = (marker, name) => {
    for (const id of modulesWith(marker)) {
      const exports = figmaRequire(id);
      for (const key of Object.keys(exports ?? {})) {
        try {
          if (typeof exports[key]?.[name] === "function") return exports[key];
        } catch {}
      }
    }
    return null;
  };

  // Figma's packager of custom effects' code, found once among its modules
  // by what it does
  let packager;
  const effectPackager = () => {
    if (packager === undefined) packager = exportWith("_tryS3ArtifactFallback", "ensurePackaged");
    return packager;
  };

  // Figma's pixel preview, as pixels a unit it previews at: 1, 2 for its 2x,
  // or 0 when it's off. Read from Figma's own state, its editor preferences
  // and canvas view, found among its modules by the getters they define
  let previewState = null;
  let previewSought = -Infinity;
  const figmaState = () => {
    if (!previewState && performance.now() - previewSought > 5000) {
      previewSought = performance.now();
      const prefs = exportWith("get activeCanvasPixelPreview(){", "editorPreferences");
      if (prefs) previewState = { prefs, view: exportWith("get shouldStayZoomedToFit(){", "canvasViewState") };
    }
    return previewState;
  };
  const pixelPreview = () => {
    figmaState();
    try {
      if (previewState) {
        if (!previewState.prefs.editorPreferences().activeCanvasPixelPreview.getCopy()) return 0;
        return previewState.view?.canvasViewState().activeCanvasRetinaMode.getCopy() ? 2 : 1;
      }
      return localStorage.getItem("active-canvas-pixel-preview") === "1" ? 1 : 0;
    } catch {
      return 0;
    }
  };

  // The rest of what Figma's renderer is told each frame that's Figma's to
  // set: the pixel grid, and outline mode, where Figma's overlay draws the
  // outlines and the renderer only the editor's background, given as
  // 0xRRGGBB, or -1 when it's off
  let outlineColor = { at: -Infinity, value: 0 };
  const themeBackground = () => {
    if (performance.now() - outlineColor.at > 1000) {
      const context = document.createElement("canvas").getContext("2d");
      context.fillStyle = getComputedStyle(document.body).getPropertyValue("--color-bg").trim() || "#2c2c2c";
      const hex = /^#[0-9a-f]{6}$/i.test(context.fillStyle) ? context.fillStyle : "#2c2c2c";
      outlineColor = { at: performance.now(), value: parseInt(hex.slice(1), 16) };
    }
    return outlineColor.value;
  };
  const display = () => {
    const state = figmaState();
    let grid = true;
    let outlines = false;
    try {
      if (state) {
        grid = !!state.prefs.editorPreferences().renderGrid.getCopy();
        outlines = !!state.view?.canvasViewState().showOutlines.getCopy();
      }
    } catch {}
    return { grid, outline: outlines ? themeBackground() : -1 };
  };

  const api = () => {
    try {
      const figma = window.figma;
      return figma?.currentPage && figma.viewport ? figma : null;
    } catch {
      return null;
    }
  };

  // Figma's noise hashes each octave's seed with `sin` of numbers so large
  // that the result is down to how this GPU rounds, so they're worked out
  // here, on the GPU Figma uses: for each of the four fields, each of the
  // three octaves' seed and its two gradient scales
  let noiseOctaves = null;
  (async () => {
    const device = await (await navigator.gpu?.requestAdapter())?.requestDevice();
    if (!device) return;
    const module = device.createShaderModule({
      code: `
        @group(0) @binding(0) var<storage, read_write> out: array<f32, 36>;
        @group(0) @binding(1) var<uniform> seed: vec4<i32>;
        fn hash(x: f32) -> f32 { return fract(sin(x * 91.3458f + 12.345f) * 43758.547f); }
        @compute @workgroup_size(1) fn main() {
          for (var field = 0; field < 4; field++) {
            for (var octave = 0; octave < 3; octave++) {
              let s = fract(sin(f32(seed.x + field) * 91.3458f + f32(octave) * 57.123f + 12.345f) * 43758.547f);
              let at = (field * 3 + octave) * 3;
              out[at] = s;
              out[at + 1] = hash(s + 2.6457f);
              out[at + 2] = hash(s + 3.6055f);
            }
          }
        }`,
    });
    const pipeline = device.createComputePipeline({ layout: "auto", compute: { module, entryPoint: "main" } });
    const output = device.createBuffer({ size: 144, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC });
    const seed = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
    const readback = device.createBuffer({ size: 144, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
    device.queue.writeBuffer(seed, 0, new Int32Array([NOISE_SEED, 0, 0, 0]));
    const group = device.createBindGroup({
      layout: pipeline.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: { buffer: output } },
        { binding: 1, resource: { buffer: seed } },
      ],
    });
    const encoder = device.createCommandEncoder();
    const pass = encoder.beginComputePass();
    pass.setPipeline(pipeline);
    pass.setBindGroup(0, group);
    pass.dispatchWorkgroups(1);
    pass.end();
    encoder.copyBufferToBuffer(output, 0, readback, 0, 144);
    device.queue.submit([encoder.finish()]);
    await readback.mapAsync(GPUMapMode.READ);
    noiseOctaves = Array.from(new Float32Array(readback.getMappedRange()));
    readback.unmap();
    device.destroy();
  })().catch(() => {});

  const store = new Map();
  let next = 0;
  // Keys differ from the last time the page loaded, as Ligma may still
  // hold what was handed over then
  const session = Math.random().toString(36).slice(2, 8);
  // Where to hand Ligma each result as it's done, an address on this site
  // that Ligma answers itself (see `Pushed` in run.rs), set once it does
  let pushTo = null;
  const push = (key, entry) => {
    if (!pushTo) return;
    entry.pushing = true;
    fetch(location.origin + pushTo + key, { method: "POST", body: entry.bytes ?? entry.data, cache: "no-store", credentials: "omit" })
      .then((response) => {
        if (response.status !== 204) throw new Error(`${response.status}`);
        entry.pushed = true;
        entry.data = "";
        entry.bytes = null;
      })
      .catch(() => (entry.failed = true))
      .finally(() => (entry.pushing = false));
  };
  // Runs `job` and keeps its result under the returned key: a string, or
  // bytes, sent as base64 when they can't be handed over
  const run = (job) => {
    const key = `k${session}-${++next}`;
    const entry = { state: "pending", data: "", bytes: null, error: null, at: performance.now() };
    store.set(key, entry);
    Promise.resolve()
      .then(() => job(key))
      .then((data) => {
        if (data === PUSHED) {
          entry.ms = performance.now() - entry.at;
          entry.pushed = true;
          entry.state = "done";
          return;
        }
        if (data instanceof Uint8Array) entry.bytes = data;
        else entry.data = typeof data === "string" ? data : JSON.stringify(data);
        entry.ms = performance.now() - entry.at;
        entry.state = "done";
        push(key, entry);
      })
      .catch((error) => {
        entry.ms = performance.now() - entry.at;
        entry.error = String(error?.message ?? error);
        entry.state = "error";
      });
    return key;
  };

  // A result to come under `key`, pending until `fill` gives it `data`
  const reserve = (key) => {
    store.set(key, { state: "pending", data: "", bytes: null, error: null, at: performance.now() });
    return key;
  };
  const fill = (key, data) => {
    const entry = store.get(key);
    if (!entry || entry.state !== "pending") return;
    if (data instanceof Uint8Array) entry.bytes = data;
    else entry.data = data;
    entry.ms = performance.now() - entry.at;
    entry.state = "done";
    push(key, entry);
  };

  const base64 = (bytes) => {
    if (bytes.toBase64) return bytes.toBase64();
    let text = "";
    for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(text);
  };

  // Figma downloads image fills from signed addresses that end in the
  // image's hash, which is the SHA-1 of its bytes. While a file opens,
  // getBytesAsync() answers seconds after the download is done, so images
  // are read again from the browser's cache as soon as Figma has them
  const imageUrls = new Map();
  const urlWaiters = new Map();
  const IMAGE_URL = /\/img\/([0-9a-f]{4})\/([0-9a-f]{4})\/([0-9a-f]{32})(?:[?#]|$)/;
  const seen = (url) => {
    const match = IMAGE_URL.exec(url);
    if (!match) return;
    const hash = match[1] + match[2] + match[3];
    if (imageUrls.has(hash)) return;
    imageUrls.set(hash, url);
    for (const resolve of urlWaiters.get(hash) ?? []) resolve(url);
    urlWaiters.delete(hash);
  };
  // Those downloaded before this ran, from the page's timeline, which
  // tells of each only once it's done, and late while the page is busy
  try {
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) seen(entry.name);
    }).observe({ type: "resource", buffered: true });
  } catch {}
  // The rest as Figma starts each download, passed on untouched, so the
  // cache is read the moment it has the image
  try {
    const open = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (method, url, ...rest) {
      try {
        seen(String(url));
      } catch {}
      return open.call(this, method, url, ...rest);
    };
  } catch {}
  const hex = (buffer) => Array.from(new Uint8Array(buffer), (b) => b.toString(16).padStart(2, "0")).join("");
  // Images are read from the cache off the page's main thread, which Figma
  // keeps busy while a file opens: fetched, checked and, given where,
  // handed to Ligma from there
  const WORKER = `
    const hex = ${hex.toString()};
    onmessage = async ({ data: { id, url, hash, to } }) => {
      try {
        const response = await fetch(url, { cache: "force-cache" });
        if (!response.ok) throw new Error(String(response.status));
        const buffer = await response.arrayBuffer();
        if (hex(await crypto.subtle.digest("SHA-1", buffer)) !== hash) throw new Error("not the image");
        if (to) {
          const sent = await fetch(to, { method: "POST", body: buffer, cache: "no-store", credentials: "omit" });
          if (sent.status === 204) return postMessage({ id, pushed: true });
        }
        postMessage({ id, buffer }, [buffer]);
      } catch (error) {
        postMessage({ id, error: String(error?.message ?? error) });
      }
    };`;
  // What a job gives when it's handed its result to Ligma itself
  const PUSHED = Symbol("pushed");
  let worker;
  const workerJobs = new Map();
  let workerNext = 0;
  const inWorker = (message) => {
    if (worker === undefined) {
      try {
        worker = new Worker(URL.createObjectURL(new Blob([WORKER], { type: "text/javascript" })));
        worker.onmessage = ({ data }) => {
          const job = workerJobs.get(data.id);
          workerJobs.delete(data.id);
          if (!job) return;
          if (data.error) job.reject(new Error(data.error));
          else job.resolve(data.pushed ? PUSHED : new Uint8Array(data.buffer));
        };
      } catch {
        worker = null;
      }
    }
    if (!worker) return null;
    return new Promise((resolve, reject) => {
      const id = ++workerNext;
      workerJobs.set(id, { resolve, reject });
      worker.postMessage({ ...message, id });
    });
  };
  // Handed to Ligma under `key` when there's one and Ligma takes them
  const fromCache = async (hash, gaveUp, key) => {
    const url =
      imageUrls.get(hash) ??
      (await new Promise((resolve, reject) => {
        const waiters = urlWaiters.get(hash) ?? [];
        waiters.push(resolve);
        urlWaiters.set(hash, waiters);
        gaveUp.then(() => {
          const left = (urlWaiters.get(hash) ?? []).filter((waiter) => waiter !== resolve);
          if (left.length) urlWaiters.set(hash, left);
          else urlWaiters.delete(hash);
          reject(new Error("no address"));
        });
      }));
    const viaWorker = inWorker({ url, hash, to: key && pushTo ? location.origin + pushTo + key : null });
    if (viaWorker) return viaWorker;
    const response = await fetch(url, { cache: "force-cache" });
    if (!response.ok) throw new Error(`${response.status}`);
    const buffer = await response.arrayBuffer();
    if (hex(await crypto.subtle.digest("SHA-1", buffer)) !== hash) throw new Error("not the image");
    return new Uint8Array(buffer);
  };
  // An image fill's original bytes, from whichever comes first, or PUSHED
  const imageBytes = async (figma, hash, key) => {
    let stop;
    const gaveUp = new Promise((resolve) => (stop = resolve));
    // Without the plugin API, as in files that can only be viewed, only
    // from the cache
    if (!figma) return fromCache(hash, new Promise((resolve) => setTimeout(resolve, 30000)), key);
    const fromFigma = async () => {
      const image = figma.getImageByHash(hash);
      if (!image) throw new Error("no image");
      return image.getBytesAsync();
    };
    try {
      return await Promise.any([fromCache(hash, gaveUp, key), fromFigma()]);
    } catch (error) {
      throw error.errors?.[1] ?? error;
    } finally {
      stop();
    }
  };

  // Image fills' bytes for images near the view, started as soon as the
  // page is read so they're ready by the time Ligma asks. Those in view are
  // handed to Ligma under `i` and the hash where they can be, before it
  // asks, as asking waits behind Figma while it opens a file
  const prefetched = new Map();
  let prefetchArea = null;
  let viewArea = null;
  const prefetch = (figma, hash, inView) => {
    if (prefetched.has(hash) || prefetched.size >= 96) return;
    const ready = imageBytes(figma, hash, inView ? `i${hash}` : null);
    ready.catch(() => {});
    prefetched.set(hash, ready);
  };
  const overlaps = (b, a) => a && b && b.x < a.x + a.width && b.x + b.width > a.x && b.y < a.y + a.height && b.y + b.height > a.y;

  // Lets Figma draw and take input, and Ligma collect what's read so far,
  // in turn. A timer would wait at least 4ms each time, a quarter of every
  // slice of work, and scheduler.yield() would go ahead of Ligma
  const channel = new MessageChannel();
  const waiting = [];
  channel.port1.onmessage = () => waiting.shift()?.();
  const yieldNow = () =>
    new Promise((resolve) => {
      waiting.push(resolve);
      channel.port2.postMessage(0);
    });

  // Numbers are kept short in the JSON, but to Figma's own precision, as at
  // high zoom a thousandth of a unit is pixels, more so across a rotated node
  const round = (value) => +value.toPrecision(9);
  const path = (data) => data.replace(/-?\d+\.\d{6,}/g, (number) => String(Math.round(+number * 1e5) / 1e5));

  // One node as Ligma's scene reads it (see `scene.rs`), without children
  const node = (figma, n, parent) => {
    const out = { id: n.id, t: n.type, p: parent };
    if (n.visible === false) {
      out.hidden = true;
      return out;
    }
    const get = (key) => {
      try {
        const value = n[key];
        return value === figma.mixed ? "MIXED" : value;
      } catch {
        return undefined;
      }
    };
    const transform = get("absoluteTransform");
    if (transform) out.tf = [transform[0][0], transform[1][0], transform[0][1], transform[1][1], transform[0][2], transform[1][2]].map(round);
    if ("width" in n) {
      out.w = round(n.width);
      out.h = round(n.height);
    }
    // Sections have no render bounds, nor do masks, whose strokes and
    // effects can reach past their box. What a mask shows is in its parent's
    let bounds = get("absoluteRenderBounds");
    if (!bounds && n.isMask) bounds = n.parent?.absoluteRenderBounds;
    bounds ??= get("absoluteBoundingBox");
    if (bounds) out.rb = [bounds.x, bounds.y, bounds.width, bounds.height].map(round);
    if ("opacity" in n && n.opacity !== 1) out.o = n.opacity;
    if ("blendMode" in n && n.blendMode !== "PASS_THROUGH" && n.blendMode !== "NORMAL") out.bm = n.blendMode;
    if (n.clipsContent) out.clip = true;
    if (n.isMask) out.mask = get("maskType") ?? "ALPHA";
    if ("fills" in n) {
      out.fills = get("fills");
      if (prefetchArea && Array.isArray(out.fills) && out.fills.some((paint) => paint.type === "IMAGE" && paint.imageHash)) {
        const b = n.absoluteBoundingBox;
        if (overlaps(b, prefetchArea)) {
          const inView = overlaps(b, viewArea);
          for (const paint of out.fills) if (paint.type === "IMAGE" && paint.imageHash && paint.visible !== false) prefetch(figma, paint.imageHash, inView);
        }
      }
      if (out.fills === "MIXED" && n.type === "TEXT") {
        // Each style's fills, which match its outlines in `fg` in order
        try {
          out.segs = n.getStyledTextSegments(["fills"]).map((segment) => segment.fills);
        } catch {}
      }
      if (n.type === "VECTOR") {
        // A vector's regions can each have their own fills, which Figma
        // draws instead of the vector's. Its outlines are its regions'
        try {
          const regions = n.vectorNetwork.regions;
          const own = JSON.stringify(out.fills);
          if (regions.some((region) => region.fills?.length && JSON.stringify(region.fills) !== own) && regions.length === n.fillGeometry.length) {
            out.fills = "MIXED";
            out.segs = regions.map((region) => region.fills ?? []);
          }
        } catch {}
      }
      if (Array.isArray(out.segs)) for (const fills of out.segs) for (const paint of fills) if (paint.type === "PATTERN" && paint.visible !== false) patternSources.add(paint.sourceNodeId);
      if (Array.isArray(out.fills)) for (const paint of out.fills) if (paint.type === "PATTERN" && paint.visible !== false) patternSources.add(paint.sourceNodeId);
    }
    if ("strokes" in n && n.strokes.length) {
      out.strokes = get("strokes");
      out.sa = get("strokeAlign");
    }
    if ("effects" in n) {
      const effects = (get("effects") ?? []).filter((effect) => effect.visible !== false);
      if (effects.length) out.fx = effects;
    }
    if ("fillGeometry" in n) {
      const fill = get("fillGeometry");
      if (fill?.length) out.fg = fill.map((g) => [g.windingRule, path(g.data)]);
      // Outlining strokes costs Figma more than anything else read here,
      // text's most of all, so only for what has strokes to draw
      if (Array.isArray(out.strokes) && out.strokes.some((paint) => paint.visible !== false)) {
        const stroke = get("strokeGeometry");
        if (stroke?.length) out.sg = stroke.map((g) => [g.windingRule, path(g.data)]);
      }
    }
    if ("cornerRadius" in n) {
      const radius = get("cornerRadius");
      const corner = (value) => (Number.isFinite(value) ? value : 0);
      out.cr =
        radius === "MIXED"
          ? [n.topLeftRadius, n.topRightRadius, n.bottomRightRadius, n.bottomLeftRadius].map(corner)
          : radius
            ? [radius, radius, radius, radius].map(corner)
            : undefined;
    }
    if (n.cornerSmoothing > 0) out.cs = n.cornerSmoothing;
    if (n.type === "TEXT") {
      // Only for the record: the outlines in `fg` are what's drawn
      out.chars = n.characters.length;
    }
    return out;
  };

  // `root` and everything under it in paint order, children after their
  // parent, `p` being the parent's id. Boolean operations are drawn from
  // their own geometry, so their operands are left out. Gives the page a
  // breather every `clock.slice` ms, 12 unless said otherwise. Null if
  // `clock.stop()` says to give up on it for now
  const subtree = async (figma, root, parent, out, clock) => {
    const stack = [[root, parent]];
    while (stack.length) {
      const [n, p] = stack.pop();
      const entry = node(figma, n, p);
      out.push(entry);
      if (!entry.hidden && "children" in n && n.type !== "BOOLEAN_OPERATION") {
        const children = n.children;
        for (let i = children.length - 1; i >= 0; i--) stack.push([children[i], n.id]);
      }
      if (clock && performance.now() - clock.start > (clock.slice ?? 12)) {
        const paused = performance.now();
        clock.work = (clock.work ?? 0) + paused - clock.start;
        await yieldNow();
        clock.start = performance.now();
        clock.waited = (clock.waited ?? 0) + clock.start - paused;
        if (clock.stop?.()) return null;
      }
    }
    return out;
  };

  // Pattern fills' source nodes seen, and those sent from other pages
  let patternSources = new Set();
  let sentSources = new Set();

  // Subtrees of pattern sources on other pages not sent yet, as items
  const externalItems = async (figma, clock) => {
    const items = [];
    for (const id of patternSources) {
      if (sentSources.has(id)) continue;
      sentSources.add(id);
      const source = findNode(figma, id);
      if (!source || onPage(figma, source)) continue;
      items.push({ parent: "__ext", index: 0, nodes: await subtree(figma, source, "__ext", [], clock) });
    }
    return items;
  };

  // The page streaming in, nearest the view first, in batches kept under
  // keys Ligma knows to wait for
  let stream = null;
  const streamPage = async (figma, page, current) => {
    const units = [];
    const shells = [];
    // Sections are sent alone first, then what's in them as units of their
    // own
    const collect = (parent) => {
      parent.children.forEach((n, index) => {
        if (n.type === "SECTION" && n.visible !== false) {
          shells.push({ parent: parent.id, index, nodes: [node(figma, n, parent.id)] });
          collect(n);
        } else {
          units.push([n, parent.id, index]);
        }
      });
    };
    collect(page);
    // Nearest the view first, sorted again whenever the view moves, as
    // Figma puts the view where it was left a moment after the file opens,
    // and people move about while the page is read
    const left = units.map((unit) => ({ unit, box: unit[0].absoluteBoundingBox, distance: 0, seen: false }));
    // What's in view first, then nearest the view's center, farthest at the
    // start so the next comes off the end
    const order = (a, b) => a.seen - b.seen || b.distance - a.distance;
    let aimedAt = "";
    let aimed = 0;
    // When the view last moved, as Figma has to be let draw and take input
    // more often while it does
    let moved = 0;
    const distance = (b, cx, cy) => (b ? Math.hypot(Math.max(b.x - cx, 0, cx - (b.x + b.width)), Math.max(b.y - cy, 0, cy - (b.y + b.height))) : Infinity);
    const aim = () => {
      aimed = performance.now();
      const v = figma.viewport.bounds;
      const at = `${v.x},${v.y},${v.width},${v.height}`;
      if (at === aimedAt) return false;
      aimedAt = at;
      moved = performance.now();
      prefetchArea = { x: v.x - v.width / 2, y: v.y - v.height / 2, width: v.width * 2, height: v.height * 2 };
      viewArea = { x: v.x, y: v.y, width: v.width, height: v.height };
      const cx = v.x + v.width / 2;
      const cy = v.y + v.height / 2;
      for (const entry of left) {
        entry.distance = distance(entry.box, cx, cy);
        entry.seen = overlaps(entry.box, viewArea);
      }
      left.sort(order);
      return true;
    };
    // A frame much bigger than the view and not all in it is sent alone,
    // and what's in it as units of its own, so what of it is in view comes
    // first rather than all of it in order
    const containers = new Set(["FRAME", "GROUP", "COMPONENT", "COMPONENT_SET", "INSTANCE"]);
    const split = (entry) => {
      const [n, parent, index] = entry.unit;
      const b = entry.box;
      const v = viewArea;
      if (!b || !v || !containers.has(n.type) || n.visible === false) return false;
      const inside = b.x >= v.x && b.y >= v.y && b.x + b.width <= v.x + v.width && b.y + b.height <= v.y + v.height;
      if (inside || b.width * b.height < (v.width * v.height) / 16) return false;
      let children;
      try {
        children = n.children;
      } catch {
        return false;
      }
      if (children.length < 2) return false;
      items.push({ parent, index, nodes: [node(figma, n, parent)] });
      const cx = v.x + v.width / 2;
      const cy = v.y + v.height / 2;
      children.forEach((child, i) => {
        const box = child.absoluteBoundingBox;
        left.push({ unit: [child, n.id, i], box, distance: distance(box, cx, cy), seen: overlaps(box, v) });
      });
      left.sort(order);
      return true;
    };
    aim();
    let items = shells;
    let clock = { start: performance.now() };
    current.left = () => ({ left: left.length, seen: left.filter((entry) => entry.seen).length, since: performance.now() - moved });
    let since = performance.now();
    let first = true;
    // Batch n goes under `${current.prefix}n`, kept for it before it's
    // read, so Ligma can wait for it there. The one after the last says
    // `{ done, ms }`
    let n = 0;
    const flush = () => {
      if (!items.length) return;
      fill(`${current.prefix}${n}`, JSON.stringify({ items }));
      reserve(`${current.prefix}${++n}`);
      items = [];
    };
    flush();
    while (left.length) {
      if (stream !== current) return fill(`${current.prefix}${n}`, JSON.stringify({ error: "the page was read again" }));
      if (performance.now() - aimed > 50) aim();
      const picked = left.pop();
      if (split(picked)) continue;
      const [root, parent, index] = picked.unit;
      // What's in view is read in longer stretches, as each breather while
      // Figma opens a file can take it 100ms. Something out of view is put
      // back when the view moves onto something else
      const inView = picked.seen;
      clock.slice = inView ? 100 : performance.now() - moved < 1000 ? 12 : 40;
      clock.stop = () => {
        if (inView || performance.now() - aimed < 50 || !aim() || !left.length || !left[left.length - 1].seen) return false;
        return !overlaps(picked.box, viewArea);
      };
      const nodes = await subtree(figma, root, parent, [], clock);
      clock.stop = null;
      clock.slice = 12;
      if (!nodes) {
        left.push(picked);
        aimedAt = "";
        aim();
        continue;
      }
      items.push({ parent, index, nodes });
      items.push(...(await externalItems(figma, clock)));
      // The first batch soon, for what's in view, then bigger ones, as each
      // costs Ligma a few round trips to read
      const elapsed = performance.now() - current.started;
      if (inView || performance.now() - since > (first ? 40 : elapsed < 2000 ? 150 : 600)) {
        flush();
        since = performance.now();
        first = false;
      }
    }
    flush();
    current.ms = performance.now() - current.started;
    fill(`${current.prefix}${n}`, JSON.stringify({ done: true, ms: current.ms, work: clock.work ?? 0, waited: clock.waited ?? 0 }));
  };

  // Changes since the last `changes()`, from Figma's `documentchange`
  let dirty = new Set();
  let removed = new Set();
  let resync = true;
  let watching = false;
  const watch = (figma) => {
    if (watching) return;
    watching = true;
    const onChanges = (changes) => {
      for (const change of changes) {
        if (change.type === "DELETE") removed.add(change.id);
        else if (change.type === "CREATE" || change.type === "PROPERTY_CHANGE") dirty.add(change.id);
        else resync = true;
      }
    };
    try {
      figma.on("documentchange", (event) => onChanges(event.documentChanges));
    } catch {
      // Without every page loaded, only the current page's changes
      try {
        figma.currentPage.on("nodechange", (event) => onChanges(event.nodeChanges));
      } catch {}
    }
    figma.on("currentpagechange", () => {
      resync = true;
    });
  };

  const findNode = (figma, id) => {
    try {
      return figma.getNodeById(id);
    } catch {
      return null;
    }
  };

  const onPage = (figma, n) => {
    for (let at = n; at; at = at.parent) if (at.type === "PAGE") return at.id === figma.currentPage.id;
    return false;
  };

  window.__ligmaNative = {
    version: 1,

    // The viewport and canvas, polled for every frame drawn: world bounds,
    // zoom, the canvas's CSS size and place in the page, device pixels per
    // CSS pixel, the page, and the pixel preview
    view() {
      const figma = api();
      if (!figma) return null;
      const { x, y, width, height } = figma.viewport.bounds;
      const canvas = document.querySelector("canvas");
      const rect = canvas?.getBoundingClientRect();
      return {
        x,
        y,
        w: width,
        h: height,
        zoom: figma.viewport.zoom,
        cx: rect?.left ?? 0,
        cy: rect?.top ?? 0,
        cw: rect?.width ?? width * figma.viewport.zoom,
        ch: rect?.height ?? height * figma.viewport.zoom,
        dpr: devicePixelRatio,
        page: figma.currentPage.id,
        visible: document.visibilityState === "visible",
        preview: pixelPreview(),
        ...display(),
      };
    },

    // Dev tools: how much of the page is still to read, and of it in view
    streaming() {
      return stream?.left?.() ?? null;
    },

    // Starts streaming the current page in, its batches kept under
    // `${stream}0`, `${stream}1`... and returns that with what's needed
    // first: `{ stream, page, bg, transparent, dark }`
    snapshot() {
      const figma = api();
      if (!figma) return null;
      watch(figma);
      dirty = new Set();
      removed = new Set();
      resync = false;
      patternSources = new Set();
      sentSources = new Set();
      const page = figma.currentPage;
      const background = page.backgrounds?.find((paint) => paint.type === "SOLID" && paint.visible !== false);
      const current = { prefix: `s${session}-${++next}-`, started: performance.now(), ms: 0 };
      stream = current;
      reserve(`${current.prefix}0`);
      streamPage(figma, page, current).catch((error) => {
        // Whichever batch is being waited for says so
        for (const [key, entry] of store) {
          if (key.startsWith(current.prefix) && entry.state === "pending") fill(key, JSON.stringify({ error: String(error?.message ?? error) }));
        }
      });
      return {
        stream: current.prefix,
        page: page.id,
        name: page.name,
        bg: background && { ...background.color, a: background.opacity ?? 1 },
        // Figma shows a checkerboard for a hidden background, darker in its
        // dark theme
        transparent: !background,
        dark: document.body?.dataset.preferredTheme === "dark",
        octaves: noiseOctaves,
        // Its colors are Display P3, which Figma's renderer shows in sRGB
        p3: figma.root.documentColorProfile === "DISPLAY_P3",
      };
    },

    // Changes since the last snapshot or call: `{ resync }` when the page
    // has to be read again, or `{ updates, removed }`, each update a
    // subtree that replaces its old self, placed at `index` in its parent
    changes() {
      const figma = api();
      if (!figma) return null;
      if (resync) return { resync: true };
      if (!dirty.size && !removed.size) return { updates: [], removed: [] };
      const ids = dirty;
      const gone = [...removed];
      dirty = new Set();
      removed = new Set();
      return run(async () => {
        const roots = [];
        for (const id of ids) {
          const n = findNode(figma, id);
          if (!n || n.removed || n.type === "PAGE" || !onPage(figma, n)) continue;
          // An ancestor read again covers it
          let covered = false;
          for (let at = n.parent; at && !covered; at = at.parent) covered = ids.has(at.id);
          if (!covered) roots.push(n);
        }
        const updates = [];
        const clock = { start: performance.now() };
        for (const n of roots) {
          // A boolean operation's operand changes the operation
          let root = n;
          for (let at = n.parent; at && at.type !== "PAGE"; at = at.parent) if (at.type === "BOOLEAN_OPERATION") root = at;
          const parent = root.parent;
          const nodes = await subtree(figma, root, parent.id, [], clock);
          updates.push({ parent: parent.id, index: parent.children.indexOf(root), nodes });
        }
        return { updates, removed: gone };
      });
    },

    // An image fill's original bytes, or `{ under }`, the key they were
    // handed over under before Ligma asked
    image(hash) {
      return run(async (key) => {
        const ready = prefetched.get(hash);
        if (ready) {
          prefetched.delete(hash);
          const bytes = await ready;
          return bytes === PUSHED ? { under: `i${hash}` } : bytes;
        }
        return await imageBytes(api(), hash, key);
      });
    },

    // Node `id` as Figma renders it, effects and all, about `width` pixels
    // wide and no more than `limit` on its longer side: `{ png, rb, scale }`.
    // For custom effects, which are code the file carries and Figma runs
    render(id, width, limit) {
      return run(async () => {
        const figma = api();
        const n = figma && findNode(figma, id);
        if (!n) throw new Error("no node");
        const b = n.absoluteRenderBounds ?? n.absoluteBoundingBox;
        if (!b || !b.width || !b.height) throw new Error("no bounds");
        const scale = Math.min(width / b.width, limit / Math.max(b.width, b.height));
        const bytes = await n.exportAsync({ format: "PNG", constraint: { type: "SCALE", value: scale } });
        return { png: base64(bytes), rb: [b.x, b.y, b.width, b.height], scale };
      });
    },

    // The code of custom effect `id` (as the plugin API names it), bundled
    // as Figma bundles it to run it: `{ code, features }`
    effectCode(id) {
      return run(async () => {
        const packager = effectPackager();
        if (!packager) throw new Error("no effect packager");
        const component = `CodeComponentId:${id}`;
        // Its files may still be downloading, so it's asked for a few times
        for (let attempt = 0; attempt < 40; attempt++) {
          try {
            packager.prioritize?.(component);
          } catch {}
          const packaged = await packager.ensurePackaged(component);
          if (packaged?.code) return { code: packaged.code, features: packaged.features ?? null };
          packager.invalidate?.(component);
          await new Promise((resolve) => setTimeout(resolve, 250));
        }
        throw new Error("effect code didn't come");
      });
    },

    // Figma's own render of node `id` as a PNG at `scale`, base64, with the
    // bounds it covers: `{ png, rb }`
    exportNode(id, scale) {
      return run(async () => {
        const figma = api();
        const n = figma && findNode(figma, id);
        if (!n) throw new Error("no node");
        const bytes = await n.exportAsync({ format: "PNG", constraint: { type: "SCALE", value: scale } });
        const b = n.absoluteRenderBounds;
        return { png: base64(bytes), rb: b && [b.x, b.y, b.width, b.height] };
      });
    },

    // Pans and zooms Figma's canvas along a fixed path for `ms`, and times
    // the page's frames (requestAnimationFrame) as it goes. `{ frames,
    // intervals }`
    bench(ms) {
      return run(async () => {
        const figma = api();
        if (!figma) throw new Error("no plugin API");
        const start = { center: figma.viewport.center, zoom: figma.viewport.zoom };
        const span = figma.viewport.bounds.width;
        const intervals = [];
        let last = null;
        const t0 = performance.now();
        await new Promise((resolve) => {
          const frame = (now) => {
            if (last !== null) intervals.push(now - last);
            last = now;
            const t = (now - t0) / ms;
            if (t >= 1) return resolve();
            // Across half a screen and back, zooming out to half and in again
            const phase = Math.sin(t * Math.PI * 4);
            figma.viewport.center = { x: start.center.x + phase * span * 0.5, y: start.center.y + Math.sin(t * Math.PI * 2) * span * 0.2 };
            figma.viewport.zoom = start.zoom * (0.75 + 0.25 * Math.cos(t * Math.PI * 2));
            requestAnimationFrame(frame);
          };
          requestAnimationFrame(frame);
        });
        figma.viewport.zoom = start.zoom;
        figma.viewport.center = start.center;
        return { frames: intervals.length, intervals };
      });
    },

    // Node ids for the parity harness: top-level frames and a spread of
    // visible layers under them, `count` in all, each at least 8 by 8
    sample(count) {
      return run(async () => {
        const figma = api();
        const page = figma.currentPage;
        const picked = [];
        const top = page.children.filter((n) => n.visible !== false && n.absoluteRenderBounds);
        for (const n of top.slice(0, Math.ceil(count / 3))) picked.push(n.id);
        const all = page.findAll((n) => {
          if (n.visible === false || n.type === "SLICE") return false;
          const b = n.absoluteRenderBounds;
          return !!b && b.width >= 8 && b.height >= 8;
        });
        // Every k-th, for an even spread through the page
        const step = Math.max(1, Math.floor(all.length / Math.max(1, count - picked.length)));
        for (let i = 0; i < all.length && picked.length < count; i += step) if (!picked.includes(all[i].id)) picked.push(all[i].id);
        return picked;
      });
    },

    // For the parity harness: up to `each` nodes with each thing Figma
    // draws that the page has, as `{ id, feature }`, so every one is checked
    features(each) {
      return run(async () => {
        const figma = api();
        const picked = new Map();
        const pick = (feature, n) => {
          const list = picked.get(feature) ?? [];
          if (list.length < each && !list.includes(n.id)) list.push(n.id);
          picked.set(feature, list);
        };
        const visible = (paints) => (Array.isArray(paints) ? paints.filter((p) => p.visible !== false) : []);
        const nodes = figma.currentPage.findAll((n) => {
          if (n.visible === false || n.type === "SLICE") return false;
          const b = n.absoluteRenderBounds;
          return !!b && b.width >= 8 && b.height >= 8 && b.width <= 4000 && b.height <= 4000;
        });
        // Spread through the page rather than the first few of each
        for (let i = nodes.length - 1; i > 0; i--) {
          const j = (i * 7919) % (i + 1);
          [nodes[i], nodes[j]] = [nodes[j], nodes[i]];
        }
        for (const n of nodes) {
          pick(`type ${n.type}`, n);
          for (const e of n.effects ?? []) {
            if (e.visible === false) continue;
            pick(`effect ${e.type}${e.blurType === "PROGRESSIVE" ? " progressive" : ""}${e.spread ? " spread" : ""}${e.showShadowBehindNode ? " behind" : ""}`, n);
          }
          for (const key of ["fills", "strokes"]) {
            if (n[key] === figma.mixed) pick(`${key} mixed`, n);
            for (const p of visible(n[key])) {
              pick(`${key} ${p.type}${p.scaleMode ? " " + p.scaleMode : ""}`, n);
              if (p.blendMode && p.blendMode !== "NORMAL") pick(`paint blend ${p.blendMode}`, n);
              if (p.filters && Object.values(p.filters).some((v) => v)) pick("image adjustments", n);
            }
          }
          if (n.isMask) pick(`mask ${n.maskType}`, n);
          if (n.blendMode && n.blendMode !== "PASS_THROUGH" && n.blendMode !== "NORMAL") pick(`blend ${n.blendMode}`, n);
          if (visible(n.strokes).length) {
            pick(`stroke ${n.strokeAlign}`, n);
            if (n.dashPattern?.length) pick("stroke dashed", n);
            if (n.strokeWeight === figma.mixed) pick("stroke per side", n);
            if (n.strokeCap && n.strokeCap !== "NONE") pick("stroke caps", n);
          }
          if (n.cornerSmoothing > 0) pick("corner smoothing", n);
          if (n.clipsContent) pick("clips content", n);
          if (n.type === "TEXT") {
            const segments = n.getStyledTextSegments(["textDecoration"]);
            if (segments.some((s) => s.textDecoration !== "NONE")) pick("text decoration", n);
            if (segments.length > 1) pick("text styled runs", n);
            if (visible(n.strokes).length) pick("text stroke", n);
          }
          if (n.rotation) pick("rotated", n);
          const t = n.relativeTransform;
          if (t && t[0][0] * t[1][1] - t[0][1] * t[1][0] < 0) pick("flipped", n);
        }
        const out = [];
        for (const [feature, ids] of picked) for (const id of ids) out.push({ id, feature });
        return out;
      });
    },

    // Figma's noise octaves, once worked out, see `noiseOctaves`
    octaves() {
      return noiseOctaves;
    },

    // What Figma's renderer is told that's Figma's to set, for page_hook.js
    // to send with each frame: the pixel preview's pixels a unit, the pixel
    // grid, and outline mode's background
    settings() {
      return { preview: pixelPreview(), ...display() };
    },

    // Hands Ligma results at `path` on this site from now on
    pushTo(path) {
      pushTo = path;
      for (const [key, entry] of store) if (entry.state === "done" && !entry.pushing && !entry.pushed) push(key, entry);
      return true;
    },
    status(key) {
      const entry = store.get(key);
      if (!entry) return { state: "missing" };
      const waiting = entry.pushing || entry.pushed;
      // Bytes not handed over are read as base64
      if (entry.state === "done" && entry.bytes && !waiting && !entry.data) entry.data = base64(entry.bytes);
      return {
        state: entry.state,
        len: entry.data.length,
        binary: !!entry.bytes,
        push: !!pushTo,
        pushing: !!entry.pushing,
        pushed: !!entry.pushed,
        error: entry.error,
        ms: entry.ms,
        age: performance.now() - entry.at,
      };
    },
    slice(key, start, end) {
      return store.get(key)?.data.slice(start, end) ?? null;
    },
    drop(key) {
      store.delete(key);
    },
  };
  // The view, written to memory Ligma shares with the page (see
  // `webview2::share`) after every frame Figma draws, so Ligma follows it
  // the moment it moves without asking. Slot 0 counts writes, odd while one
  // is under way, then x, y, w, h, zoom, cx, cy, cw, ch, dpr, visible, the
  // page id's two numbers, when it was written, and the pixel preview
  let shared = null;
  let written = [];
  const writeView = () => {
    const v = shared && window.__ligmaNative.view();
    if (!v) return;
    const [a, b] = v.page.split(":").map(Number);
    const values = [v.x, v.y, v.w, v.h, v.zoom, v.cx, v.cy, v.cw, v.ch, v.dpr, v.visible ? 1 : 0, a, b, v.preview, v.grid ? 1 : 0, v.outline];
    if (values.every((value, i) => value === written[i])) return;
    written = values;
    shared[0] += 1;
    for (let i = 0; i < 13; i++) shared[i + 1] = values[i];
    shared[14] = performance.timeOrigin + performance.now();
    shared[15] = v.preview;
    shared[16] = v.grid ? 1 : 0;
    shared[17] = v.outline;
    shared[0] += 1;
  };
  // Read once Figma has drawn the frame, as it moves the view before
  const afterFrame = new MessageChannel();
  afterFrame.port1.onmessage = () => {
    writeView();
    requestAnimationFrame(nextFrame);
  };
  const nextFrame = () => afterFrame.port2.postMessage(0);
  document.addEventListener("visibilitychange", writeView);
  window.chrome?.webview?.addEventListener("sharedbufferreceived", (event) => {
    if (!event.additionalData?.ligmaNative) return;
    const first = !shared;
    shared = new Float64Array(event.getBuffer());
    if (shared[0] % 2) shared[0] += 1;
    written = [];
    if (first) requestAnimationFrame(nextFrame);
  });
  // New to this page, so anything read from an earlier one is out of date
  return "installed";
})()
