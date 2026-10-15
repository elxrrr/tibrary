//! Exact, provider-labelled AI evidence shared by suggestions and local review.
//! The small derived index is backfilled from saved metadata once, then SQLite
//! triggers record changed keys. Table reads never reparse the whole catalogue.
use crate::db::TursoDb;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::Mutex as AsyncMutex;
use turso::Connection;

const POSITIVE_CANDIDATE: &str = "instr(payload,'\"ai\"')>0 AND instr(replace(replace(replace(replace(payload,' ',''),char(9),''),char(10),''),char(13),''),'\"ai\":true')>0";

#[derive(Debug, Default, Clone)]
pub(crate) struct AiFlags {
    releases: HashSet<String>,
    releases_with_ai_tracks: HashSet<String>,
    tracks: HashSet<String>,
    release_states: HashMap<String, bool>,
    track_states: HashMap<String, bool>,
}

impl AiFlags {
    pub(crate) fn is_empty(&self) -> bool {
        self.releases.is_empty() && self.tracks.is_empty()
    }

    /// Stable within and across processes; unrelated metadata fields do not
    /// invalidate the local removal-review table.
    pub(crate) fn review_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut releases: Vec<_> = self.releases.iter().collect();
        let mut tracks: Vec<_> = self.tracks.iter().collect();
        releases.sort_unstable();
        tracks.sort_unstable();
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        releases.hash(&mut hash);
        tracks.hash(&mut hash);
        hash.finish()
    }

    pub(crate) fn payload_blocked(&self, release_id: &str, payload: &Value) -> bool {
        if self.release_blocked(release_id) {
            return true;
        }
        if self
            .release_states
            .get(release_id)
            .copied()
            .or_else(|| payload_label(payload))
            == Some(true)
        {
            return true;
        }
        for field in ["tracks", "selected_tracks"] {
            for track in payload[field].as_array().into_iter().flatten() {
                let track_id = id(&track["id"]).or_else(|| id(track));
                let explicit = track_id
                    .as_ref()
                    .and_then(|id| self.track_states.get(id))
                    .copied();
                if explicit.or_else(|| payload_label(track)) == Some(true) {
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn release_blocked(&self, release_id: &str) -> bool {
        self.release_labelled(release_id) || self.release_has_ai_tracks(release_id)
    }

    pub(crate) fn release_labelled(&self, release_id: &str) -> bool {
        self.releases.contains(release_id)
    }

    pub(crate) fn release_has_ai_tracks(&self, release_id: &str) -> bool {
        self.releases_with_ai_tracks.contains(release_id)
    }

    pub(crate) fn track_labelled(&self, track_id: &str) -> bool {
        self.tracks.contains(track_id)
    }

    /// An AI track blocks suggesting its whole release, but does not label
    /// unrelated downloaded tracks on that mixed release as AI-generated.
    pub(crate) fn evidence(&self, release_id: &str, track_id: &str) -> Option<String> {
        if self.track_labelled(track_id) {
            Some(format!(
                "Online source labels track {track_id} as AI-generated. Review for removal."
            ))
        } else if self.release_labelled(release_id) {
            Some(format!(
                "Online source labels release {release_id} as AI-generated. Review for removal."
            ))
        } else {
            None
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Source {
    release: Option<bool>,
    tracks: HashMap<String, Option<bool>>,
}

#[derive(Default)]
struct State {
    initialized: bool,
    generation: Option<i64>,
    markets: HashMap<String, Arc<AiFlags>>,
}

struct Instance {
    revision: Weak<AtomicU64>,
    state: Arc<AsyncMutex<State>>,
}

fn state_for(db: &TursoDb) -> Arc<AsyncMutex<State>> {
    static INSTANCES: OnceLock<Mutex<Vec<Instance>>> = OnceLock::new();
    let mut instances = INSTANCES.get_or_init(Default::default).lock().unwrap();
    instances.retain(|entry| entry.revision.strong_count() > 0);
    if let Some(entry) = instances
        .iter()
        .find(|entry| entry.revision.ptr_eq(&Arc::downgrade(&db.revision)))
    {
        return entry.state.clone();
    }
    let state = Arc::new(AsyncMutex::new(State::default()));
    instances.push(Instance {
        revision: Arc::downgrade(&db.revision),
        state: state.clone(),
    });
    state
}

fn source_key(key: &str) -> Option<(&str, &str, &str)> {
    let mut parts = key.split(':');
    let kind = parts.next()?;
    if !matches!(
        kind,
        "subscriber-summary" | "subscriber-items" | "tag-review"
    ) {
        return None;
    }
    let market = parts.next()?;
    let release = parts.next()?;
    if parts.next().is_some()
        || market.len() != 2
        || !market.bytes().all(|c| c.is_ascii_uppercase())
        || !valid_id(release)
    {
        return None;
    }
    Some((kind, market, release))
}

pub(crate) fn payload_label(value: &Value) -> Option<bool> {
    value["ai"]
        .as_bool()
        .or_else(|| value["catalogue_attributes"]["ai"].as_bool())
        .or_else(|| value["catalogue_metadata"]["data"]["attributes"]["ai"].as_bool())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|c| c.is_ascii_digit())
}

fn id(value: &Value) -> Option<String> {
    let text = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))?;
    valid_id(&text).then_some(text)
}

/// Use after metadata writes if the caller does not already invalidate views.
/// The SQL triggers, rather than this hook, maintain the derived index so bulk
/// transactional writes and later application sessions are covered as well.
pub(crate) async fn cache_written(db: &TursoDb, key: &str, _value: &Value) -> Result<(), String> {
    if source_key(key).is_some() {
        db.bump_revision();
    }
    Ok(())
}

pub(crate) async fn flags(db: &TursoDb, market: &str) -> Result<Arc<AiFlags>, String> {
    let market = market.trim().to_ascii_uppercase();
    if market.len() != 2 || !market.bytes().all(|c| c.is_ascii_uppercase()) {
        return Err("Invalid catalogue market for AI label checks".into());
    }
    let state = state_for(db);
    let mut state = state.lock().await;
    let conn = db.connect()?;
    if !state.initialized {
        initialize(&conn).await?;
        state.initialized = true;
    }
    let changed = refresh_dirty(&conn).await?;
    if changed {
        db.bump_revision();
    }
    let mut rows = conn
        .query(
            "SELECT value FROM ai_label_state WHERE key='generation'",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
    let generation = rows
        .next()
        .await
        .map_err(|e| e.to_string())?
        .map(|row| row.get::<i64>(0))
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(0);
    drop(rows);
    if state.generation != Some(generation) {
        if !changed && state.generation.is_some() {
            db.bump_revision();
        }
        state.markets.clear();
        state.generation = Some(generation);
    }
    if let Some(snapshot) = state.markets.get(&market) {
        return Ok(snapshot.clone());
    }
    let snapshot = Arc::new(load_flags(&conn, &market).await?);
    // A single account normally uses one market; keep this bounded for imports.
    if state.markets.len() >= 8 {
        state.markets.clear();
    }
    state.markets.insert(market, snapshot.clone());
    Ok(snapshot)
}

/// Local file/link writes have their own generation so ongoing online metadata
/// refreshes cannot repeatedly invalidate a cached 19,000-file removal review.
pub(crate) async fn local_generation(db: &TursoDb) -> Result<u64, String> {
    let state = state_for(db);
    let mut state = state.lock().await;
    let conn = db.connect()?;
    if !state.initialized {
        initialize(&conn).await?;
        state.initialized = true;
    }
    let mut rows = conn
        .query(
            "SELECT value FROM ai_label_state WHERE key='local_generation'",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
    let generation = rows
        .next()
        .await
        .map_err(|e| e.to_string())?
        .map(|row| row.get::<i64>(0))
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(0);
    Ok(generation.max(0) as u64)
}

async fn initialize(conn: &Connection) -> Result<(), String> {
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(|e| e.to_string())?;
    let result = async {
        conn.execute("CREATE TABLE IF NOT EXISTS ai_label_sources (key TEXT PRIMARY KEY, market TEXT NOT NULL, release_id TEXT NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL)", ()).await.map_err(|e| e.to_string())?;
        conn.execute("CREATE INDEX IF NOT EXISTS ai_label_sources_market ON ai_label_sources(market)", ()).await.map_err(|e| e.to_string())?;
        conn.execute("CREATE TABLE IF NOT EXISTS ai_label_dirty (key TEXT PRIMARY KEY)", ()).await.map_err(|e| e.to_string())?;
        conn.execute("CREATE TABLE IF NOT EXISTS ai_label_state (key TEXT PRIMARY KEY, value INTEGER NOT NULL)", ()).await.map_err(|e| e.to_string())?;
        conn.execute("INSERT OR IGNORE INTO ai_label_state(key,value) VALUES('generation',0)", ()).await.map_err(|e|e.to_string())?;
        conn.execute("INSERT OR IGNORE INTO ai_label_state(key,value) VALUES('local_generation',0)",()).await.map_err(|e|e.to_string())?;
        for table in ["local_files","track_links"] {
            for event in ["INSERT","UPDATE","DELETE"] {
                let sql=format!("CREATE TRIGGER IF NOT EXISTS ai_label_local_{}_{} AFTER {event} ON {table} BEGIN UPDATE ai_label_state SET value=value+1 WHERE key='local_generation'; END",table,event.to_ascii_lowercase());
                conn.execute(sql,()).await.map_err(|e|e.to_string())?;
            }
        }
        for (event, row) in [("INSERT", "NEW"), ("UPDATE", "NEW"), ("DELETE", "OLD")] {
            let trigger = format!("CREATE TRIGGER IF NOT EXISTS ai_label_cache_{} AFTER {event} ON app_preferences WHEN {row}.key GLOB 'subscriber-summary:*' OR {row}.key GLOB 'subscriber-items:*' OR {row}.key GLOB 'tag-review:*' BEGIN INSERT OR IGNORE INTO ai_label_dirty(key) VALUES({row}.key); END", event.to_ascii_lowercase());
            conn.execute(trigger, ()).await.map_err(|e| e.to_string())?;
        }
        for (table,key) in [("catalogue","'ai-catalogue:' || {row}.market || ':' || {row}.artist_id"),("queue","'ai-queue:' || {row}.id")] {
            for (event,row) in [("INSERT","NEW"),("UPDATE","NEW"),("DELETE","OLD")] {
                let key=key.replace("{row}",row);
                conn.execute(format!("CREATE TRIGGER IF NOT EXISTS ai_label_snapshot_{}_{} AFTER {event} ON {table} BEGIN INSERT OR IGNORE INTO ai_label_dirty(key) VALUES({key}); END",table,event.to_ascii_lowercase()),()).await.map_err(|e|e.to_string())?;
            }
        }
        let mut rows = conn.query("SELECT value FROM ai_label_state WHERE key='schema'", ()).await.map_err(|e| e.to_string())?;
        let version = rows.next().await.map_err(|e| e.to_string())?.map(|row|row.get::<i64>(0)).transpose().map_err(|e|e.to_string())?.unwrap_or(0);
        drop(rows);
        if version < 1 {
            // Only positive evidence and its authoritative sibling sources are
            // needed on first use. Do not build a second index of every cached
            // human/unknown recording in a multi-gigabyte catalogue.
            let mut releases=HashSet::new();
            for prefix in ["subscriber-summary:","subscriber-items:","tag-review:"] {
                let end=format!("{};",prefix.trim_end_matches(':'));
                // Cached provider JSON is serialized with literal ASCII keys.
                // This inexpensive superset finds any spelling of JSON whitespace
                // around ai:true; project_sources then validates the actual path,
                // exact boolean type and matching IDs. A positive-looking string
                // can add work here but can never create a false AI label.
                let mut rows=conn.query("SELECT key FROM app_preferences WHERE key>=? AND key<? AND instr(payload,'\"ai\"')>0 AND instr(replace(replace(replace(replace(payload,' ',''),char(9),''),char(10),''),char(13),''),'\"ai\":true')>0",(prefix,end)).await.map_err(|e|e.to_string())?;
                while let Some(row)=rows.next().await.map_err(|e|e.to_string())? {
                    let key:String=row.get(0).map_err(|e|e.to_string())?;
                    if let Some((_,market,release))=source_key(&key) {releases.insert((market.to_owned(),release.to_owned()));}
                }
                drop(rows);
            }
            let keys:Vec<_>=releases.into_iter().flat_map(|(market,release)|["subscriber-summary","subscriber-items","tag-review"].map(|kind|format!("{kind}:{market}:{release}"))).collect();
            for keys in keys.chunks(128) {
                conn.execute(format!("INSERT OR IGNORE INTO ai_label_dirty(key) VALUES {}",vec!["(?)";keys.len()].join(",")),keys.to_vec()).await.map_err(|e|e.to_string())?;
            }
            conn.execute("INSERT OR REPLACE INTO ai_label_state(key,value) VALUES('schema',1)", ()).await.map_err(|e| e.to_string())?;
        }
        if version < 2 {
            // Older normalized snapshots may be the only retained source of a
            // label. Seed only positive-looking parents, then verify exact IDs.
            let mut keys=Vec::new();
            let mut rows=conn.query(format!("SELECT artist_id,market FROM catalogue WHERE {POSITIVE_CANDIDATE}"),()).await.map_err(|e|e.to_string())?;
            while let Some(row)=rows.next().await.map_err(|e|e.to_string())? {
                let artist:String=row.get(0).map_err(|e|e.to_string())?;
                let market:String=row.get(1).map_err(|e|e.to_string())?;
                keys.push(format!("ai-catalogue:{market}:{artist}"));
            }
            drop(rows);
            let mut rows=conn.query(format!("SELECT id FROM queue WHERE {POSITIVE_CANDIDATE}"),()).await.map_err(|e|e.to_string())?;
            while let Some(row)=rows.next().await.map_err(|e|e.to_string())? {keys.push(format!("ai-queue:{}",row.get::<String>(0).map_err(|e|e.to_string())?));}
            drop(rows);
            for keys in keys.chunks(128) {
                conn.execute(format!("INSERT OR IGNORE INTO ai_label_dirty(key) VALUES {}",vec!["(?)";keys.len()].join(",")),keys.to_vec()).await.map_err(|e|e.to_string())?;
            }
            conn.execute("INSERT OR REPLACE INTO ai_label_state(key,value) VALUES('schema',2)",()).await.map_err(|e|e.to_string())?;
        }
        Ok::<(), String>(())
    }.await;
    match result {
        Ok(()) => {
            conn.execute("COMMIT", ())
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(error) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(error)
        }
    }
}

struct SnapshotSource {
    key: String,
    market: String,
    release: String,
    source: Source,
}

fn snapshot_source(value: &Value) -> Source {
    let mut tracks = HashMap::new();
    for field in ["tracks", "selected_tracks", "children"] {
        for track in value[field].as_array().into_iter().flatten() {
            if let Some(id) = id(&track["id"]).or_else(|| id(track)) {
                let label = payload_label(track);
                let current = tracks.entry(id).or_insert(None);
                if current.is_none() || label == Some(true) {
                    *current = label;
                }
            }
        }
    }
    Source {
        release: payload_label(value),
        tracks,
    }
}

fn retain_prior_evidence(source: &mut Source, previous: Option<&Source>) {
    if let Some(previous) = previous {
        source.release = source.release.or(previous.release);
        for (track, label) in &mut source.tracks {
            *label = label.or(previous.tracks.get(track).copied().flatten());
        }
    }
}

async fn project_snapshot(conn: &Connection, key: &str) -> Result<Vec<SnapshotSource>, String> {
    let (kind, scope) = if let Some(scope) = key.strip_prefix("ai-catalogue:") {
        ("catalogue", scope)
    } else if let Some(scope) = key.strip_prefix("ai-queue:") {
        ("queue", scope)
    } else {
        return Ok(Vec::new());
    };
    let prefix = format!("{key}:");
    let end = format!("{key};");
    let mut old = conn
        .query(
            "SELECT key,payload FROM ai_label_sources WHERE key>=? AND key<?",
            (prefix.as_str(), end.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut previous = HashMap::new();
    while let Some(row) = old.next().await.map_err(|e| e.to_string())? {
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        let raw: String = row.get(1).map_err(|e| e.to_string())?;
        if let Ok(source) = serde_json::from_str::<Source>(&raw) {
            previous.insert(key, source);
        }
    }
    drop(old);
    let projection = if previous.is_empty() {
        format!("CASE WHEN {POSITIVE_CANDIDATE} THEN payload END")
    } else {
        "payload".into()
    };
    let (market, mut rows) = if kind == "catalogue" {
        let Some((market, artist)) = scope.split_once(':') else {
            return Ok(Vec::new());
        };
        (
            market.to_owned(),
            conn.query(
                format!("SELECT {projection} FROM catalogue WHERE artist_id=? AND market=?"),
                (artist, market),
            )
            .await
            .map_err(|e| e.to_string())?,
        )
    } else {
        let mut account=conn.query("SELECT COALESCE((SELECT json_extract(payload,'$.country_code') FROM app_preferences WHERE key='subscriber-account-market'),(SELECT json_extract(payload,'$.market') FROM app_preferences WHERE key='desktop'),(SELECT json_extract(payload,'$.market') FROM app_preferences WHERE key='ui'),'GB')",()).await.map_err(|e|e.to_string())?;
        let market = account
            .next()
            .await
            .map_err(|e| e.to_string())?
            .and_then(|r| r.get::<String>(0).ok())
            .unwrap_or_else(|| "GB".into());
        drop(account);
        (
            market,
            conn.query(
                format!("SELECT {projection} FROM queue WHERE id=?"),
                (scope,),
            )
            .await
            .map_err(|e| e.to_string())?,
        )
    };
    let raw = rows
        .next()
        .await
        .map_err(|e| e.to_string())?
        .map(|row| row.get::<Option<String>>(0))
        .transpose()
        .map_err(|e| e.to_string())?
        .flatten();
    drop(rows);
    // Delete the previous parent projection atomically with its replacement.
    conn.execute(
        "DELETE FROM ai_label_sources WHERE key>=? AND key<?",
        (prefix, end),
    )
    .await
    .map_err(|e| e.to_string())?;
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Ok(Vec::new());
    };
    let market = crate::account::normalize_market(value["market"].as_str().unwrap_or(&market))
        .unwrap_or_else(|| "GB".into());
    let releases: Vec<&Value> = if kind == "catalogue" {
        value["releases"].as_array().into_iter().flatten().collect()
    } else {
        vec![&value]
    };
    let mut result = Vec::new();
    for release in releases {
        let Some(release_id) = id(&release["id"])
            .or_else(|| (kind == "queue" && valid_id(scope)).then(|| scope.to_owned()))
        else {
            continue;
        };
        if kind == "queue" && release_id != scope {
            continue;
        }
        let source_key = format!("{key}:{release_id}");
        let mut source = snapshot_source(release);
        retain_prior_evidence(&mut source, previous.get(&source_key));
        if source.release == Some(true) || source.tracks.values().any(|v| *v == Some(true)) {
            result.push(SnapshotSource {
                key: source_key,
                market: market.clone(),
                release: release_id,
                source,
            });
        }
    }
    Ok(result)
}

async fn save_source_rows(conn: &Connection, values: Vec<String>) -> Result<(), String> {
    for chunk in values.chunks(500) {
        conn.execute(format!("INSERT OR REPLACE INTO ai_label_sources(key,market,release_id,kind,payload) VALUES {}",vec!["(?,?,?,?,?)";chunk.len()/5].join(",")),chunk.to_vec()).await.map_err(|e|e.to_string())?;
    }
    Ok(())
}

async fn refresh_dirty(conn: &Connection) -> Result<bool, String> {
    let mut changed = false;
    loop {
        let mut pending = conn
            .query("SELECT key FROM ai_label_dirty LIMIT 64", ())
            .await
            .map_err(|e| e.to_string())?;
        let mut keys = Vec::new();
        while let Some(row) = pending.next().await.map_err(|e| e.to_string())? {
            keys.push(row.get::<String>(0).map_err(|e| e.to_string())?);
        }
        drop(pending);
        if keys.is_empty() {
            return Ok(changed);
        }
        // Re-read all authoritative sources for each changed release, including
        // siblings omitted from the initial positive-only seed.
        let mut expanded = HashSet::new();
        for key in keys {
            if let Some((_, market, release)) = source_key(&key) {
                for kind in ["subscriber-summary", "subscriber-items", "tag-review"] {
                    expanded.insert(format!("{kind}:{market}:{release}"));
                }
            } else {
                expanded.insert(key);
            }
        }
        let keys: Vec<_> = expanded.into_iter().collect();
        // A bounded write transaction makes reading a source and removing its
        // dirty marker atomic with concurrent catalogue writes. No lost update.
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|e| e.to_string())?;
        let result = async {
            let mut raw_keys:HashSet<String>=keys.iter().filter(|key|source_key(key).is_some()).cloned().collect();
            let mut values=Vec::new();
            for key in &keys {
                if key.starts_with("ai-catalogue:") || key.starts_with("ai-queue:") {
                    for snapshot in project_snapshot(conn,key).await? {
                        for kind in ["subscriber-summary","subscriber-items","tag-review"] {
                            raw_keys.insert(format!("{kind}:{}:{}",snapshot.market,snapshot.release));
                        }
                        values.extend([snapshot.key,snapshot.market,snapshot.release,"snapshot".into(),serde_json::to_string(&snapshot.source).map_err(|e|e.to_string())?]);
                    }
                }
            }
            let raw_keys:Vec<_>=raw_keys.into_iter().collect();
            for keys in raw_keys.chunks(128) {
                let projected=project_sources(conn,keys).await?;
                let placeholders=vec!["?";keys.len()].join(",");
                let mut saved=conn.query(format!("SELECT key,payload FROM ai_label_sources WHERE key IN ({placeholders})"),keys.to_vec()).await.map_err(|e|e.to_string())?;
                let mut previous=HashMap::new();
                while let Some(row)=saved.next().await.map_err(|e|e.to_string())? {
                    let key:String=row.get(0).map_err(|e|e.to_string())?;
                    let payload:String=row.get(1).map_err(|e|e.to_string())?;
                    if let Ok(source)=serde_json::from_str::<Source>(&payload) {previous.insert(key,source);}
                }
                drop(saved);
                for (key,mut source) in projected {
                    let Some((kind,market,release))=source_key(&key) else {continue;};
                    retain_prior_evidence(&mut source,previous.get(&key));
                    values.extend([key.clone(),market.to_owned(),release.to_owned(),kind.to_owned(),serde_json::to_string(&source).map_err(|e|e.to_string())?]);
                }
                conn.execute(format!("DELETE FROM ai_label_sources WHERE key IN ({placeholders})"),keys.to_vec()).await.map_err(|e|e.to_string())?;
            }
            save_source_rows(conn,values).await?;
            for keys in keys.chunks(128) {
                let placeholders=vec!["?";keys.len()].join(",");
                conn.execute(format!("DELETE FROM ai_label_dirty WHERE key IN ({placeholders})"),keys.to_vec()).await.map_err(|e|e.to_string())?;
            }
            conn.execute("UPDATE ai_label_state SET value=value+1 WHERE key='generation'",()).await.map_err(|e|e.to_string())?;
            Ok::<(), String>(())
        }.await;
        match result {
            Ok(()) => {
                conn.execute("COMMIT", ())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(error);
            }
        }
        changed = true;
        tokio::task::yield_now().await;
    }
}

async fn project_sources(
    conn: &Connection,
    keys: &[String],
) -> Result<HashMap<String, Source>, String> {
    // Project booleans and IDs in SQLite instead of transferring/parsing credits,
    // biographies and artwork for every recording. Multi-path json_extract
    // preserves JSON boolean types: 1 and "true" are not provider labels.
    let placeholders = vec!["?"; keys.len()].join(",");
    let sql=format!("SELECT p.key, json_extract(p.payload,'$.id','$.ai','$.catalogue_metadata.data.id','$.catalogue_metadata.data.attributes.ai','$.album.id','$.album.ai','$.catalogue_attributes.ai'), CASE WHEN p.key GLOB 'subscriber-summary:*' THEN '[]' ELSE (SELECT json_group_array(json_extract(j.value,'$.id','$.ai','$.item.id','$.item.ai','$.type','$.album.id','$.album.ai','$.item.album.id','$.item.album.ai')) FROM json_each(p.payload,CASE WHEN p.key GLOB 'subscriber-items:*' THEN '$.items' ELSE '$.tracks' END) j WHERE j.type='object') END FROM app_preferences p WHERE p.key IN ({placeholders}) AND json_valid(p.payload)");
    let mut rows = conn
        .query(sql, keys.to_vec())
        .await
        .map_err(|e| e.to_string())?;
    let mut result = HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        let Some((kind, _, release)) = source_key(&key) else {
            continue;
        };
        let raw: String = row.get(1).map_err(|e| e.to_string())?;
        let fields: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if id(&fields[0]).is_some_and(|value| value != release) {
            continue;
        }
        let legacy_ai = (id(&fields[2]).is_none_or(|value| value == release))
            .then(|| fields[3].as_bool())
            .flatten();
        let mut source = Source {
            release: if kind == "tag-review" {
                fields[1]
                    .as_bool()
                    .or_else(|| fields[6].as_bool())
                    .or(legacy_ai)
            } else {
                fields[1].as_bool()
            },
            tracks: HashMap::new(),
        };
        if kind == "subscriber-items" && id(&fields[4]).as_deref() == Some(release) {
            source.release = source.release.or_else(|| fields[5].as_bool());
        }
        let raw: String = row.get(2).map_err(|e| e.to_string())?;
        let tracks: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        for fields in tracks.as_array().into_iter().flatten() {
            if fields[4].as_str().is_some_and(|value| {
                value.eq_ignore_ascii_case("video") || value.eq_ignore_ascii_case("videos")
            }) {
                continue;
            }
            let Some(track) = id(&fields[2]).or_else(|| id(&fields[0])) else {
                continue;
            };
            let label = if id(&fields[2]).is_some() {
                fields[3].as_bool()
            } else {
                fields[1].as_bool()
            };
            source.tracks.insert(track, label);
            if kind == "subscriber-items" {
                for (album, label) in [(&fields[5], &fields[6]), (&fields[7], &fields[8])] {
                    if id(album).as_deref() == Some(release) && label.as_bool() == Some(true) {
                        source.release = Some(true);
                    }
                }
            }
        }
        result.insert(key, source);
    }
    Ok(result)
}

#[derive(Default)]
struct ReleaseSources {
    summary: Option<Source>,
    items: Option<Source>,
    legacy: Option<Source>,
    snapshots: Vec<Source>,
}

async fn load_flags(conn: &Connection, market: &str) -> Result<AiFlags, String> {
    let mut rows = conn
        .query(
            "SELECT release_id,kind,payload FROM ai_label_sources WHERE market=?",
            (market,),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut releases: HashMap<String, ReleaseSources> = HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let release: String = row.get(0).map_err(|e| e.to_string())?;
        let kind: String = row.get(1).map_err(|e| e.to_string())?;
        let payload: String = row.get(2).map_err(|e| e.to_string())?;
        let source: Source = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        let entry = releases.entry(release).or_default();
        match kind.as_str() {
            "subscriber-summary" => entry.summary = Some(source),
            "subscriber-items" => entry.items = Some(source),
            "tag-review" => entry.legacy = Some(source),
            "snapshot" => entry.snapshots.push(source),
            _ => {}
        }
    }
    Ok(build_flags(releases))
}

fn build_flags(releases: HashMap<String, ReleaseSources>) -> AiFlags {
    let mut flags = AiFlags::default();
    let mut track_labels: HashMap<String, (u8, bool)> = HashMap::new();
    let mut memberships = Vec::new();
    for (release, sources) in releases {
        // A primary explicit true/false wins, while optional-field omission
        // falls back to saved evidence rather than silently treating it as false.
        let label = sources
            .summary
            .as_ref()
            .and_then(|s| s.release)
            .or_else(|| sources.items.as_ref().and_then(|s| s.release))
            .or_else(|| sources.legacy.as_ref().and_then(|s| s.release))
            .or_else(|| {
                sources
                    .snapshots
                    .iter()
                    .any(|s| s.release == Some(true))
                    .then_some(true)
            });
        if let Some(label) = label {
            flags.release_states.insert(release.clone(), label);
        }
        if label == Some(true) {
            flags.releases.insert(release.clone());
        }
        for (source, priority) in sources
            .snapshots
            .iter()
            .map(|source| (Some(source), 0))
            .chain([(sources.legacy.as_ref(), 1), (sources.items.as_ref(), 2)])
        {
            if let Some(source) = source {
                for (track, label) in &source.tracks {
                    memberships.push((release.clone(), track.clone()));
                    let Some(positive) = label else {
                        continue;
                    };
                    let entry = track_labels
                        .entry(track.clone())
                        .or_insert((priority, *positive));
                    if priority > entry.0 {
                        *entry = (priority, *positive);
                    } else if priority == entry.0 {
                        entry.1 |= *positive;
                    }
                }
            }
        }
    }
    flags.track_states = track_labels
        .iter()
        .map(|(id, (_, label))| (id.clone(), *label))
        .collect();
    flags.tracks = track_labels
        .into_iter()
        .filter_map(|(id, (_, positive))| positive.then_some(id))
        .collect();
    for (release, track) in memberships {
        if flags.tracks.contains(&track) {
            flags.releases_with_ai_tracks.insert(release);
        }
    }
    flags
}

/// Snapshot payloads may outlive a provider correction. Resolve their positive
/// AI claims against exact current raw sources before blocking a download.
/// Ordinary unlabelled payloads incur no extra cache projection or network work.
pub(crate) async fn payload_blocked(
    db: &TursoDb,
    market: &str,
    release_id: &str,
    payload: &Value,
) -> Result<bool, String> {
    let cached = flags(db, market).await?;
    if cached.release_blocked(release_id) {
        return Ok(true);
    }
    if !cached.payload_blocked(release_id, payload) {
        return Ok(false);
    }
    let market = market.trim().to_ascii_uppercase();
    let keys: Vec<_> = ["subscriber-summary", "subscriber-items", "tag-review"]
        .into_iter()
        .map(|kind| format!("{kind}:{market}:{release_id}"))
        .collect();
    let projected = project_sources(&db.connect()?, &keys).await?;
    let mut sources = ReleaseSources::default();
    for (key, source) in projected {
        match source_key(&key).map(|(kind, _, _)| kind) {
            Some("subscriber-summary") => sources.summary = Some(source),
            Some("subscriber-items") => sources.items = Some(source),
            Some("tag-review") => sources.legacy = Some(source),
            _ => {}
        }
    }
    let mut effective = (*cached).clone();
    let current = build_flags(HashMap::from([(release_id.to_owned(), sources)]));
    effective.release_states.extend(current.release_states);
    effective.track_states.extend(current.track_states);
    effective.releases.extend(current.releases);
    effective
        .releases_with_ai_tracks
        .extend(current.releases_with_ai_tracks);
    effective.tracks.extend(current.tracks);
    Ok(effective.payload_blocked(release_id, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TempDb {
        root: std::path::PathBuf,
    }
    impl TempDb {
        fn new() -> Self {
            Self {
                root: std::env::temp_dir()
                    .join(format!("tibrary-ai-policy-{}", uuid::Uuid::new_v4())),
            }
        }
        async fn open(&self) -> TursoDb {
            TursoDb::open(self.root.join("library.sqlite3"))
                .await
                .unwrap()
        }
    }
    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn backfills_exact_boolean_labels_and_preserves_mixed_release_tracks() {
        let temp = TempDb::new();
        let db = temp.open().await;
        db.set_preference("subscriber-summary:GB:10", &json!({"id":10,"ai":true}))
            .await
            .unwrap();
        db.set_preference("subscriber-summary:GB:20", &json!({"id":20,"ai":false}))
            .await
            .unwrap();
        db.set_preference("subscriber-items:GB:20", &json!({"items":[{"id":201,"ai":true},{"id":202,"ai":false},{"id":203,"ai":"true"},{"id":204,"ai":1},{"id":205}]})).await.unwrap();
        db.set_preference("subscriber-items:GB:30", &json!({"items":[{"type":"video","item":{"id":301,"ai":true}},{"type":"track","item":{"id":302,"ai":true}}]})).await.unwrap();
        db.set_preference("subscriber-summary:US:40", &json!({"id":40,"ai":true}))
            .await
            .unwrap();
        db.set_preference("subscriber-summary:GB:50", &json!({"id":51,"ai":true}))
            .await
            .unwrap();
        db.set_preference(
            "tag-review:GB:60",
            &json!({"catalogue_metadata":{"data":{"id":"60","attributes":{"ai":true}}}}),
        )
        .await
        .unwrap();
        db.set_preference(
            "tag-review:GB:61",
            &json!({"catalogue_attributes":{"ai":true}}),
        )
        .await
        .unwrap();
        let result = flags(&db, "GB").await.unwrap();
        assert!(result.release_labelled("61"));
        assert!(result.release_labelled("10"));
        assert!(result.release_blocked("20"));
        assert!(!result.release_labelled("20"));
        assert!(result.track_labelled("201"));
        assert!(result.evidence("20", "201").is_some());
        for id in ["202", "203", "204", "205"] {
            assert!(!result.track_labelled(id));
            assert!(result.evidence("20", id).is_none());
        }
        assert!(!result.track_labelled("301"));
        assert!(result.track_labelled("302"));
        assert!(!result.release_blocked("40"));
        assert!(!result.release_blocked("50"));
        assert!(result.release_labelled("60"));
        assert!(flags(&db, "US").await.unwrap().release_labelled("40"));
        assert!(Arc::ptr_eq(&result, &flags(&db, "GB").await.unwrap()));
    }

    #[tokio::test]
    async fn direct_cache_writes_and_deletions_refresh_primary_overrides_without_revision_bump() {
        let temp = TempDb::new();
        let db = temp.open().await;
        db.set_preference(
            "tag-review:GB:10",
            &json!({"id":"10","ai":true,"tracks":[{"id":"101","ai":true}]}),
        )
        .await
        .unwrap();
        assert!(flags(&db, "GB").await.unwrap().release_labelled("10"));
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)",
            (
                "subscriber-summary:GB:10",
                json!({"id":10,"ai":false}).to_string(),
            ),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO app_preferences(key,payload) VALUES(?,?)",
            (
                "subscriber-items:GB:10",
                json!({"items":[{"id":101,"ai":false}]}).to_string(),
            ),
        )
        .await
        .unwrap();
        let result = flags(&db, "GB").await.unwrap();
        assert!(!result.release_blocked("10"));
        assert!(!result.track_labelled("101"));
        conn.execute(
            "UPDATE app_preferences SET payload=? WHERE key=?",
            (
                json!({"id":10,"ai":true}).to_string(),
                "subscriber-summary:GB:10",
            ),
        )
        .await
        .unwrap();
        assert!(flags(&db, "GB").await.unwrap().release_labelled("10"));
        conn.execute(
            "DELETE FROM app_preferences WHERE key=?",
            ("subscriber-summary:GB:10",),
        )
        .await
        .unwrap();
        // A remaining legacy flag is useful after the primary cache is removed.
        assert!(flags(&db, "GB").await.unwrap().release_labelled("10"));
        conn.execute(
            "UPDATE app_preferences SET payload=? WHERE key=?",
            (json!({"id":10,"ai":false}).to_string(), "tag-review:GB:10"),
        )
        .await
        .unwrap();
        assert!(!flags(&db, "GB").await.unwrap().release_labelled("10"));
    }

    #[tokio::test]
    async fn persisted_index_tracks_changes_between_sessions_and_keeps_databases_separate() {
        let first = TempDb::new();
        let second = TempDb::new();
        {
            let db = first.open().await;
            db.set_preference("subscriber-summary:GB:10", &json!({"ai":true}))
                .await
                .unwrap();
            assert!(flags(&db, "GB").await.unwrap().release_labelled("10"));
            db.set_preference("subscriber-summary:GB:20", &json!({"ai":true}))
                .await
                .unwrap();
        }
        let reopened = first.open().await;
        let result = flags(&reopened, "GB").await.unwrap();
        assert!(result.release_labelled("10"));
        assert!(result.release_labelled("20"));
        let other = second.open().await;
        assert!(!flags(&other, "GB").await.unwrap().release_labelled("10"));
    }
    #[tokio::test]
    async fn omitted_optional_labels_keep_saved_evidence_until_explicitly_cleared() {
        let temp = TempDb::new();
        let db = temp.open().await;
        db.set_preference(
            "tag-review:GB:10",
            &json!({"ai":true,"tracks":[{"id":"101","ai":true}]}),
        )
        .await
        .unwrap();
        db.set_preference("subscriber-summary:GB:10", &json!({"id":10}))
            .await
            .unwrap();
        db.set_preference("subscriber-items:GB:10", &json!({"items":[{"id":101}]}))
            .await
            .unwrap();
        assert!(flags(&db, "GB").await.unwrap().release_labelled("10"));
        assert!(flags(&db, "GB").await.unwrap().track_labelled("101"));
        db.set_preference("subscriber-summary:GB:20", &json!({"id":20,"ai":true}))
            .await
            .unwrap();
        db.set_preference(
            "subscriber-items:GB:20",
            &json!({"items":[{"id":201,"ai":true}]}),
        )
        .await
        .unwrap();
        assert!(flags(&db, "GB").await.unwrap().track_labelled("201"));
        db.set_preference("subscriber-summary:GB:20", &json!({"id":20}))
            .await
            .unwrap();
        db.set_preference("subscriber-items:GB:20", &json!({"items":[{"id":201}]}))
            .await
            .unwrap();
        let saved = flags(&db, "GB").await.unwrap();
        assert!(saved.release_labelled("20"));
        assert!(saved.track_labelled("201"));
        db.set_preference("subscriber-summary:GB:20", &json!({"id":20,"ai":false}))
            .await
            .unwrap();
        db.set_preference(
            "subscriber-items:GB:20",
            &json!({"items":[{"id":201,"ai":false}]}),
        )
        .await
        .unwrap();
        let cleared = flags(&db, "GB").await.unwrap();
        assert!(!cleared.release_blocked("20"));
        assert!(!cleared.track_labelled("201"));
    }

    #[tokio::test]
    async fn stale_positive_payloads_use_current_explicit_primary_flags() {
        let temp = TempDb::new();
        let db = temp.open().await;
        db.set_preference("subscriber-summary:GB:10", &json!({"id":10,"ai":false}))
            .await
            .unwrap();
        db.set_preference(
            "subscriber-items:GB:10",
            &json!({"items":[{"id":101,"ai":false}]}),
        )
        .await
        .unwrap();
        let old = json!({"ai":true,"tracks":[{"id":"101","ai":true}],"selected_tracks":[{"id":101,"ai":true}]});
        assert!(!payload_blocked(&db, "GB", "10", &old).await.unwrap());
        assert!(payload_blocked(&db, "GB", "20", &old).await.unwrap());
        let nested = json!({"catalogue_attributes":{"ai":true}});
        assert!(!payload_blocked(&db, "GB", "10", &nested).await.unwrap());
        assert!(payload_blocked(&db, "GB", "20", &nested).await.unwrap());
        assert!(!payload_blocked(
            &db,
            "GB",
            "20",
            &json!({"ai":false,"catalogue_attributes":{"ai":true}})
        )
        .await
        .unwrap());
        db.set_preference(
            "tag-review:GB:40",
            &json!({"ai":false,"catalogue_metadata":{"data":{"attributes":{"ai":true}}}}),
        )
        .await
        .unwrap();
        assert!(!flags(&db, "GB").await.unwrap().release_labelled("40"));
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO app_preferences(key,payload) VALUES(?,?)",
            (
                "subscriber-summary:GB:30",
                "{\"id\": 30, \"ai\" : \n  true}",
            ),
        )
        .await
        .unwrap();
        assert!(flags(&db, "GB").await.unwrap().release_labelled("30"));
    }

    #[tokio::test]
    async fn local_review_generation_ignores_unrelated_online_metadata() {
        let temp = TempDb::new();
        let db = temp.open().await;
        db.set_preference("subscriber-summary:GB:10", &json!({"ai":true}))
            .await
            .unwrap();
        let signature = flags(&db, "GB").await.unwrap().review_signature();
        let before = local_generation(&db).await.unwrap();
        db.set_preference("subscriber-summary:GB:20", &json!({"ai":false}))
            .await
            .unwrap();
        assert_eq!(
            signature,
            flags(&db, "GB").await.unwrap().review_signature()
        );
        assert_eq!(before, local_generation(&db).await.unwrap());
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO local_files(path,root,present) VALUES('/copy/track.flac','/copy',1)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "UPDATE local_files SET size=12 WHERE path='/copy/track.flac'",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO track_links(path,market,payload) VALUES('/copy/track.flac','GB','{}')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "UPDATE track_links SET payload='null' WHERE path='/copy/track.flac'",
            (),
        )
        .await
        .unwrap();
        conn.execute("DELETE FROM track_links WHERE path='/copy/track.flac'", ())
            .await
            .unwrap();
        conn.execute("DELETE FROM local_files WHERE path='/copy/track.flac'", ())
            .await
            .unwrap();
        assert_eq!(before + 6, local_generation(&db).await.unwrap());
        db.set_preference("subscriber-summary:GB:20", &json!({"ai":true}))
            .await
            .unwrap();
        assert_ne!(
            signature,
            flags(&db, "GB").await.unwrap().review_signature()
        );
    }

    #[tokio::test]
    async fn snapshot_only_labels_are_indexed_and_primary_false_overrides_them() {
        let temp = TempDb::new();
        let db = temp.open().await;
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES(?,?,?)",("1","GB",json!({"releases":[{"id":"10","catalogue_attributes":{"ai":true}},{"id":"20","tracks":[{"id":"201","ai":true},{"id":"202","ai":false}]}]}).to_string())).await.unwrap();
        conn.execute(
            "INSERT INTO queue(id,payload,decision) VALUES(?,?,?)",
            ("30", json!({"id":"30","ai":true}).to_string(), "downloaded"),
        )
        .await
        .unwrap();
        let first = flags(&db, "GB").await.unwrap();
        assert!(first.release_labelled("10"));
        assert!(first.release_blocked("20"));
        assert!(first.track_labelled("201"));
        assert!(first.evidence("20", "202").is_none());
        assert!(first.release_labelled("30"));
        assert!(payload_blocked(&db, "GB", "10", &json!({"id":"10"}))
            .await
            .unwrap());
        db.set_preference("subscriber-summary:GB:10", &json!({"ai":false}))
            .await
            .unwrap();
        db.set_preference(
            "subscriber-items:GB:20",
            &json!({"items":[{"id":201,"ai":false}]}),
        )
        .await
        .unwrap();
        let corrected = flags(&db, "GB").await.unwrap();
        assert!(!corrected.release_blocked("10"));
        assert!(!corrected.release_blocked("20"));
        conn.execute(
            "UPDATE queue SET payload=? WHERE id=?",
            (json!({"id":"30","ai":false}).to_string(), "30"),
        )
        .await
        .unwrap();
        assert!(!flags(&db, "GB").await.unwrap().release_blocked("30"));
        conn.execute(
            "INSERT INTO queue(id,payload) VALUES(?,?)",
            (
                "40",
                json!({"selected_tracks":[{"id":"401","ai":true}]}).to_string(),
            ),
        )
        .await
        .unwrap();
        assert!(flags(&db, "GB")
            .await
            .unwrap()
            .evidence("40", "401")
            .is_some());
        conn.execute("DELETE FROM queue WHERE id='40'", ())
            .await
            .unwrap();
        assert!(!flags(&db, "GB").await.unwrap().release_blocked("40"));
        conn.execute(
            "UPDATE catalogue SET payload=? WHERE artist_id='1'",
            (json!({"releases":[{"id":"50","ai":true}]}).to_string(),),
        )
        .await
        .unwrap();
        assert!(flags(&db, "GB").await.unwrap().release_labelled("50"));
        assert!(!flags(&db, "US").await.unwrap().release_labelled("50"));
        conn.execute("DELETE FROM catalogue WHERE artist_id='1'", ())
            .await
            .unwrap();
        assert!(!flags(&db, "GB").await.unwrap().release_labelled("50"));
    }

    #[tokio::test]
    #[ignore = "Performance check using an explicitly supplied disposable cache copy"]
    async fn benchmark_saved_ai_cache() {
        let path = std::env::var("TIBRARY_AI_POLICY_BENCH_DB")
            .expect("Set the disposable cache copy path");
        assert!(
            path.starts_with("/private/tmp/tibrary-ai-policy-benchmark-"),
            "Benchmark must use a disposable copy"
        );
        let db = TursoDb::open(&path).await.unwrap();
        let start = std::time::Instant::now();
        let result = flags(&db, "GB").await.unwrap();
        let initial = start.elapsed();
        let start = std::time::Instant::now();
        let cached = flags(&db, "GB").await.unwrap();
        let repeated = start.elapsed();
        assert!(Arc::ptr_eq(&result, &cached));
        let start = std::time::Instant::now();
        let review = db.get_ai_review_rows(None, "GB").await.unwrap();
        let first_review = start.elapsed();
        let start = std::time::Instant::now();
        let second_review = db.get_ai_review_rows(None, "GB").await.unwrap();
        let warm_review = start.elapsed();
        assert_eq!(review, second_review);
        let synthetic = "subscriber-summary:GB:9999999999999999998";
        assert!(db.get_preference(synthetic).await.unwrap().is_none());
        db.set_preference(synthetic, &json!({"ai":false}))
            .await
            .unwrap();
        let start = std::time::Instant::now();
        let after_metadata = db.get_ai_review_rows(None, "GB").await.unwrap();
        let after_metadata_time = start.elapsed();
        assert_eq!(review, after_metadata);
        db.connect()
            .unwrap()
            .execute("DELETE FROM app_preferences WHERE key=?", (synthetic,))
            .await
            .unwrap();
        println!("AI review timings: initial={first_review:?}, repeated={warm_review:?}, after unrelated metadata={after_metadata_time:?}");
        println!("AI label cache: initial={initial:?}, repeated={repeated:?}, labelled releases={}, releases containing labelled tracks={}, labelled tracks={}, local review rows={}",result.releases.len(),result.releases_with_ai_tracks.len(),result.tracks.len(),review.len());
    }
}
