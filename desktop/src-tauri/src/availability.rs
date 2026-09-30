//! Market-specific, bounded availability checks; transport failures are never removals.
use crate::{db::TursoDb, tidal::TidalClient};
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, sync::{Arc, atomic::{AtomicBool, Ordering}}};

pub fn available(resource: &Value) -> Option<bool> {
    resource["attributes"]["availability"]
        .as_array()
        .map(|items| items.iter().any(|v| v == "STREAM" || v == "DJ"))
}

pub async fn check(
    db: &TursoDb,
    ids: &[String],
    market: &str,
) -> Result<HashMap<String, Option<bool>>, String> {
    check_refresh(db, ids, market, false).await
}

pub async fn check_refresh(
    db: &TursoDb,
    ids: &[String],
    market: &str,
    force: bool,
) -> Result<HashMap<String, Option<bool>>, String> {
    check_with_progress(db, ids, market, force, Arc::new(AtomicBool::new(false)), |_| {}, |_| {}).await
}

fn reusable(value: &Value, now: i64) -> bool {
    let Some(at) = value["checked_at"].as_i64().filter(|at| *at > 0 && *at <= now) else { return false; };
    let lifetime = match value["available"].as_bool() {
        Some(true) => 7 * 86400,
        Some(false) => 86400,
        None => 3600,
    };
    now - at < lifetime || value["retry_after"].as_i64().is_some_and(|until| until > now)
}

/// Load just the requested checks in one read, rather than one SQLite connection per ID.
async fn cached_for(db: &TursoDb, ids: &[String], market: &str) -> Result<HashMap<String, Value>, String> {
    let conn = db.connect()?;
    let mut rows = conn.query(
        "SELECT key,payload FROM app_preferences WHERE key IN (SELECT 'release-live:' || ? || ':' || value FROM json_each(?))",
        (market, json!(ids).to_string()),
    ).await.map_err(|error| error.to_string())?;
    let mut saved = HashMap::new();
    while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
        let key: String = row.get(0).map_err(|error| error.to_string())?;
        let raw: String = row.get(1).map_err(|error| error.to_string())?;
        if let Ok(value) = serde_json::from_str(&raw) {
            saved.insert(key.rsplit(':').next().unwrap_or("").to_owned(), value);
        }
    }
    Ok(saved)
}

/// Publish a successfully returned batch. An omitted ID is unavailable; an
/// authentication, timeout or throttling error never reaches this function.
pub(crate) async fn save_response(db: &TursoDb, raw: &Value, ids: &[String], market: &str) -> Result<HashMap<String, Option<bool>>, String> {
    let resources = raw["data"].as_array().ok_or("Invalid release availability response")?;
    let now = chrono::Utc::now().timestamp();
    let conn = db.connect()?;
    conn.execute("BEGIN IMMEDIATE", ()).await.map_err(|error| error.to_string())?;
    let result = async {
        let mut statuses = HashMap::new();
        for id in ids {
            let resource = resources.iter().find(|r| r["id"].as_str() == Some(id.as_str()));
            let status = resource.map(available).unwrap_or(Some(false));
            let key = format!("release-live:{market}:{id}");
            // Unknown flags cannot erase an earlier confirmed availability result.
            let value = json!({"available":status,"checked_at":now});
            if status.is_some() {
                conn.execute("INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)", (key, value.to_string())).await.map_err(|error| error.to_string())?;
            } else {
                conn.execute("INSERT INTO app_preferences(key,payload) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET payload=json_set(app_preferences.payload,'$.retry_after',?)", (key, value.to_string(), now + 3600)).await.map_err(|error| error.to_string())?;
            }
            statuses.insert(id.clone(), status);
        }
        for (id, credits) in crate::release_artists::parse(raw, ids, now) {
            conn.execute("INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)", (format!("release-artists:{market}:{id}"), credits.to_string())).await.map_err(|error| error.to_string())?;
        }
        conn.execute("COMMIT", ()).await.map_err(|error| error.to_string())?;
        Ok::<_, String>(statuses)
    }.await;
    if result.is_err() { let _ = conn.execute("ROLLBACK", ()).await; }
    result
}

// Publish partial successful checks even if a later request fails or is cancelled.
struct Publication<'a> { db: &'a TursoDb, changed: bool }
impl Drop for Publication<'_> {
    fn drop(&mut self) { if self.changed { self.db.bump_revision(); } }
}

