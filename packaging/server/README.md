# zapper-server for Linux, without the app

zapper-server is zapper's recording service: it keeps the channel list and guide current, records
what is scheduled, and serves zapper's web interface to browsers on your network. Each release
carries it alone for a computer with no desktop (a home server, a NAS, a container) as one file,
`zapper-server-<version>-linux-x86_64`: a static x86-64 Linux program with FFmpeg built in, which
loads nothing at run time, so it runs on any distribution, old or new, glibc or musl, and in a
container with nothing else in it. The zapper app on another computer, or any browser, uses it
over the network.

This folder has what goes with it: a sample systemd unit (`zapper-server.service`) and a sample
`Dockerfile`.

Two things it takes from the system, when there is one:

- the **certificate authorities**, to reach a provider or a guide over HTTPS: the
  `ca-certificates` package, there on every distribution but in a bare container image
  (`debian:stable-slim`, `alpine`), in `/etc/ssl/certs`. It uses the system's on purpose, so they
  follow its updates rather than whatever list the binary was built with. Without any, it starts,
  records nothing and says why;
- the **time zone**, to name and schedule recordings in yours: `/etc/localtime`, or `TZ` with the
  time zone database (`tzdata`).

## Trying it

    chmod +x zapper-server-<version>-linux-x86_64
    ./zapper-server-<version>-linux-x86_64 --version
    ./zapper-server-<version>-linux-x86_64

It listens on `http://127.0.0.1:8796` only, until network access is turned on (below). Ctrl+C
stops it; a recording in progress is finished into a playable file first. `--licenses` prints the
licenses of everything in it.

## Where it keeps things

By default in your home, as the app does: settings, the API token, the sign-in password's hash
and the provider account in `~/.config/zapper`, the catalogue and the list of recordings in
`~/.local/share/zapper`, recordings in `~/Videos/zapper`.

`ZAPPER_HOME=/some/dir` (or `--home /some/dir`) keeps everything under one folder instead:
`config/`, `data/` and `cache/` in it, and recordings in `data/recordings` until you choose
another folder. The service files here use `/var/lib/zapper`.

## First-time setup

The app's Settings → Account and Settings → Recording do this on a desktop. Without the app:

1. **The provider account.** Copy `account.json` from a computer where zapper is set up
   (`~/.config/zapper/account.json` on Linux, `%APPDATA%\zapper\account.json` on Windows,
   `~/Library/Application Support/zapper/account.json` on a Mac), then:

       zapper-server --import-account account.json

   It holds the provider's password: keep it readable by you alone, and delete the copy once
   imported (`-` reads it from standard input instead). The service saves it readable by its
   own user only.

2. **A password for other devices**, read from standard input, never from the command line
   (other users can read a program's arguments):

       zapper-server --set-password            # asks twice, without echo
       zapper-server --set-password < pw.txt   # or its first line, from a file or a pipe

3. **Network access**, which needs that password:

       zapper-server --network on

   It then listens on every interface, on the same port, and serves other devices HTTPS with a
   certificate it makes for itself (browsers warn about it once; its fingerprint is in the web
   interface's Settings → Recording). `--network off` goes back to 127.0.0.1 alone.

Each of these talks to the running service when there is one, which takes the change at once (a
new password signs every device out); otherwise it writes the files the service reads when it
starts. Run them as the user the service runs as, with the same `ZAPPER_HOME`.

Then open `https://<this computer>:8796` in a browser, sign in with the password, and schedule
recordings from the guide. The zapper app on another computer can use it too.

## Ports

- **8796** (TCP): the API and the web interface, HTTP on 127.0.0.1 and HTTPS (or HTTP, when HTTPS
  is turned off in the web interface's Settings → Recording) on the network. `--port N` changes it
  for one run; Settings → Recording changes it for good.

Nothing else listens. It connects out to the provider, the guide's addresses and the streams it
records.

## As a systemd service

`zapper-server.service` runs it at boot as a `zapper` system user, with everything in
`/var/lib/zapper`:

    sudo install -m 0755 zapper-server-<version>-linux-x86_64 /usr/local/bin/zapper-server
    sudo useradd --system --home-dir /var/lib/zapper --shell /usr/sbin/nologin zapper
    sudo cp zapper-server.service /etc/systemd/system/
    sudo systemctl daemon-reload
    sudo systemctl enable --now zapper-server

    # the first-time setup, as that user:
    sudo -u zapper ZAPPER_HOME=/var/lib/zapper zapper-server --import-account /path/to/account.json
    sudo -u zapper ZAPPER_HOME=/var/lib/zapper zapper-server --set-password
    sudo -u zapper ZAPPER_HOME=/var/lib/zapper zapper-server --network on

    journalctl -u zapper-server       # its log

The unit lets it write `/var/lib/zapper` and nothing else: to record somewhere else, add that
folder to `ReadWritePaths=` (`systemctl edit zapper-server`) and give the `zapper` user write
access to it, then choose it in Settings → Recording.

To update, replace `/usr/local/bin/zapper-server` with the new release's file. It notices, and
restarts into the new version the next time it is not recording.

## In a container

The `Dockerfile` here builds a small image (distroless's static base, with the certificate
authorities and time zones, and nothing else) from a folder holding it and the release's file
renamed `zapper-server`. It is a sample; no image is published:

    cp zapper-server-<version>-linux-x86_64 zapper-server
    docker build -t zapper-server .
    docker run -d --name zapper-server -p 8796:8796 --stop-timeout 150 -e TZ=Europe/Paris \
        -v zapper-data:/var/lib/zapper -v /srv/recordings:/recordings zapper-server

    docker exec -i zapper-server zapper-server --import-account - < account.json
    docker exec -i zapper-server zapper-server --set-password < pw.txt
    docker exec zapper-server zapper-server --network on

It runs as the image's non-root user (uid 65532), with everything in the `/var/lib/zapper`
volume and recordings in its `data/recordings` until you choose another folder: here
`/recordings`, a folder that uid 65532 may write (`sudo chown 65532 /srv/recordings`), chosen in
Settings → Recording. Network access must be on for the published port to answer: inside the
container, `127.0.0.1` is the container's own. `--stop-timeout` gives a recording in progress
time to be finished into a playable file when the container stops.

## Licenses

zapper-server is under the MIT license. Built into it, and printed by `--licenses`: FFmpeg's
libavformat, libavcodec, libswresample and libavutil, zapper's own build of them with nothing
under the GPL, under the LGPL 2.1 or later (where their source is, and how to rebuild
zapper-server with another FFmpeg, as the LGPL allows); musl, the C library, under the MIT
license; and the Rust crates and the web interface's packages, each under its own license.
docs/licensing.md in zapper's source has the details.

More: https://github.com/francislavoie/zapper (docs/recording.md).
