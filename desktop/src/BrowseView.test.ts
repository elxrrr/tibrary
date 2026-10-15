import { describe, expect, it } from "vitest";
import {
  availableTracks,
  browseLink,
  groupTrackSelection,
  mergeBrowsePage,
  playlistTracks,
} from "./BrowseView";

describe("catalogue browsing", () => {
  it("opens supported catalogue links without trusting lookalike hosts or nonnumeric identifiers", () => {
    expect(browseLink("https://tidal.com/browse/album/123?u=share")).toEqual({
      kind: "release",
      id: "123",
    });
    expect(browseLink("https://listen.tidal.com/track/456")).toEqual({
      kind: "track",
      id: "456",
    });
    expect(browseLink("https://www.tidal.com/artist/789")).toEqual({
      kind: "artist",
      id: "789",
    });
    expect(
      browseLink(
        "https://tidal.com/browse/playlist/a047bb01-ea17-4a5e-a593-7185353c9225?u=share",
      ),
    ).toEqual({ kind: "playlist", id: "a047bb01-ea17-4a5e-a593-7185353c9225" });
    for (const input of [
      "https://tidal.com.evil.example/album/123",
      "file://tidal.com/album/123",
      "http://tidal.com/album/123",
      "https://name@tidal.com/album/123",
      "https://tidal.com:8080/album/123",
      "https://tidal.com/albums/123",
      "https://tidal.com/album/not-an-id",
      "https://tidal.com/track/0",
      "https://tidal.com/playlist/not-a-playlist",
      "https://tidal.com/playlist/a047bb01-ea17-4a5e-a593-7185353c9225/more",
      "an artist name",
    ])
      expect(browseLink(input)).toBeNull();
  });
  it("groups only selected playlist songs by their own releases without duplicate or blocked tracks", () => {
    const items = [
      { id: "1", title: "One", artist: "First", album_id: "10" },
      { id: "2", title: "Two", artist: "Second", release_id: "20" },
      { id: "1", title: "Repeated", artist: "First", album_id: "10" },
      {
        id: "3",
        title: "Unavailable",
        artist: "First",
        album_id: "10",
        available: false,
      },
      { id: "4", title: "AI", artist: "First", album_id: "10", ai: true },
      { id: "5", title: "No release", artist: "First" },
      {
        id: "6",
        title: "Nested release",
        artist: "Third",
        album: { id: "30", title: "Third", artist: "Third" },
      },
    ];
    expect(playlistTracks(items).map((track) => track.id)).toEqual([
      "1",
      "2",
      "6",
    ]);
    expect(
      groupTrackSelection(items, new Set(["1", "2", "3", "4", "5"])),
    ).toEqual({ "10": ["1"], "20": ["2"] });
    expect(groupTrackSelection(items, new Set())).toEqual({});
    expect(groupTrackSelection(items)).toEqual({
      "10": ["1"],
      "20": ["2"],
      "30": ["6"],
    });
  });
  it("preserves repeated playlist positions when pages merge, while queueing the song once", () => {
    const song = {
      id: "1",
      title: "Repeated song",
      artist: "Artist",
      album_id: "10",
    };
    const page = mergeBrowsePage(
      { items: [{ ...song, playlist_position: 1 }] },
      {
        items: [{ ...song, playlist_position: 2 }],
        offset: 1,
        limit: 1,
        has_more: false,
      },
    );
    expect(page.items).toHaveLength(2);
    expect(groupTrackSelection(page.items as (typeof song)[])).toEqual({
      "10": ["1"],
    });
    expect(
      mergeBrowsePage(page, { items: [{ ...song, playlist_position: 2 }] })
        .items,
    ).toHaveLength(2);
  });
  it("only selects available audio and blocks an AI-labelled or unavailable release", () => {
    const release = {
      id: "1",
      artist: "Artist",
      title: "Release",
      tracks: [
        { id: "1", title: "Available", artist: "Artist", available: true },
        { id: "2", title: "Unavailable", artist: "Artist", available: false },
        { id: "3", title: "AI-labelled", artist: "Artist", ai: true },
      ],
    };
    expect(availableTracks(release).map((track) => track.id)).toEqual(["1"]);
    expect(availableTracks({ ...release, ai: true })).toEqual([]);
    expect(availableTracks({ ...release, available: false })).toEqual([]);
    expect(availableTracks({ ...release, downloadable: false })).toEqual([]);
  });
});
