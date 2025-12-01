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
  // Enums
  CodecState,
  EncodedVideoChunkType,
  VideoPixelFormat,
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
  const encoder = new VideoEncoder(
    () => {},
    () => {},
  )
  t.truthy(encoder)
  t.is(encoder.state, CodecState.Unconfigured)
  encoder.close()
})

test('exports VideoDecoder class', (t) => {
  t.is(typeof VideoDecoder, 'function')
  const decoder = new VideoDecoder(
    () => {},
    () => {},
  )
  t.truthy(decoder)
  t.is(decoder.state, CodecState.Unconfigured)
  decoder.close()
})

test('exports VideoFrame class', (t) => {
  t.is(typeof VideoFrame, 'function')
})

test('exports EncodedVideoChunk class', (t) => {
  t.is(typeof EncodedVideoChunk, 'function')
})

test('exports CodecState enum', (t) => {
  t.true(CodecState.Unconfigured === 'Unconfigured')
  t.true(CodecState.Configured === 'Configured')
  t.true(CodecState.Closed === 'Closed')
})

test('exports EncodedVideoChunkType enum', (t) => {
  t.true(EncodedVideoChunkType.Key === 'Key')
  t.true(EncodedVideoChunkType.Delta === 'Delta')
})

test('exports VideoPixelFormat enum', (t) => {
  t.true(VideoPixelFormat.I420 === 'I420')
  t.true(VideoPixelFormat.NV12 === 'NV12')
  t.true(VideoPixelFormat.RGBA === 'RGBA')
  t.true(VideoPixelFormat.BGRA === 'BGRA')
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
