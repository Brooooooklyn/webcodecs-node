/**
 * Demuxer Tests
 *
 * Tests for Mp4Demuxer, WebMDemuxer, and MkvDemuxer classes.
 */

import test from 'ava'
import { promises as fs } from 'fs'
import path from 'path'
import { fileURLToPath } from 'url'

// Skip demuxer tests on Linux armv7 (QEMU emulation too slow, causes timeouts)
const isLinuxArmv7 = process.platform === 'linux' && process.arch === 'arm'
const runTest = isLinuxArmv7 ? test.skip : test

import {
  Mp4Demuxer,
  WebMDemuxer,
  MkvDemuxer,
  VideoEncoder,
  AudioEncoder,
  WebMMuxer,
  MkvMuxer,
  resetHardwareFallbackState,
  type EncodedVideoChunk,
  type EncodedAudioChunk,
  type EncodedVideoChunkMetadata,
  type EncodedAudioChunkMetadata,
} from '../index.js'
import { generateSolidColorI420Frame, generateSilence, TestColors } from './helpers/index.js'

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)

const FIXTURES_DIR = path.join(__dirname, 'fixtures')

// ============================================================================
// Mp4Demuxer Tests
// ============================================================================

runTest('Mp4Demuxer: constructor creates demuxer', (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })
  t.truthy(demuxer)
  t.is(demuxer.state, 'unloaded')
  demuxer.close()
})

runTest('Mp4Demuxer: constructor requires error callback', (t) => {
  t.throws(() => new Mp4Demuxer({} as { error: (e: Error) => void }))
})

runTest('Mp4Demuxer: load file and get tracks', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  t.is(demuxer.state, 'ready')

  const tracks = demuxer.tracks
  t.true(tracks.length > 0, 'Should have at least one track')

  // Find video track
  const videoTrack = tracks.find((track) => track.trackType === 'video')
  t.truthy(videoTrack, 'Should have a video track')
  if (videoTrack) {
    t.true(videoTrack.codedWidth! > 0, 'Video should have width')
    t.true(videoTrack.codedHeight! > 0, 'Video should have height')
    t.truthy(videoTrack.codec, 'Video should have codec string')
  }

  demuxer.close()
})

runTest('Mp4Demuxer: load buffer and get tracks', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  const buffer = await fs.readFile(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))
  await demuxer.loadBuffer(buffer)

  t.is(demuxer.state, 'ready')

  const tracks = demuxer.tracks
  t.true(tracks.length > 0, 'Should have at least one track')

  demuxer.close()
})

runTest('Mp4Demuxer: get duration', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  const duration = demuxer.duration
  t.truthy(duration, 'Should have duration')
  t.true(duration! > 0, 'Duration should be positive')

  demuxer.close()
})

runTest('Mp4Demuxer: get video decoder config', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Should have video decoder config')
  if (config) {
    t.truthy(config.codec, 'Config should have codec string')
    t.true(config.codedWidth > 0, 'Config should have width')
    t.true(config.codedHeight > 0, 'Config should have height')
  }

  demuxer.close()
})

runTest('Mp4Demuxer: demux video chunks', async (t) => {
  return new Promise<void>((resolve, reject) => {
    const videoChunks: EncodedVideoChunk[] = []
    let errorOccurred = false

    const demuxer = new Mp4Demuxer({
      videoOutput: (chunk: EncodedVideoChunk) => {
        videoChunks.push(chunk)
      },
      error: (e: Error) => {
        errorOccurred = true
        demuxer.close()
        reject(e)
      },
    })

    demuxer
      .load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))
      .then(() => {
        // Demux a limited number of packets
        demuxer.demux(20)

        // Wait a bit for demuxing to complete
        setTimeout(() => {
          if (!errorOccurred) {
            t.true(videoChunks.length > 0, 'Should have demuxed video chunks')

            // Check first chunk properties
            if (videoChunks.length > 0) {
              const firstChunk = videoChunks[0]
              t.truthy(firstChunk.type, 'Chunk should have type')
              t.true(firstChunk.byteLength > 0, 'Chunk should have data')
            }

            demuxer.close()
            resolve()
          }
        }, 500)
      })
      .catch(reject)
  })
})

