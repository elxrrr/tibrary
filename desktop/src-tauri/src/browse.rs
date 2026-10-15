//! Interactive catalogue browsing, independent from library scanning.
//! Pages have short, market-scoped caches. Authoritative summaries and complete
//! credited track lists use the same caches as linking and downloads, without
//! adding browsed artists to the user's library catalogue.
use crate::{
    db::TursoDb,
    tidal::{resource_id, TidalClient},
};
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::AtomicBool, Arc},
};

const PAGE_TTL: i64 = 15 * 60;
const ARTIST_TTL: i64 = 6 * 3600;
const MAX_QUEUE_RELEASES: usize = 50;

async fn market(db: &TursoDb) -> Result<String, String> {
    crate::account::catalogue_market(db, None).await
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

fn requested_id(args: &Value) -> Result<&str, String> {
    args["id"]
        .as_str()
        .filter(|id| valid_id(id))
        .ok_or_else(|| "Choose a valid catalogue item".into())
}

fn pagination(args: &Value) -> Result<(usize, usize), String> {
    let offset = args["offset"].as_u64().unwrap_or(0);
    if offset > 100_000 {
        return Err("Catalogue page is outside the supported range".into());
    }
    Ok((
        offset as usize,
        args["limit"].as_u64().unwrap_or(24).clamp(1, 50) as usize,
    ))
}

fn current(cached: &Value, ttl: i64) -> bool {
    cached["checked_at"]
        .as_i64()
        .is_some_and(|at| (0..ttl).contains(&(Utc::now().timestamp() - at)))
}

fn envelope(mut value: Value, at: i64, source: &str, stale: bool, warning: Option<&str>) -> Value {
    value["cached_at"] = json!(at);
    value["source"] = json!(source);
    value["stale"] = json!(stale);
    if let Some(warning) = warning {
        value["warning"] = json!(warning);
    }
    value
}

async fn save_page(db: &TursoDb, key: &str, value: Value) -> Result<Value, String> {
    let at = Utc::now().timestamp();
    db.set_preference(key, &json!({"checked_at":at,"value":value}))
        .await?;
    // Browsing history is disposable, unlike shared metadata. Bound only these
    // page snapshots so repeated searches cannot grow the database indefinitely.
    let conn = db.connect()?;
    conn.execute("DELETE FROM app_preferences WHERE key IN (SELECT key FROM app_preferences WHERE key LIKE 'browse:v1:%' ORDER BY CAST(json_extract(payload,'$.checked_at') AS INTEGER) DESC,key LIMIT -1 OFFSET 400)", ()).await.map_err(|e|e.to_string())?;
    Ok(envelope(value, at, "online", false, None))
}

fn saved_page(cached: &Value, stale: bool, warning: Option<&str>) -> Value {
    envelope(
        cached["value"].clone(),
        cached["checked_at"].as_i64().unwrap_or(0),
        "cache",
        stale,
        warning,
    )
}

fn availability(raw: &Value) -> Option<bool> {
    match (
        raw["allowStreaming"].as_bool(),
        raw["streamReady"].as_bool(),
    ) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => raw["available"].as_bool(),
    }
}

fn artist_values(raw: &Value) -> Vec<Value> {
    let mut seen = HashSet::new();
    std::iter::once(&raw["artist"])
        .chain(raw["artists"].as_array().into_iter().flatten())
        .filter_map(|artist| {
            let id = resource_id(&artist["id"]);
            let name = artist["name"].as_str()?;
            (valid_id(&id) && seen.insert(id.clone()))
                .then(|| json!({"id":id,"name":name,"picture":artist["picture"]}))
        })
        .collect()
}

fn artist_card(raw: &Value) -> Option<Value> {
    let id = resource_id(&raw["id"]);
    let name = raw["name"].as_str()?;
    valid_id(&id).then(|| json!({"id":id,"name":name,"picture":raw["picture"]}))
}

fn release_card(raw: &Value) -> Option<Value> {
    let id = resource_id(&raw["id"]);
    if !valid_id(&id) || !raw["title"].is_string() {
        return None;
    }
    let artists = artist_values(raw);
    let title =
        crate::tidal::format_title(raw["title"].as_str().unwrap_or(""), raw["version"].as_str());
    Some(
        json!({"id":id,"title":title,"artist":artists.first().map(|a|&a["name"]),
        "artist_id":artists.first().map(|a|&a["id"]),"artists":artists,
        "date":raw["releaseDate"],"release_date":raw["releaseDate"],"type":raw["type"],"release_type":raw["type"],
        "track_count":raw["numberOfTracks"],"cover":raw["cover"],"quality":raw["audioQuality"],
        "explicit":raw["explicit"],"available":availability(raw),"ai":raw["ai"].as_bool()}),
    )
}

fn track_card(raw: &Value) -> Option<Value> {
    let id = resource_id(&raw["id"]);
    let album_id = resource_id(&raw["album"]["id"]);
    if !valid_id(&id) || !valid_id(&album_id) || !raw["title"].is_string() {
        return None;
    }
    let artists = artist_values(raw);
    Some(
        json!({"id":id,"title":crate::tidal::format_title(raw["title"].as_str().unwrap_or(""),raw["version"].as_str()),
        "artist":artists.iter().filter_map(|a|a["name"].as_str()).collect::<Vec<_>>().join(", "),"artists":artists,
        "album_id":album_id,"release_id":album_id,"album":raw["album"]["title"],"release_title":raw["album"]["title"],
        "cover":raw["album"]["cover"],"disc_number":raw["volumeNumber"],"track_number":raw["trackNumber"],
        "duration":raw["duration"],"isrc":raw["isrc"],"explicit":raw["explicit"],"available":availability(raw),"ai":raw["ai"].as_bool()}),
    )
}

async fn cache_summaries(
    db: &TursoDb,
    client: &TidalClient,
    raw: &[Value],
    market: &str,
    source: &str,
) -> Result<(), String> {
    let now = Utc::now().timestamp();
    let ids: Vec<_> = raw
        .iter()
        .map(|value| resource_id(&value["id"]))
        .filter(|id| valid_id(id))
        .collect();
    let conn = db.connect()?;
    let mut rows = conn.query("SELECT key,payload FROM app_preferences WHERE key IN (SELECT 'subscriber-summary:' || ? || ':' || value FROM json_each(?))", (market,json!(ids).to_string())).await.map_err(|e|e.to_string())?;
    let mut prior = HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        let text: String = row.get(1).map_err(|e| e.to_string())?;
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            prior.insert(key.rsplit(':').next().unwrap_or("").to_owned(), value);
        }
    }
    drop(rows);
    let items: Vec<_> = raw
        .iter()
        .filter(|raw| valid_id(&resource_id(&raw["id"])) && raw["title"].is_string())
        .map(|raw| {
            let mut raw = raw.clone();
            if let Some(old) = prior.get(&resource_id(&raw["id"])) {
                // Search/list endpoints omit optional fields. Preserve these
                // facts without importing old availability or primary artists.
                for field in [
                    "upc",
                    "copyright",
                    "label",
                    "genres",
                    "audioModes",
                    "mediaMetadata",
                    "originalReleaseDate",
                    "replacementId",
                    "official",
                    "secondaryTypes",
                    "releaseGroupId",
                ] {
                    if raw.get(field).is_none() && !old[field].is_null() {
                        raw[field] = old[field].clone();
                    }
                }
            }
            raw["checked_at"] = json!(now);
            raw
        })
        .collect();
    client.cache_summaries(&items, market, &[], source).await?;
    // Direct writes in TidalClient update the derived AI index by DB triggers.
    // A browser lookup must never save a full artist catalogue or mapping.
    Ok(())
}

