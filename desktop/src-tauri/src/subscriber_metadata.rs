//! Bounded album-level metadata enrichment. Never changes recording placements or files.
use crate::{db::TursoDb, tidal::TidalTrack};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

const MAX_AGE: i64 = 30 * 86400;
// Match the shared API lane; spacing and 429 cooldown remain global.
const WORKERS: usize = crate::network::METADATA_CONCURRENCY;

pub fn merge(track: &mut TidalTrack, raw: &Value) {
    // Never cross-fill a different recording, even when a release title agrees.
    let id = raw["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| raw["id"].to_string());
    if id != track.id || track.isrc.as_deref().zip(raw["isrc"].as_str()).is_some_and(|(a,b)| !a.eq_ignore_ascii_case(b)) {
        return;
    }
    track.isrc = track.isrc.clone().or_else(||raw["isrc"].as_str().filter(|isrc|!isrc.trim().is_empty()).map(str::to_owned));
    track.bpm = track.bpm.or(raw["bpm"]
        .as_f64()
        .filter(|bpm| bpm.is_finite() && *bpm > 0.));
    if track.key.is_none() {
        track.key = raw["key"]
            .as_str()
            .filter(|key| !key.trim().is_empty())
            .map(str::to_owned);
        if track.key.is_some() {
            track.key_scale = raw["keyScale"].as_str().map(str::to_owned);
        }
    }
    if raw["credits"].is_array() {
        let mut incoming = TidalTrack {
            id: track.id.clone(),
            isrc: track.isrc.clone(),
            credits: normalize_credits(&raw["credits"]),
            ..Default::default()
        };
        supplement(
            std::slice::from_mut(&mut incoming),
            std::slice::from_ref(track),
        );
        if !track.credits_complete || track.credits != incoming.credits {
            track.credits_checked_at = Some(chrono::Utc::now().timestamp());
        }
        track.credits = incoming.credits;
        track.credits_complete = true;
    }
    if track.copyright.is_none() {
        track.copyright = raw["copyright"].as_str().map(str::to_owned);
    }
}

async fn fetch(
    http: reqwest::Client,
    token: String,
    album: String,
    market: String,
    cancel: Arc<AtomicBool>,
) -> Result<(String, Value), String> {
    let mut items = Vec::new();
    let mut offset = 0;
    loop {
        let response = crate::network::get(
            http.get(format!(
                "https://api.tidal.com/v1/albums/{album}/items/credits"
            ))
            .query(&[
                ("countryCode", market.as_str()),
                ("limit", "100"),
                ("offset", &offset.to_string()),
            ])
            .bearer_auth(&token),
            Duration::from_millis(350),
            2,
            Some(cancel.as_ref()),
        )
        .await?;
        if !response.status().is_success() {
            return Err(format!("Release {album}: HTTP {}", response.status()));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|_| "Invalid album metadata response")?;
        let page = payload["items"]
            .as_array()
            .ok_or("Album metadata has no track list")?;
        items.extend(audio_items(page)?);
        if page.is_empty()
            && payload["totalNumberOfItems"]
                .as_u64()
                .is_some_and(|total| total > offset as u64)
        {
            return Err("Incomplete credited track page; saved cache retained".into());
        }
        offset += page.len();
        if page.is_empty()
            || payload["totalNumberOfItems"]
                .as_u64()
                .is_some_and(|total| offset as u64 >= total)
            || (payload["totalNumberOfItems"].is_null() && page.len() < 100)
        {
            break;
        }
        if offset > 10000 {
            return Err("Album metadata pagination exceeded limit".into());
        }
    }
    Ok((
        album,
        json!({"schema":2,"checked_at":chrono::Utc::now().timestamp(),"items":items}),
    ))
}

fn audio_items(page: &[Value]) -> Result<Vec<Value>, String> {
    let mut items = Vec::new();
    for entry in page {
        if entry["type"] != "track" {
            continue;
        }
        let mut item = entry["item"].clone();
        if !item.is_object() || item["id"].is_null() || !entry["credits"].is_array() {
            return Err("Invalid credited track response".into());
        }
        item["credits"] = entry["credits"].clone();
        items.push(item);
    }
    Ok(items)
}

pub fn normalize_credits(groups: &Value) -> Value {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for group in groups.as_array().into_iter().flatten() {
        let Some(role) = group["type"].as_str() else {
            continue;
        };
        for person in group["contributors"].as_array().into_iter().flatten() {
            let Some(name) = person["name"].as_str().filter(|s| !s.trim().is_empty()) else {
                continue;
            };
            if seen.insert((name.to_lowercase(), role.to_lowercase())) {
                entries.push(json!({"name":name,"role":role,"contributor_id":person["id"],"source":"subscriber"}));
            }
        }
    }
    json!(entries)
}

pub fn tracks(value: &Value) -> Result<Vec<TidalTrack>, String> {
    let mut tracks = Vec::new();
    let mut ids = HashSet::new();
    for item in value["items"].as_array().ok_or("Missing cached tracks")? {
        let id = item["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| item["id"].to_string());
        let position = item["trackNumber"]
            .as_u64()
            .ok_or("Missing online track position")?;
        let disc = item["volumeNumber"]
            .as_u64()
            .ok_or("Missing online disc position")?;
        if id.is_empty()
            || !id.chars().all(|c| c.is_ascii_digit())
            || position == 0
            || disc == 0
            || !ids.insert(id.clone())
        {
            return Err("Invalid or repeated online recording".into());
        }
        tracks.push(TidalTrack {
            id,
            title: crate::tidal::format_title(
                item["title"].as_str().unwrap_or(""),
                item["version"].as_str(),
            ),
            isrc: item["isrc"].as_str().map(str::to_owned),
            track_number: position as u32,
            disc_number: disc as u32,
            duration: item["duration"].as_f64().unwrap_or(0.),
            bpm: item["bpm"].as_f64().filter(|n| *n > 0.),
            key: item["key"].as_str().map(str::to_owned),
            key_scale: item["keyScale"].as_str().map(str::to_owned),
            copyright: item["copyright"].as_str().map(str::to_owned),
            credits: normalize_credits(&item["credits"]),
            credits_complete: item["credits"].is_array(),
            credits_checked_at: value["checked_at"].as_i64(),
            audio_modes: item["audioModes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            media_tags: item["mediaMetadata"]["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            media_metadata: item["mediaMetadata"].clone(),
            ..Default::default()
        });
    }
    tracks.sort_by_key(|t| (t.disc_number, t.track_number));
    Ok(tracks)
}

pub async fn album_info(
    db: &TursoDb,
    _http: &reqwest::Client,
    album: &str,
    _token: &str,
    market: &str,
) -> Result<crate::stream_download::TidalAlbumInfo, String> {
    if let Some(value)=db.get_preference(&format!("subscriber-summary:{market}:{album}")).await? {
        if value["checked_at"].as_i64().is_some_and(|at|(0..MAX_AGE).contains(&(chrono::Utc::now().timestamp()-at))) {
            let info=if value["numberOfTracks"].is_number() || value["artist"].is_object() {
                crate::stream_download::album_info_from_value(&value)
            } else {serde_json::from_value(value).map_err(|e|e.to_string())};
            if let Ok(info)=info {if info.id==album {return Ok(info);}}
        }
    }
    let mut client=crate::tidal::TidalClient::from_db(db).await?;
    let payload=client.albums(&[album.to_owned()],market,false).await?;
    let item=payload["data"].as_array().and_then(|items|items.first()).ok_or("Release unavailable in this market")?;
    crate::stream_download::album_info_from_value(&item["attributes"])
}

async fn cached_snapshot(db: &TursoDb, album: &str, market: &str) -> Result<Option<Value>, String> {
    let now = chrono::Utc::now().timestamp();
    let Some(mut value) = db.get_preference(&format!("subscriber-items:{market}:{album}")).await? else { return Ok(None) };
    // An empty optional DJ/credit value is a completed check. Malformed or
    // partially credited lists, however, must never masquerade as complete.
    if value["schema"] != 2 || !value["items"].as_array().is_some_and(|items| !items.is_empty() && items.iter().all(|item| item["credits"].is_array()))
        || tracks(&value).is_err() { return Ok(None); }
    let summary = db.get_preference(&format!("subscriber-summary:{market}:{album}")).await?;
    if let Some(pinned) = value["summary_fingerprint"].as_str() {
        return Ok(summary.as_ref().is_none_or(|current| crate::tidal::summary_fingerprint(current) == pinned).then_some(value));
    }
    let checked = value["checked_at"].as_i64().unwrap_or(0);
    if let Some(summary) = summary.as_ref() {
        let fingerprint = crate::tidal::summary_fingerprint(summary);
        let prior = db.get_preference(&format!("tag-review:{market}:{album}")).await?;
        // Upgrade older cache entries only with proof that this summary was
        // observed before the list, or with a matching paired release snapshot.
        // A newly refreshed summary must not be pinned onto an older list.
        let paired = prior.as_ref().is_some_and(|prior| {
            prior["recommendation_snapshot"]["summary_fingerprint"].as_str() == Some(&fingerprint)
                && tracks(&value).ok().zip(serde_json::from_value::<Vec<TidalTrack>>(prior["tracks"].clone()).ok())
                    .is_some_and(|(raw, saved)| raw.len() == saved.len() && raw.iter().all(|track| saved.iter().any(|candidate| candidate.id == track.id && candidate.track_number == track.track_number && candidate.disc_number == track.disc_number)))
        });
        let observed_before = value["fingerprint_status"].is_null()
            && summary["checked_at"].as_i64().is_some_and(|at| at > 0 && at < checked);
        if paired || observed_before {
            value["summary_fingerprint"] = json!(fingerprint);
            return Ok(Some(value));
        }
        if prior.as_ref().is_some_and(|prior| prior["recommendation_snapshot"]["summary_fingerprint"].as_str().is_some_and(|old| old != fingerprint)) {
            return Ok(None);
        }
        // A newer (or same-second, therefore ambiguous) summary is not proof
        // that an older unpaired list describes this edition, even if recent.
        if summary["checked_at"].as_i64().is_some_and(|at|at >= checked) { return Ok(None); }
    }
    // Unpaired legacy entries retain their bounded lifetime until a validated
    // fetch binds them. Explicitly unbound responses cannot be promoted by age.
    Ok(((value["fingerprint_status"] != "unbound" || summary.is_none()) && (0..MAX_AGE).contains(&(now - checked))).then_some(value))
}

pub async fn cached(db: &TursoDb, album: &str, market: &str) -> Result<Option<Value>, String> {
    let Some(mut value) = cached_snapshot(db,album,market).await? else { return Ok(None) };
    supplement_cached_response(db,album,market,&mut value).await?;
    Ok(Some(value))
}

async fn supplement_cached_response(db: &TursoDb, album: &str, market: &str, value: &mut Value) -> Result<(),String> {
    let prior = db.get_preference(&format!("tag-review:{market}:{album}")).await?;
    let canonical: Vec<TidalTrack> = prior.and_then(|prior|serde_json::from_value(prior["tracks"].clone()).ok()).unwrap_or_default();
    let ids: Vec<_> = value["items"].as_array().into_iter().flatten().map(|item|crate::tidal::resource_id(&item["id"])).collect();
    let conn = db.connect()?;
    let mut rows = conn.query("SELECT payload FROM app_preferences WHERE key IN (SELECT 'dj-check:' || ? || ':' || value FROM json_each(?))",(market,json!(ids).to_string())).await.map_err(|e|e.to_string())?;
    let mut checks = HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e|e.to_string())? {
        if let Some(value) = row.get::<String>(0).ok().and_then(|raw|serde_json::from_str::<Value>(&raw).ok()) {
            if value["status"] != "retry" { checks.insert(crate::tidal::resource_id(&value["id"]),value); }
        }
    }
    drop(rows);
    for item in value["items"].as_array_mut().into_iter().flatten() {
        let id = crate::tidal::resource_id(&item["id"]);
        let current = canonical.iter().find(|track|track.id == id).map(|track|json!({"id":track.id,"isrc":track.isrc,"bpm":track.bpm,"key":track.key,"keyScale":track.key_scale,"copyright":track.copyright}));
        for source in current.iter().chain(checks.get(&id)) {
            if item["isrc"].as_str().zip(source["isrc"].as_str()).is_some_and(|(a,b)|!a.eq_ignore_ascii_case(b)) { continue; }
            for field in ["isrc","bpm","key","keyScale","copyright"] {
                let absent = item[field].is_null() || item[field].as_str().is_some_and(|value|value.trim().is_empty()) || (field == "bpm" && item[field].as_f64().is_some_and(|value|value <= 0.));
                if field == "keyScale" && item["key"].as_str().zip(source["key"].as_str()).is_some_and(|(a,b)|a != b) { continue; }
                if absent && !source[field].is_null() { item[field] = source[field].clone(); }
            }
        }
    }
    Ok(())
}

pub async fn load(
    db: &TursoDb,
    http: &reqwest::Client,
    album: &str,
    market: &str,
    cancel: Arc<AtomicBool>,
    force: bool,
) -> Result<Value, String> {
    load_with_cache_status(db,http,album,market,cancel,force).await.map(|(value,_)|value)
}

pub(crate) async fn load_with_cache_status(
    db: &TursoDb,
    http: &reqwest::Client,
    album: &str,
    market: &str,
    cancel: Arc<AtomicBool>,
    force: bool,
) -> Result<(Value,bool),String> {
    if album.is_empty() || !album.chars().all(|c| c.is_ascii_digit()) {
        return Err("Invalid release ID".into());
    }
    // A download and a metadata job can ask for the same release simultaneously.
    let gate =
        crate::actions::release_gate(format!("subscriber:{}:{market}:{album}", db.path.display()));
    let _guard = crate::actions::release_guard(gate,cancel.as_ref()).await?;
    if !force {
        if let Some(value) = cached(db, album, market).await? {
            return Ok((value,true));
        }
    }
    let retry_key = format!("subscriber-items-retry:{market}:{album}");
    if !force {
        if let Some(retry) = db.get_preference(&retry_key).await?.filter(|retry| retry["retry_after"].as_i64().is_some_and(|until| until > chrono::Utc::now().timestamp())) {
            return Err(retry["error"].as_str().unwrap_or("Metadata retry paused; saved data retained").to_owned());
        }
    }
    let before = db.get_preference(&format!("subscriber-summary:{market}:{album}")).await?;
    let fetched = async {
        let token = crate::stream_download::get_valid_token(db, http).await?;
        let (_, value) = fetch(http.clone(), token, album.into(), market.into(), cancel.clone()).await?;
        tracks(&value)?;
        Ok::<_,String>(value)
    }.await;
    let mut value = match fetched {
        Ok(value) => value,
        Err(error) => {
            if !cancel.load(Ordering::Relaxed) {
                db.set_preference(&retry_key, &json!({"error":error,"retry_after":chrono::Utc::now().timestamp()+300})).await?;
            }
            return Err(error);
        }
    };
    let after = db.get_preference(&format!("subscriber-summary:{market}:{album}")).await?;
    if let Some(summary) = before.as_ref().filter(|prior| after.as_ref().is_some_and(|current| crate::tidal::summary_fingerprint(prior) == crate::tidal::summary_fingerprint(current))) {
        value["summary_fingerprint"] = json!(crate::tidal::summary_fingerprint(summary));
        value["fingerprint_status"] = json!("paired");
    } else {
        value["fingerprint_status"] = json!("unbound");
    }
    tracks(&value)?; // Do not publish incomplete/malformed responses.
    db.set_preference(&format!("subscriber-items:{market}:{album}"), &value)
        .await?;
    db.set_preference(&retry_key, &Value::Null).await?;
    supplement_cached_response(db,album,market,&mut value).await?;
    Ok((value,false))
}

/// Preserve independently fetched discovery fields and never erase richer saved credits.
pub fn supplement(new: &mut [TidalTrack], old: &[TidalTrack]) {
    for track in new {
        let Some(prior) = old.iter().find(|p| {
            p.id == track.id && (p.isrc.is_none() || track.isrc.is_none() || p.isrc == track.isrc)
        }) else {
            continue;
        };
        track.bpm = track.bpm.or(prior.bpm);
        if track.key.is_none() {
            track.key = prior.key.clone();
            track.key_scale = prior.key_scale.clone();
        }
        if track.genres.is_empty() {
            track.genres = prior.genres.clone();
        }
        if track.replacement_id.is_none() {
            track.replacement_id = prior.replacement_id.clone();
        }
        track.discovery_checked_at = prior.discovery_checked_at;
        let mut credits = track.credits.as_array().cloned().unwrap_or_default();
        for credit in prior.credits.as_array().into_iter().flatten() {
            if !credits
                .iter()
                .any(|c| c["name"] == credit["name"] && c["role"] == credit["role"])
            {
                credits.push(credit.clone());
            }
        }
        track.credits = json!(credits);
    }
}

pub async fn prefetch(
    db: &TursoDb,
    http: &reqwest::Client,
    albums: Vec<String>,
    market: &str,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(&str),
) -> Result<HashMap<String, Value>, String> {
    let mut result = HashMap::new();
    let mut pending = VecDeque::new();
    let mut seen = HashSet::new();
    for id in albums {
        if cancel.load(Ordering::Relaxed) {
            return Ok(result);
        }
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) || !seen.insert(id.clone()) {
            continue;
        }
        if let Some(value) = cached(db, &id, market).await? {
            result.insert(id, value);
        } else {
            pending.push_back(id);
        }
    }
    let total = seen.len();
    if pending.is_empty() {
        progress(&format!("Track details and credits · {total} cached releases reused · no metadata download needed"));
        return Ok(result);
    }
    let _token = match crate::stream_download::get_valid_token(db, http).await {
        Ok(token) => token,
        Err(_) => {
            progress("Track details and credits · account connection unavailable; saved data retained");
            return Ok(result);
        }
    };
    progress(&format!(
        "Track details and credits · {} cached releases reused · {} releases to fetch · positions, ISRC, contributors and available BPM/key",
        result.len(),
        pending.len()
    ));
    let mut tasks = tokio::task::JoinSet::new();
    let mut completed = result.len();
    loop {
        if cancel.load(Ordering::Relaxed) {
            tasks.abort_all();
            break;
        }
        while tasks.len() < WORKERS {
            let Some(id) = pending.pop_front() else { break };
            let db = db.clone();
            let http = http.clone();
            let market = market.to_owned();
            let cancel = cancel.clone();
            tasks.spawn(async move {
                let value = load(&db, &http, &id, &market, cancel, false).await?;
                Ok::<_, String>((id, value))
            });
        }
        let Some(task) = tasks.join_next().await else {
            break;
        };
        completed += 1;
        match task {
            Ok(Ok((id, value))) => {
                progress(&format!(
                    "Track details and credits · {completed}/{total} releases ready · release {id} cached"
                ));
                result.insert(id, value);
            }
            Ok(Err(error)) => {
                progress(&format!(
                    "Track details and credits · {completed}/{total} releases checked · {error}; saved data retained"
                ));
                if ["HTTP 429", "HTTP 401", "HTTP 403", "cooldown", "deadline"]
                    .iter()
                    .any(|reason| error.contains(reason))
                {
                    tasks.abort_all();
                    break;
                }
            }
            Err(_) => progress(&format!(
                "Track details and credits · {completed}/{total} releases checked · worker stopped"
            )),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credited_audio_excludes_video_and_keeps_roles_and_supplements() {
        let page = json!([
            {"type":"track","item":{"id":1,"title":"Audio","trackNumber":1,"volumeNumber":1,"isrc":"ABC","bpm":120,"key":"C","keyScale":"MINOR"},"credits":[{"type":"Composer","contributors":[{"name":"Writer","id":2}]},{"type":"Producer","contributors":[{"name":"Writer","id":2}]},{"type":"Guitar","contributors":[{"name":"Player"}]}]},
            {"type":"video","item":{"id":3,"trackNumber":2}}
        ]);
        let items = audio_items(page.as_array().unwrap()).unwrap();
        let mut parsed = tracks(&json!({"items":items,"checked_at":1})).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].credits.as_array().unwrap().len(), 3);
        assert_eq!(
            crate::enrichment::credit_tags(&parsed[0].credits)["composer"],
            "Writer"
        );
        assert!(crate::recommendations::credit_names(&parsed[0].credits).contains("writer"));
        let old = TidalTrack {
            id: "1".into(),
            isrc: Some("ABC".into()),
            genres: vec!["House".into()],
            credits: json!([{"name":"Other","role":"Mixer"}]),
            ..Default::default()
        };
        supplement(&mut parsed, &[old]);
        assert_eq!(parsed[0].genres, vec!["House"]);
        assert_eq!(parsed[0].credits.as_array().unwrap().len(), 4);
        assert!(audio_items(&[json!({"type":"track","item":{"id":1}})]).is_err());
    }

    #[test]
    fn enrich_only_same_recording_without_overwriting_key_scale() {
        let mut track = TidalTrack {
            id: "12".into(),
            isrc: Some("ABC".into()),
            ..Default::default()
        };
        merge(&mut track, &json!({"id":13,"isrc":"ABC","bpm":120}));
        assert_eq!(track.bpm, None);
        merge(&mut track, &json!({"id":12,"isrc":"OTHER","bpm":120}));
        assert_eq!(track.bpm, None);
        merge(
            &mut track,
            &json!({"id":12,"isrc":"ABC","bpm":120,"key":"C","keyScale":"MINOR"}),
        );
        assert_eq!(track.bpm, Some(120.));
        assert_eq!(track.key.as_deref(), Some("C"));
        merge(
            &mut track,
            &json!({"id":12,"isrc":"ABC","bpm":130,"key":"D","keyScale":"MAJOR"}),
        );
        assert_eq!(track.bpm, Some(120.));
        assert_eq!(track.key_scale.as_deref(), Some("MINOR"));
    }
    #[tokio::test]
    async fn artwork_summary_cache_is_separate_from_track_items() {
        let dir = std::env::temp_dir().join(format!("subscriber-summary-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let summary = json!({"id":"12","title":"Release","cover":"cover-id","checked_at":chrono::Utc::now().timestamp()});
        db.set_preference("subscriber-summary:GB:12", &summary)
            .await
            .unwrap();
        db.set_preference("subscriber-album:GB:12", &json!({"items":[{"id":1}]}))
            .await
            .unwrap();
        let info = album_info(&db, &reqwest::Client::new(), "12", "unused", "GB")
            .await
            .unwrap();
        assert_eq!(info.cover.as_deref(), Some("cover-id"));
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cached_album_needs_no_authentication_or_requests() {
        let dir = std::env::temp_dir().join(format!("subscriber-cache-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        db.set_preference(
            "subscriber-items:GB:12",
            &json!({"schema":2,"checked_at":chrono::Utc::now().timestamp(),"items":[{"id":1,"bpm":120,"trackNumber":1,"volumeNumber":1,"credits":[]}]}),
        )
        .await
        .unwrap();
        let result = prefetch(
            &db,
            &reqwest::Client::new(),
            vec!["12".into(), "12".into()],
            "GB",
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result["12"]["items"][0]["bpm"], 120);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn credited_cache_reuses_aged_paired_lists_and_rejects_changed_or_unproven_summaries() {
        let dir = std::env::temp_dir().join(format!("subscriber-paired-{}",uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        let summary = json!({"id":12,"title":"Release","numberOfTracks":1,"artist":{"id":4,"name":"Main"},"checked_at":now});
        let paired = json!({"schema":2,"checked_at":now-40*86400,"summary_fingerprint":crate::tidal::summary_fingerprint(&summary),"items":[{"id":121,"trackNumber":1,"volumeNumber":1,"credits":[],"bpm":null,"key":null}]});
        db.set_preference("subscriber-summary:GB:12",&summary).await.unwrap();
        db.set_preference("subscriber-items:GB:12",&paired).await.unwrap();
        let reused = prefetch(&db,&reqwest::Client::new(),vec!["12".into()],"GB",Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
        assert_eq!(reused["12"]["items"],paired["items"],"Successful empty DJ fields are reusable without a connection");
        assert!(cached(&db,"12","US").await.unwrap().is_none());
        db.set_preference("tag-review:GB:12",&json!({"tracks":[{"id":"121","track_number":1,"disc_number":1,"credits_complete":true,"bpm":124,"key":"G","key_scale":"major"}]})).await.unwrap();
        let enriched = cached(&db,"12","GB").await.unwrap().unwrap();
        assert_eq!(enriched["items"][0]["bpm"].as_f64(),Some(124.),"Downloads reuse DJ values obtained by enrichment");
        assert_eq!(enriched["items"][0]["keyScale"],"major");
        let mut changed = summary.clone(); changed["numberOfTracks"] = json!(2);
        db.set_preference("subscriber-summary:GB:12",&changed).await.unwrap();
        assert!(cached(&db,"12","GB").await.unwrap().is_none());
        let mut legacy = paired.clone(); legacy.as_object_mut().unwrap().remove("summary_fingerprint");
        db.set_preference("subscriber-items:GB:12",&legacy).await.unwrap();
        assert!(cached(&db,"12","GB").await.unwrap().is_none(),"A new summary cannot validate an unpaired old list");
        let mut earlier = summary.clone(); earlier["checked_at"] = json!(now-41*86400);
        db.set_preference("subscriber-summary:GB:12",&earlier).await.unwrap();
        assert_eq!(cached(&db,"12","GB").await.unwrap().unwrap()["summary_fingerprint"],paired["summary_fingerprint"]);
        legacy["fingerprint_status"] = json!("unbound");
        db.set_preference("subscriber-items:GB:12",&legacy).await.unwrap();
        assert!(cached(&db,"12","GB").await.unwrap().is_none(),"A response captured during a summary change stays unpaired");
        let mut malformed = paired.clone(); malformed["items"][0]["credits"] = Value::Null;
        db.set_preference("subscriber-items:GB:12",&malformed).await.unwrap();
        assert!(cached(&db,"12","GB").await.unwrap().is_none());
        drop(db); std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
#[tokio::test]
#[ignore = "Explicit live parallel album-cache check; two releases, temporary database"]
async fn live_parallel_cache() {
    let dir = std::env::temp_dir().join(format!("subscriber-live-{}", uuid::Uuid::new_v4()));
    let db = TursoDb::open(dir.join("db")).await.unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let ids = vec!["234657671".into(), "140303440".into()];
    let result = prefetch(
        &db,
        &http,
        ids.clone(),
        "GB",
        Arc::new(AtomicBool::new(false)),
        |message| println!("{message}"),
    )
    .await
    .unwrap();
    assert_eq!(result.len(), 2);
    let items = result["140303440"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 12);
    assert!(items.iter().any(|v| v["bpm"].as_f64().is_some()));
    db.set_preference("tag-review:GB:140303440",&json!({"id":"140303440","title":"Cassie","artist":"Cassie","tracks_loaded":false,"discovery_checked_at":chrono::Utc::now().timestamp()})).await.unwrap();
    let parsed = tracks(&result["140303440"]).unwrap();
    assert_eq!(parsed.len(), 12);
    assert!(parsed
        .iter()
        .all(|t| t.credits.as_array().is_some_and(|c| !c.is_empty())));
    println!(
        "Cached audio tracks: {}; contributor/role entries: {}",
        parsed.len(),
        parsed
            .iter()
            .map(|t| t.credits.as_array().unwrap().len())
            .sum::<usize>()
    );

    // A second pass must still work with authentication explicitly unavailable.
    db.set_preference("account-disconnected", &json!(true))
        .await
        .unwrap();
    let start = std::time::Instant::now();
    let cached = prefetch(
        &db,
        &http,
        ids,
        "GB",
        Arc::new(AtomicBool::new(false)),
        |message| println!("{message}"),
    )
    .await
    .unwrap();
    assert_eq!(cached, result);
    let published = crate::actions::release(&db, "140303440", "GB", false)
        .await
        .unwrap();
    assert_eq!(published["track_count"], 12);
    assert_eq!(published["track_metadata_source"], "subscriber");
    assert!(published["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["credits"].as_array().is_some_and(|c| !c.is_empty())));

    let downloaded = crate::stream_download::fetch_album_tracks(
        &db,
        &http,
        "140303440",
        "unused",
        &Arc::new(AtomicBool::new(false)),
        "GB",
    )
    .await
    .unwrap();
    assert_eq!(downloaded.len(), 12);
    assert!(downloaded
        .iter()
        .all(|t| t.credits.as_array().is_some_and(|c| !c.is_empty())));

    println!("Cached repeat: {} ms", start.elapsed().as_millis());
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}