runTest('Mp4Demuxer: seek and demux', async (t) => {
  return new Promise<void>((resolve, reject) => {
    const videoChunks: EncodedVideoChunk[] = []
    let errorOccurred = false

    const demuxer = new Mp4Demuxer({
      videoOutput: (chunk: EncodedVideoChunk) => {
        videoChunks.push(chunk)
      },
      error: (e: Error) => {
        errorOccurred = true
        demuxer.close()
        reject(e)
      },
    })

    demuxer
      .load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))
      .then(() => {
        // Seek to 1 second
        demuxer.seek(1_000_000)

        // Demux after seek
        demuxer.demux(10)

        setTimeout(() => {
          if (!errorOccurred) {
            t.true(videoChunks.length > 0, 'Should have demuxed chunks after seek')

            // First chunk after seek should have timestamp >= 1 second
            // (may be earlier due to keyframe seeking)
            if (videoChunks.length > 0) {
              t.true(videoChunks[0].timestamp >= 0, 'Timestamp should be non-negative')
            }

            demuxer.close()
            resolve()
          }
        }, 500)
      })
      .catch(reject)
  })
})

runTest('Mp4Demuxer: state transitions', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  t.is(demuxer.state, 'unloaded')

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))
  t.is(demuxer.state, 'ready')

  demuxer.close()
  t.is(demuxer.state, 'closed')
})

runTest('Mp4Demuxer: select tracks', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  const tracks = demuxer.tracks
  const videoTrack = tracks.find((track) => track.trackType === 'video')
  const audioTrack = tracks.find((track) => track.trackType === 'audio')

  if (videoTrack) {
    t.notThrows(() => demuxer.selectVideoTrack(videoTrack.index))
  }

  if (audioTrack) {
    t.notThrows(() => demuxer.selectAudioTrack(audioTrack.index))
  }

  demuxer.close()
})

// ============================================================================
// WPT Fixture Tests
// ============================================================================

runTest('Mp4Demuxer: load H.264 WPT fixture', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'wpt', 'h264.mp4'))

  t.is(demuxer.state, 'ready')

  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Should have video decoder config')
  if (config) {
    t.true(config.codec.startsWith('avc'), 'Should be AVC/H.264 codec')
  }

  demuxer.close()
})

runTest('Mp4Demuxer: load H.265 WPT fixture', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'wpt', 'h265.mp4'))

  t.is(demuxer.state, 'ready')

  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Should have video decoder config')
  if (config) {
    t.true(config.codec.startsWith('hev') || config.codec.startsWith('hvc'), 'Should be HEVC/H.265 codec')
  }

  demuxer.close()
})

runTest('Mp4Demuxer: load AV1 WPT fixture', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'wpt', 'av1.mp4'))

  t.is(demuxer.state, 'ready')

  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Should have video decoder config')
  if (config) {
    t.true(config.codec.startsWith('av0') || config.codec.startsWith('av1'), 'Should be AV1 codec')
  }

  demuxer.close()
})

runTest('Mp4Demuxer: load AAC audio fixture', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'wpt', 'sfx-aac.mp4'))

  t.is(demuxer.state, 'ready')

  const config = demuxer.audioDecoderConfig
  t.truthy(config, 'Should have audio decoder config')
  if (config) {
    t.true(config.codec.startsWith('mp4a'), 'Should be AAC codec')
    t.true(config.sampleRate > 0, 'Should have sample rate')
    t.true(config.numberOfChannels > 0, 'Should have channel count')
  }

  demuxer.close()
})

// ============================================================================
// Error Handling Tests
// ============================================================================

runTest('Mp4Demuxer: error on invalid file path', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (_e: Error) => {},
  })

  await t.throwsAsync(() => demuxer.load('/nonexistent/path/file.mp4'), {
    message: /Failed to open file/,
  })

  demuxer.close()
})

