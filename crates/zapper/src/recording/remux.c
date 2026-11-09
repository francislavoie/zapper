/*
 * The recording service's remuxer: a captured MPEG-TS part into Matroska,
 * copying every packet as it is (no decoding, no encoding), plus an optional
 * ASS subtitle stream given as ready-made cues.
 *
 * Only the work that needs FFmpeg's structures is here. Everything that is a
 * decision -- what the cues say, what the file is called, what to do when
 * this fails -- is Rust's (src/recording/remux.rs, clock.rs).
 *
 * Timestamps. A part may hold several connections' worth of one feed, each
 * starting its timestamps wherever the provider's encoder was, and a live
 * feed can jump on its own. So the output timeline is made continuous here:
 * it starts at zero, and wherever a video or audio packet jumps backwards or
 * more than five seconds forwards, the timeline is joined up just after the
 * latest timestamp so far. zr_scan and zr_remux apply exactly the same rule
 * to the same packets, so the scan's sample times are the output's.
 */

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libavutil/mathematics.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    int64_t pos;
    int64_t out_ms;
    int32_t disc;
} zr_sample;

typedef struct {
    int64_t start_ms;
    int64_t end_ms;
    const char *text;
} zr_cue;

#define US_PER_MS 1000
#define JUMP_BACK_US (1 * 1000000LL)
#define JUMP_FWD_US (5 * 1000000LL)
#define JOIN_GAP_US (40 * 1000LL)

static const AVRational US = {1, 1000000};
static const AVRational MS = {1, 1000};

static void say(char *err, size_t len, const char *what, int code) {
    char buf[AV_ERROR_MAX_STRING_SIZE] = {0};
    if (code < 0)
        av_strerror(code, buf, sizeof buf);
    snprintf(err, len, "%s%s%s", what, code < 0 ? ": " : "", buf);
}

/* An input opened, and which of its streams go to the output. */
typedef struct {
    AVFormatContext *in;
    int *map;       /* input index -> output index, or -1 */
    int nb_out;
    int reference;  /* input index of the clock's reference stream */
} input;

static void close_input(input *i) {
    av_freep(&i->map);
    avformat_close_input(&i->in);
}

/* Whether Matroska can carry this stream as it is. */
static int wanted(const AVCodecParameters *p, const AVOutputFormat *mkv) {
    if (p->codec_id == AV_CODEC_ID_NONE)
        return 0;
    switch (p->codec_type) {
    case AVMEDIA_TYPE_VIDEO:
        return p->width > 0 && p->height > 0;
    case AVMEDIA_TYPE_AUDIO:
        return p->sample_rate > 0;
    case AVMEDIA_TYPE_SUBTITLE:
        return avformat_query_codec(mkv, p->codec_id, FF_COMPLIANCE_NORMAL) == 1;
    default:
        return 0;
    }
}

static int open_input(input *i, const char *path, char *err, size_t errlen) {
    memset(i, 0, sizeof *i);
    i->reference = -1;
    int r = avformat_open_input(&i->in, path, NULL, NULL);
    if (r < 0) {
        say(err, errlen, "opening the capture", r);
        return r;
    }
    /* Enough probing to see every stream of a broadcast, which may not
     * send its audio's first packet until a second or two in. */
    i->in->probesize = 20 * 1000 * 1000;
    i->in->max_analyze_duration = 10 * AV_TIME_BASE;
    r = avformat_find_stream_info(i->in, NULL);
    if (r < 0) {
        say(err, errlen, "reading the capture's streams", r);
        close_input(i);
        return r;
    }
    const AVOutputFormat *mkv = av_guess_format("matroska", NULL, NULL);
    i->map = av_calloc(i->in->nb_streams, sizeof *i->map);
    if (!mkv || !i->map) {
        say(err, errlen, "out of memory", 0);
        close_input(i);
        return AVERROR(ENOMEM);
    }
    int audio = -1;
    for (unsigned s = 0; s < i->in->nb_streams; s++) {
        const AVCodecParameters *p = i->in->streams[s]->codecpar;
        i->map[s] = wanted(p, mkv) ? i->nb_out++ : -1;
        if (i->map[s] < 0)
            continue;
        if (p->codec_type == AVMEDIA_TYPE_VIDEO && i->reference < 0)
            i->reference = (int)s;
        if (p->codec_type == AVMEDIA_TYPE_AUDIO && audio < 0)
            audio = (int)s;
    }
    if (i->reference < 0)
        i->reference = audio;
    if (i->nb_out == 0 || i->reference < 0) {
        say(err, errlen, "the capture has no video or audio that could be read", 0);
        close_input(i);
        return AVERROR_INVALIDDATA;
    }
    return 0;
}

