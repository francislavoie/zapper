// The mock's OpenSubtitles: the eight online-subtitle commands with made-up
// data, so the finder, its sign-in and its refusals can be driven in a plain
// browser. Movie 1 and the first show's first episode (id 201001) have
// results; every other title is searched by name and finds nothing.
//
// Sign in with `tester` / `test-pass`. Flags in the URL, for the UI checks:
// `?online=refused` is a signed-in account OpenSubtitles no longer accepts,
// `quota` one with no downloads left, `busy` a service asking to wait, `off`
// the service switched off, `nothing` a search that finds nothing.

import type {
  KeptSubtitleDto,
  OnDemandKind,
  OnlineServicesDto,
  PlayerOptionsDto,
  SubtitleDownloadDto,
  SubtitleResultDto,
  SubtitleSearchDto,
  TrackDto,
} from "./types";

const MODE = (typeof location !== "undefined" && new URLSearchParams(location.search).get("online")) || "";
const NOW = () => Math.floor(Date.now() / 1000);
const delay = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

const EPISODE_WITH_RESULTS = 201001;
const hasResults = (kind: OnDemandKind, id: number) =>
  (kind === "movie" && id === 1) || (kind === "episode" && id === EPISODE_WITH_RESULTS);

/** The results, in the order OpenSubtitles ranks them. */
const RESULTS: Record<string, Omit<SubtitleResultDto, "kept">[]> = {
  en: [
    { fileId: 7000001, language: "en", release: "Film.0001.2021.1080p.TEST", hearingImpaired: false, forced: false, trusted: true, downloads: 5200 },
    { fileId: 7000002, language: "en", release: "Film.0001.2021.720p.TEST", hearingImpaired: true, forced: false, trusted: false, downloads: 310 },
    { fileId: 7000003, language: "en", release: "Film.0001.2021.2160p.TEST", hearingImpaired: false, forced: true, trusted: false, downloads: 12 },
  ],
  fr: [
    { fileId: 7000004, language: "fr", release: "Film.0001.2021.1080p.TEST", hearingImpaired: false, forced: false, trusted: true, downloads: 940 },
  ],
};

export interface MockOnlineDeps {
  options: () => PlayerOptionsDto;
  patch: (p: Partial<PlayerOptionsDto>) => PlayerOptionsDto;
}

export function createMockOnline(deps: MockOnlineDeps) {
  let enabled = MODE !== "off";
  let languages: string[] = [];
  let signedInAs: string | null = MODE === "refused" || MODE === "quota" ? "tester" : null;
  let refused = MODE === "refused";
  let downloadsLeft: number | null = MODE === "quota" ? 0 : signedInAs ? 20 : null;
  const kept = new Map<string, KeptSubtitleDto[]>();
  const key = (kind: OnDemandKind, id: number) => `${kind}:${id}`;

  const status = (): OnlineServicesDto => ({
    openSubtitles: {
      enabled,
      languages: [...languages],
      signedInAs,
      signInRefused: refused,
      downloadsLeft,
      downloadsResetAt: downloadsLeft === 0 ? NOW() + 3 * 3600 : null,
    },
  });

  return {
    onlineServices: async (): Promise<OnlineServicesDto> => {
      await delay(30);
      return status();
    },

    setOpenSubtitles: async (patch: {
      enabled?: boolean;
      languages?: string[];
    }): Promise<OnlineServicesDto> => {
      if (patch.enabled !== undefined) enabled = patch.enabled;
      if (patch.languages !== undefined) languages = [...patch.languages];
      return status();
    },

    signInOpenSubtitles: async (username: string, password: string): Promise<OnlineServicesDto> => {
      await delay(120);
      if (username !== "tester" || password !== "test-pass") {
        throw "OpenSubtitles didn't accept that username and password.";
      }
      signedInAs = username;
      refused = false;
      if (downloadsLeft === null) downloadsLeft = 20;
      return status();
    },

    signOutOpenSubtitles: async (): Promise<OnlineServicesDto> => {
      signedInAs = null;
      refused = false;
      downloadsLeft = null;
      return status();
    },

    findSubtitles: async (
      kind: OnDemandKind,
      id: number,
      searched: string[],
    ): Promise<SubtitleSearchDto> => {
      await delay(150);
      if (!enabled) return { kind: "off" };
      if (MODE === "busy") return { kind: "busy", retryAt: NOW() + 90 };
      const have = kept.get(key(kind, id)) ?? [];
      const found =
        MODE === "nothing" || !hasResults(kind, id)
          ? []
          : searched.flatMap((l) => RESULTS[l] ?? []);
      return {
        kind: "results",
        results: found.map((r) => ({ ...r, kept: have.some((k) => k.fileId === r.fileId) })),
        languages: searched,
        matchedBy: hasResults(kind, id) ? "id" : "name",
      };
    },

    downloadSubtitle: async (
      kind: OnDemandKind,
      id: number,
      fileId: number,
    ): Promise<SubtitleDownloadDto> => {
      await delay(200);
      if (!enabled) return { kind: "off" };
      if (!signedInAs || refused) return { kind: "signIn", refused, email: false };
      if (MODE === "busy") return { kind: "busy", retryAt: NOW() + 90 };
      if (downloadsLeft !== null && downloadsLeft <= 0) {
        return { kind: "quota", resetAt: NOW() + 3 * 3600 };
      }
      const all = Object.values(RESULTS).flat();
      const r = all.find((x) => x.fileId === fileId);
      const list = kept.get(key(kind, id)) ?? [];
      const subtitle: KeptSubtitleDto = {
        fileId,
        language: r?.language ?? "en",
        release: r?.release ?? "Film.0001.2021.1080p.TEST",
        hearingImpaired: r?.hearingImpaired ?? false,
        downloadedAt: NOW(),
      };
      if (!list.some((k) => k.fileId === fileId)) {
        kept.set(key(kind, id), [...list, subtitle]);
        if (downloadsLeft !== null) downloadsLeft -= 1;
      }
      return { kind: "downloaded", subtitle, downloadsLeft };
    },

    keptSubtitles: async (kind: OnDemandKind, id: number): Promise<KeptSubtitleDto[]> =>
      [...(kept.get(key(kind, id)) ?? [])],

    addSubtitleFile: async (
      _kind: OnDemandKind,
      _id: number,
      _fileId: number,
      title: string,
      lang: string,
    ): Promise<PlayerOptionsDto> => {
      const subs = deps.options().subtitles;
      const added: TrackDto = {
        id: subs.reduce((m, t) => Math.max(m, t.id), 0) + 1,
        title,
        lang,
        codec: "subrip",
        channels: null,
        default: false,
        forced: false,
        hearingImpaired: false,
        visualImpaired: false,
        selected: true,
      };
      return deps.patch({
        subtitles: [...subs.map((t) => ({ ...t, selected: false })), added],
        subtitlesVisible: true,
      });
    },
  };
}
