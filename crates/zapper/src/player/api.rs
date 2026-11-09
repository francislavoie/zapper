//! The player's commands, one per command the window sends, under the same
//! names and with the same parameters: what any front end calls to drive
//! playback. The state they act on is [`super::controller`]'s.

use std::sync::atomic::Ordering;

use super::controller::*;
use super::{tracks, LIVE_CUSHION};
use crate::catalog::activity;
use crate::connections::{self, Limit};
use crate::ipc::types::{
    AspectMode, ConnectionsDto, DeinterlaceMode, PlaybackState, PlayerOptionsDto, PlayerStateDto,
};
use crate::source::{Playable, Source};

/// Stops reconnecting. Mid-attempt, the attempt is stopped too, and the
/// failure that started it is shown again with Retry.
pub async fn cancel_reconnect() -> Result<(), String> {
    let attempting = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        let attempting = m.dto.reconnect.is_some() && m.dto.state == PlaybackState::Opening;
        m.reconnect_token += 1;
        attempting
    };
    if attempting {
        on_player(|p| p.command("stop", &[]).map_err(|e| e.to_string())).await?;
    }
    commit(|m| {
        let ending = m.chain_ending.take().or(m.dto.ending);
        if attempting {
            m.failed("reconnecting was cancelled");
        }
        m.reconnect_abandoned();
        m.reconnects = 0;
        m.dto.ending = ending;
    });
    Ok(())
}

/// The account's connections, for Settings → Account: how many it allows,
/// how many are open, when it expires.
pub async fn connection_status() -> Result<ConnectionsDto, String> {
    let declared_max = crate::catalog::settings::load_or_default().simultaneous_streams;
    let src = source()?;
    let declared = matches!(src, Source::M3u(_) | Source::Tvheadend(_));
    // An account that cannot be asked has nothing worth keeping; one that
    // can is asked at most every few seconds.
    let s = if declared {
        connections::look(src, declared_max).await
    } else {
        connections::look_cached(src, declared_max).await
    };
    Ok(ConnectionsDto {
        max: s.max,
        active: s.active,
        ours: connections::held(),
        counted: matches!(s.limit(), Limit::Counted(_)),
        declared,
        expires: s.expires,
    })
}

pub async fn play(stream_id: i64) -> Result<(), String> {
    play_channel(stream_id).await
}

/// Plays a movie, from `start` seconds in when given; `held`, paused there
/// while the viewer is asked whether to resume ([`OpenAt`]). On the
/// player's own connection, like a live channel and under the same budget:
/// never through the recording service, which has nothing to do with a
/// title.
pub async fn play_movie(
    movie_id: i64,
    start: Option<f64>,
    held: Option<bool>,
) -> Result<(), String> {
    let target = movie_target(movie_id)?;
    let at = OpenAt::asked(start, held);
    open_as(
        Opened::Movie(movie_id),
        target,
        OpenKind::OnDemand,
        None,
        Via::Direct,
        TrackChoice::Fresh,
        at,
    )
    .await
}

/// Plays an episode, from `start` seconds in when given (`held`, paused
/// there): on the player's own connection and under the same rules as a
/// movie.
pub async fn play_episode(
    episode_id: i64,
    start: Option<f64>,
    held: Option<bool>,
) -> Result<(), String> {
    let target = episode_target(episode_id)?;
    let at = OpenAt::asked(start, held);
    open_as(
        Opened::Episode(episode_id),
        target,
        OpenKind::OnDemand,
        None,
        Via::Direct,
        TrackChoice::Fresh,
        at,
    )
    .await
}

