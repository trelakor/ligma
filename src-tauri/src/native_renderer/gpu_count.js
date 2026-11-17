// What Figma's own renderer holds in GPU memory, for the stats panel (see
// hud.rs): the textures and buffers it makes, counted as they're made and
// as they're destroyed or collected. The browser keeps them with the rest of
// the page's, so they can't be told apart from outside
(() => {
  if (window.__ligmaGpuBytes !== undefined || location.hostname !== "www.figma.com" || window.top !== window) return;
  if (typeof GPUDevice === "undefined") return;
  window.__ligmaGpuBytes = 0;
  const sizes = new WeakMap();
  const collected = new FinalizationRegistry((bytes) => (window.__ligmaGpuBytes -= bytes));
  const track = (object, bytes) => {
    if (!object || !(bytes > 0)) return object;
    sizes.set(object, bytes);
    window.__ligmaGpuBytes += bytes;
    collected.register(object, bytes, object);
    return object;
  };
  const release = (object) => {
    const bytes = sizes.get(object);
    if (!bytes) return;
    sizes.delete(object);
    window.__ligmaGpuBytes -= bytes;
    collected.unregister(object);
  };

  // Bytes a texel, or a 4x4 block for compressed formats
  const texel = (format = "") => {
    if (/^bc[14]|^etc2-rgb8|^eac-r11/.test(format)) return [8, 4];
    if (/^(bc|astc-4x4|etc2-rgba8|eac-rg11)/.test(format)) return [16, 4];
    if (/32float|32uint|32sint/.test(format)) return [4 * (format.match(/^rgba/) ? 4 : format.match(/^rg/) ? 2 : 1), 1];
    if (/16float|16uint|16sint|16unorm|16snorm/.test(format)) return [2 * (format.match(/^rgba/) ? 4 : format.match(/^rg/) ? 2 : 1), 1];
    if (/^(rgba8|bgra8|rgb10a2|rg11b10|rgb9e5|depth24|depth32float$|r32)/.test(format)) return [4, 1];
    if (/^depth32float-stencil8/.test(format)) return [8, 1];
    if (/^(rg8|depth16)/.test(format)) return [2, 1];
    if (/^(r8|stencil8)/.test(format)) return [1, 1];
    return [4, 1];
  };
  const textureBytes = (descriptor) => {
    const size = descriptor?.size ?? {};
    let width = size.width ?? size[0] ?? 1;
    let height = size.height ?? size[1] ?? 1;
    let layers = size.depthOrArrayLayers ?? size[2] ?? 1;
    const [bytes, block] = texel(descriptor?.format);
    const is3d = descriptor?.dimension === "3d";
    let total = 0;
    for (let level = 0; level < (descriptor?.mipLevelCount ?? 1); level++) {
      total += Math.ceil(width / block) * Math.ceil(height / block) * bytes * layers;
      width = Math.max(1, width >> 1);
      height = Math.max(1, height >> 1);
      if (is3d) layers = Math.max(1, layers >> 1);
    }
    return total * (descriptor?.sampleCount ?? 1);
  };

  const createTexture = GPUDevice.prototype.createTexture;
  GPUDevice.prototype.createTexture = function (descriptor) {
    return track(createTexture.apply(this, arguments), textureBytes(descriptor));
  };
  const createBuffer = GPUDevice.prototype.createBuffer;
  GPUDevice.prototype.createBuffer = function (descriptor) {
    return track(createBuffer.apply(this, arguments), descriptor?.size);
  };
  for (const kind of [GPUTexture, GPUBuffer]) {
    const destroy = kind.prototype.destroy;
    kind.prototype.destroy = function () {
      release(this);
      return destroy.apply(this, arguments);
    };
  }
})();
