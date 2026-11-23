// Fills a Figma Design file with the native renderer's benchmark: many
// distinct images, tens of thousands of layers, effects of every kind and
// the edge cases that show seams, each in its own section of one page.
//
// Needs WebView2's remote debugging open, as in native-audit.mjs, and a new,
// empty file open in a tab. It only writes to a file
// it made before or one with nothing in it, and makes again the sections it's
// asked to
//
//   node scripts/native-bench-file.mjs --file <key>           builds it all
//   node scripts/native-bench-file.mjs --file <key> --only images,effects
//
// The sections' ids go to the file's root as shared plugin data, under
// "ligma" and "bench", for native-bench.mjs
const args = process.argv.slice(2);
const option = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const port = Number(option("port", 9222));
const file = option("file", "");
const only = option("only", "").split(",").filter(Boolean);
if (!file) throw new Error("--file <key> is needed, the benchmark file's key from its URL");

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
const evaluate = async (body) => {
  const result = await send("Runtime.evaluate", {
    expression: `(async () => { ${body} })()`,
    awaitPromise: true,
    returnByValue: true,
    timeout: 600000,
  });
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text));
  return result.result.value;
};

const name = await evaluate("return figma.root.name");
const empty = await evaluate("return figma.root.children.every((page) => page.children.length === 0)");
const marked = await evaluate(`return figma.root.getSharedPluginData("ligma", "bench") !== ""`);
if (!marked && !empty) throw new Error(`"${name}" isn't the benchmark file nor empty, left as it is`);

// Helpers kept on the page between calls
await evaluate(`
  const seeded = (seed) => () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 4294967296);
  window.__bench = {
    seeded,
    async section(name, x, y, w, h) {
      const page = figma.currentPage;
      for (const old of page.children.filter((n) => n.name === name)) old.remove();
      const section = figma.createSection();
      section.name = name;
      section.x = x;
      section.y = y;
      section.resizeWithoutConstraints(w, h);
      page.appendChild(section);
      return section;
    },
    // An image like a photo: soft shapes over a gradient, edges and grain,
    // its number written large so views are told apart
    async photo(n, width, height, alpha) {
      const random = seeded(n * 7919 + 17);
      const canvas = new OffscreenCanvas(width, height);
      const c = canvas.getContext("2d");
      const hue = Math.floor(random() * 360);
      if (!alpha) {
        const g = c.createLinearGradient(0, 0, width, height);
        g.addColorStop(0, "hsl(" + hue + ",60%,55%)");
        g.addColorStop(1, "hsl(" + ((hue + 80) % 360) + ",50%,25%)");
        c.fillStyle = g;
        c.fillRect(0, 0, width, height);
      }
      c.filter = "blur(" + Math.round(width / 60) + "px)";
      for (let i = 0; i < 40; i++) {
        c.fillStyle = "hsla(" + Math.floor(random() * 360) + ",70%," + Math.floor(30 + random() * 50) + "%," + (alpha ? 0.8 : 0.6) + ")";
        c.beginPath();
        c.ellipse(random() * width, random() * height, (0.05 + random() * 0.25) * width, (0.05 + random() * 0.25) * height, random() * 3, 0, Math.PI * 2);
        c.fill();
      }
      c.filter = "none";
      c.strokeStyle = "rgba(255,255,255,0.8)";
      c.lineWidth = Math.max(1, width / 800);
      for (let i = 0; i < 30; i++) {
        c.beginPath();
        c.moveTo(random() * width, random() * height);
        c.lineTo(random() * width, random() * height);
        c.stroke();
      }
      c.fillStyle = alpha ? "rgba(0,0,0,0.9)" : "white";
      c.font = "bold " + Math.round(height / 4) + "px sans-serif";
      c.textAlign = "center";
      c.textBaseline = "middle";
      c.fillText(String(n), width / 2, height / 2);
      if (!alpha) {
        const data = c.getImageData(0, 0, width, height);
        const px = data.data;
        for (let i = 0; i < px.length; i += 4) {
          const grain = (random() - 0.5) * 18;
          px[i] += grain;
          px[i + 1] += grain;
          px[i + 2] += grain;
        }
        c.putImageData(data, 0, 0);
      }
      const blob = await canvas.convertToBlob(alpha ? { type: "image/png" } : { type: "image/jpeg", quality: 0.82 });
      return figma.createImage(new Uint8Array(await blob.arrayBuffer())).hash;
    },
  };
  return true;
`);