/// Moves on from episode `from_episode_id`, playing or just played to its
/// end, to episode `next_episode_id`: Up next. From where it was left when
/// it is in progress ([`next_episode_start`], without asking), else from
/// its start.
///
/// Where it was left is read first, before anything is claimed, for
/// [`PLACE_WAIT`] at most ([`place_then_claim`]): the episode playing plays
/// on meanwhile, and the claim and the open then follow each other at once.
/// A read that takes longer (a catalogue refresh holding the database) opens
/// it from its start, and its saved place is then lost once it has played
/// [`SAVE_EVERY_SECS`](crate::catalog::vod_progress::SAVE_EVERY_SECS) from
/// there.
///
/// Refused unless `from_episode_id` is still what is open or what finished
/// ([`up_next_allowed`]): never after a drop, a Stop or the sleep timer, or
/// once anything else was opened. Claimed under the state lock
/// ([`Machine::advance`]), so a second call (a double press, the countdown
/// and a press together) is refused rather than opening it twice. The
/// episode moved on from is marked watched, off the main thread, before
/// this answers (waited for [`MARK_WAIT`] at most). mpv replaces its one
/// stream, as for any open; the audio and subtitle languages are kept. It
/// opens playing, even when the episode moved on from was paused
/// ([`pause_on_open`]).
pub async fn play_next_episode(from_episode_id: i64, next_episode_id: i64) -> Result<(), String> {
    let target = episode_target(next_episode_id)?;
    let read = move || crate::catalog::db::with_db(|c| Ok(next_episode_start(c, next_episode_id)));
    let (at, (claimed, place)) = place_then_claim(read, PLACE_WAIT, || {
        let mut claimed = Err(String::new());
        let mut place = None;
        commit(|m| {
            claimed = m.advance(from_episode_id, next_episode_id);
            // Where it got to, if not written yet: written before the mark, so
            // that the place is the one it was left at.
            if claimed.is_ok() {
                place = PROGRESS.lock().ok().and_then(|mut p| p.flush());
            }
        });
        (claimed, place)
    })
    .await;
    let languages = claimed?;
    // Written beside the open, and waited for before answering: whoever
    // asked re-reads the show's marks next, and finds this one there.
    let marked = rt().spawn_blocking(move || {
        let r = crate::catalog::db::with_db(|c| {
            mark_moved_on(c, place, from_episode_id, activity::now_secs()).map(|_| ())
        });
        if let Err(e) = r {
            tracing::warn!("could not mark the episode moved on from as watched: {e}");
        }
    });
    let tracks = languages.map_or(TrackChoice::Fresh, TrackChoice::Carry);
    let opened = open_as(
        Opened::Episode(next_episode_id),
        target,
        OpenKind::OnDemand,
        None,
        Via::Direct,
        tracks,
        at,
    )
    .await;
    // Not a reason to fail the move on: the next episode is open either way.
    // Waited for a few seconds at most: a catalogue refresh holding the
    // database must not hold up the answer; the mark still lands after.
    match tokio::time::timeout(MARK_WAIT, marked).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("marking the episode moved on from did not finish: {e}"),
        Err(_) => tracing::warn!(
            "marking the episode moved on from took over {}s; answering without it",
            MARK_WAIT.as_secs()
        ),
    }
    opened
}

/// Replays a past programme on an archive-enabled channel.
/// `start` is Unix seconds; `duration_minutes` bounds the request.
pub async fn play_catchup(stream_id: i64, start: i64, duration_minutes: u32) -> Result<(), String> {
    let (start, duration_minutes, near_edge) = within_archive(stream_id, start, duration_minutes);
    let target = match source()? {
        Source::Xtream(server) => {
            // The server reads the timestamp as wall time in its own time
            // zone, which need not be this computer's: see
            // `catalog::panel_clock`.
            let offset = crate::catalog::panel_clock::offset_now(server, start).await;
            let stamp = timeshift_stamp(start, offset);
            Playable::plain(server.timeshift_url(stream_id, &stamp, duration_minutes))
        }
        // A playlist's archive is whatever its entry's template says; the
        // local offset is there for templates written in wall-clock time.
        // `{utc}`, `{lutc}` and the rest are Unix time and need no zone. A
        // playlist has no account call that says which zone its server's
        // clock is in, so the wall-clock fields stay in this computer's,
        // as IPTV Simple writes them — `xc` entries included.
        Source::M3u(m) => m
            .catchup(
                stream_id,
                start,
                duration_minutes,
                activity::now_secs(),
                crate::localtime::utc_offset(start),
            )
            .map_err(|e| crate::catalog::sync_engine::scrub(&e.to_string()))?,
        // A tuner has no archive, and a Tvheadend server's is its DVR, which
        // is not read. Their channels are never marked as having catch-up,
        // so this is only reached by a stale request from before a switch.
        Source::HdHomeRun(_) | Source::Tvheadend(_) => {
            return Err("this account has no catch-up".to_owned());
        }
    };
    open(stream_id, target, OpenKind::CatchUp(CatchUp { near_edge })).await
}

