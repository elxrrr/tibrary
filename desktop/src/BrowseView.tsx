import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  ArrowDownToLine,
  ArrowUpRight,
  Check,
  ChevronRight,
  CircleAlert,
  Disc3,
  Home,
  LoaderCircle,
  ListMusic,
  Sparkles,
  Music2,
  RefreshCw,
  Radio,
  Search,
  UserRound,
} from "lucide-react";
import { call, external } from "./api";
import { CatalogueArtwork, ReleaseArtwork } from "./ReleaseArtwork";
import "./browse.css";

export type BrowseCategory = "albums" | "artists" | "tracks" | "playlists";
export type BrowseLocation =
  | { kind: "home" }
  | { kind: "new" }
  | { kind: "radio"; id: string }
  | { kind: "playlists"; source?: "created" | "saved" }
  | { kind: "playlist"; id: string }
  | { kind: "search"; query: string; category?: BrowseCategory }
  | { kind: "artist"; id: string; filter?: "ALBUMS" | "EPSANDSINGLES" }
  | { kind: "release"; id: string; trackId?: string };
type Artist = {
  id: string;
  name: string;
  picture?: string | null;
  releases?: number;
  local_tracks?: number;
};
type Track = {
  id: string;
  title: string;
  artist: string;
  artist_id?: string;
  disc_number?: number;
  track_number?: number;
  duration?: number;
  available?: boolean;
  ai?: boolean;
  owned?: boolean;
  queued?: boolean;
  release_id?: string;
  album_id?: string;
  release_title?: string;
  album?: string | Release;
  isrc?: string;
  playlist_position?: number;
};
type Playlist = {
  id: string;
  title: string;
  description?: string;
  creator?: string;
  cover?: string | null;
  track_count?: number;
  duration?: number;
};
type Release = {
  id: string;
  title: string;
  artist: string;
  artist_id?: string;
  artists?: Artist[];
  date?: string;
  release_date?: string;
  type?: string;
  release_type?: string;
  track_count?: number;
  quality?: string;
  available?: boolean;
  ai?: boolean;
  owned_tracks?: number;
  queued_tracks?: number;
  downloadable?: boolean;
  tracks?: Track[];
};
type Section = {
  id: string;
  title: string;
  category?: "new" | "playlists" | "discover";
  artists?: Artist[];
  releases?: Release[];
  tracks?: Track[];
  playlists?: Playlist[];
};
type BrowseResult = {
  sections?: Section[];
  artists?: Artist[];
  releases?: Release[];
  items?: (Release | Artist | Track | Playlist)[];
  playlists?: Playlist[];
  playlist?: Playlist;
  seed?: Track;
  artist?: Artist;
  release?: Release;
  total?: number;
  offset?: number;
  limit?: number;
  has_more?: boolean;
  source?: string;
  cached_at?: number | string;
  stale?: boolean;
  warning?: string;
  connected?: boolean;
  market?: string;
};

const resultCache = new Map<string, { value: BrowseResult; expires: number }>();
const scrollPositions = new Map<string, number>();
const trackSelections = new Map<string, Set<string>>();
const pendingRequests = new Map<string, Promise<BrowseResult>>();
let currentCacheScope = "";
export function clearBrowseCache() {
  resultCache.clear();
  scrollPositions.clear();
  trackSelections.clear();
}
function cachedResult(key: string) {
  const entry = resultCache.get(key);
  if (entry && entry.expires > Date.now()) return entry.value;
  resultCache.delete(key);
  return null;
}
function remember<T>(cache: Map<string, T>, key: string, value: T) {
  cache.delete(key);
  cache.set(key, value);
  while (cache.size > 32) cache.delete(cache.keys().next().value!);
}
function request(location: BrowseLocation, refresh = false, offset = 0) {
  const common = { refresh, offset, limit: 24 };
  if (location.kind === "radio")
    return { method: "browse.radio", args: { ...common, id: location.id } };
  if (location.kind === "playlist")
    return {
      method: "browse.playlist",
      args: { ...common, limit: 50, id: location.id },
    };
  if (location.kind === "playlists")
    return {
      method: "browse.playlists",
      args: { ...common, source: location.source || "created" },
    };
  if (location.kind === "search")
    return {
      method: "browse.search",
      args: {
        ...common,
        query: location.query,
        kind: location.category || "albums",
      },
    };
  if (location.kind === "artist")
    return {
      method: "browse.artist",
      args: { ...common, id: location.id, filter: location.filter || "ALBUMS" },
    };
  if (location.kind === "release")
    return { method: "browse.release", args: { id: location.id, refresh } };
  return { method: "browse.home", args: { refresh } };
}
function fetchResult(
  location: BrowseLocation,
  scope: string,
  refresh = false,
  offset = 0,
) {
  const { method, args } = request(location, refresh, offset);
  const key = JSON.stringify([scope, method, args]);
  let pending = pendingRequests.get(key);
  if (!pending) {
    pending = call<BrowseResult>(method, args).finally(() =>
      pendingRequests.delete(key),
    );
    pendingRequests.set(key, pending);
  }
  return pending;
}
export function browseLink(
  input: string,
): { kind: "artist" | "release" | "track" | "playlist"; id: string } | null {
  try {
    const url = new URL(input.trim());
    if (
      url.protocol !== "https:" ||
      url.username ||
      url.password ||
      url.port ||
      !["tidal.com", "www.tidal.com", "listen.tidal.com"].includes(
        url.hostname.toLowerCase(),
      )
    )
      return null;
    const playlist = url.pathname.match(
      /^\/(?:browse\/)?playlist\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\/?$/i,
    );
    if (playlist) return { kind: "playlist", id: playlist[1].toLowerCase() };
    const match = url.pathname.match(
      /^\/(?:browse\/)?(album|artist|track)\/(\d+)\/?$/,
    );
    if (!match || !/[1-9]/.test(match[2])) return null;
    return {
      kind: match[1].startsWith("album")
        ? "release"
        : match[1].startsWith("artist")
          ? "artist"
          : "track",
      id: match[2],
    };
  } catch {
    return null;
  }
}
export function availableTracks(release: Release): Track[] {
  return release.ai ||
    release.available === false ||
    release.downloadable === false
    ? []
    : (release.tracks || []).filter(
        (track) => track.available !== false && !track.ai,
      );
}
export function playlistTracks(items: Track[]): Track[] {
  return [
    ...new Map(
      items
        .filter(
          (track) =>
            track.available !== false && !track.ai && trackRelease(track),
        )
        .map((track) => [String(track.id), track]),
    ).values(),
  ];
}
export function groupTrackSelection(
  tracks: Track[],
  selection?: Set<string>,
): Record<string, string[]> {
  const grouped: Record<string, string[]> = Object.create(null);
  for (const track of playlistTracks(tracks)) {
    if (selection && !selection.has(String(track.id))) continue;
    (grouped[trackRelease(track)] ||= []).push(String(track.id));
  }
  return grouped;
}
function duration(seconds?: number) {
  if (!seconds || !Number.isFinite(seconds)) return "—";
  const whole = Math.round(seconds);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}
