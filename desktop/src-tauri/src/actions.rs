//! Desktop actions share the same native services as tables and reviewed writes.
use crate::{
    db::{LocalFileRecord, TursoDb},
    maintenance, workflows, Backend,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

// Bump when a future recommendation field requires another upstream lookup.
const RECOMMENDATION_SCHEMA: u32 = 1;

pub fn handles(kind: &str) -> bool {
    matches!(
        kind,
        "cached_releases"
            | "release_artists"
            | "preview"
            | "apply"
            | "mqa"
            | "release_details"
            | "connections"
            | "favourites"
            | "match_artists"
            | "metadata"
            | "manual_candidate"
            | "artwork"
            | "optimizations"
            | "local_duplicates"
            | "check_replacements"
            | "queue_replacements"
            | "queue_mqa"
            | "deep_review"
            | "deep_preview"
            | "deep_apply"
            | "review_consolidation"
            | "consolidate"
    )
}
pub async fn files(db: &TursoDb, root: &str) -> Result<Vec<LocalFileRecord>, String> {
    let mut out = vec![];
    loop {
        let (page, total) = db.get_local_files_page(Some(root), 1000, out.len()).await?;
        if page.is_empty() {
            break;
        }
        out.extend(page);
        if out.len() >= total {
            break;
        }
    }
    Ok(out)
}

/// Rebuild the visible audit list from indexed files and prior per-file results.
/// Changed files become "Not audited" until the user runs the audio inspection.
pub async fn cached_mqa_rows(
    db: &TursoDb,
    indexed: &[LocalFileRecord],
) -> Result<Vec<Value>, String> {
    let conn = db.connect()?;
    let mut query = conn
        .query(
            "SELECT key,payload FROM app_preferences WHERE key LIKE 'mqa-audit:%'",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut saved = std::collections::HashMap::new();
    while let Some(row) = query.next().await.map_err(|e| e.to_string())? {
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        let payload: String = row.get(1).map_err(|e| e.to_string())?;
        if let Ok(value) = serde_json::from_str::<Value>(&payload) {
            saved.insert(key.trim_start_matches("mqa-audit:").to_owned(), value);
        }
    }
    let rows = indexed.iter().filter(|file| file.present).map(|file| {
        let tags = workflows::extract_tags_map(&file.metadata);
        let prior = saved.get(&file.path).filter(|value| value["size"] == json!(file.size) && value["mtime"] == json!(file.mtime));
        let result = prior.map(|value| &value["result"]);
        json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),"release":tags.get("album"),"title":tags.get("title"),
            "status":result.and_then(|value| value["status"].as_str()).unwrap_or("Not audited"),
            "evidence":result.and_then(|value| value["evidence"].as_str()).unwrap_or("New or changed audio; run the MQA audit"),
            "affected":result.is_some_and(|value| value["detected"] == true),"target":if result.is_some_and(|v| v["detected"] == true) { "Queue lossless replacement" } else { "—" }})
    }).collect();
    Ok(rows)
}
// Serialize only identical releases; unrelated releases and local work stay independent.
pub(crate) fn release_gate(key: String) -> Arc<tokio::sync::Mutex<()>> {
    type Gates = std::collections::HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>;
    static GATES: std::sync::OnceLock<std::sync::Mutex<Gates>> = std::sync::OnceLock::new();
    let mut gates = GATES.get_or_init(Default::default).lock().unwrap();
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(&key).and_then(std::sync::Weak::upgrade) { return gate; }
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    gates.insert(key, Arc::downgrade(&gate));
    gate
}

/// A normal catalogue refresh checks live release lists, then enriches only the
/// new, changed or incomplete releases. Explicit release refreshes still force
/// an API read when a user wants to recheck tags on an unchanged recording.
pub(crate) async fn recommendation_refresh_plan(
    db: &TursoDb,
    market: &str,
    releases: &[crate::tidal::TidalRelease],
) -> Result<(HashSet<String>, HashSet<String>, HashSet<String>), String> {
    let ids: Vec<_> = releases.iter().map(|release| &release.id).collect();
    let conn = db.connect()?;
    let mut rows = conn.query(
        "SELECT key,payload FROM app_preferences WHERE key IN (SELECT 'tag-review:' || ? || ':' || value FROM json_each(?))",
        (market, json!(ids).to_string()),
    ).await.map_err(|error|error.to_string())?;
    let mut saved = std::collections::HashMap::new();
    let prefix = format!("tag-review:{market}:");
    while let Some(row) = rows.next().await.map_err(|error|error.to_string())? {
        let key: String = row.get(0).map_err(|error|error.to_string())?;
        let payload: String = row.get(1).map_err(|error|error.to_string())?;
        if let Ok(value) = serde_json::from_str::<Value>(&payload) {
            saved.insert(key.trim_start_matches(&prefix).to_owned(),value);
        }
    }
    let mut reused = HashSet::new();
    let mut changed = HashSet::new();
    let mut cached_tracks = HashSet::new();
    for release in releases {
        let Some(prior) = saved.get(&release.id) else { continue; };
        let Some(fingerprint) = release.summary_fingerprint.as_deref() else { continue; };
        if prior["recommendation_snapshot"]["summary_fingerprint"].as_str().is_some_and(|old|old != fingerprint) {
            changed.insert(release.id.clone());
        } else if recommendation_tracks_current(prior,fingerprint) {
            cached_tracks.insert(release.id.clone());
            if prior["recommendation_snapshot"]["optional_status"] == "complete"
                || prior["recommendation_snapshot"]["optional_retry_after"].as_i64().is_some_and(|until|until > chrono::Utc::now().timestamp()) {
                reused.insert(release.id.clone());
            }
        }
    }
    Ok((reused,changed,cached_tracks))
}

fn recommendation_tracks_current(value: &Value, fingerprint: &str) -> bool {
    value["recommendation_snapshot"]["schema"] == RECOMMENDATION_SCHEMA
        && value["recommendation_snapshot"]["summary_fingerprint"] == fingerprint
        && value["tracks_loaded"] == true && value["track_metadata_source"] == "subscriber"
        // A checked, empty credit list or unavailable BPM/key is complete.
        && value["tracks"].as_array().is_some_and(|tracks| !tracks.is_empty() && tracks.iter().all(|track|track["credits_complete"] == true))
}

fn summary_change_requires_track_refresh(reused_tracks: bool, before: Option<&Value>, after: Option<&Value>) -> bool {
    reused_tracks && before.zip(after).is_some_and(|(before,after)|crate::tidal::summary_fingerprint(before) != crate::tidal::summary_fingerprint(after))
}

async fn record_recommendation_snapshot(db: &TursoDb, value: &mut Value, market: &str, summary: Option<&Value>, optional_complete: bool) -> Result<(), String> {
    if let Some(summary) = summary {
        let fresh = serde_json::to_value(crate::tidal::release_from_subscriber(&summary,"")).map_err(|error|error.to_string())?;
        // An edition/title/primary-credit change must not be overwritten by the
        // earlier tag-review snapshot when fresh track details are published.
        for field in ["artist","title","date","type","copyright","label","quality","upc","original_release_date","artist_credits","audio_modes","media_metadata","summary_fingerprint"] {
            if !fresh[field].is_null() && !fresh[field].as_str().is_some_and(str::is_empty) && !fresh[field].as_array().is_some_and(Vec::is_empty) {
                value[field]=fresh[field].clone();
            }
        }
        let retry_after = if optional_complete {Value::Null} else {
            db.get_preference(&format!("subscriber-discovery-paused:{market}")).await?
                .and_then(|paused|paused["until"].as_i64()).filter(|until|*until > chrono::Utc::now().timestamp())
                .map(|until|json!(until)).unwrap_or_else(||json!(chrono::Utc::now().timestamp()+3600))
        };
        value["recommendation_snapshot"] = json!({"schema":RECOMMENDATION_SCHEMA,"summary_fingerprint":crate::tidal::summary_fingerprint(&summary),"checked_at":chrono::Utc::now().timestamp(),"optional_status":if optional_complete {"complete"} else {"retry"},"optional_retry_after":retry_after});
    }
    Ok(())
}

pub async fn release(db: &TursoDb, id: &str, market: &str, force: bool) -> Result<Value, String> {
    release_with_cancel(db,id,market,force,Arc::new(AtomicBool::new(false))).await
}

