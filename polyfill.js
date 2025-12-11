const webcodecs = require('./index.js')

// WebCodecs classes (W3C spec)
globalThis.VideoEncoder ??= webcodecs.VideoEncoder
globalThis.VideoDecoder ??= webcodecs.VideoDecoder
globalThis.AudioEncoder ??= webcodecs.AudioEncoder
globalThis.AudioDecoder ??= webcodecs.AudioDecoder
globalThis.VideoFrame ??= webcodecs.VideoFrame
globalThis.AudioData ??= webcodecs.AudioData
globalThis.EncodedVideoChunk ??= webcodecs.EncodedVideoChunk
globalThis.EncodedAudioChunk ??= webcodecs.EncodedAudioChunk
globalThis.ImageDecoder ??= webcodecs.ImageDecoder
globalThis.VideoColorSpace ??= webcodecs.VideoColorSpace

// Supporting types (W3C spec)
globalThis.ImageTrack ??= webcodecs.ImageTrack
globalThis.ImageTrackList ??= webcodecs.ImageTrackList

// DOM types needed by WebCodecs (not available in Node.js)
globalThis.DOMRectReadOnly ??= webcodecs.DOMRectReadOnly
