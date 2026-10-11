//! Desktop actions share the same native services as tables and reviewed writes.
use crate::{
    db::{LocalFileRecord, TursoDb},
    maintenance, workflows, Backend,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
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
            | "check_availability"
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

fn require_duplicate_quality(root: &str, indexed: &[LocalFileRecord]) -> Result<(), String> {
    let missing_quality = indexed.iter().filter(|file| file.present).any(|file| {
        std::path::Path::new(&file.path).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("flac"))
            && file.metadata.as_ref().is_none_or(|metadata|
                metadata["sample_rate"].as_u64().unwrap_or(0) == 0 || metadata["bit_depth"].as_u64().unwrap_or(0) < 8)
    });
    if missing_quality && crate::library_access::require_library(root).is_err() {
        return Err("Reconnect the library once to cache audio quality before comparing local duplicates. Saved duplicate results remain available.".into());
    }
    Ok(())
}

async fn duplicate_review_clusters(db: &TursoDb, root: &str, indexed: &[LocalFileRecord]) -> Result<Vec<crate::duplicates::DuplicateCluster>, String> {
    let unavailable = match require_duplicate_quality(root, indexed) {
        Ok(()) => return Ok(crate::duplicates::find_duplicate_clusters(indexed)),
        Err(error) => error,
    };
    let manifest = crate::duplicates::manifest_fingerprint(indexed);
    let current = db.get_preference(&format!("desktop-local-manifest:{root}")).await?
        .as_ref().and_then(Value::as_str) == Some(manifest.as_str());
    if current {
        if let Some(saved) = db.get_preference(&format!("desktop-local:{root}")).await?.and_then(|value| value.as_array().cloned()) {
            let releases = crate::duplicates::parse_local_releases(indexed);
            let mut clusters = Vec::new();
            for row in saved {
                let master = releases.iter().find(|release| Some(release.folder.as_str()) == row["path"].as_str())
                    .ok_or("Saved duplicate review is incomplete; reconnect the library to check again")?.clone();
                let mut redundant = Vec::new();
                for child in row["children"].as_array().ok_or("Saved duplicate review is incomplete; reconnect the library to check again")? {
                    redundant.push(releases.iter().find(|release| Some(release.folder.as_str()) == child["path"].as_str())
                        .ok_or("Saved duplicate review is incomplete; reconnect the library to check again")?.clone());
                }
                clusters.push(crate::duplicates::DuplicateCluster {
                    cluster_id: row["id"].as_str().ok_or("Saved duplicate group has no ID")?.to_owned(),
                    master, is_chained: row["status"] == "Chained duplicate", chain_summary: String::new(),
                    total_redundant_tracks: redundant.iter().map(|release| release.tracks.len()).sum(),
                    total_recoverable_bytes: redundant.iter().flat_map(|release| &release.tracks).map(|track| track.size).sum(),
                    redundant,
                });
            }
            return Ok(clusters);
        }
    }
    Err(unavailable)
}

async fn remove_reviewed_duplicate(db: &TursoDb, root: &str, path: &str) -> Result<(), String> {
    crate::library_access::require_library(root)?;
    if !std::fs::symlink_metadata(path).map_err(|error| format!("Reviewed duplicate is unavailable; cached file retained: {error}"))?.is_file() {
        return Err("Reviewed duplicate is no longer a regular file; cached file retained".into());
    }
    crate::duplicates::trash_file_or_directory(path).await?;
    // Finder may report success or time out while the drive is disconnecting.
    crate::library_access::require_library(root)?;
    db.remove_local_file(path).await
}

/// Rebuild the visible audit list from indexed files and prior per-file results.
/// Changed files become "Not audited" until the user runs the audio inspection.
pub async fn cached_mqa_rows(
    db: &TursoDb,
    indexed: &[LocalFileRecord],
) -> Result<Vec<Value>, String> {
    let saved = maintenance::load_inspections(db, "mqa-audit:").await?;
    let rows = indexed.iter().filter(|file| mqa_eligible(file)).map(|file| {
        let tags = workflows::extract_tags_map(&file.metadata);
        let prior = saved.get(&file.path).filter(|value| mqa_inspection_current(value, file));
        let result = prior.map(|value| &value["result"]);
        json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),"release":tags.get("album"),"title":tags.get("title"),
            "scanned":result.is_some(),
            "status":result.and_then(|value| value["status"].as_str()).unwrap_or("Not audited"),
            "evidence":result.and_then(|value| value["evidence"].as_str()).unwrap_or("New or changed audio; run the MQA audit"),
            "affected":result.is_some_and(|value| value["detected"] == true),"target":if result.is_some_and(|v| v["detected"] == true) { "Queue lossless replacement" } else { "—" }})
    }).collect();
    Ok(rows)
}

fn mqa_eligible(file: &LocalFileRecord) -> bool {
    file.present && std::path::Path::new(&file.path).extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("flac"))
}

fn mqa_inspection_current(value: &Value, file: &LocalFileRecord) -> bool {
    maintenance::valid_inspection(value, file.size, file.mtime) && value["result"].is_object()
}

/// Scope and replacement readiness come from the full indexed library, never
/// the current table page. This is a cache-only read and cannot change files.
pub async fn mqa_selection(db: &TursoDb, args: &Value) -> Result<Value, String> {
    let root = args["root"].as_str().filter(|root| !root.is_empty())
        .ok_or("Choose a registered library first")?;
    let scope = args["scope"].as_str().unwrap_or("selected");
    if !matches!(scope, "all" | "unscanned" | "selected") {
        return Err("Choose all tracks, unscanned tracks or selected tracks".into());
    }
    let explicit: HashSet<String> = match args.get("ids") {
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone())
            .map_err(|_| "Select tracks with valid file paths")?.into_iter().collect(),
        None => HashSet::new(),
    };
    if scope == "selected" && explicit.is_empty() {
        return Ok(json!({"ids":[],"detected_ids":[],"unlinked_ids":[],"ready_ids":[],
            "counts":{"selected":0,"detected":0,"unlinked":0,"ready":0}}));
    }
    let conn = db.connect()?;
    let mut rows = conn.query(
        "SELECT f.path,f.size,f.mtime,p.payload FROM local_files f LEFT JOIN app_preferences p ON p.key='mqa-audit:' || f.path WHERE f.root=? AND f.present=1 AND (? != 'selected' OR f.path IN (SELECT value FROM json_each(?))) ORDER BY f.path",
        (root,scope,json!(explicit).to_string()),
    ).await.map_err(|error| error.to_string())?;
    let mut indexed = vec![];
    let mut saved = std::collections::HashMap::new();
    while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
        let path: String = row.get(0).map_err(|error| error.to_string())?;
        if let Some(value) = row.get::<Option<String>>(3).ok().flatten()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok()) {
            saved.insert(path.clone(),value);
        }
        indexed.push(LocalFileRecord {path,root:root.into(),
            size:row.get(1).map_err(|error| error.to_string())?,
            mtime:row.get(2).map_err(|error| error.to_string())?,
            metadata:None,error:None,present:true});
    }
    drop(rows);
    drop(conn);
    let eligible: Vec<_> = indexed.iter().filter(|file| mqa_eligible(file)).collect();
    let current = |file: &&LocalFileRecord| saved.get(&file.path)
        .filter(|value| mqa_inspection_current(value, file));
    let unscanned = eligible.iter().filter(|file| current(file).is_none()).count();
    let selected: Vec<_> = eligible.iter().copied().filter(|file| match scope {
        "all" => true,
        "unscanned" => current(file).is_none(),
        _ => explicit.contains(&file.path),
    }).collect();
    let detected: Vec<_> = selected.iter().copied().filter(|file|
        current(file).is_some_and(|value| value["result"]["detected"] == true)
    ).collect();
    let settings = db.get_settings().await?;
    let market = args["market"].as_str()
        .or_else(|| settings["general"]["market"].as_str()).unwrap_or("GB");
    let detected_paths: Vec<_> = detected.iter().map(|file| file.path.as_str()).collect();
    let links = metadata_link_ids(db, &detected_paths, market, &AtomicBool::new(false)).await?;
    let verified = |file: &&LocalFileRecord| links.get(&file.path).is_some_and(|ids|
        ids["album_id"].as_str().is_some_and(|id| !id.is_empty())
            && ids["track_id"].as_str().is_some_and(|id| !id.is_empty())
    );
    let ready: Vec<_> = detected.iter().filter(|file| verified(file)).map(|file| &file.path).collect();
    let unlinked: Vec<_> = detected.iter().filter(|file| !verified(file)).map(|file| &file.path).collect();
    let mut result = json!({
        "ids":selected.iter().map(|file| &file.path).collect::<Vec<_>>(),
        "detected_ids":detected_paths,"unlinked_ids":unlinked,"ready_ids":ready,
        "counts":{"selected":selected.len(),
            "detected":detected.len(),"unlinked":unlinked.len(),"ready":ready.len()}
    });
    if scope != "selected" {
        result["counts"]["all"] = json!(eligible.len());
        result["counts"]["unscanned"] = json!(unscanned);
    }
    Ok(result)
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

/// Waiting for another job's cache fill must not make cancellation wait for
/// that job's network timeout. Only the caller is cancelled; the owner keeps
/// filling the shared cache for any other interested workflows.
pub(crate) async fn release_guard(
    gate: Arc<tokio::sync::Mutex<()>>,
    cancel: &AtomicBool,
) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
    let waiting = gate.lock_owned();
    tokio::pin!(waiting);
    loop {
        if cancel.load(Ordering::Relaxed) { return Err("Cancelled".into()); }
        tokio::select! {
            guard = &mut waiting => return Ok(guard),
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
    }
}

