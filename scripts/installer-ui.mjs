// Builds the Windows installer's window (src-tauri/installer/ui) into
// src-tauri/target/installer-ui, where installer.nsi picks it up. Tauri runs
// this before bundling (beforeBundleCommand). Does nothing on macOS
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

if (process.platform === "win32") {
  const root = fileURLToPath(new URL("..", import.meta.url));
  execFileSync(
    "cargo",
    [
      "build",
      "--release",
      "--locked",
      "--manifest-path", join(root, "src-tauri", "installer", "ui", "Cargo.toml"),
      "--target-dir", join(root, "src-tauri", "target", "installer-ui"),
    ],
    { stdio: "inherit" },
  );
}
