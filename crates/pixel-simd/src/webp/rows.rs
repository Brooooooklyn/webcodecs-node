// libwebp's notice, for the code this file ports from it (the files named below):
// Copyright 2011 Google Inc. All Rights Reserved.
// Use of this source code is governed by a BSD-style license that can be found in
// licenses/libwebp-COPYING.txt. An additional intellectual property rights grant can be found
// in licenses/libwebp-PATENTS.txt.

//! libwebp's row pairs, which the SIMD paths convert a block of pixels at a time.

use super::{Yuva420, bgra, upsample};

/// The rows of one pair (libwebp's WebPUpsampleLinePairFunc, with alpha): the top row takes its
/// chroma from rows `top_*` (nearest) and `cur_*`, the bottom row the other way round. Luma and
/// alpha rows hold the pixels, chroma rows a sample for every 2 of them.
pub(super) struct Pair<'a> {
  pub top_y: &'a [u8],
  pub bot_y: Option<&'a [u8]>,
  pub top_u: &'a [u8],
  pub top_v: &'a [u8],
  pub cur_u: &'a [u8],
  pub cur_v: &'a [u8],
  pub top_a: Option<&'a [u8]>,
  pub bot_a: Option<&'a [u8]>,
}

/// Runs `pair` on a frame's row pairs like libwebp's EmitFancyRGB (io_dec.c): row 0 alone, with
/// chroma row 0 on both sides; then rows (1, 2), (3, 4), ... between chroma rows k and k + 1; then
/// the last row of an even height alone, with the last chroma row on both sides.
pub(super) fn fancy(
  src: &Yuva420,
  dst: &mut [u8],
  stride: usize,
  mut pair: impl FnMut(&Pair, &mut [u8], Option<&mut [u8]>),
) {
  let (w, h) = (src.width, src.height);
  if w == 0 || h == 0 {
    return;
  }
  let cw = w.div_ceil(2);
  let y = |r| &src.y.row(r)[..w];
  let [u, v] = [src.u, src.v].map(|plane| move |k| &plane.row(k)[..cw]);
  let a = |r| src.a.map(|a| &a.row(r)[..w]);
  let single = |r, k| Pair {
    top_y: y(r),
    bot_y: None,
    top_u: u(k),
    top_v: v(k),
    cur_u: u(k),
    cur_v: v(k),
    top_a: a(r),
    bot_a: None,
  };
  pair(&single(0, 0), &mut dst[..4 * w], None);
  for k in 0..(h - 1) / 2 {
    let (r, s) = (2 * k + 1, 2 * k + 2);
    let p = Pair {
      top_y: y(r),
      bot_y: Some(y(s)),
      top_u: u(k),
      top_v: v(k),
      cur_u: u(k + 1),
      cur_v: v(k + 1),
      top_a: a(r),
      bot_a: a(s),
    };
    let (top, bottom) = dst.split_at_mut(s * stride);
    pair(
      &p,
      &mut top[r * stride..][..4 * w],
      Some(&mut bottom[..4 * w]),
    );
  }
  if h % 2 == 0 {
    let r = h - 1;
    pair(&single(r, r / 2), &mut dst[r * stride..][..4 * w], None);
  }
}

impl<'a> Pair<'a> {
  /// The rows from pixel `pos` on (odd: chroma from sample pos / 2).
  fn from(&self, pos: usize) -> Pair<'a> {
    let uv = pos / 2;
    Pair {
      top_y: &self.top_y[pos..],
      bot_y: self.bot_y.map(|y| &y[pos..]),
      top_u: &self.top_u[uv..],
      top_v: &self.top_v[uv..],
      cur_u: &self.cur_u[uv..],
      cur_v: &self.cur_v[uv..],
      top_a: self.top_a.map(|a| &a[pos..]),
      bot_a: self.bot_a.map(|a| &a[pos..]),
    }
  }
}

/// A row pair the way libwebp's SIMD code does it (upsampling_sse2.c SSE2_UPSAMPLE_FUNC): the
/// first pixel alone, then `block` on `n` pixels at a time while n / 2 + 1 chroma samples are
/// left, then on a copy of the rest padded with the last chroma sample (UPSAMPLE_LAST_BLOCK).
/// `block` converts the first `n` pixels of the rows it is given (n <= 64).
#[inline(always)]
pub(super) fn row_pair(
  p: &Pair,
  top: &mut [u8],
  mut bottom: Option<&mut [u8]>,
  n: usize,
  mut block: impl FnMut(&Pair, &mut [u8], Option<&mut [u8]>),
) {
  let len = p.top_y.len();
  first_pixels(p, top, bottom.as_deref_mut());
  let mut pos = 1;
  while pos + n < len {
    let bottom = bottom.as_deref_mut().map(|b| &mut b[4 * pos..]);
    block(&p.from(pos), &mut top[4 * pos..], bottom);
    pos += n;
  }
  if pos >= len {
    return;
  }
  let left = len - pos;
  let pad = |row: &[u8]| {
    let mut padded = [0; 64];
    padded[..left].copy_from_slice(&row[pos..]);
    padded
  };
  let pad_uv = |row: &[u8]| {
    let row = &row[pos / 2..len.div_ceil(2)];
    let mut padded = [row[row.len() - 1]; 33];
    padded[..row.len()].copy_from_slice(row);
    padded
  };
  let (top_y, bot_y) = (pad(p.top_y), p.bot_y.map(pad));
  let (top_a, bot_a) = (p.top_a.map(pad), p.bot_a.map(pad));
  let [top_u, top_v, cur_u, cur_v] = [p.top_u, p.top_v, p.cur_u, p.cur_v].map(pad_uv);
  let padded = Pair {
    top_y: &top_y,
    bot_y: bot_y.as_ref().map(|y| &y[..]),
    top_u: &top_u,
    top_v: &top_v,
    cur_u: &cur_u,
    cur_v: &cur_v,
    top_a: top_a.as_ref().map(|a| &a[..]),
    bot_a: bot_a.as_ref().map(|a| &a[..]),
  };
  let (mut top_out, mut bottom_out) = ([0; 256], [0; 256]);
  block(
    &padded,
    &mut top_out,
    bottom.is_some().then_some(&mut bottom_out),
  );
  top[4 * pos..4 * len].copy_from_slice(&top_out[..4 * left]);
  if let Some(bottom) = bottom {
    bottom[4 * pos..4 * len].copy_from_slice(&bottom_out[..4 * left]);
  }
}

/// The first pixel of each row: no chroma column on its left, so the nearest column of the two
/// chroma rows mixed 3:1, as the scalar code does at an edge.
fn first_pixels(p: &Pair, top: &mut [u8], bottom: Option<&mut [u8]>) {
  let edge = |near: &[u8], far: &[u8]| upsample([near, far], (0, 0));
  let (u, v) = (edge(p.top_u, p.cur_u), edge(p.top_v, p.cur_v));
  top[..4].copy_from_slice(&bgra(p.top_y[0], u, v, p.top_a.map_or(255, |a| a[0])));
  if let (Some(y), Some(bottom)) = (p.bot_y, bottom) {
    let (u, v) = (edge(p.cur_u, p.top_u), edge(p.cur_v, p.top_v));
    bottom[..4].copy_from_slice(&bgra(y[0], u, v, p.bot_a.map_or(255, |a| a[0])));
  }
}
