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
const WORKERS: usize = 2;

pub fn merge(track: &mut TidalTrack, raw: &Value) {
    // Never cross-fill a different recording, even when a release title agrees.
    let id = raw["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| raw["id"].to_string());
    if id != track.id
        || track
            .isrc
            .as_deref()
            .is_none_or(|isrc| raw["isrc"].as_str() != Some(isrc))
    {
        return;
    }
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
            http.get(format!("https://api.tidal.com/v1/albums/{album}/tracks"))
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
        items.extend(page.iter().cloned());
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
        json!({"checked_at":chrono::Utc::now().timestamp(),"items":items}),
    ))
}

pub async fn prefetch(
    db: &TursoDb,
    http: &reqwest::Client,
    albums: Vec<String>,
    market: &str,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(&str),
) -> Result<HashMap<String, Value>, String> {
    let now = chrono::Utc::now().timestamp();
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
        let cached = db
            .get_preference(&format!("subscriber-album:{market}:{id}"))
            .await?;
        if let Some(value) = cached.filter(|v| {
            v["items"].is_array()
                && v["checked_at"]
                    .as_i64()
                    .is_some_and(|at| (0..MAX_AGE).contains(&(now - at)))
        }) {
            result.insert(id, value);
        } else {
            pending.push_back(id);
        }
    }
    let total = seen.len();
    if pending.is_empty() {
        progress(&format!("Album metadata · {total} cached releases reused"));
        return Ok(result);
    }
    let token = match crate::stream_download::get_valid_token(db, http).await {
        Ok(token) => token,
        Err(_) => {
            progress("Album metadata · subscriber connection unavailable; using catalogue and cached data");
            return Ok(result);
        }
    };
    progress(&format!(
        "Album metadata · {} cached · {} releases to check",
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
            tasks.spawn(fetch(
                http.clone(),
                token.clone(),
                id,
                market.into(),
                cancel.clone(),
            ));
        }
        let Some(task) = tasks.join_next().await else {
            break;
        };
        completed += 1;
        match task {
            Ok(Ok((id, value))) => {
                db.set_preference(&format!("subscriber-album:{market}:{id}"), &value)
                    .await?;
                progress(&format!(
                    "Album metadata · {completed}/{total} · release {id} cached"
                ));
                result.insert(id, value);
            }
            Ok(Err(error)) => {
                progress(&format!(
                    "Album metadata · {completed}/{total} · {error}; saved data retained"
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
                "Album metadata · {completed}/{total} · worker stopped"
            )),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn cached_album_needs_no_authentication_or_requests() {
        let dir = std::env::temp_dir().join(format!("subscriber-cache-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        db.set_preference(
            "subscriber-album:GB:12",
            &json!({"checked_at":chrono::Utc::now().timestamp(),"items":[{"id":1,"bpm":120}]}),
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
    println!("Cached repeat: {} ms", start.elapsed().as_millis());
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}
