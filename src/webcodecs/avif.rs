//! A reader for the AVIF boxes that Chrome's AVIF decoder (CrabbyAvif) uses to choose
//! what to decode: the primary item or a track, its alpha auxiliary image, 'prem', and
//! the bytes and durations of every AV1 sample. FFmpeg's mov demuxer reads neither
//! 'auxl' nor 'prem', and fails on an item stored in several extents.
//!
//! The rules follow CrabbyAvif as Chrome 155 ships it. A structure this reader does not
//! handle (grid and other derived items, 'mini', ...), and most that Chrome rejects,
//! give `Avif::Decline`: the caller then decodes with FFmpeg as before.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

/// CrabbyAvif's image count limit: 12 hours at 60 frames per second
const MAX_IMAGE_COUNT: usize = 12 * 3600 * 60;
/// Limit for every list read from the boxes (items, extents, properties, references,
/// tracks). An image without a grid has a few dozen entries.
const MAX_ENTRIES: usize = 1 << 16;
/// The auxC / auxi types of an alpha auxiliary image
const ALPHA_URNS: [&[u8]; 2] = [
  b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha",
  b"urn:mpeg:hevc:2015:auxid:1",
];

/// What `parse` found
pub(crate) enum Avif {
  Image(AvifImage),
  /// The bytes end before a box the image needs ('ftyp', 'meta' or 'moov'). With all
  /// the bytes in, the file is cut short and Chrome rejects it.
  Incomplete,
  /// A structure this reader does not handle, or that Chrome rejects
  Decline,
}

/// What Chrome decodes from an AVIF file
pub(crate) struct AvifImage {
  /// The colour image. Its sample count is the frame count.
  pub color: AvifSamples,
  /// The alpha auxiliary image, with as many samples as the colour image
  pub alpha: Option<AvifSamples>,
  /// 'prem': the colour samples are premultiplied by the alpha samples
  pub premultiplied: bool,
  /// The colour item's 'colr' nclx, which Chrome uses over the AV1 sequence header.
  /// A track has none: Chrome ignores the colr of its sample entry.
  pub nclx: Option<Nclx>,
  /// Ticks per second of the sample durations ('mdhd'), 0 for an item
  pub timescale: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Nclx {
  pub primaries: u16,
  pub transfer: u16,
  pub matrix: u16,
  pub full_range: bool,
}

/// The AV1 samples of an item (one sample, maybe in several extents) or of a track
pub(crate) struct AvifSamples {
  /// 'ispe' of an item, 'tkhd' of a track. Chrome scales a decoded frame to this size.
  pub width: u32,
  pub height: u32,
  /// The AV1CodecConfigurationRecord ('av1C')
  pub config: Vec<u8>,
  samples: Samples,
}

enum Samples {
  /// File byte ranges that together make the only sample
  Item(Vec<Range<u64>>),
  Track(Vec<TrackSample>),
}

struct TrackSample {
  offset: u64,
  size: u32,
  /// In timescale ticks
  duration: u32,
}

impl AvifSamples {
  pub fn len(&self) -> usize {
    match &self.samples {
      Samples::Item(_) => 1,
      Samples::Track(samples) => samples.len(),
    }
  }

  /// The bytes of sample `index`, or None while some of them have not arrived. With all
  /// the bytes in, a sample past the end makes Chrome reject the file.
  pub fn sample<'d>(&self, data: &'d [u8], index: usize) -> Option<Cow<'d, [u8]>> {
    let bytes = |range: &Range<u64>| {
      data.get(usize::try_from(range.start).ok()?..usize::try_from(range.end).ok()?)
    };
    match &self.samples {
      Samples::Item(extents) if index == 0 => match extents.as_slice() {
        [extent] => bytes(extent).map(Cow::Borrowed),
        extents => extents
          .iter()
          .map(bytes)
          .collect::<Option<Vec<_>>>()
          .map(|parts| Cow::Owned(parts.concat())),
      },
      Samples::Item(_) => None,
      Samples::Track(samples) => {
        let sample = samples.get(index)?;
        bytes(&(sample.offset..sample.offset + u64::from(sample.size))).map(Cow::Borrowed)
      }
    }
  }

  /// The duration of sample `index` in timescale ticks, 0 for an item
  pub fn duration(&self, index: usize) -> u32 {
    match &self.samples {
      Samples::Item(_) => 0,
      Samples::Track(samples) => samples.get(index).map_or(0, |sample| sample.duration),
    }
  }
}

/// Read an AVIF file, or as much of it as has arrived. `prefer_animation` is the
/// ImageDecoderInit option: true decodes the track, false the primary item, and
/// unset follows the major brand ('avis' track, 'avif' item, else a track if any).
pub(crate) fn parse(data: &[u8], prefer_animation: Option<bool>) -> Avif {
  let mut file = Reader {
    data,
    pos: 0,
    end: data.len(),
  };
  // The top-level boxes, up to the ones the brands require: 'meta' for 'avif', 'moov'
  // for 'avis'. Later boxes are not read, like in CrabbyAvif.
  let (mut ftyp, mut meta, mut tracks) = (None, None, None);
  let ftyp = loop {
    let start = file.pos;
    let Some((kind, size)) = file.header() else {
      return Avif::Incomplete;
    };
    // size 0: the box runs to the end of the file
    let end = match size {
      0 => None,
      size => match usize::try_from(size)
        .ok()
        .and_then(|size| start.checked_add(size))
        .filter(|&end| end >= file.pos)
      {
        Some(end) => Some(end),
        None => return Avif::Decline,
      },
    };
    if let b"ftyp" | b"meta" | b"moov" | b"mini" = &kind {
      // ftyp comes before the others
      if ftyp.is_none() && &kind != b"ftyp" {
        return Avif::Decline;
      }
      let Some(end) = end else {
        return Avif::Decline;
      };
      if end > data.len() {
        return Avif::Incomplete;
      }
      let body = Reader {
        pos: file.pos,
        end,
        ..file
      };
      let parsed = match &kind {
        b"ftyp" => parse_ftyp(body).map(|f| ftyp = Some(f)),
        b"meta" => parse_meta(body).map(|m| meta = Some(m)),
        b"moov" => parse_moov(body).map(|t| tracks = Some(t)),
        // MinimizedImageBox
        _ => None,
      };
      if parsed.is_none() {
        return Avif::Decline;
      }
      if let Some(ftyp) =
        ftyp.take_if(|f| (!f.avif || meta.is_some()) && (!f.avis || tracks.is_some()))
      {
        break ftyp;
      }
    }
    match end {
      Some(end) if end < data.len() => file.pos = end,
      _ => return Avif::Incomplete,
    }
  };
  let from_tracks = prefer_animation.unwrap_or(match &ftyp.major {
    b"avis" => true,
    b"avif" => false,
    _ => tracks.is_some(),
  });
  let image = if from_tracks {
    tracks.as_deref().and_then(track_image)
  } else {
    meta.as_ref().and_then(item_image)
  };
  image.map_or(Avif::Decline, Avif::Image)
}

