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

Settings → Recording also sets the folder, how files are named, how early to start and how late
to stop (1 and 5 minutes by default), whether to add the clock track, and — for playlists, which
cannot say — how many streams the account allows at once.

### Files

By default, laid out the way Jellyfin, Plex and Emby read a TV library:

```text
<folder>/<Show>/<Show> - 2026-11-03 20.00.mkv
<folder>/<Show>/<Show> - S02E05 - 2026-11-03 20.00.mkv        when the guide gives the episode
<folder>/<Show>/<Show> - 2026-11-03 20.00 - part1.mkv         when it switched feeds
<folder>/<Show>/<Show> - 2026-11-03 20.00 - part2.mkv
<folder>/<Show>/<Show> - 2026-11-03 20.00 (2).mkv             when the name was taken
```

**File names** in Settings → Recording chooses another layout, with a preview of the paths two
example programmes would get:

| Choice | Pattern | Gives |
|---|---|---|
| Media server (default) | `{show}/{show} - {se} - {date} {time}` | `Harbour Lights/Harbour Lights - S02E04 - 2026-11-03 20.00.mkv` |
| By channel | `{channel}/{show} - {date} {time}` | `Northwind One HD/Harbour Lights - 2026-11-03 20.00.mkv` |
| By date | `{date}/{time} {channel} - {show}` | `2026-11-03/20.00 Northwind One HD - Harbour Lights.mkv` |
| Flat | `{show} - {channel} - {date} {time}` | `Harbour Lights - Northwind One HD - 2026-11-03 20.00.mkv` |
| Custom | your own | |

A custom pattern is text with these tokens; `/` (or `\`) makes a folder:

| Token | Gives |
|---|---|
| `{show}` | the programme's title |
| `{title}` | the episode's title, when the guide has one |
| `{channel}` | the channel's name |
| `{date}` | `2026-11-03`: the programme's start, in this computer's time zone |
| `{time}` | `20.00`, likewise (24-hour; `:` is not allowed in Windows names) |
| `{year}` | `2026` |
| `{season}`, `{episode}` | `02`, `04`, when known |
| `{se}` | `S02E04`; `E04` with no season; nothing without an episode |

A token with nothing to give takes what only made sense beside it: brackets right around it
(`({year})`), then the separators (spaces, `-`, `_`, `.`, `,`, dashes) just before it — or, at the
start of a name, just after it. So `{show} - {se} - {date}` is `The Evening News - 2026-11-03`
for a programme without an episode number, `{show} ({title})` is `The Evening News`, and a folder
that comes out empty is left out (`{show}/{season}/…` puts a show without seasons straight in its
own folder).
Letters are never taken: `{season}x{episode}` with no season is `x04`. A file name that comes out
empty (`{title}` alone, for a programme without one) becomes `{show} - {date} {time}`.

The service always adds the extension, ` - partN` when there are several parts and ` (2)` rather
than replace a file; a pattern never does. Nothing can lead outside the folder: a pattern
starting with `/`, `\` or a drive, with a `..` folder, a reserved Windows name (`CON`, `NUL`,
`COM1`…) or a character Windows forbids (`<>:"|?*`), with an empty folder name, or with no token in
the file name is refused with the reason. Titles from the guide are cleaned the same way whatever
the pattern: `/`, `\` and `:` become `-`, the other forbidden characters and leading dots go, a
reserved name gets a `_`, and each value is cut to 100 bytes and each folder or file name to 180,
so the whole stays under the 255 bytes file systems allow. Where a recording goes is decided when
it is scheduled, so changing the layout leaves recordings already scheduled where they were.

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

## Network access

Off by default: the service listens on `127.0.0.1` only, for the app on the same computer.
**Settings → Recording → Network access** lets other devices on your network reach it — today
its API, and the web client once there is one.

1. Under **Other devices**, set a **sign-in password** (at least 8 characters, typed twice).
   Network access cannot be turned on without one.
2. Turn on **Allow other devices on your network**. The service then listens on every interface
   (`0.0.0.0`, and `::` where the computer has IPv6) on the same **port** (8796 unless changed;
   1024–65535). The switch and the port take effect at once: the service replaces its listeners
   without restarting, so recordings in progress carry on. A port another program holds is
   refused with that reason, and the service stays where it was.
3. **Addresses** lists where another device finds it, one per network address of this computer
   (`https://192.168.1.20:8796`; container and virtual-machine bridges are left out), each with
   *Copy*.

### HTTPS and the certificate

Other devices are served HTTPS, with a certificate the service makes itself the first time it is
needed and keeps (`server-cert.pem`, and its key in `server-key.pem`, readable by you alone). No
authority vouches for it, so a browser warns the first time. Before accepting it, open the
certificate's details in the browser's warning and check that its **SHA-256 fingerprint** is the
one Settings shows under **Certificate**: the same, pair for pair, means the browser is talking to
your service and not to something in between. A browser told to trust it keeps trusting it, since
the certificate stays the same across restarts. Deleting the two files makes a new one (with a new
fingerprint) the next time network access starts.

The app on the same computer keeps plain HTTP at `127.0.0.1`: the service tells the two apart by
the first byte of each connection (a TLS handshake or an HTTP request) on the one port.

**Plain HTTP on the network** turns HTTPS off for other devices. Everything they send and
receive, the password included, then crosses the network unencrypted for anyone on it to read:
only on a network you trust.

### Who may do what

- **The app on this computer** uses the token in `server-token`, which only your user can read.
  That token is accepted from a loopback address (`127.0.0.1`, `::1`) only: a copy of it is no use
  from another machine.
- **Another device** signs in with the password (`POST /api/login`) and gets a token of its own:
  256 random bits, shown to it once. From then on it can do what the app does with recordings
  and their settings, but never change network access, the password or the devices: those are the
  app's alone.
- From another machine, nothing at all is answered while network access is off, and nothing over
  plain HTTP while HTTPS is on (a `403` saying to use `https://`).
- Nothing secret is kept as given. The password is kept only as an Argon2id hash
  (`server-password`), device tokens only as SHA-256 hashes (`server-devices.json`); both files are
  readable by you alone, and `server.json` holds no secret.
- Wrong passwords are counted per address (an IPv6 address with the rest of its /64): after five,
  each one makes that address wait before trying again, one second doubling to 15 minutes, and
  `429` with `Retry-After` says how long. Fifteen minutes without a failure, or a right password,
  forgives it.
- No CORS headers are sent, so a page from another site cannot read any answer.

### Signing devices out

**Other devices** lists every device signed in: the name it gave or its browser's ("Firefox on
Android"), where it was last seen from, when it was last used and when it signed in. **Revoke**
signs one out; its token stops working at once. **Changing the password** signs every device out.
A device can also sign itself out (`POST /api/logout`).

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

- `--port N`: listen on port `N` for this run (default: the port in `server.json`, else 8796).
- `--no-sync`: leave the catalogue to the app.

Its files, in zapper's config and data directories (`~/.config/zapper`, `~/.local/share/zapper`
on Linux; `ZAPPER_HOME` moves both):

