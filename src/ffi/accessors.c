/**
 * Thin C accessor library for FFmpeg opaque structs
 *
 * Since we use opaque struct definitions in Rust (to avoid version-specific layouts),
 * we need C functions to access struct fields. This is compiled via the `cc` crate.
 */

#include <libavcodec/avcodec.h>
#include <libavutil/frame.h>
#include <libavutil/hwcontext.h>
#include <libavutil/imgutils.h>
#include <libavutil/opt.h>

/* ============================================================================
 * AVCodecContext Setters
 * ============================================================================ */

void ffctx_set_width(AVCodecContext* ctx, int width) {
    ctx->width = width;
}

void ffctx_set_height(AVCodecContext* ctx, int height) {
    ctx->height = height;
}

void ffctx_set_coded_width(AVCodecContext* ctx, int width) {
    ctx->coded_width = width;
}

void ffctx_set_coded_height(AVCodecContext* ctx, int height) {
    ctx->coded_height = height;
}

void ffctx_set_pix_fmt(AVCodecContext* ctx, int pix_fmt) {
    ctx->pix_fmt = pix_fmt;
}

void ffctx_set_bit_rate(AVCodecContext* ctx, int64_t bit_rate) {
    ctx->bit_rate = bit_rate;
}

void ffctx_set_rc_max_rate(AVCodecContext* ctx, int64_t rc_max_rate) {
    ctx->rc_max_rate = rc_max_rate;
}

void ffctx_set_rc_buffer_size(AVCodecContext* ctx, int rc_buffer_size) {
    ctx->rc_buffer_size = rc_buffer_size;
}

void ffctx_set_gop_size(AVCodecContext* ctx, int gop_size) {
    ctx->gop_size = gop_size;
}

void ffctx_set_max_b_frames(AVCodecContext* ctx, int max_b_frames) {
    ctx->max_b_frames = max_b_frames;
}

void ffctx_set_time_base(AVCodecContext* ctx, int num, int den) {
    ctx->time_base.num = num;
    ctx->time_base.den = den;
}

void ffctx_set_framerate(AVCodecContext* ctx, int num, int den) {
    ctx->framerate.num = num;
    ctx->framerate.den = den;
}

void ffctx_set_sample_aspect_ratio(AVCodecContext* ctx, int num, int den) {
    ctx->sample_aspect_ratio.num = num;
    ctx->sample_aspect_ratio.den = den;
}

void ffctx_set_thread_count(AVCodecContext* ctx, int thread_count) {
    ctx->thread_count = thread_count;
}

void ffctx_set_thread_type(AVCodecContext* ctx, int thread_type) {
    ctx->thread_type = thread_type;
}

void ffctx_set_color_primaries(AVCodecContext* ctx, int color_primaries) {
    ctx->color_primaries = color_primaries;
}

void ffctx_set_color_trc(AVCodecContext* ctx, int color_trc) {
    ctx->color_trc = color_trc;
}

void ffctx_set_colorspace(AVCodecContext* ctx, int colorspace) {
    ctx->colorspace = colorspace;
}

void ffctx_set_color_range(AVCodecContext* ctx, int color_range) {
    ctx->color_range = color_range;
}

void ffctx_set_flags(AVCodecContext* ctx, int flags) {
    ctx->flags = flags;
}

void ffctx_set_flags2(AVCodecContext* ctx, int flags2) {
    ctx->flags2 = flags2;
}

void ffctx_set_profile(AVCodecContext* ctx, int profile) {
    ctx->profile = profile;
}

void ffctx_set_level(AVCodecContext* ctx, int level) {
    ctx->level = level;
}

void ffctx_set_hw_device_ctx(AVCodecContext* ctx, AVBufferRef* hw_device_ctx) {
    if (ctx->hw_device_ctx) {
        av_buffer_unref(&ctx->hw_device_ctx);
    }
    if (hw_device_ctx) {
        ctx->hw_device_ctx = av_buffer_ref(hw_device_ctx);
    }
}

void ffctx_set_hw_frames_ctx(AVCodecContext* ctx, AVBufferRef* hw_frames_ctx) {
    if (ctx->hw_frames_ctx) {
        av_buffer_unref(&ctx->hw_frames_ctx);
    }
    if (hw_frames_ctx) {
        ctx->hw_frames_ctx = av_buffer_ref(hw_frames_ctx);
    }
}

/* ============================================================================
 * AVCodecContext Getters
 * ============================================================================ */

int ffctx_get_width(const AVCodecContext* ctx) {
    return ctx->width;
}

int ffctx_get_height(const AVCodecContext* ctx) {
    return ctx->height;
}

int ffctx_get_coded_width(const AVCodecContext* ctx) {
    return ctx->coded_width;
}

