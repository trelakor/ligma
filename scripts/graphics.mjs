// Renders the social cover and the README's graphics with a local Chrome or
// Edge: assets/social/cover.html to site/social-cover.png, the 3:1 banner in
// assets/social/banner.html to assets/social/banner.png and banner-light.png,
// the 16:9 cover in assets/social/cover-16x9.html to cover-16x9.png and
// cover-16x9-light.png, and each graphic
// in assets/readme/readme.html to assets/readme/<name>-<light|dark>.png
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const candidates = [
  process.env.CHROME,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
].filter(Boolean);
const browser = candidates.find((path) => existsSync(path));
if (!browser) {
  console.error("No Chrome or Edge found, set CHROME to its path");
  process.exit(1);
}

// Sizes in CSS pixels. The README's graphics are drawn at 2x, on a
// transparent ground so they sit on GitHub's light and dark pages
const readme = [
  { name: "download-macos", width: 236, height: 48 },
  { name: "download-windows", width: 252, height: 48 },
  { name: "compare", width: 880, height: 676 },
  { name: "image-tools", width: 880, height: 300 },
];

function render(page, hash, out, width, height, scale, transparent) {
  execFileSync(browser, [
    "--headless=new",
    "--disable-gpu",
    "--hide-scrollbars",
    "--allow-file-access-from-files",
    `--force-device-scale-factor=${scale}`,
    `--window-size=${width},${height}`,
    ...(transparent ? ["--default-background-color=00000000"] : []),
    "--virtual-time-budget=3000",
    `--screenshot=${out}`,
    pathToFileURL(page).href + hash,
  ], { stdio: "ignore" });
  console.log(out.slice(root.length + 1));
}

render(join(root, "assets", "social", "cover.html"), "", join(root, "site", "social-cover.png"), 1280, 640, 1, false);
render(join(root, "assets", "social", "cover.html"), "#light", join(root, "site", "social-cover-light.png"), 1280, 640, 1, false);
render(join(root, "assets", "social", "banner.html"), "", join(root, "assets", "social", "banner.png"), 1500, 500, 2, false);
render(join(root, "assets", "social", "banner.html"), "#light", join(root, "assets", "social", "banner-light.png"), 1500, 500, 2, false);
for (const theme of ["", "-light"]) {
  render(join(root, "assets", "social", "cover-16x9.html"), theme && "#light", join(root, "assets", "social", `cover-16x9${theme}.png`), 1600, 900, 2, false);
}
for (const { name, width, height } of readme) {
  for (const theme of ["light", "dark"]) {
    render(join(root, "assets", "readme", "readme.html"), `#${name}-${theme}`, join(root, "assets", "readme", `${name}-${theme}.png`), width, height, 2, true);
  }
}
