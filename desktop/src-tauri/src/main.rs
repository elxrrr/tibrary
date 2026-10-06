#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod release_artists;
mod release_anchor;
mod availability;
mod network;
mod progress;
mod subscriber_metadata;
mod appearance;
mod table_filters;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
};
use tauri::{Emitter, Manager};

pub mod account;
pub mod actions;
pub mod db;
pub mod downloads;
pub mod duplicates;
pub mod enrichment;
pub mod flac_container;
pub mod linking;
pub mod maintenance;
pub mod matching;
pub mod mqa;
pub mod musical_keys;
pub mod organisation;
pub mod recommendations;
pub mod release_matching;
pub mod scanner;
pub mod stream_download;
pub mod tag_writer;
pub mod tidal;
pub mod workflows;
use db::TursoDb;

/// Resume only matching scopes, and count each durable artist result once.
fn completed_refresh_ids(saved: &Value, ids: &[String], market: &str, detailed: bool, resume: bool) -> Vec<String> {
    if !resume || saved["market"] != market || saved["detailed"] != detailed { return Vec::new(); }
    let completed: Vec<String> = serde_json::from_value(saved["completed"].clone()).unwrap_or_default();
    ids.iter().filter(|id| completed.contains(id)).cloned().collect()
}

fn reference_refresh_progress(backend: &Backend, message: &str, completed: usize, total: usize) {
    let job = backend.online_job.lock().unwrap().clone();
    if let Some(mut job) = job {
        // These counts come from the reference producer, rather than a previous
        // measured message. Preserve them through the shared progress adapter.
        job.as_object_mut().unwrap().remove("progress_measured");
        job["completed"] = json!(completed);
        job["total"] = json!(total);
        job["progress_phase_override"] = json!("reference releases");
        backend.update_online_job_progress(message, job);
    }
}

/// Publish durable artist results while a refresh is still running, without
/// making large library views reload for every individual metadata write.
fn publish_catalogue_changes(
    app: Option<&tauri::AppHandle>,
    db: &TursoDb,
    last_published: &mut Option<std::time::Instant>,
) {
    let now = std::time::Instant::now();
    if last_published.is_some_and(|previous| now.duration_since(previous).as_secs() < 5) {
        return;
    }
    if let Some(app) = app {
        let _ = app.emit("backend-event", json!({
            "event": "changed",
            "reason": "catalogue-refresh",
            "revision": db.revision.load(Ordering::SeqCst),
        }));
        *last_published = Some(now);
    }
}

const ONLINE_JOB_KINDS: &[&str] = &[
    "link", "check_availability", "cached_releases", "release_artists",
    "discography", "release_details", "release_tracks", "connections", "favourites",
    "match_artists", "metadata", "manual_candidate", "artwork",
    "check_replacements", "optimizations", "deep_review", "deep_preview",
    "connect_account", "connect_download",
];

fn is_online_job(kind: &str) -> bool { ONLINE_JOB_KINDS.contains(&kind) }

fn workflow_name(kind: &str) -> &'static str {
    match kind {
        "cached_releases" => "Recheck cached releases",
        "check_availability" => "Check release availability",
        "release_artists" => "Check release artists",
        "preview" => "Review local changes",
        "apply" => "Apply reviewed changes",
        "mqa" => "MQA audit",
        "release_details" => "Fetch track details and credits",
        "connections" => "Test connection",
        "favourites" => "Refresh favourite artists",
        "match_artists" => "Match artists",
        "metadata" => "Find missing tags",
        "manual_candidate" => "Inspect a release match",
        "artwork" => "Find artwork",
        "optimizations" | "check_replacements" => "Check online replacements",
        "local_duplicates" => "Check local duplicates",
        "queue_replacements" => "Queue online replacements",
        "queue_mqa" => "Queue MQA replacements",
        "deep_review" => "Find release matches",
        "deep_preview" => "Review release links",
        "deep_apply" => "Save reviewed release links",
        "review_consolidation" => "Review duplicate removal",
        "consolidate" => "Remove reviewed duplicates",
        _ => "Task",
    }
}

fn workflow_outcome(kind: &str, value: &Value, cancelled: bool) -> String {
    let mut message = format!("{} · {}", if cancelled { "Cancelled" } else { "Complete" }, workflow_name(kind));
    for (key, label) in [("applied", "files updated"), ("completed", "items completed"),
        ("linked", "tracks linked"), ("opportunities", "opportunities"),
        ("releases", "releases queued"), ("artists", "artists checked"),
        ("files", "files checked"), ("checked", "items checked")] {
        if let Some(count) = value[key].as_u64() { message.push_str(&format!(" · {count} {label}")); break; }
    }
    if value["reused"] == true { message.push_str(" · saved analysis reused"); }
    message
}

fn compare_table_cell(a: &Value, b: &Value, key: &str) -> std::cmp::Ordering {
    match (a[key].as_f64(), b[key].as_f64()) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        _ => {
            let display = |value: &Value| {
                value.as_str().map(str::to_owned).unwrap_or_else(|| {
                    if value.is_null() {
                        String::new()
                    } else {
                        value.to_string()
                    }
                })
            };
            display(&a[key])
                .to_lowercase()
                .cmp(&display(&b[key]).to_lowercase())
        }
    }
}

fn activity_stream(entry: &Value) -> &'static str {
    // A job owns its channel. Words such as "download" in an online metadata
    // message must not move its details into a different panel.
    if let Some(kind) = entry["job_kind"].as_str().filter(|kind| !kind.is_empty()) {
        return if kind == "download" { "downloads" } else if is_online_job(kind) { "online" } else { "local" };
    }
    let category = entry["category"].as_str().unwrap_or("general");
    match category {
        "download" => return "downloads",
        "online" | "linking" => return "online",
        "scan" | "cleanup" | "local" => return "local",
        _ => {}
    }
    let message = entry["message"].as_str().unwrap_or("").to_lowercase();
    if ["download", "fetching track", "saving track"]
        .iter()
        .any(|word| message.contains(word))
    {
        "downloads"
    } else if [
        "catalogue",
        "api",
        "remote",
        "artist search",
        "releases",
        "metadata source",
    ]
    .iter()
    .any(|word| message.contains(word))
    {
        "online"
    } else {
        "local"
    }
}

fn activity_stream_index(stream: &str) -> usize {
    match stream {
        "online" => 0,
        "downloads" => 2,
        _ => 1,
    }
}

#[derive(Default)]
struct ActivityStreamBuffer {
    entries: Vec<Value>,
}

impl ActivityStreamBuffer {
    fn push(&mut self, mut entry: Value) -> Value {
        if let Some(existing) = self.entries.iter_mut().find(|saved| {
            entry["progress_id"].as_str().is_some_and(|id| saved["progress_id"] == id)
                || (saved["at"] == entry["at"] && saved["message"] == entry["message"] && saved["job_id"] == entry["job_id"])
        }) {
            if existing["updated_at"].as_str().unwrap_or("") > entry["updated_at"].as_str().unwrap_or("") {
                return existing.clone();
            }
            entry["at"] = existing["at"].clone();
            *existing = entry.clone();
            return entry;
        }
        self.entries.push(entry.clone());
        if self.entries.len() > 1000 { self.entries.remove(0); }
        entry
    }

    fn snapshot(&self) -> Vec<Value> {
        self.entries.clone()
    }
}

#[derive(Default)]
pub struct ActivityBuffers {
    online: Mutex<ActivityStreamBuffer>,
    local: Mutex<ActivityStreamBuffer>,
    downloads: Mutex<ActivityStreamBuffer>,
}

impl ActivityBuffers {
    fn buffer(&self, stream: &str) -> &Mutex<ActivityStreamBuffer> {
        match stream {
            "online" => &self.online,
            "downloads" => &self.downloads,
            _ => &self.local,
        }
    }

    fn push(&self, entry: Value) -> Value {
        self.buffer(activity_stream(&entry)).lock().unwrap().push(entry)
    }

    fn replace_progress(&self, id: &str, entry: Value) {
        let mut entries = self.buffer(activity_stream(&entry)).lock().unwrap();
        if let Some(existing) = entries
            .entries.iter_mut()
            .find(|item| item["progress_id"].as_str() == Some(id))
        {
            *existing = entry;
        } else {
            entries.entries.push(entry);
        }
    }

    fn retain(&self, mut keep: impl FnMut(&Value) -> bool) {
        for stream in [&self.online, &self.local, &self.downloads] {
            let mut buffer = stream.lock().unwrap();
            buffer.entries.retain(|entry| keep(entry));
        }
    }

    fn clear(&self, stream: &str) {
        if stream == "all" {
            *self.online.lock().unwrap() = ActivityStreamBuffer::default();
            *self.local.lock().unwrap() = ActivityStreamBuffer::default();
            *self.downloads.lock().unwrap() = ActivityStreamBuffer::default();
        } else {
            *self.buffer(stream).lock().unwrap() = ActivityStreamBuffer::default();
        }
    }

    fn load(&self, entries: Vec<Value>) {
        self.clear("all");
        for entry in entries {
            self.push(entry);
        }
    }

    fn snapshot(&self) -> Vec<Value> {
        let mut entries = Vec::new();
        entries.extend(self.online.lock().unwrap().snapshot());
        entries.extend(self.local.lock().unwrap().snapshot());
        entries.extend(self.downloads.lock().unwrap().snapshot());
        entries.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
        entries
    }
}

pub struct Backend {
    pub db: Mutex<Option<Arc<TursoDb>>>,
    pub active_job_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub active_job: Mutex<Option<Value>>,
    pub online_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub online_job: Mutex<Option<Value>>,
    pub download_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub download_job: Mutex<Option<Value>>,
    pub download_monitor: Mutex<HashMap<String, Value>>,
    quit_prompt: AtomicBool,
    quit_approved: AtomicBool,
    pub logs: ActivityBuffers,
    pub log_epochs: Arc<[AtomicU64; 3]>,
    pub log_persist_gate: Arc<tokio::sync::Mutex<()>>,
    log_pending: Arc<AtomicU64>,
    log_scheduled: Arc<AtomicU64>,
    log_completed: Arc<AtomicU64>,
    log_flushed: Arc<tokio::sync::Notify>,
    log_write_queue: Arc<Mutex<VecDeque<(Value, usize, u64)>>>,
    log_writer_active: Arc<AtomicBool>,
    pub previews: Mutex<HashMap<String, Value>>,
    progress_estimates: Mutex<progress::Progress>,
    progress_clock: std::time::Instant,
    pub dispatch_gate: tokio::sync::Mutex<()>,
    read_gates: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    pub view_cache: Mutex<HashMap<String, (u64, std::time::Instant, Value)>>,
    pub pending_pkce: Mutex<Option<crate::stream_download::PkceFlow>>,
    pub persist_logs: Arc<AtomicBool>,
}

