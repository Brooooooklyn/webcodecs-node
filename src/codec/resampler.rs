//! Safe wrapper around FFmpeg SwrContext
//!
//! Provides audio resampling and format conversion functionality.

use crate::ffi::{
    swresample::{
        av_channel_layout_default, av_channel_layout_uninit, swr_alloc_set_opts2, swr_convert,
        swr_free, swr_get_delay, swr_get_out_samples, swr_init,
    },
    AVChannelLayout, AVSampleFormat, SwrContext,
};
use std::ptr::NonNull;

use super::{CodecError, CodecResult, Frame};

/// Safe wrapper around SwrContext for audio resampling and format conversion
pub struct Resampler {
    ptr: NonNull<SwrContext>,
    src_channels: u32,
    src_sample_rate: u32,
    src_format: AVSampleFormat,
    dst_channels: u32,
    dst_sample_rate: u32,
    dst_format: AVSampleFormat,
}

impl Resampler {
    /// Create a new resampler for the given conversion
    pub fn new(
        src_channels: u32,
        src_sample_rate: u32,
        src_format: AVSampleFormat,
        dst_channels: u32,
        dst_sample_rate: u32,
        dst_format: AVSampleFormat,
    ) -> CodecResult<Self> {
        // Create channel layouts using the new FFmpeg API
        let mut src_ch_layout: AVChannelLayout = unsafe { std::mem::zeroed() };
        let mut dst_ch_layout: AVChannelLayout = unsafe { std::mem::zeroed() };

        unsafe {
            av_channel_layout_default(&mut src_ch_layout, src_channels as i32);
            av_channel_layout_default(&mut dst_ch_layout, dst_channels as i32);
        }

        // Allocate and configure resampler context using swr_alloc_set_opts2
        let mut ctx: *mut SwrContext = std::ptr::null_mut();

        let ret = unsafe {
            swr_alloc_set_opts2(
                &mut ctx,
                &dst_ch_layout,
                dst_format.as_raw(),
                dst_sample_rate as i32,
                &src_ch_layout,
                src_format.as_raw(),
                src_sample_rate as i32,
                0,
                std::ptr::null_mut(),
            )
        };

        // Clean up channel layouts - they've been copied by FFmpeg
        unsafe {
            av_channel_layout_uninit(&mut src_ch_layout);
            av_channel_layout_uninit(&mut dst_ch_layout);
        }

        if ret < 0 {
            return Err(CodecError::InvalidConfig(format!(
                "Failed to configure resampler (err: {})",
                ret
            )));
        }

        if ctx.is_null() {
            return Err(CodecError::AllocationFailed("SwrContext"));
        }

        let ptr = NonNull::new(ctx).ok_or_else(|| {
            CodecError::InvalidConfig("Internal error: ctx should not be null".into())
        })?;

        // Initialize the context
        let ret = unsafe { swr_init(ptr.as_ptr()) };
        if ret < 0 {
            // Free the context on error
            unsafe {
                let mut p = ptr.as_ptr();
                swr_free(&mut p);
            }
            return Err(CodecError::InvalidConfig(format!(
                "Failed to initialize resampler (err: {})",
                ret
            )));
        }

        Ok(Self {
            ptr,
            src_channels,
            src_sample_rate,
            src_format,
            dst_channels,
            dst_sample_rate,
            dst_format,
        })
    }

    /// Create a resampler for sample rate conversion only
    pub fn new_rate_converter(
        channels: u32,
        src_sample_rate: u32,
        dst_sample_rate: u32,
        format: AVSampleFormat,
    ) -> CodecResult<Self> {
        Self::new(
            channels,
            src_sample_rate,
            format,
            channels,
            dst_sample_rate,
            format,
        )
    }

    /// Create a resampler for format conversion only
    pub fn new_format_converter(
        channels: u32,
        sample_rate: u32,
        src_format: AVSampleFormat,
        dst_format: AVSampleFormat,
    ) -> CodecResult<Self> {
        Self::new(
            channels,
            sample_rate,
            src_format,
            channels,
            sample_rate,
            dst_format,
        )
    }

    /// Convert audio samples
    ///
    /// # Arguments
    /// * `src` - Source frame with audio data
    /// * `dst` - Destination frame (must have buffers allocated)
    ///
    /// # Returns
    /// Number of samples converted per channel
    pub fn convert(&mut self, src: &Frame, dst: &mut Frame) -> CodecResult<u32> {
        if !src.is_audio() {
            return Err(CodecError::InvalidConfig("Source is not an audio frame".into()));
        }

        // Prepare source data pointers
        let src_nb_samples = src.nb_samples() as i32;
        let src_data = src.audio_data(0);

        // Prepare destination data pointers
        let dst_nb_samples = dst.nb_samples() as i32;
        let dst_data = dst.audio_data_mut(0);

        // Build pointer arrays for planar/interleaved handling
        let src_channels = src.channels() as usize;
        let dst_channels = dst.channels() as usize;

        // For planar audio, we need pointers to each channel
        // For interleaved, we just need the first pointer
        let mut src_ptrs: Vec<*const u8> = Vec::with_capacity(src_channels.max(1));
        let mut dst_ptrs: Vec<*mut u8> = Vec::with_capacity(dst_channels.max(1));

        if self.src_format.is_planar() {
            for ch in 0..src_channels {
                src_ptrs.push(src.audio_data(ch));
            }
        } else {
            src_ptrs.push(src_data);
        }

        if self.dst_format.is_planar() {
            for ch in 0..dst_channels {
                dst_ptrs.push(dst.audio_data_mut(ch));
            }
        } else {
            dst_ptrs.push(dst_data);
        }

        let result = unsafe {
            swr_convert(
                self.ptr.as_ptr(),
                dst_ptrs.as_mut_ptr(),
                dst_nb_samples,
                src_ptrs.as_ptr(),
                src_nb_samples,
            )
        };

        if result < 0 {
            return Err(CodecError::InvalidState(format!(
                "Resampling failed with error {}",
                result
            )));
        }

        // Update destination frame metadata
        dst.set_nb_samples(result as u32);
        dst.set_pts(src.pts());

        Ok(result as u32)
    }

