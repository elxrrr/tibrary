import { useEffect, useMemo, useState, useRef, memo } from "react";
import { Copy, Trash2, ChevronDown, ChevronRight } from "lucide-react";
import { call, active, Job, Row, onlineKinds } from "./api";

export type ActivityEntry = {
  at: string;
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
  // Error categories and incidental words must not split one job across panels.
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

export function jobTitle(kind?: string): string {
  const titles: Record<string, string> = {discography: "Refresh releases", release_artists: "Check release artists", cached_releases: "Recheck cached releases", check_availability: "Check release availability", link: "Link releases", match_artists: "Match artists", preview: "Review local changes", mqa: "MQA audit", queue_mqa: "Queue MQA replacements", queue_replacements: "Queue online replacements", check_replacements: "Check online replacements", release_details: "Fetch track details and credits", metadata: "Find missing tags", artwork: "Find artwork", local_duplicates: "Check local duplicates", optimizations: "Check replacements", download: "Download music", scan: "Scan library", apply: "Apply reviewed changes", deep_review: "Find release matches", deep_preview: "Review release links", deep_apply: "Save reviewed release links", review_consolidation: "Review duplicate removal", consolidate: "Remove reviewed duplicates", manual_candidate: "Inspect a release match", connections: "Test connection", connect_account: "Connect account", connect_download: "Connect account", component_check: "Check streaming components", component_update: "Update streaming components", component_rollback: "Restore streaming components"};
  return titles[kind || ""] || kind?.replaceAll("_", " ") || "Task";
}

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
  const category = entry.category || (entry.level === "error" ? "error" : "general");
  return <div className={`log-row log-${category} ${entry.level === "error" ? "log-error" : ""}`}>
    <time title={stamp(entry.at)}>{new Date(entry.at).toLocaleTimeString()}</time>
    <span className={`log-badge log-badge-${category}`}>{category.toUpperCase()}</span>
    <span className="log-message">{entry.message}</span>
  </div>;
});

const terminal = new Set(["complete", "failed", "cancelled", "interrupted"]);
const entryKey = (entry: ActivityEntry) => entry.progress_id ? `progress:${entry.progress_id}` : `${entry.job_id || ""}:${entry.at}:${entry.message}`;
const compareTime = (a: string, b: string) => (Date.parse(a) || 0) - (Date.parse(b) || 0) || a.localeCompare(b);

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
    if (!old || compareTime(old.at, entry.at) <= 0) rows.set(key, entry);
  }
  const completed = new Map<string, string>();
  for (const entry of rows.values()) if (entry.job_id && !entry.progress_id && terminal.has(entry.job_status || "")) {
    completed.set(entry.job_id, [completed.get(entry.job_id) || "", entry.at].sort().at(-1)!);
  }
  const streams: Record<ActivityStream, ActivityEntry[]> = {online: [], local: [], downloads: []};
  for (const entry of [...rows.values()].sort((a, b) => compareTime(a.at,b.at))) {
    if (entry.progress_id && completed.has(entry.progress_id)) continue;
    streams[streamFor(entry)].push(entry);
  }
  return Object.values(streams).flatMap(rows => {
    const summaries = new Map<string, ActivityEntry>();
    for (const entry of rows) if (entry.job_id && !entry.progress_id) summaries.set(entry.job_id, entry);
    return [...new Map([...Array.from(summaries.values()).slice(-500), ...rows.slice(-1000)].map(entry => [entryKey(entry), entry])).values()];
  }).sort((a,b) => compareTime(a.at,b.at));
}

export type ActivityGroup = {id: string; kind?: string; status?: string; entries: ActivityEntry[]; message: string; updated: string};

export function groupActivity(entries: ActivityEntry[], job?: Job | null) {
  const groups = new Map<string, ActivityGroup>();
  const standalone: ActivityEntry[] = [];
  for (const entry of entries) {
    const id = entry.job_id || entry.progress_id;
    if (!id) { standalone.push(entry); continue; }
    const group = groups.get(id) || {id, kind: entry.job_kind, entries: [], message: "", updated: ""};
    group.kind ||= entry.job_kind;
    if (compareTime(entry.at, group.updated) >= 0 && (!terminal.has(group.status || "") || terminal.has(entry.job_status || ""))) {
      group.status = entry.job_status || group.status;
      group.message = entry.message;
      group.updated = entry.at;
    }
    if (!entry.progress_id || group.entries.at(-1)?.message !== entry.message) group.entries.push(entry);
    groups.set(id, group);
  }
  if (job?.id && (active(job) || groups.has(job.id))) {
    const group = groups.get(job.id) || {id: job.id, entries: [], message: "", updated: ""};
    Object.assign(group, {kind: job.kind, status: job.status, message: job.message});
    group.updated ||= new Date(job.started * 1000).toISOString();
    groups.set(job.id, group);
  }
  return {groups: [...groups.values()].sort((a,b) => compareTime(b.updated,a.updated)), standalone};
}

