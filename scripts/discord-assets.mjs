// Checks that the Discord application has an art asset for every tool icon
// Rich Presence asks for: `<product>-<focused|unfocused>-<slug>`, from
// TOOLS_PRODUCT and `tool_icon` in src-tauri/src/presence.rs. Upload missing
// ones in the Developer Portal (Rich Presence > Art Assets), with the key as
// the file name. Discord lowercases keys.
//
//   npm run discord-assets

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const source = readFileSync(join(root, "src-tauri", "src", "presence.rs"), "utf8");

const clientId = source.match(/const CLIENT_ID: &str = "(\d+)";/)[1];
const product = source.match(/const TOOLS_PRODUCT: &str = "([^"]+)";/)[1];
const toolIcon = source.match(/fn tool_icon[\s\S]*?\n}\n/)[0];
const slugs = [...toolIcon.matchAll(/=> "([^"]+)",/g)].map((match) => match[1]);
const wanted = ["focused", "unfocused"].flatMap((state) =>
  slugs.map((slug) => `${product}-${state}-${slug}`),
);

// Discord's CDN caches the list for up to 10 minutes; a fresh query skips it.
const response = await fetch(
  `https://discord.com/api/v10/oauth2/applications/${clientId}/assets?t=${Date.now()}`,
);
if (!response.ok) throw new Error(`Discord answered ${response.status}`);
const uploaded = new Set((await response.json()).map((asset) => asset.name));

const missing = wanted.filter((key) => !uploaded.has(key));
const others = [...uploaded].filter((key) => key.startsWith(`${product}-`) && !wanted.includes(key));
console.log(`${wanted.length - missing.length} of ${wanted.length} ${product} tool icons uploaded.`);
if (missing.length) console.log(`Missing:\n  ${missing.join("\n  ")}`);
if (others.length) console.log(`Uploaded but unused:\n  ${others.join("\n  ")}`);
process.exitCode = missing.length ? 1 : 0;