pub(crate) async fn release_with_cancel(db: &TursoDb, id: &str, market: &str, force: bool, cancel: Arc<AtomicBool>) -> Result<Value, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err("Select a release with a valid online ID".into());
    }
    let gate = release_gate(format!("{}:{market}:{id}", db.path.display()));
    let _guard = gate.lock().await;
    let key = format!("tag-review:{market}:{id}");
    let cached = db.get_preference(&key).await?;
    let mut value = match cached {
        Some(value) => value,
        None => {
            db.get_detail(&json!({"release_id":id, "market":market}))
                .await?
        }
    };
    let summary = db.get_preference(&format!("subscriber-summary:{market}:{id}")).await?;
    let force = force || value["recommendation_snapshot"]["schema"].as_u64().is_some_and(|schema|schema != u64::from(RECOMMENDATION_SCHEMA)) || summary.as_ref().is_some_and(|summary| {
        value["recommendation_snapshot"]["summary_fingerprint"].as_str().is_some_and(|prior| prior != crate::tidal::summary_fingerprint(summary))
    });
    if !force && value["subscriber_discovery_checked_at"].as_i64().is_some_and(|at|(0..30*86400).contains(&(chrono::Utc::now().timestamp()-at))) && value["tracks_loaded"] == true && value["track_metadata_source"] == "subscriber"
        && value["track_metadata_checked_at"].as_i64().is_some_and(|at|(0..30*86400).contains(&(chrono::Utc::now().timestamp()-at)))
        && value["tracks"].as_array().is_some_and(|tracks| !tracks.is_empty() && tracks.iter().all(|track|track["credits_complete"] == true))
        && (value["recommendation_snapshot"]["schema"] != RECOMMENDATION_SCHEMA || value["recommendation_snapshot"]["optional_status"] == "complete") {
        if value["recommendation_snapshot"]["schema"] != RECOMMENDATION_SCHEMA {
            record_recommendation_snapshot(db,&mut value,market,summary.as_ref(),true).await?;
            return publish_release(db,id,market,value).await;
        }
        return Ok(value);
    }
    let old_tracks: Vec<crate::tidal::TidalTrack> = serde_json::from_value(value["tracks"].clone()).unwrap_or_default();
    let mut reuse_tracks = !force && summary.as_ref().is_some_and(|summary|recommendation_tracks_current(&value,&crate::tidal::summary_fingerprint(summary)));
    let mut tracks = if reuse_tracks { old_tracks.clone() } else {
        let http=crate::network::client(20)?;
        match crate::subscriber_metadata::load(db,&http,id,market,cancel.clone(),force).await {
            Ok(raw)=>crate::subscriber_metadata::tracks(&raw)?,
            Err(error) if cancel.load(Ordering::Relaxed) => return Err(error),
            Err(error) if value["tracks_loaded"] == true && !force => {
                value["metadata_note"]=json!(format!("Saved metadata retained: {error}. Reconnect your account and retry if needed."));
                return Ok(value);
            }
            Err(error)=>return Err(format!("Subscriber metadata unavailable: {error}. Connect your account and retry.")),
        }
    };
    crate::subscriber_metadata::supplement(&mut tracks,&old_tracks);
    let summary_loaded = value["title"] == "Release not found";
    if summary_loaded {
        let mut client = crate::tidal::TidalClient::from_db(db).await?.with_cancel(cancel.clone());
        let raw = client.albums(&[id.to_owned()],market,force).await?;
        let data = raw["data"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"].as_str() == Some(id)))
            .ok_or("This release is unavailable in the selected market; saved data retained")?;
        let a = &data["attributes"];
        let artists = raw["included"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v["type"] == "artists")
            .filter_map(|v| v["attributes"]["name"].as_str())
            .next().unwrap_or_default().to_owned();
        value = json!({"id":id,"artist":artists,"title":crate::tidal::format_title(a["title"].as_str().unwrap_or(""),a["version"].as_str()),"date":a["releaseDate"].as_str().unwrap_or(""),"original_release_date":a["originalReleaseDate"],"type":a["albumType"].as_str().unwrap_or("album"),"official":a["official"],"secondary_types":a["secondaryTypes"].as_array().cloned().unwrap_or_default(),"available":a["availability"].as_array().map(|v|v.iter().any(|x|x=="STREAM"||x=="DJ")),"label":a["recordLabel"].as_str().or(a["recordLabel"]["name"].as_str()),"copyright":a["copyright"].as_str().or(a["copyright"]["text"].as_str()),"upc":a["barcodeId"].as_str().or(a["upc"].as_str()),"quality":a["mediaTags"].as_array().map(|values|values.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(),"audio_modes":a["audioModes"].as_array().cloned().unwrap_or_default(),"media_metadata":a["mediaMetadata"],"remote_metadata":raw});
        let raw = value["remote_metadata"].clone();
        apply_release_discovery(&mut value, &raw, id);
    }
    if value["discovery_checked_at"].as_i64().is_none_or(|at| chrono::Utc::now().timestamp() - at >= 30 * 86_400) || (force && !summary_loaded) {
        if let Ok(mut client)=crate::tidal::TidalClient::from_db(db).await {
            client = client.with_cancel(cancel.clone());
            fill_release_discovery(&mut client, &mut value, id, market, force).await;
        }
    }
    let mut client=crate::tidal::TidalClient::from_db(db).await?.with_cancel(cancel.clone());
    let optional=client.discovery(&[id.to_owned()],market,force).await?;
    if let Some(fields)=optional.get(id) { crate::tidal::merge_discovery(&mut value,fields); }
    let current_summary=db.get_preference(&format!("subscriber-summary:{market}:{id}")).await?;
    if summary_change_requires_track_refresh(reuse_tracks,summary.as_ref(),current_summary.as_ref()) {
        // A standalone metadata check can discover a changed edition while
        // refreshing its expired summary. Never stamp that new summary onto an
        // older credited track list; validate its current audio membership first.
        let http=crate::network::client(20)?;
        let raw=crate::subscriber_metadata::load(db,&http,id,market,cancel.clone(),true).await?;
        tracks=crate::subscriber_metadata::tracks(&raw)?;
        crate::subscriber_metadata::supplement(&mut tracks,&old_tracks);
        reuse_tracks=false;
    }
    value["track_metadata_source"]=json!("subscriber");
    if !reuse_tracks { value["track_metadata_checked_at"]=json!(chrono::Utc::now().timestamp()); }
    value["metadata_note"]=if optional.contains_key(id) {Value::Null} else {json!("Track details saved; optional catalogue fields unavailable, saved values retained.")};
    value["tracks"] = serde_json::to_value(&tracks).map_err(|e| e.to_string())?;
    value["tracks_loaded"] = json!(true);
    value["track_count"] = json!(tracks.len());
    // Pin the exact summary paired with this track list. A concurrent catalogue
    // write after this point will produce a different fingerprint on the next check.
    record_recommendation_snapshot(db,&mut value,market,current_summary.as_ref(),optional.contains_key(id)).await?;
    publish_release(db, id, market, value).await
}

async fn fill_release_discovery(client: &mut crate::tidal::TidalClient, value: &mut Value, id: &str, market: &str, force: bool) {
    // Optional metadata never invalidates a cached release or fails a linking job.
    let now = chrono::Utc::now().timestamp();
    value["discovery_checked_at"] = json!(now - 29 * 86_400); // Retry failures in one day.
    if let Ok(raw) = client.albums(&[id.to_owned()],market,force).await {
        apply_release_discovery(value, &raw, id);
    }
}

fn apply_release_discovery(value: &mut Value, raw: &Value, id: &str) {
    let data = raw["data"].as_array().and_then(|a| a.first()).unwrap_or(&raw["data"]);
    if data["id"].as_str() != Some(id) { return; }
    let included = raw["included"].as_array().cloned().unwrap_or_default();
    if data["relationships"]["genres"].is_object() { value["genres"] = json!(crate::tidal::related_genres(data, &included)); }
    if data["relationships"]["replacement"].is_object() { value["replacement_id"] = json!(crate::tidal::replacement_id(data)); }
    value["discovery_checked_at"] = json!(chrono::Utc::now().timestamp());
    if let Some(available) = crate::availability::available(data) {
        value["available"] = json!(available);
        value["availability_checked_at"] = value["discovery_checked_at"].clone();
    }
    if let Some(artists) = data["relationships"]["artists"]["data"].as_array() {
        value["album_artist_ids"] = json!(artists.iter().filter_map(|a| a["id"].as_str()).collect::<Vec<_>>());
        value["album_artists"] = json!(artists.iter().filter_map(|a| included.iter().find(|v| v["type"] == "artists" && v["id"] == a["id"]).and_then(|v| v["attributes"]["name"].as_str())).collect::<Vec<_>>());
        value["tag_checked_at"] = value["discovery_checked_at"].clone();
    }
}

async fn publish_release(
    db: &TursoDb,
    id: &str,
    market: &str,
    value: Value,
) -> Result<Value, String> {
    db.ensure_catalogue_release_index().await?;
    // Publish details to every catalogue reference and existing queue entry.
    let conn = db.connect()?;
    conn.execute("BEGIN IMMEDIATE", ()).await.map_err(|e|e.to_string())?;
    let result = async {
        conn.execute("INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)", (format!("tag-review:{market}:{id}"),value.to_string())).await.map_err(|e|e.to_string())?;
        if let (Some(available), Some(checked_at)) = (value["available"].as_bool(),value["availability_checked_at"].as_i64()) {
            conn.execute("INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)", (format!("release-live:{market}:{id}"),json!({"available":available,"checked_at":checked_at}).to_string())).await.map_err(|e|e.to_string())?;
        }
        let mut rows = conn
            .query(
                "SELECT c.artist_id,c.payload FROM catalogue c JOIN catalogue_release_index i ON i.artist_id=c.artist_id AND i.market=c.market WHERE i.market=? AND i.release_id=?",
                (market,id),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut updates = vec![];
        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            let artist: String = row.get(0).map_err(|e| e.to_string())?;
            let raw: String = row.get(1).map_err(|e| e.to_string())?;
            let mut cat: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            let mut changed = false;
            if let Some(releases) = cat["releases"].as_array_mut() {
                for r in releases {
                    if r["id"].as_str() == Some(id) {
                        *r = value.clone();
                        changed = true;
                    }
                }
            }
            if changed {
                updates.push((artist, cat.to_string()));
            }
        }
        drop(rows);
        for (artist, data) in updates {
            conn.execute(
                "UPDATE catalogue SET payload=? WHERE artist_id=? AND market=?",
                (data.as_str(), artist.as_str(), market),
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        let mut q = conn
            .query("SELECT payload FROM queue WHERE id=?", (id,))
            .await
            .map_err(|e| e.to_string())?;
        if let Some(row) = q.next().await.map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let mut queued: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            queued["tracks"] = value["tracks"].clone();
            queued["tracks_loaded"] = json!(true);
            drop(q);
            conn.execute(
                "UPDATE queue SET payload=? WHERE id=?",
                (queued.to_string(), id),
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        conn.execute("COMMIT", ()).await.map_err(|e|e.to_string())?;
        Ok::<_,String>(())
    }.await;
    if result.is_err() { let _ = conn.execute("ROLLBACK", ()).await; }
    result?;
    db.bump_revision();
    Ok(value)
}

pub async fn execute(
    db: &TursoDb,
    state: &Arc<Backend>,
    kind: &str,
    args: &Value,
    cancel: Arc<AtomicBool>,
) -> Result<Value, String> {
    let settings = db.get_settings().await?;
    let market = settings["general"]["market"].as_str().unwrap_or("GB");
    if kind == "release_artists" { return crate::release_artists::refresh(db,state,args,cancel).await; }
    if kind == "cached_releases" {
        state.progress_for(kind, "Rechecking saved releases · ownership and recommendations · local cache only; music files unchanged");
        db.invalidate_missing_rows();
        let page = db.get_missing_rows(market, Some("All missing releases"), None, None, None, None, None, None, 0, 0).await?;
        return Ok(json!({"message":format!("Cached release check complete · {} missing, incomplete or queued releases · music files unchanged", page.total), "total":page.total}));
    }
    if kind == "release_details" {
        let id = args["id"].as_str().ok_or("Select a release")?;
        state.progress_for(kind,&format!("Refreshing release {id} · subscriber track details and credits"));
        let value=release_with_cancel(db, id, market, args["force"].as_bool().unwrap_or(false), cancel.clone()).await?;
        state.progress_for(kind,&format!("Release {id} · {} tracks cached · {}",value["track_count"],value["metadata_note"].as_str().unwrap_or("complete")));
        return Ok(json!({"release_id":id}));
    }
    if kind == "connections" {
        let mut metrics = json!({});
        if let Ok(mut client) = crate::tidal::TidalClient::from_db(db).await {
            let start = std::time::Instant::now();
            let result = client.search_artists("Cassie", market).await;
            let connected_on = if result.is_ok() {
                match db.get_preference("catalogue_connected_at").await? {
                    Some(saved) if saved.as_str().is_some_and(|date| !date.is_empty()) => saved,
                    _ => {
                        let first_seen = json!(chrono::Local::now().format("%Y-%m-%d").to_string());
                        db.set_preference("catalogue_connected_at", &first_seen)
                            .await?;
                        first_seen
                    }
                }
            } else {
                Value::Null
            };
            metrics["catalogue"] = json!({
                "ok": result.is_ok(),
                "message": result.err().unwrap_or_default(),
                "latency_ms": start.elapsed().as_millis(),
                "connected_on": connected_on
            });
        } else {
            metrics["catalogue"] = json!({"ok":false,"message":"Connect your streaming account"});
        }
        let http = crate::network::client(20)?;
        let token = crate::stream_download::get_valid_token(db, &http).await;
        let mut user_detail = String::new();
        let result = match token {
            Ok(token) => match crate::network::get(http
                .get("https://api.tidal.com/v1/sessions")
                .bearer_auth(token)
                , std::time::Duration::from_millis(350), 3, Some(cancel.as_ref()))
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(sess) = resp.json::<Value>().await {
                        if let Some(uid) = sess.get("userId").and_then(|v| v.as_i64()) {
                            user_detail = format!("Account ID: {}", uid);
                        }
                    }
                    Ok(())
                }
                Ok(resp) => Err(format!("HTTP {}", resp.status())),
                Err(e) => Err(e.to_string()),
            },
            Err(e) => Err(e),
        };
        if user_detail.is_empty() {
            if let Some(tok) = crate::stream_download::load_saved_token(db).await {
                if let Some(ref uid) = tok.user_id {
                    user_detail = format!("Account ID: {}", uid);
                }
            }
        }
        let connected_on: String = if result.is_ok() {
            if let Ok(Some(saved_date)) = db.get_preference("account_connected_at").await {
                saved_date.as_str().unwrap_or("").to_string()
            } else {
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                let _ = db
                    .set_preference("account_connected_at", &json!(today))
                    .await;
                today
            }
        } else {
            String::new()
        };
        metrics["download"] = json!({
            "ok": result.is_ok(),
            "message": result.err().unwrap_or(user_detail),
            "connected_on": connected_on,
        });
        let diagnostics = json!({"metrics":metrics,"checked_at":chrono::Utc::now().to_rfc3339()});
        db.set_preference("connection-diagnostics", &diagnostics)
            .await?;
        return Ok(diagnostics);
    }
    if kind == "favourites" {
        let http = crate::network::client(20)?;
        let token = crate::stream_download::get_valid_token(db, &http).await?;
        let saved = crate::stream_download::load_saved_token(db)
            .await
            .ok_or("Connect your account")?;
        let user = saved
            .user_id
            .ok_or("Reconnect your account to identify favourites")?;
        let mut artists = vec![];
        let mut offset = 0;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled; previous favourites retained".into());
            }
            let response: Value = crate::network::get(http
                .get(format!(
                    "https://api.tidal.com/v1/users/{user}/favorites/artists"
                ))
                .query(&[
                    ("countryCode", market),
                    ("limit", "100"),
                    ("offset", &offset.to_string()),
                ])
                .bearer_auth(&token)
                , std::time::Duration::from_millis(350), 3, Some(cancel.as_ref()))
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let items = response["items"]
                .as_array()
                .ok_or("Invalid favourites response")?;
            for row in items {
                let item = &row["item"];
                let id = item["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| item["id"].to_string());
                if id != "null" {
                    artists.push(crate::tidal::TidalArtist {
                        id,
                        name: item["name"].as_str().unwrap_or("").into(),
                    });
                }
            }
            offset += items.len();
            if items.is_empty()
                || offset
                    >= response["totalNumberOfItems"]
                        .as_u64()
                        .unwrap_or(offset as u64) as usize
            {
                break;
            }
        }
        crate::account::AccountClient::new()
            .save_favourites_to_db(db, &user, &artists)
            .await?;
        db.bump_revision();
        return Ok(json!({"artists":artists.len()}));
    }
    let root = args["root"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Choose a registered library first")?;
    let indexed = files(db, root).await?;
    let ids: HashSet<String> = args["ids"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if kind == "review_consolidation" {
        if ids.is_empty() {
            return Err("Select the duplicate releases to review".into());
        }
        if args["scope"] == "remote" {
            return Err("Download replacements first, then review local duplicates".into());
        }
        let clusters = crate::duplicates::find_duplicate_clusters(&indexed);
        let mut rows = vec![];
        let mut count = 0;
        for cluster in clusters {
            for source in &cluster.redundant {
                let id = format!("{}::{}", cluster.cluster_id, source.folder);
                if !ids.contains(&id)
                    && !ids.contains(&source.folder)
                    && !ids.contains(&cluster.cluster_id)
                {
                    continue;
                }
                let mut conflicts = vec![];
                for track in &source.tracks {
                    if let Some(keep) = cluster
                        .master
                        .tracks
                        .iter()
                        .find(|t| crate::duplicates::track_matches(track, t))
                    {
                        if track.bpm != keep.bpm || track.key != keep.key {
                            conflicts.push(json!({"track":track.title,"removed_bpm":track.bpm,"retained_bpm":keep.bpm,"removed_key":track.key,"retained_key":keep.key}));
                        }
                    }
                }
                count += source.tracks.len();
                rows.push(json!({"id":id,"artist":source.artist,"release":source.title,"path":source.folder,"target":cluster.master.folder,"changes":format!("Move {} duplicate files to Trash",source.tracks.len()),"evidence":"Matching recording, duration, mix title and performer credits","reviewed_dj_conflicts":conflicts,"source_tracks":source.tracks,"keeper_tracks":cluster.master.tracks}));
            }
        }
        if rows.is_empty() {
            return Err("No current duplicate recommendations match this selection".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        state.previews.lock().unwrap().insert(id.clone(),json!({"id":id,"operation":"consolidate","root":root,"rows":rows,"count":count,"scope":"local"}));
        return Ok(json!({"preview_id":id,"operation":"consolidate","root":root}));
    }
    if kind == "consolidate" {
        if args["confirmed"] != true {
            return Err("Review and confirm duplicate removal first".into());
        }
        let id = args["preview_id"].as_str().ok_or("Review required")?;
        let preview = state
            .previews
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("Review expired")?;
        if preview["root"] != root || preview["operation"] != "consolidate" {
            return Err("Invalid duplicate review".into());
        }
        let library = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        let mut sources = vec![];
        for row in preview["rows"].as_array().ok_or("Invalid review")? {
            if !ids.contains(row["id"].as_str().unwrap_or("")) {
                continue;
            }
            for field in ["source_tracks", "keeper_tracks"] {
                for track in row[field].as_array().ok_or("Invalid file manifest")? {
                    let path = track["path"].as_str().ok_or("Missing file path")?;
                    if !std::fs::canonicalize(path)
                        .map_err(|e| e.to_string())?
                        .starts_with(&library)
                    {
                        return Err("Reviewed file is outside this library".into());
                    }
                    let stat = std::fs::metadata(path).map_err(|e| e.to_string())?;
                    let mtime = stat
                        .modified()
                        .map_err(|e| e.to_string())?
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| e.to_string())?
                        .as_nanos() as i64;
                    if track["size"] != json!(stat.len()) || track["mtime"] != json!(mtime) {
                        return Err(
                            "A source or retained file changed; review duplicates again".into()
                        );
                    }
                    if field == "source_tracks" {
                        sources.push(path.to_owned());
                    }
                }
            }
        }
        if sources.is_empty() {
            return Err("Select reviewed duplicate releases".into());
        }
        sources.sort();
        sources.dedup();
        let mut completed = 0;
        let total_sources=sources.len();
        for path in sources {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            crate::duplicates::trash_file_or_directory(&path).await?;
            db.remove_local_file(&path).await?;
            completed += 1;
            state.progress_for(
                kind,
                &format!(
                    "Moved duplicate to Trash · {completed}/{total_sources} files · {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
            );
        }
        state.previews.lock().unwrap().remove(id);
        db.set_preference(&format!("desktop-local:{root}"), &json!([]))
            .await?;
        return Ok(json!({"completed":completed}));
    }
    if kind == "deep_review" {
        if ids.is_empty() {
            return Err("Select the tracks to review first".into());
        }
        let selected: Vec<_> = indexed.iter().filter(|f| ids.contains(&f.path)).collect();
        let mut albums = HashSet::new();
        let query = args["query"].as_str().unwrap_or("").trim();
        if let Ok(url) = url::Url::parse(query) {
            let parts = url
                .path_segments()
                .map(|p| p.collect::<Vec<_>>())
                .unwrap_or_default();
            if let Some(index) = parts.iter().position(|p| *p == "album") {
                if let Some(id) = parts
                    .get(index + 1)
                    .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
                {
                    albums.insert(id.to_string());
                }
            }
            if albums.is_empty() {
                return Err("Enter a release URL or a search phrase".into());
            }
        } else {
            let mut client = crate::tidal::TidalClient::from_db(db).await?;
            let mut searched = HashSet::new();
            for file in &selected {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let tags = workflows::extract_tags_map(&file.metadata);
                let phrase = if !query.is_empty() {
                    query.to_string()
                } else {
                    format!(
                        "{} {}",
                        tags.get("albumartist")
                            .or(tags.get("artist"))
                            .map(String::as_str)
                            .unwrap_or(""),
                        tags.get("title").map(String::as_str).unwrap_or("")
                    )
                };
                if !searched.insert(phrase.clone()) {
                    continue;
                }
                state.progress_for(kind, &format!("Searching selected recordings · {phrase}"));
                let cache_key = format!("recording-search:{market}:{phrase}");
                let matches = if let Some(cached) = db.get_preference(&cache_key).await? {
                    serde_json::from_value::<Vec<crate::tidal::TidalTrackSearchResult>>(cached)
                        .map_err(|e| e.to_string())?
                } else {
                    let found = client.search_tracks(&phrase, market).await?;
                    db.set_preference(&cache_key, &json!(found)).await?;
                    found
                };
                for track in matches {
                    albums.extend(track.album_ids);
                }
            }
        }
        let mut raw = vec![];
        for album in albums {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let value = release(db, &album, market, false).await?;
            let rel: crate::tidal::TidalRelease =
                serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            let mut matched = vec![];
            for file in &selected {
                let tags = workflows::extract_tags_map(&file.metadata);
                let candidates: Vec<_> = rel
                    .tracks
                    .iter()
                    .filter(|track| {
                        crate::release_matching::recording_matches(
                            tags.get("title").map(String::as_str).unwrap_or(""),
                            file.metadata
                                .as_ref()
                                .and_then(|m| m["duration"].as_f64())
                                .unwrap_or(0.),
                            tags.get("isrc").map(String::as_str),
                            &track.title,
                            track.duration,
                            track.isrc.as_deref(),
                            true,
                        )
                    })
                    .collect();
                if candidates.len() == 1 {
                    let track = candidates[0];
                    matched.push(json!({"path":file.path,"size":file.size,"mtime":file.mtime,"album_id":album,"track_id":track.id,"artist":tags.get("albumartist").or(tags.get("artist")),"release":rel.title,"title":tags.get("title"),"evidence":format!("Local disc {} · track {} → online disc {} · track {}/{}",tags.get("discnumber").map(String::as_str).unwrap_or("?"),tags.get("tracknumber").map(String::as_str).unwrap_or("?"),track.disc_number,track.track_number,rel.track_count)}));
                }
            }
            if !matched.is_empty() {
                raw.push(json!({"release":value,"tracks_matched":matched}));
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        state
            .previews
            .lock()
            .unwrap()
            .insert(id.clone(), json!({"id":id,"root":root,"raw":raw}));
        return Ok(json!({"preview_id":id,"operation":"deep_review","root":root}));
    }
    if kind == "deep_preview" {
        let old = args["preview_id"].as_str().ok_or("Select a review")?;
        let review = state
            .previews
            .lock()
            .unwrap()
            .get(old)
            .cloned()
            .ok_or("Review expired")?;
        if review["root"] != root {
            return Err("Review belongs to another library".into());
        }
        let index = args["index"].as_u64().ok_or("Select an edition")? as usize;
        let candidate = review["raw"]
            .as_array()
            .and_then(|a| a.get(index))
            .ok_or("Invalid edition")?;
        let rows: Vec<_> = candidate["tracks_matched"]
            .as_array()
            .ok_or("No recording matches")?
            .iter()
            .map(|r| {
                let mut row = r.clone();
                row["id"] = row["path"].clone();
                row["changes"] = json!(format!(
                    "Link to release {} / recording {} · local files unchanged",
                    r["album_id"].as_str().unwrap_or(""),
                    r["track_id"].as_str().unwrap_or("")
                ));
                row
            })
            .collect();
        let id = uuid::Uuid::new_v4().to_string();
        state.previews.lock().unwrap().insert(
            id.clone(),
            json!({"id":id,"root":root,"operation":"deep_apply","rows":rows}),
        );
        return Ok(json!({"preview_id":id,"operation":"deep_apply","root":root}));
    }
    if kind == "deep_apply" {
        if args["confirmed"] != true {
            return Err("Review the links first".into());
        }
        let id = args["preview_id"].as_str().ok_or("Select a preview")?;
        let review = state
            .previews
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("Preview expired")?;
        if review["root"] != root || review["operation"] != "deep_apply" {
            return Err("Invalid link preview".into());
        }
        let mut linked = 0;
        for row in review["rows"].as_array().ok_or("Invalid link rows")? {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let file = indexed
                .iter()
                .find(|f| Some(f.path.as_str()) == row["path"].as_str())
                .ok_or("File no longer indexed")?;
            if row["size"] != json!(file.size) || row["mtime"] != json!(file.mtime) {
                return Err("Local metadata changed; review again".into());
            }
            let mut link = row.clone();
            link["market"] = json!(market);
            db.choose_track_link(&link).await?;
            linked += 1;
        }
        state.previews.lock().unwrap().remove(id);
        return Ok(json!({"linked":linked}));
    }
    if kind == "queue_mqa" {
        if ids.is_empty() {
            return Err("Select audited tracks to queue".into());
        }
        let mut selection = std::collections::HashMap::<String, Option<Vec<String>>>::new();
        for file in indexed.iter().filter(|f| ids.contains(&f.path)) {
            let audit = db
                .get_preference(&format!("mqa-audit:{}", file.path))
                .await?
                .ok_or("Audit the selected tracks first")?;
            if audit["result"]["detected"] != true
                || audit["size"] != json!(file.size)
                || audit["mtime"] != json!(file.mtime)
            {
                continue;
            }
            let detail = db
                .get_detail(&json!({"path":file.path,"market":market}))
                .await?;
            if let (Some(album), Some(track)) = (
                detail["linked_ids"]["album_id"].as_str(),
                detail["linked_ids"]["track_id"].as_str(),
            ) {
                release(db, album, market, false).await?;
                selection
                    .entry(album.into())
                    .or_insert_with(|| Some(vec![]))
                    .as_mut()
                    .unwrap()
                    .push(track.into());
            }
        }
        if selection.is_empty() {
            return Err(
                "No selected MQA tracks have verified recording links. Link them first.".into(),
            );
        }
        db.queue_add(&selection).await?;
        return Ok(json!({"releases":selection.len()}));
    }
    if kind == "queue_replacements" {
        if ids.is_empty() {
            return Err("Select replacement releases first".into());
        }
        let plans = db
            .get_preference(&format!("desktop-online:{root}"))
            .await?
            .unwrap_or(json!([]));
        let mut selection = std::collections::HashMap::new();
        for row in plans
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["id"].as_str().is_some_and(|id| ids.contains(id)))
        {
            if let Some(id) = row["online_id"].as_str() {
                selection.insert(id.to_string(), None);
            }
        }
        if selection.is_empty() {
            return Err("No current replacement plans selected".into());
        }
        db.queue_add(&selection).await?;
        return Ok(json!({"releases":selection.len()}));
    }
    if kind == "optimizations" || kind == "local_duplicates" || kind == "check_replacements" {
        if kind == "local_duplicates" || args["scope"] != "remote" && kind != "check_replacements" {
            state.progress_for(
                kind,
                &format!(
                    "Checking {} indexed files for absorbable releases",
                    indexed.len()
                ),
            );
            let files=indexed.clone();
            let cancelled=cancel.clone();
            let backend=state.clone();
            let task=kind.to_owned();
            let clusters=tokio::task::spawn_blocking(move ||crate::duplicates::find_duplicate_clusters_with_progress(&files,&cancelled,|done,total|backend.progress_for(&task,&format!("Comparing local releases · {done}/{total} artists"))))
                .await.map_err(|e|e.to_string())?;
            if cancel.load(Ordering::Relaxed) {return Ok(json!({"cancelled":true}));}
            let rows = crate::duplicates::clusters_to_group_rows(&clusters);
            db.set_preference(&format!("desktop-local:{root}"), &json!(rows))
                .await?;
            db.set_preference(
                &format!("desktop-local-manifest:{root}"),
                &json!(crate::duplicates::manifest_fingerprint(&indexed)),
            )
            .await?;
            state.progress_for(
                kind,
                &format!("Found {} absorbable release groups", rows.len()),
            );
            return Ok(json!({"opportunities":rows.len()}));
        }
        state.progress_for(kind, "Comparing cached online releases with indexed albums");
        let locals = crate::duplicates::parse_local_releases(&indexed);
        let conn = db.connect()?;
        let mut query = conn
            .query("SELECT payload FROM catalogue WHERE market=?", (market,))
            .await
            .map_err(|e| e.to_string())?;
        let mut remote = vec![];
        while let Some(row) = query.next().await.map_err(|e| e.to_string())? {
            let text: String = row.get(0).map_err(|e| e.to_string())?;
            let cat: crate::tidal::TidalCatalogue =
                serde_json::from_str(&text).map_err(|e| e.to_string())?;
            remote.extend(cat.releases);
        }
        drop(query);
        let mut rows = vec![];
        let mut seen = HashSet::new();
        let remote_total = remote.len();
        for (position, mut target) in remote.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if position % 100 == 0 {
                state.progress_for(
                    kind,
                    &format!(
                        "Checking cached releases · {}/{} · {} — {} · release ID {}",
                        position + 1,
                        remote_total, target.artist, target.title, target.id
                    ),
                );
            }
            if target.available != Some(true)
                || target.date.as_str() > chrono::Utc::now().format("%Y-%m-%d").to_string().as_str()
            {
                continue;
            }
            let relevant: Vec<_> = locals
                .iter()
                .filter(|l| {
                    crate::matching::name_key(&l.artist)
                        == crate::matching::name_key(&target.artist)
                        && l.tracks.len() < target.track_count
                })
                .collect();
            if relevant.is_empty() {
                continue;
            }
            if !target.tracks_loaded && kind == "check_replacements" {
                target = serde_json::from_value(release(db, &target.id, market, false).await?)
                    .map_err(|e| e.to_string())?;
            }
            if !target.tracks_loaded {
                continue;
            }
            for local in relevant {
                let mut claimed = HashSet::new();
                let contained = local.tracks.iter().all(|track| {
                    target.tracks.iter().any(|online| {
                        let exact_isrc = track
                            .isrc
                            .as_ref()
                            .filter(|s| !s.is_empty())
                            .zip(online.isrc.as_ref())
                            .is_some_and(|(a, b)| {
                                a.replace('-', "").eq_ignore_ascii_case(&b.replace('-', ""))
                            });
                        exact_isrc
                            && !claimed.contains(&online.id)
                            && crate::release_matching::recording_matches(
                                &track.title,
                                track.duration,
                                track.isrc.as_deref(),
                                &online.title,
                                online.duration,
                                online.isrc.as_deref(),
                                true,
                            )
                            && claimed.insert(online.id.clone())
                    })
                });
                let id = format!("{}::{}", target.id, local.folder);
                if contained && seen.insert(id.clone()) {
                    rows.push(json!({"id":id,"artist":local.artist,"release":local.title,"title":format!("{} tracks",local.tracks.len()),"tracks":local.tracks.len(),"duplicates":local.tracks.len(),"gained":target.tracks.len()-local.tracks.len(),"path":local.folder,"status":"Larger online release","evidence":format!("Every local recording has matching ISRC, mix title and duration; {} additional audio tracks",target.tracks.len()-local.tracks.len()),"target":target.title,"online_id":target.id,"affected":true,"changes":"Queue complete release; retain originals until downloaded and reviewed"}));
                }
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled; previous opportunities retained".into());
        }
        db.set_preference(&format!("desktop-online:{root}"), &json!(rows))
            .await?;
        return Ok(json!({"opportunities":rows.len()}));
    }
    if kind == "artwork" {
        let http = crate::network::client(25)?;
        let mut token = None;
        let mut output = vec![];
        let cache = db
            .path
            .parent()
            .ok_or("Invalid cache directory")?
            .join("artwork-cache");
        std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        let artwork_files:Vec<_>=indexed.iter().filter(|f|ids.is_empty() || ids.contains(&f.path)).collect();
        let total_artwork=artwork_files.len();
        for (position,file) in artwork_files.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {break;}
            state.progress_for(kind,&format!("Checking artwork · {position}/{total_artwork} files · {}",file.path));
            let tags = workflows::extract_tags_map(&file.metadata);
            let detail = db
                .get_detail(&json!({"path":file.path,"market":market}))
                .await?;
            let Some(album) = detail["linked_ids"]["album_id"]
                .as_str()
                .or_else(|| tags.get("tidal_album_id").map(String::as_str))
            else {
                continue;
            };
            if !album.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let target = cache.join(format!("{album}-1280.jpg"));
            if !target.exists() {
                if token.is_none() {
                    token = Some(crate::stream_download::get_valid_token(db, &http).await?);
                }
                let info=crate::subscriber_metadata::album_info(db,&http,album,token.as_ref().unwrap(),market).await?;
                let Some(cover) = info.cover else { continue };
                let Some(bytes) =
                    crate::stream_download::fetch_cover_art(&http, &cover, 1280).await
                else {
                    continue;
                };
                let pic = lofty::picture::Picture::from_reader(&mut std::io::Cursor::new(&bytes))
                    .map_err(|e| e.to_string())?;
                let dimensions = lofty::picture::PictureInformation::from_picture(&pic)
                    .map_err(|e| e.to_string())?;
                if dimensions.width != 1280 || dimensions.height != 1280 {
                    continue;
                }
                std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
            }
            use lofty::file::TaggedFileExt;
            if let Ok(audio) = lofty::probe::Probe::open(&file.path).and_then(|p| p.read()) {
                if audio
                    .tags()
                    .iter()
                    .flat_map(|t| t.pictures())
                    .filter(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
                    .any(|p| {
                        lofty::picture::PictureInformation::from_picture(p)
                            .is_ok_and(|info| info.width == 1280 && info.height == 1280)
                    })
                {
                    continue;
                }
            }
            output.push(json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),"release":tags.get("album"),"title":tags.get("title"),"affected":true,"status":"Artwork available","changes":"Embed verified 1280 × 1280 front cover","size":file.size,"mtime":file.mtime,"item":{"path":file.path,"artwork":target,"tags":{}}}));
            state.progress_for(
                kind,
                &format!(
                    "Artwork ready · {}",
                    tags.get("title").unwrap_or(&file.path)
                ),
            );
        }
        let id = uuid::Uuid::new_v4().to_string();
        state.previews.lock().unwrap().insert(id.clone(),json!({"id":id,"created":chrono::Utc::now().timestamp_millis(),"operation":"artwork","root":root,"rows":output,"count":output.len()}));
        return Ok(json!({"preview_id":id,"operation":"artwork","root":root}));
    }
    if kind == "match_artists" {
        let selected: HashSet<String> = args["artists"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let page = db
            .get_artist_rows(Some(root), None, None, None, None, 0, usize::MAX)
            .await?;
        let pending: Vec<_> = page.rows.into_iter().filter(|artist| {
            let name = artist["artist"].as_str().unwrap_or("");
            !name.is_empty() && if selected.is_empty() { artist["resolved"] != true } else { selected.contains(name) }
        }).collect();
        if pending.is_empty() { return Ok(json!({"checked":0})); }
        let mut client = crate::tidal::TidalClient::from_db(db).await?;
        let conn = db.connect()?;
        let mut favourites = Vec::<crate::tidal::TidalArtist>::new();
        let mut saved = conn.query("SELECT payload FROM favourite_artists", ()).await.map_err(|e|e.to_string())?;
        while let Some(row) = saved.next().await.map_err(|e|e.to_string())? {
            if let Ok(raw) = row.get::<String>(0) {
                if let Ok(items) = serde_json::from_str::<Vec<crate::tidal::TidalArtist>>(&raw) { favourites.extend(items); }
            }
        }
        drop(saved);
        let mut checked = 0;
        let total_artists=pending.len();
        for artist in pending {
            let name = artist["artist"].as_str().unwrap_or("");
            if name.is_empty()
                || (!selected.is_empty() && !selected.contains(name))
                || (selected.is_empty()
                    && artist["resolved"] == true)
            {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            state.progress_for(kind, &format!("Finding artist matches · {checked}/{total_artists} artists · {name}"));
            let titles: Vec<String> = indexed
                .iter()
                .filter_map(|f| {
                    let tags = workflows::extract_tags_map(&f.metadata);
                    if tags
                        .get("albumartist")
                        .or(tags.get("artist"))
                        .map(String::as_str)
                        == Some(name)
                    {
                        tags.get("album").cloned()
                    } else {
                        None
                    }
                })
                .collect();
            let search_key = format!("artist-search:{market}:{}", crate::matching::name_key(name));
            let favourite_matches: Vec<_> = favourites.iter().filter(|f| crate::matching::name_key(&f.name) == crate::matching::name_key(name)).cloned().collect();
            let candidates: Vec<crate::tidal::TidalArtist> =
                if !favourite_matches.is_empty() {
                    favourite_matches
                } else if let Some(c) = db.get_preference(&search_key).await? {
                    serde_json::from_value(c).unwrap_or_default()
                } else {
                    let c = client.search_artists(name, market).await?;
                    db.set_preference(&search_key, &json!(c)).await?;
                    c
                };
            let mut scores = vec![];
            let mut accepted = vec![];
            for candidate in candidates {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if crate::matching::name_key(name) != crate::matching::name_key(&candidate.name) {
                    scores.push(json!({"id":candidate.id,"name":candidate.name,"score":0,"evidence":"Artist name differs; manual review required"}));
                    continue;
                }
                let cache_key = format!("artist-evidence:{market}:{}", candidate.id);
                let cat: crate::tidal::TidalCatalogue =
                    if let Some(c) = db.get_preference(&cache_key).await? {
                        serde_json::from_value(c).map_err(|e| e.to_string())?
                    } else {
                        let c = client
                            .get_artist_catalogue(&candidate.id, market, false)
                            .await?;
                        db.set_preference(&cache_key, &json!(c)).await?;
                        c
                    };
                let albums = cat
                    .releases
                    .iter()
                    .filter(|release| {
                        let unofficial = release.official == Some(false)
                            || release.secondary_types.iter().any(|kind| {
                                ["bootleg", "promo", "unofficial"]
                                    .iter()
                                    .any(|flag| kind.eq_ignore_ascii_case(flag))
                            })
                            || release.title.to_ascii_lowercase().contains("bootleg");
                        let compilation = release.r#type.eq_ignore_ascii_case("compilation")
                            || release
                                .secondary_types
                                .iter()
                                .any(|kind| kind.eq_ignore_ascii_case("compilation"));
                        (!unofficial || settings["general"]["recommend_bootlegs"] == true)
                            && (!compilation
                                || settings["general"]["recommend_compilations"] == true)
                    })
                    .map(|r| r.title.clone())
                    .collect::<Vec<_>>();
                let score = crate::matching::score_artist_candidate(
                    name,
                    &titles,
                    &candidate.id,
                    &candidate.name,
                    &albums,
                );
                if score.exact_name && score.matched_releases > 0 {
                    accepted.push(candidate.id.clone());
                    client.save_catalogue_to_db(db, market, &cat).await?;
                }
                scores.push(json!({"id":candidate.id,"name":candidate.name,"score":score.score,"evidence":score.evidence}));
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let status = if accepted.is_empty() {
                "review"
            } else {
                "auto"
            };
            conn.execute("INSERT OR REPLACE INTO match_reviews(artist,status,payload,error,updated) VALUES(?,?,?,NULL,?)",(name,status,json!({"candidates":scores}).to_string(),chrono::Utc::now().to_rfc3339())).await.map_err(|e|e.to_string())?;
            if !accepted.is_empty() {
                db.choose_artist(name, &accepted).await?;
            }
            checked += 1;
            let outcome = if accepted.is_empty() {
                format!("Needs review · {name} · no release-title evidence for an exact artist name; use match details to inspect candidates or check recording links")
            } else {
                format!("Artist linked · {name} · {} supported artist IDs saved", accepted.len())
            };
            state.log_with_category(&outcome, "info", Some("online"));
            state.progress_for(kind, &outcome);
        }
        return Ok(json!({"checked":checked}));
    }
    if kind == "metadata" || kind == "manual_candidate" {
        let mut output = vec![];
        let http = crate::network::client(20)?;
        let mut subscriber_token = None;
        let mut prepared = Vec::new();
        let mut albums = Vec::new();
        let mut complete_dj: std::collections::HashMap<String, HashSet<String>> = std::collections::HashMap::new();
        for file in indexed.iter().filter(|f| ids.is_empty() || ids.contains(&f.path)) {
            if cancel.load(Ordering::Relaxed) { break; }
            let detail = db.get_detail(&json!({"path":file.path,"market":market})).await?;
            let tags = workflows::extract_tags_map(&file.metadata);
            if let Some(album) = args["album_id"].as_str().or(detail["linked_ids"]["album_id"].as_str()).or_else(||tags.get("tidal_album_id").map(String::as_str)) {
                if !complete_dj.contains_key(album) {
                    let cached = db.get_preference(&format!("tag-review:{market}:{album}")).await?;
                    let complete = cached.as_ref().filter(|v|v["track_metadata_source"]=="subscriber" && v["track_metadata_checked_at"].as_i64().is_some_and(|at|(0..30*86400).contains(&(chrono::Utc::now().timestamp()-at)))).and_then(|v|v["tracks"].as_array()).into_iter().flatten()
                        .filter(|t|t["bpm"].as_f64().is_some_and(|n|n>0.) && t["key"].as_str().is_some_and(|s|!s.is_empty()))
                        .filter_map(|t|t["id"].as_str().map(str::to_owned)).collect();
                    complete_dj.insert(album.to_owned(),complete);
                }
                let track_id = detail["linked_ids"]["track_id"].as_str().or_else(||tags.get("tidal_track_id").map(String::as_str));
                if track_id.is_none_or(|id| !complete_dj[album].contains(id)) { albums.push(album.to_owned()); }
            }
            // Retain only IDs, not the full candidate/metadata payload for every file.
            prepared.push((file,json!({"linked_ids":detail["linked_ids"]})));
            if prepared.len() % 100 == 0 { state.progress_for(kind,&format!("Preparing metadata · {} files checked for cached DJ data",prepared.len())); }
        }
        let album_metadata = if kind == "metadata" {
            crate::subscriber_metadata::prefetch(db, &http, albums, market, cancel.clone(), |message| state.progress_for(kind,message)).await?
        } else { std::collections::HashMap::new() };
        let total_files=prepared.len();
        for (position, (file, detail)) in prepared.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) { break; }
            let tags = workflows::extract_tags_map(&file.metadata);
            let album = args["album_id"]
                .as_str()
                .or(detail["linked_ids"]["album_id"].as_str())
                .or_else(|| tags.get("tidal_album_id").map(String::as_str));
            let Some(album) = album else { continue };
            state.progress_for(kind, &format!("Checking missing metadata · {position}/{total_files} files · {} — {} · {} · release ID {album}", tags.get("albumartist").or(tags.get("artist")).map(String::as_str).unwrap_or("Unknown artist"), tags.get("title").map(String::as_str).unwrap_or("Untitled track"), tags.get("album").map(String::as_str).unwrap_or("Unknown release")));
            let value = release(db, album, market, false).await?;
            let mut rel: crate::tidal::TidalRelease =
                serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            let track_id = detail["linked_ids"]["track_id"]
                .as_str()
                .or_else(|| tags.get("tidal_track_id").map(String::as_str));
            if kind == "manual_candidate" {
                let options:Vec<Value>=rel.tracks.iter().filter(|t|crate::release_matching::recording_matches(tags.get("title").map(String::as_str).unwrap_or(""),file.metadata.as_ref().and_then(|v|v["duration"].as_f64()).unwrap_or(0.),tags.get("isrc").map(String::as_str),&t.title,t.duration,t.isrc.as_deref(),true)).map(|t|json!({"id":rel.id,"title":rel.title,"track_id":t.id,"artist":rel.artist,"album":rel.title,"tracks":rel.track_count,"position":format!("Disc {} · Track {}",t.disc_number,t.track_number),"evidence":"Recording candidate; choose a placement to link"})).collect();
                let conn = db.connect()?;
                let mut q = conn
                    .query(
                        "SELECT payload FROM track_links WHERE path=? AND market=?",
                        (file.path.as_str(), market),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let mut payload = if let Some(r) = q.next().await.map_err(|e| e.to_string())? {
                    serde_json::from_str::<Value>(&r.get::<String>(0).unwrap_or_default())
                        .unwrap_or(json!({}))
                } else {
                    json!({})
                };
                payload["catalogue_options"] = json!(options);
                drop(q);
                conn.execute(
                    "INSERT OR REPLACE INTO track_links(path,market,stamp,payload) VALUES(?,?,?,?)",
                    (
                        file.path.as_str(),
                        market,
                        json!([0, 0, file.size, file.mtime]).to_string(),
                        payload.to_string(),
                    ),
                )
                .await
                .map_err(|e| e.to_string())?;
                continue;
            }
            let Some(index) = rel
                .tracks
                .iter()
                .position(|t| Some(t.id.as_str()) == track_id)
            else {
                continue;
            };
            let mut track = rel.tracks[index].clone();
            if let Some(items) = album_metadata.get(album).and_then(|v|v["items"].as_array()) {
                if let Some(raw) = items.iter().find(|v|v["id"].as_str().map(str::to_owned).unwrap_or_else(||v["id"].to_string()) == track.id) {
                    crate::subscriber_metadata::merge(&mut track, raw);
                }
            }

            if track.bpm.is_none() || track.key.is_none() {
                let key = format!("dj-check:{market}:{}", track.id);
                let cached = db.get_preference(&key).await?;
                let extra = if let Some(c) = cached {
                    c
                } else {
                    if subscriber_token.is_none() {
                        subscriber_token = crate::stream_download::get_valid_token(db, &http)
                            .await
                            .ok();
                    }
                    if let Some(ref token) = subscriber_token {
                        let response = crate::network::get(http
                            .get(format!("https://api.tidal.com/v1/tracks/{}", track.id))
                            .query(&[("countryCode", market)])
                            .bearer_auth(token)
                            , std::time::Duration::from_millis(350), 3, Some(cancel.as_ref()))
                            .await
                            .map_err(|e| e.to_string())?
                            .error_for_status()
                            .map_err(|e| e.to_string())?;
                        let data: Value = response.json().await.map_err(|e| e.to_string())?;
                        db.set_preference(&key, &data).await?;
                        data
                    } else {
                        Value::Null
                    }
                };
                crate::subscriber_metadata::merge(&mut track, &extra);
            }
            rel.tracks[index] = track.clone();
            let mut stored = value;
            stored["tracks"]=json!(rel.tracks);
            db.set_preference(&format!("tag-review:{market}:{}", rel.id), &stored).await?;
            let mut changes = crate::enrichment::compute_missing_tags(&tags, &rel, &track);
            // Page ownership is not a reliable performer or album-artist credit.
            changes.remove("artist");
            changes.remove("albumartist");
            output.push(json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),"release":tags.get("album"),"title":tags.get("title"),"tags":tags,"changes":changes,"affected":!changes.is_empty(),"size":file.size,"mtime":file.mtime,"status":if changes.is_empty(){"No supplied missing tags"}else{"Missing tags found"},"item":{"path":file.path,"tags":changes},"source_release_id":rel.id,"source_track_id":track.id,"evidence":format!("API BPM: {} · Key: {}",track.bpm.map(|n|n.to_string()).unwrap_or("not supplied".into()),track.key.unwrap_or("not supplied".into()))}));
        }
        if kind == "manual_candidate" {
            return Ok(json!({"checked":ids.len()}));
        }
        let id = uuid::Uuid::new_v4().to_string();
        state.previews.lock().unwrap().insert(id.clone(),json!({"id":id,"created":chrono::Utc::now().timestamp_millis(),"operation":"metadata","root":root,"rows":output,"count":output.len()}));
        return Ok(json!({"preview_id":id,"operation":"metadata","root":root}));
    }
    if kind == "apply" {
        if args["confirmed"] != true {
            return Err("Review and confirm the proposed changes first".into());
        }
        let id = args["preview_id"].as_str().ok_or("Preview required")?;
        let preview = state
            .previews
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("Preview expired; prepare it again")?;
        if preview["root"] != root || ids.is_empty() {
            return Err("Select changes from this library's preview".into());
        }
        let rows = preview["rows"].as_array().ok_or("Invalid preview")?;
        if ids.iter().any(|id| {
            !rows
                .iter()
                .any(|r| r["path"].as_str() == Some(id.as_str()) && r["affected"] == true)
        }) {
            return Err("Select only affected files from the current preview".into());
        }
        let mut count = 0;
        let mut repaired = std::collections::HashSet::new();
        let mut errors = vec![];
        for row in preview["rows"].as_array().ok_or("Invalid preview")? {
            let path = row["path"].as_str().unwrap_or("");
            if !ids.contains(path) {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let result = async {
                let actual = std::fs::metadata(path).map_err(|e| e.to_string())?;
                let time = actual
                    .modified()
                    .map_err(|e| e.to_string())?
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos() as i64;
                if row["size"].as_u64() != Some(actual.len()) || row["mtime"].as_i64() != Some(time)
                {
                    return Err("File changed since preview; refresh first".into());
                }
                let item: maintenance::FileApplyItem =
                    serde_json::from_value(row["item"].clone()).map_err(|e| e.to_string())?;
                maintenance::apply_file_item(db, root, &item)
                    .await
                    .map(|_| ())
            }
            .await;
            match result {
                Ok(()) => { count += 1; if row["item"]["tags"].as_object().is_some_and(|tags| tags.keys().any(|k| matches!(k.as_str(), "tracknumber" | "discnumber" | "tracktotal" | "disctotal"))) { repaired.insert(path.to_string()); } },
                Err(e) => errors.push(format!("{path}: {e}")),
            }
            state.progress_for(
                kind,
                &format!(
                    "Applying reviewed changes · {}/{} files · {count} applied · {}",
                    count + errors.len(), ids.len(),
                    std::path::Path::new(path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
            );
        }
        if !repaired.is_empty() {
            state.progress_for(kind, "Updating links from cached release evidence");
            maintenance::refresh_number_links(db, root, &repaired).await;
        }
        state.previews.lock().unwrap().remove(id);
        if !errors.is_empty() {
            return Err(format!(
                "Applied {count} files; {} failed. {}",
                errors.len(),
                errors
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        return Ok(json!({"applied":count}));
    }
    if kind == "preview" {
        let action = args["action"].as_str().unwrap_or("dates");
        if !["dates", "numbers", "keys", "lyrics", "organise"].contains(&action) {
            return Err("Unknown local operation".into());
        }
        let template = settings["organisation"]["template"].as_str();
        state.progress_for(kind,&format!("Planning local corrections · 0/{} files · cached tags only",indexed.len()));
        let plans = workflows::plan_cached(db, &indexed, action, template).await?;
        state.progress_for(kind,&format!("Local corrections ready · {0}/{0} files",indexed.len()));
        let file_index:std::collections::HashMap<_,_>=indexed.iter().map(|f|(f.path.as_str(),f)).collect();
        let rows:Vec<Value>=plans.into_iter().filter(|p|ids.is_empty()||ids.contains(&p.path)).map(|p| {
            let f=file_index[p.path.as_str()];
            json!({"id":p.path,"path":p.path,"artist":p.artist,"release":p.album,"title":p.title,"tags":p.current_tags,"changes":p.changes,"target":p.target,"folder_operation":crate::organisation::folder_operation(&p.path, p.target.as_deref()),"evidence":p.issues.join("; "),"affected":!p.changes.is_empty() || p.target.is_some(),"status":if !p.changes.is_empty() || p.target.is_some() {"Needs update"} else {"Needs review"},"size":f.size,"mtime":f.mtime,"item":{"path":p.path,"target":p.target,"tags":p.changes}})
        }).collect();
        let id = uuid::Uuid::new_v4().to_string();
        state.previews.lock().unwrap().insert(id.clone(),json!({"id":id,"created":chrono::Utc::now().timestamp_millis(),"operation":action,"root":root,"rows":rows,"count":rows.len()}));
        return Ok(json!({"preview_id":id,"operation":action,"root":root}));
    }
    if kind == "mqa" {
        let mut rows = vec![];
        for (position,file) in indexed.iter().enumerate() {
            state.progress_for(kind,&format!("MQA audit · {position}/{} files · {}",indexed.len(),file.path));
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let key = format!("mqa-audit:{}", file.path);
            let saved = db.get_preference(&key).await?;
            let result = if args["force"] != true
                && saved.as_ref().is_some_and(|v| {
                    v["size"] == json!(file.size) && v["mtime"] == json!(file.mtime)
                }) {
                saved.unwrap()["result"].clone()
            } else {
                let path=file.path.clone();
                tokio::task::spawn_blocking(move ||serde_json::to_value(crate::mqa::audit_file(std::path::Path::new(&path))))
                    .await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())?
            };
            db.set_preference(
                &key,
                &json!({"size":file.size,"mtime":file.mtime,"result":result}),
            )
            .await?;
            let tags = workflows::extract_tags_map(&file.metadata);
            rows.push(json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),"release":tags.get("album"),"title":tags.get("title"),"status":result["status"],"evidence":result["evidence"],"affected":result["detected"],"target":if result["detected"] == true { "Queue lossless replacement" } else { "—" }}));
        }
        db.set_preference(&format!("desktop-mqa:{root}"), &json!(rows))
            .await?;
        db.set_preference(
            &format!("desktop-mqa-manifest:{root}"),
            &json!(crate::duplicates::manifest_fingerprint(&indexed)),
        )
        .await?;
        return Ok(json!({"files":rows.len()}));
    }
    Err(format!(
        "{kind} is not yet available in this build; no changes were made"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn recommendation_refresh_reuses_complete_unchanged_snapshots_and_rechecks_deltas() {
        let dir=std::env::temp_dir().join(format!("recommendation-delta-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let summary=json!({"id":10,"title":"Release","numberOfTracks":2,"artist":{"id":4,"name":"Main"},"label":{"name":"Label"},"checked_at":1,"allowStreaming":true,"streamReady":true});
        let fingerprint=crate::tidal::summary_fingerprint(&summary);
        let mut reshaped=summary.clone();
        reshaped["id"]=json!("10"); reshaped["artist"]["picture"]=json!("changed-artwork");
        reshaped["label"]=json!("Label"); reshaped["numberOfVolumes"]=json!(1);
        reshaped["checked_at"]=json!(chrono::Utc::now().timestamp()); reshaped["streamReady"]=json!(false);
        assert_eq!(fingerprint,crate::tidal::summary_fingerprint(&reshaped));
        let saved=json!({"id":"10","tracks_loaded":true,"track_metadata_source":"subscriber","track_metadata_checked_at":1,"recommendation_snapshot":{"schema":RECOMMENDATION_SCHEMA,"summary_fingerprint":fingerprint,"optional_status":"complete"},"tracks":[{"id":"101","credits_complete":true,"credits":[],"bpm":null,"key":null}]});
        db.set_preference("tag-review:GB:10",&saved).await.unwrap();
        let release=crate::tidal::TidalRelease{id:"10".into(),summary_fingerprint:Some(fingerprint.clone()),..Default::default()};
        let new=crate::tidal::TidalRelease{id:"11".into(),summary_fingerprint:Some("new".into()),..Default::default()};
        let (reused,changed,_)=recommendation_refresh_plan(&db,"GB",&[release.clone(),new]).await.unwrap();
        assert_eq!(reused,HashSet::from(["10".into()])); assert!(changed.is_empty());
        let mut changed_summary=summary.clone(); changed_summary["numberOfTracks"]=json!(3);
        assert!(summary_change_requires_track_refresh(true,Some(&summary),Some(&changed_summary)),"Refreshing an expired summary cannot retain an old credited track list");
        assert!(!summary_change_requires_track_refresh(true,Some(&summary),Some(&reshaped)),"Equivalent endpoint shapes must not cause extra track fetches");
        assert!(!summary_change_requires_track_refresh(false,Some(&summary),Some(&changed_summary)));
        let changed_release=crate::tidal::TidalRelease{summary_fingerprint:Some(crate::tidal::summary_fingerprint(&changed_summary)),..release.clone()};
        let (reused,changed,_)=recommendation_refresh_plan(&db,"GB",&[changed_release]).await.unwrap();
        assert!(reused.is_empty()); assert_eq!(changed,HashSet::from(["10".into()]));
        let mut incomplete=saved.clone(); incomplete["tracks"][0]["credits_complete"]=json!(false);
        db.set_preference("tag-review:GB:10",&incomplete).await.unwrap();
        assert!(recommendation_refresh_plan(&db,"GB",&[release.clone()]).await.unwrap().0.is_empty());
        db.set_preference("tag-review:GB:10",&saved).await.unwrap();
        let mut deferred = saved.clone();
        deferred["recommendation_snapshot"]["optional_status"]=json!("retry");
        deferred["recommendation_snapshot"]["optional_retry_after"]=json!(chrono::Utc::now().timestamp()+3600);
        db.set_preference("tag-review:GB:10",&deferred).await.unwrap();
        assert!(recommendation_refresh_plan(&db,"GB",&[release.clone()]).await.unwrap().0.contains("10"));
        deferred["recommendation_snapshot"]["optional_retry_after"]=json!(1);
        db.set_preference("tag-review:GB:10",&deferred).await.unwrap();
        let (reused,_,cached_tracks)=recommendation_refresh_plan(&db,"GB",&[release.clone()]).await.unwrap();
        assert!(reused.is_empty()); assert!(cached_tracks.contains("10"),"Retry optional fields without refetching complete credits");
        // The legacy shortcut must consume the richer saved response, rather
        // than returning an incomplete tag-review row without rechecking it.
        incomplete["subscriber_discovery_checked_at"]=json!(chrono::Utc::now().timestamp());
        incomplete["track_metadata_checked_at"]=json!(chrono::Utc::now().timestamp());
        db.set_preference("tag-review:GB:10",&incomplete).await.unwrap();
        db.set_preference("subscriber-summary:GB:10",&reshaped).await.unwrap();
        db.set_preference("subscriber-items:GB:10",&json!({"schema":2,"checked_at":chrono::Utc::now().timestamp(),"items":[{"id":101,"title":"Track","isrc":"ABC","trackNumber":1,"volumeNumber":1,"credits":[]}]})).await.unwrap();
        db.set_preference("subscriber-discovery:GB:10",&json!({"checked_at":chrono::Utc::now().timestamp(),"genres":[],"replacement_id":null})).await.unwrap();
        let repaired=super::release(&db,"10","GB",false).await.unwrap();
        assert_eq!(repaired["tracks"][0]["credits_complete"],true);
        assert_eq!(repaired["recommendation_snapshot"]["optional_status"],"complete");
        assert_eq!(repaired["label"],"Label");
        db.set_preference("tag-review:GB:10",&saved).await.unwrap();
        assert!(recommendation_refresh_plan(&db,"US",&[release]).await.unwrap().0.is_empty());
        drop(db);std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn release_index_backfills_and_preserves_shared_releases_queue_and_markets() {
        let dir = std::env::temp_dir().join(format!("release-index-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let conn = db.connect().unwrap();
        let snapshot = json!({"id":"a","name":"Artist","releases":[{"id":"10","title":"Old"},{"id":"11","title":"Sibling"}]});
        for (artist,market) in [("a","GB"),("b","GB"),("a","US")] {
            conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES(?,?,?)", (artist,market,snapshot.to_string())).await.unwrap();
        }
        conn.execute("INSERT INTO queue(id,payload,approved,decision) VALUES('10',?,1,'queued')",(json!({"id":"10","selected_track_ids":["101"],"destination":"keep"}).to_string(),)).await.unwrap();
        let one = json!({"id":"10","title":"Updated","tracks_loaded":true,"tracks":[{"id":"101"}]});
        let two = json!({"id":"11","title":"Updated sibling","tracks_loaded":true,"tracks":[{"id":"111"}]});
        let (a,b) = tokio::join!(publish_release(&db,"10","GB",one.clone()),publish_release(&db,"11","GB",two.clone()));
        a.unwrap(); b.unwrap();
        let mut rows = conn.query("SELECT artist_id,market,payload FROM catalogue",()).await.unwrap();
        while let Some(row) = rows.next().await.unwrap() {
            let market: String = row.get(1).unwrap();
            let payload: Value = serde_json::from_str(&row.get::<String>(2).unwrap()).unwrap();
            assert_eq!(payload["releases"],if market=="GB" {json!([one,two])} else {snapshot["releases"].clone()});
        }
        drop(rows);
        let mut rows = conn.query("SELECT payload,approved FROM queue WHERE id='10'",()).await.unwrap();
        let row=rows.next().await.unwrap().unwrap();
        let queued: Value=serde_json::from_str(&row.get::<String>(0).unwrap()).unwrap();
        assert_eq!(queued["selected_track_ids"],json!(["101"]));
        assert_eq!(queued["destination"],"keep");
        assert_eq!(queued["tracks"],one["tracks"]);
        assert_eq!(row.get::<i64>(1).unwrap(),1);
        drop(rows);
        let client=crate::tidal::TidalClient::from_db(&db).await.unwrap();
        client.save_catalogue_to_db(&db,"GB",&crate::tidal::TidalCatalogue{id:"a".into(),name:"Artist".into(),releases:vec![]}).await.unwrap();
        let mut rows=conn.query("SELECT artist_id FROM catalogue_release_index WHERE market='GB' AND release_id='10'",()).await.unwrap();
        assert_eq!(rows.next().await.unwrap().unwrap().get::<String>(0).unwrap(),"b");
        assert!(rows.next().await.unwrap().is_none());
        drop(rows); drop(conn); drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn same_release_waits_while_other_releases_remain_independent() {
        let first = release_gate("test:GB:1".into());
        let same = release_gate("test:GB:1".into());
        let other = release_gate("test:GB:2".into());
        let guard = first.lock().await;
        assert!(same.try_lock().is_err());
        assert!(other.try_lock().is_ok());
        drop(guard);
        assert!(same.try_lock().is_ok());
    }

    #[test]
    fn combined_summary_retains_artist_ids_for_filter_reuse() {
        let mut value = json!({"title":"Example","tracks_loaded":true});
        let raw = json!({"data":{"id":"123","relationships":{"artists":{"data":[{"id":"7"},{"id":"8"}]}}},"included":[{"type":"artists","id":"8","attributes":{"name":"Guest"}},{"type":"artists","id":"7","attributes":{"name":"Lead"}}]});
        apply_release_discovery(&mut value, &raw, "123");
        assert_eq!(value["album_artist_ids"], json!(["7","8"]));
        assert_eq!(value["album_artists"], json!(["Lead","Guest"]));
        assert_eq!(value["tracks_loaded"], true);
        assert!(value["discovery_checked_at"].as_i64().is_some());
        let before = value.clone();
        apply_release_discovery(&mut value, &raw, "999");
        assert_eq!(value, before);
    }

    #[tokio::test]
    async fn real_flac_prepare_workflows_are_read_only_until_confirmed() {
        let Ok(sample) = std::env::var("TIBRARY_SAMPLE_FLAC") else {
            return;
        };
        let source = std::path::Path::new(&sample);
        let before = std::fs::read(source).unwrap();
        let temp = std::env::temp_dir().join(format!("tibrary-real-prep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let copied = temp.join("sample.flac");
        std::fs::copy(source, &copied).unwrap();
        crate::tag_writer::write_tags(
            &copied,
            &std::collections::HashMap::from([
                ("tracknumber".into(), "1".into()),
                ("discnumber".into(), "1".into()),
            ]),
        )
        .unwrap();
        let db = TursoDb::open(&temp.join("db")).await.unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        crate::scanner::scan_library(&db, &temp, cancel.clone(), |_| {})
            .await
            .unwrap();
        let backend = Arc::new(Backend::new());
        for action in ["dates", "numbers", "keys", "lyrics", "organise"] {
            execute(
                &db,
                &backend,
                "preview",
                &json!({"root":temp,"action":action}),
                cancel.clone(),
            )
            .await
            .unwrap();
        }
        let preview = execute(
            &db,
            &backend,
            "preview",
            &json!({"root":temp,"action":"numbers"}),
            cancel.clone(),
        )
        .await
        .unwrap();
        let applied = execute(&db, &backend, "apply", &json!({"root":temp,"preview_id":preview["preview_id"],"ids":[copied],"confirmed":true}), cancel.clone()).await.unwrap();
        assert_eq!(applied["applied"], 1);
        let copied_tags = workflows::extract_tags_map(&Some(
            serde_json::to_value(crate::scanner::read_audio_metadata(&copied).unwrap()).unwrap(),
        ));
        assert_eq!(
            copied_tags.get("tracknumber").map(String::as_str),
            Some("01")
        );
        let audited = execute(&db, &backend, "mqa", &json!({"root":temp}), cancel.clone())
            .await
            .unwrap();
        assert_eq!(audited["files"], 1);
        assert_eq!(std::fs::read(source).unwrap(), before);
        drop(db);
        std::fs::remove_dir_all(temp).unwrap();
    }
    #[tokio::test]
    async fn preview_apply_roundtrip_preserves_audio_and_requires_review() {
        let temp = std::env::temp_dir().join(format!("tibrary-action-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let path = temp.join("track.flac");
        std::fs::write(&path, crate::stream_download::MINIMAL_FLAC).unwrap();
        crate::tag_writer::write_tags(
            &path,
            &std::collections::HashMap::from([
                ("title".into(), "Example".into()),
                ("tracknumber".into(), "1".into()),
                ("discnumber".into(), "1".into()),
                ("bpm".into(), "123".into()),
                ("initialkey".into(), "8A".into()),
            ]),
        )
        .unwrap();
        let db = TursoDb::open(&temp.join("db")).await.unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        crate::scanner::scan_library(&db, &temp, cancel.clone(), |_| {})
            .await
            .unwrap();
        let backend = Arc::new(Backend::new());
        let before = std::fs::read(&path).unwrap();
        let preview = execute(
            &db,
            &backend,
            "preview",
            &json!({"root":temp,"action":"numbers"}),
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!(before, std::fs::read(&path).unwrap());
        let mut args = json!({"root":temp,"preview_id":preview["preview_id"],"ids":[path]});
        assert!(execute(&db, &backend, "apply", &args, cancel.clone())
            .await
            .is_err());
        args["confirmed"] = json!(true);
        let result = execute(&db, &backend, "apply", &args, cancel)
            .await
            .unwrap();
        assert_eq!(result["applied"], 1);
        let meta = crate::scanner::read_audio_metadata(&path).unwrap();
        let tags = workflows::extract_tags_map(&Some(serde_json::to_value(meta).unwrap()));
        assert_eq!(tags.get("tracknumber").map(String::as_str), Some("01"));
        assert_eq!(tags.get("bpm").map(String::as_str), Some("123"));
        assert_eq!(tags.get("initialkey").map(String::as_str), Some("8A"));
        drop(db);
        std::fs::remove_dir_all(temp).unwrap();
    }
    #[tokio::test]
    async fn cached_metadata_review_and_stale_file_guard() {
        let temp = std::env::temp_dir().join(format!("tibrary-cached-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let path = temp.join("track.flac");
        std::fs::write(&path, crate::stream_download::MINIMAL_FLAC).unwrap();
        crate::tag_writer::write_tags(
            &path,
            &std::collections::HashMap::from([
                ("title".into(), "Example".into()),
                ("album".into(), "Release".into()),
                ("albumartist".into(), "Canonical Artist".into()),
                ("tidal_track_id".into(), "456".into()),
                ("tidal_album_id".into(), "123".into()),
            ]),
        )
        .unwrap();
        let db = TursoDb::open(&temp.join("db")).await.unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        crate::scanner::scan_library(&db, &temp, cancel.clone(), |_| {})
            .await
            .unwrap();
        db.set_preference("tag-review:GB:123", &json!({"id":"123","title":"Release","artist":"Wrong Featured Credit","tracks_loaded":true,"track_count":1,"tracks":[{"id":"456","title":"Example","isrc":"TEST123","track_number":1,"disc_number":1}]})).await.unwrap();
        db.set_preference("subscriber-items:GB:123", &json!({"schema":2,"checked_at":chrono::Utc::now().timestamp(),"items":[{"id":456,"title":"Example","trackNumber":1,"volumeNumber":1,"credits":[],"isrc":"TEST123","bpm":124.0,"key":"G","keyScale":"major"}]})).await.unwrap();
        let backend = Arc::new(Backend::new());
        let before = std::fs::read(&path).unwrap();
        let result = execute(
            &db,
            &backend,
            "metadata",
            &json!({"root":temp,"ids":[path]}),
            cancel.clone(),
        )
        .await
        .unwrap();
        let id = result["preview_id"].as_str().unwrap();
        let preview = backend.previews.lock().unwrap().get(id).cloned().unwrap();
        assert_eq!(preview["rows"][0]["changes"]["bpm"], "124");
        assert_eq!(preview["rows"][0]["changes"]["initialkey"], "9B");
        assert!(preview["rows"][0]["changes"]["albumartist"].is_null());
        assert_eq!(before, std::fs::read(&path).unwrap());
        // External edits invalidate the reviewed write, even if cached links remain.
        crate::tag_writer::write_tags(
            &path,
            &std::collections::HashMap::from([(
                "comment".into(),
                "Changed outside the app".into(),
            )]),
        )
        .unwrap();
        let changed = std::fs::read(&path).unwrap();
        let error = execute(
            &db,
            &backend,
            "apply",
            &json!({"root":temp,"preview_id":id,"ids":[path],"confirmed":true}),
            cancel,
        )
        .await
        .unwrap_err();
        assert!(error.contains("File changed since preview"));
        assert_eq!(changed, std::fs::read(&path).unwrap());
        drop(db);
        std::fs::remove_dir_all(temp).unwrap();
    }
}
