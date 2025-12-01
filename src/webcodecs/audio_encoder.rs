//! AudioEncoder - WebCodecs API implementation
//!
//! Provides audio encoding functionality using FFmpeg.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/AudioEncoder

use crate::codec::{
    context::get_audio_encoder_name, AudioEncoderConfig as InternalAudioEncoderConfig,
    AudioSampleBuffer, CodecContext, Resampler,
};
use crate::ffi::{AVCodecID, AVSampleFormat};
use crate::webcodecs::{AudioData, AudioEncoderConfig, AudioEncoderSupport, EncodedAudioChunk};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::sync::{Arc, Mutex};

use super::video_encoder::CodecState;

/// Type alias for output callback (takes chunk and metadata)
type OutputCallback = ThreadsafeFunction<(EncodedAudioChunk, EncodedAudioChunkMetadata)>;

/// Type alias for error callback (takes error message)
type ErrorCallback = ThreadsafeFunction<String>;

/// Output callback metadata for audio
#[napi(object)]
pub struct EncodedAudioChunkMetadata {
    /// Decoder configuration for this chunk
    pub decoder_config: Option<AudioDecoderConfigOutput>,
}

/// Decoder configuration output (for passing to decoder)
#[napi(object)]
pub struct AudioDecoderConfigOutput {
    /// Codec string
    pub codec: String,
    /// Sample rate
    pub sample_rate: Option<u32>,
    /// Number of channels
    pub number_of_channels: Option<u32>,
    /// Codec description (e.g., AudioSpecificConfig for AAC)
    pub description: Option<Buffer>,
}

/// Encode options for audio
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct AudioEncoderEncodeOptions {
    // Currently no options defined in WebCodecs spec for audio
}

/// Internal encoder state
struct AudioEncoderInner {
    state: CodecState,
    config: Option<AudioEncoderConfig>,
    context: Option<CodecContext>,
    resampler: Option<Resampler>,
    sample_buffer: Option<AudioSampleBuffer>,
    frame_count: u64,
    extradata_sent: bool,
    /// Target sample format for encoder
    target_format: AVSampleFormat,
    /// Queued output chunks
    output_queue: Vec<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
    /// Optional output callback (WebCodecs spec compliant mode)
    output_callback: Option<OutputCallback>,
    /// Optional error callback (WebCodecs spec compliant mode)
    error_callback: Option<ErrorCallback>,
}

/// AudioEncoder - WebCodecs-compliant audio encoder
///
/// Encodes AudioData objects into EncodedAudioChunk objects using FFmpeg.
///
/// Note: This implementation uses a synchronous output queue model instead of
/// callbacks for simpler integration. Use `takeEncodedChunks()` to retrieve
/// encoded output after calling `encode()` or `flush()`.
#[napi]
pub struct AudioEncoder {
    inner: Arc<Mutex<AudioEncoderInner>>,
}

