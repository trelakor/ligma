//! Toolbar layout (Settings > Editor > Toolbar): the order of the tools in
//! each mode's toolbar (Draw, Design, Motion and Dev Mode) and of the modes
//! in the switch on its right, and which of them show. A slot's tools can
//! be hidden one by one, so a group can lose its first tool and keep the
//! rest, and a slot with all its tools hidden goes, the toolbar closing up
//! around it. Draw's pen, brush and pencil are one slot, hidden one by one
//! with a stylesheet. Modes are hidden from the switch and from the
//! picker Figma's mode quick toggle shows. With one left, the toolbar shows
//! no switch. The mode you're in always shows, so there's a way back from
//! it
//!
//! Each of Figma's toolbars (Design, Draw and Dev Mode each have one, and
//! Motion shows Design's, so it's told apart by the mode the switch is in)
//! is a row of elements handed to one component, the tools of a group to
//! another, and the switch is a third, handed the modes it shows. The
//! picker lists the modes a jotai atom holds. `PAGE_JS` wraps all four as
//! their modules load (see `modules.rs`) to leave out what's hidden and
//! put the rest in order, so only in pages loaded with the layout changed
//! from Figma's. Changes apply right away in pages it wrapped. Changing it
//! in a file already open otherwise reloads it, when its tab is shown and
//! its edits have been saved. A group left with one tool loses its arrow
//!
//! Settings shows the toolbars as they are, from what a file's page
//! reports: the slots of each mode's toolbar, their tools and the modes,
//! with their names and icons, read from Figma's React elements as Figma hands
//! them over. Pages report when what they see differs from what Ligma
//! has, through `SIGNAL_HOST`, and `on_signal` keeps it in `toolbar.json`
//! in the app config folder
//!
//! Accounts without Draw get Figma's older switch instead, a single Dev
//! Mode toggle. A stylesheet hides it, with no reload, when the mode it
//! goes to is hidden
//!
//! Hiding the mode a file is in switches it to the first one shown, in the
//! switch's order, or the next if Figma doesn't take it. Only then: Figma
//! puts some accounts in a mode on its own, and would put them back
//!
//! Right-clicking the switch opens a menu of the four modes (see
//! tab_menu.rs), checked when shown. `PAGE_JS` asks for it by loading
//! `SIGNAL_HOST`, which `on_signal` handles, and picking one turns it on
//! or off as in Settings
//!
//! Figma's shortcuts and the Actions menu still reach hidden tools and
//! modes: they're handled where Ligma can't reach

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Url, Webview};

use std::collections::BTreeMap;

use crate::settings::{Settings, ToolbarLayout};