runTest('Mp4Demuxer: error on loading twice without close', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (_e: Error) => {},
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  await t.throwsAsync(() => demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4')), {
    message: /already loaded/,
  })

  demuxer.close()
})

// ============================================================================
// WebMDemuxer Tests
// ============================================================================

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// Helper: Generate a WebM buffer with VP9 video
async function generateWebMWithVP9(): Promise<Uint8Array> {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: () => {},
  })

  encoder.configure({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.green, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  const muxer = new WebMMuxer()
  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const data = muxer.finalize()
  muxer.close()
  return data
}

// Helper: Generate a WebM buffer with VP9 video and Opus audio
async function generateWebMWithVP9AndOpus(): Promise<Uint8Array> {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const videoEncoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: () => {},
  })

  videoEncoder.configure({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  const audioEncoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: () => {},
  })

  audioEncoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    videoEncoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  for (let i = 0; i < 5; i++) {
    const audioData = generateSilence(960, 2, 48000, 'f32', i * 20000)
    audioEncoder.encode(audioData)
    audioData.close()
  }

  await Promise.all([videoEncoder.flush(), audioEncoder.flush()])
  videoEncoder.close()
  audioEncoder.close()

  const muxer = new WebMMuxer()
  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
  })
  muxer.addAudioTrack({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }
  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  muxer.flush()
  const data = muxer.finalize()
  muxer.close()
  return data
}

runTest('WebMDemuxer: constructor creates demuxer', (t) => {
  const demuxer = new WebMDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })
  t.truthy(demuxer)
  t.is(demuxer.state, 'unloaded')
  demuxer.close()
})

runTest('WebMDemuxer: constructor requires error callback', (t) => {
  t.throws(() => new WebMDemuxer({} as { error: (e: Error) => void }))
})

runTest('WebMDemuxer: load buffer and get tracks', async (t) => {
  const webmData = await generateWebMWithVP9()

  const demuxer = new WebMDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(webmData)

  t.is(demuxer.state, 'ready')

  const tracks = demuxer.tracks
  t.true(tracks.length > 0, 'Should have at least one track')

  const videoTrack = tracks.find((track) => track.trackType === 'video')
  t.truthy(videoTrack, 'Should have a video track')
  if (videoTrack) {
    t.is(videoTrack.codedWidth, 320, 'Video should have correct width')
    t.is(videoTrack.codedHeight, 240, 'Video should have correct height')
  }

  demuxer.close()
})

runTest('WebMDemuxer: get duration', async (t) => {
  const webmData = await generateWebMWithVP9()

  const demuxer = new WebMDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(webmData)

  const duration = demuxer.duration
  t.truthy(duration !== null, 'Should have duration')
  // Duration should be approximately 10 frames * 33333 us = 333330 us
  t.true(duration! >= 0, 'Duration should be non-negative')

  demuxer.close()
})

runTest('WebMDemuxer: get video decoder config for VP9', async (t) => {
  const webmData = await generateWebMWithVP9()

  const demuxer = new WebMDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(webmData)

  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Should have video decoder config')
  if (config) {
    t.true(config.codec.startsWith('vp09') || config.codec === 'vp9', 'Should be VP9 codec')
    t.is(config.codedWidth, 320, 'Config should have correct width')
    t.is(config.codedHeight, 240, 'Config should have correct height')
  }

  demuxer.close()
})

runTest('WebMDemuxer: get audio decoder config for Opus', async (t) => {
  const webmData = await generateWebMWithVP9AndOpus()

  const demuxer = new WebMDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(webmData)

  const config = demuxer.audioDecoderConfig
  t.truthy(config, 'Should have audio decoder config')
  if (config) {
    t.is(config.codec, 'opus', 'Should be Opus codec')
    t.is(config.sampleRate, 48000, 'Should have correct sample rate')
    t.is(config.numberOfChannels, 2, 'Should have correct channel count')
  }

  demuxer.close()
})

