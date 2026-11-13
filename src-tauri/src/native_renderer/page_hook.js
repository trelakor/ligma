// Experimental native renderer, the part that runs as a Figma tab starts
// (see native_renderer.rs). Ligma takes over Figma's canvas renderer here,
// while Figma keeps everything else, its overlay included
//
// Figma's threaded renderer has its main thread send each frame to a render
// worker: the scene as a render tree (Kiwi, see rt.rs), the overlay
// (selection, handles, labels) and the view. Its gate is turned on here.
// Once Ligma says it's drawing (`replace(true)`), the worker is handed an
// empty scene instead, so it draws only the overlay, on a canvas made
// transparent, and Ligma draws the scene under the tab where it shows
// through. Every frame's scene goes to `sink` for Ligma
//
// When anything here doesn't look as expected, it stays out of the way and
// Figma renders as it always has
(() => {
  if (window.__ligmaHook || location.hostname !== "www.figma.com" || window.top !== window) return;

  const hook = (window.__ligmaHook = {
    // Figma's worker gets the empty scene
    replacing: false,
    // Where to hand Ligma each frame, an address on this site Ligma answers
    // itself (see `Frames` in run.rs), set by Ligma
    pushTo: null,
    frames: 0,
    sent: 0,
    ringed: 0,
    // Set when the next frame should be a full scene: the worker is sent a
    // scene it can't read, and Figma recovers by sending the whole scene
    wantFull: false,
    error: null,
    // Memory Ligma shares with the page (see page.js), where it writes the
    // last frame it has drawn
    shared: null,
    // Whether frames for Figma's worker wait for Ligma to draw (see `HOLD`)
    sync: false,
  });

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

  // The worker's canvas, made able to be transparent, which it is wherever
  // nothing is drawn as the scene it's given is empty. Run in the worker
  // before Figma's own code
  const prelude = function () {
    try {
      if (self.GPUCanvasContext) {
        const configure = GPUCanvasContext.prototype.configure;
        GPUCanvasContext.prototype.configure = function (options) {
          return configure.call(this, Object.assign({}, options, { alphaMode: "premultiplied" }));
        };
      }
      const getContext = OffscreenCanvas.prototype.getContext;
      OffscreenCanvas.prototype.getContext = function (type, attributes) {
        if (type === "webgl2" || type === "webgl") attributes = Object.assign({}, attributes, { alpha: true, premultipliedAlpha: true });
        return getContext.call(this, type, attributes);
      };
    } catch {}
    // Tells Ligma when the overlay for each frame Ligma tagged is drawn, so
    // it shows its own drawing of the frame then. The worker draws in its
    // animation frames once it asks for them, else as frames come
    try {
      let last = 0;
      let told = 0;
      let to = null;
      let animated = false;
      const tell = () => {
        if (last <= told || !to) return;
        told = last;
        fetch(to + last, { method: "POST", cache: "no-store", credentials: "omit" }).catch(() => {});
      };
      const wrapped = new WeakMap();
      const wrap = (listener) => {
        if (typeof listener !== "function" && typeof listener?.handleEvent !== "function") return listener;
        if (!wrapped.has(listener)) {
          wrapped.set(listener, function (event) {
            const tag = event?.data?.params?.ligmaSeq;
            if (tag) {
              last = tag;
              to = event.data.params.ligmaTo;
            }
            const result = typeof listener === "function" ? listener.apply(this, arguments) : listener.handleEvent(event);
            if (tag && !animated) tell();
            return result;
          });
        }
        return wrapped.get(listener);
      };
      const add = MessagePort.prototype.addEventListener;
      const remove = MessagePort.prototype.removeEventListener;
      MessagePort.prototype.addEventListener = function (type, listener, options) {
        return add.call(this, type, type === "message" ? wrap(listener) : listener, options);
      };
      MessagePort.prototype.removeEventListener = function (type, listener, options) {
        return remove.call(this, type, type === "message" && wrapped.has(listener) ? wrapped.get(listener) : listener, options);
      };
      const onmessage = Object.getOwnPropertyDescriptor(MessagePort.prototype, "onmessage");
      if (onmessage?.set) {
        Object.defineProperty(MessagePort.prototype, "onmessage", {
          configurable: true,
          enumerable: onmessage.enumerable,
          get() {
            return onmessage.get.call(this);
          },
          set(listener) {
            onmessage.set.call(this, wrap(listener));
          },
        });
      }
      const frame = self.requestAnimationFrame;
      if (frame) {
        self.requestAnimationFrame = function (callback) {
          animated = true;
          return frame.call(self, function () {
            try {
              return callback.apply(this, arguments);
            } finally {
              tell();
            }
          });
        };
      }
    } catch {}
  };
  const NativeWorker = window.Worker;
  try {
    window.Worker = class extends NativeWorker {
      constructor(url, options) {
        let source = url;
        try {
          if (String(url).includes("render_worker") && options?.type !== "module") {
            const absolute = new URL(String(url), location.href).href;
            const code = `(${prelude})();importScripts(${JSON.stringify(absolute)});`;
            source = URL.createObjectURL(new Blob([code], { type: "text/javascript" }));
          }
        } catch {
          source = url;
        }
        super(source, options);
      }
    };
  } catch {}

  // The empty scene: a group with nothing in it, under the occlusion scope
  // Figma's scenes start with. Ids are the worker's own, as it never sees
  // Figma's. Kiwi: Resources { resources: [{ node }, { node }] }
  const ROOT = 3;
  const EMPTY = new Uint8Array([0x02, 0x02, 0x02, 0x01, 0x01, 0x02, 0x00, 0x38, 0x02, 0x00, 0x00, 0x02, 0x01, 0x16, 0x03, 0x02, 0x38, 0x03, 0x00, 0x00, 0x00]);
  const SAME = new Uint8Array([0x02, 0x01, 0x02, 0x38, ROOT, 0x3a, 0x01, 0x00, 0x00, 0x00]);
  // A reference to a node the worker doesn't have, which it turns down
  const UNREADABLE = new Uint8Array([0x02, 0x01, 0x02, 0x38, 0xff, 0xff, 0xff, 0x7f, 0x3a, 0x01, 0x00, 0x00, 0x00]);
  let emptySent = false;

  const looksLikeFrame = (frame) =>
    frame && frame.renderTreeForest instanceof Uint8Array && frame.renderLayers?.staticLayer && typeof frame.updateMode === "number";

  const post = MessagePort.prototype.postMessage;
  MessagePort.prototype.postMessage = function (message) {
    let isFrame = false;
    try {
      if (message?.method === "sendCanvasFrameToRenderWorker") {
        frame(message.params);
        isFrame = true;
      }
    } catch (error) {
      hook.error = String(error);
    }
    const queue = held.get(this);
    if (queue || (isFrame && holding())) {
      const entry = { args: Array.from(arguments), seq: isFrame ? seq : 0, at: performance.now() };
      if (queue) queue.push(entry);
      else held.set(this, [entry]);
      schedule();
      return;
    }
    return post.apply(this, arguments);
  };

  // Frames for Figma's worker, which draws only what's over the canvas, are
  // held till Ligma has drawn what's under them, or for `HOLD` ms at most,
  // so the two show together. What else goes to the worker meanwhile waits
  // behind them, in order
  const HOLD = 50;
  const READY_SLOT = 18;
  const held = new Map();
  let scheduled = 0;
  // Off: the page's main thread, which holds and lets go of them, is busy
  // with Figma's own work for much of each frame, so they come late, and
  // what's over the canvas trails what's under it
  const holding = () => hook.sync && hook.replacing && hook.pushTo && hook.shared && seq > 0;
  function pump(all) {
    const now = performance.now();
    const done = hook.shared?.[READY_SLOT] ?? Infinity;
    for (const [port, queue] of held) {
      while (queue.length) {
        const first = queue[0];
        if (!all && first.seq > done && now - first.at < HOLD) break;
        queue.shift();
        try {
          post.apply(port, first.args);
        } catch (error) {
          hook.error = String(error);
        }
      }
      if (!queue.length) held.delete(port);
    }
  }
  function schedule() {
    if (scheduled) return;
    scheduled = 2;
    // Whichever comes first, a frame or a moment
    const tick = () => {
      scheduled = Math.max(0, scheduled - 1);
      pump(false);
      if (held.size && !scheduled) schedule();
    };
    requestAnimationFrame(tick);
    setTimeout(tick, 1);
  }

  function frame(params) {
    const f = params?.frame;
    if (!looksLikeFrame(f)) {
      if (hook.replacing) hook.error = "unexpected frame";
      return;
    }
    hook.frames++;
    if (hook.pushTo) {
      try {
        send(f);
      } catch (error) {
        hook.error = String(error);
      }
    }
    if (hook.wantFull && f.updateMode !== 0) {
      hook.wantFull = false;
      emptySent = false;
      f.renderTreeForest = UNREADABLE.slice();
      f.updateMode = 1;
      f.releasedNodeIds = [];
      return;
    }
    if (!hook.replacing) {
      emptySent = false;
      return;
    }
    // For the worker to say when it's drawn the overlay for it
    if (hook.pushTo) {
      params.ligmaSeq = seq;
      params.ligmaTo = location.origin + hook.pushTo + `ov-${session}-`;
    }
    // Figma sends the whole scene when its worker lost what it had, so the
    // worker starts from the empty scene again rather than one it lost
    if (f.updateMode === 0) emptySent = false;
    f.renderTreeForest = (emptySent ? SAME : EMPTY).slice();
    f.updateMode = emptySent ? 1 : 0;
    emptySent = true;
    f.canvasRootNodeId = ROOT;
    f.releasedNodeIds = [];
    f.renderLayers.staticLayer.treeRootId = ROOT;
    f.renderLayers.dynamicLayers = [];
    // Drawn under it by Ligma
    f.checkerboardStyle = 0;
    f.pixelGrid = false;
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
      }),
    );
    const forest = f.renderTreeForest;
    const body = new Uint8Array(4 + header.length + forest.length);
    new DataView(body.buffer).setUint32(0, header.length, true);
    body.set(header, 4);
    body.set(forest, 4 + header.length);
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

  // Called by Ligma. True once Figma's threaded renderer is running and
  // frames have come through as expected
  hook.ready = () => hook.frames > 0 && !hook.error;
  hook.replace = (on) => {
    if (on && !hook.ready()) return false;
    if (hook.replacing === on) return true;
    hook.replacing = on;
    pump(true);
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
    if (path) hook.wantFull = true;
    return true;
  };
})();
