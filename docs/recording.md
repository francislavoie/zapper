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
     signed in. *Start at boot (Windows service)* installs it instead as a Windows service of the
     same name, which starts with the computer and records whether or not anyone is signed in:
     Windows asks once for an administrator's permission, and it runs as an account of its own
     that may use only your zapper folders and recordings folder (not a network drive;
     docs/windows.md has the details). Having no window, it logs to
     `%LOCALAPPDATA%\zapper\zapper-server.log`. Updating zapper stops it to replace it, and it is
     started again afterwards. Uninstalling zapper removes it too
     ([Stopping and updating](#stopping-and-updating)).
   - **macOS:** a LaunchAgent, `~/Library/LaunchAgents/dev.lavoie.zapper.server.plist`, loaded
     into your login session (`launchctl bootstrap`), which starts it when you log in and again
     after a crash, but not after a clean stop. It records while you are logged in. It logs to
     `~/Library/Application Support/zapper/zapper-server.log`, and what launchd catches on its
     standard error to `zapper-server.launchd.log` beside it. The app does not carry
     `zapper-server` on macOS yet: Install uses one on `PATH`, and `zapper-server
     --install-service` installs itself.
2. **Right-click a programme** in the guide, or in the programme search results, and choose
   **Record**. A red dot marks it; it pulses while recording, and turns grey while a recording
   that has ended is saved into its file. Right-click again to cancel, or to stop a recording
   early (what was recorded is kept). What is on the channel playing can be recorded from the
   player too: **R**, the picture's right-click menu, or the panel beside the picture
   ([Recording what you watch](#recording-what-you-watch)).
3. **Recordings** is an item in Live TV's rail (also in the ☰ menu, the command palette and
   the title-bar badge below). It shows the free space on the recordings disk and lists what is
   recording, scheduled, recorded and failed; choosing one shows, beside the picture, its
   details and *Open folder*, *Cancel*/*Stop* and *Delete*, and the channel each
   one falls back to if its own fails ([backup feeds](#how-it-behaves)). While anything records,
   the title bar says so beside Settings ("Recording", "Recording 2"; the dot alone on a narrow
   window): its tooltip lists each one with how long it has recorded and until when, and whether
   it waits for a connection, is on its backup channel or has a conflict ahead, and a click
   opens Recordings.

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

The season and episode come from the guide: an XMLTV guide's `<episode-num>` (an Xtream
server's guide and Tvheadend's are XMLTV too), read in the `xmltv_ns` form (`1.4.` is S02E05,
zero-based) or as printed (`S02E05`, `2x05`, `Season 2 Episode 5`, `E05`), and SiliconDust's
`EpisodeNumber` for an HDHomeRun. A number that is not clean — two episodes (`S02E05-06`), a
year where the season should be (`2025.4.`), two that disagree — is left out, and the file is
named by date as it would be without one.

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

**Show recordings to other programs while they record** (Settings → Recording) has three choices:

- **No** (the default): the file being recorded stays hidden; other programs see the `.mkv` once
  the recording has finished.
- **Growing file** names that file so a media server can see it and play it as it grows:
  `<name> (recording).ts` for the first part, `<name> - part2 (recording).ts` after a switch of
  feed. When the recording ends it is remuxed into the `.mkv` as usual and the `.ts` is removed.
  A media server may list both for a while, the finished `.mkv` and the `(recording).ts` that is
  gone, until it next scans the folder; and a part that is still being written has no clock track
  yet (that is added to the `.mkv`). Recordings keep track of their own files, so turning this on
  or off is safe at any time: it applies from the next part recorded, and a recording that was
  interrupted is finished the same way whatever its files are called. Jellyfin plays such a file
  only as far as it went when Jellyfin looked at it; the next choice is for Jellyfin.
- **Jellyfin (.strm)**: see below.

### Watching a recording in Jellyfin while it records

Jellyfin cannot keep following a library file that is still growing: it plays it to the length it
measured. With **Jellyfin (.strm)** chosen, the file being recorded stays hidden, and while the
recording records the service keeps a small text file beside where its `.mkv` will be:

```text
<folder>/<Show>/[RECORDING] <Show> - 2026-11-03 20.00.strm
```

(square brackets, since Windows does not allow `<>` in a name; ` (2)` when two recordings of one
airing share a name). It holds one line, the address of the recording's live stream on the
service:

```text
https://192.168.1.20:8796/api/recordings/42/live?key=3f9c…
```

Jellyfin treats a `.strm` as a stream: played, it gets the recording from its first byte, and
then the service follows the file as it is written, like `tail -f`, until the recording ends;
then the stream ends. A recording that switched feeds carries on from its first part into the
next, with the new feed's packets as they came: the player sees a jump in time, and if the backup
feed's streams differ (another resolution or codec), some players stop there and need starting
again. When the recording finishes its `.mkv` lands as usual and the `.strm` is removed.

- **The address** is the one Jellyfin reaches the service by. With network access on it is this
  computer's address on the network (the first of those Settings lists), over HTTPS unless plain
  HTTP was chosen. With network access off it is `127.0.0.1`, which only a Jellyfin on this
  computer reaches; turn network access on (it needs the sign-in password) for a Jellyfin
  elsewhere. **Address in .strm files** takes a name or address of your own instead
  (`nas.local`, `192.168.1.20`; the host alone, the service's port is added), for a computer with
  several networks or a name Jellyfin knows it by. A change of address, or of network access, is
  written into the `.strm` files of recordings in progress at once.
- **The key** in the address is the recording's own: 256 random bits, made when its `.strm` is
  first written, kept by the service only as a SHA-256 hash, and good for that recording's live
  stream alone (no other recording, no other request), while it records and for a minute after it
  finishes; after that the address answers `410`. It needs no sign-in, so **anyone who can read
  the recording's folder can watch that recording while it records** — keep the folder as private
  as the recordings in it. Wrong keys are counted per address and slowed down like wrong
  passwords (separately from them, so a Jellyfin replaying an old address cannot lock its computer
  out of signing in). The key is never written to the log. Over plain HTTP the stream and the key
  cross the network unencrypted.
- **HTTPS.** The service's certificate is its own, which no authority vouches for. Jellyfin's
  FFmpeg does not check certificates by default; a Jellyfin client that is handed the address
  directly (direct play) may refuse it. If a client cannot play the `.strm`, choose plain HTTP on
  a network you trust, or have Jellyfin transcode it.
- **After a restart** of the service the `.strm` keeps its key, so what Jellyfin read from it
  stays good; one whose recording finished or failed while the service was down is removed when
  it starts again, and one whose recording carries on is written again for the address as it is
  now.

What to expect in Jellyfin:

- No runtime is shown until it is played, and the length grows as it plays.
- Seeking far ahead may not work: the stream can only start where the recording has got to.
  `Range: bytes=N-` resumes from byte N of what is written; nothing past it can be asked for.
- The swap from the `.strm` to the `.mkv` shows after a library scan, or at once with real-time
  monitoring on for the library. Until then Jellyfin may list both, or still the `.strm` that is
  gone.
- Watched state and the position reached in the `.strm` do not carry over to the `.mkv`: Jellyfin
  sees two different items.
- Only the parts still being captured are in the stream. A recording interrupted by the service
  stopping cleanly has what it recorded before finished into `… - part1.mkv` already (see
  [Stopping and updating](#stopping-and-updating)); its stream starts with the part after.

This was checked with FFmpeg and ffprobe reading the stream; not with Jellyfin itself.

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
- **A media server** with a recording's `.strm` file reads that recording's live stream with the
  key in it, and nothing else, while it records ([Watching a recording in Jellyfin while it
  records](#watching-a-recording-in-jellyfin-while-it-records)).
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

- browse the categories and channels, the guide, the favourites and what was watched
  recently, and search channels (`a|b`
  matches either) and programmes;
- watch live TV: choosing a channel plays it in the page, with the same picture band, player,
  transport, volume, fullscreen and audio tracks as the app ([Watching in the
  browser](#watching-in-the-browser));
- schedule a recording from the guide's or the search results' right-click menu, and see, cancel
  and delete recordings (**Recordings** in the rail, and in the menu);
- change what the service does with recordings (Settings → Recording), as the app can;
- set the guide's hours per screen and how times are written, which that browser keeps for itself;
- refresh the catalogue from the provider (Settings → Data, the command palette, or the title
  bar's retry), and follow the refresh in the title bar until the lists show what it brought;
- change the provider account and test it (Settings → Account), for every kind the app takes;
- sign out.

What belongs to the computer the app runs on is left out rather than shown and refused: mpv's
settings (Settings → Playback, Audio and Subtitles, and how many streams the account allows),
stats for nerds, the window, the media keys, the updater, backups, installing the service,
opening a folder, and network access, the password and the devices.

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
  styles (no inline script anywhere), loads images, data and media only from the service (media
  also from `blob:`, the address the page's player gives the Media Source it fills from the
  service's stream), and lets no page frame it (`frame-ancestors 'none'`); and `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`. Still no CORS headers.
- **Provider addresses stay in the service.** A channel's logo is fetched by the service and served
  as `/api/logo/{streamId}` (an image only, up to 1 MB, under a policy that runs nothing if opened
  on its own), so the browser never sees the provider's URL, and stream addresses, which carry the
  account's password, are never sent. A channel is watched by its id: the service opens the
  stream itself and hands the browser a viewer of its own to read (`/api/stream/{viewer}`), which
  only the device that started it may read, and only from the service's own page.
- A browser that asks for a page over plain HTTP while HTTPS is on is sent to the same page over
  HTTPS; the API is refused there, as before.

### Watching in the browser

Choosing a channel in the page plays it there. The page asks the service to watch the channel
(`POST /api/watch`, by its id), and the service streams it back as MPEG-TS from its own address,
which the page's player ([mpegts.js](https://github.com/xqq/mpegts.js), bundled in the page)
feeds to the browser's video element through Media Source Extensions.

**Connections.** The browser takes part in the [sharing rules](#sharing-connections) as one more
rider on the service's connection to the channel, one whose programme never ends:

- Watching a channel the service already has open, because it is recording it or another browser
  watches it, joins that connection and costs the account nothing.
- Otherwise it needs a connection of its own, asked of the same count the recordings ask. On a
  full account the page says why instead of playing:
  - **Recordings hold the connections**: "The account's connection is recording", naming the
    recording and its channel, which can be watched for nothing. Nothing is offered to override it:
    a recording's connection is never taken.
  - **Another browser watches through the service**, or **another device** has the account's
    connections: "Your account is in use on another device", with **Watch here anyway**, as in the
    app. Pressing it ends the other browser's stream (it is told why), or, when the service holds
    no connection at all, connects as the app's player would and leaves the provider to end
    whichever other device it ends. Where a recording holds one of the connections, it is not
    offered: connecting regardless could end the recording's rather than the other device's.
  - The same browser (a reloaded page, a second tab) is not asked: its newer stream takes the
    connection from its older one, and changing channel hands the old channel's connection to the
    new one.
- Leaving (stopping, changing channel, closing the page) gives the connection up: the service lets
  go of a stream nobody reads within 15 seconds.

**A recording that needs the connection.** A browser is never cut off for a recording, so on an
account with no connection to spare a recording due on another channel would wait for it. The page
says so instead: from two minutes before the recording's padded start, and for as long as it
waits, a card names it ("“The Match” starts recording on Sport One at 21:00 and needs this
connection") with **Watch Sport One**, which shares one connection with the recording, and **Stop
watching**. Kept watching, the recording waits and starts the moment the browser lets go, and says
in its notes that a browser was watching; a page in a background tab lets go by itself as soon as
a recording waits for it. A recording of the channel being watched simply joins its connection.

**What a browser can play.** The service reads the stream's own tables (its first packets) before
answering, and decides from them, never from a channel's name:

- The video is never re-encoded. H.264 plays in every browser with Media Source Extensions, HEVC
  where the browser says it can; a channel whose video is MPEG-2 (much broadcast SD, and most
  over-the-air TV through an HDHomeRun) says "Not playable in the browser" and why. What a channel
  carries is remembered for six hours, so asking again costs no connection.
- Sound a browser cannot play (MPEG-1 Layer II, and AC-3 or E-AC-3 where the browser lacks them)
  is converted to stereo AAC on the way by FFmpeg's own AAC encoder (LGPL, in zapper's bundled
  FFmpeg); AAC and the sound the browser does play go as they are. Another audio track goes through
  the same remuxer, which keeps only the chosen one. A zapper-server whose FFmpeg has no AAC
  encoder plays such a channel without its sound, and the page says so.
- Subtitles are not shown in the browser.

**The player.** Volume and mute are the page's own, never the computer's; the compressor is the
browser's own. Fullscreen is the browser's. Rewinding is what the browser holds: up to the last two
and a half minutes, through which back, forward, pause and Live move. Paused for more than three
minutes, the stream is let go (and the account's connection with it), and Play starts again at live.
The provider's archive (catch-up) plays in the app, not in the browser. A browser without Media
Source Extensions (an iPhone or iPad before iOS 17.1) says it cannot play live TV.

**Limits.** A device may hold three streams at once and start forty a minute; the service streams
to sixteen browsers at most.

### Recording what you watch

**Record from the player.** Right-click the picture and choose **Record “…”**, use the **Record**
button under the channel's name in the panel beside the picture, or press **R** (also in the
command palette). It records the programme the guide says is on now on the channel playing,
padding included. Once it records, the same places say **Recording** and offer **Stop** (R alone
never stops a recording: it says it is already recording). In the app and in a browser.

**One connection for the picture and the recording.** The app's player connects to the provider
by itself, and the service never takes a viewer's connection ([rule 5](#sharing-connections)), so
on an account with no connection to spare a recording of the channel you watch would wait until
you stopped watching. When the service runs on this computer, Record on the channel playing live
moves the player onto the service's connection instead (`recording::handover`):

- The player lets its own connection go, the service connects to the channel as the player did
  (asked as "Watch here anyway" is: the provider may still be counting the player's connection
  for a moment, and a refusal then is asked again twice, four seconds apart), and the player
  plays the service's stream (`POST /api/watch` with `player`, read from `127.0.0.1` with the
  app's token sent as a header, never in the address). Then the recording is scheduled, and rides
  that connection from its first moment: one connection for both. The picture stops for the
  moment that takes; the volume and the audio and subtitle tracks chosen are kept, and the rewind
  buffer starts again (what it held is kept, below).
- It happens when the account has no connection beyond the player's (an Xtream account's count,
  the playlist's "Simultaneous streams", an HDHomeRun's tuners; one that cannot be asked counts as
  full), or when the service already records something on that channel (joining costs nothing).
  On an account with a connection to spare nothing changes: the service connects for itself.
- A recording that starts later, while you watch its channel, and waits for the connection the
  player holds, moves the player the same way when the app next fetches the recordings (every
  few seconds while anything records, every half minute otherwise).
- When the service is not running, Record on a full account says "Can't record while you watch
  on a 1-stream account unless the recording service runs on this computer" rather than leaving
  the recording waiting.
- Afterwards the player stays on the service's connection, alone on it once the recording ends:
  it costs the account the same one connection. The next channel change decides afresh, and
  catch-up programmes always play from the provider. Changing channel, stopping, or a catch-up
  gives the service's place back at once, so the connection closes before the player opens
  another.
- Tuning to any channel the service is recording joins its connection the same way
  (`onlyShared`: never a new one), so watching what records is free on an account of one stream.
  The app learns what the service records from the list it fetches anyway, so tuning costs no
  extra request otherwise. Should that connection end, the player rejoins it if the service
  connected again, or else reconnects on its own as it would after any network failure.

**What you have already watched is kept.** The player keeps the last minutes of the channel to
rewind through (Settings → Playback, *Rewind buffer*). Recording the programme on now, the app
writes out what that buffer holds of it, from the padded start or as far back as the buffer
reaches (mpv's `dump-cache`, the packets as they are), and hands it to the service
(`PUT /api/recordings/{id}/prelude`), which keeps it as the recording's first part: the file plays
as `… - part1.mkv`, and what the service received follows as `… - part2.mkv` ([Files](#files)).
Its clock track reads the time each moment was received, the same as a part the service captured:
the service reads the part's timeline and places it from when its first packet arrived. A note on
the recording says how much came from the player. Nothing is kept from a catch-up programme, and
nothing when the recording already has what the buffer holds.

The buffer arrives after the recording has begun, and goes in front of what the service has
received. So it is kept out of what other programs follow while the recording records: it is
always a hidden capture file, whatever *Show recordings to other programs while they record*
says, and the `.strm`'s live stream ([Jellyfin](#watching-a-recording-in-jellyfin-while-it-records))
leaves it out. Put in front of a stream already being read, it would move every byte the reader
had been sent: a player resuming with `Range` would land somewhere else, and one reading on
would be sent the start after the middle. So Jellyfin following a recording sees what the
service received, and the buffer is in the finished `… - part1.mkv`. The recording lists it
with `handedOver: true`.

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
   waits for it as before; when that player is the app's on this computer, watching the
   recording's channel, the player moves onto the service's connection instead
   ([Recording what you watch](#recording-what-you-watch)). So is one a browser watches through
   the service, whose page is told that the recording waits
   ([Watching in the browser](#watching-in-the-browser)). A recording
   past its own programme's end does not take another recording's padding for its own.

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
one, a backup. Scheduling picks it, best first:

1. another channel carrying the same guide channel; among several, one in the same category as
   the main channel, then the one playing has shown to be better (not failing the last time it
   was opened, then the higher resolution, then the higher frame rate; a channel never opened
   comes after one that was), then the lower channel number;
2. only when no channel shares the guide channel, one whose guide has the same programme — the
   same title, ignoring case and spacing, starting within five minutes of the recording. A
   title that names a kind of programme rather than one ("News", "Movie", "Weather", anything
   under four letters) matches nothing, and neither does one on more than ten guide channels at
   once.

It can be chosen by hand too. In the guide, right-click a programme on another channel at the
same time as a recording and choose **Use as backup for “…”** (or **Stop using as backup**). In
**Recordings**, each scheduled or recording row says its backup, and *Change…*, in the panel
beside the picture when the row is selected, lists the
candidates above, best first, with *No backup*. Any channel will do, not only a candidate. A
change while recording takes effect the next time a feed fails: a feed that is delivering is
kept, and the recording notes the change. It switches only when the
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
recording or being saved into its file, and nothing starts within five minutes (padding
included); `GET /api/status` says
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
zapper-server --import-account FILE | --set-password | --network on|off [--home DIR]
```

- `--port N`: listen on port `N` for this run (default: the port in `server.json`, else 8796).
- `--no-sync`: leave the catalogue to the app.
- `--home DIR`: keep everything in `DIR`, as `ZAPPER_HOME` does. The service is given it when
  the app that installed it runs on a home of its own.
- `--install-service`, `--remove-service`: what Settings → Recording's Install and Remove do,
  with this `zapper-server`: for a computer without the app's window. `--boot` (Windows): the
  boot-time Windows service rather than the sign-in task, after a UAC prompt. That service runs
  `zapper-server --service <name>` with `--config-dir`, `--data-dir`, `--cache-dir` and
  `--recordings-dir` naming your folders (also `ZAPPER_CONFIG_DIR` and the like, each one
  directory itself; `ZAPPER_HOME` comes first).
- `--busy`: whether this user's running service is recording; exit status 3 and "recording until
  21:05" if it is, 0 otherwise. `--stop-for-setup`: stops it cleanly now and waits for it to go;
  with `--ask`, asks first (a message box on Windows) when it is recording, and exits 2 if not.
  The Windows installers use both.
- `--import-account FILE`: makes the provider account in `FILE` (an `account.json` copied from
  zapper's config directory on another computer; `-` reads standard input) the one in use, as
  Settings → Account's Save would. `--set-password`: sets the password other devices sign in
  with, read from standard input (twice, without echo, at a terminal; else its first line), never
  from the command line, where other users could read it. `--network on|off`: whether other
  devices may reach it, which needs that password. For a service with no app beside it
  ([Without the app](#without-the-app)); each goes through the running service's API when it
  answers with this home's token, so it applies at once (a new password signs every device
  out), and otherwise writes the files it reads when it starts.

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

### Without the app

Each release also carries `zapper-server-<version>-linux-x86_64`, the service alone for Linux
without a desktop (a home server, a NAS, a container), as one static file: built for
`x86_64-unknown-linux-musl`, with FFmpeg linked in, it loads nothing at run time, so it runs on
any x86-64 Linux, glibc or musl, and in a container with nothing else in it. It takes two things
from the system when there is one: the certificate authorities, to reach a provider over HTTPS
(the system's own, in `/etc/ssl`, so that they follow its updates; without any it starts but
cannot reach a provider), and the time zone (`/etc/localtime`, or `TZ` with `tzdata`).
`zapper-server --licenses` prints the notices of everything built into it.

[packaging/server/](../packaging/server/) has its README (running it, `ZAPPER_HOME`, the port,
the first-time setup with `--import-account`, `--set-password` and `--network on`), a sample
systemd system unit (a `zapper` system user, everything in `/var/lib/zapper` and write access
there alone) and a sample `Dockerfile` (distroless's static base and the binary, run as a
non-root user, not published anywhere).

How it is made (`just package-server`, `scripts/build-server-static.sh`), in a pinned Rust image
on Alpine, whose C library is musl:

- `scripts/build-ffmpeg-static.sh` builds FFmpeg's libavformat, libavcodec, libswresample and
  libavutil as static libraries from the tarball `packaging/libmpv/sources.txt` pins, LGPL, with
  only what the service uses: MPEG-TS in and out and Matroska out, from files and its own I/O
  (no network protocols), the parsers and bitstream filters those need, the decoders of the
  audio a browser cannot play (AC-3, E-AC-3, MP1/2/3) with FFmpeg's own AAC encoder and
  libswresample to turn it into AAC, and nothing found on the build machine: no hardware
  acceleration, no programs, no documentation;
- `build.rs` links them statically when the target is musl (`ZAPPER_FFMPEG_STATIC` names them),
  while the desktop builds keep the shared bundle, and compiles in the notices
  `ZAPPER_SERVER_NOTICES` names: zapper's license, FFmpeg's notice with the LGPL's text, musl's
  `COPYRIGHT`, and the Rust crates' and the interface's notices.

`just check-server-binary` (`scripts/check-server-binary.sh`) checks it: `file` and `ldd` call it
statically linked; in bare `debian:stable-slim` and `alpine` containers it says its version and
licenses, starts on a scratch home, answers `GET /api/version` and serves the web interface,
takes the setup commands and stops cleanly on SIGTERM; it runs `FROM scratch`; the sample
Dockerfile's image runs it as its README says; and `recording_check`, built for the same target,
records the fixture streams through it, each remuxed into a Matroska file ffprobe reads. Its
build runs the browser feed's tests (`recording::livemux`: an AC-3 fixture transcoded to AAC)
against the same static FFmpeg first. The release workflow builds and checks it in a job of its
own.

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

`handover_check` (built with the app's features: `cargo build --features harness,server --bin
zapper-server --bin handover_check`) records what a headless, silent mpv set up as the app's
plays, on a playlist of one stream: it checks that Record moves the player onto the service's
connection and the player plays from it, that what the service streams to the app is the bytes the
recording writes, that the fixture server sees one connection from then on, that the player's
buffer becomes `… - part1.mkv` with a clock reading when it was received and what the service
received `… - part2.mkv`, and that the connection closes at once when the player lets go.

`service_check` (same features) installs the service the way Settings → Recording does, on a
scratch home it hands the service (`--home`), checks that it answers there, then removes it. It
installs a real service for you for those few seconds, so it refuses when one is installed
already.

## API

HTTP with JSON bodies: on `127.0.0.1`, and on the network too while network access is on (HTTPS
there, unless plain HTTP was chosen; see [Network access](#network-access)). Every request except
`GET /api/version`, `GET /api/session`, `POST /api/login` and a recording's live stream (which
carries its own key) needs `Authorization: Bearer <token>`:
the contents of `server-token` from this computer, or a device's token; or, from a browser, the
cookie signing in set (see [Web access](#web-access) for what requests relying on it must also
carry). Field names are camelCase; times are Unix
seconds; errors are `{"error": "…"}` with a 4xx status (`401` for a missing, wrong or revoked
token, or the app's token from another machine; `403` for what no token changes).

| Request | Answer |
|---|---|
| `GET /api/version` | `{version, apiVersion}` |
| `GET /api/status` | `{version, apiVersion, active: [{id, title, streamId, bytes, since, until, waiting?, finishing?}], outputDir, diskFree, diskTotal, now, idle, idleAt?, nextStart?}` ([Stopping and updating](#stopping-and-updating)) |
| `GET /api/recordings` | `[Recording]`, soonest first |
| `GET /api/recordings/{id}` | `Recording` |
| `POST /api/recordings` | schedule: body `{streamId, title, start, stop, backupStreamId?, channelName?, episodeTitle?, season?, episode?, padBefore?, padAfter?}` → `201 Recording`; the same channel and start again returns the existing one |
| `POST /api/recordings/preview` | the same body: `[Conflict]` scheduling it would bring, nothing scheduled. Those of the one asked about, and with `other: true` those of recordings already scheduled |
| `GET /api/recordings/{id}/backups` | `{current?, candidates: [Backup]}`: the backup now and the channels it could be, best first (at most 20). `Backup` is `{streamId, name, num?, numLabel?, reason, height?, failing?}`, `reason` `guide` (same guide channel) or `programme` (same programme at the same time) |
| `PUT /api/recordings/{id}/backup` | body `{streamId}`, or `{streamId: null}` for none: chooses the backup of a scheduled or recording one → `Recording`. `400` for its own channel, a channel the catalogue does not list (or a divider), or a recording that has finished. While recording, used from the next time a feed fails |
| `POST /api/recordings/{id}/cancel` | a scheduled one is removed (`{id, cancelled: true}`); one recording stops and keeps what it has (`Recording`) |
| `DELETE /api/recordings/{id}?files=true` | forgets a finished recording; `files=true` deletes its files too. Refused while recording |
| `GET /api/recordings/{id}/live?key=` | no sign-in: the key from the recording's `.strm` ([Watching a recording in Jellyfin while it records](#watching-a-recording-in-jellyfin-while-it-records)). The recording as MPEG-TS (`video/mp2t`) from its first byte, following its parts as they are written, ending when it stops recording. `Range: bytes=N-` → `206` from byte N on (`Content-Range` names what was written when it answered), `bytes=N-M` → that slice as written now, `416` past what is written. `401` for a missing or wrong key (`429` with `Retry-After` after too many), `410` once the recording has finished, `503` when too many streams are read at once |
| `GET /api/settings` | `{outputDir, naming, namingPattern, padBefore, padAfter, clockTrack, whileRecording, showWhileRecording, strmHost, strmBase, defaultOutputDir}` (padding in seconds; `naming` is `mediaServer`, `byChannel`, `byDate`, `flat` or `custom`, which uses `namingPattern`; `whileRecording` is `hidden`, `visible` or `strm`, and `showWhileRecording` is `true` for `visible`, for older clients; `strmHost` is the host chosen for `.strm` files, empty for the service's; `strmBase` is where they point now) |
| `PUT /api/settings` | the same fields (`defaultOutputDir` and `strmBase` are ignored; an empty `outputDir` means the default; omitted naming fields mean the default layout; without `whileRecording`, `showWhileRecording` turns the growing file on or off). A `namingPattern` that cannot be used is refused with `400 {"error": "file name pattern: …"}`, even while a preset is chosen, and a `strmHost` that is not a bare host name or address likewise; returns what was saved |

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
| `PUT /api/recordings/{id}/prelude?firstMs=` | the app | body: MPEG-TS, what the player had buffered of the programme, whose first packet arrived at `firstMs` (Unix milliseconds) → `201 Part`: kept as a part in front of what was captured since ([Recording what you watch](#recording-what-you-watch)). `400` for anything not a transport stream the remuxer reads, `409` once the recording is being saved or is over |

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
| `GET /api/recent?limit=` | the channels watched most recently, newest first |
| `GET /api/sync` | `{phase, message, channelsTotal, lastSynced}`; `POST /api/sync` asks for a refresh (`202`) |
| `GET /api/logo/{id}` | the channel's logo, fetched by the service |
| `GET /api/account` | the provider account as the app's `AccountDto`, with no secret in it (`hasPassword`, `tvheadendHasPassword`; addresses with `<redacted>` in place of a login, flagged `playlistRedacted`, `epgRedacted`, `hdhomerunEpgRedacted`) and an empty `path` |
| `PUT /api/account` | saves a form and makes its kind the one in use: `{kind: "xtream", host, username, password}`, `{kind: "m3u", playlist, epgUrls, userAgent}`, `{kind: "hdhomerun", device, epgUrls}` or `{kind: "tvheadend", url, username, password}`. An empty password, or `null` for `playlist` or `epgUrls`, keeps what is on file. Asks for a sync; returns the same as `GET`. `400 {"error"}` with what is wrong, never with what was sent |
| `POST /api/account/test` | the same body, tested as typed and not saved (`{result}`, or `400 {"error"}`); with no body, the account on file |

Watching (the app or a device; a viewer is its device's alone, `404` to anyone else):

| Request | Answer |
|---|---|
| `POST /api/watch` | body `{streamId, anyway?, replaces?, audio?, can?: {hevc, ac3, eac3}, player?, onlyShared?}` (`can`: what the browser decodes beyond H.264 and AAC; `replaces`: the viewer this one follows, which is stopped first; `player`: the app's own player, the app's alone, which is given the stream as it comes with every track and nothing refused for its codecs; `onlyShared`: only a connection the service already has, never a new one) → `{viewer, stream, video, audio: [{index, codec, lang, described, impaired}], audioIndex, converting, silent?, shared, notice?}`. Refused with `{error, reason, anyway, slots?, ending?, watchInstead}`: `409` `recording`, `viewers` or `inUse` (the account is full; `anyway` says whether "Watch here anyway" may be offered), `409` `notOpen` (`onlyShared`, and the service has no connection to the channel), `422` `notPlayable`, `429` `limit`, `502` `feed` (the channel did not start; `ending` as the app's player reports it), `404` `channel`, `403` for `player` from a device |
| `GET /api/stream/{viewer}?audio=` | the stream, `video/mp2t`, for as long as it is read; `audio` switches to another track (from live). Refused when the browser says the request came from another site |
| `GET /api/watch/{viewer}` | `{ended?, takenOver, notice?}`: why it stopped, and the recording it is in the way of (`{recordingId, title, channel, streamId, from, start, waiting}`) |
| `DELETE /api/watch/{viewer}` | stops it at once (`204`) |

"The app" is the `server-token` from a loopback address; a device's token is refused there
(`403`). `Device` is `{id, name, userAgent, created, lastUsed?, address?}`; `fingerprint` is the
certificate's SHA-256 as `AB:CD:…`; `error` says why network access is not working as set (such
as a port taken when the service started, in which case it listens on `127.0.0.1` only).

`Recording`:

```json
{
  "id": 3, "streamId": 1201, "backupStreamId": 1305, "backupName": "One SD", "channelName": "One HD",
  "title": "The Evening News", "season": 2, "episode": 5,
  "start": 1793736000, "stop": 1793737800, "padBefore": 60, "padAfter": 300,
  "state": "done",
  "parts": [{"path": "/home/me/Videos/zapper/The Evening News/The Evening News - S02E05 - 2026-11-03 15.00.mkv",
             "streamId": 1201, "bytes": 1210000000, "started": 1793735940, "ended": 1793738100}],
  "notes": ["switched from the main feed to the backup: no data for 10 s"],
  "folder": "/home/me/Videos/zapper/The Evening News"
}
```

`state` is `scheduled`, `recording`, `finishing`, `done` or `failed` (with `error`).
`finishing` is a recording no longer capturing — stopped, or its window over — that is being
turned into its files, which for a long one takes a minute or two; it is `finishing` from the
moment it is stopped, and `active` in the status says `finishing: true`. `waiting` is set while
a recording is not capturing, such as "waiting for a free connection (1 of 1 in use)".
`conflicts`, when there are any still ahead, is `[Conflict]`: `{title, with, kind, secs,
streams}`, what recording `title` loses to `with` on another channel when the account allows
`streams` — `kind` is `cutEnd` (its padding after ends `secs` early), `lateStart` (it starts
`secs` into its padding before), `lostStart` (`secs` into its programme) or `missed`.
`backupStreamId` omitted when scheduling means "find one"; the main feed's own id means "none".
`backupName` is the backup's channel name, when the catalogue has it. A part with
`handedOver: true` is what the app's player had buffered ([Recording what you
watch](#recording-what-you-watch)).

## Not yet

- In the browser: rewinding further than the browser holds (a buffer kept by the service), the
  provider's archive, subtitles, and MPEG-2 video (it would need re-encoding).
- Waking the computer from suspend to record
  (on Windows, a task per recording with a wake timer).
- Series rules ("record every episode").
- Joining parts whose codecs match into one file.
