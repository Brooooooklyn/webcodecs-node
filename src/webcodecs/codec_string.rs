//! Codec string parser for WebCodecs API
//!
//! Parses codec strings like vp09.PP.LL.DD, av01.P.LLT.DD, avc1.PPCCLL, hev1.P.T.Lxxx
//! into structured information including profile, level, and bit depth.

use crate::ffi::types::AVCodecID;

/// Parsed codec information extracted from a WebCodecs codec string
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCodec {
  /// The FFmpeg codec ID
  pub codec_id: AVCodecID,
  /// Profile number (codec-specific interpretation)
  pub profile: Option<u8>,
  /// Level value (codec-specific, often needs multiplication by 10)
  pub level: Option<u8>,
  /// Bit depth (8, 10, or 12)
  pub bit_depth: Option<u8>,
  /// Chroma subsampling (420, 422, 444)
  pub chroma_subsampling: Option<u16>,
}

impl ParsedCodec {
  /// Create a new ParsedCodec with just the codec ID
  pub fn new(codec_id: AVCodecID) -> Self {
    Self {
      codec_id,
      profile: None,
      level: None,
      bit_depth: None,
      chroma_subsampling: None,
    }
  }
}

/// Parse a WebCodecs codec string into structured information
///
/// Supported formats:
/// - VP9: `vp09.PP.LL.DD.CC.cp.tc.mc.FF` or `vp9`
/// - AV1: `av01.P.LLT.DD.M.CCC.cp.tc.mc.F` or `av1`
/// - H.264: `avc1.PPCCLL` or `avc3.PPCCLL`
/// - H.265: `hev1.P.T.Lxxx` or `hvc1.P.T.Lxxx`
/// - VP8: `vp8`
///
/// Returns `None` if the codec string is not recognized.
pub fn parse_codec_string(codec: &str) -> Option<ParsedCodec> {
  let codec_lower = codec.to_lowercase();

  // VP9: vp09.PP.LL.DD or vp9
  if codec_lower.starts_with("vp09") {
    return parse_vp9(codec);
  }
  if codec_lower == "vp9" {
    return Some(ParsedCodec::new(AVCodecID::Vp9));
  }

  // AV1: av01.P.LLT.DD or av1
  if codec_lower.starts_with("av01") {
    return parse_av1(codec);
  }
  if codec_lower == "av1" {
    return Some(ParsedCodec::new(AVCodecID::Av1));
  }

  // H.264: avc1.PPCCLL or avc3.PPCCLL
  if codec_lower.starts_with("avc1") || codec_lower.starts_with("avc3") {
    return parse_avc(codec);
  }

  // H.265: hev1.P.T.Lxxx or hvc1.P.T.Lxxx
  if codec_lower.starts_with("hev1") || codec_lower.starts_with("hvc1") {
    return parse_hevc(codec);
  }

  // VP8 (simple, no parameters)
  if codec_lower == "vp8" {
    return Some(ParsedCodec::new(AVCodecID::Vp8));
  }

  None
}

/// Parse VP9 codec string: vp09.PP.LL.DD.CC.cp.tc.mc.FF
/// - PP: profile (00-03)
/// - LL: level (10-62)
/// - DD: bit depth (08, 10, 12)
/// - CC: chroma subsampling (00=420, 01=422, 02=444, 03=440)
fn parse_vp9(codec: &str) -> Option<ParsedCodec> {
  let parts: Vec<&str> = codec.split('.').collect();

  let mut parsed = ParsedCodec::new(AVCodecID::Vp9);

  if parts.len() >= 2 {
    // Profile
    if let Ok(profile) = parts[1].parse::<u8>() {
      parsed.profile = Some(profile);
    }
  }

  if parts.len() >= 3 {
    // Level
    if let Ok(level) = parts[2].parse::<u8>() {
      parsed.level = Some(level);
    }
  }

  if parts.len() >= 4 {
    // Bit depth
    if let Ok(depth) = parts[3].parse::<u8>() {
      parsed.bit_depth = Some(depth);
    }
  }

  if parts.len() >= 5 {
    // Chroma subsampling
    match parts[4] {
      "00" => parsed.chroma_subsampling = Some(420),
      "01" => parsed.chroma_subsampling = Some(422),
      "02" => parsed.chroma_subsampling = Some(444),
      "03" => parsed.chroma_subsampling = Some(440),
      _ => {}
    }
  }

  Some(parsed)
}

