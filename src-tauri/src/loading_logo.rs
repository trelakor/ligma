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
    '<div class="row"><div class="cap"></div><div class="span"></div><div class="dot"></div><div class="divide"><div class="line"></div></div></div></div>';

  // The bottom row as Figma builds its turning top row: the D's round end
  // (`cap`), its straight top and bottom (`span`) and the dot slide
  // together into one circle and apart again, while the D's flat side
  // (`line`) sweeps across, clipped to the row. At rest they're the D and
  // the dot. The middle D unfolds from the stem, the top out of its corner
  const SPIN = "cubic-bezier(.12, 0, .39, 0)";
  const UNFOLD = "cubic-bezier(.33, 1, .68, 1)";
  const PIECES = "[data-ligma-logo] :is(.top, .middle, .cap, .span, .dot, .divide, .line)";
  const TURNING = "[data-ligma-logo] :is(.cap, .span, .dot, .divide, .line)";
  // Turning three times, resting, then three times again
  const turn = (name, start, end) => `@keyframes ${name} {
      0%, 10%, 20%, 80%, 90%, 100% { transform: ${start}; }
      5%, 15%, 25%, 75%, 85%, 95% { transform: ${end}; }
    }`;
  const CSS = `
    @keyframes ${SEEN} { from { opacity: 1; } to { opacity: 1; } }
    ${FIGMA_LOGO} { animation: ${SEEN} 1ms; }
    ${FIGMA_LOGO} > :not([data-ligma-logo]) { display: none !important; }
    [data-ligma-logo] { position: relative; width: 22px; height: 32px; margin: -1px auto; color: var(--color-icon); pointer-events: none; }
    [data-ligma-logo] div { position: absolute; box-sizing: border-box; }
    ${PIECES} { animation: 4s infinite; will-change: transform; }
    ${TURNING} { animation-timing-function: ${SPIN}; }
    [data-ligma-logo] :is(.top, .middle) { width: 12px; height: 12px; border: 2px solid currentColor; animation-timing-function: ${UNFOLD}; }
    [data-ligma-logo] .top { left: 0; top: 0; border-radius: 6px 6px 0 6px; transform-origin: right bottom; animation-name: ligma-logo-top; }
    [data-ligma-logo] .middle { left: 0; top: 10px; border-radius: 6px 0 0 6px; transform-origin: right center; animation-name: ligma-logo-middle; }
    [data-ligma-logo] .row { left: 0; top: 20px; width: 22px; height: 12px; }
    [data-ligma-logo] .cap { left: 0; width: 6px; height: 12px; border: 2px solid currentColor; border-right: 0; border-radius: 6px 0 0 6px; animation-name: ligma-logo-cap; }
    [data-ligma-logo] .span { left: 6px; width: 6px; height: 12px; border-block: 2px solid currentColor; animation-name: ligma-logo-span; }
    [data-ligma-logo] .dot { left: 10px; width: 12px; height: 12px; border: 2px solid currentColor; border-radius: 50%; animation-name: ligma-logo-dot; }
    [data-ligma-logo] .divide { left: 0; top: -1px; width: 22px; height: 14px; border: 2px solid transparent; border-radius: 7px; overflow: hidden; animation-name: ligma-logo-divide; }
    [data-ligma-logo] .line { left: 8px; width: 2px; height: 100%; background: currentColor; animation-name: ligma-logo-line; }
    ${turn("ligma-logo-cap", "translateX(5px)", "translateX(0)")}
    ${turn("ligma-logo-span", "scaleX(.1)", "scaleX(1)")}
    ${turn("ligma-logo-dot", "translateX(-5px)", "translateX(0)")}
    ${turn("ligma-logo-divide", "scaleX(.5)", "scaleX(1)")}
    @keyframes ligma-logo-line {
      0%, 10.1%, 20.1%, 80.1%, 90.1% { transform: translateX(-9px); }
      5%, 15%, 25%, 75%, 85%, 95% { transform: translateX(0); }
      10%, 20%, 80%, 90%, 100% { transform: translateX(9px); }
    }
    @keyframes ligma-logo-middle {
      0%, 22%, 80%, 100% { transform: scaleX(.01); opacity: 0; }
      22.1%, 79.9% { opacity: 1; }
      28%, 74% { transform: scaleX(1); }
    }
    @keyframes ligma-logo-top {
      0%, 26%, 74%, 100% { transform: scale(.01); opacity: 0; }
      26.1%, 73.9% { opacity: 1; }
      32%, 68% { transform: scale(1); }
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

  let shown = null;
  const show = (figmaLogo) => {
    if (figmaLogo.querySelector("[data-ligma-logo]")) return;
    figmaLogo.insertAdjacentHTML("beforeend", LOGO);
    shown = figmaLogo.lastElementChild;
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
  addEventListener("animationstart", (event) => event.animationName === SEEN && show(event.target), true);
})();"##;