struct Ftyp {
  major: [u8; 4],
  avif: bool,
  avis: bool,
}

fn parse_ftyp(mut r: Reader) -> Option<Ftyp> {
  let major = r.array()?;
  let minor: [u8; 4] = r.array()?;
  if !r.remaining().is_multiple_of(4) {
    return None;
  }
  let mut ftyp = Ftyp {
    major,
    avif: &major == b"avif",
    avis: &major == b"avis",
  };
  while r.remaining() > 0 {
    match &r.array()? {
      b"avif" => ftyp.avif = true,
      b"avis" => ftyp.avis = true,
      _ => {}
    }
  }
  // major brand 'mif3' with minor version 'avif' needs a 'mini' box
  let mini = &major == b"mif3" && &minor == b"avif";
  (!mini && (ftyp.avif || ftyp.avis)).then_some(ftyp)
}

/// An item property ('ipco' entry) or a box of a sample entry
enum Property<'a> {
  Av1C {
    config: &'a [u8],
    depth: u8,
  },
  Ispe(u32, u32),
  /// Bits per channel, the same for every channel
  Pixi(u8),
  /// auxC (item) or auxi (track) type
  Aux(&'a [u8]),
  Nclx(Nclx),
  /// An operating point or layer selection ('a1op', 'lsel'), not handled here
  Unsupported,
  /// Known to CrabbyAvif and not used here (ICC 'colr', 'clap', 'irot', ...)
  Ignored,
  /// Unknown to CrabbyAvif: an item with such an essential property is skipped
  Unknown,
}

fn parse_property<'a>(kind: [u8; 4], mut r: Reader<'a>, track: bool) -> Option<Property<'a>> {
  Some(match &kind {
    b"av1C" => {
      let config = &r.data[r.pos..r.end];
      // marker and version, profile and level, flags, presentation delay
      let [marker, _, flags, delay] = r.array()?;
      // reserved bits, and the delay bits when no delay is present
      if marker != 0x81 || delay & 0xE0 != 0 || (delay & 0x10 == 0 && delay & 0x0F != 0) {
        return None;
      }
      // high_bitdepth 0x40, twelve_bit 0x20
      let depth = match flags & 0x60 {
        0 => 8,
        0x40 => 10,
        _ => 12,
      };
      Property::Av1C { config, depth }
    }
    b"ispe" => {
      if r.full()?.0 != 0 {
        return None;
      }
      Property::Ispe(r.u32()?, r.u32()?)
    }
    b"pixi" => {
      let (version, flags) = r.full()?;
      if version != 0 {
        return None;
      }
      // extended pixi (channel types and subsampling)
      if flags & 1 != 0 {
        return Some(Property::Unsupported);
      }
      let channels = r.u8()?;
      let depths = r.bytes(channels.into())?;
      if !(1..=4).contains(&channels) || depths.iter().any(|&d| d != depths[0]) {
        return None;
      }
      Property::Pixi(depths[0])
    }
    b"auxC" if !track => aux_type(r)?,
    b"auxi" if track => aux_type(r)?,
    b"colr" => match &r.array()? {
      b"nclx" => {
        let (primaries, transfer, matrix, range) = (r.u16()?, r.u16()?, r.u16()?, r.u8()?);
        // reserved bits
        if range & 0x7F != 0 {
          return None;
        }
        Property::Nclx(Nclx {
          primaries,
          transfer,
          matrix,
          full_range: range != 0,
        })
      }
      b"rICC" | b"prof" if r.remaining() == 0 => return None,
      _ => Property::Ignored,
    },
    b"a1op" | b"lsel" => Property::Unsupported,
    b"alpi" | b"pasp" | b"clap" | b"irot" | b"imir" | b"a1lx" | b"clli" => Property::Ignored,
    _ => Property::Unknown,
  })
}

fn aux_type(mut r: Reader) -> Option<Property> {
  if r.full()?.0 != 0 {
    return None;
  }
  r.c_string().map(Property::Aux)
}

fn parse_properties(mut r: Reader, track: bool) -> Option<Vec<Property>> {
  let mut properties = Vec::new();
  while r.remaining() > 0 {
    let (kind, body) = r.child()?;
    push(&mut properties, parse_property(kind, body, track)?)?;
  }
  Some(properties)
}

#[derive(Default)]
struct Meta<'a> {
  primary: u32,
  /// Item types from 'infe'
  item_types: HashMap<u32, [u8; 4]>,
  locations: HashMap<u32, Location>,
  /// The 'idat' payload
  idat: Option<Range<usize>>,
  properties: Vec<Property<'a>>,
  /// The (1-based property index, essential) associations of each item
  associations: HashMap<u32, Vec<(usize, bool)>>,
  /// (type, from item id, to item id)
  references: Vec<([u8; 4], u32, u32)>,
  /// Ids in the 'altr' (alternatives) entity groups
  alternatives: Vec<u32>,
}

/// An 'iloc' entry
struct Location {
  /// Construction method 1: the extents are in 'idat'
  in_idat: bool,
  extents: Vec<Range<u64>>,
  size: u64,
}

fn parse_meta(mut r: Reader) -> Option<Meta> {
  if r.full()?.0 != 0 {
    return None;
  }
  // the first box is the 'pict' handler
  let (kind, hdlr) = r.child()?;
  if &kind != b"hdlr" || &handler(hdlr)? != b"pict" {
    return None;
  }
  let mut meta = Meta::default();
  let mut seen = vec![kind];
  while r.remaining() > 0 {
    let (kind, mut body) = r.child()?;
    if let b"hdlr" | b"iloc" | b"pitm" | b"iprp" | b"iinf" | b"iref" | b"idat" | b"grpl" = &kind {
      if seen.contains(&kind) {
        return None;
      }
      seen.push(kind);
    }
    match &kind {
      b"pitm" => {
        meta.primary = match body.full()?.0 {
          0 => body.u16()?.into(),
          _ => body.u32()?,
        }
      }
      b"iinf" => parse_iinf(body, &mut meta)?,
      b"iloc" => parse_iloc(body, &mut meta)?,
      b"idat" if body.remaining() > 0 => meta.idat = Some(body.pos..body.end),
      b"idat" => return None,
      b"iprp" => parse_iprp(body, &mut meta)?,
      b"iref" => parse_iref(body, &mut meta)?,
      b"grpl" => parse_grpl(body, &mut meta)?,
      _ => {}
    }
  }
  Some(meta)
}

/// The handler type of an 'hdlr' box
fn handler(mut r: Reader) -> Option<[u8; 4]> {
  r.full()?;
  // pre_defined
  r.skip(4)?;
  r.array()
}