/// Parse AV1 codec string: av01.P.LLT.DD.M.CCC.cp.tc.mc.F
/// - P: profile (0=Main, 1=High, 2=Professional)
/// - LLT: level and tier (level*10 + tier_flag)
/// - DD: bit depth (08, 10, 12)
/// - M: monochrome flag (0=not monochrome, 1=monochrome)
/// - CCC: chroma subsampling (110=420, 100=422, 000=444)
fn parse_av1(codec: &str) -> Option<ParsedCodec> {
  let parts: Vec<&str> = codec.split('.').collect();

  let mut parsed = ParsedCodec::new(AVCodecID::Av1);

  if parts.len() >= 2 {
    // Profile
    if let Ok(profile) = parts[1].parse::<u8>() {
      parsed.profile = Some(profile);
    }
  }

  if parts.len() >= 3 {
    // Level and tier (e.g., "04M" -> level 4, Main tier)
    let level_tier = parts[2];
    // Extract numeric part
    let level_str: String = level_tier
      .chars()
      .take_while(|c| c.is_ascii_digit())
      .collect();
    if let Ok(level) = level_str.parse::<u8>() {
      parsed.level = Some(level);
    }
  }

  if parts.len() >= 4 {
    // Bit depth
    if let Ok(depth) = parts[3].parse::<u8>() {
      parsed.bit_depth = Some(depth);
    }
  }

  // Chroma subsampling is at index 5 if monochrome flag is present
  if parts.len() >= 6 {
    match parts[5] {
      "110" => parsed.chroma_subsampling = Some(420),
      "100" => parsed.chroma_subsampling = Some(422),
      "000" => parsed.chroma_subsampling = Some(444),
      _ => {}
    }
  }

  Some(parsed)
}

/// Parse AVC/H.264 codec string: avc1.PPCCLL
/// - PP: profile_idc (42=Baseline, 4D=Main, 58=Extended, 64=High, etc.)
/// - CC: constraint_set flags
/// - LL: level_idc (1F=3.1, 28=4.0, 33=5.1, etc.)
fn parse_avc(codec: &str) -> Option<ParsedCodec> {
  let parts: Vec<&str> = codec.split('.').collect();

  let mut parsed = ParsedCodec::new(AVCodecID::H264);

  if parts.len() >= 2 && parts[1].len() >= 6 {
    let hex = parts[1];

    // Profile (first 2 hex digits)
    if let Ok(profile) = u8::from_str_radix(&hex[0..2], 16) {
      parsed.profile = Some(profile);
    }

    // Level (last 2 hex digits) - divide by 10 for actual level
    if let Ok(level) = u8::from_str_radix(&hex[4..6], 16) {
      parsed.level = Some(level);
    }
  }

  // H.264 is always 8-bit (High 10 profile for 10-bit, but that's rare)
  parsed.bit_depth = Some(8);

  Some(parsed)
}

/// Parse HEVC/H.265 codec string: hev1.P.TC.Lxxx.Bx
/// - P: profile (1=Main, 2=Main10, 3=Main Still Picture)
/// - TC: tier and compatibility flags
/// - Lxxx: level (L120 = level 4.0, L150 = level 5.0)
fn parse_hevc(codec: &str) -> Option<ParsedCodec> {
  let parts: Vec<&str> = codec.split('.').collect();

  let mut parsed = ParsedCodec::new(AVCodecID::Hevc);

  if parts.len() >= 2 {
    // Profile
    if let Ok(profile) = parts[1].parse::<u8>() {
      parsed.profile = Some(profile);
      // Infer bit depth from profile
      if profile == 2 {
        parsed.bit_depth = Some(10); // Main 10
      } else {
        parsed.bit_depth = Some(8);
      }
    }
  }

  if parts.len() >= 4 {
    // Level: Lxxx format (e.g., L120 = 120/30 = 4.0, L150 = 150/30 = 5.0)
    let level_str = parts[3];
    if level_str.starts_with('L') || level_str.starts_with('l') {
      if let Ok(level) = level_str[1..].parse::<u8>() {
        parsed.level = Some(level);
      }
    }
  }

  Some(parsed)
}

/// Get the FFmpeg profile ID for H.264 from the parsed profile value
pub fn avc_profile_to_ffmpeg(profile_idc: u8) -> i32 {
  match profile_idc {
    66 => 66,   // Baseline (FF_PROFILE_H264_BASELINE)
    77 => 77,   // Main (FF_PROFILE_H264_MAIN)
    88 => 88,   // Extended (FF_PROFILE_H264_EXTENDED)
    100 => 100, // High (FF_PROFILE_H264_HIGH)
    110 => 110, // High 10 (FF_PROFILE_H264_HIGH_10)
    122 => 122, // High 4:2:2 (FF_PROFILE_H264_HIGH_422)
    244 => 244, // High 4:4:4 Predictive (FF_PROFILE_H264_HIGH_444_PREDICTIVE)
    _ => -1,
  }
}

/// Get the FFmpeg profile ID for VP9 from the parsed profile value
pub fn vp9_profile_to_ffmpeg(profile: u8) -> i32 {
  match profile {
    0 => 0, // Profile 0 (8-bit 4:2:0)
    1 => 1, // Profile 1 (8-bit 4:2:2/4:4:4)
    2 => 2, // Profile 2 (10/12-bit 4:2:0)
    3 => 3, // Profile 3 (10/12-bit 4:2:2/4:4:4)
    _ => 0,
  }
}

