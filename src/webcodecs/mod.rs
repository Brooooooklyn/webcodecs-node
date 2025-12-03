//! WebCodecs API implementation
//!
//! Provides spec-compliant WebCodecs API surface for Node.js via NAPI-RS.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/WebCodecs_API

mod audio_data;
mod audio_decoder;
mod audio_encoder;
pub mod codec_string;
mod encoded_audio_chunk;
mod encoded_video_chunk;
pub mod error;
mod hardware;
mod image_decoder;
mod video_decoder;
mod video_encoder;
mod video_frame;

pub use audio_data::{AudioData, AudioDataCopyToOptions, AudioDataInit, AudioSampleFormat};
pub use audio_decoder::AudioDecoder;
pub use audio_encoder::{
  AudioDecoderConfigOutput, AudioEncoder, AudioEncoderEncodeOptions, EncodedAudioChunkMetadata,
};
pub use encoded_audio_chunk::{
  AudioDecoderConfig, AudioDecoderSupport, AudioEncoderConfig, AudioEncoderSupport,
  EncodedAudioChunk, EncodedAudioChunkInit, EncodedAudioChunkType,
};
pub use encoded_video_chunk::{
  EncodedVideoChunk, EncodedVideoChunkInit, EncodedVideoChunkType, VideoDecoderConfig,
  VideoEncoderConfig,
};
pub(crate) use encoded_video_chunk::EncodedVideoChunkInner;
pub use hardware::{
  get_available_hardware_accelerators, get_hardware_accelerators,
  get_preferred_hardware_accelerator, is_hardware_accelerator_available, HardwareAccelerator,
};
pub use image_decoder::{
  ImageDecodeOptions, ImageDecodeResult, ImageDecoder, ImageDecoderInit, ImageTrack, ImageTrackList,
};
pub use video_decoder::{VideoDecoder, VideoDecoderSupport};
pub use video_encoder::{
  CodecState, EncodedVideoChunkMetadata, VideoDecoderConfigOutput, VideoEncoder,
  VideoEncoderEncodeOptions, VideoEncoderSupport,
};
pub use video_frame::{
  DOMRectReadOnly, VideoColorSpace, VideoFrame, VideoFrameBufferInit, VideoFrameCopyToOptions,
  VideoFrameInit, VideoFrameRect, VideoPixelFormat,
};
