//! Who's on each page: in a file others have open, each page in the left
//! sidebar's pages list shows the people on it, as profile pictures on the
//! right of its row. Hovering one shows their name, and clicking it follows
//! them, to their page and view, as their picture in the top right does
//!
//! Figma's store keeps everyone in the file, with the page each is on
//! (`multiplayer.allUsers`, `currentPageId`). `PAGE_JS` draws them in the
//! empty slot on the right of each row (`[data-page-row-id]`), and follows
//! with Figma's own picture in the top right, or, when that's hidden in
//! its overflow, the function it calls

/// Runs in Figma tabs
pub const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  // More than this on a page show as a count
  const MAX = 3;
  const NO_PAGE = "-1:-1";

  // Figma's store (not `window.store`, which is for debugging and stalls the
  // page) and the function its pictures in the top right follow people
  // with, found by text in their modules, as seen on 2026-11-11:
  // `function(user, mySessionID, dispatch, inPrototype, multiplayer,
  // presenter, onPresenter)`, which calls `observeUser(sessionID)`. One pass
  // over the modules (some 60 ms) looks for what's missing, at most every 5
  // seconds unless `now`, for a click
  const SIGNS = {
    store: "This API is intended only for debugging",
    follow: '"spotlight.prototype_observation_started"',
  };
  let figmaRequire = null;
  const found = {};
  let scanned = -Infinity;
  const figma = (now = false) => {
    if (found.store && found.follow) return found;
    if (!now && performance.now() - scanned < 5000) return found.store ? found : null;
    scanned = performance.now();
    if (!figmaRequire) {
      try {
        globalThis.rspackChunk_figma_web_bundler?.push([[Symbol("ligma")], {}, (r) => { figmaRequire = r; }]);
      } catch {}
    }
    const require = figmaRequire;
    if (!require?.m) return null;
    for (const id of Object.keys(require.m)) {
      const source = Function.prototype.toString.call(require.m[id]);
      try {
        if (!found.store && source.includes(SIGNS.store)) {
          const store = require(id).globalReduxStore;
          if (store?.getState && store.subscribe) watchStore((found.store = store));
        } else if (!found.follow && source.includes(SIGNS.follow) && source.includes(".observeUser(")) {
          const exports = require(id);
          found.follow = Object.keys(exports)
            .map((key) => exports[key])
            .find((value) => typeof value === "function" && Function.prototype.toString.call(value).includes(".observeUser("));
        }
      } catch {}
      if (found.store && found.follow) break;
    }
    return found.store ? found : null;
  };

  const follow = (sessionID) => {
    // Figma's own picture, which does everything a click on it does
    const button = document.querySelector(`button[data-tooltip-proxy-element-id="voice-avatar-session-${sessionID}"]`);
    if (button) {
      button.click();
      return;
    }
    const { store, follow } = figma(true) ?? {};
    const multiplayer = store?.getState().multiplayer;
    const user = multiplayer?.allUsers.find((user) => user.sessionID === sessionID);
    if (follow && user) follow(user, multiplayer.sessionID, store.dispatch, false, multiplayer, null, () => {});
  };

  const STYLE = `
    .ligma-page-people { display: flex; flex: none; align-items: center; gap: 4px; margin-left: auto; padding: 0 8px 0 4px; }
    [class*="pageRowButton"]:has(.ligma-page-people) > [class*="renameableNode"] { min-width: 0; }
    .ligma-page-person { display: flex; flex: none; align-items: center; justify-content: center; box-sizing: border-box; width: 16px; height: 16px; overflow: hidden; border-radius: 50%; background: var(--ligma-person); box-shadow: 0 0 0 1px var(--ligma-person); color: #fff; font-size: 9px; font-weight: 600; line-height: 1; }
    .ligma-page-person[data-following] { box-shadow: 0 0 0 2px var(--ligma-person); }
    .ligma-page-person img { display: block; width: 100%; height: 100%; object-fit: cover; }
    .ligma-page-more { width: auto; min-width: 16px; padding: 0 4px; border-radius: 8px; background: var(--color-bg-tertiary, var(--color-bg-secondary)); box-shadow: none; color: var(--color-text-secondary); }
  `;
  const addStyle = () => {
    if (document.getElementById("ligma-page-people-style") || !document.head) return;
    const style = document.createElement("style");
    style.id = "ligma-page-people-style";
    style.textContent = STYLE;
    document.head.append(style);
  };

  // Only the people's own clicks: the row under them would switch to the
  // page, start a drag or, on a double click, rename it. Figma redraws the
  // row as it's pressed, which takes the strip with it, so no click follows
  // the press: a press and release on the same person is one
  const personAt = (event) => event.target.closest?.(".ligma-page-person:not(.ligma-page-more)");
  let pressed = null;
  const onEvent = (event) => {
    const person = personAt(event);
    if (!person) return;
    event.stopPropagation();
    if (event.type === "click") event.preventDefault();
    if (event.type === "pointerdown") pressed = event.button === 0 ? person.dataset.session : null;
    if (event.type !== "pointerup") return;
    const session = pressed;
    pressed = null;
    if (event.button === 0 && session === person.dataset.session) follow(Number(session));
  };

  const person = (user, following) => {
    const name = user.name || user.handle || "";
    const element = document.createElement("span");
    element.className = "ligma-page-person";
    element.setAttribute("role", "button");
    element.setAttribute("aria-label", `Follow ${name}`);
    element.dataset.session = user.sessionID;
    element.dataset.tooltipType = "text";
    element.dataset.tooltip = name;
    element.dataset.tooltipShowAbove = "true";
    element.dataset.tooltipTimeoutDelay = "150";
    if (following) element.dataset.following = "";
    element.style.setProperty("--ligma-person", user.color || "#8c8c8c");
    if (user.imageURL) {
      const image = document.createElement("img");
      image.alt = "";
      image.draggable = false;
      image.src = user.imageURL;
      // Shows their initial instead
      image.onerror = () => image.replaceWith(document.createTextNode(initial(name)));
      element.append(image);
    } else {
      element.textContent = initial(name);
    }
    return element;
  };
  const initial = (name) => [...name.trim()][0]?.toUpperCase() ?? "";

  const more = (users) => {
    const element = document.createElement("span");
    element.className = "ligma-page-person ligma-page-more";
    element.textContent = `+${users.length}`;
    element.dataset.tooltipType = "text";
    element.dataset.tooltip = users.map((user) => user.name || user.handle).join(", ");
    element.dataset.tooltipShowAbove = "true";
    element.dataset.tooltipTimeoutDelay = "150";
    return element;
  };

  // Everyone else in the file by page, once each, leaving out other tabs of
  // your own as Figma's top right does
  const peopleByPage = (multiplayer) => {
    const me = multiplayer.allUsers.find((user) => user.sessionID === multiplayer.sessionID);
    const pages = new Map();
    for (const user of multiplayer.allUsers) {
      if (user.sessionID === multiplayer.sessionID || (me?.userID && user.userID === me.userID)) continue;
      const page = user.currentPageId;
      if (!page || page === NO_PAGE) continue;
      const people = pages.get(page) ?? [];
      if (user.userID && people.some((other) => other.userID === user.userID)) continue;
      people.push(user);
      pages.set(page, people);
    }
    return pages;
  };

  let grid = null;
  const render = () => {
    const rows = document.querySelectorAll("[data-page-row-id]");
    if (!rows.length) return;
    const multiplayer = figma()?.store.getState().multiplayer;
    if (!Array.isArray(multiplayer?.allUsers)) return;
    addStyle();
    watchRows(rows[0]);
    const pages = peopleByPage(multiplayer);
    for (const row of rows) {
      const slot = row.querySelector('[class*="pageRowRight"]');
      let strip = row.querySelector(".ligma-page-people");
      const people = pages.get(row.dataset.pageRowId) ?? [];
      if (!slot || !people.length) {
        strip?.remove();
        continue;
      }
      const key = people
        .map((user) => [user.sessionID, user.name, user.imageURL, user.color, user.sessionID === multiplayer.observingSessionID].join(" "))
        .join("\n");
      if (strip?.parentElement === slot && strip.dataset.key === key) continue;
      strip?.remove();
      strip = document.createElement("span");
      strip.className = "ligma-page-people";
      strip.dataset.key = key;
      const shown = people.length > MAX ? people.slice(0, MAX - 1) : people;
      for (const user of shown) strip.append(person(user, user.sessionID === multiplayer.observingSessionID));
      if (shown.length < people.length) strip.append(more(people.slice(shown.length)));
      for (const type of ["pointerdown", "pointerup", "mousedown", "mouseup", "click", "dblclick"]) strip.addEventListener(type, onEvent);
      slot.append(strip);
    }
  };

  let queued = false;
  const schedule = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      render();
    });
  };

  // Redrawn when someone joins, leaves or changes page
  const watchStore = (store) => {
    let last = null;
    store.subscribe(() => {
      const multiplayer = store.getState().multiplayer;
      if (multiplayer === last) return;
      last = multiplayer;
      schedule();
    });
  };

  // and when rows are drawn again, as the list scrolls or changes
  const rowsObserver = new MutationObserver((mutations) => {
    if (mutations.every((mutation) => mutation.target.closest?.(".ligma-page-people"))) return;
    schedule();
  });
  const watchRows = (row) => {
    const list = row.closest('[role="grid"]') ?? row.parentElement?.parentElement?.parentElement;
    if (!list || list === grid) return;
    grid = list;
    rowsObserver.disconnect();
    rowsObserver.observe(list, { childList: true, subtree: true });
  };

  // The pages list comes and goes with files and the panel, so it's looked
  // for every second while the tab is shown
  setInterval(() => {
    if (document.visibilityState === "visible" && !grid?.isConnected) render();
  }, 1000);
})()"##;