pub async fn stop() -> Result<(), String> {
    crate::surface::title_thumbs::stop();
    crate::surface::trickplay::clear();
    // Where a movie or an episode got to, before the title is forgotten.
    let place = PROGRESS.lock().ok().and_then(|mut p| p.flush());
    if let Some(w) = place {
        persist_progress(w);
    }
    let r = on_player(|p| p.command("stop", &[]).map_err(|e| e.to_string())).await;
    commit(|m| m.stopped());
    // Its place on the recording service's connection, given back now.
    if let Some(viewer) = current_viewer() {
        if let Ok(mut last) = LAST_OPEN.lock() {
            if let Some(l) = last.as_mut() {
                l.3 = Via::Direct;
            }
        }
        THROUGH_SERVICE.store(false, Ordering::Release);
        if let Ok(c) = crate::recording::client::Client::local() {
            let _ = c.stop_watching(&viewer).await;
        }
    }
    r
}

pub async fn toggle_pause() -> Result<(), String> {
    let paused = on_player(|p| {
        let want = !p.get_bool("pause").unwrap_or(false);
        p.set_flag("pause", want).map_err(|e| e.to_string())?;
        Ok(want)
    })
    .await?;
    commit(|m| m.chose_pause(paused));
    Ok(())
}

/// Pauses, or plays, as `paused` says: unlike [`toggle_pause`], asking twice
/// is asking once, so a Resume after a Space still plays. Playing a title
/// opened held ends its hold.
pub async fn set_paused(paused: bool) -> Result<(), String> {
    on_player(move |p| p.set_flag("pause", paused).map_err(|e| e.to_string())).await?;
    commit(|m| m.chose_pause(paused));
    Ok(())
}

/// Negative seeks back. Clamped to the buffered window.
pub async fn seek_relative(seconds: f64) -> Result<(), String> {
    // Rewinding is a deliberate choice to be behind, so stop chasing the edge.
    // Seeking forward is the viewer catching up themselves and leaves the mode
    // alone; the automatic follow will finish the job if they stop short.
    let movie = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        if seconds < 0.0 {
            m.following_live = false;
        }
        m.dto.on_demand.is_some()
    };
    on_player(move |p| {
        let probe = p.probe();
        // A movie is read from a file that can be asked for any part of it:
        // the buffered window is a live stream's limit, not its. Its own
        // ends are, though: never before the start or past the end.
        let target = movie
            .then(|| {
                probe
                    .time_pos
                    .map(|now| clamp_to_title(now + seconds, title_length(p)))
            })
            .flatten();
        let by = match (movie, probe.time_pos, target) {
            (true, Some(now), Some(at)) => at - now,
            (true, _, _) => seconds,
            (false, _, _) => clamp_seek(seconds, probe.buffered_back, probe.behind_live),
        };
        if by.abs() < 0.05 {
            return Ok(());
        }
        p.command("seek", &[&format!("{by:.3}"), "relative"])
            .map_err(|e| e.to_string())?;
        if let Some(at) = target {
            sought(at);
        }
        Ok(())
    })
    .await
}