| File | What |
|---|---|
| `server.json` | port, network access (`network`: `enabled`, `https`), folder, file names, padding, clock track |
| `server-token` | the app's API token, readable by you alone; made on first start |
| `server-password` | the sign-in password's Argon2id hash, readable by you alone |
| `server-devices.json` | the devices signed in, their tokens as SHA-256 hashes, readable by you alone |
| `server-cert.pem`, `server-key.pem` | the HTTPS certificate, and its key readable by you alone |
| `recordings.db` | the recordings (SQLite) |

Logs go to standard error: `journalctl --user -u zapper-server` when installed from the app.
`ZAPPER_FFMPEG_LOG=1` adds FFmpeg's own log of the remux.

### Building it

```sh
cd src-tauri
cargo build --release --no-default-features --features server --bin zapper-server
```

`--no-default-features` leaves out the `gui` feature, so nothing of Tauri, WebKit, GTK or libmpv
is linked; the server needs only `libavformat`, `libavcodec` and `libavutil`. It takes them from
zapper's own LGPL FFmpeg, the one the app's libmpv uses, when `just fetch-libmpv` has unpacked
it into `src-tauri/lib/<platform>/` (headers in `include/`, libraries in `link/` and `runtime/`;
docs/licensing.md); `build.rs` then links them from there and points the binary at them
(`$ORIGIN/../lib/zapper` on Linux, beside the executable on Windows). Without it, a debug build
uses the development files pkg-config finds, and a release build refuses unless
`ZAPPER_SYSTEM_LIBMPV=1`. Settings → Recording → Install looks for `zapper-server` beside the
`zapper` executable, then on `PATH` (`ZAPPER_SERVER_BIN` overrides both).

The packages ship it: `/usr/bin/zapper-server` in the `.deb`, `.rpm` and Arch package, with
FFmpeg's libraries in `/usr/lib/zapper/`, and `zapper-server.exe` beside `zapper.exe` in the
Windows installers. `just package` and `just package-windows` build it first.

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

