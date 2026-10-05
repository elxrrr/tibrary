import { useEffect, useLayoutEffect, useRef, useState, useId, Fragment } from "react";
import { createPortal } from "react-dom";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  Check as CheckIcon,
  Filter,
  MoreHorizontal,
} from "lucide-react";
import { Row, readable } from "./api";
import { Selection, parentState, toggleChild, toggleParent } from "./selection";
import { ColumnSelection, columnSelectionLabel, columnValueSelected, toggleColumnValue } from "./columnFilters";
export type { ColumnSelection } from "./columnFilters";
export type Column = { key: string; label: string };
export type ColumnFilterOption = { value: string; label: string; count?: number };
export type ColumnFilterOptions = { options: ColumnFilterOption[]; total: number };
export type HeaderFilter = {
  label: string;
  selection?: ColumnSelection;
  /** Changes when data or other column filters change, excluding this filter. */
  optionsKey?: string;
  loadOptions: (search: string) => Promise<ColumnFilterOptions>;
  onChange: (selection: ColumnSelection | undefined) => void;
};
type HeaderMenu = {
  column: Column;
  x: number;
  y: number;
  trigger: HTMLElement;
  instance: number;
};
function Check({
  state,
  onChange,
  label,
  disabled = false,
}: {
  state: string;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = state === "mixed";
  }, [state]);
  return (
    <input
      ref={ref}
      type="checkbox"
      aria-label={label}
      checked={state === "checked"}
      disabled={disabled}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onChange={(e) => onChange(e.target.checked)}
    />
  );
}
export function DataTable({
  rows,
  columns,
  selected,
  onSelect,
  sort,
  direction,
  onSort,
  tree,
  grouped,
  treeSelection,
  onTreeSelect,
  onExpand,
  loadingDetails,
  expanded,
  setExpanded,
  onDetail,
  onMenu,
  loading,
  busy,
  headerFilters,
  selectionLabel,
}: {
  rows: Row[];
  columns: Column[];
  selected: Set<string>;
  onSelect: (s: Set<string>) => void;
  sort: string;
  direction: string;
  onSort: (key: string, direction?: "asc" | "desc") => void;
  tree?: boolean;
  grouped?: boolean;
  treeSelection?: Selection;
  onTreeSelect?: (s: Selection) => void;
  onExpand?: (r: Row) => void;
  loadingDetails?: Set<string>;
  expanded: Set<string>;
  setExpanded: (s: Set<string>) => void;
  onDetail: (r: Row) => void;
  onMenu: (r: Row, x: number, y: number) => void;
  loading: boolean;
  busy: boolean;
  headerFilters?: Record<string, HeaderFilter>;
  selectionLabel?: (row: Row) => string;
}) {
  const [anchor, setAnchor] = useState<number | null>(null);
  const [headerMenu, setHeaderMenu] = useState<HeaderMenu | null>(null);
  const [menuPosition, setMenuPosition] = useState({ left: 8, top: 8 });
  const [facetSearch, setFacetSearch] = useState("");
  const [facetOptions, setFacetOptions] = useState<ColumnFilterOptions | null>(null);
  const [facetLoading, setFacetLoading] = useState(false);
  const [facetError, setFacetError] = useState("");
  const [facetRetry, setFacetRetry] = useState(0);
  const facetRequest = useRef(0);
  const menuInstance = useRef(0);
  const headerMenuRef = useRef<HTMLDivElement>(null);
  const headerMenuLabelId = useId();
  const scroller = useRef<HTMLDivElement>(null);
  const menuFilter = headerMenu ? headerFilters?.[headerMenu.column.key] : undefined;
  const latestMenuFilter = useRef(menuFilter);
  latestMenuFilter.current = menuFilter;
  const menuColumnKey = headerMenu?.column.key;
  const menuHasFilter = !!menuFilter;
  const menuOptionsKey = menuFilter?.optionsKey;
  useEffect(() => setAnchor(null), [rows]);
  function closeHeaderMenu() {
    const trigger = headerMenu?.trigger;
    setHeaderMenu(null);
    if (trigger?.isConnected) trigger.focus({ preventScroll: true });
  }
  function openHeaderMenu(column: Column, x: number, y: number, trigger: HTMLElement) {
    setFacetSearch("");
    setFacetOptions(null);
    setFacetError("");
    setMenuPosition({ left: Math.max(8, x), top: Math.max(8, y) });
    setHeaderMenu({ column, x, y, trigger, instance: ++menuInstance.current });
  }
  useEffect(() => {
    const request = ++facetRequest.current;
    if (!menuColumnKey || !menuHasFilter) return;
    setFacetLoading(true);
    setFacetError("");
    const timer = window.setTimeout(() => {
      const filter = latestMenuFilter.current;
      if (!filter || request !== facetRequest.current) return;
      Promise.resolve().then(() => filter.loadOptions(facetSearch)).then(result => {
        if (request !== facetRequest.current) return;
        setFacetOptions(result);
        setFacetLoading(false);
      }).catch(error => {
        if (request !== facetRequest.current) return;
        setFacetError(error instanceof Error ? error.message : String(error));
        setFacetLoading(false);
      });
    }, facetSearch ? 150 : 0);
    return () => {
      window.clearTimeout(timer);
      if (facetRequest.current === request) facetRequest.current++;
    };
  }, [menuColumnKey, headerMenu?.instance, menuHasFilter, menuOptionsKey, facetSearch, facetRetry]);
  useLayoutEffect(() => {
    if (!headerMenu || !headerMenuRef.current) return;
    const menu = headerMenuRef.current;
    function placeMenu() {
      const box = menu.getBoundingClientRect();
      const position = {
        left: Math.max(8, Math.min(headerMenu!.x, window.innerWidth - box.width - 8)),
        top: Math.max(8, Math.min(headerMenu!.y, window.innerHeight - box.height - 8)),
      };
      setMenuPosition(previous => previous.left === position.left && previous.top === position.top ? previous : position);
    }
    placeMenu();
    menu.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
    const observer = new ResizeObserver(placeMenu);
    observer.observe(menu);
    window.addEventListener("resize", placeMenu);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", placeMenu);
    };
  }, [headerMenu]);
  useEffect(() => {
    if (headerMenu && !columns.some((column) => column.key === headerMenu.column.key))
      closeHeaderMenu();
  }, [columns, headerMenu]);
  useEffect(() => {
    if (!headerMenu) return;
    // WebKit does not always focus a clicked menu button. Filtering can also
    // replace the focused row, so Escape must work even when focus leaves it.
    function dismiss(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      closeHeaderMenu();
    }
    window.addEventListener("keydown", dismiss, true);
    return () => window.removeEventListener("keydown", dismiss, true);
  }, [headerMenu]);
  const headerContextMenu = headerMenu && createPortal(
    <>
      <div className="menu-scrim" onClick={closeHeaderMenu} />
      <div
        ref={headerMenuRef}
        role="menu"
        aria-label={`${headerMenu.column.label} column options`}
        className="context-menu header-context-menu"
        style={{ ...menuPosition, width: "min(300px, calc(100vw - 16px))", minWidth: 0, maxHeight: "calc(100vh - 16px)", overflowY: "auto" }}
        onKeyDown={(event) => {
          if (event.key === "Escape" || event.key === "Tab") {
            if (event.key === "Escape") event.preventDefault();
            event.stopPropagation();
            closeHeaderMenu();
            return;
          }
          const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
          const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
          if (event.target instanceof HTMLInputElement && !["ArrowDown", "ArrowUp"].includes(event.key)) return;
          let next: number | undefined;
          if (event.key === "ArrowDown") next = (index + 1) % buttons.length;
          if (event.key === "ArrowUp") next = index < 0 ? buttons.length - 1 : (index + buttons.length - 1) % buttons.length;
          if (event.key === "Home") next = 0;
          if (event.key === "End") next = buttons.length - 1;
          if (next !== undefined) {
            event.preventDefault();
            event.stopPropagation();
            buttons[next]?.focus();
          }
        }}
      >
        {(["asc", "desc"] as const).map((order) => <button
          key={order}
          role="menuitemradio"
          aria-checked={sort === headerMenu.column.key && direction === order}
          onClick={() => {
            const key = headerMenu.column.key;
            closeHeaderMenu();
            if (sort !== key || direction !== order) onSort(key, order);
          }}
        >
          <CheckIcon aria-hidden="true" size={14} style={{ opacity: sort === headerMenu.column.key && direction === order ? 1 : 0 }} />
          Sort {order === "asc" ? "ascending" : "descending"}
        </button>)}
        {menuFilter && <>
          <hr />
          <div id={headerMenuLabelId} className="header-menu-label">{menuFilter.label}</div>
          <input
            className="column-filter-search"
            type="search"
            aria-label={`Find ${headerMenu.column.label} values`}
            placeholder="Find values…"
            value={facetSearch}
            onChange={event => setFacetSearch(event.target.value)}
            style={{ width: "calc(100% - 12px)", margin: "4px 6px 6px", minWidth: 0 }}
          />
          <div className="column-filter-controls" style={{ display: "flex" }}>
            <button role="menuitem" onClick={() => menuFilter.onChange(undefined)}>Select all</button>
            <button role="menuitem" onClick={() => menuFilter.onChange({ include: [] })}>Clear selection</button>
          </div>
          <div role="group" aria-labelledby={headerMenuLabelId} className="column-filter-values" style={{ maxHeight: "min(300px, 40vh)", overflowY: "auto" }}>
            {(facetOptions?.options || []).map((option) => <button
              key={option.value}
              role="menuitemcheckbox"
              aria-checked={columnValueSelected(menuFilter.selection, option.value)}
              onClick={() => {
                menuFilter.onChange(toggleColumnValue(menuFilter.selection, option.value, !columnValueSelected(menuFilter.selection, option.value)));
              }}
            >
              <CheckIcon aria-hidden="true" size={14} style={{ flexShrink: 0, opacity: columnValueSelected(menuFilter.selection, option.value) ? 1 : 0 }} />
              <span className="column-filter-value" title={option.label} style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{option.label}</span>
              {option.count !== undefined && <span className="column-filter-count" style={{ marginLeft: "auto", color: "var(--muted)", fontVariantNumeric: "tabular-nums" }}>{option.count.toLocaleString()}</span>}
            </button>)}
          </div>
          {facetLoading && <div className="header-menu-label" role="status">Loading values…</div>}
          {!facetLoading && facetError && <>
            <div className="header-menu-label" role="alert" title={facetError}>Could not load column values.</div>
            <button role="menuitem" onClick={() => setFacetRetry(value => value + 1)}>Try again</button>
          </>}
          {!facetLoading && !facetError && facetOptions && <div className="header-menu-label" role="status">
            {!facetOptions.total ? "No matching values" : facetOptions.total > facetOptions.options.length
              ? `Showing ${facetOptions.options.length.toLocaleString()} of ${facetOptions.total.toLocaleString()} values. Search to find more.`
              : `${facetOptions.total.toLocaleString()} ${facetOptions.total === 1 ? "value" : "values"}`}
            {menuFilter.selection !== undefined && ` · ${columnSelectionLabel(menuFilter.selection)}`}
          </div>}
        </>}
      </div>
    </>,
    document.body,
  );
  function select(row: Row, index: number, event: React.MouseEvent) {
    const next =
      event.metaKey || event.ctrlKey || event.shiftKey
        ? new Set(selected)
        : new Set<string>();
    if (event.shiftKey && anchor !== null) {
      for (let i = Math.min(anchor, index); i <= Math.max(anchor, index); i++)
        next.add(rows[i].id);
    } else if (next.has(row.id)) next.delete(row.id);
    else next.add(row.id);
    onSelect(next);
    setAnchor(index);
  }
  function groupState(row: Row) {
    if (selected.has(row.id)) return "checked";
    const children = row.children || [];
    const count = children.filter((c: Row) => selected.has(c.id)).length;
    return count ? count === children.length ? "checked" : "mixed" : "empty";
  }
  const checked = rows.filter((r) =>
    tree
      ? parentState(
          treeSelection?.[r.id],
          (r.children || []).map((c: Row) => c.id),
        ) === "checked"
      : grouped ? groupState(r) === "checked" : selected.has(r.id),
  ).length;
  return (
    <div
      ref={scroller}
      className={"table-scroll " + (loading ? "loading" : "")}
      aria-busy={loading}
    >
      <table>
        <thead>
          <tr>
            <th className="check">
              <Check
                label="Select visible rows"
                state={
                  rows.length && checked === rows.length
                    ? "checked"
                    : checked
                      ? "mixed"
                      : "empty"
                }
                disabled={busy && tree}
                onChange={(yes) => {
                  if (tree && onTreeSelect) {
                    let next = { ...treeSelection };
                    rows.forEach((r) => (next = toggleParent(next, r.id, yes)));
                    onTreeSelect(next);
                  } else {
                    const next = new Set(selected);
                    rows.forEach((r) => {
                      yes ? next.add(r.id) : next.delete(r.id);
                      if (grouped) (r.children || []).forEach((c: Row) => next.delete(c.id));
                    });
                    onSelect(next);
                  }
                }}
              />
            </th>
            {columns.map((c) => (
              <th
                key={c.key}
                title="Click to sort. Right-click for column options."
                onContextMenu={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  const trigger = (event.target as HTMLElement).closest("button") || event.currentTarget.querySelector("button");
                  if (trigger) openHeaderMenu(c, event.clientX, event.clientY, trigger);
                }}
                onKeyDown={(event) => {
                  if (event.key !== "ContextMenu" && !(event.key === "F10" && event.shiftKey)) return;
                  event.preventDefault();
                  event.stopPropagation();
                  const trigger = (event.target as HTMLElement).closest("button") || event.currentTarget.querySelector("button");
                  if (!trigger) return;
                  const box = trigger.getBoundingClientRect();
                  openHeaderMenu(c, box.left, box.bottom, trigger);
                }}
                aria-sort={
                  sort === c.key
                    ? direction === "asc"
                      ? "ascending"
                      : "descending"
                    : "none"
                }
              >
                <div className="table-header-actions"><button onClick={() => onSort(c.key)}>
                  {c.label}
                  {sort === c.key ? (
                    direction === "asc" ? (
                      <ArrowUp size={13} />
                    ) : (
                      <ArrowDown size={13} />
                    )
                  ) : null}
                </button>
                {headerFilters?.[c.key] && <button
                  className={"column-filter-button " + (headerFilters[c.key].selection !== undefined ? "active" : "")}
                  aria-label={`Filter ${c.label}`}
                  title={`${headerFilters[c.key].label}: ${columnSelectionLabel(headerFilters[c.key].selection)}`}
                  aria-haspopup="menu"
                  aria-expanded={headerMenu?.column.key === c.key}
                  onClick={(event) => {
                    event.stopPropagation();
                    if (headerMenu?.column.key === c.key) { closeHeaderMenu(); return; }
                    const box = event.currentTarget.getBoundingClientRect();
                    openHeaderMenu(c, box.left, box.bottom, event.currentTarget);
                  }}
                ><Filter aria-hidden="true" size={13} /></button>}
                </div>
              </th>
            ))}
            <th className="more" />
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => {
            const state = tree
              ? parentState(
                  treeSelection?.[r.id],
                  (r.children || []).map((c: Row) => c.id),
                )
              : grouped ? groupState(r) : selected.has(r.id)
                ? "checked"
                : "empty";
            return (
              <Fragment key={r.id}>
                <tr
                  className={
                    (selected.has(r.id) ? "selected " : "") +
                    (r.ignored || (tree && state === "empty") ? "inactive" : "")
                  }
                  tabIndex={0}
                  onClick={(e) => select(r, i, e)}
                  onDoubleClick={(e) => {
                    if ((e.target as Element).closest("button,input")) return;
                    onDetail(r);
                  }}
                  onKeyDown={(e) => {
                    if (e.target !== e.currentTarget) return;
                    if (e.key === "Enter") onDetail(r);
                    if (e.key === " ") {
                      e.preventDefault();
                      const s = new Set(selected);
                      s.has(r.id) ? s.delete(r.id) : s.add(r.id);
                      onSelect(s);
                    }
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    if (!selected.has(r.id)) onSelect(new Set([r.id]));
                    onMenu(r, e.clientX, e.clientY);
                  }}
                >
                  <td className="check">
                    <Check
                      label={selectionLabel?.(r) || `Select ${r.release || r.title || r.artist}`}
                      state={state}
                      disabled={busy && tree}
                      onChange={(yes) => {
                        if (tree && onTreeSelect)
                          onTreeSelect(
                            toggleParent(treeSelection || {}, r.id, yes),
                          );
                        else {
                          const next = new Set(selected);
                          yes ? next.add(r.id) : next.delete(r.id);
                          if (grouped) (r.children || []).forEach((c: Row) => next.delete(c.id));
                          onSelect(next);
                        }
                      }}
                    />
                  </td>
                  {columns.map((c, j) => (
                    <td key={c.key} title={readable(r[c.key])}>
                      {j === 0 && (tree || grouped) ? (
                        <button
                          className="disclosure"
                          aria-label={`${expanded.has(r.id) ? "Collapse" : "Expand"} ${r.release}`}
                          aria-expanded={expanded.has(r.id)}
                          onDoubleClick={(e) => e.stopPropagation()}
                          onClick={(e) => {
                            e.stopPropagation();
                            if (e.detail > 1) return;
                            const s = new Set(expanded);
                            if (s.has(r.id)) s.delete(r.id);
                            else {
                              s.add(r.id);
                              if (!r.expanded_available) onExpand?.(r);
                            }
                            setExpanded(s);
                          }}
                        >
                          {expanded.has(r.id) ? (
                            <ChevronDown size={14} />
                          ) : (
                            <ChevronRight size={14} />
                          )}
                        </button>
                      ) : null}
                      {["status", "recommendation"].includes(c.key) ? (
                        <span
                          className={
                            "badge " +
                            String(r[c.key]).toLowerCase().replaceAll(" ", "-")
                          }
                        >
                          {readable(r[c.key])}
                        </span>
                      ) : (
                        readable(r[c.key])
                      )}
                    </td>
                  ))}
                  <td className="more">
                    <button
                      aria-label={`Actions for ${r.release || r.title || r.artist}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        if (!selected.has(r.id)) onSelect(new Set([r.id]));
                        onMenu(r, e.clientX, e.clientY);
                      }}
                    >
                      <MoreHorizontal size={16} />
                    </button>
                  </td>
                </tr>
                {grouped && expanded.has(r.id) && (r.children || []).map((child: Row) => (
                  <tr key={child.id} className={"child " + (selected.has(child.id) ? "selected" : "")}
                    onClick={e => {const next=e.metaKey||e.ctrlKey||e.shiftKey ? new Set(selected):new Set<string>();next.add(child.id);onSelect(next);}}
                    onDoubleClick={(e) => {if (!(e.target as Element).closest("button,input")) onDetail(child);}}
                    onContextMenu={e => {e.preventDefault();if (!selected.has(child.id)) onSelect(new Set([child.id]));onMenu(child,e.clientX,e.clientY);}}>
                    <td><Check label={`Select duplicate ${child.release}`} state={selected.has(child.id)||selected.has(r.id)?"checked":"empty"} disabled={busy}
                      onChange={yes => {const next=new Set(selected);if(next.delete(r.id)){for(const sibling of r.children)next.add(sibling.id);}yes?next.add(child.id):next.delete(child.id);onSelect(next);}} /></td>
                    {columns.map(c => <td key={c.key} title={readable(child[c.key])}>{readable(child[c.key])}</td>)}
                    <td className="more"><button aria-label={`Actions for ${child.release}`} onClick={e=>{e.stopPropagation();onMenu(child,e.clientX,e.clientY);}}><MoreHorizontal size={16}/></button></td>
                  </tr>
                ))}
                {tree &&
                  expanded.has(r.id) &&
                  (!r.expanded_available ? (
                    <tr className="child">
                      <td />
                      <td colSpan={columns.length + 1}>
                        {loadingDetails?.has(r.id) ? <span role="status">Loading track details…</span> : <>
                          Track details are not cached yet.{" "}
                          <button onClick={() => onExpand?.(r)}>Load track details</button>
                        </>}
                      </td>
                    </tr>
                  ) : (
                    (r.children || []).map((t: Row) => (
                      <tr
                        key={r.id + ":" + t.id}
                        className={
                          "child " +
                          (state === "empty" ||
                          (treeSelection?.[r.id] !== null &&
                            !treeSelection?.[r.id]?.includes(t.id))
                            ? "inactive"
                            : "")
                        }
                        onDoubleClick={() => onDetail({ ...t, parent: r.id, online_id: t.id })}
                        onContextMenu={(e) => {e.preventDefault(); onMenu({...t,parent:r.id,online_id:t.id},e.clientX,e.clientY);}}
                      >
                        <td>
                          <Check
                            label={`Select track ${t.title}`}
                            state={
                              treeSelection?.[r.id] === null ||
                              treeSelection?.[r.id]?.includes(t.id)
                                ? "checked"
                                : "empty"
                            }
                            disabled={busy}
                            onChange={(yes) =>
                              onTreeSelect?.(
                                toggleChild(
                                  treeSelection || {},
                                  r.id,
                                  t.id,
                                  r.children.map((x: Row) => x.id),
                                  yes,
                                ),
                              )
                            }
                          />
                        </td>
                        <td className="track-title" colSpan={Math.max(1, columns.length - 3)}>
                          {t.title}
                        </td>
                        <td className="track-position" title="Disc · track">{t.position}</td>
                        <td>
                          {t.duration
                            ? `${Math.floor(t.duration / 60)}:${String(Math.round(t.duration % 60)).padStart(2, "0")}`
                            : "—"}
                        </td>
                        <td title={t.isrc || "No recording identifier"}>{t.isrc || "No ISRC"}</td>
                        <td className="more"><button aria-label={`Actions for track ${t.title}`} onClick={(e)=>{e.stopPropagation();onMenu({...t,parent:r.id,online_id:t.id},e.clientX,e.clientY);}}><MoreHorizontal size={16}/></button></td>
                      </tr>
                    ))
                  ))}
              </Fragment>
            );
          })}
        </tbody>
      </table>
      {!rows.length && (
        <div className="empty">
          <strong>
            {loading ? "Loading saved library data…" : "No matching items"}
          </strong>
          <p>
            {loading
              ? "The table will stay in place while data loads."
              : "Adjust the filter, add a library, or run the relevant check."}
          </p>
        </div>
      )}
      {headerContextMenu}
    </div>
  );
}
