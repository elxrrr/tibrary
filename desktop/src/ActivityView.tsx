import { useEffect, useMemo, useState, useRef } from "react";
import { Copy, Trash2, ChevronDown, ChevronRight } from "lucide-react";
import { call, active, Job, Row } from "./api";

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
  const titles: Record<string, string> = {discography: "Refresh releases", release_artists: "Check release artists", cached_releases: "Recheck cached releases", link: "Link releases", match_artists: "Match artists", preview: "Review local tags", metadata: "Find missing tags", artwork: "Find artwork", local_duplicates: "Check local duplicates", optimizations: "Check replacements", download: "Downloads", scan: "Scan library", apply: "Apply reviewed changes"};
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
    const batches = Object.values(monitor).filter(row => row.kind === "batch");
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

function LogRow({ entry }: { entry: ActivityEntry }) {
  const category = entry.category || (entry.level === "error" ? "error" : "general");
  return <div className={`log-row log-${category}`}>
    <time title={stamp(entry.at)}>{new Date(entry.at).toLocaleTimeString()}</time>
    <span className={`log-badge log-badge-${category}`}>{category.toUpperCase()}</span>
    <span className="log-message">{entry.message}</span>
  </div>;
}

export function groupActivity(entries: ActivityEntry[], job?: Job | null) {
  const groups = new Map<string, {id: string; kind?: string; status?: string; entries: ActivityEntry[]; message: string}>();
  const standalone: ActivityEntry[] = [];
  for (const entry of entries) {
    const id = entry.job_id || entry.progress_id;
    if (!id) { standalone.push(entry); continue; }
    const group = groups.get(id) || {id, kind: entry.job_kind, entries: [], message: ""};
    group.kind ||= entry.job_kind;
    group.status = entry.job_status || group.status;
    group.message = entry.message;
    if (!entry.progress_id || group.entries.at(-1)?.message !== entry.message) group.entries.push(entry);
    groups.set(id, group);
  }
  if (job?.id && (active(job) || groups.has(job.id))) {
    const group = groups.get(job.id) || {id: job.id, entries: [], message: ""};
    Object.assign(group, {kind: job.kind, status: job.status, message: job.message});
    groups.set(job.id, group);
  }
  return {groups: [...groups.values()].reverse(), standalone};
}

