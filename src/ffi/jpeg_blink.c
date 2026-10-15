/*
 * JPEG decoding as Chrome 155 does it: Blink's JPEGImageDecoder
 * (third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_image_decoder.cc) on the
 * libjpeg-turbo that Chrome ships (vendor/libjpeg-turbo). The same library with the same
 * settings gives Chrome's bytes: YUV planes for 4:4:4, 4:2:2 and 4:2:0 YCbCr at full size,
 * BGRA (alpha 255) for everything else.
 *
 * All the data is here at once. Where Blink would wait for more data (libjpeg suspends), the
 * decode fails, as Chrome's does once it has all the data: a truncated file, or a file without
 * its EOI marker, is an error. Only the first image of concatenated JPEGs (MPO, gain map) is
 * read. libjpeg reports errors with longjmp: every setjmp and longjmp stays in this file.
 *
 * Chrome gives other bytes for:
 * - an ICC profile that Chrome applies at decode time with skcms (BGRX): a CMYK profile on a
 *   CMYK or YCCK file, or an RGB or gray profile that Skia cannot hold exactly (a table TRC).
 *   ICC profiles are ignored here. Other RGB profiles change only Chrome's color tags, which
 *   are always sRGB here.
 * - corrupt data whose coefficients overflow the IDCT: Chrome builds libjpeg-turbo with SIMD
 *   (NEON, SSE2, AVX2), this build is C only, and the two round the overflow differently.
 *
 * This file is a C rewrite of the decode parts of Blink's jpeg_image_decoder.cc at Chrome
 * 155.0.8059.79, changed in 2026 to decode from one buffer into an AVFrame. That file's notice
 * follows; it applies to this file.
 *
 * Copyright (C) 2006 Apple Computer, Inc.
 *
 * Portions are Copyright (C) 2001-6 mozilla.org
 *
 * Other contributors:
 *   Stuart Parmenter <stuart@mozilla.com>
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA
 *
 * Alternatively, the contents of this file may be used under the terms
 * of either the Mozilla Public License Version 1.1, found at
 * http://www.mozilla.org/MPL/ (the "MPL") or the GNU General Public
 * License Version 2.0, found at http://www.fsf.org/copyleft/gpl.html
 * (the "GPL"), in which case the provisions of the MPL or the GPL are
 * applicable instead of those above.  If you wish to allow use of your
 * version of this file only under the terms of one of those two
 * licenses (the MPL or the GPL) and not to allow others to use your
 * version of this file under the LGPL, indicate your decision by
 * deletingthe provisions above and replace them with the notice and
 * other provisions required by the MPL or the GPL, as the case may be.
 * If you do not delete the provisions above, a recipient may use your
 * version of this file under any of the LGPL, the MPL or the GPL.
 */

#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <libavutil/error.h>
#include <libavutil/frame.h>
#include <libavutil/log.h>
#include <libavutil/pixfmt.h>

#include "jpeglib.h"

typedef struct {
  struct jpeg_error_mgr pub;
  int num_corrupt_warnings; // Blink stops reading a multi-scan file at a corrupt-data warning
  jmp_buf jump;
} ErrorMgr;

typedef struct {
  struct jpeg_decompress_struct info;
  ErrorMgr err;
  struct jpeg_source_mgr src;
  struct jpeg_progress_mgr progress;
  int scale_num; // JPEGImageDecoder::DesiredScaleNumerator, 1-8 (8: full size)
  int allow_yuv; // 0: BGRA even where YUV planes would do (see blink_jpeg_info)
  int yuv;       // decode to YUV planes (raw data out)
  AVFrame *dst;  // NULL: read the header only
} Decoder;

static void error_exit(j_common_ptr cinfo) {
  char message[JMSG_LENGTH_MAX];
  cinfo->err->format_message(cinfo, message);
  av_log(NULL, AV_LOG_ERROR, "libjpeg-turbo: %s\n", message);
  longjmp(((ErrorMgr *)cinfo->err)->jump, 1);
}

// libjpeg prints warnings to stderr by default.
static void output_message(j_common_ptr cinfo) {
  char message[JMSG_LENGTH_MAX];
  cinfo->err->format_message(cinfo, message);
  av_log(NULL, AV_LOG_WARNING, "libjpeg-turbo: %s\n", message);
}

// Blink's emit_message for multi-scan files: count "Corrupt JPEG" warnings.
static void emit_message(j_common_ptr cinfo, int msg_level) {
  ErrorMgr *err = (ErrorMgr *)cinfo->err;
  if (msg_level >= 0) return;
  err->pub.num_warnings++;
  int code = err->pub.msg_code;
  const char *warning =
      code > 0 && code <= err->pub.last_jpeg_message ? err->pub.jpeg_message_table[code] : NULL;
  if (warning && !strncmp("Corrupt JPEG", warning, 12)) err->num_corrupt_warnings++;
}

