//! AI actions in free projects (Settings > Features > Images): in files
//! from a free (Starter) project, the bar Figma shows above the toolbar
//! for a selected image keeps only Crop, and Ligma's Copy and Paste
//! adjustments (see adjustments.rs). Select area, Remove background, Edit
//! with prompt and the More menu, all AI actions, are hidden with their
//! divider. On by default
//!
//! In their place goes Ligma's own Remove background, which works on this
//! device (see background.rs), where it can. It changes the top image fill
//! of each selected layer through Figma's console plugin API,
//! `window.figma`, as one undo step. On by default
//!
//! A file's plan is `INITIAL_OPTIONS.plan_tier` ("starter" when free) while
//! the page is still on the file it loaded with, and otherwise the "Free"
//! badge Figma shows by the file's name. The bar is told by Crop's icon,
//! not by its text. Names as seen on 2026-10-29

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_FREE_AI_ACTIONS__` is `{ on, local, rev }`:
/// the settings when the tab was made, `local` only where this device can
/// remove backgrounds. A page loaded later in the same tab may
/// find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-free-ai-actions";
  let state = __LIGMA_FREE_AI_ACTIONS__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  const BADGE = '[class*="paid_status_badge_base--badge"]';
  const fileKey = () => /^\/(?:design|file|board|slides|proto)\/([^/]+)/.exec(location.pathname)?.[1];
  const free = () => {
    const file = window.INITIAL_OPTIONS?.editing_file;
    if (file?.key && file.key === fileKey()) return window.INITIAL_OPTIONS.plan_tier === "starter";
    return /^free$/i.test(document.querySelector(BADGE)?.textContent.trim() ?? "");
  };

  // The plugin API, if the file can be edited
  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.editorType === "figma" ? figma : null;
    } catch {
      return null;
    }
  };
  // Text with mixed fills has none to change
  const fillsOf = (node) => {
    try {
      return Array.isArray(node.fills) ? node.fills : [];
    } catch {
      return [];
    }
  };
  const plural = (count) => `${count} image${count === 1 ? "" : "s"}`;

  // Hands an image to Ligma as a download, and resolves to the cut-out
  // Ligma sends back, as PNG bytes, or rejects with "subject" when there's
  // nothing to keep, "cancelled" when the model wasn't downloaded and
  // "failed" otherwise. `onProgress` gets what Ligma says while it
  // downloads the model. Job numbers start from the time, so a result
  // meant for the page before a reload can't land on a new job
  const jobs = new Map();
  let lastJob = Date.now() * 1000;
  const cutOut = (bytes, onProgress) =>
    new Promise((resolve, reject) => {
      const job = ++lastJob;
      // Typed, or WebView2 sniffs it and names it as text
      const url = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
      let timer;
      const finish = () => {
        clearTimeout(timer);
        jobs.delete(job);
        URL.revokeObjectURL(url);
      };
      // A tab frozen in the meantime never hears back. On Windows the
      // model takes a while on the CPU, and longer the first time
      const wait = () => {
        clearTimeout(timer);
        timer = setTimeout(() => {
          finish();
          reject("failed");
        }, 300000);
      };
      wait();
      jobs.set(job, {
        progress: (text) => {
          wait();
          onProgress(text);
        },
        done: (png) => {
          finish();
          resolve(png);
        },
        failed: (reason) => {
          finish();
          reject(reason);
        },
      });
      const link = Object.assign(document.createElement("a"), { href: url, download: `ligma-background-${job}`, hidden: true });
      document.documentElement.append(link);
      link.click();
      link.remove();
    });
  // From Ligma, with a job's progress and result
  window.__ligmaRemoveBackground = {
    progress: (job, text) => jobs.get(job)?.progress(text),
    done: (job, base64) => jobs.get(job)?.done(Uint8Array.from(atob(base64), (char) => char.charCodeAt(0))),
    failed: (job, reason) => jobs.get(job)?.failed(reason),
  };

  // A white shimmer over each of `nodes` on the canvas, as Figma shows
  // while its own Remove background works, kept on them as the canvas
  // pans and zooms. Returns what stops it
  const SHIMMER = "data-ligma-shimmer";
  const shimmer = (figma, nodes) => {
    const canvas = [...document.querySelectorAll("canvas")].reduce(
      (best, canvas) => (canvas.offsetWidth * canvas.offsetHeight > (best ? best.offsetWidth * best.offsetHeight : 0) ? canvas : best),
      null,
    );
    if (!canvas) return () => {};
    const layer = document.createElement("div");
    layer.setAttribute(SHIMMER, "");
    canvas.after(layer);
    const boxes = nodes.map(() => layer.appendChild(document.createElement("div")));
    let frame;
    const place = () => {
      Object.assign(layer.style, {
        left: `${canvas.offsetLeft}px`,
        top: `${canvas.offsetTop}px`,
        width: `${canvas.offsetWidth}px`,
        height: `${canvas.offsetHeight}px`,
      });
      try {
        const { bounds } = figma.viewport;
        const zoom = figma.viewport.zoom;
        nodes.forEach((node, i) => {
          const box = boxes[i];
          if (node.removed) {
            box.hidden = true;
            return;
          }
          const [[a, c, e], [b, d, f]] = node.absoluteTransform;
          const radius = typeof node.cornerRadius === "number" ? node.cornerRadius * zoom : 0;
          box.hidden = false;
          Object.assign(box.style, {
            width: `${node.width * zoom}px`,
            height: `${node.height * zoom}px`,
            borderRadius: `${radius}px`,
            transform: `matrix(${a}, ${b}, ${c}, ${d}, ${(e - bounds.x) * zoom}, ${(f - bounds.y) * zoom})`,
          });
        });
      } catch {}
      frame = requestAnimationFrame(place);
    };
    place();
    return () => {
      cancelAnimationFrame(frame);
      layer.classList.add("done");
      setTimeout(() => layer.remove(), 200);
    };
  };

  // Replaces the top image fill of each selected layer with its cut-out,
  // as one undo step. Each image is cut out once, however many layers
  // show it
  let busy = false;
  const removeBackground = async () => {
    const figma = api();
    if (!figma || busy) return;
    const targets = [];
    for (const node of figma.currentPage.selection) {
      const fills = fillsOf(node);
      const image = (paint) => paint?.type === "IMAGE" && !!paint.imageHash;
      let index = fills.findLastIndex((paint) => image(paint) && paint.visible !== false);
      if (index < 0) index = fills.findLastIndex(image);
      if (index >= 0) targets.push({ node, index, hash: fills[index].imageHash });
    }
    if (!targets.length) {
      figma.notify("No images selected", { timeout: 2000 });
      return;
    }
    busy = true;
    tidy();
    const stopShimmer = shimmer(figma, targets.map((target) => target.node));
    let progress;
    const show = (text) => {
      try {
        progress?.cancel();
      } catch {}
      progress = figma.notify(text, { timeout: Infinity });
    };
    show("Removing background…");
    const cut = new Map();
    let missing = 0;
    let failed = 0;
    let cancelled = false;
    for (const hash of new Set(targets.map((target) => target.hash))) {
      try {
        const bytes = await figma.getImageByHash(hash).getBytesAsync();
        cut.set(hash, figma.createImage(await cutOut(bytes, show)).hash);
      } catch (reason) {
        if (reason === "cancelled") {
          cancelled = true;
          break;
        }
        if (reason === "subject") missing++;
        else failed++;
      }
    }
    let images = 0;
    for (const { node, index, hash } of targets) {
      if (!cut.has(hash)) continue;
      try {
        // Unless the layer changed in the meantime
        const fills = fillsOf(node);
        if (fills[index]?.imageHash !== hash) continue;
        node.fills = fills.map((paint, i) => (i === index ? { ...paint, imageHash: cut.get(hash) } : paint));
        images++;
      } catch {
        failed++;
      }
    }
    if (images) {
      try {
        figma.commitUndo();
      } catch {}
    }
    busy = false;
    stopShimmer();
    tidy();
    try {
      progress.cancel();
    } catch {}
    if (cancelled && !images) return;
    const message = images
      ? `Background removed${targets.length > 1 ? ` from ${plural(images)}` : ""}${missing ? `, no subject in ${missing}` : ""}${failed ? `, ${failed} failed` : ""}`
      : missing && !failed
        ? "No subject found"
        : "Couldn't remove the background";
    figma.notify(message, { timeout: 3000 });
  };

  // Ligma's Remove background, after Crop and Ligma's adjustments
  const LOCAL = "data-ligma-background";
  const STROKE = 'stroke="var(--fpl-icon-color, var(--color-icon))"';
  const ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M5.5 8.5v-2a1 1 0 0 1 1-1h2m7 0h2a1 1 0 0 1 1 1v2m0 7v2a1 1 0 0 1-1 1h-2m-7 0h-2a1 1 0 0 1-1-1v-2"/><circle ${STROKE} cx="12" cy="10" r="2.5"/><path ${STROKE} d="M8.5 17a3.5 3.5 0 0 1 7 0"/></svg>`;
  const placeButton = (bar, crop) => {
    let button = bar.querySelector(`[${LOCAL}]`);
    if (!button) {
      button = Object.assign(document.createElement("button"), { type: "button", className: crop.className });
      button.setAttribute(LOCAL, "");
      const label = crop.querySelector("span")?.cloneNode(false) ?? document.createElement("span");
      label.textContent = "Remove background";
      button.innerHTML = ICON;
      button.append(label);
      button.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopImmediatePropagation();
        removeBackground();
      });
      const ours = bar.querySelectorAll("[data-ligma-adjustments-bar]");
      (ours[ours.length - 1] ?? crop).after(button);
    }
    button.disabled = busy;
  };

  // The image bar's Crop, by its icon. Everything else in the bar but
  // Ligma's buttons is an AI action, its divider or the More menu
  const BAR = '[data-secondary-toolbelt="true"] [class*="secondary_toolbelt--secondaryToolbeltContainer"]';
  const CROP = 'path[d^="M8.5 5a.5.5 0 0 1 .5.5V15h6V9"]';
  const HIDDEN = "data-ligma-ai-hidden";
  const tidy = () => {
    const hide = state.on && free();
    if (!hide) {
      for (const node of document.querySelectorAll(`[${HIDDEN}]`)) node.removeAttribute(HIDDEN);
      for (const node of document.querySelectorAll(`[${LOCAL}]`)) node.remove();
      return;
    }
    for (const bar of document.querySelectorAll(BAR)) {
      const crop = bar.querySelector(CROP)?.closest("button");
      if (!crop || crop.parentElement !== bar) continue;
      for (const child of bar.children) {
        if (child === crop || child.matches(`[data-ligma-adjustments-bar], [${LOCAL}], [data-editor-theme]`)) continue;
        if (!child.hasAttribute(HIDDEN)) child.setAttribute(HIDDEN, "");
      }
      if (state.local) placeButton(bar, crop);
      else bar.querySelector(`[${LOCAL}]`)?.remove();
    }
  };

  const style = document.createElement("style");
  style.textContent = `[${HIDDEN}] { display: none !important; }
    [${SHIMMER}] { position: absolute; overflow: hidden; pointer-events: none; transition: opacity 200ms ease-out; }
    [${SHIMMER}].done { opacity: 0; }
    [${SHIMMER}] > div { position: absolute; left: 0; top: 0; overflow: hidden; transform-origin: 0 0; background: rgb(255 255 255 / 0.12); }
    [${SHIMMER}] > div::after {
      content: ""; position: absolute; inset: 0;
      background: linear-gradient(100deg, transparent 20%, rgb(255 255 255 / 0.45) 50%, transparent 80%);
      animation: ligma-shimmer 1.6s ease-in-out infinite;
    }
    @keyframes ligma-shimmer { from { transform: translateX(-100%); } to { transform: translateX(100%); } }
    @media (prefers-reduced-motion: reduce) { [${SHIMMER}] > div::after { animation: none; opacity: 0; } }`;
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  // `document`, not its <html>: WebView2 runs this before there is one
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      tidy();
    });
  }).observe(document, { childList: true, subtree: true });

  // From Ligma, when the setting changes
  window.__ligmaFreeAiActions = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    state = next;
    tidy();
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        tidy();
      }
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
})()"##;

/// The settings: hide the AI actions,
static ON: AtomicBool = AtomicBool::new(true);
/// put Ligma's Remove background in their place, where it works,
static LOCAL: AtomicBool = AtomicBool::new(true);
/// and when they last changed, for `PAGE_JS` to tell it from older ones in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    let (on, local) = (ON.load(Ordering::Relaxed), LOCAL.load(Ordering::Relaxed));
    format!(r#"{{"on":{on},"local":{local},"rev":{}}}"#, REV.load(Ordering::Relaxed))
}

fn local(settings: &Settings) -> bool {
    settings.local_remove_background && crate::background::available()
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.hide_free_ai_actions, Ordering::Relaxed);
    LOCAL.store(local(settings), Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_FREE_AI_ACTIONS__", &state())
}

/// Sends changed settings to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    let on = ON.swap(settings.hide_free_ai_actions, Ordering::Relaxed) != settings.hide_free_ai_actions;
    let local = LOCAL.swap(local(settings), Ordering::Relaxed) != local(settings);
    if !on && !local {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaFreeAiActions?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