const ids = (await evaluate(`return figma.root.getSharedPluginData("ligma", "bench")`)) || "{}";
const sections = JSON.parse(ids);
const save = async () => {
  await evaluate(`figma.root.setSharedPluginData("ligma", "bench", ${JSON.stringify(JSON.stringify(sections))}); return true`);
};
const want = (name) => !only.length || only.includes(name);
const log = (...what) => console.log(new Date().toISOString().slice(11, 19), ...what);

await save();
await evaluate(`figma.currentPage.name = "Benchmark"; return true`);
await evaluate(`await figma.loadFontAsync({ family: "Inter", style: "Regular" }); await figma.loadFontAsync({ family: "Inter", style: "Bold" }); return true`);

// Distinct images, by size: most a few megapixels as photos are, some huge,
// some small, some with transparency
const sizes = [
  ...Array(40).fill([512, 384]),
  ...Array(80).fill([1024, 768]),
  ...Array(80).fill([2048, 1536]),
  ...Array(30).fill([3000, 2000]),
  ...Array(10).fill([4096, 3072]),
  ...Array(20).fill([768, 768, true]),
];
let hashes = JSON.parse((await evaluate(`return figma.root.getSharedPluginData("ligma", "hashes")`)) || "[]");
if (want("images") || want("huge") || want("effects")) {
  for (let n = hashes.length; n < sizes.length; n += 10) {
    const batch = sizes.slice(n, n + 10).map(([w, h, alpha], i) => [n + i, w, h, !!alpha]);
    const made = await evaluate(`
      const out = [];
      for (const [n, w, h, alpha] of ${JSON.stringify(batch)}) out.push(await window.__bench.photo(n, w, h, alpha));
      return out;
    `);
    hashes.push(...made);
    await evaluate(`figma.root.setSharedPluginData("ligma", "hashes", ${JSON.stringify(JSON.stringify(hashes))}); return true`);
    log(`images ${hashes.length}/${sizes.length}`);
  }
}

// 1200 image fills, 48 by 25, of every scale mode, some turned, filtered,
// rounded or see-through, each image used five times over
if (want("images")) {
  sections.images = await evaluate(`
    const hashes = ${JSON.stringify(hashes)};
    const random = window.__bench.seeded(1);
    const section = await window.__bench.section("Images", 0, 0, 48 * 500 + 200, 25 * 380 + 200);
    const modes = ["FILL", "FILL", "FILL", "FIT", "CROP", "TILE"];
    for (let row = 0; row < 25; row++) {
      for (let col = 0; col < 48; col++) {
        const i = row * 48 + col;
        const r = figma.createRectangle();
        r.name = "image " + i;
        const w = 300 + (i % 5) * 40;
        r.resize(w, w * 0.7);
        r.x = 100 + col * 500;
        r.y = 100 + row * 380;
        const mode = modes[i % modes.length];
        const fill = { type: "IMAGE", imageHash: hashes[i % hashes.length], scaleMode: mode };
        if (mode === "TILE") fill.scalingFactor = 0.25;
        if (mode === "CROP") fill.imageTransform = [[0.5, 0, 0.2], [0, 0.5, 0.3]];
        if (i % 11 === 0) fill.filters = { exposure: 0.3, contrast: 0.2, saturation: -0.5, temperature: 0.3, tint: 0, highlights: 0, shadows: 0 };
        r.fills = [fill];
        if (i % 3 === 0) r.cornerRadius = 24;
        if (i % 13 === 0) r.opacity = 0.6;
        if (i % 7 === 0) r.rotation = (random() - 0.5) * 30;
        if (i % 17 === 0) {
          r.strokes = [{ type: "SOLID", color: { r: 1, g: 1, b: 1 } }];
          r.strokeWeight = 4;
        }
        section.appendChild(r);
      }
    }
    return section.id;
  `);
  await save();
  log("images", sections.images);
}