/// Runs in Figma tabs. `__LIGMA_TOOLBAR_MODES__` is `{ hidden, modeOrder,
/// layouts, rev, known }`: the modes hidden and their order, by Figma's
/// names for them, each mode's `{ order, hidden }`, the order of its slots
/// and the tools hidden, when the tab was made, and the signatures of the
/// toolbars Ligma has. A
/// page loaded later in the same tab may find a newer one in
/// `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window || !window.__ligmaModules) return;
  const KEY = "ligma-toolbar-modes";
  const normal = (next) => ({ hidden: [], modeOrder: [], layouts: {}, ...next });
  let state = normal(__LIGMA_TOOLBAR_MODES__);
  let known = { tools: {}, ...state.known };
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = normal(stored);
  } catch {}
  const orderOf = (mode) => state.layouts[mode]?.order ?? [];
  const hiddenOf = (mode) => state.layouts[mode]?.hidden ?? [];
  const custom = () => !!(state.hidden.length || state.modeOrder.length || Object.keys(state.layouts).some((mode) => orderOf(mode).length || hiddenOf(mode).length));
  const listeners = new Set();
  const subscribe = (listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };
  // Bumped on each change, so the wrapped components draw again
  let version = 0;
  const getVersion = () => version;
  const redraw = () => {
    version++;
    for (const listener of listeners) listener();
  };
  let React = null;
  // The mode the file is in, as the switch has it, and the mode of the
  // toolbar each row draws, handed down to its groups
  let activeMode = null;
  let Mode = null;

  // `ids` in `order`, those it leaves out after the one before them in
  // Figma's order, or first
  const arrange = (ids, order) => {
    const placed = order.filter((id, i) => ids.includes(id) && order.indexOf(id) === i);
    ids.forEach((id, i) => {
      if (placed.includes(id)) return;
      let at = 0;
      for (let j = i - 1; j >= 0; j--) {
        const k = placed.indexOf(ids[j]);
        if (k >= 0) {
          at = k + 1;
          break;
        }
      }
      placed.splice(at, 0, id);
    });
    return placed;
  };

  // The switch on screen: its modes, the one the file is in and how it
  // switches, for leaving a mode as it's hidden
  let shown = null;
  // Modes hidden while this page couldn't leave them, for the page it
  // reloads into to leave
  const LEAVE_KEY = "ligma-toolbar-leave";
  let leaving = [];
  try {
    leaving = JSON.parse(sessionStorage.getItem(LEAVE_KEY)) ?? [];
    sessionStorage.removeItem(LEAVE_KEY);
  } catch {}
  // The switch's module, as seen on 2026-10-29: `function f(e){let{modes:
  // t,activeMode:n,onModeSwitch:l,...}=e;...}`, its only export. Each mode
  // is `{ mode, icon, label }`, `mode` one of "illustration" (Draw),
  // "design", "metronome" (Motion) and "handoff" (Dev Mode)
  const MODULE = (source) => source.includes('"toolbelt-mode-segmented-control"');
  // The quick toggle's module: atoms, among them the modes the picker
  // lists, most recently used first (so the one you're in first), derived
  // from the others: `(0,r.eU)(e=>{...return e(a).filter(e=>"metronome"===
  // e?t:...)})`
  const QUICK_TOGGLE = (source) => source.includes('"design","illustration","handoff"') && source.includes("visibleModeCount");
  // A toolbar's row, as seen on 2026-11-17: `function b({children:e,
  // editorTheme:t}){return(0,n.jsx)("div",{className:"toolbelt_left_side--
  // leftSideRow--xLXIr",...})}`, handed a div of the toolbar's elements
  const ROW = (source) => source.includes("toolbelt_left_side--leftSideRow") && source.includes("editorTheme");
  // A group's button and its menu, its only export: `function E(e){let{
  // items:t}=e,n=t.filter(e=>!e.hidden);...}`, which draws `function I(e){
  // let{items:t,toolGroupId:u,activeToolId:p,initialToolId:h,...}=e;...}`.
  // It shows the tool in use if it's one of `items`, or else the one last
  // used, or the first
  const GROUP = (source) => source.includes("tool_group--toolGroup--") && source.includes("initialToolId");

  // The toolbar's elements: `{ id, tools }`, the slot they fill and the
  // keys Figma records their tools by, or null for dividers and the like.
  // The Move tools come as one element of their own, which draws Figma's
  // group, the rest as a group (`toolGroupId`, `tools`), a single tool
  // (`config`) or a button (Comment, Actions)
  const MOVE = ["toolDefault", "toolHand", "toolScale"];
  // Draw's pen, brush and pencil, between two dividers, by their tooltips
  const DRAWING = ["set-tool-pen", "set-tool-brush", "set-tool-pencil"];
  const drawing = (element) => {
    const children = element?.props?.children;
    return typeof element?.type === "symbol" && Array.isArray(children) && children.length === 3 && !!children[0]?.props?.extended && !!children[2]?.props?.extended;
  };
  const SINGLE = { toolFrame: "FrameTools", toolType: "TypeTools", toolCodeComponentToolbeltEntry: "CodeTools", toolSpec: "CodeTools", toolComment: "CommentTools" };
  const slotOf = (element) => {
    const props = element?.props;
    if (!props || typeof props !== "object") return null;
    if (props.toolGroupId && Array.isArray(props.tools)) return { id: props.toolGroupId, tools: props.tools.map((tool) => tool.recordingKey) };
    if (props.toolbeltMode !== undefined && props.activateTool) return { id: "SelectTools", tools: props.toolbeltMode === "handoff" ? ["toolDefault"] : MOVE };
    if (drawing(element)) return { id: "DrawingTools", tools: DRAWING };
    const key = props.config?.recordingKey;
    if (key) return { id: SINGLE[key] ?? key, tools: [key] };
    if (props.toolId === "QUICK_ACTIONS") return { id: "Actions", tools: ["actions"] };
    // Comment's key is only on the toolbar on screen: its tool id, learned
    // there, finds it in the others
    if (typeof props.recordingKey === "string" && props.recordingKey.endsWith("toolComment")) commentTool = props.toolId;
    if ((commentTool !== undefined && props.toolId === commentTool && props.icon) || props.recordingKey?.endsWith?.("toolComment")) return { id: "CommentTools", tools: ["toolComment"] };
    return null;
  };
  let commentTool;
  // The mode of the toolbar a row's elements draw
  const modeOf = (elements) => {
    const mode = elements.find((element) => element?.props?.toolbeltMode !== undefined)?.props.toolbeltMode ?? "design";
    return mode === "design" && activeMode === "metronome" ? "metronome" : mode;
  };
  // The row's elements in order, leaving out slots with all their tools
  // hidden. Dividers and the like keep their places
  const arrangeSlots = (elements, mode) => {
    const hidden = hiddenOf(mode);
    const slots = elements.map(slotOf);
    const at = [];
    const ids = [];
    slots.forEach((slot, i) => {
      if (!slot) return;
      let id = slot.id;
      while (ids.includes(id)) id += "+";
      at.push(i);
      ids.push(id);
    });
    const out = [...elements];
    arrange(ids, orderOf(mode)).forEach((id, n) => {
      const from = at[ids.indexOf(id)];
      const element = elements[from];
      out[at[n]] = slots[from].tools.every((tool) => hidden.includes(tool)) ? false : React.cloneElement(element, { key: id });
    });
    return out;
  };
  // The div of the toolbar's elements, among Figma's children for a row
  const toolbarOf = (children) => [children].flat().find((child) => child?.type === "div" && Array.isArray(child.props?.children) && child.props.children.some(slotOf));
  const arrangeRow = (children, mode) => {
    if (!orderOf(mode).length && !hiddenOf(mode).length) return children;
    const toolbar = toolbarOf(children);
    const swap = (child) => (child === toolbar ? React.cloneElement(child, undefined, ...arrangeSlots(child.props.children, mode)) : child);
    return Array.isArray(children) ? children.map(swap) : swap(children);
  };
  // Marks a group left with one tool, which loses its arrow (see `restyle`)
  const SINGLE_ATTRIBUTE = "data-ligma-single-tool";

  // The mode the switch was last drawn in
  let drawnMode = null;
  let patched = false;
  const install = () => {
    patched = true;
    window.__ligmaModules.patch([
      {
        match: MODULE,
        getter: (Switch, react) => {
          if (typeof Switch !== "function" || !react || !String(Switch).includes("activeMode")) return Switch;
          React ??= react;
          const Modes = (props) => {
            react.useSyncExternalStore(subscribe, getVersion);
            shown = props;
            // Motion draws Design's toolbar: the rows draw again for its own
            activeMode = props.activeMode;
            react.useEffect(() => {
              if (props.activeMode === "metronome" || drawnMode === "metronome") redraw();
              drawnMode = props.activeMode;
            }, [props.activeMode]);
            react.useEffect(() => () => {
              if (shown === props) shown = null;
            });
            // Once, when the page reloaded for a hidden mode has drawn the
            // file: Figma doesn't take a switch before
            react.useEffect(() => {
              const left = leaving;
              leaving = [];
              if (!left.length) return;
              const ready = () => document.querySelector("canvas") && !document.querySelector('[class*="progress_bar--"]');
              const wait = () => (ready() ? leave(left) : setTimeout(wait, 500));
              wait();
            }, []);
            if (!Array.isArray(props.modes)) return react.createElement(Switch, props);
            const kept = props.modes.filter((item) => item.mode === props.activeMode || !state.hidden.includes(item.mode));
            const modes = arrange(
              kept.map((item) => item.mode),
              state.modeOrder,
            ).map((mode) => kept.find((item) => item.mode === mode));
            if (modes.every((item, i) => item === props.modes[i]) && modes.length === props.modes.length) return react.createElement(Switch, props);
            if (modes.length < 2) return null;
            return react.createElement(Switch, { ...props, modes });
          };
          Modes.displayName = "LigmaToolbarModes";
          return Modes;
        },
      },
      {
        match: QUICK_TOGGLE,
        loaded: (exports) => {
          const atoms = Object.keys(exports).map((key) => exports[key]);
          const modes = atoms.find((atom) => typeof atom?.read === "function" && String(atom.read).includes('"metronome"==='));
          if (!modes) return;
          // Its flags, the picker's open one among them, so it's worked out
          // again with the hidden modes as they are when it opens
          const flags = atoms.filter((atom) => atom !== modes && atom?.init === false);
          const read = modes.read;
          modes.read = function (get, options) {
            for (const flag of flags) get(flag);
            const figma = read.call(this, get, options);
            return Array.isArray(figma) ? figma.filter((mode, i) => i === 0 || !state.hidden.includes(mode)) : figma;
          };
        },
      },
      {
        match: ROW,
        getter: (Row, react) => {
          if (typeof Row !== "function" || !react || !String(Row).includes("leftSideRow") || !String(Row).includes("editorTheme")) return Row;
          React ??= react;
          Mode ??= react.createContext("design");
          const Arranged = (props) => {
            react.useSyncExternalStore(subscribe, getVersion);
            const toolbar = toolbarOf(props.children);
            const mode = toolbar ? modeOf(toolbar.props.children) : "design";
            return react.createElement(Mode.Provider, { value: mode }, react.createElement(Row, { ...props, children: arrangeRow(props.children, mode) }));
          };
          Arranged.displayName = "LigmaToolbarRow";
          return Arranged;
        },
      },
      {
        match: GROUP,
        getter: (Group, react) => {
          if (typeof Group !== "function" || !react || !String(Group).includes("items")) return Group;
          React ??= react;
          Mode ??= react.createContext("design");
          const Tools = (props) => {
            react.useSyncExternalStore(subscribe, getVersion);
            const hidden = hiddenOf(react.useContext(Mode));
            const all = Array.isArray(props.items) ? props.items : null;
            const items = all?.filter((item) => !hidden.includes(item.recordingKey));
            if (!items || items.length === all.length) return react.createElement(Group, props);
            if (!items.length) return null;
            const group = react.createElement(Group, { ...props, items });
            // Pencil keeps its arrow for the lasso (see lasso.rs)
            if (items.length > 1 || (items[0].recordingKey === "toolPencil" && window.__ligmaLassoOn?.())) return group;
            return [react.createElement("span", { key: "single", hidden: true, [SINGLE_ATTRIBUTE]: "" }), react.createElement(react.Fragment, { key: "group" }, group)];
          };
          Tools.displayName = "LigmaToolbarGroup";
          return Tools;
        },
      },
    ]);
  };

  // Figma's older switch, by the shortcut its tooltip names: to Design
  // when in Dev Mode, to Dev Mode otherwise. The divider before it goes
  // with it
  const TOGGLE = '[class*="mode_switcher_toggle--toggleWrapper"]';
  const TO_DESIGN = '[data-tooltip-shortcut-key="enter-design-mode"]';

  // Switches out of the mode the file is in if it's one of `hidden`, to
  // the first one shown. Figma doesn't always take a switch (Motion from
  // Design wants something to animate), so if the file is still in a
  // hidden mode a while later, to the one after it, and so on
  const leave = (hidden, tried = 0) => {
    if (shown) {
      const { modes, activeMode, onModeSwitch } = shown;
      if (!hidden.includes(activeMode) || !Array.isArray(modes) || typeof onModeSwitch !== "function") return;
      const kept = modes.filter((item) => !item.disabled && !state.hidden.includes(item.mode));
      const order = arrange(
        kept.map((item) => item.mode),
        state.modeOrder,
      );
      const next = order[tried];
      if (!next) return;
      onModeSwitch(next);
      setTimeout(() => leave(hidden, tried + 1), 3000);
      return;
    }
    if (tried) return;
    // The older switch has only the other mode to go to
    const toggle = document.querySelector(TOGGLE);
    if (!toggle) return;
    const inDev = toggle.matches(TO_DESIGN);
    const [from, to] = inDev ? ["handoff", "design"] : ["design", "handoff"];
    if (hidden.includes(from) && !state.hidden.includes(to)) toggle.querySelector('button, [role="switch"], input')?.click();
  };

  // Asks Ligma for something (see `on_signal`): through `window.open` on
  // Windows, and on macOS as an iframe load, which WKWebView reports to
  // `on_navigation`
  const signal = (path) => {
    const url = `https://toolbar.ligma.invalid${path}`;
    if (navigator.userAgent.includes("Mac")) {
      const frame = document.createElement("iframe");
      frame.style.display = "none";
      frame.src = url;
      document.documentElement.appendChild(frame);
      setTimeout(() => frame.remove(), 1000);
    } else {
      window.open(url, "_blank");
    }
  };

  // Right-clicking the switch asks Ligma for its menu
  const SWITCH = `[class*="toolbelt_mode_segmented_control--container"], [data-testid="toolbelt-mode-segmented-control"], ${TOGGLE}`;
  addEventListener(
    "contextmenu",
    (event) => {
      if (!event.target.closest?.(SWITCH)) return;
      event.preventDefault();
      event.stopPropagation();
      signal("/menu");
    },
    true,
  );
  const style = document.createElement("style");
  const restyle = () => {
    const toggles = [];
    if (state.hidden.includes("handoff")) toggles.push(`${TOGGLE}:not(${TO_DESIGN})`);
    if (state.hidden.includes("design")) toggles.push(`${TOGGLE}${TO_DESIGN}`);
    const hidden = toggles.join(", ");
    const drawn = hiddenOf("illustration")
      .filter((key) => DRAWING.includes(key))
      .map((key) => `[class*="drawing_tools--toolWrapper"]:has([data-tooltip="${CSS.escape(key)}"])`);
    style.textContent = [
      hidden ? `:is(${hidden}), [class*="mode_toggle_switch_divider--hideIfLast"]:has(+ :is(${hidden})) { display: none !important; }` : "",
      `[${SINGLE_ATTRIBUTE}] ~ * [data-testid$="-chevron"] { display: none !important; }`,
      drawn.length ? `:is(${drawn.join(", ")}) { display: none !important; }` : "",
    ].join("\n");
  };
  restyle();
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  // The layout as Figma has it, for Settings to show: read from the props
  // Figma hands its components, above Ligma's wrappers, so with nothing
  // left out or moved
  const fiberOf = (node) => {
    const key = node && Object.keys(node).find((name) => name.startsWith("__reactFiber$"));
    return key ? node[key] : null;
  };
  // The outermost of the first run of components whose props `has`
  const outermost = (fiber, has) => {
    let found = null;
    for (; fiber; fiber = fiber.return) {
      const props = fiber.memoizedProps;
      if (props && typeof props === "object" && has(props)) found = props;
      else if (found) break;
    }
    return found;
  };
  const text = (get) => {
    try {
      return String(get() ?? "");
    } catch {
      return "";
    }
  };
  const fromConfig = (config) => ({
    key: config.recordingKey,
    name: config.text ?? text(() => config.getText?.(false)),
    icon: config.icon ?? (() => config.getIcon?.({ numUnreadComments: 0, isActive: false })),
  });
  // The tools of the row's element `element`, or null if they can't be
  // read
  const toolsOf = (element, row) => {
    const props = element.props;
    if (Array.isArray(props.tools)) return props.tools.map(fromConfig);
    if (props.config) return [fromConfig(props.config)];
    if (props.toolbeltMode !== undefined) {
      const chevron = row.querySelector('[data-testid="SelectTools-chevron"]');
      const group = outermost(fiberOf(chevron), (p) => Array.isArray(p.items) && "toolGroupId" in p);
      if (group) return group.items.map(fromConfig);
      // Dev Mode's Move is a tool of its own, the row's first
      const single = outermost(fiberOf(row.querySelector("button")), (p) => p.config);
      return single?.config.recordingKey === "toolDefault" ? [fromConfig(single.config)] : null;
    }
    // Draw's drawing tools are pictures: a tool, and over it its color
    if (drawing(element)) {
      const tools = DRAWING.map((key) => {
        const wrapper = row.querySelector(`[class*="drawing_tools--toolWrapper"]:has([data-tooltip="${key}"])`);
        const images = [...(wrapper?.querySelectorAll("img") ?? [])].map((img) => img.src).filter((src) => src.startsWith("https://static.figma.com/"));
        return wrapper && { key, name: key.replace("set-tool-", "").replace(/^./, (c) => c.toUpperCase()), images };
      });
      return tools.every(Boolean) ? tools : null;
    }
    if (props.toolId === "QUICK_ACTIONS") return [{ key: "actions", name: String(props.tooltipText ?? "Actions"), icon: props.icon }];
    // Comment's tooltip counts unread comments
    const name = String(props.tooltipText ?? "");
    return [{ key: "toolComment", name: /\d/.test(name) ? "Comment" : name, icon: props.icon }];
  };
  // Each toolbar's slots, by the mode it's for: Design's (which Motion
  // shows too), Draw's and Dev Mode's, those that can be read
  const readToolbars = () => {
    const toolbars = {};
    const rows = [...document.querySelectorAll('[class*="toolbelt_left_side--leftSideRow"]')].map((row) => {
      const props = outermost(fiberOf(row), (p) => "editorTheme" in p && "children" in p);
      return [row, toolbarOf(props?.children)?.props.children ?? []];
    });
    // The toolbar on screen first, for Comment's tool id
    for (const [, elements] of rows) elements.forEach(slotOf);
    rows: for (const [row, elements] of rows) {
      const mode = elements.find((element) => element?.props?.toolbeltMode !== undefined)?.props.toolbeltMode;
      if (!mode || toolbars[mode]) continue;
      const slots = [];
      for (const element of elements) {
        const slot = slotOf(element);
        if (!slot) continue;
        const tools = toolsOf(element, row);
        if (!tools?.length) continue rows;
        let id = slot.id;
        while (slots.some((other) => other.id === id)) id += "+";
        slots.push({ id, tools });
      }
      if (slots.length) toolbars[mode] = slots;
    }
    return toolbars;
  };
  const readModes = () => {
    const control = document.querySelector('[data-testid="toolbelt-mode-segmented-control"], [class*="toolbelt_mode_segmented_control--container"]');
    const props = outermost(fiberOf(control), (p) => Array.isArray(p.modes) && "activeMode" in p);
    return props?.modes.map((item) => ({ mode: item.mode, name: String(item.label ?? item.mode), icon: item.icon })) ?? null;
  };

  // React, for drawing icons, from Figma's own modules if no wrapper has
  // been handed it
  const findReact = () => {
    if (React) return React;
    let require = null;
    try {
      window.rspackChunk_figma_web_bundler.push([[Symbol("ligma")], {}, (r) => (require = r)]);
    } catch {}
    for (const id in require?.m ?? {}) {
      const source = Function.prototype.toString.call(require.m[id]);
      if (source.length > 40000 || !source.includes("useSyncExternalStore") || !source.includes("forwardRef") || !source.includes("createElement")) continue;
      try {
        const exports = require(id);
        if (exports?.useSyncExternalStore && exports.createElement) return exports;
      } catch {}
    }
    return null;
  };
  // An icon's markup: its components called with hooks that hand back
  // their first values, as icons only read context and memoize
  const KEEP_CASE = new Set(["viewBox", "preserveAspectRatio", "gradientUnits", "gradientTransform", "patternUnits", "maskUnits", "clipPathUnits", "stdDeviation"]);
  const escape = (value) => String(value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
  const markup = (react) => {
    const internals = react.__SECRET_INTERNALS_DO_NOT_USE_OR_YOU_WILL_BE_FIRED?.ReactCurrentDispatcher;
    const client = react.__CLIENT_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE;
    const hooks = {
      readContext: (context) => context._currentValue,
      use: (value) => value?._currentValue ?? value,
      useContext: (context) => context._currentValue,
      useState: (initial) => [typeof initial === "function" ? initial() : initial, () => {}],
      useReducer: (reducer, initial, init) => [init ? init(initial) : initial, () => {}],
      useMemo: (make) => make(),
      useCallback: (callback) => callback,
      useRef: (current) => ({ current }),
      useEffect() {},
      useLayoutEffect() {},
      useInsertionEffect() {},
      useImperativeHandle() {},
      useDebugValue() {},
      useId: () => "ligma",
      useSyncExternalStore: (_, get) => get(),
      useTransition: () => [false, (run) => run()],
      useDeferredValue: (value) => value,
    };
    const draw = (node, depth) => {
      if (node == null || typeof node === "boolean") return "";
      if (typeof node === "string" || typeof node === "number") return escape(node);
      if (Array.isArray(node)) return node.map((child) => draw(child, depth)).join("");
      let type = node.type;
      while (type && typeof type === "object" && (type.type || type.render)) type = type.type ?? type.render;
      const props = node.props ?? {};
      if (typeof type === "function") {
        if (depth > 8) return "";
        if (internals) {
          const before = internals.current;
          internals.current = hooks;
          try {
            return draw(type(props, null), depth + 1);
          } finally {
            internals.current = before;
          }
        }
        if (client) {
          const before = client.H;
          client.H = hooks;
          try {
            return draw(type(props, null), depth + 1);
          } finally {
            client.H = before;
          }
        }
        return "";
      }
      if (typeof type !== "string") return draw(props.children, depth);
      const attributes = Object.entries(props)
        .filter(([name, value]) => name !== "children" && (typeof value === "string" || typeof value === "number"))
        .map(([name, value]) => ` ${name === "className" ? "class" : KEEP_CASE.has(name) ? name : name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}="${escape(value)}"`)
        .join("");
      return `<${type}${attributes}>${draw(props.children, depth)}</${type}>`;
    };
    return (icon) => {
      try {
        return draw(typeof icon === "function" ? icon() : icon, 0);
      } catch {
        return "";
      }
    };
  };

  // Tells Ligma the toolbars that differ from what Ligma has, and the modes
  // when the switch is on screen to read them from
  const report = () => {
    const toolbars = readToolbars();
    if (!toolbars.design) return false;
    const modes = readModes();
    const sigs = Object.fromEntries(
      Object.entries(toolbars).map(([mode, slots]) => [mode, JSON.stringify(slots.map((slot) => [slot.id, slot.tools.map((tool) => [tool.key, tool.name, tool.images])]))]),
    );
    const changed = Object.keys(sigs).filter((mode) => sigs[mode] !== known.tools[mode]);
    const modesSig = modes ? JSON.stringify(modes.map((item) => [item.mode, item.name])) : known.modes;
    if (!changed.length && modesSig === known.modes) return true;
    const react = findReact();
    if (!react) return true;
    const icon = markup(react);
    const layout = { sigs: {}, toolbars: {} };
    for (const mode of changed) {
      layout.sigs[mode] = sigs[mode];
      layout.toolbars[mode] = toolbars[mode].map((slot) => ({
        id: slot.id,
        tools: slot.tools.map((tool) => (tool.images ? { key: tool.key, name: tool.name, images: tool.images } : { key: tool.key, name: tool.name, icon: icon(tool.icon) })),
      }));
    }
    if (modes && modesSig !== known.modes) {
      layout.modesSig = modesSig;
      layout.modes = modes.map((item) => ({ mode: item.mode, name: item.name, icon: icon(item.icon) }));
    }
    known = { tools: { ...known.tools, ...sigs }, modes: modesSig };
    signal(`/layout?data=${encodeURIComponent(JSON.stringify(layout))}`);
    return true;
  };
  // Once the toolbar's drawn, and each time the tab's shown again
  let tries = 0;
  const reportSoon = () => {
    if (report() || ++tries > 30) return;
    setTimeout(reportSoon, 2000);
  };
  setTimeout(reportSoon, 2000);

  // Changed the layout after the toolbar loaded: reload, once the tab is
  // shown and Figma has no edits still saving
  let reloading = false;
  const reloadWhenReady = () => {
    if (reloading || !custom()) return;
    if (document.visibilityState !== "visible") return;
    if (window.__ligmaCanRest && !window.__ligmaCanRest()) {
      setTimeout(reloadWhenReady, 1000);
      return;
    }
    reloading = true;
    try {
      sessionStorage.setItem(LEAVE_KEY, JSON.stringify(leaving));
    } catch {}
    location.reload();
  };
  // Too late for a module, once it has loaded without being wrapped
  const late = () => [MODULE, ROW, GROUP].some((match) => window.__ligmaModules.arrived(match));

  const apply = (next) => {
    next = normal(next);
    const was = state;
    const changed = ["hidden", "modeOrder", "layouts"].some((key) => JSON.stringify(next[key]) !== JSON.stringify(was[key]));
    const newly = next.hidden.filter((mode) => !was.hidden.includes(mode));
    state = next;
    if (!changed) return;
    restyle();
    redraw();
    if (newly.length) {
      if (patched || !window.__ligmaModules.arrived(MODULE)) leave(newly);
      else leaving = [...new Set([...leaving, ...newly])].filter((mode) => state.hidden.includes(mode));
    }
    if (!custom() || patched) return;
    if (late()) reloadWhenReady();
    else install();
  };
  // Draws the toolbar again, for the lasso's setting (see lasso.rs)
  window.__ligmaToolbarRedraw = redraw;
  // From Ligma, when the setting changes
  window.__ligmaToolbarModes = (next) => {
    if (next.known) known = { tools: {}, ...next.known };
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    apply(next);
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) apply(stored);
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") return;
    sync();
    if (custom() && !patched && late()) reloadWhenReady();
    report();
  });
  if (custom()) install();
})()"##;

