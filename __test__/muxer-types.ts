import type { EncodedAudioChunk, EncodedVideoChunk, MkvMuxer, Mp4Muxer, WebMMuxer } from '../index.js'

export function checkSharedArrayBufferMuxerInputs(
  mp4: Mp4Muxer,
  webm: WebMMuxer,
  mkv: MkvMuxer,
  videoChunk: EncodedVideoChunk,
  audioChunk: EncodedAudioChunk,
): void {
  const description = new SharedArrayBuffer(8)

  mp4.addVideoTrack({ codec: 'avc1.42001E', width: 2, height: 2, description })
  mp4.addAudioTrack({ codec: 'mp4a.40.2', sampleRate: 48000, numberOfChannels: 2, description })
  webm.addVideoTrack({ codec: 'vp8', width: 2, height: 2, description })
  webm.addAudioTrack({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2, description })
  mkv.addVideoTrack({ codec: 'avc1.42001E', width: 2, height: 2, description })
  mkv.addAudioTrack({ codec: 'mp4a.40.2', sampleRate: 48000, numberOfChannels: 2, description })

  mp4.addVideoChunk(videoChunk, { decoderConfig: { description } })
  mp4.addAudioChunk(audioChunk, { decoderConfig: { description } })
}