// The biggest images, shown big, and many tiny ones
if (want("huge")) {
  sections.huge = await evaluate(`
    const hashes = ${JSON.stringify(hashes)};
    const section = await window.__bench.section("Huge images", 0, 10000, 24000, 8000);
    const big = hashes.slice(220, 230);
    for (let i = 0; i < big.length; i++) {
      const r = figma.createRectangle();
      r.name = "huge " + i;
      r.resize(4096, 3072);
      r.x = 100 + (i % 5) * 4300;
      r.y = 100 + Math.floor(i / 5) * 3300;
      r.fills = [{ type: "IMAGE", imageHash: big[i], scaleMode: "FILL" }];
      section.appendChild(r);
    }
    for (let i = 0; i < 2000; i++) {
      const r = figma.createRectangle();
      r.name = "tiny " + i;
      r.resize(24, 18);
      r.x = 100 + (i % 100) * 30;
      r.y = 6800 + Math.floor(i / 100) * 24;
      r.fills = [{ type: "IMAGE", imageHash: hashes[i % 120], scaleMode: "FILL" }];
      section.appendChild(r);
    }
    return section.id;
  `);
  await save();
  log("huge", sections.huge);
}

// A component of a card, its instances 100 by 60 with some text changed,
// and pages of text, tens of thousands of layers
if (want("vectors")) {
  sections.vectors = await evaluate(`
    const random = window.__bench.seeded(2);
    const section = await window.__bench.section("Vectors and text", 25000, 0, 100 * 220 + 200, 60 * 140 + 3400);
    const card = figma.createComponent();
    card.name = "card";
    card.resize(200, 120);
    card.cornerRadius = 12;
    card.fills = [{ type: "GRADIENT_LINEAR", gradientTransform: [[1, 0, 0], [0, 1, 0]], gradientStops: [{ position: 0, color: { r: 0.95, g: 0.95, b: 1, a: 1 } }, { position: 1, color: { r: 0.8, g: 0.85, b: 0.95, a: 1 } }] }];
    card.strokes = [{ type: "SOLID", color: { r: 0.6, g: 0.6, b: 0.7 } }];
    card.strokeWeight = 1;
    card.clipsContent = true;
    const star = figma.createStar();
    star.resize(40, 40);
    star.x = 12; star.y = 12;
    star.pointCount = 5;
    star.innerRadius = 0.4;
    star.fills = [{ type: "SOLID", color: { r: 0.95, g: 0.6, b: 0.1 } }];
    card.appendChild(star);
    const poly = figma.createPolygon();
    poly.resize(30, 30);
    poly.x = 60; poly.y = 17;
    poly.pointCount = 6;
    poly.fills = [{ type: "GRADIENT_RADIAL", gradientTransform: [[1, 0, 0], [0, 1, 0]], gradientStops: [{ position: 0, color: { r: 0.2, g: 0.8, b: 0.6, a: 1 } }, { position: 1, color: { r: 0.1, g: 0.3, b: 0.6, a: 1 } }] }];
    card.appendChild(poly);
    const line = figma.createLine();
    line.resize(176, 0);
    line.x = 12; line.y = 64;
    line.strokes = [{ type: "SOLID", color: { r: 0.3, g: 0.3, b: 0.4 } }];
    line.dashPattern = [4, 3];
    card.appendChild(line);
    const title = figma.createText();
    title.fontName = { family: "Inter", style: "Bold" };
    title.characters = "Card title";
    title.fontSize = 14;
    title.x = 12; title.y = 72;
    card.appendChild(title);
    const body = figma.createText();
    body.characters = "Some body text in two lines that wraps";
    body.fontSize = 10;
    body.resize(176, 28);
    body.textAutoResize = "HEIGHT";
    body.x = 12; body.y = 92;
    card.appendChild(body);
    const vector = figma.createVector();
    vector.vectorPaths = [{ windingRule: "NONZERO", data: "M 0 20 C 10 0 20 40 30 20 C 40 0 50 40 60 20" }];
    vector.strokes = [{ type: "SOLID", color: { r: 0.8, g: 0.2, b: 0.4 } }];
    vector.strokeWeight = 2;
    vector.x = 110; vector.y = 12;
    card.appendChild(vector);
    card.x = 100; card.y = 100;
    section.appendChild(card);
    for (let row = 0; row < 60; row++) {
      for (let col = 0; col < 100; col++) {
        if (!row && !col) continue;
        const instance = card.createInstance();
        instance.x = 100 + col * 220;
        instance.y = 100 + row * 140;
        if ((row * 100 + col) % 9 === 0) instance.findOne((n) => n.type === "TEXT").characters = "Card " + (row * 100 + col);
        section.appendChild(instance);
      }
    }
    const words = "the quick brown fox jumps over a lazy dog while figma draws every glyph of it".split(" ");
    for (let i = 0; i < 400; i++) {
      const t = figma.createText();
      let s = "";
      for (let w = 0; w < 60; w++) s += words[Math.floor(random() * words.length)] + " ";
      t.characters = s;
      t.fontSize = 8 + (i % 6) * 2;
      t.resize(500, 100);
      t.textAutoResize = "HEIGHT";
      t.x = 100 + (i % 40) * 550;
      t.y = 60 * 140 + 200 + Math.floor(i / 40) * 300;
      if (i % 5 === 0) t.rotation = 15;
      section.appendChild(t);
    }
    return section.id;
  `);
  await save();
  log("vectors", sections.vectors);
}

