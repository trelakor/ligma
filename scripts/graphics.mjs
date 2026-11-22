// Renders the social cover and the README's graphics with a local Chrome or
// Edge: assets/social/cover.html to site/social-cover.png, the 3:1 banner in
// assets/social/banner.html to assets/social/banner.png and banner-light.png,
// the 16:9 cover in assets/social/cover-16x9.html to cover-16x9.png and
// cover-16x9-light.png, each graphic
// in assets/readme/readme.html to assets/readme/<name>-<light|dark>.png, and
// the installer art in assets/installer/installer.html to src-tauri/installer/:
// the DMG background as a 1x+2x TIFF (needs macOS for tiffutil) and the
// Windows installer header as a 24-bit BMP
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { inflateSync } from "node:zlib";
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
  if (out.startsWith(root)) console.log(out.slice(root.length + 1));
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

// Decodes an 8-bit, non-interlaced RGB or RGBA PNG, which is what the
// browser's screenshots are, into rows of RGB
function decodePng(file) {
  const png = readFileSync(file);
  let width, height, channels;
  const idat = [];
  for (let at = 8; at < png.length; ) {
    const length = png.readUInt32BE(at);
    const type = png.toString("ascii", at + 4, at + 8);
    const data = png.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      if (data[8] !== 8 || data[12] !== 0 || ![2, 6].includes(data[9])) throw new Error(`${file}: unsupported PNG`);
      channels = data[9] === 6 ? 4 : 3;
    } else if (type === "IDAT") idat.push(data);
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const pixels = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? pixels[y * stride + x - channels] : 0;
      const b = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const c = x >= channels && y > 0 ? pixels[(y - 1) * stride + x - channels] : 0;
      const p = a + b - c;
      const predictor = [0, a, b, (a + b) >> 1,
        Math.abs(p - a) <= Math.abs(p - b) && Math.abs(p - a) <= Math.abs(p - c) ? a : Math.abs(p - b) <= Math.abs(p - c) ? b : c][filter];
      pixels[y * stride + x] = (line[x] + predictor) & 0xff;
    }
  }
  return { width, height, channels, pixels };
}

// NSIS loads its bitmaps with LoadImage, so they're plain bottom-up 24-bit BMPs
function writeBmp(png, out) {
  const { width, height, channels, pixels } = decodePng(png);
  const row = Math.ceil((width * 3) / 4) * 4;
  const bmp = Buffer.alloc(54 + row * height);
  bmp.write("BM", 0, "ascii");
  bmp.writeUInt32LE(bmp.length, 2);
  bmp.writeUInt32LE(54, 10);
  bmp.writeUInt32LE(40, 14);
  bmp.writeInt32LE(width, 18);
  bmp.writeInt32LE(height, 22);
  bmp.writeUInt16LE(1, 26);
  bmp.writeUInt16LE(24, 28);
  bmp.writeUInt32LE(row * height, 34);
  bmp.writeInt32LE(2835, 38);
  bmp.writeInt32LE(2835, 42);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const from = (y * width + x) * channels;
      const to = 54 + (height - 1 - y) * row + x * 3;
      bmp[to] = pixels[from + 2];
      bmp[to + 1] = pixels[from + 1];
      bmp[to + 2] = pixels[from];
    }
  }
  writeFileSync(out, bmp);
  console.log(out.slice(root.length + 1));
}

// The DMG background is 660x420 points. A TIFF holding the 1x and 2x images
// lets Finder pick the sharp one on Retina screens
const installer = join(root, "assets", "installer", "installer.html");
const installerOut = join(root, "src-tauri", "installer");
const scratch = mkdtempSync(join(tmpdir(), "ligma-installer-"));
try {
  const header = join(scratch, "nsis-header.png");
  render(installer, "#header", header, 150, 57, 2, false);
  writeBmp(header, join(installerOut, "nsis-header.bmp"));

  if (process.platform === "darwin") {
    const dmg1x = join(scratch, "dmg-background.png");
    const dmg2x = join(scratch, "dmg-background@2x.png");
    render(installer, "#dmg", dmg1x, 660, 420, 1, false);
    render(installer, "#dmg", dmg2x, 660, 420, 2, false);
    const tiff = join(installerOut, "dmg-background.tiff");
    execFileSync("tiffutil", ["-cathidpicheck", dmg1x, dmg2x, "-out", tiff], { stdio: "ignore" });
    console.log(tiff.slice(root.length + 1));
  } else {
    console.log("Skipped src-tauri/installer/dmg-background.tiff, it needs macOS");
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
