#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Write,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
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

#[derive(Default)]
pub struct Backend {
    pub db: Mutex<Option<Arc<TursoDb>>>,
    pub active_job_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub active_job: Mutex<Option<Value>>,
    pub logs: Mutex<Vec<Value>>,
    pub previews: Mutex<HashMap<String, Value>>,
    pub dispatch_gate: tokio::sync::Mutex<()>,
    pub read_gate: tokio::sync::Mutex<()>,
    pub view_cache: Mutex<HashMap<String, (u64, std::time::Instant, Value)>>,
    pub pending_pkce: Mutex<Option<crate::stream_download::PkceFlow>>,
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
        let mut logs = self.logs.lock().unwrap();
        logs.push(json!({
            "at": &at,
            "message": msg,
            "level": log_level,
            "category": cat
        }));
        if logs.len() > 1000 {
            logs.remove(0);
        }

        // Persist to database if initialized
        if let Some(db) = self.db.lock().unwrap().clone() {
            let at_str = at;
            let msg_str = msg.to_string();
            let lvl_str = log_level.to_string();
            let cat_str = cat.to_string();
            tauri::async_runtime::spawn(async move {
                let _ = db.log_activity(&at_str, &msg_str, &lvl_str, &cat_str).await;
            });
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
        {
            let mut logs = self.logs.lock().unwrap();
            if let Some(existing) = logs
                .iter_mut()
                .find(|v| v["progress_id"].as_str() == Some(id))
            {
                *existing = entry;
            } else {
                logs.push(entry);
            }
        }
        *self.active_job.lock().unwrap() = Some(job);
    }