/// Metadata preparation only needs the chosen recording/release IDs. Loading
/// the full match dialog for every file also reads all alternative editions,
/// credits and DJ checks, doing the same expensive work thousands of times.
async fn metadata_link_ids(
    db: &TursoDb,
    paths: &[&str],
    market: &str,
    cancel: &AtomicBool,
) -> Result<std::collections::HashMap<String, Value>, String> {
    if paths.is_empty() || cancel.load(Ordering::Relaxed) { return Ok(Default::default()); }
    let conn = db.connect()?;
    let mut rows = conn.query(
        "SELECT path, CASE WHEN json_valid(payload) THEN CASE WHEN json_extract(payload,'$.status') IS NULL OR json_extract(payload,'$.status')='linked' THEN json_extract(payload,'$.ids') END END FROM track_links WHERE market=? AND path IN (SELECT value FROM json_each(?))",
        (market,json!(paths).to_string()),
    ).await.map_err(|error|error.to_string())?;
    let mut ids = std::collections::HashMap::new();
    while let Some(row) = rows.next().await.map_err(|error|error.to_string())? {
        if cancel.load(Ordering::Relaxed) { break; }
        let path: String = row.get(0).map_err(|error|error.to_string())?;
        if let Some(value) = row.get::<Option<String>>(1).ok().flatten().and_then(|raw|serde_json::from_str::<Value>(&raw).ok()).filter(Value::is_object) {
            ids.insert(path,value);
        }
    }
    Ok(ids)
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
            if recommendation_optional_current(prior,chrono::Utc::now().timestamp()) {
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

fn recommendation_optional_current(value: &Value, now: i64) -> bool {
    let snapshot=&value["recommendation_snapshot"];
    (snapshot["optional_status"] == "complete"
        && snapshot["optional_schema"] == crate::tidal::DISCOVERY_SCHEMA
        && snapshot["optional_valid_until"].as_i64().is_some_and(|until|until > now))
        || snapshot["optional_retry_after"].as_i64().is_some_and(|until|until > now)
}

fn summary_change_requires_track_refresh(reused_tracks: bool, before: Option<&Value>, after: Option<&Value>) -> bool {
    reused_tracks && before.zip(after).is_some_and(|(before,after)|crate::tidal::summary_fingerprint(before) != crate::tidal::summary_fingerprint(after))
}

async fn record_recommendation_snapshot(db: &TursoDb, value: &mut Value, market: &str, summary: Option<&Value>, optional_fields: Option<&Value>) -> Result<(), String> {
    if let Some(summary) = summary {
        let fresh = serde_json::to_value(crate::tidal::release_from_subscriber(&summary,"")).map_err(|error|error.to_string())?;
        // An edition/title/primary-credit change must not be overwritten by the
        // earlier tag-review snapshot when fresh track details are published.
        for field in ["artist","title","date","type","copyright","label","quality","upc","original_release_date","artist_credits","audio_modes","media_metadata","summary_fingerprint"] {
            if !fresh[field].is_null() && !fresh[field].as_str().is_some_and(str::is_empty) && !fresh[field].as_array().is_some_and(Vec::is_empty) {
                value[field]=fresh[field].clone();
            }
        }
        let retry_after = if optional_fields.is_some() {Value::Null} else {
            db.get_preference(&format!("subscriber-discovery-paused:{market}")).await?
                .and_then(|paused|paused["until"].as_i64()).filter(|until|*until > chrono::Utc::now().timestamp())
                .map(|until|json!(until)).unwrap_or_else(||json!(chrono::Utc::now().timestamp()+3600))
        };
        value["recommendation_snapshot"] = json!({"schema":RECOMMENDATION_SCHEMA,"summary_fingerprint":crate::tidal::summary_fingerprint(&summary),"checked_at":chrono::Utc::now().timestamp(),"optional_status":if optional_fields.is_some() {"complete"} else {"retry"},"optional_schema":if optional_fields.is_some() {json!(crate::tidal::DISCOVERY_SCHEMA)} else {Value::Null},"optional_valid_until":optional_fields.map(|fields|fields["valid_until"].clone()),"optional_retry_after":retry_after});
    }
    Ok(())
}

pub async fn release(db: &TursoDb, id: &str, market: &str, force: bool) -> Result<Value, String> {
    release_with_cancel(db,id,market,force,Arc::new(AtomicBool::new(false))).await
}

/// A row disclosure needs its audio track list, not a new whole-catalogue job.
/// This shares the same per-release locks and credited item cache as refreshes
/// and downloads, without occupying or cancelling their worker lanes.
pub(crate) async fn ensure_release_tracks(db: &TursoDb, id: &str, market: &str, cancel: Arc<AtomicBool>) -> Result<Value, String> {
    if id.is_empty() || !id.bytes().all(|byte|byte.is_ascii_digit()) { return Err("Choose a release with a valid online ID".into()); }
    let gate = release_gate(format!("{}:{market}:{id}",db.path.display()));
    let _guard = release_guard(gate,cancel.as_ref()).await?;
    let saved = db.get_detail(&json!({"release_id":id,"market":market})).await?;
    let listed = db.cached_catalogue_release(id,market).await?;
    // A release-list update can invalidate its old track snapshot. Use the
    // same reconciliation as Missing releases before declaring those tracks
    // ready; otherwise this returns a list the table must continue to hide.
    let mut value = listed.as_ref().map(|listed|crate::db::recommendation_candidate_overlay(listed,saved.clone())).unwrap_or_else(||saved.clone());
    let summary = db.get_preference(&format!("subscriber-summary:{market}:{id}")).await?;
    let saved_fingerprint = saved["recommendation_snapshot"]["summary_fingerprint"].as_str().or_else(||saved["summary_fingerprint"].as_str());
    let summary_changed = summary.as_ref().zip(saved_fingerprint)
        .is_some_and(|(summary,pinned)|crate::tidal::summary_fingerprint(summary) != pinned);
    if !summary_changed && value["recommendation_track_snapshot_conflict"] != true
        && value["tracks_loaded"] == true && value["tracks"].as_array().is_some_and(|tracks|!tracks.is_empty()) {
        value["track_count"] = json!(value["tracks"].as_array().unwrap().len());
        // Older queue snapshots may still be summaries even when the central
        // release is complete. A single conditional update preserves concurrent
        // approvals/track selection and avoids a redundant metadata request.
        let updated = db.connect()?.execute(
            "UPDATE queue SET payload=json_set(payload,'$.tracks',json(?),'$.tracks_loaded',json('true'),'$.track_count',?) WHERE id=? AND json_valid(payload) AND (COALESCE(json_extract(payload,'$.tracks_loaded'),0) != 1 OR COALESCE(json_array_length(payload,'$.tracks'),0)=0)",
            (value["tracks"].to_string(), value["track_count"].as_i64().unwrap_or(0), id),
        ).await.map_err(|error|error.to_string())?;
        if updated > 0 { db.bump_revision(); }
        return Ok(value);
    }
    if value["title"] == "Release not found" { return Err("Refresh this release list before opening its tracks".into()); }
    let recheck_summary = listed.as_ref().and_then(|listed|listed["summary_fingerprint"].as_str())
        .is_some_and(|current|summary.as_ref().is_none_or(|summary|crate::tidal::summary_fingerprint(summary) != current));
    if recheck_summary {
        // Older databases can have a current artist listing but a missing or
        // older shared summary. Reconcile just this release before asking the
        // credited-item cache to validate its track list.
        let mut client = crate::tidal::TidalClient::from_db(db).await?.with_cancel(cancel.clone());
        let response = client.albums(&[id.to_owned()],market,true).await?;
        if !response["data"].as_array().is_some_and(|items|items.iter().any(|item|item["id"].as_str() == Some(id))) {
            return Err("This release is unavailable in the selected market; saved metadata was retained".into());
        }
    }
    let http = crate::network::client(20)?;
    let raw = crate::subscriber_metadata::load(db,&http,id,market,cancel,false).await?;
    let mut tracks = crate::subscriber_metadata::tracks(&raw)?;
    // Check again after the read: another catalogue worker can have changed
    // the summary while the credited list was being fetched. Never relabel
    // old recordings with the new summary merely to make the table expand.
    let current_summary = db.get_preference(&format!("subscriber-summary:{market}:{id}")).await?;
    let current_listing = db.cached_catalogue_release(id,market).await?;
    let fingerprint = raw["summary_fingerprint"].as_str();
    let listing_fingerprint = |value: Option<&Value>|value.and_then(|listed|listed["summary_fingerprint"].as_str()).map(str::to_owned);
    let listing_changed = listing_fingerprint(current_listing.as_ref()) != listing_fingerprint(listed.as_ref());
    let use_checked_summary = recheck_summary && !listing_changed;
    if current_summary.as_ref().is_some_and(|summary|fingerprint != Some(crate::tidal::summary_fingerprint(summary).as_str()))
        || (!use_checked_summary && current_listing.as_ref().and_then(|listed|listed["summary_fingerprint"].as_str()).is_some_and(|current|fingerprint != Some(current))) {
        return Err("The release summary changed before its tracks could be verified. Refresh this release's track details and retry; saved metadata was retained.".into());
    }
    let old_tracks: Vec<crate::tidal::TidalTrack> = serde_json::from_value(saved["tracks"].clone()).unwrap_or_default();
    crate::subscriber_metadata::supplement(&mut tracks,&old_tracks);
    if summary_changed || recheck_summary || value["recommendation_track_snapshot_conflict"] == true {
        // Keep the current listing's facts, without carrying the earlier
        // edition's optional-metadata completion claims into the new snapshot.
        value = match current_listing.filter(|_|!use_checked_summary) {
            Some(listed) => listed,
            None => serde_json::to_value(crate::tidal::release_from_subscriber(current_summary.as_ref().ok_or("The release summary is no longer cached")?,"")).map_err(|error|error.to_string())?,
        };
        value["recommendation_snapshot"] = Value::Null;
    }
    if let Some(object) = value.as_object_mut() {object.remove("recommendation_track_snapshot_conflict");}
    // Preserve optional fields already collected by a complete metadata pass;
    // loading the disclosure must not claim unchecked catalogue fields are done.
    if let Some(fingerprint) = fingerprint {
        value["summary_fingerprint"] = json!(fingerprint);
        value["recommendation_snapshot"]["schema"] = json!(RECOMMENDATION_SCHEMA);
        value["recommendation_snapshot"]["summary_fingerprint"] = json!(fingerprint);
    }
    value["tracks"] = serde_json::to_value(&tracks).map_err(|error|error.to_string())?;
    value["tracks_loaded"] = json!(true);
    value["track_count"] = json!(tracks.len());
    value["track_metadata_source"] = json!("subscriber");
    value["track_metadata_checked_at"] = raw["checked_at"].clone();
    publish_release(db,id,market,value).await
}

pub(crate) async fn release_with_cancel(db: &TursoDb, id: &str, market: &str, force: bool, cancel: Arc<AtomicBool>) -> Result<Value, String> {
    release_with_publication(db,id,market,force,cancel,true).await
}

/// Catalogue refreshes flush canonical snapshots in groups. Completed details
/// remain durable immediately, while an artist's large JSON is rewritten once
/// for the group instead of once per release.
pub(crate) async fn release_with_cancel_staged(db: &TursoDb, id: &str, market: &str, force: bool, cancel: Arc<AtomicBool>) -> Result<Value, String> {
    release_with_publication(db,id,market,force,cancel,false).await
}

async fn release_with_publication(db: &TursoDb, id: &str, market: &str, force: bool, cancel: Arc<AtomicBool>, publish_catalogue: bool) -> Result<Value, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err("Select a release with a valid online ID".into());
    }
    let gate = release_gate(format!("{}:{market}:{id}", db.path.display()));
    let _guard = release_guard(gate,cancel.as_ref()).await?;
    let key = format!("tag-review:{market}:{id}");
    let cached = db.get_preference(&key).await?;
    let mut value = match cached {
        Some(value) => value,
        None => {
            db.get_detail(&json!({"release_id":id, "market":market}))
                .await?
        }
    };
    let previous_summary=db.get_preference(&format!("subscriber-summary:{market}:{id}")).await?;
    if !force && previous_summary.as_ref().is_some_and(|summary|recommendation_tracks_current(&value,&crate::tidal::summary_fingerprint(summary)))
        && recommendation_optional_current(&value,chrono::Utc::now().timestamp()) {
        return Ok(value);
    }
    // Pair the current summary before loading credited items. Otherwise a
    // first-time request stores an unbound list, discovers the summary later,
    // and immediately downloads that same list again to establish its identity.
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
    let summary = db.get_preference(&format!("subscriber-summary:{market}:{id}")).await?;
    let force = force || value["recommendation_snapshot"]["schema"].as_u64().is_some_and(|schema|schema != u64::from(RECOMMENDATION_SCHEMA)) || summary.as_ref().is_some_and(|summary| {
        value["recommendation_snapshot"]["summary_fingerprint"].as_str().is_some_and(|prior| prior != crate::tidal::summary_fingerprint(summary))
    });
    let snapshot_current = summary.as_ref().is_some_and(|summary| recommendation_tracks_current(&value,&crate::tidal::summary_fingerprint(summary)));
    if !force && snapshot_current && recommendation_optional_current(&value,chrono::Utc::now().timestamp()) {
        return Ok(value);
    }
    let old_tracks: Vec<crate::tidal::TidalTrack> = serde_json::from_value(value["tracks"].clone()).unwrap_or_default();
    let mut reuse_tracks = !force && snapshot_current;
    let mut track_fingerprint = None;
    let mut unpaired_reused = false;
    let mut tracks = if reuse_tracks {
        let mut tracks=old_tracks.clone();
        // Richer normalisation can expose performer IDs from an already saved
        // raw credited list; an additive field is not a reason for another GET.
        if tracks.iter().any(|track|track.artists.is_empty()) {
            if let Some(raw)=crate::subscriber_metadata::cached(db,id,market).await? {
                let saved=crate::subscriber_metadata::tracks(&raw)?;
                crate::subscriber_metadata::supplement(&mut tracks,&saved);
            }
        }
        tracks
    } else {
        let http=crate::network::client(20)?;
        match crate::subscriber_metadata::load_with_cache_status(db,&http,id,market,cancel.clone(),force).await {
            Ok((raw,cached))=>{
                track_fingerprint = raw["summary_fingerprint"].as_str().map(str::to_owned);
                unpaired_reused = cached && track_fingerprint.is_none();
                crate::subscriber_metadata::tracks(&raw)?
            },
            Err(error) if cancel.load(Ordering::Relaxed) => return Err(error),
            Err(error) if value["tracks_loaded"] == true && !force => {
                value["metadata_note"]=json!(format!("Saved metadata retained: {error}. Reconnect your account and retry if needed."));
                return Ok(value);
            }
            Err(error)=>return Err(format!("Subscriber metadata unavailable: {error}. Connect your account and retry.")),
        }
    };
    crate::subscriber_metadata::supplement(&mut tracks,&old_tracks);
    let optional = match crate::tidal::TidalClient::from_db(db).await {
        Ok(client) => match client.with_cancel(cancel.clone()).discovery(&[id.to_owned()],market,force).await {
            Ok(fields) => fields,
            Err(error) if cancel.load(Ordering::Relaxed) => return Err(error),
            Err(_) => std::collections::HashMap::new(),
        },
        Err(_) => std::collections::HashMap::new(),
    };
    if let Some(fields)=optional.get(id) {
        crate::tidal::merge_discovery(&mut value,fields);
    } else {
        let paused=db.get_preference(&format!("subscriber-discovery-paused:{market}")).await?.unwrap_or(Value::Null);
        value["catalogue_metadata_status"]=json!({"status":"retry","attempted_at":chrono::Utc::now().timestamp(),"retry_after":paused["until"],"note":paused["message"].as_str().unwrap_or("Optional catalogue fields could not be checked; saved values retained."),"fields":{"genres":"incomplete","label":"incomplete","providers":"incomplete","replacement":"incomplete"}});
    }
    let current_summary=db.get_preference(&format!("subscriber-summary:{market}:{id}")).await?;
    if summary_change_requires_track_refresh(reuse_tracks,summary.as_ref(),current_summary.as_ref())
        || (unpaired_reused && current_summary.is_some())
        || track_fingerprint.as_ref().is_some_and(|pinned| current_summary.as_ref().is_some_and(|summary| crate::tidal::summary_fingerprint(summary) != *pinned)) {
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
    record_recommendation_snapshot(db,&mut value,market,current_summary.as_ref(),optional.get(id)).await?;
    publish_release_update(db, id, market, value, None, publish_catalogue).await
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
        // Reading a cached summary today is not a new availability observation.
        // Its original timestamp must not overwrite a later withdrawal check.
        value["availability_checked_at"] = data["attributes"]["checked_at"].clone();
        value["availability_source"] = data["attributes"]["availability_source"].clone();
    }
    if let Some(artists) = data["relationships"]["artists"]["data"].as_array() {
        value["album_artist_ids"] = json!(artists.iter().filter_map(|a| a["id"].as_str()).collect::<Vec<_>>());
        value["album_artists"] = json!(artists.iter().filter_map(|a| included.iter().find(|v| v["type"] == "artists" && v["id"] == a["id"]).and_then(|v| v["attributes"]["name"].as_str())).collect::<Vec<_>>());
        value["tag_checked_at"] = value["discovery_checked_at"].clone();
    }
}

pub(crate) async fn publish_release(
    db: &TursoDb,
    id: &str,
    market: &str,
    value: Value,
) -> Result<Value, String> {
    publish_release_update(db,id,market,value,None,true).await
}

#[cfg(test)]
async fn publish_track_enrichment(db: &TursoDb, id: &str, market: &str, track: &crate::tidal::TidalTrack) -> Result<Value,String> {
    publish_track_enrichments(db,id,market,std::slice::from_ref(track)).await
}

#[cfg(test)]
async fn publish_track_enrichments(db: &TursoDb, id: &str, market: &str, tracks: &[crate::tidal::TidalTrack]) -> Result<Value,String> {
    publish_track_enrichments_mode(db,id,market,tracks,true).await
}

async fn publish_track_enrichments_mode(db: &TursoDb, id: &str, market: &str, tracks: &[crate::tidal::TidalTrack], publish_catalogue: bool) -> Result<Value,String> {
    let gate = release_gate(format!("{}:{market}:{id}",db.path.display()));
    let _guard = gate.lock().await;
    publish_release_update(db,id,market,Value::Null,Some(tracks),publish_catalogue).await
}