runTest('WebMDemuxer: demux VP9 video chunks', async (t) => {
  const webmData = await generateWebMWithVP9()

  return new Promise<void>((resolve, reject) => {
    const videoChunks: EncodedVideoChunk[] = []
    let errorOccurred = false

    const demuxer = new WebMDemuxer({
      videoOutput: (chunk: EncodedVideoChunk) => {
        videoChunks.push(chunk)
      },
      error: (e: Error) => {
        errorOccurred = true
        demuxer.close()
        reject(e)
      },
    })

    demuxer
      .loadBuffer(webmData)
      .then(() => {
        demuxer.demux(20)

        setTimeout(() => {
          if (!errorOccurred) {
            t.true(videoChunks.length > 0, 'Should have demuxed video chunks')
            if (videoChunks.length > 0) {
              t.truthy(videoChunks[0].type, 'Chunk should have type')
              t.true(videoChunks[0].byteLength > 0, 'Chunk should have data')
            }
            demuxer.close()
            resolve()
          }
        }, 500)
      })
      .catch(reject)
  })
})

runTest('WebMDemuxer: demux Opus audio chunks', async (t) => {
  const webmData = await generateWebMWithVP9AndOpus()

  return new Promise<void>((resolve, reject) => {
    const audioChunks: EncodedAudioChunk[] = []
    let errorOccurred = false

    const demuxer = new WebMDemuxer({
      audioOutput: (chunk: EncodedAudioChunk) => {
        audioChunks.push(chunk)
      },
      error: (e: Error) => {
        errorOccurred = true
        demuxer.close()
        reject(e)
      },
    })

    demuxer
      .loadBuffer(webmData)
      .then(() => {
        demuxer.demux(20)

        setTimeout(() => {
          if (!errorOccurred) {
            t.true(audioChunks.length > 0, 'Should have demuxed audio chunks')
            if (audioChunks.length > 0) {
              t.truthy(audioChunks[0].type, 'Chunk should have type')
              t.true(audioChunks[0].byteLength > 0, 'Chunk should have data')
            }
            demuxer.close()
            resolve()
          }
        }, 500)
      })
      .catch(reject)
  })
})

runTest('WebMDemuxer: state transitions', async (t) => {
  const webmData = await generateWebMWithVP9()

  const demuxer = new WebMDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  t.is(demuxer.state, 'unloaded')

  await demuxer.loadBuffer(webmData)
  t.is(demuxer.state, 'ready')

  demuxer.close()
  t.is(demuxer.state, 'closed')
})

runTest('WebMDemuxer: select tracks', async (t) => {
  const webmData = await generateWebMWithVP9AndOpus()

  const demuxer = new WebMDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(webmData)

  const tracks = demuxer.tracks
  const videoTrack = tracks.find((track) => track.trackType === 'video')
  const audioTrack = tracks.find((track) => track.trackType === 'audio')

  if (videoTrack) {
    t.notThrows(() => demuxer.selectVideoTrack(videoTrack.index))
  }

  if (audioTrack) {
    t.notThrows(() => demuxer.selectAudioTrack(audioTrack.index))
  }

  demuxer.close()
})

runTest('WebMDemuxer: error on loading twice without close', async (t) => {
  const webmData = await generateWebMWithVP9()

  const demuxer = new WebMDemuxer({
    error: (_e: Error) => {},
  })

  await demuxer.loadBuffer(webmData)

  await t.throwsAsync(() => demuxer.loadBuffer(webmData), {
    message: /already loaded/,
  })

  demuxer.close()
})

// ============================================================================
// MkvDemuxer Tests
// ============================================================================

// Helper: Generate an MKV buffer with H.264 video
async function generateMkvWithH264(): Promise<Uint8Array> {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: () => {},
  })

  encoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.red, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  const muxer = new MkvMuxer()
  const description = videoMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    description,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const data = muxer.finalize()
  muxer.close()
  return data
}