/* The continuous timeline; see the top of the file. */
typedef struct {
    int started;
    int64_t offset_us;
    int64_t max_us;
    int64_t *last_us; /* per input stream; INT64_MIN when none yet */
} timeline;

static int timeline_init(timeline *t, unsigned streams) {
    memset(t, 0, sizeof *t);
    t->last_us = av_malloc_array(streams, sizeof *t->last_us);
    if (!t->last_us)
        return AVERROR(ENOMEM);
    for (unsigned s = 0; s < streams; s++)
        t->last_us[s] = INT64_MIN;
    return 0;
}

/*
 * Places a packet on the output timeline. Returns 0 to drop it (no
 * timestamp at all), else 1 with the shift to add, in the stream's time
 * base, in *shift; sets *disc when the timeline was joined here.
 */
static int place(timeline *t, const AVStream *st, const AVPacket *pkt, int64_t *shift, int *disc) {
    int64_t ts = pkt->dts != AV_NOPTS_VALUE ? pkt->dts : pkt->pts;
    *disc = 0;
    if (ts == AV_NOPTS_VALUE)
        return 0;
    int64_t us = av_rescale_q(ts, st->time_base, US);
    int checked = st->codecpar->codec_type == AVMEDIA_TYPE_VIDEO ||
                  st->codecpar->codec_type == AVMEDIA_TYPE_AUDIO;
    if (!t->started) {
        t->started = 1;
        t->offset_us = -us;
    } else if (checked && t->last_us[st->index] != INT64_MIN) {
        int64_t out = us + t->offset_us;
        int64_t last = t->last_us[st->index];
        if (out < last - JUMP_BACK_US || out > last + JUMP_FWD_US) {
            t->offset_us = t->max_us + JOIN_GAP_US - us;
            *disc = 1;
        }
    }
    int64_t out = us + t->offset_us;
    t->last_us[st->index] = out;
    if (out > t->max_us)
        t->max_us = out;
    *shift = av_rescale_q(t->offset_us, US, st->time_base);
    return 1;
}

static int64_t end_us(const AVStream *st, const AVPacket *pkt, int64_t shift) {
    int64_t ts = pkt->pts != AV_NOPTS_VALUE ? pkt->pts : pkt->dts;
    return av_rescale_q(ts + shift + (pkt->duration > 0 ? pkt->duration : 0), st->time_base, US);
}

void zr_free(void *p) { av_free(p); }

/* FFmpeg's own log is for people debugging FFmpeg: a live capture's torn last
 * packet alone makes it complain. Failures come back as return values and
 * are reported by the caller; ZAPPER_FFMPEG_LOG=1 brings the log back. */
static void quiet(void) {
    const char *v = getenv("ZAPPER_FFMPEG_LOG");
    if (!v || !*v)
        av_log_set_level(AV_LOG_FATAL);
}

/*
 * Reads the capture without writing anything: the reference stream's
 * packets' byte positions against their output times (one every 200 ms or
 * so, and at every join), and the output's duration.
 */
