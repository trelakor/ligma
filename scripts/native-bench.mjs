// Runs the native renderer's benchmark in the file native-bench-file.mjs
// made: for each section, a jump to it, wheel pans slow and fast, and pinch
// zooms, and sums what the renderer traced meanwhile: how smooth what was
// shown moved, how long the view took to be whole and sharp, and the GPU
// memory Ligma took.
//
// Needs a dev build with the native renderer on, tracing
// (LIGMA_NATIVE_TRACE=1) and WebView2's remote debugging open, as in
// native-audit.mjs, with the benchmark file open in the tab on screen
//
//   node scripts/native-bench.mjs --file <key>
//   node scripts/native-bench.mjs --file <key> --only images --steps jump,pan
//   node scripts/native-bench.mjs --file <key> --out result.json
//   node scripts/native-bench.mjs --file <key> --keep <dir>  each step's trace too
//
// Prints a table and writes every number to --out
import { execFileSync, spawn } from "node:child_process";
import { closeSync, openSync, readSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const port = Number(option("port", 9222));
const file = option("file", "");
const only = option("only", "").split(",").filter(Boolean);
const steps = option("steps", "jump,pan,fastpan,zoom").split(",");
const out = option("out", "");
const refresh = Number(option("hz", 0));
const keep = option("keep", "");
if (!file) throw new Error("--file <key> is needed, the benchmark file's key from its URL");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const target = targets.find((t) => t.type === "page" && t.url.includes(`/design/${file}`));
if (!target) throw new Error(`no tab with file ${file}`);
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
const fire = (method, params) => ws.send(JSON.stringify({ id: ++id, method, params }));
const evaluate = async (body) => {
  const result = await send("Runtime.evaluate", { expression: `(async () => { ${body} })()`, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text));
  return result.result.value;
};

// The screen's refresh, from Windows, for what counts as a missed frame
const hz =
  refresh ||
  Math.max(
    60,
    ...execFileSync("powershell", ["-NoProfile", "-Command", "(Get-CimInstance Win32_VideoController).CurrentRefreshRate"], { encoding: "utf8" })
      .split(/\s+/)
      .map(Number)
      .filter(Boolean),
  );
const frame = 1000 / hz;

// Ligma's GPU memory, sampled each second all along
const pid = execFileSync("powershell", ["-NoProfile", "-Command", "(Get-Process ligma | Sort-Object WorkingSet64 -Descending | Select-Object -First 1).Id"], { encoding: "utf8" }).trim();
const samples = [];
const sampler = spawn("powershell", [
  "-NoProfile",
  "-Command",
  `while ($true) { $c = (Get-Counter "\\GPU Process Memory(pid_${pid}_*)\\Local Usage" -ErrorAction SilentlyContinue).CounterSamples; [Console]::Out.WriteLine([string][int](($c | Measure-Object CookedValue -Sum).Sum / 1MB)); [Console]::Out.Flush(); Start-Sleep -Milliseconds 500 }`,
]);
sampler.stdout.on("data", (data) => {
  for (const line of String(data).split(/\r?\n/).filter(Boolean)) samples.push([Date.now(), Number(line)]);
});
const vram = (from, to) => {
  const within = samples.filter(([at]) => at >= from && at <= to).map(([, mb]) => mb);
  return { peak: Math.max(0, ...within), last: within.at(-1) ?? 0 };
};

const trace = join(process.env.TEMP ?? tmpdir(), "ligma-trace.txt");
const mark = () => statSync(trace).size;
const since = (offset) => {
  const size = statSync(trace).size;
  const buffer = Buffer.alloc(Math.max(0, size - offset));
  const fd = openSync(trace, "r");
  readSync(fd, buffer, 0, buffer.length, offset);
  closeSync(fd);
  return buffer.toString("utf8").split(/\r?\n/);
};

// What the trace says of the frames shown: when, where the view was, how
// much of it was coarser than it should be, and whether it had holes; and
// of Figma's frames: when each was sent, and the page's long tasks
const presents = (lines) => {
  const shown = [];
  const sent = [];
  const long = [];
  let held = 0;
  let late = 0;
  let notAhead = 0;
  const slow = [];
  let images = 0;
  let imageMb = 0;
  const tiles = [];
  for (const line of lines) {
    const m = line.match(/present seq=\d+ at=([\d.]+) px=([\d.]+) .*? x=(-?\d+) y=(-?\d+) .*?coarse=(-?[\d.]+) coarsest=(-?[\d.]+) serial=(\d+) path=(\d+)/);
    if (m) {
      shown.push({ at: Number(m[1]), px: Number(m[2]), x: Number(m[3]), y: Number(m[4]), coarse: Math.max(0, Number(m[5])), coarsest: Number(m[6]), serial: Number(m[7]), path: Number(m[8]) });
      continue;
    }
    const came = line.match(/frame \d+ came [\d.]+ms after Figma sent it .*?sent=([\d.]+) page=(.*)$/);
    if (came) {
      sent.push(Number(came[1]));
      try {
        for (const task of JSON.parse(came[2]).long ?? []) long.push(task.ms);
      } catch {}
      continue;
    }
    if (line.includes("holes=true") || line.includes("present held")) held++;
    if (line.includes("shown late:")) late++;
    if (line.includes("present not ahead")) notAhead++;
    const tile = line.match(/ vello tiles px=\S+ at \S+ \S+ exact=\S+ rendered=([\d.]+)ms/);
    if (tile) tiles.push(Number(tile[1]));
    const pass = line.match(/slow render pass (\d+)ms/);
    if (pass) slow.push(Number(pass[1]));
    if (/ image [0-9a-f]{40}\S* \d+x\d+ areas/.test(line)) images++;
    const mb = line.match(/images (\d+) MB/);
    if (mb) imageMb = Number(mb[1]);
  }
  return { shown, sent, long, held, late, notAhead, slow, images, imageMb, tiles };
};

const percentile = (sorted, p) => (sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] : 0);
const round = (n, places = 1) => Math.round(n * 10 ** places) / 10 ** places;
const gapsOf = (times) => times.slice(1).map((at, i) => at - times[i]);