impl Default for Backend {
    fn default() -> Self {
        Self {
            db: Mutex::new(None),
            active_job_cancel: Mutex::new(None),
            active_job: Mutex::new(None),
            online_cancel: Mutex::new(None),
            online_job: Mutex::new(None),
            download_cancel: Mutex::new(None),
            download_job: Mutex::new(None),
            download_monitor: Mutex::new(HashMap::new()),
            quit_prompt: AtomicBool::new(false),
            quit_approved: AtomicBool::new(false),
            logs: ActivityBuffers::default(),
            log_epochs: Arc::new([AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)]),
            log_persist_gate: Arc::new(tokio::sync::Mutex::new(())),
            log_pending: Arc::new(AtomicU64::new(0)),
            log_scheduled: Arc::new(AtomicU64::new(0)),
            log_completed: Arc::new(AtomicU64::new(0)),
            log_flushed: Arc::new(tokio::sync::Notify::new()),
            log_write_queue: Arc::new(Mutex::new(VecDeque::new())),
            log_writer_active: Arc::new(AtomicBool::new(false)),
            previews: Mutex::new(HashMap::new()),
            progress_estimates: Mutex::new(progress::Progress::default()),
            progress_clock: std::time::Instant::now(),
            dispatch_gate: tokio::sync::Mutex::new(()),
            read_gates: Mutex::new(HashMap::new()),
            view_cache: Mutex::new(HashMap::new()),
            pending_pkce: Mutex::new(None),
            persist_logs: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl Backend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_db(&self, db: Arc<TursoDb>) {
        *self.db.lock().unwrap() = Some(db);
    }

    fn read_gate(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        // Coalesce identical requests without making a large catalogue rebuild
        // hold up unrelated queue, file or settings views. Weak entries expire
        // once the last request finishes, so changing filters cannot leak gates.
        let mut gates = self.read_gates.lock().unwrap();
        if let Some(gate) = gates.get(key).and_then(Weak::upgrade) { return gate; }
        if gates.len() >= 64 { gates.retain(|_, gate| gate.strong_count() > 0); }
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        gates.insert(key.to_owned(), Arc::downgrade(&gate));
        gate
    }

    pub fn log(&self, msg: &str) {
        self.log_with_category(msg, "info", None);
    }

    pub fn log_for(&self, kind: &str, message: &str, level: &str) {
        let (slot, category) = if kind == "download" { (&self.download_job, "download") }
            else if is_online_job(kind) { (&self.online_job, "online") }
            else { (&self.active_job, "local") };
        let job = slot.lock().unwrap().clone().filter(|job| matches!(job["status"].as_str(), Some("running"|"cancelling")));
        self.log_with_job(message, level, Some(category), job.as_ref());
    }

    fn activity_epochs(&self) -> Value {
        json!(self.log_epochs.iter().map(|epoch| epoch.load(Ordering::SeqCst)).collect::<Vec<_>>())
    }

    async fn restore_activity(&self, db: &TursoDb) {
        if !self.logs.snapshot().is_empty() { return; }
        let before = self.activity_epochs();
        if let Ok(saved) = db.load_recent_logs(500).await {
            // A slow read must not replace logs from a newly started job, or
            // restore a panel explicitly cleared while this read was pending.
            for entry in saved {
                let index = activity_stream_index(activity_stream(&entry));
                if before[index] == json!(self.log_epochs[index].load(Ordering::SeqCst)) {
                    self.logs.push(entry);
                }
            }
        }
    }

    pub fn log_with_category(&self, msg: &str, level: &str, category: Option<&str>) {
        let stream = activity_stream(&json!({"message":msg,"category":category}));
        let slot = match stream { "online" => &self.online_job, "downloads" => &self.download_job, _ => &self.active_job };
        let context = slot.try_lock().ok().and_then(|job| job.clone()).filter(|job| matches!(job["status"].as_str(), Some("running"|"cancelling")));
        self.log_with_job(msg, level, category, context.as_ref());
    }

    fn log_with_job(&self, msg: &str, level: &str, category: Option<&str>, job: Option<&Value>) {
        let cat = category.unwrap_or_else(|| {
            let lower = msg.to_lowercase();
            if lower.contains("error") || lower.contains("failed") || lower.contains("fail") {
                "error"
            } else if lower.contains("download")
                || lower.contains("fetching track")
                || lower.contains("saving track")
                || lower.contains("streamrip")
            {
                "download"
            } else if lower.contains("scan")
                || lower.contains("read tags")
                || lower.contains("indexed")
                || lower.contains("refresh local")
            {
                "scan"
            } else if lower.contains("link")
                || lower.contains("catalogue")
                || lower.contains("match")
                || lower.contains("artist")
            {
                "linking"
            } else if lower.contains("trash")
                || lower.contains("duplicate")
                || lower.contains("clean")
                || lower.contains("consolidation")
                || lower.contains("re-scan")
            {
                "cleanup"
            } else {
                "general"
            }
        });
        let at = chrono::Utc::now().to_rfc3339();
        let log_level = if cat == "error" || level == "error" {
            "error"
        } else {
            level
        };
        let entry = json!({
            "job_id":job.map(|j| &j["id"]), "job_kind":job.map(|j| &j["kind"]), "job_status":job.map(|j| &j["status"]),
            "at": &at,
            "message": msg,
            "level": log_level,
            "category": cat
        });
        self.record_activity(entry);
    }

    fn record_activity(&self, entry: Value) {
        let entry = self.logs.push(entry);

        // Persist to database if initialized and logging persistence is enabled
        if self.persist_logs.load(Ordering::SeqCst) {
            if let Some(db) = self.db.lock().unwrap().clone() {
                let stream = activity_stream(&entry);
                let index = activity_stream_index(stream);
                let epoch = self.log_epochs[index].load(Ordering::SeqCst);
                let epochs = self.log_epochs.clone();
                let gate = self.log_persist_gate.clone();
                let pending = self.log_pending.clone();
                let completed = self.log_completed.clone();
                let flushed = self.log_flushed.clone();
                let queue = self.log_write_queue.clone();
                let writer_active = self.log_writer_active.clone();
                let mut queued = queue.lock().unwrap();
                pending.fetch_add(1, Ordering::SeqCst);
                self.log_scheduled.fetch_add(1, Ordering::SeqCst);
                queued.push_back((entry, index, epoch));
                let start_writer = !writer_active.swap(true, Ordering::SeqCst);
                drop(queued);
                if start_writer {
                    tauri::async_runtime::spawn(async move {
                        loop {
                            let batch: Vec<_> = {
                                let mut queued = queue.lock().unwrap();
                                let count = queued.len().min(64);
                                let batch = queued.drain(..count).collect();
                                if count == 0 { writer_active.store(false, Ordering::SeqCst); }
                                batch
                            };
                            if batch.is_empty() { break; }
                            let _guard = gate.lock().await;
                            let entries: Vec<_> = batch.iter().filter(|(_,index,epoch)| epochs[*index].load(Ordering::SeqCst) == *epoch).map(|(entry,_,_)| entry.clone()).collect();
                            if let Err(error) = db.log_activity_entries(&entries).await {
                                eprintln!("Could not save activity history: {error}");
                            }
                            pending.fetch_sub(batch.len() as u64, Ordering::SeqCst);
                            completed.fetch_add(batch.len() as u64, Ordering::SeqCst);
                            flushed.notify_waiters();
                        }
                    });
                }
            }
        }
    }

    async fn flush_activity(&self) {
        // Wait only for entries queued before this call; a running job can
        // continue logging without holding its history view open indefinitely.
        let target = self.log_scheduled.load(Ordering::SeqCst);
        let wait = async {
            loop {
                let notified = self.log_flushed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.log_completed.load(Ordering::SeqCst) >= target { return; }
                notified.await;
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(10), wait).await.is_err() {
            eprintln!("Activity history is still saving after 10 seconds; continuing without blocking the application");
        }
    }

    fn measure_progress(&self, job: &mut Value, message: &str) {
        if job["progress_measured"] == true { job["completed"]=Value::Null; job["total"]=Value::Null; }
        self.progress_estimates.lock().unwrap().update(job, message, self.progress_clock.elapsed().as_secs_f64());
        job["progress_updated_at"]=json!(chrono::Utc::now().timestamp_millis() as f64/1000.);
    }

    pub fn progress(&self, message: &str) {
        let job = self.active_job.lock().unwrap().clone();
        if let Some(mut job) = job {
            job["message"] = json!(message);
            self.update_job_progress(message, job);
        } else {
            self.log(message);
        }
    }

    pub fn progress_for(&self, kind: &str, message: &str) {
        if is_online_job(kind) {
            let job = self.online_job.lock().unwrap().clone();
            if let Some(mut job) = job {
                job["message"] = json!(message);
                self.update_online_job_progress(message, job);
            } else {
                self.log_with_category(message, "info", Some("online"));
            }
        } else {
            self.progress(message);
        }
    }

    pub fn start_online_job(&self, job: Value, cancel: Arc<AtomicBool>) {
        if let Some(message) = job["message"].as_str() {
            self.log_with_job(message, "info", Some("online"), Some(&job));
        }
        *self.online_cancel.lock().unwrap() = Some(cancel);
        *self.online_job.lock().unwrap() = Some(job);
    }

    pub fn update_online_job_progress(&self, message: &str, mut job: Value) -> Value {
        let mut current=self.online_job.lock().unwrap();
        if let Some(saved)=current.as_ref() {
            if saved["id"] != job["id"] || matches!(saved["status"].as_str(),Some("complete"|"cancelled"|"failed")) { return saved.clone(); }
        }
        job["message"] = json!(message);
        self.measure_progress(&mut job, message);
        if self.online_cancel.lock().unwrap().as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {job["status"]=json!("cancelling");}
        let id = job["id"].as_str().unwrap_or("online").to_string();
        if job["kind"] != "discography" && current.as_ref().and_then(|j|j["message"].as_str()) != Some(message) {
            self.log_with_job(message, "info", Some("online"), Some(&job));
        }
        if job["kind"] != "discography" {
            let entry = json!({"at":chrono::Utc::now().to_rfc3339(),"message":message,"level":"info","category":"online","progress_id":id,"job_id":id,"job_kind":job["kind"],"job_status":job["status"]});
            self.logs.replace_progress(&id, entry);
        }
        *current = Some(job.clone());
        job
    }

    fn refresh_artist_log(&self, artist_id: &str, message: &str, level: &str) {
        self.refresh_artist_log_status(artist_id, message, level, if level == "error" { "failed" } else { "running" });
    }

    fn refresh_artist_log_status(&self, artist_id: &str, message: &str, level: &str, status: &str) {
        let Some(job) = self.online_job.lock().unwrap().clone() else { return; };
        let at = chrono::Utc::now().to_rfc3339();
        self.record_activity(json!({"at":at,"updated_at":at,"message":message,"level":level,
            "category":"online","progress_id":format!("{}:artist:{artist_id}",job["id"].as_str().unwrap_or("refresh")),
            "job_id":job["id"],"job_kind":job["kind"],"job_status":status}));
    }

    pub fn finish_online_job(&self, mut job: Value) {
        self.progress_estimates.lock().unwrap().finish(&mut job);
        self.view_cache.lock().unwrap().clear();
        self.logs.retain(|entry| entry["progress_id"] != job["id"]);
        if let Some(message) = job["message"].as_str().filter(|_| job["error_logged"] != true) {
            self.log_with_job(
                message,
                if job["status"] == "failed" {
                    "error"
                } else {
                    "info"
                },
                Some("online"), Some(&job),
            );
        }
        *self.online_job.lock().unwrap() = Some(job);
        *self.online_cancel.lock().unwrap() = None;
    }

    pub fn cancel_online_job(&self) -> Option<Value> {
        self.online_cancel
            .lock()
            .unwrap()
            .as_ref()?
            .store(true, Ordering::Relaxed);
        let mut lock = self.online_job.lock().unwrap();
        let job = lock.as_mut()?;
        job["status"] = json!("cancelling");
        job["message"] = json!("Cancelling online task after the current request");
        Some(job.clone())
    }

    pub fn start_job(&self, job: Value, cancel_flag: Arc<AtomicBool>) {
        if let Some(msg) = job.get("message").and_then(|v| v.as_str()) {
            let kind = job.get("kind").and_then(|v| v.as_str());
            let cat = match kind {
                Some("scan") => Some("scan"),
                Some("download") => Some("download"),
                Some("link") => Some("linking"),
                Some(k)
                    if k.contains("duplicate")
                        || k.contains("organise")
                        || k.contains("correct") =>
                {
                    Some("cleanup")
                }
                _ => None,
            };
            self.log_with_job(msg, "info", cat.or(Some("local")), Some(&job));
        }
        *self.active_job_cancel.lock().unwrap() = Some(cancel_flag);
        *self.active_job.lock().unwrap() = Some(job);
    }

    pub fn start_download_job(&self, job: Value, cancel_flag: Arc<AtomicBool>) {
        self.log_with_job("Download started", "info", Some("download"), Some(&job));
        *self.download_cancel.lock().unwrap() = Some(cancel_flag);
        *self.download_job.lock().unwrap() = Some(job);
    }

    fn record_download_monitor(&self, id: &str, mut item: Value) -> Option<Value> {
        let job = self.download_job.lock().unwrap().clone()?;
        if job["id"] != id || !matches!(job["status"].as_str(), Some("running"|"cancelling")) { return None; }
        item["job_id"] = json!(id);
        item["job_kind"] = json!("download");
        item["at"] = json!(chrono::Utc::now().to_rfc3339());
        item["updated_at"] = json!(chrono::Utc::now().timestamp_millis());
        let key = if item["kind"] == "batch" { format!("{id}:batch:{}", item["release_id"].as_str().unwrap_or("")) }
            else { format!("{id}:track:{}:{}", item["release_id"].as_str().unwrap_or(""), item["id"].as_str().unwrap_or("")) };
        let mut monitor = self.download_monitor.lock().unwrap();
        let previous = monitor.get(&key).cloned().unwrap_or(json!({}));
        let mut merged = previous.clone();
        if let (Some(fields), Some(update)) = (merged.as_object_mut(), item.as_object()) { fields.extend(update.clone()); }
        if monitor.len() >= 1000 && !monitor.contains_key(&key) {
            if let Some(oldest) = monitor.iter().min_by_key(|(_,value)|value["at"].as_str().unwrap_or("")).map(|(key,_)|key.clone()) { monitor.remove(&oldest); }
        }
        monitor.insert(key, merged.clone());
        drop(monitor);
        if item["kind"] == "track" && item["status"] != previous["status"] && matches!(item["status"].as_str(), Some("complete"|"failed"|"already downloaded")) {
            let outcome = match item["status"].as_str() { Some("complete")=>"Downloaded", Some("failed")=>"Download failed", _=>"Existing file retained" };
            self.log_with_job(&format!("{outcome} · {} · release {} · track {}{}",item["title"].as_str().unwrap_or("Track"),item["release_id"].as_str().unwrap_or(""),item["id"].as_str().unwrap_or(""),item["error"].as_str().map(|error|format!(" · {error}")).unwrap_or_default()),if item["status"] == "failed" {"error"} else {"info"},Some("download"),Some(&job));
        }
        Some(merged)
    }

    pub fn update_download_job(&self, mut job: Value) -> Value {
        let mut current=self.download_job.lock().unwrap();
        if let Some(saved)=current.as_ref() {
            if saved["id"] != job["id"] || matches!(saved["status"].as_str(),Some("complete"|"cancelled"|"failed")) { return saved.clone(); }
        }
        let message=job["message"].as_str().unwrap_or("").to_owned();
        self.measure_progress(&mut job, &message);
        if current.as_ref().and_then(|saved| saved["message"].as_str()) != Some(message.as_str()) {
            self.log_with_job(&message, if message.to_lowercase().contains("failed") {"error"} else {"info"}, Some("download"), Some(&job));
        }
        if self
            .download_cancel
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            job["status"] = json!("cancelling");
        }
        *current = Some(job.clone());
        job
    }

    pub fn finish_download_job(&self, mut job: Value) {
        self.progress_estimates.lock().unwrap().finish(&mut job);
        if let Some(message) = job["message"].as_str() {
            self.log_with_job(
                message,
                if job["status"] == "failed" {
                    "error"
                } else {
                    "info"
                },
                Some("download"), Some(&job),
            );
        }
        *self.download_job.lock().unwrap() = Some(job.clone());
        *self.download_cancel.lock().unwrap() = None;
        for item in self.download_monitor.lock().unwrap().values_mut().filter(|item| item["job_id"] == job["id"]) {
            if matches!(item["status"].as_str(), Some("running"|"preparing"|"downloading"|"staged")) {
                item["status"] = job["status"].clone();
                item["updated_at"] = json!(chrono::Utc::now().timestamp_millis());
            }
        }
        self.view_cache.lock().unwrap().clear();
    }

    pub fn cancel_download_job(&self) -> Option<Value> {
        let flag = self.download_cancel.lock().unwrap().clone()?;
        flag.store(true, Ordering::Relaxed);
        let mut job = self.download_job.lock().unwrap();
        let current = job.as_mut()?;
        current["status"] = json!("cancelling");
        current["message"] = json!("Cancelling after the current safe file boundary");
        Some(current.clone())
    }

    pub fn update_job_progress(&self, msg: &str, mut job: Value) -> Value {
        let mut current=self.active_job.lock().unwrap();
        if let Some(saved)=current.as_ref() {
            if saved["id"] != job["id"] || matches!(saved["status"].as_str(),Some("complete"|"cancelled"|"failed")) { return saved.clone(); }
        }
        job["message"] = json!(msg);
        self.measure_progress(&mut job, msg);
        if self
            .active_job_cancel
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
        {
            job["status"] = json!("cancelling");
        }
        let kind = job.get("kind").and_then(|v| v.as_str());
        let cat = match kind {
            Some("scan") => Some("scan"),
            Some("download") => Some("download"),
            Some("link") => Some("linking"),
            Some(k)
                if k.contains("duplicate") || k.contains("organise") || k.contains("correct") =>
            {
                Some("cleanup")
            }
            _ => None,
        };
        if current.as_ref().and_then(|j|j["message"].as_str()) != Some(msg) {
            self.log_with_job(msg, if msg.to_lowercase().contains("failed") {"error"} else {"info"}, cat.or(Some("local")), Some(&job));
        }
        let id = job["id"].as_str().unwrap_or("progress");
        let entry = json!({"at":chrono::Utc::now().to_rfc3339(),"message":msg,"level":"info","category":cat.unwrap_or("local"),"progress_id":id,"job_id":id,"job_kind":job["kind"],"job_status":job["status"]});
        self.logs.replace_progress(id, entry);
        *current = Some(job.clone());
        job
    }

    pub fn finish_job(&self, mut final_job: Value) {
        self.progress_estimates.lock().unwrap().finish(&mut final_job);
        self.view_cache.lock().unwrap().clear();
        self.logs
            .retain(|entry| entry["progress_id"] != final_job["id"]);
        if let Some(msg) = final_job.get("message").and_then(|v| v.as_str()) {
            let kind = final_job.get("kind").and_then(|v| v.as_str());
            let status = final_job.get("status").and_then(|v| v.as_str());
            let cat = match kind {
                Some("scan") => Some("scan"),
                Some("download") => Some("download"),
                Some("link") => Some("linking"),
                Some(k)
                    if k.contains("duplicate")
                        || k.contains("organise")
                        || k.contains("correct") =>
                {
                    Some("cleanup")
                }
                _ => None,
            };
            let lvl = if status == Some("failed") {
                "error"
            } else {
                "info"
            };
            self.log_with_job(msg, lvl, cat.or(Some("local")), Some(&final_job));
        }
        *self.active_job.lock().unwrap() = Some(final_job);
        *self.active_job_cancel.lock().unwrap() = None;
    }

    pub fn cancel_active_job(&self, cancel_msg: &str) -> Option<Value> {
        let cancel_flag = self.active_job_cancel.lock().unwrap().clone();
        if let Some(flag) = cancel_flag {
            flag.store(true, Ordering::Relaxed);
            let job = self.active_job.lock().unwrap().clone();
            self.log_with_job(cancel_msg, "warn", Some("local"), job.as_ref());
            let mut lock = self.active_job.lock().unwrap();
            if let Some(active) = lock.as_mut() {
                if let Some(obj) = active.as_object_mut() {
                    obj.insert("status".to_string(), json!("cancelling"));
                    obj.insert("message".to_string(), json!(cancel_msg));
                }
                return Some(active.clone());
            }
        }
        None
    }
}

async fn preview_inputs(db: &TursoDb, operation: &str) -> Result<Value, String> {
    Ok(json!({
        "local_revision":db.local_revision.load(Ordering::SeqCst),
        "catalogue_revision":if operation == "numbers" {Some(db.revision.load(Ordering::SeqCst))} else {None},
        "template":if operation == "organise" {db.get_preference("organisation").await?.unwrap_or(Value::Null)["template"].clone()} else {Value::Null}
    }))
}

fn refresh_runtime_state(state: &Backend, snapshot: &mut Value) {
    // Database reads may finish after a worker. Always take live job state at
    // response time so a slow dashboard read cannot resurrect a completed job.
    if let Some(job)=state.active_job.lock().unwrap().clone() {snapshot["job"]=job;}
    snapshot["online_job"]=json!(state.online_job.lock().unwrap().clone());
    snapshot["download_job"]=json!(state.download_job.lock().unwrap().clone());
    snapshot["logs"]=json!(state.logs.snapshot());
    snapshot["activity_epochs"] = state.activity_epochs();
    snapshot["download_monitor"] = json!(state.download_monitor.lock().unwrap().clone());
    snapshot["auth_url"]=state.pending_pkce.lock().unwrap().as_ref()
        .map(|flow|json!(flow.login_url)).unwrap_or(Value::Null);
}

async fn handle_rpc_call(
    app_handle: Option<&tauri::AppHandle>,
    state: &Arc<Backend>,
    db: &TursoDb,
    method: String,
    args: Value,
) -> Result<Value, String> {
    if method == "job.start" && state.quit_prompt.load(Ordering::SeqCst) {
        return Err("Finish the quit confirmation before starting another task".into());
    }

    let cacheable = matches!(method.as_str(), "table" | "table.facets" | "state" | "mqa.selection");
    let key = format!("{method}:{}", args);
    let gate = cacheable.then(|| state.read_gate(&key));
    let _read = match gate.as_ref() { Some(gate) => Some(gate.lock().await), None => None };
    let revision = db.revision.load(Ordering::SeqCst);
    if cacheable {
        let saved = state.view_cache.lock().unwrap().get(&key).cloned();
        if let Some((rev, created, mut value)) = saved {
            if rev == revision && created.elapsed().as_secs() < 30 {
                if method == "state" {
                    refresh_runtime_state(state,&mut value);
                }
                return Ok(value);
            }
        }
    }
    let result = handle_rpc_uncached(app_handle, state, db, method.clone(), args).await;
    if cacheable {
        if let Ok(ref value) = result {
            if db.revision.load(Ordering::SeqCst) == revision {
                let mut cache = state.view_cache.lock().unwrap();
                if cache.len() >= 64 {
                    cache.clear();
                }
                cache.insert(key, (revision, std::time::Instant::now(), value.clone()));
            }
        }
    } else if (method == "job.start" || method == "job.cancel")
        || method.starts_with("auth.")
        || method.starts_with("settings.")
        || method.starts_with("queue.")
        || method.starts_with("links.")
        || method.starts_with("artists.")
        || method.starts_with("files.")
    {
        state.view_cache.lock().unwrap().clear();
    }
    result
}

async fn handle_rpc_uncached(
    app_handle: Option<&tauri::AppHandle>,
    state: &Arc<Backend>,
    db: &TursoDb,
    method: String,
    mut args: Value,
) -> Result<Value, String> {
    let method = if method == "missing.rebuild" { args = json!({"kind":"cached_releases","args":args}); "job.start".to_string() } else { method };
    if method == "job.status" {
        return Ok(
            json!({"job":state.active_job.lock().unwrap().clone(),"online_job":state.online_job.lock().unwrap().clone(),"download_job":state.download_job.lock().unwrap().clone(),"logs":state.logs.snapshot(),"activity_epochs":state.activity_epochs(),"download_monitor":state.download_monitor.lock().unwrap().clone(),"auth_url":state.pending_pkce.lock().unwrap().as_ref().map(|f|f.login_url.clone())}),
        );
    }
    if method == "appearance.accent" {
        return serde_json::to_value(appearance::system_accent(app_handle).await).map_err(|error| error.to_string());
    }
    if matches!(method.as_str(), "table" | "table.facets" | "detail" | "job.start" | "mqa.selection")
        || method.starts_with("turso.") || method.starts_with("release.")
        || method.starts_with("artists.") || method.starts_with("links.") || method.starts_with("queue.") {
        let preferences = db.get_settings().await?;
        let mut market = preferences["general"]["market"]
            .as_str()
            .unwrap_or("GB")
            .to_uppercase();
        if (method == "job.start" && (is_online_job(args["kind"].as_str().unwrap_or("")) || args["kind"] == "download")
                && !matches!(args["kind"].as_str(),Some("connections"|"connect_account"|"connect_download")))
            || matches!(method.as_str(),"turso.discography"|"turso.tidal.artist"|"turso.tidal.search") {
            let http = network::client(20)?;
            market = account::ensure_account_market(db, &http, None).await?;
        }
        let account_market = db.get_preference("subscriber-account-market").await?
            .is_some_and(|saved| saved["country_code"].as_str().and_then(account::normalize_market).is_some());
        if let Some(obj) = args.as_object_mut() {
            if account_market { obj.insert("market".into(),json!(market)); }
            else { obj.entry("market").or_insert(json!(market)); }
        }
        if let Some(obj) = args.get_mut("args").and_then(Value::as_object_mut) {
            if account_market { obj.insert("market".into(),json!(market)); }
            else { obj.entry("market").or_insert(json!(market)); }
        }
    }
    let _dispatch = if matches!(method.as_str(),"job.start"|"auth.reply"|"turso.tags.write"|"turso.scan"|"turso.discography") {
        Some(state.dispatch_gate.lock().await)
    } else {
        None
    };
    if method == "job.start" {
        let kind = args["kind"].as_str().unwrap_or("");
        if kind == "download" && state.active_job.lock().unwrap().as_ref().is_some_and(|job|
            matches!(job["status"].as_str(), Some("running"|"cancelling"))
            && matches!(job["kind"].as_str(), Some("apply"|"deep_apply"|"consolidate"))) {
            return Err("Files are being updated. Downloads will be available when that operation finishes.".into());
        }
        if matches!(kind, "apply"|"deep_apply"|"consolidate") && state.download_cancel.lock().unwrap().is_some() {
            return Err("A download is writing files. Wait for it to finish or cancel it before changing files.".into());
        }
    }
    if method == "job.start" && args["kind"] != "download" {
        let kind = args["kind"].as_str().unwrap_or("");
        let occupied = if is_online_job(kind) {
            state.online_cancel.lock().unwrap().is_some()
        } else {
            state.active_job_cancel.lock().unwrap().is_some()
        };
        if occupied {
            return Err("A task in this section is already running. Wait for completion or cancel it first.".into());
        }
        if matches!(kind,"link"|"match_artists"|"metadata"|"artwork"|"deep_review"|"deep_preview"|"manual_candidate")
            && state.active_job.lock().unwrap().as_ref().is_some_and(|job|
                matches!(job["status"].as_str(),Some("running"|"cancelling"))
                && matches!(job["kind"].as_str(),Some("scan"|"apply"|"deep_apply"|"consolidate"))) {
            return Err("The local index is being updated. This action will be available when it finishes.".into());
        }
    }
    if method == "job.start" && actions::handles(args["kind"].as_str().unwrap_or("")) {
        let kind = args["kind"].as_str().unwrap().to_string();
        let input = args.get("args").cloned().unwrap_or_else(|| args.clone());
        let preview_source = if kind == "preview" {Some(preview_inputs(db,input["action"].as_str().unwrap_or("dates")).await?)} else {None};
        let action_root = input["root"].as_str().unwrap_or("").to_owned();
        let local_revision = db.local_revision.load(Ordering::SeqCst);
        let id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.;
        let cancel = Arc::new(AtomicBool::new(false));
        let initial = json!({"id":id,"kind":kind,"status":"running","message":format!("Started · {}", workflow_name(&kind)),"started":started});
        if is_online_job(&kind) {
            state.start_online_job(initial.clone(), cancel.clone());
        } else {
            state.start_job(initial.clone(), cancel.clone());
        }
        let backend = state.clone();
        let database = db.clone();
        let app = app_handle.cloned();
        let runtime = tokio::runtime::Handle::current();
        tauri::async_runtime::spawn(async move {
            let b = backend.clone();
            let d = database.clone();
            let k = kind.clone();
            let c = cancel.clone();
            let task = tokio::task::spawn_blocking(move || {
                runtime.block_on(actions::execute(&d, &b, &k, &input, c))
            })
            .await;
            let result = task.unwrap_or_else(|e| Err(format!("Worker failed: {e}")));
            let (status, message, value) = match result {
                Ok(v) => (
                    if cancel.load(Ordering::Relaxed) {
                        "cancelled"
                    } else {
                        "complete"
                    },
                    v["message"].as_str().map(str::to_owned).unwrap_or_else(|| workflow_outcome(&kind, &v, cancel.load(Ordering::Relaxed))),
                    v,
                ),
                Err(e) => (
                    if cancel.load(Ordering::Relaxed) {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    e,
                    Value::Null,
                ),
            };
            if let Some(preview_id) = value["preview_id"].as_str() {
                let mut previews=backend.previews.lock().unwrap();
                if let (Some(source),Some(p))=(preview_source,previews.get_mut(preview_id)) {p["source_inputs"]=source;}
                let operation=previews.get(preview_id).map(|p|p["operation"].clone());
                previews.retain(|key,p|key==preview_id || p["root"] != action_root || Some(&p["operation"]) != operation.as_ref());
            }
            let library_mutated = database.local_revision.load(Ordering::SeqCst) != local_revision
                && matches!(kind.as_str(), "apply" | "deep_apply" | "consolidate");
            if library_mutated {backend.previews.lock().unwrap().retain(|_,p|p["root"]!=action_root);}
            let finished = json!({"id":id,"kind":kind,"status":status,"message":message,"started":started,"finished":chrono::Utc::now().timestamp_millis() as f64/1000.,"result":value});
            if is_online_job(&kind) {
                backend.finish_online_job(finished.clone());
            } else {
                backend.finish_job(finished.clone());
            }
            if !matches!(kind.as_str(),"cached_releases"|"preview") && value["reused"] != true { database.bump_revision(); }
            let _ = database.set_preference("desktop-last-job", &finished).await;
            if let Some(app) = app {
                let _ = app.emit(
                    "backend-event",
                    if is_online_job(&kind) {
                        json!({"event":"job","online_job":finished})
                    } else {
                        json!({"event":"job","job":finished})
                    },
                );
                let _ = app.emit("backend-event", json!({"event":if library_mutated {"library-mutated"} else {"changed"}}));
            }
        });
        return Ok(initial);
    }
    if method == "turso.ping" {
        return Ok(json!({
            "status": "ok",
            "engine": "turso",
            "db_path": db.path.display().to_string(),
        }));
    }
    if method == "turso.stats" {
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let root = args.get("root").and_then(|v| v.as_str());
        let stats = db.get_stats(market, root).await?;
        return serde_json::to_value(stats).map_err(|e| e.to_string());
    }
    if method == "turso.roots" {
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let roots = db.list_roots(market).await?;
        return serde_json::to_value(roots).map_err(|e| e.to_string());
    }
    if method == "turso.files" {
        let root = args.get("root").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let (files, total) = db.get_local_files_page(root, limit, offset).await?;
        return Ok(json!({ "rows": files, "total": total }));
    }
    if method == "turso.tidal.ping" {
        let client_opt = tidal::TidalClient::from_db(db).await.ok();
        let mut client = match client_opt {
            Some(c) => c,
            None => {
                return Err(
                    "Connect your streaming account in Settings."
                        .to_string(),
                )
            }
        };
        client.authenticate().await?;
        return Ok(json!({
            "status": "ok",
            "authenticated": true,
        }));
    }
    if method == "turso.tidal.search" {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing query".to_string())?;
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let client_opt = tidal::TidalClient::from_db(db).await.ok();
        let mut client = match client_opt {
            Some(c) => c,
            None => {
                return Err(
                    "Connect your streaming account in Settings."
                        .to_string(),
                )
            }
        };
        let results = client.search_artists(query, market).await?;
        return Ok(serde_json::to_value(results).unwrap_or_default());
    }
    if method == "turso.tidal.artist" {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing artist id".to_string())?;
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let detailed = args
            .get("detailed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let client_opt = tidal::TidalClient::from_db(db).await.ok();
        let mut client = match client_opt {
            Some(c) => c,
            None => {
                return Err(
                    "Connect your streaming account in Settings."
                        .to_string(),
                )
            }
        };
        let mut catalogue = client.get_artist_catalogue(id, market, false).await?;
        if detailed {
            client.save_catalogue_to_db(db,market,&catalogue).await?;
            for release in &mut catalogue.releases {
                *release=serde_json::from_value(actions::release(db,&release.id,market,false).await?).map_err(|e|e.to_string())?;
            }
        }
        return Ok(serde_json::to_value(catalogue).unwrap_or_default());
    }
    if method == "turso.tags.write" {
        if state.active_job_cancel.lock().unwrap().is_some() {
            return Err("A local task is already running. Wait for completion or cancel it first.".into());
        }
        if state.download_cancel.lock().unwrap().is_some() {
            return Err("A download is writing files. Wait for it to finish or cancel it before changing tags.".into());
        }
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing path".to_string())?;
        let updates_val = args
            .get("tags")
            .and_then(|v| v.as_object())
            .ok_or_else(|| "Missing tags object".to_string())?;
        let mut updates = std::collections::HashMap::new();
        for (k, v) in updates_val {
            if let Some(s) = v.as_str() {
                updates.insert(k.clone(), s.to_string());
            }
        }
        let conn=db.connect()?;
        let mut registered=conn.query("SELECT root FROM local_files WHERE path=? AND present=1",(path_str,)).await.map_err(|e|e.to_string())?;
        let root=registered.next().await.map_err(|e|e.to_string())?
            .and_then(|row|row.get::<String>(0).ok()).ok_or("Index this file in a registered library before changing its tags")?;
        drop(registered);
        let count=updates.len();
        let item=maintenance::FileApplyItem{path:path_str.to_owned(),target:None,artwork:None,tags:updates};
        let database=db.clone();
        let worker_root=root.clone();
        let runtime=tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move ||runtime.block_on(maintenance::apply_file_item(&database,&worker_root,&item)))
            .await.map_err(|e|format!("Tag worker failed: {e}"))??;
        state.previews.lock().unwrap().retain(|_,p|p["root"]!=root);
        state.view_cache.lock().unwrap().clear();
        if let Some(app)=app_handle {let _=app.emit("backend-event",json!({"event":"library-mutated"}));}
        return Ok(json!({ "status": "ok", "updated": count }));
    }
    if method == "turso.organisation.preview" {
        let root_str = args.get("root").and_then(|v| v.as_str()).unwrap_or("");
        let template = args.get("template").and_then(|v| v.as_str());
        let ext = args
            .get("extension")
            .and_then(|v| v.as_str())
            .unwrap_or("flac");
        let tags_val = args
            .get("tags")
            .and_then(|v| v.as_object())
            .ok_or_else(|| "Missing tags".to_string())?;
        let mut tags = std::collections::HashMap::new();
        for (k, v) in tags_val {
            if let Some(s) = v.as_str() {
                tags.insert(k.clone(), s.to_string());
            }
        }
        let target =
            organisation::format_layout(std::path::Path::new(root_str), &tags, template, ext)?;
        return Ok(json!({ "target": target.display().to_string() }));
    }
    if method == "turso.keys.canonical" {
        let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let canonical = musical_keys::canonical_key(key);
        let camelot = musical_keys::camelot_key(key);
        return Ok(json!({ "canonical": canonical, "camelot": camelot }));
    }
    if method == "mqa.selection" {
        return actions::mqa_selection(db, &args).await;
    }
    if method == "turso.mqa.audit" {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing path".to_string())?;
        let path=std::path::PathBuf::from(path_str);
        let meta=std::fs::metadata(&path).map_err(|e|e.to_string())?;
        let size=meta.len() as i64;
        let mtime=meta.modified().map_err(|e|e.to_string())?.duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos() as i64;
        let key=format!("mqa-audit:{path_str}");
        if args["force"] != true {
            if let Some(saved)=db.get_preference(&key).await?.filter(|saved|maintenance::valid_inspection(saved,size,mtime)) {
                return Ok(saved["result"].clone());
            }
        }
        let res=tokio::task::spawn_blocking(move ||mqa::audit_file(&path)).await.map_err(|e|format!("Audio audit worker failed: {e}"))?;
        let value=serde_json::to_value(res).map_err(|e|e.to_string())?;
        db.set_preference(&key,&json!({"size":size,"mtime":mtime,"result":value})).await?;
        db.bump_revision();
        state.view_cache.lock().unwrap().clear();
        return Ok(value);
    }
    if method == "turso.enrichment.missing" {
        let local_tags_val = args
            .get("local_tags")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        let mut local_tags = std::collections::HashMap::new();
        if let Some(obj) = local_tags_val.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    local_tags.insert(k.clone(), s.to_string());
                }
            }
        }
        let release: tidal::TidalRelease =
            serde_json::from_value(args.get("release").cloned().unwrap_or(Value::Null))
                .map_err(|e| format!("Invalid release: {}", e))?;
        let track: tidal::TidalTrack =
            serde_json::from_value(args.get("track").cloned().unwrap_or(Value::Null))
                .map_err(|e| format!("Invalid track: {}", e))?;

