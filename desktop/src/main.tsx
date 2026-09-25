import React, { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  ArrowDownToLine,
  ArrowUpRight,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Folder,
  Heart,
  House,
  Image,
  Layers,
  Link,
  LoaderCircle,
  Music2,
  RefreshCw,
  Search,
  Settings,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Tags,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import {
  call,
  active,
  external,
  reveal,
  readable,
  Row,
  Job,
  AppState,
} from "./api";
import { DataTable, Column } from "./DataTable";
import { Selection, selectedReleases } from "./selection";
import "./style.css";
const groups = [
  {
    id: "prepare",
    name: "Prepare library",
    icon: Wrench,
    items: [
      ["correct", "Correct tags", Tags],
      ["organise", "Organise files", Folder],
      ["mqa", "MQA audit", ShieldCheck],
      ["local", "Local duplicates", Music2],
    ],
  },
  {
    id: "catalogue",
    name: "Link catalogue",
    icon: Link,
    items: [
      ["artists", "Link artists", Music2],
      ["links", "Link releases", Link],
      ["favourites", "Favourite artists", Heart],
    ],
  },
  {
    id: "complete",
    name: "Complete library",
    icon: ArrowDownToLine,
    items: [
      ["missing", "Missing releases", Search],
      ["queue", "Download queue", ArrowDownToLine],
      ["downloaded", "Downloaded releases", Check],
    ],
  },
  {
    id: "fix",
    name: "Update library",
    icon: Sparkles,
    items: [
      ["metadata", "Add missing tags", Tags],
      ["artwork", "Fix artwork", Image],
      ["online", "Online replacements", RefreshCw],
    ],
  },
  {
    id: "settings",
    name: "Settings",
    icon: Settings,
    items: [
      ["general", "General", SlidersHorizontal],
      ["connections", "Connections", Link],
      ["downloads", "Downloads", ArrowDownToLine],
      ["activity", "Activity", Activity],
    ],
  },
] as const;
const titles: Record<string, string> = {
  overview: "Overview",
  files: "Local files",
  ...Object.fromEntries(
    groups.flatMap((g) => [
      [g.id, g.name],
      ...g.items.map((i) => [i[0], i[1]]),
    ]),
  ),
};
const actions = [
  ["dates", "Dates", "Standardise date formatting"],
  ["numbers", "Track & disc numbers", "Pad numbers and repair proven totals"],
  ["keys", "Musical keys", "Standardise recognised keys to Camelot"],
  ["lyrics", "Remove lyrics", "Remove embedded lyrics only"],
];
const organisationActions = [
  ["organise", "Folder layout", "Arrange files using the saved tags"],
  ["strays", "Reunite stray tracks", "Review related tracks in other folders"],
  [
    "duplicates",
    "Duplicate positions",
    "Inspect duplicate disc and track slots",
  ],
  ["singles", "Redundant singles", "Review singles already held on albums"],
];
const descriptions: Record<string, string> = {
  overview: "Your library, from local preparation to new music.",
  correct:
    "Choose one correction. Preview exactly what changes before writing tags.",
  organise:
    "Arrange files from their tags. Review destinations before moving anything.",
  links: "Link local recordings and releases. Music files stay unchanged.",
  artists:
    "Match album artists, including multiple identities for the same artist.",
  metadata:
    "Fill missing tags from verified links, including BPM and Camelot key.",
  artwork:
    "Find genuine 1280 × 1280 front covers. Smaller images are never upscaled.",
  mqa: "Inspect local FLAC files and retain the results for replacement review.",
  local: "Review complete local replacements before removing duplicate files.",
  online: "Find larger online releases that preserve every recording you own.",
  missing:
    "Review available releases and choose whole releases or individual tracks.",
  queue: "Only approved audio tracks will be downloaded.",
  downloaded: "Releases explicitly completed by the download engine.",
  favourites: "One row per artist across all confirmed identities.",
  connections: "Catalogue access, account sessions and extended metadata.",
  downloads: "Audio quality, output folders and service pacing.",
  general: "Appearance, libraries and matching preferences.",
};
const fileColumns: Column[] = [
  { key: "artist", label: "Album artist" },
  { key: "release", label: "Release" },
  { key: "title", label: "Track" },
  { key: "position", label: "Disc · track" },
  { key: "status", label: "Status" },
  { key: "evidence", label: "Evidence" },
];
const releaseColumns: Column[] = [
  { key: "artist", label: "Album artist" },
  { key: "release", label: "Release" },
  { key: "date", label: "Date" },
  { key: "type", label: "Type" },
  { key: "tracks", label: "Tracks" },
  { key: "status", label: "Coverage" },
  { key: "recommendation", label: "Recommendation" },
];
function Modal({
  title,
  children,
  onClose,
  wide = false,
}: {
  title: string;
  children: React.ReactNode;
  onClose: () => void;
  wide?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    ref.current?.showModal();
  }, []);
  return (
    <dialog ref={ref} className={wide ? "wide" : ""} onCancel={onClose}>
      <header>
        <h2>{title}</h2>
        <button aria-label="Close dialog" onClick={onClose}>
          <X size={18} />
        </button>
      </header>
      {children}
    </dialog>
  );
}
function getLogCategory(log: { message: string; category?: string; level?: string }): string {
  if (log.category) return log.category;
  const msg = (log.message || "").toLowerCase();
  if (log.level === "error" || msg.includes("error") || msg.includes("failed") || msg.includes("fail") || msg.includes("err")) return "error";
  if (msg.includes("download") || msg.includes("streamrip") || msg.includes("saving track") || msg.includes("fetching track")) return "download";
  if (msg.includes("scan") || msg.includes("read tags") || msg.includes("indexed") || msg.includes("refresh local")) return "scan";
  if (msg.includes("link") || msg.includes("catalogue") || msg.includes("match") || msg.includes("artist")) return "linking";
  if (msg.includes("trash") || msg.includes("duplicate") || msg.includes("clean") || msg.includes("consolidation") || msg.includes("re-scan")) return "cleanup";
  return "general";
}

