//! WebCodecs API implementation
//!
//! Provides spec-compliant WebCodecs API surface for Node.js via NAPI-RS.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/WebCodecs_API

mod audio_data;
mod audio_decoder;
mod audio_encoder;
pub mod codec_string;
pub mod demuxer_base;
mod encoded_audio_chunk;
mod encoded_video_chunk;
pub mod error;
mod hardware;
pub(crate) mod hw_fallback;
mod image_decoder;
mod mkv_demuxer;
mod mkv_muxer;
mod mp4_demuxer;
mod mp4_muxer;
pub mod muxer_base;
mod promise_reject;
mod video_decoder;
mod video_encoder;
mod video_frame;
mod webm_demuxer;
mod webm_muxer;

pub use audio_data::{AudioData, AudioDataCopyToOptions, AudioDataInit, AudioSampleFormat};
pub use audio_decoder::AudioDecoder;
pub use audio_encoder::{
  AudioDecoderConfigOutput, AudioEncoder, AudioEncoderEncodeOptions, EncodedAudioChunkMetadata,
};
pub use encoded_audio_chunk::{
  AacBitstreamFormat, AacEncoderConfig, AudioDecoderConfig, AudioDecoderSupport,
  AudioEncoderConfig, AudioEncoderSupport, BitrateMode, EncodedAudioChunk, EncodedAudioChunkInit,
  EncodedAudioChunkType, FlacEncoderConfig, OpusApplication, OpusBitstreamFormat,
  OpusEncoderConfig, OpusSignal,
};
pub(crate) use encoded_video_chunk::EncodedVideoChunkInner;
pub use encoded_video_chunk::{
  AlphaOption, AvcBitstreamFormat, AvcEncoderConfig, EncodedVideoChunk, EncodedVideoChunkInit,
  EncodedVideoChunkType, HardwareAcceleration, HevcBitstreamFormat, HevcEncoderConfig, LatencyMode,
  VideoDecoderConfig, VideoEncoderBitrateMode, VideoEncoderConfig,
};
pub(crate) use encoded_video_chunk::{
  convert_annexb_extradata_to_avcc, convert_annexb_extradata_to_hvcc,
  convert_avcc_extradata_to_annexb, convert_avcc_to_annexb, convert_hvcc_extradata_to_annexb,
  convert_obu_extradata_to_av1c, extract_avcc_from_avcc_packet, extract_hvcc_from_hvcc_packet,
  is_av1c_extradata, is_avcc_extradata, is_avcc_format, is_hvcc_extradata,
};
pub use hardware::{
  HardwareAccelerator, get_available_hardware_accelerators, get_hardware_accelerators,
  get_preferred_hardware_accelerator, is_hardware_accelerator_available,
};
pub use hw_fallback::reset_hardware_fallback_state;
pub use image_decoder::{
  ImageDecodeOptions, ImageDecodeResult, ImageDecoder, ImageDecoderInit, ImageTrack, ImageTrackList,
};
pub use mkv_muxer::{MkvAudioTrackConfig, MkvMuxer, MkvMuxerOptions, MkvVideoTrackConfig};
pub use mp4_muxer::{Mp4AudioTrackConfig, Mp4Muxer, Mp4MuxerOptions, Mp4VideoTrackConfig};
pub use video_decoder::{VideoDecoder, VideoDecoderSupport};
pub use video_encoder::{
  CodecState, EncodedVideoChunkMetadata, SvcOutputMetadata, VideoDecoderConfigOutput, VideoEncoder,
  VideoEncoderEncodeOptions, VideoEncoderEncodeOptionsForAv1, VideoEncoderEncodeOptionsForAvc,
  VideoEncoderEncodeOptionsForHevc, VideoEncoderEncodeOptionsForVp9, VideoEncoderSupport,
};
pub use video_frame::{
  DOMRectReadOnly, VideoColorPrimaries, VideoColorSpace, VideoColorSpaceInit, VideoFrame,
  VideoFrameBufferInit, VideoFrameCopyToOptions, VideoFrameInit, VideoFrameMetadata,
  VideoFrameRect, VideoMatrixCoefficients, VideoPixelFormat, VideoTransferCharacteristics,
};
pub use webm_muxer::{WebMAudioTrackConfig, WebMMuxer, WebMMuxerOptions, WebMVideoTrackConfig};
// Demuxer types
pub use demuxer_base::{DemuxerAudioDecoderConfig, DemuxerTrackInfo, DemuxerVideoDecoderConfig};
pub use mkv_demuxer::{MkvDemuxer, MkvDemuxerInit};
pub use mp4_demuxer::{Mp4Demuxer, Mp4DemuxerInit};
pub use muxer_base::StreamingMuxerOptions;
pub use webm_demuxer::{WebMDemuxer, WebMDemuxerInit};