/// The layout set in Settings, as `PAGE_JS` takes it: the modes hidden and
/// their order, by Figma's names for them, and each mode's toolbar
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Layout {
    hidden: Vec<&'static str>,
    mode_order: Vec<String>,
    layouts: BTreeMap<String, ToolbarLayout>,
}

static LAYOUT: Mutex<Option<Layout>> = Mutex::new(None);
/// and when it last changed, for `PAGE_JS` to tell it from older ones in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

/// The toolbars as files' pages last reported them (see `report` in
/// `PAGE_JS`), kept in `toolbar.json`: each mode's slots, and the modes,
/// with the signatures pages tell them apart by
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Reported {
    sigs: BTreeMap<String, String>,
    toolbars: BTreeMap<String, serde_json::Value>,
    modes_sig: Option<String>,
    modes: serde_json::Value,
}

static REPORTED: Mutex<Option<Reported>> = Mutex::new(None);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn layout(settings: &Settings) -> Layout {
    Layout {
        hidden: [
            (settings.show_draw_mode, "illustration"),
            (settings.show_design_mode, "design"),
            (settings.show_motion_mode, "metronome"),
            (settings.show_dev_mode, "handoff"),
        ]
        .into_iter()
        .filter_map(|(shown, mode)| (!shown).then_some(mode))
        .collect(),
        mode_order: settings.mode_order.clone(),
        layouts: settings.toolbar_layouts.clone(),
    }
}

