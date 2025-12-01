//! AudioDecoder - WebCodecs API implementation
//!
//! Provides audio decoding functionality using FFmpeg.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/AudioDecoder

use crate::codec::{AudioDecoderConfig as InternalAudioDecoderConfig, CodecContext, Frame, Packet};
use crate::ffi::AVCodecID;
use crate::webcodecs::{AudioData, AudioDecoderConfig, AudioDecoderSupport, EncodedAudioChunk};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::sync::{Arc, Mutex};

use super::video_encoder::CodecState;

/// Type alias for output callback (takes AudioData)
type OutputCallback = ThreadsafeFunction<AudioData>;

/// Type alias for error callback (takes error message)
type ErrorCallback = ThreadsafeFunction<String>;

/// Internal decoder state
struct AudioDecoderInner {
    state: CodecState,
    config: Option<InternalAudioDecoderConfig>,
    context: Option<CodecContext>,
    codec_string: String,
    frame_count: u64,
    /// Queued output frames (for synchronous retrieval)
    output_queue: Vec<AudioData>,
    /// Optional output callback (WebCodecs spec compliant mode)
    output_callback: Option<OutputCallback>,
    /// Optional error callback (WebCodecs spec compliant mode)
    error_callback: Option<ErrorCallback>,
}

/// AudioDecoder - WebCodecs-compliant audio decoder
///
/// Decodes EncodedAudioChunk objects into AudioData objects using FFmpeg.
///
/// Note: This implementation uses a synchronous output queue model instead of
/// callbacks for simpler integration. Use `takeDecodedAudio()` to retrieve
/// decoded output after calling `decode()` or `flush()`.
#[napi]
pub struct AudioDecoder {
    inner: Arc<Mutex<AudioDecoderInner>>,
}

