/*
 * A live MPEG-TS feed remuxed for a browser, as it arrives: the video copied
 * packet for packet (never re-encoded), one chosen audio stream copied or
 * encoded to AAC, everything else (other audio, subtitles, data) left out.
 * MPEG-TS out, which the page's player reads.
 *
 * Bytes come in through `read` and go out through `write`, both Rust's
 * (src/recording/livemux.rs); this runs on a thread of its own until the
 * input ends or the output is refused.
 *
 * Encoding: the decoded audio is turned into stereo float at the input's
 * rate (5.1 is mixed down: a browser on a laptop or a phone plays two
 * channels), queued, and handed to FFmpeg's own AAC encoder (LGPL) in the
 * 1024-sample frames it takes. The encoded audio's timestamps follow the
 * input's: they are taken from the first decoded frame, counted on from
 * there by samples, and taken afresh whenever the input jumps by more than
 * a second (a feed that restarts its clock).
 *
 * Output starts at the first video keyframe, so the browser's decoder is
 * never handed a picture it has no reference for.
 *
 * Timestamps. A feed's clock can jump (an encoder restarting, a server
 * looping its file), which a browser's player and the muxer both refuse. So
 * the timeline is kept continuous as in remux.c: wherever a packet jumps
 * backwards by more than a second or forwards by more than five, everything
 * from there is moved to follow on just after the latest timestamp so far.
 * A packet that would still go backwards on its own stream is dropped.
 */

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/audio_fifo.h>
#include <libavutil/channel_layout.h>
#include <libavutil/mathematics.h>
#include <libavutil/opt.h>
#include <libswresample/swresample.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef int (*zl_read_fn)(void *opaque, uint8_t *buf, int size);
typedef int (*zl_write_fn)(void *opaque, const uint8_t *buf, int size);

#define IN_BUF (64 * 1024)
#define OUT_BUF (32 * 1024)

#define JUMP_BACK_US (1 * 1000000LL)
#define JUMP_FWD_US (5 * 1000000LL)
#define JOIN_GAP_US (40 * 1000LL)

static const AVRational US = {1, 1000000};

/* The output's timeline: how far the input's has been moved, and the
 * latest timestamp so far. */
typedef struct {
    int64_t offset_us;
    int64_t last_us;
} timeline;

/* Moves `pkt` (in time base `tb`) onto the continuous timeline. */
static void continuous(timeline *t, AVPacket *pkt, AVRational tb) {
    int64_t ts = pkt->dts != AV_NOPTS_VALUE ? pkt->dts : pkt->pts;
    int64_t us = av_rescale_q(ts, tb, US) + t->offset_us;
    if (t->last_us != AV_NOPTS_VALUE &&
        (us < t->last_us - JUMP_BACK_US || us > t->last_us + JUMP_FWD_US)) {
        t->offset_us += t->last_us + JOIN_GAP_US - us;
        us = t->last_us + JOIN_GAP_US;
    }
    if (t->last_us == AV_NOPTS_VALUE || us > t->last_us)
        t->last_us = us;
    int64_t off = av_rescale_q(t->offset_us, US, tb);
    if (pkt->pts != AV_NOPTS_VALUE)
        pkt->pts += off;
    if (pkt->dts != AV_NOPTS_VALUE)
        pkt->dts += off;
}

/* Writes a packet, unless it would go backwards on its stream. `last` is
 * each output stream's latest dts. */
static int put(AVFormatContext *out, AVPacket *pkt, int64_t *last) {
    int i = pkt->stream_index;
    if (pkt->dts != AV_NOPTS_VALUE) {
        if (last[i] != AV_NOPTS_VALUE && pkt->dts <= last[i]) {
            av_packet_unref(pkt);
            return 0;
        }
        last[i] = pkt->dts;
    }
    return av_interleaved_write_frame(out, pkt);
}

typedef struct {
    zl_read_fn read;
    zl_write_fn write;
    void *opaque;
} io;

static int read_cb(void *o, uint8_t *buf, int size) {
    io *x = o;
    int n = x->read(x->opaque, buf, size);
    return n > 0 ? n : AVERROR_EOF;
}