function JobHistory({id, recent, query, running}: {id: string; recent: ActivityEntry[]; query: string; running: boolean}) {
  const [saved, setSaved] = useState<ActivityEntry[]>([]);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const sentinel = useRef<HTMLDivElement>(null);
  const runningRef = useRef(running);
  runningRef.current = running;
  const loadRef = useRef<(() => void) | undefined>(undefined);
  useEffect(() => {
    let disposed = false, requesting = false, offset = 0, more = true;
    const marker = sentinel.current;
    const root = marker?.closest(".activity-log") || null;
    let observer: IntersectionObserver | undefined;
    async function load() {
      if (disposed || requesting) return;
      requesting = true;
      try {
        const rows = await call("logs.job", {id, offset}) as ActivityEntry[];
        if (disposed) return;
        offset += rows.length; more = rows.length === 1000;
        setSaved(old => [...new Map([...old, ...rows].map(row => [entryKey(row), row])).values()]);
        setError("");
      } catch (e) {if (!disposed) setError(String(e));}
      finally {requesting = false; if (!disposed) setLoading(false);}
    }
    // Append saved history, never clear it during polling or on job completion.
    // Only an expanded, active job at the saved tail needs periodic requests.
    loadRef.current = () => {if (!more) void load();};
    void load();
    const timer = window.setInterval(() => {if (runningRef.current && !more) void load();}, 2000);
    observer = new IntersectionObserver(entries => {
      if (more && entries.some(entry => entry.isIntersecting)) void load();
    }, {root, rootMargin: "150px"});
    if (marker) observer.observe(marker);
    return () => {disposed = true; observer?.disconnect(); window.clearInterval(timer); loadRef.current = undefined;};
  }, [id]);
  useEffect(() => {if (!running) loadRef.current?.();}, [running]);
  const archived = useMemo(() => [...new Map([...recent.filter(entry => !entry.progress_id), ...saved].map(entry => [entryKey(entry), entry])).values()].sort((a,b) => compareTime(a.at,b.at)), [saved,recent]);
  const live = running ? [...recent].reverse().find(entry => entry.progress_id) : undefined;
  const rows = live && !archived.some(entry => entry.message === live.message) ? [...archived,live] : archived;
  const visible = rows.filter(entry => !query || entry.message.toLocaleLowerCase().includes(query));
  return <div className="batch-children">
    {visible.map(entry => <LogRow entry={entry} key={entryKey(entry)}/>)}
    {!rows.length && <p className="history-feedback">{loading ? "Loading saved details…" : "No details recorded yet."}</p>}
    {rows.length > 0 && !visible.length && <p className="history-feedback">No matching details in this job.</p>}
    <div ref={sentinel} className="history-sentinel" aria-hidden="true"/>
    {error && <p role="alert">Saved details could not load: {error}</p>}
  </div>;
}

