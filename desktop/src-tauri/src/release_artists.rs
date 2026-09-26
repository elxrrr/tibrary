//! Small, resumable album-credit lookups. No audio or track-list requests.
use crate::{db::TursoDb, tidal::TidalClient, Backend};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub async fn cached(db: &TursoDb, market: &str) -> Result<HashMap<String, Value>, String> {
    let conn = db.connect()?;
    let mut rows = conn
        .query(
            "SELECT key,payload FROM app_preferences WHERE key LIKE ? OR key LIKE ?",
            (
                format!("release-artists:{market}:%"),
                format!("tag-review:{market}:%"),
            ),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut legacy = HashMap::new();
    let mut current = HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        let raw: String = row.get(1).map_err(|e| e.to_string())?;
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let id = key.rsplit(':').next().unwrap_or("").to_owned();
        if key.starts_with("release-artists:") {
            current.insert(id, value);
        } else if let Some(ids) = value["album_artist_ids"]
            .as_array()
            .filter(|v| !v.is_empty())
        {
            let ids: Vec<String> = ids
                .iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .or_else(|| v.as_u64().map(|v| v.to_string()))
                })
                .collect();
            if !ids.is_empty() {
                legacy.insert(id,json!({"ids":ids,"checked_at":value["tag_checked_at"].as_f64().unwrap_or(0.0) as i64,"source":"saved album credits"}));
            }
        }
    }
    // A later metadata refresh can carry fresher credits than this narrow lookup.
    for (id, value) in current {
        let saved_at = legacy
            .get(&id)
            .and_then(|v| v["checked_at"].as_i64())
            .unwrap_or(0);
        if value["checked_at"].as_i64().unwrap_or(0) >= saved_at {
            legacy.insert(id, value);
        }
    }
    Ok(legacy)
}

pub fn classify(value: Option<&Value>, linked: &std::collections::HashSet<String>) -> &'static str {
    match value
        .and_then(|v| v["ids"].as_array())
        .and_then(|a| a.first())
        .and_then(Value::as_str)
    {
        Some(id) if linked.contains(id) => "My album artists",
        Some(_) => "Other artist appearances",
        None => "Artist credits not checked",
    }
}

pub fn parse(payload: &Value, requested: &[String], now: i64) -> HashMap<String, Value> {
    let mut result = HashMap::new();
    for album in payload["data"].as_array().into_iter().flatten() {
        let Some(id) = album["id"]
            .as_str()
            .filter(|id| requested.iter().any(|r| r == id))
        else {
            continue;
        };
        let ids: Vec<_> = album["relationships"]["artists"]["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["id"].as_str())
            .collect();
        let names: Vec<_> = ids
            .iter()
            .map(|id| {
                payload["included"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|v| v["type"] == "artists" && v["id"] == *id)
                    .and_then(|v| v["attributes"]["name"].as_str())
                    .unwrap_or("")
            })
            .collect();
        result.insert(
            id.to_owned(),
            json!({"ids":ids,"names":names,"checked_at":now,"source":"album artist relationship"}),
        );
    }
    result
}

pub async fn refresh(
    db: &TursoDb,
    state: &Arc<Backend>,
    args: &Value,
    cancel: Arc<AtomicBool>,
) -> Result<Value, String> {
    let settings = db.get_settings().await?;
    let market = settings["general"]["market"].as_str().unwrap_or("GB");
    state.progress_for(
        "release_artists",
        "Checking saved album artist credits · no file changes",
    );
    let mut page = db
        .get_missing_rows(
            market,
            Some(
                args["timeline"]
                    .as_str()
                    .unwrap_or("Newer than newest owned"),
            ),
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            usize::MAX,
        )
        .await?;
    if let Some(ids) = args["ids"].as_array() {
        page.rows
            .retain(|row| ids.iter().any(|id| id.as_str() == Some(row.id.as_str())));
    }
    let cache = cached(db, market).await?;
    let now = chrono::Utc::now().timestamp();
    let mut pending: Vec<_> = page
        .rows
        .iter()
        .filter(|r| {
            cache.get(&r.id).is_none_or(|v| {
                let lifetime = if v["ids"].as_array().is_some_and(|a| !a.is_empty()) {
                    90 * 86400
                } else {
                    86400
                };
                now - v["checked_at"].as_i64().unwrap_or(0) >= lifetime
            })
        })
        .map(|r| r.id.clone())
        .collect();
    pending.sort();
    pending.dedup();
    let total = page.rows.len();
    let reused = total - pending.len();
    let mut checked = reused;
    if !pending.is_empty() {
        let mut client = TidalClient::from_db(db).await?;
        for batch in pending.chunks(20) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            state.progress_for("release_artists", &format!("Album artist credits · {checked}/{total} releases · {reused} reused · requesting {} release summaries", batch.len()));
            let mut url = url::Url::parse("https://openapi.tidal.com/v2/albums")
                .map_err(|e| e.to_string())?;
            url.query_pairs_mut()
                .append_pair("filter[id]", &batch.join(","))
                .append_pair("countryCode", market)
                .append_pair("include", "artists");
            let payload = client.get_json(url.as_str()).await?;
            let values = parse(&payload, batch, now);
            for id in batch {
                let value = values.get(id).cloned().unwrap_or_else(
                    || json!({"ids":[],"checked_at":now,"source":"not returned for this market"}),
                );
                db.set_preference(&format!("release-artists:{market}:{id}"), &value)
                    .await?;
                checked += 1;
            }
            db.invalidate_missing_rows();
        }
    }
    state.progress_for("release_artists", &format!("Album artist credits saved · {checked}/{total} releases · {reused} reused · rerun to continue any unfinished checks"));
    Ok(json!({"checked":checked,"total":total,"reused":reused}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credits_keep_order_and_unknowns_separate() {
        let values = parse(
            &json!({"data":[{"id":"1","relationships":{"artists":{"data":[{"id":"other"},{"id":"mine"}]}}},{"id":"2","relationships":{"artists":{"data":[{"id":"mine"}]}}}]}),
            &["1".into(), "2".into()],
            1,
        );
        let linked = std::collections::HashSet::from(["mine".into()]);
        assert_eq!(
            classify(values.get("1"), &linked),
            "Other artist appearances"
        );
        assert_eq!(classify(values.get("2"), &linked), "My album artists");
        assert_eq!(classify(None, &linked), "Artist credits not checked");
    }
}