/// Seeks a movie or an episode to `seconds` from its start.
pub async fn seek_to(seconds: f64) -> Result<(), String> {
    if !seconds.is_finite() {
        return Err("that is not a time".into());
    }
    let movie = STATE
        .lock()
        .expect("player state mutex poisoned")
        .dto
        .on_demand
        .is_some();
    if !movie {
        return Err("seeking to a time is for movies and episodes".into());
    }
    on_player(move |p| {
        let at = clamp_to_title(seconds, title_length(p));
        p.command("seek", &[&format!("{at:.3}"), "absolute"])
            .map_err(|e| e.to_string())?;
        sought(at);
        Ok(())
    })
    .await
}

pub async fn seek_live() -> Result<(), String> {
    // Asking for live means staying there, not visiting once.
    // A movie or an episode has no live edge, and says so.
    let to = {
        let mut m = STATE.lock().expect("player state mutex poisoned");
        let to = m.to_live();
        match to {
            ToLive::Seek => m.follow_live()?,
            ToLive::Reopen(id) => m.reopen_started(id),
            ToLive::Already => {}
        }
        to
    };
    if to == ToLive::Already {
        return Ok(());
    }
    if let ToLive::Reopen(stream_id) = to {
        // A catch-up programme's end is not live: forward through it is
        // only further into the programme. The channel is opened again, as
        // picking it would, once the programme's connection is closed — an
        // account allowed one stream has it back for the channel.
        //
        // The open's own state begins when the open does (`open_as`), not
        // here: a channel that is only being asked for — the recording
        // service may take a while to answer — is not yet an open on the
        // clock, and one that never starts must not end up counted against
        // the channel. Should it not start, the programme is gone either
        // way, and that is said.
        tracing::info!("back to live from a catch-up programme");
        let r = match on_player(|p| p.command("stop", &[]).map_err(|e| e.to_string())).await {
            Ok(()) => play_channel(stream_id).await,
            Err(e) => Err(e),
        };
        if let Err(e) = &r {
            commit(|m| m.reopen_failed(stream_id, e));
        }
        return r;
    }
    let by = on_player(|p| {
        let probe = p.probe();
        // Relative, the same operation the backward seek already does, in
        // reverse - and bounded by `behind_live`, which is measured to the end
        // of mpv's *seekable* window rather than to the end of its cache. A
        // seek past the seekable end (the cache runs one to three seconds
        // beyond it) is ignored by mpv without an error: playback stays
        // exactly where it was.
        let by = (probe.behind_live - LIVE_CUSHION).max(0.0);
        if by <= 0.05 {
            return Ok(0.0);
        }
        p.command("seek", &[&format!("{by:.3}"), "relative"])
            .map(|()| by)
            .map_err(|e| e.to_string())
    })
    .await?;
    commit(|m| m.jumped_to_live(by));
    Ok(())
}

/// 0-200; above 100 is software boost via `volume-max`.
pub async fn set_volume(volume: f64) -> Result<(), String> {
    let v = volume.clamp(0.0, 200.0);
    let to_mpv = mpv_volume(v);
    on_player(move |p| p.set_f64("volume", to_mpv).map_err(|e| e.to_string())).await?;
    commit(|m| m.dto.volume = v);
    Ok(())
}

pub async fn set_muted(muted: bool) -> Result<(), String> {
    on_player(move |p| p.set_flag("mute", muted).map_err(|e| e.to_string())).await?;
    commit(|m| m.dto.muted = muted);
    Ok(())
}

/// Toggles loudness normalisation. Named "compressor" because that is what
/// the control is usually called in players, but see [`compressor_filter`]
/// for why it is not a classic compressor.
pub async fn set_compressor(enabled: bool) -> Result<(), String> {
    let filter = if enabled {
        compressor_filter()
    } else {
        String::new()
    };
    on_player(move |p| p.set_property("af", &filter).map_err(|e| e.to_string())).await?;
    commit(|m| m.dto.compressor = enabled);
    Ok(())
}

