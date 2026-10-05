import { useEffect, useMemo, useState, useRef, memo } from "react";
import { Copy, Trash2 } from "lucide-react";
import { call, active, Job, Row, onlineKinds } from "./api";

export type ActivityEntry = {
  at: string;
  updated_at?: string;
  saved_id?: number;
  message: string;
  category?: string;
  level?: string;
  progress_id?: string;
  job_id?: string;
  job_kind?: string;
  job_status?: string;
};
export type ActivityStream = "online" | "local" | "downloads";

export function streamFor(entry: ActivityEntry): ActivityStream {
  // A metadata request made during a local task still belongs to that task.
  // Error categories and incidental words must not mislabel its worker.
  if (entry.job_kind === "download") return "downloads";
  if (entry.job_kind && onlineKinds.has(entry.job_kind)) return "online";
  if (entry.job_kind) return "local";
  const category = (entry.category || "").toLowerCase();
  const message = entry.message.toLowerCase();
  if (category === "download") return "downloads";
  if (category === "linking" || category === "online") return "online";
  if (category === "scan" || category === "cleanup" || category === "local") return "local";
  if (/download|fetching track|saving track/.test(message)) return "downloads";
  if (/catalogue|api|remote|artist search|releases|metadata source/.test(message)) return "online";
  return "local";
}

const actionTitles: Record<string, string> = {discography: "Refresh releases", favourites: "Sync favourite artists", release_artists: "Check release artists", cached_releases: "Recheck cached releases", check_availability: "Check release availability", link: "Link releases", match_artists: "Match artists", preview: "Review local changes", mqa: "MQA audit", queue_mqa: "Queue MQA replacements", queue_replacements: "Queue online replacements", check_replacements: "Check online replacements", release_details: "Fetch track details and credits", release_tracks: "Load track details", metadata: "Find missing tags", artwork: "Find artwork", local_duplicates: "Check local duplicates", optimizations: "Check replacements", download: "Download music", scan: "Scan library", apply: "Apply reviewed changes", deep_review: "Find release matches", deep_preview: "Review release links", deep_apply: "Save reviewed release links", review_consolidation: "Review duplicate removal", consolidate: "Remove reviewed duplicates", manual_candidate: "Inspect a release match", connections: "Test connection", connect_account: "Connect account", connect_download: "Connect account", component_check: "Check streaming components", component_update: "Update streaming components", component_rollback: "Restore streaming components"};

export function jobTitle(kind?: string): string {
  return actionTitles[kind || ""] || kind?.replaceAll("_", " ") || "Task";
}

export function activityFilterParameters(filter: string): {stream?: ActivityStream; job_kinds?: string[]; unassigned?: true} {
  if (filter === "all") return {};
  if (filter.startsWith("stream:")) return {stream:filter.slice(7) as ActivityStream};
  if (filter === "application") return {unassigned:true};
  return {job_kinds:filter === "connect_account" ? ["connect_account","connect_download"] : [filter]};
}

export function matchesActivityFilter(entry: ActivityEntry, filter: string): boolean {
  const parameters = activityFilterParameters(filter);
  if (parameters.stream) return streamFor(entry) === parameters.stream;
  if (parameters.unassigned) return !entry.job_kind;
  return !parameters.job_kinds || parameters.job_kinds.includes(entry.job_kind || "");
}

const actionGroups = (["local","online","downloads"] as ActivityStream[]).map(stream => ({
  label:stream === "downloads" ? "Download actions" : `${stream === "local" ? "Local" : "Online"} actions`,
  actions:Object.keys(actionTitles).filter(kind => kind !== "connect_download" && streamFor({at:"",message:"",job_kind:kind}) === stream)
    .sort((a,b) => jobTitle(a).localeCompare(jobTitle(b))),
}));

function stamp(at: string): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString(undefined, { hour12: false });
}

function formatDuration(seconds: number): string {
  const n = Math.max(0, Math.round(seconds));
  if (n >= 3600) return `${Math.floor(n / 3600)}h ${Math.floor(n % 3600 / 60)}m`;
  if (n >= 60) return `${Math.floor(n / 60)}m ${n % 60}s`;
  return `${n}s`;
}

