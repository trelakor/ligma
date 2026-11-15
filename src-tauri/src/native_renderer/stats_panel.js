(() => {
  // Settings' Show stats, in the Figma tab (see hud.rs): times the frames
  // Figma's own renderer draws, and draws what Ligma hands it each tick,
  // the renderer in use above the one used before
  const IDLE_MS = 250;
  const FRAMES = 200;
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

  const state = { measuring: false, drew: false, last: null, intervals: [], raf: 0, restore: [], host: null, canvas: null, model: null, painting: 0 };

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
  const drew = () => (state.drew = true);
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
      if (state.measuring && event.data?.result?.frameApplied) state.drew = true;
    });
  });

  // A frame of the browser's in which Figma drew
  const loop = (now) => {
    if (!state.measuring) {
      state.raf = 0;
      return;
    }
    if (state.drew) {
      state.drew = false;
      const interval = state.last != null && now - state.last < IDLE_MS ? now - state.last : null;
      state.last = now;
      state.intervals.push(interval);
      const record = state.model?.records[1];
      if (record) {
        push(record, [interval]);
        paint();
      }
    }
    state.raf = requestAnimationFrame(loop);
  };

  const push = (record, frames) => {
    record.frames.push(...frames);
    if (record.frames.length > FRAMES) record.frames.splice(0, record.frames.length - FRAMES);
  };

  // Drawn again at most once a frame of the browser's, as frames come in
  const paint = () => {
    if (state.painting) return;
    state.painting = requestAnimationFrame(() => {
      state.painting = 0;
      try {
        if (state.model) draw(state.model);
      } catch {}
    });
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

  // Over the frames in the graph, the view moving
  const summary = (record) => {
    const frames = record.frames.filter((ms) => ms != null);
    if (!frames.length) return null;
    const sorted = [...frames].sort((a, b) => a - b);
    const at = (p) => sorted[Math.round((sorted.length - 1) * p)];
    const mean = frames.reduce((a, b) => a + b, 0) / frames.length;
    return { fps: 1000 / mean, p50: at(0.5), p95: at(0.95), max: at(1) };
  };

  const dot = (ctx, x, y, color) => {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(x + 3.5, y - 3.5, 3.5, 0, Math.PI * 2);
    ctx.fill();
  };

  // A bar a frame, its time since the one before
  function frameGraph(ctx, graph, record) {
    // Labelled where there's room between the two
    const room = graph.h >= 56;
    guide(ctx, graph, 1000 / 120, SCALE_MS, room ? "120 fps" : "");
    guide(ctx, graph, 1000 / 200, SCALE_MS, room ? "200 fps" : "");
    const step = graph.w / FRAMES;
    const start = FRAMES - record.frames.length;
    record.frames.forEach((ms, i) => {
      if (ms == null) return;
      const h = Math.min(ms / SCALE_MS, 1) * graph.h;
      ctx.fillStyle = ms <= 1000 / 120 + 0.5 ? GOOD : ms <= 1000 / 60 + 0.5 ? WARNING : CRITICAL;
      ctx.fillRect(graph.x + (start + i) * step, graph.y + graph.h - h, Math.max(step - 0.5, 0.5), h);
    });
    line(ctx, graph.x, graph.y + graph.h + 0.5, graph.x + graph.w, GRID);
  }

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
    const s = summary(record);
    if (s) {
      const width = text(ctx, PAD + 8, y, [[`${Math.round(s.fps)}`, ink, BIG]]);
      text(ctx, PAD + 8 + width + 4, y, [["fps", SECONDARY]]);
      textRight(ctx, PAD + inner, y, [
        ["typical ", MUTED],
        [s.p50.toFixed(1), ink],
        ["  p95 ", MUTED],
        [s.p95.toFixed(1), ink],
        ["  worst ", MUTED],
        [`${s.max.toFixed(1)} ms`, ink],
      ]);
    } else {
      text(ctx, PAD + 8, y, [["No frames yet", ink, BIG], ["  move the view to time them", MUTED]]);
    }

    y += 8;
    const frames = { x: PAD + 8, y, w: inner - 8 - GUTTER, h: inUse ? 60 : 40 };
    frameGraph(ctx, frames, record);
    y = frames.y + frames.h + 18;

    // Figma's renderer is in its page, so it's worked out from the page
    // with and without it (see hud.rs), till then shown with the page
    const [renderer, page] = record.memory.at(-1) ?? [0, 0];
    text(ctx, PAD + 8, y, [["GPU memory ", MUTED], [mb(Math.max(renderer, 0) + page), ink]]);
    y += 16;
    let at = PAD + 8;
    const legend =
      renderer < 0
        ? [["Renderer and page ", page, PAGE]]
        : [
            [ligma ? "Renderer " : "Renderer, estimated ", renderer, COLORS[column.renderer]],
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
    const figma = document.querySelector(".gpu-view-content canvas");
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
    const host = figma.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvas.style.left = `${host.left + 8}px`;
    canvas.style.top = `${host.top + 8}px`;
    canvas.style.width = `${WIDTH}px`;
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
    canvas.style.height = `${HEIGHT}px`;
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
      if (!state.measuring) state.intervals = [];
      const out = state.intervals;
      state.intervals = [];
      if (model.records) {
        state.model = model;
        // Figma's frames timed here and not yet in Ligma's records
        if (model.records[1]) push(model.records[1], out);
      } else if (state.model?.active === model.active) {
        const record = state.model.records[model.active === "ligma" ? 0 : 1];
        if (record) push(record, model.frames);
      }
      paint();
      return out;
    },
    remove() {
      state.measuring = false;
      for (const restore of state.restore.reverse()) restore();
      state.host?.remove();
      delete window.__ligmaStats;
    },
  });
})()