// Shadows, blurs of the layer and of what's behind, progressive blur, noise
// and texture, every blend mode, masks and see-through groups, over images
if (want("effects")) {
  sections.effects = await evaluate(`
    const hashes = ${JSON.stringify(hashes)};
    const random = window.__bench.seeded(3);
    const section = await window.__bench.section("Effects", 25000, 12000, 16000, 10000);
    const solid = (r, g, b) => [{ type: "SOLID", color: { r, g, b } }];
    for (let i = 0; i < 300; i++) {
      const r = figma.createRectangle();
      r.name = "shadow " + i;
      r.resize(260, 160);
      r.x = 100 + (i % 20) * 380;
      r.y = 100 + Math.floor(i / 20) * 260;
      r.cornerRadius = 16;
      r.fills = solid(1, 1, 1);
      const effects = [{ type: "DROP_SHADOW", color: { r: 0, g: 0, b: 0, a: 0.25 }, offset: { x: 0, y: 8 }, radius: 24, spread: 0, visible: true, blendMode: "NORMAL" }];
      if (i % 2) effects.push({ type: "DROP_SHADOW", color: { r: 0.2, g: 0.3, b: 0.9, a: 0.4 }, offset: { x: 4, y: 4 }, radius: 4, spread: 2, visible: true, blendMode: "NORMAL" });
      if (i % 3 === 0) effects.push({ type: "INNER_SHADOW", color: { r: 0, g: 0, b: 0, a: 0.3 }, offset: { x: 0, y: 2 }, radius: 6, spread: 0, visible: true, blendMode: "NORMAL" });
      if (i % 10 === 5) effects.push({ type: "LAYER_BLUR", blurType: "NORMAL", radius: 8, visible: true });
      r.effects = effects;
      section.appendChild(r);
    }
    // Glass over images: blurs of what's behind
    for (let i = 0; i < 40; i++) {
      const x = 8000 + (i % 8) * 960, y = 100 + Math.floor(i / 8) * 760;
      const image = figma.createRectangle();
      image.resize(900, 700);
      image.x = x; image.y = y;
      image.fills = [{ type: "IMAGE", imageHash: hashes[i * 3], scaleMode: "FILL" }];
      section.appendChild(image);
      const glass = figma.createRectangle();
      glass.name = "glass " + i;
      glass.resize(600, 300);
      glass.x = x + 150; glass.y = y + 200;
      glass.cornerRadius = 32;
      glass.fills = [{ type: "SOLID", color: { r: 1, g: 1, b: 1 }, opacity: 0.2 }];
      glass.effects = [{ type: "BACKGROUND_BLUR", blurType: "NORMAL", radius: 10 + (i % 5) * 10, visible: true }];
      if (i % 4 === 1) glass.effects = [{ type: "BACKGROUND_BLUR", blurType: "PROGRESSIVE", radius: 40, startRadius: 0, startOffset: { x: 0.5, y: 0 }, endOffset: { x: 0.5, y: 1 }, visible: true }];
      if (i % 4 === 2) glass.rotation = 12;
      section.appendChild(glass);
    }
    // Progressive layer blur, noise, texture and glass, tried as Figma may not
    // take them
    const tried = [];
    for (let i = 0; i < 20; i++) {
      const r = figma.createEllipse();
      r.resize(300, 300);
      r.x = 100 + (i % 10) * 400;
      r.y = 4200 + Math.floor(i / 10) * 400;
      r.fills = [{ type: "GRADIENT_ANGULAR", gradientTransform: [[1, 0, 0], [0, 1, 0]], gradientStops: [{ position: 0, color: { r: 1, g: 0, b: 0, a: 1 } }, { position: 0.5, color: { r: 0, g: 0, b: 1, a: 1 } }, { position: 1, color: { r: 1, g: 0, b: 0, a: 1 } }] }];
      try {
        r.effects = [
          [{ type: "LAYER_BLUR", blurType: "PROGRESSIVE", radius: 30, startRadius: 0, startOffset: { x: 0, y: 0 }, endOffset: { x: 1, y: 1 }, visible: true }],
          [{ type: "NOISE", noiseType: "MONOTONE", noiseSize: 2, density: 0.5, color: { r: 0, g: 0, b: 0, a: 0.4 }, visible: true }],
          [{ type: "TEXTURE", noiseSize: 4, radius: 4, clipToShape: true, visible: true }],
          [{ type: "GLASS", lightIntensity: 0.5, lightAngle: 45, refraction: 0.5, depth: 10, dispersion: 0.2, radius: 4, visible: true }],
        ][i % 4];
      } catch (e) {
        tried.push(String(e).slice(0, 80));
      }
      section.appendChild(r);
    }
    // Every blend mode, over an image and a gradient
    const modes = ["DARKEN", "MULTIPLY", "LINEAR_BURN", "COLOR_BURN", "LIGHTEN", "SCREEN", "LINEAR_DODGE", "COLOR_DODGE", "OVERLAY", "SOFT_LIGHT", "HARD_LIGHT", "DIFFERENCE", "EXCLUSION", "HUE", "SATURATION", "COLOR", "LUMINOSITY"];
    for (let i = 0; i < modes.length; i++) {
      const frame = figma.createFrame();
      frame.name = "blend " + modes[i];
      frame.resize(400, 300);
      frame.x = 100 + (i % 9) * 440;
      frame.y = 5200 + Math.floor(i / 9) * 340;
      frame.fills = [{ type: "IMAGE", imageHash: hashes[100 + i], scaleMode: "FILL" }];
      const top = figma.createEllipse();
      top.resize(300, 200);
      top.x = 50; top.y = 50;
      top.fills = [{ type: "GRADIENT_LINEAR", gradientTransform: [[1, 0, 0], [0, 1, 0]], gradientStops: [{ position: 0, color: { r: 1, g: 0.2, b: 0.2, a: 1 } }, { position: 1, color: { r: 0.2, g: 0.2, b: 1, a: 1 } }] }];
      top.blendMode = modes[i];
      frame.appendChild(top);
      section.appendChild(frame);
    }
    // Masks and see-through groups
    for (let i = 0; i < 30; i++) {
      const image = figma.createRectangle();
      image.resize(300, 300);
      image.fills = [{ type: "IMAGE", imageHash: hashes[140 + i], scaleMode: "FILL" }];
      const mask = i % 2 ? figma.createStar() : figma.createEllipse();
      mask.resize(300, 300);
      mask.isMask = true;
      mask.fills = [i % 3 ? { type: "SOLID", color: { r: 0, g: 0, b: 0 } } : { type: "GRADIENT_LINEAR", gradientTransform: [[1, 0, 0], [0, 1, 0]], gradientStops: [{ position: 0, color: { r: 0, g: 0, b: 0, a: 1 } }, { position: 1, color: { r: 0, g: 0, b: 0, a: 0 } }] }];
      const group = figma.group([mask, image], section);
      group.x = 100 + (i % 10) * 340;
      group.y = 6000 + Math.floor(i / 10) * 340;
      if (i % 4 === 0) group.opacity = 0.5;
    }
    // Cards with images and text in auto layout, each with a shadow
    for (let i = 0; i < 60; i++) {
      const frame = figma.createFrame();
      frame.layoutMode = "VERTICAL";
      frame.itemSpacing = 8;
      frame.paddingLeft = frame.paddingRight = frame.paddingTop = frame.paddingBottom = 12;
      frame.cornerRadius = 12;
      frame.effects = [{ type: "DROP_SHADOW", color: { r: 0, g: 0, b: 0, a: 0.2 }, offset: { x: 0, y: 4 }, radius: 16, spread: 0, visible: true, blendMode: "NORMAL" }];
      const image = figma.createRectangle();
      image.resize(240, 160);
      image.cornerRadius = 8;
      image.fills = [{ type: "IMAGE", imageHash: hashes[i * 2 + 1], scaleMode: "FILL" }];
      frame.appendChild(image);
      const t = figma.createText();
      t.characters = "Item " + i + " with a caption";
      t.fontSize = 13;
      frame.appendChild(t);
      frame.primaryAxisSizingMode = "AUTO";
      frame.counterAxisSizingMode = "AUTO";
      frame.x = 4000 + (i % 10) * 300;
      frame.y = 7200 + Math.floor(i / 10) * 290;
      section.appendChild(frame);
    }
    return JSON.stringify({ id: section.id, tried });
  `);
  const effects = JSON.parse(sections.effects);
  sections.effects = effects.id;
  if (effects.tried.length) log("effects Figma didn't take:", effects.tried.slice(0, 3).join(" | "));
  await save();
  log("effects", sections.effects);
}

