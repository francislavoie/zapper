/*
 * A movie file remuxed for a browser, from any point in it: the video
 * copied packet for packet (never re-encoded), one chosen audio stream
 * copied or encoded to stereo AAC, MPEG-TS out, which the page's player
 * reads. The file's text subtitles are not in the output; they are decoded
 * as the file is read and handed to the caller as cues, so a browser gets
 * them from the same single read of the provider's file.
 *
 * The file comes in through `read` and `seek`, and the output goes out
 * through `write`, all Rust's (src/recording/vodmux.rs); this runs on a
 * thread of its own until the file ends or the output is refused. A movie
 * is seekable (MP4 keeps its index at the end; Matroska's is read to find
 * a time), so unlike livemux.c's feed it is opened with a seek callback, and
 * playing from the middle seeks there instead of reading everything before.
 *
 * Once the streams are known, `plan` is asked what to keep (the audio
 * stream, and whether to encode it); it may refuse, and then nothing is
 * written.
 *
 * Output starts at the first video keyframe at or after where the seek
 * lands (what precedes the keyframe is dropped), so the browser's decoder
 * is never handed a picture it has no reference for.
 *
 * Timestamps are rebased so the output starts at 0: a player given a
 * stream that begins an hour in would wait an hour on its own timeline, and
 * the page needs a start it can add the requested time to. The base is the
 * first kept keyframe's decode time, so no timestamp is negative. A movie
 * does not jump around the way a live feed does, so a single offset is all
 * it takes; a packet that would still go backwards on its stream is
 * dropped. Subtitle cues are on the same timeline.
 *
 * Encoding is as in livemux.c: decoded, mixed to stereo float at the
 * input's rate, queued and handed to FFmpeg's own AAC encoder (LGPL) in
 * the 1024-sample frames it takes.
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

typedef int (*zv_read_fn)(void *opaque, uint8_t *buf, int size);
typedef int (*zv_write_fn)(void *opaque, const uint8_t *buf, int size);
typedef int64_t (*zv_seek_fn)(void *opaque, int64_t pos);
typedef int64_t (*zv_size_fn)(void *opaque);
/* One stream of the file: kind 0 audio (`n` its channels), 1 text subtitles,
 * 2 bitmap subtitles; `index` is its number in the file. */
typedef void (*zv_stream_fn)(void *opaque, int kind, int index, const char *codec, const char *lang,
                             int n);
/* Called once with the streams listed: duration in seconds (negative when
 * unknown) and the video's codec ("" without). Sets the audio stream to keep
 * (-1 for none) and whether to encode it; returns non-zero to refuse. */
typedef int (*zv_plan_fn)(void *opaque, double duration, const char *video, int *audio,
                          int *encode);
typedef void (*zv_cue_fn)(void *opaque, int track, int64_t start_us, int64_t end_us,
                          const char *text);

#define IN_BUF (256 * 1024)
#define OUT_BUF (32 * 1024)
#define MAX_WAITING 1000
#define ZV_REFUSED 1

static const AVRational US = {1, 1000000};

typedef struct {
    zv_read_fn read;
    zv_write_fn write;
    zv_seek_fn seek;
    zv_size_fn size;
    void *opaque;
} io;

static int read_cb(void *o, uint8_t *buf, int size) {
    io *x = o;
    int n = x->read(x->opaque, buf, size);
    return n > 0 ? n : n == 0 ? AVERROR_EOF : AVERROR(EIO);
}

static int write_cb(void *o, const uint8_t *buf, int size) {
    io *x = o;
    return x->write(x->opaque, buf, size) < 0 ? AVERROR_EXIT : size;
}

static int64_t seek_cb(void *o, int64_t off, int whence) {
    io *x = o;
    whence &= ~AVSEEK_FORCE;
    if (whence == AVSEEK_SIZE)
        return x->size(x->opaque);
    if (whence == SEEK_SET)
        return x->seek(x->opaque, off);
    if (whence == SEEK_END) {
        int64_t size = x->size(x->opaque);
        return size < 0 || size + off < 0 ? -1 : x->seek(x->opaque, size + off);
    }
    return -1; /* SEEK_CUR never reaches here: libavformat turns it into SEEK_SET */
}

