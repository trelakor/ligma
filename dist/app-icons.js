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
  { id: "seasonal", name: "Seasonal" },
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
// A squircle ring `width` thick, filled with `fill-rule="evenodd"`: WebKit
// leaves hairline seams in a stroked squircle
const ligmaFrame = (width, radius) => `${ligmaSquircle(radius + width / 2)}${ligmaSquircle(radius - width / 2)}`;

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
  // `count` copies of `draw(x, y, random)` spread over the tile, away from
  // its edges
  scatter: (seed, count, draw, [x0, y0, x1, y1] = [110, 110, 914, 914]) => {
    const random = ligmaSvg.rand(seed);
    let out = "";
    for (let i = 0; i < count; i++) out += draw(x0 + random() * (x1 - x0), y0 + random() * (y1 - y0), random, i);
    return out;
  },
  // A heart `size` wide around `x`, `y`
  heart: (x, y, size) => {
    const s = size / 2;
    const p = (px, py) => `${(x + px * s).toFixed(1)} ${(y + py * s).toFixed(1)}`;
    return `M${p(0, 0.95)}C${p(-0.25, 0.72)} ${p(-1, 0.22)} ${p(-1, -0.3)}C${p(-1, -0.78)} ${p(-0.56, -1.02)} ${p(-0.26, -0.96)}C${p(-0.1, -0.93)} ${p(0, -0.8)} ${p(0, -0.64)}`
      + `C${p(0, -0.8)} ${p(0.1, -0.93)} ${p(0.26, -0.96)}C${p(0.56, -1.02)} ${p(1, -0.78)} ${p(1, -0.3)}C${p(1, 0.22)} ${p(0.25, 0.72)} ${p(0, 0.95)}Z`;
  },
  // A leaf `size` long around `x`, `y`, pointing up before `turn`
  leaf: (x, y, size, turn, fill) => {
    const s = size / 2;
    return `<g transform="translate(${x.toFixed(1)} ${y.toFixed(1)}) rotate(${turn.toFixed(0)})"><path d="M0 ${-s}Q${s * 1.15} ${-s * 0.15} 0 ${s}Q${-s * 1.15} ${-s * 0.15} 0 ${-s}Z" fill="${fill}"/><path d="M0 ${-s * 0.8}V${s * 1.25}" stroke="#000" stroke-opacity=".22" stroke-width="${(size / 22).toFixed(1)}" stroke-linecap="round"/></g>`;
  },
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

// A filter that lights shapes as raised, polished metal or foil: their
// edges domed by blurring them `blur` deep, shaded and given a specular
// highlight from the top left. `texture` streaks the face with a brushed
// grain that strong, a turbulence `frequency`
const ligmaShine = (id, { blur = 11, scale = 7, shine = 1.3, sharpness = 26, light = "#fff6dc", texture = 0, frequency = "0.004 0.7" } = {}) =>
  `<filter id="${id}" x="-5%" y="-5%" width="110%" height="110%" color-interpolation-filters="sRGB">`
  + `<feGaussianBlur in="SourceAlpha" stdDeviation="${blur}" result="dome"/>`
  + `<feDiffuseLighting in="dome" surfaceScale="${scale}" diffuseConstant="1.2" lighting-color="#fff" result="diffuse"><feDistantLight azimuth="225" elevation="48"/></feDiffuseLighting>`
  + `<feSpecularLighting in="dome" surfaceScale="${scale}" specularConstant="${shine}" specularExponent="${sharpness}" lighting-color="${light}" result="specular"><feDistantLight azimuth="225" elevation="36"/></feSpecularLighting>`
  + `<feComposite in="SourceGraphic" in2="diffuse" operator="arithmetic" k1="1.1" result="lit"/>`
  + (texture
    ? `<feTurbulence type="fractalNoise" baseFrequency="${frequency}" numOctaves="3" seed="12"/><feColorMatrix type="matrix" values="1 0 0 0 0  1 0 0 0 0  1 0 0 0 0  0 0 0 0 1" result="grain"/>`
      + `<feComposite in="lit" in2="grain" operator="arithmetic" k1="${texture * 2}" k2="${1 - texture}" result="lit"/>`
    : "")
  + `<feComposite in="specular" in2="lit" operator="arithmetic" k2="1" k3="1" result="shone"/>`
  + `<feComposite in="shone" in2="SourceAlpha" operator="in"/></filter>`;