fn parse_iinf(mut r: Reader, meta: &mut Meta) -> Option<()> {
  let count = match r.full()?.0 {
    0 => r.u16()?.into(),
    _ => r.u32()?,
  };
  for _ in 0..count {
    let (kind, mut infe) = r.child()?;
    if &kind != b"infe" {
      return None;
    }
    let id = match infe.full()?.0 {
      2 => infe.u16()?.into(),
      3 => infe.u32()?,
      _ => return None,
    };
    // protection index
    infe.skip(2)?;
    let item_type = infe.array()?;
    // item name
    infe.c_string()?;
    if id == 0 || meta.item_types.len() >= MAX_ENTRIES {
      return None;
    }
    // a later 'infe' replaces an earlier one
    meta.item_types.insert(id, item_type);
  }
  Some(())
}

fn parse_iloc(mut r: Reader, meta: &mut Meta) -> Option<()> {
  let version = r.full()?.0;
  let [sizes, more] = r.array()?;
  let (offset_size, length_size, base_size) = (sizes >> 4, sizes & 0x0F, more >> 4);
  let index_size = if version > 0 { more & 0x0F } else { 0 };
  if version > 2
    || [offset_size, length_size, base_size, index_size]
      .iter()
      .any(|size| ![0, 4, 8].contains(size))
  {
    return None;
  }
  let count = match version {
    2 => r.u32()?,
    _ => r.u16()?.into(),
  };
  let mut extent_count = 0;
  for _ in 0..count {
    let item = match version {
      2 => r.u32()?,
      _ => r.u16()?.into(),
    };
    // 12 reserved bits, then construction method 0 (file) or 1 (idat)
    let method = if version > 0 { r.u16()? } else { 0 };
    // data reference index
    r.skip(2)?;
    let base = r.uint(base_size)?;
    let mut location = Location {
      in_idat: method == 1,
      extents: Vec::new(),
      size: 0,
    };
    for _ in 0..r.u16()? {
      extent_count += 1;
      r.skip(index_size.into())?;
      let start = base.checked_add(r.uint(offset_size)?)?;
      let length = r.uint(length_size)?;
      location.extents.push(start..start.checked_add(length)?);
      location.size = location.size.checked_add(length)?;
    }
    if item == 0 || method > 1 || extent_count > MAX_ENTRIES || meta.locations.len() >= MAX_ENTRIES
    {
      return None;
    }
    // Chrome rejects several entries for one item
    if meta.locations.insert(item, location).is_some() {
      return None;
    }
  }
  Some(())
}

fn parse_iprp<'a>(mut r: Reader<'a>, meta: &mut Meta<'a>) -> Option<()> {
  // 'ipco' first, then 'ipma' boxes
  let (kind, ipco) = r.child()?;
  if &kind != b"ipco" {
    return None;
  }
  meta.properties = parse_properties(ipco, false)?;
  while r.remaining() > 0 {
    let (kind, mut ipma) = r.child()?;
    if &kind != b"ipma" {
      return None;
    }
    let (version, flags) = ipma.full()?;
    let mut previous = 0;
    for _ in 0..ipma.u32()? {
      let item = match version {
        0 => ipma.u16()?.into(),
        _ => ipma.u32()?,
      };
      // increasing item ids
      if item <= previous {
        return None;
      }
      previous = item;
      let mut associations = Vec::new();
      for _ in 0..ipma.u8()? {
        // the essential bit, then a 15-bit or 7-bit property index
        let (value, top) = match flags & 1 {
          1 => (ipma.u16()?, 0x8000),
          _ => (ipma.u8()?.into(), 0x80),
        };
        let (essential, index) = (value & top != 0, usize::from(value & (top - 1)));
        if index > meta.properties.len() || (index == 0 && essential) {
          return None;
        }
        associations.push((index, essential));
      }
      if associations.is_empty() {
        continue;
      }
      // at most one association list per item
      if meta.associations.len() >= MAX_ENTRIES
        || meta.associations.insert(item, associations).is_some()
      {
        return None;
      }
    }
  }
  Some(())
}

fn parse_iref(mut r: Reader, meta: &mut Meta) -> Option<()> {
  let version = r.full()?.0;
  // CrabbyAvif ignores later versions
  if version > 1 {
    return Some(());
  }
  let id = |r: &mut Reader| match version {
    0 => r.u16().map(u32::from),
    _ => r.u32(),
  };
  while r.remaining() > 0 {
    let (kind, mut refs) = r.child()?;
    let from = id(&mut refs).filter(|&id| id != 0)?;
    for _ in 0..refs.u16()? {
      let to = id(&mut refs).filter(|&id| id != 0)?;
      push(&mut meta.references, (kind, from, to))?;
    }
  }
  Some(())
}

fn parse_grpl(mut r: Reader, meta: &mut Meta) -> Option<()> {
  while r.remaining() > 0 {
    let (kind, mut group) = r.child()?;
    // version and flags, group id
    group.skip(8)?;
    for _ in 0..group.u32()? {
      let id = group.u32()?;
      if &kind == b"altr" {
        push(&mut meta.alternatives, id)?;
      }
    }
  }
  Some(())
}

impl<'a> Meta<'a> {
  fn item_type(&self, item: u32) -> [u8; 4] {
    self.item_types.get(&item).copied().unwrap_or_default()
  }

  fn properties(&self, item: u32) -> impl Iterator<Item = (&Property<'a>, bool)> {
    self
      .associations
      .get(&item)
      .into_iter()
      .flatten()
      .filter(|(index, _)| *index > 0)
      .map(|&(index, essential)| (&self.properties[index - 1], essential))
  }

  /// CrabbyAvif skips an item without data (unless it is 'iden'), with an unknown
  /// essential property, or that is not an image
  fn skipped(&self, item: u32) -> bool {
    let item_type = self.item_type(item);
    let image = matches!(
      &item_type,
      b"av01" | b"grid" | b"iovl" | b"iden" | b"tmap" | b"sato"
    );
    let size = self.locations.get(&item).map_or(0, |l| l.size);
    !image
      || (&item_type != b"iden" && size == 0)
      || self
        .properties(item)
        .any(|(property, essential)| essential && matches!(property, Property::Unknown))
  }

  fn is_alpha(&self, item: u32) -> bool {
    let aux = self
      .properties(item)
      .find_map(|(property, _)| match property {
        Property::Aux(aux) => Some(*aux),
        _ => None,
      });
    aux.is_some_and(|aux| ALPHA_URNS.contains(&aux)) && &self.item_type(item) != b"sato"
  }