export function workload(job: Job | null | undefined, monitor: Record<string, Row>, now: number) {
  if (!active(job)) return null;
  let done = job?.completed ?? 0;
  let total = job?.total ?? 0;
  if (job?.kind === "download" && !total) {
    const batches = Object.values(monitor).filter(row => row.kind === "batch" && (!row.job_id || row.job_id === job?.id));
    done = batches.reduce((n, row) => n + Number(row.completed_tracks || 0), 0);
    total = batches.reduce((n, row) => n + Number(row.total_tracks || 0), 0);
  }
  if (!total || !Number.isFinite(total)) return { label: "Measuring workload · ETA estimating…", percent: null as number | null };
  done = Math.min(total, Math.max(0, done));
  const stale = job?.progress_updated_at != null && now / 1000 - job.progress_updated_at > 15;
  const eta = job?.status === "cancelling" ? "Cancelling…" : stale ? "Waiting for next result · ETA updating…" : job?.eta_seconds != null ? `~${formatDuration(job.eta_seconds)} remaining` : "ETA estimating…";
  return { label: `${compactProgress(done / total * 100)} · ${done.toLocaleString()}/${total.toLocaleString()} · ${eta}`, percent: done / total * 100 };
}

export function compactProgress(percent: number | null | undefined): string {
  if (percent == null) return "Working";
  if (percent === 0) return "0%";
  if (percent < 1) return "<1%";
  return `${Math.floor(percent)}%`;
}

const LogRow = memo(function LogRow({ entry }: { entry: ActivityEntry }) {
  const stream = streamFor(entry);
  const category = stream === "downloads" ? "download" : stream;
  return <div className={`log-row log-${category} ${entry.level === "error" ? "log-error" : ""}`} data-job-id={entry.job_id}>
    <time title={stamp(entry.at)}>{new Date(entry.at).toLocaleTimeString()}</time>
    <span className={`log-badge log-badge-${category}`}>{stream === "downloads" ? "DOWNLOAD" : stream.toUpperCase()}</span>
    <span className="log-job" title={entry.job_kind ? jobTitle(entry.job_kind) : "Application"}>{entry.job_kind ? jobTitle(entry.job_kind) : "Application"}</span>
    <span className="log-message">{entry.message}</span>
  </div>;
});

const terminal = new Set(["complete", "failed", "cancelled", "interrupted"]);
const entryKey = (entry: ActivityEntry) => entry.progress_id ? `progress:${entry.progress_id}` : `${entry.job_id || ""}:${entry.at}:${entry.message}`;
const compareTime = (a: string, b: string) => (Date.parse(a) || 0) - (Date.parse(b) || 0) || a.localeCompare(b);
const updateTime = (entry: ActivityEntry) => entry.updated_at || entry.at;

export function retireWorkerProgress(entries: ActivityEntry[]): ActivityEntry[] {
  const finished = new Set(entries.filter(entry => entry.job_id && !entry.progress_id && terminal.has(entry.job_status || "")).map(entry => entry.job_id));
  // Whole-job progress ends with the worker. Per-artist actions have their own
  // progress identity and remain in the persisted chronological log.
  return entries.filter(entry => !entry.progress_id || !finished.has(entry.progress_id));
}

// A snapshot can finish after a newer event, or temporarily omit a busy channel.
// Merge it with observed rows; explicit Clear still removes the channel in App.
export function mergeActivitySnapshot(previous: ActivityEntry[], incoming: ActivityEntry[], previousEpochs: number[] = [], incomingEpochs: number[] = []): ActivityEntry[] {
  const streamIndex = (entry: ActivityEntry) => ["online", "local", "downloads"].indexOf(streamFor(entry));
  const rows = new Map<string, ActivityEntry>();
  for (const entry of previous) {
    const index = streamIndex(entry);
    if ((incomingEpochs[index] || 0) <= (previousEpochs[index] || 0)) rows.set(entryKey(entry), entry);
  }
  for (const entry of incoming) {
    const index = streamIndex(entry);
    if ((incomingEpochs[index] || 0) < (previousEpochs[index] || 0)) continue;
    const key = entryKey(entry), old = rows.get(key);
    if (!old || compareTime(updateTime(old), updateTime(entry)) <= 0) rows.set(key, entry);
  }
  const streams: Record<ActivityStream, ActivityEntry[]> = {online: [], local: [], downloads: []};
  for (const entry of retireWorkerProgress([...rows.values()]).sort((a, b) => compareTime(a.at,b.at))) {
    streams[streamFor(entry)].push(entry);
  }
  // Keep a bounded, chronological live window. Older actions come from the
  // persisted log when the user scrolls; jobs do not need separate summaries.
  return Object.values(streams).flatMap(rows => rows.slice(-1000)).sort((a,b) => compareTime(a.at,b.at));
}