async fn cache_track_search(db: &TursoDb, raw: &[Value], market: &str) -> Result<(), String> {
    let conn = db.connect()?;
    let now = Utc::now().timestamp();
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(|e| e.to_string())?;
    let result = async {
        for track in raw {
            let id = resource_id(&track["id"]);
            let album = &track["album"];
            let album_id = resource_id(&album["id"]);
            if !valid_id(&id) || !valid_id(&album_id) {
                continue;
            }
            let mut track = track.clone();
            track["checked_at"] = json!(now);
            conn.execute(
                "INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)",
                (format!("browse-track:{market}:{id}"), track.to_string()),
            )
            .await
            .map_err(|e| e.to_string())?;
            // Nested track-search albums omit primary artist and availability;
            // only retain their cover, never replace the complete album cache.
            if album["cover"].is_string() {
                conn.execute(
                    "INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)",
                    (
                        format!("browse-cover:{market}:{album_id}"),
                        json!({"id":album_id,"cover":album["cover"]}).to_string(),
                    ),
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        conn.execute("COMMIT", ())
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(())
    }
    .await;
    if result.is_err() {
        let _ = conn.execute("ROLLBACK", ()).await;
    }
    result
}

/// Apply current safety observations even when the displayed page is cached.
/// Excluded cards do not alter provider pagination offsets or totals.
async fn filter_cards(
    db: &TursoDb,
    market: &str,
    items: &mut Vec<Value>,
    kind: &str,
) -> Result<(), String> {
    let ai = crate::ai_policy::flags(db, market).await?;
    let ids: Vec<_> = items
        .iter()
        .map(|v| {
            if kind == "tracks" {
                resource_id(&v["album_id"])
            } else {
                resource_id(&v["id"])
            }
        })
        .collect();
    let conn = db.connect()?;
    let mut live = conn.query("SELECT key,payload FROM app_preferences WHERE key IN (SELECT 'release-live:' || ? || ':' || value FROM json_each(?))", (market,json!(ids).to_string())).await.map_err(|e|e.to_string())?;
    let mut unavailable = HashSet::new();
    while let Some(row) = live.next().await.map_err(|e| e.to_string())? {
        let raw: String = row.get(1).map_err(|e| e.to_string())?;
        if serde_json::from_str::<Value>(&raw)
            .ok()
            .is_some_and(|v| v["available"] == false)
        {
            let key: String = row.get(0).map_err(|e| e.to_string())?;
            unavailable.insert(key.rsplit(':').next().unwrap_or("").to_owned());
        }
    }
    drop(live);
    items.retain(|v| {
        let id = resource_id(&v["id"]);
        let release = if kind == "tracks" {
            resource_id(&v["album_id"])
        } else {
            id.clone()
        };
        v["ai"] != true
            && v["available"] != false
            && !ai.release_blocked(&release)
            && !ai.track_labelled(&id)
            && !unavailable.contains(&release)
    });
    // Queue badges use one small indexed read for the visible page.
    let mut queue = conn.query("SELECT id,payload,approved FROM queue WHERE decision='queued' AND id IN (SELECT value FROM json_each(?))", (json!(ids).to_string(),)).await.map_err(|e|e.to_string())?;
    let mut queued = HashMap::new();
    while let Some(row) = queue.next().await.map_err(|e| e.to_string())? {
        if row.get::<i64>(2).unwrap_or(0) != 1 {
            continue;
        }
        let raw: String = row.get(1).map_err(|e| e.to_string())?;
        queued.insert(
            row.get::<String>(0).map_err(|e| e.to_string())?,
            serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null),
        );
    }
    for card in items {
        let id = if kind == "tracks" {
            resource_id(&card["album_id"])
        } else {
            resource_id(&card["id"])
        };
        if let Some(payload) = queued.get(&id) {
            let selection = payload["selected_tracks"]
                .as_array()
                .or_else(|| payload["tracks"].as_array());
            card["queued_tracks"] = json!(selection.map(Vec::len).unwrap_or(0));
            if kind == "tracks" {
                card["queued"] = json!(selection.is_some_and(|tracks| tracks
                    .iter()
                    .any(|t| resource_id(&t["id"]) == resource_id(&card["id"]))));
            }
        }
    }
    Ok(())
}

async fn filter_page(
    db: &TursoDb,
    market: &str,
    mut page: Value,
    kind: &str,
) -> Result<Value, String> {
    if kind != "artists" {
        if let Some(items) = page["items"].as_array_mut() {
            filter_cards(db, market, items, kind).await?;
        }
        if let Some(items) = page["releases"].as_array_mut() {
            filter_cards(db, market, items, "albums").await?;
        }
    }
    page["market"] = json!(market);
    Ok(page)
}

pub(crate) async fn search(db: &TursoDb, args: &Value) -> Result<Value, String> {
    let query = args["query"].as_str().unwrap_or("").trim();
    if query.is_empty() || query.chars().count() > 200 {
        return Err("Enter a search between 1 and 200 characters".into());
    }
    let kind = args["kind"].as_str().unwrap_or("albums");
    if !matches!(kind, "albums" | "artists" | "tracks") {
        return Err("Choose releases, artists or tracks".into());
    }
    let (offset, limit) = pagination(args)?;
    let market = market(db).await?;
    use sha2::{Digest, Sha256};
    let fingerprint = format!("{:x}", Sha256::digest(query.as_bytes()));
    let key = format!("browse:v1:{market}:search:{kind}:{offset}:{limit}:{fingerprint}");
    let gate = crate::actions::release_gate(format!("{}:{key}", db.path.display()));
    let _guard = gate.lock().await;
    let saved = db.get_preference(&key).await?;
    if args["refresh"] != true {
        if let Some(saved) = saved.as_ref().filter(|v| current(v, PAGE_TTL)) {
            return filter_page(db, &market, saved_page(saved, false, None), kind).await;
        }
    }
    let fetched = async {
        let mut client = TidalClient::from_db(db).await?;
        let payload = client.request(&format!("search/{kind}"),&market,&[("query",query.into()),("offset",offset.to_string()),("limit",limit.to_string())]).await?;
        let raw = payload["items"].as_array().ok_or("Catalogue search returned no item list")?;
        let mut seen = HashSet::new();
        let items: Vec<_> = raw.iter().filter_map(|v| match kind {"artists"=>artist_card(v),"tracks"=>track_card(v),_=>release_card(v)})
            .filter(|v|seen.insert(resource_id(&v["id"]))).collect();
        if kind == "albums" { cache_summaries(db,&client,raw,&market,"artist_list").await?; }
        if kind == "tracks" { cache_track_search(db,raw,&market).await?; }
        let total = payload["totalNumberOfItems"].as_u64().unwrap_or((offset + raw.len()) as u64);
        save_page(db,&key,json!({"items":items,"total":total,"offset":offset,"limit":limit,"has_more":offset + raw.len() < total as usize,"query":query,"kind":kind})).await
    }.await;
    match fetched {
        Ok(value) => filter_page(db, &market, value, kind).await,
        Err(error) => match saved {
            Some(saved) => {
                filter_page(
                    db,
                    &market,
                    saved_page(&saved, true, Some(&format!("Saved results · {error}"))),
                    kind,
                )
                .await
            }
            None => Err(error),
        },
    }
}

pub(crate) async fn artist(db: &TursoDb, args: &Value) -> Result<Value, String> {
    let id = requested_id(args)?;
    let (offset, limit) = pagination(args)?;
    let filter = args["filter"].as_str().unwrap_or("ALBUMS");
    if !matches!(filter, "ALBUMS" | "EPSANDSINGLES" | "COMPILATIONS") {
        return Err("Choose a supported release type".into());
    }
    let market = market(db).await?;
    let key = format!("browse:v1:{market}:artist:{id}:{filter}:{offset}:{limit}");
    let gate = crate::actions::release_gate(format!("{}:{key}", db.path.display()));
    let _guard = gate.lock().await;
    let saved = db.get_preference(&key).await?;
    if args["refresh"] != true {
        if let Some(saved) = saved.as_ref().filter(|v| current(v, ARTIST_TTL)) {
            return filter_page(db, &market, saved_page(saved, false, None), "albums").await;
        }
    }
    let fetched = async {
        let mut client = TidalClient::from_db(db).await?;
        let profile_key = format!("subscriber-artist:{market}:{id}");
        let profile = db.get_preference(&profile_key).await?;
        let profile = match profile.filter(|p|args["refresh"] != true && current(p,ARTIST_TTL)) {
            Some(profile) => profile,
            None => {
                let mut profile = client.request(&format!("artists/{id}"),&market,&[]).await?;
                if profile["name"].as_str().is_none() { return Err("The artist profile could not be read".into()); }
                profile["checked_at"] = json!(Utc::now().timestamp());
                db.set_preference(&profile_key,&profile).await?;
                profile
            },
        };
        let mut params = vec![("offset",offset.to_string()),("limit",limit.to_string())];
        if filter != "ALBUMS" { params.push(("filter",filter.into())); }
        let payload = client.request(&format!("artists/{id}/albums"),&market,&params).await?;
        let raw = payload["items"].as_array().ok_or("Artist release list could not be read")?;
        cache_summaries(db,&client,raw,&market,"artist_list").await?;
        let mut seen = HashSet::new();
        let releases: Vec<_> = raw.iter().filter_map(release_card).filter(|v|seen.insert(resource_id(&v["id"]))).collect();
        let total = payload["totalNumberOfItems"].as_u64().unwrap_or((offset + raw.len()) as u64);
        save_page(db,&key,json!({"artist":{"id":id,"name":profile["name"],"picture":profile["picture"]},"releases":releases,"total":total,"offset":offset,"limit":limit,"has_more":offset + raw.len() < total as usize,"filter":filter})).await
    }.await;
    match fetched {
        Ok(value) => filter_page(db, &market, value, "albums").await,
        Err(error) => match saved {
            Some(saved) => {
                filter_page(
                    db,
                    &market,
                    saved_page(&saved, true, Some(&format!("Saved releases · {error}"))),
                    "albums",
                )
                .await
            }
            None => Err(error),
        },
    }
}

fn home_sections(payload: &Value) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let mut sections = Vec::new();
    let mut albums = Vec::new();
    let mut tracks = Vec::new();
    for module in payload["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row["modules"].as_array().into_iter().flatten())
    {
        let Some(raw) = module["pagedList"]["items"].as_array() else {
            continue;
        };
        let title = module["title"].as_str().unwrap_or("Explore");
        match module["type"].as_str() {
            Some("ALBUM_LIST") => {
                let releases: Vec<_> = raw.iter().take(24).filter_map(release_card).collect();
                if !releases.is_empty() {
                    sections.push(json!({"id":format!("home-{}",sections.len()),"title":title,"releases":releases}));
                }
                albums.extend(raw.iter().take(24).cloned());
            }
            Some("TRACK_LIST") => {
                let items: Vec<_> = raw.iter().take(12).filter_map(track_card).collect();
                if !items.is_empty() {
                    sections.push(json!({"id":format!("home-{}",sections.len()),"title":title,"tracks":items}));
                }
                tracks.extend(raw.iter().take(12).cloned());
            }
            _ => {}
        }
        if sections.len() >= 6 {
            break;
        }
    }
    (sections, albums, tracks)
}

async fn home_local(db: &TursoDb, market: &str, account: &str) -> Result<Vec<Value>, String> {
    let conn = db.connect()?;
    let mut sections = Vec::new();
    let mut rows = conn
        .query(
            "SELECT payload FROM favourite_artists WHERE cache_id=? LIMIT 1",
            (account,),
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let saved: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        let artists: Vec<_> = saved
            .as_array()
            .into_iter()
            .flatten()
            .take(24)
            .filter_map(artist_card)
            .collect();
        if !artists.is_empty() {
            sections.push(
                json!({"id":"favourites","title":"Your favourite artists","artists":artists}),
            );
        }
    }
    drop(rows);
    let mut rows = conn.query("SELECT payload FROM app_preferences WHERE key LIKE ? ORDER BY CAST(json_extract(payload,'$.checked_at') AS INTEGER) DESC LIMIT 12",(format!("browse:v1:{market}:release:%"),)).await.map_err(|e|e.to_string())?;
    let mut releases = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        if let Ok(saved) = serde_json::from_str::<Value>(&raw) {
            if saved["value"]["release"].is_object() {
                let mut card = saved["value"]["release"].clone();
                card.as_object_mut().unwrap().remove("tracks");
                releases.push(card);
            }
        }
    }
    if !releases.is_empty() {
        sections.push(json!({"id":"recent","title":"Recently viewed","releases":releases}));
    }
    Ok(sections)
}

pub(crate) async fn home(db: &TursoDb, args: &Value) -> Result<Value, String> {
    let market = market(db).await?;
    let session = crate::stream_download::load_saved_token(db).await;
    let connected = session.is_some();
    let saved_account = db
        .get_preference("subscriber-account-market")
        .await?
        .unwrap_or(Value::Null);
    let account = session
        .as_ref()
        .and_then(|session| session.user_id.as_deref())
        .or_else(|| saved_account["user_id"].as_str())
        .unwrap_or("anonymous");
    // Home may be personalised. Public catalogue pages share a country cache,
    // but changing accounts must not reuse another subscriber's home/favourites.
    use sha2::{Digest, Sha256};
    let account_key = format!("{:x}", Sha256::digest(account.as_bytes()));
    let key = format!("browse:v1:{market}:home:{account_key}");
    let gate = crate::actions::release_gate(format!("{}:{key}", db.path.display()));
    let _guard = gate.lock().await;
    let saved = db.get_preference(&key).await?;
    let fetched = if args["refresh"] != true && saved.as_ref().is_some_and(|v| current(v, PAGE_TTL))
    {
        Ok(saved_page(saved.as_ref().unwrap(), false, None))
    } else if connected {
        async {
            let mut client = TidalClient::from_db(db).await?;
            let locale = format!("en_{market}");
            let payload = client.request("pages/home",&market,&[("deviceType","BROWSER".into()),("locale",locale)]).await?;
            let (sections,albums,tracks) = home_sections(&payload);
            if sections.is_empty() { return Err("The catalogue home page has no supported music sections. Search for an artist, release or track.".to_owned()); }
            cache_summaries(db,&client,&albums,&market,"artist_list").await?;
            cache_track_search(db,&tracks,&market).await?;
            save_page(db,&key,json!({"sections":sections})).await
        }.await
    } else {
        Err("Connect your account to browse the online catalogue".into())
    };
    let mut value = match fetched {
        Ok(value) => value,
        Err(error) => match saved {
            Some(saved) => saved_page(&saved, true, Some(&format!("Saved catalogue · {error}"))),
            None => envelope(json!({"sections":[]}), 0, "cache", true, Some(&error)),
        },
    };
    let mut sections = home_local(db, &market, account).await?;
    sections.extend(value["sections"].as_array().into_iter().flatten().cloned());
    for section in &mut sections {
        if let Some(releases) = section["releases"].as_array_mut() {
            filter_cards(db, &market, releases, "albums").await?;
        }
        if let Some(tracks) = section["tracks"].as_array_mut() {
            filter_cards(db, &market, tracks, "tracks").await?;
        }
    }
    sections.retain(|section| {
        ["releases", "artists", "tracks"]
            .iter()
            .any(|key| section[*key].as_array().is_some_and(|v| !v.is_empty()))
    });
    value["sections"] = json!(sections);
    value["connected"] = json!(connected);
    value["market"] = json!(market);
    Ok(value)
}

async fn release_safety(db: &TursoDb, market: &str, mut value: Value) -> Result<Value, String> {
    let id = resource_id(&value["release"]["id"]);
    let current_summary = db
        .get_preference(&format!("subscriber-summary:{market}:{id}"))
        .await?;
    if value["release"]["summary_fingerprint"]
        .as_str()
        .zip(current_summary.as_ref())
        .is_some_and(|(pinned, current)| pinned != crate::tidal::summary_fingerprint(current))
    {
        value["stale"] = json!(true);
        value["warning"] = json!("The release changed. Refresh its details before queueing.");
    }
    let stale = value["stale"] == true;
    let release = &mut value["release"];
    let id = resource_id(&release["id"]);
    let flags = crate::ai_policy::flags(db, market).await?;
    if flags.payload_blocked(&id, release) {
        release["ai"] = json!(true);
    }
    if db
        .get_preference(&format!("release-live:{market}:{id}"))
        .await?
        .is_some_and(|v| v["available"] == false)
    {
        release["available"] = json!(false);
    }
    release["downloadable"] =
        json!(release["ai"] != true && release["available"] == true && !stale);
    // Approval changes are independent of metadata freshness: read only this
    // queue row so returning to a cached release shows its current selection.
    let conn = db.connect()?;
    let mut rows = conn
        .query(
            "SELECT payload FROM queue WHERE id=? AND approved=1 AND decision='queued'",
            (id.as_str(),),
        )
        .await
        .map_err(|e| e.to_string())?;
    let queued: HashSet<String> = match rows.next().await.map_err(|e| e.to_string())? {
        Some(row) => {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let payload: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            payload["selected_tracks"]
                .as_array()
                .or_else(|| payload["tracks"].as_array())
                .into_iter()
                .flatten()
                .map(|track| resource_id(&track["id"]))
                .collect()
        }
        None => HashSet::new(),
    };
    let mut queued_count = 0;
    for track in release["tracks"].as_array_mut().into_iter().flatten() {
        let selected = queued.contains(&resource_id(&track["id"]));
        track["queued"] = json!(selected);
        queued_count += usize::from(selected);
    }
    release["queued_tracks"] = json!(queued_count);
    value["market"] = json!(market);
    Ok(value)
}

fn preserve_extended_metadata(fresh: &mut Value, prior: &Value) {
    if resource_id(&fresh["id"]) != resource_id(&prior["id"]) {
        return;
    }
    // Optional metadata has its own refresh lifecycle. Opening a browser page
    // neither erases it nor claims that an incomplete response checked it.
    for field in [
        "genres",
        "label",
        "copyright",
        "providers",
        "catalogue_metadata_status",
        "catalogue_metadata",
        "catalogue_attributes",
        "discovery_checked_at",
        "original_release_date",
        "release_group_id",
        "primary_type",
        "secondary_types",
        "official",
        "upc",
        "replacement_id",
    ] {
        if (fresh[field].is_null() || fresh[field].as_array().is_some_and(Vec::is_empty))
            && !prior[field].is_null()
        {
            fresh[field] = prior[field].clone();
        }
    }
    if fresh["summary_fingerprint"].is_string()
        && fresh["summary_fingerprint"] == prior["summary_fingerprint"]
    {
        for field in ["recommendation_snapshot", "catalogue_metadata_checked_at"] {
            if !prior[field].is_null() {
                fresh[field] = prior[field].clone();
            }
        }
    }
    let mut tracks: Vec<crate::tidal::TidalTrack> =
        serde_json::from_value(fresh["tracks"].clone()).unwrap_or_default();
    let old: Vec<crate::tidal::TidalTrack> =
        serde_json::from_value(prior["tracks"].clone()).unwrap_or_default();
    crate::subscriber_metadata::supplement(&mut tracks, &old);
    if let Ok(value) = serde_json::to_value(tracks) {
        fresh["tracks"] = value;
    }
}

pub(crate) async fn release(db: &TursoDb, args: &Value) -> Result<Value, String> {
    let id = requested_id(args)?;
    let market = market(db).await?;
    let key = format!("browse:v1:{market}:release:{id}");
    let gate = crate::actions::release_gate(format!("{}:{key}", db.path.display()));
    let _guard = gate.lock().await;
    let metadata_gate =
        crate::actions::release_gate(format!("{}:{market}:{id}", db.path.display()));
    let _metadata_guard = metadata_gate.lock().await;
    let saved = db.get_preference(&key).await?;
    if args["refresh"] != true {
        if let Some(saved) = saved.as_ref().filter(|v| current(v, PAGE_TTL)) {
            return release_safety(db, &market, saved_page(saved, false, None)).await;
        }
    }
    let fetched = async {
        let mut client = TidalClient::from_db(db).await?;
        // A direct lookup confirms this exact ID in the account market. Artist
        // listing and search flags alone are insufficient to approve downloads.
        let raw = client
            .request(&format!("albums/{id}"), &market, &[])
            .await?;
        if resource_id(&raw["id"]) != id {
            return Err("The catalogue returned a different release".into());
        }
        cache_summaries(
            db,
            &client,
            std::slice::from_ref(&raw),
            &market,
            "album_lookup",
        )
        .await?;
        let raw = db
            .get_preference(&format!("subscriber-summary:{market}:{id}"))
            .await?
            .unwrap_or(raw);
        let mut normalized = crate::tidal::release_from_subscriber(&raw, "");
        let mut card = release_card(&raw).ok_or("The release details could not be read")?;
        if normalized.available == Some(false) || normalized.ai == Some(true) {
            card["tracks"] = json!([]);
            return save_page(db, &key, json!({"release":card})).await;
        }
        let http = crate::network::client(20)?;
        let items = crate::subscriber_metadata::load(
            db,
            &http,
            id,
            &market,
            Arc::new(AtomicBool::new(false)),
            args["refresh"] == true,
        )
        .await?;
        let current_summary = db
            .get_preference(&format!("subscriber-summary:{market}:{id}"))
            .await?;
        if current_summary.as_ref().is_some_and(|summary| {
            items["summary_fingerprint"].as_str()
                != Some(crate::tidal::summary_fingerprint(summary).as_str())
        }) {
            return Err(
                "The release changed while loading its tracks. Refresh its details and try again."
                    .into(),
            );
        }
        normalized.tracks = crate::subscriber_metadata::tracks(&items)?;
        normalized.tracks_loaded = true;
        normalized.track_count = normalized.tracks.len();
        let mut detail = serde_json::to_value(normalized).map_err(|e| e.to_string())?;
        if let Some(prior) = db
            .get_preference(&format!("tag-review:{market}:{id}"))
            .await?
        {
            preserve_extended_metadata(&mut detail, &prior);
        }
        for (key, value) in card.as_object().unwrap() {
            if !matches!(key.as_str(), "track_count" | "tracks") {
                detail[key] = value.clone();
            }
        }
        let raw_by_id: HashMap<_, _> = items["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| (resource_id(&t["id"]), t))
            .collect();
        for track in detail["tracks"].as_array_mut().into_iter().flatten() {
            let raw = raw_by_id
                .get(&resource_id(&track["id"]))
                .copied()
                .unwrap_or(&Value::Null);
            track["available"] = json!(availability(raw).unwrap_or(card["available"] == true));
            track["artist"] = json!(track["artists"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a["name"].as_str())
                .collect::<Vec<_>>()
                .join(", "));
            track["album_id"] = json!(id);
        }
        // This normalized cache is the existing queue/detail/linking contract.
        // It deliberately does not register an artist catalogue or local link.
        db.set_preference(&format!("tag-review:{market}:{id}"), &detail)
            .await?;
        save_page(db, &key, json!({"release":detail})).await
    }
    .await;
    match fetched {
        Ok(value) => release_safety(db, &market, value).await,
        Err(error) => match saved {
            Some(saved) => {
                release_safety(
                    db,
                    &market,
                    saved_page(&saved, true, Some(&format!("Saved release · {error}"))),
                )
                .await
            }
            None => Err(error),
        },
    }
}

pub(crate) async fn resolve(db: &TursoDb, args: &Value) -> Result<Value, String> {
    let (kind, id) = parse_url(args["url"].as_str().unwrap_or(""))?;
    if kind != "track" {
        return Ok(json!({"kind":if kind == "album" {"release"} else {"artist"},"id":id}));
    }
    let market = market(db).await?;
    let key = format!("browse-track:{market}:{id}");
    let saved = db.get_preference(&key).await?;
    let raw = match saved.filter(|v| current(v, PAGE_TTL)) {
        Some(value) => value,
        None => {
            let mut client = TidalClient::from_db(db).await?;
            let raw = client
                .request(&format!("tracks/{id}"), &market, &[])
                .await?;
            if resource_id(&raw["id"]) != id {
                return Err("The catalogue returned a different track".into());
            }
            cache_track_search(db, std::slice::from_ref(&raw), &market).await?;
            raw
        }
    };
    let album_id = resource_id(&raw["album"]["id"]);
    if !valid_id(&album_id) {
        return Err("This track has no available release".into());
    }
    Ok(json!({"kind":"release","id":album_id,"track_id":id}))
}

fn parse_url(raw: &str) -> Result<(String, String), String> {
    let url = url::Url::parse(raw.trim()).map_err(|_| "Paste a valid catalogue link".to_owned())?;
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(
            url.host_str(),
            Some("tidal.com" | "www.tidal.com" | "listen.tidal.com")
        )
    {
        return Err("Use an HTTPS link from tidal.com".into());
    }
    let segments: Vec<_> = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    let parts = if matches!(segments.first().copied(), Some("browse")) {
        &segments[1..]
    } else {
        segments.as_slice()
    };
    if parts.len() != 2 || !matches!(parts[0], "album" | "artist" | "track") || !valid_id(parts[1])
    {
        return Err("Open an artist, release or track link".into());
    }
    Ok((parts[0].into(), parts[1].into()))
}

pub(crate) async fn queue(db: &TursoDb, args: &Value) -> Result<Value, String> {
    let selection: HashMap<String, Option<Vec<String>>> =
        serde_json::from_value(args["selection"].clone())
            .map_err(|_| "Choose releases or audio tracks to queue".to_owned())?;
    if selection.is_empty() || selection.len() > MAX_QUEUE_RELEASES {
        return Err("Select between 1 and 50 releases".into());
    }
    if selection.iter().any(|(id, tracks)| {
        !valid_id(id)
            || tracks.as_ref().is_some_and(|tracks| {
                tracks.is_empty() || tracks.len() > 2000 || tracks.iter().any(|id| !valid_id(id))
            })
    }) {
        return Err("Select valid audio tracks or a complete release".into());
    }
    let market = market(db).await?;
    let mut prepared = Vec::new();
    for (id, ids) in &selection {
        let detail = release(db, &json!({"id":id})).await?;
        if detail["stale"] == true {
            return Err("Reconnect and refresh this release before adding it to the queue".into());
        }
        let payload = detail["release"].clone();
        if resource_id(&payload["id"]) != *id {
            return Err(
                "The saved release does not match this selection; refresh its details".into(),
            );
        }
        validate_queue_release(&payload, ids.as_deref())?;
        prepared.push((id.clone(), payload, ids.clone()));
    }
    // Safety labels can be published by another worker during release loading.
    let flags = crate::ai_policy::flags(db, &market).await?;
    for (id, payload, _) in &prepared {
        if flags.payload_blocked(id, payload) {
            return Err("AI-labelled music cannot be added to the download queue".into());
        }
    }
    let conn = db.connect()?;
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(|e| e.to_string())?;
    let result = async {
        let mut track_count = 0;
        for (id,mut payload,selected) in prepared {
            let mut rows = conn.query("SELECT payload,approved,decision FROM queue WHERE id=?",(id.as_str(),)).await.map_err(|e|e.to_string())?;
            let prior = rows.next().await.map_err(|e|e.to_string())?.and_then(|row| {
                (row.get::<i64>(1).ok() == Some(1) && row.get::<String>(2).ok().as_deref() == Some("queued"))
                    .then(|| row.get::<String>(0).ok().and_then(|raw|serde_json::from_str::<Value>(&raw).ok())).flatten()
            });
            drop(rows);
            let merged = merge_selection(&payload,selected.as_deref(),prior.as_ref())?;
            track_count += merged.as_array().map(Vec::len).unwrap_or_else(||payload["tracks"].as_array().map(Vec::len).unwrap_or(0));
            payload["selected_tracks"] = merged;
            payload["url"] = json!(format!("https://tidal.com/album/{id}"));
            if let Some(prior) = &prior {
                for field in ["redownload","replacement_audit","existing_destination"] {
                    if !prior[field].is_null() {payload[field] = prior[field].clone();}
                }
            }
            conn.execute("INSERT INTO queue(id,payload,approved,decision,updated) VALUES(?,?,1,'queued',?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,approved=1,decision='queued',updated=excluded.updated",(id,payload.to_string(),Utc::now().to_rfc3339())).await.map_err(|e|e.to_string())?;
        }
        conn.execute("COMMIT",()).await.map_err(|e|e.to_string())?;
        Ok::<_,String>(track_count)
    }.await;
    if result.is_err() {
        let _ = conn.execute("ROLLBACK", ()).await;
    }
    let tracks = result?;
    db.bump_revision();
    Ok(json!({"queued_releases":selection.len(),"queued_tracks":tracks}))
}

fn validate_queue_release(release: &Value, selection: Option<&[String]>) -> Result<(), String> {
    if release["ai"] == true
        || release["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|t| t["ai"] == true)
    {
        return Err("AI-labelled music cannot be added to the download queue".into());
    }
    if release["available"] != true {
        return Err("This release is unavailable in your account's country".into());
    }
    let tracks = release["tracks"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or("Load this release's audio tracks before queueing")?;
    let mut unique = HashSet::new();
    if tracks.iter().any(|track| {
        let id = resource_id(&track["id"]);
        !valid_id(&id)
            || !unique.insert(id)
            || track["disc_number"].as_u64().unwrap_or(0) == 0
            || track["track_number"].as_u64().unwrap_or(0) == 0
    }) {
        return Err(
            "This release has invalid audio track details; refresh it before queueing".into(),
        );
    }
    let wanted: Option<HashSet<_>> = selection.map(|ids| ids.iter().cloned().collect());
    if let Some(wanted) = &wanted {
        if wanted.is_empty()
            || !wanted
                .iter()
                .all(|id| tracks.iter().any(|t| resource_id(&t["id"]) == *id))
        {
            return Err(
                "Some selected tracks are no longer part of this release; refresh its details"
                    .into(),
            );
        }
    }
    if tracks
        .iter()
        .filter(|t| {
            wanted
                .as_ref()
                .is_none_or(|wanted| wanted.contains(&resource_id(&t["id"])))
        })
        .any(|t| t["available"] == false)
    {
        return Err(
            "One or more selected tracks are unavailable; select only available tracks".into(),
        );
    }
    Ok(())
}

fn merge_selection(
    release: &Value,
    selected: Option<&[String]>,
    prior: Option<&Value>,
) -> Result<Value, String> {
    validate_queue_release(release, selected)?;
    let Some(selected) = selected else {
        return Ok(Value::Null);
    };
    if prior.is_some_and(|p| p["selected_tracks"].is_null()) {
        validate_queue_release(release, None)?;
        return Ok(Value::Null);
    }
    let mut ids: HashSet<_> = selected.iter().cloned().collect();
    if let Some(prior) = prior {
        ids.extend(
            prior["selected_tracks"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| resource_id(&t["id"])),
        );
    }
    validate_queue_release(release, Some(&ids.iter().cloned().collect::<Vec<_>>()))?;
    Ok(json!(release["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| ids.contains(&resource_id(&t["id"])))
        .cloned()
        .collect::<Vec<_>>()))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    async fn database() -> (Temp, TursoDb) {
        let temp =
            Temp(std::env::temp_dir().join(format!("tibrary-browse-{}", uuid::Uuid::new_v4())));
        let db = TursoDb::open(temp.0.join("cache.sqlite3")).await.unwrap();
        db.set_preference("account-disconnected", &json!(true))
            .await
            .unwrap();
        (temp, db)
    }
    fn sample(id: &str) -> Value {
        json!({"id":id,"title":"A release","artist":"An artist","date":"2026-01-01","type":"ALBUM","track_count":2,"available":true,"ai":false,"tracks_loaded":true,
            "tracks":[{"id":format!("{id}1"),"title":"First","disc_number":1,"track_number":1,"duration":120,"available":true},
                {"id":format!("{id}2"),"title":"Second","disc_number":1,"track_number":2,"duration":140,"available":true}]})
    }
    async fn seed(db: &TursoDb, value: &Value) {
        let id = value["id"].as_str().unwrap();
        db.set_preference(
            &format!("browse:v1:GB:release:{id}"),
            &json!({"checked_at":Utc::now().timestamp(),"value":{"release":value}}),
        )
        .await
        .unwrap();
        db.set_preference(&format!("tag-review:GB:{id}"), value)
            .await
            .unwrap();
    }
    async fn queued(db: &TursoDb, id: &str) -> Option<Value> {
        let conn = db.connect().unwrap();
        let mut rows = conn
            .query("SELECT payload FROM queue WHERE id=?", (id,))
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .map(|row| serde_json::from_str(&row.get::<String>(0).unwrap()).unwrap())
    }
    #[test]
    fn direct_links_reject_external_hosts_and_ambiguous_paths() {
        assert_eq!(
            parse_url("https://tidal.com/browse/album/123?u").unwrap(),
            ("album".into(), "123".into())
        );
        assert_eq!(
            parse_url("https://listen.tidal.com/track/456").unwrap(),
            ("track".into(), "456".into())
        );
        for url in [
            "https://tidal.com.evil.invalid/album/12",
            "https://tidal.com@evil.invalid/track/12",
            "https://user@tidal.com/album/12",
            "http://tidal.com/album/12",
            "https://tidal.com/album/../tracks",
            "https://tidal.com/album/12/13",
            "https://tidal.com/album/123%2F",
        ] {
            assert!(parse_url(url).is_err(), "{url}");
        }
        assert!(pagination(&json!({"offset":100001})).is_err());
        assert_eq!(pagination(&json!({"limit":999})).unwrap(), (0, 50));
    }
    #[test]
    fn album_artist_and_audio_track_cards_are_distinct() {
        let album = json!({"id":10,"title":"Release","artist":{"id":2,"name":"Primary"},"artists":[{"id":3,"name":"Guest"}],"allowStreaming":true,"streamReady":true,"numberOfTracks":4});
        let card = release_card(&album).unwrap();
        assert_eq!(card["artist"], "Primary");
        assert_eq!(card["artist_id"], "2");
        assert_eq!(card["available"], true);
        let track=track_card(&json!({"id":11,"title":"Song","artists":[{"id":3,"name":"Guest"}],"album":album,"volumeNumber":1,"trackNumber":2})).unwrap();
        assert_eq!(track["artist"], "Guest");
        assert_eq!(track["album_id"], "10");
        assert_eq!(track["track_number"], 2);
        assert!(track_card(&json!({"id":12,"title":"No album"})).is_none());
    }
    #[test]
    fn home_accepts_supported_music_lists_only_and_bounds_payload() {
        let raw = json!({"rows":[{"modules":[
            {"type":"ALBUM_LIST","title":"New albums","pagedList":{"items":[{"id":10,"title":"Release","artists":[{"id":2,"name":"Primary"}]}]}},
            {"type":"TRACK_LIST","title":"New tracks","pagedList":{"items":[{"id":11,"title":"Song","album":{"id":10,"title":"Release"}}]}},
            {"type":"PLAYLIST_LIST","title":"Not albums","pagedList":{"items":[{"id":12,"title":"List"}]}}
        ]}]});
        let (sections, albums, tracks) = home_sections(&raw);
        assert_eq!(sections.len(), 2);
        assert_eq!(albums.len(), 1);
        assert_eq!(tracks.len(), 1);
        assert_eq!(sections[1]["tracks"][0]["album_id"], "10");
    }
    #[test]
    fn opening_release_keeps_saved_extended_metadata_without_copying_old_identity() {
        let mut fresh = sample("10");
        fresh["summary_fingerprint"] = json!("current");
        let mut prior = sample("10");
        prior["genres"] = json!(["House"]);
        prior["label"] = json!("Independent label");
        prior["artist"] = json!("Wrong old artist");
        prior["summary_fingerprint"] = json!("old");
        prior["recommendation_snapshot"] = json!({"complete":true});
        prior["tracks"][0]["bpm"] = json!(120.0);
        preserve_extended_metadata(&mut fresh, &prior);
        assert_eq!(fresh["artist"], "An artist");
        assert_eq!(fresh["label"], "Independent label");
        assert_eq!(fresh["genres"], json!(["House"]));
        assert_eq!(fresh["tracks"][0]["bpm"], 120.0);
        assert!(
            fresh["recommendation_snapshot"].is_null(),
            "Old completion evidence cannot validate changed releases"
        );
    }
    #[tokio::test]
    async fn changed_summary_or_wrong_cached_release_cannot_queue_old_selection() {
        let (_temp, db) = database().await;
        let mut value = sample("10");
        value["summary_fingerprint"] = json!("old");
        seed(&db, &value).await;
        db.set_preference(
            "subscriber-summary:GB:10",
            &json!({"id":10,"title":"Changed","numberOfTracks":3}),
        )
        .await
        .unwrap();
        assert_eq!(
            release(&db, &json!({"id":"10"})).await.unwrap()["stale"],
            true
        );
        assert!(queue(&db, &json!({"selection":{"10":null}})).await.is_err());
        db.set_preference(
            "browse:v1:GB:release:20",
            &json!({"checked_at":Utc::now().timestamp(),"value":{"release":sample("30")}}),
        )
        .await
        .unwrap();
        assert!(queue(&db, &json!({"selection":{"20":null}}))
            .await
            .unwrap_err()
            .contains("does not match"));
    }
    #[tokio::test]
    async fn cached_browser_remains_offline_and_never_adds_artist_catalogues() {
        let (_temp, db) = database().await;
        seed(&db, &sample("10")).await;
        let result = release(&db, &json!({"id":"10"})).await.unwrap();
        assert_eq!(result["source"], "cache");
        assert_eq!(result["stale"], false);
        assert_eq!(result["release"]["tracks"].as_array().unwrap().len(), 2);
        let result = home(&db, &json!({})).await.unwrap();
        assert_eq!(result["connected"], false);
        assert!(result["sections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "recent"));
        let conn = db.connect().unwrap();
        let mut count = conn
            .query("SELECT COUNT(*) FROM catalogue", ())
            .await
            .unwrap();
        assert_eq!(
            count.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }
    #[tokio::test]
    async fn browser_queue_merges_partial_and_whole_release_approvals() {
        let (_temp, db) = database().await;
        seed(&db, &sample("10")).await;
        queue(&db, &json!({"selection":{"10":["101"]}}))
            .await
            .unwrap();
        let detail = release(&db, &json!({"id":"10"})).await.unwrap();
        assert_eq!(detail["release"]["queued_tracks"], 1);
        assert_eq!(detail["release"]["tracks"][0]["queued"], true);
        assert_eq!(detail["release"]["tracks"][1]["queued"], false);
        let destination = json!({"root":"/existing-library","album_relative":"Artist/Release"});
        let audit = json!({"root":"/existing-library","source_paths":{}});
        let mut original = queued(&db, "10").await.unwrap();
        original["existing_destination"] = destination.clone();
        original["replacement_audit"] = audit.clone();
        original["redownload"] = json!(true);
        db.connect()
            .unwrap()
            .execute(
                "UPDATE queue SET payload=? WHERE id='10'",
                (original.to_string(),),
            )
            .await
            .unwrap();
        queue(&db, &json!({"selection":{"10":["102"]}}))
            .await
            .unwrap();
        let value = queued(&db, "10").await.unwrap();
        assert_eq!(value["selected_tracks"].as_array().unwrap().len(), 2);
        assert_eq!(value["existing_destination"], destination);
        assert_eq!(value["replacement_audit"], audit);
        assert_eq!(value["redownload"], true);
        queue(&db, &json!({"selection":{"10":null}})).await.unwrap();
        queue(&db, &json!({"selection":{"10":["101"]}}))
            .await
            .unwrap();
        assert!(
            queued(&db, "10").await.unwrap()["selected_tracks"].is_null(),
            "Adding a track cannot unapprove an already approved album"
        );
        let conn = db.connect().unwrap();
        conn.execute("UPDATE queue SET approved=0 WHERE id='10'", ())
            .await
            .unwrap();
        let detail = release(&db, &json!({"id":"10"})).await.unwrap();
        assert_eq!(detail["release"]["queued_tracks"], 0);
        assert!(detail["release"]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|track| track["queued"] == false));
        queue(&db, &json!({"selection":{"10":["102"]}}))
            .await
            .unwrap();
        assert_eq!(
            queued(&db, "10").await.unwrap()["selected_tracks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        conn.execute("UPDATE queue SET decision='downloaded' WHERE id='10'", ())
            .await
            .unwrap();
        assert_eq!(
            release(&db, &json!({"id":"10"})).await.unwrap()["release"]["queued_tracks"],
            0
        );
    }
    #[tokio::test]
    async fn stale_selected_tracks_and_ai_or_unavailable_releases_never_partially_queue() {
        let (_temp, db) = database().await;
        seed(&db, &sample("10")).await;
        seed(&db, &sample("20")).await;
        assert!(
            queue(&db, &json!({"selection":{"10":["101"],"20":["999"]}}))
                .await
                .unwrap_err()
                .contains("no longer part")
        );
        assert!(queued(&db, "10").await.is_none());
        let mut blocked = sample("20");
        blocked["tracks"][0]["ai"] = json!(true);
        seed(&db, &blocked).await;
        assert!(queue(&db, &json!({"selection":{"10":null,"20":null}}))
            .await
            .unwrap_err()
            .contains("AI-labelled"));
        assert!(queued(&db, "10").await.is_none());
        db.set_preference(
            "release-live:GB:10",
            &json!({"available":false,"checked_at":Utc::now().timestamp(),"source":"album_lookup"}),
        )
        .await
        .unwrap();
        assert!(queue(&db, &json!({"selection":{"10":null}}))
            .await
            .unwrap_err()
            .contains("unavailable"));
    }
    #[tokio::test]
    async fn whole_album_rejects_unavailable_audio_but_explicit_available_track_can_queue() {
        let (_temp, db) = database().await;
        let mut value = sample("10");
        value["tracks"][1]["available"] = json!(false);
        seed(&db, &value).await;
        assert!(queue(&db, &json!({"selection":{"10":null}})).await.is_err());
        queue(&db, &json!({"selection":{"10":["101"]}}))
            .await
            .unwrap();
        assert_eq!(
            queued(&db, "10").await.unwrap()["selected_tracks"][0]["id"],
            "101"
        );
    }
    #[tokio::test]
    async fn cached_cards_recheck_shared_ai_withdrawals_and_queue_badges() {
        let (_temp, db) = database().await;
        seed(&db, &sample("10")).await;
        queue(&db, &json!({"selection":{"10":["101"]}}))
            .await
            .unwrap();
        db.set_preference(
            "release-live:GB:20",
            &json!({"available":false,"checked_at":Utc::now().timestamp(),"source":"album_lookup"}),
        )
        .await
        .unwrap();
        db.set_preference(
            "subscriber-summary:GB:30",
            &json!({"id":30,"ai":true,"checked_at":Utc::now().timestamp()}),
        )
        .await
        .unwrap();
        let mut cards = vec![sample("10"), sample("20"), sample("30")];
        filter_cards(&db, "GB", &mut cards, "albums").await.unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0]["queued_tracks"], 1);
        let mut tracks = vec![
            json!({"id":"101","album_id":"10"}),
            json!({"id":"102","album_id":"10"}),
        ];
        filter_cards(&db, "GB", &mut tracks, "tracks")
            .await
            .unwrap();
        assert_eq!(tracks[0]["queued"], true);
        assert_eq!(tracks[1]["queued"], false);
    }
    #[tokio::test]
    async fn home_never_reads_another_accounts_favourites_or_personal_feed() {
        let (_temp, db) = database().await;
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO favourite_artists(cache_id,payload,fetched) VALUES('other',?,'2026-10-15')",(json!([{"id":"77","name":"Other account"}]).to_string(),)).await.unwrap();
        db.set_preference(
            "subscriber-account-market",
            &json!({"country_code":"GB","user_id":"current"}),
        )
        .await
        .unwrap();
        use sha2::{Digest, Sha256};
        let other = format!("{:x}", Sha256::digest(b"other"));
        db.set_preference(&format!("browse:v1:GB:home:{other}"),&json!({"checked_at":Utc::now().timestamp(),"value":{"sections":[{"id":"other","title":"Other account","releases":[sample("10")]}]}})).await.unwrap();
        let result = home(&db, &json!({})).await.unwrap();
        assert!(result["sections"].as_array().unwrap().is_empty());
    }
    #[tokio::test]
    async fn cache_age_is_honest_and_market_changes_do_not_reuse_other_market() {
        let (_temp, db) = database().await;
        let value = sample("10");
        seed(&db, &value).await;
        db.set_preference(
            "browse:v1:GB:release:10",
            &json!({"checked_at":Utc::now().timestamp()-PAGE_TTL-10,"value":{"release":value}}),
        )
        .await
        .unwrap();
        let result = release(&db, &json!({"id":"10"})).await.unwrap();
        assert_eq!(result["stale"], true);
        assert_eq!(result["release"]["downloadable"], false);
        assert!(queue(&db, &json!({"selection":{"10":null}}))
            .await
            .unwrap_err()
            .contains("Reconnect"));
        db.set_preference("subscriber-account-market", &json!({"country_code":"US"}))
            .await
            .unwrap();
        assert!(
            release(&db, &json!({"id":"10"})).await.is_err(),
            "GB cache must not become US evidence"
        );
    }
    #[tokio::test]
    async fn partial_track_search_cover_does_not_replace_authoritative_album_metadata() {
        let (_temp, db) = database().await;
        db.set_preference("subscriber-summary:GB:10",&json!({"id":10,"title":"Release","artist":{"id":1,"name":"Primary"},"numberOfTracks":12})).await.unwrap();
        cache_track_search(&db,&[json!({"id":101,"title":"Track","album":{"id":10,"title":"Release","cover":"cover"}})],"GB").await.unwrap();
        assert_eq!(
            db.get_preference("subscriber-summary:GB:10")
                .await
                .unwrap()
                .unwrap()["numberOfTracks"],
            12
        );
        assert_eq!(
            db.get_preference("browse-cover:GB:10")
                .await
                .unwrap()
                .unwrap()["cover"],
            "cover"
        );
    }
    #[tokio::test]
    #[ignore = "Bounded subscriber browser and one-track download using read-only saved credentials and disposable output"]
    async fn live_browser_search_to_download_roundtrip() {
        let temp = Temp(
            std::env::temp_dir().join(format!("tibrary-browse-live-{}", uuid::Uuid::new_v4())),
        );
        let db = TursoDb::open(temp.0.join("cache.sqlite3")).await.unwrap();
        crate::stream_download::use_read_only_saved_account(&db).await;
        let country =
            crate::account::ensure_account_market(&db, &crate::network::client(20).unwrap(), None)
                .await
                .unwrap();
        let home = home(&db, &json!({})).await.unwrap();
        assert!(!home["sections"].as_array().unwrap().is_empty());
        let artists = search(&db, &json!({"query":"Cassie","kind":"artists","limit":5}))
            .await
            .unwrap();
        assert!(!artists["items"].as_array().unwrap().is_empty());
        let artist_id = artists["items"][0]["id"].clone();
        let discography = artist(&db, &json!({"id":artist_id,"limit":5}))
            .await
            .unwrap();
        assert_eq!(discography["limit"], 5);
        let albums = search(&db, &json!({"query":"Cassie","kind":"albums","limit":5}))
            .await
            .unwrap();
        assert!(!albums["items"].as_array().unwrap().is_empty());
        let tracks = search(
            &db,
            &json!({"query":"Cassie Me & U","kind":"tracks","limit":5}),
        )
        .await
        .unwrap();
        assert!(!tracks["items"].as_array().unwrap().is_empty());
        let detail = release(&db, &json!({"id":"140303440"})).await.unwrap();
        let album = &detail["release"];
        assert_eq!(
            album["track_count"], 12,
            "The video is excluded from this 13-item release"
        );
        assert!(album["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["credits"].is_array()));
        let shortest = album["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .min_by(|a, b| {
                a["duration"]
                    .as_f64()
                    .unwrap_or(f64::MAX)
                    .total_cmp(&b["duration"].as_f64().unwrap_or(f64::MAX))
            })
            .unwrap();
        let track_id = shortest["id"].as_str().unwrap();
        assert_eq!(
            resolve(
                &db,
                &json!({"url":format!("https://tidal.com/track/{track_id}")})
            )
            .await
            .unwrap()["id"],
            "140303440"
        );
        let args = json!({"selection":{"140303440":[track_id]}});
        queue(&db, &args).await.unwrap();
        let output = temp.0.join("audio");
        db.set_preference(
            "downloads",
            &json!({"output":output,"quality":"LOSSLESS","cover_size":0,"skip_existing":true}),
        )
        .await
        .unwrap();
        db.set_preference("provider", &json!({"download_delay":false}))
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(180),
            crate::downloads::DownloadManager::run_downloads(
                &db,
                None,
                Arc::new(AtomicBool::new(false)),
                "browser-live",
                |_| {},
                |_| {},
            ),
        )
        .await
        .expect("Download exceeded the three-minute test bound")
        .unwrap();
        assert_eq!(result, 1);
        let connection = db.connect().unwrap();
        let mut completed = connection
            .query(
                "SELECT decision,approved FROM queue WHERE id='140303440'",
                (),
            )
            .await
            .unwrap();
        let row = completed
            .next()
            .await
            .unwrap()
            .expect("Downloaded release remains in queue history");
        assert_eq!(
            row.get::<String>(0).unwrap(),
            "downloaded",
            "Metadata enrichment during transfer must not leave a completed selection pending"
        );
        assert_eq!(row.get::<i64>(1).unwrap(), 0);
        drop(completed);
        fn files(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files(&path, out)
                } else if path.extension().is_some_and(|e| e == "flac") {
                    out.push(path)
                }
            }
        }
        let mut audio = Vec::new();
        files(&output, &mut audio);
        assert_eq!(audio.len(), 1);
        let metadata = crate::scanner::read_audio_metadata(&audio[0]).unwrap();
        assert_eq!(metadata.tidal_album_id.as_deref(), Some("140303440"));
        assert_eq!(metadata.tidal_track_id.as_deref(), Some(track_id));
        let mut decoder = claxon::FlacReader::open(&audio[0]).unwrap();
        assert!(!decoder
            .samples()
            .take(44100)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .is_empty());
        db.set_preference("account-disconnected", &json!(true))
            .await
            .unwrap();
        assert_eq!(
            release(&db, &json!({"id":"140303440"})).await.unwrap()["source"],
            "cache"
        );
        assert_eq!(
            search(&db, &json!({"query":"Cassie","kind":"artists","limit":5}))
                .await
                .unwrap()["source"],
            "cache"
        );
        let conn = db.connect().unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM catalogue", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
        println!("Browser home, 3 search types, bounded artist page, 12 audio tracks with credits, exact queue selection and one decoded FLAC download passed in {country}; cached browsing works disconnected; catalogue scope unchanged");
    }
}