  /// The sample of an AV1 item, with its 'ispe' size and 'av1C'
  fn samples(&self, item: u32) -> Option<AvifSamples> {
    if &self.item_type(item) != b"av01" || self.skipped(item) {
      return None;
    }
    let properties: Vec<_> = self
      .properties(item)
      .map(|(property, _)| property)
      .collect();
    if properties
      .iter()
      .any(|p| matches!(p, Property::Unsupported))
    {
      return None;
    }
    let (width, height) = properties
      .iter()
      .find_map(|p| match p {
        Property::Ispe(width, height) => Some((*width, *height)),
        _ => None,
      })
      .filter(|&(width, height)| width > 0 && height > 0)?;
    let (config, depth) = properties.iter().find_map(|p| match p {
      Property::Av1C { config, depth } => Some((*config, *depth)),
      _ => None,
    })?;
    let pixi = properties.iter().find_map(|p| match p {
      Property::Pixi(depth) => Some(*depth),
      _ => None,
    });
    if pixi.is_some_and(|pixi| pixi != depth) {
      return None;
    }
    let location = self.locations.get(&item)?;
    let extents = match (&self.idat, location.in_idat) {
      (Some(idat), true) => location
        .extents
        .iter()
        .map(|extent| {
          let start = idat.start as u64;
          (extent.end <= idat.len() as u64).then(|| start + extent.start..start + extent.end)
        })
        .collect::<Option<_>>()?,
      // without 'idat', CrabbyAvif reads construction method 1 from the file
      _ => location.extents.clone(),
    };
    Some(AvifSamples {
      width,
      height,
      config: config.to_vec(),
      samples: Samples::Item(extents),
    })
  }
}

/// The primary item, its alpha item and their 'prem' link
fn item_image(meta: &Meta) -> Option<AvifImage> {
  // Chrome may decode an alternative to the primary item (a 'sato' derived image)
  if meta.alternatives.contains(&meta.primary) {
    return None;
  }
  let color = meta.samples(meta.primary)?;
  // the lowest item id that is an alpha 'auxl' of the colour item
  let alpha = meta
    .references
    .iter()
    .filter(|&&(kind, from, to)| &kind == b"auxl" && to == meta.primary && !meta.skipped(from))
    .map(|&(_, from, _)| from)
    .filter(|&item| meta.is_alpha(item))
    .min();
  let mut nclx = meta
    .properties(meta.primary)
    .filter_map(|(property, _)| match property {
      Property::Nclx(nclx) => Some(*nclx),
      _ => None,
    });
  let (nclx, second_nclx) = (nclx.next(), nclx.next());
  if second_nclx.is_some() {
    return None;
  }
  Some(AvifImage {
    color,
    alpha: match alpha {
      Some(alpha) => Some(meta.samples(alpha)?),
      None => None,
    },
    premultiplied: alpha
      .is_some_and(|alpha| meta.references.contains(&(*b"prem", meta.primary, alpha))),
    nclx,
    timescale: 0,
  })
}

#[derive(Default)]
struct Track<'a> {
  id: u32,
  handler: [u8; 4],
  width: u32,
  height: u32,
  timescale: u32,
  /// 'tref' auxl: the track this one is an auxiliary image of
  aux_for: Option<u32>,
  /// 'tref' prem: the track that premultiplied this one
  prem_by: Option<u32>,
  table: Option<SampleTable<'a>>,
}