// Blink's ProgressMonitor: a file with very many scans can make the decoder hang.
static void progress_monitor(j_common_ptr cinfo) {
  if (((j_decompress_ptr)cinfo)->input_scan_number >= 100) {
    av_log(NULL, AV_LOG_ERROR, "libjpeg-turbo: 100 or more scans\n");
    longjmp(((ErrorMgr *)cinfo->err)->jump, 1);
  }
}

static void init_source(j_decompress_ptr info) { (void)info; }

// All the data is in the buffer: suspend when libjpeg wants more.
static boolean fill_input_buffer(j_decompress_ptr info) {
  (void)info;
  return FALSE;
}

static void skip_input_data(j_decompress_ptr info, long num_bytes) {
  struct jpeg_source_mgr *src = info->src;
  if (num_bytes <= 0) return;
  size_t skip = (unsigned long)num_bytes < src->bytes_in_buffer ? (size_t)num_bytes : src->bytes_in_buffer;
  src->next_input_byte += skip;
  src->bytes_in_buffer -= skip;
}

static void term_source(j_decompress_ptr info) { (void)info; }

// JPEGImageReader::Decode, state kJpegHeader: the output color space, the size check, the
// scale and the YUV choice (JPEGImageDecoder::OnSetData).
static int read_header(Decoder *d) {
  j_decompress_ptr info = &d->info;
  jpeg_create_decompress(info);
  info->src = &d->src;
  info->progress = &d->progress;
  // Blink keeps APP1 (Exif, XMP), APP2 (ICC, MPF) and APP11 (JUMBF) segments.
  jpeg_save_markers(info, JPEG_APP0 + 1, 0xFFFF);
  jpeg_save_markers(info, JPEG_APP0 + 2, 0xFFFF);
  jpeg_save_markers(info, JPEG_APP0 + 11, 0xFFFF);
  if (jpeg_read_header(info, TRUE) != JPEG_HEADER_OK) return AVERROR_INVALIDDATA;

  switch (info->jpeg_color_space) {
  case JCS_YCbCr:
  case JCS_GRAYSCALE:
  case JCS_RGB:
    info->out_color_space = JCS_EXT_BGRA;
    break;
  case JCS_CMYK:
  case JCS_YCCK:
    info->out_color_space = JCS_CMYK; // converted to BGRA by output_rows()
    break;
  default:
    return AVERROR_INVALIDDATA;
  }
  // ImageDecoder::SetSize: width * height * 4 bytes must fit in an int32.
  if ((uint64_t)info->image_width * info->image_height * 4 > INT32_MAX) return AVERROR_INVALIDDATA;

  info->scale_num = d->scale_num;
  info->scale_denom = 8;
  jpeg_calc_output_dimensions(info);
  info->buffered_image = jpeg_has_multiple_scans(info);
  if (info->buffered_image) {
    d->err.pub.emit_message = emit_message;
    d->err.num_corrupt_warnings = 0;
  }

  // YUV planes only at full size and for YuvSubsampling() 4:4:4, 4:2:2 and 4:2:0. Blink also
  // needs no decode-time color transform: an ICC profile is not looked at here.
  const jpeg_component_info *c = info->comp_info;
  int h = c[0].h_samp_factor, v = c[0].v_samp_factor;
  d->yuv = d->allow_yuv && d->scale_num == 8 && info->jpeg_color_space == JCS_YCbCr &&
           info->num_components == 3 && c[1].h_samp_factor == 1 && c[1].v_samp_factor == 1 &&
           c[2].h_samp_factor == 1 && c[2].v_samp_factor == 1 &&
           ((v == 1 && (h == 1 || h == 2)) || (v == 2 && h == 2));
  return 0;
}

static int output_format(const Decoder *d) {
  if (!d->yuv) return AV_PIX_FMT_BGRA;
  int h = d->info.comp_info[0].h_samp_factor, v = d->info.comp_info[0].v_samp_factor;
  return h == 1 ? AV_PIX_FMT_YUV444P : v == 1 ? AV_PIX_FMT_YUV422P : AV_PIX_FMT_YUV420P;
}