/// Get the FFmpeg profile ID for AV1 from the parsed profile value
pub fn av1_profile_to_ffmpeg(profile: u8) -> i32 {
  match profile {
    0 => 0, // Main
    1 => 1, // High
    2 => 2, // Professional
    _ => 0,
  }
}

/// Get the FFmpeg profile ID for HEVC from the parsed profile value
pub fn hevc_profile_to_ffmpeg(profile: u8) -> i32 {
  match profile {
    1 => 1, // Main (FF_PROFILE_HEVC_MAIN)
    2 => 2, // Main 10 (FF_PROFILE_HEVC_MAIN_10)
    3 => 3, // Main Still Picture (FF_PROFILE_HEVC_MAIN_STILL_PICTURE)
    4 => 4, // Rext (FF_PROFILE_HEVC_REXT)
    _ => -1,
  }
}

/// Convert H.264 level to FFmpeg level_idc format
/// e.g., 3.1 -> 31, 4.0 -> 40
pub fn avc_level_to_ffmpeg(level: u8) -> i32 {
  // H.264 levels are stored as level * 10 in codec string
  // e.g., 0x1F = 31 = level 3.1
  level as i32
}

/// Convert VP9 level to FFmpeg format
pub fn vp9_level_to_ffmpeg(level: u8) -> i32 {
  // VP9 level values are 10, 11, 20, 21, 30, 31, 40, 41, 50, 51, 52, 60, 61, 62
  level as i32
}

/// Convert AV1 level to FFmpeg format
pub fn av1_level_to_ffmpeg(level: u8) -> i32 {
  // AV1 level is encoded as seq_level_idx (0-23)
  level as i32
}

/// Convert HEVC level to FFmpeg format
/// e.g., 120 -> 40 (4.0), 150 -> 50 (5.0)
pub fn hevc_level_to_ffmpeg(level: u8) -> i32 {
  // HEVC level is stored as level * 30 in codec string (L120 = level 4.0 = 120/30*10)
  level as i32 / 3
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_vp9_full() {
    let parsed = parse_codec_string("vp09.00.10.08.00").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::Vp9);
    assert_eq!(parsed.profile, Some(0));
    assert_eq!(parsed.level, Some(10));
    assert_eq!(parsed.bit_depth, Some(8));
    assert_eq!(parsed.chroma_subsampling, Some(420));
  }

  #[test]
  fn test_parse_vp9_simple() {
    let parsed = parse_codec_string("vp9").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::Vp9);
    assert_eq!(parsed.profile, None);
  }

  #[test]
  fn test_parse_av1_full() {
    let parsed = parse_codec_string("av01.0.04M.10.0.110").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::Av1);
    assert_eq!(parsed.profile, Some(0));
    assert_eq!(parsed.level, Some(4));
    assert_eq!(parsed.bit_depth, Some(10));
    assert_eq!(parsed.chroma_subsampling, Some(420));
  }

  #[test]
  fn test_parse_av1_simple() {
    let parsed = parse_codec_string("av1").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::Av1);
  }

  #[test]
  fn test_parse_avc() {
    let parsed = parse_codec_string("avc1.42001f").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::H264);
    assert_eq!(parsed.profile, Some(66)); // 0x42 = 66 = Baseline
    assert_eq!(parsed.level, Some(31)); // 0x1F = 31 = Level 3.1
    assert_eq!(parsed.bit_depth, Some(8));
  }

  #[test]
  fn test_parse_avc_high() {
    let parsed = parse_codec_string("avc1.640028").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::H264);
    assert_eq!(parsed.profile, Some(100)); // 0x64 = 100 = High
    assert_eq!(parsed.level, Some(40)); // 0x28 = 40 = Level 4.0
  }

  #[test]
  fn test_parse_hevc() {
    let parsed = parse_codec_string("hev1.1.6.L120.B0").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::Hevc);
    assert_eq!(parsed.profile, Some(1)); // Main
    assert_eq!(parsed.level, Some(120)); // Level 4.0
    assert_eq!(parsed.bit_depth, Some(8));
  }

  #[test]
  fn test_parse_hevc_main10() {
    let parsed = parse_codec_string("hev1.2.4.L150.B0").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::Hevc);
    assert_eq!(parsed.profile, Some(2)); // Main 10
    assert_eq!(parsed.level, Some(150)); // Level 5.0
    assert_eq!(parsed.bit_depth, Some(10));
  }

  #[test]
  fn test_parse_vp8() {
    let parsed = parse_codec_string("vp8").unwrap();
    assert_eq!(parsed.codec_id, AVCodecID::Vp8);
  }

  #[test]
  fn test_parse_unknown() {
    let parsed = parse_codec_string("unknown-codec");
    assert!(parsed.is_none());
  }
}