function JobHistory({id, recent, query}: {id: string; recent: ActivityEntry[]; query: string}) {
  const [saved, setSaved] = useState<ActivityEntry[]>([]);
  const [error, setError] = useState("");
  const sentinel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let disposed = false, loading = false, offset = 0, more = true;
    const marker = sentinel.current;
    const root = marker?.closest(".activity-log") || null;
    let observer: IntersectionObserver | undefined;
    async function load() {
      if (disposed || loading || !more) return;
      loading = true;
      try {
        const rows = await call("logs.job", {id, offset}) as ActivityEntry[];
        if (disposed) return;
        offset += rows.length; more = rows.length === 1000;
        setSaved(old => [...old, ...rows]);
      } catch (e) {if (!disposed) setError(String(e)); more = false;}
      finally {loading = false;}
    }
    // Keep recent rows on screen while older history loads. Further pages arrive
    // on scroll, without replacing the live log or introducing an action button.
    load();
    observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) void load();
    }, {root, rootMargin: "150px"});
    if (marker) observer.observe(marker);
    return () => {disposed = true; observer?.disconnect();};
  }, [id]);
  const archived = [...new Map([...saved, ...recent.filter(entry => !entry.progress_id)].map(entry => [`${entry.at}:${entry.message}`, entry])).values()].sort((a,b) => a.at.localeCompare(b.at));
  const live = recent.find(entry => entry.progress_id);
  const rows = live && !archived.some(entry => entry.message === live.message) ? [...archived,live] : archived;
  return <div className="batch-children">
    {rows.filter(entry => !query || entry.message.toLocaleLowerCase().includes(query)).map(entry => <LogRow entry={entry} key={entry.progress_id || `${entry.at}:${entry.message}`}/>)}
    {!rows.length && <p>No details recorded yet.</p>}
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
  const items = Object.values(monitor);
  const batches = stream === "downloads" ? items.filter(item => item.kind === "batch" && (!q || `${item.artist} ${item.release}`.toLocaleLowerCase().includes(q) || items.some(track => track.kind === "track" && track.release_id === item.release_id && `${track.title} ${track.status}`.toLocaleLowerCase().includes(q)))) : [];
  const standalone = stream === "downloads" ? items.filter(item => item.kind === "track" && !items.some(parent => parent.kind === "batch" && parent.release_id === item.release_id) && (!q || `${item.title} ${item.status}`.toLocaleLowerCase().includes(q))) : [];
  const {groups, standalone: ungrouped} = groupActivity(entries, job);
  const visibleGroups = groups.filter(group => !q || `${jobTitle(group.kind)} ${group.message}`.toLocaleLowerCase().includes(q) || group.entries.some(entry => entry.message.toLocaleLowerCase().includes(q)));
  async function copy() {
    const lines = filtered.map(entry => `[${stamp(entry.at)}] [${(entry.category || "general").toUpperCase()}] ${entry.message}`);
    if (stream === "downloads") {
      for (const batch of batches) {
        lines.push(`[${new Date().toLocaleString()}] [DOWNLOAD] ${batch.artist} — ${batch.release}: ${batch.status} ${batch.completed_tracks || 0}/${batch.total_tracks} tracks`);
        if (expanded[String(batch.release_id)] !== false) {
          for (const item of items.filter(track => track.kind === "track" && track.release_id === batch.release_id)) {
            lines.push(`[${new Date().toLocaleString()}] [DOWNLOAD] ${item.title}: ${item.status} ${item.percent ?? ""}%`);
          }
        }
      }
      for (const item of standalone) lines.push(`[${new Date().toLocaleString()}] [DOWNLOAD] ${item.title}: ${item.status} ${item.percent ?? ""}%`);
    }
    await navigator.clipboard.writeText(lines.join("\n"));
  }
  return <section className="activity-panel" aria-label={title}>
    <h2>{title}<span className="stream-count">{filtered.length} entries</span></h2>
    <div className="stream-toolbar">
      <input aria-label={`Search ${title.toLowerCase()}`} type="search" placeholder={`Search ${title.toLowerCase()}…`} value={search} onChange={event => setSearch(event.target.value)} />
      <button aria-label={`Copy ${title.toLowerCase()}`} title="Copy filtered entries" onClick={copy}><Copy size={14}/></button>
      <button aria-label={`Clear ${title.toLowerCase()}`} title="Clear this panel" disabled={clearing} onClick={async () => {setClearing(true); try {await onClear(stream);} finally {setClearing(false);}}}><Trash2 size={14}/></button>
    </div>
    <div className="activity-log">
      {visibleGroups.map(group => {
        const running = job?.id === group.id && active(job);
        const status = running ? job!.status : ["complete", "failed", "cancelled"].includes(group.status || "") ? group.status : "interrupted";
        return <div className="batch-log" key={group.id}>
          <button className="batch-toggle" aria-expanded={Boolean(expanded[group.id])} onClick={() => setExpanded(old => ({...old, [group.id]: !old[group.id]}))}>
            {expanded[group.id] ? <ChevronDown size={14}/> : <ChevronRight size={14}/>}
            <strong>{jobTitle(group.kind)}</strong><span title={group.message}>{group.message}</span><span className={`status-badge status-${status}`}>{status}</span>
          </button>
          {expanded[group.id] && <JobHistory id={group.id} recent={group.entries} query={q}/>}
        </div>;
      })}
      {stream === "downloads" && batches.map(batch => {
        const children = items.filter(item => item.kind === "track" && item.release_id === batch.release_id);
        const bytes = children.reduce((n, item) => n + Number(item.bytes || 0), 0);
        const totalBytes = children.reduce((n, item) => n + Number(item.estimated_total_bytes || item.bytes || 0), 0);
        const key = String(batch.release_id);
        return <div className="batch-log" key={key}>
          <button className="batch-toggle" aria-expanded={expanded[key] !== false} onClick={() => setExpanded(old => ({...old, [key]: old[key] === false}))}>
            {expanded[key] === false ? <ChevronRight size={14}/> : <ChevronDown size={14}/>}
            <strong>{batch.artist} — {batch.release}</strong>
            <span>{batch.completed_tracks || 0}/{batch.total_tracks} tracks · {(bytes / 1048576).toFixed(1)} MB{totalBytes > bytes ? ` / ~${(totalBytes / 1048576).toFixed(1)} MB` : ""}</span>
            <span className={`status-badge status-${batch.status === "failed" ? "failed" : batch.status === "complete" ? "complete" : "running"}`}>{batch.status}</span>
          </button>
          {expanded[key] !== false && <div className="batch-children">{children.map(item => <DownloadTrack item={item} key={item.id}/>)}</div>}
        </div>;
      })}
      {stream === "downloads" && standalone.map(item => <DownloadTrack item={item} key={item.id}/>)}
      {ungrouped.filter(entry => !q || entry.message.toLocaleLowerCase().includes(q)).slice().reverse().map((entry, i) => <LogRow entry={entry} key={`${entry.at}-${i}`}/>)}
      {!filtered.length && !batches.length && !standalone.length && !visibleGroups.length && <div className="activity-empty">No {title.toLowerCase()} to show.</div>}
    </div>
  </section>;
}

function DownloadTrack({ item }: { item: Row }) {
  return <div className="download-row">
    <strong>{item.title}</strong>
    <span>{item.index} of {item.total_tracks} · {item.percent || 0}% · {((item.bytes || 0) / 1048576).toFixed(1)} MB{item.estimated_total_bytes ? ` / ~${(item.estimated_total_bytes / 1048576).toFixed(1)} MB` : ""} · {item.bytes_per_second ? `${(item.bytes_per_second / 1048576).toFixed(1)} MB/s` : "—"} · {item.eta_seconds != null ? `${item.eta_seconds}s remaining` : "ETA —"}</span>
    <span className={`status-badge status-${item.status === "failed" ? "failed" : item.status === "complete" ? "complete" : "running"}`}>{item.status}{item.error ? ` · ${item.error}` : ""}</span>
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