static int write_cb(void *o, const uint8_t *buf, int size) {
    io *x = o;
    return x->write(x->opaque, buf, size) < 0 ? AVERROR_EXIT : size;
}

static void say(char *err, size_t len, const char *what, int code) {
    char buf[AV_ERROR_MAX_STRING_SIZE] = {0};
    if (code < 0)
        av_strerror(code, buf, sizeof buf);
    snprintf(err, len, "%s%s%s", what, code < 0 ? ": " : "", buf);
}

int zl_can_encode_aac(void) {
    return avcodec_find_encoder(AV_CODEC_ID_AAC) != NULL;
}

/* The audio being encoded. */
typedef struct {
    AVCodecContext *dec;
    AVCodecContext *enc;
    SwrContext *swr;
    AVAudioFifo *fifo;
    AVFrame *frame;   /* decoded */
    AVFrame *conv;    /* converted, before the queue */
    AVFrame *chunk;   /* one encoder frame, out of the queue */
    AVPacket *pkt;
    /* What the converter was set up for. */
    int swr_rate;
    int swr_fmt;
    AVChannelLayout swr_layout;
    /* The next encoder frame's timestamp, in the encoder's time base. */
    int64_t next_pts;
    AVRational in_tb;
    int out_index;
    int64_t *last;
} encoder;

static void close_encoder(encoder *e) {
    avcodec_free_context(&e->dec);
    avcodec_free_context(&e->enc);
    swr_free(&e->swr);
    if (e->fifo)
        av_audio_fifo_free(e->fifo);
    e->fifo = NULL;
    av_frame_free(&e->frame);
    av_frame_free(&e->conv);
    av_frame_free(&e->chunk);
    av_packet_free(&e->pkt);
    av_channel_layout_uninit(&e->swr_layout);
}

static int open_encoder(encoder *e, const AVStream *in, char *err, size_t errlen) {
    memset(e, 0, sizeof *e);
    e->next_pts = AV_NOPTS_VALUE;
    e->in_tb = in->time_base;
    const AVCodec *dc = avcodec_find_decoder(in->codecpar->codec_id);
    const AVCodec *ec = avcodec_find_encoder(AV_CODEC_ID_AAC);
    if (!dc || !ec) {
        say(err, errlen, dc ? "no AAC encoder in this FFmpeg" : "no decoder for this audio", 0);
        return AVERROR_DECODER_NOT_FOUND;
    }
    e->dec = avcodec_alloc_context3(dc);
    e->enc = avcodec_alloc_context3(ec);
    e->frame = av_frame_alloc();
    e->conv = av_frame_alloc();
    e->chunk = av_frame_alloc();
    e->pkt = av_packet_alloc();
    if (!e->dec || !e->enc || !e->frame || !e->conv || !e->chunk || !e->pkt) {
        say(err, errlen, "out of memory", 0);
        return AVERROR(ENOMEM);
    }
    int r = avcodec_parameters_to_context(e->dec, in->codecpar);
    if (r >= 0) {
        e->dec->pkt_timebase = in->time_base;
        r = avcodec_open2(e->dec, dc, NULL);
    }
    if (r < 0) {
        say(err, errlen, "opening the audio decoder", r);
        return r;
    }
    int rate = in->codecpar->sample_rate > 0 ? in->codecpar->sample_rate : 48000;
    e->enc->sample_rate = rate;
    e->enc->sample_fmt = AV_SAMPLE_FMT_FLTP;
    av_channel_layout_default(&e->enc->ch_layout, 2);
    e->enc->bit_rate = 160000;
    e->enc->time_base = (AVRational){1, rate};
    r = avcodec_open2(e->enc, ec, NULL);
    if (r < 0) {
        say(err, errlen, "opening the AAC encoder", r);
        return r;
    }
    e->fifo = av_audio_fifo_alloc(e->enc->sample_fmt, e->enc->ch_layout.nb_channels,
                                  e->enc->frame_size * 4);
    if (!e->fifo) {
        say(err, errlen, "out of memory", 0);
        return AVERROR(ENOMEM);
    }
    return 0;
}

