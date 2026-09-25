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
pub mod db;
pub mod downloads;
pub mod duplicates;
pub mod enrichment;
pub mod linking;
pub mod maintenance;
pub mod matching;
pub mod mqa;
pub mod musical_keys;
pub mod organisation;
pub mod recommendations;
pub mod release_matching;
pub mod scanner;
pub mod tag_writer;
pub mod tidal;
pub mod stream_download;
pub mod workflows;
use db::TursoDb;

#[derive(Default)]
pub struct Backend {
    pub active_job_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub active_job: Mutex<Option<Value>>,
    pub logs: Mutex<Vec<Value>>,
    pub previews: Mutex<HashMap<String, Value>>,
    pub pending_pkce: Mutex<Option<crate::stream_download::PkceFlow>>,
}

impl Backend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn log(&self, msg: &str) {
        let mut logs = self.logs.lock().unwrap();
        logs.push(json!({
            "at": chrono::Utc::now().to_rfc3339(),
            "message": msg
        }));
        if logs.len() > 800 {
            logs.remove(0);
        }
    }

    pub fn start_job(&self, job: Value, cancel_flag: Arc<AtomicBool>) {
        if let Some(msg) = job.get("message").and_then(|v| v.as_str()) {
            self.log(msg);
        }
        *self.active_job_cancel.lock().unwrap() = Some(cancel_flag);
        *self.active_job.lock().unwrap() = Some(job);
    }

    pub fn update_job_progress(&self, msg: &str, job: Value) {
        self.log(msg);
        *self.active_job.lock().unwrap() = Some(job);
    }

    pub fn finish_job(&self, final_job: Value) {
        if let Some(msg) = final_job.get("message").and_then(|v| v.as_str()) {
            self.log(msg);
        }
        *self.active_job.lock().unwrap() = Some(final_job);
        *self.active_job_cancel.lock().unwrap() = None;
    }

    pub fn cancel_active_job(&self, cancel_msg: &str) -> Option<Value> {
        let cancel_flag = self.active_job_cancel.lock().unwrap().clone();
        if let Some(flag) = cancel_flag {
            flag.store(true, Ordering::Relaxed);
            self.log(cancel_msg);
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
        let client_opt = tidal::TidalClient::from_env_or_keychain();
        let mut client = match client_opt {
            Some(c) => c,
            None => {
                return Err(
                    "No TIDAL developer credentials configured in Keychain or environment."
                        .to_string(),
                )
            }
        };
        let token = client.authenticate().await?;
        return Ok(json!({
            "status": "ok",
            "authenticated": true,
            "client_id": client.client_id,
            "token_preview": format!("{}...", &token[..token.len().min(8)]),
        }));
    }
    if method == "turso.tidal.search" {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing query".to_string())?;
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let client_opt = tidal::TidalClient::from_env_or_keychain();
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
        let client_opt = tidal::TidalClient::from_env_or_keychain();
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
        let target = organisation::format_layout(
            std::path::Path::new(root_str),
            &tags,
            template,
            ext,
        )?;
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
        let local_tags_val = args.get("local_tags").cloned().unwrap_or(Value::Object(serde_json::Map::new()));
        let mut local_tags = std::collections::HashMap::new();
        if let Some(obj) = local_tags_val.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    local_tags.insert(k.clone(), s.to_string());
                }
            }
        }
        let release: tidal::TidalRelease = serde_json::from_value(
            args.get("release").cloned().unwrap_or(Value::Null)
        ).map_err(|e| format!("Invalid release: {}", e))?;
        let track: tidal::TidalTrack = serde_json::from_value(
            args.get("track").cloned().unwrap_or(Value::Null)
        ).map_err(|e| format!("Invalid track: {}", e))?;

        let missing = enrichment::compute_missing_tags(&local_tags, &release, &track);
        return serde_json::to_value(missing).map_err(|e| e.to_string());
    }
    if method == "turso.workflows.plan" {
        let root = args.get("root").and_then(|v| v.as_str()).unwrap_or("");
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("organise");
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
        let local_name = args.get("local_name").and_then(|v| v.as_str()).unwrap_or("");
        let local_albums: Vec<String> = args
            .get("local_albums")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let candidate_id = args.get("candidate_id").and_then(|v| v.as_str()).unwrap_or("");
        let candidate_name = args.get("candidate_name").and_then(|v| v.as_str()).unwrap_or("");
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
        let logs = state.logs.lock().unwrap().clone();
        let root = args.get("root").and_then(|v| v.as_str());
        return db.get_state(active, &logs, root).await;
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
        let logs = state.logs.lock().unwrap().clone();
        return Ok(json!(logs));
    }

    // TABLE ROUTES
    if method == "turso.links"
        || (method == "table" && (args.get("route").and_then(|v| v.as_str()) == Some("links") || args.get("route").and_then(|v| v.as_str()) == Some("files")))
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
                market,
                &root,
                filter,
                search,
                sort,
                direction,
                offset,
                limit,
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
        let route = args.get("route").and_then(|v| v.as_str()).unwrap_or("files");
        let search = args.get("search").and_then(|v| v.as_str());
        let sort = args.get("sort").and_then(|v| v.as_str());
        let direction = args.get("direction").and_then(|v| v.as_str());
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
        let filter = args.get("filter").and_then(|v| v.as_str());

        if route == "queue" || route == "downloaded" {
            let page = db.get_queue_rows(route, filter, search, sort, direction, offset, limit).await?;
            return serde_json::to_value(page).map_err(|e| e.to_string());
        }
        if route == "artists" {
            let page = db.get_artist_rows(search, sort, direction, offset, limit).await?;
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
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or(if route == "correct" { "dates" } else { "organise" });
            let (files, _) = db.get_local_files_page(Some(&root), 10000, 0).await?;
            let plans = workflows::plan_workflow(&files, action, None);
            let mut rows = Vec::new();
            for p in plans {
                let affected = !p.changes.is_empty() || p.target.is_some();
                let change_desc = if !p.changes.is_empty() {
                    p.changes.iter().map(|(k, v)| format!("{}: {}", k, v)).collect::<Vec<_>>().join(" · ")
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
                    status: if affected { "Needs update".to_string() } else { "No change".to_string() },
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
                preview_id: Some("workflow".to_string()),
            }).map_err(|e| e.to_string());
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
                    market,
                    &root,
                    filter,
                    search,
                    sort,
                    direction,
                    offset,
                    limit,
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
            let (files, _) = db.get_local_files_page(Some(&root), 20000, 0).await?;
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
            }).map_err(|e| e.to_string());
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
        let selection: HashMap<String, Option<Vec<String>>> = serde_json::from_value(sel_val).unwrap_or_default();
        db.queue_select(&selection).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "queue.decision" {
        let ids_val = args.get("ids").cloned().unwrap_or(json!([]));
        let ids: Vec<String> = serde_json::from_value(ids_val).unwrap_or_default();
        let decision = args.get("decision").and_then(|v| v.as_str()).unwrap_or("queued");
        db.queue_decision(&ids, decision).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "queue.add" {
        let sel_val = args.get("selection").cloned().unwrap_or(json!({}));
        let selection: HashMap<String, Option<Vec<String>>> = serde_json::from_value(sel_val).unwrap_or_default();
        db.queue_add(&selection).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "queue.export" {
        let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("json");
        let text = db.queue_export(format).await?;
        return Ok(json!({ "text": text }));
    }

    // LIBRARY ACTIONS
    if method == "library.add" {
        let path = args.get("path").and_then(|v| v.as_str()).ok_or("Missing path")?;
        db.add_root(path).await?;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!({ "root": path }));
    }
    if method == "library.remove" {
        let root = args.get("root").and_then(|v| v.as_str()).ok_or("Missing root")?;
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
        let ignored = args.get("ignored").and_then(|v| v.as_bool()).unwrap_or(true);
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
        let _ = crate::tidal::TidalClient::save_credentials(client, secret);
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "credentials.forget" {
        let _ = crate::tidal::TidalClient::forget_credentials();
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "account.disconnect" {
        let _ = crate::account::AccountClient::disconnect();
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(json!(true));
    }
    if method == "shutdown" {
        return Ok(json!({ "safe": true }));
    }

    // JOBS
    if method == "turso.scan"
        || (method == "job.start"
            && args.get("kind").and_then(|v| v.as_str()) == Some("scan"))
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

        tauri::async_runtime::spawn(async move {
            let j_id_prog = j_id.clone();
            let app_prog = app_clone.clone();
            let backend_prog = backend_task.clone();

            let scan_res = scanner::scan_library(
                &db_clone,
                &root_path,
                cancel_flag.clone(),
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
            let _ = db_clone.set_preference("desktop-last-job", &final_job).await;

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
        let client_opt = tidal::TidalClient::from_env_or_keychain();
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
                    return Err(
                        "Select a linked artist before refreshing its releases".to_string(),
                    );
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

                match client.get_artist_catalogue(artist_id, &market, detailed).await {
                    Ok(catalogue) => {
                        if let Err(e) = client.save_catalogue_to_db(&db_clone, &market, &catalogue).await {
                            eprintln!("Failed to save catalogue for {}: {}", artist_id, e);
                        } else {
                            checked += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to fetch artist catalogue for {}: {}", artist_id, e);
                    }
                }
            }

            let is_cancelled = cancel_flag.load(Ordering::Relaxed);
            let status = if is_cancelled { "cancelled" } else { "complete" };
            let message = if is_cancelled {
                "Refresh release list · cancelled; completed results retained".to_string()
            } else {
                format!("Refresh release list · finished; {} artists checked", checked)
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
            let _ = db_clone.set_preference("desktop-last-job", &final_job).await;

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
        || (method == "job.start"
            && args.get("kind").and_then(|v| v.as_str()) == Some("link"))
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

        tauri::async_runtime::spawn(async move {
            let app_prog = app_clone.clone();
            let backend_prog = backend_task.clone();
            let j_id_prog = j_id.clone();

            let link_res = linking::link_library(
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
            )
            .await;

            let is_cancelled = cancel_flag.load(Ordering::Relaxed);
            let (status, message, result_val) = match link_res {
                Ok(summary) => {
                    let st = if is_cancelled { "cancelled" } else { "complete" };
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
            let _ = db_clone.set_preference("desktop-last-job", &final_job).await;

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
            let _ = app.emit("backend-event", json!({ "event": "authentication", "auth_url": auth_url }));
            let _ = app.emit("backend-event", json!({ "event": "job", "job": job }));
        }

        return Ok(job);
    }
    if method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("component_check") {
        let job_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let job = json!({
            "id": job_id,
            "kind": "component_check",
            "status": "complete",
            "message": "Tibrary uses built-in high-performance native Rust streaming components. All components are up to date.",
            "started": now,
            "finished": now,
            "result": { "message": "All native components are up to date." }
        });
        state.finish_job(job.clone());
        let _ = db.set_preference("desktop-last-job", &job).await;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "job", "job": job }));
        }
        return Ok(job);
    }
    if method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("component_update") {
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
    if method == "job.start" && args.get("kind").and_then(|v| v.as_str()) == Some("component_rollback") {
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

            let dl_res = if let Some(ref app) = app_clone {
                downloads::DownloadManager::run_downloads(
                    &db_clone,
                    app,
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
                .await
            } else {
                Ok(0)
            };

            let is_cancelled = cancel_flag.load(Ordering::Relaxed);
            let (status, message, count) = match dl_res {
                Ok(c) => {
                    let st = if is_cancelled { "cancelled" } else { "complete" };
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
            let _ = db_clone.set_preference("desktop-last-job", &final_job).await;

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
        let items_val = args.get("items").cloned().unwrap_or(Value::Array(Vec::new()));
        let items: Vec<maintenance::FileApplyItem> = serde_json::from_value(items_val)
            .map_err(|e| format!("Invalid apply items: {}", e))?;
        let res = maintenance::apply_batch(db, root, &items).await;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return serde_json::to_value(res).map_err(|e| e.to_string());
    }
    if method == "job.start"
        && args.get("kind").and_then(|v| v.as_str()) == Some("optimizations")
    {
        let inner_args = args.get("args").cloned().unwrap_or(args.clone());
        let root = inner_args.get("root").and_then(|v| v.as_str()).unwrap_or("");
        let job_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let (files, _) = db.get_local_files_page(if root.is_empty() { None } else { Some(root) }, 20000, 0).await?;
        let clusters = duplicates::find_duplicate_clusters(&files);
        let chained_count = clusters.iter().filter(|c| c.is_chained).count();
        let total_redundant: usize = clusters.iter().map(|c| c.redundant.len()).sum();
        let finished = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let summary_msg = if chained_count > 0 {
            format!("Found {} duplicate releases across {} consolidation clusters ({} chained)", total_redundant, clusters.len(), chained_count)
        } else {
            format!("Found {} duplicate releases across {} consolidation clusters", total_redundant, clusters.len())
        };
        let final_job = json!({
            "id": job_id,
            "kind": "optimizations",
            "status": "complete",
            "message": summary_msg.clone(),
            "started": started,
            "finished": finished,
            "result": json!({
                "summary": summary_msg,
                "clusters": clusters.len(),
                "redundant": total_redundant,
                "chained": chained_count
            })
        });
        state.finish_job(final_job.clone());
        let _ = db.set_preference("desktop-last-job", &final_job).await;
        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "job", "job": final_job }));
            let _ = app.emit("backend-event", json!({ "event": "changed" }));
        }
        return Ok(final_job);
    }
    if method == "job.start"
        && args.get("kind").and_then(|v| v.as_str()) == Some("review_consolidation")
    {
        let inner_args = args.get("args").cloned().unwrap_or(args.clone());
        let root = inner_args.get("root").and_then(|v| v.as_str()).unwrap_or("");
        let ids: Vec<String> = inner_args.get("ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let (files, _) = db.get_local_files_page(if root.is_empty() { None } else { Some(root) }, 20000, 0).await?;
        let clusters = duplicates::find_duplicate_clusters(&files);
        let preview_id = format!("preview-consolidation-{}", uuid::Uuid::new_v4());

        let mut selected_files_to_delete = Vec::new();
        let mut preview_rows = Vec::new();

        for cluster in &clusters {
            let mut cluster_has_selection = false;
            for red in &cluster.redundant {
                let row_id = format!("{}::{}", cluster.cluster_id, red.folder);
                if ids.contains(&row_id) || ids.contains(&red.folder) || ids.contains(&cluster.cluster_id) {
                    cluster_has_selection = true;
                    break;
                }
            }

            if cluster_has_selection || ids.is_empty() {
                for red in &cluster.redundant {
                    let row_id = format!("{}::{}", cluster.cluster_id, red.folder);
                    if ids.is_empty() || ids.contains(&row_id) || ids.contains(&red.folder) || ids.contains(&cluster.cluster_id) {
                        for t in &red.tracks {
                            selected_files_to_delete.push(t.path.clone());
                        }
                        preview_rows.push(json!({
                            "id": row_id,
                            "artist": red.artist,
                            "release": red.title,
                            "target": cluster.master.title,
                            "path": red.folder,
                            "changes": format!("Move to Trash: {} tracks (preserved in {})", red.tracks.len(), cluster.master.title),
                            "evidence": cluster.chain_summary,
                            "is_chained": cluster.is_chained,
                            "master_folder": cluster.master.folder,
                            "master_title": cluster.master.title,
                            "tracks": red.tracks.len(),
                            "files": red.tracks.iter().map(|t| t.path.clone()).collect::<Vec<_>>(),
                        }));
                    }
                }
            }
        }

        let preview_payload = json!({
            "id": preview_id.clone(),
            "operation": "consolidate",
            "scope": inner_args.get("scope").and_then(|v| v.as_str()).unwrap_or("local"),
            "root": root,
            "rows": preview_rows,
            "count": selected_files_to_delete.len(),
            "files": selected_files_to_delete,
        });

        state.previews.lock().unwrap().insert(preview_id.clone(), preview_payload.clone());

        let job_id = uuid::Uuid::new_v4().to_string();
        let now_sec = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let job = json!({
            "id": job_id,
            "kind": "review_consolidation",
            "status": "complete",
            "message": format!("Reviewed {} files for duplicate removal", selected_files_to_delete.len()),
            "started": now_sec,
            "finished": now_sec,
            "result": json!({
                "preview_id": preview_id,
                "operation": "consolidate",
                "root": root
            })
        });

        state.finish_job(job.clone());
        let _ = db.set_preference("desktop-last-job", &job).await;

        if let Some(app) = app_handle {
            let _ = app.emit("backend-event", json!({ "event": "job", "job": job }));
        }

        return Ok(job);
    }
    if method == "job.start"
        && args.get("kind").and_then(|v| v.as_str()) == Some("consolidate")
    {
        let inner_args = args.get("args").cloned().unwrap_or(args.clone());
        let preview_id = inner_args.get("preview_id").and_then(|v| v.as_str()).unwrap_or("");
        let job_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;

        let initial_job = json!({
            "id": job_id,
            "kind": "consolidate",
            "status": "running",
            "message": "Moving reviewed duplicates to Trash…",
            "started": started,
            "result": null
        });

        let cancel_flag = Arc::new(AtomicBool::new(false));
        state.start_job(initial_job.clone(), cancel_flag.clone());

        let mut files_to_delete = Vec::new();
        let mut folders_to_clean = Vec::new();

        if let Some(prev) = state.previews.lock().unwrap().get(preview_id) {
            if let Some(files_arr) = prev.get("files").and_then(|v| v.as_array()) {
                for f in files_arr {
                    if let Some(p) = f.as_str() {
                        files_to_delete.push(p.to_string());
                    }
                }
            }
            if let Some(rows_arr) = prev.get("rows").and_then(|v| v.as_array()) {
                for r in rows_arr {
                    if let Some(p) = r.get("path").and_then(|v| v.as_str()) {
                        folders_to_clean.push(p.to_string());
                    }
                }
            }
        }

        let db_clone = db.clone();
        let app_clone = app_handle.cloned();
        let backend_task = state.clone();
        let j_id = job_id.clone();

        tauri::async_runtime::spawn(async move {
            let res = duplicates::consolidate_redundant_releases(&db_clone, &files_to_delete, &folders_to_clean).await;
            let finished = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
            let (status, msg, count) = match res {
                Ok(c) => ("complete", format!("Successfully moved {} duplicate files to Trash", c), c),
                Err(e) => ("failed", format!("Error moving duplicates to Trash: {}", e), 0),
            };

            let final_job = json!({
                "id": j_id,
                "kind": "consolidate",
                "status": status,
                "message": msg,
                "started": started,
                "finished": finished,
                "result": json!({ "completed": count })
            });

            backend_task.finish_job(final_job.clone());
            let _ = db_clone.set_preference("desktop-last-job", &final_job).await;

            if let Some(ref app) = app_clone {
                let _ = app.emit("backend-event", json!({ "event": "job", "job": final_job }));
                let _ = app.emit("backend-event", json!({ "event": "changed" }));
            }
        });

        return Ok(initial_job);
    }
    if method == "job.cancel" {
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

        let flow = {
            let mut lock = state.pending_pkce.lock().unwrap();
            lock.take()
        };

        if let Some(flow) = flow {
            let http = reqwest::Client::new();
            match crate::stream_download::exchange_pkce_code(
                &http,
                redirect_url,
                &flow.code_verifier,
                &flow.client_unique_key,
            ).await {
                Ok(token) => {
                    let _ = crate::stream_download::save_token(db, &token).await;
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

    Ok(json!({}))
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
    let p = std::path::PathBuf::from(path);
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

            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout();
            for line in stdin.lines() {
                let Ok(line) = line else { break };
                let Ok(val) = serde_json::from_str::<Value>(&line) else { continue };
                let id = val.get("id").cloned().unwrap_or(Value::Null);
                let method = val.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
        .manage(backend)
        .setup(|app| {
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
