#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Write,
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
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

fn is_online_job(kind: &str) -> bool {
    matches!(kind, "link" | "discography" | "release_details" | "connections" | "favourites" | "match_artists" | "metadata" | "manual_candidate" | "artwork" | "check_replacements" | "optimizations" | "deep_review" | "deep_preview" | "connect_account" | "connect_download")
}

fn compare_table_cell(a: &Value, b: &Value, key: &str) -> std::cmp::Ordering {
    match (a[key].as_f64(), b[key].as_f64()) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        _ => {
            let display = |value: &Value| value.as_str().map(str::to_owned).unwrap_or_else(|| if value.is_null() { String::new() } else { value.to_string() });
            display(&a[key]).to_lowercase().cmp(&display(&b[key]).to_lowercase())
        }
    }
}

fn activity_stream(entry: &Value) -> &'static str {
    let category = entry["category"].as_str().unwrap_or("general");
    match category {
        "download" => return "downloads",
        "online" | "linking" => return "online",
        "scan" | "cleanup" | "local" => return "local",
        _ => {}
    }
    let message = entry["message"].as_str().unwrap_or("").to_lowercase();
    if ["download", "fetching track", "saving track"].iter().any(|word| message.contains(word)) { "downloads" }
    else if ["catalogue", "api", "remote", "artist search", "releases", "metadata source"].iter().any(|word| message.contains(word)) { "online" }
    else { "local" }
}

fn activity_stream_index(stream: &str) -> usize {
    match stream { "online" => 0, "downloads" => 2, _ => 1 }
}

#[derive(Default)]
pub struct ActivityBuffers {
    online: Mutex<Vec<Value>>,
    local: Mutex<Vec<Value>>,
    downloads: Mutex<Vec<Value>>,
}

impl ActivityBuffers {
    fn buffer(&self, stream: &str) -> &Mutex<Vec<Value>> {
        match stream { "online" => &self.online, "downloads" => &self.downloads, _ => &self.local }
    }

    fn push(&self, entry: Value) {
        let mut entries = self.buffer(activity_stream(&entry)).lock().unwrap();
        entries.push(entry);
        if entries.len() > 1000 { entries.remove(0); }
    }

    fn replace_progress(&self, id: &str, entry: Value) {
        let mut entries = self.buffer(activity_stream(&entry)).lock().unwrap();
        if let Some(existing) = entries.iter_mut().find(|item| item["progress_id"].as_str() == Some(id)) { *existing = entry; }
        else { entries.push(entry); }
    }

    fn retain(&self, mut keep: impl FnMut(&Value) -> bool) {
        for stream in [&self.online, &self.local, &self.downloads] {
            stream.lock().unwrap().retain(|entry| keep(entry));
        }
    }

    fn clear(&self, stream: &str) {
        if stream == "all" {
            self.online.lock().unwrap().clear();
            self.local.lock().unwrap().clear();
            self.downloads.lock().unwrap().clear();
        } else { self.buffer(stream).lock().unwrap().clear(); }
    }

    fn load(&self, entries: Vec<Value>) {
        self.clear("all");
        for entry in entries { self.push(entry); }
    }

    fn snapshot(&self) -> Vec<Value> {
        let mut entries = Vec::new();
        entries.extend(self.online.lock().unwrap().iter().cloned());
        entries.extend(self.local.lock().unwrap().iter().cloned());
        entries.extend(self.downloads.lock().unwrap().iter().cloned());
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
    pub logs: ActivityBuffers,
    pub log_epochs: Arc<[AtomicU64; 3]>,
    pub log_persist_gate: Arc<tokio::sync::Mutex<()>>,
    pub previews: Mutex<HashMap<String, Value>>,
    pub dispatch_gate: tokio::sync::Mutex<()>,
    pub read_gate: tokio::sync::Mutex<()>,
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
            logs: ActivityBuffers::default(),
            log_epochs: Arc::new([AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)]),
            log_persist_gate: Arc::new(tokio::sync::Mutex::new(())),
            previews: Mutex::new(HashMap::new()),
            dispatch_gate: tokio::sync::Mutex::new(()),
            read_gate: tokio::sync::Mutex::new(()),
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

    pub fn log(&self, msg: &str) {
        self.log_with_category(msg, "info", None);
    }