    pub fn finish_job(&self, final_job: Value) {
        self.view_cache.lock().unwrap().clear();
        self.logs
            .lock()
            .unwrap()
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
                    value["logs"] = json!(state.logs.lock().unwrap().clone());
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
            json!({"job":state.active_job.lock().unwrap().clone(),"logs":state.logs.lock().unwrap().clone(),"auth_url":state.pending_pkce.lock().unwrap().as_ref().map(|f|f.login_url.clone())}),
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
    if method == "job.start" && state.active_job_cancel.lock().unwrap().is_some() {
        return Err("A job is already running. Wait for completion or cancel it first.".into());
    }
    if method == "job.start" && actions::handles(args["kind"].as_str().unwrap_or("")) {
        let kind = args["kind"].as_str().unwrap().to_string();
        let input = args.get("args").cloned().unwrap_or_else(|| args.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.;
        let cancel = Arc::new(AtomicBool::new(false));
        let initial = json!({"id":id,"kind":kind,"status":"running","message":format!("Started · {kind}"),"started":started});
        state.start_job(initial.clone(), cancel.clone());
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
            backend.finish_job(finished.clone());
            database.bump_revision();
            let _ = database.set_preference("desktop-last-job", &finished).await;
            if let Some(app) = app {
                let _ = app.emit("backend-event", json!({"event":"job","job":finished}));
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
        let mut logs = state.logs.lock().unwrap().clone();
        if logs.is_empty() {
            if let Ok(loaded) = db.load_recent_logs(500).await {
                if !loaded.is_empty() {
                    *state.logs.lock().unwrap() = loaded.clone();
                    logs = loaded;
                }
            }
        }
        let root = args.get("root").and_then(|v| v.as_str());
        let mut snapshot = db.get_state(active, &logs, root).await?;
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
        let mut logs = state.logs.lock().unwrap().clone();
        if logs.is_empty() {
            if let Ok(loaded) = db.load_recent_logs(500).await {
                if !loaded.is_empty() {
                    *state.logs.lock().unwrap() = loaded.clone();
                    logs = loaded;
                }
            }
        }
        return Ok(json!(logs));
    }
    if method == "logs.clear" {
        state.logs.lock().unwrap().clear();
        let _ = db.clear_logs().await;
        return Ok(json!({ "cleared": true }));
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
        if route == "local"
            && cached_rows
                .as_ref()
                .is_some_and(|rows| rows.iter().any(|row| row.get("children").is_none()))
        {
            let root = args["root"].as_str().unwrap_or("");
            let indexed = actions::files(db, root).await?;
            let rows =
                duplicates::clusters_to_group_rows(&duplicates::find_duplicate_clusters(&indexed));
            db.set_preference(&format!("desktop-local:{root}"), &json!(rows))
                .await?;
            cached_rows = Some(rows);
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
            rows.sort_by(|a, b| match (a[sort].as_f64(), b[sort].as_f64()) {
                (Some(a), Some(b)) => a.total_cmp(&b),
                _ => a[sort]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&b[sort].as_str().unwrap_or("").to_lowercase()),
            });
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
            let page = db.get_favourite_rows().await?;
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
            let plans = workflows::plan_workflow(&files, action, None);
            let mut rows = Vec::new();
            for p in plans {
                let affected = !p.changes.is_empty() || p.target.is_some();
                let change_desc = if !p.changes.is_empty() {
                    p.changes
                        .iter()
                        .map(|(k, v)| format!("{}: {}", k, v))
                        .collect::<Vec<_>>()
                        .join(" · ")
                } else {
                    p.issues.join(" · ")
                };
                rows.push(crate::db::LinkRow {
                    id: p.path.clone(),
                    artist: p.artist,
                    release: p.album,
                    title: p.title,
                    path: p.path,
                    position: String::new(),
                    status: if affected {
                        "Needs update".to_string()
                    } else {
                        "No change".to_string()
                    },
                    evidence: change_desc.clone(),
                    affected,
                    target: p.target.unwrap_or_default(),
                    changes: change_desc,
                    bpm: None,
                    key: None,
                    candidates: 0,
                    online_id: String::new(),
                    ignored: false,
                });
            }
            if filter == Some("affected") {
                rows.retain(|r| r.affected);
            }
            let total = rows.len();
            let page_rows = rows.into_iter().skip(offset).take(limit).collect();
            return serde_json::to_value(crate::db::TablePage {
                rows: page_rows,
                total,
                offset,
                revision: 0,
                preview_id: None,
            })
            .map_err(|e| e.to_string());
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
        let text = db.queue_export(format).await?;
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
        db.bump_revision();
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "shutdown" {
        return Ok(json!({ "safe": state.active_job_cancel.lock().unwrap().is_none() }));
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

        if state.active_job_cancel.lock().unwrap().is_some() {
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
        state.start_job(initial_job.clone(), cancel_flag.clone());

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
                backend_prog.update_job_progress(&msg, prog_job.clone());
                if let Some(ref app) = app_clone {
                    let _ = app.emit(
                        "backend-event",
                        json!({
                            "event": "progress",
                            "message": msg,
                            "job": prog_job
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
    if method == "turso.link"
        || (method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("link"))
    {
        if state.active_job_cancel.lock().unwrap().is_some() {
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
        state.start_job(initial_job.clone(), cancel_flag.clone());

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
                    backend_prog.update_job_progress(&msg, prog_job.clone());
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

        state.finish_job(job.clone());
        let _ = db.set_preference("desktop-last-job", &job).await;

        if let Some(app) = app_handle {
            let _ = app.emit(
                "backend-event",
                json!({ "event": "authentication", "auth_url": auth_url }),
            );
            let _ = app.emit("backend-event", json!({ "event": "job", "job": job }));
        }

        return Ok(job);
    }
    if method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("component_check")
    {
        let job_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let job = json!({
            "id": job_id,
            "kind": "component_check",
            "status": "complete",
            "message": "Tibrary uses built-in high-performance native Rust streaming components. Component versions are bundled with this app release.",
            "started": now,
            "finished": now,
            "result": { "message": "Native components are bundled; update Tibrary to update them." }
        });
        state.finish_job(job.clone());
        let _ = db.set_preference("desktop-last-job", &job).await;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "job", "job": job }));
        }
        return Ok(job);
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
        if state.active_job_cancel.lock().unwrap().is_some() {
            return Err("A job is already running".to_string());
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
        state.start_job(initial_job.clone(), cancel_flag.clone());

        let db_clone = db.clone();
        let app_clone = app_handle.cloned();
        let backend_task = state.clone();
        let j_id = job_id.clone();

        tauri::async_runtime::spawn(async move {
            let app_prog = app_clone.clone();
            let backend_prog = backend_task.clone();
            let j_id_prog = j_id.clone();

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
                    backend_prog.update_job_progress(&msg, prog_job.clone());
                    if let Some(ref a) = app_prog {
                        let _ = a.emit(
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
        state.pending_pkce.lock().unwrap().take();
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
                    *backend.logs.lock().unwrap() = loaded;
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
            if let Ok(loaded) = tauri::async_runtime::block_on(turso_db.load_recent_logs(500)) {
                if loaded.is_empty() {
                    backend.log_with_category(
                        "Tibrary v0.9.0-beta.1 ready · workspace initialized",
                        "info",
                        Some("general"),
                    );
                } else {
                    *backend.logs.lock().unwrap() = loaded;
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
    use super::*;
    #[test]
    fn progress_replaces_one_row_and_preserves_errors() {
        let backend = Backend::new();
        let job = json!({"id":"one","kind":"scan","status":"running"});
        backend.start_job(job.clone(), Arc::new(AtomicBool::new(false)));
        for i in 0..100 {
            backend.update_job_progress(&format!("Scanning {i}"), job.clone());
        }
        assert_eq!(backend.logs.lock().unwrap().len(), 1);
        backend.update_job_progress("One file failed", job.clone());
        assert_eq!(backend.logs.lock().unwrap().len(), 2);
        backend.finish_job(
            json!({"id":"one","kind":"scan","status":"complete","message":"Scanned 100 files"}),
        );
        let logs = backend.logs.lock().unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().all(|row| row.get("progress_id").is_none()));
    }
}