// JPEGImageDecoder::OutputScanlines for the current output pass. Fails when libjpeg suspends.
static int output_rows(Decoder *d, JSAMPROW dummy) {
  j_decompress_ptr info = &d->info;
  AVFrame *dst = d->dst;
  if (d->yuv) {
    // OutputRawData. jpeg_read_raw_data writes whole iMCU rows: rows below the planes go
    // to a dummy row as wide as the Y plane's blocks.
    JSAMPROW rows[32];
    JSAMPARRAY planes[3] = {rows, rows + 16, rows + 24};
    int v = info->comp_info[0].v_samp_factor;
    JDIMENSION uv_height = info->comp_info[1].downsampled_height;
    while (info->output_scanline < info->output_height) {
      for (int i = 0; i < DCTSIZE * v; i++) {
        JDIMENSION y = info->output_scanline + i;
        rows[i] = y < info->output_height ? dst->data[0] + (size_t)y * dst->linesize[0] : dummy;
      }
      for (int i = 0; i < DCTSIZE; i++) {
        JDIMENSION y = info->output_scanline / v + i;
        rows[16 + i] = y < uv_height ? dst->data[1] + (size_t)y * dst->linesize[1] : dummy;
        rows[24 + i] = y < uv_height ? dst->data[2] + (size_t)y * dst->linesize[2] : dummy;
      }
      if (!jpeg_read_raw_data(info, planes, DCTSIZE * v)) return AVERROR_INVALIDDATA;
    }
    if (info->output_scanline > info->output_height) info->output_scanline = info->output_height;
    return 0;
  }
  while (info->output_scanline < info->output_height) {
    JSAMPROW row = dst->data[0] + (size_t)info->output_scanline * dst->linesize[0];
    if (jpeg_read_scanlines(info, &row, 1) != 1) return AVERROR_INVALIDDATA;
    if (info->out_color_space != JCS_CMYK) continue;
    // SetPixel<JCS_CMYK>: Adobe's inverted CMYK, R = C * K / 255 (truncated), in place.
    for (JDIMENSION x = 0; x < info->output_width; x++, row += 4) {
      unsigned c = row[0], m = row[1], y = row[2], k = row[3];
      row[0] = (JSAMPLE)(y * k / 255);
      row[1] = (JSAMPLE)(m * k / 255);
      row[2] = (JSAMPLE)(c * k / 255);
      row[3] = 255;
    }
  }
  return 0;
}

// 1 when Blink's all_components_seen() holds.
static int all_components_seen(j_decompress_ptr info) {
  if (!info->coef_bits) return jpeg_input_complete(info);
  for (int c = 0; c < info->num_components; c++)
    if (info->coef_bits[c][0] == -1) return 0;
  return 1;
}

// JPEGImageReader::Decode from kJpegStartDecompress on, into d->dst.
static int decode(Decoder *d) {
  j_decompress_ptr info = &d->info;
  AVFrame *dst = d->dst;
  const jpeg_component_info *c = info->comp_info;
  if (dst->format != output_format(d) || dst->width != (int)info->output_width ||
      dst->height != (int)info->output_height)
    return AVERROR(EINVAL);
  // libjpeg writes width_in_blocks * DCTSIZE samples to each YUV row.
  for (int p = 0; p < (d->yuv ? 3 : 1); p++) {
    size_t row = d->yuv ? (size_t)c[p].width_in_blocks * DCTSIZE : (size_t)info->output_width * 4;
    if (!dst->data[p] || dst->linesize[p] < 0 || (size_t)dst->linesize[p] < row) return AVERROR(EINVAL);
  }

  if (d->yuv) {
    info->out_color_space = JCS_YCbCr;
    info->raw_data_out = TRUE;
  }
  info->dct_method = JDCT_ISLOW;
  info->dither_mode = JDITHER_FS;
  info->do_fancy_upsampling = TRUE;
  info->do_block_smoothing = TRUE;
  info->enable_2pass_quant = FALSE;
  info->enable_external_quant = FALSE;
  info->enable_1pass_quant = FALSE;
  info->quantize_colors = FALSE;
  info->colormap = NULL;
  JSAMPROW dummy = NULL;
  if (d->yuv)
    dummy = info->mem->alloc_sarray((j_common_ptr)info, JPOOL_IMAGE, c[0].width_in_blocks * DCTSIZE, 1)[0];
  if (!jpeg_start_decompress(info)) return AVERROR_INVALIDDATA;

  int ret;
  if (!info->buffered_image) {
    if ((ret = output_rows(d, dummy)) < 0) return ret;
  } else {
    // kJpegDecompressBufferedImage: read the scans (stopping at a corrupt-data warning), then
    // output scans until the last one is out.
    int status = 0;
    int first_scan = all_components_seen(info) ? info->input_scan_number : 0;
    do {
      if (d->err.num_corrupt_warnings) break;
      status = jpeg_consume_input(info);
      if ((status == JPEG_REACHED_SOS || status == JPEG_REACHED_EOI || status == JPEG_SUSPENDED) &&
          !first_scan && all_components_seen(info))
        first_scan = info->input_scan_number;
    } while (status != JPEG_SUSPENDED && status != JPEG_REACHED_EOI);
    if (!first_scan) return AVERROR_INVALIDDATA;
    for (;;) {
      int scan = info->input_scan_number;
      // Nothing out yet: show the last complete scan rather than one still arriving.
      if (!info->output_scan_number && scan > first_scan && status != JPEG_REACHED_EOI) --scan;
      if (!jpeg_start_output(info, scan)) return AVERROR_INVALIDDATA;
      if ((ret = output_rows(d, dummy)) < 0) return ret;
      if (!jpeg_finish_output(info)) return AVERROR_INVALIDDATA;
      if (jpeg_input_complete(info) && info->input_scan_number == info->output_scan_number) break;
    }
  }
  // kJpegDone. Without the EOI marker this suspends, and Blink fails.
  if (!jpeg_finish_decompress(info)) return AVERROR_INVALIDDATA;

  // Chrome's tags for a JPEG without an ICC profile: sRGB, and for YUV the full-range BT.601
  // matrix (image_decoder_core.cc YUVColorSpaceToGfxColorSpace for kJPEG_Full).
  dst->color_range = AVCOL_RANGE_JPEG;
  dst->color_primaries = AVCOL_PRI_BT709;
  dst->color_trc = AVCOL_TRC_IEC61966_2_1;
  if (d->yuv) {
    dst->colorspace = AVCOL_SPC_SMPTE170M;
    dst->chroma_location = AVCHROMA_LOC_CENTER;
  } else {
    dst->colorspace = AVCOL_SPC_RGB;
  }
  return 0;
}