/// The entries of the 'stbl' boxes, as stored
#[derive(Default)]
struct SampleTable<'a> {
  /// 'stco' (4-byte) or 'co64' (8-byte) entries, and their size
  chunk_offsets: Option<(&'a [u8], usize)>,
  /// 'stsc' entries: first chunk, samples per chunk, sample description index
  sample_to_chunk: Option<&'a [u8]>,
  /// 'stsz': the size of every sample (0: each has its own), and the sizes
  sample_sizes: Option<(u32, &'a [u8])>,
  /// 'stts' entries: sample count, sample delta
  time_to_sample: Option<&'a [u8]>,
  /// The properties of the first 'av01' sample entry
  av01: Option<Vec<Property<'a>>>,
}

fn parse_moov(mut r: Reader) -> Option<Vec<Track>> {
  let mut tracks = Vec::new();
  while r.remaining() > 0 {
    let (kind, body) = r.child()?;
    if &kind == b"trak" {
      let track = parse_trak(body)?;
      if track.is_video() && (track.width == 0 || track.height == 0) {
        return None;
      }
      push(&mut tracks, track)?;
    }
  }
  (!tracks.is_empty()).then_some(tracks)
}

fn parse_trak(mut r: Reader) -> Option<Track> {
  let mut track = Track::default();
  let mut tkhd = false;
  while r.remaining() > 0 {
    let (kind, mut body) = r.child()?;
    match &kind {
      b"tkhd" if !tkhd => {
        tkhd = true;
        // creation and modification times, then reserved and duration after the id
        let (times, rest) = match body.full()?.0 {
          0 => (8, 8),
          1 => (16, 12),
          _ => return None,
        };
        body.skip(times)?;
        track.id = body.u32()?;
        // rest, reserved, layer, alternate group, volume, reserved, matrix
        body.skip(rest + 8 + 8 + 36)?;
        // 16.16 fixed point
        track.width = body.u32()? >> 16;
        track.height = body.u32()? >> 16;
      }
      b"tkhd" => return None,
      b"tref" => {
        while body.remaining() > 0 {
          let (kind, mut ids) = body.child()?;
          // the first referenced track
          match &kind {
            b"auxl" => track.aux_for = Some(ids.u32()?),
            b"prem" => track.prem_by = Some(ids.u32()?),
            _ => {}
          }
        }
      }
      b"mdia" => {
        while body.remaining() > 0 {
          let (kind, mut mdia) = body.child()?;
          match &kind {
            b"mdhd" => {
              let times = match mdia.full()?.0 {
                0 => 8,
                1 => 16,
                _ => return None,
              };
              mdia.skip(times)?;
              track.timescale = mdia.u32()?;
            }
            b"hdlr" => track.handler = handler(mdia)?,
            b"minf" => {
              while mdia.remaining() > 0 {
                let (kind, stbl) = mdia.child()?;
                if &kind == b"stbl" {
                  if track.table.is_some() {
                    return None;
                  }
                  track.table = Some(parse_stbl(stbl)?);
                }
              }
            }
            _ => {}
          }
        }
      }
      _ => {}
    }
  }
  tkhd.then_some(track)
}

fn parse_stbl(mut r: Reader) -> Option<SampleTable> {
  let mut table = SampleTable::default();
  let mut seen = Vec::new();
  while r.remaining() > 0 {
    let (kind, mut body) = r.child()?;
    if !matches!(
      &kind,
      b"stco" | b"co64" | b"stsc" | b"stsz" | b"stts" | b"stsd"
    ) {
      continue;
    }
    // each box at most once, and only one of 'stco' and 'co64'
    let key = if &kind == b"co64" { *b"stco" } else { kind };
    if seen.contains(&key) {
      return None;
    }
    seen.push(key);
    let version = body.full()?.0;
    match &kind {
      b"stsd" => {
        if version > 1 {
          return None;
        }
        for _ in 0..body.u32()? {
          let (kind, mut entry) = body.child()?;
          if &kind == b"av01" {
            // SampleEntry and VisualSampleEntry fields
            entry.skip(78)?;
            let properties = parse_properties(entry, true)?;
            if !properties
              .iter()
              .any(|p| matches!(p, Property::Av1C { .. }))
            {
              return None;
            }
            table.av01.get_or_insert(properties);
          }
        }
      }
      _ if version != 0 => return None,
      b"stsz" => {
        let size = body.u32()?;
        let count = body.u32()?;
        let sizes = if size == 0 {
          body.entries(count, 4)?
        } else {
          &[]
        };
        table.sample_sizes = Some((size, sizes));
      }
      _ => {
        let count = body.u32()?;
        match &kind {
          b"stco" => table.chunk_offsets = Some((body.entries(count, 4)?, 4)),
          b"co64" => table.chunk_offsets = Some((body.entries(count, 8)?, 8)),
          b"stts" => table.time_to_sample = Some(body.entries(count, 8)?),
          _ => {
            let entries = body.entries(count, 12)?;
            // first chunks start at 1 and increase; description indexes are not 0
            let mut previous = 0;
            for entry in entries.as_chunks::<12>().0 {
              let first = be32(&entry[0..4]);
              if first <= previous || (previous == 0 && first != 1) || be32(&entry[8..12]) == 0 {
                return None;
              }
              previous = first;
            }
            table.sample_to_chunk = Some(entries);
          }
        }
      }
    }
  }
  Some(table)
}

impl<'a> Track<'a> {
  fn is_video(&self) -> bool {
    matches!(&self.handler, b"pict" | b"vide" | b"auxv")
  }

  /// The properties of a video track with AV1 samples
  fn av01(&self) -> Option<&[Property<'a>]> {
    let table = self.table.as_ref()?;
    let chunks = table
      .chunk_offsets
      .is_some_and(|(offsets, _)| !offsets.is_empty());
    (self.is_video() && self.id != 0 && chunks)
      .then_some(table.av01.as_deref())
      .flatten()
  }

  /// Whether the auxi type is alpha. A track without one is assumed to be alpha.
  fn is_alpha(&self) -> bool {
    let aux = self.av01().into_iter().flatten().find_map(|p| match p {
      Property::Aux(aux) => Some(*aux),
      _ => None,
    });
    aux.is_none_or(|aux| ALPHA_URNS.contains(&aux))
  }

  fn samples(&self) -> Option<AvifSamples> {
    let config = self.av01()?.iter().find_map(|p| match p {
      Property::Av1C { config, .. } => Some(*config),
      _ => None,
    })?;
    let table = self.table.as_ref()?;
    let (offsets, width) = table.chunk_offsets?;
    let (size, sizes) = table.sample_sizes.unwrap_or_default();
    let mut sample_to_chunk = table
      .sample_to_chunk
      .unwrap_or_default()
      .as_chunks::<12>()
      .0
      .iter();
    let mut next_entry = sample_to_chunk.next();
    let mut per_chunk = 0;
    let mut samples = Vec::new();
    for (chunk, offset) in offsets.chunks_exact(width).enumerate() {
      // the last stsc entry that starts at or before this (1-based) chunk
      while let Some(entry) = next_entry.filter(|e| be32(&e[0..4]) as usize <= chunk + 1) {
        per_chunk = be32(&entry[4..8]) as usize;
        next_entry = sample_to_chunk.next();
      }
      if per_chunk == 0 || per_chunk > MAX_IMAGE_COUNT - samples.len() {
        return None;
      }
      let mut offset = match width {
        4 => be32(offset).into(),
        _ => u64::from_be_bytes(offset.try_into().ok()?),
      };
      for _ in 0..per_chunk {
        let size = match size {
          0 => be32(sizes.get(samples.len() * 4..samples.len() * 4 + 4)?),
          size => size,
        };
        if size == 0 {
          return None;
        }
        samples.push(TrackSample {
          offset,
          size,
          duration: 0,
        });
        offset = offset.checked_add(size.into())?;
      }
    }
    // the delta of the 'stts' run a sample is in; samples past the table keep the
    // last delta, and without a table it is 1
    let mut runs = table
      .time_to_sample
      .unwrap_or_default()
      .as_chunks::<8>()
      .0
      .iter();
    let (mut run_end, mut delta) = (0u64, 1);
    for (index, sample) in samples.iter_mut().enumerate() {
      while index as u64 >= run_end {
        let Some(run) = runs.next() else { break };
        run_end = run_end.saturating_add(be32(&run[0..4]).into());
        delta = be32(&run[4..8]);
      }
      sample.duration = delta;
    }
    Some(AvifSamples {
      width: self.width,
      height: self.height,
      config: config.to_vec(),
      samples: Samples::Track(samples),
    })
  }
}

/// The first colour track, its alpha track and their 'prem' link
fn track_image(tracks: &[Track]) -> Option<AvifImage> {
  let color = tracks
    .iter()
    .find(|t| t.av01().is_some() && t.aux_for.is_none())?;
  let alpha = tracks
    .iter()
    .find(|t| t.av01().is_some() && t.aux_for == Some(color.id) && t.is_alpha());
  let color_samples = color.samples()?;
  Some(AvifImage {
    alpha: match alpha {
      // Chrome rejects an alpha track with another sample count
      Some(alpha) => Some(alpha.samples().filter(|a| a.len() == color_samples.len())?),
      None => None,
    },
    color: color_samples,
    premultiplied: alpha.is_some_and(|alpha| color.prem_by == Some(alpha.id)),
    nclx: None,
    timescale: color.timescale,
  })
}

/// A cursor over `data[pos..end]`. Positions are offsets in the whole file.
#[derive(Clone, Copy)]
struct Reader<'a> {
  data: &'a [u8],
  pos: usize,
  end: usize,
}

impl<'a> Reader<'a> {
  fn remaining(&self) -> usize {
    self.end - self.pos
  }