// Smoothness while the input moved the view, from `from` to `to`: the time
// between frames that showed the view moved, those that took longer than
// one of the screen's refreshes and half, and over 50 ms, as a hitch is
// seen; the same of Figma's frames, as what Figma's slow to send can't be
// shown sooner; how unevenly the view moved from one frame to the next, and
// how much of it was coarser than it should be
const motion = (traced, from, to) => {
  const shown = traced.shown.filter((s) => s.at >= from && s.at <= to + 100);
  const moves = shown.filter((s, i) => i === 0 || s.x !== shown[i - 1].x || s.y !== shown[i - 1].y || s.px !== shown[i - 1].px);
  const gaps = gapsOf(moves.map((s) => s.at));
  const sorted = [...gaps].sort((a, b) => a - b);
  const mean = gaps.reduce((a, b) => a + b, 0) / Math.max(1, gaps.length);
  const figma = gapsOf(traced.sent.filter((at) => at >= from && at <= to));
  const speeds = [];
  for (let i = 1; i < moves.length; i++) {
    const gap = moves[i].at - moves[i - 1].at;
    if (moves[i].px === moves[i - 1].px && gap > 0) speeds.push(Math.hypot(moves[i].x - moves[i - 1].x, moves[i].y - moves[i - 1].y) / gap);
  }
  let jerk = 0;
  for (let i = 1; i + 1 < speeds.length; i++) jerk += Math.abs(speeds[i] - (speeds[i - 1] + speeds[i + 1]) / 2);
  const speed = speeds.reduce((a, b) => a + b, 0) / Math.max(1, speeds.length);
  return {
    frames: moves.length,
    fps: round(1000 / Math.max(mean, 0.001)),
    p50: round(percentile(sorted, 0.5)),
    p95: round(percentile(sorted, 0.95)),
    p99: round(percentile(sorted, 0.99)),
    max: round(sorted.at(-1) ?? 0),
    missed: gaps.filter((gap) => gap > frame * 1.5).length,
    hitches: gaps.filter((gap) => gap > 50).length,
    figmaFps: round(1000 / Math.max(figma.reduce((a, b) => a + b, 0) / Math.max(1, figma.length), 0.001)),
    figmaHitches: figma.filter((gap) => gap > 50).length,
    figmaMax: round(Math.max(0, ...figma)),
    longTasks: traced.long.length,
    longest: Math.max(0, ...traced.long),
    jerk: round(speeds.length > 2 ? jerk / (speeds.length - 2) / Math.max(speed, 0.001) : 0, 3),
    coarse: round(shown.reduce((a, s) => a + s.coarse, 0) / Math.max(1, shown.length), 3),
    // Frames with a part shown at under half the detail it should have,
    // as a tile still being drawn is
    blurry: round(shown.filter((s) => s.coarse > 0.02 && s.coarsest < 0.5).length / Math.max(1, shown.length), 3),
    ...tileTimes(traced.tiles),
    held: traced.held,
    late: traced.late,
    slowPasses: traced.slow.length,
    slowest: Math.max(0, ...traced.slow),
  };
};

// How long tiles took to draw, the middle one and the longest
const tileTimes = (tiles) => {
  const sorted = [...tiles].sort((a, b) => a - b);
  return { tiles: tiles.length, tileP50: round(percentile(sorted, 0.5)), tileMax: round(sorted.at(-1) ?? 0) };
};

// How long the view took to settle after a jump: to the first frame shown,
// to one with nothing coarse, and to the last that changed what's shown
const settling = (traced, from) => {
  const { shown } = traced;
  const first = shown[0]?.at;
  const sharp = shown.find((s, i) => s.coarse <= 0 && shown.slice(i).every((later) => later.coarse <= 0))?.at;
  const last = shown.at(-1)?.at;
  return {
    firstMs: first ? Math.round(first - from) : null,
    sharpMs: sharp ? Math.round(sharp - from) : null,
    doneMs: last ? Math.round(last - from) : null,
    frames: shown.length,
    held: traced.held,
    images: traced.images,
    imageMb: traced.imageMb,
    slowPasses: traced.slow.length,
    slowest: Math.max(0, ...traced.slow),
    ...tileTimes(traced.tiles),
  };
};

