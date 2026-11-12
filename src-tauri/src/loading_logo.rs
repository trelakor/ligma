//! Ligma's logo on Figma's loading screen: while a file loads, the stroked
//! Figma logo over its progress bar becomes Ligma's, drawn the same way, in
//! outline, and animated the same way: the top piece spins three times, the
//! rest unfolds below it one piece after another, holds, folds back and the
//! top spins again, in a 4 second loop with Figma's timing and easing
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
  const LOGO = '<div data-ligma-logo aria-hidden="true"><div class="top"><div class="seam"></div></div><div class="middle"></div><div class="bottom"></div><div class="dot"></div></div>';

  // The top spins as Figma's does: it narrows and widens three times while
  // a seam sweeps across it, then rests with the seam out of sight. The
  // middle and bottom unfold from the stem, the dot out of the bottom, and
  // fold away in reverse
  const SPIN = "cubic-bezier(.12, 0, .39, 0)";
  const UNFOLD = "cubic-bezier(.33, 1, .68, 1)";
  const PIECES = "[data-ligma-logo] :is(.top, .seam, .middle, .bottom, .dot)";
  const CSS = `
    @keyframes ${SEEN} { from { opacity: 1; } to { opacity: 1; } }
    ${FIGMA_LOGO} { animation: ${SEEN} 1ms; }
    ${FIGMA_LOGO} > :not([data-ligma-logo]) { display: none !important; }
    [data-ligma-logo] { position: relative; width: 22px; height: 32px; margin: -1px auto; color: var(--color-icon); pointer-events: none; }
    [data-ligma-logo] > div { position: absolute; box-sizing: border-box; width: 12px; height: 12px; border: 2px solid currentColor; }
    ${PIECES} { animation: 4s infinite; will-change: transform; }
    [data-ligma-logo] .top { left: 0; top: 0; border-radius: 6px 6px 0 6px; overflow: hidden; animation-name: ligma-logo-spin; animation-timing-function: ${SPIN}; }
    [data-ligma-logo] .seam { position: absolute; left: 3px; top: 0; width: 2px; height: 100%; background: currentColor; animation-name: ligma-logo-seam; animation-timing-function: ${SPIN}; }
    [data-ligma-logo] .middle { left: 0; top: 10px; border-radius: 6px 0 0 6px; transform-origin: right center; animation-name: ligma-logo-middle; animation-timing-function: ${UNFOLD}; }
    [data-ligma-logo] .bottom { left: 0; top: 20px; border-radius: 6px 0 0 6px; transform-origin: right center; animation-name: ligma-logo-bottom; animation-timing-function: ${UNFOLD}; }
    [data-ligma-logo] .dot { left: 10px; top: 20px; border-radius: 50%; transform-origin: left center; animation-name: ligma-logo-dot; animation-timing-function: ${UNFOLD}; }
    @keyframes ligma-logo-spin {
      0%, 10%, 20%, 80%, 90%, 100% { transform: scaleX(.5); }
      5%, 15%, 25%, 75%, 85%, 95% { transform: scaleX(1); }
    }
    @keyframes ligma-logo-seam {
      0%, 10%, 20%, 80%, 90%, 100% { transform: translateX(0); }
      5%, 15%, 25%, 75%, 85%, 95% { transform: translateX(5px); }
      5.1%, 15.1%, 75.1%, 85.1%, 95.1% { transform: translateX(-5px); }
    }
    @keyframes ligma-logo-middle {
      0%, 22%, 80%, 100% { transform: scaleX(.01); opacity: 0; }
      22.1%, 79.9% { opacity: 1; }
      28%, 74% { transform: scaleX(1); }
    }
    @keyframes ligma-logo-bottom {
      0%, 24%, 76%, 100% { transform: scaleX(.01); opacity: 0; }
      24.1%, 75.9% { opacity: 1; }
      30%, 70% { transform: scaleX(1); }
    }
    @keyframes ligma-logo-dot {
      0%, 26%, 74%, 100% { transform: scaleX(.01); opacity: 0; }
      26.1%, 73.9% { opacity: 1; }
      32%, 68% { transform: scaleX(1); }
    }
    @media (prefers-reduced-motion: reduce) {
      ${PIECES} { animation: none; }
      [data-ligma-logo] .seam { transform: translateX(5px); }
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
