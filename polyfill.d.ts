import type {
  AudioData,
  AudioDecoder,
  AudioEncoder,
  DOMRectReadOnly,
  EncodedAudioChunk,
  EncodedVideoChunk,
  ImageDecoder,
  ImageTrack,
  ImageTrackList,
  VideoColorSpace,
  VideoDecoder,
  VideoEncoder,
  VideoFrame,
} from './index'

declare global {
  var VideoEncoder: typeof import('./index').VideoEncoder
  var VideoDecoder: typeof import('./index').VideoDecoder
  var AudioEncoder: typeof import('./index').AudioEncoder
  var AudioDecoder: typeof import('./index').AudioDecoder
  var VideoFrame: typeof import('./index').VideoFrame
  var AudioData: typeof import('./index').AudioData
  var EncodedVideoChunk: typeof import('./index').EncodedVideoChunk
  var EncodedAudioChunk: typeof import('./index').EncodedAudioChunk
  var ImageDecoder: typeof import('./index').ImageDecoder
  var VideoColorSpace: typeof import('./index').VideoColorSpace
  var ImageTrack: typeof import('./index').ImageTrack
  var ImageTrackList: typeof import('./index').ImageTrackList
  var DOMRectReadOnly: typeof import('./index').DOMRectReadOnly
}

export {}
