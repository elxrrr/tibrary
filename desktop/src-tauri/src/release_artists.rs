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
    let current_prefix = format!("release-artists:{market}:");
    // BINARY key ranges use the preference primary key. OR/LIKE previously
    // walked the whole preference table and transferred every credited track
    // list merely to read four album-credit fields.
    let mut rows = conn
        .query(
            "SELECT key,payload FROM app_preferences WHERE key>=? AND key<?",
            (current_prefix.as_str(), format!("release-artists:{market};")),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut current = HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        let raw: String = row.get(1).map_err(|e| e.to_string())?;
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let id = key.rsplit(':').next().unwrap_or("").to_owned();
        current.insert(id, value);
    }
    drop(rows);
    let legacy_prefix = format!("tag-review:{market}:");
    let mut rows = conn.query(
        "SELECT key,CASE WHEN json_valid(payload) THEN json_extract(payload,'$.album_artist_ids','$.album_artist','$.artist','$.tag_checked_at') END FROM app_preferences WHERE key>=? AND key<?",
        (legacy_prefix.as_str(),format!("tag-review:{market};")),
    ).await.map_err(|e|e.to_string())?;
    while let Some(row) = rows.next().await.map_err(|e|e.to_string())? {
        let key: String = row.get(0).map_err(|e|e.to_string())?;
        let Some(raw) = row.get::<Option<String>>(1).map_err(|e|e.to_string())? else {continue;};
        let Ok(fields) = serde_json::from_str::<Value>(&raw) else {continue;};
        let id = key.rsplit(':').next().unwrap_or("").to_owned();
        // Preserve a newer narrow lookup without copying/normalising older
        // metadata. A genuinely newer full metadata refresh still wins.
        let checked_at = fields[3].as_f64().unwrap_or(0.0) as i64;
        if current.get(&id).is_some_and(|value|value["checked_at"].as_i64().unwrap_or(0) >= checked_at) {continue;}
        if let Some(ids) = fields[0]
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
                current.insert(id,json!({"ids":ids,"names":[fields[1].as_str().or_else(||fields[2].as_str()).unwrap_or("")],"checked_at":checked_at,"source":"saved album credits"}));
            }
        }
    }
    Ok(current)
}

/// Resolve display names once per table read from album-level caches. Never use
/// the discovery page artist or a track performer as the release's album artist.
pub async fn album_names(db: &TursoDb, market: &str) -> Result<HashMap<String, String>, String> {
    let conn=db.connect()?;
    let mut rows=conn.query("SELECT key,CASE WHEN json_valid(payload) THEN json_extract(payload,'$.artist.name','$.artists[0].name') END FROM app_preferences WHERE key>=? AND key<? ORDER BY key",
        (format!("subscriber-summary:{market}:"),format!("subscriber-summary:{market};"))).await.map_err(|e|e.to_string())?;
    let mut names=HashMap::new();
    while let Some(row)=rows.next().await.map_err(|e|e.to_string())? {
        let key:String=row.get(0).map_err(|e|e.to_string())?;
        let Some(payload)=row.get::<Option<String>>(1).map_err(|e|e.to_string())? else {continue;};
        let Ok(value)=serde_json::from_str::<Value>(&payload) else {continue};
        let name=value[0].as_str().or_else(||value[1].as_str());
        if let Some(name)=name.filter(|n|!n.trim().is_empty()) {
            names.entry(key.rsplit(':').next().unwrap_or("").to_owned()).or_insert_with(||name.trim().to_owned());
        }
    }
    Ok(names)
}

