//! MP4 FastStart Post-Processing
//!
//! This module provides functionality to move the moov atom to the beginning
//! of an MP4 file for faster streaming playback. This is necessary because
//! FFmpeg's faststart option doesn't work with custom I/O contexts.

use std::io::{Cursor, Read, Seek, SeekFrom};

/// Apply faststart post-processing to MP4 data
///
/// This moves the moov atom to the beginning of the file (after ftyp),
/// updating all chunk offsets in stco/co64 boxes as needed.
///
/// Returns the modified MP4 data, or the original data if moov is already
/// at the beginning or if parsing fails.
pub fn apply_faststart(data: Vec<u8>) -> Vec<u8> {
  match apply_faststart_inner(&data) {
    Ok(result) => result,
    Err(e) => {
      tracing::warn!(target: "ffmpeg", "faststart post-processing failed: {}, returning original data", e);
      data
    }
  }
}

/// Internal faststart implementation
fn apply_faststart_inner(data: &[u8]) -> Result<Vec<u8>, FastStartError> {
  let atoms = parse_atoms(data)?;

  // Find ftyp, moov, and mdat atoms
  let ftyp = atoms.iter().find(|a| &a.atom_type == b"ftyp");
  let moov = atoms.iter().find(|a| &a.atom_type == b"moov");
  let mdat = atoms.iter().find(|a| &a.atom_type == b"mdat");

  let moov = moov.ok_or(FastStartError::MissingAtom("moov"))?;
  let mdat = mdat.ok_or(FastStartError::MissingAtom("mdat"))?;

  // Check if moov is already before mdat
  if moov.offset < mdat.offset {
    tracing::trace!(target: "ffmpeg", "moov already before mdat, no faststart needed");
    return Ok(data.to_vec());
  }

  tracing::trace!(target: "ffmpeg", "applying faststart: moov at {}, mdat at {}", moov.offset, mdat.offset);

  // Calculate the new positions
  // Layout will be: ftyp | moov | mdat | other atoms
  let _ftyp_end = ftyp.map(|f| f.offset + f.size).unwrap_or(0);

  // The offset adjustment for chunk offsets:
  // Original: chunks point to data in mdat at its old position
  // New: mdat moves forward by moov.size bytes (moov is now before mdat)
  // So all chunk offsets need to increase by moov.size
  let offset_adjustment = moov.size as i64;

  // Extract moov data and update chunk offsets
  let moov_data = &data[moov.offset..moov.offset + moov.size];
  let updated_moov = update_chunk_offsets(moov_data, offset_adjustment)?;

  // Build the new file
  let mut result = Vec::with_capacity(data.len());

  // Write ftyp (if present)
  if let Some(ftyp) = ftyp {
    result.extend_from_slice(&data[ftyp.offset..ftyp.offset + ftyp.size]);
  }

  // Write updated moov
  result.extend_from_slice(&updated_moov);

  // Write all other atoms in their original order (except moov and ftyp)
  for atom in &atoms {
    if &atom.atom_type != b"moov" && &atom.atom_type != b"ftyp" {
      result.extend_from_slice(&data[atom.offset..atom.offset + atom.size]);
    }
  }

  tracing::trace!(target: "ffmpeg", "faststart complete: {} -> {} bytes", data.len(), result.len());
  Ok(result)
}

/// Parsed atom information
#[derive(Debug)]
struct AtomInfo {
  atom_type: [u8; 4],
  offset: usize,
  size: usize,
}

/// Parse top-level atoms from MP4 data
fn parse_atoms(data: &[u8]) -> Result<Vec<AtomInfo>, FastStartError> {
  let mut atoms = Vec::new();
  let mut cursor = Cursor::new(data);

  while cursor.position() < data.len() as u64 {
    let offset = cursor.position() as usize;

    // Read atom size (4 bytes, big-endian)
    let mut size_buf = [0u8; 4];
    if cursor.read_exact(&mut size_buf).is_err() {
      break;
    }
    let mut size = u32::from_be_bytes(size_buf) as u64;

    // Read atom type (4 bytes)
    let mut atom_type = [0u8; 4];
    if cursor.read_exact(&mut atom_type).is_err() {
      break;
    }

    // Handle extended size (size == 1 means 64-bit size follows)
    if size == 1 {
      let mut ext_size_buf = [0u8; 8];
      if cursor.read_exact(&mut ext_size_buf).is_err() {
        break;
      }
      size = u64::from_be_bytes(ext_size_buf);
    } else if size == 0 {
      // Size 0 means atom extends to end of file
      size = (data.len() - offset) as u64;
    }

    // Validate size
    if size < 8 || offset + size as usize > data.len() {
      return Err(FastStartError::InvalidAtomSize);
    }

    atoms.push(AtomInfo {
      atom_type,
      offset,
      size: size as usize,
    });

    // Move to next atom
    cursor.seek(SeekFrom::Start(offset as u64 + size))?;
  }

  Ok(atoms)
}

/// Update chunk offsets in moov atom
fn update_chunk_offsets(moov_data: &[u8], adjustment: i64) -> Result<Vec<u8>, FastStartError> {
  let mut result = moov_data.to_vec();
  update_chunk_offsets_recursive(&mut result, 8, adjustment)?;
  Ok(result)
}