#[napi]
impl AudioDecoder {
    /// Create a new AudioDecoder (queue-based mode)
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        let inner = AudioDecoderInner {
            state: CodecState::Unconfigured,
            config: None,
            context: None,
            codec_string: String::new(),
            frame_count: 0,
            output_queue: Vec::new(),
            output_callback: None,
            error_callback: None,
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Create an AudioDecoder with callbacks (WebCodecs spec compliant mode)
    ///
    /// In this mode, decoded audio is delivered via the output callback
    /// instead of being queued for retrieval. Errors are reported via the
    /// error callback and the decoder transitions to the Closed state.
    ///
    /// Example:
    /// ```javascript
    /// const decoder = AudioDecoder.withCallbacks(
    ///   (audio) => { /* handle output */ },
    ///   (error) => { /* handle error */ }
    /// );
    /// ```
    #[napi(factory)]
    pub fn with_callbacks(
        output: ThreadsafeFunction<AudioData>,
        error: ThreadsafeFunction<String>,
    ) -> Result<Self> {
        let inner = AudioDecoderInner {
            state: CodecState::Unconfigured,
            config: None,
            context: None,
            codec_string: String::new(),
            frame_count: 0,
            output_queue: Vec::new(),
            output_callback: Some(output),
            error_callback: Some(error),
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Report an error via callback (if in callback mode) and close the decoder
    /// Returns true if error was reported via callback, false if should return error
    fn report_error(inner: &mut AudioDecoderInner, error_msg: &str) -> bool {
        if let Some(ref callback) = inner.error_callback {
            callback.call(Ok(error_msg.to_string()), ThreadsafeFunctionCallMode::NonBlocking);
            inner.state = CodecState::Closed;
            true
        } else {
            false
        }
    }

    /// Get decoder state
    #[napi(getter)]
    pub fn state(&self) -> Result<CodecState> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.state)
    }

    /// Get number of pending output audio data objects
    #[napi(getter)]
    pub fn decode_queue_size(&self) -> Result<u32> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.output_queue.len() as u32)
    }

    /// Configure the decoder
    #[napi]
    pub fn configure(&self, config: AudioDecoderConfig) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state == CodecState::Closed {
            return Err(Error::new(Status::GenericFailure, "Decoder is closed"));
        }

        // Parse codec string to determine codec ID
        let codec_id = parse_audio_codec_string(&config.codec)?;

        // Create decoder context
        let mut context = CodecContext::new_decoder(codec_id).map_err(|e| {
            Error::new(
                Status::GenericFailure,
                format!("Failed to create decoder: {}", e),
            )
        })?;

        // Configure decoder
        let decoder_config = InternalAudioDecoderConfig {
            codec_id,
            sample_rate: config.sample_rate.unwrap_or(0),
            channels: config.number_of_channels.unwrap_or(0),
            thread_count: 0, // Auto
            extradata: config.description.as_ref().map(|d| d.to_vec()),
        };

        context.configure_audio_decoder(&decoder_config).map_err(|e| {
            Error::new(
                Status::GenericFailure,
                format!("Failed to configure decoder: {}", e),
            )
        })?;

        // Open the decoder
        context.open().map_err(|e| {
            Error::new(
                Status::GenericFailure,
                format!("Failed to open decoder: {}", e),
            )
        })?;

        inner.context = Some(context);
        inner.config = Some(decoder_config);
        inner.codec_string = config.codec;
        inner.state = CodecState::Configured;
        inner.frame_count = 0;
        inner.output_queue.clear();

        Ok(())
    }

    /// Decode an encoded audio chunk
    #[napi]
    pub fn decode(&self, chunk: &EncodedAudioChunk) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            let msg = "Decoder not configured";
            if Self::report_error(&mut inner, msg) {
                return Ok(());
            }
            return Err(Error::new(Status::GenericFailure, msg));
        }

        // Get chunk data
        let data = match chunk.get_data_vec() {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("Failed to get chunk data: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };
        let timestamp = match chunk.get_timestamp() {
            Ok(ts) => ts,
            Err(e) => {
                let msg = format!("Failed to get timestamp: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };

        // Get context
        let context = match inner.context.as_mut() {
            Some(ctx) => ctx,
            None => {
                let msg = "No decoder context";
                if Self::report_error(&mut inner, msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Decode using the internal implementation
        let frames = match decode_audio_chunk_data(context, &data, timestamp) {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("Decode failed: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };

        inner.frame_count += 1;

        // Convert internal frames to AudioData and dispatch via callback or queue
        for frame in frames {
            let pts = frame.pts();
            let audio_data = AudioData::from_internal(frame, pts);

            // Dispatch output via callback or queue
            if let Some(ref callback) = inner.output_callback {
                callback.call(Ok(audio_data), ThreadsafeFunctionCallMode::NonBlocking);
            } else {
                inner.output_queue.push(audio_data);
            }
        }

        Ok(())
    }

    /// Flush the decoder and return all remaining audio data
    /// Returns a Promise that resolves when flushing is complete
    #[napi]
    pub async fn flush(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            let msg = "Decoder not configured";
            if Self::report_error(&mut inner, msg) {
                return Ok(());
            }
            return Err(Error::new(Status::GenericFailure, msg));
        }

        let context = match inner.context.as_mut() {
            Some(ctx) => ctx,
            None => {
                let msg = "No decoder context";
                if Self::report_error(&mut inner, msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Flush decoder
        let frames = match context.flush_decoder() {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("Flush failed: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Convert and dispatch remaining frames via callback or queue
        for frame in frames {
            let pts = frame.pts();
            let audio_data = AudioData::from_internal(frame, pts);

            // Dispatch output via callback or queue
            if let Some(ref callback) = inner.output_callback {
                callback.call(Ok(audio_data), ThreadsafeFunctionCallMode::NonBlocking);
            } else {
                inner.output_queue.push(audio_data);
            }
        }

        Ok(())
    }

    /// Take all decoded audio from the output queue
    #[napi]
    pub fn take_decoded_audio(&self) -> Result<Vec<AudioData>> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        let audio: Vec<AudioData> = inner.output_queue.drain(..).collect();
        Ok(audio)
    }

    /// Check if there are any pending decoded audio data
    #[napi]
    pub fn has_output(&self) -> Result<bool> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(!inner.output_queue.is_empty())
    }

    /// Take the next decoded audio data from the output queue (if any)
    #[napi]
    pub fn take_next_audio(&self) -> Result<Option<AudioData>> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.output_queue.is_empty() {
            Ok(None)
        } else {
            let audio = inner.output_queue.remove(0);
            Ok(Some(audio))
        }
    }

    /// Reset the decoder
    #[napi]
    pub fn reset(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state == CodecState::Closed {
            return Err(Error::new(Status::GenericFailure, "Decoder is closed"));
        }

        // Drop existing context
        inner.context = None;
        inner.config = None;
        inner.codec_string.clear();
        inner.state = CodecState::Unconfigured;
        inner.frame_count = 0;
        inner.output_queue.clear();

        Ok(())
    }

    /// Close the decoder
    #[napi]
    pub fn close(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        inner.context = None;
        inner.config = None;
        inner.codec_string.clear();
        inner.state = CodecState::Closed;
        inner.output_queue.clear();

        Ok(())
    }

    /// Check if a configuration is supported
    /// Returns a Promise that resolves with support information
    #[napi]
    pub async fn is_config_supported(config: AudioDecoderConfig) -> Result<AudioDecoderSupport> {
        // Parse codec string
        let codec_id = match parse_audio_codec_string(&config.codec) {
            Ok(id) => id,
            Err(_) => {
                return Ok(AudioDecoderSupport {
                    supported: false,
                    config,
                });
            }
        };

        // Try to create decoder
        let result = CodecContext::new_decoder(codec_id);

        Ok(AudioDecoderSupport {
            supported: result.is_ok(),
            config,
        })
    }
}

/// Parse WebCodecs audio codec string to FFmpeg codec ID
fn parse_audio_codec_string(codec: &str) -> Result<AVCodecID> {
    let codec_lower = codec.to_lowercase();

    // AAC variants
    if codec_lower.starts_with("mp4a.40") || codec_lower == "aac" {
        return Ok(AVCodecID::Aac);
    }

    // Opus
    if codec_lower == "opus" {
        return Ok(AVCodecID::Opus);
    }

    // MP3
    if codec_lower == "mp3" || codec_lower == "mp4a.6b" {
        return Ok(AVCodecID::Mp3);
    }

    // FLAC
    if codec_lower == "flac" {
        return Ok(AVCodecID::Flac);
    }

    // Vorbis
    if codec_lower == "vorbis" {
        return Ok(AVCodecID::Vorbis);
    }

    // PCM variants
    if codec_lower == "pcm-s16" || codec_lower == "pcm_s16le" {
        return Ok(AVCodecID::PcmS16le);
    }
    if codec_lower == "pcm-f32" || codec_lower == "pcm_f32le" {
        return Ok(AVCodecID::PcmF32le);
    }

    // AC3/E-AC3
    if codec_lower == "ac3" || codec_lower == "ac-3" {
        return Ok(AVCodecID::Ac3);
    }

    // ALAC (Apple Lossless)
    if codec_lower == "alac" {
        return Ok(AVCodecID::Alac);
    }

    Err(Error::new(
        Status::GenericFailure,
        format!("Unsupported audio codec: {}", codec),
    ))
}

/// Decode audio chunk data using FFmpeg
fn decode_audio_chunk_data(
    context: &mut CodecContext,
    data: &[u8],
    timestamp: i64,
) -> Result<Vec<Frame>> {
    // Create a packet and fill it with data
    let mut packet = Packet::new().map_err(|e| {
        Error::new(
            Status::GenericFailure,
            format!("Failed to create packet: {}", e),
        )
    })?;

    // Set packet timestamps
    packet.set_pts(timestamp);
    packet.set_dts(timestamp);

    // Allocate packet data
    unsafe {
        use crate::ffi::avcodec::av_new_packet;

        let ret = av_new_packet(packet.as_mut_ptr(), data.len() as i32);
        if ret < 0 {
            return Err(Error::new(
                Status::GenericFailure,
                format!("Failed to allocate packet data: {}", ret),
            ));
        }

        // Copy data to packet
        let pkt_data = packet.data() as *mut u8;
        std::ptr::copy_nonoverlapping(data.as_ptr(), pkt_data, data.len());
    }

    // Decode
    let frames = context.decode(Some(&packet)).map_err(|e| {
        Error::new(Status::GenericFailure, format!("Decode failed: {}", e))
    })?;

    Ok(frames)
}
