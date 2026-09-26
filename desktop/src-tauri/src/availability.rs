//! Market-specific, bounded availability checks; transport failures are never removals.
use crate::{db::TursoDb, tidal::TidalClient};
use serde_json::{json, Value};
use std::collections::HashMap;

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
    let now = chrono::Utc::now().timestamp();
    let mut result = HashMap::new();
    let mut pending = vec![];
    for id in ids {
        if result.contains_key(id) || pending.contains(id) {
            continue;
        }
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            result.insert(id.clone(), None);
            continue;
        }
        let saved = db
            .get_preference(&format!("release-live:{market}:{id}"))
            .await?;
        if let Some(v) = saved.filter(|v| {
            !force && {
                now - v["checked_at"].as_i64().unwrap_or(0)
                    < if v["available"] == true {
                        7 * 86400
                    } else {
                        86400
                    }
            }
        }) {
            result.insert(id.clone(), v["available"].as_bool());
        } else {
            pending.push(id.clone());
        }
    }
    if pending.is_empty() {
        return Ok(result);
    }
    let mut client = TidalClient::from_db(db).await?;
    for batch in pending.chunks(20) {
        let mut url =
            url::Url::parse("https://openapi.tidal.com/v2/albums").map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("countryCode", market)
            .append_pair("filter[id]", &batch.join(","))
            .append_pair("include", "artists");
        let raw = client.get_json(url.as_str()).await?;
        let resources = raw["data"]
            .as_array()
            .ok_or("Invalid release availability response")?;
        for id in batch {
            let resource = resources
                .iter()
                .find(|r| r["id"].as_str() == Some(id.as_str()));
            // A successful market-filtered response omitting an ID means it is not available there.
            let status = resource.map(available).unwrap_or(Some(false));
            if status.is_some() {
                db.set_preference(
                    &format!("release-live:{market}:{id}"),
                    &json!({"available":status,"checked_at":now}),
                )
                .await?;
            }
            result.insert(id.clone(), status);
        }
        for (id, credits) in crate::release_artists::parse(&raw, batch, now) {
            db.set_preference(&format!("release-artists:{market}:{id}"), &credits)
                .await?;
        }
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
        let result = check(&db, &["470388645".into(), "234657671".into()], "GB")
            .await
            .unwrap();
        assert_eq!(result["470388645"], Some(true));
        assert_eq!(
            check(&db, &["470388645".into()], "GB").await.unwrap()["470388645"],
            Some(true)
        );
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
}
