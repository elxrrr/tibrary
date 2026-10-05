import { expect, it } from "vitest";
import { columnSelectionLabel, columnValueSelected, toggleColumnValue } from "./columnFilters";

it("deselects from all without losing values outside the loaded facet page", () => {
  const selection = toggleColumnValue(undefined, "Album", false);
  expect(selection).toEqual({ exclude: ["Album"] });
  expect(columnValueSelected(selection, "Album")).toBe(false);
  expect(columnValueSelected(selection, "Not loaded yet")).toBe(true);
  expect(toggleColumnValue(selection, "Album", true)).toBeUndefined();
});

it("combines multiple selections after clear and preserves values outside a search", () => {
  let selection = toggleColumnValue({ include: [] }, "Single", true);
  selection = toggleColumnValue(selection, "EP", true);
  expect(selection).toEqual({ include: ["EP", "Single"] });
  expect(columnValueSelected(selection, "Album")).toBe(false);
  expect(toggleColumnValue(selection, "Single", false)).toEqual({ include: ["EP"] });
  expect(columnSelectionLabel(selection)).toBe("2 selected");
  expect(columnSelectionLabel({ include: [] })).toBe("No values selected");
  expect(columnSelectionLabel(undefined)).toBe("All values");
  expect(columnSelectionLabel({ exclude: ["Album"] })).toBe("All except 1");
});

it("handles blank column values and mixed legacy include/exclude states", () => {
  const selection = toggleColumnValue({ include: ["", "Album"], exclude: ["Album"] }, "EP", true);
  expect(selection).toEqual({ include: ["", "EP"] });
  expect(columnValueSelected(selection, "")).toBe(true);
});