  fn bytes(&mut self, count: usize) -> Option<&'a [u8]> {
    let end = self.pos.checked_add(count).filter(|&end| end <= self.end)?;
    let bytes = &self.data[self.pos..end];
    self.pos = end;
    Some(bytes)
  }

  fn skip(&mut self, count: usize) -> Option<()> {
    self.bytes(count).map(drop)
  }

  fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
    self.bytes(N)?.try_into().ok()
  }

  fn u8(&mut self) -> Option<u8> {
    self.array().map(u8::from_be_bytes)
  }

  fn u16(&mut self) -> Option<u16> {
    self.array().map(u16::from_be_bytes)
  }

  fn u32(&mut self) -> Option<u32> {
    self.array().map(u32::from_be_bytes)
  }

  fn u64(&mut self) -> Option<u64> {
    self.array().map(u64::from_be_bytes)
  }

  /// An unsigned integer of 0, 4 or 8 bytes
  fn uint(&mut self, size: u8) -> Option<u64> {
    match size {
      0 => Some(0),
      4 => self.u32().map(u64::from),
      _ => self.u64(),
    }
  }

  /// FullBox version and flags
  fn full(&mut self) -> Option<(u8, u32)> {
    let value = self.u32()?;
    Some(((value >> 24) as u8, value & 0x00FF_FFFF))
  }

  /// A NUL-terminated string, without the NUL
  fn c_string(&mut self) -> Option<&'a [u8]> {
    let length = self.data[self.pos..self.end].iter().position(|&b| b == 0)?;
    let string = self.bytes(length)?;
    self.skip(1)?;
    Some(string)
  }

  /// `count` table entries of `size` bytes
  fn entries(&mut self, count: u32, size: usize) -> Option<&'a [u8]> {
    self.bytes(usize::try_from(count).ok()?.checked_mul(size)?)
  }

  /// A box header: the type, and the box size from its start (0: to the end of the file)
  fn header(&mut self) -> Option<([u8; 4], u64)> {
    let size = self.u32()?;
    let kind = self.array()?;
    let size = match size {
      1 => self.u64()?,
      size => size.into(),
    };
    if &kind == b"uuid" {
      self.skip(16)?;
    }
    Some((kind, size))
  }

  /// The next child box: its type and a reader over its payload
  fn child(&mut self) -> Option<([u8; 4], Reader<'a>)> {
    let start = self.pos;
    let (kind, size) = self.header()?;
    let end = start.checked_add(usize::try_from(size).ok()?)?;
    // a size of 0 (to the end of the file) is only valid at the top level
    if end < self.pos || end > self.end {
      return None;
    }
    let body = Reader {
      pos: self.pos,
      end,
      ..*self
    };
    self.pos = end;
    Some((kind, body))
  }
}

