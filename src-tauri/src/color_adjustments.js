(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-color-adjustments";
  let state = __LIGMA_COLOR__;
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
  const paintsOf = (node, list) => {
    try {
      return Array.isArray(node[list]) ? node[list] : [];
    } catch {
      return [];
    }
  };
  const image = (paint) => paint?.type === "IMAGE" && !!paint.imageHash;
  const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

  // ---------- The adjustments ----------

  // What can be added, in the order the menu and the list show them, with
  // Photoshop's and Camera Raw's settings. Groups are split by a line in
  // the menu
  const CATALOG = [
    { id: "levels", name: "Levels", group: 0 },
    { id: "curves", name: "Curves", group: 0 },
    { id: "presence", name: "Presence", hint: "Texture, Clarity, Dehaze", group: 1 },
    { id: "sharpen", name: "Sharpen", group: 1 },
    { id: "vibrance", name: "Vibrance", group: 2 },
    { id: "hsl", name: "Hue/Saturation", group: 2 },
    { id: "balance", name: "Color Balance", group: 2 },
    { id: "mono", name: "Black & White", group: 2 },
    { id: "photo", name: "Photo Filter", group: 2 },
    { id: "mixer", name: "Channel Mixer", group: 2 },
    { id: "lookup", name: "Color Lookup", group: 2 },
    { id: "selective", name: "Selective Color", group: 2 },
    { id: "vignette", name: "Vignette", group: 3 },
    { id: "invert", name: "Invert", group: 3 },
    { id: "gradient", name: "Gradient Map", group: 3 },
  ];
  const NAMES = Object.fromEntries(CATALOG.map(({ id, name }) => [id, name]));
  // The order images were made in before the list could be reordered, for
  // settings saved without one, so they come out as they did
  const LEGACY = ["presence", "sharpen", "levels", "curves", "balance", "selective", "mixer", "hsl", "vibrance", "photo", "mono", "lookup", "gradient", "invert", "vignette"];

  // Hue/Saturation's ranges, as Photoshop's: each a sixth of the wheel,
  // fully within 15º of its hue and fading out to 45º
  const RANGES = [
    ["reds", "Reds", 0],
    ["yellows", "Yellows", 60],
    ["greens", "Greens", 120],
    ["cyans", "Cyans", 180],
    ["blues", "Blues", 240],
    ["magentas", "Magentas", 300],
  ];
  // Selective Color's, the six and three by lightness
  const SELECTIVE = [...RANGES.map(([id, name, hue]) => [id, name, `hsl(${hue} 85% 55%)`]), ["whites", "Whites", "#fff"], ["neutrals", "Neutrals", "#808080"], ["blacks", "Blacks", "#000"]];
  // Photoshop's photo filters and their colors
  const FILTERS = [
    ["warming85", "Warming Filter (85)", "#ec8a00"],
    ["warmingLBA", "Warming Filter (LBA)", "#fa9600"],
    ["warming81", "Warming Filter (81)", "#ebb113"],
    ["cooling80", "Cooling Filter (80)", "#006dff"],
    ["coolingLBB", "Cooling Filter (LBB)", "#005dff"],
    ["cooling82", "Cooling Filter (82)", "#00b5ff"],
    ["red", "Red", "#ea1a1a"],
    ["orange", "Orange", "#f38417"],
    ["yellow", "Yellow", "#f9e31c"],
    ["green", "Green", "#19c919"],
    ["cyan", "Cyan", "#1dcbea"],
    ["blue", "Blue", "#1d35ea"],
    ["violet", "Violet", "#9b1dea"],
    ["magenta", "Magenta", "#e318e3"],
    ["sepia", "Sepia", "#ac7a33"],
    ["deepRed", "Deep Red", "#ff0000"],
    ["deepBlue", "Deep Blue", "#0022cd"],
    ["deepEmerald", "Deep Emerald", "#008c00"],
    ["deepYellow", "Deep Yellow", "#ffd500"],
    ["underwater", "Underwater", "#00c1b1"],
    ["custom", "Custom color", null],
  ];
  const STYLES = [
    ["highlight", "Highlight priority"],
    ["color", "Color priority"],
    ["paint", "Paint overlay"],
  ];

  const hsl = () => ({ hue: 0, saturation: 0, lightness: 0 });
  const levels = () => [0, 1, 255, 0, 255];
  const curve = () => [
    [0, 0],
    [255, 255],
  ];
  // An adjustment as it's added: as Photoshop's, most change nothing until
  // a slider moves; the rest start where Photoshop's do
  const fresh = (id) =>
    ({
      levels: () => ({ rgb: levels(), red: levels(), green: levels(), blue: levels() }),
      curves: () => ({ rgb: curve(), red: curve(), green: curve(), blue: curve() }),
      presence: () => ({ texture: 0, clarity: 0, dehaze: 0 }),
      sharpen: () => ({ amount: 40, radius: 1, detail: 25, masking: 0 }),
      vibrance: () => ({ vibrance: 0, saturation: 0 }),
      hsl: () => ({ master: hsl(), ...Object.fromEntries(RANGES.map(([range]) => [range, hsl()])), colorize: false, tone: { hue: 0, saturation: 25, lightness: 0 } }),
      balance: () => ({ shadows: [0, 0, 0], midtones: [0, 0, 0], highlights: [0, 0, 0], preserve: true }),
      mono: () => ({ reds: 40, yellows: 60, greens: 40, cyans: 60, blues: 20, magentas: 80, tint: false, hue: 42, saturation: 20 }),
      photo: () => ({ filter: "warming85", color: "#ec8a00", density: 25, preserve: true }),
      mixer: () => ({ red: [100, 0, 0, 0], green: [0, 100, 0, 0], blue: [0, 0, 100, 0], monochrome: false }),
      lookup: () => ({ lut: "warm", name: "", size: 0, data: "", intensity: 100 }),
      selective: () => ({ ...Object.fromEntries(SELECTIVE.map(([range]) => [range, [0, 0, 0, 0]])), relative: true }),
      vignette: () => ({ amount: -25, midpoint: 50, roundness: 0, feather: 50, highlights: 0, style: "highlight" }),
      invert: () => ({}),
      gradient: () => ({ preset: "blackWhite", stops: GRADIENTS[0][2].map(([at, color]) => [at, color]), reverse: false, dither: false, method: "perceptual" }),
    })[id]();

  // Any shape saved or copied, as one Ligma can use: the template's keys
  // only, numbers finite and bounded, the lists their length
  const like = (template, value) => {
    if (typeof template === "number") return Number.isFinite(value) ? clamp(value, -1000, 1000) : template;
    if (typeof template === "boolean") return typeof value === "boolean" ? value : template;
    if (typeof template === "string") return typeof value === "string" && value.length <= 60000 ? value : template;
    if (Array.isArray(template)) {
      if (Array.isArray(template[0]) && typeof template[0][1] === "string") {
        // Gradient stops, at 0 to 100, sorted, 2 to 12 of them
        const stops = (Array.isArray(value) ? value : [])
          .filter((stop) => Array.isArray(stop) && Number.isFinite(stop[0]) && /^#[\da-f]{6}$/i.test(stop[1]))
          .map(([at, color]) => [clamp(Math.round(at), 0, 100), color.toLowerCase()])
          .sort((a, b) => a[0] - b[0])
          .slice(0, 12);
        return stops.length >= 2 ? stops : template;
      }
      if (Array.isArray(template[0])) {
        // Curve points, sorted, in 0 to 255, 2 to 16 of them
        const points = (Array.isArray(value) ? value : [])
          .filter((point) => Array.isArray(point) && Number.isFinite(point[0]) && Number.isFinite(point[1]))
          .map(([x, y]) => [clamp(Math.round(x), 0, 255), clamp(Math.round(y), 0, 255)])
          .sort((a, b) => a[0] - b[0])
          .filter((point, i, all) => !i || point[0] !== all[i - 1][0])
          .slice(0, 16);
        return points.length >= 2 ? points : template;
      }
      return template.map((item, i) => like(item, Array.isArray(value) ? value[i] : undefined));
    }
    const out = {};
    for (const [key, item] of Object.entries(template)) out[key] = like(item, value?.[key]);
    return out;
  };
  // Settings: `{ v: 2, items: { [id]: { on, ...values } }, order }`, an
  // item for each adjustment added and the order they're applied in, the
  // list's from the bottom up. Ligma's first ones, Vibrance and
  // Hue/Saturation at the top level, come in as those two
  const normal = (s) => {
    const made = itemsOf(s);
    const given = Array.isArray(s?.order) ? s.order.filter((id) => typeof id === "string" && made[id]) : [];
    const order = [...new Set(given)];
    for (const id of LEGACY) if (made[id] && !order.includes(id)) order.push(id);
    return { v: 2, items: made, order };
  };
  const itemsOf = (s) => {
    const items = {};
    if (s && typeof s === "object" && s.v === 2 && s.items && typeof s.items === "object") {
      for (const { id } of CATALOG) {
        const item = s.items[id];
        if (!item || typeof item !== "object") continue;
        items[id] = { on: item.on !== false, ...like(fresh(id), item) };
      }
    } else if (s && typeof s === "object") {
      const number = (value) => (Number.isFinite(value) ? value : 0);
      if (number(s.vibrance)) items.vibrance = { on: true, vibrance: number(s.vibrance), saturation: 0 };
      const hue = { master: { hue: number(s.hue), saturation: number(s.saturation), lightness: number(s.lightness) } };
      for (const [range] of RANGES) hue[range] = like(hsl(), s[range]);
      const shifts = [hue.master, ...RANGES.map(([range]) => hue[range])];
      if (shifts.some((values) => values.hue || values.saturation || values.lightness)) items.hsl = { on: true, ...like(fresh("hsl"), hue) };
    }
    return items;
  };
  const blank = () => ({ v: 2, items: {}, order: [] });
  // `s` with those `copied` in place of its own, or added, on top
  const withPasted = (s, copied) => ({
    v: 2,
    items: { ...s.items, ...structuredClone(copied.items) },
    order: [...orderOf(s).filter((id) => !copied.order.includes(id)), ...copied.order],
  });
  const orderOf = (s) => (Array.isArray(s.order) ? s.order.filter((id) => s.items[id]) : LEGACY.filter((id) => s.items[id]));
  // Whether an item changes nothing as it is
  const identity = (id, item) => {
    if (!item.on) return true;
    const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
    switch (id) {
      case "levels":
      case "curves":
      case "presence":
      case "vibrance":
      case "mixer":
        return same(like(fresh(id), item), fresh(id));
      case "hsl":
        return !item.colorize && same(like(fresh(id), { ...item, tone: undefined, colorize: false }), fresh(id));
      case "balance":
      case "selective":
        return same(like(fresh(id), { ...item, preserve: undefined, relative: undefined }), like(fresh(id), {}));
      case "sharpen":
        return !item.amount;
      case "photo":
        return !item.density;
      case "lookup":
        return !item.intensity || (item.lut === "file" && !item.data);
      case "vignette":
        return !item.amount;
      default:
        return false;
    }
  };
  const isBlank = (s) => Object.entries(s.items).every(([id, item]) => identity(id, item));
  const added = (s) => Object.keys(s.items).length > 0;

  // What each image on a layer was made from, `{ [hash]: { original,
  // settings } }`. The original itself has one too while its adjustments
  // are all hidden or unchanged, so they're kept
  const PLUGIN = "ligma";
  const DATA = "color";
  const madeOn = (node) => {
    try {
      return JSON.parse(node.getSharedPluginData(PLUGIN, DATA) || "{}");
    } catch {
      return {};
    }
  };
  // Kept only for images the layer still shows
  const keepMade = (node, made) => {
    const hashes = new Set([...paintsOf(node, "fills"), ...paintsOf(node, "strokes")].map((paint) => paint.imageHash));
    for (const hash of Object.keys(made)) if (!hashes.has(hash)) delete made[hash];
    try {
      node.setSharedPluginData(PLUGIN, DATA, Object.keys(made).length ? JSON.stringify(made) : "");
    } catch {}
  };
  // A paint's settings, none for one Ligma didn't adjust
  const settingsOf = (node, paint) => normal(image(paint) ? madeOn(node)[paint.imageHash]?.settings : null);
  const originalOf = (node, hash) => madeOn(node)[hash]?.original ?? hash;

  // ---------- Making the image ----------

  // `renderer`, `prepare`, `encode` and `core` use nothing outside them, so
  // a worker runs them as they are. The worker makes every picture, off
  // the page's thread, so Figma and the sliders keep up; where workers or
  // their WebGL can't start, the page makes them with the same code

  // WebGL drawing a picture with the adjustments' uniforms `u` on `canvas`.
  // Presence and Sharpen need the picture blurred at a few sizes first, a
  // pyramid of halvings; the rest is one pass. Pictures come in with their
  // top row at texture row 0, what's drawn has it at the top, so passes
  // read pictures by `uv` and what passes drew by `fuv`
  const renderer = (canvas) => {
    const gl = canvas.getContext("webgl", { premultipliedAlpha: false, preserveDrawingBuffer: true, antialias: false });
    if (!gl) throw new Error("no WebGL");
    const VERTEX = `attribute vec2 p; varying vec2 uv; varying vec2 fuv; void main() { uv = vec2(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5); fuv = vec2(uv.x, 1.0 - uv.y); gl_Position = vec4(p, 0.0, 1.0); }`;
    const HEAD = `precision highp float; varying vec2 uv; varying vec2 fuv; uniform sampler2D tex; uniform vec2 texel;`;
    // 4 reads, a 2 by 2 box: half the size
    const DOWN = (flip) => `${HEAD} void main() { vec2 q = ${flip ? "fuv" : "uv"}; gl_FragColor = 0.25 * (texture2D(tex, q + texel * vec2(-0.5, -0.5)) + texture2D(tex, q + texel * vec2(0.5, -0.5)) + texture2D(tex, q + texel * vec2(-0.5, 0.5)) + texture2D(tex, q + texel * vec2(0.5, 0.5))); }`;
    // A 9 tap Gaussian along `texel`, in 5 bilinear reads
    const BLUR = `${HEAD} void main() { vec4 c = texture2D(tex, fuv) * 0.227027; c += (texture2D(tex, fuv + texel * 1.384615) + texture2D(tex, fuv - texel * 1.384615)) * 0.316216; c += (texture2D(tex, fuv + texel * 3.230769) + texture2D(tex, fuv - texel * 3.230769)) * 0.07027; gl_FragColor = c; }`;
    // Haze's dark channel, from the blurred picture
    const DARK = `${HEAD} void main() { vec3 c = texture2D(tex, fuv).rgb; gl_FragColor = vec4(vec3(min(c.r, min(c.g, c.b))), 1.0); }`;
    const FINAL_HEAD = `
      precision highp float;
      varying vec2 uv;
      varying vec2 fuv;
      uniform sampler2D src, blur1, blur3, blur5, dark5, tone, lut, grad;
      uniform vec2 texel;
      uniform vec4 sharp;
      uniform vec3 presence;
      uniform vec3 air;
      uniform vec3 cbS, cbM, cbH;
      uniform vec2 cb;
      uniform vec4 sel[9];
      uniform vec2 selFlags;
      uniform mat3 mixer;
      uniform vec3 mixC;
      uniform vec2 mixFlags;
      uniform vec3 master;
      uniform vec3 ranges[6];
      uniform vec4 colorize;
      uniform vec3 vib;
      uniform vec4 photo;
      uniform vec2 photoFlags;
      uniform float bw[6];
      uniform vec4 bwFlags;
      uniform vec2 lutInfo;
      uniform vec2 gradFlags;
      uniform float invert;
      uniform vec4 vig;
      uniform vec4 vig2;
      float lum(vec3 c) { return dot(c, vec3(0.299, 0.587, 0.114)); }
      vec3 toHsl(vec3 c) {
        float hi = max(c.r, max(c.g, c.b));
        float lo = min(c.r, min(c.g, c.b));
        float l = (hi + lo) * 0.5;
        float d = hi - lo;
        if (d < 1e-5) return vec3(0.0, 0.0, l);
        // Rounding can take a pure color's a hair past 1, and pow() of
        // less than 0 is NaN, which comes out as gray specks
        float s = min(l > 0.5 ? d / (2.0 - hi - lo) : d / (hi + lo), 1.0);
        float h = hi == c.r ? (c.g - c.b) / d + (c.g < c.b ? 6.0 : 0.0) : hi == c.g ? (c.b - c.r) / d + 2.0 : (c.r - c.g) / d + 4.0;
        return vec3(h / 6.0, s, l);
      }
      float channel(float p, float q, float t) {
        t = fract(t);
        if (t < 1.0 / 6.0) return p + (q - p) * 6.0 * t;
        if (t < 0.5) return q;
        if (t < 2.0 / 3.0) return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
        return p;
      }
      vec3 toRgb(vec3 c) {
        if (c.y <= 0.0) return vec3(c.z);
        float q = c.z < 0.5 ? c.z * (1.0 + c.y) : c.z + c.y - c.z * c.y;
        float p = 2.0 * c.z - q;
        return vec3(channel(p, q, c.x + 1.0 / 3.0), channel(p, q, c.x), channel(p, q, c.x - 1.0 / 3.0));
      }
      float saturate(float s, float by) { return by < 0.0 ? s * (1.0 + by) : 1.0 - pow(max(1.0 - s, 0.0), 1.0 + 2.0 * by); }
      float lighten(float l, float by) { return by < 0.0 ? l * (1.0 + by) : l + (1.0 - l) * by; }
      float selective(float v, float a, float k) {
        float m = selFlags.y > 0.5 ? 1.0 - v : 1.0;
        return clamp(((-1.0 - a) * k - a) * m, -v, 1.0 - v);
      }
      vec3 lookup(vec3 c) {
        float n = lutInfo.y;
        float b = clamp(c.b, 0.0, 1.0) * (n - 1.0);
        float z0 = floor(b);
        float z1 = min(z0 + 1.0, n - 1.0);
        vec2 at = vec2(clamp(c.r, 0.0, 1.0) * (n - 1.0) + 0.5, (clamp(c.g, 0.0, 1.0) * (n - 1.0) + 0.5) / n);
        vec3 a = texture2D(lut, vec2((z0 * n + at.x) / (n * n), at.y)).rgb;
        vec3 d = texture2D(lut, vec2((z1 * n + at.x) / (n * n), at.y)).rgb;
        return mix(a, d, b - z0);
      }
      `;
    // Each adjustment's step, changing the color `c`. The image is made
    // with the steps in the list's order, one shader for each order
    const BLOCKS = {
      // Dehaze, as the dark channel prior: haze lifts the darkest channel
      // toward the air's color, so taking it away divides it out. Texture,
      // the middle sized detail; Clarity, the large, in the midtones
      presence: `
        if (presence.z > 0.0) {
          float dark = texture2D(dark5, fuv).r;
          float t = max(1.0 - 0.95 * presence.z * dark / max(max(air.r, max(air.g, air.b)), 0.05), 0.12);
          c = (c - air) / t + air;
        } else if (presence.z < 0.0) {
          c = mix(c, air, -presence.z * 0.6);
        }
        if (presence.x != 0.0) c += presence.x * 2.0 * (lum(mix(c, texture2D(blur1, fuv).rgb, 0.5)) - lum(texture2D(blur3, fuv).rgb));
        if (presence.y != 0.0) {
          float l = lum(c);
          c += presence.y * 1.4 * (l - lum(texture2D(blur5, fuv).rgb)) * (1.0 - pow(abs(2.0 * l - 1.0), 2.0));
        }`,
      // Sharpen: the picture less its blur, the detail, added back to its
      // lightness. Detail lets more of the finest through, Masking keeps
      // it to edges
      sharpen: `
        vec2 r = texel * sharp.y;
        vec3 near = texture2D(src, uv + vec2(r.x, 0.0)).rgb + texture2D(src, uv - vec2(r.x, 0.0)).rgb + texture2D(src, uv + vec2(0.0, r.y)).rgb + texture2D(src, uv - vec2(0.0, r.y)).rgb;
        vec3 far = texture2D(src, uv + r).rgb + texture2D(src, uv - r).rgb + texture2D(src, uv + vec2(r.x, -r.y)).rgb + texture2D(src, uv + vec2(-r.x, r.y)).rgb;
        vec3 blurred = (pixel.rgb * 4.0 + near * 2.0 + far) / 16.0;
        float detail = lum(pixel.rgb - blurred);
        float limit = mix(0.03, 0.4, sharp.z);
        float edge = length(near - pixel.rgb * 4.0);
        float mask = sharp.w > 0.0 ? smoothstep(sharp.w * 0.25, sharp.w * 0.25 + 0.05, edge) : 1.0;
        c += sharp.x * clamp(detail, -limit, limit) * mask;`,
      // Levels and Curves, a curve for each channel, the tone texture's
      // first row and its second
      levels: `
        vec3 at = c * (255.0 / 256.0) + 0.5 / 256.0;
        c = vec3(texture2D(tone, vec2(at.r, 0.25)).r, texture2D(tone, vec2(at.g, 0.25)).g, texture2D(tone, vec2(at.b, 0.25)).b);`,
      curves: `
        vec3 at = c * (255.0 / 256.0) + 0.5 / 256.0;
        c = vec3(texture2D(tone, vec2(at.r, 0.75)).r, texture2D(tone, vec2(at.g, 0.75)).g, texture2D(tone, vec2(at.b, 0.75)).b);`,
      // Color Balance, as GIMP's: shadows, midtones and highlights by
      // lightness, each moved toward red, green or blue or away
      balance: `
        float l0 = toHsl(c).z;
        float ws = clamp((l0 - 0.333) / -0.25 + 0.5, 0.0, 1.0) * 0.7;
        float wm = clamp((l0 - 0.333) / 0.25 + 0.5, 0.0, 1.0) * clamp((l0 + 0.333 - 1.0) / -0.25 + 0.5, 0.0, 1.0) * 0.7;
        float wh = clamp((l0 + 0.333 - 1.0) / 0.25 + 0.5, 0.0, 1.0) * 0.7;
        vec3 n = clamp(c + cbS * ws + cbM * wm + cbH * wh, 0.0, 1.0);
        if (cb.y > 0.5) {
          vec3 h = toHsl(n);
          h.z = l0;
          n = toRgb(h);
        }
        c = n;`,
      // Selective Color: each color's share of a pixel, from how far its
      // channels are apart, takes that color's cyan, magenta, yellow and
      // black
      selective: `
        float hi = max(c.r, max(c.g, c.b));
        float lo = min(c.r, min(c.g, c.b));
        float mid = c.r + c.g + c.b - hi - lo;
        float w[9];
        w[0] = hi == c.r ? hi - mid : 0.0;
        w[1] = lo == c.b ? mid - lo : 0.0;
        w[2] = hi == c.g ? hi - mid : 0.0;
        w[3] = lo == c.r ? mid - lo : 0.0;
        w[4] = hi == c.b ? hi - mid : 0.0;
        w[5] = lo == c.g ? mid - lo : 0.0;
        w[6] = lo > 0.5 ? (lo - 0.5) * 2.0 : 0.0;
        w[7] = clamp(1.0 - (abs(hi - 0.5) + abs(lo - 0.5)), 0.0, 1.0);
        w[8] = hi < 0.5 ? (0.5 - hi) * 2.0 : 0.0;
        vec3 d = vec3(0.0);
        for (int i = 0; i < 9; i++) {
          vec4 a = sel[i];
          d += w[i] * vec3(selective(c.r, a.x, a.w), selective(c.g, a.y, a.w), selective(c.b, a.z, a.w));
        }
        c = c + d;`,
      mixer: `
        vec3 o = mixer * c + mixC;
        c = mixFlags.y > 0.5 ? vec3(o.r) : o;`,
      // Hue/Saturation, as before, or Colorize, a color by lightness
      hsl: `
        vec3 h = toHsl(c);
        if (colorize.x > 0.5) {
          h = vec3(colorize.y, colorize.z, lighten(h.z, colorize.w));
        } else {
          float colorful = smoothstep(0.0, 0.06, h.y * (1.0 - abs(2.0 * h.z - 1.0)));
          vec3 shift = vec3(0.0);
          for (int i = 0; i < 6; i++) {
            float away = abs(fract(h.x - float(i) / 6.0 + 0.5) - 0.5) * 360.0;
            shift += ranges[i] * clamp((45.0 - away) / 30.0, 0.0, 1.0) * colorful;
          }
          h.x = fract(h.x + master.x + shift.x);
          h.y = saturate(saturate(h.y, shift.y), master.y);
          h.z = lighten(lighten(h.z, shift.z), master.z);
        }
        c = toRgb(h);`,
      // Vibrance: the less colorful a color, the more it changes, and
      // skin's hues, oranges, less
      vibrance: `
        vec3 h = toHsl(c);
        float skin = clamp(1.0 - abs(h.x * 360.0 - 25.0) / 30.0, 0.0, 1.0) * smoothstep(0.08, 0.25, h.y);
        h.y = vib.y < 0.0 ? h.y * (1.0 + vib.y * (1.0 - 0.5 * h.y)) : h.y * (1.0 + vib.y * 2.0 * (1.0 - h.y) * (1.0 - h.y) * (1.0 - 0.7 * skin));
        h.y = saturate(clamp(h.y, 0.0, 1.0), vib.z);
        c = toRgb(h);`,
      photo: `
        float l0 = lum(c);
        vec3 n = mix(c, c * photo.rgb, photo.a);
        if (photoFlags.y > 0.5) n *= l0 / max(lum(n), 1e-4);
        c = n;`,
      // Black & White, as Photoshop's: the gray from the color's main
      // channel and the pair with the next, each by its slider
      mono: `
        float hi = max(c.r, max(c.g, c.b));
        float lo = min(c.r, min(c.g, c.b));
        float mid = c.r + c.g + c.b - hi - lo;
        float primary = hi == c.r ? bw[0] : hi == c.g ? bw[2] : bw[4];
        float secondary = lo == c.b ? bw[1] : lo == c.r ? bw[3] : bw[5];
        float gray = clamp(lo + (hi - mid) * primary + (mid - lo) * secondary, 0.0, 1.0);
        c = bwFlags.y > 0.5 ? toRgb(vec3(bwFlags.z, bwFlags.w, gray)) : vec3(gray);`,
      lookup: `
        c = mix(c, lookup(c), lutInfo.x);`,
      // Gradient Map: lightness to the gradient's color, dithered so its
      // steps don't band
      gradient: `
        float t = clamp(lum(c), 0.0, 1.0);
        c = texture2D(grad, vec2(t * (255.0 / 256.0) + 0.5 / 256.0, 0.5)).rgb;
        if (gradFlags.y > 0.5) c += (fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453) - 0.5) / 255.0;`,
      invert: `
        c = 1.0 - c;`,
      // Vignette, as Camera Raw's: Midpoint where it starts, Feather how
      // softly, Roundness from a rectangle to a circle
      vignette: `
        vec2 d = (uv - 0.5) * 2.0;
        float aspect = vig2.z;
        vec2 circle = d * vec2(aspect, 1.0) / max(aspect, 1.0);
        vec2 q = abs(mix(d, circle, max(vig.z, 0.0)));
        float power = 2.0 + max(-vig.z, 0.0) * 6.0;
        float dist = pow(pow(q.x, power) + pow(q.y, power), 1.0 / power);
        float start = mix(0.25, 1.35, vig.y);
        float soft = mix(0.04, 1.2, vig.w);
        float mask = smoothstep(start - soft * 0.5, start + soft * 0.5, dist) * abs(vig.x);
        if (vig2.y > 1.5) {
          c = mix(c, vec3(vig.x < 0.0 ? 0.0 : 1.0), mask);
        } else if (vig.x < 0.0) {
          float keep = vig2.y < 0.5 ? vig2.x * smoothstep(0.55, 1.0, lum(c)) : 0.0;
          c *= 1.0 - mask * (1.0 - keep);
        } else {
          c = 1.0 - (1.0 - c) * (1.0 - mask);
        }`,
    };
    // Each step in a block of its own, so their names don't meet, and the
    // color kept in range after each
    const finalOf = (order) => `${FINAL_HEAD}
      void main() {
        vec4 pixel = texture2D(src, uv);
        vec3 c = pixel.rgb;
        ${order.map((id) => `{${BLOCKS[id]}\n        }\n        c = clamp(c, 0.0, 1.0);`).join("\n        ")}
        gl_FragColor = vec4(clamp(c, 0.0, 1.0), pixel.a);
      }`;
    const compile = (fragment) => {
      const shader = (type, source) => {
        const made = gl.createShader(type);
        gl.shaderSource(made, source);
        gl.compileShader(made);
        return made;
      };
      const program = gl.createProgram();
      gl.attachShader(program, shader(gl.VERTEX_SHADER, VERTEX));
      gl.attachShader(program, shader(gl.FRAGMENT_SHADER, fragment));
      gl.bindAttribLocation(program, 0, "p");
      gl.linkProgram(program);
      if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error("no shader");
      const where = new Map();
      program.at = (name) => {
        if (!where.has(name)) where.set(name, gl.getUniformLocation(program, name));
        return where.get(name);
      };
      return program;
    };
    const programs = { downSource: compile(DOWN(false)), down: compile(DOWN(true)), blur: compile(BLUR), dark: compile(DARK), };
    // The last few orders' shaders
    const finals = new Map();
    const finalFor = (order) => {
      const key = order.join();
      let made = finals.get(key);
      if (made) {
        finals.delete(key);
      } else {
        made = compile(finalOf(order));
        while (finals.size >= 6) {
          const [oldest, program] = finals.entries().next().value;
          finals.delete(oldest);
          gl.deleteProgram(program);
        }
      }
      finals.set(key, made);
      return made;
    };
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    const texture = (filter = gl.LINEAR) => {
      const made = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, made);
      for (const [key, value] of [
        [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE],
        [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE],
        [gl.TEXTURE_MIN_FILTER, filter],
        [gl.TEXTURE_MAG_FILTER, filter],
      ])
        gl.texParameteri(gl.TEXTURE_2D, key, value);
      return made;
    };
    const source = texture();
    const tone = texture();
    const lut = texture();
    const gradient = texture();
    const none = texture();
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(4));
    // Targets to draw into, by name, kept while their size stays
    const targets = new Map();
    const target = (name, width, height) => {
      let made = targets.get(name);
      if (made && made.width === width && made.height === height) return made;
      if (made) {
        gl.deleteTexture(made.texture);
        gl.deleteFramebuffer(made.buffer);
      }
      made = { width, height, texture: texture(), buffer: gl.createFramebuffer() };
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
      gl.bindFramebuffer(gl.FRAMEBUFFER, made.buffer);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, made.texture, 0);
      targets.set(name, made);
      return made;
    };
    const pass = (program, into, from, texelX, texelY) => {
      gl.useProgram(program);
      gl.bindFramebuffer(gl.FRAMEBUFFER, into.buffer);
      gl.viewport(0, 0, into.width, into.height);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, from);
      gl.uniform1i(program.at("tex"), 0);
      gl.uniform2f(program.at("texel"), texelX, texelY);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    };
    const blurred = (name, from) => {
      const across = target(`${name}-x`, from.width, from.height);
      pass(programs.blur, across, from.texture, 1 / from.width, 0);
      const done = target(`${name}-b`, from.width, from.height);
      pass(programs.blur, done, across.texture, 0, 1 / from.height);
      return done;
    };
    let drawn = null;
    let toneKey = null;
    let lutKey = null;
    let gradientKey = null;
    return {
      draw: (picture, u) => {
        if (gl.isContextLost()) throw new Error("WebGL lost");
        const { width, height } = picture;
        if (drawn !== picture) {
          gl.activeTexture(gl.TEXTURE0);
          gl.bindTexture(gl.TEXTURE_2D, source);
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, picture);
          drawn = picture;
        }
        const spatial = u.order.includes("presence");
        const levels = [source, null, null, null, null, null];
        if (spatial) {
          let from = { texture: source, width, height };
          for (let i = 1; i <= 5; i++) {
            const into = target(`l${i}`, Math.max(1, Math.ceil(from.width / 2)), Math.max(1, Math.ceil(from.height / 2)));
            pass(i === 1 ? programs.downSource : programs.down, into, from.texture, 1 / from.width, 1 / from.height);
            levels[i] = into;
            from = into;
          }
          levels.blur1 = blurred("l1", levels[1]);
          levels.blur3 = blurred("l3", levels[3]);
          levels.blur5 = blurred("l5", levels[5]);
          const dark = target("dark", levels[5].width, levels[5].height);
          pass(programs.dark, dark, levels.blur5.texture, 0, 0);
          levels.dark5 = blurred("dark", dark);
        }
        const final = finalFor(u.order);
        gl.useProgram(final);
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        if (canvas.width !== width || canvas.height !== height) Object.assign(canvas, { width, height });
        gl.viewport(0, 0, width, height);
        const bind = (unit, name, made) => {
          gl.activeTexture(gl.TEXTURE0 + unit);
          gl.bindTexture(gl.TEXTURE_2D, made ?? none);
          gl.uniform1i(final.at(name), unit);
        };
        bind(0, "src", source);
        bind(1, "blur1", levels.blur1?.texture);
        bind(2, "blur3", levels.blur3?.texture);
        bind(3, "blur5", levels.blur5?.texture);
        bind(4, "dark5", levels.dark5?.texture);
        gl.activeTexture(gl.TEXTURE5);
        gl.bindTexture(gl.TEXTURE_2D, u.tone ? tone : none);
        if (u.tone && u.toneKey !== toneKey) {
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 256, 2, 0, gl.RGBA, gl.UNSIGNED_BYTE, u.tone);
          toneKey = u.toneKey;
        }
        gl.uniform1i(final.at("tone"), 5);
        gl.activeTexture(gl.TEXTURE6);
        gl.bindTexture(gl.TEXTURE_2D, u.lut ? lut : none);
        if (u.lut && u.lut.key !== lutKey) {
          const n = u.lut.size;
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, n * n, n, 0, gl.RGBA, gl.UNSIGNED_BYTE, u.lut.data);
          lutKey = u.lut.key;
        }
        gl.uniform1i(final.at("lut"), 6);
        gl.activeTexture(gl.TEXTURE7);
        gl.bindTexture(gl.TEXTURE_2D, u.grad ? gradient : none);
        if (u.grad && u.gradKey !== gradientKey) {
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 256, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, u.grad);
          gradientKey = u.gradKey;
        }
        gl.uniform1i(final.at("grad"), 7);
        const set = (kind, name, value) => gl[`uniform${kind}`](final.at(name), value);
        set("2fv", "texel", [1 / width, 1 / height]);
        // Sharpen's radius is in the full size picture's pixels
        set("4fv", "sharp", [u.sharp[0], Math.max(0.35, u.sharp[1] * u.scale), u.sharp[2], u.sharp[3]]);
        set("3fv", "presence", u.presence);
        set("3fv", "air", u.air);
        set("3fv", "cbS", u.cb.s);
        set("3fv", "cbM", u.cb.m);
        set("3fv", "cbH", u.cb.h);
        set("2fv", "cb", u.cb.flags);
        set("4fv", "sel", u.sel.values);
        set("2fv", "selFlags", u.sel.flags);
        gl.uniformMatrix3fv(final.at("mixer"), false, u.mixer.m);
        set("3fv", "mixC", u.mixer.c);
        set("2fv", "mixFlags", u.mixer.flags);
        set("3fv", "master", u.hsl.master);
        set("3fv", "ranges", u.hsl.ranges);
        set("4fv", "colorize", u.hsl.colorize);
        set("3fv", "vib", u.vib);
        set("4fv", "photo", u.photo);
        set("2fv", "photoFlags", u.photoFlags);
        set("1fv", "bw", u.bw);
        set("4fv", "bwFlags", u.bwFlags);
        set("2fv", "lutInfo", u.lut ? [u.lut.intensity, u.lut.size] : [0, 2]);
        set("2fv", "gradFlags", u.gradFlags);
        set("1f", "invert", u.invert);
        set("4fv", "vig", u.vig);
        set("4fv", "vig2", [u.vig2[0], u.vig2[1], width / height, u.vig2[2]]);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        return canvas;
      },
    };
  };

  // An original's bytes as pictures to make from, at sizes asked for, with
  // its histogram and its haze's color, read from a small one. `size` is
  // its size, if known, so small ones decode alone. JPEGs stay JPEGs
  const prepare = async (bytes, size) => {
    const blob = new Blob([bytes]);
    const options = { premultiplyAlpha: "none", colorSpaceConversion: "none" };
    let fullPicture = size ? null : await createImageBitmap(blob, options);
    const { width, height } = size ?? fullPicture;
    const pictures = new Map();
    const full = () => (fullPicture ? Promise.resolve(fullPicture) : (pictures.get("full") ?? pictures.set("full", createImageBitmap(blob, options)).get("full")));
    // The smallest of 512, 1024 and 2048 pixels across at least `side`, or
    // the full size one
    const at = (side) => {
      const step = [512, 1024, 2048].find((each) => each >= side);
      if (!step || step >= Math.max(width, height)) return full();
      if (!pictures.has(step)) {
        const scale = step / Math.max(width, height);
        pictures.set(step, createImageBitmap(blob, { ...options, resizeWidth: Math.max(1, Math.round(width * scale)), resizeHeight: Math.max(1, Math.round(height * scale)), resizeQuality: "medium" }));
      }
      return pictures.get(step);
    };
    const small = await at(512);
    const histogram = { r: new Uint32Array(256), g: new Uint32Array(256), b: new Uint32Array(256), l: new Uint32Array(256) };
    let air = [0.9, 0.9, 0.9];
    try {
      const canvas = typeof OffscreenCanvas === "function" ? new OffscreenCanvas(small.width, small.height) : Object.assign(document.createElement("canvas"), { width: small.width, height: small.height });
      const context = canvas.getContext("2d", { willReadFrequently: true });
      context.drawImage(small, 0, 0);
      const { data } = context.getImageData(0, 0, small.width, small.height);
      const darks = new Uint32Array(256);
      for (let i = 0; i < data.length; i += 4) {
        const [r, g, b] = [data[i], data[i + 1], data[i + 2]];
        histogram.r[r]++;
        histogram.g[g]++;
        histogram.b[b]++;
        histogram.l[Math.round(r * 0.299 + g * 0.587 + b * 0.114)]++;
        darks[Math.min(r, g, b)]++;
      }
      // The air's color: the mean of the brightest thousandth by the
      // darkest channel, the haziest pixels
      const pixels = data.length / 4;
      let from = 255;
      for (let count = 0; from > 0 && count + darks[from] < pixels * 0.001; from--) count += darks[from];
      const sum = [0, 0, 0];
      let count = 0;
      for (let i = 0; i < data.length; i += 4) {
        if (Math.min(data[i], data[i + 1], data[i + 2]) >= from) {
          sum[0] += data[i];
          sum[1] += data[i + 1];
          sum[2] += data[i + 2];
          count++;
        }
      }
      if (count) air = sum.map((value) => value / count / 255);
    } catch {}
    return {
      width,
      height,
      jpeg: bytes[0] === 0xff && bytes[1] === 0xd8,
      histogram,
      air,
      at,
      full,
      close: () => {
        fullPicture?.close();
        for (const picture of pictures.values()) picture.then((each) => each.close()).catch(() => {});
      },
    };
  };

  const encode = (canvas, jpeg, quality) => {
    const type = jpeg ? "image/jpeg" : "image/png";
    if (canvas.convertToBlob) return canvas.convertToBlob({ type, quality });
    return new Promise((resolve, reject) => canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("not made"))), type, quality));
  };

  // The originals, the last few kept, and what's made from them: `draw` a
  // picture at least `side` across, as an ImageBitmap where the canvas can
  // give one, and `make` image bytes, `side` across or full size. Each
  // draws and reads back with nothing awaited between, so one can't spoil
  // another. Tone curves and lookup tables come once, by key
  const core = (makeCanvas) => {
    const sources = new Map();
    const luts = new Map();
    let gpu = null;
    const sourceOf = async (hash) => {
      const source = await sources.get(hash);
      if (!source) throw new Error("not loaded");
      return source;
    };
    const drawn = (source, picture, u) => {
      if (u.lut) {
        if (u.lut.data) luts.set(u.lut.key, u.lut.data);
        u = { ...u, lut: { ...u.lut, data: luts.get(u.lut.key) } };
        if (!u.lut.data) throw new Error("no table");
      }
      u = { ...u, air: source.air, scale: picture.width / source.width };
      try {
        gpu ??= renderer(makeCanvas());
        return gpu.draw(picture, u);
      } catch (error) {
        gpu = null;
        throw error;
      }
    };
    return {
      load: async (hash, bytes, size) => {
        if (!sources.has(hash)) {
          const made = prepare(bytes, size);
          made.catch(() => sources.delete(hash));
          sources.set(hash, made);
          while (sources.size > 4) {
            const [oldest, picture] = sources.entries().next().value;
            sources.delete(oldest);
            picture.then((source) => source.close()).catch(() => {});
          }
        }
        const { histogram, width, height } = await sources.get(hash);
        return { histogram, width, height };
      },
      draw: async (hash, u, side) => {
        const source = await sourceOf(hash);
        const canvas = drawn(source, await source.at(side), u);
        return canvas.transferToImageBitmap ? canvas.transferToImageBitmap() : canvas;
      },
      make: async (hash, u, side) => {
        const source = await sourceOf(hash);
        const canvas = drawn(source, await (side ? source.at(side) : source.full()), u);
        const blob = await encode(canvas, source.jpeg, side ? 0.9 : 0.95);
        return new Uint8Array(await blob.arrayBuffer());
      },
    };
  };

  const WORKER = [
    `const renderer = ${renderer};`,
    `const prepare = ${prepare};`,
    `const encode = ${encode};`,
    `const core = ${core};`,
    `const engine = core(() => new OffscreenCanvas(1, 1));`,
    `onmessage = async ({ data: { id, type, hash, bytes, size, u, side } }) => {`,
    `  try {`,
    `    if (type === "load") postMessage({ id, out: await engine.load(hash, bytes, size) });`,
    `    else if (type === "draw") { const out = await engine.draw(hash, u, side); postMessage({ id, out }, [out]); }`,
    `    else if (type === "make") { const out = await engine.make(hash, u, side); postMessage({ id, out }, [out.buffer]); }`,
    `  } catch (error) {`,
    `    postMessage({ id, error: String(error) });`,
    `  }`,
    `};`,
    `let ready = false;`,
    `try { ready = !!new OffscreenCanvas(1, 1).getContext("webgl"); } catch {}`,
    `postMessage({ ready });`,
  ].join("\n");

  // The worker once it's said it's ready, or null where it can't start,
  // then or later, and the page makes the pictures itself
  let worker = null;
  let jobId = 0;
  const jobs = new Map();
  const startWorker = () =>
    (worker ??= new Promise((resolve) => {
      let started = null;
      const stop = () => {
        started?.terminate();
        for (const job of jobs.values()) job.reject(new Error("no worker"));
        jobs.clear();
        loads.clear();
        sent.clear();
        worker = Promise.resolve(null);
        resolve(null);
      };
      try {
        started = new Worker(URL.createObjectURL(new Blob([WORKER], { type: "text/javascript" })));
        const late = setTimeout(stop, 3000);
        started.onmessage = ({ data }) => {
          if ("ready" in data) {
            clearTimeout(late);
            return data.ready ? resolve(started) : stop();
          }
          const job = jobs.get(data.id);
          jobs.delete(data.id);
          if (data.error) job?.reject(new Error(data.error));
          else job?.resolve(data.out);
        };
        started.onerror = (event) => {
          event.preventDefault();
          stop();
        };
      } catch {
        stop();
      }
    }));
  // Given up on after `limit` ms, so an edit can never hang on one
  const call = (target, message, transfer, limit) =>
    new Promise((resolve, reject) => {
      const id = ++jobId;
      const late = setTimeout(() => {
        jobs.delete(id);
        reject(new Error("too slow"));
      }, limit);
      const done = (then) => (value) => {
        clearTimeout(late);
        then(value);
      };
      jobs.set(id, { resolve: done(resolve), reject: done(reject) });
      target.postMessage({ id, ...message }, transfer);
    });
  // Lookup tables already with the engine, by key
  const sent = new Set();
  const withTable = (u) => {
    if (!u.lut || sent.has(u.lut.key)) return u.lut ? { ...u, lut: { ...u.lut, data: null } } : u;
    sent.add(u.lut.key);
    return u;
  };
  let local = null;
  const engine = async () => {
    const started = await startWorker();
    if (!started) return (local ??= core(() => document.createElement("canvas")));
    return {
      load: (hash, bytes, size) => call(started, { type: "load", hash, bytes, size }, [bytes.buffer], 20000),
      draw: (hash, u, side) => call(started, { type: "draw", hash, u: withTable(u), side }, [], 5000),
      make: (hash, u, side) => call(started, { type: "make", hash, u: withTable(u), side }, [], 60000),
    };
  };
  // An original sent to the engine, once, the last few as it keeps them,
  // with its histogram and size. One no paint shows any more may have to
  // come from Figma's servers first, so it's given a while, not forever
  const loads = new Map();
  const within = (promise, limit) => Promise.race([promise, new Promise((_, reject) => setTimeout(() => reject(new Error("too slow")), limit))]);
  const ready = (figma, hash) => {
    if (!loads.has(hash)) {
      const made = (async () => {
        const picture = figma.getImageByHash(hash);
        if (!picture) throw new Error("no image");
        const [bytes, size] = await within(Promise.all([picture.getBytesAsync(), picture.getSizeAsync().catch(() => null)]), 30000);
        const using = await engine();
        const info = await using.load(hash, bytes.slice(), size?.width ? { width: size.width, height: size.height } : null);
        return { using, ...info };
      })();
      made.catch(() => loads.delete(hash));
      loads.set(hash, made);
      while (loads.size > 4) loads.delete(loads.keys().next().value);
    }
    return loads.get(hash);
  };

  // ---------- The adjustments as uniforms ----------

  // A curve through `points`, smooth and never overshooting them, as
  // Photoshop's, monotone cubic: 256 outputs, 0 to 255
  const spline = (points) => {
    const xs = points.map(([x]) => x);
    const ys = points.map(([, y]) => y);
    const n = points.length;
    const slopes = [];
    for (let i = 0; i < n - 1; i++) slopes.push((ys[i + 1] - ys[i]) / Math.max(1, xs[i + 1] - xs[i]));
    const tangents = [slopes[0]];
    for (let i = 1; i < n - 1; i++) tangents.push(slopes[i - 1] * slopes[i] <= 0 ? 0 : (slopes[i - 1] + slopes[i]) / 2);
    tangents.push(slopes[n - 2]);
    for (let i = 0; i < n - 1; i++) {
      if (!slopes[i]) {
        tangents[i] = tangents[i + 1] = 0;
        continue;
      }
      const a = tangents[i] / slopes[i];
      const b = tangents[i + 1] / slopes[i];
      const h = Math.hypot(a, b);
      if (h > 3) {
        tangents[i] = (3 * a * slopes[i]) / h;
        tangents[i + 1] = (3 * b * slopes[i]) / h;
      }
    }
    const out = new Float32Array(256);
    for (let x = 0; x < 256; x++) {
      if (x <= xs[0]) out[x] = ys[0];
      else if (x >= xs[n - 1]) out[x] = ys[n - 1];
      else {
        let i = 0;
        while (xs[i + 1] < x) i++;
        const h = xs[i + 1] - xs[i];
        const t = (x - xs[i]) / h;
        const t2 = t * t;
        const t3 = t2 * t;
        out[x] = (2 * t3 - 3 * t2 + 1) * ys[i] + (t3 - 2 * t2 + t) * h * tangents[i] + (-2 * t3 + 3 * t2) * ys[i + 1] + (t3 - t2) * h * tangents[i + 1];
      }
      out[x] = clamp(out[x], 0, 255);
    }
    return out;
  };
  const leveled = ([black, gamma, white, outBlack, outWhite], x) => {
    const v = clamp((x - black) / Math.max(1, white - black), 0, 1) ** (1 / clamp(gamma, 0.1, 9.99));
    return outBlack + v * (outWhite - outBlack);
  };
  // Levels in the first row and Curves in the second, each channel's own
  // then the composite's, as one curve for each channel
  const toneOf = (items) => {
    const lv = items.levels?.on && !identity("levels", items.levels) ? items.levels : null;
    const cv = items.curves?.on && !identity("curves", items.curves) ? items.curves : null;
    if (!lv && !cv) return null;
    const curves = cv && Object.fromEntries(["rgb", "red", "green", "blue"].map((name) => [name, spline(cv[name])]));
    const out = new Uint8Array(2048);
    ["red", "green", "blue"].forEach((name, c) => {
      for (let x = 0; x < 256; x++) {
        out[x * 4 + c] = Math.round(clamp(lv ? leveled(lv.rgb, leveled(lv[name], x)) : x, 0, 255));
        out[1024 + x * 4 + c] = Math.round(clamp(curves ? curves.rgb[Math.round(clamp(curves[name][x], 0, 255))] : x, 0, 255));
      }
    });
    for (let x = 0; x < 512; x++) out[x * 4 + 3] = 255;
    return out;
  };

  // Color Lookup's own looks, Ligma's takes on Photoshop's, made as lookup
  // tables 17 across. Photoshop's own come from where it's installed
  const LUT_SIZE = 17;
  const luma = ([r, g, b]) => 0.299 * r + 0.587 * g + 0.114 * b;
  const contrasted = (c, k) => c.map((v) => {
    const x = clamp(v, 0, 1);
    return v + (x * x * (3 - 2 * x) - v) * k;
  });
  const saturated = (c, k) => {
    const l = luma(c);
    return c.map((v) => l + (v - l) * k);
  };
  // Shadows and highlights tinted, each by its own color
  const toned = (c, shadows, highlights) => {
    const l = clamp(luma(c), 0, 1);
    return c.map((v, i) => v + shadows[i] * (1 - l) ** 2 + highlights[i] * l * l);
  };
  const lifted = (c, black, white = 1) => c.map((v) => black + v * (white - black));
  const scaled = (c, by, plus = [0, 0, 0]) => c.map((v, i) => v * by[i] + plus[i]);
  // Where one channel leads the others, 0 to 1
  const leads = (c, i) => clamp((c[i] - Math.max(...c.filter((_, j) => j !== i))) * 3, 0, 1);
  const LOOKS = [
    ["bleach", "Bleach bypass", (c) => contrasted(saturated(c, 0.4), 1)],
    ["candlelight", "Candlelight", (c) => contrasted(toned(scaled(c, [1, 0.8, 0.55], [0.03, 0.01, 0]), [0.04, 0.01, -0.02], [0.02, 0, -0.04]), 0.15)],
    ["coolMatte", "Cool matte", (c) => scaled(c, [0.84, 0.85, 0.86], [0.08, 0.09, 0.12])],
    ["crispWarm", "Crisp warm", (c) => saturated(toned(contrasted(c, 0.45), [0, 0, 0.02], [0.05, 0.02, -0.04]), 1.12)],
    ["crispWinter", "Crisp winter", (c) => saturated(toned(contrasted(c, 0.4), [-0.02, 0, 0.04], [-0.03, 0, 0.05]), 0.9)],
    ["dropBlues", "Drop blues", (c) => {
      const blue = leads(c, 2);
      const l = luma(c);
      return contrasted([c[0], c[1] + (l - c[1]) * blue * 0.3, c[2] + (l - c[2]) * blue * 0.75], 0.15);
    }],
    ["edgyAmber", "Edgy amber", (c) => contrasted(scaled(saturated(c, 0.6), [1.08, 0.98, 0.72], [0.04, 0.02, 0]), 0.55)],
    ["fallColors", "Fall colors", (c) => {
      const green = leads(c, 1);
      return contrasted(saturated([c[0] + green * (c[1] - c[0]) * 0.9, c[1] - green * (c[1] - c[2]) * 0.15, c[2] * (1 - green * 0.3)], 1.15), 0.2);
    }],
    ["filmPrint", "Film print", (c) => saturated(toned(contrasted(c, 0.55), [-0.03, 0.01, 0.03], [0.04, 0.02, -0.03]), 1.08)],
    ["filmPrintSoft", "Film print, soft", (c) => toned(lifted(contrasted(c, 0.25), 0.03, 0.97), [-0.02, 0.01, 0.02], [0.03, 0.015, -0.02])],
    ["filmStock", "Film stock 50", (c) => saturated(toned(lifted(contrasted(c, 0.2), 0.05, 0.96), [-0.01, 0.02, 0], [0.03, 0.02, -0.02]), 0.92)],
    ["foggyNight", "Foggy night", (c) => toned(lifted(saturated(c, 0.55), 0.12, 0.82), [-0.02, 0.01, 0.06], [-0.02, 0, 0.03])],
    ["futuristicBleak", "Futuristic bleak", (c) => toned(contrasted(saturated(c, 0.35), 0.6), [-0.03, 0.02, 0.04], [-0.02, 0.02, 0.03])],
    ["horrorBlue", "Horror blue", (c) => contrasted(scaled(saturated(c, 0.4), [0.75, 0.92, 1.05], [0, 0.02, 0.08]), 0.5)],
    ["lateSunset", "Late sunset", (c) => contrasted(saturated(toned(scaled(c, [1.08, 0.95, 0.82], [0.04, 0.02, 0.03]), [0.03, -0.01, 0.05], [0.06, 0.03, -0.05]), 1.1), 0.2)],
    ["moonlight", "Moonlight", (c) => contrasted(scaled(saturated(c, 0.35), [0.63, 0.74, 0.88], [0, 0.02, 0.05]), 0.2)],
    ["nightFromDay", "Night from day", (c) => contrasted(scaled(saturated(c, 0.5), [0.45, 0.55, 0.8], [0, 0.01, 0.04]), 0.35)],
    ["softWarming", "Soft warming", (c) => toned(scaled(c, [1.03, 1, 0.94], [0.01, 0.005, 0]), [0.01, 0, -0.01], [0.02, 0.01, -0.02])],
    ["tealOrange", "Teal and orange", (c) => {
      const l = luma(c);
      return [c[0] + l * 0.08 - (1 - l) * 0.06, c[1] + l * 0.02 + (1 - l) * 0.03, c[2] - l * 0.08 + (1 - l) * 0.09];
    }],
    ["tealOrangeContrast", "Teal and orange, contrast", (c) => {
      const l = luma(c);
      return contrasted([c[0] + l * 0.1 - (1 - l) * 0.08, c[1] + l * 0.02 + (1 - l) * 0.04, c[2] - l * 0.1 + (1 - l) * 0.12], 0.45);
    }],
    ["tensionGreen", "Tension green", (c) => contrasted(scaled(saturated(c, 0.6), [0.9, 1.04, 0.85], [0, 0.03, 0]), 0.45)],
    ["threeStrip", "Three-strip", (c) => saturated(contrasted([c[0] * 1.15 - (c[1] + c[2]) * 0.075, c[1] * 1.15 - (c[0] + c[2]) * 0.075, c[2] * 1.15 - (c[0] + c[1]) * 0.075], 0.25), 1.15)],
    ["twoStrip", "Two-strip", (c) => {
      const cyan = (c[1] + c[2]) / 2;
      return contrasted([c[0], cyan * 0.95 + c[0] * 0.05, cyan], 0.3);
    }],
    ["warm", "Warm film", (c) => lifted(scaled(c, [1.06, 1.01, 0.88], [0.02, 0.01, luma(c) * 0.04]), 0.06, 0.98)],
  ];
  // Photoshop's tables, by file name, read from its folder when picked
  const PHOTOSHOP_LUTS = __LIGMA_LUTS__;

  const packed = (size, at) => {
    const data = new Uint8Array(size * size * size * 4);
    for (let b = 0; b < size; b++)
      for (let g = 0; g < size; g++)
        for (let r = 0; r < size; r++) {
          const out = at(r, g, b);
          const i = (g * size * size + b * size + r) * 4;
          data[i] = Math.round(clamp(out[0], 0, 1) * 255);
          data[i + 1] = Math.round(clamp(out[1], 0, 1) * 255);
          data[i + 2] = Math.round(clamp(out[2], 0, 1) * 255);
          data[i + 3] = 255;
        }
    return data;
  };
  const tables = new Map();
  const tableOf = (item) => {
    const key = item.lut === "file" ? `file:${item.name}:${item.data.length}:${item.data.slice(0, 64)}` : item.lut;
    if (!tables.has(key)) {
      let data = null;
      if (item.lut === "file") {
        // Kept as base64, red, green and blue, red fastest, 17 across
        try {
          const bytes = Uint8Array.from(atob(item.data), (char) => char.charCodeAt(0));
          if (bytes.length === LUT_SIZE ** 3 * 3) data = packed(LUT_SIZE, (r, g, b) => Array.from(bytes.subarray(((b * LUT_SIZE + g) * LUT_SIZE + r) * 3, ((b * LUT_SIZE + g) * LUT_SIZE + r) * 3 + 3), (v) => v / 255));
        } catch {}
      } else {
        const look = LOOKS.find(([id]) => id === item.lut)?.[2];
        if (look) data = packed(LUT_SIZE, (r, g, b) => look([r / (LUT_SIZE - 1), g / (LUT_SIZE - 1), b / (LUT_SIZE - 1)]));
      }
      tables.set(key, data && { key, size: LUT_SIZE, data });
    }
    return tables.get(key);
  };

  // 3D LUT files, each as `{ size, read(r, g, b) }`, its color at a point of
  // its grid, 0 to 1. Null if it isn't one. .cube lists red fastest, .3dl
  // blue fastest in whole numbers, and Photoshop's .look keeps a table as
  // hex floats
  const fromCube = (text) => {
    let size = 0;
    let min = [0, 0, 0];
    let max = [1, 1, 1];
    const values = [];
    for (const raw of text.split(/\r?\n/)) {
      const line = raw.trim();
      if (!line || line.startsWith("#")) continue;
      const words = line.split(/\s+/);
      if (words[0] === "LUT_3D_SIZE") size = Number(words[1]);
      else if (words[0] === "DOMAIN_MIN") min = words.slice(1, 4).map(Number);
      else if (words[0] === "DOMAIN_MAX") max = words.slice(1, 4).map(Number);
      else if (/^[-\d.]/.test(words[0]) && words.length >= 3) values.push(words.slice(0, 3).map(Number));
    }
    if (!(size >= 2 && size <= 128) || values.length !== size ** 3) return null;
    return { size, read: (r, g, b) => values[(b * size + g) * size + r].map((v, i) => (v - min[i]) / (max[i] - min[i] || 1)) };
  };
  const from3dl = (text) => {
    const rows = [];
    for (const raw of text.split(/\r?\n/)) {
      const line = raw.trim();
      if (!line || line.startsWith("#")) continue;
      const numbers = line.split(/\s+/).map(Number);
      if (numbers.every(Number.isFinite)) rows.push(numbers);
    }
    const at = rows.findIndex((numbers) => numbers.length > 3);
    if (at < 0) return null;
    const size = rows[at].length;
    const values = rows.slice(at + 1).filter((numbers) => numbers.length >= 3);
    if (!(size >= 2 && size <= 128) || values.length < size ** 3) return null;
    let top = 0;
    for (const value of values) top = Math.max(top, value[0], value[1], value[2]);
    const depth = top <= 1023 ? 1023 : top <= 4095 ? 4095 : 65535;
    return { size, read: (r, g, b) => values[(r * size + g) * size + b].slice(0, 3).map((v) => v / depth) };
  };
  const fromLook = (text) => {
    const size = Number(/<size>\s*"?(\d+)"?\s*<\/size>/.exec(text)?.[1]);
    const digits = /<data>\s*"?([\da-fA-F\s]+)"?\s*<\/data>/.exec(text)?.[1].replace(/\s+/g, "");
    if (!(size >= 2 && size <= 64) || !digits || digits.length < size ** 3 * 24) return null;
    const bytes = new Uint8Array(size ** 3 * 12);
    for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(digits.slice(i * 2, i * 2 + 2), 16);
    const view = new DataView(bytes.buffer);
    return {
      size,
      read: (r, g, b) => {
        const i = ((b * size + g) * size + r) * 12;
        return [view.getFloat32(i, true), view.getFloat32(i + 4, true), view.getFloat32(i + 8, true)];
      },
    };
  };
  const readTable = (name, text) => {
    const lower = name.toLowerCase();
    return lower.endsWith(".3dl") ? from3dl(text) : lower.endsWith(".look") ? fromLook(text) : fromCube(text);
  };
  // A table made 17 across, trilinear, as base64 to keep on the layer:
  // red, green and blue bytes, red fastest
  const keepTable = ({ size, read }) => {
    const sample = (r, g, b) => {
      const at = [r, g, b].map((v) => (v * (size - 1)) / (LUT_SIZE - 1));
      const lo = at.map(Math.floor);
      const hi = lo.map((v) => Math.min(v + 1, size - 1));
      const t = at.map((v, i) => v - lo[i]);
      const out = [0, 0, 0];
      for (let corner = 0; corner < 8; corner++) {
        const pick = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        const weight = pick.reduce((product, high, i) => product * (high ? t[i] : 1 - t[i]), 1);
        if (!weight) continue;
        const value = read(pick[0] ? hi[0] : lo[0], pick[1] ? hi[1] : lo[1], pick[2] ? hi[2] : lo[2]);
        for (let i = 0; i < 3; i++) out[i] += weight * value[i];
      }
      return out;
    };
    const bytes = new Uint8Array(LUT_SIZE ** 3 * 3);
    for (let b = 0; b < LUT_SIZE; b++)
      for (let g = 0; g < LUT_SIZE; g++)
        for (let r = 0; r < LUT_SIZE; r++) {
          const out = sample(r, g, b);
          const i = ((b * LUT_SIZE + g) * LUT_SIZE + r) * 3;
          for (let c = 0; c < 3; c++) bytes[i + c] = Math.round(clamp(Number.isFinite(out[c]) ? out[c] : 0, 0, 1) * 255);
        }
    let text = "";
    for (let i = 0; i < bytes.length; i += 8192) text += String.fromCharCode(...bytes.subarray(i, i + 8192));
    return btoa(text);
  };

  // Gradient Map's gradients: Photoshop's classic presets and a few
  // duotones, stops at 0 to 100
  const GRADIENTS = [
    ["blackWhite", "Black, White", [[0, "#000000"], [100, "#ffffff"]]],
    ["violetOrange", "Violet, Orange", [[0, "#290a59"], [100, "#ff7c00"]]],
    ["blueRedYellow", "Blue, Red, Yellow", [[0, "#0a00b2"], [50, "#ff0000"], [100, "#fffc00"]]],
    ["blueYellowBlue", "Blue, Yellow, Blue", [[0, "#0a00b2"], [50, "#fffc00"], [100, "#0a00b2"]]],
    ["orangeYellowOrange", "Orange, Yellow, Orange", [[0, "#ff6e02"], [50, "#ffff00"], [100, "#ff6d00"]]],
    ["violetGreenOrange", "Violet, Green, Orange", [[0, "#6f1580"], [50, "#00a14b"], [100, "#ff8c00"]]],
    ["yellowVioletOrangeBlue", "Yellow, Violet, Orange, Blue", [[0, "#f9e000"], [33, "#6e0ea8"], [66, "#ff7a00"], [100, "#0066ff"]]],
    ["copper", "Copper", [[0, "#97461a"], [30, "#fbd8c5"], [83, "#6c2e16"], [100, "#efdacd"]]],
    ["chrome", "Chrome", [[0, "#29343e"], [48, "#e8eef3"], [52, "#6e5b46"], [76, "#c6a986"], [100, "#ffffff"]]],
    ["spectrum", "Spectrum", [[0, "#ff0000"], [17, "#ffff00"], [33, "#00ff00"], [50, "#00ffff"], [67, "#0000ff"], [83, "#ff00ff"], [100, "#ff0000"]]],
    ["sepia", "Sepia", [[0, "#1d1209"], [55, "#8f6a43"], [100, "#f3e3c3"]]],
    ["cyanotype", "Cyanotype", [[0, "#071a35"], [55, "#2f6fa3"], [100, "#eaf3fa"]]],
    ["infrared", "Infrared", [[0, "#000000"], [35, "#7a0c55"], [70, "#ff6b35"], [100, "#fff6d5"]]],
    ["purplePeach", "Purple, Peach", [[0, "#2d0b4e"], [100, "#ffb38a"]]],
    ["tealCream", "Teal, Cream", [[0, "#0f3b3a"], [100, "#f6ecd6"]]],
    ["midnightGold", "Midnight, Gold", [[0, "#0b1026"], [100, "#ffd36b"]]],
  ];
  const METHODS = [
    ["perceptual", "Perceptual"],
    ["linear", "Linear"],
    ["classic", "Classic"],
  ];
  const toLinear = (v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  const toSrgb = (v) => (v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055);
  const toOklab = (c) => {
    const [r, g, b] = c.map(toLinear);
    const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
    const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
    const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
    return [0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s, 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s, 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s];
  };
  const fromOklab = ([L, a, b]) => {
    const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
    const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
    const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
    return [4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s, -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s, -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s].map((v) => toSrgb(clamp(v, 0, 1)));
  };
  // The gradient's color at `at`, 0 to 100, mixed as `method` says:
  // Perceptual in Oklab, Linear in linear light, Classic as the colors are
  const colorAt = (stops, at, method) => {
    let i = stops.findIndex(([position]) => position >= at);
    if (i < 0) return hex(stops[stops.length - 1][1]);
    if (i === 0) return hex(stops[0][1]);
    const [[from, a], [to, b]] = [stops[i - 1], stops[i]];
    const t = to > from ? (at - from) / (to - from) : 1;
    const [ca, cb] = [hex(a), hex(b)];
    const mixed = (x, y) => x.map((v, j) => v + (y[j] - v) * t);
    if (method === "classic") return mixed(ca, cb);
    if (method === "linear") return mixed(ca.map(toLinear), cb.map(toLinear)).map(toSrgb);
    return fromOklab(mixed(toOklab(ca), toOklab(cb)));
  };
  // Lightness to color, 256 steps
  const gradientOf = (item) => {
    const out = new Uint8Array(1024);
    for (let x = 0; x < 256; x++) {
      const at = ((item.reverse ? 255 - x : x) / 255) * 100;
      const color = colorAt(item.stops, at, item.method);
      for (let c = 0; c < 3; c++) out[x * 4 + c] = Math.round(clamp(color[c], 0, 1) * 255);
      out[x * 4 + 3] = 255;
    }
    return out;
  };
  const gradientCss = (stops, method = "perceptual", reverse = false) => {
    const space = { perceptual: "in oklab ", linear: "in srgb-linear ", classic: "" }[method] ?? "";
    return `linear-gradient(${space}${reverse ? 270 : 90}deg, ${stops.map(([at, color]) => `${color} ${at}%`).join(", ")})`;
  };

  const hex = (color) => {
    const value = /^#?([\da-f]{6})$/i.exec(color)?.[1] ?? "ec8a00";
    return [0, 2, 4].map((i) => parseInt(value.slice(i, i + 2), 16) / 255);
  };
  // Everything the shaders take, from the settings
  const uniformsOf = (s) => {
    const on = (id) => (s.items[id]?.on && !identity(id, s.items[id]) ? s.items[id] : null);
    const presence = on("presence");
    const sharpen = on("sharpen");
    const balance = on("balance");
    const selective = on("selective");
    const mixer = on("mixer");
    const hue = on("hsl");
    const vibrance = on("vibrance");
    const photo = on("photo");
    const mono = on("mono");
    const lookup = on("lookup");
    const vignette = on("vignette");
    const map = on("gradient");
    const tone = toneOf(s.items);
    const lut = lookup ? tableOf(lookup) : null;
    const filter = photo && (photo.filter === "custom" ? photo.color : FILTERS.find(([id]) => id === photo.filter)?.[2]);
    const shift = (values) => [values.hue / 360, values.saturation / 100, values.lightness / 100];
    return {
      order: orderOf(s).filter((id) => (id === "invert" ? !!s.items.invert?.on : !!on(id))),
      presence: presence ? [presence.texture / 100, presence.clarity / 100, presence.dehaze / 100] : [0, 0, 0],
      sharp: sharpen ? [(sharpen.amount / 100) * 2.5, clamp(sharpen.radius, 0.5, 3), sharpen.detail / 100, sharpen.masking / 100] : [0, 1, 0, 0],
      tone,
      toneKey: tone && JSON.stringify([s.items.levels, s.items.curves]),
      cb: balance ? { s: balance.shadows.map((v) => v / 100), m: balance.midtones.map((v) => v / 100), h: balance.highlights.map((v) => v / 100), flags: [1, balance.preserve ? 1 : 0] } : { s: [0, 0, 0], m: [0, 0, 0], h: [0, 0, 0], flags: [0, 0] },
      sel: { values: SELECTIVE.flatMap(([range]) => (selective ? selective[range].map((v) => v / 100) : [0, 0, 0, 0])), flags: [selective ? 1 : 0, selective?.relative === false ? 0 : 1] },
      mixer: mixer
        ? { m: [mixer.red[0], mixer.green[0], mixer.blue[0], mixer.red[1], mixer.green[1], mixer.blue[1], mixer.red[2], mixer.green[2], mixer.blue[2]].map((v) => v / 100), c: [mixer.red[3], mixer.green[3], mixer.blue[3]].map((v) => v / 100), flags: [1, mixer.monochrome ? 1 : 0] }
        : { m: [1, 0, 0, 0, 1, 0, 0, 0, 1], c: [0, 0, 0], flags: [0, 0] },
      hsl: hue
        ? { on: 1, master: shift(hue.master), ranges: RANGES.flatMap(([range]) => shift(hue[range])), colorize: [hue.colorize ? 1 : 0, (((hue.tone.hue % 360) + 360) % 360) / 360, clamp(hue.tone.saturation, 0, 100) / 100, hue.tone.lightness / 100] }
        : { on: 0, master: [0, 0, 0], ranges: new Array(18).fill(0), colorize: [0, 0, 0, 0] },
      vib: vibrance ? [1, vibrance.vibrance / 100, vibrance.saturation / 100] : [0, 0, 0],
      photo: photo ? [...hex(filter), clamp(photo.density, 0, 100) / 100] : [1, 1, 1, 0],
      photoFlags: [photo ? 1 : 0, photo?.preserve ? 1 : 0],
      bw: mono ? ["reds", "yellows", "greens", "cyans", "blues", "magentas"].map((range) => mono[range] / 100) : [0, 0, 0, 0, 0, 0],
      bwFlags: mono ? [1, mono.tint ? 1 : 0, (((mono.hue % 360) + 360) % 360) / 360, clamp(mono.saturation, 0, 100) / 100] : [0, 0, 0, 0],
      lut: lut && { ...lut, intensity: clamp(lookup.intensity, 0, 100) / 100 },
      grad: map ? gradientOf(map) : null,
      gradKey: map && JSON.stringify([map.stops, map.reverse, map.method]),
      gradFlags: [map ? 1 : 0, map?.dither ? 1 : 0],
      invert: s.items.invert?.on ? 1 : 0,
      vig: vignette ? [clamp(vignette.amount, -100, 100) / 100, vignette.midpoint / 100, vignette.roundness / 100, vignette.feather / 100] : [0, 0.5, 0, 0.5],
      vig2: [vignette ? vignette.highlights / 100 : 0, vignette ? STYLES.findIndex(([id]) => id === vignette.style) : 0, vignette ? 1 : 0],
    };
  };

  // A new image draws nothing in the file until Figma's loaded it, so it's
  // put on a tiny layer first, until that draws something. The layer sits
  // at the bottom of the page inside the image's own bounds, so the canvas
  // doesn't grow and its scrollbars stay put, and it's kept for the whole
  // edit rather than made for each image. It goes again in the same undo
  // step, so undo never brings it back
  const LOADER = "loader";
  const loaders = new Set();
  const makeLoader = (figma, node) => {
    const loader = figma.createRectangle();
    loaders.add(loader);
    loader.name = "Ligma loading";
    loader.setSharedPluginData(PLUGIN, LOADER, "1");
    figma.currentPage.insertChild(0, loader);
    loader.resizeWithoutConstraints(2, 2);
    const box = node.absoluteBoundingBox;
    loader.x = box ? box.x + box.width / 2 - 1 : 0;
    loader.y = box ? box.y + box.height / 2 - 1 : 0;
    return loader;
  };
  const dropLoader = (loader) => {
    loaders.delete(loader);
    try {
      if (!loader.removed) loader.remove();
    } catch {}
  };
  // Any left by an edit that never finished, as when a tab closed mid-way
  const dropStrayLoaders = (figma) => {
    try {
      for (const child of figma.currentPage.children.slice(0, 8)) {
        if (!loaders.has(child) && child.getSharedPluginData(PLUGIN, LOADER)) child.remove();
      }
    } catch {}
  };
  const loaded = async (loader, hash) => {
    try {
      loader.fills = [{ type: "IMAGE", imageHash: hash, scaleMode: "FILL" }];
      const canvas = Object.assign(document.createElement("canvas"), { width: 2, height: 2 });
      const context = canvas.getContext("2d", { willReadFrequently: true });
      const until = performance.now() + 2000;
      // A hidden tab doesn't draw, so exporting may wait as long as it is
      const late = new Promise((resolve) => setTimeout(resolve, 2000, null));
      while (performance.now() < until && !loader.removed) {
        const bytes = await Promise.race([loader.exportAsync({ format: "PNG", constraint: { type: "WIDTH", value: 2 } }), late]);
        if (!bytes) return;
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
        context.clearRect(0, 0, 2, 2);
        context.drawImage(bitmap, 0, 0, 2, 2);
        bitmap.close();
        const { data } = context.getImageData(0, 0, 2, 2);
        for (let i = 3; i < data.length; i += 4) if (data[i]) return;
      }
    } catch {}
  };

  // Puts `s` on the paint `index` of `list` showing `target.hash`: made
  // from its original, or the original back with none. False if the paint
  // changed meanwhile, or once `stale`, and then nothing's changed.
  // `loader` loads the new image, made for it if not given; `side` makes
  // it that many pixels across, for others to see while sliders move,
  // rather than full size
  const put = async (figma, target, list, index, s, stale = () => false, loader = null, side = 0) => {
    const { node } = target;
    const original = originalOf(node, target.hash);
    let hash = original;
    if (!isBlank(s)) {
      const using = await ready(figma, original);
      const bytes = await using.using.make(original, uniformsOf(s), side);
      if (stale()) return false;
      hash = figma.createImage(bytes).hash;
      const own = loader ?? makeLoader(figma, node);
      try {
        await loaded(own, hash);
      } finally {
        if (!loader) dropLoader(own);
      }
      if (stale()) return false;
    }
    const paints = paintsOf(node, list);
    const was = target.hash;
    if (node.removed || paints[index]?.imageHash !== was) return false;
    node[list] = paints.map((paint, i) => (i === index ? { ...paint, imageHash: hash } : paint));
    target.hash = hash;
    const now = madeOn(node);
    delete now[was];
    // The original keeps its settings too while they're all hidden or
    // unchanged, so they're there when the picker opens again
    if (hash !== original || added(s)) now[hash] = { original, settings: s };
    keepMade(node, now);
    return hash;
  };

  // Each paint adjusted gets its original as a hidden paint right below it,
  // and one adjusted back to its original loses it. Not while its picker's
  // open, which would then show another paint
  const settle = (node) => {
    if (node.removed) return;
    const made = madeOn(node);
    for (const list of ["fills", "strokes"]) {
      const paints = [...paintsOf(node, list)];
      let changed = false;
      for (let i = 0; i < paints.length; i++) {
        const paint = paints[i];
        const entry = image(paint) && made[paint.imageHash]?.original !== paint.imageHash && made[paint.imageHash];
        if (entry) {
          const below = paints[i - 1];
          if (!(image(below) && below.imageHash === entry.original && below.visible === false)) {
            paints.splice(i, 0, { ...paint, imageHash: entry.original, visible: false });
            i++;
            changed = true;
          }
        } else if (image(paint) && paint.visible === false && image(paints[i + 1]) && paints[i + 1].imageHash === paint.imageHash) {
          // The original below one now showing it again
          paints.splice(i, 1);
          i--;
          changed = true;
        }
      }
      if (changed) {
        try {
          node[list] = paints;
        } catch {}
      }
    }
  };

  // ---------- Changing them ----------

  // The paint whose picker is open, `{ list, index }`, as adjustments.rs
  // finds it: its row in the Fill or Stroke section is marked selected,
  // and names its section and paint in `data-onboarding-key`
  const ROW = '[data-testid="paint-panel-color-value-container"]';
  const KEY_PATTERN = /-(\d+)-(\d+)$/;
  const openPaint = () => {
    const row = document.querySelector(`${ROW}[class*="paintPanelValueSelected--"]`);
    const match = KEY_PATTERN.exec(row?.dataset.onboardingKey ?? "");
    if (!match) return null;
    const first = KEY_PATTERN.exec(document.querySelector(ROW)?.dataset.onboardingKey ?? "");
    const at = { list: first && first[1] !== match[1] ? "strokes" : "fills", index: Number(match[2]) };
    const nodes = api()?.currentPage.selection ?? [];
    const other = at.list === "fills" ? "strokes" : "fills";
    const has = (list) => nodes.some((node) => image(paintsOf(node, list)[at.index]));
    if (!has(at.list) && has(other)) at.list = other;
    return has(at.list) ? at : null;
  };
  const key = (figma, at) => `${figma.currentPage.selection.map((node) => node.id).join()}:${at.list}:${at.index}`;
  const targetsOf = (figma, at) => figma.currentPage.selection.filter((node) => image(paintsOf(node, at.list)[at.index])).map((node) => ({ node, hash: paintsOf(node, at.list)[at.index].imageHash }));

  // ---------- Drawing it over the canvas as it changes ----------

  // Figma's canvas, the largest on the page
  const figmaCanvas = () => {
    let best = null;
    let area = 0;
    for (const canvas of document.querySelectorAll("canvas")) {
      if (canvas.closest("[data-ligma-color]")) continue;
      const { width, height } = canvas.getBoundingClientRect();
      if (width * height > area) [best, area] = [canvas, width * height];
    }
    return best;
  };
  const intersects = (a, b) => !!a && !!b && a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
  const plain = (blend) => !blend || blend === "NORMAL" || blend === "PASS_THROUGH";
  const SHAPES = new Set(["RECTANGLE", "FRAME", "COMPONENT", "INSTANCE", "ELLIPSE"]);
  // Whether the image can be drawn over Figma's canvas, exactly as Figma
  // would, while it changes: the layer's top paint with nothing over it
  // and nothing about it Ligma can't draw. With the frames clipping it.
  // Otherwise the file's own images show it, a moment behind
  const drawable = (node, list, index) => {
    try {
      if (list !== "fills" || !SHAPES.has(node.type)) return null;
      if (node.type === "ELLIPSE") {
        const arc = node.arcData;
        if (arc.startingAngle !== 0 || Math.abs(arc.endingAngle - Math.PI * 2) > 1e-3 || arc.innerRadius) return null;
      }
      if ("children" in node && node.children.some((child) => child.visible)) return null;
      const paints = paintsOf(node, "fills");
      const paint = paints[index];
      if (!image(paint) || paint.visible === false || (paint.opacity ?? 1) < 1 || !plain(paint.blendMode)) return null;
      if (paint.filters && Object.values(paint.filters).some(Boolean)) return null;
      if (paints.slice(index + 1).some((each) => each.visible !== false)) return null;
      if (node.opacity < 1 || !plain(node.blendMode) || node.isMask) return null;
      if ((node.effects ?? []).some((effect) => effect.visible !== false && effect.type !== "DROP_SHADOW")) return null;
      if (node.strokeWeight !== 0 && node.strokeAlign !== "OUTSIDE" && paintsOf(node, "strokes").some((each) => each.visible !== false)) return null;
      const box = node.absoluteRenderBounds ?? node.absoluteBoundingBox;
      const clips = [];
      let checked = 0;
      let child = node;
      for (let parent = node.parent; parent; child = parent, parent = parent.parent) {
        const siblings = parent.children;
        const at = siblings.indexOf(child);
        for (let i = at + 1; i < siblings.length; i++) {
          const above = siblings[i];
          if (loaders.has(above) || !above.visible) continue;
          if (++checked > 3000 || intersects(above.absoluteRenderBounds, box)) return null;
        }
        if (parent.type === "PAGE") break;
        for (let i = 0; i < at; i++) if (siblings[i].isMask && siblings[i].visible) return null;
        if (parent.opacity < 1 || !plain(parent.blendMode)) return null;
        if ((parent.effects ?? []).some((effect) => effect.visible !== false && effect.type !== "DROP_SHADOW")) return null;
        if (parent.clipsContent) {
          const t = parent.absoluteTransform;
          if (Math.abs(t[0][1]) > 1e-6 || Math.abs(t[1][0]) > 1e-6) return null;
          clips.push(parent.absoluteBoundingBox);
        }
      }
      return { node, index, clips };
    } catch {
      return null;
    }
  };

  // The image drawn into `context`, one unit a pixel of the layer, as
  // Figma fills it: Fill, Fit, Crop or Tile, in the layer's shape
  const fillShape = (context, node, paint, picture, natural) => {
    const [W, H] = [node.width, node.height];
    context.save();
    context.beginPath();
    if (node.type === "ELLIPSE") context.ellipse(W / 2, H / 2, W / 2, H / 2, 0, 0, Math.PI * 2);
    else {
      const radius = typeof node.cornerRadius === "number" ? [node.cornerRadius, node.cornerRadius, node.cornerRadius, node.cornerRadius] : [node.topLeftRadius, node.topRightRadius, node.bottomRightRadius, node.bottomLeftRadius];
      context.roundRect(0, 0, W, H, radius.map((value) => Math.min(value || 0, W / 2, H / 2)));
    }
    context.clip();
    const [iw, ih] = [natural.width, natural.height];
    if (paint.scaleMode === "TILE") {
      const factor = paint.scalingFactor ?? 1;
      const pattern = context.createPattern(picture, "repeat");
      pattern.setTransform(new DOMMatrix().scale((iw * factor) / picture.width, (ih * factor) / picture.height));
      context.fillStyle = pattern;
      context.fillRect(0, 0, W, H);
    } else if (paint.scaleMode === "CROP" && paint.imageTransform) {
      // `imageTransform` takes the layer's unit square into the image's,
      // so the image is drawn through its inverse
      const [[a, c, e], [b, d, f]] = paint.imageTransform;
      const det = a * d - b * c || 1;
      context.scale(W, H);
      context.transform(d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det);
      context.drawImage(picture, 0, 0, 1, 1);
    } else {
      const turn = (((paint.rotation ?? 0) % 360) + 360) % 360;
      const [w, h] = turn % 180 === 90 ? [ih, iw] : [iw, ih];
      const k = paint.scaleMode === "FIT" ? Math.min(W / w, H / h) : Math.max(W / w, H / h);
      context.translate(W / 2, H / 2);
      context.rotate((turn * Math.PI) / 180);
      context.drawImage(picture, (-iw * k) / 2, (-ih * k) / 2, iw * k, ih * k);
    }
    context.restore();
  };

  // A picture of the layer over Figma's canvas, right after it in the page
  // so Figma's panels stay over it, following the view as it pans and
  // zooms, clipped to the canvas and the frames around the layer
  const canvasOverlay = (spot, natural) => {
    const host = document.createElement("div");
    host.dataset.ligmaColor = "canvas";
    const clip = host.appendChild(document.createElement("div"));
    const drawing = clip.appendChild(document.createElement("canvas"));
    Object.assign(host.style, { position: "absolute", overflow: "hidden", pointerEvents: "none" });
    Object.assign(clip.style, { position: "absolute", overflow: "hidden" });
    Object.assign(drawing.style, { position: "absolute", left: "0", top: "0", transformOrigin: "0 0" });
    let shown = false;
    let following = 0;
    const place = () => {
      const canvas = figmaCanvas();
      const figma = api();
      if (!canvas || !figma || spot.node.removed) return false;
      if (host.previousElementSibling !== canvas) canvas.after(host);
      const own = canvas.getBoundingClientRect();
      const holder = canvas.parentElement.getBoundingClientRect();
      Object.assign(host.style, { left: `${own.left - holder.left}px`, top: `${own.top - holder.top}px`, width: `${own.width}px`, height: `${own.height}px` });
      const zoom = figma.viewport.zoom;
      const view = figma.viewport.bounds;
      let [left, top, right, bottom] = [-1e7, -1e7, 1e7, 1e7];
      for (const box of spot.clips) {
        left = Math.max(left, (box.x - view.x) * zoom);
        top = Math.max(top, (box.y - view.y) * zoom);
        right = Math.min(right, (box.x + box.width - view.x) * zoom);
        bottom = Math.min(bottom, (box.y + box.height - view.y) * zoom);
      }
      Object.assign(clip.style, { left: `${left}px`, top: `${top}px`, width: `${Math.max(0, right - left)}px`, height: `${Math.max(0, bottom - top)}px` });
      const t = spot.node.absoluteTransform;
      Object.assign(drawing.style, {
        width: `${spot.node.width}px`,
        height: `${spot.node.height}px`,
        transform: `matrix(${t[0][0] * zoom}, ${t[1][0] * zoom}, ${t[0][1] * zoom}, ${t[1][1] * zoom}, ${(t[0][2] - view.x) * zoom - left}, ${(t[1][2] - view.y) * zoom - top})`,
      });
      return true;
    };
    const follow = () => {
      following = shown ? requestAnimationFrame(follow) : 0;
      if (shown && !place()) hide();
    };
    const hide = () => {
      shown = false;
      host.remove();
    };
    return {
      // How large a picture it wants, across
      side: () => {
        try {
          const zoom = api()?.viewport.zoom ?? 1;
          return Math.min(2048, Math.max(spot.node.width, spot.node.height) * zoom * devicePixelRatio * 1.25);
        } catch {
          return 1024;
        }
      },
      paint: (picture) => {
        const paint = paintsOf(spot.node, "fills")[spot.index];
        if (!image(paint)) return hide();
        const zoom = api()?.viewport.zoom ?? 1;
        const scale = Math.min(2048 / Math.max(spot.node.width, spot.node.height), zoom * devicePixelRatio);
        const [width, height] = [Math.max(1, Math.round(spot.node.width * scale)), Math.max(1, Math.round(spot.node.height * scale))];
        if (drawing.width !== width || drawing.height !== height) Object.assign(drawing, { width, height });
        const context = drawing.getContext("2d");
        context.setTransform(scale, 0, 0, scale, 0, 0);
        context.clearRect(0, 0, spot.node.width, spot.node.height);
        fillShape(context, spot.node, paint, picture, natural);
        shown = place();
        if (shown && !following) following = requestAnimationFrame(follow);
      },
      hide,
    };
  };

  // ---------- Changing them ----------

  // The edit under way: the paint whose picker is open in each selected
  // layer, its original, and the settings the panel shows. `version`
  // counts changes, `liveVersion` the last put in the file while a slider
  // moved, `fullVersion` the last put in at full size
  const begin = () => {
    const figma = api();
    const at = openPaint();
    if (!figma || !at) return null;
    const targets = targetsOf(figma, at).map((target) => ({ ...target, original: originalOf(target.node, target.hash) }));
    if (!targets.length) return null;
    const settings = settingsOf(targets[0].node, paintsOf(targets[0].node, at.list)[at.index]);
    dropStrayLoaders(figma);
    const spot = targets.length === 1 ? drawable(targets[0].node, at.list, at.index) : null;
    return { figma, ...at, targets, settings, spot, version: 0, liveVersion: 0, fullVersion: 0, loaders: {} };
  };
  const loaderOf = (edit, lane) => {
    const loader = edit.loaders[lane];
    if (loader && !loader.removed) return loader;
    return (edit.loaders[lane] = makeLoader(edit.figma, edit.targets[0].node));
  };
  const start = (edit) => {
    if (edit.started) return;
    edit.started = true;
    try {
      edit.figma.commitUndo();
    } catch {}
  };
  const fail = (edit) => {
    if (edit.failed) return;
    edit.failed = true;
    edit.figma.notify("Couldn't adjust the image", { timeout: 2000 });
  };

  // A change shows at once, drawn over the picker's thumbnail and the
  // canvas. The file follows: while a slider moves, an image 768 across a
  // few times a second, so others in the file see it move too, then the
  // full size one once it's let go, or a moment after a key
  const change = (edit) => {
    edit.version++;
    edit.failed = false;
    preview(edit);
    if (edit.holding) live(edit);
    later(edit, edit.holding ? null : 250);
  };
  const later = (edit, delay) => {
    clearTimeout(edit.idle);
    if (delay !== null) edit.idle = setTimeout(() => commit(edit), delay);
  };
  // Once a frame at most, the latest settings, one picture at a time
  const preview = (edit) => {
    if (!edit.frame) edit.frame = requestAnimationFrame(() => {
      edit.frame = 0;
      draw(edit);
    });
  };
  const draw = async (edit) => {
    if (edit.drawing) {
      edit.redraw = true;
      return;
    }
    edit.drawing = true;
    try {
      do {
        edit.redraw = false;
        const { original } = edit.targets[0];
        if (!edit.info) {
          edit.loading = true;
          busy(edit);
          edit.info = await ready(edit.figma, original);
          edit.loading = false;
          busy(edit);
          if (edit.spot) edit.overlay = canvasOverlay(edit.spot, edit.info);
        }
        const side = Math.max(416, edit.overlay?.side() ?? 0);
        const picture = await edit.info.using.draw(original, uniformsOf(edit.settings), side);
        if (!edit.ended && ui?.edit === edit && edit.version !== edit.handedOff) {
          ui.picture(picture);
          edit.overlay?.paint(picture);
        }
        picture.close?.();
      } while (edit.redraw && !edit.ended);
    } catch {
      edit.loading = false;
      busy(edit);
      if (ui?.edit === edit) ui.picture(null);
      edit.overlay?.hide();
    }
    edit.drawing = false;
  };
  // Others in the file see it move: an image a few times a second
  const live = async (edit) => {
    if (edit.living) return;
    edit.living = true;
    try {
      while (edit.holding && !edit.ended && !edit.failed && edit.liveVersion !== edit.version) {
        const wait = (edit.lived ?? 0) + 350 - performance.now();
        if (wait > 0) await sleep(wait);
        if (!edit.holding || edit.ended) break;
        const version = edit.version;
        start(edit);
        const s = structuredClone(edit.settings);
        for (const target of edit.targets) await put(edit.figma, target, edit.list, edit.index, s, () => edit.fullVersion >= version, loaderOf(edit, "live"), 768);
        edit.liveVersion = version;
        edit.lived = performance.now();
      }
    } catch {}
    edit.living = false;
  };
  const commit = async (edit) => {
    if (edit.holding || edit.failed || edit.fullVersion === edit.version) return;
    if (edit.committing) {
      edit.again = true;
      return;
    }
    edit.committing = true;
    start(edit);
    const version = edit.version;
    const stale = () => edit.version !== version;
    const s = structuredClone(edit.settings);
    for (const target of edit.targets) {
      try {
        await put(edit.figma, target, edit.list, edit.index, s, stale, loaderOf(edit, "full"));
      } catch {
        fail(edit);
      }
      if (stale() || edit.failed) break;
    }
    if (!stale() && !edit.failed) {
      edit.fullVersion = version;
      handOff(edit, version);
    }
    edit.committing = false;
    if (edit.ended) finish(edit);
    else if (edit.again) {
      edit.again = false;
      commit(edit);
    }
  };
  // Figma draws the new image over the next frames; then the canvas shows
  // the file again, and the thumbnail once Figma's made its own
  const handOff = (edit, version) => {
    edit.landed = performance.now();
    edit.landedSrc = ui?.edit === edit ? ui.thumbnail()?.src : null;
    requestAnimationFrame(() =>
      requestAnimationFrame(() =>
        setTimeout(() => {
          if (edit.version !== version) return;
          edit.handedOff = version;
          edit.overlay?.hide();
        }, 120),
      ),
    );
    setTimeout(() => ui?.edit === edit && ui.show(), 1600);
  };
  // Once the picker's closed: the last settings in, the originals kept
  // below and one undo step for it all
  const finish = (edit) => {
    edit.ended = true;
    edit.holding = false;
    clearTimeout(edit.idle);
    if (edit.committing || edit.settled) return;
    if (!edit.failed && edit.fullVersion !== edit.version) return commit(edit);
    edit.settled = true;
    edit.overlay?.hide();
    for (const loader of Object.values(edit.loaders)) dropLoader(loader);
    if (!edit.started) return;
    for (const { node } of edit.targets) settle(node);
    try {
      edit.figma.commitUndo();
    } catch {}
  };

  // Given up for what the image has now, as when someone else changed it;
  // what it made gets its original below once the picker's closed
  const abandon = (edit) => {
    edit.ended = edit.settled = true;
    clearTimeout(edit.idle);
    edit.overlay?.hide();
    for (const loader of Object.values(edit.loaders)) dropLoader(loader);
    if (edit.started) for (const { node } of edit.targets) pending.add(node);
  };

  // Pasted or reset from adjustments.rs, with the picker closed: straight
  // to full size, the original kept below
  const apply = async (node, list, index, settings) => {
    const figma = api();
    const paint = paintsOf(node, list)[index];
    if (!figma || !image(paint)) return;
    const s = normal(settings);
    if (!added(s) && !madeOn(node)[paint.imageHash]) return;
    await put(figma, { node, hash: paint.imageHash }, list, index, s);
    if (!openPaint()) settle(node);
    else pending.add(node);
  };
  const pending = new Set();

  // ---------- The section under Figma's sliders ----------

  const PICKER = '[data-testid="paint-picker-v2-modal"]';
  const LABEL = '[class*="image_settings_modal--colorAdjustLabel"]';
  const SLIDER = '[role="slider"], input[type="range"]';
  const LABELS = ["Exposure", "Shadows"];
  const visible = (node) => node.getClientRects().length > 0;
  const around = (start, holds) => {
    for (let node = start.parentElement; node && node !== document.body; node = node.parentElement) {
      if (holds(node)) return node.getBoundingClientRect().height <= innerHeight * 0.8 ? node : null;
    }
    return null;
  };
  const ours = (node) => !!node.closest("[data-ligma-color]");
  // Figma's seven sliders, as adjustments.rs finds them
  const figmaSliders = (picker) => {
    const label = [...picker.querySelectorAll(LABEL)].find(visible);
    const box = label && around(label, (node) => node.querySelectorAll(LABEL).length >= 7);
    if (box) return box;
    for (const slider of picker.querySelectorAll(SLIDER)) {
      if (!ours(slider) && visible(slider)) {
        const box = around(slider, (node) => node.querySelectorAll(SLIDER).length >= 7);
        if (box) return box;
      }
    }
    const walker = document.createTreeWalker(picker, NodeFilter.SHOW_TEXT);
    for (let text = walker.nextNode(); text; text = walker.nextNode()) {
      const label = text.parentElement;
      if (text.data.trim() !== LABELS[0] || !label || !visible(label) || ours(label)) continue;
      const box = around(label, (node) => node.textContent.includes(LABELS[1]));
      if (box) return box;
    }
    return null;
  };
  // Where Figma's first label and its slider sit, so ours line up with
  // them: `{ left, column, right }` in pixels, or null if unsure
  const measure = (box, section) => {
    const label = [...box.querySelectorAll(LABEL)].find(visible);
    if (!label) return null;
    const row = label.getBoundingClientRect();
    const middle = row.top + row.height / 2;
    let track = [...box.querySelectorAll(SLIDER)].find((slider) => {
      const rect = slider.getBoundingClientRect();
      return rect.top <= middle && rect.bottom >= middle;
    });
    while (track && track !== box && track.getBoundingClientRect().width < 60) track = track.parentElement;
    if (!track || track === box) return null;
    const own = section.getBoundingClientRect();
    const rect = track.getBoundingClientRect();
    const sizes = { left: row.left - own.left, column: rect.left - row.left, right: own.right - rect.right };
    const fits = sizes.left >= 0 && sizes.left <= 32 && sizes.column >= 48 && sizes.column <= 120 && sizes.right >= 0 && sizes.right <= 32;
    return fits ? sizes : null;
  };

  const S = "[data-ligma-color]";
  const style = document.createElement("style");
  style.textContent = `
    ${S} { font-family: var(--font-family-default, Inter, sans-serif); font-size: 11px; line-height: 16px; letter-spacing: 0.005em; color: var(--color-text); -webkit-font-smoothing: antialiased; cursor: default; user-select: none; }
    ${S}[data-ligma-color="section"] { position: relative; --left: 16px; --column: 88px; --right: 16px; margin-top: 8px; border-top: 1px solid var(--color-border); }
    ${S}[data-ligma-color="section"]::before { content: ""; position: absolute; top: -1px; left: 0; right: 0; height: 1px; background: linear-gradient(90deg, transparent, var(--color-bg-brand, #0d99ff), transparent) no-repeat; background-size: 40% 100%; opacity: 0; transition: opacity 120ms; animation: ligma-color-busy 900ms linear infinite paused; }
    ${S}[data-ligma-color="section"].busy::before { opacity: 1; transition-delay: 160ms; animation-play-state: running; }
    @keyframes ligma-color-busy { from { background-position: -70% 0; } to { background-position: 170% 0; } }
    @media (prefers-reduced-motion: reduce) { ${S}[data-ligma-color="section"]::before { animation: none; background: var(--color-bg-brand, #0d99ff); } ${S} .name svg { transition: none !important; } }
    ${S}[data-ligma-color="preview"] { position: absolute; pointer-events: none; object-fit: contain; }
    ${S} button { font: inherit; color: inherit; cursor: default; }
    ${S} [hidden], ${S}[hidden] { display: none !important; }
    ${S} :focus-visible { outline: 1px solid var(--color-border-selected, #0d99ff); outline-offset: -1px; }
    ${S} .head { display: flex; align-items: center; gap: 4px; height: 40px; padding: 0 calc(var(--right) - 4px) 0 var(--left); }
    ${S} .head .title { font-weight: 550; white-space: nowrap; }
    ${S} .tag { margin-left: 4px; padding: 0 4px; border-radius: 4px; box-shadow: inset 0 0 0 1px var(--color-border); color: var(--color-text-secondary); font-weight: 450; white-space: nowrap; }
    ${S} .icon { display: grid; place-items: center; flex: none; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 5px; background: none; color: var(--color-icon); }
    ${S} .icon:hover { background: var(--color-bg-hover); }
    ${S} .icon[aria-expanded="true"] { background: var(--color-bg-selected, var(--color-bg-hover)); }
    ${S} .icon svg { display: block; }
    ${S} .head .add { margin-left: auto; }
    ${S} .item { display: flex; align-items: center; gap: 0; height: 32px; padding: 0 calc(var(--right) - 4px) 0 calc(var(--left) - 4px); }
    ${S} .name { display: flex; align-items: center; gap: 2px; flex: 1; min-width: 0; height: 24px; padding: 0 4px 0 0; border: 0; border-radius: 5px; background: none; text-align: left; }
    ${S} .name svg { flex: none; color: var(--color-icon-secondary); transition: transform 160ms cubic-bezier(0.2, 0, 0, 1); }
    ${S} .name[aria-expanded="true"] svg { transform: rotate(90deg); }
    ${S} .name span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    ${S} .name.flat { padding-left: 16px; }
    ${S} [role="list"] { position: relative; }
    ${S} .item { border-radius: 5px; }
    ${S} .item.picked { background: var(--color-bg-selected, #0d99ff26); }
    ${S} .item.lifted { position: relative; z-index: 1; background: var(--color-bg); box-shadow: var(--elevation-200-canvas, 0 2px 8px rgb(0 0 0 / 0.25)); }
    ${S} .drop { position: absolute; left: var(--left); right: var(--right); height: 2px; border-radius: 1px; background: var(--color-border-selected, #0d99ff); pointer-events: none; }
    ${S} .item.off .name span { color: var(--color-text-tertiary, var(--color-text-secondary)); }
    ${S} .item .icon { color: var(--color-icon-secondary); }
    ${S} .item .icon:hover { color: var(--color-icon); }
    ${S} .editor { padding: 0 var(--right) 8px var(--left); }
    ${S} .row { display: grid; grid-template-columns: var(--column) 1fr 36px; column-gap: 4px; align-items: center; height: 32px; }
    ${S} .row:not(:has(.value)) { grid-template-columns: var(--column) 1fr; column-gap: 0; }
    ${S} .row.pair { grid-template-columns: 48px 1fr 40px 36px; }
    ${S} .row.pair .end { color: var(--color-text-secondary); white-space: nowrap; overflow: hidden; }
    ${S} .label { display: flex; gap: 6px; min-width: 0; color: var(--color-text-secondary); white-space: nowrap; overflow: hidden; }
    ${S} .label i { font-style: normal; color: var(--color-text); font-variant-numeric: tabular-nums; }
    ${S} .label.scrub { align-self: stretch; align-items: center; cursor: ew-resize; }
    ${S} .value { width: 100%; height: 24px; box-sizing: border-box; padding: 0 4px; border: 0; border-radius: 5px; outline: 0; background: none; box-shadow: inset 0 0 0 1px transparent; color: var(--color-text-secondary); font: inherit; font-variant-numeric: tabular-nums; text-align: right; cursor: ew-resize; }
    ${S} .value.changed { color: var(--color-text); }
    ${S} .value:hover { box-shadow: inset 0 0 0 1px var(--color-border); }
    ${S} .value:focus { box-shadow: inset 0 0 0 1px var(--color-border-selected, #0d99ff); color: var(--color-text); cursor: text; }
    html[data-ligma-scrub], html[data-ligma-scrub] * { cursor: ew-resize !important; }
    ${S} .track { position: relative; height: 16px; border-radius: 8px; background: var(--color-bg-secondary); box-shadow: inset 0 0 0 1px var(--color-border); touch-action: none; outline: 0; }
    ${S} .track .fill { position: absolute; top: 0; bottom: 0; border-radius: 8px; background: var(--color-bg-brand, #0d99ff); }
    ${S} .track .middle { position: absolute; top: 6px; width: 4px; height: 4px; margin-left: -2px; border-radius: 2px; background: var(--color-icon-tertiary, #ffffff66); }
    ${S} .track.changed .middle { background: var(--color-text-onbrand, #fff); }
    ${S} .track.tinted .middle { display: none; }
    ${S} .track .knob { position: absolute; top: 0; width: 16px; height: 16px; margin-left: -8px; border-radius: 8px; background: #fff; box-shadow: 0 0 0 1px rgb(0 0 0 / 0.2), 0 1px 3px rgb(0 0 0 / 0.3); }
    ${S} .track.changed:not(.tinted) .knob { box-shadow: inset 0 0 0 3px var(--color-bg-brand, #0d99ff), 0 1px 3px rgb(0 0 0 / 0.3); }
    ${S} .track:focus-visible { outline: 0; }
    ${S} .track:focus-visible .knob { outline: 1px solid var(--color-border-selected, #0d99ff); outline-offset: 1px; }
    ${S} .seg { display: flex; gap: 2px; height: 24px; margin: 4px 0; padding: 2px; box-sizing: border-box; border-radius: 6px; background: var(--color-bg-secondary); }
    ${S} .seg button { flex: 1; min-width: 0; padding: 0 4px; border: 0; border-radius: 4px; background: none; color: var(--color-text-secondary); white-space: nowrap; }
    ${S} .seg button:hover { color: var(--color-text); }
    ${S} .seg button[aria-checked="true"] { background: var(--color-bg); color: var(--color-text); box-shadow: 0 0 0 1px var(--color-border); }
    ${S} .bar { display: flex; align-items: center; gap: 8px; }
    ${S} .bar .seg { flex: 1; }
    ${S} .text { height: 24px; padding: 0 6px; border: 0; border-radius: 5px; background: none; color: var(--color-text-secondary); white-space: nowrap; }
    ${S} .text:hover { background: var(--color-bg-hover); color: var(--color-text); }
    ${S} .check { display: flex; align-items: center; gap: 8px; width: 100%; height: 32px; padding: 0; border: 0; background: none; text-align: left; }
    ${S} .check .box { display: grid; place-items: center; flex: none; width: 14px; height: 14px; box-sizing: border-box; border-radius: 3px; box-shadow: inset 0 0 0 1px var(--color-border-strong, var(--color-icon-tertiary, #999)); color: var(--color-icon-onbrand, #fff); }
    ${S} .check[aria-checked="true"] .box { background: var(--color-bg-brand, #0d99ff); box-shadow: none; }
    ${S} .check:not([aria-checked="true"]) .box svg { visibility: hidden; }
    ${S} .select { display: flex; align-items: center; gap: 6px; width: 100%; height: 24px; margin: 4px 0; padding: 0 2px 0 8px; box-sizing: border-box; border: 0; border-radius: 5px; background: none; box-shadow: inset 0 0 0 1px var(--color-border); text-align: left; }
    ${S} .select:hover { box-shadow: inset 0 0 0 1px var(--color-border-strong, var(--color-icon-tertiary, #999)); }
    ${S} .select span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    ${S} .select svg { flex: none; color: var(--color-icon-secondary); }
    ${S} .select .swatch { flex: none; }
    ${S} .swatch { flex: none; width: 10px; height: 10px; border-radius: 5px; box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.2); }
    ${S} .chips { display: flex; justify-content: space-between; align-items: center; height: 36px; padding: 0 3px; }
    ${S} .chip { position: relative; flex: none; width: 18px; height: 18px; padding: 0; border: 0; border-radius: 50%; box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.18); }
    ${S} .chip:hover { box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.18), 0 0 0 2px var(--color-bg), 0 0 0 3px var(--color-border-strong, var(--color-icon-tertiary, #999)); }
    ${S} .chip[aria-checked="true"] { box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.18), 0 0 0 2px var(--color-bg), 0 0 0 3.5px var(--color-border-selected, #0d99ff); }
    ${S} .chip:focus-visible { outline-offset: 4px; border-radius: 50%; }
    ${S} .chip.changed::after { content: ""; position: absolute; top: -3px; right: -3px; width: 6px; height: 6px; box-sizing: border-box; border: 1.5px solid var(--color-bg); border-radius: 50%; background: var(--color-bg-brand, #0d99ff); }
    ${S} .caption { display: flex; align-items: center; justify-content: space-between; height: 24px; margin-top: 4px; color: var(--color-text-secondary); }
    ${S} .caption b { font-weight: 550; color: var(--color-text); }
    ${S} .graph { display: block; width: 100%; margin: 4px 0; border-radius: 5px; touch-action: none; }
    ${S} .handles { position: relative; height: 12px; margin: 0 0 2px; touch-action: none; }
    ${S} .handles.output { height: 12px; margin-top: 4px; border-radius: 3px; }
    ${S} .handles .ramp { position: absolute; left: 0; right: 0; top: 0; height: 6px; border-radius: 3px; box-shadow: inset 0 0 0 1px var(--color-border); }
    ${S} .handles .grip { position: absolute; top: 4px; width: 0; height: 0; margin-left: -5px; border: 5px solid transparent; border-top: 0; border-bottom: 8px solid var(--grip, #fff); filter: drop-shadow(0 0 1px rgb(0 0 0 / 0.6)); }
    ${S} .readout { display: flex; justify-content: space-between; height: 20px; align-items: center; color: var(--color-text-secondary); font-variant-numeric: tabular-nums; }
    ${S} .note { margin: 4px 0 0; padding: 0 var(--right) 8px var(--left); color: var(--color-text-secondary); }
    ${S} .file { height: 24px; display: flex; align-items: center; color: var(--color-text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    ${S} input[type="color"] { width: 100%; height: 24px; padding: 0; border: 0; border-radius: 5px; background: none; }
    ${S}[data-ligma-color="menu"] { position: fixed; z-index: 2147483000; min-width: 200px; max-height: 70vh; overflow: auto; overscroll-behavior: contain; scrollbar-width: thin; scrollbar-color: #b3b3b380 transparent; padding: 8px; box-sizing: border-box; border-radius: 13px; background: var(--color-bg-menu, #1e1e1e); color: var(--color-text-menu, #fff); box-shadow: var(--elevation-400-menu-panel, 0 5px 17px rgb(0 0 0 / 0.2), 0 2px 7px rgb(0 0 0 / 0.15)); outline: 0; }
    ${S} .mi { display: grid; grid-template-columns: 16px 1fr auto; align-items: center; gap: 8px; height: 24px; padding: 0 8px; border-radius: 5px; white-space: nowrap; }
    ${S} .mi[data-on="true"] { background: var(--color-bg-menu-selected, #0d99ff); color: var(--color-text-menu, #fff); }
    ${S} .mh { display: flex; align-items: center; height: 24px; padding: 0 8px 0 32px; color: var(--color-text-menu-secondary, #ffffffb3); white-space: nowrap; }
    ${S} .swatch.ramp { width: 24px; height: 10px; border-radius: 3px; }
    ${S} .gbar { position: relative; height: 24px; margin: 8px 0 0; border-radius: 5px; box-shadow: inset 0 0 0 1px var(--color-border); touch-action: none; }
    ${S} .gstops { position: relative; height: 18px; margin: 2px 0 4px; touch-action: none; }
    ${S} .gstop { position: absolute; top: 5px; width: 12px; height: 12px; margin-left: -6px; box-sizing: border-box; border: 2px solid var(--color-bg); border-radius: 3px; box-shadow: 0 0 0 1px var(--color-border-strong, var(--color-icon-tertiary, #999)); }
    ${S} .gstop::before { content: ""; position: absolute; left: 50%; top: -9px; margin-left: -3px; border: 3px solid transparent; border-bottom: 3px solid var(--color-border-strong, var(--color-icon-tertiary, #999)); }
    ${S} .gstop.on { box-shadow: 0 0 0 1.5px var(--color-border-selected, #0d99ff); }
    ${S} .gstop.on::before { border-bottom-color: var(--color-border-selected, #0d99ff); }
    ${S} .mi[aria-disabled="true"] { color: var(--color-text-menu-tertiary, #ffffff59); }
    ${S} .mi .hint { color: var(--color-text-menu-secondary, #ffffffb3); }
    ${S} .mi[data-on="true"] .hint { color: inherit; }
    ${S} .mi svg { display: block; }
    ${S} .sep { height: 1px; margin: 8px -8px; background: var(--color-border-menu, #ffffff1a); }`;

  const element = (tag, props = {}, ...children) => {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children.filter((child) => child != null));
    return node;
  };
  const svg = (html) => {
    const holder = document.createElement("span");
    holder.innerHTML = html;
    return holder.firstChild;
  };
  const STROKE = 'stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"';
  const ICONS = {
    chevron: `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M6.5 5L9.5 8L6.5 11" ${STROKE}/></svg>`,
    down: `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M5.5 7L8 9.5L10.5 7" ${STROKE}/></svg>`,
    plus: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M12 7.5V16.5M7.5 12H16.5" ${STROKE}/></svg>`,
    minus: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M7.5 12H16.5" ${STROKE}/></svg>`,
    eye: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M5.5 12C6.6 9.9 9 8.5 12 8.5C15 8.5 17.4 9.9 18.5 12C17.4 14.1 15 15.5 12 15.5C9 15.5 6.6 14.1 5.5 12Z" ${STROKE}/><circle cx="12" cy="12" r="1.75" ${STROKE}/></svg>`,
    hidden: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M7.2 9.6C6.5 10.3 5.9 11.1 5.5 12C6.6 14.1 9 15.5 12 15.5C12.9 15.5 13.8 15.4 14.6 15.1M10 8.7C10.6 8.6 11.3 8.5 12 8.5C15 8.5 17.4 9.9 18.5 12C18.1 12.8 17.6 13.5 17 14.1M6.5 6.5L17.5 17.5" ${STROKE}/></svg>`,
    check: `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M4.5 8.5L7 11L11.5 5.5" ${STROKE}/></svg>`,
    tick: `<svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true"><path d="M2 5.2L4.1 7.2L8 2.8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  };

  // Presses go through the router below, ahead of Figma: an element with
  // `press(event)` starts a drag of its own, `{ move, end }`
  let dragging = null;
  const drag = (event, move, end) => {
    event.preventDefault();
    try {
      event.target.setPointerCapture(event.pointerId);
    } catch {}
    const held = {
      move,
      end: () => {
        if (dragging !== held) return;
        dragging = null;
        end?.();
      },
    };
    dragging = held;
    event.target.addEventListener("lostpointercapture", held.end, { once: true });
  };

  // A slider as Figma's in the picker: filled from `origin`, 0 or its
  // least, with round ends, snapping to it near it unless Alt (Option) is
  // held, double-click for its default, and its value in a field beside
  // it. `set(value, done)`, `done` once it's let go
  const SNAP = 6;
  const slider = (label, o, get, set) => {
    const { min, max, step = 1, right = null, gradient = null } = o;
    // `def` and `origin` may change with what the editor shows, as Channel
    // Mixer's by output channel
    const now = (value) => (typeof value === "function" ? value() : value);
    const defOf = () => now(o.def) ?? 0;
    const originOf = () => now(o.origin) ?? (min >= 0 ? min : 0);
    const digits = step < 1 ? 1 : 0;
    const track = element("div", { className: "track", tabIndex: 0 });
    track.setAttribute("role", "slider");
    track.setAttribute("aria-label", right ? `${label} to ${right}` : label);
    track.setAttribute("aria-valuemin", String(min));
    track.setAttribute("aria-valuemax", String(max));
    if (gradient) {
      track.style.background = gradient;
      track.classList.add("tinted");
    }
    const fill = gradient ? null : track.appendChild(element("div", { className: "fill" }));
    const middle = track.appendChild(element("div", { className: "middle" }));
    const knob = track.appendChild(element("div", { className: "knob" }));
    const place = (value) => (value - min) / (max - min);
    const round = (value) => Math.round(value / step) * step;
    const valueAt = (event) => {
      const box = track.getBoundingClientRect();
      const x = event.clientX - box.left - 8;
      const width = box.width - 16;
      const snap = originOf();
      if (!event.altKey && Math.abs(x - place(snap) * width) <= SNAP) return snap;
      return clamp(round(min + (x / width) * (max - min)), min, max);
    };
    track.press = (event) => {
      if (event.button !== 0) return;
      track.focus({ preventScroll: true });
      set(valueAt(event), false);
      drag(event, (event) => set(valueAt(event), false), () => set(get(), true));
    };
    track.addEventListener("dblclick", () => set(defOf(), true));
    track.key = (event) => {
      const by = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1 }[event.key];
      if (by === undefined) return false;
      const value = get();
      const origin = originOf();
      const next = clamp(round(value + by * step * (event.shiftKey ? 10 : 1)), min, max);
      // A step across the origin stops on it first
      set(value !== origin && Math.sign(next - origin) === -Math.sign(value - origin) ? origin : next, true);
      return true;
    };
    // The value, by the track as Figma's fields: dragged across, or its
    // label, to change it, or clicked to type one
    const text = (value) => `${min < 0 && value > 0 ? "+" : ""}${value.toFixed(digits)}`;
    const field = element("input", { type: "text", className: "value", spellcheck: false, autocomplete: "off" });
    field.setAttribute("aria-label", right ? `${label} to ${right}` : label);
    field.inputMode = "decimal";
    const typed = () => {
      const value = Number.parseFloat(field.value.replace(/^\+/, ""));
      if (Number.isFinite(value)) set(clamp(round(value), min, max), true);
      else field.value = text(get());
    };
    field.addEventListener("change", typed);
    field.key = (event) => {
      if (event.key === "Enter") {
        typed();
        field.blur();
        return true;
      }
      if (event.key === "Escape") {
        field.value = text(get());
        field.blur();
        return true;
      }
      const by = { ArrowUp: 1, ArrowDown: -1 }[event.key];
      if (by === undefined) return false;
      set(clamp(round(get() + by * step * (event.shiftKey ? 10 : 1)), min, max), true);
      field.select();
      return true;
    };
    const scrub = (event, focusAfter) => {
      if (event.button !== 0) return;
      const [x, from] = [event.clientX, get()];
      let moved = false;
      drag(
        event,
        (event) => {
          const dx = event.clientX - x;
          if (!moved && Math.abs(dx) < 3) return;
          moved = true;
          document.documentElement.dataset.ligmaScrub = "";
          set(clamp(round(from + dx * step * (event.shiftKey ? 10 : 1)), min, max), false);
        },
        () => {
          delete document.documentElement.dataset.ligmaScrub;
          if (moved) set(get(), true);
          else if (focusAfter) {
            field.focus({ preventScroll: true });
            field.select();
          }
        },
      );
    };
    // A press in a field already typed in places the caret
    field.press = (event) => document.activeElement !== field && scrub(event, true);
    const name = element("span", { className: "label scrub" }, label);
    name.press = (event) => scrub(event, false);
    const row = right ? element("div", { className: "row pair" }, name, track, element("span", { className: "end", textContent: right }), field) : element("div", { className: "row" }, name, track, field);
    row.show = () => {
      const value = get();
      const origin = originOf();
      const def = defOf();
      const at = place(value);
      const from = place(origin);
      knob.style.left = `calc(8px + (100% - 16px) * ${at})`;
      middle.style.left = `calc(8px + (100% - 16px) * ${from})`;
      if (fill) {
        const [lo, hi] = [Math.min(at, from), Math.max(at, from)];
        fill.hidden = value === origin;
        fill.style.left = `calc((100% - 16px) * ${lo})`;
        fill.style.width = `calc((100% - 16px) * ${hi - lo} + 16px)`;
      }
      track.classList.toggle("changed", value !== origin);
      track.setAttribute("aria-valuenow", String(value));
      if (document.activeElement !== field) field.value = text(value);
      field.classList.toggle("changed", value !== def);
    };
    return row;
  };
  const segmented = (label, options, get, set) => {
    const seg = element("div", { className: "seg" });
    seg.setAttribute("role", "radiogroup");
    seg.setAttribute("aria-label", label);
    const buttons = options.map(([id, text]) => {
      const button = seg.appendChild(element("button", { type: "button", textContent: text }));
      button.setAttribute("role", "radio");
      button.addEventListener("click", () => set(id));
      button.key = (event) => {
        const by = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
        if (by === undefined) return false;
        const at = options.findIndex(([each]) => each === get());
        const next = options[(at + by + options.length) % options.length][0];
        set(next);
        buttons[options.findIndex(([each]) => each === next)].focus();
        return true;
      };
      return button;
    });
    seg.show = () => buttons.forEach((button, i) => {
      const on = options[i][0] === get();
      button.setAttribute("aria-checked", String(on));
      button.tabIndex = on ? 0 : -1;
    });
    return seg;
  };
  const checkbox = (label, get, set) => {
    const box = element("span", { className: "box" }, svg(ICONS.tick));
    const button = element("button", { type: "button", className: "check" }, box, element("span", { textContent: label }));
    button.setAttribute("role", "checkbox");
    button.addEventListener("click", () => set(!get()));
    button.show = () => button.setAttribute("aria-checked", String(!!get()));
    return button;
  };
  // Swatches to pick a color range by, circles with a ring when picked and
  // a dot when changed
  const chips = (label, options, get, set, changedOf) => {
    const holder = element("div", { className: "chips" });
    holder.setAttribute("role", "radiogroup");
    holder.setAttribute("aria-label", label);
    const buttons = options.map(([id, name, color]) => {
      const button = holder.appendChild(element("button", { type: "button", className: "chip", title: name }));
      button.style.background = color;
      button.setAttribute("role", "radio");
      button.setAttribute("aria-label", name);
      button.addEventListener("click", () => set(id));
      button.key = (event) => {
        const by = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
        if (by === undefined) return false;
        const at = options.findIndex(([each]) => each === get());
        const next = (at + by + options.length) % options.length;
        set(options[next][0]);
        buttons[next].focus();
        return true;
      };
      return button;
    });
    holder.show = () => buttons.forEach((button, i) => {
      const on = options[i][0] === get();
      button.setAttribute("aria-checked", String(on));
      button.tabIndex = on ? 0 : -1;
      button.classList.toggle("changed", changedOf(options[i][0]));
    });
    return holder;
  };

  // Figma's menus: grouped, a line between groups, checks and hints. Keys
  // and presses in it stay with it
  let menu = null;
  const closeMenu = () => {
    menu?.close();
    menu = null;
  };
  const openMenu = (anchor, entries, pick) => {
    closeMenu();
    const holder = element("div", { tabIndex: -1 });
    holder.dataset.ligmaColor = "menu";
    holder.setAttribute("role", "menu");
    const items = [];
    for (const entry of entries) {
      if (entry === "-") {
        holder.append(element("div", { className: "sep" }));
        continue;
      }
      if (entry.heading) {
        holder.append(element("div", { className: "mh", textContent: entry.heading }));
        continue;
      }
      const mark = entry.checked ? svg(ICONS.check) : entry.color ? element("span", { className: `swatch${entry.color.includes("gradient") ? " ramp" : ""}`, style: `background: ${entry.color}` }) : element("span");
      const row = holder.appendChild(element("div", { className: "mi" }, mark, element("span", { textContent: entry.label }), entry.hint ? element("span", { className: "hint", textContent: entry.hint }) : null));
      row.setAttribute("role", "menuitem");
      if (entry.disabled) row.setAttribute("aria-disabled", "true");
      else {
        items.push(row);
        row.addEventListener("pointerenter", () => light(items.indexOf(row)));
        row.addEventListener("click", () => {
          closeMenu();
          pick(entry.id);
        });
        row.entry = entry;
      }
    }
    let lit = -1;
    const light = (i) => {
      lit = i;
      items.forEach((row, j) => (row.dataset.on = String(i === j)));
    };
    holder.key = (event) => {
      if (event.key === "Escape") closeMenu();
      else if (event.key === "Enter" || event.key === " ") {
        if (items[lit]) {
          const { id } = items[lit].entry;
          closeMenu();
          pick(id);
        }
      } else {
        const by = { ArrowDown: 1, ArrowUp: -1 }[event.key];
        if (by === undefined) return false;
        light((lit + by + items.length) % items.length);
      }
      return true;
    };
    document.body.append(holder);
    if (!style.isConnected) document.head.append(style);
    const box = anchor.getBoundingClientRect();
    const own = holder.getBoundingClientRect();
    const left = clamp(box.right - own.width, 8, innerWidth - own.width - 8);
    const top = box.bottom + 4 + own.height <= innerHeight - 8 ? box.bottom + 4 : Math.max(8, box.top - 4 - own.height);
    Object.assign(holder.style, { left: `${left}px`, top: `${top}px` });
    holder.focus({ preventScroll: true });
    const back = document.activeElement === holder ? anchor : null;
    menu = {
      holder,
      close: () => {
        holder.remove();
        anchor.setAttribute("aria-expanded", "false");
        back?.focus?.({ preventScroll: true });
      },
    };
    anchor.setAttribute("aria-expanded", "true");
  };
  // A field opening a menu to pick one of `options`, `[id, label, color?]`
  const select = (label, options, get, set) => {
    const text = element("span");
    const dot = element("span", { className: "swatch" });
    const button = element("button", { type: "button", className: "select" }, dot, text, svg(ICONS.down));
    button.setAttribute("aria-label", label);
    button.setAttribute("aria-haspopup", "menu");
    button.addEventListener("click", () => openMenu(button, options.map(([id, name, color]) => (id === "-" ? "-" : { id, label: name, color, checked: id === get() })), set));
    button.show = () => {
      const option = options.find(([id]) => id === get()) ?? options[0];
      text.textContent = option[1];
      dot.hidden = !option[2];
      if (option[2]) dot.style.background = option[2];
    };
    return button;
  };

  // A histogram of the original, for Levels and Curves
  const drawHistogram = (context, counts, width, height, color) => {
    if (!counts) return;
    let peak = 1;
    const sorted = [...counts].sort((a, b) => b - a);
    peak = Math.max(1, sorted[3] ?? sorted[0]);
    context.fillStyle = color;
    context.beginPath();
    context.moveTo(0, height);
    for (let x = 0; x < 256; x++) context.lineTo((x / 255) * width, height - Math.min(1, counts[x] / peak) * height * 0.92);
    context.lineTo(width, height);
    context.closePath();
    context.fill();
  };
  const CHANNELS = [
    ["rgb", "RGB"],
    ["red", "Red"],
    ["green", "Green"],
    ["blue", "Blue"],
  ];
  const INK = { rgb: null, red: "#f24822", green: "#14ae5c", blue: "#0d99ff" };
  // Figma's colors where `near` is, as its themes set them on its own
  // elements rather than the page
  const theme = (near, name, fallback) => getComputedStyle(near).getPropertyValue(name).trim() || fallback;
  const sized = (canvas, height) => {
    const width = Math.max(100, canvas.clientWidth || 208);
    const ratio = devicePixelRatio || 1;
    if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) Object.assign(canvas, { width: Math.round(width * ratio), height: Math.round(height * ratio) });
    canvas.style.height = `${height}px`;
    const context = canvas.getContext("2d");
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    return [context, width];
  };

  // ---------- Each adjustment's controls ----------

  // `c` is what an editor needs: `item()` its settings now, `update(fn,
  // done)` to change them, `histogram()` the original's, or null while it
  // loads
  const EDITORS = {
    levels: (c) => {
      let channel = "rgb";
      const values = () => c.item()[channel];
      const graph = element("canvas", { className: "graph" });
      const input = element("div", { className: "handles" });
      const ramp = input.appendChild(element("div", { className: "ramp" }));
      const grips = ["#000", "#808080", "#fff"].map((color) => {
        const grip = input.appendChild(element("div", { className: "grip" }));
        grip.style.setProperty("--grip", color);
        return grip;
      });
      const inputText = element("div", { className: "readout" });
      const output = element("div", { className: "handles output" });
      const outRamp = output.appendChild(element("div", { className: "ramp" }));
      const outGrips = ["#000", "#fff"].map((color) => {
        const grip = output.appendChild(element("div", { className: "grip" }));
        grip.style.setProperty("--grip", color);
        return grip;
      });
      const outputText = element("div", { className: "readout" });
      // The midtones' grip sits as Photoshop's: halfway at gamma 1, left
      // for more
      const gammaAt = (v) => v[0] + (v[2] - v[0]) * 0.5 ** v[1];
      const atFrom = (event, holder) => {
        const box = holder.getBoundingClientRect();
        return clamp(Math.round(((event.clientX - box.left) / box.width) * 255), 0, 255);
      };
      input.press = (event) => {
        if (event.button !== 0) return;
        const x = atFrom(event, input);
        const v = values();
        const which = [v[0], gammaAt(v), v[2]].map((at, i) => [Math.abs(at - x), i]).sort((a, b) => a[0] - b[0])[0][1];
        const move = (event, done) => {
          const at = atFrom(event, input);
          c.update((item) => {
            const next = item[channel];
            if (which === 0) next[0] = clamp(at, 0, next[2] - 2);
            else if (which === 2) next[2] = clamp(at, next[0] + 2, 255);
            else next[1] = Math.round(clamp(Math.log(clamp((at - next[0]) / Math.max(1, next[2] - next[0]), 0.002, 0.998)) / Math.log(0.5), 0.1, 9.99) * 100) / 100;
          }, done);
        };
        move(event, false);
        drag(event, (event) => move(event, false), () => c.update(() => {}, true));
      };
      output.press = (event) => {
        if (event.button !== 0) return;
        const x = atFrom(event, output);
        const v = values();
        const which = Math.abs(v[3] - x) <= Math.abs(v[4] - x) ? 3 : 4;
        const move = (event, done) => c.update((item) => (item[channel][which] = atFrom(event, output)), done);
        move(event, false);
        drag(event, (event) => move(event, false), () => c.update(() => {}, true));
      };
      // Auto, as Photoshop's: each channel's darkest and lightest tenth of
      // a percent made black and white
      const auto = element("button", { type: "button", className: "text", textContent: "Auto" });
      auto.addEventListener("click", () => {
        const histogram = c.histogram();
        if (!histogram) return;
        c.update((item) => {
          for (const [name, counts] of [["red", histogram.r], ["green", histogram.g], ["blue", histogram.b]]) {
            const total = counts.reduce((sum, count) => sum + count, 0);
            let [lo, hi, seen] = [0, 255, 0];
            while (lo < 254 && (seen += counts[lo]) < total * 0.001) lo++;
            seen = 0;
            while (hi > lo + 2 && (seen += counts[hi]) < total * 0.001) hi--;
            item[name] = [lo, 1, hi, 0, 255];
          }
          item.rgb = [0, 1, 255, 0, 255];
        }, true);
      });
      const channels = segmented("Channel", CHANNELS, () => channel, (id) => ((channel = id), c.refresh()));
      const show = () => {
        const v = values();
        const [context, width] = sized(graph, 72);
        context.fillStyle = theme(graph, "--color-bg-secondary", "#f5f5f5");
        context.fillRect(0, 0, width, 72);
        const histogram = c.histogram();
        drawHistogram(context, histogram?.[{ rgb: "l", red: "r", green: "g", blue: "b" }[channel]], width, 72, INK[channel] ? `${INK[channel]}99` : theme(graph, "--color-icon-tertiary", "#999"));
        const ink = INK[channel] ?? "#fff";
        ramp.style.background = `linear-gradient(90deg, #000, ${ink})`;
        outRamp.style.background = `linear-gradient(90deg, #000, ${ink})`;
        [v[0], gammaAt(v), v[2]].forEach((at, i) => (grips[i].style.left = `${(at / 255) * 100}%`));
        outGrips[0].style.left = `${(v[3] / 255) * 100}%`;
        outGrips[1].style.left = `${(v[4] / 255) * 100}%`;
        inputText.replaceChildren(element("span", { textContent: v[0] }), element("span", { textContent: v[1].toFixed(2) }), element("span", { textContent: v[2] }));
        outputText.replaceChildren(element("span", { textContent: v[3] }), element("span", { textContent: v[4] }));
      };
      return { nodes: [element("div", { className: "bar" }, channels, auto), graph, input, inputText, element("div", { className: "caption" }, element("span", { textContent: "Output" })), output, outputText], parts: [channels], show };
    },

    curves: (c) => {
      let channel = "rgb";
      let picked = 1;
      const graph = element("canvas", { className: "graph" });
      const readout = element("div", { className: "readout" });
      const SIDE = () => graph.clientWidth || 208;
      const toPoint = (event) => {
        const box = graph.getBoundingClientRect();
        return [clamp(Math.round(((event.clientX - box.left) / box.width) * 255), 0, 255), clamp(Math.round((1 - (event.clientY - box.top) / box.height) * 255), 0, 255), event.clientY < box.top - 24 || event.clientY > box.bottom + 24];
      };
      graph.press = (event) => {
        if (event.button !== 0) return;
        const points = c.item()[channel];
        const [x, y] = toPoint(event);
        const reach = (8 / SIDE()) * 255;
        let at = points.findIndex(([px, py]) => Math.hypot(px - x, py - y) <= reach);
        if (at < 0) {
          // A new point, where the press was, between its neighbors
          if (points.some(([px]) => Math.abs(px - x) < 3) || points.length >= 16) return;
          at = points.findIndex(([px]) => px > x);
          c.update((item) => item[channel].splice(at, 0, [x, y]), false);
        }
        picked = at;
        let gone = false;
        const move = (event) => {
          const [mx, my, away] = toPoint(event);
          c.update((item) => {
            const list = item[channel];
            const ends = picked === 0 || picked === list.length - 1;
            gone = away && !ends && list.length > 2;
            const lo = picked > 0 ? list[picked - 1][0] + 1 : 0;
            const hi = picked < list.length - 1 ? list[picked + 1][0] - 1 : 255;
            list[picked] = [clamp(mx, lo, hi), my];
          }, false);
          graph.style.opacity = gone ? "0.6" : "";
        };
        drag(event, move, () => {
          graph.style.opacity = "";
          c.update((item) => {
            if (gone) {
              item[channel].splice(picked, 1);
              picked = Math.max(0, picked - 1);
            }
          }, true);
        });
      };
      graph.addEventListener("dblclick", () => c.update((item) => (item[channel] = curve()), true));
      const channels = segmented("Channel", CHANNELS, () => channel, (id) => ((channel = id), (picked = 0), c.refresh()));
      const show = () => {
        const points = c.item()[channel];
        picked = Math.min(picked, points.length - 1);
        const side = SIDE();
        const [context] = sized(graph, side);
        context.fillStyle = theme(graph, "--color-bg-secondary", "#f5f5f5");
        context.fillRect(0, 0, side, side);
        drawHistogram(context, c.histogram()?.[{ rgb: "l", red: "r", green: "g", blue: "b" }[channel]], side, side, theme(graph, "--color-border", "#e6e6e6"));
        context.strokeStyle = theme(graph, "--color-border", "#e6e6e6");
        context.lineWidth = 1;
        context.beginPath();
        for (let i = 1; i < 4; i++) {
          const at = Math.round((side * i) / 4) + 0.5;
          context.moveTo(at, 0);
          context.lineTo(at, side);
          context.moveTo(0, at);
          context.lineTo(side, at);
        }
        context.moveTo(0, side);
        context.lineTo(side, 0);
        context.stroke();
        const ink = INK[channel] ?? theme(graph, "--color-text", "#000");
        const values = spline(points);
        context.strokeStyle = ink;
        context.lineWidth = 1.5;
        context.beginPath();
        for (let x = 0; x < 256; x++) context[x ? "lineTo" : "moveTo"]((x / 255) * side, side - (values[x] / 255) * side);
        context.stroke();
        points.forEach(([x, y], i) => {
          context.beginPath();
          context.arc((x / 255) * side, side - (y / 255) * side, 4, 0, Math.PI * 2);
          context.fillStyle = i === picked ? ink : theme(graph, "--color-bg", "#fff");
          context.fill();
          context.strokeStyle = ink;
          context.lineWidth = 1.5;
          context.stroke();
        });
        const [x, y] = points[picked];
        readout.replaceChildren(element("span", { textContent: `Input ${x}` }), element("span", { textContent: `Output ${y}` }));
      };
      return { nodes: [channels, graph, readout], parts: [channels], show };
    },

    presence: (c) => {
      const rows = [
        ["texture", "Texture"],
        ["clarity", "Clarity"],
        ["dehaze", "Dehaze"],
      ].map(([key, label]) => c.slider(label, { min: -100, max: 100 }, key));
      return { nodes: rows, parts: rows };
    },

    sharpen: (c) => {
      const rows = [c.slider("Amount", { min: 0, max: 150, def: 40 }, "amount"), c.slider("Radius", { min: 0.5, max: 3, step: 0.1, def: 1, origin: 0.5 }, "radius"), c.slider("Detail", { min: 0, max: 100, def: 25 }, "detail"), c.slider("Masking", { min: 0, max: 100 }, "masking")];
      return { nodes: rows, parts: rows };
    },

    vibrance: (c) => {
      const rows = [c.slider("Vibrance", { min: -100, max: 100 }, "vibrance"), c.slider("Saturation", { min: -100, max: 100 }, "saturation")];
      return { nodes: rows, parts: rows };
    },

    hsl: (c) => {
      let range = "master";
      const options = [["master", "All colors", "conic-gradient(from 90deg, #f33, #fd3, #3d5, #3de, #36f, #e3e, #f33)"], ...RANGES.map(([id, name, hue]) => [id, name, `hsl(${hue} 85% 55%)`])];
      const pick = chips("Colors", options, () => range, (id) => ((range = id), c.refresh()), (id) => {
        const values = c.item()[id];
        return !!(values.hue || values.saturation || values.lightness);
      });
      const caption = element("span");
      const head = element("div", { className: "caption" }, element("b", { textContent: "Range" }), caption);
      const shift = [
        c.slider("Hue", { min: -180, max: 180 }, () => [range, "hue"]),
        c.slider("Saturation", { min: -100, max: 100 }, () => [range, "saturation"]),
        c.slider("Lightness", { min: -100, max: 100 }, () => [range, "lightness"]),
      ];
      const tone = [
        c.slider("Hue", { min: 0, max: 360, origin: 0, gradient: "linear-gradient(90deg, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00)" }, ["tone", "hue"]),
        c.slider("Saturation", { min: 0, max: 100, def: 25 }, ["tone", "saturation"]),
        c.slider("Lightness", { min: -100, max: 100 }, ["tone", "lightness"]),
      ];
      const colorize = c.check("Colorize", "colorize");
      const show = () => {
        const on = c.item().colorize;
        head.hidden = pick.hidden = on;
        shift.forEach((row) => (row.hidden = on));
        tone.forEach((row) => (row.hidden = !on));
        caption.textContent = options.find(([id]) => id === range)[1];
      };
      return { nodes: [head, pick, ...shift, ...tone, colorize], parts: [pick, ...shift, ...tone, colorize], show };
    },

    balance: (c) => {
      let tone = "midtones";
      const tones = segmented("Tone", [["shadows", "Shadows"], ["midtones", "Midtones"], ["highlights", "Highlights"]], () => tone, (id) => ((tone = id), c.refresh()));
      const rows = [
        ["Cyan", "Red", "#00c8ff", "#ff3b30"],
        ["Magenta", "Green", "#ff2dbe", "#34c759"],
        ["Yellow", "Blue", "#ffcc00", "#0a84ff"],
      ].map(([from, to, a, b], i) => c.slider(from, { min: -100, max: 100, right: to, gradient: `linear-gradient(90deg, ${a}, #9a9a9a 50%, ${b})` }, () => [tone, i]));
      const preserve = c.check("Preserve luminosity", "preserve");
      return { nodes: [tones, ...rows, preserve], parts: [tones, ...rows, preserve] };
    },

    mono: (c) => {
      // Filled from Photoshop's defaults, where they start
      const rows = RANGES.map(([id, name]) => c.slider(name, { min: -200, max: 300, def: fresh("mono")[id], origin: fresh("mono")[id] }, id));
      const tint = c.check("Tint", "tint");
      const tone = [c.slider("Hue", { min: 0, max: 360, origin: 0, def: 42, gradient: "linear-gradient(90deg, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00)" }, "hue"), c.slider("Saturation", { min: 0, max: 100, def: 20 }, "saturation")];
      const show = () => tone.forEach((row) => (row.hidden = !c.item().tint));
      return { nodes: [...rows, tint, ...tone], parts: [...rows, tint, ...tone], show };
    },

    photo: (c) => {
      const filter = select("Filter", FILTERS.map(([id, name, color]) => [id, name, color ?? undefined]), () => c.item().filter, (id) => c.update((item) => (item.filter = id), true));
      const color = element("input", { type: "color" });
      color.setAttribute("aria-label", "Filter color");
      color.addEventListener("input", () => c.update((item) => (item.color = color.value), true));
      const colorRow = element("div", { className: "row" }, element("span", { className: "label", textContent: "Color" }), color);
      const density = c.slider("Density", { min: 0, max: 100, def: 25 }, "density");
      const preserve = c.check("Preserve luminosity", "preserve");
      const show = () => {
        colorRow.hidden = c.item().filter !== "custom";
        color.value = c.item().color;
      };
      return { nodes: [filter, colorRow, density, preserve], parts: [filter, density, preserve], show };
    },

    mixer: (c) => {
      let output = "red";
      const outputs = segmented("Output channel", [["red", "Red"], ["green", "Green"], ["blue", "Blue"]], () => output, (id) => ((output = id), c.refresh()));
      // Each output starts as its own channel, all of it
      const own = (i) => () => (["red", "green", "blue"][i] === output ? 100 : 0);
      const rows = ["Red", "Green", "Blue", "Constant"].map((name, i) => c.slider(name, { min: -200, max: 200, def: own(i), origin: own(i) }, () => [output, i]));
      const total = element("div", { className: "readout" });
      const mono = c.check("Monochrome", "monochrome");
      const show = () => {
        const values = c.item()[output];
        const sum = values[0] + values[1] + values[2];
        total.replaceChildren(element("span", { textContent: "Total" }), element("span", { textContent: `${sum > 0 ? "+" : ""}${sum}%` }));
      };
      return { nodes: [outputs, ...rows, total, mono], parts: [outputs, ...rows, mono], show };
    },

    lookup: (c) => {
      const file = element("input", { type: "file", accept: ".cube,.3dl,.look" });
      file.hidden = true;
      let opening = "";
      // A table file's, made 17 across and kept on the layer
      const keep = (name, text) => {
        let data = null;
        try {
          const table = readTable(name, text);
          data = table && keepTable(table);
        } catch {}
        if (!data) return api()?.notify(`Couldn't read ${name} as a 3D LUT`, { timeout: 3000 });
        c.update((item) => Object.assign(item, { lut: "file", name, size: LUT_SIZE, data }), true);
      };
      file.addEventListener("change", async () => {
        const chosen = file.files?.[0];
        file.value = "";
        if (chosen) keep(chosen.name, await chosen.text().catch(() => ""));
      });
      // Photoshop's, asked of Ligma by name (see handoff.rs)
      const fromPhotoshop = async (name) => {
        opening = name;
        c.refresh();
        try {
          const text = await window.__ligmaHandOff?.("lut", new TextEncoder().encode(name), null, "", null, 20000);
          if (typeof text !== "string") throw new Error("none");
          keep(name, text);
        } catch {
          api()?.notify(`Couldn't open ${name} from Photoshop`, { timeout: 3000 });
        }
        opening = "";
        c.refresh();
      };
      const text = element("span");
      const field = element("button", { type: "button", className: "select" }, text, svg(ICONS.down));
      field.setAttribute("aria-label", "Look");
      field.setAttribute("aria-haspopup", "menu");
      // As Photoshop's: Load 3D LUT first, then the looks, Ligma's and,
      // where it's installed, Photoshop's own
      field.addEventListener("click", () => {
        const item = c.item();
        const ours = LOOKS.map(([id, name]) => [id, name]).sort((a, b) => a[1].localeCompare(b[1]));
        const loaded = item.lut === "file" && item.data && !PHOTOSHOP_LUTS.includes(item.name);
        openMenu(
          field,
          [
            { id: "load", label: "Load 3D LUT…" },
            ...(loaded ? [{ id: "file", label: item.name || "Loaded table", checked: true }] : []),
            "-",
            { heading: "Ligma" },
            ...ours.map(([id, name]) => ({ id, label: name, checked: item.lut === id })),
            ...(PHOTOSHOP_LUTS.length ? ["-", { heading: "Photoshop" }, ...PHOTOSHOP_LUTS.map((name) => ({ id: `photoshop:${name}`, label: name, checked: item.lut === "file" && item.name === name }))] : []),
          ],
          (id) => {
            if (id === "load") file.click();
            else if (id.startsWith("photoshop:")) fromPhotoshop(id.slice("photoshop:".length));
            else c.update((each) => (each.lut = id), true);
          },
        );
      });
      const intensity = c.slider("Intensity", { min: 0, max: 100, def: 100 }, "intensity");
      const show = () => {
        const item = c.item();
        text.textContent = opening ? `Opening ${opening}…` : item.lut === "file" ? item.name || "Loaded table" : (LOOKS.find(([id]) => id === item.lut)?.[1] ?? "");
      };
      return { nodes: [field, file, intensity], parts: [intensity], show };
    },

    gradient: (c) => {
      let picked = 0;
      const presetDot = element("span", { className: "swatch ramp" });
      const presetText = element("span");
      const preset = element("button", { type: "button", className: "select" }, presetDot, presetText, svg(ICONS.down));
      preset.setAttribute("aria-label", "Gradient");
      preset.setAttribute("aria-haspopup", "menu");
      preset.addEventListener("click", () => {
        const item = c.item();
        openMenu(
          preset,
          GRADIENTS.map(([id, name, stops]) => ({ id, label: name, color: gradientCss(stops), checked: item.preset === id })),
          (id) => {
            const stops = GRADIENTS.find(([each]) => each === id)[2];
            picked = 0;
            c.update((each) => Object.assign(each, { preset: id, stops: stops.map(([at, color]) => [at, color]) }), true);
          },
        );
      });
      // The gradient, pressed to add a stop there, and its stops below it,
      // dragged along or off to take one away
      const bar = element("div", { className: "gbar" });
      const marks = element("div", { className: "gstops" });
      const at = (event) => {
        const box = marks.getBoundingClientRect();
        return clamp(Math.round(((event.clientX - box.left) / box.width) * 100), 0, 100);
      };
      const slide = (event) => {
        let gone = false;
        const step = (event) => {
          const box = marks.getBoundingClientRect();
          gone = c.item().stops.length > 2 && (event.clientY > box.bottom + 24 || event.clientY < box.top - 48);
          c.update((item) => {
            const stop = item.stops[picked];
            if (!stop) return;
            stop[0] = at(event);
            item.stops.sort((a, b) => a[0] - b[0]);
            picked = item.stops.indexOf(stop);
            item.preset = "custom";
          }, false);
          marks.style.opacity = gone ? "0.5" : "";
        };
        drag(event, step, () => {
          marks.style.opacity = "";
          c.update((item) => {
            if (!gone || item.stops.length <= 2) return;
            item.stops.splice(picked, 1);
            picked = Math.max(0, picked - 1);
            item.preset = "custom";
          }, true);
        });
      };
      marks.press = (event) => {
        const x = at(event);
        let [nearest, best] = [-1, 4];
        c.item().stops.forEach(([position], i) => {
          if (Math.abs(position - x) <= best) [nearest, best] = [i, Math.abs(position - x)];
        });
        if (nearest < 0) return;
        picked = nearest;
        c.refresh();
        slide(event);
      };
      bar.press = (event) => {
        const item = c.item();
        if (item.stops.length >= 12) return;
        const x = at(event);
        const value = `#${colorAt(item.stops, x, item.method).map((v) => Math.round(clamp(v, 0, 1) * 255).toString(16).padStart(2, "0")).join("")}`;
        c.update((each) => {
          const stop = [x, value];
          each.stops.push(stop);
          each.stops.sort((a, b) => a[0] - b[0]);
          picked = each.stops.indexOf(stop);
          each.preset = "custom";
        }, false);
        slide(event);
      };
      const color = element("input", { type: "color" });
      color.setAttribute("aria-label", "Stop color");
      color.addEventListener("input", () =>
        c.update((item) => {
          if (!item.stops[picked]) return;
          item.stops[picked][1] = color.value;
          item.preset = "custom";
        }, true),
      );
      const where = element("i");
      const stopRow = element("div", { className: "row" }, element("span", { className: "label" }, "Stop", where), color);
      const method = segmented("Method", METHODS, () => c.item().method, (id) => c.update((item) => (item.method = id), true));
      const reverse = c.check("Reverse", "reverse");
      const dither = c.check("Dither", "dither");
      const show = () => {
        const item = c.item();
        picked = clamp(picked, 0, item.stops.length - 1);
        const css = gradientCss(item.stops, item.method);
        bar.style.background = css;
        presetDot.style.background = css;
        presetText.textContent = GRADIENTS.find(([id]) => id === item.preset)?.[1] ?? "Custom";
        marks.replaceChildren(
          ...item.stops.map(([position, value], i) => {
            const mark = element("div", { className: `gstop${i === picked ? " on" : ""}` });
            Object.assign(mark.style, { left: `${position}%`, background: value });
            return mark;
          }),
        );
        color.value = item.stops[picked][1];
        where.textContent = `${item.stops[picked][0]}%`;
      };
      return { nodes: [preset, bar, marks, stopRow, method, reverse, dither], parts: [method, reverse, dither], show };
    },

    selective: (c) => {
      let range = "reds";
      const pick = chips("Colors", SELECTIVE, () => range, (id) => ((range = id), c.refresh()), (id) => c.item()[id].some(Boolean));
      const caption = element("span");
      const head = element("div", { className: "caption" }, element("b", { textContent: "Colors" }), caption);
      const rows = ["Cyan", "Magenta", "Yellow", "Black"].map((name, i) => c.slider(name, { min: -100, max: 100 }, () => [range, i]));
      const method = segmented("Method", [[true, "Relative"], [false, "Absolute"]], () => c.item().relative, (value) => c.update((item) => (item.relative = value), true));
      const show = () => (caption.textContent = SELECTIVE.find(([id]) => id === range)[1]);
      return { nodes: [head, pick, ...rows, method], parts: [pick, ...rows, method], show };
    },

    vignette: (c) => {
      const kind = select("Style", STYLES, () => c.item().style, (id) => c.update((item) => (item.style = id), true));
      const rows = [
        c.slider("Amount", { min: -100, max: 100, def: -25 }, "amount"),
        c.slider("Midpoint", { min: 0, max: 100, def: 50 }, "midpoint"),
        c.slider("Roundness", { min: -100, max: 100 }, "roundness"),
        c.slider("Feather", { min: 0, max: 100, def: 50 }, "feather"),
      ];
      const highlights = c.slider("Highlights", { min: 0, max: 100 }, "highlights");
      const show = () => (highlights.hidden = c.item().style !== "highlight" || c.item().amount >= 0);
      return { nodes: [kind, ...rows, highlights], parts: [kind, ...rows, highlights], show };
    },
  };

  // ---------- The list ----------

  // Which adjustment's controls show, one at a time, kept for every picker
  let expanded = null;
  let ui = null;
  const busy = (edit) => {
    if (ui?.edit === edit) ui.section.classList.toggle("busy", !!edit.loading);
  };
  const MENU = () => {
    const s = ui.settings();
    const entries = [];
    let group = 0;
    for (const { id, name, hint, group: each } of CATALOG) {
      if (each !== group) {
        entries.push("-");
        group = each;
      }
      entries.push({ id, label: name, hint, disabled: !!s.items[id], checked: !!s.items[id] });
    }
    return entries;
  };

  const build = (shown) => {
    const section = element("div");
    section.dataset.ligmaColor = "section";
    const add = element("button", { type: "button", className: "icon add", title: "Add an adjustment" }, svg(ICONS.plus));
    add.setAttribute("aria-haspopup", "menu");
    add.setAttribute("aria-label", "Add an adjustment");
    const head = element("div", { className: "head", title: "Ligma's own adjustments, not Figma's" }, element("span", { className: "title", textContent: "Adjustments" }), element("span", { className: "tag", textContent: "Ligma" }), add);
    const list = element("div");
    list.setAttribute("role", "list");
    const note = element("p", { className: "note", textContent: "Made by Ligma as a new image, with the original kept hidden below it" });
    section.append(head, list, note);

    // Over the picker's thumbnail, the preview until the file has it
    const overlay = element("canvas");
    overlay.dataset.ligmaColor = "preview";
    overlay.hidden = true;
    const thumbnail = () => section.closest(PICKER)?.querySelector('img[class*="imagePreview"]') ?? null;
    let drawnOnce = false;
    const picture = (bitmap) => {
      if (!bitmap) {
        drawnOnce = false;
        return place();
      }
      const scale = Math.min(1, 520 / Math.max(bitmap.width, bitmap.height));
      const [width, height] = [Math.max(1, Math.round(bitmap.width * scale)), Math.max(1, Math.round(bitmap.height * scale))];
      if (overlay.width !== width || overlay.height !== height) Object.assign(overlay, { width, height });
      const context = overlay.getContext("2d");
      context.clearRect(0, 0, width, height);
      context.drawImage(bitmap, 0, 0, width, height);
      drawnOnce = true;
      place();
    };
    // Where the thumbnail is, and whether the preview's still needed: until
    // the file has the settings and Figma's drawn its thumbnail again, or
    // a moment after
    const place = () => {
      const img = thumbnail();
      const edit = ui.edit;
      const waiting = !!edit && !edit.failed && (edit.version !== edit.fullVersion || edit.committing || (edit.landedSrc !== undefined && img?.src === edit.landedSrc && performance.now() < edit.landed + 1500));
      overlay.hidden = !img || !drawnOnce || !waiting;
      if (overlay.hidden) return;
      if (overlay.previousElementSibling !== img) img.after(overlay);
      const holder = img.parentElement;
      if (getComputedStyle(holder).position === "static") holder.style.position = "relative";
      Object.assign(overlay.style, { left: `${img.offsetLeft}px`, top: `${img.offsetTop}px`, width: `${img.offsetWidth}px`, height: `${img.offsetHeight}px` });
    };

    const settings = () => ui.edit?.settings ?? ui.shown;
    // Changes to item `id`, as one edit: `fn(item)` changes it, `done` once
    // a drag's let go or for a click
    const updateItem = (id, fn, done) => {
      const edit = ui.edit ?? (ui.edit = begin());
      if (!edit) return;
      const item = edit.settings.items[id];
      if (!item) return;
      const before = JSON.stringify(item);
      fn(item);
      edit.holding = !done;
      if (JSON.stringify(item) !== before) change(edit);
      else if (done) later(edit, 150);
      show();
    };
    // `fn(settings)` changes the list
    const changeList = (fn) => {
      const edit = ui.edit ?? (ui.edit = begin());
      if (!edit) return;
      fn(edit.settings);
      edit.settings.order = orderOf(edit.settings);
      edit.holding = false;
      change(edit);
      render();
    };
    // A new one goes on top, applied last, as in Photoshop's Layers
    add.addEventListener("click", () =>
      openMenu(add, MENU(), (id) => {
        expanded = id;
        changeList((s) => {
          s.items[id] = { on: true, ...fresh(id) };
          s.order = [...orderOf(s).filter((each) => each !== id), id];
        });
      }),
    );
    // Pasted, `{ items, order }`: each replaces its own or is added, on top
    // in the order it was copied in
    const pasteItems = (copied) =>
      changeList((s) => {
        Object.assign(s, withPasted(s, copied));
        picked.clear();
        for (const id of copied.order) picked.add(id);
      });
    // Rows picked, for Copy: a press picks one, Shift adds or takes one away
    const picked = new Set();

    // Each item's row, and its editor while it's the one expanded
    const rows = new Map();
    let editor = null;
    const editorFor = (id) => {
      const make = EDITORS[id];
      if (!make) return null;
      const item = () => settings().items[id];
      const c = {
        item,
        update: (fn, done) => updateItem(id, fn, done),
        refresh: () => show(),
        histogram: () => {
          const edit = ui.edit;
          if (edit?.info) return edit.info.histogram;
          const figma = api();
          const at = openPaint();
          const target = figma && at && targetsOf(figma, at)[0];
          if (!target) return null;
          const hash = originalOf(target.node, target.hash);
          // Asked for once: `show` asks again, so an answer that showed it
          // every time would never stop
          if (!ui.histograms.has(hash)) {
            ui.histograms.set(hash, null);
            ready(figma, hash)
              .then((info) => {
                ui.histograms.set(hash, info.histogram);
                show();
              })
              .catch(() => ui.histograms.delete(hash));
          }
          return ui.histograms.get(hash);
        },
        // `path` a key, a [key, key] pair, or a function giving one
        slider: (label, o, path) => {
          const where = () => [].concat(typeof path === "function" ? path() : path);
          const read = () => where().reduce((value, key) => value[key], item());
          return slider(label, o, read, (value, done) =>
            updateItem(id, (each) => {
              const keys = where();
              const last = keys.pop();
              keys.reduce((value, key) => value[key], each)[last] = value;
            }, done),
          );
        },
        check: (label, key) => checkbox(label, () => item()[key], (value) => updateItem(id, (each) => (each[key] = value), true)),
      };
      const made = make(c);
      const holder = element("div", { className: "editor" }, ...made.nodes);
      holder.setAttribute("role", "group");
      holder.setAttribute("aria-label", NAMES[id]);
      holder.show = () => {
        for (const part of made.parts ?? []) part.show?.();
        made.show?.();
      };
      holder.id = id;
      return holder;
    };
    const render = () => {
      const s = settings();
      const ids = [...orderOf(s)].reverse();
      for (const id of picked) if (!s.items[id]) picked.delete(id);
      if (expanded && !ids.includes(expanded)) expanded = null;
      for (const [id, row] of rows) if (!ids.includes(id)) (row.remove(), rows.delete(id));
      if (editor && editor.id !== expanded) {
        editor.remove();
        editor = null;
      }
      if (expanded && !editor) editor = editorFor(expanded);
      let at = 0;
      ids.forEach((id) => {
        let row = rows.get(id);
        if (!row) {
          const flat = !EDITORS[id];
          const name = element("button", { type: "button", className: `name${flat ? " flat" : ""}` }, flat ? null : svg(ICONS.chevron), element("span", { textContent: NAMES[id] }));
          name.press = (event) => lift(event, id, name);
          name.addEventListener("click", (event) => {
            if (performance.now() - (name.dropped ?? -Infinity) < 400 || event.shiftKey || flat) return;
            expanded = expanded === id ? null : id;
            render();
          });
          // Alt+Up and Alt+Down move it in the list
          name.key = (event) => {
            const by = event.altKey ? { ArrowUp: -1, ArrowDown: 1 }[event.key] : undefined;
            if (by === undefined) return false;
            const shown = [...orderOf(settings())].reverse();
            const at = shown.indexOf(id);
            if (at + by < 0 || at + by >= shown.length) return true;
            shown.splice(at, 1);
            shown.splice(at + by, 0, id);
            changeList((s) => (s.order = shown.reverse()));
            name.focus({ preventScroll: true });
            return true;
          };
          // On the press, not the click, so it answers at once and isn't
          // lost when Figma redraws under the pointer. Keys still click
          const pressed = (button, act) => {
            button.press = () => ((button.pressed = performance.now()), act());
            button.addEventListener("click", (event) => {
              if (event.detail && performance.now() - (button.pressed ?? -Infinity) < 1000) return;
              act();
            });
          };
          const eye = element("button", { type: "button", className: "icon" });
          pressed(eye, () => changeList((s) => (s.items[id].on = !s.items[id].on)));
          const remove = element("button", { type: "button", className: "icon", title: `Remove ${NAMES[id]}` }, svg(ICONS.minus));
          remove.setAttribute("aria-label", `Remove ${NAMES[id]}`);
          pressed(remove, () => changeList((s) => delete s.items[id]));
          row = element("div", { className: "item" }, name, eye, remove);
          row.setAttribute("role", "listitem");
          row.parts = { name, eye, flat };
          rows.set(id, row);
        }
        // Moved only when out of place, as moving one under a press loses it
        for (const node of id === expanded && editor ? [row, editor] : [row]) {
          if (list.children[at] !== node) list.insertBefore(node, list.children[at] ?? null);
          at++;
        }
      });
      show();
    };
    // A press on a row's name picks it, and dragging it up or down moves it
    // in the list, a line showing where it'll go
    const line = element("div", { className: "drop", hidden: true });
    list.append(line);
    const lift = (event, id, handle) => {
      if (event.button !== 0) return;
      handle.focus({ preventScroll: true });
      if (!event.shiftKey) picked.clear();
      if (event.shiftKey && picked.has(id)) picked.delete(id);
      else picked.add(id);
      show();
      const row = rows.get(id);
      const y = event.clientY;
      let moving = false;
      let to = null;
      drag(
        event,
        (event) => {
          const dy = event.clientY - y;
          if (!moving && Math.abs(dy) < 4) return;
          moving = true;
          row.classList.add("lifted");
          row.style.transform = `translateY(${dy}px)`;
          const others = [...orderOf(settings())].reverse().filter((each) => each !== id);
          let at = others.findIndex((each) => {
            const box = rows.get(each).getBoundingClientRect();
            return event.clientY < box.top + box.height / 2;
          });
          if (at < 0) at = others.length;
          to = { others, at };
          const below = rows.get(others[at]);
          const above = others[at - 1] && (others[at - 1] === expanded && editor ? editor : rows.get(others[at - 1]));
          const edge = below ? below.getBoundingClientRect().top : above ? above.getBoundingClientRect().bottom : row.getBoundingClientRect().top;
          line.style.top = `${edge - list.getBoundingClientRect().top - 1}px`;
          line.hidden = false;
          if (line !== list.lastElementChild) list.append(line);
        },
        () => {
          row.classList.remove("lifted");
          row.style.transform = "";
          line.hidden = true;
          if (!moving || !to) return;
          handle.dropped = performance.now();
          const shown = [...to.others];
          shown.splice(to.at, 0, id);
          const order = shown.reverse();
          if (order.join() !== orderOf(settings()).join()) changeList((s) => (s.order = order));
          // Moving it took its focus, which Copy needs
          handle.focus({ preventScroll: true });
        },
      );
    };
    const show = () => {
      const s = settings();
      for (const [id, row] of rows) {
        const on = s.items[id]?.on !== false;
        row.classList.toggle("off", !on);
        row.classList.toggle("picked", picked.has(id));
        const { name, eye, flat } = row.parts;
        if (!flat) name.setAttribute("aria-expanded", String(id === expanded));
        // Only when it changes: this runs as Figma redraws, and a press
        // whose icon was swapped under it before it's let go isn't a click
        if (row.on !== on) {
          row.on = on;
          eye.replaceChildren(svg(on ? ICONS.eye : ICONS.hidden));
          eye.title = on ? `Hide ${NAMES[id]}` : `Show ${NAMES[id]}`;
          eye.setAttribute("aria-label", eye.title);
        }
      }
      note.hidden = !added(s);
      editor?.show();
      place();
    };
    return { section, overlay, render, show, picture, thumbnail, settings, pasteItems, picked, histograms: new Map(), edit: null, shown };
  };

  // The worker started and the original sent to it ahead of the first
  // change
  const warm = () => {
    const figma = api();
    const at = openPaint();
    const target = figma && at && targetsOf(figma, at)[0];
    if (!target) return;
    setTimeout(() => ready(figma, originalOf(target.node, target.hash)).catch(() => {}), 200);
  };

  const decorate = () => {
    const picker = document.querySelector(PICKER);
    const figma = state.on && api();
    const onImage = !!figma && !!picker?.querySelector('fieldset[value="IMAGE"]');
    const box = !onImage ? null : ui?.box?.isConnected && picker.contains(ui.box) ? ui.box : figmaSliders(picker);
    const at = box ? openPaint() : null;
    const nowKey = at ? key(figma, at) : null;
    // The picker closed, or moved to another paint or layer
    if (ui && (!at || ui.key !== nowKey)) {
      if (ui.edit) finish(ui.edit);
      ui.section.remove();
      ui.overlay.remove();
      closeMenu();
      ui = null;
    }
    if (!at) {
      for (const node of pending) {
        pending.delete(node);
        settle(node);
      }
      return;
    }
    if (!style.isConnected) document.head.append(style);
    if (!ui) {
      const target = targetsOf(figma, at)[0];
      ui = build(target ? settingsOf(target.node, paintsOf(target.node, at.list)[at.index]) : blank());
      ui.key = nowKey;
      ui.hash = target?.hash;
      ui.render();
      warm();
    } else {
      // The image changed under the open picker, by someone else in the
      // file or by undo: the section shows what it has now, as anyone
      // with Ligma sees what was left on it
      const target = targetsOf(figma, at)[0];
      const edit = ui.edit && !ui.edit.ended ? ui.edit : null;
      const busy = edit && (edit.holding || edit.committing || edit.living || dragging);
      if (target && target.hash !== (edit ? edit.targets[0].hash : ui.hash) && !busy) {
        if (edit) abandon(edit);
        ui.edit = null;
        ui.hash = target.hash;
        ui.shown = settingsOf(target.node, paintsOf(target.node, at.list)[at.index]);
        ui.render();
      }
    }
    const moved = box.nextElementSibling !== ui.section;
    if (moved) box.after(ui.section);
    if (moved || ui.box !== box) {
      ui.box = box;
      const sizes = measure(box, ui.section);
      for (const name of ["left", "column", "right"]) ui.section.style.setProperty(`--${name}`, sizes ? `${sizes[name]}px` : "");
      ui.render();
    }
    ui.show();
  };

  // Presses in the section and its menus, their keys and drags go to them
  // alone, ahead of Figma, which would otherwise drag its picker by them,
  // nudge the selection with arrows, close the picker on Escape, or do its
  // own work for every move. Registered before Figma's page code, so first
  const inside = (event) => event.target instanceof Element && !!event.target.closest("[data-ligma-color]");
  const only = (event) => event.stopImmediatePropagation();
  addEventListener(
    "pointerdown",
    (event) => {
      if (menu && !(event.target instanceof Element && menu.holder.contains(event.target))) closeMenu();
      if (!inside(event)) return;
      if (event.button === 0) {
        for (let node = event.target; node instanceof Element && !node.matches?.("[data-ligma-color]"); node = node.parentElement) {
          if (node.press) {
            node.press(event);
            break;
          }
        }
      }
      only(event);
    },
    true,
  );
  // Figma cancels wheels it doesn't own, so a long menu wouldn't scroll
  addEventListener(
    "wheel",
    (event) => {
      if (event.target instanceof Element && event.target.closest('[data-ligma-color="menu"]')) only(event);
    },
    { capture: true, passive: true },
  );
  for (const type of ["mousedown", "mouseup", "pointerup", "pointercancel"]) {
    addEventListener(
      type,
      (event) => {
        if (!dragging && !inside(event)) return;
        if (type === "pointerup" || type === "pointercancel") dragging?.end();
        only(event);
      },
      true,
    );
  }
  for (const type of ["pointermove", "mousemove"]) {
    addEventListener(
      type,
      (event) => {
        if (!dragging) return;
        if (type === "pointermove") dragging.move(event);
        only(event);
      },
      true,
    );
  }
  addEventListener(
    "keydown",
    (event) => {
      if (!inside(event)) return;
      const target = menu?.holder.contains(event.target) ? menu.holder : event.target;
      const handled = target.key?.(event);
      if (handled) event.preventDefault();
      // Typing in a value, and Copy and Paste here, aren't Figma's: the
      // browser's own copy and paste follow, which Ligma takes below
      const mod = /Mac/.test(navigator.platform) ? event.metaKey : event.ctrlKey;
      const clipboard = mod && ["c", "v", "x"].includes(event.key.toLowerCase());
      if (handled || event.key.startsWith("Arrow") || event.target instanceof HTMLInputElement || clipboard) only(event);
    },
    true,
  );

  // Copy with the section focused copies the rows picked, or the one whose
  // controls are focused, or all of them. Paste puts them on the image
  // whose picker is open, or on the top image of each selected layer.
  // They go through the clipboard as Ligma's own type, so they carry to
  // every tab, and as text naming them
  const CLIP = "application/x-ligma-adjustments";
  const typing = (node) => (node instanceof HTMLInputElement && !["range", "checkbox", "radio", "button", "color", "file"].includes(node.type)) || node instanceof HTMLTextAreaElement || !!node?.isContentEditable;
  addEventListener(
    "copy",
    (event) => {
      const focus = document.activeElement;
      if (!state.on || !ui || !ui.section.contains(focus) || typing(focus)) return;
      const s = ui.settings();
      let ids = orderOf(s).filter((id) => ui.picked.has(id));
      if (!ids.length) {
        const own = focus.closest(".editor")?.id;
        ids = own && s.items[own] ? [own] : orderOf(s);
      }
      if (!ids.length) return;
      const copied = { v: 2, items: Object.fromEntries(ids.map((id) => [id, s.items[id]])), order: ids };
      event.clipboardData.setData(CLIP, JSON.stringify(copied));
      event.clipboardData.setData("text/plain", `Ligma adjustments: ${ids.map((id) => NAMES[id]).join(", ")}`);
      event.preventDefault();
      only(event);
      api()?.notify(ids.length === 1 ? `Copied ${NAMES[ids[0]]}` : `Copied ${ids.length} adjustments`, { timeout: 1500 });
    },
    true,
  );
  addEventListener(
    "paste",
    (event) => {
      const figma = state.on && api();
      const raw = figma && event.clipboardData?.getData(CLIP);
      if (!raw) return;
      // Into a field shown, as Figma's width, it's text
      const focus = document.activeElement;
      if (typing(focus) && (ui?.section.contains(focus) || focus.getBoundingClientRect().width > 4)) return;
      let copied;
      try {
        copied = normal(JSON.parse(raw));
      } catch {
        return;
      }
      if (!copied.order.length) return;
      if (ui) {
        event.preventDefault();
        only(event);
        ui.pasteItems(copied);
        return;
      }
      // Each selected layer's top image fill shown, or its stroke's
      const targets = [];
      for (const node of figma.currentPage.selection) {
        for (const list of ["fills", "strokes"]) {
          const paints = paintsOf(node, list);
          const index = paints.findLastIndex((paint) => image(paint) && paint.visible !== false);
          if (index >= 0) {
            targets.push({ node, list, index });
            break;
          }
        }
      }
      // Nothing to paste them on: Figma pastes the text
      if (!targets.length) return;
      event.preventDefault();
      only(event);
      (async () => {
        try {
          figma.commitUndo();
        } catch {}
        let done = 0;
        for (const { node, list, index } of targets) {
          try {
            const paint = paintsOf(node, list)[index];
            await apply(node, list, index, withPasted(settingsOf(node, paint), copied));
            done++;
          } catch {}
        }
        try {
          figma.commitUndo();
        } catch {}
        const what = copied.order.length === 1 ? NAMES[copied.order[0]] : "Adjustments";
        figma.notify(done ? `${what} pasted to ${done === 1 ? "1 image" : `${done} images`}` : "Couldn't adjust the image", { timeout: 2000 });
      })();
    },
    true,
  );

  let queued = false;
  new MutationObserver((records) => {
    if (queued || records.every((record) => record.target.closest?.("[data-ligma-color]"))) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      try {
        decorate();
      } catch {}
    });
  }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: ["class", "src"] });

  window.__ligmaColor = {
    // For adjustments.rs: what a paint has, and putting it on another
    settingsOf: (node, paint) => (state.on ? settingsOf(node, paint) : null),
    apply: (node, list, index, settings) => (state.on ? apply(node, list, index, settings) : Promise.resolve()),
    set: (next) => {
      try {
        localStorage.setItem(KEY, JSON.stringify(next));
      } catch {}
      state = next;
      decorate();
    },
  };
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        decorate();
      }
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
})()