fn state() -> String {
    let mut state = serde_json::to_value(LAYOUT.lock().unwrap().clone().unwrap_or_default()).unwrap();
    state["rev"] = REV.load(Ordering::Relaxed).into();
    let reported = REPORTED.lock().unwrap();
    state["known"] = serde_json::json!({
        "tools": reported.as_ref().map(|reported| reported.sigs.clone()).unwrap_or_default(),
        "modes": reported.as_ref().and_then(|reported| reported.modes_sig.clone()),
    });
    state.to_string()
}

fn reported_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("toolbar.json"))
}

/// Call at launch, before any tab
pub fn init(app: &AppHandle, settings: &Settings) {
    *LAYOUT.lock().unwrap() = Some(layout(settings));
    REV.store(now(), Ordering::Relaxed);
    let reported = reported_path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Reported>(&text).ok());
    *REPORTED.lock().unwrap() = reported;
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_TOOLBAR_MODES__", &state())
}

/// Origin of the links `PAGE_JS` opens to ask for the menu and to report
/// the layout
pub const SIGNAL_HOST: &str = "toolbar.ligma.invalid";

/// The menu's items, in Figma's order: their actions, text and Figma's
/// names for them
const MODES: [(&str, &str, &str); 4] = [
    ("draw", "Draw", "illustration"),
    ("design", "Design", "design"),
    ("motion", "Motion", "metronome"),
    ("dev", "Dev Mode", "handoff"),
];