async fn publish_release_update(
    db: &TursoDb,
    id: &str,
    market: &str,
    mut value: Value,
    track_patches: Option<&[crate::tidal::TidalTrack]>,
    publish_catalogue: bool,
) -> Result<Value,String> {
    if publish_catalogue { db.ensure_catalogue_release_index().await?; }
    // Publish details to every catalogue reference and existing queue entry.
    let conn = db.connect()?;
    conn.execute("BEGIN IMMEDIATE", ()).await.map_err(|e|e.to_string())?;
    let result = async {
        if let Some(incoming_tracks) = track_patches {
            // Merge against the latest canonical recording inside the write
            // transaction, preserving parallel updates to all other tracks.
            let mut rows = conn.query("SELECT payload FROM app_preferences WHERE key=?",(format!("tag-review:{market}:{id}"),)).await.map_err(|e|e.to_string())?;
            let saved = rows.next().await.map_err(|e|e.to_string())?.ok_or("Release metadata no longer cached; refresh it first")?;
            value = serde_json::from_str(&saved.get::<String>(0).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
            drop(rows);
            let tracks = value["tracks"].as_array_mut().ok_or("Release track list no longer cached")?;
            for incoming in incoming_tracks {
            let target = tracks.iter_mut().find(|track| track["id"].as_str() == Some(&incoming.id)).ok_or("Recording no longer belongs to this release; refresh it first")?;
            let mut current: crate::tidal::TidalTrack = serde_json::from_value(target.clone()).map_err(|e|e.to_string())?;
            if current.isrc.as_ref().zip(incoming.isrc.as_ref()).is_some_and(|(a,b)|!a.eq_ignore_ascii_case(b)) { return Err("Recording identifier changed; saved metadata retained".to_owned()); }
            crate::subscriber_metadata::supplement(std::slice::from_mut(&mut current),std::slice::from_ref(incoming));
            current.copyright = current.copyright.or_else(||incoming.copyright.clone());
            current.credits_complete |= incoming.credits_complete;
            current.credits_checked_at = current.credits_checked_at.max(incoming.credits_checked_at);
            *target = json!(current);
            }
        }
        conn.execute("INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)", (format!("tag-review:{market}:{id}"),value.to_string())).await.map_err(|e|e.to_string())?;
        if let (Some(available), Some(checked_at)) = (value["available"].as_bool(),value["availability_checked_at"].as_i64()) {
            crate::availability::save_observation(&conn, id, market, Some(available), checked_at, value["availability_source"].as_str().unwrap_or("legacy")).await?;
        }
        let pending_key=format!("pending-catalogue:{market}:{id}");
        if publish_catalogue {
            publish_catalogue_values(&conn,market,&HashMap::from([(id.to_owned(),value.clone())])).await?;
            conn.execute("DELETE FROM app_preferences WHERE key=?",(pending_key,)).await.map_err(|error|error.to_string())?;
        } else {
            // Durable marker makes cancellation/crash recovery unambiguous:
            // only completed staged writes may be copied into artist catalogues.
            conn.execute("INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)",(pending_key,json!({"id":id,"market":market}).to_string())).await.map_err(|error|error.to_string())?;
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
            queued["track_count"] = value["track_count"].clone();
            drop(q);
            conn.execute(
                "UPDATE queue SET payload=? WHERE id=?",
                (queued.to_string(), id),
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        conn.execute("COMMIT", ()).await.map_err(|e|e.to_string())?;
        Ok::<_,String>(value.clone())
    }.await;
    if result.is_err() { let _ = conn.execute("ROLLBACK", ()).await; }
    let value = result?;
    db.bump_revision();
    Ok(value)
}

/// Apply only current canonical release snapshots, so a delayed batch cannot
/// overwrite metadata added by a concurrent download/tag job. Queue choices are
/// never included in these artist-catalogue snapshots.
pub(crate) async fn publish_catalogue_releases(db: &TursoDb, ids: &[String], market: &str) -> Result<(),String> {
    if ids.is_empty() { return Ok(()); }
    let conn=db.connect()?;
    let mut pending=conn.query("SELECT 1 FROM app_preferences WHERE key IN (SELECT 'pending-catalogue:' || ? || ':' || value FROM json_each(?)) LIMIT 1",(market,json!(ids).to_string())).await.map_err(|error|error.to_string())?;
    if pending.next().await.map_err(|error|error.to_string())?.is_none() {return Ok(());}
    drop(pending);
    db.ensure_catalogue_release_index().await?;
    conn.execute("BEGIN IMMEDIATE",()).await.map_err(|error|error.to_string())?;
    let result=async {
        let mut rows=conn.query("SELECT p.payload,summary.payload FROM app_preferences p JOIN app_preferences pending ON pending.key='pending-catalogue:' || ? || ':' || json_extract(p.payload,'$.id') LEFT JOIN app_preferences summary ON summary.key='subscriber-summary:' || ? || ':' || json_extract(p.payload,'$.id') WHERE p.key IN (SELECT 'tag-review:' || ? || ':' || value FROM json_each(?))",(market,market,market,json!(ids).to_string())).await.map_err(|error|error.to_string())?;
        let mut values=HashMap::new();
        let mut completed=Vec::new();
        while let Some(row)=rows.next().await.map_err(|error|error.to_string())? {
            let value: Value=serde_json::from_str(&row.get::<String>(0).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
            if let Some(id)=value["id"].as_str() {
                completed.push(id.to_owned());
                let summary=row.get::<Option<String>>(1).ok().flatten().and_then(|raw|serde_json::from_str::<Value>(&raw).ok());
                // An artist refresh may have observed a newer edition while
                // this canonical update waited for its batch. Clear the stale
                // marker and let normal fingerprint invalidation refill it.
                let superseded=summary.as_ref().is_some_and(|summary| value["recommendation_snapshot"]["summary_fingerprint"].as_str() != Some(crate::tidal::summary_fingerprint(summary).as_str()));
                if !superseded {values.insert(id.to_owned(),value);}
            }
        }
        drop(rows);
        publish_catalogue_values(&conn,market,&values).await?;
        conn.execute("DELETE FROM app_preferences WHERE key IN (SELECT 'pending-catalogue:' || ? || ':' || value FROM json_each(?))",(market,json!(completed).to_string())).await.map_err(|error|error.to_string())?;
        conn.execute("COMMIT",()).await.map_err(|error|error.to_string())?;
        Ok::<_,String>(!values.is_empty())
    }.await;
    if result.is_err() { let _=conn.execute("ROLLBACK",()).await; }
    if result? {db.bump_revision();}
    Ok(())
}

/// Recover only completed staged publications after a restart. No API work is
/// repeated, and incomplete/failed releases never acquire a pending marker.
pub(crate) async fn recover_catalogue_publications(db: &TursoDb) -> Result<(),String> {
    let conn=db.connect()?;
    let mut rows=conn.query("SELECT payload FROM app_preferences WHERE key LIKE 'pending-catalogue:%'",()).await.map_err(|error|error.to_string())?;
    let mut markets:HashMap<String,Vec<String>>=HashMap::new();
    while let Some(row)=rows.next().await.map_err(|error|error.to_string())? {
        let raw:String=row.get(0).map_err(|error|error.to_string())?;
        let value:Value=serde_json::from_str(&raw).map_err(|error|error.to_string())?;
        if let (Some(market),Some(id))=(value["market"].as_str(),value["id"].as_str()) {markets.entry(market.to_owned()).or_default().push(id.to_owned());}
    }
    drop(rows);
    for (market,ids) in markets {publish_catalogue_releases(db,&ids,&market).await?;}
    Ok(())
}

async fn publish_catalogue_values(conn: &turso::Connection, market: &str, values: &HashMap<String,Value>) -> Result<(),String> {
    if values.is_empty() { return Ok(()); }
    let ids: Vec<_>=values.keys().collect();
    let mut rows=conn.query(
        "SELECT c.artist_id,c.payload FROM catalogue c WHERE c.market=? AND c.artist_id IN (SELECT artist_id FROM catalogue_release_index WHERE market=? AND release_id IN (SELECT value FROM json_each(?)))",
        (market,market,json!(ids).to_string()),
    ).await.map_err(|error|error.to_string())?;
    let mut updates=Vec::new();
    while let Some(row)=rows.next().await.map_err(|error|error.to_string())? {
        let artist: String=row.get(0).map_err(|error|error.to_string())?;
        let raw: String=row.get(1).map_err(|error|error.to_string())?;
        let mut catalogue: Value=serde_json::from_str(&raw).map_err(|error|error.to_string())?;
        let mut changed=false;
        for release in catalogue["releases"].as_array_mut().into_iter().flatten() {
            if let Some(value)=release["id"].as_str().and_then(|id|values.get(id)) {
                if release != value { *release=value.clone(); changed=true; }
            }
        }
        if changed { updates.push((artist,catalogue.to_string())); }
    }
    drop(rows);
    for (artist,payload) in updates {
        conn.execute("UPDATE catalogue SET payload=? WHERE artist_id=? AND market=?",(payload,artist,market)).await.map_err(|error|error.to_string())?;
    }
    Ok(())
}

/// A completed optional check may legitimately contain no BPM/key. Failures
/// have a retry deadline and never become a permanent "not supplied" result.
fn dj_check_reusable(value: &Value, now: i64) -> bool {
    value.is_object() && (value["status"] != "retry" || value["retry_after"].as_i64().is_some_and(|until|until > now))
}

async fn check_dj_metadata(db: &TursoDb, http: &reqwest::Client, track: &str, market: &str, cancel: Arc<AtomicBool>) -> Result<Value,String> {
    let key = format!("dj-check:{market}:{track}");
    let gate = release_gate(format!("dj:{}:{market}:{track}",db.path.display()));
    let _guard = release_guard(gate,cancel.as_ref()).await?;
    if let Some(saved) = db.get_preference(&key).await?.filter(|saved|crate::tidal::resource_id(&saved["id"]) == track && dj_check_reusable(saved,chrono::Utc::now().timestamp())) { return Ok(saved); }
    let fetched = async {
        let token = crate::stream_download::get_valid_token(db,http).await?;
        let response = crate::network::get(http.get(format!("https://api.tidal.com/v1/tracks/{track}"))
            .query(&[("countryCode",market)]).bearer_auth(token),std::time::Duration::from_millis(350),3,Some(cancel.as_ref()))
            .await?.error_for_status().map_err(|e|e.to_string())?;
        let mut value: Value = response.json().await.map_err(|e|e.to_string())?;
        if !value.is_object() || crate::tidal::resource_id(&value["id"]) != track { return Err("Invalid recording metadata response".to_owned()); }
        value["status"] = json!("complete");
        value["checked_at"] = json!(chrono::Utc::now().timestamp());
        Ok::<_,String>(value)
    }.await;
    let value = match fetched {
        Ok(value)=>value,
        Err(error) if cancel.load(Ordering::Relaxed)=>return Err(error),
        Err(error)=>json!({"id":track,"status":"retry","retry_after":chrono::Utc::now().timestamp()+300,"error":error}),
    };
    db.set_preference(&key,&value).await?;
    Ok(value)
}

fn metadata_service_paused(error: &str) -> bool {
    let error=error.to_ascii_lowercase();
    ["401 unauthorized","403 forbidden","429 too many requests","http 401","http 403","http 429","service cooldown","no tidal login token","login token expired","authentication required","token refresh request failed","token refresh failed"].iter().any(|phrase|error.contains(phrase))
}

/// Prepare each selected release once. Both the release and recording queues
/// are bounded; the shared network lane still owns all pacing and 429 cooldowns.
/// The raw credited list is fetched by release_with_cancel, not fetched in an
/// earlier pass and then decoded/fetched again for every selected file.
#[derive(Clone, Copy, Default)]
struct DjMetadataNeeds { bpm: bool, key: bool }

impl DjMetadataNeeds {
    fn from_tags(tags: &HashMap<String,String>) -> Self {
        // Match the missing-tag writer: existing nonempty local values are
        // preserved, so looking them up cannot produce an additional proposal.
        Self { bpm: !crate::enrichment::has_any_tag(tags,"bpm"), key: !crate::enrichment::has_any_tag(tags,"initialkey") }
    }
    fn missing_from(&self, track: &crate::tidal::TidalTrack) -> bool {
        (self.bpm && track.bpm.is_none() && !track.bpm_checked) || (self.key && track.key.is_none() && !track.key_checked)
    }
}

struct PreparedMetadataRelease {
    release: crate::tidal::TidalRelease,
    dj_notes: HashMap<String, String>,
}

async fn prepare_metadata_release(
    db: TursoDb, http: reqwest::Client, album: String, selected_tracks: HashMap<String,DjMetadataNeeds>,
    market: String, cancel: Arc<AtomicBool>, progress_state: Arc<std::sync::Mutex<(usize,String)>>,
) -> Result<PreparedMetadataRelease, String> {
    let value=release_with_cancel_staged(&db,&album,&market,false,cancel.clone()).await?;
    let mut release: crate::tidal::TidalRelease=serde_json::from_value(value).map_err(|error|error.to_string())?;
    let before:HashMap<_,_>=release.tracks.iter().map(|track|(track.id.clone(),json!(track))).collect();
    if release.tracks.iter().any(|track|selected_tracks.get(&track.id).is_some_and(|needs|needs.missing_from(track))) {
        // Legacy canonical rows predate explicit checked-empty flags. Recover
        // these observations from the validated raw cache without an API call.
        if let Some(raw)=crate::subscriber_metadata::cached(&db,&album,&market).await? {
            let normalized=crate::subscriber_metadata::tracks(&raw)?;
            crate::subscriber_metadata::supplement(&mut release.tracks,&normalized);
        }
    }
    let mut pending: VecDeque<_>=release.tracks.iter()
        .filter(|track| selected_tracks.get(&track.id).is_some_and(|needs|needs.missing_from(track)))
        .map(|track|track.id.clone()).collect();
    {
        let mut progress=progress_state.lock().unwrap();
        progress.0 += 1 + selected_tracks.len() - pending.len();
        progress.1 = format!("{} — {} · saved track details and credits ready",release.artist,release.title);
    }
    let mut tasks=tokio::task::JoinSet::new();
    let mut notes=HashMap::new();
    loop {
        if cancel.load(Ordering::Relaxed) { break; }
        while tasks.len() < crate::network::METADATA_CONCURRENCY {
            let Some(id)=pending.pop_front() else { break; };
            let db=db.clone(); let http=http.clone(); let market=market.clone(); let cancel=cancel.clone();
            tasks.spawn(async move { let result=check_dj_metadata(&db,&http,&id,&market,cancel).await; (id,result) });
        }
        let Some(result)=tasks.join_next().await else { break; };
        let (id,extra)=result.map_err(|error|error.to_string())?;
        let extra=extra?;
        if extra["status"] == "retry" {
            let error=extra["error"].as_str().unwrap_or("Retry paused");
            if metadata_service_paused(error) { return Err(format!("Metadata checks paused: {error}. Completed results are saved; retry later.")); }
            notes.insert(id.clone(),error.to_owned());
        }
        if let Some(track)=release.tracks.iter_mut().find(|track|track.id == id) {
            crate::subscriber_metadata::merge(track,&extra);
            let mut progress=progress_state.lock().unwrap();
            progress.0 += 1;
            progress.1 = format!("{} — {} · {} · BPM/key checked",release.artist,release.title,track.title);
        }
    }
    if cancel.load(Ordering::Relaxed) { return Err("Cancelled".into()); }
    let enriched:Vec<_>=release.tracks.iter().filter(|track|before.get(&track.id) != Some(&json!(track))).cloned().collect();
    if !enriched.is_empty() {
        // Stage one patch per release, then publish containing artist catalogues
        // in groups. Legacy checked-empty normalization must not cause thousands
        // of complete catalogue rewrites merely to add two booleans.
        release=serde_json::from_value(publish_track_enrichments_mode(&db,&album,&market,&enriched,false).await?).map_err(|error|error.to_string())?;
    }
    Ok(PreparedMetadataRelease {release,dj_notes:notes})
}

async fn prepare_metadata_releases(
    db: &TursoDb, http: &reqwest::Client, albums: HashMap<String, HashMap<String,DjMetadataNeeds>>,
    market: &str, cancel: Arc<AtomicBool>, progress: impl Fn(&str),
) -> Result<HashMap<String, PreparedMetadataRelease>, String> {
    let total=albums.len();
    let total_checks=total + albums.values().map(HashMap::len).sum::<usize>();
    let all_ids:Vec<_>=albums.keys().cloned().collect();
    let progress_state=Arc::new(std::sync::Mutex::new((0_usize,String::new())));
    let mut last_report=std::time::Instant::now();
    let mut last_completed=0;
    let mut pending: VecDeque<_>=albums.into_iter().collect();
    let mut tasks=tokio::task::JoinSet::new();
    let mut result=HashMap::new();
    let mut completed_ids=Vec::new();
    progress(&format!("Preparing missing tags · 0/{total_checks} checks · {total} releases · reusing saved credits, genres and BPM/key; fetching missing data"));
    let prepared=async {
        loop {
            if cancel.load(Ordering::Relaxed) { return Err("Cancelled".to_owned()); }
            while tasks.len() < crate::network::METADATA_CONCURRENCY {
                let Some((id,tracks))=pending.pop_front() else { break; };
                let db=db.clone(); let http=http.clone(); let market=market.to_owned(); let cancel=cancel.clone();
                let progress_state=progress_state.clone();
                tasks.spawn(async move {
                    let prepared=prepare_metadata_release(db,http,id.clone(),tracks,market,cancel,progress_state).await;
                    (id,prepared)
                });
            }
            if tasks.is_empty() { break; }
            let completed=tokio::select! {
                completed=tasks.join_next()=>completed,
                _=tokio::time::sleep(std::time::Duration::from_millis(100))=>{
                    // Large editions keep updating the same job line while DJ
                    // checks run; throttle UI notifications to once per second.
                    let (checked,detail)=progress_state.lock().unwrap().clone();
                    if checked != last_completed && last_report.elapsed() >= std::time::Duration::from_secs(1) {
                        progress(&format!("Preparing missing tags · {checked}/{total_checks} checks · {detail}"));
                        last_completed=checked; last_report=std::time::Instant::now();
                    }
                    continue;
                }
            };
            let Some(completed)=completed else { break; };
            let (id,prepared)=completed.map_err(|error|error.to_string())?;
            let prepared=prepared?;
            let checked=progress_state.lock().unwrap().0;
            progress(&format!("Missing tags prepared · {checked}/{total_checks} checks · {} of {total} releases ready · {} — {}",result.len()+1,prepared.release.artist,prepared.release.title));
            last_completed=checked; last_report=std::time::Instant::now();
            completed_ids.push(id.clone());
            result.insert(id,prepared);
            if completed_ids.len() >= 20 {
                publish_catalogue_releases(db,&completed_ids,market).await?;
                completed_ids.clear();
            }
        }
        Ok::<_,String>(())
    }.await;
    // Wait for aborted workers to release any transaction before flushing. Only
    // durable staged markers are eligible, including a worker cancelled between
    // committing its result and returning it to this coordinator.
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    publish_catalogue_releases(db,&all_ids,market).await?;
    prepared?;
    Ok(result)
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
    if kind == "check_availability" {
        let mut ids: Vec<String> = if let Some(selected) = args.get("ids") {
            serde_json::from_value(selected.clone()).map_err(|_| "Select releases with valid online IDs")?
        } else {
            state.progress_for(kind, "Preparing availability check · saved missing releases · music files unchanged");
            let mut rows = db.get_missing_rows(market, Some("All missing releases"), Some("My album artists"), None, None, None, None, None, 0, usize::MAX).await?.rows;
            // Revisit earlier unavailable results after their shorter retry window,
            // so a release that returns to the market can become visible again.
            rows.extend(db.get_missing_rows(market, None, None, Some("Unavailable"), None, None, None, None, 0, usize::MAX).await?.rows);
            rows.into_iter().map(|row| row.id).collect()
        };
        if ids.iter().any(|id| id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit())) { return Err("Select releases with valid online IDs".into()); }
        ids.sort(); ids.dedup();
        let result = crate::availability::check_with_progress(db, &ids, market, args["force"].as_bool().unwrap_or(false), cancel,
            |message| state.progress_for(kind, message),
            |message| state.log_with_category(message, "info", Some("online")),
        ).await?;
        let available = result.values().filter(|value| **value == Some(true)).count();
        let unavailable = result.values().filter(|value| **value == Some(false)).count();
        let unknown = result.len() - available - unavailable;
        return Ok(json!({"message":format!("Availability checked · {}/{} releases · {available} available · {unavailable} unavailable · {unknown} not confirmed · {market}", result.len(), ids.len()),"total":ids.len(),"checked":result.len(),"available":available,"unavailable":unavailable,"unknown":unknown}));
    }
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
                        crate::account::save_account_market(db, &sess, None).await?;
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
    if crate::library_access::requires_local_files(kind) { crate::library_access::require_library(root)?; }
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
        let clusters = duplicate_review_clusters(db, root, &indexed).await?;
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
            remove_reviewed_duplicate(db, root, &path).await?;
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
        let mut replacements = std::collections::HashMap::<String, Value>::new();
        let mut queued_paths = vec![];
        let selected_paths: Vec<_> = indexed.iter().filter(|file|ids.contains(&file.path)).map(|file|file.path.as_str()).collect();
        let links = metadata_link_ids(db,&selected_paths,market,cancel.as_ref()).await?;
        for file in indexed.iter().filter(|f| ids.contains(&f.path)) {
            if cancel.load(Ordering::Relaxed) { return Err("Cancelled; replacements were not queued".into()); }
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
            let linked = links.get(&file.path).cloned().unwrap_or(Value::Null);
            if let (Some(album), Some(track)) = (
                linked["album_id"].as_str().filter(|id|!id.is_empty()),
                linked["track_id"].as_str().filter(|id|!id.is_empty()),
            ) {
                queued_paths.push(file.path.clone());
                let tracks=selection
                    .entry(album.into())
                    .or_insert_with(|| Some(vec![]))
                    .as_mut()
                    .unwrap();
                if !tracks.iter().any(|id|id==track) { tracks.push(track.into()); }
                let audit=replacements.entry(album.into()).or_insert_with(||json!({"root":root,"quality":"LOSSLESS","source_paths":{}}));
                let sources=audit["source_paths"].as_object_mut().unwrap()
                    .entry(track.to_owned()).or_insert_with(||json!([])).as_array_mut().unwrap();
                if !sources.iter().any(|path|path.as_str()==Some(&file.path)) { sources.push(json!(file.path)); }
            }
        }
        if selection.is_empty() {
            return Err(
                "No selected MQA tracks have verified recording links. Link them first.".into(),
            );
        }
        for album in selection.keys() {
            release_with_cancel(db,album,market,false,cancel.clone()).await?;
        }
        if cancel.load(Ordering::Relaxed) { return Err("Cancelled; replacements were not queued".into()); }
        db.queue_add_with_replacements(&selection,&replacements).await?;
        return Ok(json!({"root":root,"releases":selection.len(),"queued_paths":queued_paths}));
    }
    if kind == "queue_replacements" {
        if ids.is_empty() {
            return Err("Select replacement releases first".into());
        }
        let plans = db
            .get_preference(&format!("desktop-online:{root}"))
            .await?
            .unwrap_or(json!([]));
        let chosen = crate::duplicates::selected_online_replacement_releases(
            plans.as_array().map(Vec::as_slice).unwrap_or_default(), &ids);
        let selection: std::collections::HashMap<String, Option<Vec<String>>> = chosen.into_iter().map(|id|(id,None)).collect();
        if selection.is_empty() {
            return Err("No current replacement plans selected".into());
        }
        db.queue_add(&selection).await?;
        return Ok(json!({"releases":selection.len()}));
    }
    if kind == "optimizations" || kind == "local_duplicates" || kind == "check_replacements" {
        if kind == "local_duplicates" || args["scope"] != "remote" && kind != "check_replacements" {
            let manifest = crate::duplicates::manifest_fingerprint(&indexed);
            if args["force"] != true
                && db.get_preference(&format!("desktop-local-manifest:{root}")).await?.as_ref().and_then(Value::as_str) == Some(manifest.as_str())
            {
                if let Some(rows) = db.get_preference(&format!("desktop-local:{root}")).await?.and_then(|value| value.as_array().cloned()) {
                    state.progress_for(kind, &format!("Local duplicates ready · {} saved groups reused · indexed files unchanged", rows.len()));
                    return Ok(json!({"opportunities":rows.len(),"reused":true}));
                }
            }
            require_duplicate_quality(root, &indexed)?;
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
            require_duplicate_quality(root, &indexed)?;
            let rows = crate::duplicates::clusters_to_group_rows(&clusters);
            db.set_preference(&format!("desktop-local:{root}"), &json!(rows))
                .await?;
            db.set_preference(
                &format!("desktop-local-manifest:{root}"),
                &json!(manifest),
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
                // A larger published count can include videos. Replacements
                // must add audio, using the loaded audio-only track list.
                if target.tracks.len() <= local.tracks.len() { continue; }
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
                    let mut matched_track_ids: Vec<_> = claimed.into_iter().collect();
                    matched_track_ids.sort();
                    let target_metadata = json!({"id":target.id,"artist":target.artist,"title":target.title,"date":target.date,
                        "track_count":target.tracks.len(),"type":target.r#type,"quality":target.quality,"label":target.label,"copyright":target.copyright,"upc":target.upc});
                    rows.push(json!({"id":id,"artist":local.artist,"release":local.title,"date":local.date,"title":format!("{} tracks",local.tracks.len()),"tracks":local.tracks.len(),"duplicates":local.tracks.len(),"gained":target.tracks.len()-local.tracks.len(),"path":local.folder,"status":"Larger online release","evidence":format!("Every local recording has matching ISRC, mix title and duration; {} additional audio tracks",target.tracks.len()-local.tracks.len()),"target":target.title,"online_id":target.id,
                        "target_artist":target.artist,"target_date":target.date,"target_tracks":target.tracks.len(),"target_metadata":target_metadata,"matched_track_ids":matched_track_ids,
                        "affected":true,"changes":"Queue complete release; retain originals until downloaded and reviewed"}));
                }
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled; previous opportunities retained".into());
        }
        db.set_preference(&format!("desktop-online:{root}"), &json!(rows))
            .await?;
        let opportunities = rows.iter().filter_map(|row|row["online_id"].as_str()).collect::<HashSet<_>>().len();
        return Ok(json!({"opportunities":opportunities,"local_releases":rows.len()}));
    }
    if kind == "artwork" {
        let http = crate::network::client(25)?;
        let mut output = vec![];
        let cache = db
            .path
            .parent()
            .ok_or("Invalid cache directory")?
            .join("artwork-cache");
        std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        let saved_inspections = maintenance::load_inspections(db, "artwork-inspection:").await?;
        let mut reused = 0;
        let mut inspected = 0;
        let mut checked = 0;
        let artwork_files:Vec<_>=indexed.iter().filter(|f|ids.is_empty() || ids.contains(&f.path)).collect();
        let total_artwork=artwork_files.len();
        let selected_paths: Vec<_> = artwork_files.iter().map(|file|file.path.as_str()).collect();
        let links = metadata_link_ids(db,&selected_paths,market,cancel.as_ref()).await?;
        for (position,file) in artwork_files.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {break;}
            checked += 1;
            state.progress_for(kind,&format!("Checking artwork · {}/{total_artwork} files · {}",position+1,file.path));
            let (inspection, cached) = match maintenance::inspect_artwork(db, file, saved_inspections.get(&file.path), args["force"] == true).await {
                Ok(result) => result,
                Err(error) => {
                    state.log_for(kind,&format!("Artwork inspection failed · {} · {error}; file unchanged", file.path),"error");
                    continue;
                }
            };
            if cached { reused += 1; } else { inspected += 1; }
            if inspection.has_standard_cover() { continue; }
            let tags = workflows::extract_tags_map(&file.metadata);
            let linked = links.get(&file.path).cloned().unwrap_or(Value::Null);
            let Some(album) = linked["album_id"]
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
                // A saved cover URL is usable without reauthenticating. A
                // missing summary obtains its own valid session when needed.
                let info=crate::subscriber_metadata::album_info(db,&http,album,"",market).await?;
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
        state.progress_for(kind, &format!("Artwork checked · {checked}/{total_artwork} files · {reused} saved inspections reused · {inspected} files inspected · {} changes ready", output.len()));
        state.previews.lock().unwrap().insert(id.clone(),json!({"id":id,"created":chrono::Utc::now().timestamp_millis(),"operation":"artwork","root":root,"rows":output,"count":output.len()}));
        return Ok(json!({"preview_id":id,"operation":"artwork","root":root,"reused":reused,"inspected":inspected,"checked":checked}));
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
        let mut client = crate::tidal::TidalClient::from_db(db).await?.with_cancel(cancel.clone());
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
                let cat = client.artist_match_evidence(&candidate.id,market).await?;
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
        let mut inspected_paths = vec![];
        let http = crate::network::client(20)?;
        let mut prepared = Vec::new();
        let mut albums: HashMap<String, HashMap<String,DjMetadataNeeds>> = HashMap::new();
        let selected_paths: Vec<_> = indexed.iter().filter(|file| ids.is_empty() || ids.contains(&file.path)).map(|file|file.path.as_str()).collect();
        let link_ids = metadata_link_ids(db,&selected_paths,market,cancel.as_ref()).await?;
        for file in indexed.iter().filter(|f| ids.is_empty() || ids.contains(&f.path)) {
            if cancel.load(Ordering::Relaxed) { break; }
            let linked = link_ids.get(&file.path).cloned().unwrap_or(Value::Null);
            let tags = workflows::extract_tags_map(&file.metadata);
            if let Some(album) = args["album_id"].as_str().or(linked["album_id"].as_str()).or_else(||tags.get("tidal_album_id").map(String::as_str)) {
                let tracks=albums.entry(album.to_owned()).or_default();
                if let Some(track)=linked["track_id"].as_str().or_else(||tags.get("tidal_track_id").map(String::as_str)) {
                    let needs=DjMetadataNeeds::from_tags(&tags);
                    let combined=tracks.entry(track.to_owned()).or_default();
                    combined.bpm |= needs.bpm; combined.key |= needs.key;
                }
            }
            // Retain only IDs, not the full candidate/metadata payload for every file.
            prepared.push((file,json!({"linked_ids":linked})));
            if prepared.len() % 100 == 0 { state.progress_for(kind,&format!("Preparing metadata · {} files checked for cached DJ data",prepared.len())); }
        }
        let album_metadata = if kind == "metadata" {
            prepare_metadata_releases(db, &http, albums, market, cancel.clone(), |message| state.progress_for(kind,message)).await?
        } else { HashMap::new() };
        let total_files=prepared.len();
        let mut release_cache: std::collections::HashMap<String, crate::tidal::TidalRelease> = Default::default();
        for (position, (file, detail)) in prepared.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) { break; }
            let tags = workflows::extract_tags_map(&file.metadata);
            let album = args["album_id"]
                .as_str()
                .or(detail["linked_ids"]["album_id"].as_str())
                .or_else(|| tags.get("tidal_album_id").map(String::as_str));
            let Some(album) = album else { continue };
            state.progress_for(kind, &format!("Checking missing metadata · {}/{total_files} files · {} — {} · {} · release ID {album}",position+1, tags.get("albumartist").or(tags.get("artist")).map(String::as_str).unwrap_or("Unknown artist"), tags.get("title").map(String::as_str).unwrap_or("Untitled track"), tags.get("album").map(String::as_str).unwrap_or("Unknown release")));
            if kind == "manual_candidate" && !release_cache.contains_key(album) {
                let value = release_with_cancel(db,album,market,false,cancel.clone()).await?;
                release_cache.insert(album.to_owned(),serde_json::from_value(value).map_err(|e|e.to_string())?);
            }
            let rel = album_metadata.get(album).map(|prepared|&prepared.release).or_else(||release_cache.get(album)).ok_or("Release metadata preparation incomplete")?;
            let track_id = detail["linked_ids"]["track_id"]
                .as_str()
                .or_else(|| tags.get("tidal_track_id").map(String::as_str));
            if kind == "manual_candidate" {
                let options = crate::db::manual_recording_placements(&tags, file.metadata.as_ref().and_then(|v|v["duration"].as_f64()).unwrap_or(0.), &json!(rel));
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
                inspected_paths.push(file.path.clone());
                continue;
            }
            let Some(index) = rel
                .tracks
                .iter()
                .position(|t| Some(t.id.as_str()) == track_id)
            else {
                continue;
            };
            let track = &rel.tracks[index];
            let dj_note = album_metadata.get(album).and_then(|prepared|prepared.dj_notes.get(&track.id));
            let mut changes = crate::enrichment::compute_missing_tags(&tags, rel, track);
            // Page ownership is not a reliable performer or album-artist credit.
            changes.remove("artist");
            changes.remove("albumartist");
            output.push(json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),"release":tags.get("album"),"title":tags.get("title"),"tags":tags,"changes":changes,"affected":!changes.is_empty(),"size":file.size,"mtime":file.mtime,"status":if changes.is_empty(){"No supplied missing tags"}else{"Missing tags found"},"item":{"path":file.path,"tags":changes},"source_release_id":rel.id,"source_track_id":track.id,"evidence":format!("API BPM: {} · Key: {}{}",track.bpm.map(|n|n.to_string()).unwrap_or("not supplied".into()),track.key.as_deref().unwrap_or("not supplied"),dj_note.map(|note|format!(" · DJ check deferred: {note}")).unwrap_or_default())}));
        }
        if kind == "manual_candidate" {
            return Ok(json!({"checked":inspected_paths.len(),"root":root,"inspected_paths":inspected_paths}));
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
        let chosen: Option<Vec<String>> = args.get("ids").map(|value|
            serde_json::from_value(value.clone()).map_err(|_| "Select tracks with valid file paths".to_owned())
        ).transpose()?;
        if chosen.as_ref().is_some_and(Vec::is_empty) {
            return Err("Select tracks to scan".into());
        }
        let selected: Vec<_> = indexed.iter().filter(|file|
            mqa_eligible(file) && (chosen.is_none() || ids.contains(&file.path))
        ).collect();
        if selected.is_empty() {
            return Err("Select indexed FLAC tracks to scan".into());
        }
        let saved_inspections = maintenance::load_inspections(db, "mqa-audit:").await?;
        let mut reused = 0;
        let mut inspected = 0;
        for (position,file) in selected.iter().enumerate() {
            state.progress_for(kind,&format!("MQA audit · {position}/{} files · {}",selected.len(),file.path));
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let key = format!("mqa-audit:{}", file.path);
            let saved = saved_inspections.get(&file.path);
            if args["force"] != true
                && saved.is_some_and(|value| mqa_inspection_current(value, file)) {
                reused += 1;
            } else {
                let path=file.path.clone();
                let result = tokio::task::spawn_blocking(move ||serde_json::to_value(crate::mqa::audit_file(std::path::Path::new(&path))))
                    .await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())?;
                // Losing a drive during the read must not replace a saved audit with an I/O error.
                crate::library_access::require_library(root)?;
                inspected += 1;
                db.set_preference(&key, &json!({"size":file.size,"mtime":file.mtime,"result":result})).await?;
            }
        }
        // Refresh the whole view from per-file caches, including unchecked files
        // and partial progress retained when an audit is cancelled.
        let rows = cached_mqa_rows(db, &indexed).await?;
        db.set_preference(&format!("desktop-mqa:{root}"), &json!(rows))
            .await?;
        db.set_preference(
            &format!("desktop-mqa-manifest:{root}"),
            &json!(crate::duplicates::manifest_fingerprint(&indexed)),
        )
        .await?;
        state.progress_for(kind, &format!("MQA audit ready · {}/{} files checked · {reused} saved inspections reused · {inspected} audio inspections", reused + inspected, selected.len()));
        return Ok(json!({"files":selected.len(),"cached_files":rows.len(),"reused":reused,"inspected":inspected}));
    }
    Err(format!(
        "{kind} is not yet available in this build; no changes were made"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn offline_duplicate_checks_keep_saved_results_and_reviews_without_live_quality() {
        let temp = std::env::temp_dir().join(format!("offline-duplicates-{}", uuid::Uuid::new_v4()));
        let root = temp.join("drive");
        let root_str = root.to_str().unwrap();
        let db = TursoDb::open(temp.join("db")).await.unwrap();
        let backend = Arc::new(Backend::new());
        for album in ["Original", "Copy"] {
            let path = root.join(album).join("song.flac");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, crate::stream_download::MINIMAL_FLAC).unwrap();
            // Model a library indexed before audio quality was persisted.
            db.apply_file_update(path.to_str().unwrap(), path.to_str().unwrap(), root_str,
                &json!({"title":"Song","artist":"Artist","album":album,"duration":1.0,"isrc":"TEST1"}), 123, 456).await.unwrap();
        }
        let input = json!({"root":root});
        let initial = execute(&db, &backend, "local_duplicates", &input, Arc::new(AtomicBool::new(false))).await.unwrap();
        assert_eq!(initial["opportunities"], 1);
        let saved = db.get_preference(&format!("desktop-local:{root_str}")).await.unwrap().unwrap();
        std::fs::rename(&root, temp.join("unplugged")).unwrap();
        let reused = execute(&db, &backend, "local_duplicates", &input, Arc::new(AtomicBool::new(false))).await.unwrap();
        assert_eq!(reused["reused"], true);
        assert_eq!(reused["opportunities"], 1);
        let review = execute(&db, &backend, "review_consolidation", &json!({"root":root,"ids":[saved[0]["id"]]}),
            Arc::new(AtomicBool::new(false))).await.unwrap();
        let preview = backend.previews.lock().unwrap()[review["preview_id"].as_str().unwrap()].clone();
        assert_eq!(preview["rows"].as_array().unwrap().len(), 1);
        assert_eq!(preview["rows"][0]["source_tracks"].as_array().unwrap().len(), 1);
        assert_eq!(preview["rows"][0]["keeper_tracks"].as_array().unwrap().len(), 1);
        let forced = execute(&db, &backend, "local_duplicates", &json!({"root":root,"force":true}),
            Arc::new(AtomicBool::new(false))).await.unwrap_err();
        assert!(forced.contains("Reconnect"));
        assert_eq!(db.get_preference(&format!("desktop-local:{root_str}")).await.unwrap(), Some(saved.clone()));

        // Newer cached quality supports a fresh comparison with the drive absent.
        for file in files(&db, root_str).await.unwrap() {
            let mut metadata = file.metadata.unwrap();
            metadata["sample_rate"] = json!(44_100);
            metadata["bit_depth"] = json!(16);
            db.apply_file_update(&file.path, &file.path, root_str, &metadata, file.size, file.mtime).await.unwrap();
        }
        let fresh = execute(&db, &backend, "local_duplicates", &json!({"root":root,"force":true}),
            Arc::new(AtomicBool::new(false))).await.unwrap();
        assert_eq!(fresh["opportunities"], 1);
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[tokio::test]
    async fn duplicate_removed_after_review_or_disconnected_before_trash_keeps_cache() {
        let temp = std::env::temp_dir().join(format!("reviewed-trash-disconnect-{}", uuid::Uuid::new_v4()));
        let root = temp.join("drive");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("song.flac");
        let root_str = root.to_str().unwrap();
        let path_str = path.to_str().unwrap();
        let db = TursoDb::open(temp.join("db")).await.unwrap();
        std::fs::write(&path, crate::stream_download::MINIMAL_FLAC).unwrap();
        db.apply_file_update(path_str, path_str, root_str, &json!({"title":"Song"}), 1, 2).await.unwrap();
        db.save_track_link(path_str, "GB", "[0,0,1,2]", r#"{"status":"linked"}"#).await.unwrap();
        // The reviewed file existed at preflight; the drive vanishes before its turn.
        assert!(std::fs::metadata(&path).unwrap().is_file());
        std::fs::rename(&root, temp.join("unplugged")).unwrap();
        assert!(remove_reviewed_duplicate(&db, root_str, path_str).await.is_err());
        std::fs::rename(temp.join("unplugged"), &root).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(remove_reviewed_duplicate(&db, root_str, path_str).await.is_err());
        assert!(crate::duplicates::trash_file_or_directory(path_str).await.is_err());
        assert_eq!(files(&db, root_str).await.unwrap().len(), 1);
        let conn = db.connect().unwrap();
        let mut rows = conn.query("SELECT COUNT(*) FROM track_links WHERE path=?", (path_str,)).await.unwrap();
        assert_eq!(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(), 1);
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[tokio::test]
    async fn mqa_scope_uses_current_audits_and_only_chosen_market_links() {
        let dir = std::env::temp_dir().join(format!("mqa-selection-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let conn = db.connect().unwrap();
        let root = "/synthetic";
        for (name,present) in [("ready.flac",1),("unlinked.FLAC",1),("clean.flac",1),
            ("changed.flac",1),("new.flac",1),("other.mp3",1),("absent.flac",0)] {
            conn.execute("INSERT INTO local_files(path,root,size,mtime,metadata,present) VALUES(?,?,10,20,'{}',?)",
                (format!("{root}/{name}"),root,present)).await.unwrap();
        }
        for (name,detected,mtime) in [("ready.flac",true,20),("unlinked.FLAC",true,20),
            ("clean.flac",false,20),("changed.flac",true,19),("other.mp3",true,20),
            ("absent.flac",true,20)] {
            db.set_preference(&format!("mqa-audit:{root}/{name}"),
                &json!({"size":10,"mtime":mtime,"result":{"detected":detected,
                    "status":if detected {"MQA signal"} else {"No signal found"}}})).await.unwrap();
        }
        for (name,market) in [("ready.flac","GB"),("unlinked.FLAC","US"),("changed.flac","GB")] {
            db.choose_track_link(&json!({"path":format!("{root}/{name}"),"album_id":"10","track_id":"101","market":market})).await.unwrap();
        }
        let all = mqa_selection(&db,&json!({"root":root,"scope":"all","market":"GB"})).await.unwrap();
        assert_eq!(all["counts"],json!({"all":5,"unscanned":2,"selected":5,"detected":2,"ready":1,"unlinked":1}));
        assert_eq!(all["ready_ids"],json!(["/synthetic/ready.flac"]));
        assert_eq!(all["unlinked_ids"],json!(["/synthetic/unlinked.FLAC"]),"Another market's placement is not a verified replacement source");
        let unscanned = mqa_selection(&db,&json!({"root":root,"scope":"unscanned"})).await.unwrap();
        assert_eq!(unscanned["ids"],json!(["/synthetic/changed.flac","/synthetic/new.flac"]));
        assert_eq!(unscanned["counts"]["selected"],2);
        assert_eq!(unscanned["counts"]["detected"],0,"Changed file audits cannot queue replacements");
        let selected = mqa_selection(&db,&json!({"root":root,"scope":"selected","market":"GB",
            "ids":["/synthetic/ready.flac","/synthetic/unlinked.FLAC","/synthetic/other.mp3","/synthetic/absent.flac","/another/track.flac"]})).await.unwrap();
        assert_eq!(selected["counts"],json!({"selected":2,"detected":2,"ready":1,"unlinked":1}));
        assert_eq!(selected["ids"].as_array().unwrap().len(),2);
        for args in [json!({"root":root,"scope":"selected"}),json!({"root":root,"scope":"selected","ids":[]})] {
            let empty = mqa_selection(&db,&args).await.unwrap();
            assert_eq!(empty["ids"],json!([]),"Empty selected scope must never mean the whole library");
            assert_eq!(empty["counts"]["selected"],0);
        }
        let rows = cached_mqa_rows(&db,&files(&db,root).await.unwrap()).await.unwrap();
        assert_eq!(rows.len(),5);
        assert_eq!(rows.iter().find(|row|row["path"]=="/synthetic/changed.flac").unwrap()["scanned"],false);
        assert_eq!(rows.iter().find(|row|row["path"]=="/synthetic/clean.flac").unwrap()["scanned"],true);
        drop(conn); drop(db); std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn selected_mqa_scan_preserves_unchecked_files_and_rejects_empty_selection() {
        let dir = std::env::temp_dir().join(format!("mqa-selected-audit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<_> = (0..3).map(|index|dir.join(format!("track-{index}.flac"))).collect();
        for path in &paths { std::fs::write(path,crate::stream_download::MINIMAL_FLAC).unwrap(); }
        let before: Vec<_> = paths.iter().map(|path|std::fs::read(path).unwrap()).collect();
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        crate::scanner::scan_library(&db,&dir,cancel.clone(),|_|{}).await.unwrap();
        let root = dir.to_string_lossy();
        let indexed = files(&db,&root).await.unwrap();
        let unchanged = indexed.iter().find(|file|file.path==paths[1].to_string_lossy()).unwrap();
        let saved = json!({"size":unchanged.size,"mtime":unchanged.mtime,
            "result":{"detected":true,"status":"MQA signal","evidence":"Cached inspection"}});
        db.set_preference(&format!("mqa-audit:{}",unchanged.path),&saved).await.unwrap();
        let backend = Arc::new(Backend::new());
        let result = execute(&db,&backend,"mqa",
            &json!({"root":root,"ids":[paths[0]],"force":true}),cancel.clone()).await.unwrap();
        assert_eq!(result["files"],1);
        assert_eq!(result["inspected"],1);
        assert_eq!(result["cached_files"],3);
        assert_eq!(db.get_preference(&format!("mqa-audit:{}",unchanged.path)).await.unwrap(),Some(saved));
        let visible = db.get_preference(&format!("desktop-mqa:{root}")).await.unwrap().unwrap();
        assert_eq!(visible.as_array().unwrap().len(),3,"Selected scans must preserve the full audit view");
        assert_eq!(visible.as_array().unwrap().iter().find(|row|row["id"]==json!(paths[1])).unwrap()["status"],"MQA signal");
        assert_eq!(visible.as_array().unwrap().iter().find(|row|row["id"]==json!(paths[2])).unwrap()["scanned"],false);
        for ids in [json!([]),json!(["/not-indexed.flac"]),json!({"path":"broken"})] {
            assert!(execute(&db,&backend,"mqa",&json!({"root":root,"ids":ids}),cancel.clone()).await.is_err());
        }
        assert_eq!(before,paths.iter().map(|path|std::fs::read(path).unwrap()).collect::<Vec<_>>());
        drop(db); std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn metadata_preparation_reads_only_chosen_ids_in_the_requested_market() {
        let dir=std::env::temp_dir().join(format!("metadata-links-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let conn=db.connect().unwrap();
        let selected=json!({"album_id":"12","track_id":"121"});
        let payload=json!({"ids":selected,"placements":[{"album_id":"13","track_id":"131"}],"catalogue_options":[{"id":"99","track_id":"991"}]});
        for (path,market,raw) in [("/chosen.flac","GB",payload.to_string()),("/chosen.flac","US",json!({"ids":{"album_id":"14","track_id":"141"}}).to_string()),("/other.flac","GB",payload.to_string()),("/ignored.flac","GB",json!({"status":"ignored","ids":selected}).to_string()),("/bad.flac","GB","broken legacy payload".into())] {
            conn.execute("INSERT INTO track_links(path,market,stamp,payload) VALUES(?,?,'',?)",(path,market,raw)).await.unwrap();
        }
        let cancel=AtomicBool::new(false);
        let result=metadata_link_ids(&db,&["/chosen.flac","/bad.flac","/missing.flac","/ignored.flac"],"GB",&cancel).await.unwrap();
        assert_eq!(result.len(),1);
        assert_eq!(result["/chosen.flac"],selected,"Alternative editions must not silently replace the selected recording");
        cancel.store(true,Ordering::Relaxed);
        assert!(metadata_link_ids(&db,&["/chosen.flac"],"GB",&cancel).await.unwrap().is_empty());
        drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    #[ignore = "Explicit local cache benchmark; synthetic records only, no audio or API access"]
    async fn metadata_preparation_benchmark() {
        let dir=std::env::temp_dir().join(format!("metadata-speed-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let conn=db.connect().unwrap();
        let paths: Vec<_>=(0..200).map(|i|format!("/synthetic/Release/{i:02} - Track.flac")).collect();
        let ids=json!({"album_id":"10","track_id":"101"});
        let alternatives: Vec<_>=(10..18).map(|i|json!({"album_id":i.to_string(),"track_id":format!("{i}1")})).collect();
        let link=json!({"ids":ids,"placements":alternatives,"catalogue_options":alternatives});
        conn.execute("BEGIN IMMEDIATE",()).await.unwrap();
        for path in &paths {
            conn.execute("INSERT INTO local_files(path,root,size,mtime,metadata,present) VALUES(?,'/synthetic',1,1,'{}',1)",(path.as_str(),)).await.unwrap();
            conn.execute("INSERT INTO track_links(path,market,stamp,payload) VALUES(?,'GB','',?)",(path.as_str(),link.to_string())).await.unwrap();
        }
        conn.execute("COMMIT",()).await.unwrap();
        let start=std::time::Instant::now();
        for path in &paths {
            assert_eq!(db.get_detail(&json!({"path":path,"market":"GB"})).await.unwrap()["linked_ids"],ids);
        }
        let before=start.elapsed();
        let start=std::time::Instant::now();
        let selected: Vec<_>=paths.iter().map(String::as_str).collect();
        let after=metadata_link_ids(&db,&selected,"GB",&AtomicBool::new(false)).await.unwrap();
        let elapsed=start.elapsed();
        assert_eq!(after.len(),paths.len());
        assert!(after.values().all(|value|value==&ids));
        println!("files={} full_match_view_ms={} chosen_ids_batch_ms={} improvement={:.1}x",paths.len(),before.as_millis(),elapsed.as_millis(),before.as_secs_f64()/elapsed.as_secs_f64());
        drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancelled_metadata_waiter_leaves_the_cache_owner_running() {
        let gate=release_gate(format!("cancel-test:{}",uuid::Uuid::new_v4()));
        let owner=gate.clone().lock_owned().await;
        let cancel=Arc::new(AtomicBool::new(false));
        let worker_cancel=cancel.clone();
        let worker_gate=gate.clone();
        let task=tokio::spawn(async move { release_guard(worker_gate,worker_cancel.as_ref()).await.map(|_|()) });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cancel.store(true,Ordering::Relaxed);
        assert_eq!(tokio::time::timeout(std::time::Duration::from_millis(300),task).await.unwrap().unwrap().unwrap_err(),"Cancelled");
        assert!(gate.try_lock().is_err(),"Cancelling a waiter cannot cancel or unlock the cache owner");
        drop(owner);
        assert!(gate.try_lock().is_ok());
    }

    #[tokio::test]
    async fn mqa_replacement_queue_deduplicates_recordings_and_never_changes_local_audio() {
        let dir=std::env::temp_dir().join(format!("mqa-queue-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<_>=(0..2).map(|index|dir.join(format!("track-{index}.flac"))).collect();
        for path in &paths { std::fs::write(path,crate::stream_download::MINIMAL_FLAC).unwrap(); }
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let cancel=Arc::new(AtomicBool::new(false));
        crate::scanner::scan_library(&db,&dir,cancel.clone(),|_|{}).await.unwrap();
        db.set_preference("account-disconnected",&json!(true)).await.unwrap();
        db.set_preference("tag-review:GB:10",&json!({"id":"10","title":"Release","artist":"Main","tracks_loaded":false,"discovery_checked_at":chrono::Utc::now().timestamp()})).await.unwrap();
        db.set_preference("subscriber-items:GB:10",&json!({"schema":2,"checked_at":chrono::Utc::now().timestamp(),"items":[{"id":101,"title":"Track","trackNumber":1,"volumeNumber":1,"credits":[]}]})).await.unwrap();
        let root=dir.to_string_lossy();
        let indexed=files(&db,&root).await.unwrap();
        for file in &indexed {
            db.choose_track_link(&json!({"path":file.path,"album_id":"10","track_id":"101","market":"GB"})).await.unwrap();
            db.set_preference(&format!("mqa-audit:{}",file.path),&json!({"size":file.size,"mtime":file.mtime,"result":{"detected":true}})).await.unwrap();
        }
        let before: Vec<_>=paths.iter().map(|path|std::fs::read(path).unwrap()).collect();
        let backend=Arc::new(Backend::new());
        let args=json!({"root":root,"ids":paths});
        let result = execute(&db,&backend,"queue_mqa",&args,cancel).await.unwrap();
        assert_eq!(result["releases"],1);
        assert_eq!(result["root"],json!(root));
        assert_eq!(result["queued_paths"],json!(paths));
        let conn=db.connect().unwrap();
        let mut rows=conn.query("SELECT payload FROM queue WHERE id='10'",()).await.unwrap();
        let queued: Value=serde_json::from_str(&rows.next().await.unwrap().unwrap().get::<String>(0).unwrap()).unwrap();
        assert_eq!(queued["selected_tracks"].as_array().unwrap().len(),1);
        assert_eq!(queued["selected_tracks"][0]["id"],"101");
        assert_eq!(queued["redownload"],true,"Replacement downloads must bypass skip-existing");
        assert_eq!(queued["replacement_audit"]["root"],json!(root));
        assert_eq!(queued["replacement_audit"]["source_paths"]["101"].as_array().unwrap().len(),2,"Every approved duplicate copy must have a replacement destination");
        assert!(paths.iter().all(|path|queued["replacement_audit"]["source_paths"]["101"].as_array().unwrap().contains(&json!(path))));
        drop(rows);
        assert_eq!(before,paths.iter().map(|path|std::fs::read(path).unwrap()).collect::<Vec<_>>());
        conn.execute("DELETE FROM queue",()).await.unwrap();
        assert!(execute(&db,&backend,"queue_mqa",&args,Arc::new(AtomicBool::new(true))).await.unwrap_err().contains("Cancelled"));
        let mut rows=conn.query("SELECT COUNT(*) FROM queue",()).await.unwrap();
        assert_eq!(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),0);
        drop(rows);drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn availability_job_reuses_saved_checks_and_old_metadata_cannot_revive_a_withdrawn_release() {
        let dir = std::env::temp_dir().join(format!("availability-job-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        for (id, status) in [("1",json!(true)),("2",json!(false)),("3",Value::Null)] {
            db.set_preference(&format!("release-live:GB:{id}"), &json!({"available":status,"checked_at":now,"source":"album_lookup"})).await.unwrap();
        }
        let backend = Arc::new(Backend::new());
        let cancel = Arc::new(AtomicBool::new(false));
        backend.start_online_job(json!({"id":"availability-test","kind":"check_availability","status":"running"}),cancel.clone());
        let result = execute(&db,&backend,"check_availability",&json!({"ids":["1","2","3","1"]}),cancel).await.unwrap();
        assert_eq!((result["checked"].as_u64(),result["available"].as_u64(),result["unavailable"].as_u64(),result["unknown"].as_u64()), (Some(3),Some(1),Some(1),Some(1)));
        assert_eq!(backend.online_job.lock().unwrap().as_ref().unwrap()["completed"],3);
        let mut old = json!({"id":"2","title":"Release","tracks":[]});
        apply_release_discovery(&mut old,&json!({"data":[{"id":"2","attributes":{"availability":["STREAM"],"checked_at":now-14*86400}}]}),"2");
        assert_eq!(old["availability_checked_at"],now-14*86400);
        publish_release(&db,"2","GB",old).await.unwrap();
        assert_eq!(db.get_preference("release-live:GB:2").await.unwrap().unwrap()["available"],false);
        let mut listed = json!({"id":"2","title":"Release","tracks":[]});
        apply_release_discovery(&mut listed,&json!({"data":[{"id":"2","attributes":{"availability":["STREAM"],"checked_at":now,"availability_source":"artist_list"}}]}),"2");
        assert_eq!(listed["availability_source"],"artist_list");
        publish_release(&db,"2","GB",listed).await.unwrap();
        assert_eq!(db.get_preference("release-live:GB:2").await.unwrap().unwrap()["available"],false,"Metadata enrichment cannot launder a listing into a verified restoration");
        let cancelled = crate::availability::check_with_progress(&db,&["2".into(),"4".into()],"GB",false,Arc::new(AtomicBool::new(true)),|_|{},|_|{}).await;
        assert!(cancelled.is_err());
        assert!(db.get_preference("release-live:GB:4").await.unwrap().is_none());
        assert_eq!(db.get_preference("release-live:GB:2").await.unwrap().unwrap()["available"],false);
        drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn track_enrichment_publishes_parallel_updates_without_replacing_sibling_metadata_or_queue_selection() {
        let dir = std::env::temp_dir().join(format!("track-publish-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let conn = db.connect().unwrap();
        let one = crate::tidal::TidalTrack{id:"101".into(),isrc:Some("ONE".into()),track_number:1,disc_number:1,credits_complete:true,credits:json!([]),..Default::default()};
        let two = crate::tidal::TidalTrack{id:"102".into(),isrc:Some("TWO".into()),track_number:2,disc_number:1,credits_complete:true,credits:json!([]),..Default::default()};
        let release = json!({"id":"10","title":"Release","artist":"Main","tracks_loaded":true,"tracks":[one,two]});
        for (artist,market) in [("4","GB"),("5","GB"),("4","US")] {
            conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES(?,?,?)",(artist,market,json!({"id":artist,"name":"Main","releases":[release]}).to_string())).await.unwrap();
        }
        conn.execute("INSERT INTO queue(id,payload,approved,decision) VALUES('10',?,1,'queued')",(json!({"id":"10","selected_track_ids":["101"],"destination":"keep"}).to_string(),)).await.unwrap();
        publish_release(&db,"10","GB",release.clone()).await.unwrap();
        let mut first = one.clone(); first.bpm = Some(124.); first.key = Some("G".into());
        let mut second = two.clone(); second.copyright = Some("Label".into()); second.credits = json!([{"name":"Writer","role":"Composer"}]);
        let (a,b) = tokio::join!(publish_track_enrichment(&db,"10","GB",&first),publish_track_enrichment(&db,"10","GB",&second));
        a.unwrap(); b.unwrap();
        let canonical = db.get_preference("tag-review:GB:10").await.unwrap().unwrap();
        assert_eq!(canonical["tracks"][0]["bpm"],124.);
        assert_eq!(canonical["tracks"][1]["copyright"],"Label");
        assert_eq!(canonical["tracks"][1]["credits"],second.credits);
        let mut rows = conn.query("SELECT market,payload FROM catalogue",()).await.unwrap();
        while let Some(row) = rows.next().await.unwrap() {
            let market:String = row.get(0).unwrap();
            let value:Value = serde_json::from_str(&row.get::<String>(1).unwrap()).unwrap();
            assert_eq!(value["releases"][0]["tracks"],if market == "GB" {canonical["tracks"].clone()} else {release["tracks"].clone()});
        }
        drop(rows);
        let mut rows = conn.query("SELECT payload,approved FROM queue WHERE id='10'",()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let queued:Value = serde_json::from_str(&row.get::<String>(0).unwrap()).unwrap();
        assert_eq!(queued["tracks"],canonical["tracks"]); assert_eq!(queued["selected_track_ids"],json!(["101"]));
        assert_eq!(queued["destination"],"keep"); assert_eq!(row.get::<i64>(1).unwrap(),1);
        drop(rows);
        first.isrc = Some("OTHER".into());
        assert!(publish_track_enrichment(&db,"10","GB",&first).await.is_err());
        assert_eq!(db.get_preference("tag-review:GB:10").await.unwrap().unwrap(),canonical);
        drop(conn); drop(db); std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_tag_dj_requests_only_fill_needed_fields() {
        let track=crate::tidal::TidalTrack {bpm:Some(124.),..Default::default()};
        let local=HashMap::from([("tkey".into(),"8A".into())]);
        assert!(!DjMetadataNeeds::from_tags(&local).missing_from(&track),"A missing remote key cannot change an existing local key");
        assert!(DjMetadataNeeds::from_tags(&HashMap::new()).missing_from(&track));
        let complete=HashMap::from([("tempo".into(),"124".into()),("initialkey".into(),"8A".into())]);
        assert!(!DjMetadataNeeds::from_tags(&complete).missing_from(&crate::tidal::TidalTrack::default()));
    }

    #[tokio::test]
    async fn metadata_release_preparation_reuses_cache_and_only_checks_requested_recordings() {
        let dir=std::env::temp_dir().join(format!("metadata-release-warm-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let summary=json!({"id":10,"title":"Release","numberOfTracks":3,"numberOfVolumes":1,"artists":[{"id":1,"name":"Artist"}]});
        let now=chrono::Utc::now().timestamp();
        db.set_preference("subscriber-summary:GB:10",&summary).await.unwrap();
        let tracks: Vec<_>=(101..104).map(|id|crate::tidal::TidalTrack {id:id.to_string(),track_number:id-100,disc_number:1,credits_complete:true,credits:json!([]),..Default::default()}).collect();
        let release=json!({"id":"10","artist":"Artist","title":"Release","discovery_checked_at":now,"tracks_loaded":true,"track_metadata_source":"subscriber","tracks":tracks,"recommendation_snapshot":{"schema":RECOMMENDATION_SCHEMA,"summary_fingerprint":crate::tidal::summary_fingerprint(&summary),"optional_status":"complete","optional_schema":crate::tidal::DISCOVERY_SCHEMA,"optional_valid_until":now+3600}});
        db.set_preference("tag-review:GB:10",&release).await.unwrap();
        db.set_preference("dj-check:GB:101",&json!({"id":"101","status":"complete","bpm":124.,"key":"G","checked_at":now})).await.unwrap();
        // 102 belongs to this album but is not selected. 103 already has local
        // DJ tags. Neither should create an optional request or retry cache.
        let selected=HashMap::from([("101".into(),DjMetadataNeeds {bpm:true,key:true}),("103".into(),DjMetadataNeeds::default())]);
        let albums=HashMap::from([("10".into(),selected)]);
        let http=reqwest::Client::new();
        for _ in 0..2 {
            let prepared=prepare_metadata_releases(&db,&http,albums.clone(),"GB",Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
            assert_eq!(prepared.len(),1);
            assert_eq!(prepared["10"].release.tracks[0].bpm,Some(124.));
            assert_eq!(prepared["10"].release.tracks[0].key.as_deref(),Some("G"));
            assert!(prepared["10"].dj_notes.is_empty());
            assert!(db.get_preference("dj-check:GB:102").await.unwrap().is_none());
            assert!(db.get_preference("dj-check:GB:103").await.unwrap().is_none());
        }
        let cancel=Arc::new(AtomicBool::new(true));
        assert!(prepare_metadata_releases(&db,&http,albums,"GB",cancel,|_|{}).await.is_err());
        assert_eq!(db.get_preference("tag-review:GB:10").await.unwrap().unwrap()["tracks"][0]["bpm"],124.);
        drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancelling_parallel_metadata_preparation_releases_waiters_without_touching_owner() {
        let dir=std::env::temp_dir().join(format!("metadata-preparation-cancel-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let summary=json!({"id":10,"title":"Release","numberOfTracks":1});
        db.set_preference("subscriber-summary:GB:10",&summary).await.unwrap();
        db.set_preference("tag-review:GB:10",&json!({"id":"10","artist":"Artist","title":"Release","tracks_loaded":true,"track_metadata_source":"subscriber","tracks":[{"id":"101","title":"Track","track_number":1,"disc_number":1,"credits_complete":true,"credits":[]}],"recommendation_snapshot":{"schema":RECOMMENDATION_SCHEMA,"summary_fingerprint":crate::tidal::summary_fingerprint(&summary),"optional_status":"complete","optional_schema":crate::tidal::DISCOVERY_SCHEMA,"optional_valid_until":chrono::Utc::now().timestamp()+3600}})).await.unwrap();
        let gate=release_gate(format!("dj:{}:GB:101",db.path.display()));
        let owner=gate.clone().lock_owned().await;
        let cancel=Arc::new(AtomicBool::new(false));
        let worker_db=db.clone();let worker_cancel=cancel.clone();
        let worker=tokio::spawn(async move {
            prepare_metadata_releases(&worker_db,&reqwest::Client::new(),HashMap::from([("10".into(),HashMap::from([("101".into(),DjMetadataNeeds {bpm:true,key:true})]))]),"GB",worker_cancel,|_|{}).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.store(true,Ordering::Relaxed);
        let result=tokio::time::timeout(std::time::Duration::from_secs(1),worker).await.expect("Cancellation must not wait for another worker's DJ lookup").unwrap();
        assert!(result.is_err());
        assert!(gate.try_lock().is_err(),"Other worker still owns its cache fill");
        assert!(db.get_preference("dj-check:GB:101").await.unwrap().is_none());
        drop(owner);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn missing_tag_preparation_recovers_explicit_empty_dj_checks_from_legacy_raw_cache() {
        let dir=std::env::temp_dir().join(format!("legacy-raw-dj-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let summary=json!({"id":10,"title":"Release","numberOfTracks":1});
        let fingerprint=crate::tidal::summary_fingerprint(&summary);
        db.set_preference("subscriber-summary:GB:10",&summary).await.unwrap();
        db.set_preference("tag-review:GB:10",&json!({"id":"10","artist":"Artist","title":"Release","tracks_loaded":true,"track_metadata_source":"subscriber","tracks":[{"id":"101","title":"Track","track_number":1,"disc_number":1,"credits_complete":true,"credits":[],"bpm":null,"key":null}],"recommendation_snapshot":{"schema":RECOMMENDATION_SCHEMA,"summary_fingerprint":fingerprint,"optional_status":"complete","optional_schema":crate::tidal::DISCOVERY_SCHEMA,"optional_valid_until":chrono::Utc::now().timestamp()+3600}})).await.unwrap();
        db.set_preference("subscriber-items:GB:10",&json!({"schema":2,"summary_fingerprint":fingerprint,"checked_at":chrono::Utc::now().timestamp(),"items":[{"id":101,"title":"Track","trackNumber":1,"volumeNumber":1,"credits":[],"bpm":null,"key":null}]})).await.unwrap();
        let selected=HashMap::from([("10".into(),HashMap::from([("101".into(),DjMetadataNeeds {bpm:true,key:true})]))]);
        let progress=std::sync::Mutex::new(Vec::new());
        let prepared=prepare_metadata_releases(&db,&reqwest::Client::new(),selected,"GB",Arc::new(AtomicBool::new(false)),|message|progress.lock().unwrap().push(message.to_owned())).await.unwrap();
        let track=&prepared["10"].release.tracks[0];
        assert!(track.bpm_checked && track.key_checked);
        assert!(track.bpm.is_none() && track.key.is_none());
        assert!(prepared["10"].dj_notes.is_empty());
        assert!(db.get_preference("dj-check:GB:101").await.unwrap().is_none(),"An explicit checked-empty observation is sufficient without a recording GET");
        assert!(db.get_preference("pending-catalogue:GB:10").await.unwrap().is_none(),"Normal preparation flushes all durable staged markers");
        assert_eq!(db.get_preference("tag-review:GB:10").await.unwrap().unwrap()["tracks"][0]["bpm_checked"],true);
        assert!(progress.lock().unwrap().last().unwrap().contains("2/2 checks"));
        drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    #[ignore = "Bounded live missing-tag preparation benchmark; reads saved account, one release and at most six DJ lookups"]
    async fn live_metadata_preparation_warm_cache_benchmark() {
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {fn drop(&mut self) {let _=std::fs::remove_dir_all(&self.0);}}
        let dir=std::env::temp_dir().join(format!("metadata-live-warm-{}",uuid::Uuid::new_v4()));
        let _cleanup=Cleanup(dir.clone());
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let mut token=crate::stream_download::load_saved_token(&db).await.expect("Sign in before this explicit live benchmark");
        assert!(token.expires_at > chrono::Utc::now().timestamp() as f64 + 600.,"Saved token needs ten minutes remaining");
        token.refresh_token=None;
        db.set_preference("tidal_token",&json!(token)).await.unwrap();
        let http=crate::network::client(20).unwrap();
        let cancel=Arc::new(AtomicBool::new(false));
        // Load the real credited album once to identify a bounded selection;
        // this is also the normal shared-cache input from a release refresh.
        let start=std::time::Instant::now();
        let before=crate::network::request_count();
        let raw=release_with_cancel(&db,"140303440","GB",false,cancel.clone()).await.unwrap();
        let release:crate::tidal::TidalRelease=serde_json::from_value(raw).unwrap();
        let source_gets=crate::network::request_count()-before;
        let source_ms=start.elapsed().as_millis();
        let selected:HashMap<_,_>=release.tracks.iter().filter(|track|track.bpm.is_none() || track.key.is_none()).take(6)
            .map(|track|(track.id.clone(),DjMetadataNeeds {bpm:true,key:true})).collect();
        assert!(!selected.is_empty(),"Fixture no longer has optional DJ gaps; choose another bounded release");
        let selected_count=selected.len();
        let albums=HashMap::from([("140303440".into(),selected)]);
        let start=std::time::Instant::now();
        let before=crate::network::request_count();
        let first=prepare_metadata_releases(&db,&http,albums.clone(),"GB",cancel.clone(),|_|{}).await.unwrap();
        let cold_gets=crate::network::request_count()-before;
        let cold_ms=start.elapsed().as_millis();
        assert!(first["140303440"].dj_notes.is_empty(),"Live DJ checks must finish without a retry");
        assert!(first["140303440"].release.tracks.iter().all(|track|track.credits_complete));
        for original in &release.tracks {
            let current=first["140303440"].release.tracks.iter().find(|track|track.id==original.id).unwrap();
            assert_eq!(current.isrc,original.isrc);
            for credit in original.credits.as_array().into_iter().flatten() {assert!(current.credits.as_array().unwrap().contains(credit));}
            if original.bpm.is_some() {assert_eq!(current.bpm,original.bpm);}
            if original.key.is_some() {assert_eq!(current.key,original.key);}
        }
        let credited=db.get_preference("subscriber-items:GB:140303440").await.unwrap().unwrap();
        let mut explicit_empty_fields=0;
        let mut explicit_field_differences=0;
        for id in albums["140303440"].keys() {
            let individual=check_dj_metadata(&db,&http,id,"GB",cancel.clone()).await.unwrap();
            let item=credited["items"].as_array().unwrap().iter().find(|item|crate::tidal::resource_id(&item["id"]) == *id).unwrap();
            assert_eq!(individual["isrc"],item["isrc"]);
            for field in ["bpm","key","keyScale"] {
                if item.get(field).is_some() {
                    if item[field].is_null() {explicit_empty_fields+=1;}
                    if item[field] != individual[field] {explicit_field_differences+=1;}
                }
            }
        }
        println!("metadata_live album_item_vs_individual selected_tracks={selected_count} explicit_empty_fields={explicit_empty_fields} explicit_field_differences={explicit_field_differences}");
        assert_eq!(explicit_field_differences,0,"Album credited-item DJ observations differ from individual endpoint; do not infer negative-cache equivalence");
        let start=std::time::Instant::now();
        let before=crate::network::request_count();
        let second=prepare_metadata_releases(&db,&http,albums,"GB",cancel,|_|{}).await.unwrap();
        let warm_gets=crate::network::request_count()-before;
        let warm_ms=start.elapsed().as_millis();
        assert_eq!(warm_gets,0,"A complete missing-tag repeat must make zero GETs");
        assert_eq!(json!(first["140303440"].release),json!(second["140303440"].release));
        println!("metadata_live selected_tracks={selected_count} source_ms={source_ms} source_gets={source_gets} first_dj_ms={cold_ms} first_dj_gets={cold_gets} warm_ms={warm_ms} warm_gets={warm_gets}");
    }

    #[tokio::test]
    async fn staged_catalogue_publication_batches_artists_and_preserves_newer_metadata() {
        let dir=std::env::temp_dir().join(format!("catalogue-publication-batch-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let conn=db.connect().unwrap();
        let old:Vec<_>=(10..22).map(|id|json!({"id":id.to_string(),"title":"Before","tracks":[]})).collect();
        for market in ["GB","US"] {
            conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES('1',?,?)",(market,json!({"id":"1","name":"Artist","releases":old}).to_string())).await.unwrap();
        }
        conn.execute("CREATE TABLE writes(count INTEGER)",()).await.unwrap();
        conn.execute("INSERT INTO writes VALUES(0)",()).await.unwrap();
        conn.execute("CREATE TRIGGER count_catalogue_writes AFTER UPDATE ON catalogue BEGIN UPDATE writes SET count=count+1; END",()).await.unwrap();
        let ids:Vec<_>=(10..22).map(|id|id.to_string()).collect();
        db.set_preference("tag-review:GB:10",&json!({"id":"10","title":"Stale canonical","tracks":[]})).await.unwrap();
        publish_catalogue_releases(&db,&ids,"GB").await.unwrap();
        let mut rows=conn.query("SELECT count FROM writes",()).await.unwrap();
        assert_eq!(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),0,"Never publish a release that failed before staging");drop(rows);
        for id in &ids {
            publish_release_update(&db,id,"GB",json!({"id":id,"title":"After","tracks":[]}),None,false).await.unwrap();
        }
        let mut rows=conn.query("SELECT count FROM writes",()).await.unwrap();
        assert_eq!(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),0);drop(rows);
        // Another workflow adds a field after staging. Flush must use that
        // latest canonical value, not the job's older in-memory release.
        db.set_preference("tag-review:GB:10",&json!({"id":"10","title":"After","label":"Newer label","tracks":[]})).await.unwrap();
        recover_catalogue_publications(&db).await.unwrap();
        publish_catalogue_releases(&db,&ids,"GB").await.unwrap();
        assert!(db.get_preference("pending-catalogue:GB:10").await.unwrap().is_none());
        let mut rows=conn.query("SELECT count FROM writes",()).await.unwrap();
        assert_eq!(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),1,"One containing artist rewritten once; repeat is a no-op");drop(rows);
        let mut rows=conn.query("SELECT market,payload FROM catalogue",()).await.unwrap();
        while let Some(row)=rows.next().await.unwrap() {
            let market:String=row.get(0).unwrap();
            let value:Value=serde_json::from_str(&row.get::<String>(1).unwrap()).unwrap();
            assert_eq!(value["releases"][0]["title"],if market == "GB" {"After"} else {"Before"});
            if market == "GB" {assert_eq!(value["releases"][0]["label"],"Newer label");}
        }
        drop(rows);drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn deferred_catalogue_does_not_overwrite_a_newer_authoritative_summary() {
        let dir=std::env::temp_dir().join(format!("catalogue-newer-summary-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let conn=db.connect().unwrap();
        let old_summary=json!({"id":10,"title":"Earlier","numberOfTracks":1});
        let current_summary=json!({"id":10,"title":"Current","numberOfTracks":2});
        conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES('1','GB',?)",(json!({"id":"1","releases":[{"id":"10","title":"Current","tracks":[]}]}).to_string(),)).await.unwrap();
        publish_release_update(&db,"10","GB",json!({"id":"10","title":"Earlier","tracks":[],"recommendation_snapshot":{"summary_fingerprint":crate::tidal::summary_fingerprint(&old_summary)}}),None,false).await.unwrap();
        db.set_preference("subscriber-summary:GB:10",&current_summary).await.unwrap();
        recover_catalogue_publications(&db).await.unwrap();
        let mut rows=conn.query("SELECT payload FROM catalogue WHERE artist_id='1' AND market='GB'",()).await.unwrap();
        let value:Value=serde_json::from_str(&rows.next().await.unwrap().unwrap().get::<String>(0).unwrap()).unwrap();
        assert_eq!(value["releases"][0]["title"],"Current");
        assert!(db.get_preference("pending-catalogue:GB:10").await.unwrap().is_none());
        drop(rows);drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn metadata_account_failures_stop_remaining_requests() {
        for error in ["HTTP status client error (401 Unauthorized)","HTTP 403 Forbidden","429 Too Many Requests","Service cooldown · retry later","Tidal login token expired and no refresh token available","Token refresh request failed: HTTP 400 invalid_grant","Token refresh failed: invalid_grant"] {assert!(metadata_service_paused(error));}
        assert!(!metadata_service_paused("HTTP status client error (404 Not Found) for track 401234"));
        assert!(!metadata_service_paused("error decoding response body"));
    }

    #[tokio::test]
    #[ignore = "Explicit synthetic catalogue publication benchmark; no audio files or API"]
    async fn catalogue_publication_benchmark() {
        let dir=std::env::temp_dir().join(format!("catalogue-publish-speed-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        let conn=db.connect().unwrap();
        let releases:Vec<_>=(1..=100).map(|id|json!({"id":id.to_string(),"title":"Release","tracks":(0..20).map(|track|json!({"id":format!("{id}{track:02}"),"title":"Recording","credits":[{"role":"Composer","name":"Composer Name"}]})).collect::<Vec<_>>()})).collect();
        conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES('1','GB',?)",(json!({"id":"1","releases":releases}).to_string(),)).await.unwrap();
        let start=std::time::Instant::now();
        for mut value in releases.clone() { let id=value["id"].as_str().unwrap().to_owned(); value["label"]=json!("First"); publish_release(&db,&id,"GB",value).await.unwrap(); }
        let sequential=start.elapsed();
        let start=std::time::Instant::now();
        let mut ids=Vec::new();
        for mut value in releases { let id=value["id"].as_str().unwrap().to_owned(); value["label"]=json!("Second"); publish_release_update(&db,&id,"GB",value,None,false).await.unwrap(); ids.push(id); }
        publish_catalogue_releases(&db,&ids,"GB").await.unwrap();
        let batched=start.elapsed();
        println!("catalogue_publish releases=100 tracks=2000 immediate_ms={} staged_batch_ms={} speedup={:.2}x",sequential.as_millis(),batched.as_millis(),sequential.as_secs_f64()/batched.as_secs_f64());
        drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn dj_cache_keeps_successful_absence_and_bounds_failed_retries() {
        let dir = std::env::temp_dir().join(format!("dj-cache-state-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        let absent = json!({"id":101,"isrc":"ONE","status":"complete","bpm":null,"key":null,"checked_at":1});
        db.set_preference("dj-check:GB:101",&absent).await.unwrap();
        assert_eq!(check_dj_metadata(&db,&reqwest::Client::new(),"101","GB",Arc::new(AtomicBool::new(false))).await.unwrap(),absent);
        assert!(db.get_preference("dj-check:US:101").await.unwrap().is_none());
        assert!(dj_check_reusable(&json!({"status":"retry","retry_after":now+10}),now));
        assert!(!dj_check_reusable(&json!({"status":"retry","retry_after":now-1}),now));
        assert!(!dj_check_reusable(&Value::Null,now));
        drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn selected_release_tracks_replace_stale_snapshot_and_refresh_missing_rows() {
        let dir = std::env::temp_dir().join(format!("selected-release-tracks-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        db.set_preference("account-disconnected",&json!(true)).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        let old_summary = json!({"id":10,"title":"Release","artist":{"id":4,"name":"Main"},"releaseDate":"2020-01-01","type":"ALBUM","numberOfTracks":1,"checked_at":now-100});
        let mut summary = old_summary.clone(); summary["numberOfTracks"]=json!(2); summary["checked_at"]=json!(now);
        let fingerprint = crate::tidal::summary_fingerprint(&summary);
        let old_fingerprint = crate::tidal::summary_fingerprint(&old_summary);
        let old = json!({"id":"10","artist":"Main","title":"Release","date":"2020-01-01","type":"ALBUM","tracks_loaded":true,"track_count":1,"summary_fingerprint":old_fingerprint,
            "recommendation_snapshot":{"schema":RECOMMENDATION_SCHEMA,"summary_fingerprint":old_fingerprint,"optional_status":"complete"},
            "tracks":[{"id":"101","title":"Old track name","track_number":1,"disc_number":1,"isrc":"ONE","bpm":124.0,"credits_complete":true,"credits":[]}]});
        let listed = serde_json::to_value(crate::tidal::release_from_subscriber(&summary,"4")).unwrap();
        let raw = json!({"schema":2,"checked_at":now,"summary_fingerprint":fingerprint,"items":[
            {"id":101,"title":"Current track name","trackNumber":1,"volumeNumber":1,"isrc":"ONE","credits":[]},
            {"id":102,"title":"New second track","trackNumber":2,"volumeNumber":1,"isrc":"TWO","credits":[]}]});
        db.set_preference("subscriber-summary:GB:10",&summary).await.unwrap();
        db.set_preference("tag-review:GB:10",&old).await.unwrap();
        let mut stale_raw=raw.clone(); stale_raw["summary_fingerprint"]=json!(old_fingerprint); stale_raw["items"].as_array_mut().unwrap().pop();
        db.set_preference("subscriber-items:GB:10",&stale_raw).await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO mappings(artist,tidal_id,status) VALUES('Main','4','confirmed')",()).await.unwrap();
        conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES('4','GB',?)",(json!({"id":"4","name":"Main","releases":[listed]}).to_string(),)).await.unwrap();
        conn.execute("INSERT INTO queue(id,payload,approved,decision) VALUES('10',?,1,'queued')",(json!({"id":"10","selected_track_ids":["101"],"destination":"keep","tracks_loaded":true,"track_count":1,"tracks":old["tracks"]}).to_string(),)).await.unwrap();
        let before = db.get_missing_rows("GB",None,None,None,None,None,None,None,0,10).await.unwrap();
        assert_eq!(before.rows.len(),1);
        assert!(!before.rows[0].expanded_available);
        assert!(before.rows[0].children.is_empty());

        // A failed request must not claim that the rejected old cache is ready,
        // nor replace the canonical or catalogue snapshots with partial data.
        assert!(ensure_release_tracks(&db,"10","GB",Arc::new(AtomicBool::new(false))).await.is_err());
        assert_eq!(db.get_preference("tag-review:GB:10").await.unwrap().unwrap(),old);
        assert_eq!(db.cached_catalogue_release("10","GB").await.unwrap().unwrap(),listed);
        let after_failure = db.get_missing_rows("GB",None,None,None,None,None,None,None,0,10).await.unwrap();
        assert!(after_failure.rows[0].children.is_empty());

        // Current credited data can already be cached by another workflow.
        // It must be reused offline, even though the canonical snapshot is old.
        db.set_preference("subscriber-items:GB:10",&raw).await.unwrap();
        let loaded = ensure_release_tracks(&db,"10","GB",Arc::new(AtomicBool::new(false))).await.unwrap();
        assert_eq!(loaded["summary_fingerprint"],fingerprint);
        assert_eq!(loaded["recommendation_snapshot"]["summary_fingerprint"],fingerprint);
        assert!(loaded["recommendation_snapshot"]["optional_status"].is_null());
        assert_eq!(loaded["tracks"][0]["title"],"Current track name");
        assert_eq!(loaded["tracks"][0]["bpm"],124.0);
        assert_eq!(loaded["track_count"],2);
        let updated = db.get_missing_rows("GB",None,None,None,None,None,None,None,0,10).await.unwrap();
        assert!(updated.revision > before.revision);
        assert!(updated.rows[0].expanded_available);
        assert_eq!(updated.rows[0].tracks,2);
        assert_eq!(updated.rows[0].children.iter().map(|track|track.id.as_str()).collect::<Vec<_>>(),vec!["101","102"]);
        let mut rows=conn.query("SELECT payload,approved FROM queue WHERE id='10'",()).await.unwrap();
        let row=rows.next().await.unwrap().unwrap();
        let queued:Value=serde_json::from_str(&row.get::<String>(0).unwrap()).unwrap();
        assert_eq!(queued["selected_track_ids"],json!(["101"]));
        assert_eq!(queued["destination"],"keep");
        assert_eq!(row.get::<i64>(1).unwrap(),1);
        assert_eq!(queued["tracks"],loaded["tracks"]);
        assert_eq!(queued["track_count"],2);
        drop(rows);
        let warm = ensure_release_tracks(&db,"10","GB",Arc::new(AtomicBool::new(false))).await.unwrap();
        assert_eq!(warm["tracks"],loaded["tracks"]);
        assert_eq!(db.revision.load(Ordering::SeqCst),updated.revision);
        drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn selected_release_tracks_check_listing_when_shared_summary_is_missing_or_stale() {
        let dir = std::env::temp_dir().join(format!("selected-release-summary-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        db.set_preference("account-disconnected",&json!(true)).await.unwrap();
        let old_summary=json!({"id":10,"title":"Earlier edition","numberOfTracks":1,"artist":{"id":4,"name":"Main"}});
        let current_summary=json!({"id":10,"title":"Current edition","numberOfTracks":2,"artist":{"id":4,"name":"Main"}});
        let old=json!({"id":"10","title":"Earlier edition","track_count":1,"tracks_loaded":true,"summary_fingerprint":crate::tidal::summary_fingerprint(&old_summary),"tracks":[{"id":"101","title":"Old track"}]});
        let listed=serde_json::to_value(crate::tidal::release_from_subscriber(&current_summary,"4")).unwrap();
        db.set_preference("tag-review:GB:10",&old).await.unwrap();
        let conn=db.connect().unwrap();
        conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES('4','GB',?)",(json!({"id":"4","name":"Main","releases":[listed]}).to_string(),)).await.unwrap();
        for saved_summary in [None,Some(&old_summary)] {
            if let Some(summary)=saved_summary {db.set_preference("subscriber-summary:GB:10",summary).await.unwrap();}
            assert!(ensure_release_tracks(&db,"10","GB",Arc::new(AtomicBool::new(false))).await.is_err(),"A listing conflict requires a current check, even without a current shared summary");
            assert_eq!(db.get_preference("tag-review:GB:10").await.unwrap().unwrap(),old);
            assert_eq!(db.cached_catalogue_release("10","GB").await.unwrap().unwrap(),listed);
        }
        drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn disconnected_release_publishes_complete_credited_cache_without_inventing_summary_pairing() {
        let dir = std::env::temp_dir().join(format!("offline-release-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        db.set_preference("account-disconnected",&json!(true)).await.unwrap();
        let shell = json!({"id":"10","artist":"Main","title":"Release","tracks_loaded":false,"discovery_checked_at":chrono::Utc::now().timestamp()});
        db.set_preference("tag-review:GB:10",&shell).await.unwrap();
        let raw = json!({"schema":2,"checked_at":chrono::Utc::now().timestamp(),"fingerprint_status":"unbound","items":[{"id":101,"title":"Song","isrc":"ONE","trackNumber":1,"volumeNumber":1,"credits":[{"type":"Composer","contributors":[{"name":"Writer"}]}],"bpm":null,"key":null}]});
        db.set_preference("subscriber-items:GB:10",&raw).await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES('4','GB',?)",(json!({"id":"4","name":"Main","releases":[shell]}).to_string(),)).await.unwrap();
        conn.execute("INSERT INTO queue(id,payload,approved,decision) VALUES('10',?,1,'queued')",(json!({"id":"10","selected_track_ids":["101"]}).to_string(),)).await.unwrap();
        let published = release(&db,"10","GB",false).await.unwrap();
        assert_eq!(published["tracks_loaded"],true);
        assert_eq!(published["tracks"][0]["credits_complete"],true);
        assert_eq!(published["tracks"][0]["credits"][0]["name"],"Writer");
        assert!(published["recommendation_snapshot"].is_null(),"An unpaired cache must not invent a matching summary");
        assert_eq!(db.get_preference("subscriber-items:GB:10").await.unwrap().unwrap(),raw);
        assert_eq!(release(&db,"10","GB",false).await.unwrap()["tracks"],published["tracks"]);
        let mut rows = conn.query("SELECT payload FROM catalogue WHERE artist_id='4' AND market='GB'",()).await.unwrap();
        let catalogue: Value = serde_json::from_str(&rows.next().await.unwrap().unwrap().get::<String>(0).unwrap()).unwrap();
        assert_eq!(catalogue["releases"][0]["tracks"],published["tracks"]);
        drop(rows);
        let mut rows = conn.query("SELECT payload FROM queue WHERE id='10'",()).await.unwrap();
        let queue: Value = serde_json::from_str(&rows.next().await.unwrap().unwrap().get::<String>(0).unwrap()).unwrap();
        assert_eq!(queue["tracks"],published["tracks"]);
        assert_eq!(queue["selected_track_ids"],json!(["101"]));
        drop(rows);
        // An explicitly paired, old cache is reusable offline despite its age.
        let summary = json!({"id":10,"title":"Release","artist":{"id":4,"name":"Main"},"numberOfTracks":1,"checked_at":1});
        let mut paired = raw.clone();
        paired["checked_at"] = json!(2);
        paired["summary_fingerprint"] = json!(crate::tidal::summary_fingerprint(&summary));
        db.set_preference("subscriber-items:GB:10",&paired).await.unwrap();
        db.set_preference("subscriber-summary:GB:10",&summary).await.unwrap();
        db.set_preference("tag-review:GB:10",&shell).await.unwrap();
        let paired_release = release(&db,"10","GB",false).await.unwrap();
        assert_eq!(paired_release["tracks"][0]["id"],published["tracks"][0]["id"]);
        assert_eq!(paired_release["tracks"][0]["credits"],published["tracks"][0]["credits"]);
        assert_eq!(paired_release["recommendation_snapshot"]["summary_fingerprint"],paired["summary_fingerprint"]);
        assert_eq!(paired_release["recommendation_snapshot"]["optional_status"],"retry");
        drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

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
        let saved=json!({"id":"10","tracks_loaded":true,"track_metadata_source":"subscriber","track_metadata_checked_at":1,"recommendation_snapshot":{"schema":RECOMMENDATION_SCHEMA,"summary_fingerprint":fingerprint,"optional_status":"complete","optional_schema":crate::tidal::DISCOVERY_SCHEMA,"optional_valid_until":chrono::Utc::now().timestamp()+3600},"tracks":[{"id":"101","track_number":1,"disc_number":1,"credits_complete":true,"credits":[],"bpm":null,"key":null}]});
        db.set_preference("tag-review:GB:10",&saved).await.unwrap();
        let release=crate::tidal::TidalRelease{id:"10".into(),summary_fingerprint:Some(fingerprint.clone()),..Default::default()};
        let new=crate::tidal::TidalRelease{id:"11".into(),summary_fingerprint:Some("new".into()),..Default::default()};
        let (reused,changed,_)=recommendation_refresh_plan(&db,"GB",&[release.clone(),new]).await.unwrap();
        assert_eq!(reused,HashSet::from(["10".into()])); assert!(changed.is_empty());
        let mut legacy=saved.clone();legacy["recommendation_snapshot"]["optional_schema"]=Value::Null;
        db.set_preference("tag-review:GB:10",&legacy).await.unwrap();
        let (reused,_,cached_tracks)=recommendation_refresh_plan(&db,"GB",&[release.clone()]).await.unwrap();
        assert!(reused.is_empty());assert!(cached_tracks.contains("10"),"Optional genre/provider upgrade must not refetch complete credits");
        legacy["recommendation_snapshot"]["optional_schema"]=json!(crate::tidal::DISCOVERY_SCHEMA);
        legacy["recommendation_snapshot"]["optional_valid_until"]=json!(1);
        db.set_preference("tag-review:GB:10",&legacy).await.unwrap();
        let (reused,_,cached_tracks)=recommendation_refresh_plan(&db,"GB",&[release.clone()]).await.unwrap();
        assert!(reused.is_empty());assert!(cached_tracks.contains("10"),"Expired optional fields refresh without another credited track-list fetch");
        db.set_preference("tag-review:GB:10",&saved).await.unwrap();
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
        db.set_preference("subscriber-discovery:GB:10",&json!({"schema":crate::tidal::DISCOVERY_SCHEMA,"checked_at":chrono::Utc::now().timestamp(),"valid_until":chrono::Utc::now().timestamp()+3600,"genres":[],"replacement_id":null})).await.unwrap();
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

    #[tokio::test]
    async fn local_duplicate_and_mqa_jobs_fill_gaps_and_allow_forced_rechecks() {
        let temp = std::env::temp_dir().join(format!("tibrary-local-reuse-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let path = temp.join("track.flac");
        std::fs::write(&path, crate::stream_download::MINIMAL_FLAC).unwrap();
        let db = TursoDb::open(temp.join("library.db")).await.unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        crate::scanner::scan_library(&db, &temp, cancel.clone(), |_| {}).await.unwrap();
        let backend = Arc::new(Backend::new());
        let args = json!({"root":temp});
        let first = execute(&db, &backend, "local_duplicates", &args, cancel.clone()).await.unwrap();
        assert_ne!(first["reused"], true);
        let repeated = execute(&db, &backend, "local_duplicates", &args, cancel.clone()).await.unwrap();
        assert_eq!(repeated["reused"], true);
        let forced = execute(&db, &backend, "local_duplicates", &json!({"root":temp,"force":true}), cancel.clone()).await.unwrap();
        assert_ne!(forced["reused"], true);
        let first = execute(&db, &backend, "mqa", &args, cancel.clone()).await.unwrap();
        assert_eq!(first["inspected"], 1);
        let repeated = execute(&db, &backend, "mqa", &args, cancel.clone()).await.unwrap();
        assert_eq!(repeated["inspected"], 0);
        assert_eq!(repeated["reused"], 1);
        let forced = execute(&db, &backend, "mqa", &json!({"root":temp,"force":true}), cancel.clone()).await.unwrap();
        assert_eq!(forced["inspected"], 1);
        crate::tag_writer::write_tags(&path, &std::collections::HashMap::from([("title".into(),"Changed externally".into())])).unwrap();
        crate::scanner::scan_library(&db, &temp, cancel.clone(), |_| {}).await.unwrap();
        assert_ne!(execute(&db, &backend, "local_duplicates", &args, cancel.clone()).await.unwrap()["reused"], true);
        assert_eq!(execute(&db, &backend, "mqa", &args, cancel).await.unwrap()["inspected"], 1);
        drop(db);
        std::fs::remove_dir_all(temp).unwrap();
    }
}