const completeDownload = (status: string) => status === "complete" || status === "already downloaded";

/** Archive pages and live snapshots share an identity; a late reply must never
 * replace newer live progress or put a finished job back into a running state. */
export function mergeActivityHistory(previous: ActivityEntry[], incoming: ActivityEntry[]): ActivityEntry[] {
  const rows = new Map<string, ActivityEntry>();
  for (const entry of [...previous, ...incoming]) {
    const key = entryKey(entry), old = rows.get(key);
    if (!old || compareTime(updateTime(old), updateTime(entry)) <= 0) {
      const next = {...old, ...entry};
      if (!next.saved_id && old?.saved_id) next.saved_id = old.saved_id;
      rows.set(key,next);
    }
  }
  return [...rows.values()].sort((a,b) => compareTime(b.at,a.at) || (b.saved_id || 0) - (a.saved_id || 0));
}

export function downloadActivity(monitor: Record<string, Row>, job?: Job | null): ActivityEntry[] {
  return Object.entries(monitor).map(([id,item]) => {
    const status = item.status === "staged" ? "Writing tags" : String(item.status || "Waiting");
    const tracks = item.kind === "batch"
      ? `${item.completed_tracks || 0}/${item.total_tracks || 0} tracks`
      : `Track ${item.index || 0}/${item.total_tracks || 0} · ${compactProgress(Number(item.percent || 0))}`;
    const children = item.kind === "batch" ? Object.values(monitor).filter(track => track.kind === "track" && track.release_id === item.release_id && track.job_id === item.job_id) : [];
    const bytes = children.length ? children.reduce((sum,track) => sum + Number(track.bytes || 0),0) : Number(item.bytes || 0);
    const total = children.length ? children.reduce((sum,track) => sum + Number(track.estimated_total_bytes || track.total_bytes || track.bytes || 0),0) : Number(item.estimated_total_bytes || item.total_bytes || 0);
    const size = `${(bytes / 1048576).toFixed(1)} MB${total > bytes ? ` / ~${(total / 1048576).toFixed(1)} MB` : ""}`;
    const speed = item.bytes_per_second ? `${(item.bytes_per_second / 1048576).toFixed(1)} MB/s` : "";
    const eta = item.eta_seconds != null ? `~${formatDuration(Number(item.eta_seconds))} remaining` : "";
    const title = item.kind === "batch" ? `${item.artist || "Artist"} — ${item.release || "Release"}` : item.title || `Track ${item.id}`;
    const time = Number(item.updated_at || 0);
    const at = item.at || new Date(time ? time < 1e12 ? time * 1000 : time : (job?.started || 0) * 1000).toISOString();
    return {at, message:[title, status, tracks, size, speed, eta, item.error].filter(Boolean).join(" · "), category:"download", level:item.status === "failed" ? "error" : "info", progress_id:`download:${id}`, job_id:String(item.job_id || job?.id || ""), job_kind:"download", job_status:completeDownload(item.status) ? "complete" : item.status};
  });
}

export function mergeDownloadMonitor(previous: Record<string, Row>, incoming: Record<string, Row>): Record<string, Row> {
  const result = {...previous};
  let changed=false;
  for (const [key, item] of Object.entries(incoming)) {
    const old = result[key];
    if (old && (terminal.has(old.status) && !terminal.has(item.status) || Number(old.updated_at || 0) > Number(item.updated_at || 0) || Number(old.bytes || 0) > Number(item.bytes || 0))) continue;
    if (old && Object.keys(item).every(field=>old[field] === item[field])) continue;
    result[key] = {...old, ...item};
    changed=true;
  }
  if (!changed) return previous;
  if (Object.keys(result).length <= 1000) return result;
  return Object.fromEntries(Object.entries(result).sort(([,a],[,b]) => Number(b.updated_at || 0)-Number(a.updated_at || 0)).slice(0,1000));
}

