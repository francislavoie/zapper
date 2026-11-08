// What the Shows pane says about a show and its episodes: codes, seasons,
// facts, the main action. Pure; the logic lives in src/logic/shows.ts.
// Run: node --test src/__checks__/

import assert from "node:assert/strict";
import { test } from "node:test";
import { OVERSCAN } from "../logic/virtual.ts";
import {
  EPISODE_ROW_HEIGHT,
  SHOW_FAVOURITES,
  SHOW_RECENT,
  SHOW_SEARCH_PLACEHOLDER,
  SHOW_WATCHED,
  SHOWS_VIEW_STORAGE_KEY,
  episodeCode,
  episodeLine,
  episodeProgress,
  episodeTitle,
  episodeWindow,
  firstSeason,
  fixedShowItems,
  heldEpisode,
  seasonLabel,
  seasonMeta,
  seasonsText,
  showAction,
  showCard,
  showCount,
  detailsShown,
  showDetailsCache,
  showFacts,
  showQuery,
  showRailKind,
  showTitle,
  showsAvailable,
  showsOriginChanged,
  yearsText,
} from "../logic/shows.ts";

const ep = (episodeId, season, episode, o = {}) => ({
  episodeId, seriesId: 1, season, episode, episodeEnd: null, title: null, plot: null, durationSecs: null,
  stillUrl: null, airDate: null, watched: false, position: null, duration: null, ...o,
});
const show = (o = {}) => ({
  seriesId: 1, name: "Harbour Lights", cleanName: "Harbour Lights", year: 2021, rating: 8.1, posterUrl: null,
  added: null, categoryId: "s1", seasonCount: 3, episodeCount: 20, favourite: false, watched: false, ...o,
});
const details = (o = {}) => ({
  series: show(), plot: null, genres: [], director: null, cast: [], releaseDate: null, backdropUrls: [], tmdbId: null,
  firstYear: null, lastYear: null, seasons: [], episodes: [], nextUp: null, fetchedAt: 1, ...o,
});

