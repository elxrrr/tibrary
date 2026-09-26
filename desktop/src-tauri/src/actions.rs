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

pub fn handles(kind: &str) -> bool {
    matches!(
        kind,
        "preview"
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
pub async fn release(db: &TursoDb, id: &str, market: &str, force: bool) -> Result<Value, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err("Select a release with a valid online ID".into());
    }
    let key = format!("tag-review:{market}:{id}");
    let cached = db.get_preference(&key).await?;
    if !force {
        if let Some(v) = cached.as_ref().filter(|v| v["tracks_loaded"] == true) {
            return Ok(v.clone());
        }
    }
    let mut value = db
        .get_detail(&json!({"release_id":id, "market":market}))
        .await?;
    if !force && value["tracks_loaded"] == true {
        return Ok(value);
    }
    let mut client = crate::tidal::TidalClient::from_db(db).await?;
    if value["title"] == "Release not found" {
        let mut url = url::Url::parse(&format!("https://openapi.tidal.com/v2/albums/{id}"))
            .map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("countryCode", market)
            .append_pair("include", "artists");
        let raw = client.get_json(url.as_str()).await?;
        let data = raw["data"]
            .as_array()
            .and_then(|a| a.first())
            .unwrap_or(&raw["data"]);
        let a = &data["attributes"];
        let artists = raw["included"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v["type"] == "artists")
            .filter_map(|v| v["attributes"]["name"].as_str())
            .collect::<Vec<_>>()
            .join(", ");
        value = json!({"id":id,"artist":artists,"title":crate::tidal::format_title(a["title"].as_str().unwrap_or(""),a["version"].as_str()),"date":a["releaseDate"].as_str().unwrap_or(""),"type":a["albumType"].as_str().unwrap_or("album"),"available":a["availability"].as_array().map(|v|v.iter().any(|x|x=="STREAM"||x=="DJ")),"label":a["recordLabel"].as_str().or(a["recordLabel"]["name"].as_str()),"copyright":a["copyright"].as_str(),"remote_metadata":raw});
    }
    let tracks = client.get_release_details(id, market).await?;
    value["tracks"] = serde_json::to_value(&tracks).map_err(|e| e.to_string())?;
    value["tracks_loaded"] = json!(true);
    value["track_count"] = json!(tracks.len());
    db.set_preference(&key, &value).await?;
    // Publish details to every catalogue reference and existing queue entry.
    let conn = db.connect()?;
    let mut rows = conn
        .query(
            "SELECT artist_id,payload FROM catalogue WHERE market=?",
            (market,),
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
    if kind == "release_details" {
        let id = args["id"].as_str().ok_or("Select a release")?;
        release(db, id, market, args["force"].as_bool().unwrap_or(false)).await?;
        return Ok(json!({"release_id":id}));
    }
    if kind == "connections" {
        let mut metrics = json!({});
        if let Ok(mut client) = crate::tidal::TidalClient::from_db(db).await {
            let start = std::time::Instant::now();
            let result = client.authenticate().await;
            metrics["catalogue"] = json!({
                "ok": result.is_ok(),
                "message": result.err().unwrap_or_default(),
                "latency_ms": start.elapsed().as_millis()
            });
        } else {
            metrics["catalogue"] = json!({"ok":false,"message":"Application credentials required"});
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        let token = crate::stream_download::get_valid_token(db, &http).await;
        let mut user_detail = String::new();
        let result = match token {
            Ok(token) => match http
                .get("https://api.tidal.com/v1/sessions")
                .bearer_auth(token)
                .send()
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
                let _ = db.set_preference("account_connected_at", &json!(today)).await;
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
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
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
            let response: Value = http
                .get(format!(
                    "https://api.tidal.com/v1/users/{user}/favorites/artists"
                ))
                .query(&[
                    ("countryCode", market),
                    ("limit", "100"),
                    ("offset", &offset.to_string()),
                ])
                .bearer_auth(&token)
                .send()
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
        for path in sources {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            crate::duplicates::trash_file_or_directory(&path).await?;
            db.remove_local_file(&path).await?;
            completed += 1;
            state.progress(&format!(
                "Moved duplicate to Trash · {}",
                std::path::Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ));
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
                state.progress(&format!("Searching selected recordings · {phrase}"));
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
    if kind == "optimizations" || kind == "check_replacements" {
        if args["scope"] != "remote" && kind != "check_replacements" {
            let clusters = crate::duplicates::find_duplicate_clusters(&indexed);
            let rows = crate::duplicates::clusters_to_group_rows(&clusters);
            db.set_preference(&format!("desktop-local:{root}"), &json!(rows))
                .await?;
            return Ok(json!({"opportunities":rows.len()}));
        }
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
        for mut target in remote {
            if cancel.load(Ordering::Relaxed) {
                break;
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
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(25))
            .build()
            .map_err(|e| e.to_string())?;
        let mut token = None;
        let mut output = vec![];
        let cache = db
            .path
            .parent()
            .ok_or("Invalid cache directory")?
            .join("artwork-cache");
        std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        for file in indexed
            .iter()
            .filter(|f| ids.is_empty() || ids.contains(&f.path))
        {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
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
                let album_key = format!("subscriber-album:{market}:{album}");
                let info = if let Some(info) = db.get_preference(&album_key).await? {
                    serde_json::from_value::<crate::stream_download::TidalAlbumInfo>(info)
                        .map_err(|e| e.to_string())?
                } else {
                    let info = crate::stream_download::fetch_album_info(
                        &http,
                        album,
                        token.as_ref().unwrap(),
                        market,
                    )
                    .await?;
                    db.set_preference(&album_key, &json!(info)).await?;
                    info
                };
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
            state.progress(&format!(
                "Artwork ready · {}",
                tags.get("title").unwrap_or(&file.path)
            ));
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
        let mut client = crate::tidal::TidalClient::from_db(db).await?;
        let conn = db.connect()?;
        let mut checked = 0;
        for artist in page.rows {
            let name = artist["artist"].as_str().unwrap_or("");
            if name.is_empty()
                || (!selected.is_empty() && !selected.contains(name))
                || (selected.is_empty()
                    && matches!(artist["status"].as_str(), Some("confirmed" | "auto")))
            {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            state.progress(&format!("Finding artist matches · {name}"));
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
            let candidates: Vec<crate::tidal::TidalArtist> =
                if let Some(c) = db.get_preference(&search_key).await? {
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
            state.progress(&format!(
                "Artist checked · {name} · {} supported matches",
                accepted.len()
            ));
        }
        return Ok(json!({"checked":checked}));
    }
    if kind == "metadata" || kind == "manual_candidate" {
        let mut output = vec![];
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        let mut subscriber_token = None;
        for file in indexed
            .iter()
            .filter(|f| ids.is_empty() || ids.contains(&f.path))
        {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let detail = db
                .get_detail(&json!({"path":file.path,"market":market}))
                .await?;
            let tags = workflows::extract_tags_map(&file.metadata);
            let album = args["album_id"]
                .as_str()
                .or(detail["linked_ids"]["album_id"].as_str())
                .or_else(|| tags.get("tidal_album_id").map(String::as_str));
            let Some(album) = album else { continue };
            let value = release(db, album, market, false).await?;
            let mut rel: crate::tidal::TidalRelease =
                serde_json::from_value(value).map_err(|e| e.to_string())?;
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
                        let response = http
                            .get(format!("https://api.tidal.com/v1/tracks/{}", track.id))
                            .query(&[("countryCode", market)])
                            .bearer_auth(token)
                            .send()
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
                if extra["isrc"].as_str().is_some()
                    && extra["isrc"].as_str() == track.isrc.as_deref()
                {
                    track.bpm = track.bpm.or(extra["bpm"].as_f64());
                    track.key = track
                        .key
                        .or_else(|| extra["key"].as_str().map(str::to_owned));
                    track.key_scale = track
                        .key_scale
                        .or_else(|| extra["keyScale"].as_str().map(str::to_owned));
                }
            }
            rel.tracks[index] = track.clone();
            db.set_preference(&format!("tag-review:{market}:{}", rel.id), &json!(rel))
                .await?;
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
                Ok(()) => count += 1,
                Err(e) => errors.push(format!("{path}: {e}")),
            }
            state.progress(&format!(
                "Applied {count} files · {}",
                std::path::Path::new(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ));
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
        let plans = workflows::plan_workflow(&indexed, action, template);
        let rows:Vec<Value>=plans.into_iter().filter(|p|ids.is_empty()||ids.contains(&p.path)).map(|p| {
            let f=indexed.iter().find(|f|f.path==p.path).unwrap();
            json!({"id":p.path,"path":p.path,"artist":p.artist,"release":p.album,"title":p.title,"tags":p.current_tags,"changes":p.changes,"target":p.target,"affected":true,"status":"Needs update","size":f.size,"mtime":f.mtime,"item":{"path":p.path,"target":p.target,"tags":p.changes}})
        }).collect();
        let id = uuid::Uuid::new_v4().to_string();
        state.previews.lock().unwrap().insert(id.clone(),json!({"id":id,"created":chrono::Utc::now().timestamp_millis(),"operation":action,"root":root,"rows":rows,"count":rows.len()}));
        return Ok(json!({"preview_id":id,"operation":action,"root":root}));
    }
    if kind == "mqa" {
        let mut rows = vec![];
        for file in indexed {
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
                serde_json::to_value(crate::mqa::audit_file(std::path::Path::new(&file.path)))
                    .map_err(|e| e.to_string())?
            };
            db.set_preference(
                &key,
                &json!({"size":file.size,"mtime":file.mtime,"result":result}),
            )
            .await?;
            let tags = workflows::extract_tags_map(&file.metadata);
            rows.push(json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),"release":tags.get("album"),"title":tags.get("title"),"status":result["status"],"evidence":result["evidence"],"affected":result["detected"],"target":"Queue lossless replacement"}));
        }
        db.set_preference(&format!("desktop-mqa:{root}"), &json!(rows))
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
    async fn real_flac_prepare_workflows_are_read_only_until_confirmed() {
        let Ok(sample) = std::env::var("TIBRARY_SAMPLE_FLAC") else { return };
        let source = std::path::Path::new(&sample);
        let before = std::fs::read(source).unwrap();
        let temp = std::env::temp_dir().join(format!("tibrary-real-prep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let copied = temp.join("sample.flac");
        std::fs::copy(source, &copied).unwrap();
        crate::tag_writer::write_tags(&copied, &std::collections::HashMap::from([
            ("tracknumber".into(), "1".into()),
            ("discnumber".into(), "1".into()),
        ])).unwrap();
        let db = TursoDb::open(&temp.join("db")).await.unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        crate::scanner::scan_library(&db, &temp, cancel.clone(), |_| {}).await.unwrap();
        let backend = Arc::new(Backend::new());
        for action in ["dates", "numbers", "keys", "lyrics", "organise"] {
            execute(&db, &backend, "preview", &json!({"root":temp,"action":action}), cancel.clone()).await.unwrap();
        }
        let preview = execute(&db, &backend, "preview", &json!({"root":temp,"action":"numbers"}), cancel.clone()).await.unwrap();
        let applied = execute(&db, &backend, "apply", &json!({"root":temp,"preview_id":preview["preview_id"],"ids":[copied],"confirmed":true}), cancel.clone()).await.unwrap();
        assert_eq!(applied["applied"], 1);
        let copied_tags = workflows::extract_tags_map(&Some(serde_json::to_value(crate::scanner::read_audio_metadata(&copied).unwrap()).unwrap()));
        assert_eq!(copied_tags.get("tracknumber").map(String::as_str), Some("01"));
        let audited = execute(&db, &backend, "mqa", &json!({"root":temp}), cancel.clone()).await.unwrap();
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
        db.set_preference("tag-review:GB:123", &json!({"id":"123","title":"Release","artist":"Wrong Featured Credit","tracks_loaded":true,"track_count":1,"tracks":[{"id":"456","title":"Example","track_number":1,"disc_number":1,"bpm":124.0,"key":"G","key_scale":"major"}]})).await.unwrap();
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
