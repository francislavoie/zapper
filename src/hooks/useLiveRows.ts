// The rows of whatever the Live TV rail has selected, as one source.
//
// All channels and the provider's categories are paged from the backend;
// Favourites and Recently watched are lists the store already holds, in the
// order the backend ranked them; Recordings has no channels at all. The list
// and the media keys read them all the same way, so what is selected decides
// where rows come from and nothing downstream needs to ask.

import { useMemo } from "react";
import { useChannelPages, type RowSource } from "./useChannelPages";
import { listedFor, pagedCategory, railItemKind, selectionEstimate, type RailItemKind } from "../logic/railItems";
import { loadWatchLists } from "../actions";
import { useApp, useDispatch } from "../store";
import type { ChannelDto } from "../types";

export interface LiveRows extends RowSource {
  kind: RailItemKind;
  /** The stream ids of a favourites or recent selection, else null. */
  listed: readonly number[] | null;
}

const NOOP = () => {};

export function useLiveRows(selectionId: string): LiveRows {
  const dispatch = useDispatch();
  const { categories, sync, favourites, recent, catalogueRev } = useApp();
  const kind = railItemKind(selectionId);
  const paged = pagedCategory(selectionId);
  const estimate =
    paged === undefined
      ? 0
      : selectionEstimate(
          selectionId,
          categories.status === "ready" ? categories.data : [],
          sync.channelsTotal,
        );
  // Called whatever is selected, as hooks must be: with an estimate of zero
  // it fetches nothing.
  const pages = useChannelPages(paged ?? null, estimate, catalogueRev);

  const list = kind === "favourites" ? favourites : kind === "recent" ? recent : null;
  const entries = list && list.status === "ready" ? list.data : null;
  const listed = useMemo(
    () => listedFor(selectionId, kind === "favourites" ? entries : null, kind === "recent" ? entries : null),
    [selectionId, kind, entries],
  );
  const channels = useMemo<ChannelDto[]>(() => (entries ? entries.map((f) => f.channel) : []), [entries]);

  if (paged !== undefined) return { ...pages, kind, listed: null };
  return {
    total: channels.length,
    rowAt: (i) => channels[i],
    ensure: NOOP,
    error: list?.status === "error" ? list.message : null,
    loading: list ? list.status === "loading" || list.status === "idle" : false,
    retry: () => void loadWatchLists(dispatch),
    kind,
    listed,
  };
}
