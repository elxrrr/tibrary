/** An omitted selection includes every value, including values not loaded in a menu. */
export type ColumnSelection = { include?: string[]; exclude?: string[] };

/** Counts, positions and free-form evidence stay sortable, without value menus. */
export function columnFilterAvailable(route: string, key: string): boolean {
  if (["tracks", "gained", "duplicates", "position", "online_id", "evidence"].includes(key)) return false;
  return key !== "release" || !["artists", "favourites"].includes(route);
}

export function availableColumnSelections(route: string, selections: Record<string, ColumnSelection>): Record<string, ColumnSelection> {
  return Object.fromEntries(Object.entries(selections).filter(([key]) => columnFilterAvailable(route, key)));
}

export function columnValueSelected(selection: ColumnSelection | undefined, value: string): boolean {
  return (selection?.include === undefined || selection.include.includes(value)) && !selection?.exclude?.includes(value);
}

/** Exclusion mode keeps "all except this value" accurate across paging and search. */
export function toggleColumnValue(selection: ColumnSelection | undefined, value: string, checked: boolean): ColumnSelection | undefined {
  if (selection?.include !== undefined) {
    const values = new Set(selection.include.filter(item => !selection.exclude?.includes(item)));
    checked ? values.add(value) : values.delete(value);
    return { include: [...values].sort() };
  }
  const values = new Set(selection?.exclude || []);
  checked ? values.delete(value) : values.add(value);
  return values.size ? { exclude: [...values].sort() } : undefined;
}

export function columnSelectionLabel(selection: ColumnSelection | undefined): string {
  if (selection?.include !== undefined) {
    const count = selection.include.filter(value => !selection.exclude?.includes(value)).length;
    return count ? `${count} selected` : "No values selected";
  }
  return selection?.exclude?.length ? `All except ${selection.exclude.length}` : "All values";
}