// What shows seams and gaps: hairlines on and between tile edges, lines of
// every width at fractions of a pixel, fine checkers, long thin lines, tiny
// layers, turned text, and layers far from the origin
if (want("edges")) {
  sections.edges = await evaluate(`
    const section = await window.__bench.section("Edges", -12000, 0, 11000, 10000);
    const solid = (r, g, b) => [{ type: "SOLID", color: { r, g, b } }];
    const at = (n, x, y) => { n.x = x; n.y = y; section.appendChild(n); return n; };
    for (let i = 0; i < 200; i++) {
      const v = figma.createRectangle();
      v.resize(0.25 + (i % 8) * 0.25, 4000);
      v.fills = solid(0, 0, 0);
      at(v, 100 + i * 16 + (i % 5) * 0.2, 100);
    }
    for (let i = 0; i < 200; i++) {
      const h = figma.createLine();
      h.resize(3500, 0);
      h.strokes = solid(0.1, 0.2, 0.8);
      h.strokeWeight = 0.5 + (i % 4) * 0.5;
      at(h, 3600, 100 + i * 19.7);
    }
    const checker = figma.createFrame();
    checker.name = "checker";
    checker.resize(2000, 2000);
    checker.fills = solid(1, 1, 1);
    for (let y = 0; y < 50; y++) for (let x = 0; x < 50; x++) {
      if ((x + y) % 2) continue;
      const r = figma.createRectangle();
      r.resize(40, 40);
      r.x = x * 40; r.y = y * 40;
      r.fills = solid(0, 0, 0);
      checker.appendChild(r);
    }
    at(checker, 7300, 100);
    const long = figma.createRectangle();
    long.resize(11000, 1);
    long.fills = solid(1, 0, 0);
    at(long, 0, 4300);
    for (let i = 0; i < 400; i++) {
      const tiny = figma.createEllipse();
      tiny.resize(0.5 + (i % 10) * 0.3, 0.5 + (i % 10) * 0.3);
      tiny.fills = solid(0, 0, 0);
      at(tiny, 100 + (i % 40) * 5, 4500 + Math.floor(i / 40) * 5);
    }
    for (let i = 0; i < 36; i++) {
      const t = figma.createText();
      t.characters = "Turned text " + (i * 10) + "° Ag 漢字 🙂";
      t.fontSize = 24;
      t.rotation = i * 10;
      at(t, 500 + (i % 6) * 400, 5000 + Math.floor(i / 6) * 400);
    }
    for (let i = 0; i < 12; i++) {
      const r = figma.createRectangle();
      r.resize(800, 800);
      r.fills = solid(0.3, 0.6, 0.3);
      r.strokes = solid(0, 0, 0);
      r.strokeWeight = 1 + (i % 4) * 3;
      r.strokeAlign = ["INSIDE", "CENTER", "OUTSIDE"][i % 3];
      if (i % 2) r.dashPattern = [10, 6];
      r.rotation = i * 7.5;
      at(r, 3600 + (i % 4) * 1000, 4500 + Math.floor(i / 4) * 1000);
    }
    const big = figma.createEllipse();
    big.resize(3000, 3000);
    big.fills = [{ type: "GRADIENT_DIAMOND", gradientTransform: [[1, 0, 0], [0, 1, 0]], gradientStops: [{ position: 0, color: { r: 1, g: 1, b: 0, a: 1 } }, { position: 1, color: { r: 0, g: 0.5, b: 0.5, a: 1 } }] }];
    at(big, 7500, 6500);
    const far = figma.createText();
    far.characters = "Far from the origin";
    far.fontSize = 48;
    far.x = 900000; far.y = -900000;
    figma.currentPage.appendChild(far);
    far.name = "far";
    return section.id;
  `);
  await save();
  log("edges", sections.edges);
}

const counts = await evaluate(`
  const out = {};
  for (const section of figma.currentPage.children) {
    if (section.type !== "SECTION") continue;
    out[section.name] = section.findAll(() => true).length;
  }
  return out;
`);
log("layers by section", JSON.stringify(counts));
ws.close();