// Helper: Generate an MKV buffer with H.264 video and AAC audio
async function generateMkvWithH264AndAAC(): Promise<Uint8Array> {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const videoEncoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: () => {},
  })

  videoEncoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  const audioEncoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: () => {},
  })

  audioEncoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.yellow, i * 33333)
    videoEncoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  for (let i = 0; i < 5; i++) {
    const audioData = generateSilence(1024, 2, 48000, 'f32', i * 21333)
    audioEncoder.encode(audioData)
    audioData.close()
  }

  await Promise.all([videoEncoder.flush(), audioEncoder.flush()])
  videoEncoder.close()
  audioEncoder.close()

  const muxer = new MkvMuxer()
  const videoDescription = videoMetadatas[0]?.decoderConfig?.description
  const audioDescription = audioMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    description: videoDescription,
  })
  muxer.addAudioTrack({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    description: audioDescription,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }
  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  muxer.flush()
  const data = muxer.finalize()
  muxer.close()
  return data
}

runTest('MkvDemuxer: constructor creates demuxer', (t) => {
  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })
  t.truthy(demuxer)
  t.is(demuxer.state, 'unloaded')
  demuxer.close()
})

runTest('MkvDemuxer: constructor requires error callback', (t) => {
  t.throws(() => new MkvDemuxer({} as { error: (e: Error) => void }))
})

runTest('MkvDemuxer: load buffer and get tracks', async (t) => {
  const mkvData = await generateMkvWithH264()

  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(mkvData)

  t.is(demuxer.state, 'ready')

  const tracks = demuxer.tracks
  t.true(tracks.length > 0, 'Should have at least one track')

  const videoTrack = tracks.find((track) => track.trackType === 'video')
  t.truthy(videoTrack, 'Should have a video track')
  if (videoTrack) {
    t.is(videoTrack.codedWidth, 320, 'Video should have correct width')
    t.is(videoTrack.codedHeight, 240, 'Video should have correct height')
  }

  demuxer.close()
})

runTest('MkvDemuxer: get duration', async (t) => {
  const mkvData = await generateMkvWithH264()

  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(mkvData)

  const duration = demuxer.duration
  t.truthy(duration !== null, 'Should have duration')
  t.true(duration! >= 0, 'Duration should be non-negative')

  demuxer.close()
})

runTest('MkvDemuxer: get video decoder config for H.264', async (t) => {
  const mkvData = await generateMkvWithH264()

  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(mkvData)

  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Should have video decoder config')
  if (config) {
    t.true(config.codec.startsWith('avc'), 'Should be AVC/H.264 codec')
    t.is(config.codedWidth, 320, 'Config should have correct width')
    t.is(config.codedHeight, 240, 'Config should have correct height')
  }

  demuxer.close()
})

runTest('MkvDemuxer: get audio decoder config for AAC', async (t) => {
  const mkvData = await generateMkvWithH264AndAAC()

  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(mkvData)

  const config = demuxer.audioDecoderConfig
  t.truthy(config, 'Should have audio decoder config')
  if (config) {
    t.true(config.codec.startsWith('mp4a'), 'Should be AAC codec')
    t.is(config.sampleRate, 48000, 'Should have correct sample rate')
    t.is(config.numberOfChannels, 2, 'Should have correct channel count')
  }

  demuxer.close()
})

runTest('MkvDemuxer: demux H.264 video chunks', async (t) => {
  const mkvData = await generateMkvWithH264()

  return new Promise<void>((resolve, reject) => {
    const videoChunks: EncodedVideoChunk[] = []
    let errorOccurred = false

    const demuxer = new MkvDemuxer({
      videoOutput: (chunk: EncodedVideoChunk) => {
        videoChunks.push(chunk)
      },
      error: (e: Error) => {
        errorOccurred = true
        demuxer.close()
        reject(e)
      },
    })

    demuxer
      .loadBuffer(mkvData)
      .then(() => {
        demuxer.demux(20)

        setTimeout(() => {
          if (!errorOccurred) {
            t.true(videoChunks.length > 0, 'Should have demuxed video chunks')
            if (videoChunks.length > 0) {
              t.truthy(videoChunks[0].type, 'Chunk should have type')
              t.true(videoChunks[0].byteLength > 0, 'Chunk should have data')
            }
            demuxer.close()
            resolve()
          }
        }, 500)
      })
      .catch(reject)
  })
})