HTTP with JSON bodies: on `127.0.0.1`, and on the network too while network access is on (HTTPS
there, unless plain HTTP was chosen; see [Network access](#network-access)). Every request except
`GET /api/version` and `POST /api/login` needs `Authorization: Bearer <token>`: the contents of
`server-token` from this computer, or a device's token. Field names are camelCase; times are Unix
seconds; errors are `{"error": "…"}` with a 4xx status (`401` for a missing, wrong or revoked
token, or the app's token from another machine; `403` for what no token changes).

| Request | Answer |
|---|---|
| `GET /api/version` | `{version, apiVersion}` |
| `GET /api/status` | `{version, apiVersion, active: [{id, title, streamId, bytes, since, waiting?}], outputDir, diskFree, diskTotal, now}` |
| `GET /api/recordings` | `[Recording]`, soonest first |
| `GET /api/recordings/{id}` | `Recording` |
| `POST /api/recordings` | schedule: body `{streamId, title, start, stop, backupStreamId?, channelName?, episodeTitle?, season?, episode?, padBefore?, padAfter?}` → `201 Recording`; the same channel and start again returns the existing one |
| `POST /api/recordings/{id}/cancel` | a scheduled one is removed (`{id, cancelled: true}`); one recording stops and keeps what it has (`Recording`) |
| `DELETE /api/recordings/{id}?files=true` | forgets a finished recording; `files=true` deletes its files too. Refused while recording |
| `GET /api/settings` | `{outputDir, naming, namingPattern, padBefore, padAfter, clockTrack, defaultOutputDir}` (padding in seconds; `naming` is `mediaServer`, `byChannel`, `byDate`, `flat` or `custom`, which uses `namingPattern`) |
| `PUT /api/settings` | the same fields (`defaultOutputDir` is ignored; an empty `outputDir` means the default; omitted naming fields mean the default layout). A `namingPattern` that cannot be used is refused with `400 {"error": "file name pattern: …"}`, even while a preset is chosen; returns what was saved |

Network access and devices:

| Request | Who | Answer |
|---|---|---|
| `POST /api/login` | anyone | body `{password, name?}` → `201 {token, device: Device}`; `401` wrong password, `429` (with `Retry-After`) after too many, `403` when no password is set. `name` defaults to the browser's ("Firefox on Android") |
| `POST /api/logout` | a device | signs the device presenting the token out |
| `GET /api/network` | the app | `{enabled, port, https, passwordSet, fingerprint?, addresses, error?, devices: [Device]}` |
| `PUT /api/network` | the app | body `{enabled, port, https}`: moves the listeners at once and returns the same as `GET`. `400` for a port outside 1024–65535 or turning on without a password, `409` for a port in use (nothing changes) |
| `PUT /api/network/password` | the app | body `{password}` (at least 8 characters): sets it, signs every device out, returns the same as `GET` |
| `GET /api/devices` | the app | `[Device]`, most recently used first |
| `DELETE /api/devices/{id}` | the app | signs that device out; returns the same as `GET /api/network` |

"The app" is the `server-token` from a loopback address; a device's token is refused there
(`403`). `Device` is `{id, name, userAgent, created, lastUsed?, address?}`; `fingerprint` is the
certificate's SHA-256 as `AB:CD:…`; `error` says why network access is not working as set (such
as a port taken when the service started, in which case it listens on `127.0.0.1` only).

`Recording`:

```json
{
  "id": 3, "streamId": 1201, "backupStreamId": 1305, "channelName": "One HD",
  "title": "The Evening News", "season": 2, "episode": 5,
  "start": 1793736000, "stop": 1793737800, "padBefore": 60, "padAfter": 300,
  "state": "done",
  "parts": [{"path": "/home/me/Videos/zapper/The Evening News/The Evening News - S02E05 - 2026-11-03 15.00.mkv",
             "streamId": 1201, "bytes": 1210000000, "started": 1793735940, "ended": 1793738100}],
  "notes": ["switched from the main feed to the backup: no data for 10 s"],
  "folder": "/home/me/Videos/zapper/The Evening News"
}
```

`state` is `scheduled`, `recording`, `done` or `failed` (with `error`). `waiting` is set while
a recording is not capturing, such as "waiting for a free connection (1 of 1 in use)".
`backupStreamId` omitted when scheduling means "find one"; the main feed's own id means "none".

## Not yet

- A web client: the app's interface in a browser, served by the service over network access
  (streams proxied, so provider credentials never reach the browser).
- Installing the service on Windows (a logon task) and macOS (a LaunchAgent); waking the
  computer from suspend to record.
- Series rules ("record every episode"), and choosing the backup feed by hand.
- Season and episode from the guide: the catalogue does not keep XMLTV's episode numbers yet, so
  files are named by date unless a client passes them.
- Joining parts whose codecs match into one file.
- A headless package without the GUI's dependencies.
