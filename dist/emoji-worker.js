// Draws the emoji picker's chunks (dist/emoji.html) off the page's thread.
// Windows' color emoji font takes a few milliseconds to draw each emoji,
// and the browser's glyph cache holds only a few dozen, so a scrolling list
// of emoji kept drawing them again. Here each emoji is drawn once, kept as
// a bitmap, and chunks are put together from those
//
// In: { gen, dpr, jobs: [{ key, height, glyphs: [[emoji, x, y]] }] }, the
// chunks wanted, most wanted first. A new message replaces the jobs not
// yet done. Out: { gen, key, bitmap } per chunk

const CELL = 32;
const FONT = '20px "Segoe UI Emoji", "Apple Color Emoji", "Noto Color Emoji", sans-serif';
// Emoji kept, most recently used last; enough for a few screens of them
const KEEP = 400;

const glyphs = new Map();
let dpr = 1;
let gen = -1;
let jobs = [];
let busy = false;

// One emoji, centered by its ink in a CELL square
const glyph = (emoji) => {
  const cached = glyphs.get(emoji);
  if (cached) {
    glyphs.delete(emoji);
    glyphs.set(emoji, cached);
    return cached;
  }
  const size = Math.round(CELL * dpr);
  const canvas = new OffscreenCanvas(size, size);
  const g = canvas.getContext("2d");
  g.scale(dpr, dpr);
  g.font = FONT;
  g.textAlign = "center";
  g.textBaseline = "alphabetic";
  const ink = g.measureText(emoji);
  const y = CELL / 2 + (ink.actualBoundingBoxAscent - ink.actualBoundingBoxDescent) / 2;
  g.fillText(emoji, CELL / 2, Math.round(y * dpr) / dpr);
  const bitmap = canvas.transferToImageBitmap();
  glyphs.set(emoji, bitmap);
  if (glyphs.size > KEEP) {
    const [oldest, old] = glyphs.entries().next().value;
    glyphs.delete(oldest);
    old.close();
  }
  return bitmap;
};

const work = () => {
  const job = jobs.shift();
  if (!job) {
    busy = false;
    return;
  }
  const canvas = new OffscreenCanvas(Math.round(256 * dpr), Math.max(1, Math.round(job.height * dpr)));
  const g = canvas.getContext("2d");
  for (const [emoji, x, y] of job.glyphs) g.drawImage(glyph(emoji), Math.round(x * dpr), Math.round(y * dpr));
  const bitmap = canvas.transferToImageBitmap();
  postMessage({ gen, key: job.key, bitmap }, [bitmap]);
  // A turn between chunks, for newer wishes to come in
  setTimeout(work);
};

onmessage = ({ data }) => {
  if (data.dpr !== dpr) {
    for (const bitmap of glyphs.values()) bitmap.close();
    glyphs.clear();
    dpr = data.dpr;
  }
  gen = data.gen;
  jobs = data.jobs;
  if (!busy) {
    busy = true;
    setTimeout(work);
  }
};
