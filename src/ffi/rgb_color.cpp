#include "skcms.h"

extern "C" {
#include <libavutil/csp.h>
#include <libavutil/error.h>
#include <libavutil/frame.h>
#include <libavutil/mem.h>
}

#include <cstdint>

static bool rgb_profile(AVColorPrimaries primaries,
                        AVColorTransferCharacteristic transfer,
                        skcms_ICCProfile* profile) {
  *profile = *skcms_sRGB_profile();
  if (primaries == AVCOL_PRI_SMPTE432) {
    // skia's named display p3 matrix retains its published float coefficients.
    profile->toXYZD50 = {{{.515102f, .291965f, .157153f},
                          {.241182f, .692236f, .0665819f},
                          {-.00104941f, .0418818f, .784378f}}};
  } else if (primaries != AVCOL_PRI_BT709) {
    const auto* desc = av_csp_primaries_desc_from_id(primaries);
    if (!desc || !skcms_PrimariesToXYZD50(
        float(av_q2d(desc->prim.r.x)), float(av_q2d(desc->prim.r.y)),
        float(av_q2d(desc->prim.g.x)), float(av_q2d(desc->prim.g.y)),
        float(av_q2d(desc->prim.b.x)), float(av_q2d(desc->prim.b.y)),
        float(av_q2d(desc->wp.x)), float(av_q2d(desc->wp.y)),
        &profile->toXYZD50)) return false;
  }
  skcms_TransferFunction curve;
  switch (transfer) {
    case AVCOL_TRC_IEC61966_2_1:
      curve = *skcms_sRGB_TransferFunction();
      break;
    case AVCOL_TRC_LINEAR:
      curve = {1, 1, 0, 1, 0, 0, 0};
      break;
    default:
      return false;
  }
  skcms_SetTransferFunction(profile, &curve);
  return true;
}

extern "C" int ffframe_transform_rgb_color(const AVFrame* src, AVFrame* dst) {
  // chromium's CompatRGBColorSpace maps legacy sdr encodings to srgb.
  const auto compatible_primaries = [](AVColorPrimaries value) {
    return value == AVCOL_PRI_SMPTE170M ? AVCOL_PRI_BT709 : value;
  };
  const auto compatible_transfer = [](AVColorTransferCharacteristic value) {
    return value == AVCOL_TRC_BT709 || value == AVCOL_TRC_SMPTE170M
        ? AVCOL_TRC_IEC61966_2_1 : value;
  };
  const auto source_primaries = compatible_primaries(src->color_primaries);
  const auto target_primaries = compatible_primaries(dst->color_primaries);
  const auto source_transfer = compatible_transfer(src->color_trc);
  const auto target_transfer = compatible_transfer(dst->color_trc);
  skcms_ICCProfile source, target;
  if (!rgb_profile(source_primaries, source_transfer, &source) ||
      !rgb_profile(target_primaries, target_transfer, &target))
    return AVERROR(ENOSYS);
  // skcms recognizes identity by profile address and still unpremultiplies.
  // avoid a lossy transfer-function round trip for the same color space.
  const auto* output_profile =
      source_primaries == target_primaries && source_transfer == target_transfer
      ? &source : &target;
  const auto input_format = src->format == AV_PIX_FMT_RGBA
      ? skcms_PixelFormat_RGBA_8888 : skcms_PixelFormat_BGRA_8888;
  const auto output_format = dst->format == AV_PIX_FMT_RGBA
      ? skcms_PixelFormat_RGBA_8888 : skcms_PixelFormat_BGRA_8888;
  auto* row = static_cast<uint8_t*>(av_malloc_array(src->width, 4));
  if (!row) return AVERROR(ENOMEM);
  int result = 0;
  for (int y = 0; y < src->height; ++y) {
    const auto* pixels = src->data[0] + y * src->linesize[0];
    // explicit rgb copy passes through chromium's encoded premultiplied bytes.
    // the ordinary copy path remains an exact raw plane copy in rust.
    for (int x = 0; x < src->width; ++x) {
      const unsigned alpha = pixels[x * 4 + 3];
      for (int channel = 0; channel < 3; ++channel)
        row[x * 4 + channel] = (pixels[x * 4 + channel] * alpha + 127) / 255;
      row[x * 4 + 3] = alpha;
    }
    if (!skcms_Transform(row, input_format, skcms_AlphaFormat_PremulAsEncoded,
        &source, dst->data[0] + y * dst->linesize[0], output_format,
        skcms_AlphaFormat_Unpremul, output_profile, src->width)) {
      result = AVERROR(ENOSYS);
      break;
    }
  }
  av_free(row);
  return result;
}
