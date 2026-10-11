import type { LibraryAvailability } from "./api";

export const reconnectLibrary = "Reconnect the library drive to use its files.";
export const fileActionKinds = new Set(["scan", "mqa", "artwork", "apply", "consolidate"]);

export function unavailableLibrary(roots: LibraryAvailability[] | undefined, root: string): string {
  const library = roots?.find(library => library.root === root);
  return library?.available === false ? library.unavailable_reason || reconnectLibrary : "";
}

/** A downloaded file may be on a different, available drive from the library. */
export function unavailableFile(roots: LibraryAvailability[] | undefined, path: string): string {
  const library = roots?.filter(library => {
    const root = library.root.replace(/\/+$/, "") || "/";
    return path === root || path.startsWith(root === "/" ? root : `${root}/`);
  }).sort((a, b) => b.root.length - a.root.length)[0];
  return library?.available === false ? library.unavailable_reason || reconnectLibrary : "";
}
