/**
 * WebCodecs Node - Test Suite Entry Point
 *
 * This file validates that all exports are available and the module loads correctly.
 */

import test from 'ava'

import {
  // Classes
  VideoEncoder,
  VideoDecoder,
  VideoFrame,
  EncodedVideoChunk,
  // Hardware functions
  getHardwareAccelerators,
  getAvailableHardwareAccelerators,
  getPreferredHardwareAccelerator,
  isHardwareAcceleratorAvailable,
} from '../index.js'

// ============================================================================
// Module Export Tests
// ============================================================================

test('exports VideoEncoder class', (t) => {
  t.is(typeof VideoEncoder, 'function')
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })
  t.truthy(encoder)
  t.is(encoder.state, 'unconfigured')
  encoder.close()
})

test('exports VideoDecoder class', (t) => {
  t.is(typeof VideoDecoder, 'function')
  const decoder = new VideoDecoder({
    output: () => {},
    error: () => {},
  })
  t.truthy(decoder)
  t.is(decoder.state, 'unconfigured')
  decoder.close()
})

test('exports VideoFrame class', (t) => {
  t.is(typeof VideoFrame, 'function')
})

test('exports EncodedVideoChunk class', (t) => {
  t.is(typeof EncodedVideoChunk, 'function')
})

test('CodecState uses string literals', (t) => {
  const encoder = new VideoEncoder({ output: () => {}, error: () => {} })
  t.is(encoder.state, 'unconfigured')
  encoder.close()
  t.is(encoder.state, 'closed')
})

test('EncodedVideoChunkType uses string literals', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00]),
  })
  t.is(chunk.type, 'key')
})

test('VideoPixelFormat uses string literals', (t) => {
  const frame = new VideoFrame(new Uint8Array(4 * 4 * 4), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 4,
    timestamp: 0,
  })
  t.is(frame.format, 'RGBA')
  frame.close()
})

// ============================================================================
// Hardware Acceleration Export Tests
// ============================================================================

test('exports getHardwareAccelerators function', (t) => {
  t.is(typeof getHardwareAccelerators, 'function')
  const accelerators = getHardwareAccelerators()
  t.true(Array.isArray(accelerators))
  t.true(accelerators.length > 0)

  // Verify structure of returned objects
  for (const accel of accelerators) {
    t.is(typeof accel.name, 'string')
    t.is(typeof accel.description, 'string')
    t.is(typeof accel.available, 'boolean')
  }
})

test('exports getAvailableHardwareAccelerators function', (t) => {
  t.is(typeof getAvailableHardwareAccelerators, 'function')
  const available = getAvailableHardwareAccelerators()
  t.true(Array.isArray(available))

  // All items should be strings
  for (const name of available) {
    t.is(typeof name, 'string')
  }
})

test('exports getPreferredHardwareAccelerator function', (t) => {
  t.is(typeof getPreferredHardwareAccelerator, 'function')
  const preferred = getPreferredHardwareAccelerator()
  // Can be null or string
  t.true(preferred === null || typeof preferred === 'string')
})

test('exports isHardwareAcceleratorAvailable function', (t) => {
  t.is(typeof isHardwareAcceleratorAvailable, 'function')
  // Test with a valid accelerator name
  const result = isHardwareAcceleratorAvailable('videotoolbox')
  t.is(typeof result, 'boolean')
})
