# Recording

zapper records through a separate, headless program, **zapper-server**: the recording
service. It runs in the background whether or not the app is open, wakes when a recording is
due, and saves it as a Matroska file a media library such as Jellyfin can play. The app
schedules recordings from the guide and shows them; the service does the recording.

## Using it

1. **Settings → Recording → Install** runs the service on this computer, as you, with no
   administrator or root needed. **Remove** stops and removes it and keeps your recordings.
   - **Linux:** a systemd user service. *Keep running when logged out* keeps it recording while
     nobody is logged in (`loginctl enable-linger`).
   - **Windows:** a scheduled task, `zapper-server-<your user name>`, that starts
     `zapper-server.exe` when you sign in and is started right away. It records while you are
     signed in; recording with nobody signed in would need your password stored with the task, so
     it isn't offered. Having no window, it logs to `%LOCALAPPDATA%\zapper\zapper-server.log`.
     Updating zapper stops it to replace it, and zapper starts it again when it reopens.
     Uninstalling zapper removes the task too ([Stopping and updating](#stopping-and-updating)).
   - **macOS:** not yet; run `zapper-server` yourself.
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
been) and the recording says so. Two recordings that come out with the same name (the same
airing on two channels) never share a file: the second one's gets a ` (2)`.

**Show recordings to other programs while they record** (Settings → Recording, off by default)
names that file so a media server can see it and play it as it grows: `<name> (recording).ts`
for the first part, `<name> - part2 (recording).ts` after a switch of feed. When the recording
ends it is remuxed into the `.mkv` as usual and the `.ts` is removed. A media server may list
both for a while, the finished `.mkv` and the `(recording).ts` that is gone, until it next scans
the folder; and a part that is still being written has no clock track yet (that is added to the
`.mkv`). Recordings keep track of their own files, so turning this on or off is safe at any time:
it applies from the next part recorded, and a recording that was interrupted is finished the
same way whatever its files are called.

### The clock track

An optional subtitle track, titled *Clock*, showing the wall-clock time (`HH:MM:SS`) each moment
was live, small in the bottom-right corner. It is never marked as a default track, so players
leave it off until you choose it. The time comes from when the stream's bytes arrived, carried
through the stream's own timestamps, so it ticks smoothly while the feed does and jumps where the
recording has a gap (a reconnect, a switch of feed). Turn it off in Settings → Recording.

## Network access

Off by default: the service listens on `127.0.0.1` only, for the app on the same computer.
**Settings → Recording → Network access** lets other devices on your network reach it: zapper's
interface in a browser ([Web access](#web-access)), and its API.

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
  and their settings, and read the channels and the guide, but never change network access, the
  password or the devices: those are the app's alone.
- **A browser** signs in the same way and gets the same kind of token, in a cookie its page cannot
  read ([Web access](#web-access)). The cookie only ever stands for a device: the app's token in
  one is refused.
- A device may also change the provider account and test it (`/api/account`), as Settings →
  Account does: the service keeps the catalogue from it. It is never sent a saved password, and
  a blank one keeps what is on file.
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

## Web access

With network access on, opening one of the addresses Settings lists (`https://192.168.1.20:8796`)
in a browser shows zapper's own interface, after a sign-in page asking for the password. On the
service's own computer, `http://127.0.0.1:8796` works too, once a password is set.

What a browser can do there:

- browse the categories and channels, the guide and the favourites, and search channels (`a|b`
  matches either) and programmes;
- schedule a recording from the guide's or the search results' right-click menu, and see, cancel
  and delete recordings (**Recordings** in the menu);
- change what the service does with recordings (Settings → Recording), as the app can;
- set the guide's hours per screen and how times are written, which that browser keeps for itself;
- refresh the catalogue from the provider (Settings → Data, the command palette, or the title
  bar's retry), and follow the refresh in the title bar until the lists show what it brought;
- change the provider account and test it (Settings → Account), for every kind the app takes;
- sign out.

Watching is not there yet: choosing a channel says so. What belongs to the computer the app runs
on is left out rather than shown and refused: the player and its settings (how many streams the
account allows among them), the window, the media keys, the updater, backups, installing the
service, opening a folder, and network access, the password and the devices.

**The account in a browser.** The page is sent the account as the app's own Settings shows it:
whether a password is on file but never the password, and a playlist or guide address with any
login in it shown as `<redacted>`. Leaving a password empty, or such an address untouched, keeps
what is saved; testing tries the form as typed and saves nothing. Saving makes the service sync
at once. The app on the same computer reads the same account file and catalogue: it notices a
change within a few seconds, uses the new account for what it plays next, syncs from it if the
service does not (`--no-sync`), and reads the catalogue again once a sync lands. Over plain HTTP
from another device, the password typed there crosses the network unencrypted, and the page says
so; turn HTTPS back on (Settings → Recording, *Plain HTTP on the network* off) to avoid it.

The interface is the app's, built into `zapper-server` (`src-tauri/build.rs`): the same version as
the service, nothing installed beside it. Its files under `/assets/` are named by their contents
and kept by the browser for a year; the page itself is asked for again each time.

### How a browser is kept safe

- **Signing in** (`POST /api/login` with `"cookie": true`) sets the device's token as a cookie
  marked `HttpOnly` (the page's scripts cannot read it), `SameSite=Strict` (the browser sends it
  only with requests from the service's own pages), `Path=/`, and `Secure` when signed in over
  HTTPS. The answer carries the device, never the token. The browser is listed under **Other
  devices** in the app's Settings like any other device, named after it ("Chrome on Android"), and
  **Revoke** there signs it out; so does **Sign out** in the page, which also clears the cookie.
  Wrong passwords are slowed down as for any device.
- **Requests that change something** (anything but `GET` and `HEAD`) are refused (`403`) when
  they rely on the cookie without the `X-Zapper: 1` header, which a page on another site cannot
  add; when their `Origin` is not the address they were sent to; or when the browser says
  (`Sec-Fetch-Site`) they came from another site. A request with a bearer token needs no header:
  no page can attach one for it. Reading changes nothing.
- **Every answer** carries a content security policy that runs only the service's own scripts and
  styles (no inline script anywhere), loads images and data only from the service, and lets no
  page frame it (`frame-ancestors 'none'`); and `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`. Still no CORS headers.
- **Provider addresses stay in the service.** A channel's logo is fetched by the service and served
  as `/api/logo/{streamId}` (an image only, up to 1 MB, under a policy that runs nothing if opened
  on its own), so the browser never sees the provider's URL, and stream addresses, which carry the
  account's password, are never sent.
- A browser that asks for a page over plain HTTP while HTTPS is on is sent to the same page over
  HTTPS; the API is refused there, as before.

## How it behaves

**Connections.** Most providers allow few streams at once per account, often one, and end the
oldest when another device opens one. Before every connection the service asks whether one is
free — Xtream accounts report their active and maximum connections through the API (an API
call, not a stream); playlists count the service's own streams against the setting; HDHomeRun
and Tvheadend allot their own tuners. When the account is full the recording waits ("waiting
for a free connection"), asking again after 30 s, doubling to five minutes, with jitter. It
never takes the connection from someone watching, and never alternates grabbing it with another
client.

### Sharing connections

Recordings share the account's connections by these rules (`recording::sharing`, which also
foresees the conflicts below):

1. **One connection per channel.** Recordings of the same channel at the same time share one
   connection: the service hands its stream to each recording's file. One that starts while
   another of the channel is recording joins it, and its padding before comes from the live
   stream; one whose end comes leaves, and the connection closes when nobody is left on it.
   Joining costs no connection. So two programmes back to back on one channel are both recorded
   whole, padding and all, on an account of one stream — the second one starts a minute early on
   the first one's connection, and the first one's five minutes after are the second one's first
   five. If the stream fails, each recording on it decides for itself what to do (its own
   [backup feed](#how-it-behaves)), and those that go back to the same channel share again.
2. **A free connection is taken** when the account has one.
3. **On a full account, the next programme's start beats the last one's padding.** When a
   recording needs a connection, the account is full, and one of the service's own connections
   carries only recordings past their programme's end (in their padding after), that connection
   is closed and the recording takes its place. The recordings it carried end there, keep what
   they recorded and say so in their notes ("ended 4 min early for “The Match” on another
   channel…"). Of several such connections, the one with the least padding left goes.
4. **A programme is never cut.** A connection carrying a programme that is still on is never
   closed for another recording. The one that wants it waits, giving up its own padding before,
   and takes the connection the moment that programme ends (rule 3): a recording that waits is
   woken then, and whenever one of the service's own connections closes, rather than at its next
   back-off.
5. **A viewer's connection is never taken.** Only the service's own connections are closed; one
   the account counts for another device or the app's player is left alone, and the recording
   waits for it as before. A recording past its own programme's end does not take another
   recording's padding for its own.

For example, on an account of one stream: *The Film* 20:00–21:00 on one channel and *The Match*
21:00–22:00 on another (with 1 minute before and 5 after): the film is recorded 19:59–21:00 and
the match 21:00–22:05 — the match's minute before gives way to the film's last minute, and the
film's five minutes after to the match. Were the match at 21:02, the film would end at 21:01, the
match's padded start. On the same channel both would be recorded whole.

An account that counts its connections (Xtream) may take a few seconds to notice one closed.
For 20 seconds after the service closes one, a recording that finds the account full looks again
every 3 seconds, and closes no other connection for the same need.

**Conflicts when scheduling.** The Record item in a programme's menu, and each scheduled row in
Recordings, say when recordings on other channels overlap with more connections needed than the
account allows: "Overlaps “The Film” on another channel: starts 1m late", "…: the last 5m will be
cut", or, red when part of a programme is lost, "…and the account allows 1 stream: the first 30m
will be missed" or "…: it can’t be recorded". They play the schedule through the rules above on
the account's limit (Xtream's maximum connections, the playlist's "Simultaneous streams", the
HDHomeRun's tuners; Tvheadend's is unknown, so nothing is said), as if nothing else used the
account: someone watching can still make a recording wait. Overlaps on the same channel are not
conflicts.

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

### Stopping and updating

The service never cuts a recording short without saying so. It counts as **idle** when nothing is
recording and nothing starts within five minutes (padding included); `GET /api/status` says
whether it is (`idle`), and if not, when it will be (`idleAt`: the end of what records now, and
of anything starting too soon after that to stop between).

- **Stopping in the middle of a recording** (`POST /api/shutdown` "now", SIGTERM, Ctrl+C): the
  recording's part is closed and finished into its `.mkv` at once, so what was recorded plays;
  the recording stays *recording*, and if the service runs again while the programme is on it
  carries on in a new part (`… - part1.mkv`, `… - part2.mkv`). Killed outright (a crash, Task
  Scheduler's End), the capture is finished the same way at the next start.
- **Updating on Linux:** the package replaces `/usr/bin/zapper-server` while it runs. The
  service notices (the file at its path is another one), and restarts into the new version at
  the first idle moment. Nothing to do, and nothing interrupted.
- **Updating on Windows:** a running `zapper-server.exe` cannot be replaced. zapper's updater
  asks the service first: if it is recording, the update dialog offers to install when the
  recording finishes (it waits, then installs) or now (stopping it as above). The installers
  ask too when run by hand: the setup shows "zapper's recording service is recording until
  21:05. Stop it now and continue?", and cancels if not; the MSI asks the same (its
  `zapper-server.exe --stop-for-setup --ask`), and if the answer is no, Windows Installer's
  files-in-use dialog follows, which can cancel. Silent installs do not ask.
- **Removing it:** Settings → Recording → **Remove** is the clean way, everywhere.
  - Windows: uninstalling zapper also removes it. The setup's uninstaller stops this user's
    service cleanly and deletes its task and log (not when a newer setup runs it to replace
    this one); the MSI's removal deletes every user's `zapper-server-*` task (an action running
    as SYSTEM, which can see them all) after stopping the installing user's cleanly.
  - Linux: removing the package (not upgrading it) stops a running `zapper-server` with
    SIGTERM, which finishes its recording as above. The unit stays in each user's
    `~/.config/systemd/user/`, which a package removal does not touch; it is skipped from then on
    (`ConditionPathExists`), so it neither fails nor restarts. Settings → Recording → Remove
    (or `zapper-server --remove-service`) deletes it.

**Catalogue.** On the same machine the service and the app share the catalogue database. Either
may refresh it; a lease in the database keeps them from refreshing at the same moment, and both
honour the interval set in Settings → Guide.

## The service

```text
zapper-server [--port N] [--no-sync] [--home DIR]
zapper-server --install-service | --remove-service [--home DIR]
zapper-server --busy | --stop-for-setup [--ask] [--home DIR]
```

- `--port N`: listen on port `N` for this run (default: the port in `server.json`, else 8796).
- `--no-sync`: leave the catalogue to the app.
- `--home DIR`: keep everything in `DIR`, as `ZAPPER_HOME` does. The service is given it when
  the app that installed it runs on a home of its own.
- `--install-service`, `--remove-service`: what Settings → Recording's Install and Remove do,
  with this `zapper-server`: for a computer without the app's window.
- `--busy`: whether this user's running service is recording; exit status 3 and "recording until
  21:05" if it is, 0 otherwise. `--stop-for-setup`: stops it cleanly now and waits for it to go;
  with `--ask`, asks first (a message box on Windows) when it is recording, and exits 2 if not.
  The Windows installers use both.

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
npm run build          # the interface it serves to browsers, compiled in
cd src-tauri
cargo build --release --no-default-features --features server --bin zapper-server
```

The web interface is the frontend's build in `dist/`, compiled into the binary by `build.rs`: a
release build refuses without it, and a debug build without it serves a page saying it was not
built.

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

It then records a second programme and stops the service part way through it (SIGTERM; on
Windows, `POST /api/shutdown` "now"), and checks that `… - part1.mkv` was left playable with the
recording still *recording*, and that starting the service again carries on into
`… - part2.mkv` and finishes it.

`service_check` (same features) installs the service the way Settings → Recording does, on a
scratch home it hands the service (`--home`), checks that it answers there, then removes it. It
installs a real service for you for those few seconds, so it refuses when one is installed
already.

## API

HTTP with JSON bodies: on `127.0.0.1`, and on the network too while network access is on (HTTPS
there, unless plain HTTP was chosen; see [Network access](#network-access)). Every request except
`GET /api/version`, `GET /api/session` and `POST /api/login` needs `Authorization: Bearer <token>`:
the contents of `server-token` from this computer, or a device's token; or, from a browser, the
cookie signing in set (see [Web access](#web-access) for what requests relying on it must also
carry). Field names are camelCase; times are Unix
seconds; errors are `{"error": "…"}` with a 4xx status (`401` for a missing, wrong or revoked
token, or the app's token from another machine; `403` for what no token changes).

| Request | Answer |
|---|---|
| `GET /api/version` | `{version, apiVersion}` |
| `GET /api/status` | `{version, apiVersion, active: [{id, title, streamId, bytes, since, until, waiting?}], outputDir, diskFree, diskTotal, now, idle, idleAt?, nextStart?}` ([Stopping and updating](#stopping-and-updating)) |
| `GET /api/recordings` | `[Recording]`, soonest first |
| `GET /api/recordings/{id}` | `Recording` |
| `POST /api/recordings` | schedule: body `{streamId, title, start, stop, backupStreamId?, channelName?, episodeTitle?, season?, episode?, padBefore?, padAfter?}` → `201 Recording`; the same channel and start again returns the existing one |
| `POST /api/recordings/preview` | the same body: `[Conflict]` scheduling it would bring, nothing scheduled. Those of the one asked about, and with `other: true` those of recordings already scheduled |
| `POST /api/recordings/{id}/cancel` | a scheduled one is removed (`{id, cancelled: true}`); one recording stops and keeps what it has (`Recording`) |
| `DELETE /api/recordings/{id}?files=true` | forgets a finished recording; `files=true` deletes its files too. Refused while recording |
| `GET /api/settings` | `{outputDir, naming, namingPattern, padBefore, padAfter, clockTrack, showWhileRecording, defaultOutputDir}` (padding in seconds; `naming` is `mediaServer`, `byChannel`, `byDate`, `flat` or `custom`, which uses `namingPattern`) |
| `PUT /api/settings` | the same fields (`defaultOutputDir` is ignored; an empty `outputDir` means the default; omitted naming fields mean the default layout). A `namingPattern` that cannot be used is refused with `400 {"error": "file name pattern: …"}`, even while a preset is chosen; returns what was saved |

Network access and devices:

| Request | Who | Answer |
|---|---|---|
| `POST /api/login` | anyone | body `{password, name?, cookie?}` → `201 {token, device: Device}`; `401` wrong password, `429` (with `Retry-After`) after too many, `403` when no password is set. `name` defaults to the browser's ("Firefox on Android"). With `cookie: true` (and the `X-Zapper: 1` header) the token is set as the session cookie instead and the answer is `{device}` |
| `GET /api/session` | anyone | `{signedIn: true, device}` for a signed-in device, `{signedIn: false}` otherwise, `{app: true}` for the app |
| `POST /api/logout` | a device | signs the device presenting the token out, and clears a browser's cookie |
| `GET /api/network` | the app | `{enabled, port, https, passwordSet, fingerprint?, addresses, error?, devices: [Device]}` |
| `PUT /api/network` | the app | body `{enabled, port, https}`: moves the listeners at once and returns the same as `GET`. `400` for a port outside 1024–65535 or turning on without a password, `409` for a port in use (nothing changes) |
| `PUT /api/network/password` | the app | body `{password}` (at least 8 characters): sets it, signs every device out, returns the same as `GET` |
| `GET /api/devices` | the app | `[Device]`, most recently used first |
| `DELETE /api/devices/{id}` | the app | signs that device out; returns the same as `GET /api/network` |
| `POST /api/shutdown` | the app | body `{when: "idle" \| "now", restart?}`: stops (or with `restart` runs itself again) at the first idle moment, or now, finishing what it records into playable files. Answers first, with `GET /api/status` |

The catalogue, for the web interface (the app or a device); channels come as the app's own
`ChannelDto`, with `logoUrl` pointing at `/api/logo/{streamId}` rather than at the provider:

| Request | Answer |
|---|---|
| `GET /api/categories` | `[{id, name, channelCount}]` |
| `GET /api/channels?category=&offset=&limit=` | a page of a category's channels (no `category`: every channel), each with what is on now; `limit` at most 1000 |
| `GET /api/channels/by-id?ids=1,2` | those channels, the ones that still exist |
| `GET /api/channels/numbers` | `[[number, streamId]]`, in list order |
| `GET /api/channels/search?q=&limit=` | channels matching `q` (`a\|b` matches either) |
| `GET /api/channels/{id}/index?category=` | where the channel sits in that listing, or `null` |
| `GET /api/programmes/search?q=&offset=&limit=` | airings whose title matches: on now, upcoming, then catch-up |
| `GET /api/guide?ids=&from=&to=` | `[[streamId, [Programme]]]` overlapping the window (at most 500 channels) |
| `GET /api/guide/rows?ids=&from=&to=` | the guide's rows: each channel's name, logo and programmes |
| `GET /api/favourites?limit=` | the most watched channels; `DELETE /api/favourites/{id}` forgets one |
| `GET /api/sync` | `{phase, message, channelsTotal, lastSynced}`; `POST /api/sync` asks for a refresh (`202`) |
| `GET /api/logo/{id}` | the channel's logo, fetched by the service |
| `GET /api/account` | the provider account as the app's `AccountDto`, with no secret in it (`hasPassword`, `tvheadendHasPassword`; addresses with `<redacted>` in place of a login, flagged `playlistRedacted`, `epgRedacted`, `hdhomerunEpgRedacted`) and an empty `path` |
| `PUT /api/account` | saves a form and makes its kind the one in use: `{kind: "xtream", host, username, password}`, `{kind: "m3u", playlist, epgUrls, userAgent}`, `{kind: "hdhomerun", device, epgUrls}` or `{kind: "tvheadend", url, username, password}`. An empty password, or `null` for `playlist` or `epgUrls`, keeps what is on file. Asks for a sync; returns the same as `GET`. `400 {"error"}` with what is wrong, never with what was sent |
| `POST /api/account/test` | the same body, tested as typed and not saved (`{result}`, or `400 {"error"}`); with no body, the account on file |

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
`conflicts`, when there are any still ahead, is `[Conflict]`: `{title, with, kind, secs,
streams}`, what recording `title` loses to `with` on another channel when the account allows
`streams` — `kind` is `cutEnd` (its padding after ends `secs` early), `lateStart` (it starts
`secs` into its padding before), `lostStart` (`secs` into its programme) or `missed`.
`backupStreamId` omitted when scheduling means "find one"; the main feed's own id means "none".

## Not yet

- Watching in the browser: the service proxying streams to it (so provider credentials never
  reach the browser) and audio the browser cannot play turned into audio it can. Sharing one
  connection between a recording and its viewers is what the service's per-channel connection
  (`recording::hub`) is built for: a viewer would join it as one more subscriber, one the rules
  never cut off.
- Installing the service on macOS (a LaunchAgent); waking the computer from suspend to record
  (on Windows, a task per recording with a wake timer); recording on Windows while nobody is
  signed in.
- Series rules ("record every episode"), and choosing the backup feed by hand.
- Season and episode from the guide: the catalogue does not keep XMLTV's episode numbers yet, so
  files are named by date unless a client passes them.
- Joining parts whose codecs match into one file.
- A headless package without the GUI's dependencies.
