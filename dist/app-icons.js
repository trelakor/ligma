// Ligma's app icons, for Settings > App Icon. Each is an SVG drawn from
// the logo's own shapes (`ligmaGlyph`) on the macOS icon grid: an 824pt
// tile in a 1024pt canvas. The settings window shows them as they are and,
// once one is picked, draws it into a PNG for src-tauri/src/app_icon.rs to
// put in the Dock or on the taskbar. "ligma" is the icon Ligma ships with
// (assets/Ligma.icon), drawn from dist/app-icon-*.png instead

const LIGMA_APP_ICON_COLLECTIONS = [
  { id: "colorways", name: "Colorways" },
  { id: "materials", name: "Materials" },
  { id: "graphic", name: "Graphic" },
  { id: "playful", name: "Playful" },
];

// A superellipse `radius` from the middle, close to the system's own icon
// corners
function ligmaSquircle(radius) {
  const points = [];
  for (let i = 0; i < 360; i++) {
    const t = (i / 360) * Math.PI * 2;
    const at = (v) => Math.sign(v) * Math.abs(v) ** 0.4 * radius + 512;
    points.push(`${at(Math.cos(t)).toFixed(1)} ${at(Math.sin(t)).toFixed(1)}`);
  }
  return `M${points.join("L")}Z`;
}

// The macOS tile
const LIGMA_TILE = ligmaSquircle(412);

// The logo: three half-rounded blocks stacked, and a dot beside the last.
// `unit` is a block's height. Parts are top, middle, bottom and dot
function ligmaGlyph(unit = 193, cx = 512, cy = 512) {
  const r = unit / 2;
  const x0 = cx - unit;
  const x1 = cx;
  const y0 = cy - 1.5 * unit;
  const xc = x0 + r;
  const bx = x1 + r;
  const by = y0 + 2.5 * unit;
  const half = (y) => `M${xc} ${y}H${x1}V${y + unit}H${xc}a${r} ${r} 0 0 1 0 ${-unit}Z`;
  const parts = [
    `M${xc} ${y0}a${r} ${r} 0 0 1 ${r} ${r}V${y0 + unit}H${xc}a${r} ${r} 0 0 1 0 ${-unit}Z`,
    half(y0 + unit),
    half(y0 + 2 * unit),
    `M${bx - r} ${by}a${r} ${r} 0 1 0 ${unit} 0a${r} ${r} 0 1 0 ${-unit} 0Z`,
  ];
  const centers = [[xc, y0 + r], [xc, y0 + unit + r], [xc, y0 + 2 * unit + r], [bx, by]];
  // Whether a point is inside part `i`
  const inside = (i, x, y) => {
    const [px, py] = centers[i];
    if ((x - px) ** 2 + (y - py) ** 2 <= r * r) return true;
    if (i === 3) return false;
    const top = y0 + i * unit;
    return x >= xc && x <= x1 && y >= (i === 0 ? top + r : top) && y <= top + unit;
  };
  return { unit, r, x0, x1, y0, xc, bx, by, parts, centers, inside, right: bx + r, bottom: y0 + 3 * unit };
}

const LIGMA_G = ligmaGlyph();
const LIGMA_COLORS = ["#00CF66", "#FF0026", "#904BFF", "#00BAFF"];
// The frame inset in the tile (Gilded, Holo), `width` thick, and the logo
// inside it, smaller so the frame has room. Frames are filled rings:
// WebKit leaves hairline seams in a stroked squircle
const LIGMA_FRAME = { radius: 358, width: 34 };
const ligmaFrame = (width = LIGMA_FRAME.width, radius = LIGMA_FRAME.radius) => `${ligmaSquircle(radius + width / 2)}${ligmaSquircle(radius - width / 2)}`;
const LIGMA_FRAMED = ligmaGlyph(178);

