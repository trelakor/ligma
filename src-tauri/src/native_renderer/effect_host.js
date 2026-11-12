// WebGPU for custom effects, over Ligma's own GPU device (see
// effect_host.rs). Effects are the file's own code: modules with `setup`
// and `render`, each given the device and a frame with the node's pixels
// in `input` and where to draw in `output`. Calls go to `__native(op,
// json, bytes)`, which holds the GPU objects behind ids
"use strict";
(() => {
  const N = globalThis.__native;
  delete globalThis.__native;

  globalThis.GPUBufferUsage = { MAP_READ: 1, MAP_WRITE: 2, COPY_SRC: 4, COPY_DST: 8, INDEX: 16, VERTEX: 32, UNIFORM: 64, STORAGE: 128, INDIRECT: 256, QUERY_RESOLVE: 512 };
  globalThis.GPUTextureUsage = { COPY_SRC: 1, COPY_DST: 2, TEXTURE_BINDING: 4, STORAGE_BINDING: 8, RENDER_ATTACHMENT: 16 };
  globalThis.GPUShaderStage = { VERTEX: 1, FRAGMENT: 2, COMPUTE: 4 };
  globalThis.GPUMapMode = { READ: 1, WRITE: 2 };
  globalThis.GPUColorWrite = { RED: 1, GREEN: 2, BLUE: 4, ALPHA: 8, ALL: 15 };
  const say = (level) => (...parts) => N("log", JSON.stringify([level, parts.map((part) => (typeof part === "string" ? part : JSON.stringify(part) ?? String(part))).join(" ")]));
  globalThis.console = { log: say("log"), info: say("log"), debug: say("log"), warn: say("warn"), error: say("error") };

  // GPU objects go over as `{ "$": id }`
  const json = (value) => JSON.stringify(value, (_, x) => (x && typeof x === "object" && "_id" in x ? { $: x._id } : x));
  const call = (op, args, bytes) => {
    const result = N(op, json(args), bytes);
    return result == null ? null : JSON.parse(result);
  };

  // `data` as the bytes it views, `offset` and `size` in its elements
  const bytesOf = (data, offset = 0, size) => {
    if (data instanceof ArrayBuffer) return new Uint8Array(data, offset, size ?? data.byteLength - offset);
    if (ArrayBuffer.isView(data)) {
      const each = data.BYTES_PER_ELEMENT ?? 1;
      const start = data.byteOffset + offset * each;
      return new Uint8Array(data.buffer, start, size !== undefined ? size * each : data.byteLength - offset * each);
    }
    if (Array.isArray(data)) return new Uint8Array(new Float32Array(data).buffer);
    throw new TypeError("not a buffer source");
  };

  class GPUObject {
    constructor(id, label) {
      this._id = id;
      this.label = label ?? "";
    }
  }

  class GPUBuffer extends GPUObject {
    constructor(id, desc) {
      super(id, desc.label);
      this.size = desc.size;
      this.usage = desc.usage;
      this._ranges = [];
      this.mapState = desc.mappedAtCreation ? "mapped" : "unmapped";
    }
    getMappedRange(offset = 0, size) {
      if (this.mapState !== "mapped") throw new Error("buffer isn't mapped");
      const range = new ArrayBuffer(size ?? this.size - offset);
      this._ranges.push([offset, range]);
      return range;
    }
    unmap() {
      for (const [offset, range] of this._ranges) call("writeBuffer", [this, offset], new Uint8Array(range));
      this._ranges = [];
      this.mapState = "unmapped";
    }
    mapAsync() {
      return Promise.reject(new Error("reading buffers back isn't supported"));
    }
    destroy() {
      call("drop", [this]);
    }
  }

  const extent = (size) => (Array.isArray(size) ? { width: size[0], height: size[1] ?? 1, depthOrArrayLayers: size[2] ?? 1 } : { width: size.width, height: size.height ?? 1, depthOrArrayLayers: size.depthOrArrayLayers ?? 1 });

  class GPUTexture extends GPUObject {
    constructor(id, desc) {
      super(id, desc.label);
      const size = extent(desc.size);
      this.width = size.width;
      this.height = size.height;
      this.depthOrArrayLayers = size.depthOrArrayLayers;
      this.mipLevelCount = desc.mipLevelCount ?? 1;
      this.sampleCount = desc.sampleCount ?? 1;
      this.dimension = desc.dimension ?? "2d";
      this.format = desc.format;
      this.usage = desc.usage;
    }
    createView(desc) {
      return new GPUObject(call("createView", [this, desc ?? {}]), desc?.label);
    }
    destroy() {
      call("drop", [this]);
    }
  }

  class GPUPipeline extends GPUObject {
    getBindGroupLayout(index) {
      return new GPUObject(call("getBindGroupLayout", [this, index]));
    }
  }

  class GPUShaderModule extends GPUObject {
    getCompilationInfo() {
      return Promise.resolve({ messages: [] });
    }
  }

  class GPUPass {
    constructor(encoder, kind, desc) {
      this._encoder = encoder;
      this._kind = kind;
      this._desc = desc ?? {};
      this._commands = [];
      this.label = desc?.label ?? "";
    }
    setPipeline(pipeline) {
      this._commands.push(["setPipeline", pipeline]);
    }
    setBindGroup(index, group, offsets, start, length) {
      let dynamic = offsets ?? [];
      if (ArrayBuffer.isView(dynamic)) dynamic = Array.from(dynamic.subarray(start ?? 0, (start ?? 0) + (length ?? dynamic.length)));
      this._commands.push(["setBindGroup", index, group, dynamic]);
    }
    setVertexBuffer(slot, buffer, offset, size) {
      this._commands.push(["setVertexBuffer", slot, buffer, offset ?? 0, size ?? null]);
    }
    setIndexBuffer(buffer, format, offset, size) {
      this._commands.push(["setIndexBuffer", buffer, format, offset ?? 0, size ?? null]);
    }
    draw(vertices, instances, firstVertex, firstInstance) {
      this._commands.push(["draw", vertices, instances ?? 1, firstVertex ?? 0, firstInstance ?? 0]);
    }
    drawIndexed(indices, instances, firstIndex, baseVertex, firstInstance) {
      this._commands.push(["drawIndexed", indices, instances ?? 1, firstIndex ?? 0, baseVertex ?? 0, firstInstance ?? 0]);
    }
    setViewport(x, y, width, height, min, max) {
      this._commands.push(["setViewport", x, y, width, height, min, max]);
    }
    setScissorRect(x, y, width, height) {
      this._commands.push(["setScissorRect", x, y, width, height]);
    }
    setBlendConstant(color) {
      this._commands.push(["setBlendConstant", Array.isArray(color) ? { r: color[0], g: color[1], b: color[2], a: color[3] } : color]);
    }
    setStencilReference(value) {
      this._commands.push(["setStencilReference", value]);
    }
    dispatchWorkgroups(x, y, z) {
      this._commands.push(["dispatchWorkgroups", x, y ?? 1, z ?? 1]);
    }
    pushDebugGroup() {}
    popDebugGroup() {}
    insertDebugMarker() {}
    end() {
      call(this._kind, [this._encoder, this._desc, this._commands]);
    }
  }

  class GPUCommandEncoder extends GPUObject {
    beginRenderPass(desc) {
      return new GPUPass(this, "renderPass", desc);
    }
    beginComputePass(desc) {
      return new GPUPass(this, "computePass", desc);
    }
    copyBufferToBuffer(source, sourceOffset, destination, destinationOffset, size) {
      // The newer form leaves out the offsets
      if (typeof sourceOffset !== "number") [sourceOffset, destination, destinationOffset, size] = [0, sourceOffset, 0, destination];
      call("copyBufferToBuffer", [this, source, sourceOffset, destination, destinationOffset, size ?? null]);
    }
    copyBufferToTexture(source, destination, size) {
      call("copyBufferToTexture", [this, source, destination, extent(size)]);
    }
    copyTextureToBuffer(source, destination, size) {
      call("copyTextureToBuffer", [this, source, destination, extent(size)]);
    }
    copyTextureToTexture(source, destination, size) {
      call("copyTextureToTexture", [this, source, destination, extent(size)]);
    }
    clearBuffer(buffer, offset, size) {
      call("clearBuffer", [this, buffer, offset ?? 0, size ?? null]);
    }
    pushDebugGroup() {}
    popDebugGroup() {}
    insertDebugMarker() {}
    finish(desc) {
      return new GPUObject(call("finish", [this]), desc?.label);
    }
  }

  const queue = {
    label: "",
    writeBuffer(buffer, offset, data, dataOffset, size) {
      call("writeBuffer", [buffer, offset], bytesOf(data, dataOffset ?? 0, size));
    },
    writeTexture(destination, data, layout, size) {
      call("writeTexture", [destination, layout, extent(size)], bytesOf(data));
    },
    submit(buffers) {
      call("submit", [buffers]);
    },
    onSubmittedWorkDone() {
      return Promise.resolve();
    },
    copyExternalImageToTexture() {
      throw new Error("external images aren't supported");
    },
  };

  const limits = JSON.parse(N("limits", "[]"));
  const scopes = [];
  const device = {
    label: "",
    queue,
    limits,
    features: new Set(JSON.parse(N("features", "[]"))),
    lost: new Promise(() => {}),
    onuncapturederror: null,
    createBuffer: (desc) => new GPUBuffer(call("createBuffer", desc), desc),
    createTexture: (desc) => new GPUTexture(call("createTexture", desc), desc),
    createSampler: (desc) => new GPUObject(call("createSampler", desc ?? {}), desc?.label),
    createShaderModule: (desc) => new GPUShaderModule(call("createShaderModule", { code: desc.code, label: desc.label }), desc.label),
    createBindGroupLayout: (desc) => new GPUObject(call("createBindGroupLayout", desc), desc.label),
    createPipelineLayout: (desc) => new GPUObject(call("createPipelineLayout", desc), desc.label),
    createBindGroup: (desc) => new GPUObject(call("createBindGroup", desc), desc.label),
    createRenderPipeline: (desc) => new GPUPipeline(call("createRenderPipeline", desc), desc.label),
    createComputePipeline: (desc) => new GPUPipeline(call("createComputePipeline", desc), desc.label),
    createRenderPipelineAsync: (desc) => Promise.resolve(device.createRenderPipeline(desc)),
    createComputePipelineAsync: (desc) => Promise.resolve(device.createComputePipeline(desc)),
    createCommandEncoder: (desc) => new GPUCommandEncoder(call("createCommandEncoder", desc ?? {}), desc?.label),
    pushErrorScope: (filter) => {
      scopes.push(filter);
      call("pushErrorScope", [filter]);
    },
    popErrorScope: () => {
      scopes.pop();
      const error = call("popErrorScope", []);
      return Promise.resolve(error ? { message: error } : null);
    },
    addEventListener() {},
    removeEventListener() {},
    destroy() {},
  };

  const effects = new Map();
  const runtimes = new Map();

  globalThis.__load = (id, code) => {
    const module = { exports: {} };
    new Function("module", "exports", "require", code)(module, module.exports, (name) => {
      throw new Error(`can't require ${name}`);
    });
    const loaded = module.exports;
    if (typeof loaded.render !== "function") throw new Error("the effect has no render function");
    effects.set(id, loaded);
    for (const [key, runtime] of runtimes) if (runtime.id === id) runtimes.delete(key);
  };
  globalThis.__has = (id) => effects.has(id);

  const placeholder = (usage) => new GPUTexture(call("createTexture", { size: [1, 1], format: "rgba8unorm", usage }), { size: [1, 1], format: "rgba8unorm", usage });

  // Runs node `node`'s chain of effects, `[[id, params], ...]`, from
  // texture `input` to texture `output`, both `width` by `height`. Each
  // effect's state is kept between runs, as Figma keeps it for each node
  globalThis.__run = async (node, chain, input, output, [width, height, renderScale, time]) => {
    const size = [width, height];
    const source = new GPUTexture(input, { size, format: "rgba8unorm", usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT });
    const target = new GPUTexture(output, { size, format: "rgba8unorm", usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC | GPUTextureUsage.TEXTURE_BINDING });
    const between = [];
    let from = source;
    try {
      for (const [index, [id, params]] of chain.entries()) {
        const effect = effects.get(id);
        if (!effect) throw new Error(`effect ${id} isn't loaded`);
        const key = `${node}|${index}`;
        let runtime = runtimes.get(key);
        if (!runtime || runtime.id !== id) {
          const frameState = { input: placeholder(GPUTextureUsage.TEXTURE_BINDING), output: placeholder(GPUTextureUsage.RENDER_ATTACHMENT), time: 0, deltaTime: 0, frame: 0, mousePosition: { x: 0, y: 0, down: false }, renderScale: 1, params: {}, state: {} };
          runtime = { id, frameState, ready: false };
          runtimes.set(key, runtime);
        }
        const frameState = runtime.frameState;
        const frame = () => ({
          ...frameState,
          get state() {
            return frameState.state;
          },
          set state(value) {
            frameState.state = value;
          },
        });
        if (!runtime.ready) {
          if (typeof effect.setup === "function") await effect.setup(device, frame());
          runtime.ready = true;
        }
        const last = index === chain.length - 1;
        let to = target;
        if (!last) {
          to = device.createTexture({ size, format: "rgba8unorm", usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.RENDER_ATTACHMENT });
          between.push(to);
        }
        frameState.renderScale = renderScale;
        frameState.input = from;
        frameState.output = to;
        frameState.deltaTime = time - frameState.time;
        frameState.time = time;
        frameState.frame += 1;
        frameState.params = params;
        await effect.render(device, frame());
        from = to;
      }
    } finally {
      for (const texture of between) texture.destroy();
    }
  };

  globalThis.__forget = (node) => {
    for (const key of [...runtimes.keys()]) if (key.startsWith(`${node}|`)) runtimes.delete(key);
  };
})();