        let missing = enrichment::compute_missing_tags(&local_tags, &release, &track);
        return serde_json::to_value(missing).map_err(|e| e.to_string());
    }
    if method == "turso.workflows.plan" {
        let root = args.get("root").and_then(|v| v.as_str()).unwrap_or("");
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("organise");
        let template = args.get("template").and_then(|v| v.as_str());

        let files = actions::files(db, root).await?;
        let plans = workflows::plan_cached(db, &files, action, template).await?;
        return serde_json::to_value(plans).map_err(|e| e.to_string());
    }
    if method == "turso.account.status" {
        let session = account::AccountClient::load_saved_session();
        return Ok(json!({
            "logged_in": session.is_some(),
            "user_id": session.as_ref().and_then(|s| s.user_id.clone()),
            "expires_at": session.as_ref().map(|s| s.expires_at),
        }));
    }
    if method == "turso.matching.score" {
        let local_name = args
            .get("local_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let local_albums: Vec<String> = args
            .get("local_albums")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let candidate_id = args
            .get("candidate_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let candidate_name = args
            .get("candidate_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let candidate_albums: Vec<String> = args
            .get("candidate_albums")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let score = matching::score_artist_candidate(
            local_name,
            &local_albums,
            candidate_id,
            candidate_name,
            &candidate_albums,
        );
        return Ok(json!({
            "artist_id": score.artist_id,
            "artist_name": score.artist_name,
            "score": score.score,
            "matched_releases": score.matched_releases,
            "local_releases": score.local_releases,
            "exact_name": score.exact_name,
            "matched_titles": score.matched_titles,
            "evidence": score.evidence,
        }));
    }

    // STATE & SETTINGS
    if method == "state" {
        let active = state.active_job.lock().unwrap().clone();
        state.restore_activity(db).await;
        let logs = state.logs.snapshot();
        let root = args.get("root").and_then(|v| v.as_str());
        let mut snapshot = if args["bootstrap"] == true {
            db.get_initial_state(active, &logs, root).await?
        } else { db.get_state(active, &logs, root).await? };
        snapshot["catalogue_refresh"] = db.get_preference("catalogue-refresh-checkpoint").await?.unwrap_or(Value::Null);
        refresh_runtime_state(state,&mut snapshot);
        return Ok(snapshot);
    }
    if method == "settings" {
        return db.get_settings().await;
    }
    if method == "settings.save" || method == "settings.update" {
        let section = args.get("section").and_then(|v| v.as_str()).unwrap_or("ui");
        let values = args.get("values").unwrap_or(&args);
        let res = if method == "settings.update" {
            db.patch_settings(section, values).await?
        } else { db.save_settings(section, values).await? };
        if matches!(section, "desktop" | "general" | "ui") {
            if let Some(p) = res["general"]["persist_logs"].as_bool() {
                state.persist_logs.store(p, Ordering::SeqCst);
            }
        }
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(res);
    }
    if method == "settings.reset" {
        let group = args.get("group").and_then(|v| v.as_str()).unwrap_or("");
        let res = db.reset_settings(group).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(res);
    }
    if method == "logs" {
        state.restore_activity(db).await;
        return Ok(json!(state.logs.snapshot()));
    }
    if method == "logs.history" {
        // Read the durable page immediately. The frontend merges live entries
        // while the asynchronous writer catches up; ongoing jobs must never
        // delay opening their activity history.
        let kinds: Vec<String> = serde_json::from_value(args["job_kinds"].clone()).unwrap_or_default();
        return db.activity_history_filtered(args["before_id"].as_i64(), args["limit"].as_u64().unwrap_or(500) as usize, args["search"].as_str(), args["stream"].as_str(), &kinds, args["unassigned"] == true).await;
    }
    if method == "logs.clear" {
        let stream = args.get("stream").and_then(Value::as_str).unwrap_or("all");
        let _guard = state.log_persist_gate.lock().await;
        if stream == "all" {
            for epoch in state.log_epochs.iter() {
                epoch.fetch_add(1, Ordering::SeqCst);
            }
            state.logs.clear("all");
            db.clear_logs().await?;
        } else {
            if !["online", "local", "downloads"].contains(&stream) {
                return Err("Unknown activity stream".into());
            }
            state.log_epochs[activity_stream_index(stream)].fetch_add(1, Ordering::SeqCst);
            state.logs.clear(stream);
            db.clear_log_stream(stream).await?;
        }
        if stream == "all" || stream == "downloads" { state.download_monitor.lock().unwrap().clear(); }
        return Ok(json!({ "cleared": true, "stream": stream, "activity_epochs":state.activity_epochs() }));
    }

    // TABLE ROUTES
    if matches!(method.as_str(), "table" | "table.facets" | "turso.links" | "turso.missing") {
        if method == "turso.links" { args["route"] = json!("links"); }
        if method == "turso.missing" { args["route"] = json!("missing"); }
        return table_filters::table_result(state, db, &args, method == "table.facets").await;
    }

    // DETAILS & PREVIEW
    if method == "release.ensure_tracks" {
        let id = args["id"].as_str().ok_or("Choose a release")?;
        let settings = db.get_settings().await?;
        let market = args["market"].as_str().filter(|market|!market.trim().is_empty())
            .or_else(||settings["general"]["market"].as_str()).unwrap_or("GB").to_uppercase();
        let market = market.as_str();
        let current = db.get_detail(&json!({"release_id":id,"market":market})).await?;
        let title = current["title"].as_str().unwrap_or(id);
        let context = json!({"id":uuid::Uuid::new_v4().to_string(),"kind":"release_tracks","status":"running"});
        state.log_with_job(&format!("Loading selected release tracks · {title} · saved data first"), "info", Some("online"), Some(&context));
        let cancel = Arc::new(AtomicBool::new(false));
        struct CancelOnDrop(Arc<AtomicBool>);
        impl Drop for CancelOnDrop { fn drop(&mut self) { self.0.store(true, Ordering::Relaxed); } }
        let _cancel_on_drop = CancelOnDrop(cancel.clone());
        let result = actions::ensure_release_tracks(db,id,market,cancel).await;
        let mut finished = context;
        match result {
            Ok(value) => {
                finished["status"] = json!("complete");
                state.log_with_job(&format!("Selected release tracks ready · {title} · release ID {id} · {} audio tracks cached",value["track_count"]), "info", Some("online"), Some(&finished));
                state.view_cache.lock().unwrap().clear();
                if let Some(app)=app_handle { let _=app.emit("backend-event",json!({"event":"changed"})); }
                return Ok(value);
            }
            Err(error) => {
                finished["status"] = json!("failed");
                state.log_with_job(&format!("Selected release tracks could not be loaded · {title} · release ID {id}: {error}"), "error", Some("online"), Some(&finished));
                return Err(error);
            }
        }
    }
    if method == "detail" {
        let mut detail = db.get_detail(&args).await?;
        if args["check_availability"] == true {
            let market = db.get_settings().await?["general"]["market"].as_str().unwrap_or("GB").to_string();
            let ids: Vec<String> = detail["catalogue_options"].as_array().into_iter().flatten().filter_map(|o|o["id"].as_str().map(str::to_owned)).collect();
            let checked = availability::check_refresh(db,&ids,&market,args["force_availability"] == true).await;
            let mut hidden=0;
            if let Some(options)=detail["catalogue_options"].as_array_mut() {
                options.retain(|o| {let keep=checked.as_ref().ok().and_then(|c|o["id"].as_str().and_then(|id|c.get(id))).copied().flatten()==Some(true);if !keep {hidden+=1;} keep});
            }
            detail["availability_note"] = json!(match checked {Ok(_) => format!("Showing releases confirmed available in {market}. {hidden} unavailable or unverified placements omitted. Saved links are retained."), Err(e) => format!("Availability could not be confirmed: {e}. Saved links are retained; reopen this review to retry.")});
        }
        return Ok(detail);
    }
    if method == "preview" {
        let preview_id = args.get("id").and_then(|v| v.as_str()).unwrap_or("preview");
        if let Some(prev) = state.previews.lock().unwrap().get(preview_id) {
            return Ok(prev.clone());
        }
        return Ok(json!({
            "id": preview_id,
            "rows": [],
            "count": 0
        }));
    }

    // QUEUE ACTIONS
    if method == "queue.preview" {
        let page = db
            .get_queue_rows("queue", None, None, None, None, 0, usize::MAX)
            .await?;
        let rows: Vec<_> = page.rows.into_iter().filter(|row| row.approved).collect();
        return serde_json::to_value(rows).map_err(|e| e.to_string());
    }
    if method == "queue.select" {
        let sel_val = args.get("selection").cloned().unwrap_or(json!({}));
        let selection: HashMap<String, Option<Vec<String>>> =
            serde_json::from_value(sel_val).unwrap_or_default();
        db.queue_select(&selection).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "queue.decision" {
        let ids_val = args.get("ids").cloned().unwrap_or(json!([]));
        let ids: Vec<String> = serde_json::from_value(ids_val).unwrap_or_default();
        let decision = args
            .get("decision")
            .and_then(|v| v.as_str())
            .unwrap_or("queued");
        db.queue_decision(&ids, decision).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "queue.redownload" {
        let release_id = args["release_id"].as_str().ok_or("Choose a release")?;
        db.queue_redownload(release_id, args["track_id"].as_str())
            .await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({"event":"changed"}));
        }
        return Ok(json!(true));
    }
    if method == "queue.add" {
        let sel_val = args.get("selection").cloned().unwrap_or(json!({}));
        let selection: HashMap<String, Option<Vec<String>>> =
            serde_json::from_value(sel_val).unwrap_or_default();
        db.queue_add(&selection).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "queue.export" {
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("json");
        let decision = args.get("decision").and_then(|v| v.as_str());
        let selection: Option<HashMap<String, Option<Vec<String>>>> = args.get("selection")
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value(value.clone())
                .map_err(|e| format!("Invalid export selection: {e}")))
            .transpose()?;
        let text = if let Some(selection) = selection.as_ref() {
            db.queue_export_selection(format, decision, Some(selection)).await?
        } else {
            db.queue_export(format, decision).await?
        };
        return Ok(json!({ "text": text }));
    }

    // LIBRARY ACTIONS
    if method == "library.add" {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("Missing path")?;
        db.add_root(path).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!({ "root": path }));
    }
    if method == "library.remove" {
        let root = args
            .get("root")
            .and_then(|v| v.as_str())
            .ok_or("Missing root")?;
        db.remove_root(root).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }

    // TRACK & ARTIST ACTIONS
    if method == "tracks.ignore" {
        let paths: Vec<String> = args
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let ignored = args
            .get("ignored")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        db.set_local_files_ignored(&paths, ignored).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "tracks.unlink" {
        let paths: Vec<String> = args
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        db.unlink_tracks(&paths).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "tracks.choose" {
        if args["require_live"] == true {
            let market = db.get_settings().await?["general"]["market"].as_str().unwrap_or("GB").to_string();
            let id=args["album_id"].as_str().unwrap_or("").to_string();
            if availability::check(db,&[id.clone()],&market).await?.get(&id).copied().flatten()!=Some(true) {return Err("This release is unavailable or its availability cannot be confirmed".into());}
        }
        db.choose_track_link(&args).await?;
        let linked = release_anchor::propagate(db, args["path"].as_str().unwrap_or(""), args["market"].as_str().unwrap_or("GB")).await?;
        state.log_with_category(&format!("Manual placement saved · {linked} additional tracks linked from the complete cached release · file tags unchanged"), "info", Some("linking"));
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "artists.unlink" {
        let artists: Vec<String> = args["artists"].as_array().into_iter().flatten()
            .filter_map(Value::as_str).filter(|v| !v.trim().is_empty()).map(str::to_owned).collect();
        if artists.is_empty() { return Err("Select artists to unlink".into()); }
        db.unlink_artists(&artists).await?;
        state.view_cache.lock().unwrap().clear();
        state.log_with_category(&format!("Unlinked {} artist associations · recording links and files retained", artists.len()), "info", Some("online"));
        if let Some(app) = app_handle { let _ = app.emit("backend-event", json!({"event":"changed"})); }
        return Ok(json!(true));
    }
    if method == "artists.choose" {
        let artist = args.get("artist").and_then(|v| v.as_str()).unwrap_or("");
        let ids: Vec<String> = args
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if !artist.is_empty() && !ids.is_empty() {
            db.choose_artist(artist, &ids).await?;
            if let Some(app) = app_handle {
                let _ = app.emit("backend-event", json!({ "event": "changed" }));
            }
        }
        return Ok(json!(true));
    }

    // CREDENTIALS & ACCOUNT
    if method == "account.disconnect" {
        crate::account::AccountClient::disconnect().map_err(|e| e.to_string())?;
        db.set_preference("tidal_token", &Value::Null).await?;
        db.set_preference("account-disconnected", &json!(true))
            .await?;
        db.set_preference("account_connected_at", &Value::Null)
            .await?;
        db.bump_revision();
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "shutdown" {
        return Ok(
            json!({ "safe": state.active_job_cancel.lock().unwrap().is_none() && state.online_cancel.lock().unwrap().is_none() && state.download_cancel.lock().unwrap().is_none() }),
        );
    }

    // JOBS
    if method == "turso.scan"
        || (method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("scan"))
    {
        if state.active_job_cancel.lock().unwrap().is_some() {
            return Err("A job is already running".to_string());
        }
        let inner_args = args.get("args").cloned().unwrap_or(Value::Null);
        let root_opt = inner_args
            .get("root")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("root").and_then(|v| v.as_str()));
        let root_str = if let Some(r) = root_opt {
            r.to_string()
        } else {
            let roots = db.list_roots("GB").await?;
            roots.into_iter().next().map(|r| r.root).unwrap_or_default()
        };
        if root_str.is_empty() {
            return Err("Choose a registered library first".to_string());
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let initial_job = json!({
            "id": job_id,
            "kind": "scan",
            "status": "running",
            "message": "Checking local changes · unchanged file tags will be reused",
            "started": started,
            "result": null
        });

        let cancel_flag = Arc::new(AtomicBool::new(false));
        state.start_job(initial_job.clone(), cancel_flag.clone());

        let db_clone = db.clone();
        let app_clone = app_handle.cloned();
        let backend_task = state.clone();
        let j_id = job_id.clone();
        let root_path = std::path::PathBuf::from(root_str);
        let force = inner_args["force"].as_bool().unwrap_or(false);

        tauri::async_runtime::spawn(async move {
            let j_id_prog = j_id.clone();
            let app_prog = app_clone.clone();
            let backend_prog = backend_task.clone();

            let before_scan = db_clone.local_revision.load(Ordering::SeqCst);
            let scan_res = scanner::scan_library_with_options(
                &db_clone,
                &root_path,
                cancel_flag.clone(),
                force,
                move |msg| {
                    let prog_job = json!({
                        "id": j_id_prog,
                        "kind": "scan",
                        "status": "running",
                        "message": msg,
                        "started": started,
                        "result": null
                    });
                    let prog_job = backend_prog.update_job_progress(msg, prog_job.clone());
                    if let Some(ref app) = app_prog {
                        let _ = app.emit(
                            "backend-event",
                            json!({
                                "event": "progress",
                                "message": msg,
                                "job": prog_job
                            }),
                        );
                    }
                },
            )
            .await;

            let is_cancelled = cancel_flag.load(Ordering::Relaxed);
            let (status, message, result_val) = match scan_res {
                Ok(summary) => {
                    let st = if is_cancelled || summary.status == "cancelled" {
                        "cancelled"
                    } else {
                        "complete"
                    };
                    let msg = if st == "cancelled" {
                        "Refresh local files · cancelled; completed results retained".to_string()
                    } else {
                        format!(
                            "Local index updated · {} tags read · {} unchanged reused · {} newly missing · {} restored",
                            summary.read, summary.unchanged, summary.removed, summary.restored
                        )
                    };
                    (
                        st,
                        msg,
                        json!({
                            "files": summary.read + summary.unchanged,
                            "read": summary.read,
                            "unchanged": summary.unchanged,
                            "missing": summary.missing,
                            "removed": summary.removed,
                            "restored": summary.restored,
                            "errors": summary.errors,
                        }),
                    )
                }
                Err(e) => ("failed", format!("Scan failed: {}", e), json!(null)),
            };

            let library_mutated = db_clone.local_revision.load(Ordering::SeqCst) != before_scan
                || result_val["read"].as_u64().unwrap_or(0) > 0
                || result_val["removed"].as_u64().unwrap_or(0) > 0
                || result_val["restored"].as_u64().unwrap_or(0) > 0;
            if library_mutated { backend_task.previews.lock().unwrap().retain(|_, p| p["root"].as_str() != root_path.to_str()); }
            let finished_at = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
            let final_job = json!({
                "id": j_id,
                "kind": "scan",
                "status": status,
                "message": message,
                "started": started,
                "finished": finished_at,
                "result": result_val
            });

            backend_task.finish_job(final_job.clone());
            let _ = db_clone
                .set_preference("desktop-last-job", &final_job)
                .await;

            if let Some(ref app) = app_clone {
                let _ = app.emit(
                    "backend-event",
                    json!({
                        "event": "job",
                        "job": final_job
                    }),
                );
                let _ = app.emit("backend-event", json!({ "event": if library_mutated { "library-mutated" } else { "changed" } }));
            }
        });

        return Ok(initial_job);
    }
    if method == "turso.discography"
        || (method == "job.start"
            && args.get("kind").and_then(|v| v.as_str()) == Some("discography"))
    {
        let client_opt = tidal::TidalClient::from_db(db).await.ok();
        let client = match client_opt {
            Some(c) => c,
            None => {
                return Err(
                    "Connect your streaming account in Settings."
                        .to_string(),
                );
            }
        };

        if state.online_cancel.lock().unwrap().is_some() {
            return Err("A job is already running".to_string());
        }

        let inner_args = args.get("args").cloned().unwrap_or(args.clone());
        let detailed = inner_args
            .get("detailed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let market = inner_args
            .get("market")
            .and_then(|v| v.as_str())
            .unwrap_or("GB")
            .to_string();

        let mut ids: Vec<String> = if let Some(ids_val) = inner_args.get("ids") {
            if let Some(arr) = ids_val.as_array() {
                if arr.is_empty() {
                    return Err("Select a linked artist before refreshing its releases".to_string());
                }
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            db.get_linked_artist_ids().await?
        };

        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            return Err("No linked artists found in library. Match artists first.".to_string());
        }

        // Checkpoints contain only IDs/options; completed artist catalogues are already durable.
        let saved = db.get_preference("catalogue-refresh-checkpoint").await?.unwrap_or(Value::Null);
        let resume = inner_args["resume"].as_bool().unwrap_or(false);
        let completed = completed_refresh_ids(&saved, &ids, &market, detailed, resume);
        let mut names = HashMap::new();
        let conn = db.connect()?;
        let mut artists = conn.query("SELECT tidal_id,artist FROM mappings UNION SELECT tidal_id,artist FROM additional_mappings", ()).await.map_err(|e|e.to_string())?;
        while let Some(row) = artists.next().await.map_err(|e|e.to_string())? {
            if let (Ok(id),Ok(name)) = (row.get::<String>(0),row.get::<String>(1)) { names.insert(id,name); }
        }
        db.set_preference("catalogue-refresh-checkpoint", &json!({"ids":ids,"completed":completed,"market":market,"detailed":detailed,"status":"running"})).await?;
        let job_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let total = ids.len();
        let initial_job = json!({
            "id": job_id,
            "kind": "discography",
            "status": "running",
            "message": format!("Checking release lists · {}/{} artists · {}", completed.len(), total, if detailed { "new or changed track details and credits" } else { "new releases and market availability" }),
            "completed": completed.len(), "total": total,
            "started": started,
            "result": null
        });

        let cancel_flag = Arc::new(AtomicBool::new(false));
        state.start_online_job(initial_job.clone(), cancel_flag.clone());

        let db_clone = db.clone();
        let app_clone = app_handle.cloned();
        let backend_task = state.clone();
        let j_id = job_id.clone();

        tauri::async_runtime::spawn(async move {
            let mut completed = completed;
            let mut checked = completed.len();
            let mut reused_details = 0usize;
            let mut checked_details = 0usize;
            let mut failure: Option<String> = None;
            let mut failed_artist: Option<(String,String)> = None;
            let backend_prog = backend_task.clone();
            let mut last_published = None;

            // Reference albums need the same evidence as candidates, including
            // albums no longer returned on the artist's current catalogue page.
            // Cache complete work individually so cancellation/resume never
            // requires downloading the already checked credits again.
            if detailed {
                match db_clone.linked_reference_release_ids(&market, &ids).await {
                    Err(error) => failure = Some(error),
                    Ok(references) => {
                        let mut reference_checked = 0usize;
                        for batch in references.chunks(20) {
                            if failure.is_some() || cancel_flag.load(Ordering::Relaxed) { break; }
                            reference_refresh_progress(&backend_task, &format!("Downloaded reference metadata · {reference_checked}/{} releases · reusing saved details and filling missing catalogue fields and credits", references.len()), reference_checked, references.len());
                            let mut reference_client = client.clone().with_cancel(cancel_flag.clone());
                            if let Err(error) = reference_client.albums(batch, &market, false).await {
                                failure = Some(format!("Could not check downloaded release references: {error}"));
                                break;
                            }
                            if let Err(error) = reference_client.discovery(batch, &market, false).await {
                                if cancel_flag.load(Ordering::Relaxed) { break; }
                                backend_task.log_for("discography", &format!("Optional catalogue fields could not be checked; saved reference metadata retained: {error}"), "warning");
                            }
                            let mut remaining = batch.iter();
                            let mut reference_workers = tokio::task::JoinSet::new();
                            loop {
                                if failure.is_some() || cancel_flag.load(Ordering::Relaxed) { break; }
                                while reference_workers.len() < network::METADATA_CONCURRENCY {
                                    let Some(id) = remaining.next() else { break; };
                                    let id = id.clone();
                                    let reference_db = db_clone.clone();
                                    let reference_market = market.clone();
                                    let reference_cancel = cancel_flag.clone();
                                    reference_workers.spawn(async move {
                                        let result = actions::release_with_cancel(&reference_db, &id, &reference_market, false, reference_cancel).await;
                                        (id, result)
                                    });
                                }
                                let Some(result) = reference_workers.join_next().await else { break; };
                                match result {
                                    Ok((_id, Ok(release))) => {
                                        reference_checked += 1;
                                        let name = release["artist"].as_str().unwrap_or("Downloaded artist");
                                        let title = release["title"].as_str().unwrap_or("Downloaded release");
                                        reference_refresh_progress(&backend_task, &format!("Downloaded reference checked · {reference_checked}/{} releases · {name} — {title} · saved details reused where complete", references.len()), reference_checked, references.len());
                                        publish_catalogue_changes(app_clone.as_ref(), &db_clone, &mut last_published);
                                    }
                                    Ok((id, Err(error))) => {
                                        // Withdrawn online editions remain valid downloaded
                                        // references. Retain their cached evidence.
                                        if error.contains("HTTP 404") || error.contains("unavailable in the selected market") {
                                            reference_checked += 1;
                                            backend_task.log_for("discography", &format!("Downloaded reference {id} is no longer available online; its saved metadata remains usable"), "warning");
                                        } else { failure = Some(format!("Downloaded reference {id}: {error}")); }
                                    }
                                    Err(error) => failure = Some(format!("Reference metadata worker stopped: {error}")),
                                }
                            }
                            reference_workers.abort_all();
                            while reference_workers.join_next().await.is_some() {}
                        }
                    }
                }
            }

            // Keep three artists in flight, sharing the global API pacing lane.
            // Catalogue/queue writes and checkpoints remain serial and durable.
            let mut pending: std::collections::VecDeque<_> = ids.iter().filter(|id| !completed.contains(id)).cloned().collect();
            let mut fetches = tokio::task::JoinSet::new();
            let artist_job = backend_task.online_job.lock().unwrap().clone();
            if let Some(mut job) = artist_job {
                job.as_object_mut().unwrap().remove("progress_phase_override");
                job["completed"] = json!(checked);
                job["total"] = json!(total);
                backend_task.update_online_job_progress(&format!("Checking release lists · {checked}/{total} artists"), job);
            }
            loop {
                if failure.is_some() || cancel_flag.load(Ordering::Relaxed) { break; }
                while fetches.len() < network::METADATA_CONCURRENCY {
                    let Some(artist_id) = pending.pop_front() else { break; };
                    let mut worker = client.clone().with_cancel(cancel_flag.clone());
                    let worker_market = market.clone();
                    let name = names.get(&artist_id).map(String::as_str).unwrap_or("Linked artist");
                    let message = format!("{checked}/{total} artists complete · {name} · Checking releases");
                    backend_task.refresh_artist_log(&artist_id, &message, "info");
                    backend_task.progress_for("discography", &message);
                    fetches.spawn(async move {
                        let result = worker.get_artist_catalogue_with_discovery(&artist_id, &worker_market, false, detailed).await;
                        (artist_id, result)
                    });
                }
                let Some(task) = fetches.join_next().await else { break; };
                let (artist_id, catalogue_result) = match task {
                    Ok(value) => value,
                    Err(error) => { failure = Some(format!("Catalogue worker stopped: {error}")); break; }
                };
                if cancel_flag.load(Ordering::Relaxed) { break; }
                let name = names.get(&artist_id).map(String::as_str).unwrap_or("Linked artist");
                failed_artist = Some((artist_id.clone(),name.to_owned()));
                let msg = format!("{checked}/{total} artists complete · {name} · Checking {}", if detailed { "new or changed track details and credits" } else { "new releases and market availability" });
                let prog_job = json!({
                    "id": j_id.clone(),
                    "kind": "discography",
                    "status": "running",
                    "message": msg.clone(),
                    "completed": checked, "total": total,
                    "started": started,
                    "result": null
                });
                let prog_job = backend_prog.update_online_job_progress(&msg, prog_job.clone());
                if let Some(ref app) = app_clone {
                    let _ = app.emit(
                        "backend-event",
                        json!({
                            "event": "progress",
                            "message": msg,
                            "online_job": prog_job
                        }),
                    );
                }

                match catalogue_result {
                    Ok(catalogue) => {
                        if let Err(e) = client
                            .save_catalogue_to_db(&db_clone, &market, &catalogue)
                            .await
                        {
                            failure = Some(format!("Could not save releases: {e}"));
                            break;
                        } else {
                            publish_catalogue_changes(app_clone.as_ref(), &db_clone, &mut last_published);
                            if detailed {
                                let (reused, changed, _) = match actions::recommendation_refresh_plan(&db_clone, &market, &catalogue.releases).await {
                                    Ok(plan) => plan,
                                    Err(error) => { failure = Some(error); break; }
                                };
                                let pending: Vec<_> = catalogue.releases.iter().filter(|release| !reused.contains(&release.id)).cloned().collect();
                                reused_details += reused.len();
                                let message = format!("{checked}/{total} artists complete · {name} · {} releases reused · Checking {} new or incomplete releases", reused.len(), pending.len());
                                backend_task.refresh_artist_log(&artist_id, &message, "info");
                                backend_task.progress_for("discography", &message);
                                if !pending.is_empty() {
                                    // Each worker owns a complete enrichment pipeline and reads
                                    // its summary through the shared indexed cache selector.
                                    let pending_count = pending.len();
                                    let mut releases = pending.into_iter();
                                    let mut metadata = tokio::task::JoinSet::new();
                                    let mut completed_details = 0;
                                    loop {
                                        if cancel_flag.load(Ordering::Relaxed) { break; }
                                        while metadata.len() < network::METADATA_CONCURRENCY {
                                            let Some(release) = releases.next() else { break; };
                                            let worker_db = db_clone.clone();
                                            let worker_market = market.clone();
                                            let worker_cancel = cancel_flag.clone();
                                            let force = changed.contains(&release.id);
                                            metadata.spawn(async move {
                                                let result = actions::release_with_cancel(&worker_db, &release.id, &worker_market, force, worker_cancel).await;
                                                (release.title, result)
                                            });
                                        }
                                        let Some(result) = metadata.join_next().await else { break; };
                                        match result {
                                            Ok((title, Ok(_))) => {
                                                completed_details += 1;
                                                checked_details += 1;
                                                let message = format!("{checked}/{total} artists complete · {name} · Details and credits {completed_details}/{pending_count} releases · {title} · {} reused", reused.len());
                                                backend_task.refresh_artist_log(&artist_id, &message, "info");
                                                backend_task.progress_for("discography", &message);
                                                publish_catalogue_changes(app_clone.as_ref(), &db_clone, &mut last_published);
                                            }
                                            Ok((title, Err(error))) => { failure = Some(format!("{title}: {error}")); break; }
                                            Err(error) => { failure = Some(format!("Recommendation worker stopped: {error}")); break; }
                                        }
                                    }
                                    metadata.abort_all();
                                    while metadata.join_next().await.is_some() {}
                                }
                                if failure.is_some() || cancel_flag.load(Ordering::Relaxed) { break; }
                            }
                            checked += 1;
                            completed.push(artist_id.clone());
                            let mut saved_progress = prog_job.clone();
                            saved_progress["completed"] = json!(checked);
                            let message = format!("{checked}/{total} artists complete · {name} · Updated {} releases", catalogue.releases.len());
                            backend_task.refresh_artist_log_status(&artist_id, &message, "info", "complete");
                            saved_progress["message"] = json!(message);
                            let saved_progress = backend_prog.update_online_job_progress(&message, saved_progress.clone());
                            if let Some(ref app) = app_clone {
                                let _ = app.emit("backend-event", json!({"event":"progress","message":message,"online_job":saved_progress}));
                            }

                            if let Err(e) = db_clone.set_preference("catalogue-refresh-checkpoint", &json!({"ids":ids,"completed":completed,"market":market,"detailed":detailed,"status":"running"})).await {
                                failure = Some(format!("Could not save refresh progress: {e}"));
                                break;
                            }
                            failed_artist = None;
                        }
                    }
                    Err(e) => {
                        failure = Some(format!("Could not check releases: {e}"));
                        break;
                    }
                }
            }

            fetches.abort_all();
            while fetches.join_next().await.is_some() {}
            let is_cancelled = cancel_flag.load(Ordering::Relaxed);
            let error_logged = !is_cancelled && failure.is_some() && failed_artist.is_some();
            if let (Some(error),Some((id,name))) = (&failure,&failed_artist) {
                if !is_cancelled { backend_task.refresh_artist_log(id,&format!("{checked}/{total} artists complete · {name} · {error}"),"error"); }
            }
            if is_cancelled || failure.is_some() {
                for id in ids.iter().filter(|id| !completed.contains(id)) {
                    let logs = backend_task.logs.snapshot();
                    let key = format!("{j_id}:artist:{id}");
                    if logs.iter().any(|entry| entry["progress_id"] == key && entry["job_status"] == "running") {
                        let name = names.get(id).map(String::as_str).unwrap_or("Linked artist");
                        let state = if is_cancelled { "Paused; saved metadata retained" } else { "Stopped after another artist failed; saved metadata retained" };
                        backend_task.refresh_artist_log_status(id,&format!("{checked}/{total} artists complete · {name} · {state}"),"info",if is_cancelled { "cancelled" } else { "interrupted" });
                    }
                }
            }
            let status = if is_cancelled {
                "cancelled"
            } else if failure.is_some() {
                "failed"
            } else {
                "complete"
            };
            let message = if is_cancelled {
                if backend_task.quit_prompt.load(Ordering::SeqCst) { format!("Refresh paused · {checked}/{total} artists saved · resumes next launch") } else { format!("Refresh cancelled · {checked}/{total} artists saved · use Resume refresh to continue") }
            } else if let Some(error) = failure {
                format!("Refresh stopped after {checked} artists; cached results retained. {error}")
            } else {
                if detailed {
                    format!("Recommendation update complete · {checked} artists checked · {reused_details} unchanged releases reused · {checked_details} new, changed or incomplete releases updated")
                } else {
                    format!("Release-list check complete · {checked} artists checked · new releases and availability saved · existing track details retained")
                }
            };

            let checkpoint_status = if is_cancelled && backend_task.quit_prompt.load(Ordering::SeqCst) { "running" } else { status };
            let _ = db_clone.set_preference("catalogue-refresh-checkpoint", &json!({"ids":ids,"completed":completed,"market":market,"detailed":detailed,"status":checkpoint_status})).await;
            let finished_at = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
            let final_job = json!({
                "id": j_id,
                "kind": "discography",
                "error_logged": error_logged,
                "status": status,
                "message": message,
                "started": started,
                "finished": finished_at,
                "result": json!({ "checked": checked, "reused_details":reused_details, "checked_details":checked_details })
            });

            backend_task.finish_online_job(final_job.clone());
            let _ = db_clone
                .set_preference("desktop-last-job", &final_job)
                .await;

            if let Some(ref app) = app_clone {
                let _ = app.emit(
                    "backend-event",
                    json!({
                        "event": "job",
                        "online_job": final_job
                    }),
                );
                let _ = app.emit("backend-event", json!({ "event": "changed" }));
            }
        });

        return Ok(initial_job);
    }
    if method == "turso.link"
        || (method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("link"))
    {
        if state.online_cancel.lock().unwrap().is_some() {
            return Err("A job is already running".to_string());
        }

        let inner_args = args.get("args").cloned().unwrap_or(args.clone());
        let market = inner_args
            .get("market")
            .and_then(|v| v.as_str())
            .unwrap_or("GB")
            .to_string();

        let root_opt = inner_args
            .get("root")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("root").and_then(|v| v.as_str()));
        let root_str = if let Some(r) = root_opt {
            r.to_string()
        } else {
            let roots = db.list_roots(&market).await?;
            roots.into_iter().next().map(|r| r.root).unwrap_or_default()
        };
        if root_str.is_empty() {
            return Err("Choose a registered library first".to_string());
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let initial_job = json!({
            "id": job_id,
            "kind": "link",
            "status": "running",
            "message": "Linking releases…",
            "started": started,
            "result": null
        });

        let cancel_flag = Arc::new(AtomicBool::new(false));
        state.start_online_job(initial_job.clone(), cancel_flag.clone());

        let db_clone = db.clone();
        let app_clone = app_handle.cloned();
        let backend_task = state.clone();
        let j_id = job_id.clone();
        let mkt = market.clone();
        let rt = root_str.clone();
        let selected: Option<std::collections::HashSet<String>> =
            inner_args.get("ids").and_then(|v| v.as_array()).map(|ids| {
                ids.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            });
        let editions_only = inner_args
            .get("editions_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        tauri::async_runtime::spawn(async move {
            let app_prog = app_clone.clone();
            let backend_prog = backend_task.clone();
            let j_id_prog = j_id.clone();

            let link_res = linking::link_library_scoped(
                &db_clone,
                &mkt,
                &rt,
                cancel_flag.clone(),
                move |msg| {
                    let prog_job = json!({
                        "id": j_id_prog.clone(),
                        "kind": "link",
                        "status": "running",
                        "message": msg.clone(),
                        "started": started,
                        "result": null
                    });
                    let prog_job = backend_prog.update_online_job_progress(&msg, prog_job.clone());
                    if let Some(ref app) = app_prog {
                        let _ = app.emit(
                            "backend-event",
                            json!({
                                "event": "progress",
                                "message": msg,
                                "online_job": prog_job
                            }),
                        );
                    }
                },
                selected.as_ref(),
                editions_only,
            )
            .await;

            let is_cancelled = cancel_flag.load(Ordering::Relaxed);
            let (status, message, result_val) = match link_res {
                Ok(summary) => {
                    let st = if is_cancelled {
                        "cancelled"
                    } else {
                        "complete"
                    };
                    let msg = if is_cancelled {
                        "Link releases · cancelled; completed results retained".to_string()
                    } else {
                        format!(
                            "Link releases · finished; {} linked · {} need review · {} unmatched",
                            summary.linked, summary.review, summary.unmatched
                        )
                    };
                    (
                        st,
                        msg,
                        json!({
                            "total": summary.total,
                            "linked": summary.linked,
                            "review": summary.review,
                            "unmatched": summary.unmatched,
                        }),
                    )
                }
                Err(e) => ("failed", format!("Link failed: {}", e), json!(null)),
            };

            let finished_at = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
            let final_job = json!({
                "id": j_id,
                "kind": "link",
                "status": status,
                "message": message,
                "started": started,
                "finished": finished_at,
                "result": result_val
            });

            backend_task.finish_online_job(final_job.clone());
            let _ = db_clone
                .set_preference("desktop-last-job", &final_job)
                .await;

            if let Some(ref app) = app_clone {
                let _ = app.emit(
                    "backend-event",
                    json!({
                        "event": "job",
                        "online_job": final_job
                    }),
                );
                let _ = app.emit("backend-event", json!({ "event": "changed" }));
            }
        });

        return Ok(initial_job);
    }
    if method == "job.start"
        && (args.get("kind").and_then(|v| v.as_str()) == Some("connect_download")
            || args.get("kind").and_then(|v| v.as_str()) == Some("connect_account"))
    {
        let flow = crate::stream_download::create_pkce_flow();
        let auth_url = flow.login_url.clone();
        *state.pending_pkce.lock().unwrap() = Some(flow);

        let job_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let job = json!({
            "id": job_id,
            "kind": "connect_download",
            "status": "complete",
            "message": "Sign-in prompt opened. Please authenticate in your browser.",
            "started": started,
            "finished": started,
            "result": json!({ "auth_url": auth_url })
        });

        state.finish_online_job(job.clone());
        let _ = db.set_preference("desktop-last-job", &job).await;

        if let Some(app) = app_handle {
            let _ = app.emit(
                "backend-event",
                json!({ "event": "authentication", "auth_url": auth_url }),
            );
            let _ = app.emit(
                "backend-event",
                json!({ "event": "job", "online_job": job }),
            );
        }

        return Ok(job);
    }
    if method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("component_check")
    {
        return Ok(json!({ "status": "ok" }));
    }
    if method == "job.start"
        && args.get("kind").and_then(|v| v.as_str()) == Some("component_update")
    {
        let job_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let job = json!({
            "id": job_id,
            "kind": "component_update",
            "status": "complete",
            "message": "Native download components are built directly into Tibrary. No external components needed.",
            "started": now,
            "finished": now,
            "result": { "message": "Components are built into the binary." }
        });
        state.finish_job(job.clone());
        let _ = db.set_preference("desktop-last-job", &job).await;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "job", "job": job }));
        }
        return Ok(job);
    }
    if method == "job.start"
        && args.get("kind").and_then(|v| v.as_str()) == Some("component_rollback")
    {
        let job_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let job = json!({
            "id": job_id,
            "kind": "component_rollback",
            "status": "complete",
            "message": "Native streaming engine is active.",
            "started": now,
            "finished": now,
            "result": { "message": "Native components active." }
        });
        state.finish_job(job.clone());
        let _ = db.set_preference("desktop-last-job", &job).await;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "job", "job": job }));
        }
        return Ok(job);
    }
    if method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("download") {
        if state.download_cancel.lock().unwrap().is_some() {
            return Err("A download is already running".to_string());
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let initial_job = json!({
            "id": job_id,
            "kind": "download",
            "status": "running",
            "message": "Starting downloader…",
            "started": started,
            "result": null
        });

        let cancel_flag = Arc::new(AtomicBool::new(false));
        state.start_download_job(initial_job.clone(), cancel_flag.clone());

        let db_clone = db.clone();
        let app_clone = app_handle.cloned();
        let backend_task = state.clone();
        let j_id = job_id.clone();

        tauri::async_runtime::spawn(async move {
            let app_prog = app_clone.clone();
            let backend_prog = backend_task.clone();
            let j_id_prog = j_id.clone();
            let app_monitor = app_clone.clone();
            let backend_monitor = backend_task.clone();
            let monitor_job_id = j_id.clone();

            let dl_res = downloads::DownloadManager::run_downloads(
                &db_clone,
                app_clone.as_ref(),
                cancel_flag.clone(),
                &j_id,
                move |msg| {
                    let prog_job = json!({
                        "id": j_id_prog.clone(),
                        "kind": "download",
                        "status": "running",
                        "message": msg.clone(),
                        "started": started,
                        "result": null
                    });
                    let prog_job = backend_prog.update_download_job(prog_job.clone());
                    if let Some(ref a) = app_prog {
                        let _ = a.emit(
                            "backend-event",
                            json!({
                                "event": "progress",
                                "message": msg,
                                "download_job": prog_job
                            }),
                        );
                    }
                },
                move |item| {
                    if let Some(item) = backend_monitor.record_download_monitor(&monitor_job_id, item) {
                      if let Some(ref a) = app_monitor {
                        let _ = a.emit(
                            "backend-event",
                            json!({"event":"download-monitor","item":item}),
                        );
                      }
                    }
                },
            )
            .await;

            let is_cancelled = cancel_flag.load(Ordering::Relaxed);
            let (status, message, count) = match dl_res {
                Ok(c) => {
                    let st = if is_cancelled {
                        "cancelled"
                    } else {
                        "complete"
                    };
                    let msg = if is_cancelled {
                        "Download cancelled".to_string()
                    } else {
                        format!("Downloads finished · {} releases completed", c)
                    };
                    (st, msg, c)
                }
                Err(e) => ("failed", format!("Download error: {}", e), 0),
            };

            let finished_at = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
            let final_job = json!({
                "id": j_id,
                "kind": "download",
                "status": status,
                "message": message,
                "started": started,
                "finished": finished_at,
                "result": json!({ "completed": count })
            });

            backend_task.finish_download_job(final_job.clone());
            let _ = db_clone
                .set_preference("desktop-last-job", &final_job)
                .await;

            if let Some(ref app) = app_clone {
                let _ = app.emit(
                    "backend-event",
                    json!({
                        "event": "job",
                        "download_job": final_job
                    }),
                );
                let _ = app.emit("backend-event", json!({ "event": if count > 0 { "library-mutated" } else { "changed" } }));
            }
        });

        return Ok(initial_job);
    }
    if method == "turso.maintenance.apply"
        || (method == "job.start"
            && (args.get("kind").and_then(|v| v.as_str()) == Some("apply")
                || args.get("kind").and_then(|v| v.as_str()) == Some("workflow")))
    {
        let root = args.get("root").and_then(|v| v.as_str()).unwrap_or("");
        let items_val = args
            .get("items")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        let items: Vec<maintenance::FileApplyItem> =
            serde_json::from_value(items_val).map_err(|e| format!("Invalid apply items: {}", e))?;
        let res = maintenance::apply_batch(db, root, &items).await;
        if res.applied > 0 {
            if let Some(app) = app_handle {
                let _ = app.emit("backend-event", json!({ "event": "library-mutated" }));
            }
        }
        return serde_json::to_value(res).map_err(|e| e.to_string());
    }
    if method == "job.cancel" {
        let requested = args["kind"].as_str().unwrap_or("");
        let pending_sign_in = if requested.is_empty() || requested.starts_with("connect_") {
            state.pending_pkce.lock().unwrap().take().is_some()
        } else {
            false
        };
        if pending_sign_in && requested.is_empty() {
            return Ok(json!(true));
        }
        if requested == "download"
            || (requested.is_empty()
                && state.active_job_cancel.lock().unwrap().is_none()
                && state.online_cancel.lock().unwrap().is_none())
        {
            if let Some(job) = state.cancel_download_job() {
                if let Some(app) = app_handle {
                    let _ = app.emit("backend-event", json!({"event":"job","download_job":job}));
                }
            }
            return Ok(json!(true));
        }
        if is_online_job(requested)
            || (requested.is_empty() && state.online_cancel.lock().unwrap().is_some())
        {
            if let Some(job) = state.cancel_online_job() {
                if let Some(app) = app_handle {
                    let _ = app.emit("backend-event", json!({"event":"job","online_job":job}));
                }
            }
            return Ok(json!(true));
        }
        let cancel_msg = "Cancellation requested · finishing the current safe file boundary";
        if let Some(active) = state.cancel_active_job(cancel_msg) {
            if let Some(app) = app_handle {
                let _ = app.emit("backend-event", json!({ "event": "job", "job": active }));
                let _ = app.emit(
                    "backend-event",
                    json!({
                        "event": "progress",
                        "message": cancel_msg,
                    }),
                );
            }
        }
        return Ok(json!(true));
    }
    if method == "auth.reply" {
        let redirect_url = args
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let flow = { state.pending_pkce.lock().unwrap().clone() };

        if let Some(flow) = flow {
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| e.to_string())?;
            match crate::stream_download::exchange_pkce_code(
                &http,
                redirect_url,
                &flow.code_verifier,
                &flow.client_unique_key,
            )
            .await
            {
                Ok(token) => {
                    crate::stream_download::save_token(db, &token).await?;
                    if let Err(error) = account::refresh_account_market(db, &http, None).await {
                        state.log_for("connect_account", &format!("Connected; account region check will be retried: {error}"), "warning");
                    }
                    state.pending_pkce.lock().unwrap().take();
                    state.log_for("connect_account", "Streaming account connected · metadata and downloads ready", "info");
                    if let Some(app) = app_handle {
                        let _ = app.emit("backend-event", json!({ "event": "ready" }));
                        let _ = app.emit("backend-event", json!({ "event": "changed" }));
                    }
                    return Ok(json!(true));
                }
                Err(e) => {
                    state.log_for("connect_account", &format!("Failed to complete sign-in: {e}"), "error");
                    return Err(e);
                }
            }
        }
        return Ok(json!(false));
    }

    Err(format!(
        "Unsupported action: {}. No changes were made.",
        method
    ))
}

