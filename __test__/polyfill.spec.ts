/**
 * Polyfill Entry Point Tests
 *
 * Validates that importing '@napi-rs/webcodecs/polyfill' correctly injects
 * all WebCodecs classes into globalThis using nullish coalescing assignment.
 */

import test from 'ava'

import * as webcodecs from '../index.js'

// Import polyfill - this injects all classes to globalThis
import '../polyfill.js'

// All classes that should be injected to globalThis
const injectedClasses = [
  'VideoEncoder',
  'VideoDecoder',
  'AudioEncoder',
  'AudioDecoder',
  'VideoFrame',
  'AudioData',
  'EncodedVideoChunk',
  'EncodedAudioChunk',
  'ImageDecoder',
  'VideoColorSpace',
  'ImageTrack',
  'ImageTrackList',
  'DOMRectReadOnly',
] as const

// ============================================================================
// Polyfill Injection Tests
// ============================================================================

test('polyfill injects all WebCodecs classes to globalThis', (t) => {
  for (const className of injectedClasses) {
    t.truthy((globalThis as Record<string, unknown>)[className], `globalThis.${className} should be defined`)
  }
})

test('polyfill injects the same classes as the module exports', (t) => {
  for (const className of injectedClasses) {
    t.is(
      (globalThis as Record<string, unknown>)[className],
      (webcodecs as Record<string, unknown>)[className],
      `globalThis.${className} should be the same as webcodecs.${className}`,
    )
  }
})

// ============================================================================
// Type Verification Tests
// ============================================================================

test('VideoEncoder is usable from globalThis', (t) => {
  const encoder = new globalThis.VideoEncoder({
    output: () => {},
    error: () => {},
  })
  t.truthy(encoder)
  t.is(encoder.state, 'unconfigured')
  encoder.close()
})

test('VideoDecoder is usable from globalThis', (t) => {
  const decoder = new globalThis.VideoDecoder({
    output: () => {},
    error: () => {},
  })
  t.truthy(decoder)
  t.is(decoder.state, 'unconfigured')
  decoder.close()
})

test('AudioEncoder is usable from globalThis', (t) => {
  const encoder = new globalThis.AudioEncoder({
    output: () => {},
    error: () => {},
  })
  t.truthy(encoder)
  t.is(encoder.state, 'unconfigured')
  encoder.close()
})

test('AudioDecoder is usable from globalThis', (t) => {
  const decoder = new globalThis.AudioDecoder({
    output: () => {},
    error: () => {},
  })
  t.truthy(decoder)
  t.is(decoder.state, 'unconfigured')
  decoder.close()
})

test('VideoFrame is usable from globalThis', (t) => {
  // Create a small I420 frame
  const width = 2
  const height = 2
  const ySize = width * height
  const uvSize = (width / 2) * (height / 2)
  const data = new Uint8Array(ySize + uvSize * 2)
  data.fill(128)

  const frame = new globalThis.VideoFrame(data, {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })
  t.truthy(frame)
  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)
  frame.close()
})

test('AudioData is usable from globalThis', (t) => {
  const data = new Float32Array(1024)
  const audioData = new globalThis.AudioData({
    data,
    format: 'f32',
    sampleRate: 48000,
    numberOfFrames: 1024,
    numberOfChannels: 1,
    timestamp: 0,
  })
  t.truthy(audioData)
  t.is(audioData.sampleRate, 48000)
  t.is(audioData.numberOfChannels, 1)
  audioData.close()
})

test('EncodedVideoChunk is usable from globalThis', (t) => {
  const data = new Uint8Array([0, 0, 0, 1, 0x67])
  const chunk = new globalThis.EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data,
  })
  t.truthy(chunk)
  t.is(chunk.type, 'key')
  t.is(chunk.timestamp, 0)
})

test('EncodedAudioChunk is usable from globalThis', (t) => {
  const data = new Uint8Array([0xff, 0xf1])
  const chunk = new globalThis.EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data,
  })
  t.truthy(chunk)
  t.is(chunk.type, 'key')
  t.is(chunk.timestamp, 0)
})

test('VideoColorSpace is usable from globalThis', (t) => {
  const colorSpace = new globalThis.VideoColorSpace({
    primaries: 'bt709',
    transfer: 'bt709',
    matrix: 'bt709',
    fullRange: false,
  })
  t.truthy(colorSpace)
  t.is(colorSpace.primaries, 'bt709')
})

test('DOMRectReadOnly is usable from globalThis', (t) => {
  const rect = new globalThis.DOMRectReadOnly(0, 0, 100, 100)
  t.truthy(rect)
  t.is(rect.x, 0)
  t.is(rect.y, 0)
  t.is(rect.width, 100)
  t.is(rect.height, 100)
})