/// The scrollback preview nearest a point in the buffer.
///
/// `behind_live` is the same number the scrubber speaks in everywhere else,
/// so the interface does not have to know that the thumbnails are keyed by
/// stream time. `None` is an ordinary answer: nothing has been captured near
/// that moment yet, or the archive was asked about, which is left of the
/// buffer and was never on this screen to be photographed.
pub async fn thumbnail_at(
    behind_live: f64,
) -> Result<Option<crate::ipc::types::ThumbnailDto>, String> {
    if !behind_live.is_finite() || behind_live < 0.0 {
        return Ok(None);
    }
    on_player(move |p| {
        let probe = p.probe();
        // The seekable end is the live edge in stream time, and it is what
        // the thumbnails were stamped against. Falling back to the playhead
        // plus its own lag covers the moments before mpv reports a window.
        let Some(edge) = probe
            .live_edge
            .or_else(|| probe.time_pos.map(|t| t + probe.behind_live))
        else {
            return Ok(None);
        };
        Ok(
            crate::surface::trickplay::nearest_png(edge - behind_live).map(|(at, png)| {
                crate::ipc::types::ThumbnailDto {
                    data_url: format!(
                        "data:image/png;base64,{}",
                        crate::surface::trickplay::base64(&png)
                    ),
                    behind_live: (edge - at).max(0.0),
                }
            }),
        )
    })
    .await
}

/// The preview for a moment of the open movie or episode, `at` seconds in.
///
/// The frame of the slot nearest `at` (a slot every ten seconds of a title,
/// more for a very long one): drawn while it played, or decoded from what
/// mpv has buffered. `None` where there is neither, which the seek bar shows
/// as the time alone. Answered from the store, so it never waits on mpv.
pub async fn thumbnail_for_title(
    at: f64,
) -> Result<Option<crate::ipc::types::TitleThumbnailDto>, String> {
    Ok(
        crate::surface::trickplay::title_frame(at).map(|(at, mime, bytes)| {
            crate::ipc::types::TitleThumbnailDto {
                data_url: format!(
                    "data:{mime};base64,{}",
                    crate::surface::trickplay::base64(&bytes)
                ),
                at,
            }
        }),
    )
}

/// Everything the diagnostics panel shows: one reading, and the history.
///
/// Polled rather than pushed: it is far more than the state feed needs, and
/// nothing should pay for it while the panel is closed. The history is the
/// exception and is collected continuously — see `ipc::health` for why, and
/// for how little it costs — but it still only crosses the boundary when
/// somebody asks, which is what makes one call enough to draw the panel.
pub async fn player_stats() -> Result<crate::ipc::types::StatsDto, String> {
    on_player(|p| {
        let s = p.stats();
        let probe = p.probe();
        Ok(crate::ipc::types::StatsDto {
            container: s.container,
            video_codec: s.video_codec,
            audio_codec: s.audio_codec,
            hwdec: s.hwdec,
            pixel_format: s.pixel_format,
            primaries: s.primaries,
            gamma: s.gamma,
            video_bitrate: s.video_bitrate,
            audio_bitrate: s.audio_bitrate,
            measured_fps: s.measured_fps,
            container_fps: s.container_fps,
            audio_samplerate: s.audio_samplerate,
            audio_channels: s.audio_channels,
            cache_duration: s.cache_duration,
            cache_speed: s.cache_speed,
            demuxer_bytes: s.demuxer_bytes,
            buffering: s.buffering,
            cache_buffering_percent: s.cache_buffering_percent,
            dropped_frames: s.dropped_frames,
            decoder_dropped_frames: s.decoder_dropped_frames,
            avsync: s.avsync,
            behind_live: probe.behind_live,
            buffered_back: probe.buffered_back,
            history: crate::ipc::health::samples(),
            stalls: crate::ipc::health::stalls(),
        })
    })
    .await
}

pub async fn player_state() -> Result<PlayerStateDto, String> {
    Ok(STATE
        .lock()
        .expect("player state mutex poisoned")
        .dto
        .clone())
}

pub async fn player_options() -> Result<PlayerOptionsDto, String> {
    Ok(OPTIONS
        .lock()
        .expect("player options mutex poisoned")
        .clone())
}