    pub fn log_with_category(&self, msg: &str, level: &str, category: Option<&str>) {
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
        self.logs.push(json!({
            "at": &at,
            "message": msg,
            "level": log_level,
            "category": cat
        }));

        // Persist to database if initialized and logging persistence is enabled
        if self.persist_logs.load(Ordering::SeqCst) {
            if let Some(db) = self.db.lock().unwrap().clone() {
                let stream = activity_stream(&json!({"category":cat,"message":msg}));
                let index = activity_stream_index(stream);
                let epoch = self.log_epochs[index].load(Ordering::SeqCst);
                let epochs = self.log_epochs.clone();
                let gate = self.log_persist_gate.clone();
                let at_str = at;
                let msg_str = msg.to_string();
                let lvl_str = log_level.to_string();
                let cat_str = cat.to_string();
                tauri::async_runtime::spawn(async move {
                    let _guard = gate.lock().await;
                    if epochs[index].load(Ordering::SeqCst) == epoch {
                        let _ = db.log_activity(&at_str, &msg_str, &lvl_str, &cat_str).await;
                    }
                });
            }
        }
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
            } else { self.log_with_category(message, "info", Some("online")); }
        } else { self.progress(message); }
    }

    pub fn start_online_job(&self, job: Value, cancel: Arc<AtomicBool>) {
        if let Some(message) = job["message"].as_str() { self.log_with_category(message, "info", Some("online")); }
        *self.online_cancel.lock().unwrap() = Some(cancel);
        *self.online_job.lock().unwrap() = Some(job);
    }

    pub fn update_online_job_progress(&self, message: &str, job: Value) {
        let id = job["id"].as_str().unwrap_or("online").to_string();
        *self.online_job.lock().unwrap() = Some(job);
        let entry = json!({"at":chrono::Utc::now().to_rfc3339(),"message":message,"level":"info","category":"online","progress_id":id});
        self.logs.replace_progress(&id, entry);
    }

    pub fn finish_online_job(&self, job: Value) {
        self.view_cache.lock().unwrap().clear();
        self.logs.retain(|entry| entry["progress_id"] != job["id"]);
        if let Some(message) = job["message"].as_str() { self.log_with_category(message, if job["status"] == "failed" { "error" } else { "info" }, Some("online")); }
        *self.online_job.lock().unwrap() = Some(job);
        *self.online_cancel.lock().unwrap() = None;
    }

    pub fn cancel_online_job(&self) -> Option<Value> {
        self.online_cancel.lock().unwrap().as_ref()?.store(true, Ordering::Relaxed);
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
            self.log_with_category(msg, "info", cat);
        }
        *self.active_job_cancel.lock().unwrap() = Some(cancel_flag);
        *self.active_job.lock().unwrap() = Some(job);
    }

    pub fn start_download_job(&self, job: Value, cancel_flag: Arc<AtomicBool>) {
        self.log_with_category("Download started", "info", Some("download"));
        *self.download_cancel.lock().unwrap() = Some(cancel_flag);
        *self.download_job.lock().unwrap() = Some(job);
    }

    pub fn update_download_job(&self, mut job: Value) {
        if self.download_cancel.lock().unwrap().as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            job["status"] = json!("cancelling");
        }
        *self.download_job.lock().unwrap() = Some(job);
    }

    pub fn finish_download_job(&self, job: Value) {
        if let Some(message) = job["message"].as_str() {
            self.log_with_category(message, if job["status"] == "failed" { "error" } else { "info" }, Some("download"));
        }
        *self.download_job.lock().unwrap() = Some(job);
        *self.download_cancel.lock().unwrap() = None;
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

    pub fn update_job_progress(&self, msg: &str, mut job: Value) {
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
        let lower = msg.to_lowercase();
        if lower.contains("failed") || lower.contains("unavailable") || lower.contains("warning") {
            self.log_with_category(msg, "error", cat);
        }
        let id = job["id"].as_str().unwrap_or("progress");
        let entry = json!({"at":chrono::Utc::now().to_rfc3339(),"message":msg,"level":"info","category":cat.unwrap_or("general"),"progress_id":id});
        self.logs.replace_progress(id, entry);
        *self.active_job.lock().unwrap() = Some(job);
    }

    pub fn finish_job(&self, final_job: Value) {
        self.view_cache.lock().unwrap().clear();
        self.logs.retain(|entry| entry["progress_id"] != final_job["id"]);
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
            self.log_with_category(msg, lvl, cat);
        }
        *self.active_job.lock().unwrap() = Some(final_job);
        *self.active_job_cancel.lock().unwrap() = None;
    }

    pub fn cancel_active_job(&self, cancel_msg: &str) -> Option<Value> {
        let cancel_flag = self.active_job_cancel.lock().unwrap().clone();
        if let Some(flag) = cancel_flag {
            flag.store(true, Ordering::Relaxed);
            self.log_with_category(cancel_msg, "warn", Some("general"));
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

async fn handle_rpc_call(
    app_handle: Option<&tauri::AppHandle>,
    state: &Arc<Backend>,
    db: &TursoDb,
    method: String,
    args: Value,
) -> Result<Value, String> {
    let cacheable = method == "table" || method == "state";
    let _read = if cacheable {
        Some(state.read_gate.lock().await)
    } else {
        None
    };
    let revision = db.revision.load(Ordering::SeqCst);
    let key = format!("{method}:{}", args);
    if cacheable {
        let saved = state.view_cache.lock().unwrap().get(&key).cloned();
        if let Some((rev, created, mut value)) = saved {
            if rev == revision && created.elapsed().as_secs() < 30 {
                if method == "state" {
                    if let Some(job) = state.active_job.lock().unwrap().clone() {
                        value["job"] = job;
                    }
                    value["download_job"] = json!(state.download_job.lock().unwrap().clone());
                    value["online_job"] = json!(state.online_job.lock().unwrap().clone());
                    value["logs"] = json!(state.logs.snapshot());
                    value["auth_url"] = state
                        .pending_pkce
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|flow| json!(flow.login_url))
                        .unwrap_or(Value::Null);
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
        || method.starts_with("credentials.")
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
    if method == "job.status" {
        return Ok(
            json!({"job":state.active_job.lock().unwrap().clone(),"online_job":state.online_job.lock().unwrap().clone(),"download_job":state.download_job.lock().unwrap().clone(),"logs":state.logs.snapshot(),"auth_url":state.pending_pkce.lock().unwrap().as_ref().map(|f|f.login_url.clone())}),
        );
    }
    if method == "table" || method == "detail" || method == "job.start" {
        let preferences = db
            .get_preference("desktop")
            .await?
            .or(db.get_preference("ui").await?)
            .unwrap_or(Value::Null);
        let market = preferences["market"]
            .as_str()
            .unwrap_or("GB")
            .to_uppercase();
        if let Some(obj) = args.as_object_mut() {
            obj.entry("market").or_insert(json!(market));
        }
        if let Some(obj) = args.get_mut("args").and_then(Value::as_object_mut) {
            obj.entry("market").or_insert(json!(market));
        }
    }
    let _dispatch = if method == "job.start" || method == "auth.reply" {
        Some(state.dispatch_gate.lock().await)
    } else {
        None
    };
    if method == "job.start" && args["kind"] != "download" {
        let kind = args["kind"].as_str().unwrap_or("");
        let occupied = if is_online_job(kind) { state.online_cancel.lock().unwrap().is_some() } else { state.active_job_cancel.lock().unwrap().is_some() };
        if occupied { return Err("A task in this section is already running. Wait for completion or cancel it first.".into()); }
    }
    if method == "job.start" && actions::handles(args["kind"].as_str().unwrap_or("")) {
        let kind = args["kind"].as_str().unwrap().to_string();
        let input = args.get("args").cloned().unwrap_or_else(|| args.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.;
        let cancel = Arc::new(AtomicBool::new(false));
        let initial = json!({"id":id,"kind":kind,"status":"running","message":format!("Started · {kind}"),"started":started});
        if is_online_job(&kind) { state.start_online_job(initial.clone(), cancel.clone()); }
        else { state.start_job(initial.clone(), cancel.clone()); }
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
                    format!("Finished · {kind}"),
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
            let finished = json!({"id":id,"kind":kind,"status":status,"message":message,"started":started,"finished":chrono::Utc::now().timestamp_millis() as f64/1000.,"result":value});
            if is_online_job(&kind) { backend.finish_online_job(finished.clone()); }
            else { backend.finish_job(finished.clone()); }
            database.bump_revision();
            let _ = database.set_preference("desktop-last-job", &finished).await;
            if let Some(app) = app {
                let _ = app.emit("backend-event", if is_online_job(&kind) { json!({"event":"job","online_job":finished}) } else { json!({"event":"job","job":finished}) });
                let _ = app.emit("backend-event", json!({"event":"changed"}));
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
                    "No TIDAL developer credentials configured in Keychain or environment."
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
                    "No TIDAL developer credentials configured in Keychain or environment."
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
                    "No TIDAL developer credentials configured in Keychain or environment."
                        .to_string(),
                )
            }
        };
        let catalogue = client.get_artist_catalogue(id, market, detailed).await?;
        return Ok(serde_json::to_value(catalogue).unwrap_or_default());
    }
    if method == "turso.tags.write" {
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
        tag_writer::write_tags(std::path::Path::new(path_str), &updates)?;
        return Ok(json!({ "status": "ok", "updated": updates.len() }));
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
    if method == "turso.mqa.audit" {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing path".to_string())?;
        let res = mqa::audit_file(std::path::Path::new(path_str));
        return serde_json::to_value(res).map_err(|e| e.to_string());
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

        let (files, _) = db.get_local_files_page(Some(root), 10000, 0).await?;
        let plans = workflows::plan_workflow(&files, action, template);
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
        let mut logs = state.logs.snapshot();
        if logs.is_empty() {
            if let Ok(loaded) = db.load_recent_logs(500).await {
                if !loaded.is_empty() {
                    state.logs.load(loaded.clone());
                    logs = loaded;
                }
            }
        }
        let root = args.get("root").and_then(|v| v.as_str());
        let mut snapshot = db.get_state(active, &logs, root).await?;
        snapshot["online_job"] = json!(state.online_job.lock().unwrap().clone());
        snapshot["download_job"] = json!(state.download_job.lock().unwrap().clone());
        snapshot["auth_url"] = state
            .pending_pkce
            .lock()
            .unwrap()
            .as_ref()
            .map(|flow| json!(flow.login_url))
            .unwrap_or(Value::Null);
        return Ok(snapshot);
    }
    if method == "settings" {
        return db.get_settings().await;
    }
    if method == "settings.save" || method == "settings.update" {
        let section = args.get("section").and_then(|v| v.as_str()).unwrap_or("ui");
        let values = args.get("values").unwrap_or(&args);
        if section == "desktop" || section == "general" {
            if let Some(p) = values.get("persist_logs").and_then(|v| v.as_bool()) {
                state.persist_logs.store(p, Ordering::SeqCst);
            }
        }
        let res = db.save_settings(section, values).await?;
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
        let mut logs = state.logs.snapshot();
        if logs.is_empty() {
            if let Ok(loaded) = db.load_recent_logs(500).await {
                if !loaded.is_empty() {
                    state.logs.load(loaded.clone());
                    logs = loaded;
                }
            }
        }
        return Ok(json!(logs));
    }
    if method == "logs.clear" {
        let stream = args.get("stream").and_then(Value::as_str).unwrap_or("all");
        let _guard = state.log_persist_gate.lock().await;
        if stream == "all" {
            for epoch in state.log_epochs.iter() { epoch.fetch_add(1, Ordering::SeqCst); }
            state.logs.clear("all");
            db.clear_logs().await?;
        } else {
            if !["online", "local", "downloads"].contains(&stream) { return Err("Unknown activity stream".into()); }
            state.log_epochs[activity_stream_index(stream)].fetch_add(1, Ordering::SeqCst);
            state.logs.clear(stream);
            db.clear_log_stream(stream).await?;
        }
        return Ok(json!({ "cleared": true, "stream": stream }));
    }
    if method == "missing.rebuild" {
        db.invalidate_missing_rows();
        state.view_cache.lock().unwrap().clear();
        if let Some(app) = app_handle { let _ = app.emit("backend-event", json!({"event":"changed"})); }
        return Ok(json!({"rebuilt":true}));
    }

    // TABLE ROUTES
    if method == "turso.links"
        || (method == "table"
            && (args.get("route").and_then(|v| v.as_str()) == Some("links")
                || args.get("route").and_then(|v| v.as_str()) == Some("files")))
    {
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let root_opt = args.get("root").and_then(|v| v.as_str());
        let root = if let Some(r) = root_opt {
            r.to_string()
        } else {
            let roots = db.list_roots(market).await?;
            roots.into_iter().next().map(|r| r.root).unwrap_or_default()
        };
        if root.is_empty() {
            return Ok(json!({
                "rows": [],
                "total": 0,
                "offset": 0,
                "revision": 0,
                "preview_id": null
            }));
        }
        let filter = args.get("filter").and_then(|v| v.as_str());
        let search = args.get("search").and_then(|v| v.as_str());
        let sort = args.get("sort").and_then(|v| v.as_str());
        let direction = args.get("direction").and_then(|v| v.as_str());
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

        let page = db
            .get_link_rows(
                market, &root, filter, search, sort, direction, offset, limit,
            )
            .await?;
        return serde_json::to_value(page).map_err(|e| e.to_string());
    }
    if method == "turso.missing"
        || (method == "table" && args.get("route").and_then(|v| v.as_str()) == Some("missing"))
    {
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let timeline = args.get("timeline").and_then(|v| v.as_str());
        let recommendation = args.get("recommendation").and_then(|v| v.as_str());
        let status_filter = args
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("filter").and_then(|v| v.as_str()));
        let type_filter = args.get("type").and_then(|v| v.as_str());
        let search = args.get("search").and_then(|v| v.as_str());
        let sort = args.get("sort").and_then(|v| v.as_str());
        let direction = args.get("direction").and_then(|v| v.as_str());
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

        let page = db
            .get_missing_rows(
                market,
                timeline,
                recommendation,
                status_filter,
                type_filter,
                search,
                sort,
                direction,
                offset,
                limit,
            )
            .await?;
        return serde_json::to_value(page).map_err(|e| e.to_string());
    }
    if method == "table" {
        let route = args
            .get("route")
            .and_then(|v| v.as_str())
            .unwrap_or("files");
        let search = args.get("search").and_then(|v| v.as_str());
        let sort = args.get("sort").and_then(|v| v.as_str());
        let direction = args.get("direction").and_then(|v| v.as_str());
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
        let filter = args.get("filter").and_then(|v| v.as_str());

        let operation = if route == "correct" || route == "organise" {
            args["action"].as_str().unwrap_or("dates")
        } else {
            route
        };
        let preview = {
            let previews = state.previews.lock().unwrap();
            args["preview_id"]
                .as_str()
                .and_then(|id| previews.get(id))
                .or_else(|| {
                    previews
                        .values()
                        .filter(|p| {
                            p["root"] == args["root"] && p["operation"].as_str() == Some(operation)
                        })
                        .max_by_key(|p| p["created"].as_i64().unwrap_or(0))
                })
                .cloned()
        };
        let mut cached_rows = if let Some(ref p) = preview {
            p["rows"].as_array().cloned()
        } else if ["mqa", "local", "online"].contains(&route) {
            Some(
                db.get_preference(&format!(
                    "desktop-{route}:{}",
                    args["root"].as_str().unwrap_or("")
                ))
                .await?
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default(),
            )
        } else {
            None
        };
        if route == "local" && cached_rows.as_ref().is_some_and(|rows| rows.iter().any(|row| row.get("date").is_none() || row.get("children").is_none())) {
            let root = args["root"].as_str().unwrap_or("");
            let indexed = actions::files(db, root).await?;
            let mut rows = cached_rows.take().unwrap_or_default();
            if rows.iter().any(|row| row.get("children").is_none()) {
                rows = duplicates::clusters_to_group_rows(&duplicates::find_duplicate_clusters(&indexed));
            } else {
                let dates: HashMap<String,String> = indexed.iter().filter_map(|file| {
                    let tags = workflows::extract_tags_map(&file.metadata);
                    Some((duplicates::extract_release_folder(&file.path),tags.get("date")?.clone()))
                }).collect();
                for row in &mut rows {
                    row["date"] = json!(row["path"].as_str().and_then(|path|dates.get(path)).cloned().unwrap_or_default());
                    if let Some(children)=row["children"].as_array_mut() {
                        for child in children { child["date"]=json!(child["path"].as_str().and_then(|path|dates.get(path)).cloned().unwrap_or_default()); }
                    }
                }
            }
            db.set_preference(&format!("desktop-local:{root}"), &json!(rows)).await?;
            cached_rows=Some(rows);
        }
        if let Some(mut rows) = cached_rows {
            if filter == Some("affected") {
                rows.retain(|r| r["affected"] == true);
            }
            if let Some(q) = search.filter(|s| !s.is_empty()) {
                let q = q.to_lowercase();
                rows.retain(|r| r.to_string().to_lowercase().contains(&q));
            }
            let sort = sort.unwrap_or("artist");
            rows.sort_by(|a, b| compare_table_cell(a, b, sort));
            if direction == Some("desc") {
                rows.reverse();
            }
            let total = rows.len();
            return Ok(
                json!({"rows":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"total":total,"offset":offset,"revision":db.revision.load(Ordering::SeqCst),"preview_id":preview.as_ref().map(|p|p["id"].clone())}),
            );
        }
        if route == "queue" || route == "downloaded" {
            let page = db
                .get_queue_rows(route, filter, search, sort, direction, offset, limit)
                .await?;
            return serde_json::to_value(page).map_err(|e| e.to_string());
        }
        if route == "artists" {
            let root = args.get("root").and_then(|v| v.as_str());
            let page = db
                .get_artist_rows(root, filter, search, sort, direction, offset, limit)
                .await?;
            return serde_json::to_value(page).map_err(|e| e.to_string());
        }
        if route == "favourites" {
            let page = db.get_favourite_rows(args.get("root").and_then(Value::as_str), filter.unwrap_or("all"), search.unwrap_or(""), sort.unwrap_or("artist"), direction.unwrap_or("asc"), offset, limit).await?;
            return serde_json::to_value(page).map_err(|e| e.to_string());
        }
        if route == "correct" || route == "organise" {
            let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
            let root_opt = args.get("root").and_then(|v| v.as_str());
            let root = if let Some(r) = root_opt {
                r.to_string()
            } else {
                let roots = db.list_roots(market).await?;
                roots.into_iter().next().map(|r| r.root).unwrap_or_default()
            };
            let action =
                args.get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or(if route == "correct" {
                        "dates"
                    } else {
                        "organise"
                    });
            let files = actions::files(db, &root).await?;
            let settings = db.get_settings().await.unwrap_or(json!({}));
            let template_str = settings
                .get("organisation")
                .and_then(|v| v.get("template"))
                .and_then(|v| v.as_str());
            let plans = workflows::plan_workflow(&files, action, template_str);
            let file_index: HashMap<_, _> = files.iter().map(|file| (file.path.as_str(), file)).collect();
            let preview_id = uuid::Uuid::new_v4().to_string();
            let mut rows: Vec<Value> = plans.iter().filter_map(|plan| {
                let file = file_index.get(plan.path.as_str())?;
                let description = if plan.target.is_some() { "Move or rename file to match its tags" } else { "Standardise local tags" };
                Some(json!({"id":plan.path,"path":plan.path,"artist":plan.artist,"release":plan.album,"title":plan.title,
                    "tags":plan.current_tags,"changes":plan.changes,"target":plan.target,"evidence":description,
                    "affected":true,"status":"Needs update","size":file.size,"mtime":file.mtime,
                    "item":{"path":plan.path,"target":plan.target,"tags":plan.changes}}))
            }).collect();
            let affected_paths: std::collections::HashSet<String> = plans.iter().map(|plan|plan.path.clone()).collect();
            for file in &files {
                if affected_paths.contains(&file.path) { continue }
                let tags = workflows::extract_tags_map(&file.metadata);
                rows.push(json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),
                    "release":tags.get("album"),"title":tags.get("title"),"tags":tags,"changes":{},"target":null,
                    "affected":false,"status":"No change","evidence":"No changes needed for this operation"}));
            }
            state.previews.lock().unwrap().insert(preview_id.clone(), json!({"id":preview_id,
                "created":chrono::Utc::now().timestamp_millis(),"operation":action,"root":root,"rows":rows,"count":plans.len()}));
            if filter == Some("affected") { rows.retain(|row| row["affected"] == true); }
            if let Some(q) = search.filter(|q| !q.is_empty()) {
                let query=q.to_lowercase(); rows.retain(|row|row.to_string().to_lowercase().contains(&query));
            }
            let key = sort.unwrap_or("artist");
            rows.sort_by(|a, b| compare_table_cell(a, b, key));
            if direction == Some("desc") { rows.reverse(); }
            let total=rows.len();
            return Ok(json!({"rows":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"total":total,
                "offset":offset,"revision":db.revision.load(Ordering::SeqCst),"preview_id":preview_id}));
        }

        if route == "metadata" || route == "artwork" || route == "mqa" {
            let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
            let root_opt = args.get("root").and_then(|v| v.as_str());
            let root = if let Some(r) = root_opt {
                r.to_string()
            } else {
                let roots = db.list_roots(market).await?;
                roots.into_iter().next().map(|r| r.root).unwrap_or_default()
            };
            if root.is_empty() {
                return Ok(json!({
                    "rows": [],
                    "total": 0,
                    "offset": 0,
                    "revision": 0,
                    "preview_id": null
                }));
            }
            let page = db
                .get_link_rows(
                    market, &root, filter, search, sort, direction, offset, limit,
                )
                .await?;
            return serde_json::to_value(page).map_err(|e| e.to_string());
        }

        if route == "local" {
            let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
            let root_opt = args.get("root").and_then(|v| v.as_str());
            let root = if let Some(r) = root_opt {
                r.to_string()
            } else {
                let roots = db.list_roots(market).await?;
                roots.into_iter().next().map(|r| r.root).unwrap_or_default()
            };
            if root.is_empty() {
                return Ok(json!({
                    "rows": [],
                    "total": 0,
                    "offset": 0,
                    "revision": 0,
                    "preview_id": null
                }));
            }
            let files = actions::files(db, &root).await?;
            let clusters = duplicates::find_duplicate_clusters(&files);
            let mut rows = duplicates::clusters_to_link_rows(&clusters);

            if let Some(q) = search {
                let q_lower = q.to_lowercase();
                rows.retain(|r| {
                    r.artist.to_lowercase().contains(&q_lower)
                        || r.release.to_lowercase().contains(&q_lower)
                        || r.target.to_lowercase().contains(&q_lower)
                        || r.evidence.to_lowercase().contains(&q_lower)
                });
            }

            if let Some(key) = sort {
                rows.sort_by(|a, b| {
                    let left = serde_json::to_value(a).unwrap_or(Value::Null);
                    let right = serde_json::to_value(b).unwrap_or(Value::Null);
                    let order = compare_table_cell(&left, &right, key);
                    if direction == Some("desc") { order.reverse() } else { order }
                });
            }
            let total = rows.len();
            let page_rows = rows.into_iter().skip(offset).take(limit).collect();
            return serde_json::to_value(crate::db::TablePage {
                rows: page_rows,
                total,
                offset,
                revision: 0,
                preview_id: Some("local_duplicates".to_string()),
            })
            .map_err(|e| e.to_string());
        }

        return Ok(json!({
            "rows": [],
            "total": 0,
            "offset": 0,
            "revision": 0,
            "preview_id": null
        }));
    }

    // DETAILS & PREVIEW
    if method == "detail" {
        return db.get_detail(&args).await;
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
        let page = db.get_queue_rows("queue", None, None, None, None, 0, usize::MAX).await?;
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
        db.queue_redownload(release_id, args["track_id"].as_str()).await?;
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
        let text = db.queue_export(format, decision).await?;
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
        db.choose_track_link(&args).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
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
    if method == "credentials.save" {
        let client = args.get("client").and_then(|v| v.as_str()).unwrap_or("");
        let secret = args.get("secret").and_then(|v| v.as_str()).unwrap_or("");
        crate::tidal::TidalClient::save_credentials(client, secret).map_err(|e| e.to_string())?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "credentials.forget" {
        crate::tidal::TidalClient::forget_credentials().map_err(|e| e.to_string())?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
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
        return Ok(json!({ "safe": state.active_job_cancel.lock().unwrap().is_none() && state.online_cancel.lock().unwrap().is_none() && state.download_cancel.lock().unwrap().is_none() }));
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
            "message": "Scanning files…",
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
                    backend_prog.update_job_progress(msg, prog_job.clone());
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
                            "Refresh local files · finished; {} tags read · {} unchanged · {} missing",
                            summary.read, summary.unchanged, summary.missing
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
                            "errors": summary.errors,
                        }),
                    )
                }
                Err(e) => ("failed", format!("Scan failed: {}", e), json!(null)),
            };

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
                let _ = app.emit("backend-event", json!({ "event": "changed" }));
            }
        });

        return Ok(initial_job);
    }
    if method == "turso.discography"
        || (method == "job.start"
            && args.get("kind").and_then(|v| v.as_str()) == Some("discography"))
    {
        let client_opt = tidal::TidalClient::from_db(db).await.ok();
        let mut client = match client_opt {
            Some(c) => c,
            None => {
                return Err(
                    "No TIDAL developer credentials configured in Keychain or environment."
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

        let ids: Vec<String> = if let Some(ids_val) = inner_args.get("ids") {
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

        if ids.is_empty() {
            return Err("No linked artists found in library. Match artists first.".to_string());
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let total = ids.len();
        let initial_job = json!({
            "id": job_id,
            "kind": "discography",
            "status": "running",
            "message": format!("Refreshing releases · 0/{}", total),
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
            let mut checked = 0;
            let mut failure: Option<String> = None;
            let backend_prog = backend_task.clone();

            for (index, artist_id) in ids.iter().enumerate() {
                if cancel_flag.load(Ordering::Relaxed) {
                    break;
                }

                let msg = format!("Refreshing releases · {}/{}", index + 1, total);
                let prog_job = json!({
                    "id": j_id.clone(),
                    "kind": "discography",
                    "status": "running",
                    "message": msg.clone(),
                    "started": started,
                    "result": null
                });
                backend_prog.update_online_job_progress(&msg, prog_job.clone());
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

                match client
                    .get_artist_catalogue(artist_id, &market, detailed)
                    .await
                {
                    Ok(catalogue) => {
                        if let Err(e) = client
                            .save_catalogue_to_db(&db_clone, &market, &catalogue)
                            .await
                        {
                            backend_task
                                .log(&format!("Could not save releases for {artist_id}: {e}"));
                            failure = Some(e);
                            break;
                        } else {
                            checked += 1;
                        }
                    }
                    Err(e) => {
                        backend_task
                            .log(&format!("Could not refresh releases for {artist_id}: {e}"));
                        failure = Some(e);
                        break;
                    }
                }
            }

            let is_cancelled = cancel_flag.load(Ordering::Relaxed);
            let status = if is_cancelled {
                "cancelled"
            } else if failure.is_some() {
                "failed"
            } else {
                "complete"
            };
            let message = if is_cancelled {
                "Refresh release list · cancelled; completed results retained".to_string()
            } else if let Some(error) = failure {
                format!("Refresh stopped after {checked} artists; cached results retained. {error}")
            } else {
                format!(
                    "Refresh release list · finished; {} artists checked",
                    checked
                )
            };

            let finished_at = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
            let final_job = json!({
                "id": j_id,
                "kind": "discography",
                "status": status,
                "message": message,
                "started": started,
                "finished": finished_at,
                "result": json!({ "checked": checked })
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
                    backend_prog.update_online_job_progress(&msg, prog_job.clone());
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
            let _ = app.emit("backend-event", json!({ "event": "job", "online_job": job }));
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

            let dl_res = downloads::DownloadManager::run_downloads(
                &db_clone,
                app_clone.as_ref(),
                cancel_flag.clone(),
                &j_id,
                move |msg| {
                    let lower = msg.to_lowercase();
                    if lower.contains("failed") || lower.contains("error") || lower.contains("unavailable") {
                        backend_prog.log_with_category(&msg, "error", Some("download"));
                    }
                    let prog_job = json!({
                        "id": j_id_prog.clone(),
                        "kind": "download",
                        "status": "running",
                        "message": msg.clone(),
                        "started": started,
                        "result": null
                    });
                    backend_prog.update_download_job(prog_job.clone());
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
                    if let Some(ref a) = app_monitor {
                        let _ = a.emit("backend-event", json!({"event":"download-monitor","item":item}));
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
                let _ = app.emit("backend-event", json!({ "event": "changed" }));
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
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return serde_json::to_value(res).map_err(|e| e.to_string());
    }
    if method == "job.cancel" {
        let requested = args["kind"].as_str().unwrap_or("");
        let pending_sign_in = if requested.is_empty() || requested.starts_with("connect_") {
            state.pending_pkce.lock().unwrap().take().is_some()
        } else { false };
        if pending_sign_in && requested.is_empty() { return Ok(json!(true)); }
        if requested == "download" || (requested.is_empty() && state.active_job_cancel.lock().unwrap().is_none() && state.online_cancel.lock().unwrap().is_none()) {
            if let Some(job) = state.cancel_download_job() {
                if let Some(app) = app_handle {
                    let _ = app.emit("backend-event", json!({"event":"job","download_job":job}));
                }
            }
            return Ok(json!(true));
        }
        if is_online_job(requested) || (requested.is_empty() && state.online_cancel.lock().unwrap().is_some()) {
            if let Some(job) = state.cancel_online_job() {
                if let Some(app) = app_handle { let _ = app.emit("backend-event", json!({"event":"job","online_job":job})); }
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
                    state.pending_pkce.lock().unwrap().take();
                    state.log("Tidal account connected · download authorization ready");
                    if let Some(app) = app_handle {
                        let _ = app.emit("backend-event", json!({ "event": "ready" }));
                        let _ = app.emit("backend-event", json!({ "event": "changed" }));
                    }
                    return Ok(json!(true));
                }
                Err(e) => {
                    state.log(&format!("Failed to complete sign-in: {}", e));
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
    if args.iter().any(|a| a == "--rpc") {
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
            backend.persist_logs.store(persist, std::sync::atomic::Ordering::SeqCst);
            if let Ok(loaded) = tauri::async_runtime::block_on(turso_db.load_recent_logs(500)) {
                if loaded.is_empty() {
                    backend.log_with_category(
                        &format!("Tibrary v{} ready · workspace initialized", env!("CARGO_PKG_VERSION")),
                        "info",
                        Some("general"),
                    );
                } else {
                    backend.logs.load(loaded);
                }
            }
            app.manage(turso_db);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend_call,
            open_external,
            reveal_file,
            save_export
        ])
        .run(tauri::generate_context!())
        .expect("Could not start Tibrary");
}

#[cfg(test)]
mod activity_tests {
    #[test]
    fn activity_buffers_route_categories_and_clear_independently() {
        let buffers = super::ActivityBuffers::default();
        buffers.push(serde_json::json!({"at":"2026-09-26T12:00:00Z","category":"scan","message":"Scanning downloads folder"}));
        buffers.push(serde_json::json!({"at":"2026-09-26T12:00:01Z","category":"online","message":"Searching catalogue"}));
        buffers.push(serde_json::json!({"at":"2026-09-26T12:00:02Z","category":"download","message":"Fetching track"}));
        assert_eq!(buffers.local.lock().unwrap().len(), 1);
        assert_eq!(buffers.online.lock().unwrap().len(), 1);
        assert_eq!(buffers.downloads.lock().unwrap().len(), 1);
        buffers.clear("local");
        assert_eq!(buffers.snapshot().len(), 2);
    }
    use super::*;
    #[test]
    fn download_and_local_jobs_have_independent_cancellation() {
        let backend = Backend::new();
        let download_cancel = Arc::new(AtomicBool::new(false));
        let local_cancel = Arc::new(AtomicBool::new(false));
        backend.start_download_job(json!({"id":"download","kind":"download","status":"running"}), download_cancel.clone());
        backend.start_job(json!({"id":"scan","kind":"scan","status":"running"}), local_cancel.clone());
        backend.cancel_download_job().unwrap();
        assert!(download_cancel.load(Ordering::Relaxed));
        assert!(!local_cancel.load(Ordering::Relaxed));
        backend.finish_download_job(json!({"id":"download","status":"cancelled"}));
        assert_eq!(backend.active_job.lock().unwrap().as_ref().unwrap()["id"], "scan");
    }
    #[test]
    fn online_local_and_download_jobs_keep_separate_progress_and_cancel_flags() {
        let backend = Backend::new();
        let online_cancel = Arc::new(AtomicBool::new(false));
        let local_cancel = Arc::new(AtomicBool::new(false));
        let download_cancel = Arc::new(AtomicBool::new(false));
        backend.start_online_job(json!({"id":"online","kind":"link","status":"running"}), online_cancel.clone());
        backend.start_job(json!({"id":"local","kind":"scan","status":"running"}), local_cancel.clone());
        backend.start_download_job(json!({"id":"download","kind":"download","status":"running"}), download_cancel.clone());
        backend.progress_for("link", "Checking 2/10 releases");
        backend.progress_for("scan", "Scanning 7/20 files");
        assert_eq!(backend.online_job.lock().unwrap().as_ref().unwrap()["message"], "Checking 2/10 releases");
        assert_eq!(backend.active_job.lock().unwrap().as_ref().unwrap()["message"], "Scanning 7/20 files");
        backend.cancel_online_job().unwrap();
        assert!(online_cancel.load(Ordering::Relaxed));
        assert!(!local_cancel.load(Ordering::Relaxed));
        assert!(!download_cancel.load(Ordering::Relaxed));
    }
    #[test]
    fn progress_replaces_one_row_and_preserves_errors() {
        let backend = Backend::new();
        let job = json!({"id":"one","kind":"scan","status":"running"});
        backend.start_job(job.clone(), Arc::new(AtomicBool::new(false)));
        for i in 0..100 {
            backend.update_job_progress(&format!("Scanning {i}"), job.clone());
        }
        assert_eq!(backend.logs.snapshot().len(), 1);
        backend.update_job_progress("One file failed", job.clone());
        assert_eq!(backend.logs.snapshot().len(), 2);
        backend.finish_job(
            json!({"id":"one","kind":"scan","status":"complete","message":"Scanned 100 files"}),
        );
        let logs = backend.logs.snapshot();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().all(|row| row.get("progress_id").is_none()));
    }
}
