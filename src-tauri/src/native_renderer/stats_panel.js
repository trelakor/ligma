(() => {
  // Settings' Show stats, in the Figma tab (see hud.rs): times the frames
  // Figma's own renderer draws, and draws what Ligma hands it each tick,
  // the renderer in use above the one used before
  const FRAMES = 200;
  const ANSWERS_MS = 250;
  const IDLE_MS = 250;
  const PAGE_FRAME_MS = 50;
  const SAMPLES = 240;
  const SCALE_MS = 20;
  const WIDTH = 300;
  // Room for the panel at its tallest
  const HEIGHT = 680;
  const PAD = 10;
  const GUTTER = 44;
  const SURFACE = "#1a1a19";
  const INK = "#f0efec";
  const SECONDARY = "#c3c2b7";
  const MUTED = "#898781";
  const GRID = "#383835";
  const GOOD = "#0ca30c";
  const WARNING = "#fab219";
  const CRITICAL = "#d03b3b";
  const LIGMA = "#3987e5";
  const FIGMA = "#d95926";
  const PAGE = "#898781";
  const FONT = '11px "Segoe UI", system-ui, sans-serif';
  const BIG = '600 15px "Segoe UI", system-ui, sans-serif';
  const BOLD = '600 12px "Segoe UI", system-ui, sans-serif';
  const TAG = '600 10px "Segoe UI", system-ui, sans-serif';
  // Drawn at most this often. Each draw takes the page's main thread, which
  // Figma's frames need, so more often slows the very frames it times
  const PAINT_MS = 33;

  // A frame's time is from the input it answers, or else the start of the
  // browser's frame it's drawn in, to its last drawing: how long Figma took
  // to make it, not how long it waited for input
  const state = { measuring: false, input: null, frame: null, from: 0, made: null, done: null, timed: [], raf: 0, restore: [], host: null, canvas: null, model: null, painting: 0, painted: 0, figma: null, at: null, styles: {} };

  // Figma draws on the page with WebGL or WebGPU, or in its render worker,
  // which answers each frame it's sent
  const wrap = (proto, name, before) => {
    const original = proto?.[name];
    if (typeof original !== "function") return;
    proto[name] = function () {
      before(this, arguments);
      return original.apply(this, arguments);
    };
    state.restore.push(() => (proto[name] = original));
  };
  const drew = () => {
    if (!state.measuring) return;
    const now = performance.now();
    const frame = document.timeline?.currentTime ?? now;
    if (state.frame !== frame) {
      done();
      state.frame = frame;
      // As Ligma's renderer times Figma's part (see run.rs): not from input
      // long before, nor from a frame of the browser's it's drawn outside
      state.from = state.input != null && now - state.input <= ANSWERS_MS ? state.input : now - frame <= PAGE_FRAME_MS ? frame : now;
      state.input = null;
    }
    state.made = now - state.from;
  };
  const input = (event) => {
    if (state.measuring) state.input = event.timeStamp;
  };
  for (const type of ["pointermove", "wheel"]) {
    addEventListener(type, input, { capture: true, passive: true });
    state.restore.push(() => removeEventListener(type, input, { capture: true }));
  }
  // The frame drawn last, timed
  const done = () => {
    if (state.made == null) return;
    const ms = state.made;
    state.made = null;
    // Since the frame before, for the frame rate
    const now = performance.now();
    const since = state.done != null && now - state.done < IDLE_MS ? now - state.done : null;
    state.done = now;
    state.timed.push([ms, since]);
    const record = state.model?.records[1];
    if (record) {
      push(record, [[ms, 0, since]]);
      paint();
    }
  };
  for (const proto of [window.WebGLRenderingContext?.prototype, window.WebGL2RenderingContext?.prototype]) {
    for (const name of ["drawArrays", "drawElements", "drawArraysInstanced", "drawElementsInstanced"]) wrap(proto, name, drew);
  }
  wrap(window.GPUQueue?.prototype, "submit", drew);
  wrap(window.GPUCanvasContext?.prototype, "getCurrentTexture", drew);
  const heard = new WeakSet();
  wrap(window.MessagePort?.prototype, "postMessage", (port, args) => {
    if (args[0]?.method !== "sendCanvasFrameToRenderWorker" || heard.has(port)) return;
    heard.add(port);
    port.addEventListener("message", (event) => {
      if (event.data?.result?.frameApplied) drew();
    });
  });

  // A frame of the browser's: Figma's drawing in the one before is done
  const loop = (now) => {
    if (!state.measuring) {
      state.raf = 0;
      return;
    }
    if (state.frame != null && state.frame !== now) done();
    state.raf = requestAnimationFrame(loop);
  };

  const push = (record, frames) => {
    record.frames.push(...frames);
    if (record.frames.length > FRAMES) record.frames.splice(0, record.frames.length - FRAMES);
  };

  // Drawn again as frames come in, in a frame of the browser's, at most
  // every `PAINT_MS`
  const paint = () => {
    if (state.painting) return;
    const wait = Math.max(0, state.painted + PAINT_MS - performance.now());
    state.painting = setTimeout(() => {
      requestAnimationFrame(() => {
        state.painting = 0;
        state.painted = performance.now();
        try {
          if (state.model) draw(state.model);
        } catch {}
      });
    }, wait);
  };

  // Where Figma's canvas is, measured again only when it or the window
  // changes size, as measuring has the browser lay out the page when
  // anything's changed since, which Figma then waits on each frame
  const measure = (figma) => {
    if (state.figma !== figma) {
      state.observer?.disconnect();
      state.figma = figma;
      state.at = null;
      state.observer = new ResizeObserver(() => (state.at = null));
      state.observer.observe(figma);
    }
    return (state.at ??= figma.getBoundingClientRect());
  };
  const resized = () => (state.at = null);
  addEventListener("resize", resized);
  state.restore.push(() => removeEventListener("resize", resized));

  // Styles set only when they change, as each set has the page laid out again
  const style = (element, name, value) => {
    const key = `${element === state.canvas ? "c" : "h"}.${name}`;
    if (state.styles[key] === value) return;
    state.styles[key] = value;
    element.style[name] = value;
  };

  const text = (ctx, x, y, parts) => {
    let at = x;
    for (const [part, color, font] of parts) {
      ctx.font = font ?? FONT;
      ctx.fillStyle = color;
      ctx.fillText(part, at, y);
      at += ctx.measureText(part).width;
    }
    return at - x;
  };
  const textRight = (ctx, right, y, parts) => {
    const width = parts.reduce((sum, [part, , font]) => {
      ctx.font = font ?? FONT;
      return sum + ctx.measureText(part).width;
    }, 0);
    text(ctx, right - width, y, parts);
  };
  const line = (ctx, x0, y, x1, color, dash) => {
    ctx.strokeStyle = color;
    ctx.lineWidth = 1;
    ctx.setLineDash(dash ? [3, 3] : []);
    ctx.beginPath();
    ctx.moveTo(x0, y);
    ctx.lineTo(x1, y);
    ctx.stroke();
    ctx.setLineDash([]);
  };
  const guide = (ctx, graph, value, top, label) => {
    if (value >= top) return;
    const y = Math.round(graph.y + graph.h - (value / top) * graph.h) + 0.5;
    line(ctx, graph.x, y, graph.x + graph.w, GRID, true);
    text(ctx, graph.x + graph.w + 6, y + 3.5, [[label, MUTED]]);
  };
  const mb = (value) => (value >= 1000 ? `${(value / 1000).toFixed(2)} GB` : `${Math.round(value)} MB`);
  const count = (value) => Math.round(value).toLocaleString("en-US");

  const NAMES = { ligma: "Ligma", figma: "Figma" };
  const COLORS = { ligma: LIGMA, figma: FIGMA };

  // Over the frames in the graph
  const summary = (times) => {
    if (!times.length) return null;
    const sorted = [...times].sort((a, b) => a - b);
    const at = (p) => sorted[Math.round((sorted.length - 1) * p)];
    return { p50: at(0.5), p95: at(0.95), max: at(1) };
  };

  const dot = (ctx, x, y, color) => {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(x + 3.5, y - 3.5, 3.5, 0, Math.PI * 2);
    ctx.fill();
  };

  // A bar a frame, how long it took, colored by how it does against a
  // screen's refresh, or all in `color`
  function frameGraph(ctx, graph, times, color) {
    // Labelled where there's room between the two
    const room = graph.h >= 56;
    guide(ctx, graph, 1000 / 60, SCALE_MS, graph.h >= 32 ? "16.7 ms" : "");
    guide(ctx, graph, 1000 / 120, SCALE_MS, room ? "8.3 ms" : "");
    const step = graph.w / FRAMES;
    const start = FRAMES - times.length;
    times.forEach((ms, i) => {
      const h = Math.max(Math.min(ms / SCALE_MS, 1) * graph.h, 0.5);
      ctx.fillStyle = color ?? (ms <= 1000 / 120 + 0.5 ? GOOD : ms <= 1000 / 60 + 0.5 ? WARNING : CRITICAL);
      ctx.fillRect(graph.x + (start + i) * step, graph.y + graph.h - h, Math.max(step - 0.5, 0.5), h);
    });
    line(ctx, graph.x, graph.y + graph.h + 0.5, graph.x + graph.w, GRID);
  }

  // A line of frame times: the typical big or not, then p95 and worst
  const timesLine = (ctx, y, right, s, ink, label, big, fps) => {
    const rate = fps ? [[`${Math.round(fps)}`, ink, BIG], [" fps  ", SECONDARY]] : [];
    const parts = big ? [...rate, [s.p50.toFixed(1), ink, BOLD], [" ms a frame", SECONDARY]] : [[label, SECONDARY], [s.p50.toFixed(1), ink], [" ms a frame", MUTED]];
    text(ctx, PAD + 8, y, parts);
    textRight(ctx, right, y, [
      ["p95 ", MUTED],
      [s.p95.toFixed(1), ink],
      ["  worst ", MUTED],
      [`${s.max.toFixed(1)} ms`, ink],
    ]);
  };

  // One renderer's section: its stripe and name, its numbers, and graphs of
  // its frames and GPU memory. Gives where it ends
  function section(ctx, y, column, inUse, top) {
    const inner = WIDTH - 2 * PAD;
    const ligma = column.renderer === "ligma";
    const record = column.record;
    const ink = inUse ? INK : SECONDARY;
    const start = y;
    ctx.save();
    if (!inUse) ctx.globalAlpha = 0.85;

    y += 14;
    const name = text(ctx, PAD + 8, y, [[NAMES[column.renderer], ink, BOLD], [ligma ? " (experimental)" : "", MUTED]]);
    if (inUse) {
      ctx.font = TAG;
      const tagWidth = ctx.measureText("In use").width + 12;
      const tagX = PAD + 8 + name + 8;
      ctx.beginPath();
      ctx.roundRect(tagX + 0.5, y - 11.5, tagWidth, 15, 7.5);
      ctx.fillStyle = COLORS[column.renderer];
      ctx.fill();
      text(ctx, tagX + 6.5, y, [["In use", "#ffffff", TAG]]);
    }

    y += 22;
    // Ligma's renderer's time, from Figma's app handing it a frame to
    // showing it, or Figma's renderer's, from input to drawn
    const own = record.frames.map(([figma, renderer]) => (ligma ? renderer : figma));
    const s = summary(own);
    // New frames a second while the view's not at rest
    const since = record.frames.map(([, , since]) => since).filter((ms) => ms != null);
    const fps = since.length ? 1000 / (since.reduce((a, b) => a + b, 0) / since.length) : null;
    if (s) {
      timesLine(ctx, y, PAD + inner, s, ink, "", true, fps);
    } else {
      text(ctx, PAD + 8, y, [["No frames yet", ink, BIG], ["  move the view to time them", MUTED]]);
    }

    y += 8;
    const frames = { x: PAD + 8, y, w: inner - 8 - GUTTER, h: inUse ? 60 : 40 };
    frameGraph(ctx, frames, own);
    y = frames.y + frames.h + 18;

    // Before Ligma's renderer has a frame: how long Figma's app took to
    // make it from the input it answers, small and apart, as it's not
    // the renderer's doing
    const app = ligma ? record.frames.map(([figma]) => figma) : [];
    const a = summary(app);
    if (a) {
      timesLine(ctx, y, PAD + inner, a, ink, "Figma's app ", false);
      y += 6;
      const graph = { x: PAD + 8, y, w: inner - 8 - GUTTER, h: inUse ? 24 : 18 };
      frameGraph(ctx, graph, app, PAGE);
      y = graph.y + graph.h + 18;
    }

    // Figma's renderer is counted in its page (see gpu_count.js), or till
    // it's loaded with it, shown with the page
    const [renderer, page] = record.memory.at(-1) ?? [0, 0];
    text(ctx, PAD + 8, y, [["GPU memory ", MUTED], [mb(Math.max(renderer, 0) + page), ink]]);
    y += 16;
    let at = PAD + 8;
    const legend =
      renderer < 0
        ? [["Renderer and page ", page, PAGE]]
        : [
            ["Renderer ", renderer, COLORS[column.renderer]],
            ["Page ", page, PAGE],
          ];
    for (const [label, value, color] of legend) {
      dot(ctx, at, y, color);
      at += 16 + text(ctx, at + 10, y, [[label, MUTED], [mb(value), ink]]);
    }
    y += 8;
    const memory = { x: PAD + 8, y, w: inner - 8 - GUTTER, h: inUse ? 40 : 32 };
    memoryGraph(ctx, memory, record, top, COLORS[column.renderer]);
    y = memory.y + memory.h;

    const numbers = ligma && inUse ? record.numbers : null;
    if (numbers) {
      y += 18;
      text(ctx, PAD + 8, y, [
        ["Present ", MUTED],
        [numbers.presentMs.toFixed(1), ink],
        ["  Tiles ", MUTED],
        [numbers.tilesMs.toFixed(1), ink],
        ["  Whole view ", MUTED],
        [`${numbers.wholeMs.toFixed(1)} ms`, ink],
      ]);
      y += 16;
      text(ctx, PAD + 8, y, [
        [count(numbers.drawn), ink],
        [" drawn  ", MUTED],
        [count(numbers.approximate), ink],
        [" approximate  ", MUTED],
        [count(numbers.tiles), ink],
        [" tiles  ", MUTED],
        [`${Math.round(numbers.zoom * 100)}%`, ink],
      ]);
    }
    y += 6;
    // The renderer's stripe down the section
    ctx.fillStyle = COLORS[column.renderer];
    ctx.beginPath();
    ctx.roundRect(PAD, start + 2, 3, y - start - 2, 1.5);
    ctx.fill();
    ctx.restore();
    return y;
  }

  // Ligma's process and Figma's page over the last minute in use
  function memoryGraph(ctx, graph, record, top, color) {
    guide(ctx, graph, 400, top, "400 MB");
    const step = graph.w / (SAMPLES - 1);
    const start = SAMPLES - record.memory.length;
    ctx.lineWidth = 1.5;
    for (const [pick, stroke] of [[1, PAGE], [0, color]]) {
      ctx.strokeStyle = stroke;
      ctx.beginPath();
      let drawing = false;
      record.memory.forEach((sample, i) => {
        if (sample[pick] < 0) {
          drawing = false;
          return;
        }
        const point = [graph.x + (start + i) * step, graph.y + graph.h - Math.min(sample[pick] / top, 1) * graph.h];
        drawing ? ctx.lineTo(...point) : ctx.moveTo(...point);
        drawing = true;
      });
      ctx.stroke();
    }
    line(ctx, graph.x, graph.y + graph.h + 0.5, graph.x + graph.w, GRID);
  }

  // The renderer in use, and under it the one used before, to compare, on
  // the same scales. Gives the height it took
  function panel(ctx, active, before) {
    const columns = [active, before].filter(Boolean);
    const top = columns.reduce((top, column) => column.record.memory.reduce((top, [renderer, page]) => Math.max(top, renderer, page), top), 500) * 1.1;
    let y = PAD + 11;
    text(ctx, PAD, y, [["Active Renderer: ", MUTED], [NAMES[active.renderer], INK], [active.renderer === "ligma" ? " (experimental)" : "", MUTED]]);
    y += 6;
    y = section(ctx, y, active, true, top);
    if (before) {
      y += 8;
      line(ctx, PAD, Math.round(y) + 0.5, WIDTH - PAD, GRID);
      y += 4;
      y = section(ctx, y, before, false, top);
    }
    return Math.ceil(y + PAD);
  }

  // Over the corner of Figma's canvas, the largest on the page
  function draw(model) {
    const records = { ligma: model.records[0], figma: model.records[1] };
    const other = model.active === "ligma" ? "figma" : "ligma";
    const active = { renderer: model.active, record: records[model.active] ?? { frames: [], memory: [], numbers: null } };
    const was = records[other];
    const before = was && (was.frames.length || was.memory.length) ? { renderer: other, record: was } : null;
    // Not till Figma's canvas is up, and out of reach of Figma looking for
    // it, in a shadow root
    const figma = state.figma?.isConnected ? state.figma : document.querySelector(".gpu-view-content canvas");
    if (!figma) return;
    if (!state.host?.isConnected) {
      if (!state.host) {
        state.host = document.createElement("div");
        state.host.style.cssText = "position:fixed;left:0;top:0;z-index:2147483647;pointer-events:none;";
        state.canvas = document.createElement("canvas");
        state.canvas.style.cssText = "position:fixed;pointer-events:none;";
        state.host.attachShadow({ mode: "closed" }).append(state.canvas);
      }
      document.documentElement.appendChild(state.host);
    }
    const canvas = state.canvas;
    const host = measure(figma);
    const dpr = window.devicePixelRatio || 1;
    style(canvas, "left", `${host.left + 8}px`);
    style(canvas, "top", `${host.top + 8}px`);
    style(canvas, "width", `${WIDTH}px`);
    if (canvas.width !== Math.round(WIDTH * dpr) || canvas.height !== Math.round(HEIGHT * dpr)) {
      canvas.width = Math.round(WIDTH * dpr);
      canvas.height = Math.round(HEIGHT * dpr);
    }
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, WIDTH, HEIGHT);
    // Laid out once unseen for its height, then drawn on its background
    ctx.save();
    ctx.globalAlpha = 0;
    const height = panel(ctx, active, before);
    ctx.restore();
    style(canvas, "height", `${HEIGHT}px`);
    ctx.fillStyle = SURFACE;
    ctx.beginPath();
    ctx.roundRect(0, 0, WIDTH, height, 8);
    ctx.fill();
    panel(ctx, active, before);
  }

  return (window.__ligmaStats = {
    // Ligma's records in, all of them or the frames its renderer presented
    // since, and the frames Figma drew since the last tick out
    tick(model) {
      state.measuring = model.active === "figma";
      if (state.measuring && !state.raf) state.raf = requestAnimationFrame(loop);
      if (!state.measuring) state.timed = [];
      const out = state.timed;
      state.timed = [];
      if (model.records) {
        state.model = model;
        // Figma's frames timed here and not yet in Ligma's records
        if (model.records[1]) push(model.records[1], out.map(([ms, since]) => [ms, 0, since]));
      } else if (state.model?.active === model.active) {
        const record = state.model.records[model.active === "ligma" ? 0 : 1];
        if (record) push(record, model.frames);
      }
      paint();
      return out;
    },
    remove() {
      state.measuring = false;
      clearTimeout(state.painting);
      state.observer?.disconnect();
      for (const restore of state.restore.reverse()) restore();
      state.host?.remove();
      delete window.__ligmaStats;
    },
  });
})()