fn be32(bytes: &[u8]) -> u32 {
  u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn push<T>(list: &mut Vec<T>, value: T) -> Option<()> {
  (list.len() < MAX_ENTRIES).then(|| list.push(value))
}

#[cfg(test)]
mod tests {
  use super::{Avif, AvifImage, AvifSamples, Nclx, Reader, Samples, parse, parse_meta};

  /// AVIF files from the Chrome 155 comparison (avifenc 1.4.2, MP4Box), and two of them
  /// rewritten. Each test says what Chrome decodes from them.
  fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
      "{}/__test__/fixtures/avif/{name}",
      env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
  }

  fn outcome(data: &[u8], prefer_animation: Option<bool>) -> &'static str {
    match parse(data, prefer_animation) {
      Avif::Image(_) => "image",
      Avif::Incomplete => "incomplete",
      Avif::Decline => "decline",
    }
  }

  fn parsed(data: &[u8], prefer_animation: Option<bool>) -> AvifImage {
    match parse(data, prefer_animation) {
      Avif::Image(image) => image,
      _ => panic!("{}", outcome(data, prefer_animation)),
    }
  }

  fn samples(data: &[u8], samples: &AvifSamples) -> Vec<Vec<u8>> {
    (0..samples.len())
      .map(|i| samples.sample(data, i).expect("sample bytes").into_owned())
      .collect()
  }

  fn bytes(data: &[u8], ranges: &[(usize, usize)]) -> Vec<Vec<u8>> {
    ranges
      .iter()
      .map(|&(offset, size)| data[offset..offset + size].to_vec())
      .collect()
  }

  fn patch(data: &[u8], offset: usize, bytes: &[u8]) -> Vec<u8> {
    let mut data = data.to_vec();
    data[offset..offset + bytes.len()].copy_from_slice(bytes);
    data
  }

  /// Every position of `needle` in `data`
  fn find(data: &[u8], needle: &[u8]) -> Vec<usize> {
    (0..=data.len() - needle.len())
      .filter(|&i| data[i..].starts_with(needle))
      .collect()
  }

  /// The start of the only box whose type (and following bytes) are `kind`
  fn box_at(data: &[u8], kind: &[u8]) -> usize {
    let found = find(data, kind);
    assert_eq!(found.len(), 1);
    found[0] - 4
  }

  fn without_alpha(data: &[u8]) -> bool {
    matches!(parse(data, None), Avif::Image(image) if image.alpha.is_none())
  }

  #[test]
  fn reads_a_still_item() {
    // Chrome: I420 64x48
    let data = fixture("s8_420.avif");
    let image = parsed(&data, None);
    let color = &image.color;
    assert_eq!((color.width, color.height), (64, 48));
    assert_eq!(color.config, [0x81, 0x00, 0x0c, 0x00]);
    assert_eq!(samples(&data, color), bytes(&data, &[(275, 854)]));
    assert_eq!(color.duration(0), 0);
    assert!(color.sample(&data, 1).is_none());
    assert!(image.alpha.is_none() && !image.premultiplied);
    let nclx = Nclx {
      primaries: 1,
      transfer: 13,
      matrix: 6,
      full_range: true,
    };
    assert_eq!(image.nclx, Some(nclx));
    assert_eq!(image.timescale, 0);
  }

  #[test]
  fn reads_alpha_items() {
    // Chrome: BGRA
    let data = fixture("a8_420.avif");
    let image = parsed(&data, None);
    let alpha = image.alpha.as_ref().expect("alpha");
    assert_eq!(samples(&data, &image.color), bytes(&data, &[(852, 854)]));
    assert_eq!(samples(&data, alpha), bytes(&data, &[(430, 422)]));
    assert_eq!(alpha.config, [0x81, 0x00, 0x1c, 0x00]);
    assert_eq!((alpha.width, alpha.height), (64, 48));
    assert!(!image.premultiplied);

    // Chrome: I420. A depth auxiliary image is not alpha.
    let depth = find(&data, b"auxiliary:alpha")[0] + 10;
    assert!(without_alpha(&patch(&data, depth, b"depth")));

    // MP4Box: every alpha sample is 255, offsets from base_offset. Chrome: BGRA.
    let data = fixture("aopitem8_420.avif");
    let image = parsed(&data, None);
    assert_eq!(samples(&data, &image.color), bytes(&data, &[(450, 854)]));
    let alpha = image.alpha.as_ref().expect("alpha");
    assert_eq!(samples(&data, alpha), bytes(&data, &[(1304, 25)]));

    // 'prem'. Chrome: BGRA, un-premultiplied.
    let data = fixture("p8_420.avif");
    let image = parsed(&data, None);
    assert!(image.premultiplied);
    assert_eq!(samples(&data, &image.color), bytes(&data, &[(866, 872)]));
    let alpha = image.alpha.as_ref().expect("alpha");
    assert_eq!(samples(&data, alpha), bytes(&data, &[(444, 422)]));
  }

  #[test]
  fn joins_item_extents() {
    // avifenc --progressive: two extents. Chrome: I420 from both.
    let data = fixture("prog8_420.avif");
    let image = parsed(&data, None);
    assert_eq!(
      samples(&data, &image.color),
      [bytes(&data, &[(299, 104), (403, 987)]).concat()]
    );

    // a8_420.avif with iloc v2 (and index fields): the colour item in 'idat' as two
    // extents in reverse order, the alpha item in mdat. Chrome: the same BGRA bytes.
    let a8 = fixture("a8_420.avif");
    let original = parsed(&a8, None);
    let data = fixture("idat8_420a.avif");
    let image = parsed(&data, None);
    assert_eq!(samples(&data, &image.color), samples(&a8, &original.color));
    assert_eq!(
      samples(&data, image.alpha.as_ref().unwrap()),
      samples(&a8, original.alpha.as_ref().unwrap())
    );
  }

  #[test]
  fn reads_tracks() {
    // Chrome: BGRX x3 at 0, 33333 and 66666 us
    let data = fixture("anim8_420.avif");
    let image = parsed(&data, None);
    let color = &image.color;
    assert_eq!(
      samples(&data, color),
      bytes(&data, &[(1001, 940), (1941, 830), (2771, 788)])
    );
    assert_eq!((color.width, color.height), (64, 48));
    assert_eq!(color.config, [0x81, 0x00, 0x0c, 0x00]);
    assert_eq!(image.timescale, 30);
    assert_eq!(
      (0..3).map(|i| color.duration(i)).collect::<Vec<_>>(),
      [1; 3]
    );
    assert!(color.sample(&data, 3).is_none());
    assert!(image.alpha.is_none() && image.nclx.is_none());

    // An alpha track and 'prem'. Chrome: BGRA x3, un-premultiplied.
    let data = fixture("panim8_420a.avif");
    let panim = parsed(&data, None);
    assert_eq!(
      samples(&data, &panim.color),
      bytes(&data, &[(2641, 1124), (3765, 962), (4727, 896)])
    );
    let alpha = panim.alpha.as_ref().expect("alpha");
    assert_eq!(
      samples(&data, alpha),
      bytes(&data, &[(1833, 586), (2419, 123), (2542, 99)])
    );
    assert!(panim.premultiplied);

    // The same samples in two chunks (2 samples, then 1) with co64 offsets, two stsc
    // entries, and colour stts runs (1, 1), (0, 9), (1, 2). Chrome: the same bytes,
    // durations 33333, 66666 and 66666 us.
    let tables = fixture("tables_panim8_420a.avif");
    let image = parsed(&tables, None);
    assert_eq!(samples(&tables, &image.color), samples(&data, &panim.color));
    assert_eq!(
      samples(&tables, image.alpha.as_ref().unwrap()),
      samples(&data, alpha)
    );
    let durations: Vec<_> = (0..3).map(|i| image.color.duration(i)).collect();
    assert_eq!((image.timescale, durations), (30, vec![1, 2, 2]));
    assert!(image.premultiplied);

    // tref 'prem' renamed. Chrome: BGRA, other bytes.
    let prem = box_at(&data, b"prem\0\0\0\x02") + 4;
    let image = parsed(&patch(&data, prem, b"xxxx"), None);
    assert!(image.alpha.is_some() && !image.premultiplied);
    // auxi type depth. Chrome: BGRX.
    let depth = find(&data, b"auxiliary:alpha")[1] + 10;
    assert!(without_alpha(&patch(&data, depth, b"depth")));
    // no auxi box: alpha. Chrome: the same BGRA bytes.
    let auxi = box_at(&data, b"auxi\0\0\0\0urn") + 4;
    let image = parsed(&patch(&data, auxi, b"xxxx"), None);
    assert_eq!(
      samples(&data, image.alpha.as_ref().unwrap()),
      samples(&data, alpha)
    );

    // 2 alpha samples for 3 colour samples. Chrome rejects it.
    let per_chunk = find(&data, b"stsc")[1] + 16;
    let alpha2 = patch(&data, per_chunk, &2u32.to_be_bytes());
    assert_eq!(outcome(&alpha2, None), "decline");

    // MP4Box: one frame, 'stsz' with one size for all samples. Chrome: I420.
    let data = fixture("avis1_8_420.avif");
    let image = parsed(&data, None);
    assert_eq!(samples(&data, &image.color), bytes(&data, &[(962, 938)]));
    assert_eq!((image.timescale, image.color.duration(0)), (15360, 512));
  }

  #[test]
  fn chooses_the_source_like_chrome() {
    // (file, preferAnimation, Chrome frame count or 0 when Chrome rejects the file)
    let anim = fixture("anim8_420.avif");
    let a8 = fixture("a8_420.avif");
    let s8 = fixture("s8_420.avif");
    let cases = [
      // major brand 'avis'
      (anim.clone(), None, 3),
      (anim.clone(), Some(false), 1),
      (anim.clone(), Some(true), 3),
      // major brand 'avif', 'avis' compatible
      (patch(&anim, 8, b"avif"), None, 1),
      (patch(&anim, 8, b"avif"), Some(true), 3),
      // major brand 'mif1', 'avif' and 'avis' compatible
      (patch(&anim, 8, b"mif1"), None, 3),
      (patch(&anim, 8, b"mif1"), Some(false), 1),
      // major brand 'mif1', 'avif' compatible, no moov
      (patch(&a8, 8, b"mif1"), None, 1),
      (patch(&a8, 8, b"mif1"), Some(true), 0),
      (s8.clone(), Some(true), 0),
    ];
    for (i, (data, prefer_animation, frames)) in cases.iter().enumerate() {
      match parse(data, *prefer_animation) {
        Avif::Image(image) => assert_eq!(image.color.len(), *frames, "case {i}"),
        _ => assert_eq!(*frames, 0, "case {i}"),
      }
    }
    // the primary item of an 'avis' file: the first track sample here, with alpha
    let item = parsed(&anim, Some(false));
    assert_eq!(samples(&anim, &item.color), bytes(&anim, &[(1001, 940)]));
    let panim = fixture("panim8_420a.avif");
    let item = parsed(&panim, Some(false));
    assert_eq!(item.timescale, 0);
    assert!(item.alpha.is_some() && item.premultiplied);

    // Chrome rejects these. 'avis' without 'moov' waits for it; a file without the
    // 'avif' and 'avis' brands, or that needs 'mini', is declined.
    for prefer_animation in [None, Some(false), Some(true)] {
      assert_eq!(
        outcome(&patch(&s8, 8, b"avis"), prefer_animation),
        "incomplete"
      );
      assert_eq!(
        outcome(&patch(&s8, 20, b"avis"), prefer_animation),
        "incomplete"
      );
      let no_brand = patch(&patch(&s8, 8, b"mif1"), 16, b"mif1");
      assert_eq!(outcome(&no_brand, prefer_animation), "decline");
      assert_eq!(
        outcome(&patch(&s8, 8, b"mif3avif"), prefer_animation),
        "decline"
      );
    }
  }

  #[test]
  fn declines_grids() {
    // Chrome: I420 128x128 from four tiles
    let data = fixture("grid8_420.avif");
    for prefer_animation in [None, Some(false), Some(true)] {
      assert_eq!(outcome(&data, prefer_animation), "decline");
    }
  }

  /// End of the bytes of sample `index`
  fn sample_end(samples: &AvifSamples, index: usize) -> u64 {
    match &samples.samples {
      Samples::Item(extents) => extents.iter().map(|e| e.end).max().unwrap(),
      Samples::Track(samples) => samples[index].offset + u64::from(samples[index].size),
    }
  }

  #[test]
  fn waits_for_the_boxes_the_brands_need() {
    for name in [
      "s8_420.avif",
      "a8_420.avif",
      "aopitem8_420.avif",
      "p8_420.avif",
      "prog8_420.avif",
      "idat8_420a.avif",
      "anim8_420.avif",
      "panim8_420a.avif",
      "tables_panim8_420a.avif",
      "avis1_8_420.avif",
      "grid8_420.avif",
    ] {
      let data = fixture(name);
      // the boxes the brands need ('meta', also 'moov' for 'avis') come before 'mdat'
      let needed = box_at(&data, b"mdat");
      let full = parse(&data, None);
      for length in 0..=data.len() {
        let prefix = &data[..length];
        match (parse(prefix, None), &full) {
          (Avif::Incomplete, _) => assert!(length < needed, "{name} {length}"),
          (Avif::Decline, Avif::Decline) => assert!(length >= needed, "{name} {length}"),
          (Avif::Image(image), Avif::Image(full)) => {
            assert!(length >= needed, "{name} {length}");
            assert_eq!(image.color.len(), full.color.len());
            assert_eq!(image.alpha.is_some(), full.alpha.is_some());
            let both = [(&image.color, &full.color)]
              .into_iter()
              .chain(image.alpha.as_ref().zip(full.alpha.as_ref()));
            for (part, full_part) in both {
              for i in 0..part.len() {
                let arrived = sample_end(part, i) <= length as u64;
                assert_eq!(part.sample(prefix, i).is_some(), arrived, "{name} {length}");
                if arrived {
                  assert_eq!(part.sample(prefix, i), full_part.sample(&data, i));
                }
              }
            }
          }
          _ => panic!("{name} {length}: {}", outcome(prefix, None)),
        }
      }
    }
  }

  #[test]
  fn limits_counts_and_box_sizes() {
    let data = fixture("avis1_8_420.avif");
    // 3 samples of the one 'stsz' size, the last two past the end of the file: they have
    // no bytes (Chrome rejects the complete file)
    let per_chunk = box_at(&data, b"stsc") + 20;
    let image = parsed(&patch(&data, per_chunk, &3u32.to_be_bytes()), None);
    assert_eq!(image.color.len(), 3);
    assert!(image.color.sample(&data, 1).is_none());
    // more samples than CrabbyAvif allows
    assert_eq!(
      outcome(&patch(&data, per_chunk, &[0xFF; 4]), None),
      "decline"
    );
    // more chunk offsets than the box holds
    let count = box_at(&data, b"stco") + 12;
    assert_eq!(outcome(&patch(&data, count, &[0xFF; 4]), None), "decline");

    let s8 = fixture("s8_420.avif");
    // 'meta' smaller than its header, a child bigger than 'meta', a 64-bit size overflow
    assert_eq!(
      outcome(&patch(&s8, 32, &4u32.to_be_bytes()), None),
      "decline"
    );
    let iinf = box_at(&s8, b"iinf");
    assert_eq!(
      outcome(&patch(&s8, iinf, &[0, 0, 0xFF, 0xFF]), None),
      "decline"
    );
    let mut huge = s8[..32].to_vec();
    huge.extend_from_slice(b"\0\0\0\x01meta\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF");
    assert_eq!(outcome(&huge, None), "decline");

    // 'iloc' extents of 0 bytes each: 65535 of them for one item, more for two
    let boxed = |kind: &[u8], payload: &[u8]| {
      let mut b = (payload.len() as u32 + 8).to_be_bytes().to_vec();
      b.extend_from_slice(kind);
      b.extend_from_slice(payload);
      b
    };
    let meta = |items: u16| {
      let mut iloc = vec![0; 6];
      iloc.extend_from_slice(&items.to_be_bytes());
      for item in 1..=items {
        iloc.extend_from_slice(&item.to_be_bytes());
        iloc.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
      }
      let mut meta = vec![0; 4];
      meta.extend(boxed(
        b"hdlr",
        b"\0\0\0\0\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0",
      ));
      meta.extend(boxed(b"iloc", &iloc));
      meta
    };
    let read = |data: &[u8]| {
      parse_meta(Reader {
        data,
        pos: 0,
        end: data.len(),
      })
      .is_some()
    };
    assert!(read(&meta(1)));
    assert!(!read(&meta(2)));
  }

  #[test]
  fn survives_mutated_and_random_input() {
    // xorshift64
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
      state ^= state << 13;
      state ^= state >> 7;
      state ^= state << 17;
      state
    };
    let exercise = |data: &[u8]| {
      for prefer_animation in [None, Some(false), Some(true)] {
        if let Avif::Image(image) = parse(data, prefer_animation) {
          for part in [Some(&image.color), image.alpha.as_ref()]
            .into_iter()
            .flatten()
          {
            for i in 0..part.len().min(4) {
              let _ = (part.sample(data, i), part.duration(i));
            }
            assert!(part.sample(data, part.len()).is_none());
          }
        }
      }
    };
    for name in [
      "a8_420.avif",
      "idat8_420a.avif",
      "prog8_420.avif",
      "tables_panim8_420a.avif",
      "avis1_8_420.avif",
      "grid8_420.avif",
    ] {
      let data = fixture(name);
      let boxes = box_at(&data, b"mdat");
      for _ in 0..500 {
        let mut mutated = data.clone();
        for _ in 0..=next() % 4 {
          // mostly in the boxes before mdat
          let range = if next() % 4 == 0 { data.len() } else { boxes };
          mutated[next() as usize % range] = next() as u8;
        }
        exercise(&mutated);
      }
    }
    let ftyp = &fixture("s8_420.avif")[..32];
    for _ in 0..3000 {
      let mut data = if next() % 2 == 0 {
        ftyp.to_vec()
      } else {
        Vec::new()
      };
      data.extend((0..next() % 300).map(|_| next() as u8));
      exercise(&data);
    }
  }
}