// The gold of Gilded and Fireworks, `id` its fill: polished, with the
// bright and dark bands of what it reflects, and a brushed grain
const ligmaGold = (id, texture = 0.4, g = LIGMA_G) => {
  const { linear } = ligmaSvg;
  return linear(id, [[0, "#fff6cf"], [0.16, "#f6d06a"], [0.34, "#b7801f"], [0.46, "#fbe39a"], [0.58, "#e2b148"], [0.78, "#8c5a12"], [0.92, "#c9932f"], [1, "#f4d27a"]], [g.x0, g.y0, g.right, g.bottom], true)
    + ligmaShine(`${id}-shine`, { texture });
};
// The glyph in ligmaGold, `id` as given to it
const ligmaGilt = (id, g = LIGMA_G) => `<g filter="url(#${id}-shine)">${ligmaSvg.parts(Array(4).fill(`url(#${id})`), "", g)}</g>`;

// A dark tile fading down from black to `bottom`, each part in its own
// gradient: `gradients[i]` is its stops and its [x1, y1, x2, y2] on the grid
const ligmaGradientParts = (bottom, gradients) => () => {
  const { linear, fill, parts } = ligmaSvg;
  return {
    defs: linear("tile-fill", [[0, "#000"], [1, bottom]], [512, 100, 512, 924], true)
      + gradients.map(([stops, axis], i) => linear(`part-fill${i}`, stops, axis, true)).join(""),
    body: fill("url(#tile-fill)") + parts(gradients.map((_, i) => `url(#part-fill${i})`)),
  };
};
// One gradient on every part, running down the blocks and across the dot
const ligmaWave = (stops) => [[395.783, 220.663, 395.783, 417.967], [326.5, 459.5, 523, 599.5], [492, 808.5, 527.5, 635], [715.5, 709.5, 512, 722]].map((axis) => [stops, axis]);
// Flat `colors` on a flat `tile`
const ligmaFlat = (tile, colors) => () => ({ body: ligmaSvg.fill(tile) + ligmaSvg.parts(colors) });

// Scribe's pen lines, one inside each part
const LIGMA_SCRIBE_INK = [
  "M415.5 211.625C415.5 218.875 415.5 226.125 415.5 233.375C425.083 234.581 434.245 237.347 442.567 241.404C453.728 246.835 463.348 254.585 470.75 263.75C483.745 279.745 489.695 299.995 488.129 319C486.214 351.167 484.835 383.333 483.993 415.5L512 387.493C486.737 386.831 461.474 386.5 436.211 386.5C429.307 386.5 422.404 386.525 415.5 386.574C397.802 386.804 380.006 380.112 366.632 367.868C353.206 355.709 344.331 338.047 342.871 319C341.305 299.995 347.255 279.745 360.25 263.75C367.652 254.585 377.272 246.835 388.433 241.404C396.755 237.347 405.917 234.581 415.5 233.375C415.5 226.125 415.5 218.875 415.5 211.625C403.488 210.412 391.065 211.303 379.001 214.364C362.831 218.446 347.327 226.463 334.278 237.778C311.375 257.425 296.513 287.419 295.129 319C293.487 350.54 305.424 383.121 327.896 406.604C350.164 430.258 382.612 444.378 415.5 444.426C422.404 444.475 429.307 444.5 436.211 444.5C461.474 444.5 486.737 444.169 512 443.507C527.838 443.093 540.422 431.338 540.007 415.5C539.165 383.333 537.786 351.167 535.871 319C534.487 287.419 519.625 257.425 496.722 237.778C483.673 226.463 468.169 218.446 451.999 214.364C439.935 211.303 427.512 210.412 415.5 211.625ZM415.5 233.375V211.625V233.375V233.375Z",
  "M415.5 404.625C415.5 411.875 415.5 419.125 415.5 426.375C426.986 427.583 438.472 428.731 449.958 429.819C470.639 431.777 491.319 433.539 512 435.105L492.395 415.5C487.522 479.833 484.545 544.167 483.463 608.5L512 579.963C493.641 579.654 475.281 579.5 456.922 579.5C443.114 579.5 429.307 579.587 415.5 579.762C397.753 580.133 379.795 573.507 366.287 561.213C352.725 549.009 343.756 531.194 342.314 512C340.762 492.848 346.836 472.472 359.972 456.472C367.096 447.739 376.23 440.324 386.774 435.003C395.558 430.562 405.304 427.581 415.5 426.375C415.5 419.125 415.5 411.875 415.5 404.625C402.72 403.411 389.502 404.574 376.764 408.172C361.482 412.468 346.907 420.288 334.557 431.057C311.794 450.698 297.056 480.566 295.686 512C294.062 543.393 305.905 575.822 328.241 599.259C350.375 622.863 382.66 637.049 415.5 637.238C429.307 637.413 443.114 637.5 456.922 637.5C475.281 637.5 493.641 637.346 512 637.037C528.141 636.766 540.808 624.641 540.537 608.5C539.455 544.167 536.478 479.833 531.605 415.5C530.768 404.441 523.059 395.057 512 395.895C491.319 397.461 470.639 399.223 449.958 401.181C438.472 402.269 426.986 403.417 415.5 404.625ZM415.5 426.375V404.625V426.375V426.375Z",
  "M415.5 597.625C415.5 604.875 415.5 612.125 415.5 619.375C426.986 620.583 438.472 621.731 449.958 622.819C470.639 624.777 491.319 626.539 512 628.105L492.395 608.5C487.522 672.833 484.545 737.167 483.463 801.5L512 772.963C493.641 772.654 475.281 772.5 456.922 772.5C443.114 772.5 429.307 772.587 415.5 772.762C397.753 773.133 379.795 766.507 366.287 754.213C352.725 742.009 343.756 724.194 342.314 705C340.762 685.848 346.836 665.472 359.972 649.472C367.096 640.739 376.23 633.324 386.774 628.003C395.558 623.562 405.304 620.581 415.5 619.375C415.5 612.125 415.5 604.875 415.5 597.625C402.72 596.411 389.502 597.574 376.764 601.172C361.482 605.468 346.907 613.288 334.557 624.057C311.794 643.698 297.056 673.566 295.686 705C294.062 736.393 305.905 768.822 328.241 792.259C350.375 815.863 382.66 830.049 415.5 830.238C429.307 830.413 443.114 830.5 456.922 830.5C475.281 830.5 493.641 830.346 512 830.037C528.141 829.766 540.808 817.641 540.537 801.5C539.455 737.167 536.478 672.833 531.605 608.5C530.768 597.441 523.059 588.057 512 588.895C491.319 590.461 470.639 592.223 449.958 594.181C438.472 595.269 426.986 596.417 415.5 597.625ZM415.5 619.375V597.625V619.375Z",
  "M522.875 705C515.625 705 508.375 705 501.125 705C499.913 716.243 500.55 727.873 503.105 739.245C506.915 756.307 515.104 772.771 526.967 786.533C546.623 809.59 576.761 824.582 608.5 825.969C640.198 827.62 672.934 815.565 696.441 792.941C720.124 770.523 734.136 737.906 734 705C734 705 734 705 734 705C734.136 672.094 720.124 639.477 696.441 617.059C672.934 594.435 640.198 582.38 608.5 584.031C576.761 585.418 546.623 600.41 526.967 623.467C515.104 637.229 506.915 653.693 503.105 670.755C500.55 682.127 499.913 693.757 501.125 705C508.375 705 515.625 705 522.875 705C524.082 696.03 526.645 687.453 530.341 679.605C535.878 667.823 543.965 657.723 553.561 650.061C569.547 637.22 589.653 631.4 608.5 632.969C627.389 634.438 644.896 643.195 657.031 656.469C669.245 669.693 676.045 687.32 676 705C676.045 722.68 669.245 740.307 657.031 753.531C644.896 766.805 627.389 775.562 608.5 777.031C589.653 778.6 569.547 772.78 553.561 759.939C543.965 752.277 535.878 742.177 530.341 730.395C526.645 722.547 524.082 713.97 522.875 705ZM501.125 705H522.875H501.125V705Z",
];

// Ligmerity's face, eyes and mouth, drawn on the top block
const LIGMA_SMILEY = [
  "M486.375 320.486L485.98 321.079C483.828 321.079 481.771 321.152 479.662 320.684L479.07 321.079C477.974 323.272 476.146 325.019 474.6 326.917C472.01 330.093 469.226 333.383 466.115 336.19C465.878 336.403 465.572 336.465 465.327 336.66C464.964 336.948 464.744 337.389 464.383 337.679C463.019 338.773 461.583 339.799 460.223 340.893C454.192 345.744 447.315 349.912 439.972 352.654C426.213 357.795 408.839 358.465 394.776 354.18C392.018 353.341 389.325 352.287 386.711 351.169C386.124 350.919 385.639 350.469 385.052 350.218C384.456 349.963 383.846 349.882 383.25 349.574C382.132 348.998 381.088 348.188 380 347.56C379.096 347.041 378.087 346.696 377.208 346.135C376.223 345.505 375.285 344.713 374.377 344.018C372.561 342.632 370.567 341.426 368.817 339.953C366.217 337.762 363.001 334.707 360.703 332.206C360.488 331.971 360.426 331.667 360.231 331.422C359.943 331.059 359.502 330.84 359.212 330.478C358.32 329.363 357.427 328.265 356.503 327.171C356.261 326.881 356.152 326.508 355.945 326.196C355.427 325.42 354.681 324.777 354.168 323.991C353.483 322.94 352.875 321.888 352.12 320.881C350.026 321.432 348.584 321.89 346.395 321.671L346 321.079C346 318.07 347.433 315.734 350.555 315.006C351.814 314.711 353.394 314.958 354.687 314.958L355.082 315.156C355.266 316.814 355.133 318.245 353.897 319.499C354.361 320.889 355.252 321.519 356.188 322.587C356.436 322.869 356.5 323.242 356.752 323.521C357.499 324.34 358.359 325.079 359.157 325.847C364.259 330.767 370.026 334.773 376.19 338.214C381.844 341.369 387.88 344.014 394.178 345.744C410.034 350.097 431.191 348.204 446.505 342.459C449.226 341.438 451.828 340.129 454.438 338.824C456.148 337.969 457.864 337.12 459.477 336.151C460.432 335.576 461.248 334.773 462.172 334.153C463.139 333.505 464.239 332.994 465.137 332.261C465.838 331.691 466.438 331 467.139 330.429C467.428 330.194 467.797 330.133 468.065 329.868C468.419 329.517 468.689 329.055 469.086 328.751C470.841 327.398 472.359 325.88 473.712 324.125C474.016 323.728 474.478 323.458 474.829 323.104C475.506 322.421 476.861 320.741 477.293 319.894C477.228 318.921 476.162 318.028 475.714 317.13V314.958L475.911 314.563L476.306 314.366C477.331 314.303 478.442 314.105 479.465 314.202C482.815 314.516 485.678 317.353 486.375 320.486Z",
  "M443.334 286.923C444.098 290.556 444.724 294.534 443.502 298.167C443.291 298.799 442.88 299.349 442.651 299.975C441.895 302.039 440.604 303.689 438.596 304.692C437.849 304.755 436.979 304.955 436.234 304.838C433.073 304.343 431.693 301.126 430.501 298.572C429.836 294.416 429.19 289.956 430.992 285.954C432.27 283.118 433.911 281 437.214 281C437.98 281 438.681 281.008 439.356 281.424C441.427 282.698 442.298 284.85 443.334 286.923Z",
  "M402.664 298.967C401.353 301.806 400.533 302.954 397.925 304.693H394.766C393.303 303.88 392.13 302.979 391.238 301.516C388.034 296.272 388.184 285.489 393.603 281.631C394.519 280.979 395.483 281.031 396.543 281.094C397.256 281.135 398.057 281.078 398.713 281.4C401.661 282.849 403.453 288.401 403.453 291.465C403.453 294.355 403.293 296.136 402.664 298.967Z",
];

const LIGMA_APP_ICONS = [
  { id: "ligma", name: "Ligma", collection: "colorways" },

  // Colorways
  {
    // Windows only: macOS already shows Ligma's own icon dark when the
    // system's icon style is dark
    id: "ligma-dark", name: "Ligma Dark", collection: "colorways", windowsOnly: true,
    art() {
      // Ligma's own icon on a dark tile, as Figma's dark icon is
      const { fill, linear, shadow, parts } = ligmaSvg;
      return {
        defs: linear("tile-fill", [[0, "#2e2e30"], [1, "#161617"]]) + linear("sheen", [[0, "#fff", 0.07], [0.5, "#fff", 0]])
          + linear("gloss", [[0, "#fff", 0.28], [0.5, "#fff", 0.04], [1, "#fff", 0]]) + shadow("lift", 12, 16, 0.55),
        body: fill("url(#tile-fill)") + fill("url(#sheen)")
          + `<g filter="url(#lift)">${parts(LIGMA_COLORS)}</g>`
          + parts(Array(4).fill("url(#gloss)"))
          + parts(Array(4).fill("none"), 'stroke="#fff" stroke-opacity=".18" stroke-width="3"'),
      };
    },
  },
  { id: "onyx", name: "Onyx", collection: "colorways", art: ligmaColorway([[0, "#2c2c31"], [1, "#0b0b0d"]], ["#ffffff", "#d0d0d6", "#94949c", "#ffffff"], 0.5) },
  { id: "ink", name: "Ink", collection: "colorways", art: ligmaColorway([[0, "#fcf9f2"], [1, "#ece5d6"]], ["#151515", "#3d3d3d", "#6f6f6f", "#151515"], 0.14) },
  { id: "slate", name: "Slate", collection: "colorways", art: ligmaColorway([[0, "#5d6d82"], [1, "#2d3747"]], ["#eef3f8", "#bccada", "#8fa3ba", "#ffc94d"], 0.35) },
  { id: "pastel", name: "Pastel", collection: "colorways", art: ligmaColorway([[0, "#fffaf2"], [1, "#f3e9ff"]], ["#72dda5", "#ff98a9", "#bea2ff", "#86d3ff"], 0.12) },
  { id: "blush", name: "Blush", collection: "colorways", art: ligmaColorway([[0, "#ffe9ee"], [1, "#ffc6d3"]], ["#e8577d", "#c42d5a", "#8f1740", "#e8577d"], 0.16) },
  { id: "sunset", name: "Sunset", collection: "colorways", art: ligmaColorway([[0, "#ffb347"], [0.5, "#ff5e62"], [1, "#9a3cff"]], ["#fff4d6", "#ffd6a3", "#ffc0da", "#ffe14d"], 0.22) },
  { id: "tangerine", name: "Tangerine", collection: "colorways", art: ligmaColorway([[0, "#ffa040"], [1, "#ff5a1f"]], ["#ffffff", "#ffe0c7", "#ffc296", "#ffffff"], 0.24) },
  { id: "citrus", name: "Citrus", collection: "colorways", art: ligmaColorway([[0, "#fffbea"], [1, "#ffefb8"]], ["#74c83a", "#ff4d6d", "#ff8c1a", "#ffc400"], 0.16) },
  { id: "retro", name: "Retro", collection: "colorways", art: ligmaColorway([[0, "#f6e7c8"], [1, "#ead2a4"]], ["#e8a33d", "#d9622b", "#9b3b22", "#2f6f6a"], 0.18) },
  { id: "matcha", name: "Matcha", collection: "colorways", art: ligmaColorway([[0, "#d5e6bd"], [1, "#a9c98c"]], ["#3e6b2a", "#5a8a3c", "#7ea85c", "#fdfbe9"], 0.18) },
  { id: "forest", name: "Forest", collection: "colorways", art: ligmaColorway([[0, "#1f5c45"], [1, "#09271d"]], ["#b5e48c", "#f4a259", "#74c69d", "#f1faee"], 0.4) },
  { id: "tide", name: "Tide", collection: "colorways", art: ligmaColorway([[0, "#167dd6"], [1, "#0a2b5e"]], ["#7df9ff", "#3ac7ff", "#93a9ff", "#e9fdff"], 0.35) },
  { id: "cobalt", name: "Cobalt", collection: "colorways", art: ligmaColorway([[0, "#3a86ff"], [1, "#1446d8"]], ["#ffffff", "#d6e4ff", "#a9c4ff", "#ffffff"], 0.28) },
  { id: "ultraviolet", name: "Ultraviolet", collection: "colorways", art: ligmaColorway([[0, "#3d108f"], [1, "#11032b"]], ["#ff5cdb", "#c24cff", "#7b5cff", "#2bf0ff"], 0.45) },
  {
    id: "prism", name: "Prism", collection: "colorways",
    art() {
      // One spectrum running through the whole logo, on near black
      const { fill, linear, radial, blur, parts } = ligmaSvg;
      const g = LIGMA_G;
      const spectrum = [[0, "#00d26a"], [0.25, "#00b8ff"], [0.5, "#8a4dff"], [0.75, "#ff2e7a"], [1, "#ff9a1f"]];
      return {
        defs: radial("tile-fill", [[0, "#1c1c22"], [1, "#09090b"]], [0.5, 0.35, 0.85]) + linear("spectrum", spectrum, [g.x0, g.y0, g.right, g.bottom], true)
          + linear("gloss", [[0, "#fff", 0.22], [0.5, "#fff", 0]]) + blur("glow", 40),
        body: fill("url(#tile-fill)")
          + `<g filter="url(#glow)" opacity=".55">${parts(Array(4).fill("url(#spectrum)"))}</g>`
          + parts(Array(4).fill("url(#spectrum)")) + parts(Array(4).fill("url(#gloss)")),
      };
    },
  },
  { id: "mocha", name: "Mocha", collection: "colorways", art: ligmaColorway([[0, "#5c3d2c"], [1, "#2a1911"]], ["#f5e3c8", "#d8b38c", "#b07f58", "#fff8ec"], 0.45) },

  {
    // Glossy ice blue, like the crest of a wave
    id: "glacier", name: "Glacier", collection: "colorways",
    art: ligmaGradientParts("#121320", ligmaWave([[0, "#fff"], [0.263, "#58ffff"], [0.491, "#3ee8ff"], [1, "#0066ff"]])),
  },
  {
    // A different run of the spectrum through each part
    id: "chroma", name: "Chroma", collection: "colorways",
    art: ligmaGradientParts("#16121d", [
      [[[0, "#f30000"], [0.087, "#f30000"], [0.263, "#fa6b05"], [0.491, "#ffd900"], [1, "#00ff26"]], [364.5, 187.5, 512, 415]],
      [[[0, "#00ffcc"], [0.263, "#0066ff"], [0.567, "#7300ff"], [1, "#ea00ff"]], [301.5, 451, 546, 595]],
      [[[0, "#7300ff"], [0.263, "#fc58ff"], [0.491, "#ff3ea2"], [1, "#ff00b3"]], [492, 808.5, 527.5, 635]],
      [[[0, "#ff0004"], [0.263, "#ff914c"], [0.638, "#c5ff3e"], [1, "#00f7ff"]], [702.5, 671, 512, 722]],
    ]),
  },
  {
    id: "petal", name: "Petal", collection: "colorways",
    art: ligmaGradientParts("#17060b", ligmaWave([[0, "#fff"], [0.263, "#ff58cd"], [0.491, "#ff3e3e"], [1, "#760000"]])),
  },
  { id: "sherbet", name: "Sherbet", collection: "colorways", art: ligmaFlat("#24202f", ["#ff6363", "#99e4ff", "#b8f57c", "#ffe683"]) },
  { id: "lullaby", name: "Lullaby", collection: "colorways", art: ligmaFlat("#24202f", ["#b8f57c", "#ff6363", "#b783ff", "#99e4ff"]) },

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
      const { fill, radial, shadow, clips } = ligmaSvg;
      return {
        defs: radial("tile-fill", [[0, "#26221c"], [0.6, "#121110"], [1, "#070707"]], [0.45, 0.3, 0.9])
          + radial("warmth", [[0, "#d9a54a", 0.16], [1, "#d9a54a", 0]], [0.5, 0.5, 0.42])
          + ligmaGold("gold") + clips() + shadow("lift", 16, 18, 0.7),
        body: fill("url(#tile-fill)") + fill("url(#warmth)")
          + `<g filter="url(#lift)">${ligmaSvg.parts(Array(4).fill("#9a6a2a"))}</g>`
          + ligmaGilt("gold"),
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

  {
    id: "scribe", name: "Scribe", collection: "graphic",
    art() {
      // The logo's outline in pen, its line swelling and thinning, on cream
      const { fill, linear, clips } = ligmaSvg;
      return {
        defs: linear("tile-fill", [[0, "#fff"], [1, "#ffe6cf"]], [512, 100, 512, 924], true) + clips(),
        body: fill("url(#tile-fill)") + LIGMA_SCRIBE_INK.map((d, i) => `<path d="${d}" fill="#241711" clip-path="url(#part${i})"/>`).join(""),
      };
    },
  },
  {
    id: "interface", name: "Interface", collection: "graphic",
    art() {
      // The logo as a white outline, like an icon in a toolbar
      const { fill, clips } = ligmaSvg;
      return {
        defs: clips(),
        body: fill("#2c2c2c") + LIGMA_G.parts.map((d, i) => `<path d="${d}" fill="none" stroke="#fff" stroke-width="50" stroke-linejoin="round" clip-path="url(#part${i})"/>`).join(""),
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
          // Its outside edge reaches as far as the other icons' logo
          + parts(Array(4).fill("none"), `stroke="${ink}" stroke-opacity=".88" stroke-width="34" stroke-linejoin="round"`, ligmaGlyph(LIGMA_G.unit - 34 / 3)),
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
  {
    id: "balloon", name: "Balloon", collection: "playful",
    art() {
      // Foil party balloons, each block blown up on its own, on strings.
      // Shrunk and stroked round, so their corners puff out
      const { fill, linear, radial, shadow, scatter, shrink, clips, mix } = ligmaSvg;
      const g = LIGMA_G;
      const colors = ["#ff4fa0", "#ffad1f", "#8a63ff", "#14c2d3"];
      const puff = 0.8;
      // A string curling down from `x`, `y` to the tile's bottom
      const string = (x, y) => {
        let d = `M${x} ${y}`;
        for (let at = 0; y + at < 940; at += 70) d += `q${at % 140 ? 22 : -22} 35 0 70`;
        return `<path d="${d}" fill="none" stroke="#8b93b8" stroke-opacity=".8" stroke-width="4"/>`;
      };
      const confetti = scatter(17, 24, (x, y, random, i) => {
        if (x > g.x0 - 40 && x < g.right + 40 && y > g.y0 - 40 && y < g.bottom + 40) return "";
        return `<circle cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="${(5 + random() * 7).toFixed(1)}" fill="${colors[i % 4]}" opacity=".7"/>`;
      });
      // Where a balloon's knot is, under part `i`
      const knot = (i) => {
        const [x, y] = g.centers[i];
        const bottom = y + g.r * puff + 14;
        return [x + (i === 3 ? 0 : g.r * 0.5), bottom];
      };
      const tie = (i) => {
        const [x, y] = knot(i);
        return string(x, y + 14) + `<path d="M${x} ${y - 6}l-13 22h26z" fill="${mix(colors[i], "#000000", 0.2)}"/>`;
      };
      const balloon = (d, i) => `<path d="${d}" fill="url(#foil${i})" stroke="url(#foil${i})" stroke-width="30" stroke-linejoin="round" ${shrink(i, puff)}/>`;
      return {
        defs: linear("tile-fill", [[0, "#ffe6f2"], [1, "#d9efff"]]) + shadow("lift", 18, 18, 0.25, "#5a2a6e") + clips()
          + colors.map((c, i) => radial(`foil${i}`, [[0, mix(c, "#ffffff", 0.55)], [0.35, c], [0.85, mix(c, "#000000", 0.18)], [1, mix(c, "#000000", 0.32)]], [0.32, 0.28, 0.85])).join(""),
        body: fill("url(#tile-fill)") + confetti + tie(2) + tie(3)
          + `<g filter="url(#lift)">${g.parts.map(balloon).join("")}</g>`
          + g.parts.map((d, i) => {
            const [x, y] = g.centers[i];
            const r = g.r * puff;
            return `<ellipse cx="${x - r * 0.38}" cy="${y - r * 0.45}" rx="${r * 0.3}" ry="${r * 0.14}" fill="#fff" opacity=".85" transform="rotate(-38 ${x - r * 0.38} ${y - r * 0.45})"/>`
              + `<path d="M${x - r * 0.15} ${y + r * 0.95}a${r} ${r} 0 0 1 ${-r * 0.75} ${-r * 0.75}" fill="none" stroke="#fff" stroke-opacity=".45" stroke-width="7" stroke-linecap="round"/>`;
          }).join(""),
      };
    },
  },
  {
    id: "doodle", name: "Doodle", collection: "playful",
    art() {
      // Crayon and pen on a page of a school notebook
      const { fill, parts, mix } = ligmaSvg;
      const g = LIGMA_G;
      const colors = ["#22b46a", "#ff4757", "#8a5cff", "#1e9bff"];
      const turns = [-38, 42, -30, 50];
      const page = `<pattern id="rules" width="1024" height="46" patternUnits="userSpaceOnUse" y="20"><path d="M0 45H1024" stroke="#8fbcf0" stroke-opacity=".65" stroke-width="2.5"/></pattern>`;
      const hatches = colors.map((c, i) => `<pattern id="hatch${i}" width="20" height="20" patternUnits="userSpaceOnUse" patternTransform="rotate(${turns[i]})"><path d="M0 10H20" stroke="${c}" stroke-width="9" stroke-linecap="round"/></pattern>`).join("");
      const wobble = `<filter id="wobble" x="-10%" y="-10%" width="120%" height="120%"><feTurbulence type="fractalNoise" baseFrequency="0.025" numOctaves="2" seed="3"/><feDisplacementMap in="SourceGraphic" scale="9" xChannelSelector="R" yChannelSelector="G"/></filter>`;
      const star = (x, y, r) => `M${Array.from({ length: 10 }, (_, i) => {
        const a = (i * Math.PI) / 5 - Math.PI / 2;
        const at = i % 2 ? r * 0.45 : r;
        return `${(x + Math.cos(a) * at).toFixed(1)} ${(y + Math.sin(a) * at).toFixed(1)}`;
      }).join("L")}Z`;
      const ink = 'stroke="#24262b" stroke-width="8" stroke-linecap="round" stroke-linejoin="round"';
      const pen = `fill="none" ${ink}`;
      return {
        defs: page + hatches + wobble,
        body: fill("#fffdf5") + fill("url(#rules)") + `<path d="M196 0V1024" stroke="#ff8f8f" stroke-width="3.5"/>`
          + `<g filter="url(#wobble)">`
          + parts(colors.map((c) => mix(c, "#ffffff", 0.8))) + parts(colors.map((_, i) => `url(#hatch${i})`), 'fill-opacity=".85"')
          + parts(Array(4).fill("none"), ink)
          + `<path d="${star(800, 250, 62)}" fill="#ffd23f" ${ink}/>`
          + `<path d="M850 470C890 560 850 660 ${g.right + 40} ${g.by - 14}" ${pen}/><path d="M${g.right + 75} ${g.by - 42}L${g.right + 40} ${g.by - 14}L${g.right + 79} ${g.by + 6}" ${pen}/>`
          + `<path d="M188 776c30 -40 70 -10 50 18c-22 30 -70 4 -46 -36c26 -42 100 -20 96 38" ${pen.replace('width="8"', 'width="6"')}/>`
          + `</g>`,
      };
    },
  },
  {
    id: "knit", name: "Knit", collection: "playful",
    art() {
      // A sweater with the logo knitted into it, each part in its own yarn
      const { fill, linear, mix, clips } = ligmaSvg;
      const g = LIGMA_G;
      // Twelve stitches across the logo and seven to a block, lined up
      // with its edges
      const w = g.unit / 6;
      const h = g.unit / 7;
      const colors = ["#3fb36c", "#ea4a46", "#8b6ce0", "#38a2ec"];
      // A stitch in `color`: two loops of yarn leaning into a V, with a
      // fibre down each
      const stitch = (id, color) => `<pattern id="${id}" width="${w}" height="${h}" patternUnits="userSpaceOnUse" x="${g.x0}" y="${g.y0}">`
        + `<rect width="${w}" height="${h}" fill="${mix(color, "#000000", 0.38)}"/>`
        + [-1, 1].map((side) => {
          const [x, y] = [w / 2 + side * w * 0.22, h / 2];
          const turn = `transform="rotate(${side * 30} ${x} ${y})"`;
          return `<ellipse cx="${x}" cy="${y}" rx="${w * 0.25}" ry="${h * 0.52}" fill="${color}" ${turn}/><ellipse cx="${x}" cy="${y}" rx="${w * 0.25}" ry="${h * 0.52}" fill="url(#yarn)" ${turn}/>`
            + `<path d="M${x} ${y - h * 0.3}V${y + h * 0.3}" stroke="#000" stroke-opacity=".14" stroke-width="1.6" stroke-linecap="round" ${turn}/>`;
        }).join("")
        + `</pattern>`;
      const fuzz = `<filter id="fuzz" x="-5%" y="-5%" width="110%" height="110%"><feTurbulence type="fractalNoise" baseFrequency="0.3" numOctaves="2" seed="8"/><feDisplacementMap in="SourceGraphic" scale="5" xChannelSelector="R" yChannelSelector="G"/></filter>`;
      return {
        defs: linear("yarn", [[0, "#fff", 0.32], [0.45, "#fff", 0], [1, "#000", 0.28]]) + clips() + fuzz
          + stitch("ground", "#ebdfc9") + colors.map((c, i) => stitch(`stitch${i}`, c)).join(""),
        body: fill("url(#ground)")
          + `<g filter="url(#fuzz)">${g.parts.map((_, i) => `<rect x="${g.x0 - 10}" y="${g.y0 - 10}" width="${g.right - g.x0 + 20}" height="${g.bottom - g.y0 + 20}" fill="url(#stitch${i})" clip-path="url(#part${i})"/>`).join("")}</g>`,
      };
    },
  },
  {
    id: "buddies", name: "Buddies", collection: "playful",
    art() {
      // The blocks and the dot as four little friends
      const { fill, linear, shadow, parts, scatter } = ligmaSvg;
      const g = LIGMA_G;
      const r = g.r;
      const ink = "#2b1d24";
      const line = `fill="none" stroke="${ink}" stroke-width="${r * 0.08}" stroke-linecap="round"`;
      const eye = (x, y) => `<circle cx="${x}" cy="${y}" r="${r * 0.14}" fill="${ink}"/><circle cx="${x + r * 0.05}" cy="${y - r * 0.06}" r="${r * 0.055}" fill="#fff"/>`;
      const happy = (x, y) => `<path d="M${x - r * 0.1} ${y + r * 0.03}q${r * 0.1} ${-r * 0.16} ${r * 0.2} 0" ${line}/>`;
      const cheeks = (x, y) => `<ellipse cx="${x - r * 0.5}" cy="${y + r * 0.2}" rx="${r * 0.13}" ry="${r * 0.08}" fill="#ff7aa8" opacity=".6"/><ellipse cx="${x + r * 0.5}" cy="${y + r * 0.2}" rx="${r * 0.13}" ry="${r * 0.08}" fill="#ff7aa8" opacity=".6"/>`;
      const smile = (x, y, w = 0.16) => `<path d="M${x - r * w} ${y + r * 0.16}q${r * w} ${r * w * 1.2} ${r * w * 2} 0" ${line}/>`;
      const faces = [
        ([x, y]) => happy(x - r * 0.3, y) + happy(x + r * 0.3, y) + `<path d="M${x - r * 0.16} ${y + r * 0.15}q${r * 0.16} ${r * 0.3} ${r * 0.32} 0z" fill="${ink}"/>` + cheeks(x, y),
        ([x, y]) => eye(x - r * 0.3, y) + eye(x + r * 0.3, y) + smile(x, y) + cheeks(x, y),
        ([x, y]) => eye(x - r * 0.3, y) + happy(x + r * 0.3, y) + smile(x, y, 0.2) + `<path d="M${x - r * 0.02} ${y + r * 0.26}q${r * 0.08} ${r * 0.16} ${r * 0.16} 0" fill="#ff6f91"/>` + cheeks(x, y),
        ([x, y]) => eye(x - r * 0.28, y - r * 0.05) + eye(x + r * 0.28, y - r * 0.05) + `<ellipse cx="${x}" cy="${y + r * 0.25}" rx="${r * 0.09}" ry="${r * 0.12}" fill="${ink}"/>` + cheeks(x, y),
      ];
      const sparkles = scatter(29, 14, (x, y, random) => {
        if (x > g.x0 - 50 && x < g.right + 40 && y > g.y0 - 50 && y < g.bottom + 40) return "";
        const s = 10 + random() * 14;
        return `<path d="M${x} ${y - s}Q${x} ${y} ${x + s} ${y}Q${x} ${y} ${x} ${y + s}Q${x} ${y} ${x - s} ${y}Q${x} ${y} ${x} ${y - s}Z" fill="#fff" opacity=".9"/>`;
      });
      return {
        defs: linear("tile-fill", [[0, "#fff6cc"], [1, "#ffd98c"]]) + linear("gloss", [[0, "#fff", 0.35], [0.5, "#fff", 0]]) + shadow("lift", 14, 12, 0.22, "#8a5300"),
        body: fill("url(#tile-fill)") + sparkles
          + `<g filter="url(#lift)">${parts(["#2ed47a", "#ff4a64", "#9c66ff", "#2fc0ff"])}</g>` + parts(Array(4).fill("url(#gloss)"))
          + g.centers.map((at, i) => faces[i](at)).join(""),
      };
    },
  },

  {
    id: "ligmerity", name: "Ligmerity", collection: "playful",
    art() {
      // The blocks and the dot as four smiley faces
      const { fill, linear } = ligmaSvg;
      const g = LIGMA_G;
      const [x, y] = g.centers[0];
      const colors = ["#4c57ff", "#ff2c2b", "#ff32f2", "#ffc402"];
      const face = LIGMA_SMILEY.map((d) => `<path d="${d}" fill="#020100"/>`).join("");
      return {
        defs: linear("tile-fill", [[0, "#fff"], [1, "#ececec"]], [512, 100, 512, 924], true) + linear("sheen", [[0, "#fff", 0.16], [1, "#fff", 0]], [512, 100, 512, 512], true),
        body: fill("url(#tile-fill)") + fill("url(#sheen)")
          + g.parts.map((d, i) => `<g${i === 2 ? ' opacity=".81"' : ""}><path d="${d}" fill="${colors[i]}"/><g transform="translate(${g.centers[i][0] - x} ${g.centers[i][1] - y})">${face}</g></g>`).join(""),
      };
    },
  },

  // Seasonal
  {
    id: "fireworks", name: "Fireworks", collection: "seasonal",
    art() {
      // New Year's: a gold logo under bursts of fireworks
      const { fill, radial, blur, scatter, clips, parts, mix } = ligmaSvg;
      // A burst of `count` sparks `size` across, around `x`, `y`. `core`
      // draws the white-hot middle of each spark instead
      const burst = (x, y, size, count, color, turn, core = false) => {
        const paint = core ? mix(color, "#ffffff", 0.7) : color;
        const width = size / (core ? 90 : 34);
        let out = "";
        for (let i = 0; i < count; i++) {
          const a = ((i / count) * 360 + turn) * (Math.PI / 180);
          const at = (r) => `${(x + Math.cos(a) * r).toFixed(1)} ${(y + Math.sin(a) * r).toFixed(1)}`;
          out += `<path d="M${at(size * 0.22)}L${at(size * 0.42)}" stroke="${paint}" stroke-width="${width.toFixed(1)}" stroke-linecap="round"/>`
            + `<circle cx="${(x + Math.cos(a) * size * 0.5).toFixed(1)}" cy="${(y + Math.sin(a) * size * 0.5).toFixed(1)}" r="${(size / (core ? 100 : 40)).toFixed(1)}" fill="${paint}"/>`;
        }
        return out;
      };
      const shows = [[760, 330, 300, 16, "#ffcf5a", 0], [196, 236, 170, 12, "#ff5fa0", 10], [836, 650, 190, 12, "#5fe0ff", 6], [206, 810, 120, 10, "#a98bff", 4]];
      const bursts = shows.map((show) => burst(...show)).join("");
      const cores = shows.map((show) => burst(...show, true)).join("");
      // The light each burst throws on the sky around it
      const halos = shows.map(([x, y, size, , color]) => `<circle cx="${x}" cy="${y}" r="${size * 0.62}" fill="url(#halo-${color.slice(1)})"/>`).join("");
      const stars = scatter(23, 40, (x, y, random) => `<circle cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="${(1 + random() * 2.2).toFixed(1)}" fill="#fff" opacity="${(0.3 + random() * 0.6).toFixed(2)}"/>`);
      return {
        defs: radial("tile-fill", [[0, "#1d2350"], [0.6, "#0b0d24"], [1, "#04040c"]], [0.5, 0.25, 1]) + blur("glow", 7) + blur("bloom", 24) + clips() + ligmaGold("gold", 0.04)
          + shows.map(([, , , , color]) => radial(`halo-${color.slice(1)}`, [[0, color, 0.3], [0.5, color, 0.1], [1, color, 0]])).join("")
          + ligmaSvg.shadow("lift", 14, 16, 0.6),
        body: fill("url(#tile-fill)") + halos + stars
          + `<g filter="url(#bloom)">${bursts}</g><g filter="url(#glow)">${bursts}</g>${bursts}${cores}`
          + `<g filter="url(#lift)">${parts(Array(4).fill("#9a6a2a"))}</g>` + ligmaGilt("gold"),
      };
    },
  },
  {
    id: "sweetheart", name: "Sweetheart", collection: "seasonal",
    art() {
      // Valentine's Day: the dot turned into a heart, among hearts
      const { fill, linear, shadow, scatter, heart, mix } = ligmaSvg;
      const g = LIGMA_G;
      const colors = ["#ff4d79", "#e01e4f", "#b0103d", "#ff2a55"];
      const big = heart(g.bx, g.by + 4, g.unit * 1.22);
      const hearts = scatter(5, 26, (x, y, random) => {
        if (x > g.x0 - 60 && x < g.right + 50 && y > g.y0 - 60 && y < g.bottom + 60) return "";
        return `<path d="${heart(x, y, 26 + random() * 40)}" fill="#fff" opacity="${(0.35 + random() * 0.4).toFixed(2)}" transform="rotate(${Math.round(random() * 50 - 25)} ${x.toFixed(1)} ${y.toFixed(1)})"/>`;
      });
      return {
        defs: linear("tile-fill", [[0, "#ffd3e2"], [1, "#ff8fb4"]])
          + colors.map((c, i) => linear(`candy${i}`, [[0, mix(c, "#ffffff", 0.28)], [0.6, c], [1, mix(c, "#000000", 0.15)]])).join("")
          + linear("gloss", [[0, "#fff", 0.45], [0.45, "#fff", 0]]) + shadow("lift", 14, 14, 0.3, "#a3003b"),
        body: fill("url(#tile-fill)") + hearts
          + `<g filter="url(#lift)">${g.parts.slice(0, 3).map((d, i) => `<path d="${d}" fill="url(#candy${i})"/>`).join("")}<path d="${big}" fill="url(#candy3)"/></g>`
          + g.parts.slice(0, 3).map((d) => `<path d="${d}" fill="url(#gloss)"/>`).join("") + `<path d="${big}" fill="url(#gloss)"/>`,
      };
    },
  },
  {
    id: "bloom", name: "Bloom", collection: "seasonal",
    art() {
      // Spring: the dot a flower, with petals drifting by
      const { fill, linear, shadow, scatter } = ligmaSvg;
      const g = LIGMA_G;
      const flower = (x, y, r, petal, middle) => {
        let out = "";
        for (let i = 0; i < 5; i++) out += `<ellipse cx="${x}" cy="${y - r * 0.52}" rx="${r * 0.4}" ry="${r * 0.55}" fill="${petal}" transform="rotate(${i * 72} ${x} ${y})"/>`;
        return `${out}<circle cx="${x}" cy="${y}" r="${r * 0.3}" fill="${middle}"/>`;
      };
      const petals = scatter(31, 22, (x, y, random) => {
        if (x > g.x0 - 50 && x < g.right + 50 && y > g.y0 - 50 && y < g.bottom + 50) return "";
        const turn = Math.round(random() * 180);
        return `<ellipse cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" rx="${(9 + random() * 7).toFixed(1)}" ry="${(15 + random() * 8).toFixed(1)}" fill="${random() > 0.5 ? "#ffffff" : "#ffc2d6"}" opacity=".85" transform="rotate(${turn} ${x.toFixed(1)} ${y.toFixed(1)})"/>`;
      });
      return {
        defs: linear("tile-fill", [[0, "#e6f8e0"], [1, "#b7e4c7"]]) + shadow("lift", 12, 12, 0.22, "#2d6a4f"),
        body: fill("url(#tile-fill)") + petals
          + flower(222, 236, 70, "#ffffff", "#ffd23f") + flower(838, 300, 56, "#ffb3cb", "#ffd23f") + flower(824, 836, 64, "#d9c2ff", "#ffd23f")
          + `<g filter="url(#lift)">${g.parts.slice(0, 3).map((d, i) => `<path d="${d}" fill="${["#52b788", "#ff8fab", "#b197fc"][i]}"/>`).join("")}${flower(g.bx, g.by, g.r * 1.18, "#ffffff", "#ffc300")}</g>`,
      };
    },
  },
  {
    id: "pool", name: "Pool", collection: "seasonal",
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
    id: "harvest", name: "Harvest", collection: "seasonal",
    art() {
      // Autumn: the logo in fall colors, leaves coming down around it
      const { fill, linear, shadow, scatter, leaf, parts } = ligmaSvg;
      const g = LIGMA_G;
      const tones = ["#d9541e", "#f2b134", "#a8321e", "#e8772e", "#c9a227"];
      const leaves = scatter(41, 40, (x, y, random, i) => {
        if (x > g.x0 - 50 && x < g.right + 40 && y > g.y0 - 40 && y < g.bottom + 40) return "";
        return leaf(x, y, 58 + random() * 40, random() * 360, tones[i % tones.length]);
      });
      return {
        defs: linear("tile-fill", [[0, "#fde9c8"], [1, "#f4c48a"]]) + shadow("lift", 14, 14, 0.28, "#7a3a0a"),
        body: fill("url(#tile-fill)") + leaves
          + `<path d="M100 880C300 846 420 870 560 852S820 846 924 870V924H100Z" fill="#e39a4e" opacity=".55"/>`
          + `<g filter="url(#lift)">${parts(["#6f8f2f", "#e0611f", "#b23a1f", "#f2b134"])}</g>`,
      };
    },
  },
  {
    id: "pumpkin", name: "Pumpkin", collection: "seasonal",
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
    id: "festive", name: "Festive", collection: "seasonal",
    art() {
      // The holidays: the blocks wrapped as presents, the dot a bauble and
      // a string of lights overhead
      const { fill, radial, shadow, clips, parts, mix, blur } = ligmaSvg;
      const g = LIGMA_G;
      const colors = ["#d62839", "#f2c14e", "#1f8a5b", "#d62839"];
      const ribbons = ["#f2c14e", "#d62839", "#f2c14e"];
      const wrap = g.parts.slice(0, 3).map((d, i) => {
        const top = g.y0 + i * g.unit;
        const x = g.xc + 20;
        return `<g clip-path="url(#part${i})"><rect x="${x - 15}" y="${top - 10}" width="30" height="${g.unit + 20}" fill="${ribbons[i]}"/><rect x="${g.x0 - 10}" y="${top + g.unit / 2 - 15}" width="${g.unit + 20}" height="30" fill="${ribbons[i]}"/>`
          + `<rect x="${g.x0 - 10}" y="${top - 10}" width="${g.unit + 20}" height="${g.unit / 2 + 10}" fill="#fff" opacity=".1"/></g>`;
      }).join("");
      const bow = (x, y) => `<path d="M${x} ${y}c-34 -30 -60 -6 -40 14c12 12 30 4 40 -14zM${x} ${y}c34 -30 60 -6 40 14c-12 12 -30 4 -40 -14z" fill="#f2c14e" stroke="#c99a2e" stroke-width="3"/><circle cx="${x}" cy="${y}" r="10" fill="#e0ad3a"/>`;
      // Bulbs along a sagging wire
      const wire = (t) => {
        const [ax, ay, cx, cy, bx, by] = [80, 120, 512, 330, 944, 120];
        return [(1 - t) ** 2 * ax + 2 * (1 - t) * t * cx + t * t * bx, (1 - t) ** 2 * ay + 2 * (1 - t) * t * cy + t * t * by];
      };
      const bulbColors = ["#ff4d4d", "#ffd23f", "#3ddc97", "#4dc9ff", "#ff7ad9"];
      let bulbs = "";
      for (let i = 1; i < 12; i++) {
        const [x, y] = wire(i / 12);
        const color = bulbColors[i % bulbColors.length];
        bulbs += `<rect x="${(x - 7).toFixed(1)}" y="${(y - 2).toFixed(1)}" width="14" height="14" rx="3" fill="#2b3a2f"/><ellipse cx="${x.toFixed(1)}" cy="${(y + 28).toFixed(1)}" rx="13" ry="19" fill="${color}"/><ellipse cx="${(x - 4).toFixed(1)}" cy="${(y + 22).toFixed(1)}" rx="4" ry="7" fill="#fff" opacity=".6"/>`;
      }
      const [x, y, r] = [g.bx, g.by, g.r];
      return {
        defs: radial("tile-fill", [[0, "#1f6b48"], [1, "#0a2a1c"]], [0.5, 0.35, 0.85]) + clips() + shadow("lift", 14, 14, 0.45) + blur("glow", 10)
          + radial("bauble", [[0, "#ff8a8a"], [0.5, colors[3]], [1, mix(colors[3], "#000000", 0.4)]], [0.35, 0.3, 0.8]),
        body: fill("url(#tile-fill)")
          + `<path d="M80 120Q512 330 944 120" fill="none" stroke="#2b3a2f" stroke-width="5"/>`
          + `<g filter="url(#glow)" opacity=".7">${bulbs}</g>${bulbs}`
          + `<g filter="url(#lift)">${parts(colors.slice(0, 3).concat("none"))}<circle cx="${x}" cy="${y}" r="${r}" fill="url(#bauble)"/></g>`
          + wrap + bow(g.xc + 20, g.y0 + g.unit / 2 - 4)
          + `<rect x="${x - 22}" y="${y - r - 18}" width="44" height="26" rx="5" fill="#e0ad3a"/><circle cx="${x}" cy="${y - r - 26}" r="10" fill="none" stroke="#e0ad3a" stroke-width="5"/>`
          + `<ellipse cx="${x - r * 0.35}" cy="${y - r * 0.4}" rx="${r * 0.22}" ry="${r * 0.12}" fill="#fff" opacity=".55" transform="rotate(-35 ${x - r * 0.35} ${y - r * 0.4})"/>`,
      };
    },
  },
  {
    id: "snowfall", name: "Snowfall", collection: "seasonal",
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