/// Recursively update chunk offsets in moov sub-atoms
fn update_chunk_offsets_recursive(
  data: &mut [u8],
  start: usize,
  adjustment: i64,
) -> Result<(), FastStartError> {
  let mut pos = start;
  let len = data.len();

  while pos + 8 <= len {
    // Read atom size
    let size_bytes: [u8; 4] = data[pos..pos + 4].try_into().unwrap();
    let size = u32::from_be_bytes(size_bytes) as usize;

    if size < 8 || pos + size > len {
      break;
    }

    // Read atom type
    let atom_type: [u8; 4] = data[pos + 4..pos + 8].try_into().unwrap();

    match &atom_type {
      b"stco" => {
        // Standard chunk offset table (32-bit offsets)
        update_stco(&mut data[pos..pos + size], adjustment)?;
      }
      b"co64" => {
        // Extended chunk offset table (64-bit offsets)
        update_co64(&mut data[pos..pos + size], adjustment)?;
      }
      // Container atoms that may contain stco/co64
      b"trak" | b"mdia" | b"minf" | b"stbl" | b"moov" => {
        update_chunk_offsets_recursive(data, pos + 8, adjustment)?;
      }
      _ => {}
    }

    pos += size;
  }

  Ok(())
}

/// Update 32-bit chunk offsets in stco atom
fn update_stco(data: &mut [u8], adjustment: i64) -> Result<(), FastStartError> {
  // stco format:
  // 4 bytes: size
  // 4 bytes: type ("stco")
  // 1 byte: version
  // 3 bytes: flags
  // 4 bytes: entry count
  // N * 4 bytes: chunk offsets

  if data.len() < 16 {
    return Err(FastStartError::InvalidAtomSize);
  }

  let entry_count = u32::from_be_bytes(data[12..16].try_into().unwrap()) as usize;
  let expected_size = 16 + entry_count * 4;

  if data.len() < expected_size {
    return Err(FastStartError::InvalidAtomSize);
  }

  for i in 0..entry_count {
    let offset_pos = 16 + i * 4;
    let old_offset = u32::from_be_bytes(data[offset_pos..offset_pos + 4].try_into().unwrap());
    let new_offset = (old_offset as i64 + adjustment) as u32;
    data[offset_pos..offset_pos + 4].copy_from_slice(&new_offset.to_be_bytes());
  }

  tracing::trace!(target: "ffmpeg", "updated {} stco entries by {} bytes", entry_count, adjustment);
  Ok(())
}

/// Update 64-bit chunk offsets in co64 atom
fn update_co64(data: &mut [u8], adjustment: i64) -> Result<(), FastStartError> {
  // co64 format:
  // 4 bytes: size
  // 4 bytes: type ("co64")
  // 1 byte: version
  // 3 bytes: flags
  // 4 bytes: entry count
  // N * 8 bytes: chunk offsets

  if data.len() < 16 {
    return Err(FastStartError::InvalidAtomSize);
  }

  let entry_count = u32::from_be_bytes(data[12..16].try_into().unwrap()) as usize;
  let expected_size = 16 + entry_count * 8;

  if data.len() < expected_size {
    return Err(FastStartError::InvalidAtomSize);
  }

  for i in 0..entry_count {
    let offset_pos = 16 + i * 8;
    let old_offset = u64::from_be_bytes(data[offset_pos..offset_pos + 8].try_into().unwrap());
    let new_offset = (old_offset as i64 + adjustment) as u64;
    data[offset_pos..offset_pos + 8].copy_from_slice(&new_offset.to_be_bytes());
  }

  tracing::trace!(target: "ffmpeg", "updated {} co64 entries by {} bytes", entry_count, adjustment);
  Ok(())
}

/// FastStart error type
#[derive(Debug, thiserror::Error)]
pub enum FastStartError {
  #[error("Missing required atom: {0}")]
  MissingAtom(&'static str),

  #[error("Invalid atom size")]
  InvalidAtomSize,

  #[error("I/O error: {0}")]
  Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_atoms() {
    // Simple MP4 structure: ftyp + mdat + moov
    let mut data = Vec::new();

    // ftyp atom (20 bytes)
    data.extend_from_slice(&20u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    data.extend_from_slice(b"isom");
    data.extend_from_slice(&0x200u32.to_be_bytes());
    data.extend_from_slice(b"isom");

    // mdat atom (16 bytes)
    data.extend_from_slice(&16u32.to_be_bytes());
    data.extend_from_slice(b"mdat");
    data.extend_from_slice(b"testdata");

    // moov atom (8 bytes, minimal)
    data.extend_from_slice(&8u32.to_be_bytes());
    data.extend_from_slice(b"moov");

    let atoms = parse_atoms(&data).unwrap();
    assert_eq!(atoms.len(), 3);
    assert_eq!(&atoms[0].atom_type, b"ftyp");
    assert_eq!(&atoms[1].atom_type, b"mdat");
    assert_eq!(&atoms[2].atom_type, b"moov");
  }
}