/// The setting that shows mode `action` (see `MODES`)
fn shows<'a>(settings: &'a mut Settings, action: &str) -> Option<&'a mut bool> {
    match action {
        "draw" => Some(&mut settings.show_draw_mode),
        "design" => Some(&mut settings.show_design_mode),
        "motion" => Some(&mut settings.show_motion_mode),
        "dev" => Some(&mut settings.show_dev_mode),
        _ => None,
    }
}

/// A right-click on the switch in tab `tab`: opens the menu of modes, in
/// the switch's order. Or a page's report of the layout. Not from within
/// the webview's own callback, which is still deciding on the page
pub fn on_signal(app: &AppHandle, tab: u32, url: &Url) {
    match url.path() {
        "/menu" => {
            let app = app.clone();
            std::thread::spawn(move || {
                let mut settings = crate::settings::current(&app);
                let mut modes = MODES.to_vec();
                let rank = |name: &str| settings.mode_order.iter().position(|mode| mode == name).unwrap_or(usize::MAX);
                if !settings.mode_order.is_empty() {
                    modes.sort_by_key(|&(_, _, name)| rank(name));
                }
                let items = modes.iter().map(|&(action, text, _)| Some((action, text))).collect();
                let checked = modes
                    .iter()
                    .map(|&(action, _, _)| action)
                    .filter(|action| shows(&mut settings, action).is_some_and(|shown| *shown))
                    .collect();
                let _ = crate::tab_menu::show(&app, tab, items, crate::tab_menu::Extras { checked: Some(checked), ..Default::default() }, on_pick);
            });
        }
        "/layout" => {
            let Some((_, data)) = url.query_pairs().find(|(name, _)| name == "data") else {
                return;
            };
            if data.len() > 1 << 20 {
                return;
            }
            let Ok(layout) = serde_json::from_str::<Reported>(&data) else {
                return;
            };
            if layout.toolbars.values().any(|slots| !slots.is_array()) {
                return;
            }
            // Pages report the toolbars that changed, and the modes only
            // with the switch on screen
            let merged = {
                let mut reported = REPORTED.lock().unwrap();
                let mut merged = reported.clone().unwrap_or_default();
                merged.sigs.extend(layout.sigs);
                merged.toolbars.extend(layout.toolbars);
                if layout.modes_sig.is_some() {
                    merged.modes_sig = layout.modes_sig;
                    merged.modes = layout.modes;
                }
                *reported = Some(merged.clone());
                merged
            };
            if let Some(path) = reported_path(app) {
                if let Err(error) = std::fs::write(path, serde_json::to_string(&merged).unwrap()) {
                    log::error!("Couldn't save the toolbar's layout: {error}");
                }
            }
            if let Some(window) = app.get_webview_window(crate::settings::WINDOW) {
                let _ = window.eval(format!("window.applyToolbarLayout?.({})", serde_json::to_string(&merged).unwrap()));
            }
        }
        _ => {}
    }
}

/// Turns the mode picked in the menu on or off
fn on_pick(app: &AppHandle, _tab: u32, action: &str) {
    let mut settings = crate::settings::current(app);
    let Some(shown) = shows(&mut settings, action) else {
        return;
    };
    *shown = !*shown;
    if let Err(error) = crate::settings::change(app, settings.clone()) {
        log::error!("Couldn't save settings: {error}");
        return;
    }
    crate::settings::show_in_window(app, &settings);
}

/// The toolbar as last reported, for the settings window to draw
#[tauri::command]
pub async fn get_toolbar_layout(webview: Webview) -> Option<serde_json::Value> {
    if !crate::settings::from_settings_window(&webview) {
        return None;
    }
    REPORTED.lock().unwrap().as_ref().and_then(|reported| serde_json::to_value(reported).ok())
}

/// Sends changed settings to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    let layout = layout(settings);
    if std::mem::replace(&mut *LAYOUT.lock().unwrap(), Some(layout.clone())) == Some(layout) {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaToolbarModes?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
