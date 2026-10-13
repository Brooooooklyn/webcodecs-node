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
#include <libavutil/channel_layout.h>
#include <libavutil/samplefmt.h>
#include <libavformat/avformat.h>

#if LIBAVCODEC_VERSION_MAJOR != 63 || LIBAVUTIL_VERSION_MAJOR != 61 || LIBAVFORMAT_VERSION_MAJOR != 63
#error "Unsupported FFmpeg headers: webcodecs-node is pinned to FFmpeg 9.x (libavcodec 63, libavutil 61, libavformat 63)"
#endif

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

void ffctx_set_qmin(AVCodecContext* ctx, int qmin) {
    ctx->qmin = qmin;
}

void ffctx_set_qmax(AVCodecContext* ctx, int qmax) {
    ctx->qmax = qmax;
}

int ffctx_get_qmin(const AVCodecContext* ctx) {
    return ctx->qmin;
}

int ffctx_get_qmax(const AVCodecContext* ctx) {
    return ctx->qmax;
}

void ffctx_set_gop_size(AVCodecContext* ctx, int gop_size) {
    ctx->gop_size = gop_size;
}

void ffctx_set_max_b_frames(AVCodecContext* ctx, int max_b_frames) {
    ctx->max_b_frames = max_b_frames;
}

void ffctx_set_has_b_frames(AVCodecContext* ctx, int has_b_frames) {
    ctx->has_b_frames = has_b_frames;
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

int ffctx_get_flags(const AVCodecContext* ctx) {
    return ctx->flags;
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

/**
 * Set extradata on a codec context.
 * This allocates new memory with av_malloc and copies the data.
 * Any existing extradata is freed first.
 *
 * @param ctx The codec context
 * @param data Pointer to the extradata to copy
 * @param size Size of the extradata in bytes
 * @return 0 on success, negative error code on failure
 */
int ffctx_set_extradata(AVCodecContext* ctx, const uint8_t* data, int size) {
    // Free existing extradata if any
    if (ctx->extradata) {
        av_freep(&ctx->extradata);
        ctx->extradata_size = 0;
    }

    if (!data || size <= 0) {
        return 0;  // Nothing to set
    }

    // Allocate new extradata with padding (FFmpeg requires AV_INPUT_BUFFER_PADDING_SIZE)
    ctx->extradata = av_mallocz(size + AV_INPUT_BUFFER_PADDING_SIZE);
    if (!ctx->extradata) {
        return AVERROR(ENOMEM);
    }

    // Copy the data
    memcpy(ctx->extradata, data, size);
    ctx->extradata_size = size;

    return 0;
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

void* ffframe_get_opaque(const AVFrame* frame) {
    return frame ? frame->opaque : NULL;
}

void ffframe_set_opaque(AVFrame* frame, void* opaque) {
    if (frame) frame->opaque = opaque;
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

void ffframe_set_quality(AVFrame* frame, int quality) {
    frame->quality = quality;
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

int ffframe_get_quality(const AVFrame* frame) {
    return frame->quality;
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

AVBufferRef* ffframe_get_hw_frames_ctx(const AVFrame* frame) {
    return frame ? frame->hw_frames_ctx : NULL;
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

void* ffpkt_get_opaque(const AVPacket* pkt) {
    return pkt ? pkt->opaque : NULL;
}

void ffpkt_set_opaque(AVPacket* pkt, void* opaque) {
    if (pkt) pkt->opaque = opaque;
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
 * Audio-specific AVCodecContext Setters
 * ============================================================================ */

void ffctx_set_sample_rate(AVCodecContext* ctx, int sample_rate) {
    ctx->sample_rate = sample_rate;
}

void ffctx_set_sample_fmt(AVCodecContext* ctx, int sample_fmt) {
    ctx->sample_fmt = sample_fmt;
}

void ffctx_set_channels(AVCodecContext* ctx, int channels) {
#if LIBAVCODEC_VERSION_MAJOR >= 60
    // FFmpeg 6.0+ uses ch_layout
    av_channel_layout_default(&ctx->ch_layout, channels);
#elif LIBAVCODEC_VERSION_MAJOR >= 59 && LIBAVCODEC_VERSION_MINOR >= 24
    // FFmpeg 5.1+ - has both but channels is deprecated
    av_channel_layout_default(&ctx->ch_layout, channels);
#else
    // Older FFmpeg
    ctx->channels = channels;
    ctx->channel_layout = av_get_default_channel_layout(channels);
#endif
}

void ffctx_set_channel_layout(AVCodecContext* ctx, uint64_t channel_layout) {
#if LIBAVCODEC_VERSION_MAJOR >= 60
    // FFmpeg 6.0+ uses ch_layout
    av_channel_layout_from_mask(&ctx->ch_layout, channel_layout);
#elif LIBAVCODEC_VERSION_MAJOR >= 59 && LIBAVCODEC_VERSION_MINOR >= 24
    // FFmpeg 5.1+
    av_channel_layout_from_mask(&ctx->ch_layout, channel_layout);
#else
    // Older FFmpeg
    ctx->channel_layout = channel_layout;
    ctx->channels = av_get_channel_layout_nb_channels(channel_layout);
#endif
}

void ffctx_set_frame_size(AVCodecContext* ctx, int frame_size) {
    ctx->frame_size = frame_size;
}

/* ============================================================================
 * Audio-specific AVCodecContext Getters
 * ============================================================================ */

int ffctx_get_sample_rate(const AVCodecContext* ctx) {
    return ctx->sample_rate;
}

int ffctx_get_sample_fmt(const AVCodecContext* ctx) {
    return ctx->sample_fmt;
}

int ffctx_get_channels(const AVCodecContext* ctx) {
#if LIBAVCODEC_VERSION_MAJOR >= 60
    return ctx->ch_layout.nb_channels;
#elif LIBAVCODEC_VERSION_MAJOR >= 59 && LIBAVCODEC_VERSION_MINOR >= 24
    return ctx->ch_layout.nb_channels;
#else
    return ctx->channels;
#endif
}

uint64_t ffctx_get_channel_layout(const AVCodecContext* ctx) {
#if LIBAVCODEC_VERSION_MAJOR >= 60
    // FFmpeg 6.0+ - extract mask from ch_layout
    if (ctx->ch_layout.order == AV_CHANNEL_ORDER_NATIVE) {
        return ctx->ch_layout.u.mask;
    }
    // For other orders, construct a default layout and return its mask
    AVChannelLayout default_layout = {0};
    av_channel_layout_default(&default_layout, ctx->ch_layout.nb_channels);
    uint64_t mask = default_layout.u.mask;
    av_channel_layout_uninit(&default_layout);
    return mask;
#elif LIBAVCODEC_VERSION_MAJOR >= 59 && LIBAVCODEC_VERSION_MINOR >= 24
    // FFmpeg 5.1+
    if (ctx->ch_layout.order == AV_CHANNEL_ORDER_NATIVE) {
        return ctx->ch_layout.u.mask;
    }
    AVChannelLayout default_layout = {0};
    av_channel_layout_default(&default_layout, ctx->ch_layout.nb_channels);
    uint64_t mask = default_layout.u.mask;
    av_channel_layout_uninit(&default_layout);
    return mask;
#else
    return ctx->channel_layout;
#endif
}

int ffctx_get_frame_size(const AVCodecContext* ctx) {
    return ctx->frame_size;
}

/* ============================================================================
 * Audio-specific AVFrame Setters
 * ============================================================================ */

void ffframe_set_nb_samples(AVFrame* frame, int nb_samples) {
    frame->nb_samples = nb_samples;
}

void ffframe_set_sample_rate(AVFrame* frame, int sample_rate) {
    frame->sample_rate = sample_rate;
}

void ffframe_set_channels(AVFrame* frame, int channels) {
#if LIBAVUTIL_VERSION_MAJOR >= 58
    // FFmpeg 6.0+ uses ch_layout
    av_channel_layout_default(&frame->ch_layout, channels);
#elif LIBAVUTIL_VERSION_MAJOR >= 57 && LIBAVUTIL_VERSION_MINOR >= 24
    // FFmpeg 5.1+
    av_channel_layout_default(&frame->ch_layout, channels);
#else
    // Older FFmpeg
    frame->channels = channels;
    frame->channel_layout = av_get_default_channel_layout(channels);
#endif
}

void ffframe_set_channel_layout(AVFrame* frame, uint64_t channel_layout) {
#if LIBAVUTIL_VERSION_MAJOR >= 58
    // FFmpeg 6.0+ uses ch_layout
    av_channel_layout_from_mask(&frame->ch_layout, channel_layout);
#elif LIBAVUTIL_VERSION_MAJOR >= 57 && LIBAVUTIL_VERSION_MINOR >= 24
    // FFmpeg 5.1+
    av_channel_layout_from_mask(&frame->ch_layout, channel_layout);
#else
    // Older FFmpeg
    frame->channel_layout = channel_layout;
    frame->channels = av_get_channel_layout_nb_channels(channel_layout);
#endif
}

/* ============================================================================
 * Audio-specific AVFrame Getters
 * ============================================================================ */

int ffframe_get_nb_samples(const AVFrame* frame) {
    return frame->nb_samples;
}

int ffframe_get_sample_rate(const AVFrame* frame) {
    return frame->sample_rate;
}

int ffframe_get_channels(const AVFrame* frame) {
#if LIBAVUTIL_VERSION_MAJOR >= 58
    return frame->ch_layout.nb_channels;
#elif LIBAVUTIL_VERSION_MAJOR >= 57 && LIBAVUTIL_VERSION_MINOR >= 24
    return frame->ch_layout.nb_channels;
#else
    return frame->channels;
#endif
}

uint64_t ffframe_get_channel_layout(const AVFrame* frame) {
#if LIBAVUTIL_VERSION_MAJOR >= 58
    // FFmpeg 6.0+ - extract mask from ch_layout
    if (frame->ch_layout.order == AV_CHANNEL_ORDER_NATIVE) {
        return frame->ch_layout.u.mask;
    }
    // For other orders, construct a default layout and return its mask
    AVChannelLayout default_layout = {0};
    av_channel_layout_default(&default_layout, frame->ch_layout.nb_channels);
    uint64_t mask = default_layout.u.mask;
    av_channel_layout_uninit(&default_layout);
    return mask;
#elif LIBAVUTIL_VERSION_MAJOR >= 57 && LIBAVUTIL_VERSION_MINOR >= 24
    // FFmpeg 5.1+
    if (frame->ch_layout.order == AV_CHANNEL_ORDER_NATIVE) {
        return frame->ch_layout.u.mask;
    }
    AVChannelLayout default_layout = {0};
    av_channel_layout_default(&default_layout, frame->ch_layout.nb_channels);
    uint64_t mask = default_layout.u.mask;
    av_channel_layout_uninit(&default_layout);
    return mask;
#else
    return frame->channel_layout;
#endif
}

/* ============================================================================
 * Audio Frame Data Access (extended_data for planar audio)
 * ============================================================================ */

uint8_t** ffframe_get_extended_data(AVFrame* frame) {
    return frame->extended_data;
}

const uint8_t* const* ffframe_get_extended_data_const(const AVFrame* frame) {
    return (const uint8_t* const*)frame->extended_data;
}

uint8_t* ffframe_extended_data_plane(AVFrame* frame, int plane) {
    if (frame->extended_data == NULL) {
        return NULL;
    }
    return frame->extended_data[plane];
}

void ffframe_set_extended_data(AVFrame* frame, uint8_t** extended_data) {
    frame->extended_data = extended_data;
}

/* ============================================================================
 * Audio Utility Functions
 * ============================================================================ */

int ff_get_bytes_per_sample(int sample_fmt) {
    return av_get_bytes_per_sample(sample_fmt);
}

int ff_sample_fmt_is_planar(int sample_fmt) {
    return av_sample_fmt_is_planar(sample_fmt);
}

int ff_get_audio_buffer_size(int channels, int nb_samples, int sample_fmt, int align) {
    return av_samples_get_buffer_size(NULL, channels, nb_samples, sample_fmt, align);
}

int ff_samples_fill_arrays(uint8_t** audio_data, int* linesize,
                           const uint8_t* buf, int channels,
                           int nb_samples, int sample_fmt, int align) {
    return av_samples_fill_arrays(audio_data, linesize, buf, channels,
                                  nb_samples, sample_fmt, align);
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

/* ============================================================================
 * AVFormatContext Accessors (libavformat)
 * ============================================================================ */

void fffmt_set_pb(AVFormatContext* ctx, AVIOContext* pb) {
    ctx->pb = pb;
}

AVIOContext* fffmt_get_pb(AVFormatContext* ctx) {
    return ctx->pb;
}

unsigned int fffmt_get_nb_streams(const AVFormatContext* ctx) {
    return ctx->nb_streams;
}

AVStream* fffmt_get_stream(AVFormatContext* ctx, unsigned int index) {
    if (index >= ctx->nb_streams) {
        return NULL;
    }
    return ctx->streams[index];
}

int64_t fffmt_get_duration(const AVFormatContext* ctx) {
    return ctx->duration;
}

int64_t fffmt_get_bit_rate(const AVFormatContext* ctx) {
    return ctx->bit_rate;
}

const AVOutputFormat* fffmt_get_oformat(const AVFormatContext* ctx) {
    return ctx->oformat;
}

const AVInputFormat* fffmt_get_iformat(const AVFormatContext* ctx) {
    return ctx->iformat;
}

int fffmt_get_oformat_flags(const AVFormatContext* ctx) {
    return ctx->oformat ? ctx->oformat->flags : 0;
}

/* ============================================================================
 * AVStream Accessors
 * ============================================================================ */

int ffstream_get_index(const AVStream* stream) {
    return stream->index;
}

AVCodecParameters* ffstream_get_codecpar(AVStream* stream) {
    return stream->codecpar;
}

const AVCodecParameters* ffstream_get_codecpar_const(const AVStream* stream) {
    return stream->codecpar;
}

void ffstream_get_time_base(const AVStream* stream, int* num, int* den) {
    *num = stream->time_base.num;
    *den = stream->time_base.den;
}

void ffstream_set_time_base(AVStream* stream, int num, int den) {
    stream->time_base.num = num;
    stream->time_base.den = den;
}

void ffstream_get_avg_frame_rate(const AVStream* stream, int* num, int* den) {
    *num = stream->avg_frame_rate.num;
    *den = stream->avg_frame_rate.den;
}

int64_t ffstream_get_duration(const AVStream* stream) {
    return stream->duration;
}

int64_t ffstream_get_nb_frames(const AVStream* stream) {
    return stream->nb_frames;
}

int64_t ffstream_get_start_time(const AVStream* stream) {
    return stream->start_time;
}

/* ============================================================================
 * AVCodecParameters Accessors
 * ============================================================================ */

int ffcodecpar_get_codec_type(const AVCodecParameters* par) {
    return par->codec_type;
}

void ffcodecpar_set_codec_type(AVCodecParameters* par, int codec_type) {
    par->codec_type = codec_type;
}

int ffcodecpar_get_codec_id(const AVCodecParameters* par) {
    return par->codec_id;
}

void ffcodecpar_set_codec_id(AVCodecParameters* par, int codec_id) {
    par->codec_id = codec_id;
}

unsigned int ffcodecpar_get_codec_tag(const AVCodecParameters* par) {
    return par->codec_tag;
}

void ffcodecpar_set_codec_tag(AVCodecParameters* par, unsigned int codec_tag) {
    par->codec_tag = codec_tag;
}

int ffcodecpar_get_format(const AVCodecParameters* par) {
    return par->format;
}

void ffcodecpar_set_format(AVCodecParameters* par, int format) {
    par->format = format;
}

int64_t ffcodecpar_get_bit_rate(const AVCodecParameters* par) {
    return par->bit_rate;
}

void ffcodecpar_set_bit_rate(AVCodecParameters* par, int64_t bit_rate) {
    par->bit_rate = bit_rate;
}

int ffcodecpar_get_width(const AVCodecParameters* par) {
    return par->width;
}

void ffcodecpar_set_width(AVCodecParameters* par, int width) {
    par->width = width;
}

int ffcodecpar_get_height(const AVCodecParameters* par) {
    return par->height;
}

void ffcodecpar_set_height(AVCodecParameters* par, int height) {
    par->height = height;
}

int ffcodecpar_get_sample_rate(const AVCodecParameters* par) {
    return par->sample_rate;
}

void ffcodecpar_set_sample_rate(AVCodecParameters* par, int sample_rate) {
    par->sample_rate = sample_rate;
}

int ffcodecpar_get_channels(const AVCodecParameters* par) {
#if LIBAVCODEC_VERSION_MAJOR >= 60
    return par->ch_layout.nb_channels;
#elif LIBAVCODEC_VERSION_MAJOR >= 59 && LIBAVCODEC_VERSION_MINOR >= 24
    return par->ch_layout.nb_channels;
#else
    return par->channels;
#endif
}

void ffcodecpar_set_channels(AVCodecParameters* par, int channels) {
#if LIBAVCODEC_VERSION_MAJOR >= 60
    av_channel_layout_default(&par->ch_layout, channels);
#elif LIBAVCODEC_VERSION_MAJOR >= 59 && LIBAVCODEC_VERSION_MINOR >= 24
    av_channel_layout_default(&par->ch_layout, channels);
#else
    par->channels = channels;
    par->channel_layout = av_get_default_channel_layout(channels);
#endif
}

int ffcodecpar_get_frame_size(const AVCodecParameters* par) {
    return par->frame_size;
}

void ffcodecpar_set_frame_size(AVCodecParameters* par, int frame_size) {
    par->frame_size = frame_size;
}

const uint8_t* ffcodecpar_get_extradata(const AVCodecParameters* par) {
    return par->extradata;
}

int ffcodecpar_get_extradata_size(const AVCodecParameters* par) {
    return par->extradata_size;
}

int ffcodecpar_set_extradata(AVCodecParameters* par, const uint8_t* data, int size) {
    // Free existing extradata
    av_freep(&par->extradata);
    par->extradata_size = 0;

    if (data == NULL || size <= 0) {
        return 0;
    }

    // Allocate new extradata with padding
    par->extradata = av_mallocz(size + AV_INPUT_BUFFER_PADDING_SIZE);
    if (par->extradata == NULL) {
        return AVERROR(ENOMEM);
    }

    memcpy(par->extradata, data, size);
    par->extradata_size = size;
    return 0;
}

int ffcodecpar_get_color_primaries(const AVCodecParameters* par) {
    return par->color_primaries;
}

void ffcodecpar_set_color_primaries(AVCodecParameters* par, int color_primaries) {
    par->color_primaries = color_primaries;
}

int ffcodecpar_get_color_trc(const AVCodecParameters* par) {
    return par->color_trc;
}

void ffcodecpar_set_color_trc(AVCodecParameters* par, int color_trc) {
    par->color_trc = color_trc;
}

int ffcodecpar_get_color_space(const AVCodecParameters* par) {
    return par->color_space;
}

void ffcodecpar_set_color_space(AVCodecParameters* par, int color_space) {
    par->color_space = color_space;
}

int ffcodecpar_get_color_range(const AVCodecParameters* par) {
    return par->color_range;
}

void ffcodecpar_set_color_range(AVCodecParameters* par, int color_range) {
    par->color_range = color_range;
}

void ffcodecpar_get_sample_aspect_ratio(const AVCodecParameters* par, int* num, int* den) {
    *num = par->sample_aspect_ratio.num;
    *den = par->sample_aspect_ratio.den;
}

void ffcodecpar_set_sample_aspect_ratio(AVCodecParameters* par, int num, int den) {
    par->sample_aspect_ratio.num = num;
    par->sample_aspect_ratio.den = den;
}

/* ============================================================================
 * AVIOContext Accessors
 * ============================================================================ */

void* fffio_get_opaque(AVIOContext* ctx) {
    return ctx ? ctx->opaque : NULL;
}

void fffio_set_opaque(AVIOContext* ctx, void* opaque) {
    if (ctx) {
        ctx->opaque = opaque;
    }
}

int fffio_get_seekable(AVIOContext* ctx) {
    return ctx ? ctx->seekable : 0;
}

void fffio_set_seekable(AVIOContext* ctx, int seekable) {
    if (ctx) {
        ctx->seekable = seekable;
    }
}

/* ============================================================================
 * Hardware Decoding Support
 * ============================================================================ */

/**
 * Static get_format callback that selects hardware pixel format from opaque.
 * This is required for hardware decoding - FFmpeg needs this callback to
 * negotiate the pixel format with the hardware decoder.
 */
static enum AVPixelFormat ff_get_format_hw(AVCodecContext *ctx, const enum AVPixelFormat *pix_fmts) {
    intptr_t hw_pix_fmt = (intptr_t)ctx->opaque;

    // Find the hardware format in the list of supported formats
    for (const enum AVPixelFormat *p = pix_fmts; *p != AV_PIX_FMT_NONE; p++) {
        if (*p == (enum AVPixelFormat)hw_pix_fmt) {
            return *p;
        }
    }

    // Fall back to first software format
    return pix_fmts[0];
}

/**
 * Set up get_format callback for hardware decoding.
 * Must be called after setting hw_device_ctx but before avcodec_open2().
 *
 * WARNING: This function uses ctx->opaque to store the hardware pixel format.
 * If other code needs to use ctx->opaque for application-specific data,
 * this will conflict. Consider refactoring to use a wrapper struct if
 * additional opaque data is needed in the future.
 *
 * @param ctx The codec context
 * @param hw_pix_fmt The hardware pixel format (e.g., AV_PIX_FMT_VIDEOTOOLBOX)
 */
void ffctx_set_hw_get_format(AVCodecContext* ctx, int hw_pix_fmt) {
    ctx->opaque = (void*)(intptr_t)hw_pix_fmt;
    ctx->get_format = ff_get_format_hw;
}

/* ============================================================================
 * AVCodecHWConfig Accessors
 * ============================================================================ */

/**
 * Get the pixel format from a codec hardware config.
 * Returns the hardware pixel format (e.g., AV_PIX_FMT_VIDEOTOOLBOX).
 */
int ffhwconfig_get_pix_fmt(const AVCodecHWConfig* config) {
    return config ? config->pix_fmt : AV_PIX_FMT_NONE;
}

/**
 * Get the methods bitmask from a codec hardware config.
 * Check against AV_CODEC_HW_CONFIG_METHOD_* constants.
 */
int ffhwconfig_get_methods(const AVCodecHWConfig* config) {
    return config ? config->methods : 0;
}

/**
 * Get the device type from a codec hardware config.
 * Returns the AVHWDeviceType (e.g., AV_HWDEVICE_TYPE_VIDEOTOOLBOX).
 */
int ffhwconfig_get_device_type(const AVCodecHWConfig* config) {
    return config ? config->device_type : 0;
}

/**
 * Check if codec supports hardware decoding with a specific device type.
 * Returns the pixel format if supported, or AV_PIX_FMT_NONE if not.
 *
 * @param codec The codec to check
 * @param device_type The hardware device type to look for
 * @return The hardware pixel format, or AV_PIX_FMT_NONE if not supported
 */
int ff_codec_get_hw_pix_fmt(const AVCodec* codec, int device_type) {
    if (!codec) {
        return AV_PIX_FMT_NONE;
    }

    for (int i = 0; ; i++) {
        const AVCodecHWConfig* config = avcodec_get_hw_config(codec, i);
        if (!config) {
            break;
        }

        // Check if this config matches our device type and supports hw_device_ctx method
        if ((config->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX) &&
            config->device_type == (enum AVHWDeviceType)device_type) {
            return config->pix_fmt;
        }
    }

    return AV_PIX_FMT_NONE;
}

/* ============================================================================
 * VideoFrame.copyTo to RGBA/BGRA with a target colour space (Chrome's model).
 * Integer libyuv front end, then 256-entry decode LUT -> 3x3 (-> tone map) -> exact
 * sRGB encode. Only the copy rect is touched; output goes straight to the caller.
 * ============================================================================ */
#include <math.h>
#include <stdint.h>
#include <string.h>
#include <libavutil/csp.h>
#include <libavutil/error.h>
#include <libavutil/mem.h>
#include <libavutil/pixdesc.h>
#include <libswscale/swscale.h>

enum { RC_CURVE_SRGB, RC_CURVE_LINEAR, RC_CURVE_G22, RC_CURVE_G28, RC_CURVE_PQ, RC_CURVE_HLG };
enum { RC_IDENTITY, RC_LUT8, RC_MATRIX, RC_PQ, RC_HLG };

typedef struct RcState {
  int cls;
  float dec[256];   // 8-bit code -> linear (SDR) / 203-nit relative (PQ) / scene light (HLG)
  float m[9];       // SDR: src -> target. HDR: src -> bt2020
  float m2[9];      // HDR: bt2020 -> target
  float ky[3];      // HLG: luminance row of the source primaries
  float tm_k;       // HDR: x7 * 2^y7
  float thr[256];   // encode: code k+1 starts at linear thr[k]
  struct { float t; int32_t k; } enc[4097]; // bucket [i/4096, (i+1)/4096): code k, k+1 from t on
  uint8_t lut8[256];
} RcState;

// libyuv YuvConstants (row_common.cc, LIBYUV_UNLIMITED_* off): YG, YB, UB, UG, VG, VR
static const int32_t kRcYuv[6][6] = {
  {18997, -1160, 128, 25, 52, 102}, // I601
  {16320, 32, 113, 22, 46, 90},     // JPEG
  {18997, -1160, 128, 14, 34, 115}, // H709
  {16320, 32, 119, 12, 30, 101},    // F709
  {19003, -1160, 128, 12, 42, 107}, // 2020
  {16320, 32, 120, 11, 37, 94},     // V2020
};

// AGTM RWTMO control points for frames without HDR metadata (x, log2 gain, slope), SPEC.md §5
static const double kRcTm[8][3] = {
  {1.0, -1.0, 0.0},
  {1.2392578125, -1.0712890625, -0.439697265625},
  {1.5859375, -1.2353515625, -0.47802734375},
  {2.0390625, -1.44140625, -0.42529296875},
  {2.599609375, -1.6611328125, -0.359619140625},
  {3.267578125, -1.8798828125, -0.30126953125},
  {4.04296875, -2.09375, -0.253662109375},
  {4.92578125, -2.30078125, -0.2158203125},
};

static double rc_srgb_dec(double v) {
  return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4);
}

static double rc_curve_dec(int curve, double v) {
  switch (curve) {
  case RC_CURVE_LINEAR: return v;
  case RC_CURVE_G22: return pow(v, 2.2);
  case RC_CURVE_G28: return pow(v, 2.8);
  case RC_CURVE_PQ: {
    const double m1 = 2610.0 / 16384, m2 = 2523.0 / 4096 * 128;
    const double c1 = 3424.0 / 4096, c2 = 2413.0 / 4096 * 32, c3 = 2392.0 / 4096 * 32;
    double p = pow(v, 1.0 / m2), n = p - c1;
    return pow((n > 0 ? n : 0) / (c2 - c3 * p), 1.0 / m1) * 10000.0 / 203.0;
  }
  case RC_CURVE_HLG:
    return v <= 0.5 ? v * v / 3 : (exp((v - 0.55991073) / 0.17883277) + 0.28466892) / 12;
  default: return rc_srgb_dec(v);
  }
}

// RGB -> XYZ for D65 primaries; returns 0 when the primaries are unknown or not D65.
static int rc_npm(enum AVColorPrimaries p, double o[9]) {
  const AVColorPrimariesDesc *d = av_csp_primaries_desc_from_id(p);
  if (!d || av_cmp_q(d->wp.x, av_make_q(3127, 10000)) || av_cmp_q(d->wp.y, av_make_q(3290, 10000)))
    return 0;
  const AVCIExy *c[3] = {&d->prim.r, &d->prim.g, &d->prim.b};
  double P[9], inv[9], W[3] = {0.3127 / 0.3290, 1.0, (1 - 0.3127 - 0.3290) / 0.3290};
  for (int i = 0; i < 3; i++) {
    double x = av_q2d(c[i]->x), y = av_q2d(c[i]->y);
    P[i] = x / y; P[3 + i] = 1.0; P[6 + i] = (1 - x - y) / y;
  }
  double det = P[0] * (P[4] * P[8] - P[5] * P[7]) - P[1] * (P[3] * P[8] - P[5] * P[6]) +
               P[2] * (P[3] * P[7] - P[4] * P[6]);
  inv[0] = (P[4] * P[8] - P[5] * P[7]) / det; inv[1] = (P[2] * P[7] - P[1] * P[8]) / det;
  inv[2] = (P[1] * P[5] - P[2] * P[4]) / det; inv[3] = (P[5] * P[6] - P[3] * P[8]) / det;
  inv[4] = (P[0] * P[8] - P[2] * P[6]) / det; inv[5] = (P[2] * P[3] - P[0] * P[5]) / det;
  inv[6] = (P[3] * P[7] - P[4] * P[6]) / det; inv[7] = (P[1] * P[6] - P[0] * P[7]) / det;
  inv[8] = (P[0] * P[4] - P[1] * P[3]) / det;
  for (int i = 0; i < 3; i++) {
    double s = inv[i * 3] * W[0] + inv[i * 3 + 1] * W[1] + inv[i * 3 + 2] * W[2];
    for (int r = 0; r < 3; r++) o[r * 3 + i] = P[r * 3 + i] * s;
  }
  return 1;
}

static void rc_inv3(const double a[9], double o[9]) {
  double det = a[0] * (a[4] * a[8] - a[5] * a[7]) - a[1] * (a[3] * a[8] - a[5] * a[6]) +
               a[2] * (a[3] * a[7] - a[4] * a[6]);
  o[0] = (a[4] * a[8] - a[5] * a[7]) / det; o[1] = (a[2] * a[7] - a[1] * a[8]) / det;
  o[2] = (a[1] * a[5] - a[2] * a[4]) / det; o[3] = (a[5] * a[6] - a[3] * a[8]) / det;
  o[4] = (a[0] * a[8] - a[2] * a[6]) / det; o[5] = (a[2] * a[3] - a[0] * a[5]) / det;
  o[6] = (a[3] * a[7] - a[4] * a[6]) / det; o[7] = (a[1] * a[6] - a[0] * a[7]) / det;
  o[8] = (a[0] * a[4] - a[1] * a[3]) / det;
}

// out = inv(npm(dst)) * npm(src), as float
static void rc_gamut(enum AVColorPrimaries src, enum AVColorPrimaries dst, float out[9],
                     double ky[3]) {
  double s[9], d[9], di[9];
  rc_npm(src, s);
  rc_npm(dst, d);
  rc_inv3(d, di);
  for (int r = 0; r < 3; r++)
    for (int c = 0; c < 3; c++)
      out[r * 3 + c] = (float)(di[r * 3] * s[c] + di[r * 3 + 1] * s[3 + c] + di[r * 3 + 2] * s[6 + c]);
  if (ky) for (int c = 0; c < 3; c++) ky[c] = s[3 + c];
}

// round(sRGB_encode(l) * 255), exact: a bucket is narrower than any code, so it holds at most
// one code boundary.
static inline uint8_t rc_enc(const RcState *t, float l) {
  l = l > 0.0f ? (l < 1.0f ? l : 1.0f) : 0.0f; // also maps NaN to 0
  int i = (int)(l * 4096.0f);
  return (uint8_t)(t->enc[i].k + (l >= t->enc[i].t));
}

// SPEC.md §2: tags -> class. Returns the YUV table index for YUV sources.
static int rc_setup(RcState *t, int pri, int trc, int mat, int range, int target_p3) {
  int missing = pri == AVCOL_PRI_RESERVED0 || pri == AVCOL_PRI_UNSPECIFIED ||
                pri == AVCOL_PRI_RESERVED || trc == AVCOL_TRC_RESERVED0 ||
                trc == AVCOL_TRC_UNSPECIFIED || trc == AVCOL_TRC_RESERVED ||
                mat == AVCOL_SPC_UNSPECIFIED || mat == AVCOL_SPC_RESERVED ||
                range == AVCOL_RANGE_UNSPECIFIED;
  int curve = RC_CURVE_SRGB;
  switch (trc) {
  case AVCOL_TRC_LINEAR: curve = RC_CURVE_LINEAR; break;
  case AVCOL_TRC_GAMMA22: curve = RC_CURVE_G22; break;
  case AVCOL_TRC_GAMMA28: curve = RC_CURVE_G28; break;
  case AVCOL_TRC_SMPTE2084: curve = RC_CURVE_PQ; break;
  case AVCOL_TRC_ARIB_STD_B67: curve = RC_CURVE_HLG; break;
  case AVCOL_TRC_BT709: case AVCOL_TRC_SMPTE170M: case AVCOL_TRC_SMPTE240M:
  case AVCOL_TRC_IEC61966_2_1: case AVCOL_TRC_BT2020_10: case AVCOL_TRC_BT2020_12: break;
  default: missing = 1; // log, log-sqrt, iec61966-2-4, bt1361, smpte428: drawn as sRGB
  }
  enum AVColorPrimaries gamut = (enum AVColorPrimaries)pri, dst = target_p3 ? AVCOL_PRI_SMPTE432 : AVCOL_PRI_BT709;
  double tmp[9];
  if (gamut == AVCOL_PRI_SMPTE170M || gamut == AVCOL_PRI_SMPTE240M) gamut = AVCOL_PRI_BT709;
  if (missing || !rc_npm(gamut, tmp)) { curve = RC_CURVE_SRGB; gamut = AVCOL_PRI_BT709; }

  int full = range == AVCOL_RANGE_JPEG, yuv = 0;
  if (mat == AVCOL_SPC_BT709) yuv = full ? 3 : 2;
  else if (mat == AVCOL_SPC_BT470BG || mat == AVCOL_SPC_SMPTE170M) yuv = full ? 1 : 0;
  else if (mat == AVCOL_SPC_BT2020_NCL) yuv = full ? 5 : 4;

  if (curve == RC_CURVE_SRGB && gamut == dst) { t->cls = RC_IDENTITY; return yuv; }
  for (int i = 0; i < 256; i++) t->dec[i] = (float)rc_curve_dec(curve, i / 255.0);
  for (int k = 0; k < 255; k++) t->thr[k] = (float)rc_srgb_dec((k + 0.5) / 255.0);
  t->thr[255] = 2.0f;
  for (int i = 0, k = 0; i <= 4096; i++) {
    while (k < 255 && t->thr[k] <= (float)i / 4096.0f) k++;
    t->enc[i].k = k;
    t->enc[i].t = t->thr[k];
  }
  if (curve == RC_CURVE_PQ || curve == RC_CURVE_HLG) {
    double ky[3];
    t->cls = curve == RC_CURVE_PQ ? RC_PQ : RC_HLG;
    rc_gamut(gamut, AVCOL_PRI_BT2020, t->m, ky);
    rc_gamut(AVCOL_PRI_BT2020, dst, t->m2, NULL);
    for (int c = 0; c < 3; c++) t->ky[c] = (float)ky[c];
    t->tm_k = (float)(kRcTm[7][0] * exp2(kRcTm[7][1]));
  } else if (gamut == dst) {
    t->cls = RC_LUT8;
    for (int i = 0; i < 256; i++) t->lut8[i] = rc_enc(t, t->dec[i]);
  } else {
    t->cls = RC_MATRIX;
    rc_gamut(gamut, dst, t->m, NULL);
  }
  return yuv;
}

static inline float rc_tm_gain(const RcState *t, float x) {
  if (x <= 1.0f) return 0.5f;
  if (x >= (float)kRcTm[7][0]) return t->tm_k / x;
  int i = 0;
  while (x > (float)kRcTm[i + 1][0]) i++;
  float xa = (float)kRcTm[i][0], ya = (float)kRcTm[i][1], ma = (float)kRcTm[i][2];
  float xb = (float)kRcTm[i + 1][0], yb = (float)kRcTm[i + 1][1], mb = (float)kRcTm[i + 1][2];
  float h = xb - xa, s = (x - xa) / h;
  float c3 = 2 * ya + ma * h - 2 * yb + mb * h, c2 = -3 * ya + 3 * yb - 2 * ma * h - mb * h;
  return exp2f(((c3 * s + c2) * s + ma * h) * s + ya);
}

// Colour stage, in place on n pixels of 8-bit R'G'B'A (ri/bi = byte index of R and B).
// Coefficients are copied to locals: stores through uint8_t * may alias *t.
static av_always_inline void rc_color_row(const RcState *t, uint8_t *p, int n, int ri, int bi) {
  const float *dec = t->dec;
  const float a0 = t->m[0], a1 = t->m[1], a2 = t->m[2], a3 = t->m[3], a4 = t->m[4],
              a5 = t->m[5], a6 = t->m[6], a7 = t->m[7], a8 = t->m[8];
  const float b0 = t->m2[0], b1 = t->m2[1], b2 = t->m2[2], b3 = t->m2[3], b4 = t->m2[4],
              b5 = t->m2[5], b6 = t->m2[6], b7 = t->m2[7], b8 = t->m2[8];
  const float k0 = t->ky[0], k1 = t->ky[1], k2 = t->ky[2];
  const int hlg = t->cls == RC_HLG;
  if (t->cls == RC_LUT8) {
    const uint8_t *lut = t->lut8;
    for (int i = 0; i < n; i++, p += 4) {
      uint8_t r = lut[p[0]], g = lut[p[1]], b = lut[p[2]];
      p[0] = r; p[1] = g; p[2] = b;
    }
  } else if (t->cls == RC_MATRIX) {
    for (int i = 0; i < n; i++, p += 4) {
      float r = dec[p[ri]], g = dec[p[1]], b = dec[p[bi]];
      uint8_t R = rc_enc(t, a0 * r + a1 * g + a2 * b);
      uint8_t G = rc_enc(t, a3 * r + a4 * g + a5 * b);
      uint8_t B = rc_enc(t, a6 * r + a7 * g + a8 * b);
      p[ri] = R; p[1] = G; p[bi] = B;
    }
  } else { // PQ / HLG: SPEC.md §5
    for (int i = 0; i < n; i++, p += 4) {
      float r = dec[p[ri]], g = dec[p[1]], b = dec[p[bi]];
      if (hlg) { // OOTF, gamma 1.2, Lw 1000, 1.0 = 203 nits
        float ys = k0 * r + k1 * g + k2 * b;
        float k = ys > 0.0f ? powf(ys, 0.2f) * (1000.0f / 203.0f) : 0.0f;
        r *= k; g *= k; b *= k;
      }
      float R = a0 * r + a1 * g + a2 * b, G = a3 * r + a4 * g + a5 * b, B = a6 * r + a7 * g + a8 * b;
      float x = R > G ? R : G, s = rc_tm_gain(t, x > B ? x : B);
      R *= s; G *= s; B *= s;
      uint8_t oR = rc_enc(t, b0 * R + b1 * G + b2 * B);
      uint8_t oG = rc_enc(t, b3 * R + b4 * G + b5 * B);
      uint8_t oB = rc_enc(t, b6 * R + b7 * G + b8 * B);
      p[ri] = oR; p[1] = oG; p[bi] = oB;
    }
  }
}

static inline int32_t rc_clamp6(int32_t z) {
  z = z < 0 ? 0 : z >> 6;
  return z > 255 ? 255 : z;
}

#define RC_CHUNK 256
// Stage 1 for a chunk: 16-bit expanded luma + 8-bit chroma/alpha, nearest chroma (libyuv
// YuvPixel / YuvPixel10_16 input stage). Stage 2: libyuv integer matrix, unit stride.
static av_always_inline void rc_yuv_stage(const uint8_t *ys, const uint8_t *us, const uint8_t *vs,
                                          const uint8_t *as, int n, int xo, int hs, int cstep,
                                          int depth, int shift, uint16_t *y16, uint8_t *u8,
                                          uint8_t *v8, uint8_t *a8) {
  uint8_t cu[RC_CHUNK + 1], cv[RC_CHUNK + 1];
  int nc = ((n - 1 + xo) >> hs) + 1;
  if (depth == 8) {
    for (int i = 0; i < n; i++) y16[i] = (uint16_t)(ys[i] * 0x0101);
    for (int j = 0; j < nc; j++) { cu[j] = us[j * cstep]; cv[j] = vs[j * cstep]; }
    if (as) memcpy(a8, as, (size_t)n);
  } else {
    const uint16_t *y2 = (const uint16_t *)ys, *u2 = (const uint16_t *)us, *v2 = (const uint16_t *)vs;
    for (int i = 0; i < n; i++) {
      uint32_t v = (uint32_t)(y2[i] >> shift);
      y16[i] = (uint16_t)((v << (16 - depth)) | (v >> (2 * depth - 16)));
    }
    for (int j = 0; j < nc; j++) {
      int u = (u2[j * cstep] >> shift) >> (depth - 8), v = (v2[j * cstep] >> shift) >> (depth - 8);
      cu[j] = (uint8_t)(u > 255 ? 255 : u); cv[j] = (uint8_t)(v > 255 ? 255 : v);
    }
    if (as)
      for (int i = 0; i < n; i++) a8[i] = (uint8_t)((((const uint16_t *)as)[i] >> shift) >> (depth - 8));
  }
  if (hs == 0) {
    memcpy(u8, cu, (size_t)n); memcpy(v8, cv, (size_t)n);
  } else if (hs == 1) { // 2:1 nearest chroma as a pair loop so it vectorises
    int pairs = (n - xo) >> 1, j;
    if (xo) { u8[0] = cu[0]; v8[0] = cv[0]; }
    for (j = 0; j < pairs; j++) {
      u8[xo + 2 * j] = u8[xo + 2 * j + 1] = cu[xo + j];
      v8[xo + 2 * j] = v8[xo + 2 * j + 1] = cv[xo + j];
    }
    if (xo + 2 * pairs < n) { u8[n - 1] = cu[xo + j]; v8[n - 1] = cv[xo + j]; }
  } else {
    for (int i = 0; i < n; i++) { u8[i] = cu[(i + xo) >> hs]; v8[i] = cv[(i + xo) >> hs]; }
  }
}

static av_always_inline void rc_yuv_matrix(const uint16_t *y16, const uint8_t *u8, const uint8_t *v8,
                                           const uint8_t *a8, uint8_t *o, int n, const int32_t *k,
                                           int ri, int bi) {
  const uint32_t yg = (uint32_t)k[0];
  const int32_t yb = k[1], ub = k[2], ug = k[3], vg = k[4], vr = k[5];
  const int32_t rb = vr * 128 - yb, gb = ug * 128 + vg * 128 + yb, bb = ub * 128 - yb;
  for (int i = 0; i < n; i++) {
    int32_t y1 = (int32_t)((y16[i] * yg) >> 16), u = u8[i], v = v8[i];
    o[4 * i + ri] = (uint8_t)rc_clamp6(y1 + v * vr - rb);
    o[4 * i + 1] = (uint8_t)rc_clamp6(y1 + gb - (u * ug + v * vg));
    o[4 * i + bi] = (uint8_t)rc_clamp6(y1 + u * ub - bb);
    o[4 * i + 3] = a8[i];
  }
}

static av_always_inline void rc_yuv_row(const uint8_t *ys, const uint8_t *us, const uint8_t *vs,
                                        const uint8_t *as, uint8_t *o, int n, int xo, int hs,
                                        int cstep, int depth, int shift, const int32_t *k, int ri,
                                        int bi) {
  uint16_t y16[RC_CHUNK];
  uint8_t u8[RC_CHUNK], v8[RC_CHUNK], a8[RC_CHUNK];
  int bps = depth > 8 ? 2 : 1;
  if (!as) memset(a8, 255, sizeof(a8));
  for (int i = 0; i < n; i += RC_CHUNK) { // chunks start at even pixels, so chroma stays aligned
    int m = n - i < RC_CHUNK ? n - i : RC_CHUNK, c = ((i + xo) >> hs) * cstep * bps;
    rc_yuv_stage(ys + i * bps, us + c, vs + c, as ? as + i * bps : NULL, m, xo, hs, cstep, depth,
                 shift, y16, u8, v8, a8);
    rc_yuv_matrix(y16, u8, v8, a8, o + 4 * i, m, k, ri, bi);
  }
}

static av_always_inline void rc_rgb_row(const uint8_t *s, uint8_t *o, int n,
                                        int step, int sr, int sg, int sb, int sa, int ri, int bi) {
  for (int i = 0; i < n; i++, s += step, o += 4) {
    o[ri] = s[sr]; o[1] = s[sg]; o[bi] = s[sb]; o[3] = sa >= 0 ? s[sa] : 255;
  }
}

// Front end for one row: source -> 8-bit R'G'B'A in destination byte order.
static av_always_inline int rc_front_row(const AVFrame *f, const AVPixFmtDescriptor *d, int x, int y, int n,
                        const int32_t *k, uint8_t *o, int ri, int bi) {
  if (d->flags & AV_PIX_FMT_FLAG_RGB) {
    const uint8_t *s = f->data[0] + (ptrdiff_t)y * f->linesize[0];
    int step = d->comp[0].step, sa = d->nb_components == 4 ? d->comp[3].offset : -1;
    int sr = d->comp[0].offset, sg = d->comp[1].offset, sb = d->comp[2].offset;
    s += (ptrdiff_t)x * step;
    if (step == 4 && sa >= 0 && sr == ri && sb == bi && sg == 1 && sa == 3)
      memcpy(o, s, (size_t)n * 4);
    else if (step == 4 && sr == 2 && sg == 1 && sb == 0 && sa == 3 && ri == 0)
      rc_rgb_row(s, o, n, 4, 2, 1, 0, 3, 0, 2); // BGRA -> RGBA
    else if (step == 4 && sr == 0 && sg == 1 && sb == 2 && sa == 3 && ri == 2)
      rc_rgb_row(s, o, n, 4, 0, 1, 2, 3, 2, 0); // RGBA -> BGRA
    else
      rc_rgb_row(s, o, n, step, sr, sg, sb, sa, ri, bi);
    return 0;
  }
  int hs = d->log2_chroma_w, vs = d->log2_chroma_h, depth = d->comp[0].depth;
  int cstep = d->comp[1].step / (depth > 8 ? 2 : 1), cy = y >> vs, cx = x >> hs;
  const uint8_t *p[4];
  for (int c = 0; c < 4 && c < d->nb_components; c++) {
    int py = c == 1 || c == 2 ? cy : y, px = c == 1 || c == 2 ? cx * d->comp[c].step : x * d->comp[c].step;
    p[c] = f->data[d->comp[c].plane] + (ptrdiff_t)py * f->linesize[d->comp[c].plane] + px + d->comp[c].offset;
  }
  const uint8_t *a = d->nb_components == 4 ? p[3] : NULL;
  int xo = x & ((1 << hs) - 1), sh = d->comp[0].shift;
  if (depth == 8 && hs == 1 && cstep == 1) rc_yuv_row(p[0], p[1], p[2], a, o, n, xo, 1, 1, 8, 0, k, ri, bi);
  else if (depth == 8 && hs == 1 && cstep == 2) rc_yuv_row(p[0], p[1], p[2], a, o, n, xo, 1, 2, 8, 0, k, ri, bi);
  else if (depth == 8) rc_yuv_row(p[0], p[1], p[2], a, o, n, xo, hs, cstep, 8, 0, k, ri, bi);
  else if (depth == 10 && hs == 1 && cstep == 1) rc_yuv_row(p[0], p[1], p[2], a, o, n, xo, 1, 1, 10, 0, k, ri, bi);
  else rc_yuv_row(p[0], p[1], p[2], a, o, n, xo, hs, cstep, depth, sh, k, ri, bi);
  return 0;
}

// ri/bi are literals at both call sites so every row loop is specialised per byte order.
static av_always_inline void rc_rows(const RcState *t, const AVFrame *f, const AVPixFmtDescriptor *d,
                                     int x, int y, int w, int h, const int32_t *k, uint8_t *dst,
                                     ptrdiff_t dst_stride, int ri, int bi) {
  for (int r = 0; r < h; r++) {
    uint8_t *o = dst + (ptrdiff_t)r * dst_stride;
    rc_front_row(f, d, x, y + r, w, k, o, ri, bi);
    if (t->cls != RC_IDENTITY) rc_color_row(t, o, w, ri, bi);
  }
}

// Planar/semi-planar YUV (8..16 bit LE, 1-byte or 2-byte samples) or packed 8-bit RGB.
static int rc_direct(const AVPixFmtDescriptor *d) {
  if (d->flags & (AV_PIX_FMT_FLAG_BE | AV_PIX_FMT_FLAG_PAL | AV_PIX_FMT_FLAG_HWACCEL |
                  AV_PIX_FMT_FLAG_BITSTREAM | AV_PIX_FMT_FLAG_FLOAT | AV_PIX_FMT_FLAG_BAYER))
    return 0;
  if (d->flags & AV_PIX_FMT_FLAG_RGB)
    return !(d->flags & AV_PIX_FMT_FLAG_PLANAR) && d->comp[0].depth == 8 && d->nb_components >= 3 &&
           d->comp[0].plane == 0 && (d->comp[0].step == 3 || d->comp[0].step == 4);
  if (d->nb_components < 3 || d->comp[0].plane != 0 || d->comp[1].plane == 0) return 0;
  return d->comp[0].depth == 8 ? d->comp[0].step == 1 : d->comp[0].step == 2;
}

int ffframe_copy_rect_to_rgb(const AVFrame *src, int x, int y, int w, int h, int pri, int trc,
                             int mat, int range, int target_p3, int dst_bgr, uint8_t *dst,
                             ptrdiff_t dst_stride) {
  if (!src || !dst || x < 0 || y < 0 || w <= 0 || h <= 0 || x > src->width - w ||
      y > src->height - h || dst_stride < (ptrdiff_t)w * 4)
    return AVERROR(EINVAL);
  if (pri < 0) pri = src->color_primaries;
  if (trc < 0) trc = src->color_trc;
  if (mat < 0) mat = src->colorspace;
  if (range < 0) range = src->color_range;
  const AVPixFmtDescriptor *d = av_pix_fmt_desc_get((enum AVPixelFormat)src->format);
  if (!d) return AVERROR(EINVAL);
  int direct = rc_direct(d);
  // GPU frames and formats swscale cannot read are not supported (checked before any allocation).
  if (!direct && ((d->flags & AV_PIX_FMT_FLAG_HWACCEL) ||
                  !sws_isSupportedInput((enum AVPixelFormat)src->format)))
    return AVERROR(ENOSYS);
  if (range == AVCOL_RANGE_UNSPECIFIED && (src->format == AV_PIX_FMT_YUVJ420P ||
      src->format == AV_PIX_FMT_YUVJ422P || src->format == AV_PIX_FMT_YUVJ444P))
    range = AVCOL_RANGE_JPEG;
  RcState *t = av_malloc(sizeof(*t)); // ~37 KB: kept off the (blocking pool) thread stack
  if (!t) return AVERROR(ENOMEM);
  int ret = 0;
  int yuv = rc_setup(t, pri, trc, mat, range, target_p3);
  const AVFrame *f = src;
  AVFrame *tmp = NULL;
  if (!direct) { // rare formats (pal8, gray, rgb48, ...): whole frame to RGBA with swscale
    struct SwsContext *s = sws_getContext(src->width, src->height, (enum AVPixelFormat)src->format,
                                          src->width, src->height, AV_PIX_FMT_RGBA, SWS_POINT,
                                          NULL, NULL, NULL);
    tmp = av_frame_alloc();
    ret = !s || !tmp ? AVERROR(ENOMEM) : 0;
    if (!ret) {
      int cs = mat == AVCOL_SPC_BT709 ? SWS_CS_ITU709
             : mat == AVCOL_SPC_BT2020_NCL ? SWS_CS_BT2020
             : SWS_CS_DEFAULT;
      sws_setColorspaceDetails(s, sws_getCoefficients(cs), range == AVCOL_RANGE_JPEG,
                               sws_getCoefficients(SWS_CS_DEFAULT), 1, 0, 1 << 16, 1 << 16);
      tmp->format = AV_PIX_FMT_RGBA;
      tmp->width = src->width;
      tmp->height = src->height;
      ret = av_frame_get_buffer(tmp, 0);
    }
    if (!ret)
      ret = sws_scale(s, (const uint8_t *const *)src->data, src->linesize, 0, src->height,
                      tmp->data, tmp->linesize);
    sws_freeContext(s);
    if (ret < 0) goto end;
    ret = 0;
    f = tmp;
    d = av_pix_fmt_desc_get(AV_PIX_FMT_RGBA);
  }
  if (!dst_bgr) rc_rows(t, f, d, x, y, w, h, kRcYuv[yuv], dst, dst_stride, 0, 2);
  else rc_rows(t, f, d, x, y, w, h, kRcYuv[yuv], dst, dst_stride, 2, 0);
end:
  av_frame_free(&tmp);
  av_free(t);
  return ret;
}