pub fn display_name(release: &Value, saved: Option<&str>, credits: Option<&Value>) -> String {
    saved.or_else(|| release["album_artist"].as_str())
        .or_else(||credits?.get("names")?.as_array()?.first()?.as_str())
        .filter(|v|!v.trim().is_empty())
        .or_else(||release["artist"].as_str().filter(|v|!v.trim().is_empty()))
        .unwrap_or("Unknown album artist").trim().to_owned()
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
            let payload = client.albums(batch,market,true).await?;
            let values = parse(&payload, batch, now);
            crate::availability::save_response(db, &payload, batch, market).await?;
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
    #[tokio::test]
    async fn projected_cache_preserves_newer_legacy_credits_and_album_display_names() {
        let dir=std::env::temp_dir().join(format!("release-credit-projection-{}",uuid::Uuid::new_v4()));
        let db=TursoDb::open(dir.join("db")).await.unwrap();
        for (id,current_at,legacy_at) in [("1",10,20),("2",20,10),("3",20,20)] {
            db.set_preference(&format!("release-artists:GB:{id}"),&json!({"ids":["current"],"names":["Current name"],"checked_at":current_at})).await.unwrap();
            db.set_preference(&format!("tag-review:GB:{id}"),&json!({"album_artist_ids":[7,"legacy"],"album_artist":"Legacy name","artist":"Fallback name","tag_checked_at":legacy_at,"tracks":[{"large_unneeded_field":"ignored"}]})).await.unwrap();
        }
        db.set_preference("tag-review:GB:4",&json!({"album_artist_ids":[8],"artist":"Fallback name","tag_checked_at":42.9})).await.unwrap();
        db.set_preference("tag-review:GB:5",&json!({"album_artist_ids":[false,null],"tag_checked_at":50})).await.unwrap();
        db.set_preference("tag-review:US:6",&json!({"album_artist_ids":[9],"tag_checked_at":50})).await.unwrap();
        db.set_preference("subscriber-summary:GB:1",&json!({"artist":{"name":"Album owner"},"artists":[{"name":"Guest"}],"unneeded":"metadata"})).await.unwrap();
        db.set_preference("subscriber-summary:GB:2",&json!({"artists":[{"name":"Fallback album owner"}]})).await.unwrap();
        let conn=db.connect().unwrap();
        conn.execute("INSERT INTO app_preferences(key,payload) VALUES('tag-review:GB:broken','invalid JSON')",()).await.unwrap();
        conn.execute("INSERT INTO app_preferences(key,payload) VALUES('subscriber-summary:GB:broken','invalid JSON')",()).await.unwrap();
        let credits=cached(&db,"GB").await.unwrap();
        assert_eq!(credits.len(),4);
        assert_eq!(credits["1"]["ids"],json!(["7","legacy"]));
        assert_eq!(credits["1"]["names"],json!(["Legacy name"]));
        assert_eq!(credits["2"]["ids"],json!(["current"]));
        assert_eq!(credits["3"]["ids"],json!(["current"]),"Narrow cache wins a timestamp tie");
        assert_eq!(credits["4"]["names"],json!(["Fallback name"]));
        assert_eq!(credits["4"]["checked_at"],42);
        let names=album_names(&db,"GB").await.unwrap();
        assert_eq!(names,HashMap::from([("1".into(),"Album owner".into()),("2".into(),"Fallback album owner".into())]));
        let mut plan=conn.query("EXPLAIN QUERY PLAN SELECT key FROM app_preferences WHERE key>=? AND key<?",("tag-review:GB:","tag-review:GB;")).await.unwrap();
        let detail:String=plan.next().await.unwrap().unwrap().get(3).unwrap();
        assert!(detail.contains("SEARCH"),"The preference primary key must bound the lookup: {detail}");
        drop(plan);drop(conn);drop(db);std::fs::remove_dir_all(dir).unwrap();
    }

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

#[cfg(test)]
mod display_tests {
    use super::*;
    #[test]
    fn album_cache_precedes_guest_and_discovery_names() {
        let release=json!({"artist":"Featured performer","tracks":[{"artist":"Guest"}]});
        assert_eq!(display_name(&release,Some("Album owner"),None),"Album owner");
        assert_eq!(display_name(&release,None,Some(&json!({"names":["Album owner","Guest"]}))),"Album owner");
        assert_eq!(display_name(&json!({"tracks":[{"artist":"Guest"}]}),None,None),"Unknown album artist");
    }
}
