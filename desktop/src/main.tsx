import { MetadataView, DownloadReview, TagChanges, ReleaseMetadata } from "./MetadataView";
import { availableColumnSelections, columnFilterAvailable } from "./columnFilters";
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
  ChevronLeft,
  ChevronRight,
  PanelLeft,
  Copy,
  Disc,
  Download,
  Folder,
  FilterX,
  Heart,
  House,
  Image,
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
  mergeJob,
  onlineKinds,
} from "./api";
import { DataTable, Column, HeaderFilter, ColumnSelection, ColumnFilterOptions } from "./DataTable";
import { ActivityView, streamFor, mergeActivitySnapshot, mergeDownloadMonitor } from "./ActivityView";
import { workload, jobTitle } from "./ActivityView";
import { Selection, selectedReleases } from "./selection";
import { CoalescedQuery } from "./query";
import { ScanButton, ScanScope } from "./ScanButton";
import { ActionButton } from "./ActionButton";
import "./style.css";
import { version as appVersion } from "../package.json";
const groups = [
  {
    id: "prepare",
    name: "Prepare library",
    icon: Wrench,
    items: [
      ["correct", "Correct tags", Tags],
      ["organise", "Organise files", Folder],
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
      ["mqa", "MQA audit", ShieldCheck],
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
function placementId(value: unknown): string {
  if (typeof value === "number") return Number.isSafeInteger(value) && value > 0 ? String(value) : "";
  if (typeof value !== "string") return "";
  const id = value.trim();
  return /^\d+$/.test(id) && /[1-9]/.test(id) ? id : "";
}
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
const groupRoutes = Object.fromEntries(groups.map(group => [group.id, group.items[0][0]]));
const fileGroupRoutes = new Set(["files", "correct", "organise", "metadata", "artwork", "mqa"]);
function linkedFiles(rows: Row[]): Row[] {
  return rows.flatMap(row => row.artist_group || row.link_group || row.file_group ? linkedFiles(row.children || []) : [row]);
}
const fileColumns: Column[] = [
  { key: "artist", label: "Album artist" },
  { key: "release", label: "Release" },
  { key: "tracks", label: "Tracks" },
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
const defaultRecommendation = "Recommended and potential";
const defaultArtistScope = "My album artists";
const overviewMissingFilters = {timeline: "All missing releases", status: "all", artist_scope: defaultArtistScope, recommendation: defaultRecommendation};
const releaseTimelines = ["Newer than newest owned", "Between newest two owned", "All missing releases", "Incomplete albums", "All releases"];
const artistScopes = [defaultArtistScope, "All artist appearances", "Other artist appearances", "Artist credits not checked"];
type ColumnSelections = Record<string, ColumnSelection>;
function defaultColumnSelections(route: string): ColumnSelections {
  if (route === "missing") return {recommendation:{include:["Recommended", "Potential"]},status:{exclude:["Unavailable"]}};
  if (route === "links") return {status:{exclude:["Linked", "Ignored"]}};
  if (route === "correct") return {changes:{exclude:["", "—"]}};
  if (route === "organise") return {folder_operation:{exclude:["No change"]}};
  return {};
}
type MqaSelection = {
  ids: string[];
  detected_ids: string[];
  unlinked_ids: string[];
  ready_ids: string[];
  counts: {selected: number; detected: number; unlinked: number; ready: number};
};
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
  const titleId = React.useId();
  useEffect(() => {
    ref.current?.showModal();
  }, []);
  return (
    <dialog ref={ref} aria-labelledby={titleId} className={wide ? "wide" : ""} onCancel={onClose}>
      <header>
        <h2 id={titleId}>{title}</h2>
        <button aria-label="Close dialog" onClick={onClose}>
          <X size={18} />
        </button>
      </header>
      {children}
    </dialog>
  );
}
function App() {
  const initialRoute: string = "overview";
  const [state, setState] = useState<AppState | null>(null),
    [root, setRoot] = useState(localStorage.getItem("tibrary.root") || "");
  const currentRoot = useRef(root);
  currentRoot.current = root;
  const [navigation, setNavigation] = useState({ pages: ["overview"], index: 0 });
  const route = navigation.pages[navigation.index];
  const setRoute = useCallback((target: string) => {
    const page = groupRoutes[target] || (["connections", "downloads"].includes(target) ? "general" : target);
    setNavigation(previous =>
    previous.pages[previous.index] === page ? previous : {
      pages: [...previous.pages.slice(0, previous.index + 1), page], index: previous.index + 1,
    });
  }, []);
  const [sidebarVisible, setSidebarVisible] = useState(() => localStorage.getItem("tibrary.sidebar") !== "hidden");
  function moveHistory(offset: number) {
    setNavigation(previous => ({ ...previous, index: Math.max(0, Math.min(previous.pages.length - 1, previous.index + offset)) }));
  }
  const [collapsed, setCollapsed] = useState(new Set<string>()),
    [error, setError] = useState(""),
    [toast, setToast] = useState(""),
    [closing, setClosing] = useState(false),
    [stopped, setStopped] = useState(false);
  const [data, setData] = useState<{ rows: Row[]; total: number; artist_total?: number; release_total?: number; track_total?: number; missing_total?: number; scanned?: boolean }>({
      rows: [],
      total: 0,
    }),
    [loading, setLoading] = useState(false),
    [query, setQuery] = useState(""),
    [columnSelections, setColumnSelections] = useState<ColumnSelections>(() => defaultColumnSelections(initialRoute)),
    [viewOptionsOpen, setViewOptionsOpen] = useState(false),
    [sort, setSort] = useState(initialRoute === "missing" ? "date" : "artist"),
    [direction, setDirection] = useState(
      initialRoute === "missing" ? "desc" : "asc",
    ),
    [offset, setOffset] = useState(0);
  const [selected, setSelected] = useState(new Set<string>()),
    [expanded, setExpanded] = useState(new Set<string>()),
    [selection, setSelection] = useState<Selection>({});
  const [mqaScope, setMqaScope] = useState<ScanScope | null>("unscanned");
  const [mqaScopePending, setMqaScopePending] = useState(false);
  const [mqaSummary, setMqaSummary] = useState<(MqaSelection & {key: string}) | null>(null);
  const mqaScopeRequest = useRef(0);
  const [action, setAction] = useState("dates"),
    [preview, setPreview] = useState<string | undefined>(),
    [timeline, setTimeline] = useState("Newer than newest owned"),
    [artistScope, setArtistScope] = useState(defaultArtistScope);
  const viewOptionsButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!viewOptionsOpen) return;
    document.querySelector<HTMLInputElement>('.table-view-options input:checked')?.focus({preventScroll:true});
    return () => { if (viewOptionsButton.current?.isConnected) viewOptionsButton.current.focus({preventScroll:true}); };
  }, [viewOptionsOpen]);
  const [latestMissing, setLatestMissing] = useState<Row[] | null>(null);
  const [missingReleaseCount, setMissingReleaseCount] = useState<number | null>(null);
  const [downloadMonitor, setDownloadMonitor] = useState<Record<string, Row>>({});
  const [tableReader] = useState(() => new CoalescedQuery<any>(setLoading, e => setError(String(e))));
  const [overviewReader] = useState(() => new CoalescedQuery<any>(() => {}, e => setError(String(e))));
  const queueSelectionGuard = useRef({pending:0, minRevision:0});
  const detailReads = useRef(new Set<string>());
  const [loadingDetails, setLoadingDetails] = useState(new Set<string>());
  const [tableRefresh, setTableRefresh] = useState(0);
  useEffect(() => () => { tableReader.clear(); overviewReader.clear(); }, [tableReader, overviewReader]);
  const activityEpochs = useRef([0,0,0]);
  function mergeActivityEpochs(epochs?: number[]) {
    const previous = activityEpochs.current;
    const incoming = epochs || [0,0,0];
    activityEpochs.current = previous.map((epoch,i)=>Math.max(epoch,incoming[i] || 0));
    return {downloadsAccepted:(incoming[2] || 0) >= previous[2], downloadsCleared:(incoming[2] || 0) > previous[2]};
  }
  const [clock, setClock] = useState(Date.now());
  useEffect(() => {
    if (!active(state?.job) && !active(state?.online_job) && !active(state?.download_job)) return;
    const timer = window.setInterval(() => setClock(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [state?.job?.status, state?.online_job?.status, state?.download_job?.status]);
  useEffect(() => {
    if (route !== "overview" || !state) { overviewReader.clear(); return; }
    const args = {route: "missing", ...overviewMissingFilters,
      sort: "date", direction: "desc", limit: 20};
    overviewReader.request({key:JSON.stringify(args), revision:state.revision,
      read:()=>call("table",args), publish:result=>{setLatestMissing(result.rows); setMissingReleaseCount(result.total);}});
  }, [route, state?.revision]);
  const [detail, setDetail] = useState<any>(null),
    [review, setReview] = useState<any>(null),
    [exportPreview, setExportPreview] = useState<{
      title: string;
      content: string;
      filename: string;
    } | null>(null),
    [menu, setMenu] = useState<{ row: Row; x: number; y: number } | null>(null),
    [settings, setSettings] = useState<any>(null),
    [auth, setAuth] = useState("");
  const [manual, setManual] = useState(""),
    [deepQuery, setDeepQuery] = useState(""),
    [deep, setDeep] = useState<any>(null);
  const [submitting, setSubmitting] = useState(false);
  const [settingsSaving, setSettingsSaving] = useState(false);
  const settingWrites = useRef({next:0, versions:new Map<string,number>(), queue:Promise.resolve()});
  useEffect(() => {
    if (menu)
      document
        .querySelector<HTMLButtonElement>('[role="menu"] button')
        ?.focus();
  }, [menu]);
  const seenJobs = useRef(new Set<string>()),
    lastRoute = useRef(route),
    stateRef = useRef(state);
  stateRef.current = state;
  const onlineRoute = ["catalogue", "artists", "links", "favourites", "missing", "metadata", "artwork", "online", "connections", "fix"].includes(route);
  const displayTheme = settings?.general?.theme || state?.settings.theme || "system";
  const localBusy = submitting || active(state?.job);
  const indexChanging = active(state?.job) && ["scan", "apply", "deep_apply", "consolidate"].includes(state?.job?.kind || "");
  const busy = submitting || active(onlineRoute ? state?.online_job : state?.job) || (onlineRoute && indexChanging);
  const fileChangesRunning = active(state?.job) && ["apply", "deep_apply", "consolidate"].includes(state?.job?.kind || "");
  const queueBusy = submitting || (route === "queue" && active(state?.download_job));
  const downloadBusy = submitting || active(state?.download_job) || fileChangesRunning || (active(state?.job) && state?.job?.startup === true);
  const tree = ["missing", "queue", "downloaded"].includes(route);
  const notifyError = (e: any) => setError(String(e?.message || e));
  async function refresh(targetRoot?: string) {
    try {
      const activeRoot = targetRoot !== undefined ? targetRoot : root;
      if (!stateRef.current) {
        const initial = await call<AppState>("state", {bootstrap:true, ...(activeRoot ? {root:activeRoot} : {})});
        if (currentRoot.current !== activeRoot) return;
        stateRef.current = initial;
        setState(initial);
        const selectedRoot = initial.roots.some(library => library.root === activeRoot) ? activeRoot : initial.roots[0]?.root || "";
        if (selectedRoot !== activeRoot) { setRoot(selectedRoot); return; }
      }
      const s = await call<AppState>("state", activeRoot ? { root: activeRoot } : {});
      // A slow read from the previous library must not replace the new view.
      if (currentRoot.current !== activeRoot) return;
      const epochs = mergeActivityEpochs(s.activity_epochs);
      setState(previous => previous ? {...s, revision:Math.max(previous.revision,s.revision), logs:mergeActivitySnapshot(previous.logs,s.logs,previous.activity_epochs,s.activity_epochs), activity_epochs:[0,1,2].map(i=>Math.max(previous.activity_epochs?.[i] || 0,s.activity_epochs?.[i] || 0)), job:mergeJob(previous.job,s.job), online_job:mergeJob(previous.online_job,s.online_job), download_job:mergeJob(previous.download_job,s.download_job)} : s);
      if (s.download_monitor && epochs.downloadsAccepted) setDownloadMonitor(old => mergeDownloadMonitor(epochs.downloadsCleared ? {} : old,s.download_monitor!));
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
    call("settings").then(setSettings).catch(notifyError);
  }, []);
  useEffect(() => {
    localStorage.setItem("tibrary.root", root);
    setSelected(new Set());
    setOffset(0);
    setPreview(undefined);
    queueSelectionGuard.current = {pending:0, minRevision:0};
    refresh(root);
  }, [root]);
  useEffect(() => {
    localStorage.setItem("tibrary.route", route);
    setData({rows: [], total: 0});
    setSelection({});
    queueSelectionGuard.current = {pending:0, minRevision:0};
    setSelected(new Set());
    setOffset(0);
    setQuery("");
    setColumnSelections(defaultColumnSelections(route));
    setViewOptionsOpen(false);
    if (route === "missing") {
      setSort("date");
      setDirection("desc");
    } else {
      setSort("artist");
      setDirection("asc");
    }
    setAction(route === "organise" ? "organise" : "dates");
    setPreview(undefined);
    lastRoute.current = route;
  }, [route]);
  // Choosing a scope changes checkmarks, never starts audio work. Navigation
  // does not own the audit job; a completed job still refreshes its shared cache.
  useEffect(() => {
    if (route !== "mqa" || !root) return;
    chooseMqaScope("unscanned");
    return () => { mqaScopeRequest.current++; };
  }, [route, root]);
  const mqaSummaryKey = route === "mqa" ? JSON.stringify([root, state?.revision, [...selected]]) : "";
  const mqaSummaryCurrent = !!mqaSummary && mqaSummary.key === mqaSummaryKey;
  useEffect(() => {
    if (route !== "mqa" || !root) return;
    let live = true;
    const timer = window.setTimeout(() => {
      call<MqaSelection>("mqa.selection", {root, scope:"selected", ids:[...selected]})
        .then(summary => { if (live) setMqaSummary({...summary, key:mqaSummaryKey}); })
        .catch(error => { if (live) notifyError(error); });
    }, 120);
    return () => { live = false; window.clearTimeout(timer); };
  }, [mqaSummaryKey]);
  async function chooseMqaScope(scope: ScanScope) {
    const request = ++mqaScopeRequest.current;
    setMqaScopePending(true);
    try {
      const summary = await call<MqaSelection>("mqa.selection", {root, scope});
      if (request !== mqaScopeRequest.current) return;
      setMqaScope(scope);
      setSelected(new Set(summary.ids));
      setColumnSelections({});
      setQuery("");
      setOffset(0);
    } catch (error) { if (request === mqaScopeRequest.current) notifyError(error); }
    finally { if (request === mqaScopeRequest.current) setMqaScopePending(false); }
  }
  useEffect(() => {
    document.documentElement.dataset.theme = displayTheme;
  }, [displayTheme]);
  useEffect(() => {
    let live = true;
    let refreshing = false;
    const dark = window.matchMedia("(prefers-color-scheme: dark)");
    const contrast = window.matchMedia("(prefers-contrast: more)");
    const refresh = () => {
      if (refreshing) return;
      refreshing = true;
      call<{name:string;hex:string;palettes?:Record<string,Record<string,string>>}>("appearance.accent").then(accent => {
        if (!live) return;
        const theme = displayTheme;
        const mode = (theme === "dark" || (theme === "system" && dark.matches) ? "dark" : "light")
          + (contrast.matches ? "_high_contrast" : "");
        const palette = accent.palettes?.[mode];
        const style = document.documentElement.style;
        const validColour = (value: string | undefined) => value && /^#[0-9a-f]{6}$/i.test(value);
        for (const key of ["accent", "blue", "purple", "pink", "red", "orange", "yellow", "green", "graphite", "selection", "selection_text"]) {
          const value = palette?.[key] || (key === "accent" ? accent.hex : undefined);
          if (validColour(value)) style.setProperty(`--system-${key.replaceAll("_", "-")}`, value!);
        }
        if (validColour(palette?.table_header)) style.setProperty("--table-header", palette!.table_header);
      }).catch(() => {}).finally(() => { refreshing = false; });
    };
    refresh();
    const timer = window.setInterval(refresh,10000);
    window.addEventListener("focus",refresh);
    dark.addEventListener("change",refresh);
    contrast.addEventListener("change",refresh);
    return () => {
      live=false;
      window.clearInterval(timer);
      window.removeEventListener("focus",refresh);
      dark.removeEventListener("change",refresh);
      contrast.removeEventListener("change",refresh);
    };
  }, [displayTheme]);
  const pageSize = Number(settings?.general?.page_size || 50);
  useEffect(() => { setOffset(0); }, [pageSize]);
  const activeColumnSelections = availableColumnSelections(route, columnSelections);
  const viewArgs = {
    route,
    root: root || undefined,
    offset,
    limit: pageSize,
    search: query,
    filter: "all",
    column_filters: activeColumnSelections,
    sort,
    direction,
    action,
    preview_id: preview,
    timeline,
    recommendation: "All recommendations",
    artist_scope: artistScope,
    type: "All types",
    include_unavailable: route === "missing",
    group_releases: route === "links" || fileGroupRoutes.has(route),
    group_artists: route === "links",
    local_releases: ["artists", "favourites"].includes(route),
  };
  useEffect(() => {
    if (!state || (root && !state.roots.some((r) => r.root === root))
      || ["overview", "prepare", "catalogue", "complete", "fix", "settings", "general", "connections", "downloads", "activity"].includes(route)) {
      tableReader.clear();
      return;
    }
    tableReader.request({key:JSON.stringify(viewArgs), revision:state.revision, version:tableRefresh,
      read:()=>call("table",viewArgs), publish:(value, readRevision)=>{
        setData(value);
        if (!preview && value.preview_id) setPreview(value.preview_id);
        if (route === "queue" && !queueSelectionGuard.current.pending && readRevision >= queueSelectionGuard.current.minRevision)
          setSelection(Object.fromEntries(value.rows.map((r:Row)=>[r.id,r.selected])));
      }}, query ? 150 : 0);
  }, [
    tableRefresh,
    route,
    root,
    offset,
    query,
    columnSelections,
    sort,
    direction,
    action,
    preview,
    timeline,
    artistScope,
    pageSize,
    state?.revision,
  ]);
  useEffect(() => {
    let dispose: (() => void) | undefined;
    let refreshTimer: number | undefined;
    const scheduleRefresh = (delay = 60) => {
      // Coalesce artist results without delaying an already scheduled refresh.
      // Completion and local mutations still publish immediately.
      if (refreshTimer !== undefined && delay > 60) return;
      window.clearTimeout(refreshTimer);
      refreshTimer=window.setTimeout(() => {refreshTimer=undefined; refresh();}, delay);
    };
    listen<any>("backend-event", ({ payload: p }) => {
      if (p.event === "download-monitor" && p.item) {
        const item = p.item as Row;
        const jobId = item.job_id || stateRef.current?.download_job?.id || "saved-downloads";
        const key = item.kind === "batch" ? `${jobId}:batch:${item.release_id}` : `${jobId}:track:${item.release_id}:${item.id}`;
        setDownloadMonitor((old) => mergeDownloadMonitor(old,{[key]:{...item,job_id:jobId}}));
      }
      if (p.event === "progress" && p.download_job) {
        setState((s) => s ? { ...s, download_job: mergeJob(s.download_job,p.download_job) } : s);
      }
      if (p.event === "progress" && p.online_job) {
        setState((s) => s ? { ...s, online_job: mergeJob(s.online_job,p.online_job), logs: mergeActivitySnapshot(s.logs,[
          {job_id:p.online_job.id, job_kind:p.online_job.kind, job_status:p.online_job.status, progress_id:p.online_job.id, at:new Date().toISOString(), message:p.message, category:"online", level:"info"}
        ])} : s);
      }
      if (p.event === "progress" && !p.download_job && !p.online_job)
        setState((s) =>
          s
            ? {
                ...s,
                job: mergeJob(s.job,p.job),
                logs: mergeActivitySnapshot(s.logs,[
                  {
                    job_id: p.job?.id, job_kind: p.job?.kind, job_status: p.job?.status,
                    progress_id: p.job?.id,
                    at: new Date().toISOString(),
                    message: p.message,
                    level: p.job?.status === "failed" ? "error" : p.level || "info",
                    category: p.category || (p.job?.kind ? (p.job.kind === "scan" ? "scan" : p.job.kind === "download" ? "download" : p.job.kind === "link" ? "linking" : p.job.kind.includes("duplicate") ? "cleanup" : "local") : undefined),
                  },
                ]),
              }
            : s,
        );
      if (p.event === "library-mutated") { setPreview(undefined); setReview(null); setDeep(null); }
      if (["changed", "library-mutated", "job", "ready"].includes(p.event))
        scheduleRefresh(p.event === "changed" && p.reason === "catalogue-refresh" ? 2000 : 60);
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
    return () => {dispose?.(); window.clearTimeout(refreshTimer);};
  }, [root]);
  // Fallback snapshot also recovers a missed completion event during window startup.
  useEffect(() => {
    let polling = false, disposed = false;
    const timer = setInterval(async () => {
      if (polling || ![stateRef.current?.job, stateRef.current?.online_job, stateRef.current?.download_job].some(active)) return;
      polling = true;
      try {
        const update = await call<any>("job.status");
        if (!disposed && (update.job || update.online_job || update.download_job)) {
          const epochs = mergeActivityEpochs(update.activity_epochs);
          setState(s => s ? {...s, ...update, revision:Math.max(s.revision,update.revision || 0), logs:mergeActivitySnapshot(s.logs,update.logs || [],s.activity_epochs,update.activity_epochs), activity_epochs:[0,1,2].map(i=>Math.max(s.activity_epochs?.[i] || 0,update.activity_epochs?.[i] || 0)), job:mergeJob(s.job,update.job), online_job:mergeJob(s.online_job,update.online_job), download_job:mergeJob(s.download_job,update.download_job)} : s);
          if (update.download_monitor && epochs.downloadsAccepted) setDownloadMonitor(old=>mergeDownloadMonitor(epochs.downloadsCleared ? {} : old,update.download_monitor));
          if (![update.job, update.online_job, update.download_job].some(active)) await refresh();
        }
      } catch (e) { if (!disposed) notifyError(e); }
      finally { polling = false; }
    }, 2000);
    return () => { disposed = true; clearInterval(timer); };
  }, [root]);
  useEffect(() => {
    for (const j of [state?.job, state?.online_job]) {
    if (!j || j.historical || active(j) || seenJobs.current.has(j.id)) continue;
    seenJobs.current.add(j.id);
    if (j.status === "failed") {
      setError(j.message);
      continue;
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
      else if (root === j.result.root && route === target)
        setColumnSelections(target === "organise" ? defaultColumnSelections(target) : {changes:{exclude:["", "—"]}});
    }
    if (["apply", "deep_apply", "consolidate"].includes(j.kind)) {
      setPreview(undefined);
      setSelected(new Set());
    }
    if (j.kind === "queue_mqa" && j.status === "complete") {
      setToast("Replacements queued; review them in Download queue");
      if (j.result?.root === root && route === "mqa") {
        const queued = new Set<string>(j.result?.queued_paths || []);
        setSelected(previous => new Set([...previous].filter(path => !queued.has(path))));
        setMqaScope(null);
      }
    }
    if (j.kind === "manual_candidate" && j.status === "complete" && j.result?.root === root && detail?.path &&
        j.result?.inspected_paths?.includes(detail.path)) {
      const path = detail.path;
      call("detail", {root, path, check_availability:route === "links"})
        .then(value => setDetail((current: any) => current?.path === path ? {...current, ...value} : current))
        .catch(notifyError);
    }
    if (j.kind === "release_details" && detail?.release?.id && j.status === "complete") {
      const id = detail.release.id;
      call("detail", {release_id:id}).then(value => setDetail((current: any) => current?.release?.id === id ? {
        ...current,
        release:{...value, release:value.release || value.title},
        track:current.track ? (value.tracks || []).find((track: Row) => track.id === current.track.id) || current.track : null,
      } : current)).catch(notifyError);
    }
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
    }
  }, [state?.job, state?.online_job]);
  async function run(kind: string, args: any = {}) {
    setError("");
    setSubmitting(true);
    try {
      const j = await call<Job>("job.start", { kind, args: { root, ...args } });
      setState((s) => (s ? kind === "download" ? { ...s, download_job: mergeJob(s.download_job,j) } : onlineKinds.has(kind) ? { ...s, online_job: mergeJob(s.online_job,j) } : { ...s, job: mergeJob(s.job,j) } : s));
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
      if (row.artist_group && route === "links") {
        setDetail({local_artist: row});
      } else if (row.file_group) {
        setDetail({local_file_release: row});
      } else if (row.link_group || row.local_release) {
        setDetail({local_release:row});
      } else if (tree) {
        const value = await call("detail", { release_id: row.parent || row.id });
        setDetail({
          release: { ...value, release: value.release || value.title,
            children: value.children || (value.tracks || []).map((t: Row) => ({...t, position: t.position || `Disc ${t.disc_number || 1} · Track ${t.track_number || "?"}`})) },
          track: row.parent ? {...row, ...(value.tracks || []).find((track: Row) => track.id === row.id)} : null,
          evidence: row.evidence || [],
        });
      } else if (route === "artists" || route === "favourites") {
        const d = await call("detail", { artist: row.lookup_artist || row.artist, root });
        setManual((d.ids || []).join(","));
        setDetail(d);
      } else if (route === "local" || route === "online") {
        setDetail({operation: row});
      } else if (row.path || route === "links") {
        setDetail({...await call("detail", { root, path: row.path || row.id, check_availability: route === "links" }), proposed: row.changes, folder_target: route === "organise" ? (row.target || row.path || row.id) : undefined, folder_evidence: route === "organise" ? row.evidence : undefined, source_release_id: row.source_release_id, source_track_id: row.source_track_id});
      }
    } catch (e) {
      notifyError(e);
    }
  }
  function scope() {
    return selected.size ? { ids: [...selected] } : {};
  }
  async function prepareMqaReplacements(kind: "link" | "queue_mqa") {
    if (mqaScopePending || !selected.size) return;
    setSubmitting(true);
    try {
      // Read the whole selection again, including off-page tracks. Never pass
      // an empty scope to linking, where omission means the entire library.
      const summary = await call<MqaSelection>("mqa.selection", {root, scope:"selected", ids:[...selected]});
      const ids = kind === "link" ? summary.unlinked_ids : summary.ready_ids;
      if (!ids.length) {
        setToast(kind === "link" ? "Selected MQA tracks already have matches" : "Find an online match for the selected MQA tracks first");
        return;
      }
      await run(kind, {ids});
    } catch (error) { notifyError(error); }
    finally { setSubmitting(false); }
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
    const prior = selection;
    setSelection(next);
    if (route === "queue") {
      const guard = queueSelectionGuard.current;
      guard.pending++;
      guard.minRevision = Math.max(guard.minRevision,(stateRef.current?.revision || 0)+1);
      try {
        if (!await mutate("queue.select", {selection:next})) {guard.minRevision=0; setSelection(prior);}
      } finally {guard.pending--;}
    }
  }

  async function loadReleaseTracks(row: Row) {
    if (detailReads.current.has(row.id)) return;
    detailReads.current.add(row.id);
    setLoadingDetails(previous=>new Set(previous).add(row.id));
    try {
      await call("release.ensure_tracks", {id:row.id});
      setTableRefresh(previous=>previous+1);
      await refresh();
    } catch (e) { notifyError(e); }
    finally {
      detailReads.current.delete(row.id);
      setLoadingDetails(previous=>{const next=new Set(previous);next.delete(row.id);return next;});
    }
  }
  async function prepareApply() {
    if (!preview || !selected.size) return;
    try {
      const p = await call("preview", { id: preview });
      const affected = p.rows.filter((r: Row) => selected.has(r.id) && r.affected !== false);
      if (!affected.length) { setToast("No proposed changes in the selected files"); return; }
      setReview({
        ...p,
        rows: affected,
        selected: [...selected],
      });
    } catch (e) {
      notifyError(e);
    }
  }
  function updateSetting(section: string, key: string, value: any) {
    const version = ++settingWrites.current.next;
    const fieldId = `${section}.${key}`;
    settingWrites.current.versions.set(fieldId, version);
    setSettings((prev: any) => prev ? { ...prev, [section]: { ...prev[section], [key]: value } } : prev);
    setSettingsSaving(true);
    setError("");
    const backendSection =
      section === "general"
        ? "desktop"
        : section === "links"
        ? "release_links"
        : section;

    // Apply key patches in input order. An older response must not replace a
    // newer choice, another field or a draft the user is still editing.
    const save = async () => {
      try {
        const result = await call<any>("settings.update", {section:backendSection, values:{[key]:value}});
        if (settingWrites.current.versions.get(fieldId) === version) {
          setSettings((prev: any) => prev ? {...prev, [section]:{...prev[section], [key]:result[section]?.[key] ?? value}} : result);
        }
      } catch (error) {
        notifyError(error);
        const saved = await call<any>("settings").catch(() => null);
        if (saved && settingWrites.current.versions.get(fieldId) === version) {
          setSettings((prev: any) => prev ? {...prev, [section]:{...prev[section], [key]:saved[section]?.[key]}} : saved);
        }
      } finally {
        if (settingWrites.current.next === version) {
          setSettingsSaving(false);
          void refresh();
        }
      }
    };
    const queued = settingWrites.current.queue.then(save, save);
    settingWrites.current.queue = queued;
    return queued;
  }
  async function openExport(targetRoute: "queue" | "downloaded" = "queue") {
    try {
      const isDownloaded = targetRoute === "downloaded";
      const { text } = await call("queue.export", {
        format: "text",
        decision: isDownloaded ? "downloaded" : "queued",
        selection: isDownloaded && Object.keys(selectedReleases(selection)).length
          ? selectedReleases(selection) : undefined,
      });
      const content = typeof text === "string" ? text : "";
      setExportPreview({
        title: isDownloaded ? "Export downloaded releases" : "Export queue",
        content,
        filename: isDownloaded
          ? "downloaded-releases.txt"
          : "acquisition-queue.txt",
      });
    } catch (e) {
      notifyError(e);
    }
  }
  const columns: Column[] = tree
    ? releaseColumns
    : route === "links"
      ? [
          {key:"artist",label:"Album artist",width:"20%"},
          {key:"release",label:"Release",width:"28%"},
          {key:"tracks",label:"Tracks",width:180},
          {key:"status",label:"Status",width:110},
          {key:"evidence",label:"Evidence"},
        ]
    : route === "artists"
      ? [
          { key: "artist", label: "Album artist", width:"22%" },
          { key: "tracks", label: "Tracks", width:86 },
          { key: "release", label: "Releases", width:"27%" },
          { key: "status", label: "Match status", width:140 },
          { key: "evidence", label: "Evidence" },
        ]
      : route === "favourites"
        ? [
            { key: "artist", label: "Artist", width:"20%" },
            { key: "status", label: "Library status", width:130 },
            { key: "catalogue_status", label: "Catalogue link", width:130 },
            { key: "tracks", label: "Local tracks", width:110 },
            { key: "release", label: "Releases", width:"23%" },
            { key: "evidence", label: "Evidence" },
          ]
        : route === "local"
          ? [
              { key: "artist", label: "Artist" },
              { key: "release", label: "Release to keep" },
              { key: "date", label: "Release date" },
              { key: "target", label: "Replacement" },
              { key: "tracks", label: "Tracks" },
              { key: "duplicates", label: "Duplicates" },
              { key: "evidence", label: "Evidence" },
            ]
        : route === "online"
          ? [
              { key: "artist", label: "Artist" },
              { key: "release", label: "Release" },
              { key: "date", label: "Release date" },
              { key: "tracks", label: "Tracks" },
              { key: "duplicates", label: "Duplicate files" },
              { key: "gained", label: "Tracks gained" },
              { key: "evidence", label: "Evidence" },
            ]
        : route === "mqa"
          ? [
              { key: "artist", label: "Artist" },
              { key: "release", label: "Release" },
              { key: "tracks", label: "Tracks" },
              { key: "status", label: "Status" },
              { key: "evidence", label: "Evidence" },
              { key: "target", label: "Action" },
            ]
        : route === "organise"
          ? [
              ...fileColumns.slice(0, 3),
              { key: "folder_operation", label: "Folder operation" },
              { key: "target", label: "Destination" },
              { key: "evidence", label: "Evidence" },
            ]
        : ["correct", "metadata", "artwork"].includes(route)
          ? [
              ...fileColumns.slice(0, 3),
              { key: "changes", label: "Proposed tag changes" },
              { key: "evidence", label: "Evidence" },
            ]
          : fileColumns;
  const headerFilters: Record<string, HeaderFilter> = {};
  for (const column of columns) {
    if (!columnFilterAvailable(route, column.key)) continue;
    const otherSelections = {...activeColumnSelections};
    delete otherSelections[column.key];
    const facetArgs = {...viewArgs, offset:0, limit:100, column:column.key, column_filters:otherSelections};
    headerFilters[column.key] = {
      label:`${column.label} values`,
      selection:activeColumnSelections[column.key],
      optionsKey:JSON.stringify({...facetArgs, revision:state?.revision, refresh:tableRefresh}),
      loadOptions:search => call<ColumnFilterOptions>("table.facets", {...facetArgs, facet_search:search}),
      onChange:selection => {
        setColumnSelections(previous => {
          const next = {...previous};
          if (selection === undefined) delete next[column.key];
          else next[column.key] = selection;
          return next;
        });
        setOffset(0);
      },
    };
  }
  function openMissingReleases() {
    setTimeline(overviewMissingFilters.timeline);
    setArtistScope(overviewMissingFilters.artist_scope);
    setColumnSelections(defaultColumnSelections("missing"));
    setQuery("");
    setOffset(0);
    setRoute("missing");
  }
  function card(
    title: string,
    value: any,
    note: string,
    target: string,
    Icon: any = Music2,
  ) {
    return (
      <button className="metric" title={target === "missing" ? "My album artists · Recommended and Potential · all missing, incomplete and queued releases" : undefined} onClick={() => target === "missing" ? openMissingReleases() : setRoute(target)}>
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
  function overviewPage() {
    const s = state?.stats || {};
    const libraries = root ? state?.roots.filter(library => library.root === root) : state?.roots;
    const indexed = state?.stats_pending ? libraries?.reduce((count,library) => count + library.tracks,0) : s.track_count;
    const linked = state?.stats_pending ? libraries?.reduce((count,library) => count + library.linked,0) : s.linked_tracks;
    return (
      <div className="overview-dashboard">
        <div className="metrics">
              {card(
                "Local tracks",
                indexed == null ? null : `${(linked || 0).toLocaleString()} / ${indexed.toLocaleString()}`,
                "Linked tracks / indexed locally",
                "files",
              )}
              {card(
                "Linked releases",
                state?.stats_pending || !state ? null : `${s.linked_releases || 0} / ${s.release_count || 0}`,
                "Whole-release associations",
                "links",
                Link,
              )}
              {card(
                "Missing releases",
                missingReleaseCount?.toLocaleString(),
                "My album artists · Recommended and Potential",
                "missing",
                Disc,
              )}
              {card(
                "Download queue",
                s.queued,
                "Releases waiting for approval",
                "queue",
                ArrowDownToLine,
              )}
        </div>
        <section className="card libraries-card">
          <div className="section-heading">
            <div>
              <h2>Your libraries</h2>
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
        <section className="card latest-missing-card">
            <div className="section-heading">
              <h2>Latest missing releases</h2>
              <button onClick={() => { openMissingReleases(); setSort("date"); setDirection("desc"); }}>
                View missing releases
              </button>
            </div>
            <div className="latest-missing-scroll">
            <div className="overview-list" role="region" aria-label="Latest missing releases" tabIndex={0}>
            {latestMissing?.length ? (
              latestMissing.map((r) => (
                <div className="library-row" key={r.id}>
                  <Music2 size={18} />
                  <strong>
                    {r.artist} — {r.release}
                  </strong>
                  <span>
                    {[
                      r.date,
                      r.type ? r.type.toUpperCase() : "",
                      r.recommendation,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
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
              <p>{latestMissing === null ? "Loading cached missing releases…" : "No missing releases with verified album artists. Check release artists or view all recommendations in Missing releases."}</p>
            )}
            </div>
            <div className="edge-gradient-top" aria-hidden="true" />
            <div className="edge-gradient-bottom" aria-hidden="true" />
            </div>
        </section>
      </div>
    );
  }
  function chooseOperation(operation: string) {
    setAction(operation);
    setPreview(undefined);
    setSelected(new Set());
    setColumnSelections(defaultColumnSelections(route));
    setOffset(0);
  }
  function selectedDetail() {
    const id = [...selected][0];
    if (id) loadDetail((route === "links" || fileGroupRoutes.has(route) ? linkedFiles(data.rows) : data.rows).find(row => row.id === id) || {id, artist:id});
  }
  function tableActions() {
    const selectedReleaseCount = Object.keys(selectedReleases(selection)).length;
    const clear = !tree && selected.size > 0 && <button onClick={() => setSelected(new Set())}>Clear selection</button>;
    const apply = <button disabled={localBusy || active(state?.download_job) || !preview || !selected.size} onClick={prepareApply}>Review & apply ({selected.size})</button>;
    let controls: React.ReactNode;
    if (["correct", "organise"].includes(route)) {
      const operations = route === "correct" ? actions : organisationActions;
      const name = operations.find(([id]) => id === action)?.[1] || operations[0][1];
      const label = action === "dates" ? "Preview dates" : action === "numbers" ? "Preview track & disc numbers" : action === "keys" ? "Preview musical keys" : action === "lyrics" ? "Preview lyric removal" : `Preview ${name.toLowerCase()}`;
      controls = <><ActionButton label={label} ariaLabel={route === "organise" ? "Preview moves" : "Preview changes"}
        onClick={() => run("preview", {action})} disabled={busy || !root}
        busy={active(state?.job) && state?.job?.kind === "preview"} busyLabel="Preparing preview…"
        menuLabel={route === "correct" ? "Choose correction" : "Choose organisation action"}
        title="Preview one operation at a time. Files change only after you review and apply the selected proposals."
        options={operations.map(([id,label,note]) => ({id,label,note,selected:action === id,onClick:() => chooseOperation(id)}))} />
        {clear}<div className="table-result-actions">{apply}</div></>;
    } else if (route === "files") {
      controls = clear;
    } else if (route === "links") {
      controls = <><ActionButton label={selected.size ? `Recheck ${selected.size} selected` : "Link unresolved tracks"}
        onClick={() => run("link",scope())} disabled={submitting || !root || (busy && localBusy)} actionDisabled={busy} menuLabel="Linking options"
        title="Match selected tracks, or only unresolved tracks when nothing is selected. Saves links without changing music files."
        options={[
          {id:"editions",label:"Recheck edition choices",note:"Recheck only unresolved tracks that need an edition choice across this library.",disabled:busy,onClick:() => run("link",{editions_only:true})},
          {id:"extended",label:"Extended review",note:"Review the selected tracks with extended matching evidence.",disabled:busy || !selected.size,onClick:() => run("deep_review",scope())},
        ]} />{clear}<div className="table-result-actions"><button disabled={!selected.size} onClick={selectedDetail}>Choose match</button></div></>;
    } else if (route === "artists") {
      controls = <><ActionButton label={selected.size ? "Match selected artists" : "Match artist"}
        onClick={() => run("match_artists",selected.size ? {artists:[...selected]} : {})} disabled={submitting || !root || (busy && localBusy)} actionDisabled={busy}
        menuLabel="Artist matching options" title="Match selected album artists, or unresolved album artists when nothing is selected."
        options={[
          {id:"unresolved",label:"Match unresolved artists",note:"Reuse confirmed artists and check only artists that still need a match.",disabled:busy,onClick:() => run("match_artists")},
          {id:"unlink",label:"Unlink selected artists",note:"Remove artist associations. File tags and recording links stay unchanged.",disabled:busy || !selected.size,onClick:() => unlinkArtists([...selected])},
        ]} />{clear}<div className="table-result-actions"><button disabled={!selected.size} onClick={selectedDetail}>Review match</button></div></>;
    } else if (["metadata", "artwork"].includes(route)) {
      controls = <><ActionButton label={route === "metadata" ? "Find missing tags (online)" : "Find artwork (online)"}
        onClick={() => run(route,scope())} disabled={submitting || !root || (busy && localBusy)} actionDisabled={busy}
        title={route === "metadata" ? "Reuse verified links and cached metadata, then fill missing credits, tags and available BPM/key. Files change only after review and approval." : "Reuse saved cover information, then find genuine 1280 × 1280 covers for linked tracks. Files change only after review and approval."}
        menuLabel={route === "metadata" ? "Metadata lookup options" : "Artwork lookup options"}
        options={[
          {id:"all",label:route === "metadata" ? "Find missing tags for all files" : "Find artwork for all files",note:"Use every indexed file in this library, reusing completed checks where possible.",disabled:busy,onClick:() => run(route)},
        ]} />{clear}<div className="table-result-actions">{apply}</div></>;
    } else if (["local", "online"].includes(route)) {
      const local = route === "local";
      controls = <><ActionButton label={local ? "Check local duplicates" : "Find cached replacements"}
        onClick={() => run(local ? "local_duplicates" : "optimizations",{scope:local ? "local" : "remote"})}
        disabled={busy || !root} menuLabel={local ? "Duplicate check options" : "Replacement search options"}
        title={local ? "Find releases fully contained in another local release, using the shared tag index. Review before moving redundant audio to Trash." : "Find complete online replacements using saved catalogue data. Makes no network requests."}
        options={local ? [
          {id:"chained",label:"Select chained duplicates",note:"Select chained duplicate groups on this page for one removal review.",disabled:loading || !data.rows.some(row => row.status === "Chained duplicate"),onClick:() => setSelected(new Set(data.rows.filter(row => row.status === "Chained duplicate").map(row => row.id)))},
        ] : [
          {id:"online",label:"Search online replacements",note:"Fill missing catalogue evidence before checking for larger releases that preserve your recordings.",onClick:() => run("check_replacements",{scope:"remote"})},
        ]} />{clear}<div className="table-result-actions">{local
          ? <button disabled={busy || !selected.size} onClick={() => run("review_consolidation",{ids:[...selected],scope:"local"})}>Review duplicate removal {selected.size ? `(${selected.size})` : ""}</button>
          : <button disabled={busy || !selected.size} onClick={() => run("queue_replacements",{ids:[...selected]})}>Queue replacement releases {selected.size ? `(${selected.size})` : ""}</button>}</div></>;
    } else if (route === "favourites") {
      controls = <><ActionButton label="Refresh favourite artists" onClick={() => run("favourites")} disabled={submitting || (busy && localBusy)} actionDisabled={busy}
        menuLabel="Favourite artist options" title="Refresh online favourites and compare them with linked local album artists."
        options={[{id:"match",label:"Match local artists",note:"Match unresolved local album artists to their online identities.",disabled:busy || !root,onClick:() => run("match_artists")}]} />{clear}</>;
    } else if (route === "missing") {
      const refresh = state?.catalogue_refresh;
      const resumable = refresh && refresh.status !== "complete" && refresh.completed.length < refresh.ids.length;
      controls = <><ActionButton label="Update missing releases" onClick={() => run("discography",{detailed:true})} disabled={busy}
        menuLabel="Release update options"
        title="Check linked artists' release lists and fill missing track details, credits and recommendation evidence. Saved complete checks are reused; no audio is downloaded."
        options={[
          ...(resumable ? [{id:"resume",label:`Resume refresh (${refresh.completed.length}/${refresh.ids.length})`,note:"Continue the saved refresh from the last completed artist.",onClick:() => run("discography",{...refresh,resume:true})}] : []),
          {id:"lists",label:"Check release lists only",note:"A quicker discovery check that reuses saved track details and credits. No audio is downloaded.",onClick:() => run("discography")},
          {id:"artists",label:"Fill missing release artists",note:"Complete missing album artist evidence for saved releases in the current release range.",onClick:() => run("release_artists",{timeline})},
          {id:"selected-availability",label:"Check availability",note:selectedReleaseCount ? "Check the selected releases, reusing recent availability checks." : "Check the releases on this page, reusing recent availability checks.",disabled:loading || (!data.rows.length && !selectedReleaseCount),onClick:() => run("check_availability",{ids:selectedReleaseCount ? Object.keys(selectedReleases(selection)) : data.rows.map(row => row.id)})},
          {id:"saved-availability",label:"Check saved release availability",note:"Check all saved missing releases for linked album artists, reusing recent checks.",onClick:() => run("check_availability")},
          {id:"fresh-availability",label:"Recheck saved availability online",note:"Request fresh availability for all saved releases, including unavailable ones. Bypasses saved availability checks.",onClick:() => run("check_availability",{force:true})},
          {id:"cached",label:"Recalculate saved results",note:"Update ownership and recommendations from the central cache. Makes no network requests.",onClick:() => run("cached_releases")},
        ]} /><div className="table-result-actions"><button disabled={queueBusy || !selectedReleaseCount}
          onClick={() => mutate("queue.add",{selection:selectedReleases(selection)})}>Queue selected ({selectedReleaseCount})</button></div></>;
    } else if (route === "queue") {
      controls = <ActionButton label={active(state?.download_job) ? "Downloading…" : "Download"} ariaLabel="Download"
        disabled={submitting} actionDisabled={downloadBusy || !state?.stats.approved_queue}
        onClick={async () => {
          setSubmitting(true);
          try {setReview({operation:"download",rows:await call("queue.preview")});}
          catch(e) {notifyError(e);} finally {setSubmitting(false);}
        }} menuLabel="Download options" title="Review and download all approved audio tracks in the queue, including approvals on other pages."
        options={[{id:"export",label:"Export",note:"Save queued release or track links as a text list for an external downloader.",onClick:() => openExport("queue")}]} />;
    } else if (route === "downloaded") {
      controls = <button disabled={submitting} onClick={() => openExport("downloaded")}>Export</button>;
    }
    return controls ? <div className="table-actions" role="group" aria-label="Table actions" key={route}>{controls}</div> : null;
  }
  function pagination() {
    return <nav className="table-pagination-top" aria-label="Table pagination">
      <span>
        {data.total
          ? `${offset + 1}–${Math.min(offset + pageSize, data.total)} of ${data.total.toLocaleString()}`
          : "No items"}
        {route === "links" && data.total > 0 && ` artists · ${(data.release_total || 0).toLocaleString()} releases · ${(data.track_total || 0).toLocaleString()} tracks`}
        {fileGroupRoutes.has(route) && data.total > 0 && " releases"}
      </span>
      <div>
        <button disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - pageSize))}>Previous</button>
        <button disabled={offset + pageSize >= data.total} onClick={() => setOffset(offset + pageSize)}>Next</button>
      </div>
    </nav>;
  }
  function tablePage() {
    return (
      <>
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
          {route === "missing" && <div className="table-view-control">
            <button ref={viewOptionsButton} aria-haspopup="dialog" aria-expanded={viewOptionsOpen} onClick={() => setViewOptionsOpen(open=>!open)} title={`${artistScope} · ${timeline}`}>
              <SlidersHorizontal size={15} /> View options
            </button>
            {viewOptionsOpen && <>
              <div className="view-options-scrim" onClick={() => setViewOptionsOpen(false)} />
              <div className="table-view-options" role="dialog" aria-label="Table view options" onKeyDown={event => {if(event.key === "Escape") {event.stopPropagation();setViewOptionsOpen(false);}}}>
                <header><strong>View options</strong><button aria-label="Close view options" onClick={() => setViewOptionsOpen(false)}><X size={16} /></button></header>
                <fieldset role="radiogroup" aria-label="Album artist scope">
                  <legend>Album artist scope</legend>
                  {artistScopes.map(value => <label key={value}><input type="radio" name="artist-scope" checked={artistScope === value} onChange={() => {setArtistScope(value);setOffset(0);}} />{value}</label>)}
                </fieldset>
                <fieldset role="radiogroup" aria-label="Release timeline">
                  <legend>Release range</legend>
                  {releaseTimelines.map(value => <label key={value}><input type="radio" name="release-timeline" checked={timeline === value} onChange={() => {setTimeline(value);setOffset(0);}} />{value}</label>)}
                </fieldset>
              </div>
            </>}
          </div>}
          {Object.keys(activeColumnSelections).length > 0 && <button className="reset-column-filters" aria-label="Reset column filters" onClick={() => {setColumnSelections({});setOffset(0);}} title="Reset column filters: show all column values within the current view options"><FilterX size={16} /></button>}
          {selected.size > 0 && <span>{selected.size} selected</span>}
          {pagination()}
        </div>
        <DataTable
          rows={data.rows}
          columns={columns}
          headerFilters={headerFilters}
          selected={selected}
          onSelect={next => {setSelected(next); if (route === "mqa") setMqaScope(null);}}
          sort={sort}
          direction={direction}
          onSort={(key, requestedDirection) => {
            setSort(key);
            setDirection(requestedDirection || (sort === key && direction === "asc" ? "desc" : "asc"));
            setOffset(0);
          }}
          tree={tree}
          grouped={["local", "online"].includes(route)}
          fileGroups={fileGroupRoutes.has(route)}
          hierarchy={route === "links" ? "links" : ["artists", "favourites"].includes(route) ? "artists" : undefined}
          treeSelection={selection}
          onTreeSelect={selectTree}
          expanded={expanded}
          setExpanded={setExpanded}
          onExpand={loadReleaseTracks}
          loadingDetails={loadingDetails}
          onDetail={loadDetail}
          onMenu={(row, x, y) => setMenu({ row, x, y })}
          loading={loading}
          busy={tree ? queueBusy : busy}
        />
        {route !== "mqa" && tableActions()}
        {route === "mqa" && <div className="mqa-actions table-actions" role="group" aria-label="Table actions">
          <ScanButton count={mqaSummaryCurrent ? mqaSummary!.counts.selected : selected.size} scope={mqaScope}
            disabled={localBusy || !root || mqaScopePending} selectionPending={!mqaSummaryCurrent}
            scanning={active(state?.job) && state?.job?.kind === "mqa"}
            onScope={chooseMqaScope}
            onScan={() => { if (mqaSummaryCurrent && mqaSummary!.ids.length) run("mqa", {ids:mqaSummary!.ids, force:true}); }} />
          <button disabled={!selected.size || mqaScopePending} onClick={() => {setSelected(new Set());setMqaScope(null);}}>Clear selection</button>
          <div className="mqa-replacement-actions">
            <button aria-label="Find online matches" disabled={localBusy || active(state?.online_job) || mqaScopePending || !mqaSummaryCurrent || !mqaSummary!.counts.unlinked}
              title="Step 1: identify online recordings for selected MQA tracks without a verified match. Saves database links; no files are changed."
              onClick={() => prepareMqaReplacements("link")}>
              Find online matches ({mqaSummaryCurrent ? mqaSummary!.counts.unlinked : "…"})
            </button>
            <button aria-label="Queue replacements" disabled={localBusy || active(state?.online_job) || mqaScopePending || !mqaSummaryCurrent || !mqaSummary!.counts.ready}
              title="Step 2: add selected MQA tracks with verified matches to the download queue as lossless replacements. Review and download them in Download queue."
              onClick={() => prepareMqaReplacements("queue_mqa")}>
              Queue replacements ({mqaSummaryCurrent ? mqaSummary!.counts.ready : "…"})
            </button>
          </div>
        </div>}
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
    if (key === "page_size" && (!curVal || curVal === "")) {
      curVal = 50;
    }
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
              setSettings((previous: any) => ({
                ...previous,
                [section]: {
                  ...previous[section],
                  [key]: number
                    ? e.target.value === ""
                      ? 0
                      : Number(e.target.value)
                    : e.target.value,
                },
              }))
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
      <label className="setting-row" title={hint}>
        <span>{label}</span>
        <input
          type="checkbox"
          checked={Boolean(settings[section]?.[key])}
          onChange={(e) => {
            updateSetting(section, key, e.target.checked);
          }}
        />

      </label>
    );
  }
  function settingsPage() {
    if (!settings) return <div className="card">Loading settings…</div>;
    return (
        <>
          <section className="card">
            <h2>Application & display</h2>
            {field("Colour theme", "general", "theme", [
              "system",
              "light",
              "dark",
            ])}
            {field(
              "Items per page",
              "general",
              "page_size",
              [
                { value: 25, label: "25" },
                { value: 50, label: "50" },
                { value: 100, label: "100" },
                { value: 250, label: "250" },
                { value: 500, label: "500" },
                { value: 1000, label: "1000" },
              ],
              true,
            )}
            <label className="setting-row">
              <span>Save activity logs</span>
              <input
                type="checkbox"
                checked={settings.general?.persist_logs !== false}
                onChange={(e) =>
                  updateSetting("general", "persist_logs", e.target.checked)
                }
              />
            </label>
          </section>
          {connectionSettings()}
          {downloadSettings()}
          <section className="card">
            <h2>Release matching &amp; recommendations</h2>
            <label className="setting-row" title="Treat a complete local standard or deluxe edition as owned when its artist and release title agree. Turn off to inspect each edition separately."><span>Treat a complete standard or deluxe edition as owned</span>
              <input type="checkbox" checked={settings.general?.treat_editions_as_owned !== false}
                onChange={(event) => updateSetting("general", "treat_editions_as_owned", event.target.checked)}/></label>
            {toggle("Include live releases", "general", "recommend_live", "Allow live recordings in recommendations. Studio and live recordings still require separate recording matches.")}
            <label className="setting-row" title="Include bootlegs/unofficial live recordings in match candidates and artist recommendations">
              <span>Include bootlegs and unofficial recordings</span>
              <input type="checkbox" checked={settings.general?.recommend_bootlegs === true}
                onChange={(event) => updateSetting("general", "recommend_bootlegs", event.target.checked)}/>
            </label>
            {toggle("Include compilations in matching and recommendations", "general", "recommend_compilations", "Allow compilation editions as automatic match candidates and recommendation candidates. Artist, recording and release checks still apply.")}
            {toggle("Allow fuzzy title matches for review", "general", "fuzzy_release_matching", "Consider related release titles for manual review. Similar titles alone never authorize a link or tag change.")}
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
                  onClick={() =>
                    setReview({
                      operation: "remove-root",
                      root: r.root,
                      rows: [],
                    })
                  }
                >
                  Remove
                </button>
              </div>
            ))}
          </section>
        </>
      );
  }
  function connectionSettings() {
    const diagnostics = Object.values(state?.diagnostics?.metrics || {}) as any[];
    const needsAttention = diagnostics.some(metric => !metric.ok);
    return <section className="card" id="connection-settings">
            <div className="section-heading"><h2>Connection</h2><span className={`status-badge ${state?.connections.account ? "status-complete" : "status-warning"}`}>{state?.connections.account ? needsAttention ? "Needs attention" : "Connected" : "Sign-in needed"}</span></div>

            <p className="muted">One sign-in for catalogue searches, favourites, track credits, artwork and downloads.</p>
            <div className="toolbar">
              <button
                className={state?.connections.account ? "" : "primary"}
                disabled={submitting || active(state?.online_job) || Boolean(state?.connections.account)}
                onClick={() => run("connect_account")}
              >
                Connect account
              </button>
              <button
                disabled={submitting || active(state?.online_job) || !state?.connections.account}
                onClick={() => mutate("account.disconnect")}
              >
                Disconnect account
              </button>
          <button className="connection-test" disabled={submitting || active(state?.online_job)} onClick={() => run("connections")}
            title={state?.online_job?.kind === "connections" && !active(state.online_job)
              ? Object.values(state?.diagnostics?.metrics || {}).map((metric: any) => metric.ok ? "Connected" : metric.message || "Needs attention").join(" · ") || state.online_job.message
              : "Test configured connections"}>
            <RefreshCw className={state?.online_job?.kind === "connections" && active(state.online_job) ? "spin" : ""} size={16} />
            {state?.online_job?.kind === "connections" && active(state.online_job) ? "Testing connections…" : "Test connections"}
            {state?.online_job?.kind === "connections" && !active(state.online_job) && <span className={state.online_job.status === "complete" && Object.values(state?.diagnostics?.metrics || {}).length > 0 && Object.values(state?.diagnostics?.metrics || {}).every((metric: any) => metric.ok) ? "connection-result ok" : "connection-result warning"} aria-label={state.online_job.status === "complete" && Object.values(state?.diagnostics?.metrics || {}).every((metric: any) => metric.ok) ? "Connection test passed" : "Connection test needs attention"}/>}
          </button>
            </div>
          {diagnostics.length > 0 && <details className="connection-details"><summary>Connection details</summary>
            {Object.entries(state?.diagnostics?.metrics || {}).map(([key, metric]: [string, any]) => <p key={key}><strong>{key === "download" ? "Account and downloads" : key === "catalogue" ? "Catalogue" : key}</strong> · {metric.ok ? "Ready" : "Needs attention"}{metric.latency_ms != null ? ` · ${metric.latency_ms} ms` : ""}{metric.message ? ` · ${metric.message}` : ""}</p>)}
          </details>}
        </section>;
  }
  function downloadSettings() {
    return (
      <>
        <section className="card">
          <h2>Download folder & layout</h2>
          <label className="setting-row">
            <span>Download folder</span>
            <input
              value={settings.downloads?.output || ""}
              onChange={(e) =>
                setSettings((previous: any) => ({
                  ...previous,
                  downloads: { ...previous.downloads, output: e.target.value },
                }))
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
          <details className="template-reference">
            <summary>Template reference & tag variables</summary>
            <div className="template-tokens">{[
              ["albumartist", "Album artist"], ["album", "Release title"], ["year", "Release year"],
              ["disc", "Disc folder, only for multiple discs"], ["disc_prefix", "Disc prefix, only for multiple discs"],
              ["tracknumber", "Padded track number"], ["title", "Track title"],
              ["artist", "Track artist"], ["discnumber", "Disc number"],
            ].map(([token, description]) => <span className="template-token" key={token} title={description}><code>{`{${token}}`}</code><small>{description}</small></span>)}</div>
          </details>
        </section>
        <section className="card">
          <h2>Audio and file options</h2>
          {field("Audio quality", "downloads", "quality", [
            { value: "HI_RES_LOSSLESS", label: "FLAC (24/192khz)" },
            { value: "LOSSLESS", label: "FLAC (16/44.1khz)" },
            { value: "HIGH", label: "AAC (320 kbps)" },
            { value: "LOW", label: "AAC (96 kbps)" },
          ])}
          {field("Embedded artwork size", "downloads", "cover_size", [
            { value: "1280", label: "1280 × 1280 px" },
            { value: "640", label: "640 × 640 px" },
            { value: "0", label: "Do not embed artwork" },
          ], true)}
          {toggle("Skip already downloaded files", "downloads", "skip_existing")}
          {toggle("Save companion cover.jpg to album folder", "downloads", "cover_album_file")}
          {toggle("Embed lyrics into audio files", "downloads", "lyrics_embed")}
          {toggle("Save separate .lrc lyrics file", "downloads", "lyrics_file")}
          {toggle("Create .m3u8 playlist file for albums", "downloads", "playlist_create")}
          {toggle("Write ReplayGain volume tags", "downloads", "replay_gain")}
          {field(
            "Parallel downloads",
            "provider",
            "download_concurrency",
            undefined,
            true,
          )}
          {field(
            "Connections per audio file",
            "provider",
            "segment_concurrency",
            undefined,
            true,
          )}
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
              disabled={busy || settingsSaving}
              onClick={async () => {
                const v = await mutate("settings.reset", {
                  group: "downloads",
                });
                if (v) setSettings(v);
              }}
            >
              Reset to default
            </button>
          </div>
        </section>
      </>
    );
  }
  async function unlinkArtists(artists: string[]) {
    setMenu(null);
    const result = await mutate("artists.unlink", {artists});
    if (result) { setSelected(new Set()); setToast("Artist links removed. File tags and recording links are unchanged."); }
  }
  function contextMenu() {
    if (!menu) return null;
    const r = menu.row;
    const replacementGroup = ["local", "online"].includes(route) && !!r.children?.length;
    const groupSelected = ["local", "online"].includes(route) && data.rows.some(parent =>
      selected.has(parent.id) && (parent.children || []).some((child: Row) => child.id === r.id));
    const ids: string[] = (route === "links" && (r.artist_group || r.link_group)) || r.file_group
      ? (selected.size ? [...selected] : linkedFiles([r]).map(child => child.id))
      : replacementGroup || selected.has(r.id) || groupSelected ? (selected.size ? [...selected] : [r.id]) : [r.id];
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
          {route === "artists" && <>
            <button role="menuitem" disabled={busy} onClick={() => {setMenu(null); run("match_artists", {artists: ids});}}>Recheck selected artists</button>
            <button role="menuitem" disabled={busy} onClick={() => unlinkArtists(ids)}>Unlink selected artists</button>
          </>}
          {["local", "online"].includes(route) && <button role="menuitem" disabled={busy} onClick={() => {
            setMenu(null);
            if (route === "local") run("review_consolidation", {ids, scope:"local"});
            else run("queue_replacements", {ids});
          }}>{route === "local" ? "Review duplicate removal" : "Queue selected replacement releases"}</button>}
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
                  `https://tidal.com/${r.parent ? "track" : route === "artists" || route === "favourites" ? "artist" : "album"}/${r.online_id}`,
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
          {["missing", "queue", "downloaded"].includes(route) && (
            <button role="menuitem" disabled={busy}
              title="Refresh track metadata and credits using your streaming account. Saved metadata is retained if the request fails."
              onClick={() => {run("release_details", {id:r.parent || r.id, force:true}); setMenu(null);}}>
              Refresh track details and credits
            </button>
          )}
          {["missing", "queue"].includes(route) && (
            <button role="menuitem" disabled={busy}
              title="Recheck this release in your configured market, replacing its saved availability check."
              onClick={() => {run("check_availability", {ids:[r.parent || r.id], force:true}); setMenu(null);}}>
              Check release availability
            </button>
          )}
          {route === "downloaded" && (
            <>
              <hr />
              <button role="menuitem" disabled={queueBusy || active(state?.download_job)} onClick={() => {
                mutate("queue.redownload", { release_id: r.parent || r.id, ...(r.parent ? { track_id: r.id } : {}) });
                setMenu(null);
              }}>{r.parent ? "Redownload track" : "Redownload release"}</button>
            </>
          )}
          {!r.parent && ["queue", "missing"].includes(route) && (
            <>
              <hr />
              <button
                role="menuitem"
                disabled={queueBusy}
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
                disabled={queueBusy}
                onClick={() => {
                  mutate("queue.decision", { ids, decision: "ignored" });
                  setMenu(null);
                }}
              >
                Ignore selected releases
              </button>
              <button
                role="menuitem"
                disabled={queueBusy}
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
    <div className={"app" + (sidebarVisible ? "" : " sidebar-hidden")}>
      <div className="drag-region" data-tauri-drag-region>
        <nav className="window-navigation" aria-label="Page navigation">
          <button aria-label="Back" title="Back" disabled={navigation.index === 0} onClick={() => moveHistory(-1)}><ChevronLeft size={18}/></button>
          <button aria-label="Forward" title="Forward" disabled={navigation.index === navigation.pages.length - 1} onClick={() => moveHistory(1)}><ChevronRight size={18}/></button>
          <button aria-label={sidebarVisible ? "Hide sidebar" : "Show sidebar"} title={sidebarVisible ? "Hide sidebar" : "Show sidebar"} aria-expanded={sidebarVisible} aria-controls="app-sidebar" onClick={() => { localStorage.setItem("tibrary.sidebar", sidebarVisible ? "hidden" : "visible"); setSidebarVisible(!sidebarVisible); }}><PanelLeft size={18}/></button>
        </nav>
      </div>
      <aside id="app-sidebar" hidden={!sidebarVisible}>
        <nav className="sidebar-navigation" aria-label="Library navigation">
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
                onClick={() => {
                  setRoute(g.id);
                  setCollapsed(previous => { const next = new Set(previous); next.delete(g.id); return next; });
                }}
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
                  {id === "activity" && (active(state?.job) || active(state?.online_job) || active(state?.download_job)) && (
                    <span className="live-dot" />
                  )}
                </button>
              ))}
          </div>
        ))}
        </nav>
        <div className="sidebar-bottom">
          <select
            aria-label="Active library"
            title={root || "Choose the library used across the app"}
            value={root}
            onChange={(e) => setRoot(e.target.value)}
          >
            <option value="">{!state ? "Loading libraries…" : state.roots.length ? "Choose library" : "Add a library to begin"}</option>
            {state?.roots.map((r) => (
              <option key={r.root} value={r.root}>{r.root.split("/").pop()}</option>
            ))}
          </select>
          <span className="sidebar-version">v{appVersion}</span>
        </div>
      </aside>
      <main aria-label={titles[route] || "Overview"}>
        {[state?.job, state?.online_job, state?.download_job].some(job => active(job)) && (
          <header className="page-header">
              <div className="header-workloads" role="status" aria-live="off">
                {[state?.job, state?.online_job, state?.download_job].filter(job => active(job)).map((job) => {
                  const progress = workload(job, downloadMonitor, clock);
                  return <button className="header-workload" aria-label={`Show activity: ${jobTitle(job?.kind)}`} key={job!.id} title={`${job?.message} · ${progress?.label}`} onClick={() => setRoute("activity")}>
                    <LoaderCircle className="spin" size={13}/>
                    <span>{jobTitle(job?.kind)} · {progress?.label}</span>
                    {progress?.percent != null && <span className="header-workload-bar" style={{width: `${progress.percent}%`}}/>}
                  </button>;
                })}
              </div>
          </header>
        )}
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
        {(() => {
          const isScrollPage = route === "general";

          return (
            <div className={"page-body-wrap" + (isScrollPage ? " has-scroll-fade" : "")}>
              <div
                className={
                  "page-body " + (route === "overview" ? "overview-page" : isScrollPage ? "scroll-page" : "table-page")
                }
              >
          {route === "overview" ? (
            overviewPage()
          ) : ["general", "connections", "downloads"].includes(route) ? (
            settingsPage()
          ) : route === "activity" ? (
            <ActivityView
              logs={state?.logs || []}
              monitor={downloadMonitor}
              job={state?.job}
              onlineJob={state?.online_job}
              downloadJob={state?.download_job}
              onClear={async (stream) => {
                const cleared = await call("logs.clear", {stream});
                mergeActivityEpochs(cleared.activity_epochs);
                if (stream === "downloads" || stream === "all") setDownloadMonitor({});
                setState(old => old ? {...old, activity_epochs:cleared.activity_epochs || old.activity_epochs, logs: stream === "all" ? [] : old.logs.filter(entry => streamFor(entry) !== stream)} : old);
              }}
              onCancel={(kind) => call("job.cancel", {kind}).catch(notifyError)}
            />
          ) : (
            tablePage()
          )}
        </div>
        {isScrollPage && (
          <>
            <div className="edge-gradient-top" aria-hidden="true" />
            <div className="edge-gradient-bottom" aria-hidden="true" />
          </>
        )}
      </div>
    );
  })()}
      </main>
      {contextMenu()}
      {detail && (
        <Modal
          title={
            detail.operation ? route === "online" ? "Replacement release details" : "Local duplicate details"
              : detail.local_file_release ? "Release review" : detail.local_artist ? "Artist release links" : detail.local_release ? "Local release details" : detail.artist && !detail.tags
              ? "Artist candidates"
              : detail.release
                ? "Release details"
                : detail.folder_target ? "Folder layout preview" : "Recording & release match"
          }
          wide
          onClose={() => setDetail(null)}
        >
          <div className="modal-body">
            {detail.local_file_release && <section className="metadata-source">
              <h3>{detail.local_file_release.artist} — {detail.local_file_release.release}</h3>
              <p>{detail.local_file_release.path}</p><p>{detail.local_file_release.evidence}</p>
              <button onClick={() => reveal(detail.local_file_release.path).catch(notifyError)}>Show in Finder</button>
              <div className="metadata-table"><table aria-label={`Files in ${detail.local_file_release.release}`}>
                <thead><tr><th>Track</th><th>Disc · track</th><th>Status</th><th>Evidence</th><th>Proposed action</th><th /></tr></thead>
                <tbody>{(detail.local_file_release.children || []).map((track: Row) => <tr key={track.id}>
                  <td title={track.path}>{track.title}</td><td>{track.position}</td>
                  <td><span className={"badge " + String(track.status).toLowerCase().replaceAll(" ", "-")}>{track.status}</span></td>
                  <td>{track.evidence}</td><td title={track.target || undefined}>{readable(track.changes && Object.keys(track.changes).length ? track.changes : track.target)}</td>
                  <td><button onClick={() => loadDetail(track)}>View file</button></td>
                </tr>)}</tbody>
              </table></div>
            </section>}
            {detail.local_artist && <section className="metadata-source">
              <h3>{detail.local_artist.artist}</h3>
              <p>{detail.local_artist.evidence}</p>
              <div className="metadata-table"><table aria-label={`Releases by ${detail.local_artist.artist}`}>
                <thead><tr><th>Release</th><th>Tracks</th><th>Status</th><th>Evidence</th><th/></tr></thead>
                <tbody>{(detail.local_artist.children || []).map((release: Row) => <tr key={release.id}>
                  <td>{release.release}</td><td>{release.tracks}</td>
                  <td><span className={"badge " + String(release.status).toLowerCase().replaceAll(" ", "-")}>{release.status}</span></td>
                  <td title={release.evidence}>{release.evidence}</td>
                  <td><button onClick={() => loadDetail(release)}>View release</button></td>
                </tr>)}</tbody>
              </table></div>
            </section>}
            {detail.local_release && <section className="metadata-source">
              <h3>{detail.local_release.artist} — {detail.local_release.release}</h3>
              <p>{detail.local_release.path}</p><p>{detail.local_release.evidence}</p>
              <p>{detail.local_release.tracks} local tracks · {detail.local_release.status}</p>
              <button onClick={() => reveal(detail.local_release.path).catch(notifyError)}>Show in Finder</button>
              {detail.local_release.children?.length > 0 && <div className="metadata-table"><table aria-label={`Tracks in ${detail.local_release.release}`}>
                <thead><tr><th>Track</th><th>Disc · track</th><th>Status</th><th>Evidence</th><th/></tr></thead>
                <tbody>{(detail.local_release.children || []).map((track: Row) => <tr key={track.id}>
                  <td>{track.title}</td><td>{track.position}</td><td><span className={"badge " + String(track.status).toLowerCase().replaceAll(" ","-")}>{track.status}</span></td>
                  <td title={track.evidence}>{track.evidence || "Not checked"}</td>
                  <td><button onClick={() => loadDetail(track)}>Choose match</button></td>
                </tr>)}</tbody>
              </table></div>}
            </section>}
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
                {detail.folder_target && <section className="metadata-source"><h3>Proposed folder layout</h3><p>{detail.folder_evidence}</p><dl><dt>Current path</dt><dd style={{whiteSpace:"break-spaces"}}>{detail.path}</dd><dt>Proposed path</dt><dd style={{whiteSpace:"break-spaces"}}>{detail.folder_target}</dd></dl><p>{detail.folder_target === detail.path ? "This file already follows the saved folder template. No change is proposed." : "Only the file location or filename changes. Tags remain unchanged."}</p></section>}
                {detail.proposed && Object.keys(detail.proposed).length > 0 && <section><h3>Proposed changes</h3>
                  {detail.source_release_id && <p>Online source · Release {detail.source_release_id} · Track {detail.source_track_id}</p>}
                  <TagChanges changes={detail.proposed} current={detail.tags}/></section>}
                {!detail.folder_target && <><h3>{detail.catalogue_options?.some((option: any) => option.recording_absent) ? "Inspected releases and placements" : "Available placements"}</h3>
                {detail.availability_note && <><p>{detail.availability_note}</p><button disabled={detail.availability_checking} onClick={async () => {
                  const path = detail.path;
                  setDetail({...detail, availability_checking:true});
                  try { const refreshed = await call("detail", {root,path,check_availability:true,force_availability:true}); setDetail((current:any) => current?.path === path ? refreshed : current); }
                  catch (error) { notifyError(error); setDetail((current:any) => current?.path === path ? {...current,availability_checking:false} : current); }
                }}>{detail.availability_checking ? "Checking availability…" : "Recheck availability"}</button></>}
                {detail.catalogue_options?.length ? (
                  detail.catalogue_options.map((o: any, i: number) => {
                    const albumId = placementId(o.id) || placementId(o.album_id);
                    const trackId = placementId(o.track_id);
                    return (
                    <article
                      className="candidate"
                      key={i}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        if (albumId) external(`https://tidal.com/album/${albumId}`).catch(
                          notifyError,
                        );
                      }}
                    >
                      <div>
                        <strong>
                          {o.artist} — {o.album || o.title || "Release candidate"}
                        </strong>
                        <p>
                          {o.position_label || "Position not yet verified"} · Release ID {albumId || "Unavailable"}
                        </p>
                        {!trackId && <p>{o.recording_absent ? "This edition’s saved audio track list omits this recording." : "Release candidate; inspect it to find this track’s placement."}</p>}
                        <p>{o.evidence}</p>
                        {(o.structure?.compatible || (o.structure?.reason || o.structure?.reasons) && readable(o.structure?.reason || o.structure?.reasons) !== readable(o.evidence)) && <p
                          className={
                            o.structure?.compatible ? "success" : "warning"
                          }
                        >
                          {o.structure?.compatible
                            ? "Structure verified"
                            : readable(
                                o.structure?.reason ||
                                  o.structure?.reasons ||
                                  "Structural difference; review carefully",
                              )}
                        </p>}
                      </div>
                      <div className="toolbar">
                        <button
                          disabled={!albumId}
                          onClick={() =>
                            external(`https://tidal.com/album/${albumId}`).catch(
                              notifyError,
                            )
                          }
                        >
                          Open on web <ArrowUpRight size={14} />
                        </button>
                        <button
                          disabled={busy || !albumId || !detail.path}
                          title={trackId ? "Link only this track. Other tracks and file tags stay unchanged." : "Inspect the selected release for this track; no link is saved."}
                          onClick={async () => {
                            if (!trackId) {
                              await run("manual_candidate", {ids:[detail.path], album_id:albumId});
                              return;
                            }
                            if (
                              await mutate("tracks.choose", {
                                root,
                                path: detail.path,
                                album_id: albumId,
                                require_live: true,
                                track_id: trackId,
                                choice_key: o.choice_key,
                                scope: "track",
                              })
                            )
                              setDetail(null);
                          }}
                        >
                          {trackId ? "Use this placement" : "Inspect release"}
                        </button>
                      </div>
                    </article>
                  );})
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
                  {detail.linked_ids?.album_id && <button disabled={busy}
                    title="Refresh this release using your streaming account."
                    onClick={() => run("release_details", {id:detail.linked_ids.album_id, force:true})}>
                    Refresh track details and credits
                  </button>}
                </div>
                </>}
                <details>
                  <summary>{detail.folder_target ? "Saved file tags" : "All saved tags & DJ checks"}</summary>
                  <h4>Local file tags</h4>
                  <MetadataView value={detail.tags} title="Local file tags"/>
                  {!detail.folder_target && <><h4>Online metadata by release</h4>
                  <MetadataView value={detail.dj_checks} title="Online source"/></>}
                  {detail.catalogue_note && <p>{readable(detail.catalogue_note)}</p>}

                </details>
              </>
            )}
            {detail.operation && <section>
              <h3>{detail.operation.artist} — {detail.operation.release}</h3>
              <p>{detail.operation.evidence}</p>
              <dl>
                {detail.operation.path && <><dt>Local folder</dt><dd>{detail.operation.path}</dd></>}
                {detail.operation.target && <><dt>Retained / replacement destination</dt><dd>{detail.operation.target}</dd></>}
                <dt>Local duplicate files</dt><dd>{detail.operation.duplicates}</dd>
              </dl>
              {detail.operation.online_id && <button onClick={() => external(`https://tidal.com/album/${detail.operation.online_id}`).catch(notifyError)}>Open on web</button>}
              {!!detail.operation.children?.length && <div className="metadata-table"><table aria-label="Affected local releases">
                <thead><tr><th>Local release</th><th>Tracks</th><th>Folder</th><th>Evidence</th><th /></tr></thead>
                <tbody>{detail.operation.children.map((child: Row) => <tr key={child.id}>
                  <td>{child.release}</td><td>{child.tracks}</td><td title={child.path}>{child.path}</td><td>{child.evidence}</td>
                  <td><button onClick={() => loadDetail(child)}>View release</button></td>
                </tr>)}</tbody>
              </table></div>}
            </section>}
            {detail.artist && !detail.tags && (
              <>
                <p>
                  Choose one or more confirmed artist IDs. Existing mappings
                  remain until you save.
                </p>
                {!!detail.local_files?.length && <section className="metadata-source"><h3>Local recordings</h3>
                  <p>Recording links can identify a release despite an incorrect album artist. This does not change file tags or assign a different artist identity.</p>
                  <div className="table-wrap"><table className="metadata-table"><thead><tr><th>Track</th><th>Release</th><th>Performer credits</th><th>ISRC</th></tr></thead><tbody>{detail.local_files.map((f: any) => <tr key={f.path}><td>{f.title}</td><td>{f.release}</td><td>{f.performers}</td><td>{f.isrc || "Not saved"}</td></tr>)}</tbody></table></div>
                  <button disabled={busy} onClick={async () => { await run("link", { ids: detail.local_files.map((f: any) => f.path) }); setDetail(null); }}>Check these recording links</button>
                </section>}
                {(detail.review?.candidates || []).map((c: any, i: number) => (
                  <article className="candidate" key={i}>
                    <div>
                      <strong>{c.name || c.artist?.name}</strong>
                      <p>{readable(c.evidence || c.reason || "No additional evidence saved")}</p>
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
                <button disabled={busy} onClick={() => run("release_details", {id:detail.release.id})}
                  title="Reuse saved track details and credits, fetching only missing or changed evidence for this release. Files are not changed.">
                  Get missing metadata
                </button>
                {!!detail.evidence?.length && <section><h3>Recommendation evidence</h3><ul>{detail.evidence.map((line: string, index: number) => <li key={index}>{line}</li>)}</ul></section>}
                <ReleaseMetadata release={detail.release} track={detail.track}/>
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
              {review.operation === "download" ? <DownloadReview rows={review.rows || []}/> : review.rows?.map((r: any, i: number) => (
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
                    review.operation === "organise" ? <section className="metadata-source">
                      <dl><dt>Current path</dt><dd style={{whiteSpace:"break-spaces",overflowWrap:"anywhere"}}>{r.path}</dd>
                      <dt>Proposed path</dt><dd style={{whiteSpace:"break-spaces",overflowWrap:"anywhere"}}>{r.target || r.path}</dd></dl>
                      <small>File tags stay unchanged.</small>
                    </section> : <><p>{r.path || r.target}</p>{r.changes && Object.keys(r.changes).length > 0 && <TagChanges changes={r.changes} current={r.tags}/>}</>
                  )}
                  {r.source_release_id && <p>Online source · Release {r.source_release_id} · Track {r.source_track_id}</p>}
                  {r.evidence && <small style={{ display: "block", marginTop: "4px" }}>{readable(r.evidence)}</small>}
                  {r.reviewed_dj_conflicts?.length > 0 && (
                    <MetadataView value={r.reviewed_dj_conflicts} title="Track comparison"/>
                  )}
                </article>
              ))}
            </div>
          </div>
          <footer>
            <button onClick={() => setReview(null)}>Cancel</button>
            <button className="primary" disabled={review.operation === "download"
              ? downloadBusy || !review.rows?.length
              : review.operation === "component_update"
                ? submitting || active(state?.online_job)
                : localBusy || active(state?.download_job)} onClick={confirmReview}>
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
      {exportPreview && (
        <Modal
          title={exportPreview.title}
          wide
          onClose={() => setExportPreview(null)}
        >
          <div className="modal-body">
            <p>One link per line, ready to use in an external downloader. Whole releases use release links; individually approved tracks use track links.</p>
            {exportPreview.content ? <textarea className="export-links" aria-label="Download links" value={exportPreview.content} readOnly spellCheck={false}/>
              : <p className="muted">No approved music to export.</p>}
          </div>
          <footer>
            <button
              disabled={!exportPreview.content}
              onClick={() => {
                navigator.clipboard.writeText(exportPreview.content);
                setToast("Copied to clipboard");
                setTimeout(() => setToast(""), 2000);
              }}
              title="Copy export content"
            >
              <Copy size={14} />
              Copy
            </button>
            <button
              className="primary"
              disabled={!exportPreview.content}
              onClick={async () => {
                const path = await save({
                  defaultPath: exportPreview.filename,
                  filters: [{ name: "Text", extensions: ["txt"] }],
                });
                if (path) {
                  await invoke("save_export", {
                    path,
                    content: exportPreview.content,
                  });
                  setToast("Saved to " + path.split("/").pop());
                  setTimeout(() => setToast(""), 2000);
                  setExportPreview(null);
                }
              }}
              title="Save export to file"
            >
              <Download size={14} />
              Save
            </button>
            <button onClick={() => setExportPreview(null)}>Close</button>
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
        <p>Background work continues independently of this window.</p>
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
