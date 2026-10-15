import { describe, it, expect } from "vitest";
import {
  toggleParent,
  toggleChild,
  parentState,
  selectedReleases,
  releaseRows,
  releaseSelectionState,
  toggleReleaseRows,
} from "./selection";
describe("release selection", () => {
  it("cascades approval and clearing", () => {
    let s = toggleParent({}, "a", true);
    expect(parentState(s.a, ["1", "2"])).toBe("checked");
    s = toggleChild(s, "a", "1", ["1", "2"], false);
    expect(s.a).toEqual(["2"]);
    expect(parentState(s.a, ["1", "2"])).toBe("mixed");
    s = toggleParent(s, "a", false);
    expect(selectedReleases(s)).toEqual({});
    s = toggleParent(s, "a", true);
    expect(s.a).toBeNull();
  });
  it("isolates releases sharing recording ids", () => {
    let s = toggleParent({}, "a", true);
    s = toggleChild(s, "b", "1", ["1", "2"], true);
    expect(s.a).toBeNull();
    expect(s.b).toEqual(["1"]);
  });
});

describe("artist release selection", () => {
  const artist = {
    id: "missing-artist:one", artist_group: true, children: [
      {id: "album-one", children: [{id:"track-one"}, {id:"shared-track"}]},
      {id: "album-two", children: [{id:"shared-track"}]},
      {id: "uncached-album", children: []},
    ],
  };
  it("cascades through releases without submitting synthetic artist or track IDs", () => {
    let selection = toggleReleaseRows({"off-page":null}, [artist], true);
    expect(selectedReleases(selection)).toEqual({"off-page":null,"album-one":null,"album-two":null,"uncached-album":null});
    expect(releaseSelectionState(selection, artist)).toBe("checked");
    selection = toggleChild(selection, "album-one", "shared-track", ["track-one","shared-track"], false);
    expect(releaseSelectionState(selection, artist)).toBe("mixed");
    expect(selection["album-two"]).toBeNull();
    expect(selection["album-one"]).toEqual(["track-one"]);
    selection = toggleReleaseRows(selection, [artist], false);
    expect(releaseSelectionState(selection, artist)).toBe("empty");
    expect(selectedReleases(selection)).toEqual({"off-page":null});
    selection = toggleReleaseRows(selection, [artist], true);
    expect(releaseSelectionState(selection, artist)).toBe("checked");
  });
  it("keeps flat release rows compatible and never flattens their tracks", () => {
    expect(releaseRows([artist])).toEqual(artist.children);
    expect(releaseRows(artist.children)).toEqual(artist.children);
    expect(releaseSelectionState({}, {id:"empty-group",artist_group:true,children:[]})).toBe("empty");
  });
});