static void say(char *err, size_t len, const char *what, int code) {
    char buf[AV_ERROR_MAX_STRING_SIZE] = {0};
    if (code < 0)
        av_strerror(code, buf, sizeof buf);
    snprintf(err, len, "%s%s%s", what, code < 0 ? ": " : "", buf);
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

/* Moves a packet's timestamps back by `base_us`. */
static void rebase(AVPacket *pkt, AVRational tb, int64_t base_us) {
    int64_t off = av_rescale_q(base_us, US, tb);
    if (pkt->pts != AV_NOPTS_VALUE)
        pkt->pts -= off;
    if (pkt->dts != AV_NOPTS_VALUE)
        pkt->dts -= off;
}

/* ---- text subtitles ---- */

static int is_text_sub(enum AVCodecID id) {
    return id == AV_CODEC_ID_SUBRIP || id == AV_CODEC_ID_ASS || id == AV_CODEC_ID_SSA ||
           id == AV_CODEC_ID_MOV_TEXT || id == AV_CODEC_ID_WEBVTT;
}

/* Appends to `out` (length `len`, capacity `cap`, NUL kept). */
static size_t add(char *out, size_t len, size_t cap, const char *s) {
    size_t n = strlen(s);
    if (len + n + 1 >= cap)
        return len;
    memcpy(out + len, s, n + 1);
    return len + n;
}

/* The text of a decoded event (an ASS line: ReadOrder, Layer, Style, Name,
 * MarginL, MarginR, MarginV, Effect, Text) as the payload of a WebVTT cue:
 * styling dropped except bold, italic and underline, line breaks kept, the
 * characters WebVTT reserves escaped. Appended to `out`; returns the new
 * length. */
static size_t ass_text(const char *ass, char *out, size_t len, size_t cap) {
    int commas = 0;
    const char *p = ass;
    while (*p && commas < 8) {
        if (*p++ == ',')
            commas++;
    }
    if (commas < 8)
        p = ass;
    int open[3] = {0, 0, 0};
    const char tag[3] = {'b', 'i', 'u'};
    for (; *p; p++) {
        char s[8];
        if (*p == '{') {
            const char *e = strchr(p, '}');
            if (!e)
                break;
            for (const char *q = p; q + 2 < e; q++) {
                if (*q != '\\')
                    continue;
                for (int t = 0; t < 3; t++) {
                    int on = q[2] == '1';
                    if (q[1] == tag[t] && (q[2] == '0' || on) && on != open[t]) {
                        open[t] = on;
                        snprintf(s, sizeof s, on ? "<%c>" : "</%c>", tag[t]);
                        len = add(out, len, cap, s);
                    }
                }
            }
            p = e;
        } else if (*p == '\\' && (p[1] == 'N' || p[1] == 'n')) {
            if (len > 0 && out[len - 1] != '\n')
                len = add(out, len, cap, "\n");
            p++;
        } else if (*p == '\\' && p[1] == 'h') {
            len = add(out, len, cap, " ");
            p++;
        } else if (*p == '\r' || *p == '\n') {
            if (len > 0 && out[len - 1] != '\n')
                len = add(out, len, cap, "\n");
        } else if (*p == '&') {
            len = add(out, len, cap, "&amp;");
        } else if (*p == '<') {
            len = add(out, len, cap, "&lt;");
        } else if (*p == '>') {
            len = add(out, len, cap, "&gt;");
        } else {
            s[0] = *p;
            s[1] = 0;
            len = add(out, len, cap, s);
        }
    }
    for (int t = 2; t >= 0; t--) {
        if (open[t]) {
            char s[8];
            snprintf(s, sizeof s, "</%c>", tag[t]);
            len = add(out, len, cap, s);
        }
    }
    while (len > 0 && (out[len - 1] == '\n' || out[len - 1] == ' '))
        out[--len] = 0;
    return len;
}

typedef struct {
    int track;
    int64_t start, end; /* the file's own time, microseconds */
    char *text;
} cue;

typedef struct {
    zv_cue_fn fn;
    void *opaque;
    /* The output's zero, once the first keyframe is written; until then
     * cues wait. */
    int64_t base;
    int have_base;
    cue *wait;
    int n_wait, cap_wait;
} cues;

static void send_cue(cues *c, int track, int64_t start, int64_t end, const char *text) {
    start -= c->base;
    end -= c->base;
    if (end <= 0)
        return;
    c->fn(c->opaque, track, start < 0 ? 0 : start, end, text);
}

static void hold_cue(cues *c, int track, int64_t start, int64_t end, const char *text) {
    if (c->have_base) {
        send_cue(c, track, start, end, text);
        return;
    }
    if (c->n_wait >= MAX_WAITING)
        return;
    if (c->n_wait == c->cap_wait) {
        int cap = c->cap_wait ? c->cap_wait * 2 : 16;
        cue *w = realloc(c->wait, cap * sizeof *w);
        if (!w)
            return;
        c->wait = w;
        c->cap_wait = cap;
    }
    size_t n = strlen(text) + 1;
    char *copy = malloc(n);
    if (!copy)
        return;
    memcpy(copy, text, n);
    c->wait[c->n_wait++] = (cue){track, start, end, copy};
}

static void start_cues(cues *c, int64_t base) {
    c->base = base;
    c->have_base = 1;
    for (int i = 0; i < c->n_wait; i++) {
        send_cue(c, c->wait[i].track, c->wait[i].start, c->wait[i].end, c->wait[i].text);
        free(c->wait[i].text);
    }
    c->n_wait = 0;
}

static void free_cues(cues *c) {
    for (int i = 0; i < c->n_wait; i++)
        free(c->wait[i].text);
    free(c->wait);
}

/* One subtitle packet in: decoded, and its text passed on. A packet the
 * decoder can't read is let go. */
static void subtitle_packet(cues *c, AVCodecContext *dec, AVStream *st, AVPacket *pkt) {
    AVSubtitle sub;
    int got = 0;
    if (pkt->pts == AV_NOPTS_VALUE || avcodec_decode_subtitle2(dec, &sub, &got, pkt) < 0 || !got)
        return;
    int64_t at = av_rescale_q(pkt->pts, st->time_base, US);
    int64_t start = at + sub.start_display_time * 1000LL;
    int64_t end = 0;
    if (pkt->duration > 0)
        end = start + av_rescale_q(pkt->duration, st->time_base, US);
    else if (sub.end_display_time > sub.start_display_time && sub.end_display_time != UINT32_MAX)
        end = at + sub.end_display_time * 1000LL;
    char text[4096];
    size_t len = 0;
    text[0] = 0;
    for (unsigned i = 0; i < sub.num_rects; i++) {
        if (sub.rects[i]->type != SUBTITLE_ASS || !sub.rects[i]->ass)
            continue;
        if (len > 0)
            len = add(text, len, sizeof text, "\n");
        len = ass_text(sub.rects[i]->ass, text, len, sizeof text);
    }
    if (len > 0 && end > start)
        hold_cue(c, st->index, start, end, text);
    avsubtitle_free(&sub);
}

/* ---- audio encoding (as livemux.c) ---- */

typedef struct {
    AVCodecContext *dec;
    AVCodecContext *enc;
    SwrContext *swr;
    AVAudioFifo *fifo;
    AVFrame *frame; /* decoded */
    AVFrame *conv;  /* converted, before the queue */
    AVFrame *chunk; /* one encoder frame, out of the queue */
    AVPacket *pkt;
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
            /* The encoder's priming puts its first packet before zero. */
            if (e->pkt->pts < 0 || e->pkt->dts < 0) {
                av_packet_unref(e->pkt);
                continue;
            }
            r = put(out, e->pkt, e->last);
            if (r < 0)
                return r;
        }
        if (r != AVERROR(EAGAIN) && r != AVERROR_EOF)
            return r;
    }
    return 0;
}

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