function WorkerStatus({ title, job, monitor, onCancel }: { title: string; job?: Job | null; monitor: Record<string, Row>; onCancel: (kind: string) => void }) {
  const [completedId, setCompletedId] = useState("");
  useEffect(() => {
    if (!job || job.historical || active(job) || job.status !== "complete") {
      setCompletedId("");
      return;
    }
    const remaining = 3000 - (Date.now() - Number(job.finished || 0) * 1000);
    if (remaining <= 0) { setCompletedId(""); return; }
    setCompletedId(job.id);
    const timer = window.setTimeout(() => setCompletedId(""), remaining);
    return () => window.clearTimeout(timer);
  }, [job?.id, job?.status, job?.finished, job?.historical]);
  const running = active(job);
  const [now, setNow] = useState(Date.now());
  useEffect(() => { if (!running) return; const timer = window.setInterval(() => setNow(Date.now()), 1000); return () => window.clearInterval(timer); }, [running]);
  const progress = workload(job, monitor, now);
  const detail = running ? (job?.message || "Working") : completedId === job?.id ? "Task complete" : "Awaiting task...";
  return <section className="card activity-status" aria-label={`${title} worker`}>
    <div><small>{title}</small><h2>{running ? jobTitle(job?.kind) : detail}</h2>
      {running && <><p title={job?.message}>{job?.message}</p><p title={progress?.label} className="job-progress-label">{progress?.label}</p><progress aria-label={`${title} progress`} max={100} value={progress?.percent ?? undefined}/></>}
    </div>
    {running && <button disabled={job?.status === "cancelling"} onClick={() => onCancel(job!.kind)}>{job?.status === "cancelling" ? "Cancelling…" : "Cancel task"}</button>}
  </section>;
}

type HistoryPage = { entries: ActivityEntry[]; next_before_id: number | null };