/* Sets the converter up for `f`'s format, unless it already is. */
static int converter_for(encoder *e, const AVFrame *f) {
    if (e->swr && e->swr_rate == f->sample_rate && e->swr_fmt == f->format &&
        !av_channel_layout_compare(&e->swr_layout, &f->ch_layout))
        return 0;
    swr_free(&e->swr);
    int r = swr_alloc_set_opts2(&e->swr, &e->enc->ch_layout, e->enc->sample_fmt,
                                e->enc->sample_rate, &f->ch_layout, f->format, f->sample_rate,
                                0, NULL);
    if (r < 0)
        return r;
    r = swr_init(e->swr);
    if (r < 0)
        return r;
    e->swr_rate = f->sample_rate;
    e->swr_fmt = f->format;
    av_channel_layout_uninit(&e->swr_layout);
    return av_channel_layout_copy(&e->swr_layout, &f->ch_layout);
}

/* Encodes whatever whole frames the queue holds, or with `flush` the rest
 * too, and writes the packets. */
static int drain_queue(encoder *e, AVFormatContext *out, int flush) {
    int size = e->enc->frame_size;
    for (;;) {
        int have = av_audio_fifo_size(e->fifo);
        if (have <= 0 || (have < size && !flush))
            break;
        int n = have < size ? have : size;
        av_frame_unref(e->chunk);
        e->chunk->nb_samples = n;
        e->chunk->format = e->enc->sample_fmt;
        e->chunk->sample_rate = e->enc->sample_rate;
        int r = av_channel_layout_copy(&e->chunk->ch_layout, &e->enc->ch_layout);
        if (r >= 0)
            r = av_frame_get_buffer(e->chunk, 0);
        if (r < 0)
            return r;
        if (av_audio_fifo_read(e->fifo, (void **)e->chunk->data, n) < n)
            return AVERROR_BUG;
        e->chunk->pts = e->next_pts;
        e->next_pts += n;
        r = avcodec_send_frame(e->enc, e->chunk);
        if (r < 0)
            return r;
        while ((r = avcodec_receive_packet(e->enc, e->pkt)) >= 0) {
            e->pkt->stream_index = e->out_index;
            av_packet_rescale_ts(e->pkt, e->enc->time_base, out->streams[e->out_index]->time_base);
            r = put(out, e->pkt, e->last);
            if (r < 0)
                return r;
        }
        if (r != AVERROR(EAGAIN) && r != AVERROR_EOF)
            return r;
    }
    return 0;
}

/* One audio packet in: decoded, converted, queued, encoded. A packet the
 * decoder can't make sense of (a damaged one, as a live feed has) is let
 * go. */
static int encode_packet(encoder *e, AVFormatContext *out, AVPacket *pkt) {
    int r = avcodec_send_packet(e->dec, pkt);
    if (r < 0 && r != AVERROR(EAGAIN))
        return r == AVERROR_INVALIDDATA ? 0 : r;
    while ((r = avcodec_receive_frame(e->dec, e->frame)) >= 0) {
        AVFrame *f = e->frame;
        int64_t ts = f->best_effort_timestamp;
        if (ts != AV_NOPTS_VALUE) {
            int64_t at = av_rescale_q(ts, e->in_tb, e->enc->time_base);
            int64_t expected = e->next_pts == AV_NOPTS_VALUE
                                   ? AV_NOPTS_VALUE
                                   : e->next_pts + av_audio_fifo_size(e->fifo);
            if (expected == AV_NOPTS_VALUE || llabs(at - expected) > e->enc->sample_rate) {
                av_audio_fifo_reset(e->fifo);
                e->next_pts = at;
            }
        }
        if (e->next_pts == AV_NOPTS_VALUE) {
            av_frame_unref(f);
            continue;
        }
        r = converter_for(e, f);
        if (r < 0)
            return r;
        int room = swr_get_out_samples(e->swr, f->nb_samples);
        av_frame_unref(e->conv);
        e->conv->nb_samples = room;
        e->conv->format = e->enc->sample_fmt;
        e->conv->sample_rate = e->enc->sample_rate;
        r = av_channel_layout_copy(&e->conv->ch_layout, &e->enc->ch_layout);
        if (r >= 0)
            r = av_frame_get_buffer(e->conv, 0);
        if (r < 0)
            return r;
        int n = swr_convert(e->swr, e->conv->data, room, (const uint8_t **)f->extended_data,
                            f->nb_samples);
        av_frame_unref(f);
        if (n < 0)
            return n;
        if (n > 0 && av_audio_fifo_write(e->fifo, (void **)e->conv->data, n) < n)
            return AVERROR(ENOMEM);
        r = drain_queue(e, out, 0);
        if (r < 0)
            return r;
    }
    return r == AVERROR(EAGAIN) || r == AVERROR_EOF ? 0 : r;
}

