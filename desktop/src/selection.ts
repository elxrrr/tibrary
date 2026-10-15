export type Selection = Record<string, string[] | null>;
export function parentState(
  value: string[] | null | undefined,
  children: string[],
): "checked" | "mixed" | "empty" {
  if (value === null) return "checked";
  if (!value?.length) return "empty";
  return children.length > 0 && children.every((id) => value.includes(id))
    ? "checked"
    : "mixed";
}
export function toggleParent(
  selection: Selection,
  id: string,
  checked: boolean,
): Selection {
  return { ...selection, [id]: checked ? null : [] };
}
export function toggleChild(
  selection: Selection,
  parent: string,
  id: string,
  children: string[],
  checked: boolean,
): Selection {
  const before =
    selection[parent] === null ? children : selection[parent] || [];
  const next = new Set(before);
  checked ? next.add(id) : next.delete(id);
  return { ...selection, [parent]: [...next] };
}
export function selectedReleases(selection: Selection): Selection {
  return Object.fromEntries(
    Object.entries(selection).filter(([, v]) => v === null || v.length > 0),
  );
}

export type ReleaseSelectionRow = {
  id: string;
  artist_group?: boolean;
  children?: ReleaseSelectionRow[];
};
/** Artist rows are display containers; only real release IDs enter queue/API state. */
export function releaseRows<T extends ReleaseSelectionRow>(rows: T[]): T[] {
  return rows.flatMap(row => row.artist_group
    ? releaseRows((row.children || []) as T[])
    : [row]);
}
export function releaseSelectionState(
  selection: Selection,
  row: ReleaseSelectionRow,
): "checked" | "mixed" | "empty" {
  const states = releaseRows([row]).map(release =>
    parentState(selection[release.id], (release.children || []).map(track => track.id)));
  if (states.length && states.every(state => state === "checked")) return "checked";
  return states.some(state => state !== "empty") ? "mixed" : "empty";
}
export function toggleReleaseRows(
  selection: Selection,
  rows: ReleaseSelectionRow[],
  checked: boolean,
): Selection {
  const next = { ...selection };
  releaseRows(rows).forEach(release => { next[release.id] = checked ? null : []; });
  return next;
}
