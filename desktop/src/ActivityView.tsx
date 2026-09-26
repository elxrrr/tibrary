import { useMemo, useState } from "react";
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
          <strong>{job!.kind.replaceAll("_", " ")}</strong><span>{job!.message}</span><span className="status-badge status-running">Running</span>
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
      <div className="card activity-status"><div><h2>{active(job) ? job?.message : "No local task running"}</h2><p>File and tag work has its own worker.</p></div>{active(job) && <button onClick={() => onCancel(job!.kind)}>Cancel task</button>}</div>
      <div className="card activity-status"><div><h2>{active(onlineJob) ? onlineJob?.message : "No online task running"}</h2><p>Catalogue requests have their own worker.</p></div>{active(onlineJob) && <button onClick={() => onCancel(onlineJob!.kind)}>Cancel online task</button>}</div>
      <div className="card activity-status"><div><h2>{active(downloadJob) ? downloadJob?.message : "No downloads running"}</h2><p>Transfers and placement run independently.</p></div>{active(downloadJob) && <button onClick={() => onCancel("download")}>Cancel download</button>}</div>
    </div>
    <div className="activity-split">
      <StreamPanel stream="online" title="Online actions" entries={streams.online} monitor={monitor} job={onlineJob} onClear={onClear}/>
      <StreamPanel stream="local" title="Local actions" entries={streams.local} monitor={monitor} job={job} onClear={onClear}/>
      <StreamPanel stream="downloads" title="Downloads" entries={streams.downloads} monitor={monitor} job={downloadJob} onClear={onClear}/>
    </div>
  </div>;
}