int ffctx_get_coded_height(const AVCodecContext* ctx) {
    return ctx->coded_height;
}

int ffctx_get_pix_fmt(const AVCodecContext* ctx) {
    return ctx->pix_fmt;
}

int64_t ffctx_get_bit_rate(const AVCodecContext* ctx) {
    return ctx->bit_rate;
}

int ffctx_get_gop_size(const AVCodecContext* ctx) {
    return ctx->gop_size;
}

int ffctx_get_max_b_frames(const AVCodecContext* ctx) {
    return ctx->max_b_frames;
}

void ffctx_get_time_base(const AVCodecContext* ctx, int* num, int* den) {
    *num = ctx->time_base.num;
    *den = ctx->time_base.den;
}

void ffctx_get_framerate(const AVCodecContext* ctx, int* num, int* den) {
    *num = ctx->framerate.num;
    *den = ctx->framerate.den;
}

int ffctx_get_profile(const AVCodecContext* ctx) {
    return ctx->profile;
}

int ffctx_get_level(const AVCodecContext* ctx) {
    return ctx->level;
}

const uint8_t* ffctx_get_extradata(const AVCodecContext* ctx) {
    return ctx->extradata;
}

int ffctx_get_extradata_size(const AVCodecContext* ctx) {
    return ctx->extradata_size;
}

/* ============================================================================
 * AVFrame Setters
 * ============================================================================ */

void ffframe_set_width(AVFrame* frame, int width) {
    frame->width = width;
}

void ffframe_set_height(AVFrame* frame, int height) {
    frame->height = height;
}

void ffframe_set_format(AVFrame* frame, int format) {
    frame->format = format;
}

void ffframe_set_pts(AVFrame* frame, int64_t pts) {
    frame->pts = pts;
}

void ffframe_set_duration(AVFrame* frame, int64_t duration) {
    frame->duration = duration;
}

void ffframe_set_pkt_dts(AVFrame* frame, int64_t pkt_dts) {
    frame->pkt_dts = pkt_dts;
}

void ffframe_set_time_base(AVFrame* frame, int num, int den) {
    frame->time_base.num = num;
    frame->time_base.den = den;
}

void ffframe_set_key_frame(AVFrame* frame, int key_frame) {
    // FFmpeg 7.0+ removed key_frame field, use flags instead
#if LIBAVUTIL_VERSION_MAJOR >= 59
    if (key_frame) {
        frame->flags |= AV_FRAME_FLAG_KEY;
    } else {
        frame->flags &= ~AV_FRAME_FLAG_KEY;
    }
#else
    frame->key_frame = key_frame;
#endif
}

void ffframe_set_pict_type(AVFrame* frame, int pict_type) {
    frame->pict_type = pict_type;
}

void ffframe_set_color_primaries(AVFrame* frame, int color_primaries) {
    frame->color_primaries = color_primaries;
}

void ffframe_set_color_trc(AVFrame* frame, int color_trc) {
    frame->color_trc = color_trc;
}

void ffframe_set_colorspace(AVFrame* frame, int colorspace) {
    frame->colorspace = colorspace;
}

void ffframe_set_color_range(AVFrame* frame, int color_range) {
    frame->color_range = color_range;
}

void ffframe_set_sample_aspect_ratio(AVFrame* frame, int num, int den) {
    frame->sample_aspect_ratio.num = num;
    frame->sample_aspect_ratio.den = den;
}

/* ============================================================================
 * AVFrame Getters
 * ============================================================================ */

int ffframe_get_width(const AVFrame* frame) {
    return frame->width;
}

int ffframe_get_height(const AVFrame* frame) {
    return frame->height;
}

int ffframe_get_format(const AVFrame* frame) {
    return frame->format;
}

int64_t ffframe_get_pts(const AVFrame* frame) {
    return frame->pts;
}

int64_t ffframe_get_duration(const AVFrame* frame) {
    return frame->duration;
}

int64_t ffframe_get_pkt_dts(const AVFrame* frame) {
    return frame->pkt_dts;
}

void ffframe_get_time_base(const AVFrame* frame, int* num, int* den) {
    *num = frame->time_base.num;
    *den = frame->time_base.den;
}

int ffframe_get_key_frame(const AVFrame* frame) {
    // FFmpeg 7.0+ removed key_frame field, use flags instead
#if LIBAVUTIL_VERSION_MAJOR >= 59
    return (frame->flags & AV_FRAME_FLAG_KEY) != 0;
#else
    return frame->key_frame;
#endif
}

int ffframe_get_pict_type(const AVFrame* frame) {
    return frame->pict_type;
}

int ffframe_get_color_primaries(const AVFrame* frame) {
    return frame->color_primaries;
}

int ffframe_get_color_trc(const AVFrame* frame) {
    return frame->color_trc;
}