int zr_scan(const char *path, zr_sample **samples, size_t *count, int64_t *duration_ms, char *err,
            size_t errlen) {
    *samples = NULL;
    *count = 0;
    *duration_ms = 0;
    quiet();
    input in;
    int r = open_input(&in, path, err, errlen);
    if (r < 0)
        return r;
    timeline t = {0};
    AVPacket *pkt = av_packet_alloc();
    size_t cap = 1024;
    zr_sample *out = av_malloc_array(cap, sizeof *out);
    if (!pkt || !out || timeline_init(&t, in.in->nb_streams) < 0) {
        say(err, errlen, "out of memory", 0);
        r = AVERROR(ENOMEM);
        goto done;
    }
    int64_t last_sample_ms = INT64_MIN, max_end = 0;
    while ((r = av_read_frame(in.in, pkt)) >= 0) {
        int s = pkt->stream_index;
        int64_t shift;
        int disc;
        if (in.map[s] < 0 || !place(&t, in.in->streams[s], pkt, &shift, &disc)) {
            av_packet_unref(pkt);
            continue;
        }
        const AVStream *st = in.in->streams[s];
        int64_t e = end_us(st, pkt, shift);
        if (e > max_end)
            max_end = e;
        if (s == in.reference && pkt->pos >= 0 && pkt->pts != AV_NOPTS_VALUE) {
            int64_t ms = av_rescale_q(pkt->pts + shift, st->time_base, MS);
            if (disc || last_sample_ms == INT64_MIN || ms - last_sample_ms >= 200) {
                if (*count == cap) {
                    cap *= 2;
                    zr_sample *grown = av_realloc_array(out, cap, sizeof *out);
                    if (!grown) {
                        say(err, errlen, "out of memory", 0);
                        r = AVERROR(ENOMEM);
                        av_packet_unref(pkt);
                        goto done;
                    }
                    out = grown;
                }
                out[(*count)++] = (zr_sample){pkt->pos, ms, disc};
                last_sample_ms = ms;
            }
        }
        av_packet_unref(pkt);
    }
    r = r == AVERROR_EOF ? 0 : r;
    if (r < 0)
        say(err, errlen, "reading the capture", r);
    *duration_ms = max_end / US_PER_MS;
done:
    if (r < 0) {
        av_free(out);
        *count = 0;
    } else {
        *samples = out;
    }
    av_packet_free(&pkt);
    av_free(t.last_us);
    close_input(&in);
    return r;
}

static int write_cue(AVFormatContext *o, int index, const zr_cue *c, int order) {
    char head[32];
    int n = snprintf(head, sizeof head, "%d,0,Clock,,0,0,0,,", order);
    size_t len = (size_t)n + strlen(c->text);
    AVPacket *p = av_packet_alloc();
    if (!p)
        return AVERROR(ENOMEM);
    int r = av_new_packet(p, (int)len);
    if (r >= 0) {
        memcpy(p->data, head, (size_t)n);
        memcpy(p->data + n, c->text, strlen(c->text));
        p->stream_index = index;
        p->pts = p->dts = c->start_ms;
        p->duration = c->end_ms - c->start_ms;
        p->flags |= AV_PKT_FLAG_KEY;
        av_packet_rescale_ts(p, MS, o->streams[index]->time_base);
        r = av_interleaved_write_frame(o, p);
    }
    av_packet_free(&p);
    return r;
}

/*
 * The capture at `in_path` into Matroska at `out_path`. With `ass_header`
 * non-NULL, adds a subtitle stream titled "Clock" carrying `cues`, which is
 * never flagged as a default track.
 */