function releaseType(release: Release) {
  const value = (
    release.type ||
    release.release_type ||
    "Release"
  ).toLowerCase();
  return value === "ep" ? "EP" : value.charAt(0).toUpperCase() + value.slice(1);
}
function year(release: Release) {
  return (release.date || release.release_date || "").slice(0, 4);
}
function releaseStatus(release: Release) {
  if (release.ai) return { text: "AI-labelled", className: "ai-generated" };
  if (release.available === false)
    return { text: "Unavailable", className: "unavailable" };
  if (release.track_count && (release.owned_tracks || 0) >= release.track_count)
    return { text: "In library", className: "in-library" };
  if ((release.queued_tracks || 0) > 0)
    return { text: "Queued", className: "potential" };
  if ((release.owned_tracks || 0) > 0)
    return { text: `${release.owned_tracks} owned`, className: "in-library" };
  return null;
}
function trackRelease(track: Track): string {
  return String(
    track.release_id ||
      track.album_id ||
      (typeof track.album === "object" ? track.album.id : "") ||
      "",
  );
}
function trackReleaseTitle(track: Track): string {
  return (
    track.release_title ||
    (typeof track.album === "object" ? track.album.title : track.album) ||
    "View release"
  );
}
export function mergeBrowsePage(
  current: BrowseResult | null,
  next: BrowseResult,
): BrowseResult {
  const unique = <T extends { id: string }>(
    old: T[] = [],
    incoming: T[] = [],
    itemKey: (item: T) => string = (item) => String(item.id),
  ) =>
    Array.from(
      new Map(
        [...old, ...incoming].map((item) => [itemKey(item), item]),
      ).values(),
    );
  return {
    ...next,
    items: unique(current?.items, next.items, (item) =>
      "playlist_position" in item && item.playlist_position
        ? `${item.id}:${item.playlist_position}`
        : String(item.id),
    ),
    releases: unique(current?.releases, next.releases),
    playlists: unique(current?.playlists, next.playlists),
  };
}