function App() {
  const [state, setState] = useState<AppState | null>(null),
    [route, setRoute] = useState(
      localStorage.getItem("tibrary.route") || "overview",
    ),
    [root, setRoot] = useState(localStorage.getItem("tibrary.root") || "");
  const [logCategory, setLogCategory] = useState("all"),
    [logSearch, setLogSearch] = useState("");
  const [collapsed, setCollapsed] = useState(new Set<string>()),
    [error, setError] = useState(""),
    [toast, setToast] = useState(""),
    [closing, setClosing] = useState(false),
    [stopped, setStopped] = useState(false);
  const [data, setData] = useState<{ rows: Row[]; total: number }>({
      rows: [],
      total: 0,
    }),
    [loading, setLoading] = useState(false),
    [query, setQuery] = useState(""),
    [filter, setFilter] = useState("all"),
    [sort, setSort] = useState("artist"),
    [direction, setDirection] = useState("asc"),
    [offset, setOffset] = useState(0);
  const [selected, setSelected] = useState(new Set<string>()),
    [expanded, setExpanded] = useState(new Set<string>()),
    [selection, setSelection] = useState<Selection>({});
  const [action, setAction] = useState("dates"),
    [preview, setPreview] = useState<string | undefined>(),
    [timeline, setTimeline] = useState("Newer than newest owned"),
    [recommendation, setRecommendation] = useState("All recommendations"),
    [copyright, setCopyright] = useState("All copyrights"),
    [releaseType, setReleaseType] = useState("All types");
  const [latestMissing, setLatestMissing] = useState<Row[] | null>(null);
  useEffect(() => {
    if (route !== "overview" || !state) return;
    let alive = true;
    call("table", {route: "missing", timeline: "All missing releases", status: "Missing release",
      sort: "date", direction: "desc", limit: 20, recommendation: "All recommendations"})
      .then((result) => { if (alive) setLatestMissing(result.rows); })
      .catch((e) => { if (alive) notifyError(e); });
    return () => { alive = false; };
  }, [route, state?.revision]);
  const [detail, setDetail] = useState<any>(null),
    [review, setReview] = useState<any>(null),
    [menu, setMenu] = useState<{ row: Row; x: number; y: number } | null>(null),
    [settings, setSettings] = useState<any>(null),
    [auth, setAuth] = useState(""),
    [credentials, setCredentials] = useState({ client: "", secret: "" });
  const [manual, setManual] = useState(""),
    [deepQuery, setDeepQuery] = useState(""),
    [deep, setDeep] = useState<any>(null);
  const [submitting, setSubmitting] = useState(false);
  useEffect(() => {
    if (menu)
      document
        .querySelector<HTMLButtonElement>('[role="menu"] button')
        ?.focus();
  }, [menu]);
  const sequence = useRef(0),
    seenJob = useRef(""),
    lastRoute = useRef(route),
    stateRef = useRef(state);
  stateRef.current = state;
  const busy = submitting || active(state?.job);
  const tree = ["missing", "queue", "downloaded"].includes(route);
  const notifyError = (e: any) => setError(String(e?.message || e));
  async function refresh(targetRoot?: string) {
    try {
      const activeRoot = targetRoot !== undefined ? targetRoot : root;
      const s = await call<AppState>("state", activeRoot ? { root: activeRoot } : {});
      setState(s);
      if (!activeRoot && s.roots.length > 0) {
        setRoot(s.roots[0].root);
      } else if (activeRoot && !s.roots.some((r) => r.root === activeRoot)) {
        setRoot(s.roots[0]?.root || "");
      }
    } catch (e) {
      notifyError(e);
    }
  }
  useEffect(() => {
    refresh(root);
    call("settings").then(setSettings).catch(notifyError);
  }, []);
  useEffect(() => {
    localStorage.setItem("tibrary.root", root);
    setSelected(new Set());
    setOffset(0);
    setPreview(undefined);
    refresh(root);
  }, [root]);
  useEffect(() => {
    localStorage.setItem("tibrary.route", route);
    setSelection({});
    setSelected(new Set());
    setOffset(0);
    setQuery("");
    setFilter(
      ["correct", "organise"].includes(route)
        ? "affected"
        : route === "links"
          ? "unlinked"
          : "all",
    );
    setAction(route === "organise" ? "organise" : "dates");
    setPreview(undefined);
    lastRoute.current = route;
  }, [route]);
  useEffect(() => {
    document.documentElement.dataset.theme = state?.settings.theme || "system";
  }, [state?.settings.theme]);
  const viewArgs = {
    route,
    root: root || undefined,
    offset,
    search: query,
    filter,
    sort,
    direction,
    action,
    preview_id: preview,
    timeline,
    recommendation,
    copyright,
    type: releaseType,
  };
  useEffect(() => {
    let alive = true;
    const n = ++sequence.current;
    if (!state || (root && !state.roots.some((r) => r.root === root))) return;
    if (["overview", "prepare", "catalogue", "complete", "fix", "settings"].includes(route)) {
      setLoading(false);
      return;
    }
    setLoading(true);
    const timer = setTimeout(
      () =>
        call("table", viewArgs)
          .then((value) => {
            if (alive && n === sequence.current) {
              setData(value);
              if (!preview && value.preview_id) setPreview(value.preview_id);
              if (route === "queue")
                setSelection(
                  Object.fromEntries(
                    value.rows.map((r: Row) => [r.id, r.selected]),
                  ),
                );
            }
          })
          .catch((e) => alive && notifyError(e))
          .finally(() => alive && setLoading(false)),
      query ? 150 : 0,
    );
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [
    route,
    root,
    offset,
    query,
    filter,
    sort,
    direction,
    action,
    preview,
    timeline,
    recommendation,
    copyright,
    releaseType,
    state?.revision,
  ]);
  useEffect(() => {
    let dispose: (() => void) | undefined;
    listen<any>("backend-event", ({ payload: p }) => {
      if (p.event === "progress")
        setState((s) =>
          s
            ? {
                ...s,
                job: p.job || s.job,
                logs: [
                  ...s.logs.filter(log => !p.job?.id || log.progress_id !== p.job.id),
                  {
                    progress_id: p.job?.id,
                    at: new Date().toISOString(),
                    message: p.message,
                    level: p.job?.status === "failed" ? "error" : p.level || "info",
                    category: p.category || (p.job?.kind ? (p.job.kind === "scan" ? "scan" : p.job.kind === "download" ? "download" : p.job.kind === "link" ? "linking" : p.job.kind.includes("duplicate") ? "cleanup" : undefined) : undefined),
                  },
                ].slice(-1000),
              }
            : s,
        );
      if (["changed", "job", "ready"].includes(p.event)) refresh();
      if (p.event === "authentication") {
        setAuth("");
        refresh();
      }
      if (p.event === "open_url") external(p.url).catch(notifyError);
      if (p.event === "closing") setClosing(true);
      if (p.event === "stopped") setStopped(true);
    })
      .then((fn) => (dispose = fn))
      .catch(() => {});
    return () => dispose?.();
  }, [root]);
  // Fallback snapshot also recovers a missed completion event during window startup.
  useEffect(() => {
    let polling = false, disposed = false;
    const timer = setInterval(async () => {
      if (polling || !active(stateRef.current?.job)) return;
      polling = true;
      try {
        const update = await call<any>("job.status");
        if (!disposed && update.job) {
          setState(s => s ? {...s, ...update} : s);
          if (!active(update.job)) await refresh();
        }
      } catch (e) { if (!disposed) notifyError(e); }
      finally { polling = false; }
    }, 2000);
    return () => { disposed = true; clearInterval(timer); };
  }, [root]);
  useEffect(() => {
    const j = state?.job;
    if (!j || active(j) || seenJob.current === j.id) return;
    seenJob.current = j.id;
    if (j.status === "failed") {
      setError(j.message);
      return;
    }
    if (j.result?.preview_id) {
      const op = j.result.operation;
      const target = ["dates", "numbers", "keys", "lyrics"].includes(op)
        ? "correct"
        : ["organise", "strays", "duplicates", "singles"].includes(op)
          ? "organise"
          : op;
      if (root === j.result.root && route === target)
        setPreview(j.result.preview_id);
      if (["review_consolidation", "deep_preview"].includes(j.kind))
        call("preview", { id: j.result.preview_id })
          .then(setReview)
          .catch(notifyError);
      else if (j.kind === "deep_review")
        call("preview", { id: j.result.preview_id })
          .then(setDeep)
          .catch(notifyError);
      else if (root === j.result.root && route === target) setFilter("affected");
    }
    if (["apply", "deep_apply", "consolidate"].includes(j.kind)) {
      setPreview(undefined);
      setSelected(new Set());
    }
    if (j.kind === "manual_candidate" && selected.size)
      loadDetail({ id: [...selected][0] });
    setToast(
      j.status === "cancelled"
        ? "Operation cancelled. Completed changes are saved."
        : j.result?.message ||
            j.result?.summary ||
            (typeof j.result === "string" ? j.result : "Operation complete"),
    );
    if (
      [
        "connections",
        "connect_account",
        "connect_download",
        "component_update",
        "component_rollback",
      ].includes(j.kind)
    )
      call("settings").then(setSettings);
  }, [state?.job]);
  async function run(kind: string, args: any = {}) {
    setError("");
    setSubmitting(true);
    try {
      const j = await call<Job>("job.start", { kind, args: { root, ...args } });
      setState((s) => (s ? { ...s, job: j } : s));
      if (kind === "preview") setSelected(new Set());
      if (!active(j)) await refresh();
    } catch (e) {
      notifyError(e);
    } finally {
      setSubmitting(false);
    }
  }
  async function mutate(method: string, args: any = {}) {
    setSubmitting(true);
    setError("");
    try {
      const result = await call(method, args);
      await refresh();
      return result;
    } catch (e) {
      notifyError(e);
      return undefined;
    } finally {
      setSubmitting(false);
    }
  }
  async function loadDetail(row: Row) {
    setMenu(null);
    try {
      if (tree) {
        setDetail({
          release: await call("detail", { release_id: row.parent || row.id }),
          track: row.parent ? row : null,
        });
      } else if (route === "artists" || route === "favourites") {
        const d = await call("detail", { artist: row.artist });
        setManual((d.ids || []).join(","));
        setDetail(d);
      } else if (route === "local" || route === "online") {
        setDetail({operation: row});
      } else if (row.path || route === "links") {
        setDetail(await call("detail", { root, path: row.path || row.id }));
      }
    } catch (e) {
      notifyError(e);
    }
  }
  function scope() {
    return selected.size ? { ids: [...selected] } : {};
  }
  async function addLibrary() {
    try {
      const path = await open({
        directory: true,
        multiple: false,
        title: "Choose music library",
      });
      if (typeof path === "string") {
        const result = await mutate("library.add", { path });
        if (result) {
          setRoot(result.root);
          await run("scan", { root: result.root });
        }
      }
    } catch (e) {
      notifyError(e);
    }
  }
  async function selectTree(next: Selection) {
    setSelection(next);
    if (route === "queue") await mutate("queue.select", { selection: next });
  }
  async function prepareApply() {
    if (!preview || !selected.size) return;
    try {
      const p = await call("preview", { id: preview });
      setReview({
        ...p,
        rows: p.rows.filter((r: Row) => selected.has(r.id)),
        selected: [...selected],
      });
    } catch (e) {
      notifyError(e);
    }
  }
  async function saveSettings(section: string, values: any) {
    const result = await mutate("settings.save", { section, values });
    if (result) setSettings(result);
    return result;
  }
  async function updateSetting(section: string, key: string, value: any) {
    const currentSection = settings?.[section] || {};
    const nextSection = { ...currentSection, [key]: value };
    setSettings((prev: any) => (prev ? { ...prev, [section]: nextSection } : prev));

    const backendSection =
      section === "general"
        ? "desktop"
        : section === "links"
        ? "release_links"
        : section;

    await saveSettings(backendSection, nextSection);
  }
  async function exportQueue() {
    try {
      const { text } = await call("queue.export", { format: "json" });
      const path = await save({
        defaultPath: "acquisition-queue.json",
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (path)
        await invoke("save_export", {
          path,
          content:
            typeof text === "string" ? text : JSON.stringify(text, null, 2),
        });
    } catch (e) {
      notifyError(e);
    }
  }
  const columns: Column[] = tree
    ? releaseColumns
    : route === "artists"
      ? [
          { key: "artist", label: "Album artist" },
          { key: "tracks", label: "Tracks" },
          { key: "release", label: "Releases" },
          { key: "status", label: "Match status" },
          { key: "online_id", label: "Online IDs" },
          { key: "evidence", label: "Evidence" },
        ]
      : route === "favourites"
        ? [
            { key: "artist", label: "Artist" },
            { key: "status", label: "Library status" },
            { key: "tracks", label: "Local tracks" },
            { key: "online_id", label: "Online IDs" },
          ]
        : ["local", "online"].includes(route)
          ? [
              { key: "artist", label: "Artist" },
              { key: "release", label: route === "local" ? "Release to keep" : "Local release" },
              { key: "target", label: "Replacement" },
              { key: "tracks", label: "Tracks" },
              { key: "duplicates", label: "Duplicates" },
              { key: "gained", label: "Tracks gained" },
              { key: "evidence", label: "Evidence" },
            ]
          : route === "mqa"
            ? [
                { key: "artist", label: "Artist" },
                { key: "release", label: "Release" },
                { key: "title", label: "File" },
                { key: "status", label: "Status" },
                { key: "evidence", label: "Evidence" },
                { key: "target", label: "Action" },
              ]
            : ["correct", "organise", "metadata", "artwork"].includes(route)
              ? [
                  ...fileColumns.slice(0, 3),
                  { key: "changes", label: "Proposed tag changes" },
                  { key: "target", label: "Destination" },
                  { key: "evidence", label: "Evidence" },
                ]
              : fileColumns;
  function card(
    title: string,
    value: any,
    note: string,
    target: string,
    Icon: any = Music2,
  ) {
    return (
      <button className="metric" onClick={() => setRoute(target)}>
        <span className="metric-title">
          <Icon size={18} />
          {title}
          <ChevronRight size={14} />
        </span>
        <strong>{value ?? "—"}</strong>
        <small>{note}</small>
      </button>
    );
  }
  function dashboard() {
    const s = state?.stats || {};
    const group = groups.find((g) => g.id === route);
    return (
      <>
        <div className="metrics">
          {route === "overview" ? (
            <>
              {card(
                "Local tracks",
                `${(s.linked_tracks || 0).toLocaleString()} / ${(s.track_count || 0).toLocaleString()}`,
                "Linked tracks / indexed locally",
                "files",
              )}
              {card(
                "Linked releases",
                `${s.linked_releases || 0} / ${s.release_count || 0}`,
                "Whole-release associations",
                "links",
                Link,
              )}
              {card(
                "Artists to resolve",
                s.unresolved_artists,
                "Album artists requiring a match",
                "artists",
                Music2,
              )}
              {card(
                "Download queue",
                s.queued,
                "Releases waiting for approval",
                "queue",
                ArrowDownToLine,
              )}
            </>
          ) : (
            group?.items.map(([id, name, Icon]) =>
              card(
                name,
                id === "links"
                  ? `${s.linked_tracks || 0} tracks linked`
                  : id === "artists"
                    ? `${s.unresolved_artists || 0} unresolved`
                    : id === "queue"
                      ? `${s.queued || 0} releases`
                      : id === "general"
                        ? `${state?.roots.length || 0} libraries`
                        : id === "connections"
                          ? state?.diagnostics?.metrics &&
                            Object.values(state.diagnostics.metrics).every(
                              (m: any) => m.ok,
                            )
                            ? "Connected"
                            : "Check connections"
                          : [
                                "correct",
                                "organise",
                                "metadata",
                                "artwork",
                                "mqa",
                                "local",
                                "online",
                              ].includes(id)
                            ? `${s[id] ?? "—"} ${["local", "online"].includes(id) ? "opportunities" : "files to review"}`
                            : id === "downloaded"
                              ? `${s.downloaded || 0} releases`
                              : id === "downloads"
                                ? "Lossless audio"
                                : "Open",
                descriptions[id] || "Review and manage",
                id,
                Icon,
              ),
            )
          )}
        </div>
        <section className="card">
          <div className="section-heading">
            <div>
              <h2>Your libraries</h2>
              <p>Indexed once, updated when files change.</p>
            </div>
            <button disabled={busy} onClick={addLibrary}>
              <Folder size={16} />
              Add library
            </button>
          </div>
          <div className="overview-list" role="region" aria-label="Your libraries" tabIndex={0}>
          {state?.roots.length ? (
            state.roots.map((r) => (
              <div className="library-row" key={r.root}>
                <Folder size={20} />
                <div>
                  <strong>{r.root.split("/").pop()}</strong>
                  <small>{r.root}</small>
                </div>
                <span>
                  {r.tracks.toLocaleString()} tracks ·{" "}
                  {r.linked.toLocaleString()} linked
                </span>
                <button
                  onClick={() => {
                    setRoot(r.root);
                    setRoute("files");
                  }}
                >
                  Open
                </button>
                <button
                  disabled={busy}
                  onClick={() => run("scan", { root: r.root })}
                >
                  Update
                </button>
              </div>
            ))
          ) : (
            <p className="empty-note">
              Add your music folder to begin. Scanning never changes music
              files.
            </p>
          )}
          </div>
        </section>
        {route === "overview" && (
          <section className="card">
            <div className="section-heading">
              <h2>Latest missing releases</h2>
              <button onClick={() => { setTimeline("All missing releases"); setSort("date"); setDirection("desc"); setRoute("missing"); }}>
                View missing releases
              </button>
            </div>
            <div className="overview-list" role="region" aria-label="Latest missing releases" tabIndex={0}>
            {latestMissing?.length ? (
              latestMissing.map((r) => (
                <div className="library-row" key={r.id}>
                  <Music2 size={18} />
                  <strong>
                    {r.artist} — {r.release}
                  </strong>
                  <span>{r.date} · {r.recommendation}</span>
                  <button
                    onClick={() =>
                      external(`https://tidal.com/album/${r.id}`).catch(
                        notifyError,
                      )
                    }
                  >
                    Open on web
                  </button>
                </div>
              ))
            ) : (
              <p>{latestMissing === null ? "Loading cached missing releases…" : "No missing releases in the cached catalogue. Scan for new releases to update it."}</p>
            )}
            </div>
          </section>
        )}
        {route === "overview" && (
          <div className="workflow-guide">
            <h2>A clear path to a complete library</h2>
            <div>
              {groups.slice(0, 4).map((g, i) => (
                <button key={g.id} onClick={() => setRoute(g.id)}>
                  <span>{i + 1}</span>
                  <strong>{g.name}</strong>
                  <ChevronRight size={17} />
                </button>
              ))}
            </div>
          </div>
        )}
      </>
    );
  }
  function toolbar() {
    return (
      <div className="toolbar">
        {[
          "files",
          "correct",
          "organise",
          "metadata",
          "artwork",
          "links",
        ].includes(route) && (
          <button disabled={busy || !root} onClick={() => run("scan")}>
            <RefreshCw size={15} />
            Update local index
          </button>
        )}
        {["correct", "organise"].includes(route) && (
          <>
            <button
              className="primary"
              disabled={busy || !root}
              onClick={() => run("preview", { action })}
            >
              Preview {action === "organise" ? "moves" : "changes"}
            </button>
            <button
              disabled={busy || !preview || !selected.size}
              onClick={prepareApply}
            >
              Review & apply ({selected.size})
            </button>
          </>
        )}
        {route === "links" && (
          <>
            <button
              className="primary"
              disabled={busy || !root}
              onClick={() => run("link", scope())}
            >
              {selected.size
                ? `Recheck ${selected.size} selected`
                : "Link unresolved tracks"}
            </button>
            <button
              disabled={busy || !root}
              onClick={() => run("link", { editions_only: true })}
            >
              Recheck edition choices
            </button>
            <button
              disabled={!selected.size}
              onClick={() => loadDetail({ id: [...selected][0] })}
            >
              Choose match
            </button>
            <button
              disabled={busy || !selected.size}
              onClick={() => run("deep_review", scope())}
            >
              Extended review
            </button>
          </>
        )}
        {route === "artists" && (
          <>
            <button
              className="primary"
              disabled={busy}
              onClick={() =>
                run(
                  "match_artists",
                  selected.size ? { artists: [...selected] } : {},
                )
              }
            >
              {selected.size
                ? "Match selected artists"
                : "Match unresolved artists"}
            </button>
            <button
              disabled={busy}
              onClick={() =>
                run("discography", {
                  ids: selected.size
                    ? data.rows
                        .filter((r) => selected.has(r.id))
                        .map((r) => r.online_id)
                        .filter(Boolean)
                    : undefined,
                })
              }
            >
              Refresh release list
            </button>
            <button
              disabled={busy || !selected.size}
              onClick={() =>
                run("discography", {
                  ids: data.rows
                    .filter((r) => selected.has(r.id))
                    .map((r) => r.online_id)
                    .filter(Boolean),
                  detailed: true,
                })
              }
            >
              Download track details
            </button>
          </>
        )}
        {["metadata", "artwork"].includes(route) && (
          <>
            <button
              className="primary"
              disabled={busy || !root}
              onClick={() => run(route, scope())}
            >
              {route === "metadata"
                ? "Find missing tags"
                : "Find 1280 × 1280 artwork"}
            </button>
            <button
              disabled={busy || !preview || !selected.size}
              onClick={prepareApply}
            >
              Review & apply ({selected.size})
            </button>
          </>
        )}
        {route === "mqa" && (
          <>
            <button
              className="primary"
              disabled={busy || !root}
              onClick={() => run("mqa")}
            >
              Inspect unscanned files
            </button>
            <button
              disabled={busy || !root}
              onClick={() => run("mqa", { force: true })}
            >
              Recheck all audio
            </button>
            <button
              disabled={busy || !selected.size}
              onClick={() => run("link", scope())}
            >
              Link selected for replacement
            </button>
            <button
              disabled={busy || !selected.size}
              onClick={() => run("queue_mqa", scope())}
            >
              Queue lossless replacements
            </button>
          </>
        )}
        {["local", "online"].includes(route) && (
          <>
            <button
              className="primary"
              disabled={busy || !root}
              onClick={() =>
                run("optimizations", {
                  scope: route === "local" ? "local" : "remote",
                })
              }
            >
              {route === "local" ? "Scan for duplicates" : "Find opportunities in cache"}
            </button>
            {route === "local" && data.rows.some((r: any) => r.status === "Chained duplicate") && (
              <button
                disabled={busy}
                onClick={() => {
                  const chained = data.rows
                    .filter((r: any) => r.status === "Chained duplicate")
                    .map((r: any) => r.id);
                  setSelected(new Set(chained));
                }}
              >
                Select all chained ({data.rows.filter((r: any) => r.status === "Chained duplicate").length})
              </button>
            )}
            {route === "online" && (
              <button
                disabled={busy || !root}
                onClick={() => run("check_replacements", { scope: "remote" })}
              >
                Check online candidates
              </button>
            )}
            <button
              disabled={busy || route === "online" || !selected.size}
              onClick={() =>
                run("review_consolidation", {
                  ids: [...selected],
                  scope: route === "local" ? "local" : "remote",
                })
              }
            >
              Review duplicate removal {selected.size ? `(${selected.size})` : ""}
            </button>
            {route === "online" && (
              <button
                disabled={busy || !selected.size}
                onClick={() =>
                  run("queue_replacements", { ids: [...selected] })
                }
              >
                Queue replacement releases
              </button>
            )}
          </>
        )}
        {route === "favourites" && (
          <button
            className="primary"
            disabled={busy}
            onClick={() => run("favourites")}
          >
            Refresh favourite artists
          </button>
        )}
        {route === "missing" && (
          <>
            <button
              className="primary"
              disabled={busy}
              onClick={() => run("discography")}
            >
              Scan for new releases
            </button>
            <button
              disabled={
                busy || !Object.keys(selectedReleases(selection)).length
              }
              onClick={() =>
                mutate("queue.add", { selection: selectedReleases(selection) })
              }
            >
              Queue selected ({Object.keys(selectedReleases(selection)).length})
            </button>
          </>
        )}
        {route === "queue" && (
          <>
            <button
              className="primary"
              disabled={busy || !state?.stats.approved_queue}
              onClick={() =>
                setReview({
                  operation: "download",
                  rows: data.rows.filter((r) => r.approved),
                })
              }
            >
              <ArrowDownToLine size={16} />
              Download approved
            </button>
            <button disabled={busy} onClick={exportQueue}>
              Export queue
            </button>
          </>
        )}
        {tree && (
          <button
            disabled={busy || selected.size !== 1}
            onClick={() =>
              run("release_details", { id: [...selected][0], force: true })
            }
          >
            Refresh track details
          </button>
        )}
      </div>
    );
  }
  function tablePage() {
    return (
      <>
        {["correct", "organise"].includes(route) && (
          <div className="action-grid">
            {(route === "correct" ? actions : organisationActions).map(
              ([id, name, note]) => (
                <button
                  key={id}
                  className={"action-card " + (action === id ? "chosen" : "")}
                  disabled={busy}
                  onClick={() => {
                    setAction(id);
                    setPreview(undefined);
                    setSelected(new Set());
                    setFilter("affected");
                  }}
                >
                  <Tags size={18} />
                  <strong>{name}</strong>
                  <small>{note}</small>
                  {action === id && <Check size={16} />}
                </button>
              ),
            )}
          </div>
        )}
        {toolbar()}
        {route === "missing" && (
          <div className="filters secondary">
            <select
              aria-label="Release timeline"
              value={timeline}
              onChange={(e) => {
                setTimeline(e.target.value);
                setOffset(0);
              }}
            >
              {[
                "Newer than newest owned",
                "Between newest two owned",
                "All missing releases",
                "Incomplete albums",
                "All releases",
              ].map((v) => (
                <option key={v}>{v}</option>
              ))}
            </select>
            <select
              aria-label="Recommendation"
              value={recommendation}
              onChange={(e) => setRecommendation(e.target.value)}
            >
              {[
                "All recommendations",
                "Recommended",
                "Potential",
                "Suspect / Low match",
                "Unmatched",
              ].map((v) => (
                <option key={v}>{v}</option>
              ))}
            </select>
            <select
              aria-label="Copyright match"
              value={copyright}
              onChange={(e) => setCopyright(e.target.value)}
            >
              {[
                "All copyrights",
                "Matching local copyrights",
                "No copyright match",
              ].map((v) => (
                <option key={v}>{v}</option>
              ))}
            </select>
            <select
              aria-label="Release type"
              value={releaseType}
              onChange={(e) => setReleaseType(e.target.value)}
            >
              {["All types", "ALBUM", "EP", "SINGLE"].map((v) => (
                <option key={v}>{v}</option>
              ))}
            </select>
          </div>
        )}
        <div className="filters">
          <label className="search">
            <Search size={16} />
            <input
              aria-label="Filter table"
              placeholder="Find an artist, release or track…"
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setOffset(0);
              }}
            />
          </label>
          <select
            aria-label="Table filter"
            value={filter}
            onChange={(e) => {
              setFilter(e.target.value);
              setOffset(0);
            }}
          >
            <option value="all">
              {route === "artists" ? "All artists" : "All items"}
            </option>
            {["correct", "organise", "metadata", "artwork", "mqa"].includes(
              route,
            ) && <option value="affected">Affected files only</option>}
            {route === "links" && (
              <>
                <option value="unlinked">Unlinked tracks</option>
                <option value="choice">Needs an edition choice</option>
                <option value="linked">Linked tracks</option>
                <option value="ignored">Ignored tracks</option>
              </>
            )}
            {route === "missing" &&
              [
                "Missing release",
                "Owned partial",
                "Owned complete",
                "Queued",
                "Ignored",
                "Unavailable",
              ].map((v) => <option key={v}>{v}</option>)}
            {route === "artists" && (
              <>
                <option value="unresolved">Unresolved artists</option>
                <option value="matched">Matched artists</option>
                <option value="review">Needs review</option>
              </>
            )}
            {route === "favourites" &&
              ["Missing locally", "In library", "Local only"].map((v) => (
                <option key={v}>{v}</option>
              ))}
          </select>
          <span>
            {selected.size
              ? `${selected.size} selected`
              : `${data.total.toLocaleString()} items`}
          </span>
        </div>
        {route === "local" && data.rows.some((r: any) => r.status === "Chained duplicate") && (
          <div className="cluster-callout">
            <div className="cluster-callout-text">
              <Layers size={16} />
              <span>
                <strong>Chained duplicates detected:</strong> Multiple smaller releases are completely absorbed into one comprehensive master album (e.g. Single → EP → Album). You can select all and safely trash them in a single clean operation.
              </span>
            </div>
            <button
              onClick={() => {
                const chained = data.rows
                  .filter((r: any) => r.status === "Chained duplicate")
                  .map((r: any) => r.id);
                setSelected(new Set(chained));
              }}
            >
              Select all chained ({data.rows.filter((r: any) => r.status === "Chained duplicate").length})
            </button>
          </div>
        )}
        <DataTable
          rows={data.rows}
          columns={columns}
          selected={selected}
          onSelect={setSelected}
          sort={sort}
          direction={direction}
          onSort={(key) => {
            setSort(key);
            setDirection(sort === key && direction === "asc" ? "desc" : "asc");
          }}
          tree={tree}
          grouped={route === "local"}
          treeSelection={selection}
          onTreeSelect={selectTree}
          expanded={expanded}
          setExpanded={setExpanded}
          onExpand={(r) => {
            if (!busy) run("release_details", { id: r.id });
            else
              setToast(
                "Track details can be loaded when the current operation finishes.",
              );
          }}
          onDetail={loadDetail}
          onMenu={(row, x, y) => setMenu({ row, x, y })}
          loading={loading}
          busy={busy}
        />
        <footer className="table-footer">
          <span>
            {data.total
              ? `${offset + 1}–${Math.min(offset + 100, data.total)} of ${data.total.toLocaleString()}`
              : "No items"}
            {route === "links" &&
              ` · ${state?.stats.linked_releases || 0} complete releases / ${state?.stats.linked_tracks || 0} linked tracks`}
          </span>
          <div>
            <button
              disabled={offset === 0}
              onClick={() => setOffset(Math.max(0, offset - 100))}
            >
              Previous
            </button>
            <button
              disabled={offset + 100 >= data.total}
              onClick={() => setOffset(offset + 100)}
            >
              Next
            </button>
          </div>
        </footer>
      </>
    );
  }
  function field(
    label: string,
    section: string,
    key: string,
    options?: (string | { value: string | number; label: string })[],
    number = false,
  ) {
    let curVal = settings[section]?.[key] ?? "";
    if (key === "quality" && typeof curVal === "string") {
      const lower = curVal.toLowerCase();
      if (
        lower.includes("hi_res") ||
        lower.includes("24") ||
        lower.includes("192") ||
        lower.includes("hires")
      ) {
        curVal = "HI_RES_LOSSLESS";
      } else if (lower.includes("low") || lower.includes("96")) {
        curVal = "LOW";
      } else if (
        lower.includes("high") ||
        lower.includes("320") ||
        lower.includes("mp3") ||
        lower.includes("aac")
      ) {
        curVal = "HIGH";
      } else {
        curVal = "LOSSLESS";
      }
    }

    return (
      <label className="setting-row">
        <span>{label}</span>
        {options ? (
          <select
            value={String(curVal)}
            onChange={(e) => {
              const val = number ? Number(e.target.value) : e.target.value;
              if (key === "theme") {
                document.documentElement.dataset.theme = String(val);
              }
              updateSetting(section, key, val);
            }}
          >
            {options.map((opt) => {
              const val = typeof opt === "string" ? opt : String(opt.value);
              const lab =
                typeof opt === "string"
                  ? key === "theme"
                    ? opt === "system"
                      ? "System"
                      : opt[0].toUpperCase() + opt.slice(1)
                    : opt
                  : opt.label;
              return (
                <option key={val} value={val}>
                  {lab}
                </option>
              );
            })}
          </select>
        ) : (
          <input
            type={number ? "number" : "text"}
            value={settings[section]?.[key] ?? ""}
            onChange={(e) =>
              setSettings({
                ...settings,
                [section]: {
                  ...settings[section],
                  [key]: number
                    ? e.target.value === ""
                      ? 0
                      : Number(e.target.value)
                    : e.target.value,
                },
              })
            }
            onBlur={(e) => {
              const val = number
                ? e.target.value === ""
                  ? 0
                  : Number(e.target.value)
                : e.target.value;
              updateSetting(section, key, val);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.currentTarget.blur();
              }
            }}
          />
        )}
      </label>
    );
  }
  function toggle(
    label: string,
    section: string,
    key: string,
    hint?: string,
  ) {
    return (
      <label className="setting-row">
        <span>{label}</span>
        <input
          type="checkbox"
          checked={Boolean(settings[section]?.[key])}
          onChange={(e) => {
            updateSetting(section, key, e.target.checked);
          }}
        />
        {hint && <span className="hint">{hint}</span>}
      </label>
    );
  }
  function settingsPage() {
    if (!settings) return <div className="card">Loading settings…</div>;
    if (route === "general")
      return (
        <>
          <section className="card">
            <h2>Appearance & catalogue</h2>
            {field("Colour theme", "general", "theme", [
              "system",
              "light",
              "dark",
            ])}
            {field("Catalogue market", "general", "market")}
            <label className="setting-row">
              <span>Cache and persist activity logs</span>
              <input
                type="checkbox"
                checked={settings.general?.persist_logs !== false}
                onChange={(e) =>
                  updateSetting("general", "persist_logs", e.target.checked)
                }
              />
            </label>
            <p>
              Retain historical activity and error logs in database across app restarts.
            </p>
          </section>
          <section className="card">
            <h2>Artist matching</h2>
            <label className="setting-row">
              <span>Auto-accept verified release matches</span>
              <input
                type="checkbox"
                checked={Boolean(settings.matching?.enabled)}
                onChange={(e) =>
                  updateSetting("matching", "enabled", e.target.checked)
                }
              />
            </label>
            <p>
              Artists need matching local release evidence. Name-only candidates
              remain available for review.
            </p>
            {field(
              "Release cache age (days, 0 = no expiry)",
              "links",
              "max_age_days",
              undefined,
              true,
            )}
          </section>
          <section className="card">
            <div className="section-heading">
              <h2>Libraries</h2>
              <button disabled={busy} onClick={addLibrary}>
                Add library
              </button>
            </div>
            {state?.roots.map((r) => (
              <div className="library-row" key={r.root}>
                <div>
                  <strong>{r.root}</strong>
                  <small>
                    {r.linked} / {r.tracks} tracks linked
                  </small>
                </div>
                <button
                  disabled={busy}
                  onClick={() => run("scan", { root: r.root, force: true })}
                >
                  Recheck all tags
                </button>
                <button
                  disabled={busy}
                  onClick={() =>
                    setReview({
                      operation: "remove-root",
                      root: r.root,
                      rows: [],
                    })
                  }
                >
                  Remove from index
                </button>
              </div>
            ))}
          </section>
          <section className="card about-card">
            <h2>About Tibrary</h2>
            <div className="about-details">
              <div className="about-field">
                <span className="about-label">Version</span>
                <span className="about-val">v0.9.0-beta.1 (pre-1.0)</span>
              </div>
              <div className="about-field">
                <span className="about-label">Architecture</span>
                <span className="about-val">Native Rust + Tauri v2 Core</span>
              </div>
              <div className="about-field">
                <span className="about-label">Database</span>
                <span className="about-val">Turso / libsql Embedded SQLite</span>
              </div>
              <div className="about-field">
                <span className="about-label">Audio Engine</span>
                <span className="about-val">Native AES-CBC / MPEG-DASH / Lofty / Claxon (Pure Rust)</span>
              </div>
            </div>
            <p className="about-description">
              Tibrary is a local-first music library manager and lossless acquisition companion designed for precision audio workflows, automated discography syncing, and duplicate consolidation.
            </p>
          </section>
        </>
      );
    if (route === "connections")
      return (
        <>
          <div className="metrics">
            {Object.entries(state?.diagnostics?.metrics || {}).map(
              ([key, m]: [string, any]) => (
                <div className="metric static" key={key}>
                  <span className="metric-title">
                    {key === "download" ? "Download & metadata account" : key}
                  </span>
                  <strong>{m.ok ? "Connected" : "Needs attention"}</strong>
                  <small>
                    {m.latency_ms ? `${m.latency_ms} ms · ` : ""}
                    {m.message}
                  </small>
                </div>
              ),
            )}
          </div>
          <section className="card">
            <h2>Online account</h2>
            <p>
              Sign in once for collection access. The download account also
              supplies extended BPM and key metadata.
            </p>
            <div className="toolbar">
              <button
                className="primary"
                disabled={busy}
                onClick={() => run("connect_account")}
              >
                {state?.connections.account
                  ? "Reconnect account"
                  : "Connect account"}
              </button>
              <button
                disabled={busy || !state?.connections.account}
                onClick={() => mutate("account.disconnect")}
              >
                Disconnect account
              </button>
            </div>
            <p>One account connection is shared by favourites, downloads and extended metadata. Application credentials below provide catalogue access.</p>
          </section>
          <section className="card">
            <h2>Application credentials</h2>
            <p>
              {state?.connections.configured
                ? "Credentials are configured. Forget them to enter a replacement."
                : "Enter the client credentials issued for your catalogue application. Saved credentials use macOS Keychain."}
            </p>
            <label className="setting-row">
              <span>Client ID</span>
              <input
                autoComplete="off"
                disabled={state?.connections.configured}
                value={credentials.client}
                onChange={(e) =>
                  setCredentials({ ...credentials, client: e.target.value })
                }
              />
            </label>
            <label className="setting-row">
              <span>Client secret</span>
              <input
                type="password"
                autoComplete="off"
                disabled={state?.connections.configured}
                value={credentials.secret}
                onChange={(e) =>
                  setCredentials({ ...credentials, secret: e.target.value })
                }
              />
            </label>
            <div className="toolbar">
              <button
                disabled={
                  busy ||
                  state?.connections.configured ||
                  !credentials.client ||
                  !credentials.secret
                }
                onClick={async () => {
                  if (
                    await mutate("credentials.save", {
                      ...credentials,
                      remember: true,
                    })
                  )
                    setCredentials({ client: "", secret: "" });
                }}
              >
                Save securely
              </button>
              <button
                disabled={busy || !state?.connections.configured}
                onClick={() => mutate("credentials.forget")}
              >
                Forget credentials
              </button>
            </div>
          </section>
          <button disabled={busy} onClick={() => run("connections")}>
            <RefreshCw size={16} />
            Test configured connections
          </button>
        </>
      );
    return (
      <>
        <section className="card">
          <h2>Download folder & structure</h2>
          <p>
            Music is arranged using saved tags. Disc folders appear only for
            multi-disc releases.
          </p>
          <label className="setting-row">
            <span>Download folder</span>
            <input
              value={settings.downloads?.output || ""}
              onChange={(e) =>
                setSettings({
                  ...settings,
                  downloads: { ...settings.downloads, output: e.target.value },
                })
              }
              onBlur={(e) => updateSetting("downloads", "output", e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.currentTarget.blur();
                }
              }}
            />
            <button
              onClick={async () => {
                const path = await open({ directory: true, multiple: false });
                const selectedPath = Array.isArray(path) ? path[0] : path;
                if (selectedPath && typeof selectedPath === "string") {
                  updateSetting("downloads", "output", selectedPath);
                }
              }}
            >
              Choose…
            </button>
          </label>
          {field("Folder template", "organisation", "template")}
          <details>
            <summary>Template reference & tag variables</summary>
            <div className="tokens">
              {[
                "albumartist",
                "artist",
                "album",
                "year",
                "disc",
                "discnumber",
                "disc_prefix",
                "tracknumber",
                "title",
              ].map((t) => (
                <code key={t}>{"{" + t + "}"}</code>
              ))}
            </div>
            <p>
              Default: Album artist / Release (Year) / Disc (when needed) /
              Track number - Title. Accents and hyphens are preserved; unsafe
              path punctuation is normalised.
            </p>
          </details>
        </section>
        <section className="card">
          <h2>Download engine</h2>
          {field("Audio quality", "downloads", "quality", [
            { value: "HI_RES_LOSSLESS", label: "FLAC (24/192khz)" },
            { value: "LOSSLESS", label: "FLAC (16/44.1khz)" },
            { value: "HIGH", label: "MP3 (320kbps)" },
            { value: "LOW", label: "MP3 (96kbps)" },
          ])}
          <p className="hint">
            FLAC up to 24-bit / 192 kHz (Hi-Res Lossless), FLAC 16-bit / 44.1 kHz (Lossless CD quality), or compressed AAC/MP3.
          </p>
          {field("Embedded artwork size", "downloads", "cover_size", [
            { value: "1280", label: "1280 × 1280 px" },
            { value: "640", label: "640 × 640 px" },
          ], true)}
          {toggle("Skip already downloaded files", "downloads", "skip_existing")}
          {toggle("Save companion cover.jpg to album folder", "downloads", "cover_album_file")}
          {toggle("Embed lyrics into audio files", "downloads", "lyrics_embed")}
          {toggle("Save separate .lrc lyrics file", "downloads", "lyrics_file")}
          {toggle("Create .m3u8 playlist file for albums", "downloads", "playlist_create")}
          {toggle("Write ReplayGain volume tags", "downloads", "replay_gain")}
          <p className="hint">Downloads run one track at a time to keep requests predictable and reduce throttling.</p>
          {field(
            "Minimum release pause (seconds)",
            "provider",
            "download_delay_min_sec",
            undefined,
            true,
          )}
          {field(
            "Maximum release pause (seconds)",
            "provider",
            "download_delay_max_sec",
            undefined,
            true,
          )}
          <div className="toolbar">
            <button
              disabled={busy}
              onClick={async () => {
                const v = await mutate("settings.reset", {
                  group: "downloads",
                });
                if (v) setSettings(v);
              }}
            >
              Reset download defaults
            </button>
          </div>
        </section>
      </>
    );
  }
  function contextMenu() {
    if (!menu) return null;
    const r = menu.row;
    const ids = selected.has(r.id) ? [...selected] : [r.id];
    return (
      <>
        <div className="menu-scrim" onClick={() => setMenu(null)} />
        <div
          role="menu"
          className="context-menu"
          style={{
            left: Math.min(menu.x, window.innerWidth - 250),
            top: Math.min(menu.y, window.innerHeight - 320),
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") setMenu(null);
            const buttons = [...e.currentTarget.querySelectorAll("button")];
            const i = buttons.indexOf(
              document.activeElement as HTMLButtonElement,
            );
            if (e.key === "ArrowDown") {
              e.preventDefault();
              buttons[(i + 1) % buttons.length]?.focus();
            }
            if (e.key === "ArrowUp") {
              e.preventDefault();
              buttons[(i + buttons.length - 1) % buttons.length]?.focus();
            }
          }}
        >
          <button role="menuitem" onClick={() => loadDetail(r)}>
            View metadata / match details
          </button>
          {r.path && (
            <>
              <button
                role="menuitem"
                onClick={() => {
                  reveal(r.path).catch(notifyError);
                  setMenu(null);
                }}
              >
                Show in Finder
              </button>
              <button
                role="menuitem"
                onClick={() => {
                  navigator.clipboard.writeText(r.path).catch(notifyError);
                  setMenu(null);
                }}
              >
                Copy path
              </button>
            </>
          )}
          {r.online_id && String(r.online_id).match(/^\d+$/) && (
            <button
              role="menuitem"
              onClick={() => {
                external(
                  `https://tidal.com/${route === "artists" || route === "favourites" ? "artist" : "album"}/${r.online_id}`,
                ).catch(notifyError);
                setMenu(null);
              }}
            >
              Open on web
            </button>
          )}
          {[
            "links",
            "files",
            "correct",
            "organise",
            "metadata",
            "artwork",
            "mqa",
          ].includes(route) && (
            <>
              <hr />
              <button
                role="menuitem"
                disabled={busy}
                onClick={() => {
                  run("link", { ids });
                  setMenu(null);
                }}
              >
                Recheck {ids.length} selected tracks
              </button>
              <button
                role="menuitem"
                disabled={busy}
                onClick={() => {
                  mutate("tracks.ignore", { root, ids, ignored: !r.ignored });
                  setMenu(null);
                }}
              >
                {r.ignored ? "Restore" : "Ignore"} {ids.length} selected tracks
              </button>
              <button
                role="menuitem"
                disabled={busy}
                onClick={() => {
                  mutate("tracks.unlink", { root, ids });
                  setMenu(null);
                }}
              >
                Unlink {ids.length} selected tracks
              </button>
            </>
          )}
          {["queue", "downloaded", "missing"].includes(route) && (
            <>
              <hr />
              <button
                role="menuitem"
                disabled={busy}
                onClick={() => {
                  if (route === "missing")
                    mutate("queue.add", {
                      selection: Object.fromEntries(
                        ids.map((id) => [id, null]),
                      ),
                    });
                  else mutate("queue.decision", { ids, decision: "queued" });
                  setMenu(null);
                }}
              >
                Queue whole release
              </button>
              <button
                role="menuitem"
                disabled={busy}
                onClick={() => {
                  mutate("queue.decision", { ids, decision: "ignored" });
                  setMenu(null);
                }}
              >
                Ignore selected releases
              </button>
              <button
                role="menuitem"
                disabled={busy}
                onClick={() => {
                  mutate("queue.decision", { ids, decision: "removed" });
                  setMenu(null);
                }}
              >
                Remove from queue
              </button>
            </>
          )}
        </div>
      </>
    );
  }
  async function confirmReview() {
    const r = review;
    setReview(null);
    if (r.operation === "download") {
      await run("download");
      return;
    }
    if (r.operation === "remove-root") {
      await mutate("library.remove", { root: r.root });
      return;
    }
    if (r.operation === "component_update") {
      await run("component_update");
      return;
    }
    if (r.operation === "consolidate") {
      await run("consolidate", {
        preview_id: r.id,
        ids: r.rows.map((x: Row) => x.id),
        scope: r.scope,
        root: r.root,
        confirmed: true,
      });
      return;
    }
    if (r.operation === "deep_apply") {
      await run("deep_apply", {
        preview_id: r.id,
        root: r.root,
        confirmed: true,
      });
      return;
    }
    await run("apply", {
      preview_id: r.id,
      ids: r.selected,
      confirmed: true,
      root: r.root,
    });
  }
  return (
    <div className="app">
      <div className="drag-region" data-tauri-drag-region aria-hidden="true" />
      <aside>
        <button
          className={"nav-item " + (route === "overview" ? "current" : "")}
          onClick={() => setRoute("overview")}
        >
          <House size={18} />
          Overview
        </button>
        {groups.map((g) => (
          <div className="nav-group" key={g.id}>
            <div className="nav-heading">
              <button
                aria-label={`${collapsed.has(g.id) ? "Expand" : "Collapse"} ${g.name}`}
                onClick={() => {
                  const n = new Set(collapsed);
                  n.has(g.id) ? n.delete(g.id) : n.add(g.id);
                  setCollapsed(n);
                }}
              >
                {collapsed.has(g.id) ? (
                  <ChevronRight size={13} />
                ) : (
                  <ChevronDown size={13} />
                )}
              </button>
              <button
                className={route === g.id ? "current" : ""}
                onClick={() => setRoute(g.id)}
              >
                <g.icon size={17} />
                {g.name}
              </button>
            </div>
            {!collapsed.has(g.id) &&
              g.items.map(([id, name, Icon]) => (
                <button
                  key={id}
                  className={"nav-item sub " + (route === id ? "current" : "")}
                  onClick={() => setRoute(id)}
                >
                  <Icon size={16} />
                  {name}
                  {id === "activity" && active(state?.job) && (
                    <span className="live-dot" />
                  )}
                </button>
              ))}
          </div>
        ))}
        <div className="sidebar-bottom">
          <span className="sidebar-version">v0.9.0-beta.1</span>
        </div>
      </aside>
      <main>
        <header className="page-header">
          <div>
            <h1>{titles[route] || "Overview"}</h1>
            <p>{descriptions[route] || "Choose a step to continue."}</p>
          </div>
          <div className="header-actions">
            <select
              aria-label="Active library"
              value={root}
              onChange={(e) => setRoot(e.target.value)}
            >
              <option value="">{!state ? "Loading libraries…" : state.roots.length ? "Choose library" : "Add a library to begin"}</option>
              {state?.roots.map((r) => (
                <option key={r.root} value={r.root}>
                  {r.root.split("/").pop()}
                </option>
              ))}
            </select>
            {route !== "activity" && (
              <button
                className={
                  active(state?.job) ? "activity-pill running" : "activity-pill"
                }
                onClick={() => setRoute("activity")}
              >
                {active(state?.job) ? (
                  <LoaderCircle className="spin" size={16} />
                ) : (
                  <Activity size={16} />
                )}{" "}
                {active(state?.job)
                  ? state?.job?.status === "cancelling"
                    ? "Cancelling…"
                    : "Working"
                  : "Activity"}
              </button>
            )}
          </div>
        </header>
        {error && (
          <div className="notice error" role="alert">
            <span>{error}</span>
            <button aria-label="Dismiss error" onClick={() => setError("")}>
              <X size={16} />
            </button>
          </div>
        )}
        {toast && (
          <div className="notice" role="status">
            <span>{toast}</span>
            <button aria-label="Dismiss notice" onClick={() => setToast("")}>
              <X size={16} />
            </button>
          </div>
        )}
        <div
          className={
            "page-body " +
            ([
              "overview",
              ...groups.map((g) => g.id),
              "general",
              "connections",
              "downloads",
              "activity",
            ].includes(route)
              ? "scroll-page"
              : "table-page")
          }
        >
          {["overview", ...groups.map((g) => g.id)].includes(route) ? (
            dashboard()
          ) : ["general", "connections", "downloads"].includes(route) ? (
            settingsPage()
          ) : route === "activity" ? (
            <>
              {(() => {
                const job = state?.job;
                const rawStatus = (job?.status || "ready").toLowerCase();
                const rawKind = (job?.kind || "").toLowerCase();

                let statusLabel = "READY";
                let statusClass = "ready";
                if (rawStatus === "failed" || rawStatus === "error") {
                  statusLabel = "FAILED";
                  statusClass = "failed";
                } else if (rawStatus === "running" || rawStatus === "in_progress") {
                  statusLabel = "RUNNING";
                  statusClass = "running";
                } else if (rawStatus === "complete" || rawStatus === "completed" || rawStatus === "done") {
                  statusLabel = "COMPLETED";
                  statusClass = "complete";
                } else if (rawStatus === "cancelling") {
                  statusLabel = "CANCELLING";
                  statusClass = "cancelling";
                } else if (rawStatus === "cancelled") {
                  statusLabel = "CANCELLED";
                  statusClass = "cancelled";
                }

                let cat = "general";
                if (rawKind === "download" || rawKind.includes("download")) cat = "download";
                else if (rawKind === "scan" || rawKind.includes("scan")) cat = "scan";
                else if (rawKind === "link" || rawKind.includes("link") || rawKind.includes("catalogue") || rawKind.includes("artist")) cat = "linking";
                else if (rawKind.includes("duplicate") || rawKind.includes("consolidation") || rawKind.includes("trash") || rawKind.includes("organise") || rawKind.includes("correct")) cat = "cleanup";
                else if (rawStatus === "failed" || rawStatus === "error") cat = "error";

                const kindTitles: Record<string, string> = {
                  download: "Lossless Audio Download",
                  scan: "Library Scan",
                  link: "Catalogue Linker",
                  mqa: "MQA Audio Inspection",
                  queue_mqa: "Queue Lossless Replacements",
                  duplicates: "Duplicate Scanner",
                  review_consolidation: "Duplicate Consolidation",
                  apply_consolidation: "Trash Duplicate Files",
                  metadata: "Metadata Tagging",
                  artwork: "Artwork Fetcher",
                  correct: "Tag Correction",
                  organise: "Folder Organization",
                  favourites: "Refresh Favourite Artists",
                  startup: "System Ready",
                };
                const displayTitle = kindTitles[rawKind] || (rawKind ? rawKind.split("_").map((w: string) => w.charAt(0).toUpperCase() + w.slice(1)).join(" ") : "No Active Operation");

                return (
                  <section className="card activity-status">
                    <div className="activity-status-info">
                      <div className="activity-status-badges">
                        <span className={`status-badge status-${statusClass}`}>
                          {statusLabel}
                        </span>
                        {rawKind && rawKind !== "startup" && (
                          <span className={`log-badge log-badge-${cat}`}>
                            {cat.toUpperCase()}
                          </span>
                        )}
                      </div>
                      <h2>{displayTitle}</h2>
                      <p>
                        {job?.message ||
                          "Operations will appear here. You can keep browsing while they run."}
                      </p>
                    </div>
                    <button
                      disabled={
                        !active(state?.job) || state?.job?.status === "cancelling"
                      }
                      onClick={() => call("job.cancel").catch(notifyError)}
                    >
                      Cancel
                    </button>
                  </section>
                );
              })()}
              <div className="activity-controls">
                <div className="activity-filters">
                  <select
                    aria-label="Filter activity category"
                    value={logCategory}
                    onChange={(e) => setLogCategory(e.target.value)}
                  >
                    <option value="all">All</option>
                    <option value="download">DOWNLOAD</option>
                    <option value="scan">SCAN</option>
                    <option value="linking">LINKING</option>
                    <option value="cleanup">CLEANUP</option>
                    <option value="error">ERROR</option>
                    <option value="general">GENERAL</option>
                  </select>
                  <input
                    type="search"
                    placeholder="Search logs..."
                    aria-label="Search activity logs"
                    value={logSearch}
                    onChange={(e) => setLogSearch(e.target.value)}
                  />
                </div>
                <div className="activity-actions">
                  <label className="cache-toggle" title="Keep activity logs cached in database across app restarts">
                    <input
                      type="checkbox"
                      checked={settings?.general?.persist_logs !== false}
                      onChange={(e) => {
                        const enabled = e.target.checked;
                        const nextGen = { ...(settings?.general || {}), persist_logs: enabled };
                        setSettings((s: any) => s ? { ...s, general: nextGen } : s);
                        saveSettings("desktop", nextGen);
                        setToast(enabled ? "Log caching enabled" : "Log caching disabled");
                        setTimeout(() => setToast(""), 2000);
                      }}
                    />
                    <span>Cache logs</span>
                  </label>
                  <span className="log-count">
                    {(() => {
                      const allLogs = state?.logs || [];
                      const filtered = allLogs.filter((l) => {
                        const cat = l.category || getLogCategory(l);
                        if (logCategory !== "all" && cat !== logCategory) return false;
                        if (logSearch.trim()) {
                          const q = logSearch.trim().toLowerCase();
                          return l.message.toLowerCase().includes(q) || cat.toLowerCase().includes(q);
                        }
                        return true;
                      });
                      return `${filtered.length} of ${allLogs.length} entries`;
                    })()}
                  </span>
                  <button
                    className="action-btn"
                    onClick={() => {
                      const allLogs = state?.logs || [];
                      const filtered = allLogs
                        .filter((l) => {
                          const cat = l.category || getLogCategory(l);
                          if (logCategory !== "all" && cat !== logCategory) return false;
                          if (logSearch.trim()) {
                            const q = logSearch.trim().toLowerCase();
                            return l.message.toLowerCase().includes(q) || cat.toLowerCase().includes(q);
                          }
                          return true;
                        })
                        .map(
                          (l) =>
                            `[${new Date(l.at).toLocaleTimeString()}] [${(l.category || getLogCategory(l)).toUpperCase()}] ${l.message}`,
                        )
                        .join("\n");
                      navigator.clipboard.writeText(filtered);
                      setToast("Logs copied to clipboard");
                      setTimeout(() => setToast(""), 2000);
                    }}
                    title="Copy filtered logs"
                  >
                    <Copy size={14} />
                    Copy
                  </button>
                  <button
                    className="action-btn"
                    onClick={() => {
                      call("logs.clear")
                        .then(() => {
                          setState((s) => (s ? { ...s, logs: [] } : s));
                          setToast("Logs cleared");
                          setTimeout(() => setToast(""), 2000);
                        })
                        .catch(notifyError);
                    }}
                    title="Clear log history"
                  >
                    <Trash2 size={14} />
                    Clear
                  </button>
                </div>
              </div>
              <div className="activity-log">
                {(() => {
                  const allLogs = (state?.logs || []).slice().reverse();
                  const filtered = allLogs.filter((l) => {
                    const cat = l.category || getLogCategory(l);
                    if (logCategory !== "all" && cat !== logCategory) return false;
                    if (logSearch.trim()) {
                      const q = logSearch.trim().toLowerCase();
                      return l.message.toLowerCase().includes(q) || cat.toLowerCase().includes(q);
                    }
                    return true;
                  });
                  if (filtered.length === 0) {
                    return (
                      <div className="activity-empty">
                        <p>No activity log entries match your filter.</p>
                      </div>
                    );
                  }
                  return filtered.map((l, i) => {
                    const cat = l.category || getLogCategory(l);
                    return (
                      <div key={i} className={`log-row log-${cat}`}>
                        <time title={new Date(l.at).toLocaleString()}>
                          {new Date(l.at).toLocaleTimeString()}
                        </time>
                        <span className={`log-badge log-badge-${cat}`}>
                          {cat.toUpperCase()}
                        </span>
                        <span className="log-message">{l.message}</span>
                      </div>
                    );
                  });
                })()}
              </div>
            </>
          ) : (
            tablePage()
          )}
        </div>
      </main>
      {contextMenu()}
      {detail && (
        <Modal
          title={
            detail.artist && !detail.tags
              ? "Artist candidates"
              : detail.release
                ? "Release details"
                : "Recording & release match"
          }
          wide
          onClose={() => setDetail(null)}
        >
          <div className="modal-body">
            {detail.tags && (
              <>
                <div className="detail-heading">
                  <h3>{detail.tags.title?.join(", ")}</h3>
                  <p>{detail.path}</p>
                  <p>Local: {detail.local_position}</p>
                  <button
                    onClick={() => reveal(detail.path).catch(notifyError)}
                  >
                    Show in Finder
                  </button>
                </div>
                <h3>Available placements</h3>
                {detail.catalogue_options?.length ? (
                  detail.catalogue_options.map((o: any, i: number) => (
                    <article
                      className="candidate"
                      key={i}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        external(`https://tidal.com/album/${o.id}`).catch(
                          notifyError,
                        );
                      }}
                    >
                      <div>
                        <strong>
                          {o.artist} — {o.album}
                        </strong>
                        <p>
                          {o.position_label} · ID {o.id}
                        </p>
                        <p>{o.evidence}</p>
                        <p
                          className={
                            o.structure?.compatible ? "success" : "warning"
                          }
                        >
                          {o.structure?.compatible
                            ? "Structure verified"
                            : JSON.stringify(
                                o.structure?.reason ||
                                  o.structure?.reasons ||
                                  "Structural difference; review carefully",
                              )}
                        </p>
                      </div>
                      <div className="toolbar">
                        <button
                          onClick={() =>
                            external(`https://tidal.com/album/${o.id}`).catch(
                              notifyError,
                            )
                          }
                        >
                          Open on web <ArrowUpRight size={14} />
                        </button>
                        <button
                          disabled={busy}
                          onClick={async () => {
                            if (
                              await mutate("tracks.choose", {
                                root,
                                path: detail.path,
                                album_id: o.id,
                                track_id: o.track_id,
                                choice_key: o.choice_key,
                              })
                            )
                              setDetail(null);
                          }}
                        >
                          Use this placement
                        </button>
                      </div>
                    </article>
                  ))
                ) : (
                  <p>
                    No inspected candidates. Recheck this recording or search a
                    release below.
                  </p>
                )}
                <div className="toolbar">
                  <input
                    placeholder="Online release ID"
                    aria-label="Online release ID"
                    value={manual}
                    onChange={(e) => setManual(e.target.value)}
                  />
                  <button
                    disabled={busy || !/^\d+$/.test(manual)}
                    onClick={() =>
                      run("manual_candidate", {
                        ids: [detail.path],
                        album_id: manual,
                      })
                    }
                  >
                    Inspect this release
                  </button>
                  <button
                    disabled={busy}
                    onClick={() => run("link", { ids: [detail.path] })}
                  >
                    Recheck this track
                  </button>
                </div>
                <details>
                  <summary>All saved tags & DJ checks</summary>
                  <pre>
                    {JSON.stringify(
                      {
                        tags: detail.tags,
                        dj_checks: detail.dj_checks,
                        note: detail.catalogue_note,
                      },
                      null,
                      2,
                    )}
                  </pre>
                </details>
              </>
            )}
            {detail.operation && <section><h3>{detail.operation.release}</h3><p>{detail.operation.evidence}</p><dl><dt>Local folder</dt><dd>{detail.operation.path}</dd><dt>Retained / replacement destination</dt><dd>{detail.operation.target}</dd><dt>Duplicate tracks</dt><dd>{detail.operation.duplicates}</dd></dl></section>}
            {detail.artist && !detail.tags && (
              <>
                <p>
                  Choose one or more confirmed artist IDs. Existing mappings
                  remain until you save.
                </p>
                {(detail.review?.candidates || []).map((c: any, i: number) => (
                  <article className="candidate" key={i}>
                    <div>
                      <strong>{c.name || c.artist?.name}</strong>
                      <p>{c.evidence || JSON.stringify(c)}</p>
                    </div>
                    <button
                      onClick={() =>
                        external(`https://tidal.com/artist/${c.id}`).catch(
                          notifyError,
                        )
                      }
                    >
                      Open on web
                    </button>
                    <button
                      disabled={busy}
                      onClick={() =>
                        setManual((v) =>
                          [
                            ...new Set([
                              ...v.split(",").filter(Boolean),
                              String(c.id),
                            ]),
                          ].join(","),
                        )
                      }
                    >
                      Add ID
                    </button>
                  </article>
                ))}
                <label className="setting-row">
                  <span>Artist IDs (comma-separated)</span>
                  <input
                    value={manual}
                    onChange={(e) => setManual(e.target.value)}
                  />
                </label>
                <button
                  disabled={busy || !manual}
                  onClick={async () => {
                    if (
                      await mutate("artists.choose", {
                        artist: detail.artist,
                        ids: manual.split(",").map((s) => s.trim()),
                      })
                    )
                      setDetail(null);
                  }}
                >
                  Save artist identities
                </button>
              </>
            )}
            {detail.release && (
              <>
                <h3>
                  {detail.release.artist} — {detail.release.release}
                </h3>
                <button
                  onClick={() =>
                    external(
                      `https://tidal.com/${detail.track ? "track" : "album"}/${detail.track?.id || detail.release.id}`,
                    ).catch(notifyError)
                  }
                >
                  Open on web
                </button>
                <div className="review-list">
                  {(detail.track
                    ? [detail.track]
                    : detail.release.children || []
                  ).map((t: Row) => (
                    <article key={t.id}>
                      <strong>
                        {t.position} · {t.title}
                      </strong>
                      <p>
                        {t.isrc || "No recording identifier"} · BPM{" "}
                        {t.bpm || "Not supplied"} · Key{" "}
                        {t.key || "Not supplied"}
                      </p>
                      <button
                        onClick={() =>
                          external(`https://tidal.com/track/${t.id}`).catch(
                            notifyError,
                          )
                        }
                      >
                        Open track on web
                      </button>
                    </article>
                  ))}
                </div>
                {detail.release.downloaded_files?.length > 0 && (
                  <section>
                    <h3>Downloaded files</h3>
                    {detail.release.downloaded_files.map((path: string) => (
                      <div className="library-row" key={path}>
                        <small>{path}</small>
                        <button onClick={() => reveal(path).catch(notifyError)}>
                          Show in Finder
                        </button>
                      </div>
                    ))}
                  </section>
                )}
              </>
            )}
          </div>
          <footer>
            <button onClick={() => setDetail(null)}>Done</button>
          </footer>
        </Modal>
      )}
      {review && (
        <Modal
          title={
            review.operation === "consolidate"
              ? "Review duplicate removal"
              : review.operation === "download"
                ? "Download approved music"
                : review.operation === "remove-root"
                  ? "Remove library from index"
                  : "Review changes"
          }
          wide
          onClose={() => setReview(null)}
        >
          <div className="modal-body">
            <p>
              {review.operation === "consolidate"
                ? "The listed source files will move to macOS Trash only after replacement audio and metadata are verified. Review BPM/key differences below."
                : review.operation === "remove-root"
                  ? "Only the library index is removed. Music files remain on disk."
                  : review.operation === "deep_apply"
                    ? "Only database links change. Local tags and file locations remain unchanged."
                    : review.operation === "download"
                    ? "Only approved tracks are downloaded, using the configured quality and folder structure. Videos and lyrics are excluded."
                    : review.operation === "component_update"
                      ? "Build and activate updated streaming components. The previous runtime is retained for rollback."
                      : "Only the reviewed changes are applied. Existing audio is verified and preserved."}
            </p>
            <div className="review-list">
              {review.rows?.map((r: any, i: number) => (
                <article key={i} className={review.operation === "consolidate" ? "review-cluster-item" : ""}>
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: "8px" }}>
                    <strong>
                      {r.artist} — {r.release || r.title || r.id}
                    </strong>
                    {r.is_chained && (
                      <span className="badge chained-duplicate">Chained duplicate</span>
                    )}
                  </div>
                  {review.operation === "consolidate" ? (
                    <>
                      <p style={{ marginTop: "4px", color: "var(--muted)", fontSize: "12px" }}>
                        🎯 <strong>Master Keeper:</strong> {r.target}
                      </p>
                      <p style={{ color: "var(--text)", fontSize: "12px", marginTop: "2px" }}>
                        🗑️ <strong>Safe Trash:</strong> {r.changes || r.path}
                      </p>
                    </>
                  ) : (
                    <p>{readable(r.changes || r.path || r.target)}</p>
                  )}
                  {r.evidence && <small style={{ display: "block", marginTop: "4px" }}>{readable(r.evidence)}</small>}
                  {r.reviewed_dj_conflicts?.length > 0 && (
                    <pre>
                      {JSON.stringify(r.reviewed_dj_conflicts, null, 2)}
                    </pre>
                  )}
                </article>
              ))}
            </div>
          </div>
          <footer>
            <button onClick={() => setReview(null)}>Cancel</button>
            <button className="primary" disabled={busy} onClick={confirmReview}>
              {review.operation === "consolidate"
                ? review.count
                  ? `Move ${review.count} duplicate files to Trash`
                  : "Move reviewed duplicates to Trash"
                : review.operation === "download"
                  ? "Start download"
                  : "Confirm & continue"}
            </button>
          </footer>
        </Modal>
      )}
      {deep && (
        <Modal
          title="Extended release review"
          wide
          onClose={() => setDeep(null)}
        >
          <div className="modal-body">
            <p>
              Review recording matches and choose release links. Local tags and folders stay unchanged.
            </p>
            {deep.raw?.map((r: any, i: number) => (
              <article className="candidate" key={i}>
                <div>
                  <strong>
                    {r.release?.artist} — {r.release?.title}
                  </strong>
                  <p>{r.tracks_matched?.length || 0} matching recordings</p>
                </div>
                <button
                  disabled={busy}
                  onClick={() => {
                    run("deep_preview", { preview_id: deep.id, index: i });
                    setDeep(null);
                  }}
                >
                  Review links
                </button>
              </article>
            ))}
            <input
              placeholder="Release title or URL"
              value={deepQuery}
              onChange={(e) => setDeepQuery(e.target.value)}
            />
            <button
              disabled={busy}
              onClick={() =>
                run("deep_review", { ...scope(), query: deepQuery })
              }
            >
              Search releases
            </button>
          </div>
        </Modal>
      )}
      {state?.auth_url && (
        <Modal
          title="Connect download & metadata account"
          onClose={() => { void mutate("job.cancel"); }}
        >
          <div className="modal-body">
            <p>
              Open the secure sign-in page, then paste the final redirect URL
              here.
            </p>
            <button
              onClick={() => external(state.auth_url!).catch(notifyError)}
            >
              Open sign-in page
            </button>
            <label className="setting-row">
              <span>Redirect URL</span>
              <input
                type="password"
                value={auth}
                onChange={(e) => setAuth(e.target.value)}
              />
            </label>
          </div>
          <footer>
            <button onClick={() => mutate("job.cancel")}>Cancel</button>
            <button
              className="primary"
              disabled={!auth}
              onClick={() => {
                mutate("auth.reply", { response: auth });
                setAuth("");
              }}
            >
              Complete connection
            </button>
          </footer>
        </Modal>
      )}
      {(closing || stopped) && (
        <div className="blocking">
          <div className="card">
            <h2>
              {closing ? "Closing safely…" : "The background service stopped"}
            </h2>
            <p>
              {closing
                ? "Cancelling work and finishing the current safe file boundary."
                : "Restart Tibrary. Completed changes are indexed; update the library before preparing another file operation."}
            </p>
          </div>
        </div>
      )}
      <div className="edge-fade-right" aria-hidden="true" />
    </div>
  );
}
class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: string }
> {
  state = { error: "" };
  static getDerivedStateFromError(e: Error) {
    return { error: e.message };
  }
  render() {
    return this.state.error ? (
      <div className="fatal">
        <h1>The interface needs to reload</h1>
        <p>Background work stays in the Python service.</p>
        <pre>{this.state.error}</pre>
        <button onClick={() => location.reload()}>Reload interface</button>
      </div>
    ) : (
      this.props.children
    );
  }
}
createRoot(document.getElementById("root")!).render(
  <ErrorBoundary>
    <App />
  </ErrorBoundary>,
);
