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

#include <libavformat/avformat.h>

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
