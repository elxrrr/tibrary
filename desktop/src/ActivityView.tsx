import { useEffect, useMemo, useState } from "react";
import { Copy, Trash2, ChevronDown, ChevronRight } from "lucide-react";
import { call, active, Job, Row } from "./api";

export type ActivityEntry = {
  at: string;
  message: string;
  category?: string;
  level?: string;
  progress_id?: string;
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
  const titles: Record<string, string> = {discography: "Refresh releases", cached_releases: "Recheck cached releases", link: "Link releases", match_artists: "Match artists", preview: "Review local tags", metadata: "Find missing tags", artwork: "Find artwork", local_duplicates: "Check local duplicates", optimizations: "Check replacements", download: "Downloads", scan: "Scan library", apply: "Apply reviewed changes"};
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
  const parsed = job?.message?.match(/(?:^|\D)([\d,]+)\s*(?:\/|of)\s*([\d,]+)(?!\d)/i);
  let done = parsed ? Number(parsed[1].replaceAll(",", "")) : 0;
  let total = parsed ? Number(parsed[2].replaceAll(",", "")) : 0;
  if (job?.kind === "download") {
    const batches = Object.values(monitor).filter(row => row.kind === "batch");
    if (batches.length) {
      done = batches.reduce((n, row) => n + Number(row.completed_tracks || 0), 0);
      total = batches.reduce((n, row) => n + Number(row.total_tracks || 0), 0);
    }
  }
  if (!total || !Number.isFinite(total)) return { label: "Working", percent: null as number | null };
  done = Math.min(total, Math.max(0, done));
  const elapsed = Math.max(1, now / 1000 - Number(job?.started || now / 1000));
  const rate = done / elapsed;
  const remaining = done && rate > 0 ? ` · ~${formatDuration((total - done) / rate)} remaining · ${rate.toFixed(1)} items/s` : "";
  return { label: `${Math.round(done / total * 100)}% · ${done.toLocaleString()}/${total.toLocaleString()}${remaining}`, percent: done / total * 100 };
}

function LogRow({ entry }: { entry: ActivityEntry }) {
  const category = entry.category || (entry.level === "error" ? "error" : "general");
  return <div className={`log-row log-${category}`}>
    <time title={stamp(entry.at)}>{new Date(entry.at).toLocaleTimeString()}</time>
    <span className={`log-badge log-badge-${category}`}>{category.toUpperCase()}</span>
    <span className="log-message">{entry.message}</span>
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
  const jobMatches = active(job) && (!q || `${job?.kind} ${job?.message}`.toLocaleLowerCase().includes(q) || filtered.some(entry => new Date(entry.at).getTime() >= Number(job?.started || 0) * 1000));
  const recent = jobMatches ? filtered.filter(entry => new Date(entry.at).getTime() >= Number(job?.started || 0) * 1000) : [];
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
      {jobMatches && stream !== "downloads" && <div className="batch-log">
        <button className="batch-toggle" aria-expanded={Boolean(expanded[job!.id])} onClick={() => setExpanded(old => ({...old, [job!.id]: !old[job!.id]}))}>
          {expanded[job!.id] ? <ChevronDown size={14}/> : <ChevronRight size={14}/>}
          <strong>{jobTitle(job!.kind)}</strong><span>{job!.message}</span><span className="status-badge status-running">Running</span>
        </button>
        {expanded[job!.id] && <div className="batch-children">{recent.length ? recent.map((entry, i) => <LogRow entry={entry} key={`${entry.at}-${i}`}/>) : <p>Waiting for the next step…</p>}</div>}
      </div>}
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
      {filtered.filter(entry => !recent.includes(entry)).slice().reverse().map((entry, i) => <LogRow entry={entry} key={`${entry.at}-${i}`}/>)}
      {!filtered.length && !batches.length && !standalone.length && !jobMatches && <div className="activity-empty">No {title.toLowerCase()} to show.</div>}
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
  const progress = running && job?.message?.match(/([\d,]+)\s*(?:\/|of)\s*([\d,]+)/i);
  const detail = running ? (job?.message || "Working") : completedId === job?.id ? "Task complete" : "Awaiting task...";
  return <div className="card activity-status" role="status">
    <div><small>{title}</small><h2>{running ? "Task in progress..." : detail}</h2>
      {running && <p>{progress ? `Processing ${progress[1]}/${progress[2]} · ` : ""}{job?.message}</p>}
    </div>
    {running && <button onClick={() => onCancel(job!.kind)}>Cancel task</button>}
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
