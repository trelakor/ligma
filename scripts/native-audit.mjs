// Compares the experimental native renderer with Figma's own canvas, view
// by view, and lists where they differ and which layers are there.
//
// Needs a dev build with the native renderer on and WebView2's remote
// debugging open, as `LIGMA_NATIVE_RENDERER=1 LIGMA_NATIVE_DEV=1` and
// `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`, with
// a Figma Design file open. It moves Figma's view and clears the selection
// while it runs, and puts both back after
//
//   node scripts/native-audit.mjs                     the view as it is
//   node scripts/native-audit.mjs --frames 20         20 top-level layers, fit and at 100%
//   node scripts/native-audit.mjs --ids 1:2,3:4       those layers
//   node scripts/native-audit.mjs --zooms fit,1,4     zooms for each layer ("fit" or a zoom)
//
// Each view's figma, native and diff images, a crop of each region that
// differs, and report.json go to Ligma's log folder, under native-renderer.
// The summary goes to audit-summary.json there and is printed
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const port = Number(option("port", 9222));
const frames = Number(option("frames", 0));
const ids = option("ids", "").split(",").filter(Boolean);
const zooms = option("zooms", "fit,1").split(",");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function connect(match) {
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const target = targets.find((t) => t.type === "page" && t.url.includes(match));
  if (!target) throw new Error(`no page matching ${match}`);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve);
    ws.addEventListener("error", reject);
  });
  let id = 0;
  const pending = new Map();
  ws.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      pending.get(message.id)(message);
      pending.delete(message.id);
    }
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const n = ++id;
      pending.set(n, (message) => (message.error ? reject(new Error(message.error.message)) : resolve(message.result)));
      ws.send(JSON.stringify({ id: n, method, params }));
    });
  const evaluate = async (body) => {
    const result = await send("Runtime.evaluate", {
      expression: `(async () => { ${body} })()`,
      awaitPromise: true,
      returnByValue: true,
      timeout: 600000,
    });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails.exception?.value ?? result.exceptionDetails.text));
    return result.result.value;
  };
  return { send, evaluate, close: () => ws.close() };
}

const figma = await connect("figma.com/design");
const native = await connect("native.html");

// Figma's canvas as drawn: its buffer, read back, which leaves out the
// interface over it and the browser's resampling when it's composited at a
// fraction of a pixel
async function screenshot() {
  const view = await figma.evaluate("return window.__ligmaNative.view()");
  const data = await figma.evaluate('return document.querySelector("canvas").toDataURL("image/png").split(",")[1]');
  const canvas = await figma.evaluate("const c = document.querySelector('canvas'); return { width: c.width, height: c.height }");
  return { view, data, crop: [0, 0, canvas.width, canvas.height] };
}

// A screenshot once Figma has stopped drawing: two in a row the same
async function settled() {
  await sleep(500);
  let last = await screenshot();
  for (let i = 0; i < 12; i++) {
    await sleep(400);
    const next = await screenshot();
    if (next.data === last.data) return next;
    last = next;
  }
  return last;
}

const saved = await figma.evaluate(
  "return { center: figma.viewport.center, zoom: figma.viewport.zoom, selection: figma.currentPage.selection.map((n) => n.id) }",
);
const views = [];
if (frames || ids.length) {
  const picked = ids.length
    ? ids
    : await figma.evaluate(`
        const out = [];
        const add = (parent) => {
          for (const n of parent.children) {
            if (n.visible === false || !n.absoluteBoundingBox) continue;
            if (n.type === "SECTION") add(n);
            else out.push(n.id);
          }
        };
        add(figma.currentPage);
        const step = Math.max(1, out.length / ${frames});
        return Array.from({ length: Math.min(${frames}, out.length) }, (_, i) => out[Math.floor(i * step)]);
      `);
  for (const id of picked) for (const zoom of zooms) views.push({ id, zoom });
} else {
  views.push(null);
}

const shots = mkdtempSync(join(tmpdir(), "ligma-audit-"));
const reports = [];
try {
  await figma.evaluate("figma.currentPage.selection = []");
  // Keep Figma's hover outline off the canvas
  await figma.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 2, y: 2 });
  for (const [i, target] of views.entries()) {
    if (target) {
      const moved = await figma.evaluate(`
        const n = figma.getNodeById(${JSON.stringify(target.id)});
        if (!n) return false;
        if (${JSON.stringify(target.zoom)} === "fit") figma.viewport.scrollAndZoomIntoView([n]);
        else {
          const b = n.absoluteBoundingBox;
          figma.viewport.zoom = ${Number(target.zoom) || 1};
          figma.viewport.center = { x: b.x + b.width / 2, y: b.y + b.height / 2 };
        }
        return true;
      `);
      if (!moved) continue;
    }
    const { view, data, crop } = await settled();
    const name = target ? `${target.id.replace(/[:;]/g, "-")}-${target.zoom}` : "current";
    const path = join(shots, `${i}-${name}.png`);
    writeFileSync(path, Buffer.from(data, "base64"));
    const spec = JSON.stringify({ view, figma: path, crop, name });
    const report = JSON.parse(
      await native.evaluate(`return await window.__TAURI_INTERNALS__.invoke("native_action", { action: ${JSON.stringify(`audit:${spec}`)} })`),
    );
    reports.push(report);
    console.log(
      `${name}: ${(report.exact * 100).toFixed(2)}% exact, ${(report.matched * 100).toFixed(2)}% within 6, ${report.regions.length} regions${report.imagesWaiting ? `, ${report.imagesWaiting} images missing` : ""}`,
    );
  }
} finally {
  await figma.evaluate(`
    figma.viewport.zoom = ${saved.zoom};
    figma.viewport.center = ${JSON.stringify(saved.center)};
    figma.currentPage.selection = ${JSON.stringify(saved.selection)}.map((id) => figma.getNodeById(id)).filter(Boolean);
  `);
}

// The worst regions over every view, with their layers' names
const regions = reports
  .flatMap((report) => report.regions.map((region) => ({ ...region, view: report.name, dir: report.dir })))
  .sort((a, b) => b.pixels - a.pixels);
const named = await figma.evaluate(`
  const ids = ${JSON.stringify([...new Set(regions.slice(0, 60).flatMap((region) => region.nodes))])};
  return Object.fromEntries(ids.map((id) => { const n = figma.getNodeById(id); return [id, n ? n.type + " " + n.name : "?"]; }));
`);
const summary = {
  views: reports.map(({ name, dir, exact, matched, meanError, imagesWaiting, regions }) => ({
    name,
    dir,
    exact,
    matched,
    meanError,
    imagesWaiting,
    regions: regions.length,
  })),
  worst: regions.slice(0, 60).map((region) => ({
    view: region.view,
    image: region.image && join(region.dir, region.image),
    pixels: region.pixels,
    meanError: Math.round(region.meanError * 10) / 10,
    maxError: region.maxError,
    world: region.world.map((v) => Math.round(v)),
    layers: region.nodes.map((id) => `${id} ${named[id] ?? ""}`),
  })),
};
const out = reports[0] ? join(reports[0].dir, "..", "audit-summary.json") : join(shots, "audit-summary.json");
writeFileSync(out, JSON.stringify(summary, null, 2));
console.log(`\nWorst regions (${out}):`);
for (const region of summary.worst.slice(0, 20)) {
  console.log(`  ${region.pixels} px, mean ${region.meanError}, max ${region.maxError} in ${region.view}: ${region.layers.at(-1) ?? "background"}`);
}
figma.close();
native.close();
