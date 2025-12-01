//! WebCodecs API implementation
//!
//! Provides spec-compliant WebCodecs API surface for Node.js via NAPI-RS.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/WebCodecs_API

mod encoded_video_chunk;
mod hardware;
mod video_decoder;
mod video_encoder;
mod video_frame;

pub use encoded_video_chunk::{
    EncodedVideoChunk, EncodedVideoChunkInit, EncodedVideoChunkType,
    VideoDecoderConfig, VideoEncoderConfig,
};
pub use hardware::{
    get_available_hardware_accelerators, get_hardware_accelerators,
    get_preferred_hardware_accelerator, is_hardware_accelerator_available,
    HardwareAccelerator,
};
pub use video_decoder::{VideoDecoder, VideoDecoderSupport};
pub use video_encoder::{
    CodecState, EncodedVideoChunkMetadata, VideoDecoderConfigOutput, VideoEncoder,
    VideoEncoderEncodeOptions, VideoEncoderSupport,
};
pub use video_frame::{
    VideoColorSpace, VideoFrame, VideoFrameCopyToOptions, VideoFrameInit, VideoFrameRect,
    VideoPixelFormat,
};