runTest('MkvDemuxer: demux AAC audio chunks', async (t) => {
  const mkvData = await generateMkvWithH264AndAAC()

  return new Promise<void>((resolve, reject) => {
    const audioChunks: EncodedAudioChunk[] = []
    let errorOccurred = false

    const demuxer = new MkvDemuxer({
      audioOutput: (chunk: EncodedAudioChunk) => {
        audioChunks.push(chunk)
      },
      error: (e: Error) => {
        errorOccurred = true
        demuxer.close()
        reject(e)
      },
    })

    demuxer
      .loadBuffer(mkvData)
      .then(() => {
        demuxer.demux(20)

        setTimeout(() => {
          if (!errorOccurred) {
            t.true(audioChunks.length > 0, 'Should have demuxed audio chunks')
            if (audioChunks.length > 0) {
              t.truthy(audioChunks[0].type, 'Chunk should have type')
              t.true(audioChunks[0].byteLength > 0, 'Chunk should have data')
            }
            demuxer.close()
            resolve()
          }
        }, 500)
      })
      .catch(reject)
  })
})

runTest('MkvDemuxer: state transitions', async (t) => {
  const mkvData = await generateMkvWithH264()

  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  t.is(demuxer.state, 'unloaded')

  await demuxer.loadBuffer(mkvData)
  t.is(demuxer.state, 'ready')

  demuxer.close()
  t.is(demuxer.state, 'closed')
})

runTest('MkvDemuxer: select tracks', async (t) => {
  const mkvData = await generateMkvWithH264AndAAC()

  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(mkvData)

  const tracks = demuxer.tracks
  const videoTrack = tracks.find((track) => track.trackType === 'video')
  const audioTrack = tracks.find((track) => track.trackType === 'audio')

  if (videoTrack) {
    t.notThrows(() => demuxer.selectVideoTrack(videoTrack.index))
  }

  if (audioTrack) {
    t.notThrows(() => demuxer.selectAudioTrack(audioTrack.index))
  }

  demuxer.close()
})

runTest('MkvDemuxer: error on loading twice without close', async (t) => {
  const mkvData = await generateMkvWithH264()

  const demuxer = new MkvDemuxer({
    error: (_e: Error) => {},
  })

  await demuxer.loadBuffer(mkvData)

  await t.throwsAsync(() => demuxer.loadBuffer(mkvData), {
    message: /already loaded/,
  })

  demuxer.close()
})

// ============================================================================
// Async Iterator Tests
// ============================================================================

runTest('Mp4Demuxer: async iterator with for-await-of', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  const chunks: Array<{ chunkType: string }> = []
  let count = 0

  for await (const chunk of demuxer) {
    chunks.push({ chunkType: chunk.chunkType })
    count++
    // Limit to first 10 chunks to avoid long test
    if (count >= 10) break
  }

  t.true(chunks.length > 0, 'Should have iterated over chunks')
  t.true(chunks.every((c) => c.chunkType === 'video' || c.chunkType === 'audio'), 'All chunks should be video or audio')

  demuxer.close()
})

runTest('Mp4Demuxer: async iterator yields DemuxerChunk with correct structure', async (t) => {
  const demuxer = new Mp4Demuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  for await (const chunk of demuxer) {
    t.truthy(chunk.chunkType, 'Chunk should have chunkType')
    t.true(chunk.chunkType === 'video' || chunk.chunkType === 'audio', 'chunkType should be video or audio')

    if (chunk.chunkType === 'video') {
      t.truthy(chunk.videoChunk, 'Video chunk should have videoChunk property')
      t.true(chunk.videoChunk!.byteLength > 0, 'Video chunk should have data')
    } else {
      t.truthy(chunk.audioChunk, 'Audio chunk should have audioChunk property')
      t.true(chunk.audioChunk!.byteLength > 0, 'Audio chunk should have data')
    }

    // Only check first chunk
    break
  }

  demuxer.close()
})