/* Runs until the input ends (0) or something fails (an AVERROR, said in
 * `err`; the output refusing is AVERROR_EXIT). `audio_pid` is the input
 * stream (by its PID) to keep, or -1 for none; `encode` turns it into AAC. */
int zl_run(zl_read_fn read, zl_write_fn write, void *opaque, int audio_pid, int encode, char *err,
           size_t errlen) {
    io x = {read, write, opaque};
    /* The encoder's statistics and a live feed's every damaged packet are
     * not worth a line each on the service's console. */
    if (av_log_get_level() > AV_LOG_FATAL)
        av_log_set_level(AV_LOG_FATAL);
    AVFormatContext *in = NULL, *out = NULL;
    AVIOContext *in_io = NULL, *out_io = NULL;
    AVPacket *pkt = av_packet_alloc();
    encoder enc;
    int have_enc = 0, header = 0;
    int *map = NULL;
    int r = 0;
    memset(&enc, 0, sizeof enc);

    uint8_t *ibuf = av_malloc(IN_BUF), *obuf = av_malloc(OUT_BUF);
    in = avformat_alloc_context();
    if (!pkt || !ibuf || !obuf || !in) {
        av_free(ibuf);
        av_free(obuf);
        say(err, errlen, "out of memory", 0);
        r = AVERROR(ENOMEM);
        goto done;
    }
    in_io = avio_alloc_context(ibuf, IN_BUF, 0, &x, read_cb, NULL, NULL);
    if (!in_io) {
        av_free(ibuf);
        av_free(obuf);
        r = AVERROR(ENOMEM);
        say(err, errlen, "out of memory", 0);
        goto done;
    }
    in->pb = in_io;
    /* A second or two: enough to see the audio's first frame (its rate and
     * channels), and as long as the picture waits to start. */
    in->probesize = 2 * 1000 * 1000;
    in->max_analyze_duration = 2 * AV_TIME_BASE;
    r = avformat_open_input(&in, NULL, av_find_input_format("mpegts"), NULL);
    if (r < 0) {
        av_free(obuf);
        say(err, errlen, "reading the stream", r);
        goto done;
    }
    r = avformat_find_stream_info(in, NULL);
    if (r < 0) {
        av_free(obuf);
        say(err, errlen, "reading the stream's contents", r);
        goto done;
    }

    r = avformat_alloc_output_context2(&out, NULL, "mpegts", NULL);
    if (r < 0 || !out) {
        av_free(obuf);
        say(err, errlen, "setting up the output", r);
        goto done;
    }
    out_io = avio_alloc_context(obuf, OUT_BUF, 1, &x, NULL, write_cb, NULL);
    if (!out_io) {
        av_free(obuf);
        r = AVERROR(ENOMEM);
        say(err, errlen, "out of memory", 0);
        goto done;
    }
    out->pb = out_io;
    out->flags |= AVFMT_FLAG_CUSTOM_IO;
    /* Nothing is held back waiting for a stream that goes quiet. */
    out->max_interleave_delta = AV_TIME_BASE;

    map = av_calloc(in->nb_streams, sizeof *map);
    if (!map) {
        r = AVERROR(ENOMEM);
        say(err, errlen, "out of memory", 0);
        goto done;
    }
    int video = -1, audio = -1;
    for (unsigned s = 0; s < in->nb_streams; s++) {
        AVStream *st = in->streams[s];
        map[s] = -1;
        if (st->codecpar->codec_type == AVMEDIA_TYPE_VIDEO && video < 0 &&
            st->codecpar->codec_id != AV_CODEC_ID_NONE)
            video = (int)s;
        if (st->codecpar->codec_type == AVMEDIA_TYPE_AUDIO && audio < 0 && audio_pid >= 0 &&
            st->id == audio_pid)
            audio = (int)s;
    }
    if (video < 0 && audio < 0) {
        r = AVERROR_STREAM_NOT_FOUND;
        say(err, errlen, "the stream has nothing to send", 0);
        goto done;
    }
    if (video >= 0) {
        AVStream *o = avformat_new_stream(out, NULL);
        if (!o || (r = avcodec_parameters_copy(o->codecpar, in->streams[video]->codecpar)) < 0) {
            say(err, errlen, "setting up the video", r);
            r = r < 0 ? r : AVERROR(ENOMEM);
            goto done;
        }
        o->codecpar->codec_tag = 0;
        o->time_base = in->streams[video]->time_base;
        map[video] = o->index;
    }
    if (audio >= 0) {
        AVStream *o = avformat_new_stream(out, NULL);
        if (!o) {
            r = AVERROR(ENOMEM);
            say(err, errlen, "out of memory", 0);
            goto done;
        }
        if (encode) {
            r = open_encoder(&enc, in->streams[audio], err, errlen);
            have_enc = 1;
            if (r < 0)
                goto done;
            r = avcodec_parameters_from_context(o->codecpar, enc.enc);
            o->time_base = enc.enc->time_base;
            enc.out_index = o->index;
        } else {
            r = avcodec_parameters_copy(o->codecpar, in->streams[audio]->codecpar);
            o->codecpar->codec_tag = 0;
            o->time_base = in->streams[audio]->time_base;
        }
        if (r < 0) {
            say(err, errlen, "setting up the audio", r);
            goto done;
        }
        map[audio] = o->index;
    }
    r = avformat_write_header(out, NULL);
    if (r < 0) {
        say(err, errlen, "starting the output", r);
        goto done;
    }
    header = 1;

    int64_t last[2] = {AV_NOPTS_VALUE, AV_NOPTS_VALUE};
    timeline line = {0, AV_NOPTS_VALUE};
    enc.last = last;
    int started = video < 0;
    while ((r = av_read_frame(in, pkt)) >= 0) {
        int s = pkt->stream_index;
        if (s < 0 || (unsigned)s >= in->nb_streams || map[s] < 0 ||
            (pkt->dts == AV_NOPTS_VALUE && pkt->pts == AV_NOPTS_VALUE)) {
            av_packet_unref(pkt);
            continue;
        }
        if (!started) {
            if (s != video || !(pkt->flags & AV_PKT_FLAG_KEY)) {
                av_packet_unref(pkt);
                continue;
            }
            started = 1;
        }
        continuous(&line, pkt, in->streams[s]->time_base);
        if (s == audio && have_enc) {
            r = encode_packet(&enc, out, pkt);
            av_packet_unref(pkt);
        } else {
            AVStream *o = out->streams[map[s]];
            av_packet_rescale_ts(pkt, in->streams[s]->time_base, o->time_base);
            pkt->stream_index = map[s];
            pkt->pos = -1;
            r = put(out, pkt, last);
        }
        if (r < 0) {
            say(err, errlen, r == AVERROR_EXIT ? "the browser left" : "remuxing the stream", r);
            goto done;
        }
    }
    r = r == AVERROR_EOF ? 0 : r;
    if (r < 0)
        say(err, errlen, "reading the stream", r);
    else if (have_enc)
        drain_queue(&enc, out, 1);

done:
    if (header)
        av_write_trailer(out);
    if (have_enc)
        close_encoder(&enc);
    av_free(map);
    av_packet_free(&pkt);
    if (out) {
        if (out_io) {
            av_freep(&out_io->buffer);
            avio_context_free(&out_io);
        }
        avformat_free_context(out);
    }
    if (in)
        avformat_close_input(&in);
    if (in_io) {
        av_freep(&in_io->buffer);
        avio_context_free(&in_io);
    }
    return r;
}