// The setjmp for every libjpeg call: a libjpeg error returns here.
static int run(Decoder *d) {
  if (setjmp(d->err.jump)) return AVERROR_INVALIDDATA;
  int ret = read_header(d);
  return ret < 0 || !d->dst ? ret : decode(d);
}

static int run_decoder(const uint8_t *data, size_t size, int scale_num, int allow_yuv, AVFrame *dst,
                       int *out) {
  Decoder d;
  if (!data || scale_num < 1 || scale_num > 8) return AVERROR(EINVAL);
  memset(&d, 0, sizeof(d));
  d.info.err = jpeg_std_error(&d.err.pub);
  d.err.pub.error_exit = error_exit;
  d.err.pub.output_message = output_message;
  d.src.next_input_byte = data;
  d.src.bytes_in_buffer = size;
  d.src.init_source = init_source;
  d.src.fill_input_buffer = fill_input_buffer;
  d.src.skip_input_data = skip_input_data;
  d.src.resync_to_restart = jpeg_resync_to_restart;
  d.src.term_source = term_source;
  d.progress.progress_monitor = progress_monitor;
  d.scale_num = scale_num;
  d.allow_yuv = allow_yuv;
  d.dst = dst;
  int ret = run(&d);
  if (ret >= 0 && out) {
    out[0] = (int)d.info.image_width;
    out[1] = (int)d.info.image_height;
    out[2] = (int)d.info.output_width;
    out[3] = (int)d.info.output_height;
    out[4] = output_format(&d);
  }
  jpeg_destroy_decompress(&d.info);
  return ret;
}

// Read the header for a decode at scale_num/8 (1-8): out = {image width, image height, output
// width, output height, AVPixelFormat (yuv420p, yuv422p, yuv444p or bgra)}. allow_yuv = 0 gives
// bgra for every image: Chrome's WebCodecs ImageDecoder keeps RGB once it has decoded rows before
// all the data came (image_decoder_core.cc have_completed_rgb_decode_; Blink's
// allow_decode_to_yuv_ needs all the data).
int blink_jpeg_info(const uint8_t *data, size_t size, int scale_num, int allow_yuv, int *out) {
  if (!out) return AVERROR(EINVAL);
  return run_decoder(data, size, scale_num, allow_yuv, NULL, out);
}

// Decode at scale_num/8 into dst, allocated with the format and output size blink_jpeg_info
// gives for the same scale_num and allow_yuv. On failure dst holds partial rows.
int blink_jpeg_decode(const uint8_t *data, size_t size, int scale_num, int allow_yuv, AVFrame *dst) {
  if (!dst) return AVERROR(EINVAL);
  return run_decoder(data, size, scale_num, allow_yuv, dst, NULL);
}