// SVG helpers
const ligmaSvg = {
  rand(seed) {
    return () => {
      seed = (seed + 0x6d2b79f5) | 0;
      let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  },
  stops: (stops) => stops.map(([offset, color, opacity = 1]) => `<stop offset="${offset}" stop-color="${color}" stop-opacity="${opacity}"/>`).join(""),
  linear: (id, stops, [x1, y1, x2, y2] = [0, 0, 0, 1], user = false) =>
    `<linearGradient id="${id}" x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}"${user ? ' gradientUnits="userSpaceOnUse"' : ""}>${ligmaSvg.stops(stops)}</linearGradient>`,
  radial: (id, stops, [cx, cy, r] = [0.5, 0.5, 0.5], user = false) =>
    `<radialGradient id="${id}" cx="${cx}" cy="${cy}" r="${r}"${user ? ' gradientUnits="userSpaceOnUse"' : ""}>${ligmaSvg.stops(stops)}</radialGradient>`,
  shadow: (id, dy, blur, opacity, color = "#000", dx = 0) =>
    `<filter id="${id}" x="-40%" y="-40%" width="180%" height="190%"><feDropShadow dx="${dx}" dy="${dy}" stdDeviation="${blur}" flood-color="${color}" flood-opacity="${opacity}"/></filter>`,
  blur: (id, amount) => `<filter id="${id}" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="${amount}"/></filter>`,
  // Fine paper or film grain, laid over with `mix-blend-mode`
  grain: (id, opacity, frequency = 0.85) =>
    `<filter id="${id}" x="0" y="0" width="1024" height="1024" filterUnits="userSpaceOnUse"><feTurbulence type="fractalNoise" baseFrequency="${frequency}" numOctaves="2" seed="4" stitchTiles="stitch"/><feColorMatrix type="saturate" values="0"/><feComponentTransfer><feFuncA type="table" tableValues="${opacity} ${opacity}"/></feComponentTransfer></filter>`,
  fill: (fill) => `<rect width="1024" height="1024" fill="${fill}"/>`,
  // The glyph's parts, each filled with `fills[i]`
  parts: (fills, attrs = "", glyph = LIGMA_G) => glyph.parts.map((d, i) => `<path d="${d}" fill="${fills[i]}" ${attrs}/>`).join(""),
  // Part `i` scaled by `s` around its middle
  shrink: (i, s, glyph = LIGMA_G) => {
    const [x, y] = glyph.centers[i];
    return `transform="translate(${x} ${y}) scale(${s}) translate(${-x} ${-y})"`;
  },
  clips: (glyph = LIGMA_G) =>
    `<clipPath id="glyph">${glyph.parts.map((d) => `<path d="${d}"/>`).join("")}</clipPath>` + glyph.parts.map((d, i) => `<clipPath id="part${i}"><path d="${d}"/></clipPath>`).join(""),
  mix: (hex, to, amount) => {
    const a = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
    const b = [1, 3, 5].map((i) => parseInt(to.slice(i, i + 2), 16));
    return `#${a.map((v, i) => Math.round(v + (b[i] - v) * amount).toString(16).padStart(2, "0")).join("")}`;
  },
};

// A tile in a vertical gradient with the glyph in flat `colors` on it
const ligmaColorway = (stops, colors, shade = 0.2) => () => {
  const { linear, shadow, fill, parts } = ligmaSvg;
  return {
    defs: linear("tile-fill", stops) + linear("sheen", [[0, "#fff", 0.16], [0.5, "#fff", 0]]) + shadow("lift", 12, 14, shade),
    body: fill("url(#tile-fill)") + fill("url(#sheen)") + `<g filter="url(#lift)">${parts(colors)}</g>`,
  };
};

const LIGMA_APP_ICONS = [
  { id: "ligma", name: "Ligma", collection: "colorways" },

  // Colorways
  { id: "onyx", name: "Onyx", collection: "colorways", art: ligmaColorway([[0, "#2c2c31"], [1, "#0b0b0d"]], ["#ffffff", "#d0d0d6", "#94949c", "#ffffff"], 0.5) },
  { id: "ink", name: "Ink", collection: "colorways", art: ligmaColorway([[0, "#fcf9f2"], [1, "#ece5d6"]], ["#151515", "#3d3d3d", "#6f6f6f", "#151515"], 0.14) },
  { id: "pastel", name: "Pastel", collection: "colorways", art: ligmaColorway([[0, "#fffaf2"], [1, "#f3e9ff"]], ["#72dda5", "#ff98a9", "#bea2ff", "#86d3ff"], 0.12) },
  { id: "sunset", name: "Sunset", collection: "colorways", art: ligmaColorway([[0, "#ffb347"], [0.5, "#ff5e62"], [1, "#9a3cff"]], ["#fff4d6", "#ffd6a3", "#ffc0da", "#ffe14d"], 0.22) },
  { id: "forest", name: "Forest", collection: "colorways", art: ligmaColorway([[0, "#1f5c45"], [1, "#09271d"]], ["#b5e48c", "#f4a259", "#74c69d", "#f1faee"], 0.4) },
  { id: "tide", name: "Tide", collection: "colorways", art: ligmaColorway([[0, "#167dd6"], [1, "#0a2b5e"]], ["#7df9ff", "#3ac7ff", "#93a9ff", "#e9fdff"], 0.35) },
  { id: "ultraviolet", name: "Ultraviolet", collection: "colorways", art: ligmaColorway([[0, "#3d108f"], [1, "#11032b"]], ["#ff5cdb", "#c24cff", "#7b5cff", "#2bf0ff"], 0.45) },
  { id: "citrus", name: "Citrus", collection: "colorways", art: ligmaColorway([[0, "#fffbea"], [1, "#ffefb8"]], ["#74c83a", "#ff4d6d", "#ff8c1a", "#ffc400"], 0.16) },

  // Materials
  {
    id: "frosted", name: "Frosted", collection: "materials",
    art() {
      const { fill, blur, clips, parts, linear } = ligmaSvg;
      const blobs = `<circle cx="290" cy="250" r="270" fill="#00CF66"/><circle cx="800" cy="320" r="250" fill="#00BAFF"/><circle cx="770" cy="820" r="290" fill="#904BFF"/><circle cx="240" cy="830" r="240" fill="#FF0026"/>`;
      return {
        defs: blur("soft", 46) + blur("frost", 90) + blur("drop", 20) + clips() + linear("glare", [[0, "#fff", 0.55], [0.55, "#fff", 0.08], [1, "#fff", 0]]),
        body: fill("#efeaff") + `<g filter="url(#soft)" opacity=".9">${blobs}</g>`
          + `<g transform="translate(0 18)" filter="url(#drop)" opacity=".22">${parts(["#1b0b3a", "#1b0b3a", "#1b0b3a", "#1b0b3a"])}</g>`
          + `<g clip-path="url(#glyph)"><g filter="url(#frost)">${blobs}</g><rect width="1024" height="1024" fill="#fff" opacity=".5"/></g>`
          + parts(["url(#glare)", "url(#glare)", "url(#glare)", "url(#glare)"])
          + parts(["none", "none", "none", "none"], 'stroke="#fff" stroke-opacity=".9" stroke-width="4"'),
      };
    },
  },
  {
    id: "chrome", name: "Chrome", collection: "materials",
    art() {
      const { fill, radial, linear, shadow, parts } = ligmaSvg;
      return {
        defs: radial("tile-fill", [[0, "#3a3d45"], [1, "#07080a"]], [0.5, 0.15, 0.95])
          + linear("chrome", [[0, "#fdfdfd"], [0.2, "#d6d9de"], [0.44, "#666b74"], [0.5, "#2b2e33"], [0.54, "#c6cad0"], [0.78, "#8c9199"], [1, "#f4f5f7"]])
          + linear("edge", [[0, "#ffffff"], [0.5, "#8a8f97"], [1, "#2a2d31"]])
          + shadow("lift", 18, 18, 0.7),
        body: fill("url(#tile-fill)")
          + `<g filter="url(#lift)">${parts(["url(#chrome)", "url(#chrome)", "url(#chrome)", "url(#chrome)"])}</g>`
          + parts(["none", "none", "none", "none"], 'stroke="url(#edge)" stroke-width="5"'),
      };
    },
  },
  {
    id: "clay", name: "Clay", collection: "materials",
    art() {
      const { fill, radial, shadow, shrink } = ligmaSvg;
      const colors = ["#3fcf85", "#ff6a5c", "#9a7bff", "#46b4ff"];
      const clay = `<filter id="clay" x="-10%" y="-10%" width="120%" height="120%"><feGaussianBlur in="SourceAlpha" stdDeviation="14" result="b"/>`
        + `<feOffset in="b" dx="-10" dy="-13" result="o1"/><feComposite in="SourceAlpha" in2="o1" operator="out" result="e1"/><feFlood flood-color="#2b1200" flood-opacity=".32"/><feComposite in2="e1" operator="in" result="shade"/>`
        + `<feOffset in="b" dx="10" dy="13" result="o2"/><feComposite in="SourceAlpha" in2="o2" operator="out" result="e2"/><feFlood flood-color="#fff" flood-opacity=".42"/><feComposite in2="e2" operator="in" result="light"/>`
        + `<feMerge><feMergeNode in="SourceGraphic"/><feMergeNode in="shade"/><feMergeNode in="light"/></feMerge><feComposite in2="SourceAlpha" operator="in"/></filter>`;
      return {
        defs: radial("tile-fill", [[0, "#f6efe7"], [1, "#e2d4c4"]], [0.3, 0.2, 1]) + clay + shadow("lift", 18, 18, 0.25, "#5a4630"),
        body: fill("url(#tile-fill)") + `<g filter="url(#lift)">${LIGMA_G.parts.map((d, i) => `<path d="${d}" fill="${colors[i]}" filter="url(#clay)" ${shrink(i, 0.93)}/>`).join("")}</g>`,
      };
    },
  },
  {
    id: "paper", name: "Paper", collection: "materials",
    art() {
      const { fill, grain, shadow, shrink } = ligmaSvg;
      const colors = ["#3bab78", "#e3464b", "#8a69d6", "#3a9dd6"];
      const turns = [-2.2, 1.6, -1.2, 2.8];
      return {
        defs: grain("paper", 0.22, 0.75) + shadow("cut", 7, 6, 0.3, "#3a2a10"),
        body: fill("#f2eadc")
          + LIGMA_G.parts.map((d, i) => {
            const [x, y] = LIGMA_G.centers[i];
            return `<g transform="rotate(${turns[i]} ${x} ${y})"><path d="${d}" fill="${colors[i]}" filter="url(#cut)" ${shrink(i, 0.96)}/></g>`;
          }).join("")
          + `<rect width="1024" height="1024" filter="url(#paper)" style="mix-blend-mode:multiply"/>`,
      };
    },
  },
  {
    id: "enamel", name: "Enamel", collection: "materials",
    art() {
      const { fill, linear, shadow, parts } = ligmaSvg;
      const twill = `<filter id="twill" x="0" y="0" width="1024" height="1024" filterUnits="userSpaceOnUse"><feTurbulence type="fractalNoise" baseFrequency="0.9 0.05" numOctaves="2" seed="2"/><feColorMatrix type="matrix" values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0 0 0 .5 -.12"/></filter>`;
      return {
        defs: twill + linear("gold", [[0, "#fff3c2"], [0.35, "#e8b94a"], [0.6, "#9c6b12"], [1, "#f7d987"]], [0, 0, 1, 1]) + linear("gloss", [[0, "#fff", 0.45], [0.45, "#fff", 0]]) + shadow("pin", 16, 14, 0.5, "#05081a"),
        body: fill("#24315a") + `<rect width="1024" height="1024" filter="url(#twill)"/>`
          + `<g filter="url(#pin)">${parts(["#000", "#000", "#000", "#000"], 'stroke="url(#gold)" stroke-width="26" stroke-linejoin="round"')}</g>`
          + parts(["#0fbf63", "#ec1c3a", "#8648f2", "#0aa8ee"])
          + parts(["url(#gloss)", "url(#gloss)", "url(#gloss)", "url(#gloss)"])
          + parts(["none", "none", "none", "none"], 'stroke="url(#gold)" stroke-width="10" stroke-linejoin="round"'),
      };
    },
  },
  {
    id: "gilded", name: "Gilded", collection: "materials",
    art() {
      const { fill, radial, linear, shadow, parts } = ligmaSvg;
      return {
        defs: radial("tile-fill", [[0, "#24201a"], [1, "#050505"]], [0.5, 0.3, 0.9])
          + linear("gold", [[0, "#fff7d1"], [0.22, "#f2cc68"], [0.48, "#a87414"], [0.56, "#f9e195"], [0.8, "#86590c"], [1, "#e8c058"]])
          + linear("frame", [[0, "#fff1b8"], [0.3, "#e3b44c"], [0.55, "#8a5d10"], [0.8, "#f3d27a"], [1, "#9c6c14"]], [0, 0, 1, 1])
          + shadow("lift", 14, 16, 0.75),
        body: fill("url(#tile-fill)")
          + `<path d="${ligmaFrame(LIGMA_FRAME.width + 6)}" fill="#000" fill-opacity=".6" fill-rule="evenodd" transform="translate(0 5)"/><path d="${ligmaFrame()}" fill="url(#frame)" fill-rule="evenodd"/><path d="${ligmaFrame(2)}" fill="#fff6d0" fill-opacity=".5" fill-rule="evenodd"/>`
          // Inside the frame, the size Holo's outlined logo reaches (168 + its 30 outline, see holo)
          + `<g filter="url(#lift)">${parts(["url(#gold)", "url(#gold)", "url(#gold)", "url(#gold)"], "", LIGMA_FRAMED)}</g>`
          + parts(["none", "none", "none", "none"], 'stroke="#fff3c0" stroke-opacity=".55" stroke-width="2"', LIGMA_FRAMED),
      };
    },
  },
  {
    id: "walnut", name: "Walnut", collection: "materials",
    art() {
      const { clips, parts } = ligmaSvg;
      const wood = (id, rows, seed) => `<filter id="${id}" x="0" y="0" width="1024" height="1024" filterUnits="userSpaceOnUse"><feTurbulence type="fractalNoise" baseFrequency="0.003 0.07" numOctaves="4" seed="${seed}"/><feColorMatrix type="matrix" values="${rows}"/></filter>`;
      return {
        defs: wood("walnut", "0.55 0 0 0 0.2  0.34 0 0 0 0.1  0.2 0 0 0 0.05  0 0 0 0 1", 3)
          + wood("maple", "0.3 0 0 0 0.72  0.28 0 0 0 0.56  0.22 0 0 0 0.36  0 0 0 0 1", 9)
          + clips() + `<filter id="soft" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="5"/></filter>`,
        body: `<rect width="1024" height="1024" filter="url(#walnut)"/>`
          + `<g clip-path="url(#glyph)"><rect width="1024" height="1024" filter="url(#maple)"/>`
          + `<g filter="url(#soft)">${parts(["none", "none", "none", "none"], 'stroke="#2a1608" stroke-opacity=".75" stroke-width="16"')}</g></g>`
          + `<g transform="translate(0 3)">${parts(["none", "none", "none", "none"], 'stroke="#fff3dc" stroke-opacity=".35" stroke-width="2"')}</g>`,
      };
    },
  },
  {
    id: "marble", name: "Marble", collection: "materials",
    art() {
      const { clips, linear, shadow, parts } = ligmaSvg;
      // Mostly `base`, with a thin band of `vein` where the noise crosses the middle
      const table = (base, vein) => Array.from({ length: 40 }, (_, i) => (i === 19 ? vein : i === 23 ? (base + vein) / 2 : base)).join(" ");
      const veins = (id, table, seed) => `<filter id="${id}" x="0" y="0" width="1024" height="1024" filterUnits="userSpaceOnUse"><feTurbulence type="fractalNoise" baseFrequency="0.003" numOctaves="6" seed="${seed}"/><feColorMatrix type="matrix" values="1 0 0 0 0  1 0 0 0 0  1 0 0 0 0  0 0 0 0 1"/><feComponentTransfer><feFuncR type="table" tableValues="${table}"/><feFuncG type="table" tableValues="${table}"/><feFuncB type="table" tableValues="${table}"/></feComponentTransfer></filter>`;
      return {
        defs: veins("carrara", table(0.95, 0.5), 8)
          + linear("onyx", [[0, "#3a3a3d"], [0.45, "#151517"], [1, "#050506"]], [0, 0, 1, 1])
          + linear("glare", [[0, "#fff", 0.32], [0.3, "#fff", 0.06], [0.31, "#fff", 0]]) + shadow("lift", 16, 16, 0.4),
        body: `<rect width="1024" height="1024" filter="url(#carrara)"/>`
          + `<g filter="url(#lift)">${parts(["url(#onyx)", "url(#onyx)", "url(#onyx)", "url(#onyx)"])}</g>`
          + parts(["url(#glare)", "url(#glare)", "url(#glare)", "url(#glare)"])
          + parts(["none", "none", "none", "none"], 'stroke="#fff" stroke-opacity=".22" stroke-width="2"'),
      };
    },
  },

  // Graphic
  {
    id: "neon", name: "Neon", collection: "graphic",
    art() {
      const { fill, radial, blur, shrink } = ligmaSvg;
      const colors = ["#3dff8f", "#ff2e63", "#b54bff", "#2ee6ff"];
      const tube = (attrs) => LIGMA_G.parts.map((d, i) => `<path d="${d}" fill="none" stroke="${attrs.color ?? colors[i]}" stroke-width="${attrs.width}" stroke-opacity="${attrs.opacity ?? 1}" stroke-linejoin="round" ${shrink(i, 0.82)}/>`).join("");
      const bricks = `<pattern id="bricks" width="128" height="64" patternUnits="userSpaceOnUse"><path d="M0 0.5H128M0 32.5H128M0.5 0V32M64.5 32V64" stroke="#fff" stroke-opacity=".045" stroke-width="2" fill="none"/></pattern>`;
      return {
        defs: bricks + radial("spill", [[0, "#7a2cff", 0.32], [1, "#7a2cff", 0]], [0.45, 0.55, 0.55]) + blur("glow", 16) + blur("haze", 40),
        body: fill("#140c1f") + fill("url(#bricks)") + fill("url(#spill)")
          + `<g filter="url(#haze)" opacity=".55">${tube({ width: 30 })}</g>`
          + `<g filter="url(#glow)">${tube({ width: 20 })}</g>`
          + tube({ width: 13 }) + tube({ width: 4, color: "#fff", opacity: 0.85 }),
      };
    },
  },
  {
    id: "blueprint", name: "Blueprint", collection: "graphic",
    art() {
      const { fill, linear, parts } = ligmaSvg;
      const g = LIGMA_G;
      const grid = `<pattern id="minor" width="32" height="32" patternUnits="userSpaceOnUse"><path d="M32 0V32H0" fill="none" stroke="#fff" stroke-opacity=".09" stroke-width="1.5"/></pattern>`
        + `<pattern id="major" width="128" height="128" patternUnits="userSpaceOnUse" x="-1" y="-1"><path d="M128 0V128H0" fill="none" stroke="#fff" stroke-opacity=".2" stroke-width="2.5"/></pattern>`;
      const line = 'stroke="#fff" stroke-width="3" fill="none"';
      const dx = g.right + 64;
      const dy = g.bottom + 52;
      const tick = (x, y, v) => (v ? `<path d="M${x - 12} ${y}H${x + 12}" ${line}/>` : `<path d="M${x} ${y - 12}V${y + 12}" ${line}/>`);
      return {
        defs: grid + linear("tile-fill", [[0, "#1a5fbf"], [1, "#0b3a82"]]),
        body: fill("url(#tile-fill)") + fill("url(#minor)") + fill("url(#major)")
          + g.centers.map(([x, y]) => `<circle cx="${x}" cy="${y}" r="${g.r + 26}" ${line} stroke-opacity=".35" stroke-dasharray="10 12"/><path d="M${x - 18} ${y}H${x + 18}M${x} ${y - 18}V${y + 18}" ${line} stroke-opacity=".6"/>`).join("")
          + parts(["#fff", "#fff", "#fff", "#fff"], 'fill-opacity=".1" stroke="#fff" stroke-width="6" stroke-linejoin="round"')
          + `<path d="M${dx} ${g.y0}V${g.bottom}" ${line}/>${tick(dx, g.y0, true)}${tick(dx, g.bottom, true)}${tick(dx, g.y0 + g.unit, true)}${tick(dx, g.y0 + 2 * g.unit, true)}`
          + `<path d="M${g.x0} ${dy}H${g.right}" ${line}/>${tick(g.x0, dy)}${tick(g.right, dy)}${tick(g.x1, dy)}`
          + `<path d="M${g.right} ${g.y0}H${dx + 16}M${g.right + 8} ${g.bottom}H${dx + 16}M${g.x0} ${g.bottom + 12}V${dy + 16}M${g.right} ${g.by + 12}V${dy + 16}" ${line} stroke-opacity=".45" stroke-dasharray="6 8"/>`,
      };
    },
  },
  {
    id: "pixel", name: "Pixel", collection: "graphic",
    art() {
      const { fill, mix } = ligmaSvg;
      const g = LIGMA_G;
      const colors = ["#00e436", "#ff004d", "#9d4dff", "#29adff"];
      const size = g.unit / 8;
      const cols = 16;
      const rows = 24;
      const which = (c, r) => {
        if (c < 0 || r < 0 || c >= cols || r >= rows) return -1;
        const x = g.x0 + (c + 0.5) * size;
        const y = g.y0 + (r + 0.5) * size;
        return [0, 1, 2, 3].find((i) => g.inside(i, x, y)) ?? -1;
      };
      let cells = "";
      for (let r = 0; r < rows; r++) {
        for (let c = 0; c < cols; c++) {
          const i = which(c, r);
          if (i < 0) continue;
          let color = colors[i];
          if (which(c, r - 1) !== i || which(c - 1, r) !== i) color = mix(color, "#ffffff", 0.38);
          else if (which(c, r + 1) !== i || which(c + 1, r) !== i) color = mix(color, "#000000", 0.32);
          cells += `<rect x="${(g.x0 + c * size).toFixed(2)}" y="${(g.y0 + r * size).toFixed(2)}" width="${(size + 0.6).toFixed(2)}" height="${(size + 0.6).toFixed(2)}" fill="${color}"/>`;
        }
      }
      const scan = `<pattern id="scan" width="${size}" height="${size}" patternUnits="userSpaceOnUse" x="${g.x0}" y="${g.y0}"><rect width="${size}" height="${size}" fill="none" stroke="#000" stroke-opacity=".18" stroke-width="2"/></pattern>`;
      return {
        defs: scan,
        body: fill("#1d2b53") + `<g transform="translate(${size} ${size})" opacity=".45">${cells.replace(/fill="#[0-9a-f]+"/g, 'fill="#000"')}</g>` + cells + fill("url(#scan)"),
      };
    },
  },
  {
    id: "riso", name: "Riso", collection: "graphic",
    art() {
      const { fill, grain, clips } = ligmaSvg;
      const g = LIGMA_G;
      const ink = `<filter id="ink" x="0" y="0" width="1024" height="1024" filterUnits="userSpaceOnUse"><feTurbulence type="fractalNoise" baseFrequency="0.7" numOctaves="2" seed="5" result="n"/><feColorMatrix in="n" type="matrix" values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 -2.2 1.95" result="m"/><feComposite in="SourceGraphic" in2="m" operator="in"/></filter>`;
      const dots = `<pattern id="dots" width="22" height="22" patternUnits="userSpaceOnUse" patternTransform="rotate(15)"><circle cx="11" cy="11" r="6.5" fill="#ff48b0"/></pattern>`;
      const [top, mid, low, dot] = g.parts;
      return {
        defs: ink + dots + grain("paper", 0.28, 0.8) + clips(),
        body: fill("#f4efe4")
          + `<g filter="url(#ink)" style="mix-blend-mode:multiply">`
          + `<path d="${dot}" fill="#ffe800"/><path d="${mid}" fill="#ff48b0"/><path d="${low}" fill="#ff48b0"/><g clip-path="url(#part3)">${fill("url(#dots)")}</g></g>`
          + `<g filter="url(#ink)" style="mix-blend-mode:multiply" transform="translate(9 -6)"><path d="${top}" fill="#0078bf"/><path d="${low}" fill="#0078bf"/></g>`
          + `<rect width="1024" height="1024" filter="url(#paper)" style="mix-blend-mode:multiply"/>`,
      };
    },
  },
  {
    id: "terminal", name: "Terminal", collection: "graphic",
    art() {
      const { fill, radial, blur, parts } = ligmaSvg;
      const green = "#3bff74";
      const scan = `<pattern id="scan" width="12" height="12" patternUnits="userSpaceOnUse"><rect width="12" height="8" fill="${green}"/></pattern>`
        + `<pattern id="lines" width="6" height="6" patternUnits="userSpaceOnUse"><rect width="6" height="2.5" fill="#000" fill-opacity=".35"/></pattern>`;
      const prompt = `<path d="M600 262l62 46-62 46" fill="none" stroke="${green}" stroke-width="24"/><rect x="694" y="334" width="86" height="24" fill="${green}"/>`;
      return {
        defs: scan + radial("tile-fill", [[0, "#0f2a16"], [1, "#030805"]], [0.5, 0.5, 0.7]) + radial("vignette", [[0.6, "#000", 0], [1, "#000", 0.55]]) + blur("glow", 14),
        body: fill("url(#tile-fill)")
          + `<g filter="url(#glow)" opacity=".75">${parts([green, green, green, green])}${prompt}</g>`
          + parts(["url(#scan)", "url(#scan)", "url(#scan)", "url(#scan)"]) + prompt
          + fill("url(#lines)") + fill("url(#vignette)"),
      };
    },
  },
  {
    id: "selected", name: "Selected", collection: "graphic",
    art() {
      const { fill, parts, shadow } = ligmaSvg;
      const g = LIGMA_G;
      const blue = "#0d99ff";
      const dotGrid = `<pattern id="grid" width="32" height="32" patternUnits="userSpaceOnUse"><circle cx="16" cy="16" r="2.2" fill="#c9c9c9"/></pattern>`;
      const [l, t, r, b] = [g.x0, g.y0, g.right, g.bottom];
      const handles = [[l, t], [r, t], [l, b], [r, b]].map(([x, y]) => `<rect x="${x - 11}" y="${y - 11}" width="22" height="22" fill="#fff" stroke="${blue}" stroke-width="4"/>`).join("");
      const midX = (l + r) / 2;
      // The multiplayer cursor from the website (site/index.html), tip at the corner handle
      const cursor = `<path transform="translate(${r + 18} ${b + 20}) scale(5.8) translate(-2.2 -2.2)" d="M3.4 2.2c-.7-.3-1.4.4-1.1 1.1l4.3 10.3c.3.7 1.3.7 1.6-.1l1.3-3.7c.1-.3.3-.5.6-.6l3.7-1.3c.8-.3.8-1.3.1-1.6z" fill="#9747ff" stroke="#fff" stroke-width="1.2" stroke-linejoin="round"/>`;
      return {
        defs: dotGrid + shadow("lift", 3, 3, 0.25),
        body: fill("#f5f5f5") + fill("url(#grid)") + parts(LIGMA_COLORS)
          + `<rect x="${l}" y="${t}" width="${r - l}" height="${b - t}" fill="none" stroke="${blue}" stroke-width="4"/>` + handles
          + `<rect x="${midX - 82}" y="${b + 26}" width="164" height="44" rx="8" fill="${blue}"/>`
          + `<text x="${midX}" y="${b + 56}" text-anchor="middle" fill="#fff" font-family="Inter, -apple-system, 'Segoe UI', sans-serif" font-size="25" font-weight="600">${Math.round(r - l)} × ${Math.round(b - t)}</text>`
          + `<g filter="url(#lift)">${cursor}</g>`,
      };
    },
  },
  {
    id: "sticker", name: "Sticker", collection: "graphic",
    art() {
      const { fill, linear, shadow, parts, clips } = ligmaSvg;
      const stripes = `<pattern id="stripes" width="48" height="48" patternUnits="userSpaceOnUse" patternTransform="rotate(-35)"><rect width="22" height="48" fill="#fff" fill-opacity=".16"/></pattern>`;
      return {
        defs: stripes + clips() + shadow("peel", 14, 12, 0.32, "#7a4a00") + linear("shine", [[0.3, "#fff", 0], [0.48, "#fff", 0.45], [0.62, "#fff", 0]], [0, 0, 1, 1], false),
        body: fill("#ffcc33") + fill("url(#stripes)")
          + `<g filter="url(#peel)">${parts(["#fff", "#fff", "#fff", "#fff"], 'stroke="#fff" stroke-width="52" stroke-linejoin="round"')}</g>`
          + parts(LIGMA_COLORS)
          + `<g clip-path="url(#glyph)"><rect x="${LIGMA_G.x0}" y="${LIGMA_G.y0}" width="${LIGMA_G.right - LIGMA_G.x0}" height="${LIGMA_G.bottom - LIGMA_G.y0}" fill="url(#shine)"/></g>`,
      };
    },
  },
  {
    id: "bauhaus", name: "Bauhaus", collection: "graphic",
    art() {
      const { fill, grain, parts } = ligmaSvg;
      const g = LIGMA_G;
      return {
        defs: grain("paper", 0.16, 0.8),
        body: fill("#efe5cf")
          + `<circle cx="512" cy="512" r="340" fill="none" stroke="#1d1d1b" stroke-width="5"/>`
          + `<path d="M${g.x1 + 40} ${g.y0 + g.r}H860" stroke="#1d1d1b" stroke-width="10"/><circle cx="${g.bx + 40}" cy="${g.y0 + g.unit * 1.35}" r="44" fill="#d62828"/>`
          + parts(["#f2b705", "#d62828", "#1d3557", "#1d1d1b"])
          + `<rect width="1024" height="1024" filter="url(#paper)" style="mix-blend-mode:multiply"/>`,
      };
    },
  },

  // Playful
  {
    id: "candy", name: "Candy", collection: "playful",
    art() {
      const { fill, linear, shadow, clips, rand, mix } = ligmaSvg;
      const g = LIGMA_G;
      const colors = ["#2fe08a", "#ff3b6b", "#a46bff", "#2cc6ff"];
      const random = rand(11);
      const sprinkleColors = ["#ffffff", "#ffe066", "#7bdff2", "#b9fbc0", "#ff5d8f", "#a78bfa"];
      let sprinkles = "";
      for (let i = 0; i < 46; i++) {
        const x = 110 + random() * 804;
        const y = 110 + random() * 804;
        sprinkles += `<rect x="${(x - 17).toFixed(1)}" y="${(y - 6).toFixed(1)}" width="34" height="12" rx="6" fill="${sprinkleColors[i % sprinkleColors.length]}" transform="rotate(${Math.round(random() * 180)} ${x.toFixed(1)} ${y.toFixed(1)})"/>`;
      }
      return {
        defs: linear("tile-fill", [[0, "#ffd9ee"], [1, "#ff9cce"]]) + clips()
          + colors.map((c, i) => linear(`jelly${i}`, [[0, mix(c, "#ffffff", 0.35)], [0.55, c], [1, mix(c, "#000000", 0.22)]])).join("")
          + shadow("lift", 16, 14, 0.35, "#a0004d"),
        body: fill("url(#tile-fill)") + sprinkles
          + `<g filter="url(#lift)">${g.parts.map((d, i) => `<path d="${d}" fill="url(#jelly${i})"/>`).join("")}</g>`
          + g.parts.map((d, i) => {
            const [x, y] = g.centers[i];
            return `<g clip-path="url(#part${i})"><ellipse cx="${x - g.r * 0.32}" cy="${y - g.r * 0.48}" rx="${g.r * 0.48}" ry="${g.r * 0.2}" fill="#fff" opacity=".7" transform="rotate(-24 ${x} ${y})"/></g>`;
          }).join(""),
      };
    },
  },
  {
    id: "holo", name: "Holo", collection: "playful",
    art() {
      const { fill, parts, blur } = ligmaSvg;
      // Holographic foil: silvery ice in the middle, with the color pooling
      // toward the edges (violet, peach, mint, rose, teal, periwinkle) and
      // blurred together. The logo is outlined in ink over the same foil
      const fields = `<circle cx="80" cy="330" r="200" fill="#6f52aa"/><circle cx="100" cy="610" r="160" fill="#8ea0ba"/>`
        + `<circle cx="230" cy="130" r="180" fill="#a38ba5"/><circle cx="520" cy="90" r="190" fill="#e2ccad"/>`
        + `<circle cx="820" cy="130" r="170" fill="#bddad3"/><circle cx="940" cy="420" r="200" fill="#f2d7d7"/>`
        + `<circle cx="910" cy="720" r="180" fill="#99c8c2"/><circle cx="580" cy="950" r="210" fill="#b3c6ee"/>`
        + `<circle cx="220" cy="890" r="190" fill="#a9dccf"/>`
        + `<circle cx="520" cy="540" r="250" fill="#e6f1ee" opacity=".85"/><circle cx="350" cy="320" r="150" fill="#d9d6cc" opacity=".8"/>`
        + `<circle cx="700" cy="350" r="170" fill="#efe6ea"/><circle cx="430" cy="720" r="160" fill="#e3f4ef" opacity=".8"/>`
        + `<ellipse cx="560" cy="470" rx="380" ry="70" fill="#fff" opacity=".6" transform="rotate(-35 560 470)"/>`;
      const ink = "#2f2f2f";
      return {
        defs: blur("foil", 70),
        body: fill("#dfe6e6") + `<g filter="url(#foil)">${fields}</g>`
          + `<path d="${ligmaFrame()}" fill="${ink}" fill-rule="evenodd"/>`
          // Its outline reaches as far as Gilded's filled logo, LIGMA_FRAMED
          + parts(["none", "none", "none", "none"], `stroke="${ink}" stroke-opacity=".88" stroke-width="30" stroke-linejoin="round"`, ligmaGlyph(LIGMA_FRAMED.unit - 10)),
      };
    },
  },
  {
    id: "cosmos", name: "Cosmos", collection: "playful",
    art() {
      const { fill, radial, blur, rand, shadow } = ligmaSvg;
      const g = LIGMA_G;
      const random = rand(7);
      let stars = "";
      for (let i = 0; i < 90; i++) {
        stars += `<circle cx="${(100 + random() * 824).toFixed(1)}" cy="${(100 + random() * 824).toFixed(1)}" r="${(1 + random() * 2.8).toFixed(1)}" fill="#fff" opacity="${(0.35 + random() * 0.65).toFixed(2)}"/>`;
      }
      const [top, mid, low] = g.parts;
      const ring = (half) => `<ellipse cx="${g.bx}" cy="${g.by}" rx="${g.r * 1.75}" ry="${g.r * 0.42}" fill="none" stroke="#ffd6a5" stroke-width="12" opacity=".9" transform="rotate(-18 ${g.bx} ${g.by})"${half ? ' clip-path="url(#front)"' : ""}/>`;
      return {
        defs: radial("tile-fill", [[0, "#2c1a63"], [0.55, "#0c0724"], [1, "#03020b"]], [0.68, 0.3, 0.9])
          + radial("aurora", [[0, "#9dffd8"], [1, "#009e66"]], [0.35, 0.3, 0.8]) + radial("nova", [[0, "#ffa3b5"], [1, "#c9184a"]], [0.35, 0.3, 0.8])
          + radial("nebula", [[0, "#d7c6ff"], [1, "#5a189a"]], [0.35, 0.3, 0.8]) + radial("planet", [[0, "#a9dbff"], [0.6, "#2167c7"], [1, "#0a1b4a"]], [0.32, 0.28, 0.85])
          + blur("haze", 50) + shadow("glow", 0, 22, 0.55, "#b18cff")
          + `<clipPath id="front"><rect x="0" y="${g.by}" width="1024" height="600" transform="rotate(-18 ${g.bx} ${g.by})"/></clipPath>`,
        body: fill("url(#tile-fill)")
          + `<g filter="url(#haze)" opacity=".5"><circle cx="760" cy="260" r="150" fill="#7b2cbf"/><circle cx="250" cy="760" r="170" fill="#0077b6"/><circle cx="640" cy="560" r="110" fill="#c9184a"/></g>`
          + stars + ring(false)
          + `<g filter="url(#glow)"><path d="${top}" fill="url(#aurora)"/><path d="${mid}" fill="url(#nova)"/><path d="${low}" fill="url(#nebula)"/><circle cx="${g.bx}" cy="${g.by}" r="${g.r}" fill="url(#planet)"/></g>`
          + ring(true),
      };
    },
  },
  {
    id: "lava", name: "Lava", collection: "playful",
    art() {
      const { fill, linear, radial, parts } = ligmaSvg;
      const g = LIGMA_G;
      const goo = `<filter id="goo" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="20" result="b"/><feColorMatrix in="b" type="matrix" values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 28 -11" result="g"/><feComposite in="SourceGraphic" in2="g" operator="atop"/></filter>`;
      const blobs = `<circle cx="${g.bx + 18}" cy="${g.y0 + 120}" r="54" fill="#ff7b00"/><circle cx="${g.bx + 70}" cy="${g.y0 + 290}" r="34" fill="#ff3d6e"/><circle cx="${g.bx - 10}" cy="${g.by - g.r - 78}" r="26" fill="#ffc300"/><circle cx="${g.x0 - 30}" cy="${g.bottom + 40}" r="26" fill="#ff3d6e"/>`;
      return {
        defs: goo + linear("tile-fill", [[0, "#2a0638"], [1, "#6b0f3e"]]) + radial("heat", [[0, "#ff7a00", 0.55], [1, "#ff7a00", 0]], [0.5, 1, 0.75]) + radial("sheen", [[0, "#fff", 0.35], [1, "#fff", 0]], [0.3, 0.25, 0.6]),
        body: fill("url(#tile-fill)") + fill("url(#heat)")
          + `<g filter="url(#goo)">${parts(["#ffb000", "#ff5400", "#ff1f6b", "#ffd23f"])}${blobs}</g>`
          + parts(["url(#sheen)", "url(#sheen)", "url(#sheen)", "url(#sheen)"]),
      };
    },
  },
  {
    id: "snowfall", name: "Snowfall", collection: "playful",
    art() {
      const { fill, linear, rand, shadow, parts, clips } = ligmaSvg;
      const g = LIGMA_G;
      const random = rand(3);
      let flakes = "";
      for (let i = 0; i < 70; i++) {
        flakes += `<circle cx="${(100 + random() * 824).toFixed(1)}" cy="${(100 + random() * 824).toFixed(1)}" r="${(2.5 + random() * 6).toFixed(1)}" fill="#fff" opacity="${(0.5 + random() * 0.5).toFixed(2)}"/>`;
      }
      // A cap of snow with a wavy edge, `depth` deep from `top`, across `x0` to `x1`
      const cap = (x0, x1, top, depth) => {
        let d = `M${x0 - 20} ${top - 30}H${x1 + 20}V${top + depth}`;
        const steps = Math.ceil((x1 - x0 + 40) / 34);
        for (let i = 0; i < steps; i++) d += `q-17 ${i % 2 ? -16 : 18} -34 0`;
        return `${d}Z`;
      };
      return {
        defs: linear("tile-fill", [[0, "#d4edff"], [1, "#86c3f2"]]) + clips() + shadow("lift", 12, 12, 0.25, "#1d4f8a"),
        body: fill("url(#tile-fill)") + flakes
          + `<path d="M100 880C260 830 380 870 520 846S800 830 924 860V924H100Z" fill="#fff"/>`
          + `<g filter="url(#lift)">${parts(["#14c46a", "#f2213d", "#8a4df2", "#16a9ee"])}</g>`
          + `<g clip-path="url(#part0)"><path d="${cap(g.x0, g.x1, g.y0, 54)}" fill="#fff"/></g>`
          + `<g clip-path="url(#part3)"><path d="${cap(g.bx - g.r, g.bx + g.r, g.by - g.r, 50)}" fill="#fff"/></g>`
          + `<g clip-path="url(#part1)"><path d="${cap(g.x0, g.xc, g.y0 + g.unit, 30)}" fill="#fff"/></g>`
          + `<g clip-path="url(#part2)"><path d="${cap(g.x0, g.xc, g.y0 + 2 * g.unit, 30)}" fill="#fff"/></g>`,
      };
    },
  },
  {
    id: "pumpkin", name: "Pumpkin", collection: "playful",
    art() {
      const { fill, radial, blur, clips, rand, shadow } = ligmaSvg;
      const g = LIGMA_G;
      const random = rand(19);
      let stars = "";
      for (let i = 0; i < 30; i++) stars += `<circle cx="${(100 + random() * 824).toFixed(1)}" cy="${(100 + random() * 420).toFixed(1)}" r="${(1.2 + random() * 2.2).toFixed(1)}" fill="#fff" opacity="${(0.4 + random() * 0.5).toFixed(2)}"/>`;
      const ribs = (i) => {
        const [x, y] = g.centers[i];
        return [-0.55, -0.18, 0.18, 0.55].map((k) => `<path d="M${x + k * g.r} ${y - g.r - 10}Q${x + k * g.r * 1.7} ${y} ${x + k * g.r} ${y + g.r + 10}" fill="none" stroke="#b8430a" stroke-width="7" stroke-opacity=".55"/>`).join("");
      };
      const [x, y, r] = [g.bx, g.by, g.r];
      const face = `<path d="M${x - r * 0.5} ${y - r * 0.08}l${r * 0.2} ${-r * 0.38}l${r * 0.2} ${r * 0.38}z M${x + r * 0.1} ${y - r * 0.08}l${r * 0.2} ${-r * 0.38}l${r * 0.2} ${r * 0.38}z M${x - r * 0.58} ${y + r * 0.18}Q${x} ${y + r * 0.72} ${x + r * 0.58} ${y + r * 0.18}l${-r * 0.2} ${r * 0.14}l${-r * 0.16} ${-r * 0.12}l${-r * 0.22} ${r * 0.16}l${-r * 0.22} ${-r * 0.16}l${-r * 0.16} ${r * 0.12}z" fill="#ffe14d"/>`;
      return {
        defs: radial("tile-fill", [[0, "#4a2368"], [1, "#130920"]], [0.75, 0.2, 1]) + radial("pumpkin", [[0, "#ffab4a"], [1, "#e85d04"]], [0.35, 0.3, 0.9])
          + radial("leaf", [[0, "#7bd389"], [1, "#2b8a3e"]], [0.35, 0.3, 0.9]) + clips() + blur("glow", 12) + shadow("lift", 14, 14, 0.5),
        body: fill("url(#tile-fill)") + stars + `<circle cx="790" cy="250" r="64" fill="#fff4d6"/><circle cx="818" cy="232" r="58" fill="#3a1b56"/>`
          + `<g filter="url(#lift)"><path d="${g.parts[0]}" fill="url(#leaf)"/><path d="${g.parts[1]}" fill="url(#pumpkin)"/><path d="${g.parts[2]}" fill="url(#pumpkin)"/><path d="${g.parts[3]}" fill="url(#pumpkin)"/></g>`
          + `<g clip-path="url(#part0)"><path d="M${g.xc} ${g.y0 + g.unit - 6}Q${g.xc + 10} ${g.y0 + g.r} ${g.x1 - 30} ${g.y0 + 30}" fill="none" stroke="#1f6b2e" stroke-width="7"/></g>`
          + [1, 2, 3].map((i) => `<g clip-path="url(#part${i})">${ribs(i)}</g>`).join("")
          + `<g filter="url(#glow)" opacity=".9">${face}</g>${face}`,
      };
    },
  },
  {
    id: "pool", name: "Pool", collection: "playful",
    art() {
      const { fill, linear, clips, blur, shrink, mix } = ligmaSvg;
      const g = LIGMA_G;
      const caustics = `<filter id="caustics" x="0" y="0" width="1024" height="1024" filterUnits="userSpaceOnUse"><feTurbulence type="fractalNoise" baseFrequency="0.011" numOctaves="2" seed="6"/><feColorMatrix type="matrix" values="1 0 0 0 0  1 0 0 0 0  1 0 0 0 0  0 0 0 0 1"/><feComponentTransfer><feFuncR type="table" tableValues="0 0 0 0 1 0 0 0 0 1 0 0"/><feFuncG type="table" tableValues="0 0 0 0 1 0 0 0 0 1 0 0"/><feFuncB type="table" tableValues="0 0 0 0 1 0 0 0 0 1 0 0"/></feComponentTransfer><feColorMatrix type="matrix" values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  .45 0 0 0 0"/></filter>`;
      const colors = ["#7ef0a6", "#ff7a8c", "#c39bff"];
      const [x, y, r] = [g.bx, g.by, g.r];
      const mid = r * 0.72;
      const circumference = 2 * Math.PI * mid;
      const buoy = `<circle cx="${x}" cy="${y}" r="${mid}" fill="none" stroke="#fff" stroke-width="${r * 0.56}"/><circle cx="${x}" cy="${y}" r="${mid}" fill="none" stroke="#ff3b3b" stroke-width="${r * 0.56}" stroke-dasharray="${circumference / 8} ${circumference / 8}" transform="rotate(22 ${x} ${y})"/><circle cx="${x}" cy="${y}" r="${r}" fill="none" stroke="#fff" stroke-opacity=".7" stroke-width="3"/>`;
      return {
        defs: linear("tile-fill", [[0, "#48d7ff"], [1, "#0784d1"]]) + caustics + clips() + blur("soft", 12)
          + colors.map((c, i) => linear(`float${i}`, [[0, mix(c, "#ffffff", 0.4)], [0.6, c], [1, mix(c, "#000000", 0.15)]])).join(""),
        body: fill("url(#tile-fill)") + `<rect width="1024" height="1024" filter="url(#caustics)"/>`
          + `<g transform="translate(22 30)" filter="url(#soft)" opacity=".35">${g.parts.map((d, i) => `<path d="${d}" fill="#00305a" ${shrink(i, 0.95)}/>`).join("")}</g>`
          + g.parts.slice(0, 3).map((d, i) => `<path d="${d}" fill="url(#float${i})" ${shrink(i, 0.95)}/><path d="${d}" fill="none" stroke="#fff" stroke-opacity=".6" stroke-width="5" ${shrink(i, 0.86)}/>`).join("")
          + buoy,
      };
    },
  },
  {
    id: "bricks", name: "Bricks", collection: "playful",
    art() {
      const { fill, linear, shadow, mix, shrink } = ligmaSvg;
      const g = LIGMA_G;
      const colors = ["#00a843", "#e3000b", "#ffcd00", "#0055bf"];
      const studs = (i) => {
        const [cx, cy] = g.centers[i];
        const color = colors[i];
        return [[-1, -1], [1, -1], [-1, 1], [1, 1]].map(([sx, sy]) => {
          const x = cx + sx * g.unit * 0.22;
          const y = cy + sy * g.unit * 0.22;
          return `<circle cx="${x}" cy="${y + 5}" r="27" fill="${mix(color, "#000000", 0.3)}"/><circle cx="${x}" cy="${y}" r="27" fill="${mix(color, "#ffffff", 0.12)}"/><path d="M${x - 17} ${y - 8}a20 20 0 0 1 22 -12" fill="none" stroke="#fff" stroke-opacity=".6" stroke-width="5" stroke-linecap="round"/>`;
        }).join("");
      };
      return {
        defs: linear("tile-fill", [[0, "#f3f6fa"], [1, "#d3dce8"]]) + shadow("lift", 16, 14, 0.25, "#24324a"),
        body: fill("url(#tile-fill)")
          + `<g filter="url(#lift)">${g.parts.map((d, i) => `<g ${shrink(i, 0.97)}><path d="${d}" fill="${mix(colors[i], "#000000", 0.32)}" transform="translate(0 14)"/><path d="${d}" fill="${colors[i]}"/></g>`).join("")}</g>`
          + g.parts.map((d, i) => `<g ${shrink(i, 0.97)}>${studs(i)}</g>`).join(""),
      };
    },
  },
];

// The icon's SVG. `crop` trims the macOS grid's margin, for Windows,
// where icons fill their square. "tile" trims it to the tile itself, with
// no shadow, for Discord (scripts/discord-icons.mjs)
function ligmaAppIconSvg(id, crop = false) {
  const icon = LIGMA_APP_ICONS.find((icon) => icon.id === id && icon.art);
  if (!icon) return null;
  const { defs = "", body = "" } = icon.art();
  const box = crop === "tile" ? "100 100 824 824" : crop ? "88 88 848 848" : "0 0 1024 1024";
  return `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="${box}"><defs>`
    + `<clipPath id="tile"><path d="${LIGMA_TILE}"/></clipPath>`
    + `<filter id="tile-shadow" x="-10%" y="-10%" width="120%" height="125%"><feGaussianBlur in="SourceAlpha" stdDeviation="14"/><feOffset dy="12"/><feComponentTransfer><feFuncA type="linear" slope=".32"/></feComponentTransfer></filter>${defs}</defs>`
    + (crop === "tile" ? "" : `<path d="${LIGMA_TILE}" filter="url(#tile-shadow)"/>`)
    + `<g clip-path="url(#tile)">${body}<path d="${ligmaFrame(5, 412)}" fill="#fff" fill-opacity=".22" fill-rule="evenodd"/></g></svg>`;
}

// A URL for an <img> of the icon
function ligmaAppIconUrl(id, crop = false) {
  if (id === "ligma") return crop ? "app-icon-windows.png" : "app-icon-macos.png";
  const svg = ligmaAppIconSvg(id, crop);
  return svg && `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

// A CSS mask of the cropped tile, so app-icon-windows.png, which is near
// square, previews with the same corners as the others
const LIGMA_TILE_MASK = `url("data:image/svg+xml;charset=utf-8,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="88 88 848 848"><path d="${LIGMA_TILE}"/></svg>`)}")`;
