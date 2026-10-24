// Prints the GitHub release notes for a version: its CHANGELOG.md section,
// then install steps naming that version's files. Exits with an error if
// CHANGELOG.md has no section for the version.
//
//   node scripts/release-notes.mjs 0.5.0
//   node scripts/release-notes.mjs   # version from src-tauri/tauri.conf.json

import { readFileSync } from "node:fs";

const root = new URL("..", import.meta.url);
const version =
  process.argv[2] ?? JSON.parse(readFileSync(new URL("src-tauri/tauri.conf.json", root))).version;

// The section runs from "## <version>" to the next "## " heading.
const changelog = readFileSync(new URL("CHANGELOG.md", root), "utf8");
const escaped = version.replaceAll(".", "\\.");
const section = changelog.match(new RegExp(`^## ${escaped}(?: .*)?\\n([\\s\\S]*?)(?=^## |(?![\\s\\S]))`, "m"));
const changes = section?.[1].trim();
if (!changes) {
  console.error(`CHANGELOG.md has no section for ${version}. Add "## ${version} - <date>" with its changes.`);
  process.exit(1);
}

// Ligma updates itself from 0.5.0 on.
const [major, minor] = version.split(".").map(Number);
const selfUpdating = major > 0 || minor >= 5;
const firstSelfUpdating = version === "0.5.0";

const updating = selfUpdating
  ? firstSelfUpdating
    ? `
**Updating**: earlier versions can't update themselves, so install ${version} once by hand. From then on Ligma updates itself.
`
    : `
**Updating**: Ligma 0.5.0 or later updates itself to ${version}. It offers to restart when the update is ready. Older versions need to install ${version} once by hand.
`
  : "";

console.log(`## What's new in ${version}

${changes}

## Install

**macOS** (Apple Silicon and Intel): download \`Ligma_${version}_universal.dmg\` and drag Ligma to Applications.
The app is not notarized, so run \`xattr -dr com.apple.quarantine /Applications/Ligma.app\` once, or allow it in System Settings → Privacy & Security → Open Anyway.

**Windows**: download \`Ligma_${version}_x64-setup.exe\`.
The installer is not signed. If SmartScreen warns, choose More info → Run anyway.
${updating}`);