#[tauri::command]
async fn backend_call(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, Arc<Backend>>,
    db: tauri::State<'_, TursoDb>,
    method: String,
    args: Value,
) -> Result<Value, String> {
    handle_rpc_call(Some(&app_handle), &state, &db, method, args).await
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
    let host = parsed.host_str().unwrap_or("");
    if parsed.scheme() != "https" || !(host == "tidal.com" || host.ends_with(".tidal.com")) {
        return Err("Only verified service links can open from this action.".into());
    }
    Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_file(path: String) -> Result<(), String> {
    let p = if path.starts_with("file:") {
        url::Url::parse(&path)
            .map_err(|e| e.to_string())?
            .to_file_path()
            .map_err(|_| "Invalid local file URL")?
    } else {
        std::path::PathBuf::from(path)
    };
    if !p.is_absolute() || !p.exists() {
        return Err("The file is unavailable. Reconnect the library or update its index.".into());
    }
    Command::new("open")
        .arg("-R")
        .arg(p)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_export(path: String, content: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(path);
    if !p.is_absolute()
        || !matches!(
            p.extension().and_then(|v| v.to_str()),
            Some("json" | "csv" | "txt" | "m3u")
        )
    {
        return Err("Choose a JSON, CSV or text export file.".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)
        .map_err(|e| e.to_string())?;
    file.write_all(content.as_bytes())
        .map_err(|e| e.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--appearance-probe") {
        match appearance::native_probe().and_then(|accent|serde_json::to_string(&accent).map_err(|error|error.to_string())) {
            Ok(palette) => println!("{palette}"),
            Err(error) => { eprintln!("{error}"); std::process::exit(1); }
        }
        return;
    }
    if args.iter().any(|a| a == "--rpc") {
        #[cfg(target_os = "macos")]
        let _ = appearance::native_probe();
        let db_path = args
            .windows(2)
            .find(|w| w[0] == "--db")
            .map(|w| std::path::PathBuf::from(&w[1]))
            .unwrap_or_else(|| std::path::PathBuf::from("library.sqlite3"));

        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        rt.block_on(async move {
            let turso_db = TursoDb::open(&db_path).await.expect("Failed to open DB");
            let backend = Arc::new(Backend::new());
            backend.set_db(Arc::new(turso_db.clone()));
            if let Ok(loaded) = turso_db.load_recent_logs(500).await {
                if !loaded.is_empty() {
                    backend.logs.load(loaded);
                }
            }

            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout();
            for line in stdin.lines() {
                let Ok(line) = line else { break };
                let Ok(val) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let id = val.get("id").cloned().unwrap_or(Value::Null);
                let method = val
                    .get("method")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let rpc_args = val.get("args").cloned().unwrap_or(json!({}));

                let res = handle_rpc_call(None, &backend, &turso_db, method, rpc_args).await;
                let out = match res {
                    Ok(r) => json!({ "id": id, "result": r }),
                    Err(e) => json!({ "id": id, "error": e }),
                };
                let _ = writeln!(stdout, "{}", out);
                let _ = stdout.flush();
            }
            backend.flush_activity().await;
        });
        return;
    }

    let backend = Arc::new(Backend::new());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(backend.clone())
        .setup(move |app| {
            let db_path = if let Ok(db) = std::env::var("TIBRARY_TEST_DB") {
                std::path::PathBuf::from(db)
            } else {
                let home = std::env::var("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                let folder = home.join("Library/Application Support/Tibrary");
                if std::env::var_os("TIBRARY_DEMO").is_some() {
                    folder.join("tauri-demo.sqlite3")
                } else {
                    folder.join("library.sqlite3")
                }
            };
            let turso_db = tauri::async_runtime::block_on(TursoDb::open(&db_path))
                .map_err(|e| tauri::Error::from(std::io::Error::other(e)))?;
            backend.set_db(Arc::new(turso_db.clone()));
            let persist = tauri::async_runtime::block_on(turso_db.get_preference("desktop"))
                .ok()
                .flatten()
                .and_then(|v| v.get("persist_logs").and_then(|b| b.as_bool()))
                .unwrap_or(true);
            backend
                .persist_logs
                .store(persist, std::sync::atomic::Ordering::SeqCst);
            if let Ok(loaded) = tauri::async_runtime::block_on(turso_db.load_recent_logs(500)) {
                if loaded.is_empty() {
                    backend.log_with_category(
                        &format!(
                            "Tibrary v{} ready · workspace initialized",
                            env!("CARGO_PKG_VERSION")
                        ),
                        "info",
                        Some("general"),
                    );
                } else {
                    backend.logs.load(loaded);
                }
            }
            // Only read-only catalogue refreshes resume automatically. Explicit cancellation stays cancelled.
            let resume_db = turso_db.clone();
            let resume_backend = backend.clone();
            let resume_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(Some(saved)) = resume_db.get_preference("catalogue-refresh-checkpoint").await {
                    if saved["status"] == "running" {
                        let args = json!({"kind":"discography","args":{"ids":saved["ids"],"market":saved["market"],"detailed":saved["detailed"],"resume":true}});
                        if let Err(error) = handle_rpc_call(Some(&resume_app), &resume_backend, &resume_db, "job.start".into(), args).await {
                            resume_backend.log_with_category(&format!("Saved catalogue refresh could not resume: {error}"), "error", Some("online"));
                        }
                    }
                }
            });
            app.manage(turso_db);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend_call,
            open_external,
            reveal_file,
            save_export
        ])
        .build(tauri::generate_context!())
        .expect("Could not start Tibrary")
        .run(|app, event| {
            let backend = app.state::<Arc<Backend>>().inner().clone();
            if backend.quit_approved.load(Ordering::SeqCst) { return; }
            let active_download = backend.download_cancel.lock().unwrap().is_some();
            let active_task = active_download || backend.active_job_cancel.lock().unwrap().is_some() || backend.online_cancel.lock().unwrap().is_some();
            match event {
                tauri::RunEvent::WindowEvent { event: tauri::WindowEvent::CloseRequested { api, .. }, .. } => api.prevent_close(),
                tauri::RunEvent::ExitRequested { api, .. } => api.prevent_exit(),
                _ => return,
            }
            if backend.quit_prompt.swap(true, Ordering::SeqCst) { return; }
            if !active_task {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    backend.flush_activity().await;
                    backend.quit_approved.store(true, Ordering::SeqCst);
                    handle.exit(0);
                });
                return;
            }
            use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
            let handle = app.clone();
            app.dialog().message(if active_download { "Downloads are still running. Stop active tasks and quit? Completed tracks will be kept; unfinished tracks remain in the queue." } else { "A task is still running. Stop active tasks and quit? File changes already completed will be kept." })
                .title("Quit Tibrary?")
                .buttons(MessageDialogButtons::OkCancelCustom(if active_download { "Stop downloads and quit" } else { "Stop tasks and quit" }.into(), if active_download { "Keep downloading" } else { "Keep working" }.into()))
                .show(move |confirmed| {
                    if !confirmed { backend.quit_prompt.store(false, Ordering::SeqCst); return; }
                    if let Some(job) = backend.cancel_download_job() {
                        let _ = handle.emit("backend-event", json!({"event":"job", "download_job":job}));
                    }
                    backend.cancel_active_job("Stopping safely before quitting");
                    backend.cancel_online_job();
                    tauri::async_runtime::spawn(async move {
                        // Let the worker publish completed files and clean up temporary output.
                        while backend.download_cancel.lock().unwrap().is_some() || backend.active_job_cancel.lock().unwrap().is_some() || backend.online_cancel.lock().unwrap().is_some() {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                        backend.flush_activity().await;
                        backend.quit_approved.store(true, Ordering::SeqCst);
                        handle.exit(0);
                    });
                });
        });
}

#[cfg(test)]
mod table_filter_tests {
    use super::*;

    #[tokio::test]
    async fn every_table_route_filters_complete_cached_rows_and_facets_before_pagination() {
        let dir = std::env::temp_dir().join(format!("table-column-filters-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let backend = Arc::new(Backend::new());
        let root = "/table-fixture";
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO roots(root,status) VALUES(?,'complete')",(root,)).await.unwrap();
        let cached = json!([
            {"id":"one","artist":"One","release":"A","date":"2020","children":[],"status":"Needs update","affected":true},
            {"id":"two","artist":"One","release":"B","date":"2021","children":[],"status":"No change","affected":false},
            {"id":"three","artist":"Two","release":"C","date":"2022","children":[],"status":"Needs update","affected":true}
        ]);
        for (index,artist) in ["One","One","Two"].iter().enumerate() {
            let path = format!("{root}/{index}.flac");
            let metadata = json!({"albumartist":artist,"artist":artist,"album":"Local","title":format!("Track {index}"),"date":"2020","tracknumber":format!("{}",index+1),"tracktotal":"3","discnumber":"1","disctotal":"1"});
            conn.execute("INSERT INTO local_files(path,root,size,mtime,metadata,present) VALUES(?,?,1,1,?,1)",(path.as_str(),root,metadata.to_string())).await.unwrap();
        }
        conn.execute("INSERT INTO ignored_local_files(path) VALUES(?)",(format!("{root}/1.flac"),)).await.unwrap();
        conn.execute("INSERT INTO favourite_artists(cache_id,payload) VALUES('user',?)",(json!([{"id":"o","name":"One"},{"id":"t","name":"Two"}]).to_string(),)).await.unwrap();
        for (name,id) in [("One","o"),("Two","t")] {
            conn.execute("INSERT INTO mappings(artist,tidal_id,status) VALUES(?,?,'confirmed')",(name,id)).await.unwrap();
            let releases:Vec<_> = (0..if name == "One" {2}else{1}).map(|index|json!({"id":format!("{id}{index}"),"title":format!("Remote {index}"),"artist":name,"date":"2023-01-01","type":"ALBUM","track_count":2,"available":index==0,"tracks":[]})).collect();
            conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES(?,'GB',?)",(id,json!({"name":name,"releases":releases}).to_string())).await.unwrap();
            for release in releases { db.set_preference(&format!("release-artists:GB:{}",release["id"].as_str().unwrap()),&json!({"ids":[id],"names":[name],"checked_at":1})).await.unwrap(); }
        }
        for index in 0..120 {
            let payload = json!({"artist":if index%2==0{"One"}else{"Two"},"title":format!("Queued {index:03}"),"date":"2023","track_count":1,"tracks_loaded":index==119});
            conn.execute("INSERT INTO queue(id,payload,approved,decision) VALUES(?,?,1,'queued')",(format!("q{index}"),payload.to_string())).await.unwrap();
        }
        for index in 0..3 {
            conn.execute("INSERT INTO queue(id,payload,approved,decision) VALUES(?,?,1,'downloaded')",(format!("d{index}"),json!({"artist":if index<2{"One"}else{"Two"},"title":format!("Downloaded {index}"),"date":"2023","track_count":1}).to_string())).await.unwrap();
        }
        for route in ["local","online","mqa"] {
            db.set_preference(&format!("desktop-{route}:{root}"),&cached).await.unwrap();
        }
        let manifest = duplicates::indexed_manifest_fingerprint(&db,root).await.unwrap();
        for route in ["local","mqa"] { db.set_preference(&format!("desktop-{route}-manifest:{root}"),&json!(manifest)).await.unwrap(); }
        for operation in ["dates","organise"] {
            backend.previews.lock().unwrap().insert(operation.into(),json!({"id":operation,"root":root,"operation":operation,"created":1,"rows":cached,"source_inputs":preview_inputs(&db,operation).await.unwrap()}));
        }

        for (route,expected) in [("files",2),("links",2),("artists",1),("favourites",1),("correct",2),("organise",2),("metadata",2),("artwork",2),("mqa",2),("local",2),("online",2),("missing",1),("queue",60),("downloaded",2)] {
            let args = json!({"route":route,"root":root,"filter":"all","timeline":"All missing releases","recommendation":"All recommendations","type":"All types","action":if route=="organise"{"organise"}else{"dates"},"sort":"artist","direction":"asc","offset":0,"limit":1,"column_filters":{"artist":{"include":["One"]}}});
            let page = handle_rpc_call(None,&backend,&db,"table".into(),args.clone()).await.unwrap();
            assert_eq!(page["total"],expected,"{route} must filter all rows, not just its first page");
            assert_eq!(page["rows"].as_array().unwrap().len(),1,"{route}");
            assert_eq!(page["rows"][0]["artist"],"One","{route}");
            let mut facet_args=args.clone();facet_args["column"]=json!("artist");facet_args["limit"]=json!(100);
            let facets=handle_rpc_call(None,&backend,&db,"table.facets".into(),facet_args).await.unwrap();
            assert_eq!(facets["total"],2,"{route} must retain its own unchecked value");
            assert_eq!(facets["options"][0]["count"],expected,"{route}");
        }
        let after_first_page = handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":"queue","sort":"release","offset":55,"limit":5,"column_filters":{"artist":{"include":["One"]}}})).await.unwrap();
        for route in ["metadata", "artwork"] {
            let pending = handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":route,"root":root,"filter":"affected"})).await.unwrap();
            assert_eq!(pending["total"],0,"{route} must not classify unchanged fallback files as proposed changes");
        }
        assert_eq!(after_first_page["total"],60);
        assert_eq!(after_first_page["rows"].as_array().unwrap().len(),5);
        assert_eq!(after_first_page["rows"][0]["release"],"Queued 110");
        let unfiltered_sorted = handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":"queue","sort":"expanded_available","direction":"desc","offset":0,"limit":1})).await.unwrap();
        assert_eq!(unfiltered_sorted["rows"][0]["id"],"q119","a column not handled by the old database sorter must sort the entire dataset");
        let selected_sorted = handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":"queue","sort":"expanded_available","direction":"desc","offset":0,"limit":1,"column_filters":{"artist":{"include":["One","Two"]}}})).await.unwrap();
        assert_eq!(unfiltered_sorted["rows"],selected_sorted["rows"],"Select all uses the same global ordering as column selections");
        let multiple = handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":"queue","limit":1000,"column_filters":{"artist":{"include":["One","Two"]},"release":{"include":["Queued 110","Queued 111"],"exclude":["Queued 111"]}}})).await.unwrap();
        assert_eq!(multiple["total"],1);
        assert_eq!(multiple["rows"][0]["release"],"Queued 110");
        let all_none = handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":"queue","column_filters":{"artist":{"include":[]}}})).await.unwrap();
        assert_eq!(all_none["total"],0);
        let ignored = handle_rpc_call(None,&backend,&db,"table.facets".into(),json!({"route":"links","root":root,"filter":"all","column":"status","column_filters":{"status":{"exclude":["Ignored"]}}})).await.unwrap();
        assert!(ignored["options"].as_array().unwrap().iter().any(|option|option["value"]=="Ignored"));
        let include_unavailable = handle_rpc_call(None,&backend,&db,"table.facets".into(),json!({"route":"missing","timeline":"All missing releases","artist_scope":"My album artists","include_unavailable":true,"column":"status","column_filters":{"status":{"exclude":["Unavailable"]}}})).await.unwrap();
        assert!(include_unavailable["options"].as_array().unwrap().iter().any(|option|option["value"]=="Unavailable"));
        let no_artist = handle_rpc_call(None,&backend,&db,"table.facets".into(),json!({"route":"missing","timeline":"All missing releases","artist_scope":"Other artist appearances","include_unavailable":true,"column":"status"})).await.unwrap();
        assert_eq!(no_artist["total"],0,"unavailable rows must still obey album artist scope");
        let search = handle_rpc_call(None,&backend,&db,"table.facets".into(),json!({"route":"queue","search":"Queued 11","column":"release","facet_search":"8","column_filters":{"artist":{"include":["One"]}}})).await.unwrap();
        assert_eq!(search["total"],1);
        assert_eq!(search["options"][0]["value"],"Queued 118");
        let cache_args=json!({"route":"queue","root":root,"filter":"all","timeline":"All missing releases","recommendation":"All recommendations","type":"All types","action":"dates","sort":"artist","direction":"asc","offset":0,"limit":100,"column":"artist","column_filters":{"artist":{"include":["One"]}}});
        conn.execute("INSERT INTO queue(id,payload,approved,decision) VALUES('extra',?,1,'queued')",(json!({"artist":"One","title":"Extra","date":"2023","track_count":1}).to_string(),)).await.unwrap();
        let reused=handle_rpc_call(None,&backend,&db,"table.facets".into(),cache_args.clone()).await.unwrap();
        assert_eq!(reused["options"][0]["count"],60,"column changes reuse the shared snapshot until its revision changes");
        db.bump_revision();
        let changed=handle_rpc_call(None,&backend,&db,"table.facets".into(),cache_args).await.unwrap();
        assert_eq!(changed["options"][0]["count"],61,"normal database invalidation refreshes the same facet source");
        drop(conn);drop(backend);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod activity_tests {
    #[tokio::test]
    async fn artist_refresh_has_one_durable_row_and_one_error_at_the_failed_stage() {
        let dir = std::env::temp_dir().join(format!("artist-log-{}",uuid::Uuid::new_v4()));
        let db = Arc::new(super::TursoDb::open(dir.join("db")).await.unwrap());
        let backend = super::Backend::new();
        backend.set_db(db.clone());
        backend.start_online_job(serde_json::json!({"id":"refresh","kind":"discography","status":"running","message":"Refresh started"}),std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
        super::reference_refresh_progress(&backend,"Downloaded reference metadata · 0/4 releases",0,4);
        super::reference_refresh_progress(&backend,"Downloaded reference checked · 2/4 releases",2,4);
        let reference = backend.online_job.lock().unwrap().clone().unwrap();
        assert_eq!(reference["completed"],2);
        assert_eq!(reference["percent"],50.);
        assert_eq!(reference["progress_phase"],"reference releases");
        for n in 0..20 {
            backend.refresh_artist_log("one",&format!("Example · Credits {n}/20 releases"),"info");
            backend.progress_for("discography",&format!("Example · {n}/20 releases"));
        }
        backend.refresh_artist_log_status("one","Example · Updated 20 releases","info","complete");
        backend.refresh_artist_log("two","Other Artist · Checking releases","info");
        backend.refresh_artist_log("three","Third Artist · Checking releases","info");
        backend.refresh_artist_log("two","Other Artist · Could not check credits: HTTP 429","error");
        backend.refresh_artist_log_status("three","Third Artist · Stopped; saved metadata retained","info","interrupted");
        backend.finish_online_job(serde_json::json!({"id":"refresh","kind":"discography","status":"failed","message":"Refresh stopped","error_logged":true}));
        backend.flush_activity().await;
        let page = db.activity_history(None,100,None).await.unwrap();
        let rows = page["entries"].as_array().unwrap();
        assert_eq!(rows.len(),4,"start plus one row for each artist; no repeated stage messages");
        assert_eq!(rows.iter().filter(|entry|entry["level"] == "error").count(),1);
        assert!(rows.iter().any(|entry|entry["message"] == "Example · Updated 20 releases"));
        let live = backend.logs.snapshot();
        let saved = rows.iter().find(|entry|entry["progress_id"] == "refresh:artist:one").unwrap();
        assert_eq!(saved["job_status"],"complete");
        assert_eq!(rows.iter().find(|entry|entry["progress_id"] == "refresh:artist:two").unwrap()["job_status"],"failed");
        assert_eq!(rows.iter().find(|entry|entry["progress_id"] == "refresh:artist:three").unwrap()["job_status"],"interrupted");
        assert_eq!(saved["at"],live.iter().find(|entry|entry["progress_id"] == saved["progress_id"]).unwrap()["at"]);
        drop(backend); drop(db);
        let reopened = super::TursoDb::open(dir.join("db")).await.unwrap();
        assert_eq!(reopened.load_recent_logs(100).await.unwrap().len(),4);
        drop(reopened); std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn slow_catalogue_reads_do_not_block_queue_updates_or_independent_workers() {
        let dir=std::env::temp_dir().join(format!("independent-reads-{}",uuid::Uuid::new_v4()));
        let db=Arc::new(TursoDb::open(dir.join("db")).await.unwrap());
        let backend=Arc::new(Backend::new());
        let online_cancel=Arc::new(AtomicBool::new(false));
        let download_cancel=Arc::new(AtomicBool::new(false));
        let local_cancel=Arc::new(AtomicBool::new(false));
        backend.start_online_job(json!({"id":"refresh","kind":"discography","status":"running"}),online_cancel.clone());
        backend.start_download_job(json!({"id":"download","kind":"download","status":"running"}),download_cancel.clone());
        backend.start_job(json!({"id":"scan","kind":"scan","status":"running"}),local_cancel.clone());
        db.set_preference("tag-review:GB:123",&json!({"id":"123","artist":"Example","title":"Release","label":"Saved label","tracks_loaded":false,"track_count":1,"tracks":[]})).await.unwrap();
        db.set_preference("subscriber-items:GB:123",&json!({"schema":2,"checked_at":chrono::Utc::now().timestamp(),"items":[{"id":"456","title":"Track","trackNumber":1,"volumeNumber":1,"credits":[]}]})).await.unwrap();

        let args=json!({"route":"missing","limit":10});
        let gate=backend.read_gate(&format!("table:{args}"));
        let held=gate.lock().await;
        let (task_backend,task_db)=(backend.clone(),db.clone());
        let missing=tokio::spawn(async move {handle_rpc_call(None,&task_backend,&task_db,"table".into(),args).await});
        tokio::task::yield_now().await;
        let add=handle_rpc_call(None,&backend,&db,"queue.add".into(),json!({"selection":{"123":null}}));
        tokio::time::timeout(std::time::Duration::from_secs(1),add).await.unwrap().unwrap();
        let queue=tokio::time::timeout(std::time::Duration::from_secs(1),handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":"queue","limit":10}))).await.unwrap().unwrap();
        assert_eq!(queue["total"],1);
        assert_eq!(queue["rows"][0]["id"],"123");
        assert!(!missing.is_finished());
        let tracks=handle_rpc_call(None,&backend,&db,"release.ensure_tracks".into(),json!({"id":"123","market":"GB"})).await.unwrap();
        assert_eq!(tracks["tracks"][0]["id"],"456");
        assert_eq!(tracks["label"],"Saved label");
        let updated=handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":"queue","limit":10})).await.unwrap();
        assert_eq!(updated["rows"][0]["children"][0]["id"],"456");
        assert_eq!(updated["rows"][0]["approved"],true);
        db.connect().unwrap().execute("UPDATE queue SET payload=json_set(payload,'$.tracks',json('[]'),'$.tracks_loaded',json('false'),'$.selected_tracks',json('[{\"id\":\"456\"}]')),approved=0 WHERE id='123'",()).await.unwrap();
        handle_rpc_call(None,&backend,&db,"release.ensure_tracks".into(),json!({"id":"123","market":"GB"})).await.unwrap();
        let hydrated=handle_rpc_call(None,&backend,&db,"table".into(),json!({"route":"queue","limit":10})).await.unwrap();
        assert_eq!(hydrated["rows"][0]["children"][0]["id"],"456");
        assert_eq!(hydrated["rows"][0]["approved"],false);
        let mut queued=db.connect().unwrap().query("SELECT payload FROM queue WHERE id='123'",()).await.unwrap();
        let payload:Value=serde_json::from_str(&queued.next().await.unwrap().unwrap().get::<String>(0).unwrap()).unwrap();
        assert_eq!(payload["selected_tracks"][0]["id"],"456");
        drop(queued);
        // A disclosure only fills an absent list. Saved rows remain usable
        // without authentication while the separate refresh rechecks them.
        db.set_preference("desktop",&json!({"market":"US"})).await.unwrap();
        db.set_preference("tag-review:US:789",&json!({"id":"789","title":"US cached release","tracks_loaded":true,"tracks":[{"id":"987","title":"US track"}]})).await.unwrap();
        let default_market=handle_rpc_call(None,&backend,&db,"release.ensure_tracks".into(),json!({"id":"789"})).await.unwrap();
        assert_eq!(default_market["title"],"US cached release");
        assert_eq!(backend.online_job.lock().unwrap().as_ref().unwrap()["id"],"refresh");
        handle_rpc_call(None,&backend,&db,"job.cancel".into(),json!({"kind":"discography"})).await.unwrap();
        assert!(online_cancel.load(Ordering::Relaxed));
        assert!(!download_cancel.load(Ordering::Relaxed));
        assert!(!local_cancel.load(Ordering::Relaxed));
        drop(held);
        missing.await.unwrap().unwrap();
        drop(gate);drop(backend);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn file_mutations_and_downloads_have_bidirectional_conflict_guards() {
        let dir=std::env::temp_dir().join(format!("file-job-guards-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let backend=Arc::new(Backend::new());
        for kind in ["apply","deep_apply","consolidate"] {
            backend.start_job(json!({"id":kind,"kind":kind,"status":"running"}),Arc::new(AtomicBool::new(false)));
            let error=handle_rpc_call(None,&backend,&db,"job.start".into(),json!({"kind":"download"})).await.unwrap_err();
            assert!(error.contains("Files are being updated"));
            backend.finish_job(json!({"id":kind,"kind":kind,"status":"complete"}));
        }
        backend.start_download_job(json!({"id":"download","kind":"download","status":"running"}),Arc::new(AtomicBool::new(false)));
        for kind in ["apply","deep_apply","consolidate"] {
            let error=handle_rpc_call(None,&backend,&db,"job.start".into(),json!({"kind":kind})).await.unwrap_err();
            assert!(error.contains("download is writing files"));
        }
        drop(backend);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn slow_state_reads_cannot_resurrect_completed_jobs() {
        let backend=Backend::new();
        let running=json!({"id":"local","kind":"local_duplicates","status":"running","message":"Checking local duplicates"});
        backend.start_job(running.clone(),Arc::new(AtomicBool::new(false)));
        let mut old_snapshot=json!({"job":running,"logs":[]});
        backend.finish_job(json!({"id":"local","kind":"local_duplicates","status":"complete","message":"Local duplicates ready"}));
        refresh_runtime_state(&backend,&mut old_snapshot);
        assert_eq!(old_snapshot["job"]["status"],"complete");
        assert_eq!(old_snapshot["logs"].as_array().unwrap().last().unwrap()["job_status"],"complete");
        backend.start_job(json!({"id":"next","kind":"scan","status":"running"}),Arc::new(AtomicBool::new(false)));
        refresh_runtime_state(&backend,&mut old_snapshot);
        assert_eq!(old_snapshot["job"]["id"],"next");
    }

    #[tokio::test]
    async fn local_preview_inputs_refresh_only_when_relevant_data_changes_and_gate_linking() {
        let dir=std::env::temp_dir().join(format!("shared-preview-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let backend=Arc::new(Backend::new());
        let root=dir.join("music").to_string_lossy().to_string();
        let path=format!("{root}/Old.flac");
        let metadata=json!({"albumartist":"Example","artist":"Example","album":"Release","title":"Track","date":"2020","tracknumber":"1","tracktotal":"1","discnumber":"1"}).to_string();
        db.connect().unwrap().execute("INSERT INTO local_files(path,root,size,mtime,metadata,present) VALUES(?,?,1,1,?,1)",(path.as_str(),root.as_str(),metadata.as_str())).await.unwrap();
        let args=json!({"route":"organise","action":"organise","root":root,"limit":10,"filter":"affected"});
        let first=handle_rpc_call(None,&backend,&db,"table".into(),args.clone()).await.unwrap();
        assert!(first["preview_id"].as_str().is_some());
        db.bump_revision(); // An unrelated online update must not discard local work.
        let unchanged=handle_rpc_call(None,&backend,&db,"table".into(),args.clone()).await.unwrap();
        assert_eq!(first["preview_id"],unchanged["preview_id"]);
        db.set_preference("organisation",&json!({"template":"{albumartist}/{album}/{title}"})).await.unwrap();
        db.bump_revision();
        let changed=handle_rpc_call(None,&backend,&db,"table".into(),args.clone()).await.unwrap();
        assert_ne!(first["preview_id"],changed["preview_id"]);
        db.note_local_change();
        let refreshed=handle_rpc_call(None,&backend,&db,"table".into(),args).await.unwrap();
        assert_ne!(changed["preview_id"],refreshed["preview_id"]);
        backend.start_job(json!({"id":"scan","kind":"scan","status":"running"}),Arc::new(AtomicBool::new(false)));
        let error=handle_rpc_call(None,&backend,&db,"job.start".into(),json!({"kind":"link","args":{"root":root}})).await.unwrap_err();
        assert!(error.contains("local index"));
        backend.finish_job(json!({"id":"scan","kind":"scan","status":"complete"}));
        drop(backend);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn saved_activity_survives_restart_and_verbose_jobs_without_crowding_other_streams() {
        let dir = std::env::temp_dir().join(format!("activity-restart-{}", uuid::Uuid::new_v4()));
        let db = Arc::new(TursoDb::open(dir.join("db")).await.unwrap());
        let backend = Backend::new();
        backend.set_db(db.clone());
        backend.start_job(json!({"id":"local","kind":"scan","status":"running","message":"Reading local tags"}), Arc::new(AtomicBool::new(false)));
        backend.finish_job(json!({"id":"local","kind":"scan","status":"complete","message":"Local tags checked"}));
        backend.start_download_job(json!({"id":"download","kind":"download","status":"running","message":"Download started"}), Arc::new(AtomicBool::new(false)));
        backend.finish_download_job(json!({"id":"download","kind":"download","status":"complete","message":"Download complete"}));
        backend.start_online_job(json!({"id":"online","kind":"discography","status":"running","message":"Refresh started"}), Arc::new(AtomicBool::new(false)));
        for index in 0..650 {
            backend.log_with_category(&format!("Checked artist {index}"), "info", Some("online"));
        }
        backend.finish_online_job(json!({"id":"online","kind":"discography","status":"complete","message":"Release refresh complete"}));
        tokio::time::timeout(std::time::Duration::from_secs(20), backend.flush_activity()).await.unwrap();
        assert_eq!(backend.log_pending.load(Ordering::SeqCst), 0);
        drop(backend);
        drop(db);

        let reopened = TursoDb::open(dir.join("db")).await.unwrap();
        let summaries = reopened.load_recent_logs(500).await.unwrap();
        assert_eq!(summaries.len(), 500);
        assert_eq!(summaries.last().unwrap()["message"], "Release refresh complete");
        let history = reopened.activity_history(None, 1000, None).await.unwrap();
        let online: Vec<_> = history["entries"].as_array().unwrap().iter().filter(|entry| entry["job_id"] == "online").collect();
        assert_eq!(online.len(), 652);
        assert_eq!(online.first().unwrap()["message"], "Release refresh complete");
        assert_eq!(online.last().unwrap()["message"], "Refresh started");
        reopened.clear_log_stream("online").await.unwrap();
        assert_eq!(reopened.load_recent_logs(500).await.unwrap().len(), 4);
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    #[ignore = "Read-only live catalogue refresh, metadata, cancellation and resume using a disposable database"]
    async fn live_catalogue_refresh_pipeline() {
        let dir = std::env::temp_dir().join(format!("catalogue-refresh-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let backend = Arc::new(Backend::new());
        async fn wait(backend: &Arc<Backend>) -> Value {
            tokio::time::timeout(std::time::Duration::from_secs(120),async {
                loop {
                    let job = backend.online_job.lock().unwrap().clone().unwrap();
                    if ["complete","failed","cancelled"].contains(&job["status"].as_str().unwrap_or("")) { return job; }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }).await.expect("Refresh did not finish")
        }
        for (label,ids,detailed) in [
            ("cold release lists",vec!["3870503","3924"],false),
            ("warm release lists",vec!["3870503","3924"],false),
            ("cold recommendation metadata",vec!["3870503"],true),
            ("warm recommendation metadata",vec!["3870503"],true),
        ] {
            let start=std::time::Instant::now();
            handle_rpc_call(None,&backend,&db,"job.start".into(),json!({"kind":"discography","args":{"ids":ids,"market":"GB","detailed":detailed}})).await.unwrap();
            let job=wait(&backend).await;
            assert_eq!(job["status"],"complete","{job}");
            assert_eq!(job["result"]["checked"],ids.len());
            if label == "warm recommendation metadata" {
                assert_eq!(job["result"]["checked_details"],0,"Unchanged complete details must not be fetched again: {job}");
                assert!(job["result"]["reused_details"].as_u64().unwrap() > 0);
            }
            let saved=db.get_preference("catalogue-refresh-checkpoint").await.unwrap().unwrap();
            assert_eq!(saved["completed"].as_array().unwrap().len(),ids.len());
            println!("{label}: {} artists, {} ms",ids.len(),start.elapsed().as_millis());
        }
        let args=json!({"kind":"discography","args":{"ids":["3870503","3924"],"market":"GB"}});
        handle_rpc_call(None,&backend,&db,"job.start".into(),args.clone()).await.unwrap();
        handle_rpc_call(None,&backend,&db,"job.cancel".into(),json!({"kind":"discography"})).await.unwrap();
        assert_eq!(wait(&backend).await["status"],"cancelled");
        let mut resumed=args;
        resumed["args"]["resume"]=json!(true);
        handle_rpc_call(None,&backend,&db,"job.start".into(),resumed).await.unwrap();
        assert_eq!(wait(&backend).await["status"],"complete");
        println!("Cancellation and resume passed; no local audio accessed");
        drop(backend);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn catalogue_resume_skips_only_saved_artists_in_the_same_scope() {
        let ids = vec!["one".into(), "two".into(), "three".into()];
        let saved = serde_json::json!({"market":"GB","detailed":false,"completed":["one","one","removed","two"]});
        assert_eq!(super::completed_refresh_ids(&saved, &ids, "GB", false, true), vec!["one", "two"]);
        assert!(super::completed_refresh_ids(&saved, &ids, "US", false, true).is_empty());
        assert!(super::completed_refresh_ids(&saved, &ids, "GB", true, true).is_empty());
        assert!(super::completed_refresh_ids(&saved, &ids, "GB", false, false).is_empty());
    }

    #[test]
    fn activity_buffers_route_categories_and_clear_independently() {
        let buffers = super::ActivityBuffers::default();
        buffers.push(serde_json::json!({"at":"2026-09-26T12:00:00Z","category":"scan","message":"Scanning downloads folder"}));
        buffers.push(serde_json::json!({"at":"2026-09-26T12:00:01Z","category":"online","message":"Searching catalogue"}));
        buffers.push(serde_json::json!({"at":"2026-09-26T12:00:02Z","category":"download","message":"Fetching track"}));
        assert_eq!(buffers.local.lock().unwrap().entries.len(), 1);
        assert_eq!(buffers.online.lock().unwrap().entries.len(), 1);
        assert_eq!(buffers.downloads.lock().unwrap().entries.len(), 1);
        buffers.clear("local");
        assert_eq!(buffers.snapshot().len(), 2);
    }
    use super::*;
    #[test]
    fn workflow_summaries_describe_results_without_internal_action_codes() {
        assert_eq!(workflow_outcome("local_duplicates", &json!({"opportunities":4,"reused":true}), false), "Complete · Check local duplicates · 4 opportunities · saved analysis reused");
        assert_eq!(workflow_name("deep_apply"), "Save reviewed release links");
        assert!(workflow_outcome("apply", &json!({"applied":2}), true).starts_with("Cancelled · Apply reviewed changes · 2 files updated"));
    }

    #[test]
    fn activity_job_identity_survives_verbose_logs_and_download_monitor_updates() {
        let backend = Backend::new();
        backend.start_job(json!({"id":"local","kind":"correct","status":"running"}), Arc::new(AtomicBool::new(false)));
        backend.start_online_job(json!({"id":"online","kind":"discography","status":"running"}), Arc::new(AtomicBool::new(false)));
        backend.log_for("discography", "Download folder checked while reading credits", "error");
        let logs = backend.logs.snapshot();
        assert_eq!(logs.last().unwrap()["job_id"], "online");
        assert_eq!(activity_stream(logs.last().unwrap()), "online");
        backend.finish_online_job(json!({"id":"online","kind":"discography","status":"complete","message":"Credits cached"}));
        backend.start_online_job(json!({"id":"verbose","kind":"discography","status":"running"}), Arc::new(AtomicBool::new(false)));
        for index in 0..1100 { backend.log_for("discography", &format!("Saved release {index}"), "info"); }
        assert!(!backend.logs.snapshot().iter().any(|entry| entry["job_id"] == "online")); // Older rows are available through saved pagination.
        assert!(backend.logs.online.lock().unwrap().entries.len() <= 1000);

        backend.start_download_job(json!({"id":"download","kind":"download","status":"running"}), Arc::new(AtomicBool::new(false)));
        backend.record_download_monitor("download", json!({"kind":"track","release_id":"1","id":"2","title":"Track","status":"downloading","bytes":100})).unwrap();
        let item = backend.record_download_monitor("download", json!({"kind":"track","release_id":"1","id":"2","status":"complete"})).unwrap();
        assert_eq!(item["bytes"], 100);
        assert_eq!(item["job_id"], "download");
        assert!(item["updated_at"].is_i64());
        backend.finish_download_job(json!({"id":"download","kind":"download","status":"complete","message":"Downloaded"}));
        assert!(backend.record_download_monitor("download", json!({"kind":"track","id":"2","status":"downloading"})).is_none());
    }

    #[test]
    fn download_and_local_jobs_have_independent_cancellation() {
        let backend = Backend::new();
        let download_cancel = Arc::new(AtomicBool::new(false));
        let local_cancel = Arc::new(AtomicBool::new(false));
        backend.start_download_job(
            json!({"id":"download","kind":"download","status":"running"}),
            download_cancel.clone(),
        );
        backend.start_job(
            json!({"id":"scan","kind":"scan","status":"running"}),
            local_cancel.clone(),
        );
        backend.cancel_download_job().unwrap();
        assert!(download_cancel.load(Ordering::Relaxed));
        assert!(!local_cancel.load(Ordering::Relaxed));
        backend.finish_download_job(json!({"id":"download","status":"cancelled"}));
        assert_eq!(
            backend.active_job.lock().unwrap().as_ref().unwrap()["id"],
            "scan"
        );
    }
    #[test]
    fn online_local_and_download_jobs_keep_separate_progress_and_cancel_flags() {
        let backend = Backend::new();
        let online_cancel = Arc::new(AtomicBool::new(false));
        let local_cancel = Arc::new(AtomicBool::new(false));
        let download_cancel = Arc::new(AtomicBool::new(false));
        backend.start_online_job(
            json!({"id":"online","kind":"link","status":"running"}),
            online_cancel.clone(),
        );
        backend.start_job(
            json!({"id":"local","kind":"scan","status":"running"}),
            local_cancel.clone(),
        );
        backend.start_download_job(
            json!({"id":"download","kind":"download","status":"running"}),
            download_cancel.clone(),
        );
        backend.progress_for("link", "Checking 2/10 releases");
        backend.progress_for("scan", "Scanning 7/20 files");
        assert_eq!(
            backend.online_job.lock().unwrap().as_ref().unwrap()["message"],
            "Checking 2/10 releases"
        );
        assert_eq!(
            backend.active_job.lock().unwrap().as_ref().unwrap()["message"],
            "Scanning 7/20 files"
        );
        backend.cancel_online_job().unwrap();
        assert!(online_cancel.load(Ordering::Relaxed));
        assert!(!local_cancel.load(Ordering::Relaxed));
        assert!(!download_cancel.load(Ordering::Relaxed));
    }
    #[test]
    fn late_worker_updates_cannot_restart_finished_jobs() {
        let backend=Backend::new();
        let job=json!({"id":"old","kind":"download","status":"running"});
        backend.start_download_job(job.clone(),Arc::new(AtomicBool::new(false)));
        backend.finish_download_job(json!({"id":"old","status":"cancelled"}));
        assert_eq!(backend.update_download_job(job.clone())["status"],"cancelled");
        backend.start_download_job(json!({"id":"new","status":"running"}),Arc::new(AtomicBool::new(false)));
        assert_eq!(backend.update_download_job(job)["id"],"new");
    }
    #[test]
    fn progress_keeps_one_live_row_and_archives_job_details() {
        let backend = Backend::new();
        let job = json!({"id":"one","kind":"scan","status":"running"});
        backend.start_job(job.clone(), Arc::new(AtomicBool::new(false)));
        for i in 0..100 {
            backend.update_job_progress(&format!("Scanning {i}"), job.clone());
        }
        assert_eq!(backend.logs.snapshot().iter().filter(|r|r["progress_id"] == "one").count(), 1);
        assert_eq!(backend.logs.snapshot().len(), 101);
        backend.update_job_progress("One file failed", job.clone());
        assert_eq!(backend.logs.snapshot().len(), 102);
        backend.finish_job(
            json!({"id":"one","kind":"scan","status":"complete","message":"Scanned 100 files"}),
        );
        let logs = backend.logs.snapshot();
        assert_eq!(logs.len(), 102);
        assert!(logs.iter().all(|row|row["job_id"] == "one"));
        assert_eq!(logs.last().unwrap()["job_status"], "complete");
        assert!(logs.iter().any(|row| row["level"] == "error"));
        assert!(logs.iter().all(|row| row.get("progress_id").is_none()));
    }
}
