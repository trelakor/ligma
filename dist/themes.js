// Ligma's color themes: Figma's own palette, tinted. A theme recolors the
// neutral greys toward its hue and moves Figma's blue (brand, selection,
// focus) to its accent hue, keeping each step's lightness, so contrast stays
// as Figma made it. Used by the settings window for its previews, and by
// Figma tabs, which get it through src-tauri/src/appearance.rs.

const LIGMA_THEMES = [
  { id: "figma", name: "Figma" },
  { id: "grove", name: "Grove", hue: 160, accent: 155 },
  { id: "ocean", name: "Ocean", hue: 230, accent: 215 },
  { id: "ember", name: "Ember", hue: 50, accent: 45 },
  { id: "iris", name: "Iris", hue: 290, accent: 293 },
  { id: "rose", name: "Rose", hue: 355, accent: 0 },
];

// Figma's ramps (UI3), steps 100 to 1000. Blue differs between light and dark.
const LIGMA_STEPS = [100, 200, 300, 400, 500, 600, 700, 800, 900, 1000];
const LIGMA_GREY = ["#f5f5f5", "#e6e6e6", "#d9d9d9", "#b3b3b3", "#757575", "#444444", "#383838", "#2c2c2c", "#1e1e1e", "#111111"];
const LIGMA_PALE_BLUE = ["#f1f5f8", "#e3ecf2", "#d2dae4", "#afbccf", "#667799", "#536383", "#4a5878", "#394360", "#282e3e", "#14181f"];
const LIGMA_BLUE = {
  light: ["#f2f9ff", "#e5f4ff", "#bde3ff", "#80caff", "#0d99ff", "#007be5", "#0768cf", "#034ac1", "#093077", "#0d193f"],
  dark: ["#e2f1fd", "#cfe9fc", "#a8d7fa", "#7cc4f8", "#0c8ce9", "#0a6dc2", "#105cad", "#184591", "#1b335f", "#161e36"],
};
// How far the greys lean toward the theme's hue (OKLCH chroma, at mid grey).
const LIGMA_TINT = { light: 0.018, dark: 0.024 };

const ligmaOklch = (hex) => {
  const lin = (x) => (x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4);
  const [r, g, b] = [1, 3, 5].map((i) => lin(parseInt(hex.slice(i, i + 2), 16) / 255));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s, Math.hypot(a, bb)];
};

// `#rrggbb` for an OKLCH color, its chroma lowered until it fits in sRGB.
const ligmaHex = (L, C, H) => {
  const rgb = (c) => {
    const a = c * Math.cos((H * Math.PI) / 180);
    const b = c * Math.sin((H * Math.PI) / 180);
    const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
    const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
    const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
    return [
      4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
      -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
      -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ];
  };
  let channels = rgb(C);
  for (let i = 0; i < 40 && channels.some((x) => x < 0 || x > 1); i++) channels = rgb((C *= 0.9));
  const gamma = (x) => (x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055);
  return "#" + channels.map((x) => Math.round(gamma(Math.min(1, Math.max(0, x))) * 255).toString(16).padStart(2, "0")).join("");
};

/** The CSS variables theme `id` sets in `mode` ("light" or "dark"); none for Figma's own. */
function ligmaPalette(id, mode) {
  const theme = LIGMA_THEMES.find((theme) => theme.id === id);
  if (!theme?.hue) return {};
  const vars = {};
  // Most tint mid-ramp, little at the near-white and near-black ends.
  const tint = (L) => LIGMA_TINT[mode] * Math.sqrt(Math.sin(Math.PI * L));
  LIGMA_STEPS.forEach((step, i) => {
    const [L] = ligmaOklch(LIGMA_GREY[i]);
    vars[`--ramp-grey-${step}`] = ligmaHex(L, tint(L), theme.hue);
    for (const [name, ramp] of [["blue", LIGMA_BLUE[mode]], ["pale-blue", LIGMA_PALE_BLUE]]) {
      const [L2, C2] = ligmaOklch(ramp[i]);
      vars[`--ramp-${name}-${step}`] = ligmaHex(L2, C2, theme.accent);
    }
  });
  // The light theme's background is white; give it a breath of the hue.
  // White stays white in dark mode, where it's the text.
  if (mode === "light") vars["--ramp-white-1000"] = ligmaHex(0.993, LIGMA_TINT.light * 0.3, theme.hue);
  return vars;
}

/** A few of the colors theme `id` gives Figma in `mode`, for previews. */
function ligmaSwatch(id, mode) {
  const vars = ligmaPalette(id, mode);
  const grey = (step) => vars[`--ramp-grey-${step}`] ?? LIGMA_GREY[LIGMA_STEPS.indexOf(step)];
  const blue = (step) => vars[`--ramp-blue-${step}`] ?? LIGMA_BLUE[mode][LIGMA_STEPS.indexOf(step)];
  return mode === "light"
    ? { bg: vars["--ramp-white-1000"] ?? "#ffffff", panel: grey(100), line: grey(200), muted: grey(300), accent: blue(500), soft: blue(300) }
    : { bg: grey(800), panel: grey(900), line: grey(600), muted: grey(600), accent: blue(500), soft: blue(700) };
}
