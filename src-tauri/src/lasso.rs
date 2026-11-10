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
//! Figma's scripts see them and draws the lasso itself. It looks as
//! Figma's Select area does: its icon and cursor, and a dashed outline
//! tinted with the selection color
//!
//! When the button's let go, the loop is evened out to a point every
//! pixel on screen and smoothed, as mice and pens report whole pixels and
//! hands shake, keeping turns over 120° as corners. It's then fitted with
//! as few cubic curves as stay within 2 pixels, with Philip J. Schneider's
//! algorithm, which comes out smoother than Pencil. It becomes a filled
//! vector, through Figma's
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
  // Figma's own Select area icon, a dotted loop, and its cursor, an arrow
  // with a dotted loop by it
  const LOOP =
    "m10.197 17.884.33.06q.496.073.98.05a.5.5 0 0 1 .043 1 6 6 0 0 1-1.169-.062l-.393-.07-.097-.031a.501.501 0 0 1 .205-.958zm4.23-.859a.5.5 0 0 1 .534.846q-.667.422-1.377.694a.5.5 0 0 1-.357-.934 7 7 0 0 0 1.2-.606m-7.703-.582a.5.5 0 0 1 .63-.053l.077.065.126.124q.193.18.402.321l.29.186q.146.088.29.168a.5.5 0 0 1-.483.876 8 8 0 0 1-.328-.19l-.326-.21a4 4 0 0 1-.527-.422l-.163-.158-.062-.08a.5.5 0 0 1 .074-.627m10.105-1.63a.5.5 0 0 1 .726.68l-.231.291-.25.287q-.258.282-.526.541a.5.5 0 0 1-.695-.719q.245-.236.481-.496l.221-.254q.107-.127.204-.256zm-11.322-1.85a.5.5 0 0 1 .54.328l.025.098.062.376q.11.557.301 1.06a.5.5 0 0 1-.935.355 7.5 7.5 0 0 1-.347-1.224l-.071-.428-.004-.101a.5.5 0 0 1 .43-.464m13.012-1.264a.5.5 0 0 1 .429.563 6.3 6.3 0 0 1-.393 1.48.5.5 0 0 1-.93-.37 5.3 5.3 0 0 0 .331-1.245.5.5 0 0 1 .563-.428M5.46 9.622a.5.5 0 0 1 .944.332c-.166.47-.279.958-.342 1.448l-.022.099a.501.501 0 0 1-.97-.228l.065-.416a8 8 0 0 1 .325-1.235m12.753-.865c.287.434.5.922.631 1.443a.501.501 0 0 1-.97.244 3.7 3.7 0 0 0-.496-1.136l.418-.275zm-8.182-.42a.5.5 0 0 1 .47.877l-.277.191q-.14.099-.28.205l-.086.053a.5.5 0 0 1-.516-.85l.301-.221q.15-.107.299-.207zm7.49.278a.5.5 0 0 1 .692.142l-.835.551a.5.5 0 0 1 .143-.693m-9.706-2.28a.5.5 0 0 1 .538.838l-.286.242q-.413.376-.738.807a.5.5 0 1 1-.797-.604A7 7 0 0 1 7.73 6.39zm5.392 1.105.109.488a7 7 0 0 0-1.247.414l-.097.03a.501.501 0 0 1-.303-.947l.366-.15a8 8 0 0 1 1.063-.323zm1.943-.59.204.033q.605.112 1.129.363l.172.087.084.056a.5.5 0 0 1-.461.865l-.094-.04-.268-.126a3.6 3.6 0 0 0-.906-.248.5.5 0 0 1 .14-.99m-2.052.102a.5.5 0 0 1 .218.976zM10.62 5.001a.5.5 0 0 1 .25.946l-.094.036q-.374.099-.714.228l-.098.026a.5.5 0 0 1-.255-.962l.397-.137a9 9 0 0 1 .414-.122z";
  const ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24" data-fpl-icon-size="24"><path fill="var(--fpl-icon-color, var(--color-icon))" d="${LOOP}"/></svg>`;
  const MASK = `url("data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" fill="none"><path fill="#000" d="${LOOP}"/></svg>`)}")`;
  const CURSOR = `url("data:image/svg+xml,${encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" fill="none"><defs><filter id="s" x="-50%" y="-50%" width="200%" height="200%"><feDropShadow dy="0.5" stdDeviation="0.8" flood-opacity="0.35"/></filter></defs><g filter="url(#s)"><path d="M4 4l11.8 4.6-5.6 1.6-1.6 5.6z" fill="#000" stroke="#fff" stroke-width="1.2" stroke-linejoin="round"/><g transform="translate(7.9 10.2) scale(0.9)"><path d="${LOOP}" fill="#fff" stroke="#fff" stroke-width="2.4" stroke-linejoin="round"/><path d="${LOOP}" fill="#000"/></g></g></svg>`,
  )}") 4 4, crosshair`;

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
  // The lasso as it's drawn, over the canvas, in its points on the canvas:
  // closed, tinted and dashed in the selection color, as Figma's Select
  // area draws its own
  let drawing = null;
  const overlay = (canvas) => {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute(LASSO, "overlay");
    const path = svg.appendChild(document.createElementNS("http://www.w3.org/2000/svg", "path"));
    canvas.after(svg);
    return { svg, path };
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
    view.path.setAttribute("d", `M${screen.join("L")}Z`);
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
        drawing = { canvas, figma, pointer: event.pointerId, points: [toCanvas(figma, canvas, event)], view: overlay(canvas), fill: lastFill(figma) };
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
      const prepared = prepare(local, 1 / zoom);
      const curves = prepared && fitLoop(prepared, 2 / zoom);
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

  // Evenly spaced points a pixel (`px`) apart around the loop, smoothed,
  // with the sharp turns that stay sharp. Mice and pens report whole
  // pixels, so a slow stroke comes as a staircase, which fitting closely
  // would follow
  const prepare = (input, px) => {
    const len = (x, y) => Math.hypot(x, y);
    const loop = [...input, input[0]];
    const even = [loop[0]];
    let since = 0;
    for (let i = 1; i < loop.length; i++) {
      const [ax, ay] = loop[i - 1];
      const [bx, by] = loop[i];
      const segment = len(bx - ax, by - ay);
      let at = 0;
      while (since + segment - at >= px) {
        at += px - since;
        since = 0;
        even.push([ax + ((bx - ax) * at) / segment, ay + ((by - ay) * at) / segment]);
      }
      since += segment - at;
    }
    while (even.length > 3 && len(even.at(-1)[0] - even[0][0], even.at(-1)[1] - even[0][1]) < px / 2) even.pop();
    const n = even.length;
    if (n < 8) return null;
    const wrap = (i) => ((i % n) + n) % n;

    // Gaussian smoothing, `sigma` points either way, around the loop. Pins
    // stay where they are, and points near one reach only as far the
    // other way, so nothing is pulled around it
    const smooth = (points, sigma, pins) => {
      const reach = Math.ceil(sigma * 3);
      const weights = [];
      for (let j = 0; j <= reach; j++) weights.push(Math.exp(-(j * j) / (2 * sigma * sigma)));
      const room = new Array(n).fill(reach);
      for (const pin of pins) {
        for (let j = 0; j <= reach; j++) {
          room[wrap(pin + j)] = Math.min(room[wrap(pin + j)], j);
          room[wrap(pin - j)] = Math.min(room[wrap(pin - j)], j);
        }
      }
      return points.map((_, i) => {
        let x = 0;
        let y = 0;
        let total = 0;
        for (let j = -room[i]; j <= room[i]; j++) {
          const w = weights[Math.abs(j)];
          const [px2, py2] = points[wrap(i + j)];
          x += w * px2;
          y += w * py2;
          total += w;
        }
        return [x / total, y / total];
      });
    };

    // Corners: where the way in and the way out, twelve pixels either side,
    // turn by more than 120°, at the sharpest turn among their neighbors.
    // A hand's tremor turns sharply too, but only over a few pixels
    const rough = smooth(even, 2.5, []);
    const R = 12;
    const unit = (x, y) => {
      const l = len(x, y) || 1;
      return [x / l, y / l];
    };
    const turn = rough.map((p, i) => {
      const a = rough[wrap(i - R)];
      const b = rough[wrap(i + R)];
      const [ux, uy] = unit(p[0] - a[0], p[1] - a[1]);
      const [vx, vy] = unit(b[0] - p[0], b[1] - p[1]);
      return ux * vx + uy * vy;
    });
    const corners = [];
    for (let i = 0; i < n; i++) {
      if (turn[i] > -0.5) continue;
      let sharpest = true;
      for (let j = 1; j <= R && sharpest; j++) sharpest = turn[i] <= turn[wrap(i + j)] && turn[i] < turn[wrap(i - j)];
      if (sharpest) corners.push(i);
    }
    return { points: smooth(even, 3, corners), corners };
  };

  // Fits `prepare`'s loop with as few cubic curves as stay within
  // `tolerance` of it, by Schneider's algorithm ("An Algorithm for
  // Automatically Fitting Digitized Curves", Graphics Gems, 1990), as
  // Paper.js simplifies paths. Corners stay sharp, and elsewhere curves
  // meet smoothly. Returns `[start, control, control, end]` for each, the
  // last ending where the first starts
  const fitLoop = ({ points, corners }, tolerance) => {
    const sub = (a, b) => [a[0] - b[0], a[1] - b[1]];
    const add = (a, b) => [a[0] + b[0], a[1] + b[1]];
    const scale = (a, s) => [a[0] * s, a[1] * s];
    const dot = (a, b) => a[0] * b[0] + a[1] * b[1];
    const len = (a) => Math.hypot(a[0], a[1]);
    const unit = (a) => {
      const l = len(a);
      return l ? [a[0] / l, a[1] / l] : [0, 0];
    };
    const n = points.length;
    const at = (i) => points[((i % n) + n) % n];
    // Ways in and out are taken over a few points, as single steps wobble
    const W = 3;

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
      const middle = unit(sub(run[Math.max(0, split - W)], run[Math.min(run.length - 1, split + W)]));
      fitCubics(run.slice(0, split + 1), left, middle, error, out);
      fitCubics(run.slice(split), scale(middle, -1), right, error, out);
    };

    // Each stretch between corners, or the whole loop
    const stops = corners.length ? corners : [0];
    const curves = [];
    for (let k = 0; k < stops.length; k++) {
      const from = stops[k];
      const to = k + 1 < stops.length ? stops[k + 1] : stops[0] + n;
      const run = [];
      for (let i = from; i <= to; i++) run.push(at(i));
      // At a corner each side keeps its own way, elsewhere both share one
      const shared = unit(sub(at(from + W), at(from - W)));
      const reach = Math.max(1, Math.min(W, to - from));
      const left = corners.length ? unit(sub(at(from + reach), at(from))) : shared;
      const right = corners.length ? unit(sub(at(to - reach), at(to))) : scale(shared, -1);
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
    [${LASSO}="overlay"] path { fill: color-mix(in srgb, var(--color-border-selected, #0d99ff) 25%, transparent); stroke: var(--color-border-selected, #0d99ff); stroke-width: 1px; stroke-dasharray: 3 2; stroke-linejoin: round; }
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
