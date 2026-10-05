/** An omitted selection includes every value, including values not loaded in a menu. */
export type ColumnSelection = { include?: string[]; exclude?: string[] };

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