const sections = JSON.parse((await evaluate(`return figma.root.getSharedPluginData("ligma", "bench")`)) || "{}");
const names = Object.keys(sections).filter((name) => !only.length || only.includes(name));
if (!names.length) throw new Error("no sections, build the file with native-bench-file.mjs first");
const saved = await evaluate("return { center: figma.viewport.center, zoom: figma.viewport.zoom, selection: figma.currentPage.selection.map((n) => n.id) }");
await evaluate("figma.currentPage.selection = []; return true");
const box = await evaluate(`
  const c = document.querySelector(".gpu-view-content canvas") ?? document.querySelector("canvas");
  const r = (c.parentElement ?? c).getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
`);
// Keep Figma's hover outline off the canvas
fire("Input.dispatchMouseEvent", { type: "mouseMoved", x: 2, y: 2 });

// Wheel events every 8 ms for `ms`, `per` pixels each, the direction
// turning every 700 ms, as a trackpad pans; with ctrl, a pinch
const wheel = async (ms, per, zoom) => {
  const start = Date.now();
  while (Date.now() - start < ms) {
    const sign = Math.floor((Date.now() - start) / 700) % 2 ? -1 : 1;
    // A pinch in first, so it never meets Figma's least zoom, where the
    // view would stand still
    fire("Input.dispatchMouseEvent", zoom
      ? { type: "mouseWheel", x: box.x, y: box.y, deltaX: 0, deltaY: -per * sign, modifiers: 2 }
      : { type: "mouseWheel", x: box.x, y: box.y, deltaX: per * sign * 0.8, deltaY: per * sign * 0.6 });
    await sleep(8);
  }
};
const fit = (name) => evaluate(`figma.viewport.scrollAndZoomIntoView([figma.getNodeById(${JSON.stringify(sections[name])})]); return figma.viewport.zoom`);

const results = {};
await sleep(1500);
try {
  for (const name of names) {
    const result = (results[name] = {});
    for (const step of steps) {
      // Each step from the section fit to the view, settled
      await fit(name);
      await sleep(step === "jump" ? 300 : 4000);
      if (step === "jump") {
        // From another section, so what's there isn't drawn already
        const other = Object.keys(sections).find((n) => n !== name) ?? name;
        await fit(other);
        await sleep(4000);
      }
      const offset = mark();
      const from = Date.now();
      if (step === "jump") {
        await fit(name);
        await sleep(15000);
      } else if (step === "pan") await wheel(4000, 6);
      else if (step === "fastpan") await wheel(3000, 40);
      else if (step === "zoom") await wheel(2800, 12, true);
      else if (step === "deep") {
        // In close at 100%, then panned there
        await evaluate(`figma.viewport.zoom = 1; return true`);
        await sleep(3000);
        await wheel(3000, 10);
      }
      const to = Date.now();
      if (step !== "jump") await sleep(600);
      const lines = since(offset);
      if (keep) writeFileSync(join(keep, `${name}-${step}.txt`), lines.join("\n"));
      const traced = presents(lines);
      result[step] = { ...(step === "jump" ? settling(traced, from) : motion(traced, from, to)), vram: vram(from, Date.now()) };
      console.log(name.padEnd(8), step.padEnd(8), JSON.stringify(result[step]));
    }
  }
} finally {
  await evaluate(`
    figma.viewport.zoom = ${saved.zoom};
    figma.viewport.center = ${JSON.stringify(saved.center)};
    figma.currentPage.selection = ${JSON.stringify(saved.selection)}.map((id) => figma.getNodeById(id)).filter(Boolean);
    return true;
  `);
  sampler.kill();
  ws.close();
}

const summary = { at: new Date().toISOString(), hz, results };
if (out) writeFileSync(out, JSON.stringify(summary, null, 2));
const rows = [];
for (const [name, result] of Object.entries(results)) {
  for (const [step, r] of Object.entries(result)) {
    rows.push(
      step === "jump"
        ? `${name.padEnd(8)} ${step.padEnd(8)} first ${r.firstMs}ms sharp ${r.sharpMs}ms done ${r.doneMs}ms held ${r.held} slow ${r.slowPasses}/${r.slowest}ms tiles ${r.tiles} p50 ${r.tileP50} max ${r.tileMax}ms images ${r.imageMb}MB vram ${r.vram.peak}MB`
        : `${name.padEnd(8)} ${step.padEnd(8)} ${r.fps}fps p95 ${r.p95} p99 ${r.p99} max ${r.max}ms missed ${r.missed} hitches ${r.hitches} | figma ${r.figmaFps}fps hitches ${r.figmaHitches} max ${r.figmaMax} long ${r.longTasks}/${r.longest}ms | jerk ${r.jerk} coarse ${r.coarse} blurry ${r.blurry} held ${r.held} slow ${r.slowPasses}/${r.slowest}ms tiles ${r.tiles} p50 ${r.tileP50} max ${r.tileMax}ms vram ${r.vram.peak}MB`,
    );
  }
}
console.log(rows.join("\n"));
process.exit(0);
