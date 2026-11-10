// Builds the app icons from assets/Ligma.icon, the Icon Composer document.
// Open it in Icon Composer (Xcode 26 or later) to edit the design
//
// - assets/icon.png is rendered from it with ictool, then tauri icon makes
//   the PNGs and icon.icns in src-tauri/icons from that. icon.icns is the
//   icon on macOS 11 to 15
// - src-tauri/icons/Assets.car is compiled from it with actool. It carries
//   the Liquid Glass icon with its dark, tinted and clear appearances, which
//   macOS 26 and later use. Committing it means release builds don't need
//   Xcode 26
// - src-tauri/icons/icon.ico comes from assets/icon-windows.png
//
// Needs macOS with Xcode 26 or later. If actool fails with "Bad file
// descriptor", quit its stuck helper with `pkill ibtoold` and run it again
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const source = join(root, "assets/Ligma.icon");
const icons = join(root, "src-tauri/icons");
const ictool = "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool";

// actool fails with "Bad file descriptor" when stdin is closed, so give
// every tool a real one
const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, stdio: ["ignore", "inherit", "inherit"] });
const tauri = (...args) => run(process.platform === "win32" ? "npx.cmd" : "npx", ["tauri", ...args]);

const work = mkdtempSync(join(tmpdir(), "ligma-icon-"));
try {
  // ictool draws the icon edge to edge, so it's drawn at 824 and padded to
  // 1024 to match the macOS icon grid
  const render = join(work, "icon.png");
  run(ictool, [
    source, "--export-image", "--output-file", render,
    "--platform", "macOS", "--rendition", "Default", "--width", "824", "--height", "824", "--scale", "1",
  ]);
  run("sips", ["--padToHeightWidth", "1024", "1024", render, "--out", join(root, "assets/icon.png")]);
  tauri("icon", "assets/icon.png", "-o", "src-tauri/icons");
  for (const mobile of ["android", "ios"]) rmSync(join(icons, mobile), { recursive: true, force: true });

  // Named Icon so the app icon in Assets.car is called Icon
  const icon = join(work, "Icon.icon");
  run("cp", ["-R", source, icon]);
  run("xcrun", [
    "actool", icon, "--compile", work, "--output-format", "human-readable-text", "--errors",
    "--output-partial-info-plist", join(work, "Info.plist"), "--app-icon", "Icon", "--include-all-app-icons",
    "--target-device", "mac", "--platform", "macosx", "--minimum-deployment-target", "11.0",
  ]);
  copyFileSync(join(work, "Assets.car"), join(icons, "Assets.car"));
} finally {
  rmSync(work, { recursive: true, force: true });
}

const windows = join(icons, ".windows");
tauri("icon", "assets/icon-windows.png", "-o", windows);
copyFileSync(join(windows, "icon.ico"), join(icons, "icon.ico"));
rmSync(windows, { recursive: true, force: true });