#[napi]
impl AudioEncoder {
    /// Create a new AudioEncoder (queue-based mode)
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        let inner = AudioEncoderInner {
            state: CodecState::Unconfigured,
            config: None,
            context: None,
            resampler: None,
            sample_buffer: None,
            frame_count: 0,
            extradata_sent: false,
            target_format: AVSampleFormat::Fltp,
            output_queue: Vec::new(),
            output_callback: None,
            error_callback: None,
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Create an AudioEncoder with callbacks (WebCodecs spec compliant mode)
    ///
    /// In this mode, encoded chunks are delivered via the output callback
    /// instead of being queued for retrieval. Errors are reported via the
    /// error callback and the encoder transitions to the Closed state.
    ///
    /// Example:
    /// ```javascript
    /// const encoder = AudioEncoder.withCallbacks(
    ///   (chunk, metadata) => { /* handle output */ },
    ///   (error) => { /* handle error */ }
    /// );
    /// ```
    #[napi(factory)]
    pub fn with_callbacks(
        output: ThreadsafeFunction<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
        error: ThreadsafeFunction<String>,
    ) -> Result<Self> {
        let inner = AudioEncoderInner {
            state: CodecState::Unconfigured,
            config: None,
            context: None,
            resampler: None,
            sample_buffer: None,
            frame_count: 0,
            extradata_sent: false,
            target_format: AVSampleFormat::Fltp,
            output_queue: Vec::new(),
            output_callback: Some(output),
            error_callback: Some(error),
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Report an error via callback (if in callback mode) and close the encoder
    /// Returns true if error was reported via callback, false if should return error
    fn report_error(inner: &mut AudioEncoderInner, error_msg: &str) -> bool {
        if let Some(ref callback) = inner.error_callback {
            callback.call(Ok(error_msg.to_string()), ThreadsafeFunctionCallMode::NonBlocking);
            inner.state = CodecState::Closed;
            true
        } else {
            false
        }
    }

    /// Get encoder state
    #[napi(getter)]
    pub fn state(&self) -> Result<CodecState> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.state)
    }

    /// Get number of pending output chunks
    #[napi(getter)]
    pub fn encode_queue_size(&self) -> Result<u32> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.output_queue.len() as u32)
    }

    /// Configure the encoder
    #[napi]
    pub fn configure(&self, config: AudioEncoderConfig) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state == CodecState::Closed {
            return Err(Error::new(Status::GenericFailure, "Encoder is closed"));
        }

        // Parse codec string to determine codec ID
        let codec_id = parse_audio_codec_string(&config.codec)?;

        // Get encoder name (prefer external libraries for better quality)
        let encoder_name = get_audio_encoder_name(codec_id);

        // Create encoder context
        let mut context = if let Some(name) = encoder_name {
            CodecContext::new_encoder_by_name(name).or_else(|_| CodecContext::new_encoder(codec_id))
        } else {
            CodecContext::new_encoder(codec_id)
        }
        .map_err(|e| {
            Error::new(
                Status::GenericFailure,
                format!("Failed to create encoder: {}", e),
            )
        })?;

        // Determine target sample format based on codec
        let target_format = get_encoder_sample_format(codec_id);

        // Configure encoder
        let sample_rate = config.sample_rate.unwrap_or(48000);
        let channels = config.number_of_channels.unwrap_or(2);

        let encoder_config = InternalAudioEncoderConfig {
            sample_rate,
            channels,
            sample_format: target_format,
            bitrate: config.bitrate.unwrap_or(128_000.0) as u64,
            thread_count: 0,
        };

        context.configure_audio_encoder(&encoder_config).map_err(|e| {
            Error::new(
                Status::GenericFailure,
                format!("Failed to configure encoder: {}", e),
            )
        })?;

        // Open the encoder
        context.open().map_err(|e| {
            Error::new(
                Status::GenericFailure,
                format!("Failed to open encoder: {}", e),
            )
        })?;

        // Get the actual frame size from the encoder
        let frame_size = context.frame_size();
        let frame_size = if frame_size == 0 {
            // Some encoders don't set frame_size, use codec default
            AudioSampleBuffer::frame_size_for_codec(&config.codec)
        } else {
            frame_size as usize
        };

        // Create sample buffer
        let sample_buffer = AudioSampleBuffer::new(frame_size, channels, sample_rate, target_format);

        inner.context = Some(context);
        inner.config = Some(config);
        inner.sample_buffer = Some(sample_buffer);
        inner.target_format = target_format;
        inner.state = CodecState::Configured;
        inner.extradata_sent = false;
        inner.frame_count = 0;
        inner.resampler = None;
        inner.output_queue.clear();

        Ok(())
    }

    /// Encode audio data
    #[napi]
    pub fn encode(&self, data: &AudioData) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            let msg = "Encoder not configured";
            if Self::report_error(&mut inner, msg) {
                return Ok(());
            }
            return Err(Error::new(Status::GenericFailure, msg));
        }

        // Get config info
        let (target_sample_rate, target_channels, codec_string) = match inner.config.as_ref() {
            Some(config) => (
                config.sample_rate.unwrap_or(48000),
                config.number_of_channels.unwrap_or(2),
                config.codec.clone(),
            ),
            None => {
                let msg = "No encoder config";
                if Self::report_error(&mut inner, msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Get audio data properties
        let src_format = match data.format() {
            Ok(Some(fmt)) => fmt,
            Ok(None) => {
                let msg = "AudioData has no format";
                if Self::report_error(&mut inner, msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
            Err(e) => {
                let msg = format!("Failed to get format: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };
        let src_sample_rate = match data.sample_rate() {
            Ok(sr) => sr,
            Err(e) => {
                let msg = format!("Failed to get sample rate: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };
        let src_channels = match data.number_of_channels() {
            Ok(ch) => ch,
            Err(e) => {
                let msg = format!("Failed to get channels: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };
        let timestamp = match data.timestamp() {
            Ok(ts) => ts,
            Err(e) => {
                let msg = format!("Failed to get timestamp: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };

        // Check if we need resampling
        let needs_resampling = src_sample_rate != target_sample_rate
            || src_channels != target_channels
            || src_format.to_av_format() != inner.target_format;

        // Create resampler if needed and not already created
        if needs_resampling && inner.resampler.is_none() {
            match Resampler::new(
                src_channels,
                src_sample_rate,
                src_format.to_av_format(),
                target_channels,
                target_sample_rate,
                inner.target_format,
            ) {
                Ok(resampler) => inner.resampler = Some(resampler),
                Err(e) => {
                    let msg = format!("Failed to create resampler: {}", e);
                    if Self::report_error(&mut inner, &msg) {
                        return Ok(());
                    }
                    return Err(Error::new(Status::GenericFailure, msg));
                }
            }
        }

        // Get frame from AudioData
        let frame_result = match data.with_frame(|f| f.try_clone()) {
            Ok(res) => res,
            Err(e) => {
                let msg = format!("Failed to get frame: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };
        let frame = match frame_result {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("Failed to clone frame: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Resample if needed
        let frame_to_add = if let Some(ref mut resampler) = inner.resampler {
            match resampler.convert_alloc(&frame) {
                Ok(f) => f,
                Err(e) => {
                    let msg = format!("Resampling failed: {}", e);
                    if Self::report_error(&mut inner, &msg) {
                        return Ok(());
                    }
                    return Err(Error::new(Status::GenericFailure, msg));
                }
            }
        } else {
            frame
        };

        // Add frame to sample buffer
        {
            let sample_buffer = match inner.sample_buffer.as_mut() {
                Some(buf) => buf,
                None => {
                    let msg = "No sample buffer";
                    if Self::report_error(&mut inner, msg) {
                        return Ok(());
                    }
                    return Err(Error::new(Status::GenericFailure, msg));
                }
            };

            if let Err(e) = sample_buffer.add_frame(&frame_to_add) {
                let msg = format!("Failed to add samples: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        }

        // Get extradata before encoding first frame
        let extradata = if !inner.extradata_sent {
            inner
                .context
                .as_ref()
                .and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
        } else {
            None
        };

        // Process complete frames
        loop {
            // Check if we have a full frame and get buffer info
            let (has_frame, frame_size, sample_rate) = match inner.sample_buffer.as_ref() {
                Some(buf) => (
                    buf.has_full_frame(),
                    buf.frame_size() as i64,
                    buf.sample_rate() as i64,
                ),
                None => {
                    let msg = "No sample buffer";
                    if Self::report_error(&mut inner, msg) {
                        return Ok(());
                    }
                    return Err(Error::new(Status::GenericFailure, msg));
                }
            };

            if !has_frame {
                break;
            }

            // Take frame from buffer
            let mut frame_to_encode = {
                let sample_buffer = match inner.sample_buffer.as_mut() {
                    Some(buf) => buf,
                    None => {
                        let msg = "No sample buffer";
                        if Self::report_error(&mut inner, msg) {
                            return Ok(());
                        }
                        return Err(Error::new(Status::GenericFailure, msg));
                    }
                };
                match sample_buffer.take_frame() {
                    Ok(Some(f)) => f,
                    Ok(None) => {
                        let msg = "No frame available";
                        if Self::report_error(&mut inner, msg) {
                            return Ok(());
                        }
                        return Err(Error::new(Status::GenericFailure, msg));
                    }
                    Err(e) => {
                        let msg = format!("Failed to get frame: {}", e);
                        if Self::report_error(&mut inner, &msg) {
                            return Ok(());
                        }
                        return Err(Error::new(Status::GenericFailure, msg));
                    }
                }
            };

            // Set timestamp (approximate based on frame count)
            let frame_timestamp = if inner.frame_count == 0 {
                timestamp
            } else {
                timestamp + (inner.frame_count as i64 * frame_size * 1_000_000) / sample_rate
            };
            frame_to_encode.set_pts(frame_timestamp);

            // Encode the frame
            let context = match inner.context.as_mut() {
                Some(ctx) => ctx,
                None => {
                    let msg = "No encoder context";
                    if Self::report_error(&mut inner, msg) {
                        return Ok(());
                    }
                    return Err(Error::new(Status::GenericFailure, msg));
                }
            };

            let packets = match context.encode(Some(&frame_to_encode)) {
                Ok(pkts) => pkts,
                Err(e) => {
                    let msg = format!("Encode failed: {}", e);
                    if Self::report_error(&mut inner, &msg) {
                        return Ok(());
                    }
                    return Err(Error::new(Status::GenericFailure, msg));
                }
            };

            inner.frame_count += 1;

            // Calculate duration per frame in microseconds
            let duration_us = (frame_size * 1_000_000) / sample_rate;

            // Process output packets
            for packet in packets {
                let chunk = EncodedAudioChunk::from_packet(&packet, Some(duration_us));

                // Create metadata
                let metadata = if !inner.extradata_sent {
                    inner.extradata_sent = true;

                    EncodedAudioChunkMetadata {
                        decoder_config: Some(AudioDecoderConfigOutput {
                            codec: codec_string.clone(),
                            sample_rate: Some(target_sample_rate),
                            number_of_channels: Some(target_channels),
                            description: extradata.clone().map(Buffer::from),
                        }),
                    }
                } else {
                    EncodedAudioChunkMetadata {
                        decoder_config: None,
                    }
                };

                // Dispatch output via callback or queue
                if let Some(ref callback) = inner.output_callback {
                    callback.call(Ok((chunk, metadata)), ThreadsafeFunctionCallMode::NonBlocking);
                } else {
                    inner.output_queue.push((chunk, metadata));
                }
            }
        }

        Ok(())
    }

    /// Flush the encoder and return all remaining chunks
    /// Returns a Promise that resolves when flushing is complete
    #[napi]
    pub async fn flush(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            let msg = "Encoder not configured";
            if Self::report_error(&mut inner, msg) {
                return Ok(());
            }
            return Err(Error::new(Status::GenericFailure, msg));
        }

        // Flush any remaining samples in buffer
        if let Some(ref mut sample_buffer) = inner.sample_buffer {
            if let Ok(Some(mut frame)) = sample_buffer.flush() {
                // Set timestamp
                let frame_size = sample_buffer.frame_size() as i64;
                let sample_rate = sample_buffer.sample_rate() as i64;
                let frame_timestamp =
                    (inner.frame_count as i64 * frame_size * 1_000_000) / sample_rate;
                frame.set_pts(frame_timestamp);

                let context = match inner.context.as_mut() {
                    Some(ctx) => ctx,
                    None => {
                        let msg = "No encoder context";
                        if Self::report_error(&mut inner, msg) {
                            return Ok(());
                        }
                        return Err(Error::new(Status::GenericFailure, msg));
                    }
                };

                if let Ok(packets) = context.encode(Some(&frame)) {
                    let duration_us = (frame.nb_samples() as i64 * 1_000_000) / sample_rate;
                    for packet in packets {
                        let chunk = EncodedAudioChunk::from_packet(&packet, Some(duration_us));
                        let metadata = EncodedAudioChunkMetadata {
                            decoder_config: None,
                        };
                        // Dispatch output via callback or queue
                        if let Some(ref callback) = inner.output_callback {
                            callback.call(Ok((chunk, metadata)), ThreadsafeFunctionCallMode::NonBlocking);
                        } else {
                            inner.output_queue.push((chunk, metadata));
                        }
                    }
                }
            }
        }

        // Flush encoder
        let context = match inner.context.as_mut() {
            Some(ctx) => ctx,
            None => {
                let msg = "No encoder context";
                if Self::report_error(&mut inner, msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        let packets = match context.flush_encoder() {
            Ok(pkts) => pkts,
            Err(e) => {
                let msg = format!("Flush failed: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Process remaining packets
        for packet in packets {
            let chunk = EncodedAudioChunk::from_packet(&packet, None);
            let metadata = EncodedAudioChunkMetadata {
                decoder_config: None,
            };
            // Dispatch output via callback or queue
            if let Some(ref callback) = inner.output_callback {
                callback.call(Ok((chunk, metadata)), ThreadsafeFunctionCallMode::NonBlocking);
            } else {
                inner.output_queue.push((chunk, metadata));
            }
        }

        Ok(())
    }

    /// Take all encoded chunks from the output queue
    #[napi]
    pub fn take_encoded_chunks(&self) -> Result<Vec<EncodedAudioChunk>> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        let chunks: Vec<EncodedAudioChunk> = inner
            .output_queue
            .drain(..)
            .map(|(chunk, _)| chunk)
            .collect();

        Ok(chunks)
    }

    /// Check if there are any pending encoded chunks
    #[napi]
    pub fn has_output(&self) -> Result<bool> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(!inner.output_queue.is_empty())
    }

    /// Take the next encoded chunk from the output queue (if any)
    #[napi]
    pub fn take_next_chunk(&self) -> Result<Option<EncodedAudioChunk>> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.output_queue.is_empty() {
            Ok(None)
        } else {
            let (chunk, _) = inner.output_queue.remove(0);
            Ok(Some(chunk))
        }
    }

    /// Reset the encoder
    #[napi]
    pub fn reset(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state == CodecState::Closed {
            return Err(Error::new(Status::GenericFailure, "Encoder is closed"));
        }

        // Drop existing context
        inner.context = None;
        inner.resampler = None;
        inner.sample_buffer = None;
        inner.config = None;
        inner.state = CodecState::Unconfigured;
        inner.frame_count = 0;
        inner.extradata_sent = false;
        inner.output_queue.clear();

        Ok(())
    }

    /// Close the encoder
    #[napi]
    pub fn close(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        inner.context = None;
        inner.resampler = None;
        inner.sample_buffer = None;
        inner.config = None;
        inner.state = CodecState::Closed;
        inner.output_queue.clear();

        Ok(())
    }

    /// Check if a configuration is supported
    /// Returns a Promise that resolves with support information
    #[napi]
    pub async fn is_config_supported(config: AudioEncoderConfig) -> Result<AudioEncoderSupport> {
        // Parse codec string
        let codec_id = match parse_audio_codec_string(&config.codec) {
            Ok(id) => id,
            Err(_) => {
                return Ok(AudioEncoderSupport {
                    supported: false,
                    config,
                });
            }
        };

        // Try to find encoder
        let encoder_name = get_audio_encoder_name(codec_id);
        let result = if let Some(name) = encoder_name {
            CodecContext::new_encoder_by_name(name).or_else(|_| CodecContext::new_encoder(codec_id))
        } else {
            CodecContext::new_encoder(codec_id)
        };

        Ok(AudioEncoderSupport {
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

/// Get the preferred sample format for an encoder
fn get_encoder_sample_format(codec_id: AVCodecID) -> AVSampleFormat {
    match codec_id {
        AVCodecID::Aac => AVSampleFormat::Fltp,  // AAC prefers float planar
        AVCodecID::Opus => AVSampleFormat::Flt,  // Opus prefers float interleaved
        AVCodecID::Mp3 => AVSampleFormat::S16p,  // MP3 prefers s16 planar
        AVCodecID::Flac => AVSampleFormat::S16,  // FLAC prefers s16
        AVCodecID::Vorbis => AVSampleFormat::Fltp, // Vorbis prefers float planar
        AVCodecID::PcmS16le => AVSampleFormat::S16,
        AVCodecID::PcmS16be => AVSampleFormat::S16,
        AVCodecID::PcmF32le => AVSampleFormat::Flt,
        AVCodecID::PcmF32be => AVSampleFormat::Flt,
        AVCodecID::Ac3 => AVSampleFormat::Fltp,
        AVCodecID::Alac => AVSampleFormat::S16p,
        _ => AVSampleFormat::Fltp, // Default to float planar
    }
}