    /// Convert audio samples to a newly allocated frame
    pub fn convert_alloc(&mut self, src: &Frame) -> CodecResult<Frame> {
        let out_samples = self.get_out_samples(src.nb_samples());
        let mut dst = Frame::new_audio(
            out_samples,
            self.dst_channels,
            self.dst_sample_rate,
            self.dst_format,
        )?;
        self.convert(src, &mut dst)?;
        Ok(dst)
    }

    /// Flush any remaining buffered samples
    ///
    /// # Arguments
    /// * `dst` - Destination frame to receive flushed samples
    ///
    /// # Returns
    /// Number of samples flushed per channel
    pub fn flush(&mut self, dst: &mut Frame) -> CodecResult<u32> {
        let dst_nb_samples = dst.nb_samples() as i32;
        let dst_data = dst.audio_data_mut(0);

        let mut dst_ptrs: Vec<*mut u8> = Vec::with_capacity(self.dst_channels as usize);

        if self.dst_format.is_planar() {
            for ch in 0..self.dst_channels as usize {
                dst_ptrs.push(dst.audio_data_mut(ch));
            }
        } else {
            dst_ptrs.push(dst_data);
        }

        let result = unsafe {
            swr_convert(
                self.ptr.as_ptr(),
                dst_ptrs.as_mut_ptr(),
                dst_nb_samples,
                std::ptr::null(),
                0,
            )
        };

        if result < 0 {
            return Err(CodecError::InvalidState("Flush failed".into()));
        }

        dst.set_nb_samples(result as u32);
        Ok(result as u32)
    }

    /// Get the number of buffered samples (delay)
    pub fn get_delay(&self) -> i64 {
        unsafe { swr_get_delay(self.ptr.as_ptr() as *const _, self.dst_sample_rate as i64) }
    }

    /// Get the estimated number of output samples for a given input sample count
    pub fn get_out_samples(&self, in_samples: u32) -> u32 {
        let result = unsafe { swr_get_out_samples(self.ptr.as_ptr() as *const _, in_samples as i32) };
        result.max(0) as u32
    }

    // ========================================================================
    // Accessors
    // ========================================================================

    /// Get source channel count
    pub fn src_channels(&self) -> u32 {
        self.src_channels
    }

    /// Get source sample rate
    pub fn src_sample_rate(&self) -> u32 {
        self.src_sample_rate
    }

    /// Get source sample format
    pub fn src_format(&self) -> AVSampleFormat {
        self.src_format
    }

    /// Get destination channel count
    pub fn dst_channels(&self) -> u32 {
        self.dst_channels
    }

    /// Get destination sample rate
    pub fn dst_sample_rate(&self) -> u32 {
        self.dst_sample_rate
    }

    /// Get destination sample format
    pub fn dst_format(&self) -> AVSampleFormat {
        self.dst_format
    }

    /// Check if this resampler only changes sample rate
    pub fn is_rate_only(&self) -> bool {
        self.src_channels == self.dst_channels
            && self.src_format == self.dst_format
            && self.src_sample_rate != self.dst_sample_rate
    }

    /// Check if this resampler only changes format
    pub fn is_format_only(&self) -> bool {
        self.src_channels == self.dst_channels
            && self.src_sample_rate == self.dst_sample_rate
            && self.src_format != self.dst_format
    }

    /// Check if any resampling is actually needed
    pub fn needs_conversion(&self) -> bool {
        self.src_channels != self.dst_channels
            || self.src_sample_rate != self.dst_sample_rate
            || self.src_format != self.dst_format
    }
}

impl Drop for Resampler {
    fn drop(&mut self) {
        unsafe {
            let mut ptr = self.ptr.as_ptr();
            swr_free(&mut ptr);
        }
    }
}

// SwrContext is thread-safe for reading
unsafe impl Send for Resampler {}

impl std::fmt::Debug for Resampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resampler")
            .field(
                "src",
                &format!(
                    "{} ch @ {} Hz {:?}",
                    self.src_channels, self.src_sample_rate, self.src_format
                ),
            )
            .field(
                "dst",
                &format!(
                    "{} ch @ {} Hz {:?}",
                    self.dst_channels, self.dst_sample_rate, self.dst_format
                ),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resampler_creation() {
        let resampler = Resampler::new(
            2,
            44100,
            AVSampleFormat::Fltp,
            2,
            48000,
            AVSampleFormat::Fltp,
        );
        assert!(resampler.is_ok(), "Resampler creation failed: {:?}", resampler.err());
    }

    #[test]
    fn test_rate_converter_creation() {
        let converter =
            Resampler::new_rate_converter(2, 44100, 48000, AVSampleFormat::S16);
        assert!(converter.is_ok(), "Rate converter creation failed: {:?}", converter.err());
        assert!(converter.unwrap().is_rate_only());
    }

    #[test]
    fn test_format_converter_creation() {
        let converter = Resampler::new_format_converter(
            2,
            48000,
            AVSampleFormat::Fltp,
            AVSampleFormat::S16,
        );
        assert!(converter.is_ok(), "Format converter creation failed: {:?}", converter.err());
        assert!(converter.unwrap().is_format_only());
    }
}
