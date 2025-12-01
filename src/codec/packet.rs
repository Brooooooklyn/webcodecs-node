//! Safe wrapper around FFmpeg AVPacket
//!
//! Provides RAII-based memory management for encoded video data.

use crate::ffi::{
    self,
    accessors::{
        ffpkt_data, ffpkt_dts, ffpkt_duration, ffpkt_flags, ffpkt_pts, ffpkt_set_dts,
        ffpkt_set_duration, ffpkt_set_flags, ffpkt_set_pts, ffpkt_size,
    },
    avcodec::{av_packet_alloc, av_packet_free, av_packet_ref, av_packet_unref},
    pkt_flag, AVPacket,
};
use std::ptr::NonNull;

use super::CodecError;

/// Safe wrapper around AVPacket with RAII cleanup
pub struct Packet {
    ptr: NonNull<AVPacket>,
}

impl Packet {
    /// Allocate a new empty packet
    pub fn new() -> Result<Self, CodecError> {
        let ptr = unsafe { av_packet_alloc() };
        NonNull::new(ptr)
            .map(|ptr| Self { ptr })
            .ok_or(CodecError::AllocationFailed("AVPacket"))
    }

    /// Create a Packet from a raw pointer (takes ownership)
    ///
    /// # Safety
    /// The pointer must be a valid AVPacket allocated by FFmpeg
    pub unsafe fn from_raw(ptr: *mut AVPacket) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| Self { ptr })
    }

    /// Get the raw pointer (for FFmpeg API calls)
    #[inline]
    pub fn as_ptr(&self) -> *const AVPacket {
        self.ptr.as_ptr()
    }

    /// Get the mutable raw pointer (for FFmpeg API calls)
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut AVPacket {
        self.ptr.as_ptr()
    }

    /// Consume the Packet and return the raw pointer
    /// The caller is responsible for freeing the packet
    pub fn into_raw(self) -> *mut AVPacket {
        let ptr = self.ptr.as_ptr();
        std::mem::forget(self);
        ptr
    }

    // ========================================================================
    // Data Access
    // ========================================================================

    /// Get pointer to packet data
    pub fn data(&self) -> *const u8 {
        unsafe { ffpkt_data(self.as_ptr()) }
    }

    /// Get packet data as a slice
    pub fn as_slice(&self) -> &[u8] {
        let ptr = self.data();
        let size = self.size();
        if ptr.is_null() || size == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(ptr, size as usize) }
        }
    }

    /// Get packet size in bytes
    #[inline]
    pub fn size(&self) -> i32 {
        unsafe { ffpkt_size(self.as_ptr()) }
    }

    /// Check if packet has data
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size() == 0
    }

    // ========================================================================
    // Timestamps
    // ========================================================================

    /// Get presentation timestamp
    #[inline]
    pub fn pts(&self) -> i64 {
        unsafe { ffpkt_pts(self.as_ptr()) }
    }

    /// Set presentation timestamp
    #[inline]
    pub fn set_pts(&mut self, pts: i64) {
        unsafe { ffpkt_set_pts(self.as_mut_ptr(), pts) }
    }

    /// Get decoding timestamp
    #[inline]
    pub fn dts(&self) -> i64 {
        unsafe { ffpkt_dts(self.as_ptr()) }
    }

    /// Set decoding timestamp
    #[inline]
    pub fn set_dts(&mut self, dts: i64) {
        unsafe { ffpkt_set_dts(self.as_mut_ptr(), dts) }
    }

    /// Get duration
    #[inline]
    pub fn duration(&self) -> i64 {
        unsafe { ffpkt_duration(self.as_ptr()) }
    }

    /// Set duration
    #[inline]
    pub fn set_duration(&mut self, duration: i64) {
        unsafe { ffpkt_set_duration(self.as_mut_ptr(), duration) }
    }

    // ========================================================================
    // Flags
    // ========================================================================

    /// Get packet flags
    #[inline]
    pub fn flags(&self) -> i32 {
        unsafe { ffpkt_flags(self.as_ptr()) }
    }

    /// Set packet flags
    #[inline]
    pub fn set_flags(&mut self, flags: i32) {
        unsafe { ffpkt_set_flags(self.as_mut_ptr(), flags) }
    }

    /// Check if this is a key frame packet
    #[inline]
    pub fn is_key(&self) -> bool {
        (self.flags() & pkt_flag::KEY) != 0
    }

    /// Check if packet is corrupted
    #[inline]
    pub fn is_corrupt(&self) -> bool {
        (self.flags() & pkt_flag::CORRUPT) != 0
    }

    // ========================================================================
    // Lifecycle
    // ========================================================================

    /// Unreference the packet data
    pub fn unref(&mut self) {
        unsafe { av_packet_unref(self.as_mut_ptr()) }
    }

    /// Create a new reference to this packet's data
    pub fn try_clone(&self) -> Result<Self, CodecError> {
        let new_pkt = Self::new()?;
        let ret = unsafe { av_packet_ref(new_pkt.ptr.as_ptr(), self.as_ptr()) };
        ffi::check_error(ret)?;
        Ok(new_pkt)
    }

    /// Copy packet data to a new Vec
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        unsafe {
            let mut ptr = self.ptr.as_ptr();
            av_packet_free(&mut ptr);
        }
    }
}

// Packet data can be sent between threads
unsafe impl Send for Packet {}

impl std::fmt::Debug for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Packet")
            .field("size", &self.size())
            .field("pts", &self.pts())
            .field("dts", &self.dts())
            .field("is_key", &self.is_key())
            .finish()
    }
}

impl Clone for Packet {
    fn clone(&self) -> Self {
        self.try_clone().expect("Failed to clone packet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_allocation() {
        let pkt = Packet::new().unwrap();
        assert!(pkt.is_empty());
        assert_eq!(pkt.size(), 0);
    }
}
