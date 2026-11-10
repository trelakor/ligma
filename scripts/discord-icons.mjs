// Renders each app icon in dist/app-icons.js to assets/discord-icons/<id>.png,
// the large image Discord Rich Presence shows for the icon picked in
// Settings > App Icon (`large_image` in src-tauri/src/presence.rs). Discord
// fetches them from main, so commit them with any new or changed icon.
// "ligma" uses assets/discord-logo.png instead
//
//   npm run discord-icons            renders them
//   npm run discord-icons -- --check fails if any is missing or unused
//
// The check doesn't compare pixels: "selected" has text, and it renders
// with whichever system font each platform has

import { Resvg } from "@resvg/resvg-js";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "assets", "discord-icons");
const SIZE = 256;
const check = process.argv.includes("--check");

const script = readFileSync(join(root, "dist", "app-icons.js"), "utf8");
const { icons, svg } = vm.runInNewContext(`${script}\n({ icons: LIGMA_APP_ICONS, svg: ligmaAppIconSvg })`);

const wanted = icons.filter((icon) => icon.art).map((icon) => icon.id);
const problems = [];
if (!check) mkdirSync(out, { recursive: true });
for (const id of wanted) {
  const file = join(out, `${id}.png`);
  if (check) {
    if (!existsSync(file)) problems.push(`missing ${id}.png`);
    continue;
  }
  const png = new Resvg(svg(id, "tile"), {
    fitTo: { mode: "width", value: SIZE },
    font: { loadSystemFonts: true },
  }).render().asPng();
  writeFileSync(file, png);
}
const extra = existsSync(out) ? readdirSync(out).filter((name) => !wanted.includes(name.replace(/\.png$/, ""))) : [];
for (const name of extra) {
  if (check) problems.push(`unused ${name}`);
  else rmSync(join(out, name));
}

if (check) {
  console.log(problems.length ? `Run npm run discord-icons:\n  ${problems.join("\n  ")}` : `All ${wanted.length} Discord icons up to date.`);
  process.exitCode = problems.length ? 1 : 0;
} else {
  console.log(`Rendered ${wanted.length} Discord icons to assets/discord-icons.`);
}
