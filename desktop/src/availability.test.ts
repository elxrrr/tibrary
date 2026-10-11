import { describe, expect, it } from "vitest";
import { unavailableFile, unavailableLibrary } from "./availability";

describe("cached library access", () => {
  const roots = [
    {root: "/Volumes/Music", available: false, unavailable_reason: "Music is disconnected"},
    {root: "/Users/dj/Downloads", available: true},
  ];
  it("blocks missing library files while allowing a separate download destination", () => {
    expect(unavailableLibrary(roots, "/Volumes/Music")).toBe("Music is disconnected");
    expect(unavailableFile(roots, "/Volumes/Music/Artist/track.flac")).toBe("Music is disconnected");
    expect(unavailableFile(roots, "/Users/dj/Downloads/track.flac")).toBe("");
    expect(unavailableFile(roots, "/Volumes/Music Archive/track.flac")).toBe("");
  });
  it("uses the most specific registered root and handles trailing slashes", () => {
    expect(unavailableFile([...roots, {root: "/Volumes/Music/Downloads/", available: true}], "/Volumes/Music/Downloads/track.flac")).toBe("");
    expect(unavailableFile([{root: "/Volumes/Music/", available: false}], "/Volumes/Music/track.flac")).toContain("Reconnect");
  });
});
