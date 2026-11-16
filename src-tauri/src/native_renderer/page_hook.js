// Experimental native renderer, the part that runs as a Figma tab starts
// (see native_renderer.rs). Ligma's renderer takes the place of Figma's here
//
// Figma's threaded renderer has its main thread send each frame to a render
// worker: the scene as a render tree (Kiwi, see rt.rs), what goes over it
// (selection, handles, labels, as `uiCommands`, see overlay.rs) and the
// view, which the worker draws and answers. Its gate is turned on here.
// Every frame goes to Ligma once it asks (`push`). Once Ligma says it's
// drawing (`replace(true)`), frames no longer go to Figma's worker at all:
// Ligma answers them as the worker would and draws them, scene and overlay,
// under the tab, where they show through its canvas
//
// When anything here doesn't look as expected, it stays out of the way and
// Figma renders as it always has
(() => {
  if (window.__ligmaHook || location.hostname !== "www.figma.com" || window.top !== window) return;
  // Ligma's renderer was turned off since the tab opened (see `reload_tabs`
  // in run.rs)
  try {
    if (sessionStorage.getItem("ligmaNativeOff")) return;
  } catch {}

  const hook = (window.__ligmaHook = {
    // Ligma draws frames, Figma's worker gets none
    replacing: false,
    // Where to hand Ligma each frame, an address on this site Ligma answers
    // itself (see `Frames` in run.rs), set by Ligma
    pushTo: null,
    frames: 0,
    sent: 0,
    ringed: 0,
    answered: 0,
    // Set when the next frame should be a full scene: Figma is told the
    // frame wasn't taken, and recovers by sending the whole scene
    wantFull: false,
    error: null,
    // Memory Ligma shares with the page (see page.js)
    shared: null,
    // Figma's interface fonts, as its worker is given them
    fonts: null,
    // Frames go to Figma's worker: not from when Ligma asks for them, so
    // the worker isn't sent the whole scene again for nothing, till Ligma
    // hands drawing back (`worker`)
    toWorker: true,
    // Figma's image publishers that left images for its worker undecoded
    // meanwhile
    skipped: new Set(),
    // Changes when the scene might have, for what's worked out from it to
    // be worked out again
    playback: 0,
  });
  // And now and then regardless
  setInterval(() => hook.playback++, 1000);

  // Figma's threaded renderer, behind a gate it reads once as it starts
  let options;
  try {
    Object.defineProperty(window, "INITIAL_OPTIONS", {
      configurable: true,
      get: () => options,
      set(value) {
        try {
          if (value?.feature_flags && typeof value.feature_flags === "object") value.feature_flags.fullscreen_use_threaded_rendering = true;
        } catch {}
        options = value;
      },
    });
  } catch {}

  // A reference to a node Figma's worker doesn't have, which it turns down,
  // so Figma sends it the whole scene again
  const UNREADABLE = new Uint8Array([0x02, 0x01, 0x02, 0x38, 0xff, 0xff, 0xff, 0x7f, 0x3a, 0x01, 0x00, 0x00, 0x00]);

  const looksLikeFrame = (frame) =>
    frame && frame.renderTreeForest instanceof Uint8Array && frame.renderLayers?.staticLayer && typeof frame.updateMode === "number";

  // Figma talks to its worker in JSON-RPC over a MessagePort
  const post = MessagePort.prototype.postMessage;
  MessagePort.prototype.postMessage = function (message) {
    try {
      if (message?.method === "init" && Array.isArray(message.params?.fonts)) {
        hook.fonts = message.params.fonts;
        sendFonts();
      } else if (message?.method === "sendCanvasFrameToRenderWorker" && frame(message.params)) {
        answer(this, message);
        return;
      }
    } catch (error) {
      hook.error = String(error);
    }
    return post.apply(this, arguments);
  };

  // What Figma's worker answers a frame: whether it took it, and how far
  // it's got, as the worker's counters, which are strings. Ligma takes
  // every frame, and shows it within a frame or two
  let lastTaken = null;
  function answer(port, message) {
    const f = message.params.frame;
    const taken = !(hook.wantFull && f.updateMode !== 0);
    if (taken) lastTaken = { revision: String(f.transportRevision ?? "0"), generation: String(f.sceneGeneration ?? "0") };
    else hook.wantFull = false;
    const last = lastTaken ?? { revision: "0", generation: "0" };
    const result = {
      frameApplied: taken,
      continuousMetrics: null,
      gpuMetricsFrameNumber: "0",
      lastAppliedTransportRevision: last.revision,
      lastUICommandsDecodedSceneGeneration: last.generation,
      lastCommittedSceneGeneration: last.generation,
      lastPresentedSceneGeneration: last.generation,
      lastConvergedSceneGeneration: last.generation,
    };
    hook.answered++;
    const reply = { jsonrpc: "2.0", id: message.id, result };
    queueMicrotask(() => {
      try {
        port.dispatchEvent(new MessageEvent("message", { data: reply }));
      } catch (error) {
        hook.error = String(error);
      }
    });
  }

  // As a file opens, Figma sends the rest of its scene in batches about a
  // second apart. At times the batches stop with nodes in view never sent,
  // as on 2026-11-16 with 2,749 of them, images and all, till their bounds
  // were asked for 60 s later. So once the batches stop, the bounds of what's
  // in and near the view are asked for, a few milliseconds at a time
  const SETTLED = 2000;
  const OPENING = 60000;
  let opened = -Infinity;
  let settling = 0;
  function loading(f) {
    const now = performance.now();
    if (f.updateMode === 0) opened = now;
    if (now - opened > OPENING || f.renderTreeForest.length < 16384) return;
    clearTimeout(settling);
    settling = setTimeout(askBounds, SETTLED);
  }
  async function askBounds() {
    const figma = window.figma;
    const view = figma?.viewport?.bounds;
    if (!view || !figma.currentPage) return;
    const pad = Math.max(view.width, view.height) / 2;
    const area = { x: view.x - pad, y: view.y - pad, right: view.x + view.width + pad, bottom: view.y + view.height + pad };
    const near = (box) => box && box.x < area.right && box.x + box.width > area.x && box.y < area.bottom && box.y + box.height > area.y;
    const started = performance.now();
    let slice = started;
    let nodes = 0;
    const left = [...figma.currentPage.children];
    while (left.length) {
      if (performance.now() - slice > 4) {
        await new Promise((resolve) => setTimeout(resolve));
        slice = performance.now();
      }
      const node = left.pop();
      nodes++;
      try {
        const rendered = node.absoluteRenderBounds;
        if ((near(rendered) || near(node.absoluteBoundingBox)) && "children" in node) left.push(...node.children);
      } catch {}
    }
    hook.boundsAsked = { nodes, ms: Math.round(performance.now() - started) };
  }

  // Media Figma hands its worker with a frame, let go of, as Ligma gets its
  // own
  function release(params) {
    const close = (what) => {
      try {
        what?.close?.();
      } catch {}
    };
    for (const update of params.imageUpdates ?? []) close(update.image);
    for (const update of params.videoUpdates ?? []) close(update.frame);
    for (const update of params.emojiUpdates ?? []) close(update.image);
  }

  // True when Ligma takes the frame in place of Figma's worker
  function frame(params) {
    const f = params?.frame;
    if (!looksLikeFrame(f)) {
      if (hook.replacing) hook.error = "unexpected frame";
      return false;
    }
    hook.frames++;
    if (!hook.toWorker) loading(f);
    if (!hook.panels?.dragging && (f.updateMode === 0 || f.renderTreeForest.length > 16 || f.releasedNodeIds?.length)) hook.playback++;
    if (hook.pushTo) {
      try {
        send(f);
      } catch (error) {
        hook.error = String(error);
      }
    }
    if (!hook.toWorker) {
      release(params);
      return true;
    }
    if (hook.wantFull && f.updateMode !== 0) {
      hook.wantFull = false;
      f.renderTreeForest = UNREADABLE.slice();
      f.updateMode = 1;
      f.releasedNodeIds = [];
    }
    hook.workerScene = true;
    return false;
  }

  // Figma's interface fonts, for Ligma to draw the overlay's text with
  let fontsSent = false;
  function sendFonts() {
    if (fontsSent || !hook.fonts || !hook.pushTo) return;
    fontsSent = true;
    for (const { family, url } of hook.fonts) {
      if (typeof url !== "string" || typeof family !== "string") continue;
      fetch(url)
        .then((response) => (response.ok ? response.arrayBuffer() : Promise.reject(new Error(`${response.status}`))))
        .then((body) => fetch(location.origin + hook.pushTo + `font-${encodeURIComponent(family)}`, { method: "POST", body, cache: "no-store", credentials: "omit" }))
        .catch(() => (fontsSent = false));
    }
  }

  // Each frame as Figma's main thread sends it, in order: a little-endian
  // length, that much JSON of what Ligma needs to draw it, then the forest.
  // A frame lost on the way has Figma send the whole scene again
  const session = Math.random().toString(36).slice(2, 8);
  let seq = 0;
  let rect = null;
  const measure = () => {
    const canvas = document.querySelector(".gpu-view-content canvas");
    if (!canvas) return null;
    const r = canvas.getBoundingClientRect();
    rect = [r.left, r.top, r.width, r.height];
    if (!canvas.__ligmaMeasured) {
      canvas.__ligmaMeasured = true;
      new ResizeObserver(() => (rect = null)).observe(canvas);
    }
    return rect;
  };
  addEventListener("resize", () => (rect = null));
  const encoder = new TextEncoder();
  // What the page went through since the frame before, for Ligma's trace:
  // the pointer moves it got, by when the browser had them and when the page
  // ran them, and its long tasks, so time a frame took to come can be put
  // down to input coming late or the page being busy
  const pageLog = { moves: 0, first: 0, last: 0, ran: 0, long: [] };
  const moved = (event) => {
    const now = performance.now();
    if (!pageLog.moves) pageLog.first = event.timeStamp;
    pageLog.moves++;
    pageLog.last = event.timeStamp;
    pageLog.ran = Math.max(pageLog.ran, now - event.timeStamp);
  };
  for (const type of ["pointermove", "wheel"]) addEventListener(type, moved, { capture: true, passive: true });
  try {
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        if (entry.duration < 30 || pageLog.long.length > 8) continue;
        const scripts = (entry.scripts ?? [])
          .filter((script) => script.duration > 5)
          .map((script) => `${script.invoker}|${(script.sourceURL ?? "").split("/").pop().slice(0, 40)}|${script.sourceFunctionName}|${Math.round(script.duration)}`);
        pageLog.long.push({ at: Math.round(entry.startTime), ms: Math.round(entry.duration), block: Math.round(entry.blockingDuration), layout: Math.round(entry.styleAndLayoutStart ? entry.startTime + entry.duration - entry.styleAndLayoutStart : 0), scripts });
      }
    }).observe({ type: "long-animation-frame" });
  } catch {}
  function pageSince() {
    const now = performance.now();
    const tenths = (ms) => Math.round(ms * 10) / 10;
    const log = { frame: tenths(now - (document.timeline?.currentTime ?? now)), moves: pageLog.moves, first: tenths(now - pageLog.first), last: tenths(now - pageLog.last), ran: Math.round(pageLog.ran), long: pageLog.long };
    pageLog.moves = 0;
    pageLog.ran = 0;
    pageLog.long = [];
    return log;
  }
  function send(f) {
    const [cx, cy, cw, ch] = rect ?? measure() ?? [0, 0, innerWidth, innerHeight];
    const ui = f.uiCommands instanceof Uint8Array ? f.uiCommands : new Uint8Array(0);
    const color = f.backgroundColor ?? {};
    const settings = window.__ligmaNative?.settings?.() ?? null;
    const header = encoder.encode(
      JSON.stringify({
        session,
        seq: ++seq,
        mode: f.updateMode,
        root: f.canvasRootNodeId,
        released: Array.from(f.releasedNodeIds ?? []),
        ox: f.canvasToPixels?.offset?.x ?? 0,
        oy: f.canvasToPixels?.offset?.y ?? 0,
        scale: f.canvasToPixels?.scale ?? 1,
        dpr: f.devicePixelRatio ?? devicePixelRatio,
        bg: [color.red ?? 0, color.green ?? 0, color.blue ?? 0, color.alpha ?? 0],
        checker: f.checkerboardStyle ?? 0,
        grid: !!f.pixelGrid,
        preview: f.isPixelPreviewEnabled ? settings?.preview || 1 : 0,
        outline: settings?.outline ?? -1,
        cx,
        cy,
        cw,
        ch,
        visible: document.visibilityState === "visible",
        at: performance.timeOrigin + performance.now(),
        ui: ui.length,
        page: pageSince(),
      }),
    );
    const forest = f.renderTreeForest;
    const body = new Uint8Array(4 + header.length + forest.length + ui.length);
    new DataView(body.buffer).setUint32(0, header.length, true);
    body.set(header, 4);
    body.set(forest, 4 + header.length);
    body.set(ui, 4 + header.length + forest.length);
    hook.sent++;
    if (ring(body)) return;
    fetch(location.origin + hook.pushTo + `f${session}-${seq}`, { method: "POST", body, cache: "no-store", credentials: "omit" })
      .then((response) => {
        if (response.status !== 204) throw new Error(`${response.status}`);
      })
      .catch(() => (hook.wantFull = true));
  }

  // Memory shared with Ligma that frames go through (see `read_ring` in
  // run.rs): the bytes written in all and those Ligma has read, then parts of
  // frames, round. A frame bigger than what's free goes in parts, waiting for
  // Ligma to read the ones before, which it does within a millisecond or so
  const RING_HEADER = 64;
  const RING_PART = 4 << 20;
  const RING_WAIT = 100;
  let shared = null;
  window.chrome?.webview?.addEventListener("sharedbufferreceived", (event) => {
    if (!event.additionalData?.ligmaFrames) return;
    const buffer = event.getBuffer();
    const positions = new Float64Array(buffer, 0, 2);
    // Starting from what Ligma has read
    positions[0] = positions[1];
    shared = { positions, bytes: new Uint8Array(buffer, RING_HEADER), view: new DataView(buffer, RING_HEADER), capacity: buffer.byteLength - RING_HEADER };
  });
  function ring(body) {
    if (!shared) return false;
    const { positions, bytes, view, capacity } = shared;
    let offset = 0;
    let waiting = 0;
    let read = positions[1];
    do {
      const written = positions[0];
      const at = written % capacity;
      const free = capacity - (written - positions[1]);
      const end = capacity - at;
      const rest = body.length - offset;
      // Too little before the end for a part worth writing
      if (end - 8 < Math.min(rest, 4096) && free >= end) {
        if (end >= 8) view.setUint32(at, 0xffffffff, true);
        positions[0] = written + end;
        continue;
      }
      const part = Math.min(rest, RING_PART, Math.min(free, end) - 8);
      if (part <= 0 || (part < rest && part < 4096)) {
        // Ligma reading, or not: then frames go as requests till the tab
        // gets memory again
        const now = performance.now();
        if (positions[1] !== read) {
          read = positions[1];
          waiting = now;
        } else if (!waiting) {
          waiting = now;
        } else if (now - waiting > RING_WAIT) {
          if (!offset) shared = null;
          return false;
        }
        continue;
      }
      view.setUint32(at, part, true);
      view.setUint32(at + 4, (offset + part < body.length ? 1 : 0) | (offset ? 2 : 0), true);
      bytes.set(body.subarray(offset, offset + part), at + 8);
      positions[0] = written + 8 + Math.ceil(part / 8) * 8;
      offset += part;
    } while (offset < body.length);
    hook.ringed++;
    return true;
  }

  // What's behind Figma's canvas made transparent, but for a backing of the
  // page's own background everywhere else, so the tab shows Ligma's layer
  // only where the canvas is. The backing is four strips of plain color
  // around the canvas, which Chrome keeps no pixels for. One layer the size
  // of the window with the canvas cut out of it took 30 MB of video memory
  const STYLE = "ligma-native-under";
  let watcher = null;
  const hole = () => {
    const canvas = document.querySelector(".gpu-view-content canvas") ?? document.querySelector("canvas");
    const style = document.getElementById(STYLE);
    if (!canvas || !style) return;
    const r = canvas.getBoundingClientRect();
    const [w, h] = [innerWidth, innerHeight];
    const strip = (selector, left, top, width, height) => `
      ${selector} {
        content: ""; position: fixed; z-index: -1; pointer-events: none;
        background: var(--color-bg, #2c2c2c);
        left: ${left}px; top: ${top}px; width: ${Math.max(0, width)}px; height: ${Math.max(0, height)}px;
      }`;
    // Over the panels' color appearance.rs gives them (its `SEAM`)
    style.textContent = `
      html > body:has(.gpu-view-content), html .gpu-view-content, html .gpu-view-content > canvas {
        background-color: transparent !important; background-image: none !important;
      }
      html .gpu-view-content > canvas { opacity: 0 !important; }
      ${strip("html::before", 0, 0, w, r.top)}
      ${strip("html::after", 0, r.bottom, w, h - r.bottom)}
      ${strip("body::before", 0, r.top, r.left, r.height)}
      ${strip("body::after", r.right, r.top, w - r.right, r.height)}`;
  };
  const under = (on) => {
    document.getElementById(STYLE)?.remove();
    watcher?.disconnect();
    watcher = null;
    removeEventListener("resize", hole);
    if (!on) return;
    const style = document.createElement("style");
    style.id = STYLE;
    document.head.append(style);
    hole();
    const canvas = document.querySelector(".gpu-view-content canvas");
    if (canvas) {
      watcher = new ResizeObserver(hole);
      watcher.observe(canvas);
    }
    addEventListener("resize", hole);
  };

  // While something is dragged on the canvas, Figma's side panels wait for
  // the drag to end. Redrawing them took about 15 of the 25 ms each frame of
  // a drag took, so the canvas followed the mouse at 40 fps. Figma redraws
  // them from its store's listeners, which it flushes once a frame, and from
  // its scene's deferrable listeners, which it lets run later anyway. What
  // Figma draws on the canvas, the size label included, keeps up. They catch
  // up once the canvas has had no input for `QUIET` ms after, as that takes
  // Figma's main thread 60 ms or so, which would hold up a pan or a hover
  // started right after
  const QUIET = 120;
  const panels = { dragging: false, holding: false, flush: null, deferred: new Set() };
  hook.panels = panels;
  {
    let down = null;
    let input = 0;
    let waiting = 0;
    const onCanvas = (target) => target instanceof Element && !!target.closest(".gpu-view-content");
    const catchUp = () => {
      waiting = 0;
      if (panels.dragging) return;
      const quiet = performance.now() - input;
      if (quiet < QUIET) {
        waiting = setTimeout(catchUp, QUIET - quiet);
        return;
      }
      panels.holding = false;
      const deferred = [...panels.deferred];
      panels.deferred.clear();
      for (const listeners of deferred) {
        try {
          listeners.runDeferredListeners();
        } catch {}
      }
      const flush = panels.flush;
      panels.flush = null;
      try {
        flush?.();
      } catch {}
    };
    const end = () => {
      down = null;
      if (!panels.dragging) return;
      panels.dragging = false;
      hook.playback++;
      input = performance.now();
      // After Figma has seen the drag end
      if (!waiting) waiting = setTimeout(catchUp, QUIET);
    };
    const touched = (event) => {
      if (panels.holding && onCanvas(event.target)) input = performance.now();
    };
    addEventListener("wheel", touched, { capture: true, passive: true });
    addEventListener("pointermove", touched, { capture: true, passive: true });
    addEventListener("pointerdown", (event) => (down = event.button === 0 && onCanvas(event.target) ? [event.clientX, event.clientY] : null), true);
    addEventListener(
      "pointermove",
      (event) => {
        if (!down || panels.dragging) return;
        if (!(event.buttons & 1)) return end();
        if (Math.hypot(event.clientX - down[0], event.clientY - down[1]) > 2) panels.dragging = panels.holding = true;
      },
      true,
    );
    addEventListener("pointerup", end, true);
    addEventListener("pointercancel", end, true);
    addEventListener("blur", end);
    window.__ligmaModules?.patch([
      {
        // Flushes the store's listeners, which Figma calls each frame: the
        // export that takes nothing, as seen on 2026-11-13
        match: (source) => source.includes("BeforeReactRender") && source.includes("AfterReactRender") && source.includes("subscribeImmediate"),
        getter: (value) =>
          typeof value !== "function" || value.length !== 0
            ? value
            : function () {
                if (panels.holding) {
                  panels.flush = value;
                  return;
                }
                panels.flush = null;
                return value.apply(this, arguments);
              },
      },
      {
        // Which nodes have animated custom effects, which Figma's threaded
        // renderer works out by walking the page several times a frame,
        // 3 to 5 ms a frame in a file with many, a third of a pan's frame.
        // It's kept till the scene changes (`playback`), and a drag's moves
        // and resizes don't change it
        match: (source) => source.includes("getPlaybackNodes(){") && source.includes("hasAnimatedCustomEffectOrDescendant"),
        source: (source) =>
          source.replace(
            "getPlaybackNodes(){",
            "getPlaybackNodes(){const h=window.__ligmaHook;if(!h)return this.__ligmaPlaybackNodes();if(this.__ligmaPlaybackAt!==h.playback||!this.__ligmaPlayback){this.__ligmaPlayback=this.__ligmaPlaybackNodes();this.__ligmaPlaybackAt=h.playback}return this.__ligmaPlayback}__ligmaPlaybackNodes(){",
          ),
      },
      {
        // The layers panel places its rows with 3D translations, for which
        // Chrome gives each row a layer of its own (see `ROWS`). Moved in 2D
        // they're drawn in the panel's layer
        many: true,
        match: (source) => source.includes("translate3d(0px, ${") && /--selection-height|--object-row-height|objects_panel--|object_row--/.test(source),
        source: (source) => source.replace(/translate3d\(0px, (\$\{[^`]*?\})px, 0\)/g, "translate(0px, $1px)"),
      },
      {
        // Figma decodes each image a frame shows, at each size it's shown,
        // and hands the pixels to its worker. Ligma decodes its own, so
        // while Ligma draws they're left undecoded: hundreds as a file
        // opens, which held the main thread up and hundreds of MB of video
        // memory. They're marked as not yet handed over, so Figma hands
        // them all over when its worker draws again (see `worker`)
        match: (source) => source.includes("_attemptPublish(") && source.includes("_publishWhenDecoded(") && source.includes("abandonInFlightDecodes(){"),
        source: (source) =>
          source.replace(
            /_attemptPublish\(([\w$]+),([\w$]+),([\w$]+)\)\{/,
            (all, publication, session, given) =>
              `${all}{const h=window.__ligmaHook;if(h&&!h.toWorker&&h.skipped){if(${given}?.kind==="pixels")try{${given}.decoded.image?.close?.()}catch{}h.skipped.add(this);return!0}}`,
          ),
      },
      {
        // How Figma's main thread decodes images (see `MAIN_IMAGES`), as
        // its bindings hand it to Figma's wasm. Its own module comes with
        // the page, before any patch can take it
        many: true,
        match: (source) => /ImageIo:\(\)=>[\w$]+\.[\w$]+,/.test(source),
        source: (source) => source.replace(/ImageIo:\(\)=>([\w$]+\.[\w$]+),/, "ImageIo:()=>window.__ligmaHook?.imageIo?.($1)??$1,"),
      },
      {
        // Listeners some of which may run later
        match: (source) => source.includes("deferrableListeners") && source.includes("runDeferredListeners"),
        getter: (value) =>
          typeof value !== "function" || !value.prototype?.runDeferredListeners
            ? value
            : class extends value {
                runDeferredListeners(...args) {
                  if (!panels.holding) return super.runDeferredListeners(...args);
                  panels.deferred.add(this);
                  this.deferredCancel = () => panels.deferred.delete(this);
                }
              },
      },
    ]);
  }

  // The layers panel's rows. Chrome sorts the page's compositor layers out
  // again every frame something moves, as the view does in a pan, at a
  // cost that grows with the layers and what's painted over them. Each row
  // had two: one for its 3D translation (see the patch above) and one for
  // what's painted over its icon, which sticks to the panel's side when it
  // scrolls across. That made the 118 layers that took 5 ms of a pan's
  // frame with something selected, which the panels' 32 take 1.3 ms of.
  // The icons scroll with their rows instead
  const ROWS = `[class*="object_row--layerIcon"] { position: relative !important; }`;
  {
    const style = document.createElement("style");
    style.id = "ligma-native-rows";
    style.textContent = ROWS;
    const add = () => (document.head ?? document.documentElement).append(style);
    if (document.documentElement) add();
    else addEventListener("DOMContentLoaded", add, { once: true });
  }

  // Figma's main thread decodes every image it downloads and keeps a copy
  // of each on its own GPU, at full size, for what it still draws itself
  // while Ligma draws the canvas: thumbnails and previews a few hundred
  // pixels across. While Ligma draws, those copies are made no larger than
  // `MAIN_IMAGES`, and Figma draws them as it draws any image. A file's
  // took 958 MB of video memory at full size and 244 MB so. Figma still
  // gets each image at its own size, as its size controls and plugins read
  // it, and only its texture is smaller
  const MAIN_IMAGES = 512;
  // Sizes of the textures Figma is about to make for smaller copies, each
  // with the size to make instead, for this task only
  const shrinking = new Map();
  // Each smaller copy's full size image, kept till it's copied to the GPU
  // in case Figma's texture is full size after all
  const fullSize = new WeakMap();
  // Copies are made off the main thread, once the worker has said it runs,
  // as images handed to it are gone from here
  const SHRINK = `postMessage("ready");
    onmessage = async ({ data: { id, bitmap, width, height } }) => {
      let small = null;
      try {
        small = await createImageBitmap(bitmap, { resizeWidth: width, resizeHeight: height, resizeQuality: "high", premultiplyAlpha: "none" });
      } catch {}
      postMessage({ id, bitmap, small }, small ? [bitmap, small] : [bitmap]);
    };`;
  let shrinker = null;
  let shrinkerReady = false;
  const shrinkJobs = new Map();
  let shrinkNext = 0;
  function startShrinker() {
    try {
      shrinker = new Worker(URL.createObjectURL(new Blob([SHRINK], { type: "text/javascript" })));
      shrinker.onmessage = ({ data }) => {
        if (data === "ready") shrinkerReady = true;
        else {
          shrinkJobs.get(data.id)?.(data);
          shrinkJobs.delete(data.id);
        }
      };
    } catch {
      shrinker = false;
    }
  }
  // Figma's decoded image `result`, with a smaller copy in place of its
  // pixels when Ligma draws
  function smaller(result) {
    const bitmap = result?.bitmap;
    if (!hook.pushTo || !(bitmap instanceof ImageBitmap)) return result;
    const scale = MAIN_IMAGES / Math.max(bitmap.width, bitmap.height);
    if (!(scale < 1)) return result;
    if (shrinker === null) startShrinker();
    if (!shrinkerReady) return result;
    const width = Math.max(1, Math.round(bitmap.width * scale));
    const height = Math.max(1, Math.round(bitmap.height * scale));
    return new Promise((resolve) => {
      const id = shrinkNext++;
      shrinkJobs.set(id, resolve);
      shrinker.postMessage({ id, bitmap, width, height }, [bitmap]);
    }).then(({ bitmap: full, small }) => {
      if (!small) return { ...result, bitmap: full };
      fullSize.set(small, full);
      const key = `${full.width}x${full.height}`;
      const entry = { width, height };
      if (!shrinking.has(key)) shrinking.set(key, []);
      shrinking.get(key).push(entry);
      setTimeout(() => {
        const list = shrinking.get(key);
        const at = list?.indexOf(entry) ?? -1;
        if (at >= 0) list.splice(at, 1);
        if (list && !list.length) shrinking.delete(key);
      });
      return { ...result, bitmap: small };
    });
  }
  // Figma's image decoding, as its wasm gets it, decoding as it did with
  // `smaller` after
  hook.imageIo = (io) => {
    if (typeof io?.decodeImage !== "function" || io.__ligmaDecode) return io;
    const decode = io.decodeImage;
    io.__ligmaDecode = decode;
    io.decodeImage = function () {
      const decoded = decode.apply(this, arguments);
      return hook.pushTo ? Promise.resolve(decoded).then(smaller) : decoded;
    };
    return io;
  };
  try {
    const createTexture = GPUDevice.prototype.createTexture;
    GPUDevice.prototype.createTexture = function (descriptor) {
      if (shrinking.size && descriptor?.label === "Image") {
        const size = descriptor.size;
        const key = `${size?.width ?? size?.[0]}x${size?.height ?? size?.[1] ?? 1}`;
        const list = shrinking.get(key);
        const entry = list?.shift();
        if (list && !list.length) shrinking.delete(key);
        if (entry && (size?.depthOrArrayLayers ?? size?.[2] ?? 1) === 1 && (descriptor.mipLevelCount ?? 1) === 1 && (descriptor.dimension ?? "2d") === "2d")
          descriptor = { ...descriptor, size: { width: entry.width, height: entry.height, depthOrArrayLayers: 1 } };
      }
      return createTexture.call(this, descriptor);
    };
    const copyExternal = GPUQueue.prototype.copyExternalImageToTexture;
    GPUQueue.prototype.copyExternalImageToTexture = function (source, destination, copySize) {
      const small = source?.source;
      const full = small && fullSize.get(small);
      if (full) {
        fullSize.delete(small);
        const texture = destination?.texture;
        if (texture && (texture.width !== small.width || texture.height !== small.height)) {
          try {
            small.close();
          } catch {}
          source = { ...source, source: full };
        } else {
          try {
            full.close();
          } catch {}
        }
      }
      return copyExternal.call(this, source, destination, copySize);
    };
  } catch {}

  // Figma's worker keeps the scene and images of any frame it's drawn, and
  // the video memory for them, after Ligma takes over: 630 MB in a file
  // drawn once, as it is as Ligma starts. So once Ligma draws, the worker is
  // swapped for a fresh one, as Figma does when its worker crashes. Ligma
  // takes the scene Figma sends the new worker, and the worker gets it when
  // Ligma hands drawing back (`wantFull`)
  // Should Figma change, the worker is just kept, as it was before
  window.__ligmaModules?.patch([
    {
      // Figma's manager of its workers, the canvas's among them, as it
      // starts one
      match: (source) => source.includes("restartWorker(") && source.includes("spawnWorker(") && source.includes("_role"),
      source: (source) =>
        source.replace(
          /\bspawnWorker\(\)\{/,
          (all) => `${all}"canvas"===this._role&&window.__ligmaHook&&(window.__ligmaHook.canvasWorker=this);`,
        ),
    },
  ]);
  let freshening = 0;
  let tries = 0;
  let warned = false;
  function freshWorker() {
    clearTimeout(freshening);
    if (!hook.replacing || hook.toWorker || !hook.workerScene) return;
    const manager = hook.canvasWorker;
    if (typeof manager?.restartWorker !== "function") {
      if (!warned) console.warn("Ligma: Figma's canvas worker wasn't found, so the memory it holds isn't freed");
      warned = true;
      return;
    }
    // Not mid export, which a restart would send down Figma's slower path,
    // nor while it starts. Given up on after a minute, as when it's failed
    if (manager._currentState !== "started") {
      if (++tries < 60) freshening = setTimeout(freshWorker, 1000);
      return;
    }
    tries = 0;
    hook.workerScene = false;
    try {
      manager.restartWorker();
      hook.freshWorkers = (hook.freshWorkers ?? 0) + 1;
    } catch (error) {
      hook.error = String(error);
    }
    // Figma puts a new canvas in for the new worker
    under(true);
  }

  // Whether frames go to Figma's worker. Images it was meant to get
  // meanwhile are decoded and sent along with the next frames
  function worker(on) {
    if (on && !hook.toWorker) {
      for (const publisher of hook.skipped) {
        try {
          publisher.abandonInFlightDecodes();
        } catch {}
      }
      hook.skipped.clear();
    }
    hook.toWorker = on;
  }

  // Called by Ligma. True once Figma's threaded renderer is running and
  // frames have come through as expected
  hook.ready = () => hook.frames > 0 && !hook.error;
  hook.replace = (on) => {
    if (on && !hook.ready()) return false;
    if (hook.replacing === on) return true;
    hook.replacing = on;
    under(on);
    // Figma's worker needs the whole scene back when it draws it again
    if (!on) hook.wantFull = true;
    worker(!on);
    if (on) freshWorker();
    return true;
  };
  hook.resync = () => {
    hook.wantFull = true;
    return true;
  };
  // Called by Ligma with where to hand it frames, or null to stop. Figma
  // then sends the whole scene, for Ligma to start from, or for its worker
  // when Ligma stops. Figma's canvas holds still till Ligma draws, a frame
  // or two, rather than its worker loading the whole scene and its images
  // again. Unless Ligma hasn't taken over by `HAND_BACK` ms, as when it
  // reads the page another way
  const HAND_BACK = 4000;
  let handBack = 0;
  hook.push = (path) => {
    hook.pushTo = path;
    hook.wantFull = true;
    worker(!path);
    clearTimeout(handBack);
    if (path) {
      sendFonts();
      if (shrinker === null) startShrinker();
      handBack = setTimeout(() => {
        if (hook.replacing || hook.toWorker) return;
        worker(true);
        hook.wantFull = true;
      }, HAND_BACK);
    } else window.__ligmaNative?.follow?.();
    return true;
  };
})();
