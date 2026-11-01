# Recording

zapper records through a separate, headless program, **zapper-server**: the recording
service. It runs in the background whether or not the app is open, wakes when a recording is
due, and saves it as a Matroska file a media library such as Jellyfin can play. The app
schedules recordings from the guide and shows them; the service does the recording.

## Using it

1. **Settings → Recording → Install** runs the service on this computer (Linux: a systemd user
   service, no root needed). *Keep running when logged out* keeps it recording while nobody is
   logged in (`loginctl enable-linger`). **Remove** stops and removes it and keeps your
   recordings. Windows and macOS: not yet — run `zapper-server` yourself.
2. **Right-click a programme** in the guide, or in the programme search results, and choose
   **Record**. A red dot marks it; it pulses while recording. Right-click again to cancel, or to
   stop a recording early (what was recorded is kept).
3. **Recordings** (in the menu and the command palette) lists what is recording, scheduled,
   recorded and failed, with *Open folder*, *Cancel*/*Stop* and *Delete*.

Settings → Recording also sets the folder, how early to start and how late to stop (1 and 5
minutes by default), whether to add the clock track, and — for playlists, which cannot say — how
many streams the account allows at once.

### Files

```text
<folder>/<Show>/<Show> - 2026-11-03 20.00.mkv
<folder>/<Show>/<Show> - S02E05 - 2026-11-03 20.00.mkv        when the guide gives the episode
<folder>/<Show>/<Show> - 2026-11-03 20.00 - part1.mkv         when it switched feeds
<folder>/<Show>/<Show> - 2026-11-03 20.00 - part2.mkv
```

The date and time are the programme's start in this computer's time zone. Nothing is
re-encoded: the provider's video and audio are copied into the file as they came. While
recording, the stream goes to a hidden `.… - partN.ts` beside it, which becomes the `.mkv` when
the recording ends. If that remux fails, the `.ts` is kept (renamed like the `.mkv` would have
been) and the recording says so.

### The clock track

An optional subtitle track, titled *Clock*, showing the wall-clock time (`HH:MM:SS`) each moment
was live, small in the bottom-right corner. It is never marked as a default track, so players
leave it off until you choose it. The time comes from when the stream's bytes arrived, carried
through the stream's own timestamps, so it ticks smoothly while the feed does and jumps where the
recording has a gap (a reconnect, a switch of feed). Turn it off in Settings → Recording.

## How it behaves

**Connections.** Most providers allow few streams at once per account, often one, and end the
oldest when another device opens one. Before every connection the service asks whether one is
free — Xtream accounts report their active and maximum connections through the API (an API
call, not a stream); playlists count the service's own streams against the setting; HDHomeRun
and Tvheadend allot their own tuners. When the account is full the recording waits ("waiting
for a free connection"), asking again after 30 s, doubling to five minutes, with jitter. It
never takes the connection from someone watching, and never alternates grabbing it with another
client.

**Main and backup feeds.** A recording has a main feed (the channel chosen) and, when there is
one, a backup: another channel carrying the same guide channel. It switches only when the
current feed *fails* — no data for 10 s, a refusal, the stream ending, the server unreachable —
and never back while the current one works. A refusal for the account's connection limit (509,
429, "max connections" in words, or a `200` carrying text instead of video) is not a feed
failure: the backup is on the same account and would be refused too, so nothing switches and the
same feed is asked again after one minute, doubling to ten. After three switches within ten
minutes it stops switching, stays on the last feed that delivered and retries with back-off
(5 s, doubling to a minute), and notes it on the recording. Each switch starts a new part.

**Schedule.** Recordings are kept in `recordings.db` beside the catalogue, so they survive
restarts. A start missed while the service was off begins at once if the programme is still on;
one whose whole window passed is marked failed ("missed"). A recording interrupted by a restart
resumes in a new part. The service checks its schedule at least every 30 seconds, so a suspend
or a clock change cannot make it late.

**Catalogue.** On the same machine the service and the app share the catalogue database. Either
may refresh it; a lease in the database keeps them from refreshing at the same moment, and both
honour the interval set in Settings → Guide.

## The service

```text
zapper-server [--port N] [--no-sync]
```

- `--port N`: listen on `127.0.0.1:N` (default: the port in `server.json`, else 8796).
- `--no-sync`: leave the catalogue to the app.

Its files, in zapper's config and data directories (`~/.config/zapper`, `~/.local/share/zapper`
on Linux; `ZAPPER_HOME` moves both):

| File | What |
|---|---|
| `server.json` | port, folder, padding, clock track, simultaneous streams |
| `server-token` | the API token, readable by you alone; made on first start |
| `recordings.db` | the recordings (SQLite) |

Logs go to standard error: `journalctl --user -u zapper-server` when installed from the app.
`ZAPPER_FFMPEG_LOG=1` adds FFmpeg's own log of the remux.

### Building it

```sh
cd src-tauri
cargo build --release --no-default-features --features server --bin zapper-server
```

`--no-default-features` leaves out the `gui` feature, so nothing of Tauri, WebKit, GTK or libmpv
is linked; the server needs only `libavformat`, `libavcodec` and `libavutil` (their development
files, found with pkg-config — set `PKG_CONFIG_PATH` to build against another FFmpeg). Settings →
Recording → Install looks for `zapper-server` beside the `zapper` executable, then on `PATH`
(`ZAPPER_SERVER_BIN` overrides both). The packages do not ship it yet.

### Checking it

```sh
cd src-tauri
cargo build --no-default-features --features harness,server --bin zapper-server --bin recording_check
target/debug/recording_check
```

On a scratch `ZAPPER_HOME`, never your account: two `tools/fake_stream.py` servers stand for a
channel and its backup, the main one ending its stream after ten seconds. It schedules 25 s
through the API, then checks that the recording switched to the backup, that it is two `.mkv`
parts that ffprobe reads with the video, the audio and a non-default *Clock* track, that the
clock reads when it was recorded, and that each feed was asked exactly once.

## API

HTTP with JSON bodies, on `127.0.0.1` only. Every request except `GET /api/version` needs
`Authorization: Bearer <token>`, the contents of `server-token`. Field names are camelCase;
times are Unix seconds; errors are `{"error": "…"}` with a 4xx status.

| Request | Answer |
|---|---|
| `GET /api/version` | `{version, apiVersion}` |
| `GET /api/status` | `{version, apiVersion, active: [{id, title, streamId, bytes, since, waiting?}], outputDir, diskFree, diskTotal, now}` |
| `GET /api/recordings` | `[Recording]`, soonest first |
| `GET /api/recordings/{id}` | `Recording` |
| `POST /api/recordings` | schedule: body `{streamId, title, start, stop, backupStreamId?, channelName?, episodeTitle?, season?, episode?, padBefore?, padAfter?}` → `201 Recording`; the same channel and start again returns the existing one |
| `POST /api/recordings/{id}/cancel` | a scheduled one is removed (`{id, cancelled: true}`); one recording stops and keeps what it has (`Recording`) |
| `DELETE /api/recordings/{id}?files=true` | forgets a finished recording; `files=true` deletes its files too. Refused while recording |
| `GET /api/settings` | `{outputDir, padBefore, padAfter, clockTrack, simultaneousStreams}` (padding in seconds) |
| `PUT /api/settings` | the same, all fields; returns what was saved |

`Recording`:

```json
{
  "id": 3, "streamId": 1201, "backupStreamId": 1305, "channelName": "One HD",
  "title": "The News", "season": 2, "episode": 5,
  "start": 1793736000, "stop": 1793737800, "padBefore": 60, "padAfter": 300,
  "state": "done",
  "parts": [{"path": "/home/me/Videos/zapper/The News/The News - S02E05 - 2026-11-03 15.00.mkv",
             "streamId": 1201, "bytes": 1210000000, "started": 1793735940, "ended": 1793738100}],
  "notes": ["switched from the main feed to the backup: no data for 10 s"],
  "folder": "/home/me/Videos/zapper/The News"
}
```

`state` is `scheduled`, `recording`, `done` or `failed` (with `error`). `waiting` is set while
a recording is not capturing, such as "waiting for a free connection (1 of 1 in use)".
`backupStreamId` omitted when scheduling means "find one"; the main feed's own id means "none".

## Not yet

- A web client: the app's interface in a browser, served by the service (streams proxied, so
  provider credentials never reach the browser).
- Installing the service on Windows (a logon task) and macOS (a LaunchAgent); waking the
  computer from suspend to record.
- Series rules ("record every episode"), and choosing the backup feed by hand.
- Season and episode from the guide: the catalogue does not keep XMLTV's episode numbers yet, so
  files are named by date unless a client passes them.
- Joining parts whose codecs match into one file.
- Shipping `zapper-server` in the packages, and a headless package without the GUI's
  dependencies.
