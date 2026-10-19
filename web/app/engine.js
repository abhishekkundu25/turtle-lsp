// Runs the engine (engine.wasm, built from web/engine) and calls it. The
// page's worker and the tests load it the same way.

// The engine stopped (a trap: a panic aborts in WebAssembly). It has been
// started again, so the next call works.
export class EngineCrash extends Error {
  constructor(cause) {
    super(`the engine crashed and was restarted (${cause?.message ?? cause})`)
    this.crashed = true
  }
}

// `bytes`: the module's bytes (an ArrayBuffer or typed array).
export async function createEngine(bytes) {
  const module = await WebAssembly.compile(bytes)
  let instance
  const imports = {
    env: {
      random_fill(ptr, len) {
        // getRandomValues fills at most 65,536 bytes per call.
        for (let at = 0; at < len; at += 65536) {
          crypto.getRandomValues(new Uint8Array(instance.exports.memory.buffer, ptr + at, Math.min(65536, len - at)))
        }
      },
    },
  }
  const start = () => {
    instance = new WebAssembly.Instance(module, imports)
  }
  start()
  const encoder = new TextEncoder()
  const decoder = new TextDecoder()

  return {
    // The answer to `op` with `payload` ({ files } for analyse and fix_all).
    // Throws the engine's error message, or an EngineCrash.
    call(op, payload = {}) {
      const ex = instance.exports
      const request = encoder.encode(JSON.stringify({ ...payload, op }))
      let answer
      try {
        const ptr = ex.tl_alloc(request.length)
        new Uint8Array(ex.memory.buffer, ptr, request.length).set(request)
        const len = ex.tl_call(ptr, request.length)
        answer = decoder.decode(new Uint8Array(ex.memory.buffer, ex.tl_result(), len))
      } catch (e) {
        start()
        throw new EngineCrash(e)
      }
      const out = JSON.parse(answer)
      if ("error" in out) throw new Error(out.error)
      return out.ok
    },
    // The size of the engine's memory, for leak checks.
    memoryBytes: () => instance.exports.memory.buffer.byteLength,
  }
}
