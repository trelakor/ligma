//! Ligma's logo on Figma's loading screen: while a file loads, the stroked
//! Figma logo over its progress bar becomes Ligma's, drawn the same way, in
//! outline, and animated the same way, in a 4 second loop with Figma's
//! timing and easing. Figma's top row, two shapes side by side, turns like a
//! cylinder three times: it narrows to a circle and widens again while the
//! line between the two sweeps across. The rest unfolds from it one piece
//! after another, holds, folds back and the row turns again. Ligma's pair
//! is its bottom row, the D and the dot, so that row turns and the D above
//! and the top unfold upward from it
//!
//! Figma's logo is a React element (`progress_bar--strokedLogo--<hash>`).
//! Its parts are hidden and Ligma's added beside them, so React finds its
//! own as it left them. `PAGE_JS` watches the page for it only while the
//! page loads. After that, a do-nothing CSS animation on it says when one
//! appears, without watching every change to the page

/// Runs in Figma tabs
pub const PAGE_JS: &str = r##"(() => {
  if (!/(^|\.)figma\.com$/.test(location.hostname)) return;
  const FIGMA_LOGO = '[class*="progress_bar--strokedLogo"]';
  const SEEN = "ligma-loading-logo-seen";
  // Ligma's logo in boxes, as Figma draws its own: 10px squares with a 2px
  // border, overlapping by theirs. Figma's main thread is busy for most of
  // a load, and only transforms of HTML boxes keep moving smoothly then,
  // on the compositor; an SVG's would stall with it
  const LOGO =
    '<div data-ligma-logo aria-hidden="true"><div class="top"></div><div class="middle"></div>' +
    '<div class="row"><div class="cap"></div><div class="span"></div><div class="rest"></div><div class="end"></div>' +
    '<div class="divide"><div class="boundary"><div class="line"></div><div class="arc"></div></div></div></div></div>';

  // The bottom row as Figma builds its turning top row: an outline of the
  // row's two round ends (`cap`, `end`) and the straight top and bottom
  // between them (`span`) closes into one circle and opens again, while
  // the boundary between the D and the dot, the D's flat side and the dot's
  // left arc, sweeps across it, clipped to the row. At rest the straight
  // parts are the D's own (`rest`), which leaves the row the D and the dot.
  // The middle D unfolds from the stem, the top out of its corner
  const SPIN = "cubic-bezier(.12, 0, .39, 0)";
  const UNFOLD = "cubic-bezier(.33, 1, .68, 1)";
  // Folding away is unfolding played backwards: the same steps, mirrored
  // about the middle of the loop, eased in instead of out
  const FOLD = "cubic-bezier(.32, 0, .67, 0)";
  const PIECES = "[data-ligma-logo] :is(.top, .middle, .cap, .span, .rest, .end, .divide, .boundary)";
  const TURNING = "[data-ligma-logo] :is(.cap, .span, .rest, .end, .divide, .boundary)";
  // Turning three times, resting, then three times again
  const turn = (name, start, end) => `@keyframes ${name} {
      0%, 10%, 20%, 80%, 90%, 100% { transform: ${start}; }
      5%, 15%, 25%, 75%, 85%, 95% { transform: ${end}; }
    }`;
  const CSS = `
    @keyframes ${SEEN} { from { opacity: 1; } to { opacity: 1; } }
    ${FIGMA_LOGO} { animation: ${SEEN} 1s infinite; }
    ${FIGMA_LOGO} > :not([data-ligma-logo]) { display: none !important; }
    [data-ligma-logo] { position: relative; width: 22px; height: 32px; margin: -1px auto; color: var(--color-icon); pointer-events: none; }
    [data-ligma-logo] div { position: absolute; box-sizing: border-box; }
    ${PIECES} { animation: 4s infinite; will-change: transform; }
    ${TURNING} { animation-timing-function: ${SPIN}; }
    [data-ligma-logo] :is(.top, .middle) { width: 12px; height: 12px; border: 2px solid currentColor; animation-timing-function: ${UNFOLD}; }
    [data-ligma-logo] .top { left: 0; top: 0; border-radius: 6px 6px 0 6px; transform-origin: right bottom; animation-name: ligma-logo-top; }
    [data-ligma-logo] .middle { left: 0; top: 10px; border-radius: 6px 0 0 6px; transform-origin: right center; animation-name: ligma-logo-middle; }
    [data-ligma-logo] .row { left: 0; top: 20px; width: 22px; height: 12px; }
    [data-ligma-logo] :is(.cap, .arc) { width: 6px; height: 12px; border: 2px solid currentColor; border-right: 0; border-radius: 6px 0 0 6px; }
    [data-ligma-logo] .cap { left: 0; animation-name: ligma-logo-cap; }
    [data-ligma-logo] :is(.span, .rest) { left: 6px; height: 12px; border-block: 2px solid currentColor; }
    [data-ligma-logo] .span { width: 10px; opacity: 0; animation-name: ligma-logo-span; }
    [data-ligma-logo] .rest { width: 6px; animation-name: ligma-logo-rest; }
    [data-ligma-logo] .end { left: 16px; width: 6px; height: 12px; border: 2px solid currentColor; border-left: 0; border-radius: 0 6px 6px 0; animation-name: ligma-logo-end; }
    [data-ligma-logo] .divide { left: 0; width: 22px; height: 12px; border-radius: 6px; overflow: hidden; animation-name: ligma-logo-divide; }
    [data-ligma-logo] .boundary { left: 0; width: 22px; height: 12px; animation-name: ligma-logo-boundary; }
    [data-ligma-logo] .line { left: 10px; width: 2px; height: 12px; background: currentColor; }
    [data-ligma-logo] .arc { left: 10px; }
    ${turn("ligma-logo-cap", "translateX(5px)", "translateX(0)")}
    ${turn("ligma-logo-end", "translateX(-5px)", "translateX(0)")}
    ${turn("ligma-logo-divide", "scaleX(.5)", "scaleX(1)")}
    @keyframes ligma-logo-span {
      0%, 10%, 20%, 80%, 90%, 100% { transform: scaleX(.1); }
      5%, 15%, 25%, 75%, 85%, 95% { transform: scaleX(1); }
      0%, 25%, 75%, 100% { opacity: 1; }
      25.1%, 74.9% { opacity: 0; }
    }
    @keyframes ligma-logo-rest {
      0%, 25%, 75%, 100% { opacity: 0; }
      25.1%, 74.9% { opacity: 1; }
    }
    @keyframes ligma-logo-boundary {
      0%, 10.1%, 20.1%, 80.1%, 90.1% { transform: translateX(-9px); }
      5%, 15%, 25%, 75%, 85%, 95% { transform: translateX(0); }
      10%, 20%, 80%, 90%, 100% { transform: translateX(9px); }
    }
    @keyframes ligma-logo-middle {
      0%, 22%, 78%, 100% { transform: scaleX(.01); opacity: 0; }
      22.1%, 77.9% { opacity: 1; }
      28% { transform: scaleX(1); }
      72% { transform: scaleX(1); animation-timing-function: ${FOLD}; }
    }
    @keyframes ligma-logo-top {
      0%, 26%, 74%, 100% { transform: scale(.01); opacity: 0; }
      26.1%, 73.9% { opacity: 1; }
      32% { transform: scale(1); }
      68% { transform: scale(1); animation-timing-function: ${FOLD}; }
    }
    @media (prefers-reduced-motion: reduce) {
      ${PIECES} { animation: none; }
    }`;

  const style = document.createElement("style");
  style.id = "ligma-loading-logo";
  style.textContent = CSS;
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  // Figma replaces its logo while a file loads, and the new one would
  // start the animation over. Every logo of one loading screen runs on
  // one clock instead: its animations all start when the first one did,
  // so a new logo, or one shown again, carries on where the last one was.
  // A logo that turns up once none has been on screen for 10 seconds is a
  // new loading screen, which starts the clock again. `SEEN` repeats each
  // second while Figma's logo is on screen, which says it still is
  let shown = null;
  let started = null;
  let lastSeen = -Infinity;
  const sync = (element) => {
    for (const animation of element.getAnimations({ subtree: true })) animation.startTime = started;
  };
  const show = (figmaLogo) => {
    const now = performance.now();
    if (now - lastSeen > 10000) started = document.timeline.currentTime;
    lastSeen = now;
    if (figmaLogo.querySelector("[data-ligma-logo]")) return;
    figmaLogo.insertAdjacentHTML("beforeend", LOGO);
    shown = figmaLogo.lastElementChild;
    sync(shown);
  };
  // While the page loads, Figma is too busy to run the animation's event
  // for most of a second, so the logo is caught as it's added instead,
  // before it's drawn. Figma may put a new one in its place. Once none has
  // been back for a while, the event is enough
  let settling = false;
  const settle = () => {
    settling = false;
    if (document.querySelector(FIGMA_LOGO)) return;
    loading.disconnect();
  };
  const loading = new MutationObserver((records) => {
    if (shown && !shown.isConnected && !settling) {
      settling = true;
      setTimeout(settle, 3000);
    }
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node.nodeType !== Node.ELEMENT_NODE) continue;
        const figmaLogo = node.matches(FIGMA_LOGO) ? node : node.querySelector(FIGMA_LOGO);
        if (figmaLogo) show(figmaLogo);
      }
    }
  });
  loading.observe(document, { childList: true, subtree: true });
  setTimeout(() => loading.disconnect(), 60000);
  addEventListener("animationstart", (event) => {
    if (event.animationName === SEEN) show(event.target);
    // A piece's animation starts over when Figma hides the logo and shows it again
    else if (event.animationName.startsWith("ligma-logo-") && started != null) sync(event.target);
  }, true);
  addEventListener("animationiteration", (event) => event.animationName === SEEN && (lastSeen = performance.now()), true);
})();"##;
