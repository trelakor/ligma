// Feature demos for what's new: short looping scenes of a release's main
// features, drawn in HTML rather than recorded, so they follow the theme
// and weigh a few KB. Each is a crop of the real interface at its real
// size: the tab bar (index.html), tab menu (menu.html), emoji picker
// (emoji.html), Settings, Ligma's home page, and Figma's editor with
// Ligma's additions to it. The what's new prompt (prompt.html) shows the
// demos of every release it lists, Settings > What's New each release's
// own. Small changes and fixes get none
//
// A scene is drawn at its `size`, 480 × 267 unless it says, and scaled to
// fit. Its steps play in order and loop: each adds `at-<step>` to the
// scene, so CSS for a step holds from then on, moves the pointer to `at`
// over `move` ms and clicks with `click`. Under it, the feature's name and
// what's happening

(() => {
  const SIZE = [480, 267];
  const EASE = "cubic-bezier(0.45, 0, 0.2, 1)";

  // Demos per release, in the order of its notes
  const RELEASES = {
    "0.44.0": ["color-frames", "settings-search"],
    "0.43.0": ["heal", "upscale"],
    "0.42.0": ["account-settings"],
    "0.41.0": ["themed-loading"],
    "0.40.0": ["home-page", "tab-emoji"],
    "0.38.0": ["ligma-mcp"],
    "0.37.0": ["lasso-fill"],
    "0.34.0": ["app-icons"],
    "0.30.0": ["split-view"],
    "0.28.0": ["select-object"],
    "0.27.0": ["vectorize"],
    "0.25.0": ["remove-background"],
    "0.22.0": ["tab-previews"],
    "0.19.0": ["plugins-rail"],
    "0.17.0": ["themes"],
    "0.3.0": ["discord"],
  };

  const svg = (body, size = 16, extra = "") =>
    `<svg width="${size}" height="${size}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"${extra}>${body}</svg>`;
  const glyph = (path) => `<svg width="16" height="16" viewBox="0 0 16 16"><path fill="currentColor" ${path}/></svg>`;

  const ICON = {
    // The tab bar's (index.html)
    home: svg('<path d="M2.5 7 8 2.5 13.5 7v6.5h-4v-4h-3v4h-4z"/>'),
    design: glyph('fill-rule="evenodd" clip-rule="evenodd" d="M4.14 3.207a1 1 0 0 1 .837-.181l3.762.878a4.57 4.57 0 0 1 3.531 4.452l.437.436a1 1 0 0 1 0 1.415l-2.5 2.5a1 1 0 0 1-1.414 0l-.437-.437A4.57 4.57 0 0 1 3.96 8.949l-.054-.21-.879-3.762a1 1 0 0 1 .267-.935l.75-.75zm.59.813 2.623 2.624-.006.006a1.5 1.5 0 1 1-.697.697l-.004.004-2.624-2.624L4 4.749l.879 3.763a3.573 3.573 0 0 0 3.87 2.737l.75.75L12 9.5l-.75-.75a3.57 3.57 0 0 0-2.737-3.872L4.75 4zM8 7.5a.5.5 0 1 0 0 1 .5.5 0 0 0 0-1"'),
    board: glyph('d="M12 3a1 1 0 0 1 1 1v4.586l-.005.099a1 1 0 0 1-.288.608l-3.414 3.414-.073.066a1 1 0 0 1-.634.227H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1zm-8 9h4V9a1 1 0 0 1 1-1h3V4H4zm5-.414L11.586 9H9z"'),
    close: svg('<path d="m5 5 6 6M11 5l-6 6"/>', 12),
    plus: svg('<path d="M8 3.5v9M3.5 8h9"/>'),
    // Figma's layers panel
    frame: svg('<path d="M5.5 3v10M10.5 3v10M3 5.5h10M3 10.5h10"/>', 16, ' stroke-width="1"'),
    text: svg('<path d="M4 4h8M8 4v8.5"/>', 16, ' stroke-width="1"'),
    image: svg('<rect x="3" y="3.5" width="10" height="9" rx="1"/><path d="m3.5 11 3-3 2 2 1.5-1.5 2.5 2.5"/>', 16, ' stroke-width="1"'),
    rect: svg('<rect x="3.5" y="3.5" width="9" height="9" rx="1"/>', 16, ' stroke-width="1"'),
    caretDown: svg('<path d="m6 7 2 2 2-2"/>', 16, ' stroke-width="1"'),
    caretRight: svg('<path d="m7 6 2 2-2 2"/>', 16, ' stroke-width="1"'),
    // Settings (settings.html)
    search: svg('<circle cx="7" cy="7" r="4"/><path d="m10 10 3 3"/>'),
    general: svg('<path d="M2.5 5.5h2M7.5 5.5h6M2.5 10.5h6M11.5 10.5h2"/><circle cx="6" cy="5.5" r="1.5"/><circle cx="10" cy="10.5" r="1.5"/>'),
    appearance: svg('<circle cx="8" cy="8" r="5.5"/><path d="M8 2.5a5.5 5.5 0 0 0 0 11z" fill="currentColor" stroke="none"/>'),
    appIcon: svg('<path d="M8 2.5c3.9 0 5.5.6 5.5 5.5S11.9 13.5 8 13.5 2.5 11.9 2.5 8 4.1 2.5 8 2.5z"/><path d="M6.5 5.2v5.3"/><circle cx="9.6" cy="9.6" r="1.1" fill="currentColor" stroke="none"/>'),
    tabs: svg('<path d="M2.5 13V5.5a1 1 0 0 1 1-1h3l1 1.5h5a1 1 0 0 1 1 1V13z"/><path d="M2.5 13h11"/>'),
    editor: svg('<path d="M8 2.5 12 7l-1.5 5h-5L4 7z"/><path d="M8 2.5V7"/><circle cx="8" cy="8" r="1"/><path d="M5.5 13.5h5"/>'),
    images: svg('<rect x="2.5" y="3" width="11" height="10" rx="1.5"/><circle cx="6" cy="6.5" r="1"/><path d="m2.5 11.5 3.5-3 3 2.5 1.5-1.5 3 2.5"/>'),
    plugins: svg('<path d="M3 5.5h2.2a1.3 1.3 0 1 1 2.6 0H10v2.2a1.3 1.3 0 1 1 0 2.6v2.2H7.8a1.3 1.3 0 1 0-2.6 0H3z"/>'),
    ai: svg('<path d="M8 2.5l1.4 3.6 3.6 1.4-3.6 1.4L8 12.5 6.6 8.9 3 7.5l3.6-1.4z"/>'),
    discord: svg('<path d="M2.5 4.5a1.5 1.5 0 0 1 1.5-1.5h8a1.5 1.5 0 0 1 1.5 1.5v5.5a1.5 1.5 0 0 1-1.5 1.5H7.5L4.5 14v-2.5a2 2 0 0 1-2-2z"/>'),
    chevron: svg('<path d="m4 6 4 4 4-4"/>', 12),
    // Ligma's home page (home.html)
    clock: svg('<circle cx="8" cy="8" r="5.5"/><path d="M8 5v3l2 1.5"/>'),
    star: svg('<path d="m8 2.5 1.7 3.5 3.8.5-2.8 2.6.7 3.8L8 11l-3.4 1.9.7-3.8-2.8-2.6 3.8-.5z"/>'),
    people: svg('<circle cx="6" cy="6" r="2.2"/><path d="M2.5 13c.4-2 1.8-3 3.5-3s3.1 1 3.5 3"/><path d="M10.5 4.2a2 2 0 0 1 0 3.6M12 10.3c.9.4 1.4 1.3 1.6 2.7"/>'),
    file: svg('<path d="M4 2.5h5.5L12 5v8.5H4z"/><path d="M9.5 2.5V5H12"/>'),
    bell: svg('<path d="M4.5 11V7.5a3.5 3.5 0 0 1 7 0V11l1 1.5h-9z"/><path d="M6.8 14h2.4"/>'),
    account: svg('<circle cx="6" cy="6" r="2.2"/><path d="M2.5 13c.4-2 1.8-3 3.5-3s3.1 1 3.5 3"/><path d="M10.5 4.2a2 2 0 0 1 0 3.6M12 10.3c.9.4 1.4 1.3 1.6 2.7"/>'),
    community: svg('<path d="m8 2.5 5 2.8v5.4L8 13.5l-5-2.8V5.3z"/><path d="M3 5.3 8 8l5-2.7M8 8v5.5"/>'),
    lock: svg('<rect x="3.5" y="7" width="9" height="6.5" rx="1"/><path d="M5.5 7V5.5a2.5 2.5 0 0 1 5 0V7"/>'),
    external: svg('<path d="M6.5 3.5h-3v9h9v-3M9 3.5h3.5V7M12.5 3.5 7.5 8.5"/>', 12),
    right: svg('<path d="m6 4 4 4-4 4"/>', 12),
    // The image bar
    crop: svg('<path d="M5 2.5V11h8.5M2.5 5H11v8.5"/>'),
    heal: svg('<rect x="2" y="5.5" width="12" height="5" rx="2.5" transform="rotate(-45 8 8)"/><path d="M6.6 6.6l2.8 2.8" stroke-width="2.6" stroke-linecap="butt"/>'),
    upscale: svg('<path d="M3 13V9M3 13h4M3 13l4-4M13 3v4M13 3H9M13 3 9 7"/>'),
    cutout: svg('<circle cx="8" cy="6" r="2.2"/><path d="M4.5 13c.5-2.2 1.8-3.3 3.5-3.3s3 1.1 3.5 3.3"/><path d="M2.5 4.5v-2h2M11.5 2.5h2v2M13.5 11.5v2h-2M4.5 13.5h-2v-2"/>'),
    check: svg('<path d="m4 8.5 2.5 2.5L12 5.5"/>'),
    arrows: svg('<path d="m5.5 5-3 3 3 3M10.5 5l3 3-3 3"/>', 16, ' stroke-width="1.5"'),
  };

  // Settings' clear button in its search box
  const CLEAR = svg('<path d="m3.5 3.5 9 9M12.5 3.5l-9 9"/>', 10, ' stroke-width="1.6"');

  // Ligma's logo on Figma's loading screen (loading_logo.rs), in boxes
  const LOGO = '<div class="logo"><div class="top"></div><div class="middle"></div><div class="row"><div class="cap"></div><div class="span"></div><div class="rest"></div><div class="end"></div><div class="divide"><div class="boundary"><div class="line"></div><div class="arc"></div></div></div></div></div>';

  // The files the scenes show, drawn small: the sample files of Ligma's
  // home page. Each is drawn at 400 × 225 and scaled into `w` × `h`
  const art = (kind, w, h) => `<div class="art" style="width:${w}px;height:${h}px"><div class="${kind}" style="transform:scale(${w / 400})">${ART[kind]}</div></div>`;
  const ART = {
    landing: `<i class="lg"></i><b class="brand">Ligma</b><span class="links">Product<span>Pricing</span><span>Docs</span></span><em class="cta">Sign up</em>
      <h4>Ship the<br>next thing</h4><p>A short line on what it does and who for</p><em class="go">Get started</em>
      <i class="c1"></i><i class="c2"></i><i class="c3"></i>`,
    icons: ["home", "search", "clock", "star", "file", "tabs", "bell", "design", "people", "general", "lock", "images", "discord", "appearance", "community", "plugins", "plus", "editor", "check", "upscale", "crop", "right", "external", "ai"]
      .map((name) => `<span>${ICON[name].replace(/width="1[26]" height="1[26]"/, 'width="22" height="22"')}</span>`).join(""),
    bank: ["Mobile", "Activity", "Profile"].map((title, i) => `<div class="phone"><b>${title}</b>${[["Coffee", "−4.20"], ["Salary", "+3,100"], ["Rent", "−1,250"], ["Books", "−18.00"]].map(([what, amount], j) => `<span><i style="--h:${(i * 90 + j * 70) % 360}"></i>${what}<em>${amount}</em></span>`).join("")}<em class="pay">Continue</em></div>`).join(""),
    board: `<i class="sticky" style="--s:#ffe58f;left:40px;top:60px">Faster home</i><i class="sticky" style="--s:#ffc6c6;left:150px;top:44px">Mac sign-in</i><i class="sticky" style="--s:#d9c8ff;left:260px;top:64px">Ship 1.0</i><b class="tag">Q4 roadmap</b>`,
  };

  // A tab bar as index.html draws it: Home as wide as Figma's rail, then
  // each tab's icon, title and close button
  const tabBar = (tabs) =>
    `<div class="tb"><span class="t home${tabs.home ? " on" : ""}">${ICON.home}</span>${tabs.list
      .map((tab) => `<span class="t ${tab.cls ?? ""}" style="--accent:${tab.kind === "board" ? "#9747ff" : "#0d99ff"}">${tab.icon ?? ICON[tab.kind]}<span class="title">${tab.title}</span><span class="x">${ICON.close}</span></span>`)
      .join("")}<span class="t new">${ICON.plus}</span></div>`;

  // Figma's editor: the layers panel beside the canvas, with one frame
  const editor = (rows, canvas) => `
    <div class="ed">
      <div class="lp"><div class="lh">Layers</div>${rows}</div>
      <div class="cv">${canvas}</div>
    </div>`;
  const layer = (icon, name, level = 0, caret = null, cls = "") =>
    `<div class="lr ${cls}" style="--level:${level}"><span class="caret">${caret ? ICON[caret] : ""}</span><span class="li">${ICON[icon]}</span><span class="ln">${name}</span><i class="guide"></i></div>`;

  // Figma's image bar for a selected image: Crop, Ligma's Heal and
  // Upscale, Figma's own Remove background
  const imageBar = `<div class="bar image"><span class="b icon">${ICON.crop}</span><span class="div"></span><span class="b heal-b" style="width:56px">${ICON.heal}Heal</span><span class="b up-b" style="width:74px">${ICON.upscale}Upscale</span><span class="div"></span><span class="b" style="width:134px">${ICON.cutout}Remove background</span></div>`;

  // A selected image on the canvas, with its size under it
  const selected = (inner, size) => `<div class="pic">${inner}<i class="sel"><b></b><b></b><b></b><b></b></i><span class="dims">${size}</span></div>`;

  Object.assign(ICON, {
    // Figma's toolbar
    move: svg('<path d="M4 2.5v10l3-2.8 1.8 4 1.6-.7-1.8-3.9h4z"/>'),
    frameTool: svg('<path d="M5.5 2.5v11M10.5 2.5v11M2.5 5.5h11M2.5 10.5h11"/>'),
    rectTool: svg('<rect x="3" y="3" width="10" height="10" rx="1"/>'),
    pen: svg('<path d="M8 2.5 12 7l-1.5 5h-5L4 7z"/><path d="M8 2.5V7"/><circle cx="8" cy="8" r="1"/>'),
    pencil: svg('<path d="m10.5 2.5 3 3L6 13l-3.5.5L3 10z"/><path d="m9 4 3 3"/>'),
    lasso: svg('<ellipse cx="8.5" cy="6.5" rx="5.5" ry="3.5" stroke-dasharray="2 1.6"/><path d="M5 9.5c-1.2 1-1.4 2.6-.2 3.4"/>'),
    textTool: svg('<path d="M3.5 3.5h9M8 3.5v9"/>'),
    comment: svg('<path d="M3 12.5V8a5 5 0 1 1 5 5z"/>'),
    // Figma's rail and panels
    layers: svg('<path d="m8 2.5 5.5 3L8 8.5l-5.5-3z"/><path d="m2.5 8.5 5.5 3 5.5-3M2.5 11 8 14l5.5-3"/>'),
    assets: svg('<path d="M3 3.5h4v4H3zM9 3.5h4v4H9zM3 9.5h4v4H3zM9 9.5h4v4H9z"/>'),
    find: svg('<circle cx="7" cy="7" r="4"/><path d="m10 10 3 3"/>'),
    gear: svg('<circle cx="8" cy="8" r="2"/><path d="M8 2v1.5M8 12.5V14M2 8h1.5M12.5 8H14M3.8 3.8l1 1M11.2 11.2l1 1M3.8 12.2l1-1M11.2 4.8l1-1"/>'),
    spinner: svg('<circle cx="8" cy="8" r="5" opacity=".3"/><path d="M8 3a5 5 0 0 1 5 5"/>'),
    done: svg('<path d="m4 8.5 2.5 2.5L12 5.5"/>'),
    target: svg('<path d="M2.5 5.5v-3h3M10.5 2.5h3v3M13.5 10.5v3h-3M5.5 13.5h-3v-3"/><circle cx="8" cy="8" r="1.5"/>'),
    // The image bar's other actions
    vectorize: svg('<path d="M3 13 6.5 4 13 8.5 8 13z"/><circle cx="3" cy="13" r="1.2" fill="currentColor"/><circle cx="6.5" cy="4" r="1.2" fill="currentColor"/><circle cx="13" cy="8.5" r="1.2" fill="currentColor"/>'),
    pick: svg('<path d="M2.5 5.5v-3h3M10.5 2.5h3v3M2.5 10.5v3h3" stroke-dasharray="1.6 1.4"/><path d="m8 8 5.5 2-2.3 1 -1 2.5z"/>'),
    isolate: svg('<circle cx="8" cy="8" r="5.5" stroke-dasharray="2 1.6"/><circle cx="8" cy="8" r="2.2" fill="currentColor" stroke="none"/>'),
    erase: svg('<path d="m6 13.5-3.5-3.5 7-7 4 4-6.5 6.5z"/><path d="M6 13.5h7.5M5 7.5l4 4"/>'),
    brush: svg('<path d="M13 3 7.5 9.5M6.5 9a2 2 0 0 0-2 2c0 1.2-.8 2-2 2 2.5 1 5.5 0 5.5-2.5z"/>'),
  });

  // A potted plant against a wall, as a photo of it would be, for the
  // image demos that find what's in an image. The plant is `#subject`, so
  // an outline can be drawn around it
  const PLANT = `<svg class="plant" viewBox="0 0 260 146" width="260" height="146" preserveAspectRatio="xMidYMid slice">
    <defs>
      <linearGradient id="wall" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#efe1cf"/><stop offset="1" stop-color="#dcc6ab"/></linearGradient>
      <radialGradient id="window" cx=".72" cy=".18" r=".7"><stop offset="0" stop-color="#fff8ec" stop-opacity=".95"/><stop offset="1" stop-color="#fff8ec" stop-opacity="0"/></radialGradient>
      <linearGradient id="pot" x1="0" x2="1"><stop offset="0" stop-color="#b9583a"/><stop offset=".5" stop-color="#e3906a"/><stop offset="1" stop-color="#a94d31"/></linearGradient>
      <linearGradient id="leaf" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#7cc47e"/><stop offset="1" stop-color="#2e7346"/></linearGradient>
      <linearGradient id="leaf2" x1="1" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#6ab872"/><stop offset="1" stop-color="#245f3a"/></linearGradient>
    </defs>
    <g class="backdrop">
      <rect width="260" height="146" fill="url(#wall)"/>
      <rect width="260" height="146" fill="url(#window)"/>
      <rect y="116" width="260" height="30" fill="#cfb796"/>
      <rect y="116" width="260" height="1.5" fill="#bba183"/>
      <ellipse cx="134" cy="126" rx="46" ry="5" fill="#3b2a1a" opacity=".22"/>
      <path d="M200 116V60M206 116V70M212 116V64" stroke="#c9b293" stroke-width="2"/>
    </g>
    <g id="subject">
      <path d="M129 72C118 52 98 36 78 32c8 18 26 34 48 42z" fill="url(#leaf)"/>
      <path d="M133 72c8-24 28-42 52-48-8 22-28 40-50 50z" fill="url(#leaf2)"/>
      <path d="M131 72c-6-24-2-48 8-64 6 20 4 46-6 66z" fill="url(#leaf)"/>
      <path d="M127 76c-18-8-40-8-58 2 20 8 40 8 58 0z" fill="#3d8b54"/>
      <path d="M135 76c18-10 40-10 58 0-20 8-40 8-58 2z" fill="#357d4b"/>
      <path d="M106 78h56l-7 44q-21 5-42 0z" fill="url(#pot)"/>
      <rect x="101" y="72" width="66" height="10" rx="2.5" fill="#d27a52"/>
    </g>
  </svg>`;

  // Figma's bottom toolbar: Move, Frame, Rectangle, Pen and Pencil (with
  // its menu), Text and Comment
  const toolbar = (pencil = "pencil") => `<div class="toolbar"><span class="tool on">${ICON.move}</span><span class="tool">${ICON.frameTool}</span><span class="tool">${ICON.rectTool}</span><span class="tool">${ICON.pen}</span><span class="tool pencil"><span class="pi">${ICON[pencil]}</span><span class="li2">${ICON.lasso}</span></span><span class="more">${ICON.chevron}</span><span class="tool">${ICON.textTool}</span><span class="tool">${ICON.comment}</span></div>`;

  // A theme's colors for the editor in a scene (themes.js), as CSS
  const themeVars = (id, mode) => {
    const swatch = typeof ligmaSwatch === "function" ? ligmaSwatch(id, mode) : null;
    if (!swatch) return "";
    return `--t-bg:${swatch.bg};--t-panel:${swatch.panel};--t-line:${swatch.line};--t-accent:${swatch.accent};--t-soft:${swatch.soft};`;
  };
  const meshOf = (id, mode) => (typeof ligmaMesh === "function" ? ligmaMesh(id, mode) : "#888");

  // A theme's card in Settings > Appearance: its light and dark orbs
  const themeCard = (id, name) => `<div class="theme ${id}"><span class="orbs"><i class="orb" data-mesh="${id}:light"></i><i class="orb" data-mesh="${id}:dark"></i></span><span class="tn">${name}</span></div>`;

  // Ligma's logo in color, as Discord shows it
  const LOGO_COLOR = '<svg viewBox="0 0 24 24" width="24" height="24"><path d="M9 3a3 3 0 0 1 3 3v3H9a3 3 0 0 1 0-6z" fill="#00cf66"/><path d="M12 9H9a3 3 0 0 0 0 6h3z" fill="#ff0026"/><path d="M12 15H9a3 3 0 0 0 0 6h3z" fill="#904bff"/><circle cx="15" cy="18" r="3" fill="#00baff"/></svg>';

  const DEMOS = {
    "color-frames": {
      title: "Color frames",
      pointer: false,
      steps: [
        { caption: "Frames in the layers panel", ms: 1500 },
        { caption: "Each gets a color, with a guide down what's in it", ms: 2200 },
        { caption: "Tint rows washes each row in it", ms: 2000 },
        { caption: "Or pick a palette, like Sunset", ms: 2200 },
      ],
      html: `<div class="win">${editor(
        [
          layer("frame", "Hero", 0, "caretDown", "g0 own"),
          layer("text", "Headline", 1, null, "g0"),
          layer("image", "Photo", 1, null, "g0"),
          layer("rect", "Get started", 1, null, "g0"),
          layer("frame", "Pricing", 0, "caretDown", "g1 own"),
          layer("text", "Plans", 1, null, "g1"),
          layer("frame", "Footer", 0, "caretRight", "g2"),
        ].join(""),
        `<span class="fl" style="left:24px;top:28px">Hero</span><div class="fr" style="left:24px;top:46px">${art("landing", 232, 130)}</div>
         <span class="fl" style="left:24px;top:190px">Pricing</span><div class="fr pricing" style="left:24px;top:208px;width:232px;height:90px"><b>Simple pricing</b><span><i>Free</i><i>$12</i><i>$40</i></span></div>`,
      )}</div>`,
      css: `
        .win { left: 20px; top: 20px; width: 540px; height: 300px; }
        .lr { transition: background-color 400ms ${EASE}; }
        .lr .li { transition: color 400ms ${EASE}; }
        .guide { transform: scaleY(0); transform-origin: top; transition: transform 300ms ${EASE}, background-color 400ms ${EASE}; }
        .at-1 .lr:is(.g0, .g1, .g2) .li { color: var(--c); }
        .at-1 .guide { transform: none; }
        .at-1 .lr:nth-child(3) .guide { transition-delay: 60ms; }
        .at-1 .lr:nth-child(4) .guide { transition-delay: 120ms; }
        .at-1 .lr:nth-child(5) .guide { transition-delay: 180ms; }
        .at-1 .lr:nth-child(6) .guide { transition-delay: 240ms; }
        .at-1 .lr:nth-child(7) .guide { transition-delay: 300ms; }
        .at-2 .lr:is(.g0, .g1, .g2) { background-image: linear-gradient(90deg, color-mix(in oklab, var(--c) 13%, transparent), color-mix(in oklab, var(--c) 3%, transparent) 70%); }`,
    },

    "settings-search": {
      title: "Settings search",
      steps: [
        { caption: "Settings has a search box", ms: 1300, at: [320, 210] },
        { caption: "Settings has a search box", ms: 700, at: [110, 76] },
        { caption: "Settings has a search box", ms: 500, click: true },
        { caption: "It finds a setting on any page", ms: 1700 },
        { caption: "Turn it on right there", ms: 750, at: [438, 149] },
        { caption: "Turn it on right there", ms: 1800, click: true },
      ],
      html: `
        <div class="win">
          <div class="head">Settings<span class="shut">${ICON.close}</span></div>
          <div class="body">
            <nav>
              <div class="find">${ICON.search}<span class="q"><span class="ph">Search</span><span class="typed">${[..."frame"].map((c) => `<b>${c}</b>`).join("")}</span><i class="caret"></i></span><span class="clear">${CLEAR}</span></div>
              <div class="pages">
                <span class="pg on">${ICON.general}General</span><span class="pg">${ICON.appearance}Appearance</span><span class="pg">${ICON.appIcon}App Icon</span><span class="pg">${ICON.tabs}Tabs &amp; Memory</span>
                <i class="rule"></i>
                <span class="pg hit">${ICON.editor}Editor<em>2</em></span><span class="pg">${ICON.images}Images</span><span class="pg">${ICON.plugins}Plugins</span>
                <i class="rule"></i>
                <span class="pg">${ICON.ai}AI Tools</span><span class="pg">${ICON.discord}Discord</span>
              </div>
            </nav>
            <main class="page">
              <div class="pt">General</div>
              <h5>Home</h5>
              <div class="sr"><span><b>Ligma's home page</b><small>A faster home of Ligma's own, with starred files, Recents and search</small></span><i class="sw on"></i></div>
              <div class="sr"><span><b>Use the classic home page</b><small>Keeps Figma's usual home page on accounts trying its new one</small></span><i class="sw"></i></div>
            </main>
            <main class="results">
              <div class="pt">Editor</div>
              <h5>Layers</h5>
              <div class="sr"><span><b>Color <mark>frame</mark>s</b><small><mark>Frame</mark>s in the layers panel take turns through the rainbow, on their icons, with a guide in their color</small></span><i class="sw pick"></i></div>
              <div class="sr sub off"><span><b>Palette</b></span><span class="pal"><i></i>Rainbow${ICON.chevron}</span></div>
              <div class="sr sub off"><span><b>Tint rows</b><small>Each layer's row also takes a light wash of its <mark>frame</mark>'s color</small></span><i class="sw"></i></div>
            </main>
          </div>
        </div>`,
      css: `
        .win { left: 16px; top: 16px; width: 452px; height: 300px; }
        .head { position: relative; display: flex; align-items: center; height: 38px; padding-left: 16px; border-bottom: 1px solid var(--border); font-weight: 550; }
        .shut { position: absolute; right: 0; top: 0; display: grid; place-items: center; width: 46px; height: 38px; color: var(--text-secondary); }
        .body { display: flex; height: 262px; }
        nav { display: flex; flex: none; flex-direction: column; gap: 8px; width: 184px; padding: 8px; border-right: 1px solid var(--border); }
        .at-2 .find { border-color: var(--brand); }
        .at-2 .find .ph { display: none; }
        .at-2 .find .caret { opacity: 1; animation: demo-blink 1s steps(1) infinite; }
        .at-3 .find .typed b { max-width: 1em; }
        .at-3 .find .typed b:nth-child(2) { transition: max-width 0s 110ms; }
        .at-3 .find .typed b:nth-child(3) { transition: max-width 0s 220ms; }
        .at-3 .find .typed b:nth-child(4) { transition: max-width 0s 330ms; }
        .at-3 .find .typed b:nth-child(5) { transition: max-width 0s 440ms; }
        .at-3 .find .clear { opacity: 1; transition-delay: 500ms; }
        .pages { display: flex; flex-direction: column; gap: 2px; }
        .pg { display: flex; align-items: center; gap: 8px; height: 28px; padding: 0 8px; border-radius: 5px; color: var(--text-secondary); white-space: nowrap; transition: color 200ms, background-color 200ms; }
        .pg svg { flex: none; }
        .pg.on { background: var(--bg-secondary); color: var(--text); font-weight: 550; }
        .pg em { margin-left: auto; color: var(--text-secondary); font-style: normal; font-weight: 450; opacity: 0; }
        .rule { height: 1px; margin: 4px 8px; background: var(--border); }
        .at-3 .pg { color: var(--d-text3); transition-delay: 450ms; }
        .at-3 .pg.on { background: none; font-weight: 450; }
        .at-3 .pg.hit { color: var(--text); }
        .at-3 .pg.hit em { opacity: 1; transition: opacity 200ms 450ms; }
        main { position: absolute; left: 185px; top: 39px; right: 0; transition: opacity 200ms; }
        .pt { padding: 14px 16px 2px; font-size: 13px; font-weight: 600; line-height: 20px; }
        h5 { margin: 0; padding: 8px 16px 4px; font-size: 11px; font-weight: 550; line-height: 16px; }
        .sr { display: flex; align-items: center; gap: 16px; min-height: 32px; padding: 6px 16px; }
        .sr > span:first-child { flex: 1; min-width: 0; }
        .sr b { display: block; font-weight: 450; }
        .sr small { display: -webkit-box; margin-top: 1px; overflow: hidden; color: var(--text-secondary); font-size: 11px; -webkit-box-orient: vertical; -webkit-line-clamp: 2; }
        .sr.sub { padding-left: 32px; }
        .sr.off b, .sr.off small, .sr.off .pal { color: var(--d-text3); transition: color 200ms; }
        .sr.off .sw { opacity: .4; }
        .pal { display: flex; align-items: center; gap: 6px; height: 24px; padding: 0 4px 0 8px; border-radius: 5px; }
        .pal i { width: 22px; height: 8px; border-radius: 4px; background: linear-gradient(90deg, #e5484d, #f5a524, #e8d44d, #3dbf6e, #3e8eff, #a463f2); }
        mark { border-radius: 2px; background: color-mix(in srgb, var(--brand) 35%, transparent); color: inherit; }
        .results { opacity: 0; }
        .at-3 .page { opacity: 0; transition-delay: 450ms; }
        .at-3 .results { opacity: 1; transition-delay: 450ms; }
        .at-5 .sw.pick { background: var(--brand); }
        .at-5 .sw.pick::before { transform: translateX(12px); }
        .at-5 .sr.off :is(b, small, .pal) { color: var(--text); }
        .at-5 .sr.off small { color: var(--text-secondary); }
        .at-5 .sr.off .sw { opacity: 1; }`,
    },

    heal: {
      title: "Heal",
      brush: true,
      steps: [
        { caption: "An image with a date stamp", ms: 1300, at: [270, 120] },
        { caption: "Heal is in the image bar", ms: 700, at: [149, 236] },
        { caption: "Heal is in the image bar", ms: 600, click: true },
        { caption: "Paint over what to remove", ms: 700, at: [296, 182] },
        { caption: "Paint over what to remove", ms: 1150, at: [370, 180], move: 1000 },
        { caption: "Healed, on your computer", ms: 1600 },
        { caption: "Healed, on your computer", ms: 700, at: [418, 236] },
        { caption: "Healed, on your computer", ms: 1300, click: true },
      ],
      html: `
        <div class="win canvas"></div>
        ${selected(`<canvas data-size="260x146" data-sharp="1"></canvas><span class="stamp">08 14 '19</span>`, "260 × 146")}
        <svg class="stroke" viewBox="0 0 480 267" width="480" height="267"><path d="M296 182c18-6 30 4 46-2s20 2 28 0" pathLength="1"/></svg>
        ${imageBar}
        <div class="bar heal"><span class="hint">Paint over what to remove</span><span class="div"></span><span class="seg"><span class="on">Heal</span><span>Scrub</span></span><span class="div"></span><label class="field"><span>Size</span><i>24</i></label><span class="div"></span><span class="b done">Done</span></div>`,
      css: `
        .stamp { position: absolute; right: 8px; bottom: 6px; color: #ff8a1f; font: 700 10px/1 "Roboto Mono", ui-monospace, Consolas, monospace; letter-spacing: .04em; text-shadow: 0 0 3px rgba(255, 120, 20, .6); transition: opacity 700ms ${EASE} 150ms, filter 700ms ${EASE} 150ms; }
        .stroke { position: absolute; left: 0; top: 0; pointer-events: none; }
        .stroke path { fill: none; stroke: color-mix(in srgb, var(--brand) 40%, transparent); stroke-width: 24; stroke-linecap: round; stroke-dasharray: 1; stroke-dashoffset: 1; transition: opacity 400ms; }
        .at-4 .stroke path { stroke-dashoffset: 0; transition: stroke-dashoffset 1000ms ${EASE}; }
        .at-5 .stroke path { opacity: 0; }
        .at-5 .stamp { opacity: 0; filter: blur(3px); }
        .bar.heal { opacity: 0; }
        .at-2 .bar.image { opacity: 0; }
        .at-2 .bar.heal { opacity: 1; }
        .at-3 .demo-pointer svg { opacity: 0; }
        .at-3 .demo-brush { opacity: 1; }
        .at-6 .demo-pointer svg { opacity: 1; }
        .at-6 .demo-brush { opacity: 0; }
        .at-7 .bar.image { opacity: 1; }
        .at-7 .bar.heal { opacity: 0; }`,
    },

    upscale: {
      title: "Upscale",
      steps: [
        { caption: "A small image", ms: 1300, at: [270, 120] },
        { caption: "Upscale is in the image bar", ms: 700, at: [218, 236] },
        { caption: "Upscale is in the image bar", ms: 600, click: true },
        { caption: "Make it 2 or 4 times as large", ms: 650, at: [93, 236] },
        { caption: "Make it 2 or 4 times as large", ms: 600, click: true },
        { caption: "Make it 2 or 4 times as large", ms: 650, at: [342, 236] },
        { caption: "Compare before and after", ms: 1300, click: true },
        { caption: "Compare before and after", ms: 650, at: [240, 119] },
        { caption: "Compare before and after", ms: 1100, at: [175, 119], move: 900 },
        { caption: "Apply keeps it", ms: 700, at: [338, 236] },
        { caption: "Apply keeps it", ms: 1600, click: true },
      ],
      html: `
        <div class="win canvas"></div>
        ${selected(`<canvas class="lo" data-size="40x22"></canvas><canvas class="hi" data-size="260x146" data-sharp="1"></canvas>
          <span class="tag before">Before</span><span class="tag after">After</span><i class="line"></i><i class="knob">${ICON.arrows}</i>`, '<span class="d1">260 × 146</span><span class="d2">1040 × 584</span>')}
        ${imageBar}
        <div class="bar up"><span class="seg x"><span class="x2">2×</span><span class="x4">4×</span></span><span class="div"></span><span class="seg"><span>Fast</span><span class="on">Photo</span><span>Illustration</span></span><span class="div"></span><span class="b go" style="width:72px">${ICON.upscale}Upscale</span><span class="b apply" style="width:62px">${ICON.check}Apply</span><span class="div"></span><span class="b">Cancel</span></div>`,
      css: `
        .lo { image-rendering: pixelated; }
        .hi { clip-path: inset(0 0 0 100%); }
        .at-6 .hi { clip-path: inset(0 0 0 50%); transition: clip-path 700ms ${EASE} 400ms; }
        .at-8 .hi { clip-path: inset(0 0 0 25%); transition: clip-path 900ms ${EASE}; }
        .at-10 .hi { clip-path: inset(0 0 0 0); transition: clip-path 500ms ${EASE}; }
        .tag { position: absolute; top: 8px; padding: 2px 6px; border-radius: 4px; background: rgb(0 0 0 / .55); color: #fff; opacity: 0; transition: opacity 120ms ease-out; }
        .before { left: 8px; } .after { right: 8px; }
        .line { position: absolute; top: 0; bottom: 0; left: 50%; width: 2px; margin-left: -1px; background: #fff; box-shadow: 0 0 0 .5px rgb(0 0 0 / .2), 0 0 8px rgb(0 0 0 / .25); opacity: 0; }
        .knob { position: absolute; top: 50%; left: 50%; display: grid; place-items: center; width: 28px; height: 28px; margin: -14px 0 0 -14px; border-radius: 50%; background: #fff; color: #1e1e1e; box-shadow: 0 1px 4px rgb(0 0 0 / .3), 0 0 0 .5px rgb(0 0 0 / .12); opacity: 0; }
        .line, .knob { transition: opacity 200ms, left 900ms ${EASE}, transform 140ms; }
        .at-6 :is(.tag, .line, .knob) { opacity: 1; transition-delay: 400ms, 0ms, 0ms; }
        .at-8 :is(.line, .knob) { left: 25%; transition-delay: 0ms; }
        .at-8 .knob { transform: scale(1.1); }
        .at-9 .knob { transform: none; }
        .at-10 :is(.tag, .line, .knob) { opacity: 0; transition-delay: 0ms; }
        .d2 { display: none; }
        .at-10 .d1 { display: none; }
        .at-10 .d2 { display: inline; }
        .bar.up { opacity: 0; }
        .at-2 .bar.image { opacity: 0; }
        .at-2 .bar.up { opacity: 1; }
        .x2 { background: var(--bg); color: var(--text); box-shadow: 0 0 0 1px var(--border); }
        .at-4 .x2 { background: none; color: var(--text-secondary); box-shadow: none; }
        .at-4 .x4 { background: var(--bg); color: var(--text); box-shadow: 0 0 0 1px var(--border); }
        .bar .b.apply { display: none; background: var(--brand); color: #fff; }
        .at-6 .bar .b.go { display: none; }
        .at-6 .bar .b.apply { display: flex; }
        .at-10 .bar.up { opacity: 0; }
        .at-10 .bar.image { opacity: 1; }`,
    },

    "account-settings": {
      title: "Account settings",
      size: [560, 311],
      steps: [
        { caption: "Your account, on Ligma's home page", ms: 1200, at: [330, 240] },
        { caption: "Your account, on Ligma's home page", ms: 650, at: [90, 40] },
        { caption: "Its menu, as Figma has it", ms: 1000, click: true },
        { caption: "Settings for your Figma account", ms: 650, at: [90, 248] },
        { caption: "Settings for your Figma account", ms: 1600, click: true },
        { caption: "Emails stay blurred until you click", ms: 800, at: [420, 141] },
        { caption: "Emails stay blurred until you click", ms: 1800, click: true },
      ],
      html: `
        <div class="win home">
          <aside>
            <div class="acct"><i class="av" style="--av:#e8604c">C</i><b>Chris</b>${ICON.chevron}<span class="bell">${ICON.bell}<em>4</em></span></div>
            <div class="tree">
              <span class="nav find">${ICON.search}Search<kbd>Ctrl+K</kbd></span>
              <span class="nav on">${ICON.home}Home</span>
              <span class="nav">${ICON.clock}Recents</span>
              <span class="nav">${ICON.star}Starred<small>5</small></span>
              <span class="nav">${ICON.people}Shared with you</span>
              <span class="nav">${ICON.file}Drafts<small>6</small></span>
            </div>
          </aside>
          <section class="main"><h4>Home</h4><div class="chips"><span class="on">All</span><span>Design</span><span>FigJam</span><span>Slides</span></div><h5>${ICON.chevron}Starred<small>5</small></h5><div class="cards"><div class="thumb">${art("landing", 260, 146)}</div><div class="thumb">${art("icons", 260, 146)}</div></div></section>
          <i class="veil"></i>
        </div>
        <div class="hmenu">
          <div class="acard"><i class="av" style="--av:#e8604c">C</i><b>Chris</b><span class="blur">chris@example.com</span></div>
          <i class="msep"></i>
          <span class="mi"><span class="mic">${ICON.plus}</span>Add account</span>
          <i class="msep"></i>
          <span class="mi">Change theme<span class="arrow">${ICON.right}</span></span>
          <span class="mi hover">Settings</span>
          <i class="msep"></i>
          <span class="mi">Log out</span>
        </div>
        <div class="acs">
          <div class="ah"><b>Settings</b><span class="blur">chris@example.com</span><span class="x">${ICON.close}</span></div>
          <div class="ab">
            <div class="an"><span class="nav on">${ICON.account}Account</span><span class="nav">${ICON.community}Community</span><span class="nav">${ICON.bell}Notifications</span><span class="nav">${ICON.lock}Security</span></div>
            <div class="ac">
              <div class="aprof"><i class="av big" style="--av:#14ae9a">C</i><div class="id"><b>Chris</b><span class="blur email">chris@example.com</span></div><span class="btn">Change photo</span></div>
              <div class="srow"><div class="sl"><small>Name</small><span>Chris</span></div><span class="btn">Change name</span></div>
              <div class="srow"><div class="sl"><small>Email</small><span class="blur">chris@example.com</span></div><span class="btn">Change email${ICON.external}</span></div>
            </div>
          </div>
        </div>`,
      css: `
        .win { left: 16px; top: 16px; width: 700px; height: 360px; }
        .hmenu { position: absolute; left: 24px; top: 68px; width: 200px; padding: 8px; border-radius: 13px; background: var(--d-menu); box-shadow: var(--d-shadow), 0 0 0 1px var(--d-menu-line); opacity: 0; transform: translateY(-2px) scale(.98); transform-origin: top left; transition: opacity 120ms cubic-bezier(.16, 1, .3, 1), transform 120ms cubic-bezier(.16, 1, .3, 1); }
        .at-2 .hmenu { opacity: 1; transform: none; }
        .at-2 .acct { background: var(--hover); }
        .at-3 .mi.hover { background: var(--brand); color: #fff; }
        .at-4 .hmenu { opacity: 0; }
        .at-4 .acct { background: none; }
        .acs { position: absolute; left: 66px; top: 26px; width: 760px; height: 420px; border-radius: 13px; background: var(--bg); box-shadow: var(--d-shadow), 0 0 0 1px var(--border); opacity: 0; transform: translateY(-2px) scale(.98); transition: opacity 140ms cubic-bezier(.16, 1, .3, 1), transform 140ms cubic-bezier(.16, 1, .3, 1); }
        .veil { position: absolute; inset: 0; background: rgb(0 0 0 / .24); opacity: 0; transition: opacity 140ms; }
        .at-4 :is(.acs, .veil) { opacity: 1; transform: none; }
        .at-6 .email { filter: none; }`,
    },

    "themed-loading": {
      title: "Themed loading",
      steps: [
        { caption: "Open a file from Home", ms: 1100, at: [330, 250] },
        { caption: "Open a file from Home", ms: 650, at: [390, 196] },
        { caption: "It opens in your theme, not white", ms: 1500, click: true },
        { caption: "Ligma's logo while it loads", ms: 2600 },
        { caption: "Then your file", ms: 2000, at: [420, 250] },
      ],
      html: `
        <div class="win">
          ${tabBar({ home: true, list: [{ kind: "design", title: "Onboarding flow" }, { kind: "board", title: "Team retro" }, { kind: "design", title: "Ligma site — v3", cls: "opening" }] })}
          <div class="view homeview home">
            <aside><div class="acct"><i class="av" style="--av:#e8604c">C</i><b>Chris</b>${ICON.chevron}</div><div class="tree"><span class="nav find">${ICON.search}Search</span><span class="nav on">${ICON.home}Home</span><span class="nav">${ICON.clock}Recents</span><span class="nav">${ICON.star}Starred<small>5</small></span></div></aside>
            <section class="main"><h5>${ICON.chevron}Starred<small>5</small></h5><div class="cards"><div class="card"><div class="thumb">${art("landing", 232, 130)}</div><div class="cn"><span class="g">${ICON.design}</span>Ligma site — v3</div></div></div></section>
          </div>
          <div class="view load">${LOGO}<div class="progress"><i></i></div></div>
          <div class="view file">${editor(
            [layer("frame", "Desktop", 0, "caretDown", "own"), layer("frame", "Nav", 1, "caretRight"), layer("text", "Ship the next thing", 1), layer("rect", "Get started", 1), layer("frame", "Footer", 1, "caretRight")].join(""),
            `<span class="fl" style="left:24px;top:24px">Desktop</span><div class="fr" style="left:24px;top:42px">${art("landing", 232, 130)}</div>`,
          )}</div>
        </div>`,
      css: `
        .win { left: 16px; top: 16px; width: 540px; height: 300px; }
        .t.opening { max-width: 0; padding: 0; opacity: 0; transition: max-width 200ms ${EASE}, padding 200ms ${EASE}, opacity 150ms; }
        .at-2 .t.opening { max-width: 240px; padding: 0 8px 0 12px; opacity: 1; background: var(--bg); color: var(--text); }
        .at-2 .t.opening > svg { color: var(--accent); }
        .at-2 .t.opening .x { visibility: visible; }
        .at-2 .t.home { background: none; color: var(--d-tab-text); }
        .at-2 .t:nth-child(3)::after { display: none; }
        .view { position: absolute; left: 0; right: 0; top: 38px; bottom: 0; transition: opacity 300ms ${EASE}; }
        .load { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 18px; padding-bottom: 40px; background: var(--bg); opacity: 0; }
        .file { opacity: 0; }
        .at-2 .homeview { opacity: 0; transition-duration: 0ms; }
        .at-2 .load { opacity: 1; transition-duration: 0ms; }
        .at-4 .load { opacity: 0; }
        .at-4 .file { opacity: 1; }
        .progress { width: 120px; height: 2px; border-radius: 1px; background: var(--border); overflow: hidden; }
        .progress i { display: block; width: 0; height: 100%; background: var(--text); }
        .at-2 .progress i { width: 85%; transition: width 4000ms cubic-bezier(.2, .6, .3, 1); }
        .logo { color: var(--text); }
        .logo :is(.top, .middle, .cap, .span, .rest, .end, .divide, .boundary) { animation-play-state: paused; }
        .at-2 .logo :is(.top, .middle, .cap, .span, .rest, .end, .divide, .boundary) { animation-play-state: running; }`,
    },

    "home-page": {
      title: "Ligma's home page",
      size: [640, 356],
      steps: [
        { caption: "Starred files come first", ms: 1300, at: [420, 330] },
        { caption: "Pick one up", ms: 700, at: [542, 206] },
        { caption: "Pick one up", ms: 500, click: true },
        { caption: "Put it where you want it", ms: 1300, at: [142, 206], move: 1100 },
        { caption: "Put it where you want it", ms: 700 },
        { caption: "See who has a file open", ms: 2000, at: [420, 330] },
      ],
      html: `
        <div class="win home">
          <section class="main">
            <h4>Home</h4>
            <div class="chips"><span class="on">All</span><span>Design</span><span>FigJam</span><span>Slides</span></div>
            <h5>${ICON.chevron}Starred<small>4</small><span class="all">View all${ICON.right}</span></h5>
            <i class="hole"></i>
            <div class="card a"><div class="thumb">${art("landing", 180, 101)}<span class="pin">${ICON.star}</span></div><div class="faces"><i class="face" style="--av:#9747ff">S</i></div><div class="cn"><span class="g">${ICON.design}</span>Ligma site — v3</div><div class="cm">Website · You edited 12 min ago</div></div>
            <div class="card b"><div class="thumb">${art("icons", 180, 101)}<span class="pin">${ICON.star}</span></div><div class="cn"><span class="g">${ICON.design}</span>Icon set 16px</div><div class="cm">App · You edited 5 h ago</div></div>
            <div class="card c"><div class="thumb">${art("bank", 180, 101)}<span class="pin">${ICON.star}</span></div><div class="cn"><span class="g">${ICON.design}</span>Halcyon Bank</div><div class="cm">Client work · 4 h ago</div></div>
            <div class="card d"><div class="thumb">${art("board", 180, 101)}<span class="pin">${ICON.star}</span></div><div class="cn"><span class="g board">${ICON.board}</span>Q4 roadmap</div><div class="cm">Marketing · Yesterday</div></div>
          </section>
        </div>`,
      css: `
        .win { left: 20px; top: 20px; width: 740px; height: 420px; }
        .card { position: absolute; top: 136px; left: calc(32px + var(--slot) * 200px); width: 180px; transition: left 360ms cubic-bezier(.16, 1, .3, 1); }
        .a { --slot: 0; } .b { --slot: 1; } .c { --slot: 2; } .d { --slot: 3; }
        .at-3 .a { --slot: 1; transition-delay: 500ms; } .at-3 .b { --slot: 2; transition-delay: 300ms; }
        .c { z-index: 2; transition: left 1100ms ${EASE}, padding 140ms, margin 140ms, background-color 140ms, box-shadow 140ms; }
        .at-2 .c { margin: -6px 0 0 -6px; padding: 6px; border-radius: 12px; background: var(--bg); box-shadow: 0 18px 44px rgba(0, 0, 0, .28), 0 4px 12px rgba(0, 0, 0, .18), 0 0 0 1px var(--border); }
        .at-3 .c { --slot: 0; }
        .at-4 .c { margin: 0; padding: 0; background: none; box-shadow: none; }
        .hole { position: absolute; top: 136px; left: 432px; width: 180px; height: 101px; border-radius: 6px; background: color-mix(in srgb, var(--brand) 10%, transparent); box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--brand) 40%, transparent); opacity: 0; transition: left 360ms cubic-bezier(.16, 1, .3, 1) 300ms, opacity 140ms; }
        .at-2 .hole { opacity: 1; }
        .at-3 .hole { left: 32px; transition-delay: 600ms, 0ms; }
        .at-4 .hole { opacity: 0; }
        .faces .face { transform: scale(0); transition: transform 300ms cubic-bezier(.16, 1, .3, 1); }
        .at-5 .faces .face { transform: none; }`,
    },

    "tab-emoji": {
      title: "Tab emoji",
      steps: [
        { caption: "Your file tabs", ms: 1100, at: [330, 190] },
        { caption: "Right-click one", ms: 650, at: [268, 35] },
        { caption: "Right-click one", ms: 900, click: true },
        { caption: "Add Emoji", ms: 600, at: [306, 82] },
        { caption: "Add Emoji", ms: 700, click: true },
        { caption: "Pick one", ms: 700, at: [402, 146] },
        { caption: "It's the tab's icon now", ms: 1900, click: true },
      ],
      html: `
        <div class="win">
          ${tabBar({ list: [{ kind: "design", title: "Brand kit" }, { kind: "design", title: "Onboarding flow", cls: "on before", icon: `<span class="ico">${ICON.design}<span class="emoji">🚀</span></span>` }, { kind: "board", title: "Team retro" }] })}
          <div class="view">${editor(
            [layer("frame", "Welcome", 0, "caretDown", "own"), layer("text", "Get started", 1), layer("image", "Hero", 1), layer("frame", "Sign up", 0, "caretRight"), layer("frame", "Done", 0, "caretRight")].join(""),
            `<span class="fl" style="left:24px;top:24px">Welcome</span><div class="fr phone" style="left:24px;top:42px"><b>Welcome</b><small>Plan, design and ship together</small><em>Get started</em></div>`,
          )}</div>
        </div>
        <div class="tmenu"><span class="it">Pin Tab</span><span class="it pick">Add Emoji</span><span class="it">Copy Link</span><span class="it">Open in Browser</span><span class="it">Reload</span><span class="it">Move to New Window</span><i class="sep"></i><span class="it">Close</span></div>
        <div class="picker">
          <div class="psearch">${ICON.search}<span>Search emoji</span></div>
          <h6>Suggested</h6>
          <div class="cells">${[..."😀❤👍🔥✨🎨🚀💡✅📌⭐🎯📱🧩🌈🍋"].map((e, i) => `<span${i === 6 ? ' class="pick"' : ""}>${e}</span>`).join("")}</div>
          <h6>Smileys &amp; People</h6>
          <div class="cells">${[..."😃😄😁😆😅🤣😂🙂"].map((e) => `<span>${e}</span>`).join("")}</div>
        </div>`,
      css: `
        .win { left: 16px; top: 16px; width: 540px; height: 300px; }
        .view { position: absolute; left: 0; right: 0; top: 38px; bottom: 0; }
        .ico { position: relative; display: grid; flex: none; place-items: center; width: 16px; height: 16px; color: var(--accent); }
        .ico .emoji { position: absolute; font-size: 14px; line-height: 16px; transform: scale(0); transition: transform 280ms cubic-bezier(.16, 1, .3, 1); }
        .ico svg { transition: opacity 120ms; }
        .at-6 .ico svg { opacity: 0; }
        .at-6 .ico .emoji { transform: none; }
        .tmenu { position: absolute; left: 268px; top: 38px; display: flex; flex-direction: column; min-width: 176px; padding: 8px; border-radius: 8px; background: #1e1e1e; color: #fff; box-shadow: 0 5px 17px rgba(0, 0, 0, .3), 0 2px 7px rgba(0, 0, 0, .2), inset 0 0 0 .5px rgba(255, 255, 255, .12); opacity: 0; transition: opacity 100ms; }
        .it { display: flex; align-items: center; height: 24px; padding: 0 16px 0 8px; border-radius: 5px; white-space: nowrap; }
        .sep { height: 1px; margin: 8px -8px; background: rgba(255, 255, 255, .1); }
        .at-2 .tmenu { opacity: 1; }
        .at-3 .it.pick { background: #0d99ff; }
        .at-4 .tmenu { opacity: 0; }
        .picker { position: absolute; left: 186px; top: 54px; width: 288px; height: 360px; padding: 8px; border-radius: 8px; background: var(--bg); box-shadow: 0 5px 17px rgba(0, 0, 0, .3), 0 2px 7px rgba(0, 0, 0, .2), 0 0 0 .5px var(--border); opacity: 0; transition: opacity 120ms; }
        .psearch { display: flex; align-items: center; gap: 4px; height: 28px; padding: 0 6px; border: 1px solid var(--brand); border-radius: 5px; background: var(--bg-secondary); color: var(--text-secondary); }
        h6 { margin: 0; padding: 10px 4px 4px; font-size: 11px; font-weight: 550; line-height: 16px; }
        .cells { display: grid; grid-template-columns: repeat(8, 32px); grid-auto-rows: 32px; }
        .cells span { display: grid; place-items: center; border-radius: 5px; font-size: 20px; line-height: 1; }
        .at-5 .cells .pick { background: var(--hover); }
        .at-4 .picker { opacity: 1; }
        .at-6 .picker { opacity: 0; }`,
    },

    "ligma-mcp": {
      title: "Ligma MCP",
      steps: [
        { caption: "Ligma MCP, in the right panel", ms: 1500, at: [150, 250] },
        { caption: "What AI tools did in the file, newest first", ms: 1300 },
        { caption: "What AI tools did in the file, newest first", ms: 1200 },
        { caption: "What AI tools did in the file, newest first", ms: 1300 },
        { caption: "What AI tools did in the file, newest first", ms: 900 },
        { caption: "Select what a call made", ms: 800, at: [462, 233] },
        { caption: "Select what a call made", ms: 1900, click: true },
      ],
      html: `
        <div class="win">
          <div class="ed">
            <div class="cv">
              <span class="fl card-l">Pricing card</span>
              <div class="card2"><b>Pro</b><span>$12 a month</span><em>Choose Pro</em></div>
            </div>
            <div class="rp">
              <div class="rh"><b>Design</b><span>Prototype</span><em>100%</em></div>
              <div class="sec"><div class="sh">Page</div><div class="srow2"><i class="swatch"></i>1E1E1E<span>100%</span></div></div>
              <div class="sec mcp">
                <div class="sh">Ligma MCP<span class="chip">Connected</span><span class="cog">${ICON.gear}</span></div>
                <div class="mr r3"><span class="mi2"><span class="spin">${ICON.spinner}</span><span class="ok">${ICON.done}</span></span><span class="mn">create_text</span><span class="mm">now</span></div>
                <div class="mr r2"><span class="mi2"><span class="spin">${ICON.spinner}</span><span class="ok">${ICON.done}</span></span><span class="mn">set_auto_layout</span><span class="mm">now</span></div>
                <div class="mr r1"><span class="mi2"><span class="spin">${ICON.spinner}</span><span class="ok">${ICON.done}</span></span><span class="mn">create_frame</span><span class="mm">now</span><span class="sel2">${ICON.target}</span></div>
              </div>
            </div>
          </div>
        </div>`,
      css: `
        .win { left: -80px; top: 16px; width: 560px; height: 300px; }
        .rp { flex: none; width: 240px; border-left: 1px solid var(--border); background: var(--bg); }
        .rh { display: flex; align-items: center; gap: 12px; height: 40px; padding: 0 16px; border-bottom: 1px solid var(--border); }
        .rh b { font-weight: 550; } .rh span { color: var(--text-secondary); } .rh em { margin-left: auto; color: var(--text-secondary); font-style: normal; }
        .sec { padding-bottom: 8px; border-bottom: 1px solid var(--border); }
        .sh { display: flex; align-items: center; height: 32px; padding: 0 8px 0 16px; font-weight: 550; }
        .chip { display: inline-flex; align-items: center; height: 16px; margin-left: 8px; padding: 0 4px; border-radius: 5px; box-shadow: inset 0 0 0 1px var(--border); font-weight: 450; }
        .cog { display: grid; place-items: center; width: 24px; height: 24px; margin-left: auto; color: var(--text-secondary); }
        .srow2 { display: flex; align-items: center; gap: 8px; height: 32px; padding: 0 16px; }
        .srow2 .swatch { width: 14px; height: 14px; border-radius: 3px; background: var(--d-canvas); box-shadow: inset 0 0 0 1px var(--border); }
        .srow2 span { margin-left: auto; color: var(--text-secondary); }
        .mr { position: relative; display: flex; align-items: center; height: 0; margin: 0 8px; padding-right: 8px; overflow: hidden; border-radius: 5px; opacity: 0; transition: height 220ms ${EASE}, opacity 220ms; }
        .mi2 { position: relative; display: grid; flex: none; place-items: center; width: 24px; height: 24px; color: var(--text-secondary); }
        .mi2 > span { position: absolute; display: grid; place-items: center; }
        .spin svg { animation: demo-spin .8s linear infinite; }
        .ok { opacity: 0; }
        .mn { flex: 1; min-width: 0; padding-left: 4px; white-space: nowrap; }
        .mm { margin-left: 8px; color: var(--text-secondary); }
        .sel2 { position: absolute; right: 2px; top: 1px; display: none; place-items: center; width: 24px; height: 24px; border-radius: 4px; color: var(--text); background: var(--hover); }
        .at-1 .r1, .at-2 .r2, .at-3 .r3 { height: 26px; opacity: 1; }
        .at-2 .r1 .spin, .at-3 .r2 .spin, .at-4 .r3 .spin { opacity: 0; }
        .at-2 .r1 .ok, .at-3 .r2 .ok, .at-4 .r3 .ok { opacity: 1; }
        .at-5 .r1 { background: var(--hover); }
        .at-5 .r1 .sel2 { display: grid; }
        .at-5 .r1 .mm { visibility: hidden; }
        .card-l { left: 120px; top: 50px; opacity: 0; transition: opacity 300ms; }
        .card2 { position: absolute; left: 120px; top: 68px; display: flex; flex-direction: column; gap: 0; width: 150px; height: 110px; padding: 0; overflow: hidden; border-radius: 2px; background: #fff; color: #1e1a3a; opacity: 0; transform: scale(.96); transition: opacity 300ms, transform 300ms ${EASE}, gap 400ms ${EASE}, padding 400ms ${EASE}, box-shadow 200ms; }
        .card2 > * { opacity: 0; transition: opacity 300ms; }
        .card2 b { font-size: 16px; font-weight: 700; line-height: 20px; }
        .card2 span { color: #6b6880; }
        .card2 em { align-self: flex-start; margin-top: auto; padding: 4px 10px; border-radius: 6px; background: #6a5cff; color: #fff; font-style: normal; font-weight: 600; }
        .at-1 :is(.card-l, .card2) { opacity: 1; transform: none; }
        .at-2 .card2 { gap: 4px; padding: 16px; }
        .at-3 .card2 > * { opacity: 1; }
        .at-6 .card2 { box-shadow: 0 0 0 1px var(--brand); }
        .at-6 .card-l { color: var(--brand); }`,
    },

    "lasso-fill": {
      title: "Lasso fill",
      steps: [
        { caption: "Lasso fill, under Pencil", ms: 1100, at: [330, 120] },
        { caption: "Lasso fill, under Pencil", ms: 700, at: [288, 236] },
        { caption: "Lasso fill, under Pencil", ms: 700, click: true },
        { caption: "Lasso fill, under Pencil", ms: 600, at: [300, 192] },
        { caption: "Lasso fill, under Pencil", ms: 500, click: true },
        { caption: "Draw around an area", ms: 700, at: [186, 112] },
        { caption: "Draw around an area", ms: 1650, along: "M186 112C188 76 230 58 262 64C296 70 318 100 312 128C306 154 262 162 226 150C204 143 188 132 190 124", move: 1500 },
        { caption: "It fills with color, smoothed", ms: 2000, at: [380, 190] },
      ],
      html: `
        <div class="win canvas"></div>
        <span class="fl" style="left:110px;top:30px">Sticker</span>
        <div class="sheet"></div>
        <svg class="loop" viewBox="0 0 480 267" width="480" height="267">
          <defs><mask id="reveal"><path class="rv" d="M186 112C188 76 230 58 262 64C296 70 318 100 312 128C306 154 262 162 226 150C204 143 188 132 190 124" pathLength="1"/></mask></defs>
          <path class="filled" d="M186 112C188 76 230 58 262 64C296 70 318 100 312 128C306 154 262 162 226 150C204 143 188 132 186 112Z"/>
          <path class="dash" mask="url(#reveal)" d="M186 112C188 76 230 58 262 64C296 70 318 100 312 128C306 154 262 162 226 150C204 143 188 132 190 124"/>
        </svg>
        ${toolbar()}
        <div class="pmenu"><span class="pit">${ICON.pencil}Pencil<em>Shift+P</em></span><span class="pit las">${ICON.lasso}Lasso fill</span></div>`,
      css: `
        .sheet { position: absolute; left: 110px; top: 48px; width: 260px; height: 150px; background: #fff; }
        .loop { position: absolute; left: 0; top: 0; }
        .rv { fill: none; stroke: #fff; stroke-width: 8; stroke-dasharray: 1; stroke-dashoffset: 1; }
        .at-6 .rv { stroke-dashoffset: 0; transition: stroke-dashoffset 1500ms linear; }
        .dash { fill: color-mix(in srgb, var(--brand) 8%, transparent); stroke: var(--brand); stroke-width: 1; stroke-dasharray: 4 3; fill-opacity: 0; }
        .filled { fill: #ff7262; stroke: var(--brand); stroke-width: 1; opacity: 0; transition: opacity 300ms; }
        .at-7 .filled { opacity: 1; }
        .at-7 .dash { opacity: 0; }
        .pmenu { position: absolute; left: 250px; top: 148px; display: flex; flex-direction: column; width: 170px; padding: 8px; border-radius: 13px; background: #1e1e1e; color: #fff; box-shadow: 0 5px 17px rgba(0, 0, 0, .3), 0 2px 7px rgba(0, 0, 0, .2); opacity: 0; transition: opacity 120ms; }
        .pit { display: flex; align-items: center; gap: 8px; height: 24px; padding: 0 8px; border-radius: 5px; }
        .pit em { margin-left: auto; font-style: normal; opacity: .5; }
        .at-2 .pmenu { opacity: 1; }
        .at-3 .las { background: #0d99ff; }
        .at-4 .pmenu { opacity: 0; }
        .toolbar .tool .li2 { display: none; }
        .at-4 .toolbar .tool .pi { display: none; }
        .at-4 .toolbar .tool .li2 { display: grid; }
        .at-4 .toolbar .tool.on { background: none; color: var(--text); }
        .at-4 .toolbar .tool.pencil { background: var(--brand); color: #fff; }`,
    },

    "app-icons": {
      title: "App icons",
      size: [640, 356],
      steps: [
        { caption: "Settings > App Icon", ms: 1300, at: [300, 230] },
        { caption: "Pick one of dozens", ms: 700, at: [451, 260] },
        { caption: "Pick one of dozens", ms: 1500, click: true },
        { caption: "It's on the taskbar and the Dock", ms: 1700, at: [342, 336] },
        { caption: "Change it any time", ms: 700, at: [354, 260] },
        { caption: "Change it any time", ms: 1800, click: true },
      ],
      html: `
        <div class="win">
          <div class="head">Settings<span class="shut">${ICON.close}</span></div>
          <nav class="snav">
            <div class="find">${ICON.search}<span class="q"><span class="ph">Search</span></span></div>
            <span class="pg">${ICON.general}General</span><span class="pg">${ICON.appearance}Appearance</span><span class="pg on">${ICON.appIcon}App Icon</span><span class="pg">${ICON.tabs}Tabs &amp; Memory</span>
            <i class="rule"></i><span class="pg">${ICON.editor}Editor</span><span class="pg">${ICON.images}Images</span>
          </nav>
          <main class="ai-page">
            <div class="pt">App Icon</div>
            <h5>Current icon</h5>
            <div class="cur"><span class="stack"><img data-icon="ligma" class="c0"><img data-icon="onyx" class="c1"><img data-icon="ligma-dark" class="c2"></span><span class="cn2"><b><span class="c0">Ligma</span><span class="c1">Onyx</span><span class="c2">Ligma Dark</span></b><small>On the taskbar and in the Start menu, and kept after updates</small></span></div>
            <h5>Colorways</h5>
            <div class="iconset">
              <span class="ic i0"><img data-icon="ligma">Ligma</span>
              <span class="ic i2"><img data-icon="ligma-dark">Ligma Dark</span>
              <span class="ic i1"><img data-icon="onyx">Onyx</span>
              <span class="ic"><img data-icon="ink">Ink</span>
            </div>
          </main>
        </div>
        <div class="taskbar"><span class="tbi"><i class="g1"></i></span><span class="tbi"><i class="g2"></i></span><span class="tbi me"><span class="stack"><img data-icon="ligma" class="c0"><img data-icon="onyx" class="c1"><img data-icon="ligma-dark" class="c2"></span></span><span class="tbi"><i class="g3"></i></span></div>`,
      css: `
        .win { left: 16px; top: 8px; width: 660px; height: 400px; }
        .head { position: relative; display: flex; align-items: center; height: 38px; padding-left: 16px; border-bottom: 1px solid var(--border); font-weight: 550; }
        .shut { position: absolute; right: 0; top: 0; display: grid; place-items: center; width: 46px; height: 38px; color: var(--text-secondary); }
        .ai-page { position: absolute; left: 185px; top: 39px; right: 0; }
        .pt { padding: 14px 16px 2px; font-size: 13px; font-weight: 600; line-height: 20px; }
        h5 { margin: 0; padding: 8px 16px 4px; font-size: 11px; font-weight: 550; line-height: 16px; }
        .cur { display: flex; align-items: center; gap: 16px; padding: 6px 16px 10px; }
        .cur .stack { width: 64px; height: 64px; }
        .cn2 b { display: grid; font-size: 13px; font-weight: 550; line-height: 20px; }
        .cn2 b > span { grid-area: 1 / 1; }
        .cn2 small { display: block; color: var(--text-secondary); font-size: 11px; }
        .stack { position: relative; display: grid; flex: none; }
        .stack img { grid-area: 1 / 1; width: 100%; height: 100%; transition: opacity 200ms; }
        .c1, .c2 { opacity: 0; }
        .at-2 .c0 { opacity: 0; } .at-2 .c1 { opacity: 1; } .at-5 .c1 { opacity: 0; } .at-5 .c2 { opacity: 1; }
        .iconset { display: grid; grid-template-columns: repeat(4, 97px); padding: 4px 8px; }
        .ic { display: flex; flex-direction: column; align-items: center; gap: 6px; height: 97px; padding-top: 8px; border-radius: 8px; color: var(--text-secondary); transition: box-shadow 160ms; }
        .ic img { width: 58px; height: 58px; }
        .i0 { box-shadow: inset 0 0 0 2px var(--brand); color: var(--text); font-weight: 550; }
        .at-2 .i0 { box-shadow: none; color: var(--text-secondary); font-weight: 450; }
        .at-2 .i1 { box-shadow: inset 0 0 0 2px var(--brand); color: var(--text); font-weight: 550; }
        .at-5 .i1 { box-shadow: none; color: var(--text-secondary); font-weight: 450; }
        .at-5 .i2 { box-shadow: inset 0 0 0 2px var(--brand); color: var(--text); font-weight: 550; }
        .taskbar { position: absolute; left: 0; right: 0; bottom: 0; display: flex; justify-content: center; gap: 4px; height: 40px; padding-top: 2px; background: rgb(28 28 30 / .88); box-shadow: 0 -1px rgb(255 255 255 / .08); backdrop-filter: blur(20px); }
        .tbi { position: relative; display: grid; place-items: center; width: 40px; height: 36px; border-radius: 5px; }
        .tbi i { width: 24px; height: 24px; border-radius: 6px; }
        .g1 { background: linear-gradient(135deg, #ffd25e, #f2a33a); }
        .g2 { background: linear-gradient(135deg, #5ec8ff, #2a7bf2); border-radius: 50% !important; }
        .g3 { background: linear-gradient(135deg, #9b8cff, #6a5cff); }
        .tbi.me { background: rgb(255 255 255 / .08); }
        .tbi.me::after { content: ""; position: absolute; bottom: 1px; width: 16px; height: 3px; border-radius: 2px; background: #6cb8ff; }
        .tbi .stack { width: 26px; height: 26px; }`,
    },

    "split-view": {
      title: "Split view",
      size: [560, 311],
      steps: [
        { caption: "Drag a file tab onto the canvas", ms: 1100, at: [350, 250] },
        { caption: "Drag a file tab onto the canvas", ms: 700, at: [297, 35] },
        { caption: "Drag a file tab onto the canvas", ms: 400, click: true },
        { caption: "Drop it on either half", ms: 1300, at: [470, 190], move: 900 },
        { caption: "Two files side by side, one set of panels", ms: 2300, click: true },
      ],
      html: `
        <div class="win">
          ${tabBar({ home: false, list: [{ kind: "design", title: "Onboarding flow", cls: "on" }, { kind: "board", title: "Team retro", cls: "two" }] })}
          <div class="view">${editor(
            [layer("frame", "Welcome", 0, "caretDown", "own"), layer("text", "Get started", 1), layer("image", "Hero", 1), layer("frame", "Sign up", 0, "caretRight"), layer("frame", "Done", 0, "caretRight")].join(""),
            `<span class="fl" style="left:24px;top:24px">Welcome</span><div class="fr phone" style="left:24px;top:42px"><b>Welcome</b><small>Plan, design and ship together</small><em>Get started</em></div>
             <div class="half"><span class="fl" style="left:16px;top:24px">Retro board</span><div class="fr" style="left:16px;top:42px">${art("board", 220, 124)}</div></div>
             <i class="hint2"></i>`,
          )}</div>
        </div>
        <div class="ghost">${ICON.board}Team retro</div>`,
      css: `
        .win { left: 16px; top: 16px; width: 528px; height: 330px; }
        .view { position: absolute; left: 0; right: 0; top: 38px; bottom: 0; }
        .half { position: absolute; left: 50%; top: 0; bottom: 0; right: 0; background: var(--d-canvas); box-shadow: -1px 0 var(--border); opacity: 0; transition: opacity 250ms; }
        .hint2 { position: absolute; left: calc(50% + 6px); top: 6px; right: 6px; bottom: 6px; border: 2px solid var(--brand); border-radius: 8px; background: color-mix(in srgb, var(--brand) 16%, transparent); opacity: 0; transition: opacity 160ms; }
        .at-3 .hint2 { opacity: 1; transition-delay: 600ms; }
        .at-4 .hint2 { opacity: 0; transition-delay: 0ms; }
        .at-4 .half { opacity: 1; }
        .at-4 .t.two { background: var(--bg); color: var(--text); }
        .at-4 .t.two > svg { color: var(--accent); }
        .ghost { position: absolute; left: 0; top: 0; display: flex; align-items: center; gap: 8px; height: 38px; padding: 0 12px; border-radius: 6px; background: var(--bg); color: var(--text); box-shadow: 0 8px 24px rgba(0, 0, 0, .35); font: 500 12px/1 -apple-system, BlinkMacSystemFont, "Segoe UI Variable Text", "Segoe UI", "Inter", sans-serif; letter-spacing: 0; transform: translate(232px, 16px); opacity: 0; transition: opacity 120ms, transform 900ms ${EASE}; }
        .ghost svg { color: #9747ff; }
        .at-2 .ghost { opacity: .85; }
        .at-3 .ghost { transform: translate(405px, 171px); }
        .at-4 .ghost { opacity: 0; }`,
    },

    "select-object": {
      title: "Select object",
      steps: [
        { caption: "An image", ms: 1000, at: [330, 120] },
        { caption: "Select object, in the image bar", ms: 700, at: [191, 236] },
        { caption: "Select object, in the image bar", ms: 600, click: true },
        { caption: "Point at what you want", ms: 900, at: [240, 100] },
        { caption: "Click to pick it, on your computer", ms: 1400, click: true },
        { caption: "Isolate cuts it out", ms: 700, at: [257, 236] },
        { caption: "Isolate cuts it out", ms: 1900, click: true },
      ],
      html: `
        <div class="win canvas"></div>
        ${selected(`${PLANT}<svg class="ov" viewBox="0 0 260 146" width="260" height="146" preserveAspectRatio="xMidYMid slice">
          <defs>
            <filter id="hover" x="-5%" y="-5%" width="110%" height="110%"><feMorphology in="SourceAlpha" operator="dilate" radius="1.2" result="d"/><feComposite in="d" in2="SourceAlpha" operator="out" result="edge"/><feFlood class="fb" result="b"/><feComposite in="b" in2="edge" operator="in" result="line"/><feFlood class="fb18" result="f"/><feComposite in="f" in2="SourceAlpha" operator="in" result="fill"/><feMerge><feMergeNode in="fill"/><feMergeNode in="line"/></feMerge></filter>
            <filter id="picked" x="-5%" y="-5%" width="110%" height="110%"><feMorphology in="SourceAlpha" operator="dilate" radius="1.2" result="d"/><feComposite in="d" in2="SourceAlpha" operator="out" result="edge"/><feFlood flood-color="#fff" result="w"/><feComposite in="w" in2="edge" operator="in" result="line"/><feFlood class="fb30" result="f"/><feComposite in="f" in2="SourceAlpha" operator="in" result="fill"/><feMerge><feMergeNode in="fill"/><feMergeNode in="line"/></feMerge></filter>
          </defs>
          <use class="o-hover" href="#subject" filter="url(#hover)"/><use class="o-picked" href="#subject" filter="url(#picked)"/>
        </svg>`, "260 × 146")}
        <div class="bar image"><span class="b icon">${ICON.crop}</span><span class="div"></span><span class="b" style="width:102px">${ICON.pick}Select object</span><span class="b" style="width:134px">${ICON.cutout}Remove background</span></div>
        <div class="bar obj"><span class="seg"><span class="on">New</span><span>Add</span><span>Take away</span></span><span class="div"></span><span class="b" style="width:70px">${ICON.isolate}Isolate</span><span class="b" style="width:62px">${ICON.erase}Erase</span><span class="div"></span><span class="b done">Done</span></div>`,
      css: `
        .fb { flood-color: var(--brand); } .fb18 { flood-color: var(--brand); flood-opacity: .18; } .fb30 { flood-color: var(--brand); flood-opacity: .3; }
        .ov { position: absolute; inset: 0; }
        .ov use { opacity: 0; transition: opacity 120ms; }
        .at-3 .o-hover { opacity: 1; }
        .at-4 .o-hover { opacity: 0; }
        .at-4 .o-picked { opacity: 1; }
        .at-6 .o-picked { opacity: 0; }
        .backdrop { transition: opacity 400ms ${EASE}; }
        .at-6 .backdrop { opacity: 0; }
        .bar.obj { opacity: 0; }
        .at-2 .bar.image { opacity: 0; }
        .at-2 .bar.obj { opacity: 1; }`,
    },

    vectorize: {
      title: "Vectorize",
      steps: [
        { caption: "An image", ms: 1200, at: [300, 120] },
        { caption: "Vectorize, in the image bar", ms: 700, at: [329, 236] },
        { caption: "Traced into flat-colored shapes", ms: 1900, click: true },
        { caption: "Recolor it, like in grayscale", ms: 700, at: [151, 236] },
        { caption: "Recolor it, like in grayscale", ms: 1400, click: true },
        { caption: "or merge it down to fewer colors", ms: 700, at: [281, 236] },
        { caption: "or merge it down to fewer colors", ms: 1900, at: [268, 236], move: 600 },
      ],
      html: `
        <div class="win canvas"></div>
        ${selected(`<canvas class="orig" data-size="260x146" data-sharp="1"></canvas><canvas class="v6" data-size="260x146" data-sharp="1" data-paint="vector:6"></canvas><canvas class="g6" data-size="260x146" data-sharp="1" data-paint="vector:6:gray"></canvas><canvas class="g3" data-size="260x146" data-sharp="1" data-paint="vector:3:gray"></canvas>`, "260 × 146")}
        <div class="bar image"><span class="b icon">${ICON.crop}</span><span class="div"></span><span class="b" style="width:134px">${ICON.cutout}Remove background</span><span class="b" style="width:82px">${ICON.vectorize}Vectorize</span></div>
        <div class="bar rec"><span class="rt">Recolor</span><span class="modes"><span class="mode color on"><i></i></span><span class="mode gray"><i></i></span><span class="mode bw"><i></i></span></span><span class="div"></span><span class="count"><span class="n6">6</span><span class="n3">3</span></span><span class="slider"><i class="fill"></i><i class="thumb"></i></span><span class="div"></span><span class="b done primary">Done</span></div>`,
      css: `
        .v6, .g6, .g3 { opacity: 0; transition: opacity 250ms; }
        .at-2 .v6 { opacity: 1; }
        .at-4 .g6 { opacity: 1; }
        .at-6 .g3 { opacity: 1; transition-delay: 300ms; }
        .bar.rec { opacity: 0; }
        .at-2 .bar.image { opacity: 0; }
        .at-2 .bar.rec { opacity: 1; }
        .rt { padding: 0 4px; }
        .modes { display: flex; gap: 2px; }
        .mode { display: grid; place-items: center; width: 24px; height: 24px; border-radius: 5px; }
        .mode i { width: 14px; height: 14px; border-radius: 3px; box-shadow: inset 0 0 0 1px rgb(255 255 255 / .2); }
        .color i { background: conic-gradient(#ff3b30, #ffcc00, #34c759, #0a84ff, #af52de, #ff3b30); }
        .gray i { background: linear-gradient(135deg, #fff, #000); }
        .bw i { background: linear-gradient(135deg, #fff 50%, #000 50%); }
        .mode.on { background: var(--hover); box-shadow: inset 0 0 0 1px var(--border); }
        .at-4 .mode.color { background: none; box-shadow: none; }
        .at-4 .mode.gray { background: var(--hover); box-shadow: inset 0 0 0 1px var(--border); }
        .count { display: grid; width: 40px; height: 24px; padding: 0 8px; border-radius: 5px; background: var(--bg-secondary); line-height: 24px; }
        .count > span { grid-area: 1 / 1; }
        .n3 { opacity: 0; }
        .at-6 .n6 { opacity: 0; transition-delay: 300ms; } .at-6 .n3 { opacity: 1; transition-delay: 300ms; }
        .slider { position: relative; width: 96px; height: 16px; margin: 0 4px; }
        .slider::before { content: ""; position: absolute; left: 0; right: 0; top: 7px; height: 2px; border-radius: 1px; background: var(--border); }
        .slider .fill { position: absolute; left: 0; top: 7px; width: 27px; height: 2px; border-radius: 1px; background: var(--brand); transition: width 600ms ${EASE}; }
        .slider .thumb { position: absolute; left: 21px; top: 2px; width: 12px; height: 12px; border-radius: 50%; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / .35); transition: left 600ms ${EASE}; }
        .at-6 .slider .fill { width: 14px; } .at-6 .slider .thumb { left: 8px; }
        .b.primary { background: var(--brand); color: #fff; }`,
    },

    "remove-background": {
      title: "Remove background",
      steps: [
        { caption: "An image with a background", ms: 1300, at: [320, 120] },
        { caption: "Remove background, in the image bar", ms: 700, at: [260, 236] },
        { caption: "It runs on your computer", ms: 1500, click: true },
        { caption: "The original stays underneath, hidden", ms: 2100 },
      ],
      html: `
        <div class="win canvas"></div>
        ${selected(`${PLANT}<i class="shimmer"></i>`, "260 × 146")}
        <div class="bar image"><span class="b icon">${ICON.crop}</span><span class="div"></span><span class="b" style="width:134px">${ICON.cutout}Remove background</span></div>`,
      css: `
        .backdrop { transition: opacity 450ms ${EASE}; }
        .at-3 .backdrop { opacity: 0; }
        .shimmer { position: absolute; inset: 0; background: linear-gradient(100deg, transparent 35%, rgb(255 255 255 / .55) 50%, transparent 65%) no-repeat; background-size: 250% 100%; background-position: 150% 0; opacity: 0; }
        .at-2 .shimmer { opacity: 1; animation: demo-shimmer 1300ms linear; }
        .at-3 .shimmer { opacity: 0; }`,
    },

    "tab-previews": {
      title: "Tab previews",
      steps: [
        { caption: "Point at a tab", ms: 1000, at: [300, 200] },
        { caption: "Point at a tab", ms: 700, at: [125, 35] },
        { caption: "to see how it last looked", ms: 1800 },
        { caption: "Asleep tabs say so", ms: 700, at: [418, 35] },
        { caption: "Asleep tabs say so", ms: 1800 },
      ],
      html: `
        <div class="win">
          ${tabBar({ list: [{ kind: "design", title: "Brand kit", cls: "a" }, { kind: "design", title: "Onboarding flow", cls: "on" }, { kind: "board", title: "Team retro", cls: "b" }] })}
          <div class="view">${editor(
            [layer("frame", "Welcome", 0, "caretDown", "own"), layer("text", "Get started", 1), layer("image", "Hero", 1), layer("frame", "Sign up", 0, "caretRight")].join(""),
            `<span class="fl" style="left:24px;top:24px">Welcome</span><div class="fr phone" style="left:24px;top:42px"><b>Welcome</b><small>Plan, design and ship together</small><em>Get started</em></div>`,
          )}</div>
        </div>
        <div class="pv p1"><div class="pimg">${art("landing", 248, 140)}</div><div class="ptext"><b>Brand kit</b><span>Figma Design</span></div></div>
        <div class="pv p2"><div class="pimg">${art("board", 248, 140)}</div><div class="ptext"><b>Team retro</b><span>FigJam · Asleep</span></div></div>`,
      css: `
        .win { left: 16px; top: 16px; width: 520px; height: 300px; }
        .view { position: absolute; left: 0; right: 0; top: 38px; bottom: 0; }
        .t.b > *:not(.x) { opacity: .5; }
        .at-1 .t.a, .at-3 .t.b { background: var(--d-tab-hover, rgba(127, 127, 127, .14)); }
        .at-3 .t.a { background: none; }
        .pv { position: absolute; top: 58px; display: flex; flex-direction: column; gap: 8px; width: 264px; padding: 8px 8px 10px; border-radius: 13px; background: var(--bg); box-shadow: 0 0 0 .5px var(--border), 0 2px 5px rgba(0, 0, 0, .15), 0 6px 14px rgba(0, 0, 0, .2); opacity: 0; transform: translateY(-2px); transition: opacity 140ms, transform 140ms; }
        .p1 { left: 65px; } .p2 { left: 208px; }
        .pimg { overflow: hidden; border-radius: 6px; box-shadow: inset 0 0 0 1px var(--border); }
        .ptext { padding: 0 4px; }
        .ptext b { display: block; font-size: 13px; font-weight: 550; line-height: 20px; }
        .ptext span { color: var(--text-secondary); }
        .at-2 .p1 { opacity: 1; transform: none; }
        .at-3 .p1 { opacity: 0; }
        .at-3 .p2 { opacity: 1; transform: none; transition-delay: 600ms; }`,
    },

    "plugins-rail": {
      title: "Plugins in the left rail",
      steps: [
        { caption: "Your plugins, under Figma's own buttons", ms: 1500, at: [260, 200] },
        { caption: "Your plugins, under Figma's own buttons", ms: 1100, at: [40, 196] },
        { caption: "One click opens it", ms: 1800, click: true },
        { caption: "Right-click one for its menu", ms: 700, at: [40, 232] },
        { caption: "Right-click one for its menu", ms: 1700, click: true },
      ],
      html: `
        <div class="win">
          <div class="ed">
            <div class="rail">
              <span class="rb on">${ICON.layers}</span><span class="rb">${ICON.assets}</span><span class="rb">${ICON.find}</span>
              <i class="rsep"></i>
              <span class="rb"><i class="pl" style="--p:linear-gradient(135deg,#ff8a65,#f2416b)">${ICON.appearance}</i></span>
              <span class="rb p2b"><i class="pl" style="--p:linear-gradient(135deg,#8f7bff,#5b3df5)">${ICON.assets}</i></span>
              <span class="rb p3b"><i class="pl" style="--p:linear-gradient(135deg,#3dd68c,#0f9d63)">${ICON.images}</i></span>
              <span class="rb"><i class="pl" style="--p:linear-gradient(135deg,#ffd34d,#f5a524)">${ICON.textTool}</i></span>
            </div>
            <div class="lp"><div class="lh">Layers</div>${[layer("frame", "Home", 0, "caretDown", "own"), layer("text", "Title", 1), layer("rect", "Card", 1), layer("frame", "Profile", 0, "caretRight")].join("")}</div>
            <div class="cv"><span class="fl" style="left:24px;top:24px">Home</span><div class="fr phone" style="left:24px;top:42px"><b>Home</b><small>Today's picks</small><em>Open</em></div></div>
          </div>
        </div>
        <span class="tip">Icon Library</span>
        <div class="pw"><div class="pwh"><i class="pl sm" style="--p:linear-gradient(135deg,#8f7bff,#5b3df5)">${ICON.assets}</i><b>Icon Library</b>${ICON.close}</div><div class="pwb">${["home", "search", "star", "bell", "file", "lock", "plus", "check", "clock", "people", "images", "gear"].map((name) => `<span>${ICON[name]}</span>`).join("")}</div></div>
        <div class="cmenu"><span class="it">View details</span><span class="it">Remove</span></div>`,
      css: `
        .win { left: 16px; top: 16px; width: 560px; height: 300px; }
        .rail { display: flex; flex: none; flex-direction: column; align-items: center; gap: 4px; width: 48px; padding-top: 8px; border-right: 1px solid var(--border); background: var(--bg); }
        .rb { display: grid; place-items: center; width: 32px; height: 32px; border-radius: 5px; color: var(--text-secondary); }
        .rb.on { background: var(--bg-secondary); color: var(--text); }
        .rsep { width: 24px; height: 1px; margin: 4px 0; background: var(--border); }
        .pl { display: grid; place-items: center; width: 24px; height: 24px; border-radius: 6px; background: var(--p); color: #fff; }
        .pl svg { width: 14px; height: 14px; }
        .pl.sm { width: 16px; height: 16px; border-radius: 4px; }
        .pl.sm svg { width: 10px; height: 10px; }
        .at-1 .p2b, .at-4 .p3b { background: var(--hover); }
        .at-2 .p2b { background: var(--bg-secondary); }
        .at-3 .p2b { background: none; }
        .tip { position: absolute; left: 66px; top: 186px; padding: 2px 8px; border-radius: 5px; background: #1e1e1e; color: #fff; white-space: nowrap; opacity: 0; transition: opacity 100ms; }
        .at-1 .tip { opacity: 1; transition-delay: 800ms; }
        .at-2 .tip { opacity: 0; transition-delay: 0ms; }
        .pw { position: absolute; left: 250px; top: 40px; width: 220px; height: 210px; overflow: hidden; border-radius: 13px; background: var(--bg); box-shadow: 0 10px 28px rgba(0, 0, 0, .35), 0 0 0 .5px var(--border); opacity: 0; transform: scale(.97); transition: opacity 160ms, transform 160ms ${EASE}; }
        .pwh { display: flex; align-items: center; gap: 8px; height: 40px; padding: 0 12px; border-bottom: 1px solid var(--border); }
        .pwh b { flex: 1; font-weight: 550; }
        .pwh > svg { color: var(--text-secondary); }
        .pwb { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; padding: 12px; background: #fff; color: #1e1e1e; height: 100%; }
        .pwb span { display: grid; place-items: center; height: 36px; border-radius: 6px; background: #f5f5f5; }
        .at-2 .pw { opacity: 1; transform: none; }
        .cmenu { position: absolute; left: 44px; top: 236px; display: flex; flex-direction: column; min-width: 140px; padding: 8px; border-radius: 13px; background: #1e1e1e; color: #fff; box-shadow: 0 5px 17px rgba(0, 0, 0, .3); opacity: 0; transition: opacity 100ms; }
        .cmenu .it { display: flex; align-items: center; height: 24px; padding: 0 8px; border-radius: 5px; }
        .at-4 .cmenu { opacity: 1; }`,
    },

    themes: {
      title: "Color themes",
      size: [640, 356],
      steps: [
        { caption: "Themes, in Settings > Appearance", ms: 1400, at: [520, 300] },
        { caption: "Themes, in Settings > Appearance", ms: 700, at: [319, 228] },
        { caption: "Each colors Figma's interface", ms: 1900, click: true },
        { caption: "Nine, each with a light and a dark", ms: 700, at: [319, 320] },
        { caption: "Nine, each with a light and a dark", ms: 1900, click: true },
      ],
      html: `
        <div class="tw ted">
          <div class="ed">
            <div class="lp"><div class="lh">Layers</div>${[layer("frame", "Desktop", 0, "caretDown", "own picked"), layer("frame", "Nav", 1, "caretRight"), layer("text", "Ship the next thing", 1), layer("rect", "Get started", 1), layer("frame", "Footer", 1, "caretRight")].join("")}</div>
            <div class="cv"><span class="fl">Desktop</span><div class="fr">${art("landing", 232, 130)}</div></div>
          </div>
        </div>
        <div class="win setw">
          <div class="head">Settings<span class="shut">${ICON.close}</span></div>
          <nav class="snav"><div class="find">${ICON.search}<span class="q"><span class="ph">Search</span></span></div><span class="pg">${ICON.general}General</span><span class="pg on">${ICON.appearance}Appearance</span><span class="pg">${ICON.appIcon}App Icon</span><span class="pg">${ICON.tabs}Tabs &amp; Memory</span><i class="rule"></i><span class="pg">${ICON.editor}Editor</span><span class="pg">${ICON.images}Images</span></nav>
          <main class="tpage"><h5>Themes</h5><small class="lede">One for light mode, one for dark. They color Figma's interface, not the canvas</small>
            ${themeCard("figma", "Figma")}${themeCard("grove", "Grove")}${themeCard("lagoon", "Lagoon")}
          </main>
        </div>`,
      css: `
        .tw { position: absolute; left: 420px; top: 40px; width: 420px; height: 340px; overflow: hidden; border-radius: 10px; box-shadow: 0 16px 40px rgba(0, 0, 0, .3), 0 0 0 .5px rgba(127, 127, 127, .35); }
        .ted .lp { background: var(--t-bg); border-color: var(--t-line); transition: background-color 400ms ${EASE}; }
        .ted .cv { background: var(--t-panel); transition: background-color 400ms ${EASE}; }
        .ted .lr.picked { background: var(--t-soft); transition: background-color 400ms ${EASE}; }
        .ted .lr.picked .li { color: var(--text); }
        .ted .fl { left: 24px; top: 24px; color: var(--t-accent); }
        .ted .fr { left: 24px; top: 42px; box-shadow: 0 0 0 1px var(--t-accent); }
        .setw { left: 16px; top: 8px; width: 420px; height: 380px; }
        .head { position: relative; display: flex; align-items: center; height: 38px; padding-left: 16px; border-bottom: 1px solid var(--border); font-weight: 550; }
        .shut { position: absolute; right: 0; top: 0; display: grid; place-items: center; width: 46px; height: 38px; color: var(--text-secondary); }
        .tpage { position: absolute; left: 185px; top: 39px; right: 0; padding: 0 16px; }
        h5 { margin: 0; padding: 8px 0 4px; font-size: 11px; font-weight: 550; line-height: 16px; }
        .lede { display: block; margin-bottom: 8px; color: var(--text-secondary); font-size: 11px; }
        .theme { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 4px; height: 84px; margin-bottom: 8px; border-radius: 6px; box-shadow: inset 0 0 0 1px var(--border); color: var(--text-secondary); transition: color 160ms; }
        .orbs { display: flex; gap: 12px; }
        .orb { position: relative; width: 44px; height: 44px; overflow: hidden; border-radius: 50%; isolation: isolate; box-shadow: 0 0 0 1px rgba(127, 127, 127, .2); transition: box-shadow 160ms; }
        .orb::before { content: ""; position: absolute; inset: -18%; background: var(--orb); filter: blur(5px) saturate(1.2); }
        .figma { color: var(--text); font-weight: 550; }
        .figma .orb { box-shadow: 0 0 0 2px var(--bg), 0 0 0 4px var(--brand); }
        .at-2 .figma { color: var(--text-secondary); font-weight: 450; }
        .at-2 .figma .orb { box-shadow: 0 0 0 1px rgba(127, 127, 127, .2); }
        .at-2 .grove { color: var(--text); font-weight: 550; }
        .at-2 .grove .orb { box-shadow: 0 0 0 2px var(--bg), 0 0 0 4px var(--brand); }
        .at-4 .grove { color: var(--text-secondary); font-weight: 450; }
        .at-4 .grove .orb { box-shadow: 0 0 0 1px rgba(127, 127, 127, .2); }
        .at-4 .lagoon { color: var(--text); font-weight: 550; }
        .at-4 .lagoon .orb { box-shadow: 0 0 0 2px var(--bg), 0 0 0 4px var(--brand); }`,
    },

    discord: {
      title: "Discord status",
      steps: [
        { caption: "Your status on Discord", ms: 1800, at: [200, 220] },
        { caption: "Your status on Discord", ms: 700, at: [292, 35] },
        { caption: "It follows the file you're in", ms: 2200, click: true },
      ],
      html: `
        <div class="win">
          ${tabBar({ home: false, list: [{ kind: "design", title: "Onboarding flow", cls: "on a" }, { kind: "design", title: "Brand kit", cls: "b" }] })}
          <div class="view">${editor(
            [layer("frame", "Welcome", 0, "caretDown", "own"), layer("text", "Get started", 1), layer("image", "Hero", 1), layer("frame", "Sign up", 0, "caretRight")].join(""),
            "",
          )}</div>
        </div>
        <div class="dc">
          <div class="banner"></div>
          <div class="dav">C<i></i></div>
          <div class="dn"><b>Chris</b><span>chris</span></div>
          <div class="dsec">
            <h6>Playing</h6>
            <div class="act">
              <span class="app">${LOGO_COLOR}<i class="tool">${ICON.move}</i></span>
              <span class="lines"><b>Ligma</b><span class="swap"><span class="f1">Onboarding flow</span><span class="f2">Brand kit</span></span><span class="dim"><span class="f1">Welcome</span><span class="f2">Page 1</span></span><span class="dim">02:14 elapsed</span></span>
            </div>
          </div>
        </div>`,
      css: `
        .win { left: 16px; top: 16px; width: 380px; height: 300px; }
        .view { position: absolute; left: 0; right: 0; top: 38px; bottom: 0; }
        .at-2 .t.a { background: none; color: var(--d-tab-text); }
        .at-2 .t.a > svg { color: inherit; }
        .at-2 .t.a .x { visibility: hidden; }
        .at-2 .t.b { background: var(--bg); color: var(--text); }
        .at-2 .t.b > svg { color: var(--accent); }
        .at-2 .t.b .x { visibility: visible; }
        .dc { position: absolute; left: 292px; top: 64px; width: 176px; overflow: hidden; border-radius: 8px; background: #232428; color: #f2f3f5; box-shadow: 0 8px 24px rgba(0, 0, 0, .45), 0 0 0 1px rgba(255, 255, 255, .06); font-family: "gg sans", "Segoe UI", -apple-system, sans-serif; }
        .banner { height: 48px; background: linear-gradient(120deg, #5865f2, #9b6bff); }
        .dav { position: absolute; left: 12px; top: 20px; display: grid; place-items: center; width: 52px; height: 52px; border: 5px solid #232428; border-radius: 50%; background: #e8604c; color: #fff; font-size: 18px; font-weight: 600; }
        .dav i { position: absolute; right: -4px; bottom: -4px; width: 14px; height: 14px; border: 3px solid #232428; border-radius: 50%; background: #23a55a; }
        .dn { padding: 30px 12px 8px; }
        .dn b { display: block; font-size: 15px; font-weight: 700; line-height: 18px; }
        .dn span { color: #b5bac1; }
        .dsec { margin: 0 8px 8px; padding: 8px; border-radius: 6px; background: #111214; }
        h6 { margin: 0 0 6px; font-size: 11px; font-weight: 700; line-height: 14px; }
        .act { display: flex; gap: 8px; }
        .app { position: relative; display: grid; flex: none; place-items: center; width: 44px; height: 44px; border-radius: 8px; background: #fff; }
        .app > svg { width: 30px; height: 30px; }
        .tool { position: absolute; right: -5px; bottom: -5px; display: grid; place-items: center; width: 18px; height: 18px; border: 2px solid #111214; border-radius: 50%; background: #2c2c2c; color: #fff; }
        .tool svg { width: 10px; height: 10px; }
        .lines { display: flex; flex-direction: column; min-width: 0; font-size: 11px; line-height: 14px; }
        .lines b { font-weight: 600; }
        .lines .dim { color: #b5bac1; }
        .swap, .lines .dim:nth-of-type(2) { display: grid; }
        .swap > span, .dim > span { grid-area: 1 / 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; transition: opacity 250ms; }
        .f2 { opacity: 0; }
        .at-2 .f1 { opacity: 0; transition-delay: 500ms; }
        .at-2 .f2 { opacity: 1; transition-delay: 500ms; }`,
    },
  };

  // Shared by every scene and the carousel
  const STYLE = `
    .demos {
      -webkit-user-select: none;
      user-select: none;
      --demo-ease: ${EASE};
      --d-canvas: #1e1e1e; --d-sel: #24425c; --d-menu: #1e1e1e; --d-menu-line: #383838; --d-text3: rgba(255, 255, 255, .4);
      --d-switch: rgba(255, 255, 255, .2); --d-shadow: 0 2px 5px rgba(0, 0, 0, .3), 0 10px 24px rgba(0, 0, 0, .4); --d-tab-text: #b3b3b3; --d-tab-divider: #4d4d4d; --d-tab-hover: #424242;
      position: relative;
    }
    .demos[data-mode="light"] {
      --d-canvas: #f5f5f5; --d-sel: #e5f4ff; --d-menu: #ffffff; --d-menu-line: #e6e6e6; --d-text3: rgba(0, 0, 0, .3);
      --d-switch: rgba(0, 0, 0, .18); --d-shadow: 0 2px 5px rgba(0, 0, 0, .08), 0 10px 24px rgba(0, 0, 0, .12); --d-tab-text: rgba(0, 0, 0, .55); --d-tab-divider: #d9d9d9; --d-tab-hover: #e6e6e6;
    }
    .demo-stage { position: relative; aspect-ratio: 9 / 5; overflow: hidden; border-radius: 8px; isolation: isolate; background: var(--bg-secondary); box-shadow: inset 0 0 0 1px rgba(127, 127, 127, .14); }
    .demo-mesh { position: absolute; inset: -25%; background: var(--demo-mesh, transparent); filter: blur(26px) saturate(1.15); animation: demo-drift 16s ease-in-out infinite alternate; }
    .demo-view { position: absolute; inset: 0; transition: opacity 240ms var(--demo-ease); }
    .demo-view.fading { opacity: 0; }

    .demo-scene { position: absolute; left: 0; top: 0; transform-origin: 0 0; color: var(--text); font: 450 11px/16px "Inter", -apple-system, BlinkMacSystemFont, "Segoe UI Variable Text", "Segoe UI", sans-serif; font-feature-settings: "cv11", "calt"; letter-spacing: .005em; pointer-events: none; -webkit-font-smoothing: antialiased; }
    .demo-scene * { box-sizing: border-box; }
    .demo-scene.reset, .demo-scene.reset * { transition: none !important; }
    .demo-scene .win { position: absolute; overflow: hidden; border-radius: 10px; background: var(--bg); box-shadow: 0 16px 40px rgba(0, 0, 0, .3), 0 2px 8px rgba(0, 0, 0, .14), 0 0 0 .5px rgba(127, 127, 127, .35); }
    .demo-scene .win.canvas { left: 20px; top: 20px; width: 540px; height: 300px; background: var(--d-canvas); }
    .demo-scene svg { display: block; flex: none; }

    /* The tab bar (index.html) */
    .demo-scene .tb { display: flex; height: 38px; background: var(--bg-secondary); color: var(--d-tab-text); font: 500 12px/1 -apple-system, BlinkMacSystemFont, "Segoe UI Variable Text", "Segoe UI", "Inter", sans-serif; letter-spacing: 0; }
    .demo-scene .t { position: relative; display: flex; flex: none; align-items: center; gap: 8px; padding: 0 8px 0 12px; white-space: nowrap; overflow: hidden; }
    .demo-scene .t::after { content: ""; position: absolute; right: 0; top: 11px; bottom: 11px; width: 1px; background: var(--d-tab-divider); }
    .demo-scene .t.home { justify-content: center; width: 49px; padding: 0; }
    .demo-scene .t.home::after { top: 0; bottom: 0; }
    .demo-scene .t.new { justify-content: center; width: 36px; padding: 0; }
    .demo-scene .t.new::after, .demo-scene .t.on::after, .demo-scene .t.before::after { display: none; }
    .demo-scene .t.on, .demo-scene .t.home.on { background: var(--bg); color: var(--text); }
    .demo-scene .t.on > svg, .demo-scene .t.on .ico { color: var(--accent); }
    .demo-scene .t .title { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
    .demo-scene .t .x { display: grid; flex: none; place-items: center; width: 20px; height: 20px; margin-left: auto; visibility: hidden; }
    .demo-scene .t.on .x { visibility: visible; }
    .demo-scene .t:has(+ .t.on)::after { display: none; }

    /* Figma's editor */
    .demo-scene .ed { display: flex; height: 100%; }
    .demo-scene .lp { flex: none; width: 240px; border-right: 1px solid var(--border); background: var(--bg); }
    .demo-scene .lh { display: flex; align-items: center; height: 40px; padding: 0 16px; font-weight: 550; }
    .demo-scene .lr { position: relative; display: flex; align-items: center; height: 32px; padding-left: calc(16px + var(--level) * 16px); }
    .demo-scene .lr .caret { display: grid; flex: none; place-items: center; width: 16px; height: 16px; color: var(--text-secondary); }
    .demo-scene .lr .li { display: grid; flex: none; place-items: center; width: 16px; height: 16px; margin-right: 8px; color: var(--text-secondary); }
    .demo-scene .lr .guide { position: absolute; left: 23px; top: 0; bottom: 0; width: 2px; background: var(--c, transparent); }
    .demo-scene .lr[style*="--level:0"] .guide { display: none; }
    .demo-scene .lr.own .guide { display: block; top: 22px; }
    .demo-scene .cv { position: relative; flex: 1; background: var(--d-canvas); }
    .demo-scene .fl { position: absolute; color: var(--text-secondary); white-space: nowrap; }
    .demo-scene .fr { position: absolute; overflow: hidden; background: #fff; }
    .demo-scene .fr.pricing { display: flex; flex-direction: column; gap: 10px; padding: 14px 16px; color: #1e1a3a; }
    .demo-scene .fr.pricing b { font-size: 14px; font-weight: 700; line-height: 18px; }
    .demo-scene .fr.pricing span { display: flex; gap: 8px; }
    .demo-scene .fr.pricing i { flex: 1; padding: 8px; border-radius: 6px; background: #f1eeff; font-style: normal; font-weight: 600; }
    .demo-scene .fr.phone { display: flex; flex-direction: column; justify-content: flex-end; gap: 4px; width: 120px; height: 210px; padding: 14px; color: #1e1a3a; background: linear-gradient(#f1eeff, #fff 55%); }
    .demo-scene .fr.phone b { font-size: 16px; font-weight: 700; line-height: 20px; }
    .demo-scene .fr.phone small { color: #6b6880; font-size: 10px; line-height: 13px; }
    .demo-scene .fr.phone em { margin-top: 6px; padding: 6px 0; border-radius: 6px; background: #6a5cff; color: #fff; font-style: normal; font-weight: 600; text-align: center; }
    .demo-color-frames .g0 { --c: oklch(.8 .15 25); } .demo-color-frames .g1 { --c: oklch(.8 .15 60); } .demo-color-frames .g2 { --c: oklch(.8 .15 95); }
    .demo-color-frames.at-3 .g0 { --c: oklch(.83 .15 345); } .demo-color-frames.at-3 .g1 { --c: oklch(.74 .15 18); } .demo-color-frames.at-3 .g2 { --c: oklch(.83 .15 52); }
    .demo-color-frames[data-mode="light"] .g0 { --c: oklch(.62 .17 25); } .demo-color-frames[data-mode="light"] .g1 { --c: oklch(.62 .17 60); } .demo-color-frames[data-mode="light"] .g2 { --c: oklch(.62 .17 95); }
    .demo-color-frames.at-3[data-mode="light"] .g0 { --c: oklch(.65 .17 345); } .demo-color-frames.at-3[data-mode="light"] .g1 { --c: oklch(.56 .17 18); } .demo-color-frames.at-3[data-mode="light"] .g2 { --c: oklch(.65 .17 52); }

    /* A selected image and the image bar */
    .demo-scene .pic { position: absolute; left: 110px; top: 46px; width: 260px; height: 146px; }
    .demo-scene .pic canvas { position: absolute; inset: 0; display: block; width: 100%; height: 100%; }
    .demo-scene .sel { position: absolute; inset: 0; box-shadow: 0 0 0 1px var(--brand); }
    .demo-scene .sel b { position: absolute; width: 8px; height: 8px; margin: -4px; border: 1px solid var(--brand); border-radius: 2px; background: #fff; }
    .demo-scene .sel b:nth-child(1) { left: 0; top: 0; } .demo-scene .sel b:nth-child(2) { right: 0; top: 0; margin-right: -4px; }
    .demo-scene .sel b:nth-child(3) { left: 0; bottom: 0; margin-bottom: -4px; } .demo-scene .sel b:nth-child(4) { right: 0; bottom: 0; margin: 0 -4px -4px 0; }
    .demo-scene .dims { position: absolute; left: 50%; top: calc(100% + 8px); padding: 0 4px; border-radius: 3px; background: var(--brand); color: #fff; font-variant-numeric: tabular-nums; white-space: nowrap; transform: translateX(-50%); }
    .demo-scene .bar { position: absolute; left: 240px; top: 216px; display: flex; align-items: center; gap: 4px; height: 40px; padding: 8px; border-radius: 13px; background: var(--bg); color: var(--text); box-shadow: 0 3px 8px rgb(0 0 0 / .35), 0 0 0 .5px rgba(127, 127, 127, .3); font-weight: 500; white-space: nowrap; transform: translateX(-50%); transition: opacity 120ms; }
    .demo-scene .bar .b { display: flex; flex: none; align-items: center; height: 24px; padding-right: 8px; border-radius: 5px; }
    .demo-scene .bar .b svg { margin: 4px; }
    .demo-scene .bar .b.icon { justify-content: center; width: 24px; padding: 0; }
    .demo-scene .bar .b.done { padding: 0 8px; }
    .demo-scene .bar .div { align-self: stretch; width: 1px; margin: 0 4px; background: var(--border); }
    .demo-scene .bar .hint { padding: 0 8px 0 4px; color: var(--text-secondary); }
    .demo-scene .bar .seg { display: flex; padding: 2px; border-radius: 6px; background: var(--bg-secondary); }
    .demo-scene .bar .seg span { height: 20px; padding: 0 8px; border-radius: 4px; color: var(--text-secondary); line-height: 20px; font-variant-numeric: tabular-nums; transition: background-color 120ms, color 120ms; }
    .demo-scene .bar .seg .on { background: var(--bg); color: var(--text); box-shadow: 0 0 0 1px var(--border); }
    .demo-scene .bar .field { display: flex; align-items: center; gap: 6px; padding-left: 4px; color: var(--text-secondary); }
    .demo-scene .bar .field i { width: 40px; height: 24px; padding: 0 6px; border-radius: 5px; background: var(--bg-secondary); color: var(--text); font-style: normal; line-height: 24px; }

    /* Ligma's home page (home.html) */
    .demo-scene .home { display: flex; font-size: 13px; line-height: 20px; }
    .demo-scene .home aside { flex: none; width: 240px; border-right: 1px solid var(--border); background: var(--bg); }
    .demo-scene .acct { display: flex; align-items: center; gap: 8px; height: 48px; padding: 0 16px; border-bottom: 1px solid var(--border); font-weight: 550; transition: background-color 120ms; }
    .demo-scene .acct b { flex: 1; font-weight: 550; }
    .demo-scene .acct > svg { color: var(--text-secondary); }
    .demo-scene .bell { position: relative; display: grid; place-items: center; width: 28px; height: 28px; margin-left: 8px; color: var(--text-secondary); }
    .demo-scene .bell em { position: absolute; right: 1px; top: 0; min-width: 14px; height: 14px; padding: 0 3px; border-radius: 7px; background: var(--brand); color: #fff; font: 600 9px/14px "Inter", "Segoe UI", sans-serif; font-style: normal; text-align: center; }
    .demo-scene .av { display: grid; flex: none; place-items: center; width: 24px; height: 24px; border-radius: 50%; background: var(--av); color: #fff; font: 600 11px/1 "Inter", "Segoe UI", sans-serif; font-style: normal; }
    .demo-scene .tree { padding: 8px 0; }
    .demo-scene .nav { display: flex; align-items: center; gap: 8px; height: 32px; margin: 0 8px; padding: 0 8px; border-radius: 5px; white-space: nowrap; }
    .demo-scene .nav svg { color: var(--text-secondary); }
    .demo-scene .nav small { margin-left: auto; color: var(--text-secondary); font-size: 11px; }
    .demo-scene .nav.on { background: var(--d-sel); font-weight: 550; }
    .demo-scene .nav.on svg { color: var(--text); }
    .demo-scene .nav.find { color: var(--text-secondary); }
    .demo-scene kbd { display: inline-flex; align-items: center; height: 16px; margin-left: auto; padding: 0 4px; border-radius: 4px; background: var(--bg); box-shadow: inset 0 0 0 1px var(--border), inset 0 -1px 0 var(--border); color: var(--text-secondary); font: 450 11px/16px "Inter", "Segoe UI", sans-serif; }
    .demo-scene .home .main { position: relative; flex: 1; min-width: 0; padding: 0 0 0 32px; }
    .demo-scene .home h4 { margin: 0; padding: 14px 0; font-size: 13px; font-weight: 550; line-height: 20px; }
    .demo-scene .chips { display: inline-flex; gap: 2px; padding: 2px; border-radius: 6px; background: var(--bg-secondary); font-size: 11px; line-height: 24px; }
    .demo-scene .chips span { padding: 0 8px; border-radius: 4px; color: var(--text-secondary); }
    .demo-scene .chips .on { background: var(--bg); color: var(--text); }
    .demo-scene .home h5 { display: flex; align-items: center; gap: 6px; height: 36px; margin: 12px 0 0 -6px; font-size: 13px; font-weight: 550; line-height: 20px; }
    .demo-scene .home h5 > svg { color: var(--text-secondary); }
    .demo-scene .home h5 small { color: var(--text-secondary); font-size: 11px; font-weight: 450; }
    .demo-scene .home h5 .all { display: flex; align-items: center; gap: 2px; margin: 0 24px 0 auto; color: var(--text-secondary); font-size: 11px; font-weight: 450; }
    .demo-scene .home .cards { display: flex; gap: 20px; margin-top: 8px; }
    .demo-scene .thumb { position: relative; flex: none; overflow: hidden; border-radius: 6px; background: var(--d-canvas); }
    .demo-scene .thumb::after { content: ""; position: absolute; inset: 0; border-radius: inherit; box-shadow: inset 0 0 0 1px var(--border); }
    .demo-scene .pin { position: absolute; right: 6px; top: 6px; z-index: 1; display: grid; place-items: center; width: 22px; height: 22px; border-radius: 5px; background: var(--bg); color: #ffc107; }
    .demo-scene .pin svg { fill: #ffc107; stroke: #e0a400; width: 14px; height: 14px; }
    .demo-scene .cn { display: flex; align-items: center; gap: 8px; margin-top: 8px; font-weight: 550; white-space: nowrap; }
    .demo-scene .cm { padding-left: 24px; color: var(--text-secondary); font-size: 11px; line-height: 16px; white-space: nowrap; }
    .demo-scene .g { display: grid; flex: none; place-items: center; width: 16px; height: 16px; border-radius: 4px; background: #0d99ff; color: #fff; }
    .demo-scene .g.board { background: #9747ff; }
    .demo-scene .g svg { width: 12px; height: 12px; }
    .demo-scene .faces { position: relative; z-index: 1; display: flex; justify-content: flex-end; height: 0; padding: 0 8px; top: -32px; }
    .demo-scene .face { display: grid; place-items: center; width: 24px; height: 24px; border-radius: 50%; background: var(--av); box-shadow: 0 0 0 2px var(--bg); color: #fff; font: 600 10px/1 "Inter", "Segoe UI", sans-serif; font-style: normal; }
    .demo-scene .msep { display: block; height: 1px; margin: 8px -8px; background: var(--d-menu-line); }
    .demo-scene .mi { display: flex; align-items: center; height: 24px; padding: 0 16px 0 8px; border-radius: 5px; font-size: 11px; line-height: 16px; white-space: nowrap; }
    .demo-scene .mic { display: grid; place-items: center; width: 24px; height: 24px; margin: 0 4px 0 -4px; }
    .demo-scene .mi .arrow { margin-left: auto; opacity: .55; }
    .demo-scene .acard { display: flex; flex-direction: column; align-items: center; gap: 2px; padding: 12px 8px 4px; font-size: 11px; line-height: 16px; text-align: center; }
    .demo-scene .acard .av { width: 40px; height: 40px; margin-bottom: 6px; font-size: 16px; }
    .demo-scene .acard b { font-weight: 600; }
    .demo-scene .acard span { color: var(--text-secondary); }
    .demo-scene .blur { filter: blur(4px); transition: filter 160ms cubic-bezier(.16, 1, .3, 1); }
    .demo-scene .ah { display: flex; align-items: center; gap: 8px; height: 48px; padding: 0 8px 0 16px; border-bottom: 1px solid var(--border); font-size: 11px; }
    .demo-scene .ah b { font-size: 13px; font-weight: 550; line-height: 20px; }
    .demo-scene .ah > .blur { color: var(--text-secondary); }
    .demo-scene .ah .x { display: grid; place-items: center; width: 28px; height: 28px; margin-left: auto; color: var(--text-secondary); }
    .demo-scene .ab { display: grid; grid-template-columns: 184px minmax(0, 1fr); height: calc(100% - 48px); }
    .demo-scene .an { display: flex; flex-direction: column; gap: 2px; padding: 8px 0; border-right: 1px solid var(--border); font-size: 13px; line-height: 20px; }
    .demo-scene .an .nav.on { box-shadow: inset 0 0 0 1px var(--brand); }
    .demo-scene .ac { padding: 4px 24px; font-size: 11px; line-height: 16px; }
    .demo-scene .aprof { display: flex; align-items: center; gap: 16px; padding: 20px 0 16px; border-bottom: 1px solid var(--border); }
    .demo-scene .av.big { width: 56px; height: 56px; font-size: 22px; }
    .demo-scene .id { display: flex; flex: 1; flex-direction: column; min-width: 0; }
    .demo-scene .id b { font-size: 13px; font-weight: 600; line-height: 20px; }
    .demo-scene .id span { align-self: flex-start; color: var(--text-secondary); }
    .demo-scene .btn { display: inline-flex; flex: none; align-items: center; gap: 4px; height: 28px; padding: 0 10px; border-radius: 5px; box-shadow: inset 0 0 0 1px var(--border); font-weight: 550; white-space: nowrap; }
    .demo-scene .srow { display: flex; align-items: center; gap: 16px; min-height: 56px; padding: 10px 0; border-bottom: 1px solid var(--border); }
    .demo-scene .sl { display: flex; flex: 1; flex-direction: column; align-items: flex-start; min-width: 0; }
    .demo-scene .sl small { color: var(--text-secondary); font-size: 11px; }
    .demo-scene .sl span { font-size: 13px; line-height: 20px; }

    /* Settings' switch */
    .demo-scene .sw { position: relative; flex: none; width: 28px; height: 16px; border-radius: 8px; background: var(--d-switch); transition: background-color 120ms ease-out, opacity 200ms; }
    .demo-scene .sw::before { content: ""; position: absolute; left: 2px; top: 2px; width: 12px; height: 12px; border-radius: 50%; background: #fff; box-shadow: 0 1px 2px rgba(0, 0, 0, .25); transition: transform 160ms cubic-bezier(.2, .9, .3, 1); }
    .demo-scene .sw.on { background: var(--brand); }
    .demo-scene .sw.on::before { transform: translateX(12px); }

    /* The loading screen's logo (loading_logo.rs) */
    .demo-scene .logo { position: relative; width: 22px; height: 32px; }
    .demo-scene .logo div { position: absolute; }
    .demo-scene .logo :is(.top, .middle, .cap, .span, .rest, .end, .divide, .boundary) { animation: 4s infinite; }
    .demo-scene .logo :is(.cap, .span, .rest, .end, .divide, .boundary) { animation-timing-function: cubic-bezier(.12, 0, .39, 0); }
    .demo-scene .logo :is(.top, .middle) { width: 12px; height: 12px; border: 2px solid currentColor; animation-timing-function: cubic-bezier(.33, 1, .68, 1); }
    .demo-scene .logo .top { left: 0; top: 0; border-radius: 6px 6px 0 6px; transform-origin: right bottom; animation-name: demo-logo-top; }
    .demo-scene .logo .middle { left: 0; top: 10px; border-radius: 6px 0 0 6px; transform-origin: right center; animation-name: demo-logo-middle; }
    .demo-scene .logo .row { left: 0; top: 20px; width: 22px; height: 12px; }
    .demo-scene .logo :is(.cap, .arc) { width: 6px; height: 12px; border: 2px solid currentColor; border-right: 0; border-radius: 6px 0 0 6px; }
    .demo-scene .logo .cap { left: 0; animation-name: demo-logo-cap; }
    .demo-scene .logo :is(.span, .rest) { left: 6px; height: 12px; border-block: 2px solid currentColor; }
    .demo-scene .logo .span { width: 10px; opacity: 0; animation-name: demo-logo-span; }
    .demo-scene .logo .rest { width: 6px; animation-name: demo-logo-rest; }
    .demo-scene .logo .end { left: 16px; width: 6px; height: 12px; border: 2px solid currentColor; border-left: 0; border-radius: 0 6px 6px 0; animation-name: demo-logo-end; }
    .demo-scene .logo .divide { left: 0; width: 22px; height: 12px; border-radius: 6px; overflow: hidden; animation-name: demo-logo-divide; }
    .demo-scene .logo .boundary { left: 0; width: 22px; height: 12px; animation-name: demo-logo-boundary; }
    .demo-scene .logo .line { left: 10px; width: 2px; height: 12px; background: currentColor; }
    .demo-scene .logo .arc { left: 10px; }
    @keyframes demo-logo-cap { 0%, 10%, 20%, 80%, 90%, 100% { transform: translateX(5px); } 5%, 15%, 25%, 75%, 85%, 95% { transform: translateX(0); } }
    @keyframes demo-logo-end { 0%, 10%, 20%, 80%, 90%, 100% { transform: translateX(-5px); } 5%, 15%, 25%, 75%, 85%, 95% { transform: translateX(0); } }
    @keyframes demo-logo-divide { 0%, 10%, 20%, 80%, 90%, 100% { transform: scaleX(.5); } 5%, 15%, 25%, 75%, 85%, 95% { transform: scaleX(1); } }
    @keyframes demo-logo-span { 0%, 10%, 20%, 80%, 90%, 100% { transform: scaleX(.1); } 5%, 15%, 25%, 75%, 85%, 95% { transform: scaleX(1); } 0%, 25%, 75%, 100% { opacity: 1; } 25.1%, 74.9% { opacity: 0; } }
    @keyframes demo-logo-rest { 0%, 25%, 75%, 100% { opacity: 0; } 25.1%, 74.9% { opacity: 1; } }
    @keyframes demo-logo-boundary { 0%, 10.1%, 20.1%, 80.1%, 90.1% { transform: translateX(-9px); } 5%, 15%, 25%, 75%, 85%, 95% { transform: translateX(0); } 10%, 20%, 80%, 90%, 100% { transform: translateX(9px); } }
    @keyframes demo-logo-middle { 0%, 22%, 78%, 100% { transform: scaleX(.01); opacity: 0; } 22.1%, 77.9% { opacity: 1; } 28% { transform: scaleX(1); } 72% { transform: scaleX(1); animation-timing-function: cubic-bezier(.32, 0, .67, 0); } }
    @keyframes demo-logo-top { 0%, 26%, 74%, 100% { transform: scale(.01); opacity: 0; } 26.1%, 73.9% { opacity: 1; } 32% { transform: scale(1); } 68% { transform: scale(1); animation-timing-function: cubic-bezier(.32, 0, .67, 0); } }

    /* The sample files */
    .demo-scene .art { position: relative; overflow: hidden; }
    .demo-scene .art > div { position: absolute; left: 0; top: 0; width: 400px; height: 225px; transform-origin: 0 0; }
    .demo-scene .landing { background: #f4f1ff; color: #1e1a3a; font-family: "Inter", "Segoe UI", sans-serif; }
    .demo-scene .landing .lg { position: absolute; left: 24px; top: 20px; width: 16px; height: 16px; border-radius: 4px; background: #6a5cff; }
    .demo-scene .landing .brand { position: absolute; left: 48px; top: 18px; font-size: 14px; font-weight: 600; line-height: 20px; }
    .demo-scene .landing .links { position: absolute; left: 220px; top: 20px; display: flex; gap: 14px; color: #5b5872; font-size: 10px; }
    .demo-scene .landing .cta { position: absolute; right: 24px; top: 14px; padding: 5px 10px; border-radius: 6px; background: #6a5cff; color: #fff; font-size: 10px; font-style: normal; font-weight: 600; }
    .demo-scene .landing h4 { position: absolute; left: 24px; top: 62px; margin: 0; font-size: 30px; font-weight: 700; letter-spacing: -.02em; line-height: 32px; }
    .demo-scene .landing p { position: absolute; left: 24px; top: 136px; margin: 0; color: #6b6880; font-size: 10px; }
    .demo-scene .landing .go { position: absolute; left: 24px; top: 162px; padding: 7px 14px; border-radius: 6px; background: #6a5cff; color: #fff; font-size: 10px; font-style: normal; font-weight: 600; }
    .demo-scene .landing :is(.c1, .c2, .c3) { position: absolute; width: 120px; height: 120px; border-radius: 50%; mix-blend-mode: multiply; }
    .demo-scene .landing .c1 { left: 214px; top: 64px; background: #8b7dff; }
    .demo-scene .landing .c2 { left: 272px; top: 64px; background: #ffa27f; }
    .demo-scene .landing .c3 { left: 244px; top: 132px; background: #6fd9b0; }
    .demo-scene .icons { display: grid; grid-template-columns: repeat(8, 1fr); align-content: center; gap: 18px 0; padding: 0 28px; background: #fff; color: #1e1e1e; }
    .demo-scene .icons span { display: grid; place-items: center; }
    .demo-scene .bank { display: flex; gap: 14px; padding: 18px 22px; background: #1d1a33; font-family: "Inter", "Segoe UI", sans-serif; }
    .demo-scene .bank .phone { display: flex; flex: 1; flex-direction: column; gap: 8px; padding: 12px; border-radius: 8px; background: #2a2648; color: #fff; font-size: 9px; }
    .demo-scene .bank .phone b { margin: -12px -12px 2px; padding: 10px 12px; border-radius: 8px 8px 0 0; background: #6a5cff; font-size: 11px; }
    .demo-scene .bank .phone span { display: flex; align-items: center; gap: 6px; }
    .demo-scene .bank .phone span i { width: 10px; height: 10px; border-radius: 50%; background: hsl(var(--h) 70% 65%); }
    .demo-scene .bank .phone span em { margin-left: auto; font-style: normal; opacity: .7; }
    .demo-scene .bank .pay { margin-top: auto; padding: 5px; border-radius: 5px; background: #6a5cff; font-style: normal; text-align: center; }
    .demo-scene .board { background: #f7f3ea radial-gradient(#d8d2c4 1px, transparent 1.4px) 0 0 / 16px 16px; }
    .demo-scene .board .sticky { position: absolute; width: 96px; height: 96px; padding: 10px; background: var(--s); box-shadow: 0 2px 4px rgba(0, 0, 0, .1); color: #1e1e1e; font-size: 13px; font-style: normal; font-weight: 500; }
    .demo-scene .board .tag { position: absolute; left: 40px; top: 18px; padding: 3px 8px; border-radius: 5px; background: #9747ff; color: #fff; font-size: 11px; }

    /* Settings' sidebar and search (settings.html) */
    .demo-scene .snav { position: absolute; left: 0; top: 39px; bottom: 0; display: flex; flex-direction: column; gap: 2px; width: 184px; padding: 8px; border-right: 1px solid var(--border); }
    .demo-scene .snav .find { margin-bottom: 6px; }
    .demo-scene .snav .pg { display: flex; flex: none; align-items: center; gap: 8px; height: 28px; padding: 0 8px; border-radius: 5px; color: var(--text-secondary); white-space: nowrap; }
    .demo-scene .snav .pg.on { background: var(--bg-secondary); color: var(--text); font-weight: 550; }
    .demo-scene .snav .rule { flex: none; height: 1px; margin: 4px 8px; background: var(--border); }
    .demo-scene .find { display: flex; flex: none; align-items: center; height: 28px; border: 1px solid transparent; border-radius: 5px; background: var(--bg-secondary); color: var(--text-secondary); transition: border-color 160ms; }
    .demo-scene .find > svg { margin-left: 5px; }
    .demo-scene .find .q { display: flex; flex: 1; align-items: center; min-width: 0; height: 100%; padding: 0 4px; white-space: nowrap; }
    .demo-scene .find .typed { display: flex; color: var(--text); }
    .demo-scene .find .typed b { display: inline-block; max-width: 0; overflow: hidden; font-weight: 450; }
    .demo-scene .find .caret { flex: none; width: 1px; height: 14px; background: var(--brand); opacity: 0; }
    .demo-scene .find .clear { display: grid; flex: none; place-items: center; width: 22px; height: 22px; margin-right: 2px; opacity: 0; transition: opacity 160ms; }

    /* Figma's bottom toolbar */
    .demo-scene .toolbar { position: absolute; left: 240px; top: 216px; display: flex; align-items: center; height: 40px; padding: 4px; border-radius: 13px; background: var(--bg); box-shadow: 0 3px 8px rgb(0 0 0 / .35), 0 0 0 .5px rgba(127, 127, 127, .3); transform: translateX(-50%); }
    .demo-scene .toolbar .tool { display: grid; place-items: center; width: 32px; height: 32px; border-radius: 5px; transition: background-color 120ms, color 120ms; }
    .demo-scene .toolbar .tool.on { background: var(--brand); color: #fff; }
    .demo-scene .toolbar .tool > span { display: grid; place-items: center; }
    .demo-scene .toolbar .more { display: grid; place-items: center; width: 16px; height: 32px; color: var(--text-secondary); }
    @keyframes demo-spin { to { transform: rotate(360deg); } }
    @keyframes demo-shimmer { from { background-position: 150% 0; } to { background-position: -50% 0; } }

    /* The pointer and its click */
    .demo-pointer { position: absolute; left: 0; top: 0; z-index: 10; transition-property: transform; transition-timing-function: var(--demo-ease); }
    .demo-pointer svg { position: relative; filter: drop-shadow(0 1px 1px rgba(0, 0, 0, .3)); transition: opacity 120ms; }
    .demo-brush { position: absolute; left: -12px; top: -12px; width: 24px; height: 24px; border-radius: 50%; box-shadow: 0 0 0 1.5px #fff, 0 0 2px 1.5px rgba(0, 0, 0, .6); opacity: 0; transition: opacity 120ms; }
    .demo-ripple { position: absolute; left: -9px; top: -9px; width: 18px; height: 18px; border-radius: 50%; background: color-mix(in srgb, var(--brand) 45%, transparent); opacity: 0; }
    .demo-scene[data-click] .demo-ripple { animation: demo-ripple 420ms var(--demo-ease); }
    .demo-scene[data-click] .demo-pointer svg { animation: demo-press 260ms var(--demo-ease); }

    /* Under the scene: the feature and what's happening, then the dots */
    .demo-caption { display: flex; gap: 12px; align-items: baseline; justify-content: space-between; min-height: 16px; padding: 8px 2px 0; font-size: 11px; line-height: 16px; }
    .demo-caption .demo-title { flex: none; font-weight: 550; }
    .demo-caption .demo-step { overflow: hidden; color: var(--text-secondary); text-align: right; white-space: nowrap; text-overflow: ellipsis; animation: demo-caption 320ms var(--demo-ease); }
    .demos .demo-arrow { position: absolute; top: calc(50% - 12px); z-index: 3; display: grid; place-items: center; width: 24px; min-width: 0; height: 24px; padding: 0; border: 0; border-radius: 50%; background: rgba(0, 0, 0, .42); color: #fff; opacity: 0; backdrop-filter: blur(8px); -webkit-backdrop-filter: blur(8px); transition: opacity 160ms var(--demo-ease), background-color 120ms; }
    .demos .demo-arrow:hover { background: rgba(0, 0, 0, .6); }
    .demos .demo-arrow:active { background: rgba(0, 0, 0, .7); }
    .demos .demo-prev { left: 8px; }
    .demos .demo-next { right: 8px; }
    .demos:hover .demo-arrow, .demos:focus-within .demo-arrow { opacity: 1; }
    .demo-dots { display: flex; justify-content: center; padding-top: 4px; }
    .demos .demo-dot { display: grid; place-items: center; width: auto; min-width: 0; height: 14px; padding: 0 2px; border: 0; border-radius: 4px; background: none; }
    .demos .demo-dot::before { content: ""; width: 6px; height: 6px; border-radius: 3px; background: var(--text); opacity: .22; transition: width 240ms var(--demo-ease), opacity 160ms; }
    .demos .demo-dot:hover { background: none; }
    .demos .demo-dot:hover::before { opacity: .45; }
    .demos .demo-dot[aria-selected="true"]::before { width: 14px; opacity: .8; }
    .demos .demo-dot:focus-visible { outline: 2px solid var(--brand); outline-offset: -1px; }
    .demos .demo-arrow:focus-visible { outline: 2px solid #fff; outline-offset: 1px; }
    @keyframes demo-drift { from { transform: translate(-4%, -3%) rotate(0deg); } to { transform: translate(4%, 3%) rotate(8deg); } }
    @keyframes demo-ripple { from { opacity: 1; transform: scale(.3); } to { opacity: 0; transform: scale(1.6); } }
    @keyframes demo-press { 50% { transform: scale(.8); } }
    @keyframes demo-caption { from { opacity: 0; transform: translateY(2px); } }
    @keyframes demo-blink { 50% { opacity: 0; } }
    @media (prefers-reduced-motion: reduce) {
      .demo-mesh { animation: none; }
      .demo-scene, .demo-scene * { transition: none !important; animation: none !important; }
      .demo-caption .demo-step { animation: none; }
    }`;

  // Each scene's CSS, scoped to it. `.at-<step>` is on the scene itself
  const scope = (id, selector) => (selector.startsWith(".at-") ? `.demo-${id}${selector}` : `.demo-${id} ${selector}`);
  const sceneStyle = Object.entries(DEMOS)
    .map(([id, demo]) => demo.css.replace(/(^|\})\s*([^{}@]+)\{/g, (_, close, selectors) =>
      `${close}\n${selectors.split(/,(?![^(]*\))/).map((s) => scope(id, s.trim())).join(", ")} {`))
    .join("\n");
  // In the page for the carousel, and again in each one's shadow root for
  // its scene, so the page's own CSS, like Settings' rows, can't reach it
  const themeStyle = ["dark", "light"].map((mode) => {
    const at = mode === "light" ? '[data-mode="light"]' : "";
    return `.demo-themes${at} .ted { ${themeVars("figma", mode)} } .demo-themes.at-2${at} .ted { ${themeVars("grove", mode)} } .demo-themes.at-4${at} .ted { ${themeVars("lagoon", mode)} }`;
  }).join("\n");
  const CSS = STYLE + sceneStyle + themeStyle;
  const style = document.createElement("style");
  style.textContent = CSS;
  document.head.append(style);

  const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)");

  // A lake at dusk below mountains, for the image demos, at `w` × `h`
  // pixels: ridges fading into haze with distance, the sun's glow, their
  // reflection in the water and a little grain, the same every time
  const paintLandscape = (canvas, w, h, grain = true) => {
    canvas.width = w;
    canvas.height = h;
    const c = canvas.getContext("2d");
    let seed = 7;
    const random = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
    const horizon = h * 0.64;
    const sky = c.createLinearGradient(0, 0, 0, horizon);
    sky.addColorStop(0, "#2c3e78");
    sky.addColorStop(0.45, "#8a7bb8");
    sky.addColorStop(0.8, "#f0a98c");
    sky.addColorStop(1, "#ffd3a3");
    c.fillStyle = sky;
    c.fillRect(0, 0, w, horizon);
    const sunX = w * 0.66;
    const sunY = horizon - h * 0.1;
    const glow = c.createRadialGradient(sunX, sunY, 0, sunX, sunY, h * 0.55);
    glow.addColorStop(0, "rgba(255, 236, 200, 0.95)");
    glow.addColorStop(0.08, "rgba(255, 220, 170, 0.8)");
    glow.addColorStop(0.35, "rgba(255, 170, 130, 0.25)");
    glow.addColorStop(1, "rgba(255, 170, 130, 0)");
    c.fillStyle = glow;
    c.fillRect(0, 0, w, horizon);
    // Each ridge peaks where two waves of different lengths meet, roughened
    // by a line split in half again and again, each point moved less
    const ridge = (base, height) => {
      const [f1, f2, p1, p2] = [4 + random() * 4, 9 + random() * 6, random() * 6, random() * 6];
      let rough = [0, 0];
      for (let span = 1; rough.length < 257; span *= 0.6) {
        const next = [];
        for (let i = 0; i < rough.length - 1; i++) next.push(rough[i], (rough[i] + rough[i + 1]) / 2 + (random() - 0.5) * span);
        next.push(rough.at(-1));
        rough = next;
      }
      return rough.map((r, i) => {
        const x = i / (rough.length - 1);
        const peak = (1 - Math.abs(Math.sin(x * f1 + p1))) ** 1.6;
        return base - height * (0.5 * peak + 0.22 * (Math.sin(x * f2 + p2) * 0.5 + 0.5) + 0.5 * r);
      });
    };
    const layers = [
      { base: 0.6, height: 0.42, color: "#9b88c0", haze: 0.3 },
      { base: 0.62, height: 0.3, color: "#6c5c9e", haze: 0.18 },
      { base: 0.645, height: 0.18, color: "#3f3874", haze: 0.06 },
      { base: 0.655, height: 0.06, color: "#221f44", haze: 0 },
    ];
    const ridges = layers.map((layer) => ({ ...layer, points: ridge(h * layer.base, h * layer.height) }));
    const drawRidge = ({ points, color, haze }, flip) => {
      c.beginPath();
      c.moveTo(0, horizon);
      points.forEach((y, i) => c.lineTo((i / (points.length - 1)) * w, flip ? 2 * horizon - y : y));
      c.lineTo(w, horizon);
      c.closePath();
      c.fillStyle = color;
      c.fill();
      if (haze && !flip) {
        const mist = c.createLinearGradient(0, horizon - h * 0.12, 0, horizon);
        mist.addColorStop(0, "rgba(255, 200, 170, 0)");
        mist.addColorStop(1, `rgba(255, 200, 170, ${haze})`);
        c.fillStyle = mist;
        c.fill();
      }
    };
    for (const layer of ridges) drawRidge(layer, false);
    // The water: the sky and ridges upside down, darker and softened
    c.save();
    c.beginPath();
    c.rect(0, horizon, w, h - horizon);
    c.clip();
    const water = c.createLinearGradient(0, horizon, 0, h);
    water.addColorStop(0, "#ffc39a");
    water.addColorStop(0.3, "#8a7bb8");
    water.addColorStop(1, "#1c2350");
    c.fillStyle = water;
    c.fillRect(0, horizon, w, h - horizon);
    c.globalAlpha = 0.55;
    c.filter = `blur(${Math.max(1, w / 260)}px)`;
    for (const layer of ridges) drawRidge(layer, true);
    c.filter = "none";
    c.globalAlpha = 1;
    // The sun's path on the water: a soft glow, broken by ripples
    c.save();
    c.translate(sunX, horizon);
    c.scale(1, 3);
    const path = c.createRadialGradient(0, 0, 0, 0, 0, w * 0.06);
    path.addColorStop(0, "rgba(255, 220, 175, 0.6)");
    path.addColorStop(1, "rgba(255, 220, 175, 0)");
    c.fillStyle = path;
    c.fillRect(-w * 0.06, 0, w * 0.12, (h - horizon) / 3);
    c.restore();
    for (let i = 0; i < 26; i++) {
      const t = random();
      const y = horizon + 2 + t * t * (h - horizon - 4);
      const half = w * (0.006 + t * 0.03) * (0.4 + random());
      c.fillStyle = `rgba(255, 230, 190, ${0.35 * (1 - t)})`;
      c.fillRect(sunX - half + (random() - 0.5) * w * 0.02, y, half * 2, Math.max(1, h / 300));
    }
    c.restore();
    const shade = c.createLinearGradient(0, h * 0.75, 0, h);
    shade.addColorStop(0, "rgba(10, 12, 40, 0)");
    shade.addColorStop(1, "rgba(10, 12, 40, 0.45)");
    c.fillStyle = shade;
    c.fillRect(0, 0, w, h);
    // Grain, as a photo has
    if (grain && w > 100) {
      const image = c.getImageData(0, 0, w, h);
      for (let i = 0; i < image.data.length; i += 4) {
        const n = (random() - 0.5) * 7;
        image.data[i] += n;
        image.data[i + 1] += n;
        image.data[i + 2] += n;
      }
      c.putImageData(image, 0, 0);
    }
  };

  // The landscape as Vectorize leaves it: `count` flat colors, each the
  // middle of the pixels nearest it, in gray if `gray`
  const paintVector = (canvas, w, h, count, gray) => {
    paintLandscape(canvas, w, h, false);
    const c = canvas.getContext("2d");
    const image = c.getImageData(0, 0, w, h);
    const px = image.data;
    const sample = [];
    for (let i = 0; i < px.length; i += 4 * 37) sample.push([px[i], px[i + 1], px[i + 2]]);
    const light = (p) => 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    sample.sort((a, b) => light(a) - light(b));
    let centers = Array.from({ length: count }, (_, k) => sample[Math.floor(((k + 0.5) / count) * sample.length)].slice());
    const nearest = (r, g, b) => {
      let best = 0;
      let bestD = Infinity;
      centers.forEach(([cr, cg, cb], k) => {
        const d = (r - cr) ** 2 + (g - cg) ** 2 + (b - cb) ** 2;
        if (d < bestD) [best, bestD] = [k, d];
      });
      return best;
    };
    for (let round = 0; round < 6; round++) {
      const sums = centers.map(() => [0, 0, 0, 0]);
      for (const [r, g, b] of sample) {
        const sum = sums[nearest(r, g, b)];
        sum[0] += r;
        sum[1] += g;
        sum[2] += b;
        sum[3]++;
      }
      centers = centers.map((center, k) => (sums[k][3] ? sums[k].slice(0, 3).map((v) => v / sums[k][3]) : center));
    }
    const shown = centers.map((center) => (gray ? Array(3).fill(light(center)) : center));
    for (let i = 0; i < px.length; i += 4) {
      const [r, g, b] = shown[nearest(px[i], px[i + 1], px[i + 2])];
      px[i] = r;
      px[i + 1] = g;
      px[i + 2] = b;
    }
    c.putImageData(image, 0, 0);
  };

  // The active theme's mesh (themes.js), from its brand color: Figma's
  // blue, or the theme whose accent is nearest the brand's hue
  const hue = (hex) => {
    const lin = (x) => (x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4);
    const [r, g, b] = [1, 3, 5].map((i) => lin(parseInt(hex.slice(i, i + 2), 16) / 255));
    const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
    const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
    const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
    const a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    const bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
    return ((Math.atan2(bb, a) * 180) / Math.PI + 360) % 360;
  };
  const modeNow = () => {
    const root = document.documentElement;
    if (root.classList.contains("light")) return "light";
    if (root.classList.contains("dark")) return "dark";
    return matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  };
  const mesh = (mode) => {
    if (typeof ligmaMesh !== "function") return "none";
    const probe = document.createElement("canvas").getContext("2d");
    probe.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--brand").trim() || "#0d99ff";
    const brand = probe.fillStyle.toLowerCase();
    let id = "figma";
    if (/^#[0-9a-f]{6}$/.test(brand) && brand !== "#0d99ff") {
      const h = hue(brand);
      const turn = (accent) => Math.abs(((accent - h + 540) % 360) - 180);
      id = LIGMA_THEMES.filter((theme) => theme.accent != null).sort((x, y) => turn(x.accent) - turn(y.accent))[0]?.id ?? id;
    }
    return ligmaMesh(id, mode);
  };
  const carousels = new Set();
  const paint = (carousel) => {
    const mode = modeNow();
    carousel.dataset.mode = mode;
    carousel.querySelector(".demo-view")?.shadowRoot?.querySelector(".demo-scene")?.setAttribute("data-mode", mode);
    carousel.style.setProperty("--demo-mesh", mesh(mode));
  };
  // `applyTheme` sets the brand color and the light or dark class
  new MutationObserver(() => carousels.forEach(paint)).observe(document.documentElement, { attributes: true, attributeFilter: ["class", "style"] });

  const POINTER = '<svg width="13" height="19" viewBox="0 0 13 19"><path d="M1 1v14.6l3.6-3.4 2.4 5.5 2.4-1-2.4-5.4h5.1z" fill="#000" stroke="#fff" stroke-width="1.2" stroke-linejoin="round"/></svg>';

  // Plays demo `id` in `view`, with its captions in `caption`. Returns
  // controls; `onLoop` runs each time it starts over
  const play = (id, view, caption, onLoop) => {
    const demo = DEMOS[id];
    const [w, h] = demo.size ?? SIZE;
    const scene = document.createElement("div");
    scene.className = `demo-scene demo-${id}`;
    scene.style.cssText = `width:${w}px;height:${h}px;transform:scale(calc(var(--sw, ${w}) / ${w}))`;
    scene.setAttribute("aria-hidden", "true");
    scene.innerHTML = demo.html;
    // `data-size` in scene pixels, sharper by `data-sharp`
    for (const canvas of scene.querySelectorAll("canvas[data-size]")) {
      const sharp = canvas.dataset.sharp ? Math.max(2, Math.ceil((devicePixelRatio || 1) * 1.5)) : 1;
      const [cw, ch] = canvas.dataset.size.split("x").map(Number);
      const [kind, colors, gray] = (canvas.dataset.paint ?? "landscape").split(":");
      if (kind === "vector") paintVector(canvas, cw * sharp, ch * sharp, Number(colors), gray === "gray");
      else paintLandscape(canvas, cw * sharp, ch * sharp);
    }
    // Settings' app icons (app-icons.js) and the themes' orbs (themes.js)
    for (const img of scene.querySelectorAll("img[data-icon]")) {
      img.alt = "";
      if (typeof ligmaAppIconUrl === "function") img.src = ligmaAppIconUrl(img.dataset.icon, true) ?? "";
    }
    const mode = view.closest(".demos")?.dataset.mode ?? modeNow();
    for (const orb of scene.querySelectorAll("[data-mesh]")) {
      const [theme, orbMode] = orb.dataset.mesh.split(":");
      orb.style.setProperty("--orb", meshOf(theme, orbMode));
    }
    let pointer = null;
    if (demo.pointer !== false) {
      pointer = document.createElement("div");
      pointer.className = "demo-pointer";
      pointer.innerHTML = `<span class="demo-ripple"></span>${demo.brush ? '<span class="demo-brush"></span>' : ""}${POINTER}`;
      scene.append(pointer);
    }
    scene.dataset.mode = mode;
    const shadow = view.shadowRoot ?? view.attachShadow({ mode: "open" });
    shadow.replaceChildren(shadow.querySelector("style") ?? Object.assign(document.createElement("style"), { textContent: CSS }), scene);

    const title = Object.assign(document.createElement("span"), { className: "demo-title", textContent: demo.title });
    let shown = null;
    const say = (text) => {
      if (text === shown) return;
      shown = text;
      caption.replaceChildren(title, Object.assign(document.createElement("span"), { className: "demo-step", textContent: text }));
    };

    let step = -1;
    let timer = 0;
    let where = demo.steps[0].at ?? [w + 20, h + 20];
    let motion = null;
    const place = (at, ms) => {
      if (!pointer || !at) return;
      motion?.cancel();
      motion = null;
      pointer.style.transitionDuration = `${ms}ms`;
      pointer.style.transform = `translate(${at[0]}px, ${at[1]}px)`;
      where = at;
    };
    // Along SVG path `d`, at an even speed, as a hand drawing it would
    const trace = (d, ms) => {
      if (!pointer) return;
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", d);
      const length = path.getTotalLength();
      const frames = Array.from({ length: 49 }, (_, i) => {
        const point = path.getPointAtLength((length * i) / 48);
        return { transform: `translate(${point.x}px, ${point.y}px)` };
      });
      motion?.cancel();
      pointer.style.transitionDuration = "0ms";
      pointer.style.transform = frames.at(-1).transform;
      motion = pointer.animate(frames, { duration: ms, easing: "linear", fill: "forwards" });
      const end = path.getPointAtLength(length);
      where = [end.x, end.y];
    };
    const go = (next) => {
      step = next;
      const current = demo.steps[step];
      if (step === 0) {
        scene.classList.add("reset");
        scene.className = scene.className.replace(/\s*at-\d+/g, "");
        place(current.at ?? where, 0);
        void scene.offsetWidth;
        scene.classList.remove("reset");
      }
      scene.classList.add(`at-${step}`);
      if (step > 0 && current.along) trace(current.along, current.move ?? 600);
      else if (step > 0) place(current.at, current.move ?? 600);
      scene.removeAttribute("data-click");
      if (current.click) {
        void scene.offsetWidth;
        scene.dataset.click = "";
      }
      say(current.caption);
    };
    const tick = () => {
      timer = setTimeout(() => {
        if (step + 1 < demo.steps.length) {
          go(step + 1);
          tick();
          return;
        }
        // Fades out, starts over and fades back in
        view.classList.add("fading");
        timer = setTimeout(() => {
          go(0);
          view.classList.remove("fading");
          if (onLoop?.() !== false) tick();
        }, 260);
      }, demo.steps[step].ms);
    };

    // Without motion: the last step, still
    const still = () => {
      for (let i = 0; i < demo.steps.length; i++) scene.classList.add(`at-${i}`);
      place(demo.steps.findLast((s) => s.at)?.at, 0);
      say(demo.steps.at(-1).caption);
    };

    let running = false;
    go(0);
    return {
      resume() {
        if (running) return;
        running = true;
        if (reduceMotion.matches) return still();
        motion?.play();
        tick();
      },
      pause() {
        running = false;
        clearTimeout(timer);
        motion?.pause();
      },
    };
  };

  const ARROW = (d) => `<svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="${d}"/></svg>`;

  // A carousel of demos `ids`: arrows on its sides while hovered, and a dot
  // for each demo under it when there's more than one
  const carousel = (ids) => {
    const root = document.createElement("div");
    root.className = "demos";
    root.setAttribute("role", "region");
    root.setAttribute("aria-roledescription", "carousel");
    root.setAttribute("aria-label", "Feature demos");
    const stage = Object.assign(document.createElement("div"), { className: "demo-stage" });
    const view = Object.assign(document.createElement("div"), { className: "demo-view" });
    const caption = Object.assign(document.createElement("div"), { className: "demo-caption" });
    stage.append(Object.assign(document.createElement("div"), { className: "demo-mesh" }), view);
    root.append(stage, caption);

    const many = ids.length > 1;
    let index = 0;
    let player = null;
    let visible = false;
    // Moves on by itself after two loops, until you pick one
    let auto = many;
    let loops = 0;
    const dots = [];

    const show = (next, focus) => {
      index = (next + ids.length) % ids.length;
      player?.pause();
      loops = 0;
      const start = () => {
        player = play(ids[index], view, caption, () => {
          loops += 1;
          if (auto && loops >= 2) {
            show(index + 1);
            return false;
          }
        });
        view.classList.remove("fading");
        if (visible) player.resume();
      };
      if (player) {
        view.classList.add("fading");
        setTimeout(start, 200);
      } else {
        start();
      }
      dots.forEach((dot, i) => {
        dot.setAttribute("aria-selected", i === index);
        dot.tabIndex = i === index ? 0 : -1;
      });
      if (focus) dots[index]?.focus();
    };
    const pick = (next, focus) => {
      auto = false;
      show(next, focus);
    };

    if (many) {
      const prev = Object.assign(document.createElement("button"), { className: "demo-arrow demo-prev", tabIndex: -1, innerHTML: ARROW("M7.5 2.5 4 6l3.5 3.5") });
      const next = Object.assign(document.createElement("button"), { className: "demo-arrow demo-next", tabIndex: -1, innerHTML: ARROW("M4.5 2.5 8 6 4.5 9.5") });
      prev.setAttribute("aria-label", "Previous demo");
      next.setAttribute("aria-label", "Next demo");
      // A quick double click would select the text around it
      for (const control of [prev, next]) control.addEventListener("mousedown", (e) => e.preventDefault());
      prev.addEventListener("click", () => pick(index - 1));
      next.addEventListener("click", () => pick(index + 1));
      stage.append(prev, next);
      const list = Object.assign(document.createElement("div"), { className: "demo-dots" });
      list.setAttribute("role", "tablist");
      list.setAttribute("aria-label", "Demos");
      ids.forEach((id, i) => {
        const dot = Object.assign(document.createElement("button"), { className: "demo-dot" });
        dot.setAttribute("role", "tab");
        dot.setAttribute("aria-label", DEMOS[id].title);
        dot.addEventListener("mousedown", (e) => e.preventDefault());
        dot.addEventListener("click", () => pick(i));
        dots.push(dot);
        list.append(dot);
      });
      root.append(list);
      root.addEventListener("keydown", (e) => {
        if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
        e.preventDefault();
        e.stopPropagation();
        pick(index + (e.key === "ArrowLeft" ? -1 : 1), true);
      });
    }

    // Plays only while on screen
    new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting;
      if (visible) player?.resume();
      else player?.pause();
    }).observe(stage);
    new ResizeObserver(() => {
      if (stage.clientWidth) stage.style.setProperty("--sw", stage.clientWidth);
    }).observe(stage);

    carousels.add(root);
    paint(root);
    show(0);
    return root;
  };

  // The demos of release `version`
  window.ligmaReleaseDemos = (version) => (RELEASES[version] ?? []).filter((id) => DEMOS[id]);
  // A carousel of demos `ids`, or null without any
  window.ligmaDemos = (ids) => (ids.length ? carousel(ids) : null);
})();