function StreamPanel({ stream, title, entries, monitor, job, onClear }: {
  stream: ActivityStream;
  title: string;
  entries: ActivityEntry[];
  monitor: Record<string, Row>;
  job?: Job | null;
  onClear: (stream: ActivityStream) => Promise<void>;
}) {
  const [search, setSearch] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [clearing, setClearing] = useState(false);
  const q = search.trim().toLocaleLowerCase();
  const filtered = useMemo(() => entries.filter(entry => !q || `${entry.message} ${entry.category || ""}`.toLocaleLowerCase().includes(q)), [entries, q]);
  const items = useMemo(()=>stream === "downloads" ? Object.values(monitor) : [],[monitor,stream]);
  const {groups, standalone: ungrouped} = useMemo(()=>{
    const result=groupActivity(entries,job);
    return {...result,groups:stream === "downloads" ? groupDownloads(result.groups,items,job) : result.groups};
  },[entries,job,items,stream]);
  const matchingMonitor = (item: Row) => !q || `${item.artist || ""} ${item.release || ""} ${item.title || ""} ${item.status || ""}`.toLocaleLowerCase().includes(q);
  const visibleGroups = groups.filter(group => !q || expanded[group.id] || `${jobTitle(group.kind)} ${group.message}`.toLocaleLowerCase().includes(q) || group.entries.some(entry => entry.message.toLocaleLowerCase().includes(q)) || (stream === "downloads" && items.some(item => downloadJobId(item, job) === group.id && matchingMonitor(item))));
  async function copy() {
    const lines = filtered.map(entry => `[${stamp(entry.at)}] [${(entry.category || "general").toUpperCase()}] ${entry.message}`);
    if (stream === "downloads") {
      for (const item of items.filter(matchingMonitor)) lines.push(`[DOWNLOAD] ${item.kind === "batch" ? `${item.artist} — ${item.release}` : item.title}: ${item.status} ${item.percent ?? ""}%`);
    }
    await navigator.clipboard.writeText(lines.join("\n"));
  }
  return <section className="activity-panel" aria-label={title}>
    <h2>{title}<span className="stream-count">{visibleGroups.length} {visibleGroups.length === 1 ? "job" : "jobs"}</span></h2>
    <div className="stream-toolbar">
      <input aria-label={`Search ${title.toLowerCase()}`} title="Search job titles, recent activity and saved details in expanded jobs" type="search" placeholder={`Search ${title.toLowerCase()}…`} value={search} onChange={event => setSearch(event.target.value)} />
      <button aria-label={`Copy ${title.toLowerCase()}`} title="Copy filtered entries" onClick={copy}><Copy size={14}/></button>
      <button aria-label={`Clear ${title.toLowerCase()}`} title="Clear this panel" disabled={clearing} onClick={async () => {setClearing(true); try {await onClear(stream); setExpanded({});} finally {setClearing(false);}}}><Trash2 size={14}/></button>
    </div>
    <div className="activity-log">
      {visibleGroups.map(group => {
        const running = job?.id === group.id && active(job);
        const status = running ? job!.status : terminal.has(group.status || "") ? group.status : "interrupted";
        const jobQuery = `${jobTitle(group.kind)} ${group.message}`.toLocaleLowerCase().includes(q) ? "" : q;
        return <div className="batch-log" key={group.id}>
          <button className="batch-toggle job-toggle" aria-expanded={Boolean(expanded[group.id])} onClick={() => setExpanded(old => ({...old, [group.id]: !old[group.id]}))}>
            {expanded[group.id] ? <ChevronDown size={14}/> : <ChevronRight size={14}/>}
            <strong>{jobTitle(group.kind)}</strong><span title={group.message}>{group.message}</span><span className={`status-badge status-${status}`}>{status}</span>
          </button>
          {expanded[group.id] && <>
            {stream === "downloads" && <DownloadHistory items={items.filter(item => downloadJobId(item, job) === group.id)} query={jobQuery}/>}
            <JobHistory id={group.id} recent={group.entries} query={jobQuery} running={running}/>
          </>}
        </div>;
      })}
      {ungrouped.filter(entry => !q || entry.message.toLocaleLowerCase().includes(q)).slice().reverse().map((entry, i) => <LogRow entry={entry} key={`${entry.at}-${i}`}/>)}
      {!filtered.length && !visibleGroups.length && <div className="activity-empty">No {title.toLowerCase()} to show.</div>}
    </div>
  </section>;
}

const downloadJobId = (item: Row, job?: Job | null) => String(item.job_id || job?.id || "saved-downloads");
const completeDownload = (status: string) => status === "complete" || status === "already downloaded";

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

export function groupDownloads(groups: ActivityGroup[], items: Row[], job?: Job | null): ActivityGroup[] {
  const result = new Map(groups.map(group => [group.id, group]));
  for (const item of items) {
    const id = downloadJobId(item, job);
    if (!result.has(id)) {
      const related = items.filter(row => downloadJobId(row, job) === id);
      const status = related.some(row => row.status === "failed") ? "failed" : related.every(row => completeDownload(row.status)) ? "complete" : "interrupted";
      result.set(id, {id, kind: "download", status, entries: [], message: "Saved download details", updated: String(item.at || "")});
    }
  }
  return [...result.values()].sort((a,b) => compareTime(b.updated,a.updated));
}