export function ActivityView({ logs, monitor, job, onlineJob, downloadJob, onClear, onCancel }: {
  logs: ActivityEntry[];
  monitor: Record<string, Row>;
  job?: Job | null;
  onlineJob?: Job | null;
  downloadJob?: Job | null;
  onClear: (stream: ActivityStream | "all") => Promise<void>;
  onCancel: (kind: string) => void;
}) {
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const [actionFilter, setActionFilter] = useState("all");
  const [saved, setSaved] = useState<ActivityEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [clearing, setClearing] = useState(false);
  const [historyRevision, setHistoryRevision] = useState(0);
  const sentinel = useRef<HTMLDivElement>(null);
  const requestGeneration = useRef(0);
  const recentWork = useRef(0);
  const working = active(job) || active(onlineJob) || active(downloadJob);
  if (working) recentWork.current = Date.now();
  useEffect(() => { const timer = window.setTimeout(() => setQuery(search.trim()), 250); return () => window.clearTimeout(timer); }, [search]);
  useEffect(() => {
    let disposed = false, requesting = false, before: number | null = null, initialized = false;
    const generation = ++requestGeneration.current;
    const current = () => !disposed && generation === requestGeneration.current;
    const marker = sentinel.current;
    const root = marker?.closest(".activity-log") || null;
    async function load(older: boolean) {
      if (!current() || requesting || older && (!initialized || before == null)) return;
      requesting = true;
      let loaded = false;
      setLoading(true);
      try {
        const page = await call<HistoryPage>("logs.history", {limit:500, search:query || undefined, ...activityFilterParameters(actionFilter), ...(older ? {before_id:before} : {})});
        if (!current()) return;
        setSaved(old => mergeActivityHistory(old, page.entries));
        if (older || !initialized) before = page.next_before_id;
        initialized = true;
        loaded = true;
        setError("");
      } catch (e) { if (current()) setError(String(e)); }
      finally {
        requesting = false;
        if (current()) {
          setLoading(false);
          // IntersectionObserver may already consider the sentinel visible
          // before the first page arrives. Recheck after React lays out rows.
          window.requestAnimationFrame(() => {
            if (!loaded || !current() || before == null || !marker || !root) return;
            const tail = marker.getBoundingClientRect(), viewport = root.getBoundingClientRect();
            if (tail.top <= viewport.bottom + 200 && tail.bottom >= viewport.top) void load(true);
          });
        }
      }
    }
    setSaved([]);
    if (root) root.scrollTop = 0;
    void load(false);
    // Read only the latest page while a worker is active. Live snapshots remain
    // visible immediately; buffered persistence and page loads never clear it.
    const timer = window.setInterval(() => { if (Date.now() - recentWork.current < 15000) void load(false); }, 5000);
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) void load(true);
    }, {root, rootMargin:"200px"});
    if (marker) observer.observe(marker);
    return () => { disposed = true; observer.disconnect(); window.clearInterval(timer); };
  }, [query,actionFilter,historyRevision]);
  const entries = useMemo(() => {
    const all = mergeActivityHistory(saved, logs);
    return mergeActivityHistory(retireWorkerProgress(all), downloadActivity(monitor,downloadJob));
  }, [saved,logs,monitor,downloadJob]);
  const filtered = useMemo(() => entries.filter(entry => matchesActivityFilter(entry,actionFilter) && (!search.trim() || `${entry.message} ${jobTitle(entry.job_kind)} ${streamFor(entry)}`.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()))), [entries,search,actionFilter]);
  async function clear() {
    setClearing(true);
    // Reject any archive reply that began before Clear, including a slow page.
    requestGeneration.current++;
    try { await onClear("all"); setSaved([]); setError(""); }
    catch (e) { setError(String(e)); }
    finally { setClearing(false); setHistoryRevision(old => old + 1); }
  }
  async function copy() {
    try { await navigator.clipboard.writeText(filtered.slice().reverse().map(entry => `[${stamp(entry.at)}] [${streamFor(entry).toUpperCase()}] ${entry.job_kind ? `${jobTitle(entry.job_kind)}: ` : ""}${entry.message}`).join("\n")); }
    catch (e) { setError(String(e)); }
  }
  return <div className="activity-view">
    <div className="activity-status-grid">
      <WorkerStatus title="Local actions" job={job} monitor={monitor} onCancel={onCancel}/>
      <WorkerStatus title="Online actions" job={onlineJob} monitor={monitor} onCancel={onCancel}/>
      <WorkerStatus title="Downloads" job={downloadJob} monitor={monitor} onCancel={onCancel}/>
    </div>
    <section className="activity-panel activity-unified" aria-label="Activity log">
      <h2>All activity<span className="stream-count">Most recent first</span></h2>
      <div className="stream-toolbar">
        <input aria-label="Search activity" title="Search saved messages and current tasks" type="search" placeholder="Search activity…" value={search} onChange={event => setSearch(event.target.value)}/>
        <select className="activity-action-filter" aria-label="Action type" title="Filter current and saved activity by the action that produced it" value={actionFilter} onChange={event => setActionFilter(event.target.value)}>
          <option value="all">All actions</option>
          <option value="stream:local">Local actions</option>
          <option value="stream:online">Online actions</option>
          <option value="stream:downloads">Downloads</option>
          {actionGroups.map(group => <optgroup label={group.label} key={group.label}>{group.actions.map(kind => <option key={kind} value={kind}>{jobTitle(kind)}</option>)}</optgroup>)}
          <option value="application">Application</option>
        </select>
        <button aria-label="Copy activity" title="Copy visible activity" onClick={copy}><Copy size={14}/></button>
        <button aria-label="Clear activity" title="Clear saved and current activity" disabled={clearing} onClick={clear}><Trash2 size={14}/></button>
      </div>
      <div className="activity-log" role="log" aria-label="All actions" aria-live="off">
        {filtered.map(entry => <LogRow entry={entry} key={entryKey(entry)}/>)}
        {!filtered.length && <p className="activity-empty">{loading ? "Loading activity…" : search.trim() || actionFilter !== "all" ? "No matching activity." : "Your activity will appear here."}</p>}
        <div ref={sentinel} className="history-sentinel" aria-hidden="true"/>
        {loading && filtered.length > 0 && <p className="history-feedback">Loading saved activity…</p>}
        {error && <p className="history-feedback" role="alert">Activity could not load: {error}</p>}
      </div>
    </section>
  </div>;
}