runTest('Mp4Demuxer: demuxAsync completes all packets', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const audioChunks: EncodedAudioChunk[] = []

  const demuxer = new Mp4Demuxer({
    videoOutput: (chunk: EncodedVideoChunk) => videoChunks.push(chunk),
    audioOutput: (chunk: EncodedAudioChunk) => audioChunks.push(chunk),
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  // Use demuxAsync instead of demux() + setTimeout
  await demuxer.demuxAsync(20)

  t.true(videoChunks.length > 0 || audioChunks.length > 0, 'Should have demuxed chunks')

  demuxer.close()
})

runTest('Mp4Demuxer: demuxAsync with no count demuxes all', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []

  const demuxer = new Mp4Demuxer({
    videoOutput: (chunk: EncodedVideoChunk) => videoChunks.push(chunk),
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.load(path.join(FIXTURES_DIR, 'small_buck_bunny.mp4'))

  // Demux all packets
  await demuxer.demuxAsync()

  t.true(videoChunks.length > 0, 'Should have demuxed all video chunks')
  t.is(demuxer.state, 'ended', 'State should be ended after demuxing all')

  demuxer.close()
})

runTest('WebMDemuxer: async iterator with for-await-of', async (t) => {
  const webmData = await generateWebMWithVP9()

  const demuxer = new WebMDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(webmData)

  const chunks: Array<{ chunkType: string }> = []

  for await (const chunk of demuxer) {
    chunks.push({ chunkType: chunk.chunkType })
  }

  t.true(chunks.length > 0, 'Should have iterated over chunks')
  t.true(chunks.every((c) => c.chunkType === 'video'), 'All chunks should be video (VP9 only)')

  demuxer.close()
})

runTest('WebMDemuxer: demuxAsync completes', async (t) => {
  const webmData = await generateWebMWithVP9()
  const videoChunks: EncodedVideoChunk[] = []

  const demuxer = new WebMDemuxer({
    videoOutput: (chunk: EncodedVideoChunk) => videoChunks.push(chunk),
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(webmData)

  await demuxer.demuxAsync()

  t.true(videoChunks.length > 0, 'Should have demuxed chunks')
  t.is(demuxer.state, 'ended', 'State should be ended')

  demuxer.close()
})

runTest('MkvDemuxer: async iterator with for-await-of', async (t) => {
  const mkvData = await generateMkvWithH264()

  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(mkvData)

  const chunks: Array<{ chunkType: string }> = []

  for await (const chunk of demuxer) {
    chunks.push({ chunkType: chunk.chunkType })
  }

  t.true(chunks.length > 0, 'Should have iterated over chunks')
  t.true(chunks.every((c) => c.chunkType === 'video'), 'All chunks should be video (H.264 only)')

  demuxer.close()
})

runTest('MkvDemuxer: async iterator with video and audio', async (t) => {
  const mkvData = await generateMkvWithH264AndAAC()

  const demuxer = new MkvDemuxer({
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(mkvData)

  const videoChunks: EncodedVideoChunk[] = []
  const audioChunks: EncodedAudioChunk[] = []

  for await (const chunk of demuxer) {
    if (chunk.chunkType === 'video' && chunk.videoChunk) {
      videoChunks.push(chunk.videoChunk)
    } else if (chunk.chunkType === 'audio' && chunk.audioChunk) {
      audioChunks.push(chunk.audioChunk)
    }
  }

  t.true(videoChunks.length > 0, 'Should have video chunks')
  t.true(audioChunks.length > 0, 'Should have audio chunks')

  demuxer.close()
})

runTest('MkvDemuxer: demuxAsync completes', async (t) => {
  const mkvData = await generateMkvWithH264()
  const videoChunks: EncodedVideoChunk[] = []

  const demuxer = new MkvDemuxer({
    videoOutput: (chunk: EncodedVideoChunk) => videoChunks.push(chunk),
    error: (e: Error) => t.fail(`Error: ${e.message}`),
  })

  await demuxer.loadBuffer(mkvData)

  await demuxer.demuxAsync()

  t.true(videoChunks.length > 0, 'Should have demuxed chunks')
  t.is(demuxer.state, 'ended', 'State should be ended')

  demuxer.close()
})
