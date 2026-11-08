/*
 * One picture from a short excerpt of a title, for the seek bar's preview:
 * src/surface/title_thumbs.rs asks mpv to write a tenth of a second of what
 * it has already buffered (`dump-cache`), which mpv writes as whole groups of
 * pictures from the keyframe at or before that moment, and this decodes that
 * keyframe, the first picture in the file, and shrinks it to the preview's
 * size. Nothing is fetched: the file is mpv's own buffer, on disk for the
 * moment it takes to read it.
 *
 * Cheap on purpose, because it runs beside playback: one decoder thread,
 * the deblocking filter skipped (invisible at 160x90, and a good part of an
 * H.264 decode), nothing past the first picture decoded or read.
 *
 * The picture keeps its shape inside the preview, centred on black, as mpv
 * shows it on screen and as a channel's previews are taken.
 *
 * `zapper_thumb_jpeg` compresses a preview, so a whole film's worth stays
 * a few megabytes; FFmpeg's own JPEG encoder (LGPL), as in the bundle.
 */

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libswscale/swscale.h>
#include <stdint.h>
#include <string.h>

/* Black, opaque. */
static void fill_black(uint8_t *rgba, int w, int h) {
    for (int i = 0; i < w * h; i++) {
        rgba[i * 4] = 0;
        rgba[i * 4 + 1] = 0;
        rgba[i * 4 + 2] = 0;
        rgba[i * 4 + 3] = 255;
    }
}

/* `frame` shrunk into `rgba` (tw x th, top-down), its shape kept. */
static int shrink(const AVFrame *frame, uint8_t *rgba, int tw, int th) {
    int64_t dw = frame->width, dh = frame->height;
    AVRational sar = frame->sample_aspect_ratio;
    if (sar.num > 0 && sar.den > 0)
        dw = dw * sar.num / sar.den;
    if (dw <= 0 || dh <= 0)
        return -1;
    int fw, fh;
    if (dw * th >= dh * tw) {
        fw = tw;
        fh = (int)(dh * tw / dw);
    } else {
        fw = (int)(dw * th / dh);
        fh = th;
    }
    if (fw < 1)
        fw = 1;
    if (fh < 1)
        fh = 1;
    if (fw > tw)
        fw = tw;
    if (fh > th)
        fh = th;
    struct SwsContext *sws =
        sws_getContext(frame->width, frame->height, (enum AVPixelFormat)frame->format, fw, fh,
                       AV_PIX_FMT_RGBA, SWS_AREA, NULL, NULL, NULL);
    if (!sws)
        return -1;
    fill_black(rgba, tw, th);
    uint8_t *dst[4] = {rgba + (((th - fh) / 2) * tw + (tw - fw) / 2) * 4, NULL, NULL, NULL};
    int stride[4] = {tw * 4, 0, 0, 0};
    int rows = sws_scale(sws, (const uint8_t *const *)frame->data, frame->linesize, 0,
                         frame->height, dst, stride);
    sws_freeContext(sws);
    return rows > 0 ? 0 : -1;
}

/*
 * Decodes the first picture of the video in `path` into `rgba` (tw x th,
 * RGBA, top-down). Returns 0, or a negative number when there is no
 * picture to give.
 */
