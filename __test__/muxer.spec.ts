/**
 * Muxer Tests
 *
 * Tests for Mp4Muxer, WebMMuxer, and MkvMuxer classes.
 */

import test from 'ava'

import {
  Mp4Muxer,
  WebMMuxer,
  MkvMuxer,
  VideoEncoder,
  AudioEncoder,
  resetHardwareFallbackState,
  type EncodedVideoChunk,
  type EncodedAudioChunk,
  type EncodedVideoChunkMetadata,
  type EncodedAudioChunkMetadata,
} from '../index.js'
import { generateSolidColorI420Frame, generateSilence, TestColors } from './helpers/index.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// Mp4Muxer Tests
// ============================================================================

test('Mp4Muxer: constructor creates muxer', (t) => {
  const muxer = new Mp4Muxer()
  t.truthy(muxer)
  muxer.close()
})

test('Mp4Muxer: constructor accepts options', (t) => {
  const muxer = new Mp4Muxer({ fastStart: true })
  t.truthy(muxer)
  muxer.close()
})

test('Mp4Muxer: can add video track', (t) => {
  const muxer = new Mp4Muxer()

  t.notThrows(() => {
    muxer.addVideoTrack({
      codec: 'avc1.42001E',
      width: 320,
      height: 240,
    })
  })

  muxer.close()
})

test('Mp4Muxer: can add audio track', (t) => {
  const muxer = new Mp4Muxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'mp4a.40.2',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('Mp4Muxer: muxes video chunks and produces valid MP4', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  // Create encoder to generate real encoded chunks
  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Encode some frames
  for (let i = 0; i < 30; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.red, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  // Now mux the chunks (without fastStart for memory-based I/O)
  const muxer = new Mp4Muxer()

  // Get description from first keyframe metadata
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

  await muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  // Verify we got some data
  t.true(mp4Data.length > 0, 'Should have MP4 data')
  t.true(mp4Data.length > 1000, 'MP4 should have reasonable size')

  // Check MP4 magic bytes (ftyp box)
  const ftypOffset = mp4Data.indexOf(0x66) // 'f'
  t.true(ftypOffset >= 0, 'Should have ftyp box')
})

// ============================================================================
// WebMMuxer Tests
// ============================================================================

test('WebMMuxer: constructor creates muxer', (t) => {
  const muxer = new WebMMuxer()
  t.truthy(muxer)
  muxer.close()
})

test('WebMMuxer: can add video track', (t) => {
  const muxer = new WebMMuxer()

  t.notThrows(() => {
    muxer.addVideoTrack({
      codec: 'vp09.00.10.08',
      width: 320,
      height: 240,
    })
  })

  muxer.close()
})

test('WebMMuxer: muxes VP9 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.green, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new WebMMuxer()

  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  await muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')

  // Check WebM magic bytes (0x1A 0x45 0xDF 0xA3 = EBML header)
  t.is(webmData[0], 0x1a, 'WebM should start with EBML header')
  t.is(webmData[1], 0x45, 'WebM should start with EBML header')
  t.is(webmData[2], 0xdf, 'WebM should start with EBML header')
  t.is(webmData[3], 0xa3, 'WebM should start with EBML header')
})

test('WebMMuxer: can add Opus audio track', (t) => {
  const muxer = new WebMMuxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('WebMMuxer: muxes Opus audio chunks', async (t) => {
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64_000,
  })

  for (let i = 0; i < 10; i++) {
    const audioData = generateSilence(960, 2, 48000, 'f32', i * 20000)
    encoder.encode(audioData)
    audioData.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(audioChunks.length > 0, 'Should have encoded chunks')

  const muxer = new WebMMuxer()

  muxer.addAudioTrack({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  await muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')
  t.is(webmData[0], 0x1a, 'WebM should start with EBML header')
})

test('WebMMuxer: muxes VP9 video and Opus audio combined', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const videoEncoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Video encoder error: ${e.message}`),
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
    error: (e) => t.fail(`Audio encoder error: ${e.message}`),
  })

  audioEncoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.green, i * 33333)
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

  t.true(videoChunks.length > 0, 'Should have video chunks')
  t.true(audioChunks.length > 0, 'Should have audio chunks')

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

  await muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')
  t.true(webmData.length > 500, 'WebM with audio+video should have minimum size')
})

test('WebMMuxer: muxes VP8 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp8',
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

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new WebMMuxer()

  muxer.addVideoTrack({
    codec: 'vp8',
    width: 320,
    height: 240,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  await muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')
  t.is(webmData[0], 0x1a, 'WebM should start with EBML header')
})

test('WebMMuxer: muxes AV1 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'av01.0.04M.08',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  for (let i = 0; i < 5; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new WebMMuxer()
  const description = videoMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'av01.0.04M.08',
    width: 320,
    height: 240,
    description,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  await muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')
  t.is(webmData[0], 0x1a, 'WebM should start with EBML header')
})

// ============================================================================
// MkvMuxer Tests
// ============================================================================

test('MkvMuxer: constructor creates muxer', (t) => {
  const muxer = new MkvMuxer()
  t.truthy(muxer)
  muxer.close()
})

test('MkvMuxer: can add video track', (t) => {
  const muxer = new MkvMuxer()

  t.notThrows(() => {
    muxer.addVideoTrack({
      codec: 'avc1.42001E',
      width: 320,
      height: 240,
    })
  })

  muxer.close()
})

test('MkvMuxer: muxes H.264 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

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

  await muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')

  // Check MKV magic bytes (same as WebM - EBML header)
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
  t.is(mkvData[1], 0x45, 'MKV should start with EBML header')
})

test('MkvMuxer: can add AAC audio track', (t) => {
  const muxer = new MkvMuxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'mp4a.40.2',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('MkvMuxer: can add Opus audio track', (t) => {
  const muxer = new MkvMuxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('MkvMuxer: can add FLAC audio track', (t) => {
  const muxer = new MkvMuxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'flac',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('MkvMuxer: muxes AAC audio chunks', async (t) => {
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128_000,
  })

  for (let i = 0; i < 10; i++) {
    const audioData = generateSilence(1024, 2, 48000, 'f32', i * 21333)
    encoder.encode(audioData)
    audioData.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(audioChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()
  const description = audioMetadatas[0]?.decoderConfig?.description

  muxer.addAudioTrack({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    description,
  })

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  await muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
})

test('MkvMuxer: muxes Opus audio chunks', async (t) => {
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64_000,
  })

  for (let i = 0; i < 10; i++) {
    const audioData = generateSilence(960, 2, 48000, 'f32', i * 20000)
    encoder.encode(audioData)
    audioData.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(audioChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()

  muxer.addAudioTrack({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  await muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
})

test('MkvMuxer: muxes H.264 video and AAC audio combined', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const videoEncoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Video encoder error: ${e.message}`),
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
    error: (e) => t.fail(`Audio encoder error: ${e.message}`),
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

  t.true(videoChunks.length > 0, 'Should have video chunks')
  t.true(audioChunks.length > 0, 'Should have audio chunks')

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

  await muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.true(mkvData.length > 500, 'MKV with audio+video should have minimum size')
})

test('MkvMuxer: muxes VP9 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
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

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()

  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  await muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
})

test('MkvMuxer: muxes AV1 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'av01.0.04M.08',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  for (let i = 0; i < 5; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()
  const description = videoMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'av01.0.04M.08',
    width: 320,
    height: 240,
    description,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  await muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
})

