import { describe, expect, it } from "vitest";
import { availableTracks, browseLink } from "./BrowseView";

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
    for (const input of [
      "https://tidal.com.evil.example/album/123",
      "file://tidal.com/album/123",
      "http://tidal.com/album/123",
      "https://name@tidal.com/album/123",
      "https://tidal.com:8080/album/123",
      "https://tidal.com/albums/123",
      "https://tidal.com/album/not-an-id",
      "https://tidal.com/track/0",
      "an artist name",
    ])
      expect(browseLink(input)).toBeNull();
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