export function BrowseView({
  location,
  cacheScope = "account",
  onNavigate,
  onOpenQueue,
  onOpenSettings,
  onQueued,
  libraryReleases,
  libraryName,
  onOpenMissing,
}: {
  location: BrowseLocation;
  /** Account identity and market, independent of frequent job revisions. */
  cacheScope?: string;
  onNavigate: (location: BrowseLocation) => void;
  onOpenQueue: () => void;
  onOpenSettings: () => void;
  onQueued?: () => void | Promise<void>;
  libraryReleases?: Release[] | null;
  libraryName?: string;
  onOpenMissing?: () => void;
}) {
  if (currentCacheScope !== cacheScope) {
    clearBrowseCache();
    currentCacheScope = cacheScope;
  }
  const key = JSON.stringify([cacheScope, location]);
  const [snapshot, setSnapshot] = useState<{
    key: string;
    value: BrowseResult | null;
  }>(() => ({ key, value: cachedResult(key) }));
  const result = snapshot.key === key ? snapshot.value : null;
  const setResult = (value: BrowseResult | null) => setSnapshot({ key, value });
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState("");
  const [query, setQuery] = useState(
    location.kind === "search" ? location.query : "",
  );
  const [selection, setSelection] = useState<Set<string>>(new Set());
  const [queueBusy, setQueueBusy] = useState(false);
  const [queueReleaseCount, setQueueReleaseCount] = useState(0);
  const [queueMessage, setQueueMessage] = useState("");
  const [queueError, setQueueError] = useState("");
  const [resolving, setResolving] = useState(false);
  const [playlistLoading, setPlaylistLoading] = useState(false);
  const [playlistProgress, setPlaylistProgress] = useState("");
  const playlistCancel = useRef(false);
  const scroller = useRef<HTMLDivElement>(null);
  const generation = useRef(0);
  const resolveGeneration = useRef(0);
  const allCheck = useRef<HTMLInputElement>(null);
  const liveKey = useRef(key);
  const restoredKey = useRef("");
  liveKey.current = key;

  async function load(refresh = false, more = false) {
    const token = ++generation.current;
    const requestKey = key;
    if (more) setLoadingMore(true);
    else setLoading(true);
    setError("");
    const offset = more ? (result?.offset || 0) + (result?.limit || 24) : 0;
    try {
      const value = await fetchResult(location, cacheScope, refresh, offset);
      if (token !== generation.current || liveKey.current !== requestKey)
        return;
      const next = more ? mergeBrowsePage(result, value) : value;
      remember(resultCache, requestKey, {
        value: next,
        expires: Date.now() + 60_000,
      });
      setResult(next);
    } catch (failure) {
      if (token === generation.current && liveKey.current === requestKey)
        setError(String(failure instanceof Error ? failure.message : failure));
    } finally {
      if (token === generation.current && liveKey.current === requestKey) {
        setLoading(false);
        setLoadingMore(false);
      }
    }
  }

  useEffect(() => {
    const cached = cachedResult(key);
    setResult(cached || null);
    setQueueMessage("");
    setQueueError("");
    if (location.kind === "search") setQuery(location.query);
    if (cached) {
      setLoading(false);
      setLoadingMore(false);
      setError("");
    } else void load();
    return () => {
      generation.current++;
      resolveGeneration.current++;
      playlistCancel.current = true;
    };
  }, [key]);
  useLayoutEffect(() => {
    restoredKey.current = "";
  }, [key]);
  useLayoutEffect(() => {
    if (result && restoredKey.current !== key && scroller.current) {
      scroller.current.scrollTop = scrollPositions.get(key) || 0;
      restoredKey.current = key;
    }
  }, [key, result]);
  const selectionKey =
    location.kind === "playlist"
      ? `playlist:${location.id}`
      : location.kind === "release"
        ? `release:${location.id}:${location.trackId || ""}`
        : "";
  const selectable = result?.release
    ? availableTracks(result.release)
    : result?.playlist && !result.stale
      ? playlistTracks((result.items || []) as Track[])
      : [];
  useEffect(() => {
    if (!selectionKey || !result) return;
    const available = selectable.map((track) => String(track.id));
    const saved = trackSelections.get(selectionKey);
    const target = location.kind === "release" ? location.trackId : undefined;
    setSelection(
      new Set(
        saved
          ? available.filter((id) => saved.has(id))
          : target
            ? available.filter((id) => id === target)
            : available,
      ),
    );
  }, [result?.release, result?.items, result?.stale, selectionKey]);
  const selectedCount = selectable.filter((track) =>
    selection.has(String(track.id)),
  ).length;
  useEffect(() => {
    if (allCheck.current)
      allCheck.current.indeterminate =
        selectedCount > 0 && selectedCount < selectable.length;
  }, [selectedCount, selectable.length]);

  async function submitSearch() {
    const resolveToken = ++resolveGeneration.current;
    setResolving(false);
    const value = query.trim();
    if (!value) return;
    const link = browseLink(value);
    if (
      link?.kind === "release" ||
      link?.kind === "artist" ||
      link?.kind === "playlist"
    ) {
      onNavigate({ kind: link.kind, id: link.id });
      return;
    }
    if (link?.kind === "track") {
      const requestKey = key;
      setResolving(true);
      setError("");
      try {
        const target = await call<{
          kind: "release" | "artist";
          id: string;
          track_id?: string;
        }>("browse.resolve", { url: value });
        if (
          liveKey.current === requestKey &&
          resolveGeneration.current === resolveToken
        )
          onNavigate(
            target.kind === "release"
              ? {
                  kind: "release",
                  id: String(target.id),
                  trackId: target.track_id
                    ? String(target.track_id)
                    : undefined,
                }
              : target,
          );
      } catch (failure) {
        if (
          liveKey.current === requestKey &&
          resolveGeneration.current === resolveToken
        )
          setError(
            String(failure instanceof Error ? failure.message : failure),
          );
      } finally {
        if (resolveGeneration.current === resolveToken) setResolving(false);
      }
      return;
    }
    if (location.kind === "search" && location.query === value) {
      await load(true);
      return;
    }
    onNavigate({
      kind: "search",
      query: value,
      category:
        location.kind === "search"
          ? location.category || "albums"
          : location.kind === "playlists"
            ? "playlists"
            : "albums",
    });
  }

  async function queueSelection(selectionMap: Record<string, string[] | null>) {
    const entries = Object.entries(selectionMap).filter(
      ([, tracks]) => tracks === null || tracks.length,
    );
    if (queueBusy || !entries.length) return;
    const requestKey = key;
    setQueueBusy(true);
    setQueueReleaseCount(entries.length);
    setQueueError("");
    setQueueMessage("");
    try {
      // Validate every selected release before committing the exact track set.
      const added = await call<{
        queued_releases: number;
        queued_tracks: number;
      }>("browse.queue", {
        selection: Object.fromEntries(entries),
      });
      const queued = added.queued_tracks;
      resultCache.clear();
      if (liveKey.current === requestKey) {
        setQueueMessage(
          `${queued} ${queued === 1 ? "track" : "tracks"} queued.`,
        );
        if (location.kind === "playlist" && result) {
          const next = {
            ...result,
            items: ((result.items || []) as Track[]).map((track) => {
              const picked = selectionMap[trackRelease(track)];
              return picked === null || picked?.includes(String(track.id))
                ? { ...track, queued: true }
                : track;
            }),
          };
          remember(resultCache, requestKey, {
            value: next,
            expires: Date.now() + 60_000,
          });
          setResult(next);
        } else void load();
      }
      await onQueued?.();
    } catch (failure) {
      if (liveKey.current === requestKey)
        setQueueError(
          String(failure instanceof Error ? failure.message : failure),
        );
    } finally {
      setQueueBusy(false);
    }
  }
  function queue(releaseId: string, tracks: string[] | null) {
    return queueSelection({ [releaseId]: tracks });
  }
  async function loadFullPlaylist() {
    if (
      location.kind !== "playlist" ||
      !result?.playlist ||
      playlistLoading ||
      queueBusy ||
      result.stale
    )
      return;
    const requestKey = key;
    playlistCancel.current = false;
    setPlaylistLoading(true);
    setQueueError("");
    let combined = result;
    try {
      while (combined.has_more) {
        setPlaylistProgress(
          `Loading playlist tracks · ${combined.items?.length || 0} of ${combined.total || "…"}`,
        );
        const offset = (combined.offset || 0) + (combined.limit || 50);
        const page = await fetchResult(location, cacheScope, false, offset);
        if (playlistCancel.current || liveKey.current !== requestKey) return;
        if (page.stale)
          throw new Error("Refresh the playlist before queueing all tracks.");
        if ((page.offset || 0) < offset)
          throw new Error(
            "The next playlist page could not be loaded. Please refresh and try again.",
          );
        combined = mergeBrowsePage(combined, page);
        remember(resultCache, requestKey, {
          value: combined,
          expires: Date.now() + 60_000,
        });
        setResult(combined);
      }
      if (playlistCancel.current || liveKey.current !== requestKey) return;
      setQueueMessage("");
    } catch (failure) {
      if (liveKey.current === requestKey)
        setQueueError(
          String(failure instanceof Error ? failure.message : failure),
        );
    } finally {
      setPlaylistLoading(false);
      setPlaylistProgress("");
    }
  }
  function choose(next: Set<string>) {
    setSelection(next);
    if (selectionKey) remember(trackSelections, selectionKey, next);
  }
  function navigateRelease(release: Release) {
    onNavigate({ kind: "release", id: String(release.id) });
  }
  function renderArtist(artist: Artist) {
    return (
      <button
        key={artist.id}
        className="browse-artist-card"
        aria-label={`Browse artist ${artist.name}`}
        onClick={() => onNavigate({ kind: "artist", id: String(artist.id) })}
      >
        <CatalogueArtwork
          imageId={artist.picture}
          kind="artist"
          metadata={artist}
        />
        <span>
          <strong>{artist.name}</strong>
          <small>
            {artist.local_tracks
              ? `${artist.local_tracks.toLocaleString()} local tracks`
              : "Browse releases"}
          </small>
        </span>
        <ChevronRight size={16} />
      </button>
    );
  }
  function renderRelease(release: Release) {
    const status = releaseStatus(release);
    return (
      <article className="browse-release-card" key={release.id}>
        <button
          className="browse-release-open"
          onClick={() => navigateRelease(release)}
          aria-label={`Open release ${release.title} by ${release.artist}`}
        >
          <ReleaseArtwork
            releaseId={String(release.id)}
            market={result?.market}
            metadata={release}
          />
          <strong title={release.title}>{release.title}</strong>
          <span className="browse-release-artist" title={release.artist}>
            {release.artist}
          </span>
        </button>
        <div className="browse-card-foot">
          <small>
            {[year(release), releaseType(release)].filter(Boolean).join(" · ")}
          </small>
          <button
            className="browse-icon-button"
            disabled={queueBusy || release.ai || release.available === false}
            title={
              release.ai
                ? "AI-labelled releases cannot be queued"
                : release.available === false
                  ? "Unavailable for this account"
                  : "Add the full release to the download queue"
            }
            aria-label={`Queue release ${release.title}`}
            onClick={() => void queue(String(release.id), null)}
          >
            <ArrowDownToLine size={16} />
          </button>
        </div>
        {status && (
          <span className={`badge ${status.className}`}>{status.text}</span>
        )}
      </article>
    );
  }
  function renderPlaylist(playlist: Playlist) {
    return (
      <article className="browse-release-card" key={playlist.id}>
        <button
          className="browse-release-open"
          aria-label={`Open playlist ${playlist.title}`}
          onClick={() =>
            onNavigate({ kind: "playlist", id: String(playlist.id) })
          }
        >
          <CatalogueArtwork
            imageId={playlist.cover}
            kind="playlist"
            metadata={playlist}
          />
          <strong title={playlist.title}>{playlist.title}</strong>
          <span className="browse-release-artist">
            {playlist.creator || "Playlist"}
          </span>
        </button>
        <div className="browse-card-foot">
          <small>
            {typeof playlist.track_count === "number"
              ? `${playlist.track_count} tracks`
              : "Explore playlist"}
          </small>
        </div>
      </article>
    );
  }
  function renderTracks(tracks: Track[], release?: Release, playlist = false) {
    const selectableTable = Boolean(release) || playlist;
    return (
      <div className="browse-track-table">
        <table>
          <thead>
            <tr>
              {selectableTable && (
                <th className="browse-check">
                  <input
                    ref={allCheck}
                    type="checkbox"
                    aria-label={
                      playlist
                        ? "Select all loaded available tracks"
                        : "Select all available tracks"
                    }
                    disabled={!selectable.length}
                    checked={
                      selectable.length > 0 &&
                      selectedCount === selectable.length
                    }
                    onChange={(event) =>
                      choose(
                        event.target.checked
                          ? new Set(selectable.map((track) => String(track.id)))
                          : new Set(),
                      )
                    }
                  />
                </th>
              )}
              <th className="browse-position">Track</th>
              <th>Title</th>
              {!release && <th>Release</th>}
              <th className="browse-duration">Time</th>
              <th className="browse-track-status">Status</th>
              <th>
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {tracks.map((track, index) => {
              const available =
                track.available !== false &&
                !track.ai &&
                !release?.ai &&
                release?.available !== false &&
                release?.downloadable !== false &&
                (!playlist || (!result?.stale && Boolean(trackRelease(track))));
              const releaseId = release
                ? String(release.id)
                : trackRelease(track);
              return (
                <tr
                  key={`${track.id}:${track.playlist_position || index}`}
                  className={!available ? "browse-track-unavailable" : ""}
                >
                  {selectableTable && (
                    <td className="browse-check">
                      <input
                        type="checkbox"
                        aria-label={`Select ${track.title}`}
                        checked={selection.has(String(track.id)) && available}
                        disabled={!available}
                        onChange={(event) => {
                          const next = new Set(selection);
                          if (event.target.checked) next.add(String(track.id));
                          else next.delete(String(track.id));
                          choose(next);
                        }}
                      />
                    </td>
                  )}
                  <td className="browse-position">
                    {!playlist && track.disc_number && track.disc_number > 1
                      ? `${track.disc_number} · `
                      : ""}
                    {(playlist ? track.playlist_position : track.track_number)
                      ? String(
                          playlist
                            ? track.playlist_position
                            : track.track_number,
                        ).padStart(2, "0")
                      : "—"}
                  </td>
                  <td>
                    <strong>{track.title}</strong>
                    <small>{track.artist}</small>
                  </td>
                  {!release && (
                    <td>
                      <button
                        className="browse-text-button"
                        disabled={!releaseId}
                        onClick={() =>
                          onNavigate({
                            kind: "release",
                            id: releaseId,
                            trackId: String(track.id),
                          })
                        }
                      >
                        {trackReleaseTitle(track)}
                      </button>
                    </td>
                  )}
                  <td className="browse-duration">
                    {duration(track.duration)}
                  </td>
                  <td className="browse-track-status">
                    {track.ai || release?.ai ? (
                      <span className="badge ai-generated">AI-labelled</span>
                    ) : track.available === false ||
                      release?.available === false ? (
                      <span className="badge unavailable">Unavailable</span>
                    ) : release?.downloadable === false ||
                      (playlist && result?.stale) ? (
                      <span className="badge needs-review">Refresh needed</span>
                    ) : track.owned ? (
                      <span className="badge in-library">In library</span>
                    ) : track.queued ? (
                      <span className="badge potential">Queued</span>
                    ) : (
                      <span className="browse-muted">—</span>
                    )}
                  </td>
                  <td>
                    <div className="browse-row-actions">
                      {!release && (
                        <button
                          className="browse-icon-button"
                          disabled={
                            !available ||
                            !releaseId ||
                            queueBusy ||
                            playlistLoading ||
                            (playlist && loadingMore)
                          }
                          title="Add this track to the download queue"
                          aria-label={`Queue track ${track.title}`}
                          onClick={() =>
                            void queue(releaseId, [String(track.id)])
                          }
                        >
                          <ArrowDownToLine size={16} />
                        </button>
                      )}
                      <button
                        className="browse-icon-button"
                        title="Find recommended tracks like this"
                        aria-label={`More like ${track.title}`}
                        onClick={() =>
                          onNavigate({ kind: "radio", id: String(track.id) })
                        }
                      >
                        <Radio size={16} />
                      </button>
                      <button
                        className="browse-icon-button"
                        title="Open on web"
                        aria-label={`Open ${track.title} on web`}
                        onClick={() =>
                          void external(
                            `https://tidal.com/browse/track/${track.id}`,
                          ).catch((failure) => setQueueError(String(failure)))
                        }
                      >
                        <ArrowUpRight size={16} />
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    );
  }

  const release = result?.release;
  const artist = result?.artist;
  const playlist = result?.playlist;
  const playlistSelection = playlist
    ? groupTrackSelection((result?.items || []) as Track[], selection)
    : {};
  const oversizedPlaylistSelection =
    Boolean(playlist) &&
    (selectedCount > 2000 || Object.keys(playlistSelection).length > 500);
  const categories: [BrowseCategory, string][] = [
    ["albums", "Releases"],
    ["artists", "Artists"],
    ["tracks", "Tracks"],
    ["playlists", "Playlists"],
  ];
  const sectionList: Section[] = result?.sections || [
    { id: "artists", title: "Your artists", artists: result?.artists || [] },
    {
      id: "releases",
      title: "Recently explored",
      releases: result?.releases || [],
    },
  ];
  const emptyHome = !sectionList.some(
    (section) =>
      section.artists?.length ||
      section.releases?.length ||
      section.tracks?.length ||
      section.playlists?.length,
  );
  return (
    <section className="browse-view" aria-label="Browse music">
      <form
        className="browse-toolbar"
        onSubmit={(event) => {
          event.preventDefault();
          void submitSearch();
        }}
      >
        <button
          type="button"
          className="browse-icon-button"
          aria-label="Browse home"
          title="Browse home"
          onClick={() => onNavigate({ kind: "home" })}
        >
          <Home size={18} />
        </button>
        <label className="browse-search">
          <Search size={18} aria-hidden="true" />
          <input
            type="search"
            aria-label="Search online music"
            placeholder="Search music and playlists, or paste a link…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <button type="submit" disabled={!query.trim() || resolving}>
          {resolving ? (
            <LoaderCircle size={16} className="spin" />
          ) : (
            <Search size={16} />
          )}
          Search
        </button>
        <button
          type="button"
          className="browse-icon-button"
          aria-label="Refresh browse results"
          title="Refresh this page from the online catalogue"
          disabled={
            loading || loadingMore || resolving || playlistLoading || queueBusy
          }
          onClick={() => void load(true)}
        >
          <RefreshCw size={17} className={loading && result ? "spin" : ""} />
        </button>
        <button type="button" onClick={onOpenQueue}>
          <ArrowDownToLine size={16} />
          <span className="browse-queue-label">Download queue</span>
        </button>
      </form>
      <nav className="browse-navigation" aria-label="Browse sections">
        <div
          className="browse-tabs"
          role="tablist"
          aria-label="Browse sections"
        >
          {(
            [
              ["home", "Discover", Sparkles],
              ["new", "New releases", Disc3],
              ["playlists", "Playlists", ListMusic],
            ] as const
          ).map(([kind, label, Icon]) => (
            <button
              key={kind}
              role="tab"
              aria-selected={location.kind === kind}
              onClick={() => onNavigate({ kind })}
            >
              <Icon size={16} />
              {label}
            </button>
          ))}
        </div>
      </nav>
      <div
        className="browse-scroll"
        ref={scroller}
        onScroll={(event) => {
          // Rendering the next page may clamp the old scroll offset to zero.
          // Remember user scrolling while this page is active, before navigation.
          if (result && restoredKey.current === key)
            remember(scrollPositions, key, event.currentTarget.scrollTop);
        }}
      >
        {playlistLoading && !queueBusy && (
          <div className="browse-notice" role="status">
            <LoaderCircle size={18} className="spin" />
            <span>{playlistProgress || "Preparing playlist tracks…"}</span>
            <button
              onClick={() => {
                playlistCancel.current = true;
              }}
            >
              Cancel
            </button>
          </div>
        )}
        {queueBusy && (
          <div className="browse-notice" role="status">
            <LoaderCircle size={18} className="spin" />
            <span>
              Checking {queueReleaseCount}{" "}
              {queueReleaseCount === 1 ? "release" : "releases"} for the
              download queue…
            </span>
          </div>
        )}
        {queueMessage && (
          <div className="browse-notice browse-success" role="status">
            <Check size={18} />
            <span>{queueMessage}</span>
            <button onClick={onOpenQueue}>Open download queue</button>
          </div>
        )}
        {queueError && (
          <div className="browse-notice browse-error" role="alert">
            <CircleAlert size={18} />
            <span>{queueError}</span>
            <button onClick={() => setQueueError("")}>Dismiss</button>
          </div>
        )}
        {error && (
          <div className="browse-notice browse-error" role="alert">
            <CircleAlert size={18} />
            <span>{error}</span>
            <button disabled={loading} onClick={() => void load(true)}>
              Retry
            </button>
            <button onClick={onOpenSettings}>Connection settings</button>
          </div>
        )}
        {result?.warning && (
          <div className="browse-notice" role="status">
            <CircleAlert size={17} />
            <span>{result.warning}</span>
          </div>
        )}
        {result?.connected === false && (
          <div className="browse-notice">
            <UserRound size={18} />
            <span>Connect your account to browse online music.</span>
            <button onClick={onOpenSettings}>Connect account</button>
          </div>
        )}
        {location.kind === "search" && (
          <header className="browse-page-heading">
            <div>
              <span className="browse-eyebrow">Search results</span>
              <h1>{location.query}</h1>
            </div>
            <div
              className="browse-tabs"
              role="tablist"
              aria-label="Search category"
            >
              {categories.map(([category, label]) => (
                <button
                  key={category}
                  role="tab"
                  aria-selected={(location.category || "albums") === category}
                  onClick={() => onNavigate({ ...location, category })}
                >
                  {label}
                </button>
              ))}
            </div>
          </header>
        )}
        {loading && !result ? (
          <div className="browse-loading" role="status">
            <LoaderCircle size={24} className="spin" />
            <span>
              Loading{" "}
              {location.kind === "playlist"
                ? "playlist tracks"
                : location.kind === "playlists"
                  ? "playlists"
                  : location.kind === "release"
                    ? "release and tracks"
                    : location.kind === "artist"
                      ? "artist releases"
                      : location.kind === "search"
                        ? "search results"
                        : "your music"}
              …
            </span>
          </div>
        ) : (
          <>
            {location.kind === "home" && result && (
              <>
                <header className="browse-page-heading">
                  <div>
                    <span className="browse-eyebrow">Online catalogue</span>
                    <h1>Discover music</h1>
                    <p>Explore artists, playlists and recommendations.</p>
                  </div>
                </header>
                {sectionList.map((section) =>
                  section.artists?.length ||
                  section.releases?.length ||
                  section.tracks?.length ||
                  section.playlists?.length ? (
                    <section className="browse-section" key={section.id}>
                      <h2>{section.title}</h2>
                      {section.artists?.length ? (
                        <div className="browse-artists-grid">
                          {section.artists.map(renderArtist)}
                        </div>
                      ) : null}
                      {section.releases?.length ? (
                        <div className="browse-release-grid">
                          {section.releases.map(renderRelease)}
                        </div>
                      ) : null}
                      {section.tracks?.length
                        ? renderTracks(section.tracks)
                        : null}
                      {section.playlists?.length ? (
                        <div className="browse-release-grid">
                          {section.playlists.map(renderPlaylist)}
                        </div>
                      ) : null}
                    </section>
                  ) : null,
                )}
                {emptyHome && (
                  <div className="browse-empty">
                    <Disc3 size={42} strokeWidth={1.2} />
                    <h2>Your catalogue, ready to explore</h2>
                    <p>
                      Search for an artist, release or track above. You can
                      browse and queue music without adding a local library.
                    </p>
                  </div>
                )}
              </>
            )}
            {location.kind === "new" && (
              <>
                <header className="browse-page-heading">
                  <div>
                    <span className="browse-eyebrow">New releases</span>
                    <h1>Fresh music to explore</h1>
                  </div>
                </header>
                {libraryName && (
                  <section className="browse-section">
                    <div className="browse-section-heading">
                      <div>
                        <h2>From your library</h2>
                        <p>Latest missing releases for {libraryName}.</p>
                      </div>
                      {onOpenMissing && (
                        <button onClick={onOpenMissing}>
                          View all missing releases <ChevronRight size={16} />
                        </button>
                      )}
                    </div>
                    {libraryReleases == null ? (
                      <p role="status">Loading saved releases…</p>
                    ) : libraryReleases.length ? (
                      <div className="browse-release-grid">
                        {libraryReleases.map(renderRelease)}
                      </div>
                    ) : (
                      <p>
                        No recommended or potential releases in the saved
                        catalogue.
                      </p>
                    )}
                  </section>
                )}
                {sectionList
                  .filter(
                    (section) =>
                      (section.category === "new" ||
                        (!section.category &&
                          /^new(?:[\s:]|$)/i.test(section.title))) &&
                      (section.releases?.length || section.tracks?.length),
                  )
                  .map((section) => (
                    <section className="browse-section" key={section.id}>
                      <div className="browse-section-heading">
                        <div>
                          <h2>{section.title}</h2>
                          <p>From the online catalogue.</p>
                        </div>
                      </div>
                      {section.releases?.length ? (
                        <div className="browse-release-grid">
                          {section.releases.map(renderRelease)}
                        </div>
                      ) : null}
                      {section.tracks?.length
                        ? renderTracks(section.tracks)
                        : null}
                    </section>
                  ))}
                {!sectionList.some(
                  (section) =>
                    (section.category === "new" ||
                      (!section.category &&
                        /^new(?:[\s:]|$)/i.test(section.title))) &&
                    (section.releases?.length || section.tracks?.length),
                ) && (
                  <div className="browse-empty">
                    <Disc3 size={36} />
                    <h2>No new catalogue picks available</h2>
                    <p>
                      Discover music or search for an artist to explore their
                      releases.
                    </p>
                  </div>
                )}
              </>
            )}
            {location.kind === "playlists" && result && (
              <>
                <header className="browse-page-heading">
                  <div>
                    <span className="browse-eyebrow">Playlists</span>
                    <h1>Music for every mood</h1>
                  </div>
                </header>
                <div className="browse-section-heading">
                  <div
                    className="browse-tabs"
                    role="tablist"
                    aria-label="Playlist source"
                  >
                    {(
                      [
                        ["created", "Your playlists"],
                        ["saved", "Saved playlists"],
                      ] as const
                    ).map(([source, label]) => (
                      <button
                        key={source}
                        role="tab"
                        aria-selected={
                          (location.source || "created") === source
                        }
                        onClick={() => onNavigate({ ...location, source })}
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                  {typeof result.total === "number" && (
                    <small>{result.total.toLocaleString()} playlists</small>
                  )}
                </div>
                {(result.playlists || result.items)?.length ? (
                  <div className="browse-release-grid">
                    {((result.playlists || result.items) as Playlist[]).map(
                      renderPlaylist,
                    )}
                  </div>
                ) : (
                  <div className="browse-empty">
                    <ListMusic size={40} />
                    <h2>No playlists here yet</h2>
                    <p>Search by name or paste a playlist link to open one.</p>
                    <button
                      onClick={() => {
                        if (query.trim())
                          onNavigate({
                            kind: "search",
                            query: query.trim(),
                            category: "playlists",
                          });
                        else
                          scroller.current?.parentElement
                            ?.querySelector<HTMLInputElement>(
                              'input[type="search"]',
                            )
                            ?.focus();
                      }}
                    >
                      Search playlists
                    </button>
                  </div>
                )}
                {result.sections?.map((section) =>
                  section.playlists?.length ? (
                    <section className="browse-section" key={section.id}>
                      <h2>{section.title}</h2>
                      <div className="browse-release-grid">
                        {section.playlists.map(renderPlaylist)}
                      </div>
                    </section>
                  ) : null,
                )}
              </>
            )}
            {location.kind === "playlist" && playlist && (
              <>
                <header className="browse-release-heading">
                  <div className="browse-release-cover">
                    <CatalogueArtwork
                      imageId={playlist.cover}
                      kind="playlist"
                      metadata={playlist}
                    />
                  </div>
                  <div className="browse-release-summary">
                    <span className="browse-eyebrow">Playlist</span>
                    <h1>{playlist.title}</h1>
                    <p>
                      {[
                        playlist.creator,
                        `${playlist.track_count ?? result?.total ?? 0} tracks`,
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </p>
                    {playlist.description && (
                      <p className="browse-playlist-description">
                        {playlist.description}
                      </p>
                    )}
                    <div className="browse-release-actions">
                      <button
                        className="primary"
                        disabled={
                          !selectedCount ||
                          oversizedPlaylistSelection ||
                          queueBusy ||
                          playlistLoading ||
                          loadingMore ||
                          result?.stale
                        }
                        onClick={() => void queueSelection(playlistSelection)}
                      >
                        {queueBusy ? (
                          <LoaderCircle size={16} className="spin" />
                        ) : (
                          <ArrowDownToLine size={16} />
                        )}
                        Queue selected tracks ({selectedCount})
                      </button>
                      {result?.has_more && (
                        <button
                          disabled={
                            loading ||
                            loadingMore ||
                            playlistLoading ||
                            queueBusy ||
                            result.stale
                          }
                          onClick={() => void loadFullPlaylist()}
                        >
                          Load all tracks
                        </button>
                      )}
                      <button
                        aria-label="Open playlist on web"
                        title="Open on web"
                        onClick={() =>
                          void external(
                            `https://tidal.com/browse/playlist/${playlist.id}`,
                          ).catch((failure) => setQueueError(String(failure)))
                        }
                      >
                        <ArrowUpRight size={16} />
                      </button>
                    </div>
                    <small>
                      {result?.has_more
                        ? `${result.items?.length || 0} of ${result.total ?? playlist.track_count ?? "…"} tracks loaded · select from the loaded tracks`
                        : `${result?.items?.length || 0} audio tracks loaded`}
                    </small>
                  </div>
                </header>
                {result?.stale && (
                  <div className="browse-notice">
                    <CircleAlert size={18} />
                    <span>
                      Refresh this playlist before selecting tracks to queue.
                    </span>
                  </div>
                )}
                {oversizedPlaylistSelection && (
                  <div className="browse-notice">
                    <CircleAlert size={18} />
                    <span>
                      Select up to 2,000 tracks from 500 releases at a time.
                    </span>
                  </div>
                )}
                {result?.items?.length ? (
                  renderTracks(result.items as Track[], undefined, true)
                ) : (
                  <div className="browse-empty">
                    <Music2 size={36} />
                    <h2>No audio tracks available</h2>
                    <p>Videos and unavailable items cannot be queued.</p>
                  </div>
                )}
              </>
            )}
            {location.kind === "radio" && result && (
              <>
                <header className="browse-page-heading">
                  <div>
                    <span className="browse-eyebrow">Recommended tracks</span>
                    <h1>Tracks like {result.seed?.title || "this"}</h1>
                    {result.seed?.artist && (
                      <p>
                        {result.seed.artist} · Related tracks from the online
                        catalogue.
                      </p>
                    )}
                  </div>
                </header>
                {result.items?.length ? (
                  renderTracks(result.items as Track[])
                ) : (
                  <div className="browse-empty">
                    <Radio size={36} />
                    <h2>No related tracks available</h2>
                    <p>Try a different track.</p>
                  </div>
                )}
              </>
            )}
            {location.kind === "search" && result && (
              <>
                {!result.items?.length ? (
                  <div className="browse-empty">
                    <Search size={36} />
                    <h2>
                      No{" "}
                      {categories
                        .find(
                          ([category]) =>
                            category === (location.category || "albums"),
                        )?.[1]
                        .toLowerCase()}{" "}
                      found
                    </h2>
                    <p>
                      Try another spelling or choose a different search
                      category.
                    </p>
                  </div>
                ) : (location.category || "albums") === "albums" ? (
                  <div className="browse-release-grid">
                    {(result.items as Release[]).map(renderRelease)}
                  </div>
                ) : location.category === "artists" ? (
                  <div className="browse-artists-grid">
                    {(result.items as Artist[]).map(renderArtist)}
                  </div>
                ) : location.category === "playlists" ? (
                  <div className="browse-release-grid">
                    {(result.items as Playlist[]).map(renderPlaylist)}
                  </div>
                ) : (
                  renderTracks(result.items as Track[])
                )}
              </>
            )}
            {location.kind === "artist" && artist && (
              <>
                <header className="browse-artist-heading">
                  <CatalogueArtwork
                    imageId={artist.picture}
                    kind="artist"
                    metadata={artist}
                  />
                  <div>
                    <span className="browse-eyebrow">Artist</span>
                    <h1>{artist.name}</h1>
                    <button
                      className="browse-text-button"
                      onClick={() =>
                        void external(
                          `https://tidal.com/browse/artist/${artist.id}`,
                        ).catch((failure) => setQueueError(String(failure)))
                      }
                    >
                      Open on web <ArrowUpRight size={14} />
                    </button>
                  </div>
                </header>
                <div className="browse-section-heading">
                  <div
                    className="browse-tabs"
                    role="tablist"
                    aria-label="Artist releases"
                  >
                    {(
                      [
                        ["ALBUMS", "Albums"],
                        ["EPSANDSINGLES", "EPs & singles"],
                      ] as const
                    ).map(([filter, title]) => (
                      <button
                        key={filter}
                        role="tab"
                        aria-selected={(location.filter || "ALBUMS") === filter}
                        onClick={() => onNavigate({ ...location, filter })}
                      >
                        {title}
                      </button>
                    ))}
                  </div>
                  {typeof result?.total === "number" && (
                    <small>{result.total.toLocaleString()} releases</small>
                  )}
                </div>
                {result?.releases?.length ? (
                  <div className="browse-release-grid">
                    {result.releases.map(renderRelease)}
                  </div>
                ) : (
                  <div className="browse-empty">
                    <Disc3 size={36} />
                    <h2>No releases in this section</h2>
                    <p>Try the other release category.</p>
                  </div>
                )}
              </>
            )}
            {location.kind === "release" && release && (
              <>
                <header className="browse-release-heading">
                  <div className="browse-release-cover">
                    <ReleaseArtwork
                      releaseId={String(release.id)}
                      market={result?.market}
                      metadata={release}
                    />
                  </div>
                  <div className="browse-release-summary">
                    <span className="browse-eyebrow">
                      {releaseType(release)}
                    </span>
                    <h1>{release.title}</h1>
                    <button
                      className="browse-text-button browse-artist-link"
                      disabled={!release.artist_id && !release.artists?.[0]?.id}
                      onClick={() =>
                        onNavigate({
                          kind: "artist",
                          id: String(
                            release.artist_id || release.artists?.[0]?.id,
                          ),
                        })
                      }
                    >
                      {release.artist}
                      <ChevronRight size={16} />
                    </button>
                    <p>
                      {[
                        release.date || release.release_date || "",
                        `${release.tracks?.length ?? release.track_count ?? 0} audio tracks`,
                        release.quality?.replaceAll("_", " ").toLowerCase(),
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </p>
                    <div className="browse-release-actions">
                      <button
                        className="primary"
                        disabled={!selectedCount || queueBusy}
                        onClick={() =>
                          void queue(
                            String(release.id),
                            selectable
                              .filter((track) =>
                                selection.has(String(track.id)),
                              )
                              .map((track) => String(track.id)),
                          )
                        }
                      >
                        {queueBusy ? (
                          <LoaderCircle size={16} className="spin" />
                        ) : (
                          <ArrowDownToLine size={16} />
                        )}
                        Queue selected tracks ({selectedCount})
                      </button>
                      <button
                        disabled={
                          !selectable.length ||
                          selectable.length !== release.tracks?.length ||
                          queueBusy
                        }
                        title={
                          selectable.length !== release.tracks?.length
                            ? "Some tracks are unavailable. Queue the available selection instead."
                            : "Add every audio track to the download queue"
                        }
                        onClick={() => void queue(String(release.id), null)}
                      >
                        Queue full release
                      </button>
                      <button
                        aria-label="Open release on web"
                        title="Open on web"
                        onClick={() =>
                          void external(
                            `https://tidal.com/browse/album/${release.id}`,
                          ).catch((failure) => setQueueError(String(failure)))
                        }
                      >
                        <ArrowUpRight size={16} />
                      </button>
                    </div>
                    {releaseStatus(release) && (
                      <span
                        className={`badge ${releaseStatus(release)!.className}`}
                      >
                        {releaseStatus(release)!.text}
                      </span>
                    )}
                  </div>
                </header>
                {release.ai && (
                  <div className="browse-notice browse-error">
                    <CircleAlert size={18} />
                    <span>
                      This release is labelled as AI-generated and cannot be
                      downloaded.
                    </span>
                  </div>
                )}
                {release.available === false && (
                  <div className="browse-notice">
                    <CircleAlert size={18} />
                    <span>This release is unavailable for your account.</span>
                  </div>
                )}
                {release.downloadable === false &&
                  release.available !== false &&
                  !release.ai && (
                    <div className="browse-notice">
                      <CircleAlert size={18} />
                      <span>
                        Refresh this release to verify availability before
                        queueing.
                      </span>
                    </div>
                  )}
                {release.tracks?.length ? (
                  renderTracks(release.tracks, release)
                ) : (
                  <div className="browse-empty">
                    <Music2 size={36} />
                    <h2>No audio tracks available</h2>
                    <p>Videos are excluded from the download queue.</p>
                  </div>
                )}
              </>
            )}
            {result?.has_more && (
              <div className="browse-more">
                <button
                  disabled={
                    loadingMore || loading || playlistLoading || queueBusy
                  }
                  onClick={() => void load(false, true)}
                >
                  {loadingMore && <LoaderCircle size={16} className="spin" />}
                  Show more
                </button>
              </div>
            )}
            {result?.stale && !result.warning && (
              <p className="browse-cache-note">
                Showing saved catalogue information.
              </p>
            )}
          </>
        )}
      </div>
    </section>
  );
}