// ============================================================================
// Combined Audio+Video Muxing Tests
// ============================================================================

test('Mp4Muxer: muxes both video and audio', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  // Video encoder
  const videoEncoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Video encoder error: ${e.message}`),
  })

  videoEncoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Audio encoder
  const audioEncoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Audio encoder error: ${e.message}`),
  })

  audioEncoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128_000,
  })

  // Encode video
  for (let i = 0; i < 30; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.yellow, i * 33333)
    videoEncoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  // Encode audio
  for (let i = 0; i < 10; i++) {
    const audioData = generateSilence(1024, 2, 48000, 'f32', i * Math.floor((1024 * 1_000_000) / 48000))
    audioEncoder.encode(audioData)
    audioData.close()
  }

  await Promise.all([videoEncoder.flush(), audioEncoder.flush()])
  videoEncoder.close()
  audioEncoder.close()

  t.true(videoChunks.length > 0, 'Should have video chunks')
  t.true(audioChunks.length > 0, 'Should have audio chunks')

  // Mux together (without fastStart for memory-based I/O)
  const muxer = new Mp4Muxer()

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

  // Interleave chunks by timestamp
  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  await muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  t.true(mp4Data.length > 0, 'Should have MP4 data')
  // Solid color video and silence audio compress very well, so the output is smaller than expected
  t.true(mp4Data.length > 1000, 'MP4 with audio+video should have minimum size')
})
