# Features in detail

The README lists what zapper does; this is how each part behaves. Recording and the browser
interface have their own page: [recording.md](recording.md).

## Sources

- **Xtream Codes:** server URL, username and password. Live channels, categories, guide, catch-up
  on channels that have an archive, and the provider's movies and shows.
- **M3U playlists** from a URL or a file, with one or more XMLTV guides. Catch-up comes from the
  playlist's `catchup` attributes. How an entry is classified:
  - A `.ts`, `.m3u8` or `/live/` address is always a channel.
  - An on-demand address is a movie: a `/movie/` address, a video file (`.mp4`, `.mkv`, `.avi`,
    `.m4v`, `.mov`, `.wmv`, `.webm`), or a `/series/` one.
  - An on-demand entry whose name has an episode number (`S01E02`, `s01.e02`, `1x02`,
    `Season 1 Episode 2`, or a range such as `S01E01-E02`) is an episode instead. Entries in the
    same group whose names match before the number make one show (`A Show S01E01` and
    `A Show S01E02`). An entry whose name starts with the number belongs to a show named after its
    group.
- **HDHomeRun** tuners by address. Channels from the tuner's lineup, guide from SiliconDust or your
  own XMLTV; subchannels shown as `5.1`.
- **Tvheadend** servers, with or without a login. Categories from its channel tags, guide from its
  XMLTV export.
- Movies come from Xtream accounts and from playlists with movie entries, shows from Xtream
  accounts and from playlists with episode entries. HDHomeRun, Tvheadend and playlists without any
  have no Movies or Shows tab: each appears only when there is something to show in it.
- Settings, favourites and category choices can be exported to a file and imported again.

## Live TV

- The channel list and guide are stored locally in SQLite and refreshed in the background.
- The rail on the left has Favourites, Recently watched, Recordings and All channels, then your
  pinned categories, then the rest. Each channel row shows the programme on now and the one next;
  where channels from many categories mix (All channels, Favourites, Recently watched) the row
  also names its category.
- **Recently watched** lists the last 20 channels you watched for at least 30 seconds, newest
  first.
- **List and guide** views (`G`) work for any rail item, Favourites and Recently watched included.
  The guide scrolls through time with Shift+wheel, its scrollbar, or an hour at a time with
  `←`/`→`; `Home` returns to now.
- **Channel search** from the filter box, `/` or the command palette, with results grouped by
  category. `a|b` matches either term.
- **Programme search** finds airings in the guide that are on now, coming up, or still available on
  catch-up. Opening one tunes in, jumps the guide to it, or plays it from catch-up.
- Categories can be pinned, hidden, and sorted by the provider's order, name or size.
- **Recordings** sits in the rail too, with its free disk space and, for the selected recording,
  its actions beside the picture.
- Each channel's last measured resolution, frame rate and codec is shown in the list; channels
  that failed to load are dimmed, with the reason.
- The playing channel stays in a docked picture with its own controls while you browse.
- The band above the list tells of what is playing, with what is on later on its channel beside
  it. Select a movie or a show and the band shows it and, beside it, what is playing; `↩ Show`
  brings what is playing to the front until the next selection. A large picture or a small window
  folds the second one into a line underneath.
- Command palette (`Ctrl+P`; `⌘P` on a Mac) with every action. Shortcuts are listed in
  Settings › Keyboard, with ⌘ for Ctrl on a Mac.

## Movies

- A tab of its own (`M`) beside Live TV. A rail lists Favourites, Recently added, Watched and the
  movie categories; a list or posters (remembered per computer, a list at first) shows the movies
  in one, sorted by recently added, name, year or rating, and limited to a range of years.
