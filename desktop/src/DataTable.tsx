import { useEffect, useRef, useState, Fragment } from "react";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  MoreHorizontal,
} from "lucide-react";
import { Row, readable } from "./api";
import { Selection, parentState, toggleChild, toggleParent } from "./selection";
export type Column = { key: string; label: string };
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
  expanded,
  setExpanded,
  onDetail,
  onMenu,
  loading,
  busy,
}: {
  rows: Row[];
  columns: Column[];
  selected: Set<string>;
  onSelect: (s: Set<string>) => void;
  sort: string;
  direction: string;
  onSort: (key: string) => void;
  tree?: boolean;
  grouped?: boolean;
  treeSelection?: Selection;
  onTreeSelect?: (s: Selection) => void;
  onExpand?: (r: Row) => void;
  expanded: Set<string>;
  setExpanded: (s: Set<string>) => void;
  onDetail: (r: Row) => void;
  onMenu: (r: Row, x: number, y: number) => void;
  loading: boolean;
  busy: boolean;
}) {
  const [anchor, setAnchor] = useState<number | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  useEffect(() => setAnchor(null), [rows]);
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
                aria-sort={
                  sort === c.key
                    ? direction === "asc"
                      ? "ascending"
                      : "descending"
                    : "none"
                }
              >
                <button onClick={() => onSort(c.key)}>
                  {c.label}
                  {sort === c.key ? (
                    direction === "asc" ? (
                      <ArrowUp size={13} />
                    ) : (
                      <ArrowDown size={13} />
                    )
                  ) : null}
                </button>
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
                      label={`Select ${r.release || r.title || r.artist}`}
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
                        Track details are not cached yet.{" "}
                        {busy
                          ? "An operation is running."
                          : "Use “Load track details” to fetch them."}
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
                        <td colSpan={Math.max(1, columns.length - 2)}>
                          <span className="track-position">{t.position}</span>
                          {t.title}
                        </td>
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
    </div>
  );
}