static void lang_of(const AVStream *st, char *dst, size_t n) {
    AVDictionaryEntry *l = av_dict_get(st->metadata, "language", NULL, 0);
    snprintf(dst, n, "%s", l && l->value ? l->value : "");
}

/* Runs until the file ends (0), the plan refuses (ZV_REFUSED) or something
 * fails (an AVERROR, said in `err`; the output refusing is AVERROR_EXIT).
 * `from` is where to start, in seconds. */
int zv_run(zv_read_fn read, zv_seek_fn seek, zv_size_fn size, zv_write_fn write,
           zv_stream_fn stream_fn, zv_plan_fn plan_fn, zv_cue_fn cue_fn, void *opaque, double from,
           char *err, size_t errlen) {
    io x = {read, write, seek, size, opaque};
    if (av_log_get_level() > AV_LOG_FATAL)
        av_log_set_level(AV_LOG_FATAL);
    AVFormatContext *in = NULL, *out = NULL;
    AVIOContext *in_io = NULL, *out_io = NULL;
    AVPacket *pkt = av_packet_alloc();
    encoder enc;
    cues cq = {cue_fn, opaque, 0, 0, NULL, 0, 0};
    AVCodecContext **subs = NULL;
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
    in_io = avio_alloc_context(ibuf, IN_BUF, 0, &x, read_cb, NULL, seek_cb);
    if (!in_io) {
        av_free(ibuf);
        av_free(obuf);
        r = AVERROR(ENOMEM);
        say(err, errlen, "out of memory", 0);
        goto done;
    }
    in->pb = in_io;
    /* Enough to see every stream's format in a file whose audio is sparse. */
    in->probesize = 8 * 1000 * 1000;
    in->max_analyze_duration = 5 * AV_TIME_BASE;
    r = avformat_open_input(&in, NULL, NULL, NULL);
    if (r < 0) {
        av_free(obuf);
        say(err, errlen, "reading the file", r);
        goto done;
    }
    r = avformat_find_stream_info(in, NULL);
    if (r < 0) {
        av_free(obuf);
        say(err, errlen, "reading the file's contents", r);
        goto done;
    }

    int video = -1, audio = -1, encode = 0;
    for (unsigned s = 0; s < in->nb_streams; s++) {
        AVStream *st = in->streams[s];
        AVCodecParameters *cp = st->codecpar;
        char lang[16];
        lang_of(st, lang, sizeof lang);
        if (cp->codec_type == AVMEDIA_TYPE_VIDEO && video < 0 && cp->codec_id != AV_CODEC_ID_NONE &&
            !(st->disposition & AV_DISPOSITION_ATTACHED_PIC))
            video = (int)s;
        else if (cp->codec_type == AVMEDIA_TYPE_AUDIO)
            stream_fn(opaque, 0, (int)s, avcodec_get_name(cp->codec_id), lang,
                      cp->ch_layout.nb_channels);
        else if (cp->codec_type == AVMEDIA_TYPE_SUBTITLE)
            stream_fn(opaque, is_text_sub(cp->codec_id) ? 1 : 2, (int)s,
                      avcodec_get_name(cp->codec_id), lang, 0);
    }
    double duration = in->duration != AV_NOPTS_VALUE ? (double)in->duration / AV_TIME_BASE : -1;
    if (plan_fn(opaque, duration,
                video >= 0 ? avcodec_get_name(in->streams[video]->codecpar->codec_id) : "", &audio,
                &encode)) {
        av_free(obuf);
        r = ZV_REFUSED;
        goto done;
    }
    if (audio >= 0 && ((unsigned)audio >= in->nb_streams ||
                       in->streams[audio]->codecpar->codec_type != AVMEDIA_TYPE_AUDIO)) {
        av_free(obuf);
        r = AVERROR(EINVAL);
        say(err, errlen, "no such audio stream", 0);
        goto done;
    }
    if (video < 0 && audio < 0) {
        av_free(obuf);
        r = AVERROR_STREAM_NOT_FOUND;
        say(err, errlen, "the file has nothing to send", 0);
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
    out->max_interleave_delta = AV_TIME_BASE;

    map = av_calloc(in->nb_streams, sizeof *map);
    subs = av_calloc(in->nb_streams, sizeof *subs);
    if (!map || !subs) {
        r = AVERROR(ENOMEM);
        say(err, errlen, "out of memory", 0);
        goto done;
    }
    for (unsigned s = 0; s < in->nb_streams; s++) {
        map[s] = -1;
        AVStream *st = in->streams[s];
        if (st->codecpar->codec_type != AVMEDIA_TYPE_SUBTITLE ||
            !is_text_sub(st->codecpar->codec_id))
            continue;
        /* A track this FFmpeg can't decode is simply not offered. */
        const AVCodec *dc = avcodec_find_decoder(st->codecpar->codec_id);
        AVCodecContext *c = dc ? avcodec_alloc_context3(dc) : NULL;
        if (!c)
            continue;
        c->pkt_timebase = st->time_base;
        if (avcodec_parameters_to_context(c, st->codecpar) >= 0 && avcodec_open2(c, dc, NULL) >= 0)
            subs[s] = c;
        else
            avcodec_free_context(&c);
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

    int64_t from_us = (int64_t)(from * AV_TIME_BASE);
    if (from > 0) {
        int64_t ts = from_us;
        if (in->start_time != AV_NOPTS_VALUE)
            ts += in->start_time;
        r = avformat_seek_file(in, -1, INT64_MIN, ts, ts, AVSEEK_FLAG_BACKWARD);
        if (r < 0) {
            say(err, errlen, "seeking in the file", r);
            goto done;
        }
    }

    int64_t last[2] = {AV_NOPTS_VALUE, AV_NOPTS_VALUE};
    enc.last = last;
    int64_t base = 0;
    int started = 0;
    while ((r = av_read_frame(in, pkt)) >= 0) {
        int s = pkt->stream_index;
        if (s < 0 || (unsigned)s >= in->nb_streams) {
            av_packet_unref(pkt);
            continue;
        }
        if (subs[s]) {
            subtitle_packet(&cq, subs[s], in->streams[s], pkt);
            av_packet_unref(pkt);
            continue;
        }
        if (map[s] < 0 || (pkt->dts == AV_NOPTS_VALUE && pkt->pts == AV_NOPTS_VALUE)) {
            av_packet_unref(pkt);
            continue;
        }
        AVRational tb = in->streams[s]->time_base;
        int64_t at = av_rescale_q(pkt->pts != AV_NOPTS_VALUE ? pkt->pts : pkt->dts, tb, US);
        if (!started) {
            /* With a picture, its first keyframe; audio alone, its first packet. */
            if (video >= 0 ? !(s == video && (pkt->flags & AV_PKT_FLAG_KEY)) : at < from_us) {
                av_packet_unref(pkt);
                continue;
            }
            base = av_rescale_q(pkt->dts != AV_NOPTS_VALUE ? pkt->dts : pkt->pts, tb, US);
            started = 1;
            start_cues(&cq, base);
        }
        /* Audio a little ahead of the first keyframe would land before zero. */
        if (s != video && at < base) {
            av_packet_unref(pkt);
            continue;
        }
        rebase(pkt, tb, base);
        if (s == audio && have_enc) {
            r = encode_packet(&enc, out, pkt);
            av_packet_unref(pkt);
        } else {
            AVStream *o = out->streams[map[s]];
            av_packet_rescale_ts(pkt, tb, o->time_base);
            pkt->stream_index = map[s];
            pkt->pos = -1;
            r = put(out, pkt, last);
        }
        if (r < 0) {
            say(err, errlen, r == AVERROR_EXIT ? "the browser left" : "remuxing the file", r);
            goto done;
        }
    }
    r = r == AVERROR_EOF ? 0 : r;
    if (r < 0)
        say(err, errlen, "reading the file", r);
    else if (have_enc)
        drain_queue(&enc, out, 1);

done:
    if (header)
        av_write_trailer(out);
    if (have_enc)
        close_encoder(&enc);
    if (subs) {
        for (unsigned s = 0; in && s < in->nb_streams; s++)
            avcodec_free_context(&subs[s]);
        av_free(subs);
    }
    free_cues(&cq);
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