- Mark a movie as a favourite or as watched. Search by name: the search narrows the list to the
  movies whose name has the text at the start of a word (`EN -` finds "EN - A Movie" but not
  "Garden - A Movie"; case and accents don't count, `a|b` finds either), across every category,
  in the chosen sort and years. Shows search the same way.
- A movie's details (plot, cast, rating, pictures, movies like it) open in a sheet.
- Browsing never starts or stops what is playing: only Play does.

## Shows

- A tab of its own (`N`), laid out as Movies is: a rail with Favourites, Recently added, Watched and
  the show categories, and a list or posters with each show's years, seasons and rating, sorted by
  recently added, name, year or rating, limited to a range of years, and searchable by name.
- A show is a favourite as a whole. Watched goes by episode: marking a show watched marks all its
  episodes (specials aside), and a show counts as watched once all its episodes are.
- Selecting a show puts its plot, people and genres in the band above, with Play for the next
  episode: one you are partway through, else the one after the last you watched.
- A click on a show opens its episodes in a pane beside the list: Play for the next episode, the
  seasons as tabs, and each episode with its still, title, plot, length and watched mark, the next
  one and the one playing tagged. `↑`/`↓` in the list and the pane follows; `Enter` or `→` go into
  the episodes (in Posters, `←`/`→` move along the row and `Enter` goes in), `Enter` plays,
  `[`/`]` change season, `←` or `Esc` go back to the list, and `Esc` again (or the mouse's back
  button) closes the pane. Stacked under the picture, the pane opens in the list's place.
- Details (or a double-click) opens the show's sheet: the backdrop and poster, the plot and cast,
  its seasons in a line, and the same actions.
- As with movies, browsing a show plays nothing: Play, or `Enter` on an episode, does.
- An Xtream show's seasons and episodes are asked of the provider when it is first selected, then
  kept for a week.

## Continue watching

- ▶ Continue watching is the first item of both the Movies and the Shows rail, with how many
  titles it holds; `W` (or the command palette) goes there from anywhere. It lists the movies and
  episodes you stopped partway, and the next episode of each show you're following, last watched
  first, as one mixed list on either tab.
- Each row shows the poster (the show's, for an episode), the title with the episode's code
  (`S2 E4`, or `S1 E3 · next` for one not started), the year, length and rating, a progress bar
  with the time left, and when you last watched it. Posters show the same entries as cards.
- Selecting an entry plays nothing; `Enter` or a double-click plays it. Right-click (or the menu
  key, or `Shift+F10`) opens its menu: Resume from where you stopped and Start over (or Play for
  a next episode), Details, Mark watched, and Remove from Continue watching, which takes it off
  the list without forgetting the place.
- Typing in the search box searches the whole tab, as from any other list; clearing it comes back
  to Continue watching.

## Playback

- libmpv with hardware decoding (VA-API on Linux) and HDR tone-mapping.
- A live rewind buffer, sized in megabytes; Settings shows roughly how many minutes it holds. Skip
  5 seconds back or forward, `L` to jump to live.
- Catch-up on channels whose source has an archive.
- **Movies and episodes** play on demand, on a connection of their own under the same rules as a
  channel's. The picture gets a seek bar with the time played and left, `←`/`→` skip 10 seconds,
  Space or a click pauses. How far you got is kept, and a title played past 92 % counts as
  watched. A title that stops or can't start says why and can be played again from where it
  stopped. Episodes play from their beginning, with the show as the title and the episode beneath
  (`S2 E4 · Night Shift · 52m`).
- `P` switches to the previous channel; typing a channel number tunes it.
- Volume up to 200 %, on a perceptual scale, remembered between launches.
- A compressor that evens out loud and quiet audio.
- Right-click menu on the picture: audio track, subtitles, aspect ratio, deinterlacing, sleep
  timer.
- A subtitles button (`C`) beside the other controls, shown when a stream has subtitles, turns
  them on or off. Every channel, movie or episode you open afterwards keeps that choice and the
  language you last picked, across launches too.
- Subtitle font, size, colour and backdrop.
- Mouse: wheel for volume, double-click for fullscreen, double-click the edges to skip. Theater
  mode (`T`). The back button steps back like `Esc` (a menu, a sheet, the Shows pane, the
  player), then to the tab or category before; forward goes ahead again.
- Stats (`S`): bitrate, buffer, resolution, codec, dropped frames and stalls.
- When a provider ends or refuses a stream (for example because another device took the
  connection), zapper says so and doesn't reconnect on its own.
- Optional media keys and desktop media controls (MPRIS on Linux), off by default.

### Up next

- 20 seconds before an episode ends, a card names the next one and counts 10 seconds down to
  playing it, in the full player or the docked picture.
- **Play now** starts it at once, even from pause; **Cancel** lets the episode play to its end and
  stop there. In the full player `Enter` and `Esc` do the same.
- Pausing holds the count. A skip into the last seconds starts the next one at once, and a skip
  through the credits to the end counts as reaching it.
- The next episode keeps the sound and subtitle languages you had, and plays on the same
  connection. One you had already started plays on from where you left it, without asking.
- A Stop, a sleep timer, or a stream the provider cut short never moves on.

## Other

- Restores the last channel, window size and volume on launch.
- Interface zoom (`Ctrl` `+`/`−`/`0`, `⌘` on a Mac); 12- or 24-hour clock.
- Updates itself from GitHub releases, verified against the release's checksums (see the README's
  Updates section).
- Credentials are stored in your config directory, readable only by you, and kept out of logs.
- A log in the data directory (`zapper.log`, and `zapper-server.log` for the recording service),
  kept to a few megabytes, for when a sync or a recording went wrong.
- No telemetry. Apart from your sources, the only request zapper makes is the daily update check,
  which can be turned off.
