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

  // Now on the system's clock, which Ligma times frames against.
  // performance.now() drifts from it, by a second over a long session, and
  // Date.now() keeps to it only to the ms, rounded down, so the drift is the
  // most Date.now() has been ahead, set again once that's over a ms off
  let drift = null;
  const systemNow = () => {
    const precise = performance.timeOrigin + performance.now();
    const ahead = Date.now() - precise;
    if (drift == null || ahead > drift || ahead < drift - 1) drift = ahead;
    return precise + drift;
  };

  const hook = (window.__ligmaHook = {
    // Ligma draws frames, Figma's worker gets none
    replacing: false,
    // Left hidden and let go of by Ligma: neither it nor Figma's worker
    // takes frames, till Ligma asks for them again as it's shown (`rest`)
    resting: false,
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
    // Frames go to Figma's worker: not from the start, as Ligma draws the
    // tab from its first frame, nor from when Ligma asks for them, till it
    // hands drawing back (`worker`), or never asks (`HAND_BACK`)
    toWorker: false,
    // Figma's image publishers that left images for its worker undecoded
    // meanwhile
    skipped: new Set(),
    // Changes when the scene might have, for what's worked out from it to
    // be worked out again
    playback: 0,
    // Figma's noise octaves, once worked out (see below)
    octaves: null,
  });
  // And now and then regardless
  setInterval(() => hook.playback++, 1000);

  // Figma's noise hashes each octave's seed with `sin` of numbers so large
  // that the result is down to how this GPU rounds, so they're worked out
  // here, on the GPU Figma uses: for each of the four fields, each of the
  // three octaves' seed and its two gradient scales. As the page starts, in
  // design files alone, so they're there before Figma's first frame
  // Figma's noise seed, which a file can't change
  const NOISE_SEED = 5277;
  if (/^\/(design|file)\//.test(location.pathname)) (async () => {
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
    hook.octaves = Array.from(new Float32Array(readback.getMappedRange()));
    readback.unmap();
    device.destroy();
  })().catch(() => {});

  // How long Figma's canvas waits for Ligma, from the tab's first frame or
  // from when Ligma asks for its frames, before Figma's worker draws it
  // after all, as when Ligma reads the page another way
  const HAND_BACK = 4000;
  let unasked = 0;
  // Figma's worker draws the tab, sent the whole scene
  const handBackNow = () => {
    worker(true);
    hook.wantFull = true;
  };
  // Figma draws the tab should Ligma not ask for its frames, as when it
  // couldn't start. Not while it's hidden, as Ligma asks for a tab's frames
  // once it's shown: till then neither renderer draws it
  const handBackUnasked = () => {
    clearTimeout(unasked);
    unasked = setTimeout(() => {
      if (hook.pushTo || hook.toWorker) return;
      if (document.hidden) document.addEventListener("visibilitychange", handBackUnasked, { once: true });
      else handBackNow();
    }, HAND_BACK);
  };

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

  // The scene, which Figma sends as one buffer or, since November 2026, as
  // slices of one, 8 MB each for a whole scene
  const forestParts = (forest) => (forest instanceof Uint8Array ? [forest] : Array.isArray(forest) && forest.every((part) => part instanceof Uint8Array) ? forest : null);
  const forestLength = (forest) => forestParts(forest).reduce((sum, part) => sum + part.length, 0);

  const looksLikeFrame = (frame) =>
    frame && forestParts(frame.renderTreeForest) && frame.renderLayers?.staticLayer && typeof frame.updateMode === "number";

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
    if (now - opened > OPENING || forestLength(f.renderTreeForest) < 16384) return;
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
    if (hook.frames === 1) handBackUnasked();
    if (!hook.toWorker) loading(f);
    if (!hook.panels?.dragging && (f.updateMode === 0 || forestLength(f.renderTreeForest) > 16 || f.releasedNodeIds?.length)) hook.playback++;
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
      f.renderTreeForest = Array.isArray(f.renderTreeForest) ? [UNREADABLE.slice()] : UNREADABLE.slice();
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
    // A canvas Figma's made again is 300 by 150 till it sizes it, where
    // what holds it is already the size it'll be. Measured again each
    // frame till they agree
    const box = canvas.parentElement?.getBoundingClientRect();
    if (box && (Math.abs(r.width - box.width) > 2 || Math.abs(r.height - box.height) > 2)) {
      return [box.left, box.top, box.width, box.height];
    }
    rect = [r.left, r.top, r.width, r.height];
    if (!canvas.__ligmaMeasured) {
      canvas.__ligmaMeasured = true;
      new ResizeObserver(() => (rect = null)).observe(canvas);
    }
    return rect;
  };
  addEventListener("resize", () => (rect = null));
  // The part of the canvas in the window. Figma's canvas is laid out no
  // larger than it, but should its styles go missing it grows with what
  // holds it, 300,000 pixels tall, and Ligma would draw all of it
  const inWindow = ([x, y, width, height]) => {
    const left = Math.max(x, 0);
    const top = Math.max(y, 0);
    return [left, top, Math.max(0, Math.min(x + width, innerWidth) - left), Math.max(0, Math.min(y + height, innerHeight) - top)];
  };
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
  // The page's memory, about once a second, and once what the browser says
  // the machine is
  let memoryAt = -Infinity;
  let described = false;
  function pageMemory() {
    const now = performance.now();
    if (now - memoryAt < 1000) return undefined;
    memoryAt = now;
    const mb = (bytes) => Math.round(bytes / 1048576);
    const heap = performance.memory;
    const out = { heap: heap ? [mb(heap.usedJSHeapSize), mb(heap.totalJSHeapSize), mb(heap.jsHeapSizeLimit)] : null, dom: document.getElementsByTagName("*").length, canvases: document.getElementsByTagName("canvas").length };
    if (!described) {
      described = true;
      out.cores = navigator.hardwareConcurrency;
      out.ram = navigator.deviceMemory;
      out.agent = navigator.userAgent;
    }
    return out;
  }
  function pageSince() {
    const now = performance.now();
    const tenths = (ms) => Math.round(ms * 10) / 10;
    const log = { frame: tenths(now - (document.timeline?.currentTime ?? now)), moves: pageLog.moves, first: tenths(now - pageLog.first), last: tenths(now - pageLog.last), ran: Math.round(pageLog.ran), long: pageLog.long, memory: pageMemory() };
    pageLog.moves = 0;
    pageLog.ran = 0;
    pageLog.long = [];
    return log;
  }
  function send(f) {
    const whole = rect ?? measure() ?? [0, 0, innerWidth, innerHeight];
    const [cx, cy, cw, ch] = inWindow(whole);
    const dpr = f.devicePixelRatio ?? devicePixelRatio;
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
        // From the corner of the canvas's part in the window
        ox: (f.canvasToPixels?.offset?.x ?? 0) - (cx - whole[0]) * dpr,
        oy: (f.canvasToPixels?.offset?.y ?? 0) - (cy - whole[1]) * dpr,
        scale: f.canvasToPixels?.scale ?? 1,
        dpr,
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
        at: systemNow(),
        ui: ui.length,
        page: pageSince(),
      }),
    );
    const lead = new Uint8Array(4);
    new DataView(lead.buffer).setUint32(0, header.length, true);
    // Written from where Figma left them, not copied into one buffer first,
    // which for a whole scene held the page up for tens of milliseconds
    const chunks = [lead, header, ...forestParts(f.renderTreeForest), ui];
    hook.sent++;
    if (ring(chunks)) {
      sentFrame(seq);
      return;
    }
    fetch(location.origin + hook.pushTo + `f${session}-${seq}`, { method: "POST", body: new Blob(chunks), cache: "no-store", credentials: "omit" })
      .then((response) => {
        if (response.status !== 204) throw new Error(`${response.status}`);
      })
      .catch(() => (hook.wantFull = true));
  }

  // What Figma puts over the canvas as HTML, comment pins, cursors and the
  // rest, moves in the same update of the page as each frame is sent, but
  // Chrome puts it on screen only once that update's drawn, while Ligma
  // could show the frame at once. So Ligma's told when it's drawn, through
  // the ring, and shows the frame then, both reaching the screen together
  // (see `Hub::drawn` in present.rs). A frame sent in the page's frame
  // callbacks is drawn as they end, one sent from outside them in the page's
  // next frame
  let inFrame = 0;
  const requestFrame = window.requestAnimationFrame;
  window.requestAnimationFrame = function (callback) {
    if (typeof callback !== "function") return requestFrame.apply(window, arguments);
    return requestFrame.call(window, (time) => {
      inFrame++;
      try {
        return callback(time);
      } finally {
        inFrame--;
      }
    });
  };
  // The latest frame sent in the page's frame under way or one before, and
  // the latest sent outside them since
  let framed = 0;
  let outside = 0;
  let posted = false;
  let asked = false;
  const drawn = () => {
    posted = false;
    ring([new Uint8Array(new Float64Array([framed, systemNow()]).buffer)], RING_DRAWN);
  };
  const afterUpdate = new MessageChannel();
  afterUpdate.port1.onmessage = drawn;
  // Runs once the page's frame under way is drawn, as a task can't run
  // during one. Ahead of the page's other tasks where it can be, as Chrome
  // may show the frame before they're done
  const whenDrawn = () => {
    if (posted) return;
    posted = true;
    if (window.scheduler?.postTask) window.scheduler.postTask(drawn, { priority: "user-blocking" }).catch(() => {});
    else afterUpdate.port2.postMessage(0);
  };
  function sentFrame(seq) {
    if (inFrame) {
      framed = seq;
      whenDrawn();
      return;
    }
    outside = seq;
    if (asked) return;
    asked = true;
    requestFrame.call(window, () => {
      asked = false;
      framed = Math.max(framed, outside);
      whenDrawn();
    });
  }

  // Memory shared with Ligma that frames go through (see `read_ring` in
  // run.rs): the bytes written in all and those Ligma has read, then parts of
  // frames, round. A frame bigger than what's free goes in parts, waiting for
  // Ligma to read the ones before, which it does within a millisecond or so.
  // A part flagged `RING_DRAWN` is no frame's: it's the last frame sent the
  // page has drawn its own part of, and when
  const RING_HEADER = 64;
  const RING_PART = 4 << 20;
  const RING_WAIT = 100;
  const RING_DRAWN = 4;
  let shared = null;
  window.chrome?.webview?.addEventListener("sharedbufferreceived", (event) => {
    if (!event.additionalData?.ligmaFrames) return;
    const buffer = event.getBuffer();
    const positions = new Float64Array(buffer, 0, 2);
    // Starting from what Ligma has read
    positions[0] = positions[1];
    shared = { positions, bytes: new Uint8Array(buffer, RING_HEADER), view: new DataView(buffer, RING_HEADER), capacity: buffer.byteLength - RING_HEADER };
  });
  function ring(chunks, flags = 0) {
    if (!shared) return false;
    const { positions, bytes, view, capacity } = shared;
    const length = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
    // Where the next byte to write is: in which chunk, and how far into it
    let chunk = 0;
    let within = 0;
    const copy = (count, to) => {
      while (count > 0) {
        const from = chunks[chunk];
        const take = Math.min(count, from.length - within);
        if (take > 0) bytes.set(within === 0 && take === from.length ? from : from.subarray(within, within + take), to);
        to += take;
        count -= take;
        within += take;
        if (within >= from.length) {
          chunk++;
          within = 0;
        }
      }
    };
    let offset = 0;
    let waiting = 0;
    let read = positions[1];
    do {
      const written = positions[0];
      const at = written % capacity;
      const free = capacity - (written - positions[1]);
      const end = capacity - at;
      const rest = length - offset;
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
      view.setUint32(at + 4, (offset + part < length ? 1 : 0) | (offset ? 2 : 0) | flags, true);
      copy(part, at + 8);
      positions[0] = written + 8 + Math.ceil(part / 8) * 8;
      offset += part;
    } while (offset < length);
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
    const own = canvas.getBoundingClientRect();
    // What holds the canvas, while it's a canvas Figma's made again and
    // not yet sized (see `measure`)
    const box = canvas.parentElement?.getBoundingClientRect();
    const r = box && (Math.abs(own.width - box.width) > 2 || Math.abs(own.height - box.height) > 2) ? box : own;
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
      if (canvas.parentElement) watcher.observe(canvas.parentElement);
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

  // Figma's main thread decodes the images in a file as it opens and copies
  // each to its own GPU, for what it still draws itself while Ligma draws the
  // canvas: layer previews, thumbnails and exports. Of 319 in a file, it drew
  // with 28. So each is given a stand-in texture of a pixel, which tells
  // Figma its own size, and its pixels are kept as Figma decoded them, off
  // the GPU, till Figma first draws with it, as it makes a bind group with
  // it, or copies to or from it. It's made then, at once, with those pixels,
  // and used in the stand-in's place from then on. Figma draws with most of
  // those it does within seconds of decoding them, for layer previews, which
  // need no more than `PREVIEW` pixels across, so as soon as it's decoded an
  // image is kept no larger than that, 4 MB rather than up to 64. An image
  // not drawn with after `KEEP_FULL` ms is kept only `SAFETY` pixels across. Before Figma draws layers for an export, a thumbnail or a
  // preview, it makes sure their images are loaded (`loadAllImagesUnder`),
  // and those it's about to draw are decoded again then, as Figma decoded
  // them (`imagesNeeded`). What it draws without asking gets the smaller copy
  // (counted in `fromSafety`)
  // Held 30 s, a file of 1,700 images opening kept 1.5 GB of them at once
  const KEEP_FULL = 5000;
  const SAFETY = 128;
  const PREVIEW = 1024;
  // Images Figma decodes for thumbnails, no larger than this across
  const THUMBNAIL = 512;
  // Images Figma decodes larger than it draws, made smaller on a canvas, it
  // writes as bytes (`written`) rather than copying from a bitmap, and those
  // are kept the same way
  hook.images = { deferred: 0, written: 0, drawn: 0, fromSafety: 0, shrunk: 0, restored: 0, failed: 0, keptMB: 0 };
  try {
    const createTexture = GPUDevice.prototype.createTexture;
    const createView = GPUTexture.prototype.createView;
    const destroy = GPUTexture.prototype.destroy;
    const close = ImageBitmap.prototype.close;
    const copyExternal = GPUQueue.prototype.copyExternalImageToTexture;
    const write = GPUQueue.prototype.writeTexture;
    // Each stand-in's image: its device, Figma's descriptor and size, its
    // texture once drawn with (`version` counts new ones), till then its
    // pixels, and how Figma decoded them
    const images = new WeakMap();
    // Each view of a stand-in: its image, Figma's view descriptor, and the
    // view made of the image's texture
    const views = new WeakMap();
    // Each image Figma decoded on the main thread: its hash and the rest of
    // what it was decoded with
    const decodes = new WeakMap();
    // Images by hash, for an export's
    const byHash = new Map();
    let io = null;
    // Pixels kept, which Figma closing doesn't close
    const kept = new WeakSet();
    let keptBytes = 0;
    const keep = (bitmap) => {
      kept.add(bitmap);
      keptBytes += bitmap.width * bitmap.height * 4;
      hook.images.keptMB = Math.round(keptBytes / 1e6);
    };
    const letGo = (bitmap) => {
      if (!bitmap || !kept.has(bitmap)) return;
      kept.delete(bitmap);
      keptBytes -= bitmap.width * bitmap.height * 4;
      hook.images.keptMB = Math.round(keptBytes / 1e6);
      try {
        close.call(bitmap);
      } catch {}
    };
    ImageBitmap.prototype.close = function () {
      if (!kept.has(this)) return close.call(this);
    };
    // What's kept of an image let go of, its bitmap or its bytes
    const release = (pending) => {
      if (!pending) return;
      if (pending.bitmap) letGo(pending.bitmap);
      if (pending.data) {
        keptBytes -= pending.data.byteLength;
        hook.images.keptMB = Math.round(keptBytes / 1e6);
        pending.data = null;
      }
    };

    // Images Figma decoded to bytes in the last few seconds, to tell which
    // each written image is, by its size and a few of its pixels
    const RECENT = 10000;
    let recent = [];
    const sample = (bytes) => {
      const out = [];
      for (let i = 1; i < 8; i++) out.push(bytes[Math.floor((bytes.length * i) / 8 / 4) * 4 + (i % 4)]);
      return out.join();
    };
    const decodedAs = (bytes, width, height) => {
      const now = performance.now();
      recent = recent.filter((entry) => now - entry.at < RECENT);
      const same = recent.filter((entry) => entry.width === width && entry.height === height);
      const pixels = sample(bytes);
      const found = same.find((entry) => entry.sample === pixels) ?? (same.length === 1 ? same[0] : null);
      if (found) recent = recent.filter((entry) => entry !== found);
      return found?.decode ?? null;
    };

    // How Figma decodes images on the main thread, as its wasm gets it
    hook.imageIo = (value) => {
      if (!value || typeof value.decodeImage !== "function" || value.__ligmaDecode) return value;
      const decode = value.decodeImage;
      value.__ligmaDecode = decode;
      value.decodeImage = function (bytes, ...rest) {
        const decoded = decode.apply(this, arguments);
        try {
          const copy = bytes instanceof Uint8Array ? bytes.slice() : bytes instanceof ArrayBuffer ? new Uint8Array(bytes.slice(0)) : null;
          if (copy) {
            const hash = crypto.subtle.digest("SHA-1", copy).then((digest) => [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join(""));
            Promise.resolve(decoded).then(
              (result) => {
                if (result?.bitmap instanceof ImageBitmap) decodes.set(result.bitmap, { hash, rest });
                else if (result?.rgba instanceof Uint8Array && result.width > 1 && result.height > 1) {
                  recent.push({ width: result.width, height: result.height, sample: sample(result.rgba), decode: { hash, rest }, at: performance.now() });
                  if (recent.length > 64) recent.shift();
                }
              },
              () => {},
            );
          }
        } catch {}
        return decoded;
      };
      io = value;
      return value;
    };
    window.__ligmaModules?.patch([
      {
        match: (source) => source.includes("decodeImage(") && source.includes("decodeAsync(") && source.includes("cancelCurrentImageWorkers("),
        getter: (value) => hook.imageIo(value),
      },
      {
        // Where Figma's bindings hand it to its wasm, as its own module may
        // come with the page, before any patch can take it
        many: true,
        match: (source) => /ImageIo:\(\)=>[\w$]+\.[\w$]+,/.test(source),
        source: (source) => source.replace(/ImageIo:\(\)=>([\w$]+\.[\w$]+),/, "ImageIo:()=>window.__ligmaHook?.imageIo?.($1)??$1,"),
      },
      {
        // Before Figma draws layers for an export or a thumbnail
        match: (source) => source.includes("loadAllImagesUnder(") && source.includes("unresolvedImagesUnder(") && source.includes("findImagesUnder("),
        source: (source) => {
          const bindings = source.match(/([\w$]+\.[\w$]+)\?\.findImagesUnder\(/)?.[1];
          const documents = source.match(/([\w$]+\.[\w$]+)\?\.getMutableActiveDocument\(\)/)?.[1];
          if (!bindings || !documents) return source;
          return source.replace(
            /\bloadAllImagesUnder\(([^)]*)\)\{/,
            (all, params) =>
              `__ligmaImagesUnder(e,t,r){const n=r??${documents}?.getMutableActiveDocument();return n&&${bindings}?.findImagesUnder(n,e,t)||[]}` +
              `loadAllImagesUnder(...a){const p=this.__ligmaLoadAllImagesUnder(...a),h=window.__ligmaHook;return h?.imagesNeeded?p.then(r=>h.imagesNeeded(()=>this.__ligmaImagesUnder(a[0],a[1],a[4]),a[2]).then(()=>r,()=>r)):p}` +
              `__ligmaLoadAllImagesUnder(${params}){`,
          );
        },
      },
      {
        // Figma downloads image fills 3 at a time, so a file opened with a
        // thousand in view keeps its loading screen up for minutes, waiting
        // on a round trip each. Its image host takes many more at once
        match: (source) => source.includes("this.MAX_REQUESTS=3,this.SINATRA_BATCH_SIZE="),
        source: (source) => source.replace("this.MAX_REQUESTS=3,this.SINATRA_BATCH_SIZE=", "this.MAX_REQUESTS=12,this.SINATRA_BATCH_SIZE="),
      },
      {
        // Figma's decoder draws an image it decodes larger than it'll draw
        // smaller on a canvas and reads it back. Drawn on the GPU, each
        // whole image goes to video memory first, 300 MB more for a few
        // large images, so it's drawn on the CPU
        match: (source) => source.includes("decodeAsync(") && source.includes("scaleImageBitmap("),
        source: (source) =>
          source.replace(
            /(let ([\w$]+)=document\.createElement\("canvas"\);function [\w$]+\([\w$]+\)\{[\w$]+=[\w$]+\}let [\w$]+=)\2\.getContext\("2d"\)/,
            '$1$2.getContext("2d",{willReadFrequently:!0})',
          ),
      },
    ]);

    const plain = (descriptor) => {
      const size = descriptor?.size;
      const [width, height, layers] = Array.isArray(size) ? size : [size?.width, size?.height, size?.depthOrArrayLayers];
      return (
        descriptor?.label === "Image" &&
        width > 1 &&
        height > 1 &&
        (layers ?? 1) === 1 &&
        (descriptor.mipLevelCount ?? 1) === 1 &&
        (descriptor.sampleCount ?? 1) === 1 &&
        (descriptor.dimension ?? "2d") === "2d" &&
        /^(rgba8unorm|bgra8unorm)(-srgb)?$/.test(descriptor.format) &&
        !descriptor.viewFormats?.length && { width, height }
      );
    };
    GPUDevice.prototype.createTexture = function (descriptor) {
      const size = plain(descriptor);
      if (!size) return createTexture.apply(this, arguments);
      const standIn = createTexture.call(this, { label: "Image", size: [1, 1], format: descriptor.format, usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
      Object.defineProperties(standIn, {
        width: { value: size.width },
        height: { value: size.height },
        usage: { value: descriptor.usage },
      });
      const image = { device: this, descriptor, size, texture: null, version: 0, pending: null, copied: null, decode: null, timer: 0 };
      images.set(standIn, image);
      collected.register(standIn, image);
      return standIn;
    };
    // Stand-ins Figma lets go of without destroying them
    const collected = new FinalizationRegistry((image) => {
      clearTimeout(image.timer);
      release(image.pending);
      image.pending = null;
    });
    const full = (image, bitmap) => bitmap.width === image.size.width && bitmap.height === image.size.height;
    // Whether what's kept of an image is smaller than it
    const smallerKept = (image, pending) => !!pending?.bitmap && !full(image, pending.bitmap);
    // A texture for the image with `pending`'s pixels
    const upload = (image, pending) => {
      if (pending.data) {
        const texture = createTexture.call(image.device, image.descriptor);
        const { width, height } = image.size;
        try {
          write.call(image.device.queue, { ...pending.destination, texture }, pending.data, { offset: 0, bytesPerRow: width * 4, rowsPerImage: height }, [width, height]);
        } catch {
          hook.images.failed++;
        }
        release(pending);
        return texture;
      }
      const bitmap = pending.bitmap;
      const small = !full(image, bitmap);
      const texture = createTexture.call(image.device, small ? { ...image.descriptor, size: [bitmap.width, bitmap.height] } : image.descriptor);
      try {
        copyExternal.call(image.device.queue, { ...pending.source, source: bitmap }, { ...pending.destination, texture }, small ? [bitmap.width, bitmap.height] : pending.copySize);
      } catch {
        hook.images.failed++;
      }
      letGo(bitmap);
      return texture;
    };
    // The image's texture, made and given its pixels if not yet
    const made = (image) => {
      if (image.texture) return image.texture;
      const pending = image.pending;
      image.pending = null;
      clearTimeout(image.timer);
      if (pending) hook.images.drawn++;
      if (smallerKept(image, pending)) hook.images.fromSafety++;
      image.texture = pending ? upload(image, pending) : createTexture.call(image.device, image.descriptor);
      image.version++;
      return image.texture;
    };
    const texture = (t) => {
      const image = t && images.get(t);
      return image ? made(image) : t;
    };
    const view = (v) => {
      const of = v && views.get(v);
      if (!of) return v;
      const texture = made(of.image);
      if (!of.view || of.version !== of.image.version) {
        of.view = createView.call(texture, of.descriptor);
        of.version = of.image.version;
      }
      return of.view;
    };
    const withTexture = (where) => (where && images.has(where.texture) ? { ...where, texture: texture(where.texture) } : where);

    GPUTexture.prototype.createView = function (descriptor) {
      const image = images.get(this);
      if (!image) return createView.apply(this, arguments);
      const standIn = createView.call(this);
      views.set(standIn, { image, descriptor, view: null, version: -1 });
      return standIn;
    };
    GPUTexture.prototype.destroy = function () {
      const image = images.get(this);
      if (image) {
        clearTimeout(image.timer);
        release(image.pending);
        image.pending = null;
        image.texture?.destroy();
      }
      return destroy.call(this);
    };

    // Figma's pixels for a stand-in, kept, as long as they're the whole of
    // an image Figma decoded; anything else makes its texture at once
    const whole = (source, destination, copySize, image) => {
      const size = Array.isArray(copySize) ? copySize : [copySize?.width, copySize?.height];
      const origin = (o) => !o || ((o.x ?? o[0] ?? 0) === 0 && (o.y ?? o[1] ?? 0) === 0);
      return (
        source?.source instanceof ImageBitmap &&
        full(image, source.source) &&
        size[0] === image.size.width &&
        (size[1] ?? 1) === image.size.height &&
        origin(source.origin) &&
        origin(destination.origin) &&
        (destination.mipLevel ?? 0) === 0
      );
    };
    // What's kept of the image made no more than `most` pixels across
    const keepSmaller = (image, most) => {
      const pending = image.pending;
      const { width, height } = pending?.bitmap ?? image.size;
      const scale = pending && most / Math.max(width, height);
      if (!(scale < 1)) return;
      let source = pending.bitmap;
      if (!source) {
        try {
          source = new ImageData(new Uint8ClampedArray(pending.data.buffer, pending.data.byteOffset, width * height * 4), width, height);
        } catch {
          return;
        }
      }
      createImageBitmap(source, {
        resizeWidth: Math.max(1, Math.round(width * scale)),
        resizeHeight: Math.max(1, Math.round(height * scale)),
        resizeQuality: "high",
        premultiplyAlpha: "none",
        colorSpaceConversion: "none",
      }).then(
        (small) => {
          if (image.pending !== pending) return close.call(small);
          keep(small);
          // Bytes become a bitmap, copied in as Figma's bitmaps are
          image.pending = { source: pending.source ?? {}, destination: pending.destination, copySize: pending.copySize, bitmap: small };
          release(pending);
          hook.images.shrunk++;
        },
        () => {},
      );
    };
    // Kept as decoded, or, decoded again for an export, whole till it's
    // drawn or let go of
    const pend = (image, bitmap, pending, again = false) => {
      keep(bitmap);
      image.pending = { ...pending, bitmap };
      image.copied = { source: pending.source, destination: pending.destination };
      shrinkLater(image, again);
    };
    const shrinkLater = (image, again = false) => {
      if (!again) keepSmaller(image, PREVIEW);
      clearTimeout(image.timer);
      image.timer = setTimeout(() => keepSmaller(image, SAFETY), KEEP_FULL);
    };
    const pendBytes = (image, data, destination) => {
      keptBytes += data.byteLength;
      hook.images.keptMB = Math.round(keptBytes / 1e6);
      const size = [image.size.width, image.size.height];
      image.pending = { source: {}, destination, copySize: size, data };
      image.copied = { source: {}, destination };
      shrinkLater(image);
    };
    // An image Figma decoded, found by its hash before an export
    const remember = (image, decode) => {
      if (!decode || image.decode) return;
      image.decode = decode;
      const ref = new WeakRef(image);
      decode.hash.then(
        (hash) => {
          const list = (byHash.get(hash) ?? []).filter((r) => r.deref());
          list.push(ref);
          byHash.set(hash, list);
        },
        () => {},
      );
    };
    GPUQueue.prototype.copyExternalImageToTexture = function (source, destination, copySize) {
      const image = images.get(destination?.texture);
      if (!image) return copyExternal.apply(this, arguments);
      if (!image.texture && whole(source, destination, copySize, image)) {
        release(image.pending);
        const { source: bitmap, ...rest } = source;
        const { texture: _, ...to } = destination;
        pend(image, bitmap, { source: rest, destination: to, copySize });
        hook.images.deferred++;
        remember(image, decodes.get(bitmap));
        return;
      }
      const target = made(image);
      // A texture made smaller takes no other size
      if (target.width !== image.size.width || target.height !== image.size.height) return;
      return copyExternal.call(this, source, { ...destination, texture: target }, copySize);
    };

    // Images under the layers Figma is about to draw, at full size again
    // where kept or made smaller. Figma's own decoder decodes them, as it
    // did at first, from the file's bytes. Given up on after `RESTORE_WAIT`
    // ms, for Figma to go ahead as it would have
    const RESTORE_WAIT = 15000;
    const restore = async (image) => {
      const smaller = image.texture ? !full(image, image.texture) : smallerKept(image, image.pending);
      if (!smaller || !image.decode || !io?.__ligmaDecode) return;
      const hash = await image.decode.hash;
      const bytes = await window.figma?.getImageByHash?.(hash)?.getBytesAsync?.();
      if (!bytes) return;
      const result = await io.__ligmaDecode.call(io, bytes, ...image.decode.rest);
      let bitmap = result?.bitmap;
      // Decoded to bytes, as Figma writes those
      if (!bitmap && result?.rgba instanceof Uint8Array && result.width === image.size.width && result.height === image.size.height) {
        const { width, height } = image.size;
        bitmap = await createImageBitmap(new ImageData(new Uint8ClampedArray(result.rgba.buffer, result.rgba.byteOffset, width * height * 4), width, height), {
          premultiplyAlpha: "none",
          colorSpaceConversion: "none",
        }).catch(() => null);
      }
      if (!(bitmap instanceof ImageBitmap) || !full(image, bitmap)) {
        try {
          bitmap?.close?.();
        } catch {}
        return;
      }
      if (image.texture && image.copied) {
        // Bind groups made with the smaller one keep it till they're gone
        keep(bitmap);
        image.texture = upload(image, { ...image.copied, copySize: [bitmap.width, bitmap.height], bitmap });
        image.version++;
      } else if (image.pending) {
        const { source, destination, copySize } = image.pending;
        release(image.pending);
        pend(image, bitmap, { source, destination, copySize }, true);
      } else return close.call(bitmap);
      hook.images.restored++;
    };
    hook.imagesNeeded = async (hashesUnder, reason) => {
      // Thumbnails and previews are drawn with Figma's smaller decodes
      const small = /thumb|preview/i.test(String(reason));
      let hashes = [];
      try {
        hashes = hashesUnder() ?? [];
      } catch {}
      const work = [];
      for (const hash of hashes) for (const ref of byHash.get(hash) ?? []) {
        const image = ref.deref();
        if (image && (!small || Math.max(image.size.width, image.size.height) <= THUMBNAIL)) work.push(restore(image).catch(() => {}));
      }
      if (!work.length) return;
      await Promise.race([Promise.all(work), new Promise((resolve) => setTimeout(resolve, RESTORE_WAIT))]);
    };

    // The whole of an image written as bytes, rows packed, kept like a
    // bitmap Figma copies in
    const wholeBytes = (destination, data, layout, size, image) => {
      const [width, height] = Array.isArray(size) ? size : [size?.width, size?.height];
      const origin = destination.origin;
      const bytes = image.size.width * image.size.height * 4;
      return (
        width === image.size.width &&
        (height ?? 1) === image.size.height &&
        (!origin || ((origin.x ?? origin[0] ?? 0) === 0 && (origin.y ?? origin[1] ?? 0) === 0)) &&
        (destination.mipLevel ?? 0) === 0 &&
        (layout?.offset ?? 0) === 0 &&
        layout?.bytesPerRow === image.size.width * 4 &&
        (layout.rowsPerImage ?? image.size.height) === image.size.height &&
        (ArrayBuffer.isView(data) || data instanceof ArrayBuffer) &&
        data.byteLength >= bytes
      );
    };
    GPUQueue.prototype.writeTexture = function (destination, data, layout, size) {
      const image = images.get(destination?.texture);
      if (image && !image.texture && wholeBytes(destination, data, layout, size, image)) {
        const length = image.size.width * image.size.height * 4;
        // Copied, as it's a view of Figma's memory, which it reuses
        const view = ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, length) : new Uint8Array(data, 0, length);
        const bytes = view.slice();
        release(image.pending);
        const { texture: _, ...to } = destination;
        pendBytes(image, bytes, to);
        hook.images.deferred++;
        hook.images.written++;
        remember(image, decodedAs(bytes, image.size.width, image.size.height));
        return;
      }
      return write.call(this, withTexture(destination), data, layout, size);
    };
    const createBindGroup = GPUDevice.prototype.createBindGroup;
    GPUDevice.prototype.createBindGroup = function (descriptor) {
      const entries = descriptor?.entries;
      if (!entries?.some?.((e) => views.has(e?.resource))) return createBindGroup.apply(this, arguments);
      return createBindGroup.call(this, { ...descriptor, entries: entries.map((e) => (views.has(e?.resource) ? { ...e, resource: view(e.resource) } : e)) });
    };
    const beginRenderPass = GPUCommandEncoder.prototype.beginRenderPass;
    GPUCommandEncoder.prototype.beginRenderPass = function (descriptor) {
      const attached = (a) => a && (views.has(a.view) || views.has(a.resolveTarget));
      if (!descriptor?.colorAttachments?.some?.(attached) && !attached(descriptor?.depthStencilAttachment)) return beginRenderPass.apply(this, arguments);
      const swap = (a) => (attached(a) ? { ...a, view: view(a.view), resolveTarget: view(a.resolveTarget) } : a);
      return beginRenderPass.call(this, { ...descriptor, colorAttachments: descriptor.colorAttachments?.map(swap), depthStencilAttachment: swap(descriptor.depthStencilAttachment) });
    };
    for (const [name, at] of [
      ["copyTextureToTexture", [0, 1]],
      ["copyTextureToBuffer", [0]],
      ["copyBufferToTexture", [1]],
    ]) {
      const copy = GPUCommandEncoder.prototype[name];
      GPUCommandEncoder.prototype[name] = function (...args) {
        for (const i of at) args[i] = withTexture(args[i]);
        return copy.apply(this, args);
      };
    }
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
    if (!(hook.replacing || hook.resting) || hook.toWorker || !hook.workerScene) return;
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
    if (hook.replacing) under(true);
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
    if (on) hook.resting = false;
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
  let handBack = 0;
  hook.push = (path) => {
    if (path) hook.resting = false;
    hook.pushTo = path;
    hook.wantFull = true;
    worker(!path);
    clearTimeout(handBack);
    clearTimeout(unasked);
    if (path) {
      sendFonts();
      handBack = setTimeout(() => !hook.replacing && !hook.toWorker && handBackNow(), HAND_BACK);
    } else window.__ligmaNative?.follow?.();
    return true;
  };
  // Called by Ligma for a tab left hidden that it lets go of: rather than
  // Figma's worker drawing it, loading its whole scene and images again
  // for nothing, no one does, and the worker's swapped for a fresh one to
  // free what it holds. Shown again, Ligma asks for its frames (`push`)
  // and draws it from the whole scene Figma then sends
  hook.rest = () => {
    clearTimeout(handBack);
    clearTimeout(unasked);
    hook.pushTo = null;
    hook.wantFull = true;
    hook.resting = true;
    worker(false);
    if (hook.replacing) {
      hook.replacing = false;
      under(false);
    }
    freshWorker();
    return true;
  };
})();
