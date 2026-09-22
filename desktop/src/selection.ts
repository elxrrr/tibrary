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
