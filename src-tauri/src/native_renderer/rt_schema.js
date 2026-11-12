/* The render tree's Kiwi schema (render.bkiwi) out of the bytes of Figma's
   wasm, as rt.rs's schema_from_wasm does: the NUL-terminated name in the
   data segments, the asset table entry {namePtr, dataPtr, size} (u32
   little-endian) naming it, then the bytes it points at, checked to parse
   as a binary Kiwi schema. Null when there's none */
(bytes) => {
  const w = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let p = 0;
  const fail = () => { throw new Error("malformed"); };
  const leb = (b) => {
    let v = 0, shift = 0, x;
    do {
      if (p >= b.length) fail();
      x = b[p++];
      v += (x & 127) * 2 ** shift;
      shift += 7;
    } while (x & 128);
    return v;
  };
  const sleb = (b) => {
    let v = 0n, shift = 0n, x;
    do {
      if (p >= b.length) fail();
      x = b[p++];
      v |= BigInt(x & 127) << shift;
      shift += 7n;
    } while (x & 128);
    if (x & 64) v -= 1n << shift;
    return v;
  };

  // Active data segments: [memory address, bytes]
  const segments = [];
  try {
    if (w.length < 8 || w[0] !== 0 || w[1] !== 0x61 || w[2] !== 0x73 || w[3] !== 0x6d) return null;
    p = 8;
    while (p < w.length) {
      const id = w[p++];
      const size = leb(w);
      const end = p + size;
      if (end > w.length) fail();
      if (id === 11) {
        const count = leb(w);
        for (let i = 0; i < count; i++) {
          const flags = leb(w);
          let addr = null;
          if (flags === 0 || flags === 2) {
            if (flags === 2) leb(w);
            const op = w[p++];
            if (op === 0x41) addr = Number(BigInt.asUintN(32, sleb(w)));
            else if (op === 0x42) addr = Number(BigInt.asUintN(64, sleb(w)));
            else if (op === 0x23) leb(w);
            else fail();
            if (w[p++] !== 0x0b) fail();
          } else if (flags !== 1) fail();
          const n = leb(w);
          if (p + n > end) fail();
          if (addr !== null) segments.push([addr, w.subarray(p, p + n)]);
          p += n;
        }
      }
      p = end;
    }
  } catch {
    return null;
  }

  // Memory as initialized, zeros between segments, or null unless a
  // segment holds the first byte
  const read = (addr, len) => {
    const out = new Uint8Array(len);
    let starts = false;
    for (const [a, d] of segments) {
      const s = Math.max(a, addr), e = Math.min(a + d.length, addr + len);
      if (s < e) {
        out.set(d.subarray(s - a, e - a), s - addr);
        if (s === addr) starts = true;
      }
    }
    return starts ? out : null;
  };

  const isSchema = (b) => {
    try {
      p = 0;
      const str = () => {
        while (p < b.length && b[p] !== 0) p++;
        if (p++ >= b.length) fail();
      };
      const defs = leb(b);
      for (let i = 0; i < defs; i++) {
        str();
        if (p >= b.length || b[p++] > 2) fail();
        const fields = leb(b);
        for (let j = 0; j < fields; j++) {
          str();
          leb(b);
          if (p++ >= b.length) fail();
          leb(b);
        }
      }
      return p === b.length;
    } catch {
      return false;
    }
  };

  const name = [...new TextEncoder().encode("render.bkiwi"), 0];
  for (const [addr, data] of segments) {
    for (let i = data.indexOf(name[0]); i >= 0; i = data.indexOf(name[0], i + 1)) {
      if (i > 0 && data[i - 1] !== 0) continue;
      let k = 0;
      while (k < name.length && data[i + k] === name[k]) k++;
      if (k < name.length) continue;
      const namePtr = addr + i;
      if (namePtr > 0xffffffff) continue;
      const needle = [namePtr & 255, (namePtr >>> 8) & 255, (namePtr >>> 16) & 255, namePtr >>> 24];
      for (const [a, d] of segments) {
        for (let off = (4 - (a % 4)) % 4; off + 4 <= d.length; off += 4) {
          if (d[off] !== needle[0] || d[off + 1] !== needle[1] || d[off + 2] !== needle[2] || d[off + 3] !== needle[3]) continue;
          const entry = read(a + off, 12);
          if (!entry) continue;
          const view = new DataView(entry.buffer);
          const dataPtr = view.getUint32(4, true), size = view.getUint32(8, true);
          if (size === 0 || size > 64 << 20) continue;
          const schema = read(dataPtr, size);
          if (schema && isSchema(schema)) return schema;
        }
      }
    }
  }
  return null;
}