test("codes, seasons and titles read as the mockup does", () => {
  assert.equal(episodeCode(2, 4), "S2 E4");
  assert.equal(episodeCode(0, 3), "Special 3");
  assert.equal(episodeCode(1, 1, 2), "S1 E1–E2");
  assert.equal(episodeCode(0, 1, 2), "Special 1–2");
  assert.equal(seasonLabel(0), "Specials");
  assert.equal(seasonLabel(2), "Season 2");
  assert.equal(episodeTitle({ episode: 4, title: null }), "Episode 4");
  assert.equal(episodeTitle({ episode: 4, title: "Night Shift" }), "Night Shift");
  assert.equal(episodeLine({ season: 2, episode: 4, title: "Night Shift", durationSecs: 52 * 60 }), "S2 E4 · Night Shift · 52m");
  assert.equal(episodeLine({ season: 2, episode: 4, title: null, durationSecs: null }), "S2 E4");
  assert.equal(seasonMeta({ number: 2, name: "Season 2", episodeCount: 8, watchedCount: 3 }), "8 episodes · 3 watched");
  assert.equal(seasonMeta({ number: 1, name: "Season 1", episodeCount: 1, watchedCount: 0 }), "1 episode");
});
test("a range's own code is not a range of one", () => {
  assert.equal(episodeCode(1, 3, 3), "S1 E3");
  assert.equal(episodeCode(1, 3, null), "S1 E3");
});
test("a show's facts", () => {
  assert.equal(showFacts({ year: 2021, seasonCount: 3, rating: 8.1 }, { firstYear: 2021, lastYear: 2024 }), "2021–2024 · 3 seasons · ★ 8.1");
  assert.equal(showFacts({ year: null, seasonCount: null, rating: null }), "");
  assert.equal(showFacts({ year: 2019, seasonCount: 1, rating: null }), "2019 · 1 season");
  assert.equal(showCount(3812), "3,812 shows");
  assert.equal(showCount(1), "1 show");
});
test("seasons and years in words", () => {
  assert.equal(seasonsText(1), "1 season");
  assert.equal(seasonsText(3), "3 seasons");
  assert.equal(seasonsText(null), "");
  assert.equal(seasonsText(0), "");
  assert.equal(yearsText(2021, null), "2021");
  assert.equal(yearsText(2021, 2021), "2021");
  assert.equal(yearsText(2021, 2024), "2021–2024");
  assert.equal(yearsText(null, null), "");
});
test("the main action plays what is next, and nothing without episodes", () => {
  assert.deepEqual(showAction(details({ nextUp: 204, episodes: [ep(201, 2, 1), ep(204, 2, 4)] })), { label: "Play S2 E4", episodeId: 204 });
  assert.deepEqual(showAction(details({ nextUp: null, episodes: [ep(101, 1, 1)] })), { label: "Play S1 E1", episodeId: 101 });
  assert.equal(showAction(details({ nextUp: null, episodes: [] })), null);
});
test("with nothing next, the main action skips the specials listed first", () => {
  const eps = [ep(1, 0, 1), ep(101, 1, 1, { episodeEnd: 2 }), ep(103, 1, 3)];
  assert.deepEqual(showAction(details({ nextUp: null, episodes: eps })), { label: "Play S1 E1–E2", episodeId: 101 });
});
test("the sheet opens on the next-up's season, else the first", () => {
  const seasons = [
    { number: 1, name: "Season 1", episodeCount: 1, watchedCount: 1 },
    { number: 2, name: "Season 2", episodeCount: 1, watchedCount: 0 },
  ];
  const episodes = [ep(101, 1, 1), ep(201, 2, 1)];
  assert.equal(firstSeason(details({ seasons, episodes, nextUp: 201 })), 2);
  assert.equal(firstSeason(details({ seasons, episodes, nextUp: null })), 1);
  assert.equal(firstSeason(details()), null);
});
test("with nothing next, the sheet opens on a season rather than the specials", () => {
  const specials = { number: 0, name: "Specials", episodeCount: 2, watchedCount: 0 };
  const one = { number: 1, name: "Season 1", episodeCount: 3, watchedCount: 3 };
  const episodes = [ep(101, 1, 1, { watched: true }), ep(1, 0, 1)];
  assert.equal(firstSeason(details({ seasons: [one, specials], episodes, nextUp: null })), 1);
  // Even listed first, the specials are not where the sheet opens.
  assert.equal(firstSeason(details({ seasons: [specials, one], episodes, nextUp: null })), 1);
  // A show of specials alone opens on them.
  assert.equal(firstSeason(details({ seasons: [specials], episodes: [ep(1, 0, 1)], nextUp: null })), 0);
});
test("the episode list is windowed: a long season draws only what is in sight", () => {
  const H = EPISODE_ROW_HEIGHT;
  assert.equal(H, 56);
  const top = episodeWindow(0, H * 10, 400);
  assert.equal(top.start, 0);
  assert.ok(top.end <= 10 + 2 * OVERSCAN, `end ${top.end}`);
  assert.equal(top.totalHeight, H * 400);
  assert.ok(episodeWindow(H * 200, H * 10, 400).start >= 200 - OVERSCAN);
  assert.equal(episodeWindow(0, H * 10, 0).end, 0);
  assert.equal(episodeWindow(0, H * 10, 0).totalHeight, 0);
  // Scrolled to the end, the last episode is drawn.
  assert.equal(episodeWindow(H * 400 - H * 10, H * 10, 400).end, 400);
});
test("a selection becomes the query the backend reads", () => {
  assert.deepEqual(showQuery(SHOW_RECENT, "added", null), { list: "recent", categoryId: null, sort: "added", yearFrom: null, yearTo: null, search: null });
  assert.deepEqual(showQuery("s4", "az", { from: 2010, to: 2019 }), { list: "category", categoryId: "s4", sort: "az", yearFrom: 2010, yearTo: 2019, search: null });
  // A search is every show that matches, in the chosen sort and years.
  assert.deepEqual(showQuery("s4", "rating", { from: 2010, to: 2019 }, " EN - "), { list: "all", categoryId: null, sort: "rating", yearFrom: 2010, yearTo: 2019, search: "EN -" });
  assert.equal(showRailKind(SHOW_FAVOURITES), "favourites");
  assert.equal(showRailKind(SHOW_WATCHED), "watched");
});
test("the rail's own items, and the list's title", () => {
  const f = fixedShowItems({ available: true, note: null, total: 12000, favourites: 8, recent: 120, watched: 0, origin: "a" });
  assert.deepEqual(f.map((x) => [x.id, x.name, x.count]), [[SHOW_FAVOURITES, "Favourites", "8"], [SHOW_RECENT, "Recently added", "120"], [SHOW_WATCHED, "Watched", ""]]);
  assert.equal(fixedShowItems(null, 2)[0].id, "*continue*");
  assert.deepEqual(fixedShowItems(null, 2)[0], { id: "*continue*", name: "Continue watching", count: "2" });
  assert.equal(fixedShowItems(null, 0)[0].count, "");
  assert.equal(fixedShowItems(null).some((x) => x.id === "*continue*"), false);
  assert.equal(showTitle(SHOW_RECENT, []), "Recently added");
  assert.equal(showTitle("s2", [{ id: "s2", name: "Mysteries" }]), "Mysteries");
  assert.equal(showTitle("gone", []), "Shows");
});
test("availability and a change of account, as the movies'", () => {
  const ready = (origin, available = true) => ({ status: "ready", data: { available, note: null, total: 1, favourites: 0, recent: 0, watched: 0, origin } });
  assert.equal(showsAvailable(ready("a")), true);
  assert.equal(showsAvailable(ready("a", false)), false);
  assert.equal(showsAvailable({ status: "loading" }), false);
  assert.equal(showsOriginChanged(ready("a"), ready("b")), true);
  assert.equal(showsOriginChanged({ status: "idle" }, ready("b")), false);
});
test("a show as a poster card", () => {
  const c = showCard(show({ name: "Harbour Lights 4K", added: 1000 }), 2000);
  assert.equal(c.id, 1);
  assert.equal(c.title, "Harbour Lights");
  assert.deepEqual(c.badges, ["NEW", "4K"]);
  assert.equal(c.meta, "2021 · 3 seasons · ★ 8.1");
});
test("the episode the player holds: open, or just finished", () => {
  const od = (kind, id) => ({ kind, id, position: 1, duration: 10, bufferedTo: null });
  assert.equal(heldEpisode({ onDemand: od("episode", 7) }), 7);
  assert.equal(heldEpisode({ onDemand: od("movie", 7) }), null);
  assert.equal(heldEpisode({ onDemand: null }), null);
  assert.equal(heldEpisode({ onDemand: null, finished: od("episode", 7) }), 7);
  assert.equal(heldEpisode({ onDemand: null, finished: od("movie", 7) }), null);
});
test("the placeholder leaves the key hint to the box", () => {
  assert.equal(SHOW_SEARCH_PLACEHOLDER, "Search shows  ·  a|b for either");
  assert.equal(SHOWS_VIEW_STORAGE_KEY, "zapper.showsView");
});
test("an episode's progress shows only while it is in progress", () => {
  assert.equal(episodeProgress(ep(1, 1, 1, { position: 1000, durationSecs: 2000 })), 0.5);
  assert.equal(episodeProgress(ep(1, 1, 1, { position: 1000, duration: 4000, durationSecs: 2000 })), 0.25);
  assert.equal(episodeProgress(ep(1, 1, 1, { position: 10, durationSecs: 2000 })), null);
  assert.equal(episodeProgress(ep(1, 1, 1, { position: 1900, durationSecs: 2000 })), null);
  assert.equal(episodeProgress(ep(1, 1, 1, { position: 1000, durationSecs: 2000, watched: true })), null);
  assert.equal(episodeProgress(ep(1, 1, 1, { position: 1000 })), null);
  assert.equal(episodeProgress(ep(1, 1, 1)), null);
});
test("a show's details: an answer from before a clear is not kept", () => {
  const c = showDetailsCache(2);
  const asked = c.generation();
  c.clear(); // another account's shows, while the question was out
  assert.equal(c.remember(1, 0, "old account", asked), false);
  assert.equal(c.peek(1), undefined);
  assert.equal(c.remember(1, 0, "new account", c.generation()), true);
  assert.deepEqual(c.peek(1), { revision: 0, details: "new account" });
});
test("a show's details: an older revision does not replace a newer one", () => {
  const c = showDetailsCache(2);
  const g = c.generation();
  assert.equal(c.remember(1, 2, "newer", g), true);
  assert.equal(c.remember(1, 1, "older", g), false);
  assert.equal(c.peek(1).details, "newer");
  assert.equal(c.remember(1, 2, "again", g), true);
  assert.equal(c.peek(1).details, "again");
});
test("a show's details: the oldest is forgotten past the limit", () => {
  const c = showDetailsCache(2);
  const g = c.generation();
  c.remember(1, 0, "a", g);
  c.remember(2, 0, "b", g);
  c.recall(1); // 1 is now the newest
  c.remember(3, 0, "c", g);
  assert.equal(c.size(), 2);
  assert.equal(c.peek(2), undefined);
  assert.equal(c.peek(1).details, "a");
});
test("a show's details: two views asking at once share one question", async () => {
  const c = showDetailsCache(2);
  let starts = 0;
  let answer;
  const start = () => {
    starts++;
    return new Promise((r) => (answer = r));
  };
  const a = c.join(1, 0, start);
  const b = c.join(1, 0, start);
  assert.equal(starts, 1);
  assert.equal(a, b);
  // Another revision is another question.
  c.join(1, 1, () => Promise.resolve("r1"));
  answer("r0");
  assert.equal(await a, "r0");
  // Settled: the next asks again.
  const again = c.join(1, 0, start);
  assert.equal(starts, 2);
  answer("again");
  assert.equal(await again, "again");
});
test("a show's details: a failed question is asked again", async () => {
  const c = showDetailsCache(2);
  let starts = 0;
  const start = () => {
    starts++;
    return Promise.reject(new Error("down"));
  };
  await assert.rejects(c.join(1, 0, start));
  await assert.rejects(c.join(1, 0, start));
  assert.equal(starts, 2);
});
test("a show's details: a question from before a clear is not shared after it", async () => {
  const c = showDetailsCache(2);
  let oldAnswer;
  const asked = c.generation();
  const old = c.join(1, 0, () => new Promise((r) => (oldAnswer = r)));
  c.clear(); // another account's shows
  let starts = 0;
  const fresh = c.join(1, 0, () => {
    starts++;
    return Promise.resolve("new account");
  });
  assert.equal(starts, 1, "asked anew");
  assert.notEqual(fresh, old);
  oldAnswer("old account");
  assert.equal(await old, "old account");
  // The old answer is not kept, and did not end the new question early.
  assert.equal(c.remember(1, 0, "old account", asked), false);
  assert.equal(await fresh, "new account");
  assert.equal(c.peek(1), undefined);
});
test("a show's details: what a view shows", () => {
  const kept = { revision: 1, details: "r1" };
  const mine = (revision, value, generation = 0) => ({ id: 7, revision, generation, value });
  const ready = (data) => ({ status: "ready", data });
  // Nothing selected.
  assert.deepEqual(detailsShown(null, 1, 0, undefined, null), { status: "idle" });
  // Kept at this revision: that.
  assert.deepEqual(detailsShown(7, 1, 0, kept, null), ready("r1"));
  // A newer revision loading: the older copy stays.
  assert.deepEqual(detailsShown(7, 2, 0, kept, mine(2, { status: "loading" })), ready("r1"));
  // ... and when it fails.
  assert.deepEqual(detailsShown(7, 2, 0, kept, mine(2, { status: "error", message: "down" })), ready("r1"));
  // This view's answer to the newer revision, before the cache has it.
  assert.deepEqual(detailsShown(7, 2, 0, kept, mine(2, ready("r2"))), ready("r2"));
  // Nothing kept: this view's loading or failure.
  assert.deepEqual(detailsShown(7, 2, 0, undefined, mine(2, { status: "loading" })), { status: "loading" });
  assert.deepEqual(
    detailsShown(7, 2, 0, undefined, mine(2, { status: "error", message: "down" })),
    { status: "error", message: "down" },
  );
  // Another show's, revision's or account's answer is not this one's.
  assert.deepEqual(detailsShown(7, 2, 0, undefined, { ...mine(2, ready("x")), id: 8 }), { status: "loading" });
  assert.deepEqual(detailsShown(7, 2, 0, undefined, mine(1, ready("x"))), { status: "loading" });
  assert.deepEqual(detailsShown(7, 2, 1, undefined, mine(2, ready("x"), 0)), { status: "loading" });
});
