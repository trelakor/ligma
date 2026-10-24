// Draws the Rich Presence tool icons in assets/tools from Lucide icons
// (ISC, https://lucide.dev): a white glyph on a rounded square, Figma blue
// while Ligma has focus and dark grey while it doesn't.
//
//   npm run tool-icons

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const lucide = join(dirname(createRequire(import.meta.url).resolve("lucide-static/package.json")), "icons");

/** `tool_icon` slug in src-tauri/src/presence.rs → Lucide icon. */
const ICONS = {
  annotation: "message-square-text",
  arrow: "arrow-up-right",
  comment: "message-circle",
  ellipse: "circle",
  frame: "frame",
  hand: "hand",
  line: "slash",
  measurement: "ruler",
  move: "mouse-pointer-2",
  pen: "pen-tool",
  pencil: "pencil",
  polygon: "triangle",
  rectangle: "square",
  scale: "scaling",
  section: "square-dashed",
  slice: "slice",
  star: "star",
  text: "type",
  "text-on-path": "spline",
};
const BACKGROUNDS = { focused: "#0C8CE9", unfocused: "#1E1E1E" };
const SIZE = 34;
const GLYPH = 18;

for (const [slug, name] of Object.entries(ICONS)) {
  const glyph = readFileSync(join(lucide, `${name}.svg`), "utf8")
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/^[\s\S]*?<svg[^>]*>|<\/svg>\s*$/g, "");
  const offset = (SIZE - GLYPH) / 2;
  for (const [state, background] of Object.entries(BACKGROUNDS)) {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${SIZE}" height="${SIZE}">
  <rect width="${SIZE}" height="${SIZE}" rx="4" fill="${background}"/>
  <g transform="translate(${offset} ${offset}) scale(${GLYPH / 24})" fill="none" stroke="#FFFFFF"
     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${glyph}</g>
</svg>`;
    const dir = join(root, "assets", "tools", state);
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, `${slug}.png`), new Resvg(svg).render().asPng());
  }
}