int ffframe_get_colorspace(const AVFrame* frame) {
    return frame->colorspace;
}

int ffframe_get_color_range(const AVFrame* frame) {
    return frame->color_range;
}

/* ============================================================================
 * AVFrame Data Access
 * ============================================================================ */

uint8_t* ffframe_data(AVFrame* frame, int plane) {
    if (plane < 0 || plane >= AV_NUM_DATA_POINTERS) {
        return NULL;
    }
    return frame->data[plane];
}

const uint8_t* ffframe_data_const(const AVFrame* frame, int plane) {
    if (plane < 0 || plane >= AV_NUM_DATA_POINTERS) {
        return NULL;
    }
    return frame->data[plane];
}

int ffframe_linesize(const AVFrame* frame, int plane) {
    if (plane < 0 || plane >= AV_NUM_DATA_POINTERS) {
        return 0;
    }
    return frame->linesize[plane];
}

void ffframe_set_data(AVFrame* frame, int plane, uint8_t* data) {
    if (plane >= 0 && plane < AV_NUM_DATA_POINTERS) {
        frame->data[plane] = data;
    }
}

void ffframe_set_linesize(AVFrame* frame, int plane, int linesize) {
    if (plane >= 0 && plane < AV_NUM_DATA_POINTERS) {
        frame->linesize[plane] = linesize;
    }
}

/* ============================================================================
 * AVPacket Getters
 * ============================================================================ */

const uint8_t* ffpkt_data(const AVPacket* pkt) {
    return pkt->data;
}

uint8_t* ffpkt_data_mut(AVPacket* pkt) {
    return pkt->data;
}

int ffpkt_size(const AVPacket* pkt) {
    return pkt->size;
}

int64_t ffpkt_pts(const AVPacket* pkt) {
    return pkt->pts;
}

int64_t ffpkt_dts(const AVPacket* pkt) {
    return pkt->dts;
}

int64_t ffpkt_duration(const AVPacket* pkt) {
    return pkt->duration;
}

int ffpkt_flags(const AVPacket* pkt) {
    return pkt->flags;
}

int ffpkt_stream_index(const AVPacket* pkt) {
    return pkt->stream_index;
}

int64_t ffpkt_pos(const AVPacket* pkt) {
    return pkt->pos;
}

/* ============================================================================
 * AVPacket Setters
 * ============================================================================ */

void ffpkt_set_pts(AVPacket* pkt, int64_t pts) {
    pkt->pts = pts;
}

void ffpkt_set_dts(AVPacket* pkt, int64_t dts) {
    pkt->dts = dts;
}

void ffpkt_set_duration(AVPacket* pkt, int64_t duration) {
    pkt->duration = duration;
}

void ffpkt_set_flags(AVPacket* pkt, int flags) {
    pkt->flags = flags;
}

void ffpkt_set_stream_index(AVPacket* pkt, int stream_index) {
    pkt->stream_index = stream_index;
}

/* ============================================================================
 * Hardware Frames Context Accessors
 * ============================================================================ */

AVHWFramesContext* ffhwframes_get_ctx(AVBufferRef* ref) {
    return (AVHWFramesContext*)ref->data;
}

void ffhwframes_set_format(AVBufferRef* ref, int format) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    ctx->format = format;
}

void ffhwframes_set_sw_format(AVBufferRef* ref, int sw_format) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    ctx->sw_format = sw_format;
}

void ffhwframes_set_width(AVBufferRef* ref, int width) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    ctx->width = width;
}

void ffhwframes_set_height(AVBufferRef* ref, int height) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    ctx->height = height;
}

void ffhwframes_set_initial_pool_size(AVBufferRef* ref, int initial_pool_size) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    ctx->initial_pool_size = initial_pool_size;
}

int ffhwframes_get_format(AVBufferRef* ref) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    return ctx->format;
}

int ffhwframes_get_sw_format(AVBufferRef* ref) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    return ctx->sw_format;
}

int ffhwframes_get_width(AVBufferRef* ref) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    return ctx->width;
}

int ffhwframes_get_height(AVBufferRef* ref) {
    AVHWFramesContext* ctx = (AVHWFramesContext*)ref->data;
    return ctx->height;
}

/* ============================================================================
 * Utility Functions
 * ============================================================================ */

int ff_get_buffer_size(int pix_fmt, int width, int height, int align) {
    return av_image_get_buffer_size(pix_fmt, width, height, align);
}

int ff_image_fill_arrays(uint8_t* dst_data[4], int dst_linesize[4],
                         const uint8_t* src, int pix_fmt,
                         int width, int height, int align) {
    return av_image_fill_arrays(dst_data, dst_linesize, src, pix_fmt, width, height, align);
}