/// Switches to one of the stream's audio tracks, by mpv's audio id.
pub async fn set_audio_track(id: i64) -> Result<PlayerOptionsDto, String> {
    on_player(move |p| {
        p.set_property("aid", &tracks::track_value(Some(id)))
            .map_err(|e| e.to_string())
    })
    .await?;
    // The list itself follows from mpv's notification; this is so the menu
    // that asked sees its answer without waiting a heartbeat for it.
    Ok(commit_options(|o| {
        for t in &mut o.audio {
            t.selected = t.id == id;
        }
    }))
}

/// Shows one of the stream's subtitle tracks, or hides them with `None`:
/// the picture menu's choice, and the toggle's. Hiding keeps the track, and
/// showing the track already chosen is not a switch ([`subtitle_command`]).
///
/// The viewer's choice: it is the standing one from now, for every open
/// after this (the interface saves it in the settings as well), and this
/// open's standing choice is not applied over it.
pub async fn set_subtitle_track(id: Option<i64>) -> Result<PlayerOptionsDto, String> {
    let listed = OPTIONS
        .lock()
        .map(|o| o.subtitles.clone())
        .unwrap_or_default();
    let selected = on_player(|p| Ok(p.get_i64("current-tracks/sub/id"))).await?;
    let (props, chosen) = match subtitle_ask(&listed, id, selected) {
        SubtitleAsk::Choose(props, chosen) => (props, chosen),
        SubtitleAsk::Gone(props) => {
            set_subtitle_pref(|p| p.on = true);
            on_player(move |p| apply_properties(p, &props)).await?;
            commit_options(|o| o.subtitles_visible = true);
            return player_options().await;
        }
    };
    on_player(move |p| apply_properties(p, &props)).await?;
    commit(Machine::chose_subtitles);
    set_subtitle_pref(|p| p.chose(chosen.as_ref()));
    Ok(commit_options(|o| {
        if let Some(id) = id {
            for t in &mut o.subtitles {
                t.selected = t.id == id;
            }
        }
        o.subtitles_visible = id.is_some();
    }))
}

pub async fn set_aspect(aspect: AspectMode) -> Result<PlayerOptionsDto, String> {
    let props: Vec<(&'static str, String)> = tracks::aspect_properties(aspect)
        .iter()
        .map(|(k, v)| (*k, (*v).to_owned()))
        .collect();
    on_player(move |p| apply_properties(p, &props)).await?;
    Ok(commit_options(|o| o.aspect = aspect))
}

pub async fn set_deinterlace(mode: DeinterlaceMode) -> Result<PlayerOptionsDto, String> {
    on_player(move |p| {
        p.set_property("deinterlace", tracks::deinterlace_value(mode))
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(commit_options(|o| o.deinterlace = mode))
}

/// Applies subtitle styling to the running player.
///
/// Subtitles are drawn by libass inside the video layer, not the DOM, so this
/// is the only way to style them.
pub async fn set_subtitle_style(style: crate::ipc::types::SubtitleStyleDto) -> Result<(), String> {
    let props = subtitle_properties(&style);
    on_player(move |p| apply_properties(p, &props)).await
}

/// Applies playback settings that mpv owns: rewind budget, hardware decode,
/// tone mapping. Persisted by the caller.
pub async fn apply_playback_settings(
    settings: crate::ipc::types::SettingsDto,
) -> Result<(), String> {
    LIVE_START.store(settings.live_start as u8, Ordering::Release);
    set_subtitle_pref(|p| *p = SubtitlePref::of(&settings));
    let props = playback_properties(&settings);
    on_player(move |p| apply_properties(p, &props)).await
}

/// What the desktop's media controls show: the channel, its programme and
/// whether it is playing, or `None` with nothing on. Accepted whether or not
/// the controls are on, so turning them on shows what is already playing.
pub async fn set_media_session(
    session: Option<crate::ipc::types::MediaSessionDto>,
) -> Result<(), String> {
    crate::media_keys::set_session(session);
    Ok(())
}