int zapper_thumb_decode(const char *path, int tw, int th, uint8_t *rgba) {
    AVFormatContext *fmt = NULL;
    AVCodecContext *ctx = NULL;
    AVPacket *pkt = NULL;
    AVFrame *frame = NULL;
    int ret = -1, got = 0;

    if (avformat_open_input(&fmt, path, NULL, NULL) < 0)
        return -1;
    int vi = av_find_best_stream(fmt, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    if (vi < 0)
        goto out;
    AVStream *st = fmt->streams[vi];
    const AVCodec *dec = avcodec_find_decoder(st->codecpar->codec_id);
    if (!dec)
        goto out;
    ctx = avcodec_alloc_context3(dec);
    if (!ctx || avcodec_parameters_to_context(ctx, st->codecpar) < 0)
        goto out;
    ctx->thread_count = 1;
    ctx->skip_loop_filter = AVDISCARD_ALL;
    ctx->flags2 |= AV_CODEC_FLAG2_FAST;
    ctx->pkt_timebase = st->time_base;
    if (avcodec_open2(ctx, dec, NULL) < 0)
        goto out;
    pkt = av_packet_alloc();
    frame = av_frame_alloc();
    if (!pkt || !frame)
        goto out;

    while (!got && av_read_frame(fmt, pkt) >= 0) {
        if (pkt->stream_index == vi && avcodec_send_packet(ctx, pkt) >= 0 &&
            avcodec_receive_frame(ctx, frame) >= 0)
            got = 1;
        av_packet_unref(pkt);
    }
    /* A decoder that holds pictures back gives the first one when told the
     * excerpt has ended. */
    if (!got && avcodec_send_packet(ctx, NULL) >= 0 && avcodec_receive_frame(ctx, frame) >= 0)
        got = 1;
    if (got)
        ret = shrink(frame, rgba, tw, th);

out:
    av_frame_free(&frame);
    av_packet_free(&pkt);
    avcodec_free_context(&ctx);
    avformat_close_input(&fmt);
    return ret;
}

/*
 * `rgba` (w x h, top-down) as a JPEG in `out`. Returns its length, or a
 * negative number when it could not be made or would not fit in `cap`.
 */
int zapper_thumb_jpeg(const uint8_t *rgba, int w, int h, uint8_t *out, int cap) {
    const AVCodec *enc = avcodec_find_encoder(AV_CODEC_ID_MJPEG);
    AVCodecContext *ctx = NULL;
    AVFrame *frame = NULL;
    AVPacket *pkt = NULL;
    struct SwsContext *sws = NULL;
    int ret = -1;
    if (!enc)
        return -1;
    ctx = avcodec_alloc_context3(enc);
    frame = av_frame_alloc();
    pkt = av_packet_alloc();
    if (!ctx || !frame || !pkt)
        goto out;
    ctx->width = w;
    ctx->height = h;
    ctx->pix_fmt = AV_PIX_FMT_YUVJ420P;
    ctx->color_range = AVCOL_RANGE_JPEG;
    ctx->time_base = (AVRational){1, 25};
    ctx->thread_count = 1;
    /* A fixed quality: about 4KB for a busy picture at 160x90. */
    ctx->flags |= AV_CODEC_FLAG_QSCALE;
    ctx->global_quality = FF_QP2LAMBDA * 4;
    if (avcodec_open2(ctx, enc, NULL) < 0)
        goto out;
    frame->format = ctx->pix_fmt;
    frame->width = w;
    frame->height = h;
    frame->color_range = AVCOL_RANGE_JPEG;
    if (av_frame_get_buffer(frame, 0) < 0)
        goto out;
    sws = sws_getContext(w, h, AV_PIX_FMT_RGBA, w, h, ctx->pix_fmt, SWS_BICUBIC, NULL, NULL,
                         NULL);
    if (!sws)
        goto out;
    const uint8_t *src[4] = {rgba, NULL, NULL, NULL};
    int stride[4] = {w * 4, 0, 0, 0};
    if (sws_scale(sws, src, stride, 0, h, frame->data, frame->linesize) <= 0)
        goto out;
    frame->quality = ctx->global_quality;
    frame->pts = 0;
    if (avcodec_send_frame(ctx, frame) < 0 || avcodec_send_frame(ctx, NULL) < 0)
        goto out;
    if (avcodec_receive_packet(ctx, pkt) < 0)
        goto out;
    if (pkt->size <= cap) {
        memcpy(out, pkt->data, pkt->size);
        ret = pkt->size;
    }

out:
    sws_freeContext(sws);
    av_packet_free(&pkt);
    av_frame_free(&frame);
    avcodec_free_context(&ctx);
    return ret;
}
