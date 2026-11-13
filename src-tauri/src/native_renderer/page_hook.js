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
    if (!hook.panels?.dragging && (f.updateMode === 0 || f.renderTreeForest.length > 16 || f.releasedNodeIds?.length)) hook.playback++;
    if (hook.pushTo) {
      try {
        send(f);
      } catch (error) {
        hook.error = String(error);
      }
    }
    if (hook.replacing) {
      release(params);
      return true;
    }
    if (hook.wantFull && f.updateMode !== 0) {
      hook.wantFull = false;
      f.renderTreeForest = UNREADABLE.slice();
      f.updateMode = 1;
      f.releasedNodeIds = [];
    }
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
  // only where the canvas is
  const STYLE = "ligma-native-under";
  let watcher = null;
  const hole = () => {
    const canvas = document.querySelector(".gpu-view-content canvas") ?? document.querySelector("canvas");
    const style = document.getElementById(STYLE);
    if (!canvas || !style) return;
    const r = canvas.getBoundingClientRect();
    const [w, h] = [innerWidth, innerHeight];
    // Over the panels' color appearance.rs gives them (its `SEAM`)
    style.textContent = `
      html > body:has(.gpu-view-content), html .gpu-view-content, html .gpu-view-content > canvas {
        background-color: transparent !important; background-image: none !important;
      }
      html .gpu-view-content > canvas { opacity: 0 !important; }
      body::before {
        content: ""; position: fixed; inset: 0; z-index: -1; pointer-events: none;
        background: var(--color-bg, #2c2c2c);
        clip-path: path(evenodd, "M0 0H${w}V${h}H0Z M${r.left} ${r.top}H${r.right}V${r.bottom}H${r.left}Z");
      }`;
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
    return true;
  };
  hook.resync = () => {
    hook.wantFull = true;
    return true;
  };
  // Called by Ligma with where to hand it frames, or null to stop. Figma
  // then sends the whole scene, for Ligma to start from
  hook.push = (path) => {
    hook.pushTo = path;
    if (path) {
      hook.wantFull = true;
      sendFonts();
    }
    return true;
  };
})();
