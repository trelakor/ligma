// Experimental native renderer, the page side (see native_renderer.rs). Sets
// up `window.__ligmaNative` in a Figma file tab, which reads the current
// page's scene through the plugin API, `window.figma`, for Ligma to draw
//
// `eval` can't wait for a promise or carry much at once, so bulk results
// (the scene, image bytes, exports) are kept here as strings under a key.
// Ligma asks for their `status`, reads them in `slice`s and `drop`s them
(() => {
  if (window.__ligmaNative) return "ready";
  if (location.hostname !== "www.figma.com" || window.top !== window) return "unavailable";

  const api = () => {
    try {
      const figma = window.figma;
      return figma?.currentPage && figma.viewport ? figma : null;
    } catch {
      return null;
    }
  };

  const store = new Map();
  let next = 0;
  // Runs `job` and keeps its result, a string, under the returned key
  const run = (job) => {
    const key = `k${++next}`;
    const entry = { state: "pending", data: "", error: null, at: performance.now() };
    store.set(key, entry);
    Promise.resolve()
      .then(job)
      .then((data) => {
        entry.data = typeof data === "string" ? data : JSON.stringify(data);
        entry.state = "done";
      })
      .catch((error) => {
        entry.error = String(error?.message ?? error);
        entry.state = "error";
      })
      .finally(() => (entry.ms = performance.now() - entry.at));
    return key;
  };

  // Keeps `data`, done, under a new key
  const put = (data) => {
    const key = `k${++next}`;
    store.set(key, { state: "done", data, error: null, at: performance.now(), ms: 0 });
    return key;
  };

  const base64 = (bytes) => {
    let text = "";
    for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(text);
  };

  const yieldNow = () => new Promise((resolve) => setTimeout(resolve, 0));

  // Rounds to 1/1000, which is far below a pixel at any zoom Figma has, to
  // keep the JSON short
  const round = (value) => Math.round(value * 1000) / 1000;
  const path = (data) => data.replace(/-?\d+\.\d{4,}/g, (number) => String(round(+number)));

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
    // Sections have no render bounds
    const bounds = get("absoluteRenderBounds") ?? get("absoluteBoundingBox");
    if (bounds) out.rb = [bounds.x, bounds.y, bounds.width, bounds.height].map(round);
    if ("opacity" in n && n.opacity !== 1) out.o = n.opacity;
    if ("blendMode" in n && n.blendMode !== "PASS_THROUGH" && n.blendMode !== "NORMAL") out.bm = n.blendMode;
    if (n.clipsContent) out.clip = true;
    if (n.isMask) out.mask = get("maskType") ?? "ALPHA";
    if ("fills" in n) {
      out.fills = get("fills");
      if (out.fills === "MIXED" && n.type === "TEXT") {
        // Each style's fills, which match its outlines in `fg` in order
        try {
          out.segs = n.getStyledTextSegments(["fills"]).map((segment) => segment.fills);
        } catch {}
      }
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
      const stroke = get("strokeGeometry");
      if (stroke?.length) out.sg = stroke.map((g) => [g.windingRule, path(g.data)]);
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
    if (n.type === "TEXT") {
      // Only for the record: the outlines in `fg` are what's drawn
      out.chars = n.characters.length;
    }
    return out;
  };

  // `root` and everything under it in paint order, children after their
  // parent, `p` being the parent's id. Boolean operations are drawn from
  // their own geometry, so their operands are left out. Gives the page a
  // breather every 12ms
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
      if (clock && performance.now() - clock.start > 12) {
        await yieldNow();
        clock.start = performance.now();
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
  // keys for `batch()`
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
    const v = figma.viewport.bounds;
    const cx = v.x + v.width / 2;
    const cy = v.y + v.height / 2;
    const distance = (n) => {
      const b = n.absoluteBoundingBox;
      if (!b) return Infinity;
      const dx = Math.max(b.x - cx, 0, cx - (b.x + b.width));
      const dy = Math.max(b.y - cy, 0, cy - (b.y + b.height));
      return Math.hypot(dx, dy);
    };
    const keyed = units.map((unit) => [distance(unit[0]), unit]);
    keyed.sort((a, b) => a[0] - b[0]);
    let items = shells;
    let clock = { start: performance.now() };
    let since = performance.now();
    let first = true;
    const flush = () => {
      if (!items.length) return;
      current.batches.push(put(JSON.stringify({ items })));
      items = [];
    };
    flush();
    for (const [, [n, parent, index]] of keyed) {
      if (stream !== current) return;
      items.push({ parent, index, nodes: await subtree(figma, n, parent, [], clock) });
      items.push(...(await externalItems(figma, clock)));
      // The first batch soon, for what's in view, then bigger ones, as each
      // costs Ligma a few round trips to read
      const elapsed = performance.now() - current.started;
      if (performance.now() - since > (first ? 40 : elapsed < 2000 ? 150 : 600)) {
        flush();
        since = performance.now();
        first = false;
      }
    }
    flush();
    current.ms = performance.now() - current.started;
    current.done = true;
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
    // CSS pixel, and the page
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
      };
    },

    // Starts streaming the current page in, for `batch()`, and returns what's
    // needed first: `{ page, bg, transparent, dark }`
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
      const current = { batches: [], done: false, error: null, started: performance.now(), ms: 0 };
      stream = current;
      streamPage(figma, page, current).catch((error) => (current.error = String(error?.message ?? error)));
      return {
        page: page.id,
        name: page.name,
        bg: background && { ...background.color, a: background.opacity ?? 1 },
        // Figma shows a checkerboard for a hidden background, darker in its
        // dark theme
        transparent: !background,
        dark: document.body?.dataset.preferredTheme === "dark",
      };
    },

    // The key of the next batch of the page streaming in, `{ done, ms }`
    // once it's all in, or null while the next isn't ready
    batch() {
      if (!stream) return { done: true, ms: 0 };
      if (stream.batches.length) return stream.batches.shift();
      if (stream.error) return { error: stream.error };
      return stream.done ? { done: true, ms: stream.ms } : null;
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

    // An image fill's original bytes, base64
    image(hash) {
      return run(async () => {
        const figma = api();
        const image = figma?.getImageByHash(hash);
        if (!image) throw new Error("no image");
        return base64(await image.getBytesAsync());
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

    status(key) {
      const entry = store.get(key);
      if (!entry) return { state: "missing" };
      return { state: entry.state, len: entry.data.length, error: entry.error, ms: entry.ms };
    },
    slice(key, start, end) {
      return store.get(key)?.data.slice(start, end) ?? null;
    },
    drop(key) {
      store.delete(key);
    },
  };
  return "ready";
})()
