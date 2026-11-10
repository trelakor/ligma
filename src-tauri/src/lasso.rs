//! Lasso fill (Settings > Features > Toolbar): a tool in Figma Design's
//! Creation tools menu, under Pencil, that fills what's drawn around with
//! color, as Procreate's lasso fill does. On by default
//!
//! Figma's menu is its own React component, so `PAGE_JS` adds a copy of
//! Pencil's item to it as it opens, which hover and the keyboard reach
//! like the rest. Picking it turns on Figma's Pencil, so Figma's toolbar,
//! shortcuts and Escape treat it as a tool of their own, and the button
//! shows the lasso while it's the group's tool. While Pencil is on with
//! the lasso picked, `PAGE_JS` takes the canvas's pointer events before
//! Figma's scripts see them and draws the lasso itself
//!
//! When the button's let go, the loop is fitted with as few cubic curves
//! as stay within 1.5 pixels on screen, with Philip J. Schneider's
//! algorithm, keeping sharp turns as corners, which gives about as many
//! points as Pencil does. It becomes a filled vector, through Figma's
//! console plugin API, `window.figma`, in the innermost frame or section
//! under where it started, leaving out auto layout as Pencil does, as one
//! undo step. Its fill is the last lasso fill's, as it is now, so changing
//! one's color changes the next. Figma only offers that API in files that
//! can be edited, so the item isn't added in others

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_LASSO_FILL__` is `{ on, rev }`: the setting
/// when the tab was made. A page loaded later in the same tab may find a
/// newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-lasso-fill";
  let state = __LIGMA_LASSO_FILL__;
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

  const LASSO = "data-ligma-lasso";
  const CHEVRON = '[data-testid="PenTools-chevron"]';
  const STROKE = 'stroke="var(--fpl-icon-color, var(--color-icon))"';
  const FILL = 'fill="var(--fpl-icon-color, var(--color-icon))"';
  // A loop of rope, filled inside, with its end hanging
  const shape = (stroke, fill) =>
    `<path ${stroke} stroke-linecap="round" d="M8.5 14.6C6 13.8 4.5 12.2 4.5 10.5c0-2.8 3.4-5 7.5-5s7.5 2.2 7.5 5-3.4 5-7.5 5c-.9 0-1.8-.1-2.6-.3"/><ellipse ${fill} cx="12" cy="10.5" rx="5" ry="2.8"/><circle ${stroke} cx="8.5" cy="15.5" r="1.5"/><path ${stroke} stroke-linecap="round" d="M8 17c-.3 1 0 1.8.8 2.5"/>`;
  const ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24" data-fpl-icon-size="24">${shape(STROKE, FILL)}</svg>`;
  const MASK = `url("data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" fill="none">${shape('stroke="#000"', 'fill="#000"')}</svg>`)}")`;
  // A crosshair, with a small filled loop by it
  const CURSOR = `url("data:image/svg+xml,${encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" fill="none"><path d="M9 2v14M2 9h14" stroke="#fff" stroke-width="3"/><path d="M9 2v14M2 9h14" stroke="#000"/><ellipse cx="17.5" cy="17.5" rx="4.5" ry="3" fill="#000" stroke="#fff"/></svg>',
  )}") 9 9, crosshair`;

  // Whether the lasso is the Creation tools group's tool: from picking it
  // until Pen or Pencil is picked, from the menu or by their shortcuts
  let picked = false;
  const group = () => document.querySelector(CHEVRON)?.closest('[role="group"]');
  // With the lasso picked, Figma's Pencil is on: the lasso is on with it
  const pencil = () => group()?.querySelector('[data-testid="Pencil-tool"]');
  const active = () => state.on && picked && pencil()?.getAttribute("aria-pressed") === "true";
  const show = () => {
    const root = document.documentElement;
    if (state.on && picked) root.setAttribute(LASSO, active() ? "active" : "picked");
    else root.removeAttribute(LASSO);
  };
  const pick = (on) => {
    picked = on;
    show();
  };

  // The tooltip and label Figma gives the button come from its attributes,
  // read on hover: the lasso's own while it's the group's tool
  const label = (button) => {
    if (state.on && picked) {
      if (button.dataset.ligmaLassoTip) return;
      button.dataset.ligmaLassoTip = JSON.stringify([button.getAttribute("aria-label"), button.dataset.tooltip, button.dataset.tooltipShortcut]);
      button.setAttribute("aria-label", "Lasso fill");
      button.dataset.tooltip = "Lasso fill";
      delete button.dataset.tooltipShortcut;
    } else if (button.dataset.ligmaLassoTip) {
      const [aria, tooltip, shortcut] = JSON.parse(button.dataset.ligmaLassoTip);
      delete button.dataset.ligmaLassoTip;
      if (aria) button.setAttribute("aria-label", aria);
      if (tooltip) button.dataset.tooltip = tooltip;
      if (shortcut) button.dataset.tooltipShortcut = shortcut;
    }
  };

  // The menu the chevron opens, labelled by it: Pen, then Pencil. The
  // lasso's item is Pencil's, copied, with its own icon and name and no
  // shortcut, checked while the lasso is on
  const menu = () => {
    const chevron = document.querySelector(CHEVRON);
    return chevron?.id ? document.querySelector(`[role="menu"][aria-labelledby="${CSS.escape(chevron.id)}"]`) : null;
  };
  const items = (list) => [...list.querySelectorAll('[role="menuitemradio"]')];
  const addItem = (list) => {
    if (!state.on || !api() || list.querySelector(`[${LASSO}]`)) return;
    const source = items(list).find((item) => item.textContent.includes("Pencil"));
    if (!source) return;
    const item = source.cloneNode(true);
    item.setAttribute(LASSO, "");
    item.setAttribute("aria-checked", String(active()));
    item.setAttribute("tabindex", "-1");
    item.removeAttribute("data-fpl-selected");
    item.removeAttribute("id");
    const icon = item.querySelector('svg[data-fpl-icon-size="24"]');
    if (icon) icon.outerHTML = ICON;
    const name = [...item.querySelectorAll("div")].find((node) => node.firstChild?.nodeType === Node.TEXT_NODE && node.firstChild.textContent.trim() === "Pencil");
    if (name) name.firstChild.textContent = "Lasso fill";
    const shortcut = [...item.querySelectorAll('[aria-hidden="true"]')].find((node) => node.textContent.trim() === "Shift+P");
    if (shortcut) shortcut.textContent = "";
    item.addEventListener("pointerenter", () => item.focus());
    item.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      choose(source);
    });
    source.after(item);
  };
  // Pencil's item, clicked, turns on Pencil and closes the menu
  let choosing = false;
  const choose = (source) => {
    pick(true);
    choosing = true;
    try {
      source.click();
    } finally {
      choosing = false;
    }
    setTimeout(show, 100);
  };
  // As the menu opens, and while it's open, as React may draw it again
  let watching = null;
  const watch = (frames = 60) => {
    const list = menu();
    if (!list) {
      if (frames > 0) requestAnimationFrame(() => watch(frames - 1));
      return;
    }
    if (watching?.list === list) return;
    watching?.observer.disconnect();
    addItem(list);
    const observer = new MutationObserver(() => {
      if (!list.isConnected) {
        observer.disconnect();
        if (watching?.list === list) watching = null;
        return;
      }
      addItem(list);
    });
    observer.observe(list, { childList: true, subtree: true });
    watching = { list, observer };
  };

  const typing = (event) => event.target.closest?.(':is(input, textarea):not([readonly]), [contenteditable]:not([contenteditable="false"])');
  const swallow = (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
  };

  addEventListener(
    "pointerdown",
    (event) => {
      if (!state.on) return;
      if (event.target.closest?.(CHEVRON)) watch();
      // Pen or Pencil picked from the menu
      const item = event.target.closest?.('[role="menuitemradio"]');
      if (item && !item.hasAttribute(LASSO) && item.closest('[role="menu"]') === menu()) pick(false);
    },
    true,
  );
  addEventListener(
    "click",
    (event) => {
      const item = event.target.closest?.('[role="menuitemradio"]');
      if (!choosing && item && !item.hasAttribute(LASSO) && item.closest('[role="menu"]') === menu()) pick(false);
    },
    true,
  );
  addEventListener(
    "pointerover",
    (event) => {
      const button = event.target.closest?.('[data-testid="Pencil-tool"]');
      if (button) label(button);
    },
    true,
  );

  // Keys: the shortcuts for Pen and Pencil pick them, and in the menu the
  // arrows reach the lasso's item, after Pencil's and before Pen's
  let space = false;
  addEventListener(
    "keydown",
    (event) => {
      if (event.key === " ") space = true;
      if (!state.on) return;
      const list = menu();
      const inMenu = list && list.contains(event.target);
      if (!inMenu && event.target.closest?.(CHEVRON) && ["Enter", " ", "ArrowDown", "ArrowUp"].includes(event.key)) watch();
      if (inMenu) {
        const all = items(list);
        const ours = all.find((item) => item.hasAttribute(LASSO));
        if (!ours) return;
        const others = all.filter((item) => item !== ours);
        const at = event.target.closest?.('[role="menuitemradio"]');
        if (at === ours) {
          if (event.key === "Enter" || event.key === " ") {
            swallow(event);
            choose(others.find((item) => item.textContent.includes("Pencil")));
          } else if (event.key === "ArrowUp") {
            swallow(event);
            others.at(-1)?.focus();
          } else if (event.key === "ArrowDown") {
            swallow(event);
            others[0]?.focus();
          }
        } else if (at && event.key === "ArrowDown" && at === others.at(-1)) {
          swallow(event);
          ours.focus();
        } else if (at && event.key === "ArrowUp" && at === others[0]) {
          swallow(event);
          ours.focus();
        } else if (at && (event.key === "Enter" || event.key === " ")) pick(false);
        return;
      }
      if (typing(event) || event.ctrlKey || event.metaKey || event.altKey) return;
      if (event.code === "KeyP") pick(false);
      if (event.key === "Escape" && drawing) {
        swallow(event);
        stop();
      }
    },
    true,
  );
  addEventListener("keyup", (event) => event.key === " " && (space = false), true);
  addEventListener("blur", () => (space = false));

  // Figma's canvas, the largest
  const mainCanvas = () =>
    [...document.querySelectorAll("canvas")].reduce(
      (best, canvas) => (canvas.offsetWidth * canvas.offsetHeight > (best ? best.offsetWidth * best.offsetHeight : 0) ? canvas : best),
      null,
    );

  // The fill: the last lasso fill's top solid fill, as it is now, or else
  // the one it had, or black. Node ids are only unique in a file
  const FILL_KEY = "ligma-lasso-last";
  const BLACK = { color: { r: 0, g: 0, b: 0 }, opacity: 1 };
  const fileKey = () => /^\/(?:design|file|board|proto)\/([^/]+)/.exec(location.pathname)?.[1] ?? "";
  const lastFill = async (figma) => {
    let last = null;
    try {
      last = JSON.parse(localStorage.getItem(FILL_KEY));
    } catch {}
    let paint = last?.paint ?? BLACK;
    if (last?.id && last.file === fileKey()) {
      try {
        const node = await figma.getNodeByIdAsync(last.id);
        const fills = node && !node.removed && Array.isArray(node.fills) ? node.fills : [];
        const top = fills.filter((fill) => fill.type === "SOLID" && fill.visible !== false).at(-1);
        if (top) paint = { color: top.color, opacity: top.opacity ?? 1 };
      } catch {}
    }
    return paint;
  };
  const css = ({ color: { r, g, b } }) => `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;

  // The lasso as it's drawn, over the canvas, in its points on the canvas
  let drawing = null;
  const overlay = (canvas) => {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute(LASSO, "overlay");
    const make = (name) => {
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("class", name);
      return svg.appendChild(path);
    };
    const paths = { fill: make("fill"), line: make("line"), close: make("close") };
    canvas.after(svg);
    return { svg, paths };
  };
  let frame = 0;
  const render = () => {
    frame = 0;
    if (!drawing) return;
    const { canvas, view, points, figma } = drawing;
    Object.assign(view.svg.style, {
      left: `${canvas.offsetLeft}px`,
      top: `${canvas.offsetTop}px`,
      width: `${canvas.offsetWidth}px`,
      height: `${canvas.offsetHeight}px`,
    });
    let bounds;
    let zoom;
    try {
      ({ bounds, zoom } = { bounds: figma.viewport.bounds, zoom: figma.viewport.zoom });
    } catch {
      return;
    }
    const screen = points.map(([x, y]) => `${((x - bounds.x) * zoom).toFixed(1)} ${((y - bounds.y) * zoom).toFixed(1)}`);
    view.paths.fill.setAttribute("d", `M${screen.join("L")}Z`);
    view.paths.line.setAttribute("d", `M${screen.join("L")}`);
    view.paths.close.setAttribute("d", `M${screen.at(-1)}L${screen[0]}`);
    if (drawing.paint) {
      view.paths.fill.style.fill = css(drawing.paint);
      view.paths.fill.style.fillOpacity = String(0.5 * drawing.paint.opacity);
    }
  };
  const draw = () => {
    if (!frame) frame = requestAnimationFrame(render);
  };
  // A point on the canvas, from where the pointer is on screen
  const toCanvas = (figma, canvas, event) => {
    const rect = canvas.getBoundingClientRect();
    const { bounds, zoom } = figma.viewport;
    return [bounds.x + (event.clientX - rect.left) / zoom, bounds.y + (event.clientY - rect.top) / zoom];
  };
  const stop = () => {
    if (!drawing) return null;
    const done = drawing;
    drawing = null;
    try {
      done.canvas.releasePointerCapture(done.pointer);
    } catch {}
    done.view.svg.remove();
    return done;
  };

  // The canvas's pointer events are the lasso's while it's on: this runs
  // before Figma's own scripts, so it comes first. Space held, or another
  // button, leaves them to Figma to pan
  let afterDraw = false;
  let shown = 0;
  for (const type of ["pointerdown", "pointermove", "pointerup", "pointercancel", "mousedown", "mousemove", "mouseup", "click", "dblclick"]) {
    addEventListener(
      type,
      (event) => {
        if (drawing) {
          swallow(event);
          if (event.pointerId !== undefined && event.pointerId !== drawing.pointer) return;
          if (type === "pointermove") {
            const events = event.getCoalescedEvents?.() ?? [];
            for (const each of events.length ? events : [event]) drawing.points.push(toCanvas(drawing.figma, drawing.canvas, each));
            draw();
          } else if (type === "pointerup") {
            drawing.points.push(toCanvas(drawing.figma, drawing.canvas, event));
            const done = stop();
            afterDraw = true;
            setTimeout(() => (afterDraw = false));
            finish(done);
          } else if (type === "pointercancel") stop();
          return;
        }
        if (afterDraw && (type === "click" || type === "mouseup")) return swallow(event);
        // The cursor follows Figma's tool, looked up now and then
        if (type === "pointermove") {
          if (picked && event.target?.tagName === "CANVAS" && event.timeStamp - shown > 150) {
            shown = event.timeStamp;
            show();
          }
          return;
        }
        if (type !== "pointerdown" || event.button !== 0 || space || !active()) return;
        const canvas = mainCanvas();
        const figma = api();
        if (!canvas || event.target !== canvas || !figma) return;
        swallow(event);
        try {
          canvas.setPointerCapture(event.pointerId);
        } catch {}
        drawing = { canvas, figma, pointer: event.pointerId, points: [toCanvas(figma, canvas, event)], view: overlay(canvas), paint: null };
        const current = drawing;
        current.fill = lastFill(figma).then((paint) => {
          current.paint = paint;
          draw();
          return paint;
        });
        draw();
      },
      true,
    );
  }

  // The innermost frame, component or section under `point` that Pencil
  // would draw in: shown and unlocked, and not laid out by auto layout
  const CONTAINERS = new Set(["FRAME", "COMPONENT", "SECTION"]);
  const contains = (node, [x, y]) => {
    const [[a, c, e], [b, d, f]] = node.absoluteTransform;
    const det = a * d - b * c;
    if (!det) return false;
    const u = (d * (x - e) - c * (y - f)) / det;
    const v = (a * (y - f) - b * (x - e)) / det;
    return u >= 0 && v >= 0 && u <= node.width && v <= node.height;
  };
  const parentAt = (figma, point) => {
    let parent = figma.currentPage;
    for (;;) {
      const next = [...parent.children]
        .reverse()
        .find((node) => CONTAINERS.has(node.type) && node.visible && !node.locked && (node.layoutMode ?? "NONE") === "NONE" && contains(node, point));
      if (!next) return parent;
      parent = next;
    }
  };

  const finish = async (done) => {
    const { figma, points } = done;
    try {
      const zoom = figma.viewport.zoom;
      // A click, or a line, has nothing inside it
      let area = 0;
      for (let i = 0; i < points.length; i++) {
        const [x1, y1] = points[i];
        const [x2, y2] = points[(i + 1) % points.length];
        area += x1 * y2 - x2 * y1;
      }
      if (points.length < 3 || Math.abs(area / 2) * zoom * zoom < 16) return;
      const paint = await done.fill;
      const parent = parentAt(figma, points[0]);
      // Into the parent's own space, which may be rotated
      let local = points;
      if (parent.type !== "PAGE") {
        const [[a, c, e], [b, d, f]] = parent.absoluteTransform;
        const det = a * d - b * c;
        local = points.map(([x, y]) => [(d * (x - e) - c * (y - f)) / det, (a * (y - f) - b * (x - e)) / det]);
      }
      const curves = fitLoop(local, 1.5 / zoom);
      if (!curves?.length) return;
      const n = (value) => +value.toFixed(3);
      const data = [`M ${n(curves[0][0][0])} ${n(curves[0][0][1])}`, ...curves.map(([, p1, p2, p3]) => `C ${n(p1[0])} ${n(p1[1])} ${n(p2[0])} ${n(p2[1])} ${n(p3[0])} ${n(p3[1])}`), "Z"].join(" ");
      const vector = figma.createVector();
      if (vector.parent !== parent) parent.appendChild(vector);
      vector.name = "Lasso fill";
      vector.x = 0;
      vector.y = 0;
      vector.vectorPaths = [{ windingRule: "NONZERO", data }];
      vector.fills = [{ type: "SOLID", color: paint.color, opacity: paint.opacity }];
      vector.strokes = [];
      try {
        figma.commitUndo();
      } catch {}
      try {
        localStorage.setItem(FILL_KEY, JSON.stringify({ file: fileKey(), id: vector.id, paint }));
      } catch {}
    } catch {
      try {
        figma.notify("Couldn't fill the lasso", { timeout: 3000 });
      } catch {}
    }
  };

  // Fits a closed loop of points with as few cubic curves as stay within
  // `tolerance` of them, by Schneider's algorithm ("An Algorithm for
  // Automatically Fitting Digitized Curves", Graphics Gems, 1990). Sharp
  // turns are kept as corners, and elsewhere curves meet smoothly. Returns
  // `[start, control, control, end]` for each, the last ending where the
  // first starts
  const fitLoop = (input, tolerance) => {
    const sub = (a, b) => [a[0] - b[0], a[1] - b[1]];
    const add = (a, b) => [a[0] + b[0], a[1] + b[1]];
    const scale = (a, s) => [a[0] * s, a[1] * s];
    const dot = (a, b) => a[0] * b[0] + a[1] * b[1];
    const len = (a) => Math.hypot(a[0], a[1]);
    const unit = (a) => {
      const l = len(a);
      return l ? [a[0] / l, a[1] / l] : [0, 0];
    };
    // Points closer than a tenth of the tolerance add nothing
    const points = [];
    for (const point of input) if (!points.length || len(sub(point, points.at(-1))) > tolerance / 10) points.push(point);
    while (points.length > 2 && len(sub(points.at(-1), points[0])) <= tolerance / 10) points.pop();
    const n = points.length;
    if (n < 3) return null;
    const at = (i) => points[((i % n) + n) % n];

    // How many points back and on it takes to go three tolerances along
    // the loop from each point, at least one
    const reach = tolerance * 3;
    const steps = (i, way) => {
      let walked = 0;
      let j = 0;
      while (walked < reach && j < n / 3) {
        walked += len(sub(at(i + (j + 1) * way), at(i + j * way)));
        j++;
      }
      return Math.max(1, j);
    };
    const back = points.map((_, i) => steps(i, -1));
    const on = points.map((_, i) => steps(i, 1));
    // Corners: where the way in and the way out turn by more than 60°, at
    // the sharpest turn among their neighbors
    const turn = points.map((_, i) => dot(unit(sub(at(i), at(i - back[i]))), unit(sub(at(i + on[i]), at(i)))));
    const corners = [];
    for (let i = 0; i < n; i++) {
      if (turn[i] > 0.5) continue;
      let sharpest = true;
      for (let j = 1; j <= on[i] && sharpest; j++) sharpest = turn[i] <= turn[(i + j) % n];
      for (let j = 1; j <= back[i] && sharpest; j++) sharpest = turn[i] < turn[(i - j + n) % n];
      if (sharpest) corners.push(i);
    }

    const point = (curve, t) => {
      const mt = 1 - t;
      const [p0, p1, p2, p3] = curve;
      const a = mt * mt * mt;
      const b = 3 * mt * mt * t;
      const c = 3 * mt * t * t;
      const d = t * t * t;
      return [a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]];
    };
    // Each point's place along the run, by length
    const chordLength = (run) => {
      const u = [0];
      for (let i = 1; i < run.length; i++) u.push(u[i - 1] + len(sub(run[i], run[i - 1])));
      const total = u.at(-1) || 1;
      return u.map((value) => value / total);
    };
    // The cubic through the run's ends, leaving and arriving along `left`
    // and `right`, nearest its points by least squares
    const generate = (run, u, left, right) => {
      const first = run[0];
      const last = run.at(-1);
      let c00 = 0;
      let c01 = 0;
      let c11 = 0;
      let x0 = 0;
      let x1 = 0;
      for (let i = 0; i < run.length; i++) {
        const t = u[i];
        const mt = 1 - t;
        const a1 = scale(left, 3 * t * mt * mt);
        const a2 = scale(right, 3 * t * t * mt);
        c00 += dot(a1, a1);
        c01 += dot(a1, a2);
        c11 += dot(a2, a2);
        const rest = sub(run[i], add(scale(first, mt * mt * mt + 3 * t * mt * mt), scale(last, 3 * t * t * mt + t * t * t)));
        x0 += dot(a1, rest);
        x1 += dot(a2, rest);
      }
      const det = c00 * c11 - c01 * c01;
      let alphaL = 0;
      let alphaR = 0;
      if (Math.abs(det) > 1e-12) {
        alphaL = (x0 * c11 - x1 * c01) / det;
        alphaR = (c00 * x1 - c01 * x0) / det;
      }
      const chord = len(sub(last, first));
      // Too short, backwards or overshooting: a third of the way either side
      if (alphaL < 1e-6 * chord || alphaR < 1e-6 * chord || alphaL > chord * 2 || alphaR > chord * 2) alphaL = alphaR = chord / 3;
      return [first, add(first, scale(left, alphaL)), add(last, scale(right, alphaR)), last];
    };
    const maxError = (run, curve, u) => {
      let max = 0;
      let split = Math.floor(run.length / 2);
      for (let i = 1; i < run.length - 1; i++) {
        const d = sub(point(curve, u[i]), run[i]);
        if (dot(d, d) >= max) {
          max = dot(d, d);
          split = i;
        }
      }
      return [max, split];
    };
    // Newton-Raphson, moving each point's place to its nearest on the curve
    const reparameterize = (run, curve, u) => {
      const [p0, p1, p2, p3] = curve;
      const d1 = [scale(sub(p1, p0), 3), scale(sub(p2, p1), 3), scale(sub(p3, p2), 3)];
      const d2 = [scale(sub(d1[1], d1[0]), 2), scale(sub(d1[2], d1[1]), 2)];
      return u.map((t, i) => {
        const diff = sub(point(curve, t), run[i]);
        const first = add(add(scale(d1[0], (1 - t) * (1 - t)), scale(d1[1], 2 * t * (1 - t))), scale(d1[2], t * t));
        const second = add(scale(d2[0], 1 - t), scale(d2[1], t));
        const denominator = dot(first, first) + dot(diff, second);
        const next = Math.abs(denominator) < 1e-12 ? t : t - dot(diff, first) / denominator;
        return Math.min(1, Math.max(0, next));
      });
    };
    const fitCubics = (run, left, right, error, out) => {
      if (run.length === 2) {
        const d = len(sub(run[1], run[0])) / 3;
        out.push([run[0], add(run[0], scale(left, d)), add(run[1], scale(right, d)), run[1]]);
        return;
      }
      let u = chordLength(run);
      let curve = generate(run, u, left, right);
      let [max, split] = maxError(run, curve, u);
      if (max < error) return void out.push(curve);
      if (max < error * 4) {
        for (let i = 0; i < 4; i++) {
          u = reparameterize(run, curve, u);
          curve = generate(run, u, left, right);
          [max, split] = maxError(run, curve, u);
          if (max < error) return void out.push(curve);
        }
      }
      const middle = unit(sub(run[split - 1], run[split + 1]));
      fitCubics(run.slice(0, split + 1), left, middle, error, out);
      fitCubics(run.slice(split), scale(middle, -1), right, error, out);
    };

    // Each stretch between corners, or the whole loop from its sharpest
    // turn if it has none
    const stops = corners.length ? corners : [turn.indexOf(Math.min(...turn))];
    const curves = [];
    for (let k = 0; k < stops.length; k++) {
      const from = stops[k];
      const to = k + 1 < stops.length ? stops[k + 1] : stops[0] + n;
      const run = [];
      for (let i = from; i <= to; i++) run.push(at(i));
      // At a corner each side keeps its own way, elsewhere both share one
      const shared = unit(sub(at(from + on[from]), at(from - back[from])));
      const left = corners.length ? unit(sub(at(from + Math.min(on[from], to - from)), at(from))) : shared;
      const right = corners.length ? unit(sub(at(to - Math.min(back[to % n], to - from)), at(to))) : scale(shared, -1);
      fitCubics(run, left, right, tolerance * tolerance, curves);
    }
    return curves;
  };

  const style = document.createElement("style");
  style.textContent = `
    html[${LASSO}] [data-testid="Pencil-tool"] > svg { visibility: hidden; }
    html[${LASSO}] [data-testid="Pencil-tool"] { position: relative; }
    html[${LASSO}] [data-testid="Pencil-tool"]::before { content: ""; position: absolute; inset: 0; margin: auto; width: 24px; height: 24px; background-color: var(--fpl-icon-color, var(--color-icon)); -webkit-mask: ${MASK} center / 24px no-repeat; mask: ${MASK} center / 24px no-repeat; }
    html[${LASSO}="active"] canvas { cursor: ${CURSOR} !important; }
    [role="menu"]:has([${LASSO}][aria-checked="true"]) [role="menuitemradio"][aria-checked="true"]:not([${LASSO}]) > span > svg:first-child { visibility: hidden; }
    [${LASSO}="overlay"] { position: absolute; pointer-events: none; overflow: visible; z-index: 1; }
    [${LASSO}="overlay"] .fill { stroke: none; }
    [${LASSO}="overlay"] .line { fill: none; stroke: var(--color-border-selected, #0d99ff); stroke-width: 1px; stroke-linejoin: round; stroke-linecap: round; }
    [${LASSO}="overlay"] .close { fill: none; stroke: var(--color-border-selected, #0d99ff); stroke-width: 1px; stroke-dasharray: 4 3; }
  `;
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  const apply = (next) => {
    state = next;
    if (!state.on) {
      stop();
      picked = false;
      document.querySelectorAll(`[role="menu"] [${LASSO}]`).forEach((item) => item.remove());
    }
    show();
    const button = pencil();
    if (button) label(button);
  };
  // From Ligma, when the setting changes
  window.__ligmaLassoFill = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    apply(next);
  };
  // From another tab
  addEventListener("storage", (event) => {
    if (event.key !== KEY) return;
    try {
      const stored = JSON.parse(event.newValue);
      if (stored?.rev > state.rev) apply(stored);
    } catch {}
  });
})()"##;

/// The setting,
static ON: AtomicBool = AtomicBool::new(true);
/// and when it last changed, for `PAGE_JS` to tell it from older ones in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    format!(r#"{{"on":{},"rev":{}}}"#, ON.load(Ordering::Relaxed), REV.load(Ordering::Relaxed))
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.lasso_fill, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_LASSO_FILL__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.lasso_fill, Ordering::Relaxed) == settings.lasso_fill {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaLassoFill?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
