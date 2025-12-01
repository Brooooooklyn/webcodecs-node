import {
  createOnMessage as __wasmCreateOnMessageForFsProxy,
  getDefaultContext as __emnapiGetDefaultContext,
  instantiateNapiModuleSync as __emnapiInstantiateNapiModuleSync,
  WASI as __WASI,
} from '@napi-rs/wasm-runtime'



const __wasi = new __WASI({
  version: 'preview1',
})

const __wasmUrl = new URL('./package-template.wasm32-wasi.wasm', import.meta.url).href
const __emnapiContext = __emnapiGetDefaultContext()


const __sharedMemory = new WebAssembly.Memory({
  initial: 4000,
  maximum: 65536,
  shared: true,
})

const __wasmFile = await fetch(__wasmUrl).then((res) => res.arrayBuffer())

const {
  instance: __napiInstance,
  module: __wasiModule,
  napiModule: __napiModule,
} = __emnapiInstantiateNapiModuleSync(__wasmFile, {
  context: __emnapiContext,
  asyncWorkPoolSize: 4,
  wasi: __wasi,
  onCreateWorker() {
    const worker = new Worker(new URL('./wasi-worker-browser.mjs', import.meta.url), {
      type: 'module',
    })

    return worker
  },
  overwriteImports(importObject) {
    importObject.env = {
      ...importObject.env,
      ...importObject.napi,
      ...importObject.emnapi,
      memory: __sharedMemory,
    }
    return importObject
  },
  beforeInit({ instance }) {
    for (const name of Object.keys(instance.exports)) {
      if (name.startsWith('__napi_register__')) {
        instance.exports[name]()
      }
    }
  },
})
export default __napiModule.exports
export const EncodedVideoChunk = __napiModule.exports.EncodedVideoChunk
export const VideoDecoder = __napiModule.exports.VideoDecoder
export const VideoEncoder = __napiModule.exports.VideoEncoder
export const VideoFrame = __napiModule.exports.VideoFrame
export const CodecState = __napiModule.exports.CodecState
export const EncodedVideoChunkType = __napiModule.exports.EncodedVideoChunkType
export const getAvailableHardwareAccelerators = __napiModule.exports.getAvailableHardwareAccelerators
export const getHardwareAccelerators = __napiModule.exports.getHardwareAccelerators
export const getPreferredHardwareAccelerator = __napiModule.exports.getPreferredHardwareAccelerator
export const isHardwareAcceleratorAvailable = __napiModule.exports.isHardwareAcceleratorAvailable
export const VideoPixelFormat = __napiModule.exports.VideoPixelFormat