int zr_remux(const char *in_path, const char *out_path, const char *ass_header, const zr_cue *cues,
             size_t ncues, char *err, size_t errlen) {
    quiet();
    input in;
    int r = open_input(&in, in_path, err, errlen);
    if (r < 0)
        return r;
    AVFormatContext *o = NULL;
    AVPacket *pkt = av_packet_alloc();
    AVDictionary *opts = NULL;
    timeline t = {0};
    int header = 0;
    int sub = -1;
    if (!pkt || timeline_init(&t, in.in->nb_streams) < 0) {
        say(err, errlen, "out of memory", 0);
        r = AVERROR(ENOMEM);
        goto done;
    }
    r = avformat_alloc_output_context2(&o, NULL, "matroska", out_path);
    if (r < 0) {
        say(err, errlen, "starting the Matroska file", r);
        goto done;
    }
    for (unsigned s = 0; s < in.in->nb_streams; s++) {
        if (in.map[s] < 0)
            continue;
        AVStream *src = in.in->streams[s];
        AVStream *dst = avformat_new_stream(o, NULL);
        if (!dst || (r = avcodec_parameters_copy(dst->codecpar, src->codecpar)) < 0) {
            say(err, errlen, "copying a stream's parameters", r < 0 ? r : AVERROR(ENOMEM));
            r = r < 0 ? r : AVERROR(ENOMEM);
            goto done;
        }
        /* TS's codec tags mean nothing in Matroska. */
        dst->codecpar->codec_tag = 0;
        dst->time_base = src->time_base;
        av_dict_copy(&dst->metadata, src->metadata, 0);
        dst->disposition = src->disposition;
    }
    if (ass_header) {
        AVStream *st = avformat_new_stream(o, NULL);
        size_t len = strlen(ass_header);
        if (!st || !(st->codecpar->extradata = av_mallocz(len + AV_INPUT_BUFFER_PADDING_SIZE))) {
            say(err, errlen, "out of memory", 0);
            r = AVERROR(ENOMEM);
            goto done;
        }
        memcpy(st->codecpar->extradata, ass_header, len);
        st->codecpar->extradata_size = (int)len;
        st->codecpar->codec_type = AVMEDIA_TYPE_SUBTITLE;
        st->codecpar->codec_id = AV_CODEC_ID_ASS;
        st->time_base = MS;
        st->disposition = 0;
        av_dict_set(&st->metadata, "title", "Clock", 0);
        sub = st->index;
    }
    r = avio_open(&o->pb, out_path, AVIO_FLAG_WRITE);
    if (r < 0) {
        say(err, errlen, "creating the Matroska file", r);
        goto done;
    }
    /* Without this the muxer marks the first subtitle stream as a default
     * track when none is, and players would show the clock unasked. */
    av_dict_set(&opts, "default_mode", "infer_no_subs", 0);
    r = avformat_write_header(o, &opts);
    if (r < 0) {
        say(err, errlen, "writing the Matroska header", r);
        goto done;
    }
    header = 1;
    size_t next_cue = 0;
    while ((r = av_read_frame(in.in, pkt)) >= 0) {
        int s = pkt->stream_index;
        int64_t shift;
        int disc;
        if (in.map[s] < 0 || !place(&t, in.in->streams[s], pkt, &shift, &disc)) {
            av_packet_unref(pkt);
            continue;
        }
        AVStream *src = in.in->streams[s];
        if (pkt->pts != AV_NOPTS_VALUE)
            pkt->pts += shift;
        if (pkt->dts != AV_NOPTS_VALUE)
            pkt->dts += shift;
        if (pkt->pts == AV_NOPTS_VALUE)
            pkt->pts = pkt->dts;
        int64_t now_ms = av_rescale_q(pkt->dts != AV_NOPTS_VALUE ? pkt->dts : pkt->pts, src->time_base, MS);
        for (; sub >= 0 && next_cue < ncues && cues[next_cue].start_ms <= now_ms; next_cue++) {
            if ((r = write_cue(o, sub, &cues[next_cue], (int)next_cue)) < 0) {
                say(err, errlen, "writing the clock track", r);
                av_packet_unref(pkt);
                goto done;
            }
        }
        int idx = in.map[s];
        pkt->stream_index = idx;
        pkt->pos = -1;
        av_packet_rescale_ts(pkt, src->time_base, o->streams[idx]->time_base);
        r = av_interleaved_write_frame(o, pkt);
        av_packet_unref(pkt);
        if (r < 0) {
            say(err, errlen, "writing a packet", r);
            goto done;
        }
    }
    if (r != AVERROR_EOF) {
        say(err, errlen, "reading the capture", r);
        goto done;
    }
    for (; sub >= 0 && next_cue < ncues; next_cue++) {
        if ((r = write_cue(o, sub, &cues[next_cue], (int)next_cue)) < 0) {
            say(err, errlen, "writing the clock track", r);
            goto done;
        }
    }
    r = av_write_trailer(o);
    if (r < 0)
        say(err, errlen, "finishing the Matroska file", r);
done:
    if (r < 0 && header)
        av_write_trailer(o);
    if (o && o->pb)
        avio_closep(&o->pb);
    avformat_free_context(o);
    av_dict_free(&opts);
    av_packet_free(&pkt);
    av_free(t.last_us);
    close_input(&in);
    return r < 0 ? r : 0;
}