pub(crate) async fn check_with_progress(
    db: &TursoDb,
    ids: &[String],
    market: &str,
    force: bool,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(&str),
    mut detail: impl FnMut(&str),
) -> Result<HashMap<String, Option<bool>>, String> {
    let now = chrono::Utc::now().timestamp();
    let saved = cached_for(db, ids, market).await?;
    let mut result = HashMap::new();
    let mut pending = vec![];
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            result.insert(id.clone(), None);
            continue;
        }
        if let Some(v) = saved.get(id).filter(|v| !force && reusable(v, now)) {
            result.insert(id.clone(), v["available"].as_bool());
        } else {
            pending.push(id.clone());
        }
    }
    let total = result.len() + pending.len();
    let reused = result.len();
    progress(&format!("Checking availability · {reused}/{total} releases · {reused} saved checks reused · {market}"));
    if pending.is_empty() {
        return Ok(result);
    }
    let mut publication = Publication { db, changed: false };
    let mut client = TidalClient::from_db(db).await?.with_cancel(cancel.clone());
    for batch in pending.chunks(20) {
        if cancel.load(Ordering::Relaxed) { return Err("Availability check cancelled; completed checks saved".into()); }
        progress(&format!("Checking availability · {}/{total} releases · requesting {} release summaries · {market}", result.len(), batch.len()));
        let raw = client.albums(batch, market, true).await?;
        let statuses = save_response(db, &raw, batch, market).await?;
        publication.changed = true;
        for id in batch {
            let status = statuses.get(id).copied().flatten();
            let resource = raw["data"].as_array().into_iter().flatten().find(|resource| resource["id"].as_str() == Some(id.as_str()));
            let title = resource.and_then(|resource|resource["attributes"]["title"].as_str()).unwrap_or("Saved release");
            detail(&format!("{} · {title} · release {id} · {market}", match status { Some(true) => "Available", Some(false) => "Unavailable", None => "Availability not confirmed; saved evidence retained" }));
            result.insert(id.clone(), status);
        }
        progress(&format!("Checking availability · {}/{total} releases · {reused} saved checks reused · {market}", result.len()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "Read-only live release availability smoke check"]
    async fn live_release_availability() {
        let dir = std::env::temp_dir().join(format!("availability-live-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let ids = ["234657671", "429043587", "461260542", "455460887", "136650046"].map(str::to_owned);
        let result = check(&db, &ids, "GB")
            .await
            .unwrap();
        assert_eq!(result["234657671"], Some(true));
        for id in &ids[1..] { assert_eq!(result[id], Some(false), "stale release {id}"); }
        let revision = db.revision.load(Ordering::SeqCst);
        assert_eq!(
            check(&db, &ids, "GB").await.unwrap(), result
        );
        assert_eq!(db.revision.load(Ordering::SeqCst), revision, "reused checks must not invalidate views");
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn availability_distinguishes_unknown_and_reuses_market_cache() {
        assert_eq!(
            available(&json!({"attributes":{"availability":["STREAM"]}})),
            Some(true)
        );
        assert_eq!(
            available(&json!({"attributes":{"availability":[]}})),
            Some(false)
        );
        assert_eq!(available(&json!({})), None);
        let dir = std::env::temp_dir().join(format!("availability-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        db.set_preference(
            "release-live:GB:123",
            &json!({"available":false,"checked_at":chrono::Utc::now().timestamp()}),
        )
        .await
        .unwrap();
        let result = check(&db, &["123".into(), "123".into()], "GB")
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result["123"], Some(false));
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn successful_batches_preserve_unknowns_and_mark_omitted_ids_unavailable() {
        let dir = std::env::temp_dir().join(format!("availability-batch-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        db.set_preference("release-live:GB:1", &json!({"available":false,"checked_at":now-10})).await.unwrap();
        let ids = ["1", "2", "3", "4"].map(str::to_owned);
        let results = save_response(&db, &json!({"data":[{"id":"1","attributes":{}},{"id":"2","attributes":{"availability":["STREAM"]}},{"id":"4","attributes":{}}]}), &ids, "GB").await.unwrap();
        assert_eq!(results["1"], None);
        assert_eq!(results["2"], Some(true));
        assert_eq!(results["3"], Some(false));
        assert_eq!(results["4"], None);
        let saved = cached_for(&db, &ids, "GB").await.unwrap();
        assert_eq!(saved["1"]["available"], false, "unknown response cannot erase earlier confirmation");
        assert_eq!(saved["1"]["checked_at"], now-10);
        assert!(reusable(&saved["1"], now));
        assert!(reusable(&saved["4"], now));
        assert!(!reusable(&json!({"available":true,"checked_at":now+100}), now));
        assert!(!reusable(&json!({"available":true,"checked_at":now-8*86400}), now));
        assert!(save_response(&db, &json!({"error":"HTTP 429"}), &ids, "GB").await.is_err());
        assert_eq!(cached_for(&db, &ids, "GB").await.unwrap(), saved, "malformed or failed response must not mark releases unavailable");
        assert!(cached_for(&db, &ids, "US").await.unwrap().is_empty());
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