function DownloadHistory({items, query}: {items: Row[]; query: string}) {
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const matches = (item: Row) => !query || `${item.artist || ""} ${item.release || ""} ${item.title || ""} ${item.status || ""}`.toLocaleLowerCase().includes(query);
  const batches = items.filter(item => item.kind === "batch");
  const standalone = items.filter(item => item.kind === "track" && !batches.some(batch => batch.release_id === item.release_id));
  return <div className="download-job-details">
    {batches.map(batch => {
      const children = items.filter(item => item.kind === "track" && item.release_id === batch.release_id);
      if (!matches(batch) && !children.some(matches)) return null;
      const bytes = children.reduce((n, item) => n + Number(item.bytes || 0), 0);
      const totalBytes = children.reduce((n, item) => n + Number(item.estimated_total_bytes || item.bytes || 0), 0);
      const key = String(batch.release_id);
      const show = expanded[key] !== false;
      return <div className="download-batch" key={key}>
        <button className="batch-toggle release-toggle" aria-expanded={show} onClick={() => setExpanded(old => ({...old, [key]: !show}))}>
          {show ? <ChevronDown size={14}/> : <ChevronRight size={14}/>}
          <strong>{batch.artist} — {batch.release}</strong>
          <span>{batch.completed_tracks || 0}/{batch.total_tracks} tracks · {(bytes / 1048576).toFixed(1)} MB{totalBytes > bytes ? ` / ~${(totalBytes / 1048576).toFixed(1)} MB` : ""}</span>
          <span className={`status-badge status-${batch.status === "failed" ? "failed" : batch.status === "complete" ? "complete" : "running"}`}>{batch.status}</span>
        </button>
        {show && children.filter(item => matches(batch) || matches(item)).map(item => <DownloadTrack item={item} key={item.id}/>)}
      </div>;
    })}
    {standalone.filter(matches).map(item => <DownloadTrack item={item} key={`${item.release_id}:${item.id}`}/>)}
  </div>;
}

function DownloadTrack({ item }: { item: Row }) {
  const status = item.status === "staged" ? "Writing tags" : item.status;
  return <div className="download-row">
    <strong>{item.title}</strong>
    <span>{item.index} of {item.total_tracks} · {item.percent || 0}% · {((item.bytes || 0) / 1048576).toFixed(1)} MB{item.estimated_total_bytes ? ` / ~${(item.estimated_total_bytes / 1048576).toFixed(1)} MB` : ""} · {item.bytes_per_second ? `${(item.bytes_per_second / 1048576).toFixed(1)} MB/s` : "—"} · {item.eta_seconds != null ? `${item.eta_seconds}s remaining` : "ETA —"}</span>
    <span className={`status-badge status-${item.status === "failed" ? "failed" : completeDownload(item.status) ? "complete" : item.status === "cancelled" ? "cancelled" : "running"}`}>{status}{item.error ? ` · ${item.error}` : ""}</span>
  </div>;
}

function WorkerStatus({ title, job, onCancel }: { title: string; job?: Job | null; onCancel: (kind: string) => void }) {
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
  const progress = workload(job, {}, Date.now());
  const detail = running ? (job?.message || "Working") : completedId === job?.id ? "Task complete" : "Awaiting task...";
  return <div className="card activity-status" role="status">
    <div><small>{title}</small><h2>{running ? "Task in progress..." : detail}</h2>
      {running && <><p title={job?.message}>{job?.message}</p><p title={progress?.label} className="job-progress-label">{progress?.label}</p><progress aria-label={`${title} progress`} max={100} value={progress?.percent ?? undefined}/></>}
    </div>
    {running && <button disabled={job?.status === "cancelling"} onClick={() => onCancel(job!.kind)}>{job?.status === "cancelling" ? "Cancelling…" : "Cancel task"}</button>}
  </div>;
}

export function ActivityView({ logs, monitor, job, onlineJob, downloadJob, onClear, onCancel }: {
  logs: ActivityEntry[];
  monitor: Record<string, Row>;
  job?: Job | null;
  onlineJob?: Job | null;
  downloadJob?: Job | null;
  onClear: (stream: ActivityStream) => Promise<void>;
  onCancel: (kind: string) => void;
}) {
  const streams = useMemo(() => ({
    online: logs.filter(entry => streamFor(entry) === "online"),
    local: logs.filter(entry => streamFor(entry) === "local"),
    downloads: logs.filter(entry => streamFor(entry) === "downloads"),
  }), [logs]);
  return <div className="activity-view">
    <div className="activity-status-grid">
      <WorkerStatus title="Online actions" job={onlineJob} onCancel={onCancel}/>
      <WorkerStatus title="Local actions" job={job} onCancel={onCancel}/>
      <WorkerStatus title="Downloads" job={downloadJob} onCancel={onCancel}/>
    </div>
    <div className="activity-split">
      <StreamPanel stream="online" title="Online actions" entries={streams.online} monitor={monitor} job={onlineJob} onClear={onClear}/>
      <StreamPanel stream="local" title="Local actions" entries={streams.local} monitor={monitor} job={job} onClear={onClear}/>
      <StreamPanel stream="downloads" title="Downloads" entries={streams.downloads} monitor={monitor} job={downloadJob} onClear={onClear}/>
    </div>
  </div>;
}
