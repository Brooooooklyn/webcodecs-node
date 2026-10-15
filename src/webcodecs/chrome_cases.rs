//! Test data for the GIF and BMP decoders: synthetic files with the frames Chrome 155
//! decodes from them (__test__/fixtures/gif-bmp-chrome155.txt)

/// What Chrome's ImageDecoder gives for one frame
#[derive(Debug, PartialEq, Eq)]
pub enum Expected {
  /// `tracks.ready` rejects
  OpenError,
  /// `decode()` rejects
  Error,
  Frame {
    alpha: bool,
    width: u32,
    height: u32,
    /// FNV-1a 64 of the `copyTo()` bytes
    hash: u64,
  },
}

pub struct Case {
  pub name: String,
  pub data: Vec<u8>,
  /// Chrome's frame count (GIF only)
  pub frame_count: Option<usize>,
  pub frames: Vec<Expected>,
}

/// The cases whose name starts with `prefix` ("gif/" or "bmp/")
pub fn cases(prefix: &str) -> Vec<Case> {
  let path = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/__test__/fixtures/gif-bmp-chrome155.txt"
  );
  let text = std::fs::read_to_string(path).unwrap();
  let cases: Vec<Case> = text
    .lines()
    .filter(|line| line.starts_with(prefix))
    .map(|line| {
      let mut words = line.split(' ');
      let name = words.next().unwrap().to_string();
      let data = base64(words.next().unwrap());
      let mut frame_count = None;
      let frames = words
        .filter_map(|word| {
          if let Some(n) = word.strip_prefix("frames=") {
            frame_count = Some(n.parse().unwrap());
            return None;
          }
          Some(match word {
            "open-error" => Expected::OpenError,
            "error" => Expected::Error,
            _ => {
              let [format, size, hash] = word.split(':').collect::<Vec<_>>()[..] else {
                panic!("bad result {word}");
              };
              let (width, height) = size.split_once('x').unwrap();
              Expected::Frame {
                alpha: format == "BGRA",
                width: width.parse().unwrap(),
                height: height.parse().unwrap(),
                hash: u64::from_str_radix(hash, 16).unwrap(),
              }
            }
          })
        })
        .collect();
      Case {
        name,
        data,
        frame_count,
        frames,
      }
    })
    .collect();
  assert!(!cases.is_empty());
  cases
}

pub fn fnv1a(bytes: &[u8]) -> u64 {
  bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
    (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
  })
}

fn base64(text: &str) -> Vec<u8> {
  const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let mut out = Vec::new();
  let (mut bits, mut n) = (0u32, 0);
  for c in text.bytes().filter(|&c| c != b'=' && c != b'-') {
    let v = ALPHABET.iter().position(|&a| a == c).unwrap() as u32;
    bits = (bits << 6) | v;
    n += 6;
    if n >= 8 {
      n -= 8;
      out.push((bits >> n) as u8);
    }
  }
  out
}

/// Truncated and corrupted copies of `data`, the same every run
pub fn broken_copies(data: &[u8]) -> Vec<Vec<u8>> {
  let mut copies: Vec<Vec<u8>> = (0..data.len())
    .step_by(data.len() / 64 + 1)
    .map(|len| data[..len].to_vec())
    .collect();
  let mut seed = 0x9E37_79B9_7F4A_7C15_u64 ^ data.len() as u64;
  let mut next = move || {
    seed ^= seed << 13;
    seed ^= seed >> 7;
    seed ^= seed << 17;
    seed
  };
  for _ in 0..32 {
    let mut copy = data.to_vec();
    for _ in 0..1 + next() % 4 {
      if !copy.is_empty() {
        let at = next() as usize % copy.len();
        copy[at] = next() as u8;
      }
    }
    copies.push(copy);
  }
  copies
}
