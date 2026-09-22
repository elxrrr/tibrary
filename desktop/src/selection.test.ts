import { describe, it, expect } from "vitest";
import {
  toggleParent,
  toggleChild,
  parentState,
  selectedReleases,
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
