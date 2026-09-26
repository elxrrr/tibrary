use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use turso::{Builder, Connection, Database};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootRecord {
    pub root: String,
    pub scanned_at: Option<String>,
    pub status: Option<String>,
    pub tracks: usize,
    pub linked: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalFileRecord {
    pub path: String,
    pub root: String,
    pub size: i64,
    pub mtime: i64,
    pub metadata: Option<Value>,
    pub error: Option<String>,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueRecord {
    pub id: String,
    pub payload: Value,
    pub approved: bool,
    pub decision: String,
    pub updated: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatsRecord {
    pub track_count: usize,
    pub linked_tracks: usize,
    pub release_count: usize,
    pub linked_releases: usize,
    pub partial_releases: usize,
    pub artists: usize,
    pub unresolved_artists: usize,
    pub approved_queue: usize,
    pub queued: usize,
    pub downloaded: usize,
    pub missing_releases: usize,
    pub correct: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkRow {
    pub id: String,
    pub artist: String,
    pub release: String,
    pub title: String,
    pub path: String,
    pub position: String,
    pub status: String,
    pub evidence: String,
    pub affected: bool,
    pub target: String,
    pub changes: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub candidates: usize,
    pub online_id: String,
    pub ignored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingChildTrack {
    pub id: String,
    pub title: String,
    pub position: String,
    pub duration: Option<f64>,
    pub isrc: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingRow {
    pub id: String,
    pub downloaded_files: Vec<String>,
    pub artist: String,
    pub release: String,
    pub date: String,
    pub r#type: String,
    pub tracks: usize,
    pub status: String,
    pub online_id: String,
    pub recommendation: String,
    pub evidence: Vec<String>,
    pub expanded_available: bool,
    pub available: Option<bool>,
    pub approved: bool,
    pub selected: Option<Vec<String>>,
    pub children: Vec<MissingChildTrack>,
    #[serde(default)]
    pub newest_local_date: Option<String>,
    #[serde(default)]
    pub previous_local_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TablePage<T> {
    pub rows: Vec<T>,
    pub total: usize,
    pub offset: usize,
    pub revision: u64,
    pub preview_id: Option<String>,
}

#[derive(Clone)]
pub struct TursoDb {
    pub db: Database,
    pub path: PathBuf,
    pub revision: Arc<std::sync::atomic::AtomicU64>,
    missing_rows_gate: Arc<tokio::sync::Mutex<()>>,
    missing_rows_cache: Arc<std::sync::Mutex<HashMap<String, (u64, Vec<MissingRow>)>>>,
    link_rows_cache: Arc<std::sync::Mutex<HashMap<String, (u64, Vec<LinkRow>)>>>,
    favourite_rows_cache: Arc<std::sync::Mutex<HashMap<String, (u64, Vec<Value>)>>>,
}

impl TursoDb {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path_buf = path.as_ref().to_path_buf();
        if let Some(parent) = path_buf.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let path_str = path_buf
            .to_str()
            .ok_or_else(|| "Database path contains invalid UTF-8".to_string())?;
        let db = Builder::new_local(path_str)
            .build()
            .await
            .map_err(|e| format!("Failed to open Turso database at {:?}: {}", path_buf, e))?;
        let instance = Self {
            db,
            path: path_buf,
            revision: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            missing_rows_gate: Arc::new(tokio::sync::Mutex::new(())),
            missing_rows_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            link_rows_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            favourite_rows_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
        };
        instance.init_schema().await?;
        Ok(instance)
    }

    pub fn bump_revision(&self) -> u64 {
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    pub fn invalidate_missing_rows(&self) {
        self.missing_rows_cache.lock().unwrap().clear();
        self.bump_revision();
    }

    pub fn connect(&self) -> Result<Connection, String> {
        self.db
            .connect()
            .map_err(|e| format!("Turso connect error: {}", e))
    }

    pub async fn init_schema(&self) -> Result<(), String> {
        let conn = self.connect()?;
        let _ = conn.execute("PRAGMA journal_mode = WAL", ()).await;
        let _ = conn.execute("PRAGMA busy_timeout = 10000", ()).await;
        let _ = conn.execute("PRAGMA synchronous = NORMAL", ()).await;
        let ddl_statements = [
            "CREATE TABLE IF NOT EXISTS roots(root TEXT PRIMARY KEY, scanned_at TEXT, status TEXT)",
            "CREATE TABLE IF NOT EXISTS local_files(path TEXT PRIMARY KEY, root TEXT, size INTEGER, mtime INTEGER, metadata TEXT, error TEXT, present INTEGER DEFAULT 1)",
            "CREATE TABLE IF NOT EXISTS scans(id INTEGER PRIMARY KEY, root TEXT, started TEXT, ended TEXT, status TEXT, summary TEXT)",
            "CREATE TABLE IF NOT EXISTS match_reviews(artist TEXT PRIMARY KEY, status TEXT, payload TEXT, error TEXT, updated TEXT)",
            "CREATE TABLE IF NOT EXISTS app_preferences(key TEXT PRIMARY KEY, payload TEXT)",
            "CREATE TABLE IF NOT EXISTS mappings(artist TEXT PRIMARY KEY, tidal_id TEXT, status TEXT, evidence TEXT, manual INTEGER DEFAULT 0)",
            "CREATE TABLE IF NOT EXISTS additional_mappings(artist TEXT, tidal_id TEXT, PRIMARY KEY(artist,tidal_id))",
            "CREATE TABLE IF NOT EXISTS catalogue(artist_id TEXT, market TEXT, payload TEXT, fetched TEXT, PRIMARY KEY(artist_id,market))",
            "CREATE TABLE IF NOT EXISTS track_links(path TEXT, market TEXT, stamp TEXT, payload TEXT, PRIMARY KEY(path,market))",
            "CREATE TABLE IF NOT EXISTS favourite_artists(cache_id TEXT PRIMARY KEY, payload TEXT, fetched TEXT)",
            "CREATE TABLE IF NOT EXISTS queue(id TEXT PRIMARY KEY, payload TEXT, approved INTEGER DEFAULT 0, decision TEXT DEFAULT 'queued', updated TEXT)",
            "CREATE TABLE IF NOT EXISTS ignored_local_files(path TEXT PRIMARY KEY, ignored_at TEXT)",
            "CREATE TABLE IF NOT EXISTS activity_logs(id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT, message TEXT, level TEXT, category TEXT)",
        ];

        for stmt in ddl_statements {
            conn.execute(stmt, ())
                .await
                .map_err(|e| format!("Schema init error on '{}': {}", stmt, e))?;
        }
        Ok(())
    }

    pub async fn log_activity(
        &self,
        at: &str,
        message: &str,
        level: &str,
        category: &str,
    ) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO activity_logs (at, message, level, category) VALUES (?, ?, ?, ?)",
            (at, message, level, category),
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn load_recent_logs(&self, limit: usize) -> Result<Vec<Value>, String> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT at, message, level, category FROM activity_logs ORDER BY id DESC LIMIT ?",
                (limit as i64,),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut logs = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            let at: String = row.get(0).unwrap_or_default();
            let msg: String = row.get(1).unwrap_or_default();
            let lvl: String = row.get(2).unwrap_or_else(|_| "info".to_string());
            let cat: String = row.get(3).unwrap_or_else(|_| "general".to_string());
            logs.push(json!({
                "at": at,
                "message": msg,
                "level": lvl,
                "category": cat,
            }));
        }
        logs.reverse();
        Ok(logs)
    }

    pub async fn clear_logs(&self) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM activity_logs", ())
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn clear_log_stream(&self, stream: &str) -> Result<(), String> {
        let conn = self.connect()?;
        let download = "(category = 'download' OR (category NOT IN ('online', 'linking', 'scan', 'cleanup', 'local') AND (lower(message) LIKE '%download%' OR lower(message) LIKE '%fetching track%' OR lower(message) LIKE '%saving track%')))";
        let online = "(category IN ('linking', 'online') OR (category NOT IN ('scan', 'cleanup', 'local') AND (lower(message) LIKE '%catalogue%' OR lower(message) LIKE '%api%' OR lower(message) LIKE '%remote%' OR lower(message) LIKE '%artist search%' OR lower(message) LIKE '%releases%' OR lower(message) LIKE '%metadata source%')))";
        let condition = match stream {
            "downloads" => download.to_string(),
            "online" => format!("NOT {download} AND {online}"),
            "local" => format!("NOT {download} AND NOT {online}"),
            _ => return Err("Unknown activity stream".into()),
        };
        conn.execute(&format!("DELETE FROM activity_logs WHERE {condition}"), ())
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn list_roots(&self, market: &str) -> Result<Vec<RootRecord>, String> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT root, scanned_at, status FROM roots ORDER BY root",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut roots = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            let root: String = row.get(0).map_err(|e| e.to_string())?;
            let scanned_at: Option<String> = row.get(1).map_err(|e| e.to_string())?;
            let status: Option<String> = row.get(2).map_err(|e| e.to_string())?;
            roots.push((root, scanned_at, status));
        }

        // Gather track link counts per root
        let active_links = self.get_active_links(&conn, market).await?;
        let mut result = Vec::new();
        for (root, scanned_at, status) in roots {
            let clean = root.trim_end_matches('/');
            let with_slash = format!("{}/", clean);
            let mut file_rows = conn
                .query(
                    "SELECT path FROM local_files WHERE (root = ? OR root = ?) AND present = 1",
                    (clean, with_slash.as_str()),
                )
                .await
                .map_err(|e| e.to_string())?;

            let mut tracks = 0;
            let mut linked = 0;
            while let Some(file_row) = file_rows.next().await.map_err(|e| e.to_string())? {
                let path: String = file_row.get(0).map_err(|e| e.to_string())?;
                tracks += 1;
                if active_links.contains(&path) {
                    linked += 1;
                }
            }

            result.push(RootRecord {
                root,
                scanned_at,
                status,
                tracks,
                linked,
            });
        }

        Ok(result)
    }

    pub async fn add_root(&self, root: &str) -> Result<(), String> {
        let conn = self.connect()?;
        let clean = root.trim_end_matches('/');
        conn.execute(
            "INSERT OR REPLACE INTO roots (root, status) VALUES (?, 'active')",
            (clean,),
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn remove_root(&self, root: &str) -> Result<(), String> {
        let conn = self.connect()?;
        let clean = root.trim_end_matches('/');
        let with_slash = format!("{}/", clean);
        conn.execute(
            "DELETE FROM roots WHERE root = ? OR root = ?",
            (clean, with_slash.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM local_files WHERE root = ? OR root = ?",
            (clean, with_slash.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get_linked_artist_ids(&self) -> Result<Vec<String>, String> {
        let conn = self.connect()?;
        let mut ids = HashSet::new();

        let mut stmt = conn
            .query(
                "SELECT artist, tidal_id FROM mappings WHERE status IN ('confirmed', 'auto')",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;

        while let Some(row) = stmt.next().await.map_err(|e| e.to_string())? {
            let artist: String = row.get(0).unwrap_or_default();
            let tid: String = row.get(1).unwrap_or_default();
            if !is_compilation_artist(&artist) && !tid.trim().is_empty() {
                ids.insert(tid.trim().to_string());
            }
        }

        let mut add_stmt = conn
            .query("SELECT artist, tidal_id FROM additional_mappings", ())
            .await
            .map_err(|e| e.to_string())?;

        while let Some(row) = add_stmt.next().await.map_err(|e| e.to_string())? {
            let artist: String = row.get(0).unwrap_or_default();
            let tid: String = row.get(1).unwrap_or_default();
            if !is_compilation_artist(&artist) && !tid.trim().is_empty() {
                ids.insert(tid.trim().to_string());
            }
        }

        let mut res: Vec<String> = ids.into_iter().collect();
        res.sort();
        Ok(res)
    }

    pub async fn apply_file_update(
        &self,
        source: &str,
        target: &str,
        root: &str,
        meta: &Value,
        size: i64,
        mtime: i64,
    ) -> Result<(), String> {
        let conn = self.connect()?;
        let meta_json = serde_json::to_string(meta).map_err(|e| e.to_string())?;
        let mut previous = conn
            .query("SELECT metadata FROM local_files WHERE path=?", (source,))
            .await
            .map_err(|e| e.to_string())?;
        let old = if let Some(row) = previous.next().await.map_err(|e| e.to_string())? {
            serde_json::from_str::<Value>(&row.get::<String>(0).unwrap_or_default()).ok()
        } else {
            None
        };
        drop(previous);
        let identity = |value: &Option<Value>| {
            let tags = crate::workflows::extract_tags_map(value);
            [
                "title",
                "album",
                "albumartist",
                "isrc",
                "tracknumber",
                "discnumber",
            ]
            .map(|key| {
                let text = tags.get(key).cloned().unwrap_or_default();
                if key == "tracknumber" || key == "discnumber" {
                    text.split('/')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .parse::<u32>()
                        .map(|n| n.to_string())
                        .unwrap_or(text)
                } else {
                    text
                }
            })
        };
        let same_identity = identity(&old) == identity(&Some(meta.clone()));

        if source != target {
            conn.execute(
                "UPDATE local_files SET present = 0 WHERE path = ?",
                (source,),
            )
            .await
            .map_err(|e| e.to_string())?;

            conn.execute(
                "UPDATE track_links SET path = ? WHERE path = ?",
                (target, source),
            )
            .await
            .map_err(|e| e.to_string())?;
        }

        conn.execute(
            "INSERT OR REPLACE INTO local_files (path, root, size, mtime, metadata, error, present) VALUES (?, ?, ?, ?, ?, NULL, 1)",
            (target, root, size, mtime, meta_json.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;

        if same_identity {
            conn.execute(
                "UPDATE track_links SET stamp=? WHERE path=?",
                (json!([0, 0, size, mtime]).to_string(), target),
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        self.bump_revision();
        Ok(())
    }

    pub async fn remove_local_file(&self, path: &str) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute("UPDATE local_files SET present = 0 WHERE path = ?", (path,))
            .await
            .map_err(|e| e.to_string())?;

        conn.execute("DELETE FROM track_links WHERE path = ?", (path,))
            .await
            .map_err(|e| e.to_string())?;

        self.bump_revision();
        Ok(())
    }

    async fn get_active_links(
        &self,
        conn: &Connection,
        market: &str,
    ) -> Result<HashSet<String>, String> {
        let mut link_rows = conn
            .query(
                "SELECT path, stamp, payload FROM track_links WHERE market = ?",
                (market,),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut saved_links: HashMap<String, (String, String)> = HashMap::new();
        while let Some(row) = link_rows.next().await.map_err(|e| e.to_string())? {
            let path: String = row.get(0).map_err(|e| e.to_string())?;
            let stamp: String = row.get(1).map_err(|e| e.to_string())?;
            let payload: String = row.get(2).map_err(|e| e.to_string())?;
            saved_links.insert(path, (stamp, payload));
        }

        let mut file_rows = conn
            .query(
                "SELECT path, size, mtime, metadata FROM local_files WHERE present = 1",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut active = HashSet::new();
        while let Some(row) = file_rows.next().await.map_err(|e| e.to_string())? {
            let path: String = row.get(0).map_err(|e| e.to_string())?;
            let size: i64 = row.get(1).map_err(|e| e.to_string())?;
            let mtime: i64 = row.get(2).map_err(|e| e.to_string())?;
            let metadata_opt: Option<String> = row.get(3).ok().flatten();

            let mut has_link = false;
            if let Some((stamp_str, payload_str)) = saved_links.get(&path) {
                if let (Ok(_stamp_json), Ok(payload_json)) = (
                    serde_json::from_str::<Value>(stamp_str),
                    serde_json::from_str::<Value>(payload_str),
                ) {
                    let stamp_matches = link_stamp_matches(stamp_str, size, mtime);
                    if stamp_matches
                        && !matches!(
                            payload_json["status"].as_str(),
                            Some("review" | "unmatched" | "unlinked")
                        )
                    {
                        if let Some(ids) = payload_json.get("ids") {
                            let alb = ids
                                .get("album_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty());
                            let trk = ids
                                .get("track_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty());
                            if alb.is_some() && trk.is_some() {
                                has_link = true;
                            }
                        }
                        if !has_link {
                            if let Some(cc) = payload_json.get("catalogue_choice") {
                                let alb = cc
                                    .get("id")
                                    .or_else(|| cc.get("album_id"))
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.trim())
                                    .filter(|s| !s.is_empty());
                                let trk = cc
                                    .get("track_id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.trim())
                                    .filter(|s| !s.is_empty());
                                if alb.is_some() && trk.is_some() {
                                    has_link = true;
                                }
                            }
                        }
                    }
                }
            }
            if !has_link {
                if let Some(ref metadata_str) = metadata_opt {
                    if let Ok(meta) = serde_json::from_str::<Value>(metadata_str) {
                        let album_id = meta
                            .get("tidal_album_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty());
                        let track_id = meta
                            .get("tidal_track_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty());
                        if album_id.is_some() && track_id.is_some() {
                            has_link = true;
                        }
                    }
                }
            }

            if has_link {
                active.insert(path);
            }
        }

        Ok(active)
    }

    pub async fn get_stats(&self, market: &str, root: Option<&str>) -> Result<StatsRecord, String> {
        let conn = self.connect()?;
        let active_links = self.get_active_links(&conn, market).await?;

        // Query files
        let clean_root = root.map(|r| r.trim_end_matches('/').to_string());
        let (sql, params_vec): (&str, Vec<String>) = match clean_root.as_deref() {
            Some(r) if !r.is_empty() => (
                "SELECT path, metadata FROM local_files WHERE present = 1 AND (root = ? OR root = ?)",
                vec![r.to_string(), format!("{}/", r)],
            ),
            _ => (
                "SELECT path, metadata FROM local_files WHERE present = 1",
                vec![],
            ),
        };

        let mut rows = if params_vec.is_empty() {
            conn.query(sql, ()).await.map_err(|e| e.to_string())?
        } else {
            conn.query(sql, (params_vec[0].as_str(), params_vec[1].as_str()))
                .await
                .map_err(|e| e.to_string())?
        };

        let mut track_count = 0;
        let mut linked_tracks = 0;
        let mut correct_issues = 0;
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        let mut artists_set: HashSet<String> = HashSet::new();

        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            let path: String = row.get(0).map_err(|e| e.to_string())?;
            let metadata_opt: Option<String> = row.get(1).unwrap_or(None);

            track_count += 1;
            if active_links.contains(&path) {
                linked_tracks += 1;
            }

            if let Some(metadata_str) = metadata_opt {
                if let Ok(meta) = serde_json::from_str::<Value>(&metadata_str) {
                    let artist = meta
                        .get("album_artist")
                        .or_else(|| meta.get("artist"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown artist");
                    let album = meta.get("album").and_then(|v| v.as_str()).unwrap_or("");

                    let folder = crate::duplicates::extract_release_folder(&path);
                    let group_key = format!(
                        "{}|{}|{}",
                        folder.to_lowercase(),
                        artist.to_lowercase(),
                        album.to_lowercase()
                    );
                    groups.entry(group_key).or_default().push(path);

                    if !artist.is_empty() && !is_compilation_artist(artist) {
                        artists_set.insert(artist.to_lowercase());
                    }

                    // Check for tag correction issues across dates, numbers, keys, lyrics
                    let tags = crate::workflows::extract_tags_map(&Some(meta));
                    let mut has_correction = false;

                    if let Some(date) = tags.get("date") {
                        let clean = date.trim().replace(['/', '.'], "-");
                        let clean = clean.split('T').next().unwrap_or(&clean);
                        let valid = (clean.len() == 4 && clean.parse::<u32>().is_ok())
                            || chrono::NaiveDate::parse_from_str(clean, "%Y-%m-%d").is_ok();
                        if valid && clean != date {
                            has_correction = true;
                        }
                    }

                    if !has_correction {
                        for (number, total) in
                            [("tracknumber", "tracktotal"), ("discnumber", "disctotal")]
                        {
                            if let Some(value) = tags.get(number) {
                                let parts: Vec<_> = value.split('/').collect();
                                if let Ok(n) = parts[0].trim().parse::<u32>() {
                                    if n > 0 {
                                        let formatted = format!("{n:02}");
                                        if &formatted != value {
                                            has_correction = true;
                                            break;
                                        }
                                    }
                                }
                                if !tags.contains_key(total) && parts.len() == 2 {
                                    has_correction = true;
                                    break;
                                }
                            }
                        }
                    }

                    if !has_correction
                        && (tags.contains_key("unsyncedlyrics") || tags.contains_key("lyrics"))
                    {
                        has_correction = true;
                    }

                    if !has_correction {
                        let tags_vec: HashMap<String, Vec<String>> = tags
                            .iter()
                            .map(|(k, v)| (k.clone(), vec![v.clone()]))
                            .collect();
                        let (key_edits, issue) = crate::musical_keys::key_changes(&tags_vec);
                        if !key_edits.is_empty() || !issue.is_empty() {
                            has_correction = true;
                        }
                    }

                    if has_correction {
                        correct_issues += 1;
                    }
                }
            }
        }

        let release_count = groups.len();
        let mut linked_releases = 0;
        let mut partial_releases = 0;

        for paths in groups.values() {
            let linked_in_group = paths.iter().filter(|p| active_links.contains(*p)).count();
            if linked_in_group == paths.len() && !paths.is_empty() {
                linked_releases += 1;
            } else if linked_in_group > 0 {
                partial_releases += 1;
            }
        }

        // Mappings for unresolved artists
        let mut map_rows = conn
            .query(
                "SELECT artist FROM mappings WHERE status IN ('confirmed', 'auto')",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut mapped_artists = HashSet::new();
        while let Some(row) = map_rows.next().await.map_err(|e| e.to_string())? {
            let a: String = row.get(0).map_err(|e| e.to_string())?;
            mapped_artists.insert(a.to_lowercase());
        }

        let artists = artists_set.len();
        let unresolved_artists = artists_set.difference(&mapped_artists).count();

        // Queue counts
        let mut q_rows = conn
            .query("SELECT decision, approved FROM queue", ())
            .await
            .map_err(|e| e.to_string())?;

        let mut queued = 0;
        let mut approved_queue = 0;
        let mut downloaded = 0;

        while let Some(row) = q_rows.next().await.map_err(|e| e.to_string())? {
            let decision: String = row.get(0).map_err(|e| e.to_string())?;
            let approved: i64 = row.get(1).map_err(|e| e.to_string())?;

            if decision == "queued" {
                queued += 1;
                if approved != 0 {
                    approved_queue += 1;
                }
            } else if decision == "downloaded" {
                downloaded += 1;
            }
        }

        let missing_releases = match self
            .get_missing_rows(
                market,
                Some("All missing releases"),
                None,
                None,
                None,
                None,
                None,
                None,
                0,
                0,
            )
            .await
        {
            Ok(page) => page.total,
            Err(_) => 0,
        };

        Ok(StatsRecord {
            track_count,
            linked_tracks,
            release_count,
            linked_releases,
            partial_releases,
            artists,
            unresolved_artists,
            approved_queue,
            queued,
            downloaded,
            missing_releases,
            correct: correct_issues,
        })
    }

    pub async fn get_preference(&self, key: &str) -> Result<Option<Value>, String> {
        let conn = self.connect()?;
        let mut rows = conn
            .query("SELECT payload FROM app_preferences WHERE key = ?", (key,))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            let payload: String = row.get(0).map_err(|e| e.to_string())?;
            let val = serde_json::from_str(&payload).unwrap_or(Value::Null);
            Ok(Some(val))
        } else {
            Ok(None)
        }
    }

    pub async fn set_preference(&self, key: &str, val: &Value) -> Result<(), String> {
        let conn = self.connect()?;
        let payload = serde_json::to_string(val).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO app_preferences (key, payload) VALUES (?, ?)",
            (key, payload.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get_local_files_page(
        &self,
        root: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<LocalFileRecord>, usize), String> {
        let conn = self.connect()?;
        let (count_sql, select_sql, has_param) = match root {
            Some(_) => (
                "SELECT COUNT(*) FROM local_files WHERE present = 1 AND root = ?",
                "SELECT path, root, size, mtime, metadata, error, present FROM local_files WHERE present = 1 AND root = ? ORDER BY path LIMIT ? OFFSET ?",
                true,
            ),
            None => (
                "SELECT COUNT(*) FROM local_files WHERE present = 1",
                "SELECT path, root, size, mtime, metadata, error, present FROM local_files WHERE present = 1 ORDER BY path LIMIT ? OFFSET ?",
                false,
            ),
        };

        let total: usize = if has_param {
            let mut rows = conn
                .query(count_sql, (root.unwrap(),))
                .await
                .map_err(|e| e.to_string())?;
            rows.next()
                .await
                .map_err(|e| e.to_string())?
                .and_then(|r| r.get::<i64>(0).ok())
                .unwrap_or(0) as usize
        } else {
            let mut rows = conn.query(count_sql, ()).await.map_err(|e| e.to_string())?;
            rows.next()
                .await
                .map_err(|e| e.to_string())?
                .and_then(|r| r.get::<i64>(0).ok())
                .unwrap_or(0) as usize
        };

        let mut rows = if has_param {
            conn.query(select_sql, (root.unwrap(), limit as i64, offset as i64))
                .await
                .map_err(|e| e.to_string())?
        } else {
            conn.query(select_sql, (limit as i64, offset as i64))
                .await
                .map_err(|e| e.to_string())?
        };

        let mut records = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            let path: String = row.get(0).map_err(|e| e.to_string())?;
            let root_str: String = row.get(1).map_err(|e| e.to_string())?;
            let size: i64 = row.get(2).map_err(|e| e.to_string())?;
            let mtime: i64 = row.get(3).map_err(|e| e.to_string())?;
            let metadata_str: Option<String> = row.get(4).ok();
            let error: Option<String> = row.get(5).ok();
            let present: i64 = row.get(6).unwrap_or(1);

            let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
            records.push(LocalFileRecord {
                path,
                root: root_str,
                size,
                mtime,
                metadata,
                error,
                present: present != 0,
            });
        }

        Ok((records, total))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn get_link_rows(
        &self,
        market: &str,
        root: &str,
        filter: Option<&str>,
        search: Option<&str>,
        sort: Option<&str>,
        direction: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<TablePage<LinkRow>, String> {
        let revision = self.revision.load(std::sync::atomic::Ordering::SeqCst);
        let cache_key = format!("{market}:{}", root.trim_end_matches('/'));
        let cached = self
            .link_rows_cache
            .lock()
            .unwrap()
            .get(&cache_key)
            .filter(|(rev, _)| *rev == revision)
            .map(|(_, rows)| rows.clone());
        let rows = if let Some(rows) = cached {
            rows
        } else {
            let conn = self.connect()?;
            let active_links = self.get_active_links(&conn, market).await?;
            let clean = root.trim_end_matches('/');
            let with_slash = format!("{}/", clean);
            let mut stmt = conn
            .query(
                "SELECT f.path, f.metadata, tl.payload, CASE WHEN ig.path IS NOT NULL THEN 1 ELSE 0 END AS is_ignored
                 FROM local_files f
                 LEFT JOIN track_links tl ON tl.path = f.path AND tl.market = ?
                 LEFT JOIN ignored_local_files ig ON ig.path = f.path
                 WHERE (f.root = ? OR f.root = ?) AND f.present = 1 AND f.metadata IS NOT NULL",
                (market, clean, with_slash.as_str()),
            )
            .await
            .map_err(|e| e.to_string())?;

            let mut rows = Vec::new();
            while let Some(row) = stmt.next().await.map_err(|e| e.to_string())? {
                let path: String = row.get(0).map_err(|e| e.to_string())?;
                let meta_str: Option<String> = row.get(1).ok().flatten();
                let payload_str: Option<String> = row.get(2).ok().flatten();
                let is_ignored: i64 = row.get(3).unwrap_or(0);
                let ignored = is_ignored != 0;

                let meta: Value = meta_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null);

                let artist =
                    extract_album_artist(&meta).unwrap_or_else(|| "Unknown artist".to_string());
                let release = extract_tag_str(&meta, &["album", "release"])
                    .unwrap_or_else(|| "Unknown release".to_string());
                let title = extract_tag_str(&meta, &["title"]).unwrap_or_else(|| {
                    Path::new(&path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&path)
                        .to_string()
                });

                let disc =
                    extract_tag_num(&meta, &["disc_number", "discnumber", "disc"]).unwrap_or(1);
                let disc_total = extract_tag_num(&meta, &["disc_total", "disctotal", "totaldiscs"]);
                let track =
                    extract_tag_num(&meta, &["track_number", "tracknumber", "track"]).unwrap_or(1);
                let track_total =
                    extract_tag_num(&meta, &["track_total", "tracktotal", "totaltracks"]);

                let disc_total_str = disc_total
                    .map(|n| format!("{:02}", n))
                    .unwrap_or_else(|| "?".to_string());
                let track_total_str = track_total
                    .map(|n| format!("{:02}", n))
                    .unwrap_or_else(|| "?".to_string());
                let position = format!(
                    "Disc {:02}/{} · Track {:02}/{}",
                    disc, disc_total_str, track, track_total_str
                );

                let bpm = extract_tag_f64(&meta, &["bpm", "tempo"]);
                let key = extract_tag_str(&meta, &["initial_key", "initialkey", "key"]);

                let payload: Option<Value> = payload_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());

                let mut status = "Unlinked".to_string();
                let mut candidates = 0;
                let mut online_id = String::new();
                let mut evidence = String::new();

                if let Some(ref p) = payload {
                    if let Some(ids) = p.get("ids") {
                        if ids
                            .get("track_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .is_some()
                        {
                            status = "Linked".to_string();
                        }
                        if let Some(aid) = ids
                            .get("album_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                        {
                            online_id = aid.to_string();
                        }
                    }
                    if status != "Linked" {
                        if let Some(cc) = p.get("catalogue_choice") {
                            if cc
                                .get("track_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty())
                                .is_some()
                            {
                                status = "Linked".to_string();
                            }
                            if let Some(aid) = cc
                                .get("id")
                                .or_else(|| cc.get("album_id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty())
                            {
                                online_id = aid.to_string();
                            }
                        }
                    }
                    if let Some(opts) = p.get("catalogue_options").and_then(|v| v.as_array()) {
                        candidates = opts.len();
                        if status != "Linked" && candidates > 0 {
                            status = "Needs choice".to_string();
                        }
                    }
                    if let Some(note) = p
                        .get("catalogue_note")
                        .or_else(|| p.get("note"))
                        .and_then(|v| v.as_str())
                    {
                        evidence = Self::clean_evidence_str(note);
                    }
                }

                if status != "Linked" {
                    let aid = meta
                        .get("tidal_album_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty());
                    let tid = meta
                        .get("tidal_track_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty());
                    if let (Some(a), Some(_t)) = (aid, tid) {
                        status = "Linked".to_string();
                        if online_id.is_empty() {
                            online_id = a.to_string();
                        }
                        if evidence.is_empty() {
                            evidence = "Local tags contain TIDAL IDs".to_string();
                        }
                    }
                }

                if active_links.contains(&path) {
                    status = "Linked".into();
                } else if status == "Linked" {
                    status = if candidates > 0 {
                        "Needs choice"
                    } else {
                        "Unlinked"
                    }
                    .into();
                    evidence = "Local metadata changed; recheck the recording link".into();
                }
                rows.push(LinkRow {
                    id: path.clone(),
                    artist,
                    release,
                    title,
                    path: path.clone(),
                    position,
                    status,
                    evidence,
                    affected: false,
                    target: path,
                    changes: String::new(),
                    bpm,
                    key,
                    candidates,
                    online_id,
                    ignored,
                });
            }

            if self.revision.load(std::sync::atomic::Ordering::SeqCst) == revision {
                let mut cache = self.link_rows_cache.lock().unwrap();
                if cache.len() >= 4 {
                    cache.clear();
                }
                cache.insert(cache_key, (revision, rows.clone()));
            }
            rows
        };

        // Apply filter
        let filter_name = filter.unwrap_or("all");
        let mut filtered: Vec<LinkRow> = match filter_name {
            "unlinked" => rows
                .into_iter()
                .filter(|r| r.status != "Linked" && !r.ignored)
                .collect(),
            "choice" => rows
                .into_iter()
                .filter(|r| r.status != "Linked" && r.candidates > 0 && !r.ignored)
                .collect(),
            "linked" => rows.into_iter().filter(|r| r.status == "Linked").collect(),
            "ignored" => rows.into_iter().filter(|r| r.ignored).collect(),
            _ => rows,
        };

        // Apply search query
        if let Some(q) = search {
            let q_trimmed = q.trim().to_lowercase();
            if !q_trimmed.is_empty() {
                filtered.retain(|r| {
                    r.artist.to_lowercase().contains(&q_trimmed)
                        || r.release.to_lowercase().contains(&q_trimmed)
                        || r.title.to_lowercase().contains(&q_trimmed)
                        || r.path.to_lowercase().contains(&q_trimmed)
                        || r.evidence.to_lowercase().contains(&q_trimmed)
                        || r.online_id.to_lowercase().contains(&q_trimmed)
                });
            }
        }

        // Apply sort
        let sort_col = sort.unwrap_or("artist");
        let desc = direction == Some("desc");
        filtered.sort_by(|a, b| {
            let ord = match sort_col {
                "release" => a.release.to_lowercase().cmp(&b.release.to_lowercase()),
                "title" => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                "path" => a.path.to_lowercase().cmp(&b.path.to_lowercase()),
                "position" => a.position.cmp(&b.position),
                "status" => a.status.cmp(&b.status),
                "evidence" => a.evidence.to_lowercase().cmp(&b.evidence.to_lowercase()),
                "online_id" => a.online_id.cmp(&b.online_id),
                "candidates" => a.candidates.cmp(&b.candidates),
                "bpm" => {
                    let a_bpm = a.bpm.unwrap_or(0.0);
                    let b_bpm = b.bpm.unwrap_or(0.0);
                    a_bpm
                        .partial_cmp(&b_bpm)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }
                "key" => a.key.cmp(&b.key),
                _ => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
            };
            (if desc { ord.reverse() } else { ord }).then_with(|| a.id.cmp(&b.id))
        });

        let total = filtered.len();
        let page_rows = if offset < total {
            filtered.into_iter().skip(offset).take(limit).collect()
        } else {
            Vec::new()
        };

        Ok(TablePage {
            rows: page_rows,
            total,
            offset,
            revision: self.revision.load(std::sync::atomic::Ordering::SeqCst),
            preview_id: None,
        })
    }

    fn normalize_release_title(title: &str) -> String {
        let lower = title.trim().to_lowercase();
        let markers = [
            "(deluxe",
            "[deluxe",
            "(clean",
            "[clean",
            "(explicit",
            "[explicit",
            "(bonus",
            "[bonus",
            "(remaster",
            "[remaster",
            "(expanded",
            "[expanded",
            "(anniversary",
            "[anniversary",
            "(special",
            "[special",
            "(super deluxe",
            "[super deluxe",
            "(re-issue",
            "[re-issue",
            "(reissue",
            "[reissue",
        ];
        let mut cleaned = lower.as_str();
        for m in &markers {
            if let Some(pos) = cleaned.find(m) {
                cleaned = cleaned[..pos].trim();
            }
        }
        crate::matching::title_key(cleaned.trim_end_matches(['-', ':', ' ', '·']).trim())
    }

    fn coverage_priority(status: &str) -> i32 {
        match status {
            "Owned complete" => 5,
            "Owned alternate edition" => 5,
            "Queued" => 4,
            "Owned partial" => 3,
            "Missing release" => 2,
            "Unavailable" => 1,
            "Ignored" => 0,
            _ => 0,
        }
    }

    fn recommendation_priority(badge: &str) -> i32 {
        match badge {
            "Recommended" => 3,
            "Potential" => 2,
            "Suspect" => 1,
            _ => 0,
        }
    }

    fn audio_quality_priority(q: &str) -> i32 {
        let lower = q.to_lowercase();
        if lower.contains("hires") {
            4
        } else if lower.contains("lossless") && !lower.contains("dolby") && !lower.contains("360") {
            3
        } else if lower.contains("dolby") || lower.contains("atmos") || lower.contains("360") {
            2
        } else if lower.contains("high") {
            1
        } else {
            0
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_missing_rows(&self, market: &str) -> Result<Vec<MissingRow>, String> {
        let conn = self.connect()?;
        #[derive(Default)]
        struct LocalEdition {
            year: String,
            date: String,
            positions: HashSet<String>,
            totals_by_disc: HashMap<u32, usize>,
            discs: u32,
            upc: Option<String>,
        }
        impl LocalEdition {
            fn complete(&self) -> bool {
                self.discs > 0 && self.positions.len() == self.totals_by_disc.values().sum::<usize>() && (1..=self.discs).all(|disc| self.totals_by_disc.get(&disc).is_some_and(|total| *total > 0 && (1..=*total).all(|track| self.positions.contains(&format!("{disc}:{track}")))))
            }
        }
        let mut local_by_folder: HashMap<(String, String, String), LocalEdition> = HashMap::new();
        let mut local_dates: HashMap<String, Vec<String>> = HashMap::new();
        let mut local_isrcs: HashMap<String, HashSet<String>> = HashMap::new();
        let mut local_labels: HashMap<String, HashSet<String>> = HashMap::new();
        let mut local_genres: HashMap<String, HashSet<String>> = HashMap::new();
        let mut local_rights: HashMap<String, HashSet<String>> = HashMap::new();
        let mut local_contributors: HashMap<String, HashSet<String>> = HashMap::new();
        let mut local_files = conn
            .query(
                "SELECT path, metadata FROM local_files WHERE present = 1 AND metadata IS NOT NULL",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;
        while let Some(row) = local_files.next().await.map_err(|e| e.to_string())? {
            let path: String = row.get(0).unwrap_or_default();
            let metadata: String = row.get(1).unwrap_or_default();
            let Some(meta) = serde_json::from_str::<Value>(&metadata).ok() else {
                continue;
            };
            let Some(artist) = extract_album_artist(&meta) else {
                continue;
            };
            if is_compilation_artist(&artist) {
                continue;
            }
            let artist_key = crate::matching::name_key(&artist);
            let Some(album) = extract_tag_str(&meta, &["album", "release"]) else {
                continue;
            };
            let title_key = Self::normalize_release_title(&album);
            if title_key.is_empty() {
                continue;
            }
            let date = extract_tag_str(&meta, &["date", "release_date", "releasedate", "year"])
                .unwrap_or_default();
            if let Some(isrc) = extract_tag_str(&meta, &["isrc"]) {
                local_isrcs
                    .entry(artist_key.clone())
                    .or_default()
                    .insert(isrc.trim().to_uppercase());
            }
            if let Some(genres) = extract_tag_str(&meta, &["genre"]) {
                local_genres.entry(artist_key.clone()).or_default().extend(genres.split(';').map(crate::matching::name_key).filter(|s| !s.is_empty()));
            }
            if let Some(label) = extract_tag_str(&meta, &["label", "record_label", "recordlabel"]) {
                local_labels
                    .entry(artist_key.clone())
                    .or_default()
                    .insert(crate::matching::name_key(&label));
            }
            if let Some(rights) = extract_tag_str(&meta, &["copyright"]) {
                local_rights
                    .entry(artist_key.clone())
                    .or_default()
                    .insert(crate::matching::name_key(&rights));
            }
            local_contributors
                .entry(artist_key.clone())
                .or_default()
                .extend(crate::recommendations::local_credit_names(&meta));
            let folder = crate::duplicates::extract_release_folder(&path);
            let edition = local_by_folder
                .entry((artist_key, title_key, folder))
                .or_default();
            if edition.year.is_empty() {
                edition.year = date.chars().take(4).collect();
            }
            if date.len() > edition.date.len() {
                edition.date = date;
            }
            let disc = extract_tag_num(&meta, &["disc_number", "discnumber", "disc"]).unwrap_or(1);
            let track =
                extract_tag_num(&meta, &["track_number", "tracknumber", "track"]).unwrap_or(0);
            edition.positions.insert(if track > 0 {
                format!("{disc}:{track}")
            } else {
                path
            });
            let total = extract_tag_num(&meta, &["track_total", "tracktotal", "totaltracks"]).unwrap_or(0) as usize;
            edition.totals_by_disc.entry(disc).and_modify(|n| *n = (*n).max(total)).or_insert(total);
            edition.discs = edition.discs.max(extract_tag_num(&meta, &["disc_total", "disctotal", "totaldiscs"]).unwrap_or(disc).max(disc));
            if edition.upc.is_none() {
                edition.upc = extract_tag_str(&meta, &["upc", "barcode"]);
            }
        }
        drop(local_files);
        let mut local_editions: HashMap<(String, String), Vec<LocalEdition>> = HashMap::new();
        let mut complete_local_upcs: HashSet<String> = HashSet::new();
        for ((artist, title, _), edition) in local_by_folder {
            if !edition.date.is_empty() {
                local_dates
                    .entry(artist.clone())
                    .or_default()
                    .push(edition.date.clone());
            }
            if edition.complete() {
                if let Some(upc) = &edition.upc {
                    complete_local_upcs.insert(upc.clone());
                }
            }
            local_editions
                .entry((artist, title))
                .or_default()
                .push(edition);
        }
        for dates in local_dates.values_mut() {
            dates.sort();
            dates.dedup();
            dates.reverse();
        }
        let general = self.get_settings().await?["general"].clone();
        let treat_editions_as_owned = general["treat_editions_as_owned"].as_bool().unwrap_or(true);
        let recommend_live = general["recommend_live"].as_bool().unwrap_or(false);
        let recommend_bootlegs = general["recommend_bootlegs"].as_bool().unwrap_or(false);
        let recommend_compilations = general["recommend_compilations"].as_bool().unwrap_or(false);

        // 1. Load queue decisions
        let mut queue_stmt = conn
            .query("SELECT id, decision, approved FROM queue", ())
            .await
            .map_err(|e| e.to_string())?;
        let mut queue_decisions: HashMap<String, (String, bool)> = HashMap::new();
        while let Some(row) = queue_stmt.next().await.map_err(|e| e.to_string())? {
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let decision: String = row.get(1).unwrap_or_else(|_| "queued".to_string());
            let approved: i64 = row.get(2).unwrap_or(0);
            queue_decisions.insert(id, (decision, approved != 0));
        }

        // 2. Load linked album IDs from track_links
        let mut links_stmt = conn
            .query(
                "SELECT l.payload,f.metadata,l.stamp,f.size,f.mtime,f.present FROM track_links l LEFT JOIN local_files f ON f.path=l.path WHERE l.market=?",
                (market,),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut linked_credit_tracks: HashMap<String, HashSet<String>> = HashMap::new();
        let mut linked_album_tracks: HashMap<String, HashSet<String>> = HashMap::new();
        while let Some(row) = links_stmt.next().await.map_err(|e| e.to_string())? {
            if let Ok(Some(payload_str)) = row.get::<Option<String>>(0) {
                if let Ok(payload) = serde_json::from_str::<Value>(&payload_str) {
                    let metadata: String = row.get(1).unwrap_or_default();
                    let stamp: String = row.get(2).unwrap_or_default();
                    let size: i64 = row.get(3).unwrap_or_default();
                    let mtime: i64 = row.get(4).unwrap_or_default();
                    let current = row.get::<i64>(5).unwrap_or(0) == 1
                        && link_stamp_matches(&stamp, size, mtime);
                    if current && (payload["status"] == "linked" || payload["status"].is_null()) {
                        if let Some(artist) = serde_json::from_str::<Value>(&metadata)
                            .ok()
                            .and_then(|m| extract_album_artist(&m))
                        {
                            let key = crate::matching::name_key(&artist);
                            let mut sources = vec![&payload["ids"]];
                            sources.extend(payload["placements"].as_array().into_iter().flatten());
                            for source in sources {
                                if let Some(id) = source["track_id"].as_str() {
                                    linked_credit_tracks
                                        .entry(id.to_owned())
                                        .or_default()
                                        .insert(key.clone());
                                }
                            }
                        }
                    }

                    if current && (payload["status"] == "linked" || payload["status"].is_null()) {
                        for source in std::iter::once(&payload["ids"]).chain(payload["placements"].as_array().into_iter().flatten()) {
                            if let (Some(album),Some(track)) = (source["album_id"].as_str(),source["track_id"].as_str()) {
                                linked_album_tracks.entry(album.to_owned()).or_default().insert(track.to_owned());
                            }
                        }
                    }
                }
            }
        }

        // Exploratory search caches are not subscriptions to an artist's releases.
        let linked_artists: HashSet<String> = self.get_linked_artist_ids().await?.into_iter().collect();
        // 3. Load catalogue
        let mut cat_stmt = conn
            .query(
                "SELECT artist_id, payload FROM catalogue WHERE market = ? ORDER BY artist_id",
                (market,),
            )
            .await
            .map_err(|e| e.to_string())?;

        let today_utc = chrono::Utc::now().format("%Y-%m-%d").to_string();

        struct MergedRelease {
            id: String,
            title: String,
            group_key: String,
            artists: Vec<String>,
            date: String,
            rel_type: String,
            track_count: usize,
            quality: String,
            status: String,
            available: Option<bool>,
            approved: bool,
            expanded_available: bool,
            children: Vec<MissingChildTrack>,
            recommendation: String,
            evidence: Vec<String>,
        }

        let mut by_id: HashMap<String, MergedRelease> = HashMap::new();
        let mut full_releases: HashMap<String, crate::tidal::TidalRelease> = HashMap::new();

        while let Some(row) = cat_stmt.next().await.map_err(|e| e.to_string())? {
            let artist_id: String = row.get(0).map_err(|e|e.to_string())?;
            if !linked_artists.contains(&artist_id) { continue; }

            let payload_str: Option<String> = row.get(1).ok().flatten();
            let payload = match payload_str
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
            {
                Some(p) => p,
                None => continue,
            };

            let artist_name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let releases = match payload.get("releases").and_then(|v| v.as_array()) {
                Some(r) => r,
                None => continue,
            };

            // Use remote credits only when anchored to a currently verified local
            // recording. Catalogue membership or a recommendation is never an anchor.
            for track in releases
                .iter()
                .flat_map(|release| release["tracks"].as_array().into_iter().flatten())
            {
                if let Some(artists) = track["id"]
                    .as_str()
                    .and_then(|id| linked_credit_tracks.get(id))
                {
                    let names = crate::recommendations::credit_names(&track["credits"]);
                    for artist in artists {
                        local_genres.entry(artist.clone()).or_default().extend(track["genres"].as_array().into_iter().flatten().filter_map(Value::as_str).map(crate::matching::name_key));
                        local_contributors
                            .entry(artist.clone())
                            .or_default()
                            .extend(names.iter().filter(|name| *name != artist).cloned());
                    }
                }
            }

            for rel in releases {
                if let Ok(parsed) = serde_json::from_value::<crate::tidal::TidalRelease>(rel.clone()) { full_releases.entry(parsed.id.clone()).or_insert(parsed); }
                let id = rel
                    .get("id")
                    .map(|v| v.to_string().trim_matches('"').to_string())
                    .unwrap_or_default();
                if id.is_empty() {
                    continue;
                }

                let date = rel
                    .get("date")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();

                // Exclude future unreleased and prerelease items
                let is_prerelease = rel
                    .get("is_prerelease")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if is_prerelease || date.is_empty() {
                    continue;
                }
                if date.len() >= 10 && date[..10] > today_utc[..10] {
                    continue;
                }
                if date.len() == 7 && date[..7] > today_utc[..7] {
                    continue;
                }
                if date.len() == 4 && date[..4] > today_utc[..4] {
                    continue;
                }

                let title = rel
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let rel_artist = rel
                    .get("artist")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&artist_name)
                    .trim()
                    .to_string();

                let rel_type = rel
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ALBUM")
                    .to_string();
                let track_count =
                    rel.get("track_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let quality = rel
                    .get("quality")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let available = rel.get("available").and_then(|v| v.as_bool());
                let tracks_loaded = rel
                    .get("tracks_loaded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut children = Vec::new();
                if let Some(tracks) = rel.get("tracks").and_then(|v| v.as_array()) {
                    for t in tracks {
                        let tid = t
                            .get("id")
                            .map(|v| v.to_string().trim_matches('"').to_string())
                            .unwrap_or_default();
                        let ttitle = t
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let disc = t.get("disc_number").and_then(|v| v.as_u64()).unwrap_or(1);
                        let track_num = t
                            .get("track_number")
                            .map(|v| v.to_string().trim_matches('"').to_string())
                            .unwrap_or_default();
                        let pos = format!("{} · {}", disc, track_num);
                        let dur = t.get("duration").and_then(|v| v.as_f64());
                        let isrc = t
                            .get("isrc")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let bpm = t.get("bpm").and_then(|v| v.as_f64());
                        let key = t.get("key").and_then(|v| v.as_str()).map(|s| s.to_string());

                        children.push(MissingChildTrack {
                            id: tid,
                            title: ttitle,
                            position: pos,
                            duration: dur,
                            isrc,
                            bpm,
                            key,
                        });
                    }
                }

                // Determine coverage status
                let artist_key = crate::matching::name_key(&rel_artist);
                let title_key = Self::normalize_release_title(&title);
                let year = date.chars().take(4).collect::<String>().parse::<i32>().ok();
                let owned_edition = local_editions
                    .get(&(artist_key.clone(), title_key.clone()))
                    .and_then(|editions| {
                        editions
                            .iter()
                            .filter(|edition| {
                                year.zip(edition.year.parse::<i32>().ok())
                                    .is_none_or(|(remote, local)| (remote - local).abs() <= 3)
                            })
                            .max_by_key(|edition| edition.positions.len())
                    });
                let (status, approved) = if let Some((decision, app)) = queue_decisions.get(&id) {
                    let st = match decision.as_str() {
                        "ignored" => "Ignored",
                        "downloaded" => "Owned complete",
                        _ => "Queued",
                    };
                    (st.to_string(), *app)
                } else if available == Some(false) {
                    ("Unavailable".to_string(), false)
                } else if rel
                    .get("upc")
                    .and_then(Value::as_str)
                    .is_some_and(|upc| complete_local_upcs.contains(upc))
                {
                    ("Owned complete".to_string(), false)
                } else if let Some(linked) = linked_album_tracks.get(&id) {
                    let linked_count = linked.len();
                    if linked_count >= track_count && track_count > 0 {
                        ("Owned complete".to_string(), false)
                    } else if linked_count > 0 {
                        ("Owned partial".to_string(), false)
                    } else {
                        ("Missing release".to_string(), false)
                    }
                } else if let Some(edition) = owned_edition {
                    let owned = edition.positions.len();
                    let complete_local =
                        edition.complete();
                    if complete_local && track_count > 0 && owned >= track_count {
                        ("Owned complete".to_string(), false)
                    } else if treat_editions_as_owned && complete_local && owned > 0 {
                        ("Owned alternate edition".to_string(), false)
                    } else {
                        ("Owned partial".to_string(), false)
                    }
                } else {
                    ("Missing release".to_string(), false)
                };

                // Determine actual recommendation badge and evidence
                let is_compilation = rel_type == "COMPILATION"
                    || rel
                        .get("is_compilation")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                let is_bootleg = rel.get("official").and_then(Value::as_bool) == Some(false)
                    || rel["secondary_types"].as_array().is_some_and(|types| {
                        types.iter().any(|value| {
                            value.as_str().is_some_and(|name| {
                                ["bootleg", "promo", "unofficial"]
                                    .iter()
                                    .any(|flag| name.eq_ignore_ascii_case(flag))
                            })
                        })
                    })
                    || title.to_lowercase().contains("bootleg");
                let is_unofficial = is_bootleg && !recommend_bootlegs;
                let is_official = rel
                    .get("official")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let primary_verified = rel
                    .get("primary_artist_verified")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let primary = primary_verified;
                let conflict = rel["artist_credits"]
                    .as_array()
                    .and_then(|credits| credits.first())
                    .and_then(Value::as_str)
                    .is_some_and(|first| {
                        crate::matching::name_key(first) != crate::matching::name_key(&artist_name)
                    });
                let label = rel
                    .get("label")
                    .and_then(Value::as_str)
                    .map(crate::matching::name_key)
                    .unwrap_or_default();
                let rights = rel
                    .get("copyright")
                    .and_then(Value::as_str)
                    .or_else(|| rel["copyright"]["text"].as_str())
                    .map(crate::matching::name_key)
                    .unwrap_or_default();
                let label_match = !label.is_empty()
                    && local_labels
                        .get(&artist_key)
                        .is_some_and(|values| values.contains(&label));
                let rights_match = !rights.is_empty()
                    && local_rights
                        .get(&artist_key)
                        .is_some_and(|values| values.contains(&rights));
                let recordings = children
                    .iter()
                    .filter(|track| {
                        !track.isrc.is_empty()
                            && local_isrcs
                                .get(&artist_key)
                                .is_some_and(|values| values.contains(&track.isrc.to_uppercase()))
                    })
                    .count();
                let (contributor_tracks, contributors) =
                    crate::recommendations::shared_contributor_evidence(
                        local_contributors.get(&artist_key),
                        rel["tracks"].as_array(),
                    );
                let (_score, mut badge, mut reasons) = crate::recommendations::recommendation_score(
                    primary,
                    conflict,
                    true,
                    label_match,
                    label_match,
                    rights_match,
                    recordings,
                    contributor_tracks,
                    contributors,
                    is_compilation && !recommend_compilations,
                    is_unofficial,
                    is_official,
                );
                let shared_genres: Vec<_> = rel["genres"].as_array().into_iter().flatten().filter_map(Value::as_str)
                    .filter(|g| local_genres.get(&artist_key).is_some_and(|set| set.contains(&crate::matching::name_key(g)))).collect();
                if !shared_genres.is_empty() { reasons.push(format!("Shared genres: {} (supporting context, not identity proof)",shared_genres.join(", "))); }
                if let Some(replacement) = rel["replacement_id"].as_str() {
                    reasons.push(format!("Provider replacement release: {replacement}; inspect its recordings before changing links"));
                }
                let is_live = rel["secondary_types"].as_array().is_some_and(|types| {
                    types.iter().any(|value| {
                        value
                            .as_str()
                            .is_some_and(|name| name.eq_ignore_ascii_case("live"))
                    })
                }) || title.to_lowercase().contains("(live")
                    || title.to_lowercase().contains(" live at ");
                if is_live && !recommend_live {
                    badge = "Suspect".into();
                    reasons.push(
                        "Live release; show live items in recommendations to consider it".into(),
                    );
                }
                if is_bootleg && !recommend_bootlegs {
                    reasons.push(
                        "Bootleg or unofficial release; still visible for manual review".into(),
                    );
                }
                if is_compilation && !recommend_compilations {
                    reasons.push("Compilation; still visible for manual review".into());
                }
                if status == "Owned alternate edition" {
                    reasons.push("A complete local edition with the same album artist and release title is already indexed".into());
                }

                let norm_title = Self::normalize_release_title(&title);
                let group_key = rel
                    .get("release_group_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(|id| format!("group:{id}"))
                    .unwrap_or_else(|| norm_title.clone());

                match by_id.entry(id.clone()) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(MergedRelease {
                            id,
                            title,
                            group_key,
                            artists: if rel_artist.is_empty() {
                                Vec::new()
                            } else {
                                vec![rel_artist]
                            },
                            date,
                            rel_type,
                            track_count,
                            quality,
                            status,
                            available,
                            approved,
                            expanded_available: tracks_loaded,
                            children,
                            recommendation: badge,
                            evidence: reasons,
                        });
                    }
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        let existing = e.get_mut();
                        if !rel_artist.is_empty() && !existing.artists.contains(&rel_artist) {
                            existing.artists.push(rel_artist);
                        }
                        if !existing.expanded_available && tracks_loaded {
                            existing.expanded_available = true;
                            existing.children = children;
                        }
                        let existing_q = Self::audio_quality_priority(&existing.quality);
                        let new_q = Self::audio_quality_priority(&quality);
                        if new_q > existing_q {
                            existing.quality = quality;
                        }
                        if Self::coverage_priority(&status)
                            > Self::coverage_priority(&existing.status)
                        {
                            existing.status = status;
                            existing.approved = approved;
                        }
                        if Self::recommendation_priority(&badge)
                            > Self::recommendation_priority(&existing.recommendation)
                        {
                            existing.recommendation = badge;
                            existing.evidence = reasons;
                        }
                    }
                }
            }
        }

        // Secondary deduplication: merge alternate audio edition releases (Dolby Atmos vs Lossless vs HiRes)
        // that share the same primary artist, normalized title, release type, and track count.
        let mut deduped_releases: HashMap<(String, String, usize, String), MergedRelease> =
            HashMap::new();
        let mut candidates: Vec<MergedRelease> = by_id.into_values().collect();
        candidates.sort_by(|left, right| left.id.cmp(&right.id));
        for rel in candidates {
            let artist_key = rel
                .artists
                .first()
                .map(|a| a.to_lowercase())
                .unwrap_or_default();
            let key = (
                artist_key,
                rel.group_key.clone(),
                rel.track_count,
                rel.rel_type.clone(),
            );
            match deduped_releases.entry(key) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(rel);
                }
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    let existing = e.get_mut();
                    let existing_pri = Self::coverage_priority(&existing.status);
                    let cand_pri = Self::coverage_priority(&rel.status);
                    let existing_q = Self::audio_quality_priority(&existing.quality);
                    let cand_q = Self::audio_quality_priority(&rel.quality);
                    let existing_rec = Self::recommendation_priority(&existing.recommendation);
                    let cand_rec = Self::recommendation_priority(&rel.recommendation);

                    for art in &rel.artists {
                        if !existing.artists.contains(art) {
                            existing.artists.push(art.clone());
                        }
                    }

                    let old_replaced = full_releases.get(&existing.id).and_then(|r| r.replacement_id.as_ref()).is_some_and(|id| id == &rel.id);
                    let should_replace = (old_replaced && rel.available == Some(true) && cand_pri >= existing_pri) || cand_pri > existing_pri
                        || (cand_pri == existing_pri && cand_rec > existing_rec)
                        || (cand_pri == existing_pri
                            && cand_rec == existing_rec
                            && cand_q > existing_q)
                        || (cand_pri == existing_pri
                            && cand_rec == existing_rec
                            && cand_q == existing_q
                            && (!existing.expanded_available && rel.expanded_available));

                    if should_replace {
                        let mut merged_artists = existing.artists.clone();
                        for art in &rel.artists {
                            if !merged_artists.contains(art) {
                                merged_artists.push(art.clone());
                            }
                        }
                        let mut chosen = rel;
                        chosen.artists = merged_artists;
                        *existing = chosen;
                    }
                }
            }
        }

        let mut rows: Vec<MissingRow> = deduped_releases
            .into_values()
            .map(|rel| {
                let display_artist = match rel.artists.len() {
                    0 => "Unknown artist".to_string(),
                    1 => rel.artists[0].clone(),
                    2 => format!("{} & {}", rel.artists[0], rel.artists[1]),
                    _ => rel.artists.join(", "),
                };
                let dates = rel
                    .artists
                    .first()
                    .and_then(|artist| local_dates.get(&crate::matching::name_key(artist)));
                MissingRow {
                    id: rel.id.clone(),
                    downloaded_files: Vec::new(),
                    artist: display_artist,
                    release: rel.title,
                    date: rel.date,
                    r#type: rel.rel_type,
                    tracks: rel.track_count,
                    status: rel.status,
                    online_id: rel.id,
                    recommendation: rel.recommendation,
                    evidence: rel.evidence,
                    expanded_available: rel.expanded_available,
                    available: rel.available,
                    approved: rel.approved,
                    selected: if rel.approved { None } else { Some(Vec::new()) },
                    children: rel.children,
                    newest_local_date: dates.and_then(|dates| dates.first().cloned()),
                    previous_local_date: dates.and_then(|dates| dates.get(1).cloned()),
                }
            })
            .collect();

        let catalogue: Vec<_> = full_releases.values().cloned().collect();
        let superseded = crate::recommendations::subsumed_releases(&catalogue);
        let usable: HashSet<_> = rows.iter().filter(|r| r.recommendation == "Recommended" || r.recommendation == "Potential").map(|r| r.id.clone()).collect();
        for row in &mut rows {
            let replacement = superseded.get(&row.id).or_else(|| full_releases.get(&row.id).and_then(|r| r.replacement_id.as_ref()));
            if let Some(id) = replacement.filter(|id| usable.contains(*id)) {
                if let Some(target) = full_releases.get(id) {
                    if superseded.get(&row.id) == Some(id) {
                        row.recommendation = "Superseded".into();
                        row.evidence.push(format!("All recordings are contained in {} · release {}. The older release remains available for manual selection.",target.title,id));
                    } else {
                        row.evidence.push(format!("Provider replacement available: {} · release {}",target.title,id));
                    }
                }
            }
        }
        Ok(rows)
    }

    pub async fn get_missing_rows(
        &self,
        market: &str,
        timeline: Option<&str>,
        recommendation: Option<&str>,
        status_filter: Option<&str>,
        type_filter: Option<&str>,
        search: Option<&str>,
        sort: Option<&str>,
        direction: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<TablePage<MissingRow>, String> {
        let build_guard = self.missing_rows_gate.lock().await;
        let revision = self.revision.load(std::sync::atomic::Ordering::SeqCst);
        let key = format!("{market}:{}", chrono::Utc::now().format("%Y-%m-%d"));
        let cached = self
            .missing_rows_cache
            .lock()
            .unwrap()
            .get(&key)
            .filter(|(rev, _)| *rev == revision)
            .map(|(_, rows)| rows.clone());
        let mut rows = if let Some(rows) = cached {
            rows
        } else {
            let rows = self.build_missing_rows(market).await?;
            let mut cache = self.missing_rows_cache.lock().unwrap();
            cache.clear();
            cache.insert(key, (revision, rows.clone()));
            rows
        };

        drop(build_guard);

        // Apply filters
        if let Some(sf) = status_filter {
            if sf != "All statuses"
                && sf != "all"
                && [
                    "Missing release",
                    "Owned partial",
                    "Owned complete",
                    "Owned alternate edition",
                    "Queued",
                    "Ignored",
                    "Unavailable",
                ]
                .contains(&sf)
            {
                rows.retain(|r| r.status == sf);
            }
        }

        if let Some(tf) = type_filter {
            if tf != "All types" {
                rows.retain(|r| r.r#type == tf);
            }
        }

        if let Some(rf) = recommendation {
            let rf_clean = rf.trim();
            if rf_clean != "All recommendations" && rf_clean != "all" && !rf_clean.is_empty() {
                rows.retain(|r| {
                    if rf_clean == "Suspect / Low match" {
                        r.recommendation == "Suspect"
                    } else {
                        r.recommendation == rf_clean
                    }
                });
            }
        }

        if let Some(tl) = timeline {
            if tl == "Newer than newest owned" {
                rows.retain(|r| {
                    ["Missing release", "Owned partial", "Queued"].contains(&r.status.as_str())
                        && r.newest_local_date
                            .as_deref()
                            .is_none_or(|newest| compare_release_dates(&r.date, newest).is_gt())
                });
            } else if tl == "Between newest two owned" {
                rows.retain(|r| {
                    ["Missing release", "Owned partial", "Queued"].contains(&r.status.as_str())
                        && r.newest_local_date
                            .as_deref()
                            .is_some_and(|newest| !compare_release_dates(&r.date, newest).is_gt())
                        && r.previous_local_date.as_deref().is_some_and(|previous| {
                            compare_release_dates(&r.date, previous).is_gt()
                        })
                });
            } else if tl == "All missing releases" {
                rows.retain(|r| {
                    r.status == "Missing release"
                        || r.status == "Owned partial"
                        || r.status == "Queued"
                });
            } else if tl == "Incomplete albums" {
                rows.retain(|r| {
                    (r.r#type == "ALBUM" || r.r#type == "EP")
                        && (r.status == "Owned partial" || r.status == "Present locally")
                });
            }
        }

        if let Some(q) = search {
            let q_trimmed = q.trim().to_lowercase();
            if !q_trimmed.is_empty() {
                rows.retain(|r| {
                    r.artist.to_lowercase().contains(&q_trimmed)
                        || r.release.to_lowercase().contains(&q_trimmed)
                });
            }
        }

        // Sort by date desc (or requested sort)
        let sort_col = sort.unwrap_or("date");
        let desc = direction.map(|d| d == "desc").unwrap_or(sort_col == "date");
        if limit > 0 { rows.sort_by(|a, b| {
            let ord = match sort_col {
                "artist" => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
                "release" => a.release.to_lowercase().cmp(&b.release.to_lowercase()),
                "type" => a.r#type.cmp(&b.r#type),
                "tracks" => a.tracks.cmp(&b.tracks),
                "status" => a.status.cmp(&b.status),
                "recommendation" => a.recommendation.cmp(&b.recommendation),
                _ => a.date.cmp(&b.date),
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }.then_with(|| a.artist.cmp(&b.artist)).then_with(|| a.release.cmp(&b.release)).then_with(|| a.id.cmp(&b.id))
        }); }

        let total = rows.len();
        let page_rows = if offset < total {
            rows.into_iter().skip(offset).take(limit).collect()
        } else {
            Vec::new()
        };

        Ok(TablePage {
            rows: page_rows,
            total,
            offset,
            revision: self.revision.load(std::sync::atomic::Ordering::SeqCst),
            preview_id: None,
        })
    }

    pub async fn get_state(
        &self,
        active_job: Option<Value>,
        logs: &[Value],
        root: Option<&str>,
    ) -> Result<Value, String> {
        let settings = self.get_settings().await?;
        let market = settings["general"]
            .get("market")
            .and_then(|v| v.as_str())
            .unwrap_or("GB");

        let roots = self.list_roots(market).await.unwrap_or_default();
        let stats_record = self.get_stats(market, root).await.unwrap_or_default();

        let mut stats_val = serde_json::to_value(&stats_record).unwrap_or(json!({}));
        if let Some(obj) = stats_val.as_object_mut() {
            obj.insert("files".to_string(), json!(stats_record.track_count));
            obj.insert("linked".to_string(), json!(stats_record.linked_tracks));
            obj.insert("missing".to_string(), json!(stats_record.missing_releases));
            obj.insert(
                "missing_releases".to_string(),
                json!(stats_record.missing_releases),
            );
            obj.insert("correct".to_string(), json!(stats_record.correct));
        }

        // Include desktop-health maintenance counts (correct, organise, metadata, artwork, mqa, local, online)
        if let Ok(Some(health_pref)) = self.get_preference("desktop-health").await {
            if let Some(counts) = health_pref.get("counts").and_then(|v| v.as_object()) {
                if let Some(obj) = stats_val.as_object_mut() {
                    for (k, v) in counts {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        let dev_client = crate::tidal::TidalClient::from_env_or_keychain();
        let stream_tok = crate::stream_download::load_saved_token(self).await;
        let is_connected = stream_tok.is_some();
        let user_id = stream_tok.as_ref().and_then(|s| s.user_id.clone());
        let expires_at = stream_tok.as_ref().map(|s| s.expires_at);

        let conn_state = json!({
            "configured": dev_client.is_some(),
            "account": is_connected,
            "checked": false,
            "tidal": {
                "connected": is_connected,
                "user_id": user_id,
                "expires_at": expires_at,
            }
        });

        let default_job = self
            .get_preference("desktop-last-job")
            .await?
            .filter(|j| {
                let kind = j.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                let msg = j.get("message").and_then(|m| m.as_str()).unwrap_or("");
                kind != "component_check" && !msg.contains("streaming components")
            })
            .unwrap_or_else(|| {
                json!({
                    "id": "startup",
                    "kind": "startup",
                    "status": "complete",
                    "message": "Ready"
                })
            });
        let mut default_job = default_job;
        default_job["historical"] = json!(true);
        if default_job["status"] == "running" || default_job["status"] == "cancelling" {
            default_job["status"] = json!("interrupted");
            default_job["message"] =
                json!("Previous job was interrupted. Completed results retained.");
        }
        let reported_job = active_job.unwrap_or(default_job);

        Ok(json!({
            "revision": self.revision.load(std::sync::atomic::Ordering::SeqCst),
            "roots": roots,
            "stats": stats_val,
            "job": reported_job,
            "logs": logs,
            "settings": settings["general"],
            "connections": conn_state,
            "diagnostics": self.get_preference("connection-diagnostics").await?.unwrap_or(json!({"metrics":{}})),
            "demo": false,
            "auth_url": Value::Null,
        }))
    }

    pub async fn get_settings(&self) -> Result<Value, String> {
        let mut general = self
            .get_preference("desktop")
            .await?
            .or(self.get_preference("ui").await?)
            .unwrap_or_else(|| {
                json!({
                    "market": "GB",
                    "theme": "dark",
                    "persist_logs": true
                })
            });
        if general.get("persist_logs").is_none() {
            if let Some(obj) = general.as_object_mut() {
                obj.insert("persist_logs".to_string(), json!(true));
            }
        }
        let mut provider_defaults = json!({
            "request_interval_ms": 750,
            "request_timeout_sec": 20,
            "token_refresh_margin_sec": 30,
            "sign_in_timeout_sec": 180,
            "request_attempts": 2,
            "download_concurrency": 2,
            "segment_concurrency": 2,
            "download_delay": true,
            "download_delay_min_sec": 3.0,
            "download_delay_max_sec": 5.0,
            "api_batch_size": 20,
            "api_batch_delay_sec": 3.0,
            "aac_bitrate_cap": 320,
            "api_url": "https://openapi.tidal.com/v2",
            "auth_url": "https://auth.tidal.com/v1/oauth2/token"
        });
        let provider = if let Some(saved) = self.get_preference("provider").await? {
            if let Some(obj) = saved.as_object() {
                if let Some(def_obj) = provider_defaults.as_object_mut() {
                    for (k, v) in obj {
                        def_obj.insert(k.clone(), v.clone());
                    }
                }
            }
            provider_defaults
        } else {
            provider_defaults
        };
        let matching = self.get_preference("matching").await?.unwrap_or_else(|| {
            json!({
                "enabled": true,
                "threshold": 75,
                "margin": 25
            })
        });
        let links = self
            .get_preference("release_links")
            .await?
            .unwrap_or_else(|| {
                json!({
                    "max_age_days": 30
                })
            });
        let home_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let default_downloads = json!({
            "output": home_dir.join("Downloads/Music").to_string_lossy().to_string(),
            "quality": "LOSSLESS",
            "cover_size": 1280,
            "skip_existing": true,
            "cover_album_file": true,
            "lyrics_embed": false,
            "lyrics_file": false,
            "playlist_create": false,
            "replay_gain": true,
        });
        let downloads = if let Some(mut saved) = self.get_preference("downloads").await? {
            if let Some(obj) = saved.as_object_mut() {
                if let Some(def_obj) = default_downloads.as_object() {
                    for (k, v) in def_obj {
                        obj.entry(k).or_insert_with(|| v.clone());
                    }
                }
            }
            saved
        } else {
            default_downloads
        };
        let organisation = self
            .get_preference("organisation")
            .await?
            .unwrap_or_else(|| {
                json!({
                    "template": "{albumartist}/{album} ({year})/{disc}/{tracknumber} - {title}"
                })
            });

        let dev_client = crate::tidal::TidalClient::from_env_or_keychain();
        let stream_tok = crate::stream_download::load_saved_token(self).await;
        let is_connected = stream_tok.is_some();
        let connections = json!({
            "configured": dev_client.is_some(),
            "account": is_connected,
            "checked": false
        });

        Ok(json!({
            "general": general,
            "provider": provider,
            "matching": matching,
            "links": links,
            "downloads": downloads,
            "organisation": organisation,
            "connections": connections
        }))
    }

    pub async fn save_settings(&self, section: &str, values: &Value) -> Result<Value, String> {
        let sec = if section == "ui" || section == "general" {
            "desktop"
        } else if section == "links" {
            "release_links"
        } else {
            section
        };
        self.set_preference(sec, values).await?;
        if sec == "desktop" {
            let _ = self.set_preference("ui", values).await;
        }
        self.bump_revision();
        self.get_settings().await
    }

    pub async fn reset_settings(&self, group: &str) -> Result<Value, String> {
        let home_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let mut current_provider = self
            .get_preference("provider")
            .await?
            .unwrap_or_else(|| json!({}));

        if group == "downloads" {
            let downloads = json!({
                "output": home_dir.join("Downloads/Music").to_string_lossy().to_string(),
                "quality": "LOSSLESS",
                "cover_size": 1280,
                "skip_existing": true,
                "cover_album_file": true,
                "lyrics_embed": false,
                "lyrics_file": false,
                "playlist_create": false,
                "replay_gain": true,
            });
            self.set_preference("downloads", &downloads).await?;

            if let Some(obj) = current_provider.as_object_mut() {
                obj.insert("download_delay".to_string(), json!(true));
                obj.insert("download_delay_min_sec".to_string(), json!(3.0));
                obj.insert("download_delay_max_sec".to_string(), json!(5.0));
                obj.insert("aac_bitrate_cap".to_string(), json!(320));
                obj.insert("download_concurrency".to_string(), json!(2));
                obj.insert("segment_concurrency".to_string(), json!(2));
            }
            self.set_preference("provider", &current_provider).await?;
        } else if group == "metadata" {
            if let Some(obj) = current_provider.as_object_mut() {
                obj.insert("request_interval_ms".to_string(), json!(750));
                obj.insert("api_batch_size".to_string(), json!(20));
                obj.insert("request_timeout_sec".to_string(), json!(20));
                obj.insert("token_refresh_margin_sec".to_string(), json!(30));
                obj.insert("sign_in_timeout_sec".to_string(), json!(180));
                obj.insert("request_attempts".to_string(), json!(2));
                obj.insert("api_batch_delay_sec".to_string(), json!(3.0));
            }
            self.set_preference("provider", &current_provider).await?;
        }
        self.bump_revision();
        self.get_settings().await
    }

    pub async fn set_local_files_ignored(
        &self,
        paths: &[String],
        ignored: bool,
    ) -> Result<(), String> {
        let conn = self.connect()?;
        let now_iso = chrono::Utc::now().to_rfc3339();
        for p in paths {
            if ignored {
                conn.execute(
                    "INSERT OR REPLACE INTO ignored_local_files (path, ignored_at) VALUES (?, ?)",
                    (p.as_str(), now_iso.as_str()),
                )
                .await
                .map_err(|e| e.to_string())?;
            } else {
                conn.execute(
                    "DELETE FROM ignored_local_files WHERE path = ?",
                    (p.as_str(),),
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        self.bump_revision();
        Ok(())
    }

    pub async fn unlink_tracks(&self, paths: &[String]) -> Result<(), String> {
        let conn = self.connect()?;
        for p in paths {
            conn.execute("DELETE FROM track_links WHERE path = ?", (p.as_str(),))
                .await
                .map_err(|e| e.to_string())?;
        }
        self.bump_revision();
        Ok(())
    }

    pub async fn choose_artist(&self, artist: &str, ids: &[String]) -> Result<(), String> {
        let conn = self.connect()?;
        if let Some(primary) = ids.first() {
            conn.execute(
                "INSERT OR REPLACE INTO mappings (artist, tidal_id, status, evidence, manual) VALUES (?, ?, 'confirmed', 'Chosen in artist review', 1)",
                (artist, primary.as_str()),
            )
            .await
            .map_err(|e| e.to_string())?;

            for additional in &ids[1..] {
                conn.execute(
                    "INSERT OR REPLACE INTO additional_mappings (artist, tidal_id) VALUES (?, ?)",
                    (artist, additional.as_str()),
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        self.bump_revision();
        Ok(())
    }

    pub async fn choose_track_link(&self, args: &Value) -> Result<(), String> {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let album_id = args.get("album_id").and_then(|v| v.as_str()).unwrap_or("");
        let track_id = args.get("track_id").and_then(|v| v.as_str()).unwrap_or("");
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let conn = self.connect()?;
        if path.is_empty() || album_id.is_empty() || track_id.is_empty() {
            return Err("Choose a valid release and track placement".into());
        }
        let mut file = conn
            .query(
                "SELECT size,mtime FROM local_files WHERE path=? AND present=1",
                (path,),
            )
            .await
            .map_err(|e| e.to_string())?;
        let row = file
            .next()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Local file is no longer indexed")?;
        let size: i64 = row.get(0).map_err(|e| e.to_string())?;
        let mtime: i64 = row.get(1).map_err(|e| e.to_string())?;
        let now_iso = json!([0, 0, size, mtime]).to_string();
        let payload = json!({
            "status":"linked", "manual":true,
            "ids": {
                "album_id": album_id,
                "track_id": track_id
            }
        });
        let payload_str = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
            (path, market, now_iso.as_str(), payload_str.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;
        self.bump_revision();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn get_queue_rows(
        &self,
        route: &str,
        _filter: Option<&str>,
        search: Option<&str>,
        sort: Option<&str>,
        direction: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<TablePage<MissingRow>, String> {
        let conn = self.connect()?;
        let decision = if route == "downloaded" {
            "downloaded"
        } else {
            "queued"
        };
        let mut stmt = conn
            .query(
                "SELECT id, payload, approved, decision FROM queue WHERE decision = ? ORDER BY updated DESC",
                (decision,),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut rows = Vec::new();
        while let Some(row) = stmt.next().await.map_err(|e| e.to_string())? {
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let payload_str: Option<String> = row.get(1).ok().flatten();
            let approved: i64 = row.get(2).unwrap_or(0);
            let dec: String = row.get(3).unwrap_or_else(|_| decision.to_string());

            let rel = match payload_str
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
            {
                Some(p) => p,
                None => continue,
            };

            let artist = rel
                .get("artist")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let release = rel
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let date = rel
                .get("date")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let rel_type = rel
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("ALBUM")
                .to_string();
            let tracks = rel.get("track_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let available = rel.get("available").and_then(|v| v.as_bool());
            let tracks_loaded = rel
                .get("tracks_loaded")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let mut children = Vec::new();
            if let Some(t_arr) = rel.get("tracks").and_then(|v| v.as_array()) {
                for t in t_arr {
                    let tid = t
                        .get("id")
                        .map(|v| v.to_string().trim_matches('"').to_string())
                        .unwrap_or_default();
                    let ttitle = t
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let disc = t.get("disc_number").and_then(|v| v.as_u64()).unwrap_or(1);
                    let track_num = t
                        .get("track_number")
                        .map(|v| v.to_string().trim_matches('"').to_string())
                        .unwrap_or_default();
                    children.push(MissingChildTrack {
                        id: tid,
                        title: ttitle,
                        position: format!("{} · {}", disc, track_num),
                        duration: t.get("duration").and_then(|v| v.as_f64()),
                        isrc: t
                            .get("isrc")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        bpm: t.get("bpm").and_then(|v| v.as_f64()),
                        key: t.get("key").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    });
                }
            }

            let sel_tracks = if approved != 0 {
                rel.get("selected_tracks")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| {
                                x.get("id")
                                    .map(|i| i.to_string().trim_matches('"').to_string())
                            })
                            .collect()
                    })
            } else {
                Some(Vec::new())
            };

            rows.push(MissingRow {
                id: id.clone(),
                downloaded_files: Vec::new(),
                artist,
                release,
                date,
                r#type: rel_type,
                tracks,
                status: if dec == "downloaded" {
                    "Owned complete".to_string()
                } else {
                    "Queued".to_string()
                },
                online_id: id,
                recommendation: String::new(),
                evidence: Vec::new(),
                expanded_available: tracks_loaded,
                available,
                approved: approved != 0,
                selected: sel_tracks,
                children,
                newest_local_date: None,
                previous_local_date: None,
            });
        }

        if let Some(q) = search {
            let q_trimmed = q.trim().to_lowercase();
            if !q_trimmed.is_empty() {
                rows.retain(|r| {
                    r.artist.to_lowercase().contains(&q_trimmed)
                        || r.release.to_lowercase().contains(&q_trimmed)
                });
            }
        }

        if let Some(column) = sort {
            rows.sort_by(|a, b| {
                let order = match column {
                    "tracks" => a.tracks.cmp(&b.tracks),
                    "release" => a.release.to_lowercase().cmp(&b.release.to_lowercase()),
                    "date" => a.date.cmp(&b.date),
                    "type" => a.r#type.cmp(&b.r#type),
                    "status" => a.status.cmp(&b.status),
                    "online_id" => a.online_id.cmp(&b.online_id),
                    _ => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
                };
                if direction == Some("desc") {
                    order.reverse()
                } else {
                    order
                }
            });
        }
        let total = rows.len();
        let page_rows = if offset < total {
            rows.into_iter().skip(offset).take(limit).collect()
        } else {
            Vec::new()
        };

        Ok(TablePage {
            rows: page_rows,
            total,
            offset,
            revision: self.revision.load(std::sync::atomic::Ordering::SeqCst),
            preview_id: None,
        })
    }

    pub async fn queue_select(
        &self,
        selection: &HashMap<String, Option<Vec<String>>>,
    ) -> Result<(), String> {
        let conn = self.connect()?;
        let now_iso = chrono::Utc::now().to_rfc3339();
        for (ident, sel) in selection {
            let mut stmt = conn
                .query(
                    "SELECT payload FROM queue WHERE id = ? AND decision = 'queued'",
                    (ident.as_str(),),
                )
                .await
                .map_err(|e| e.to_string())?;
            if let Some(row) = stmt.next().await.map_err(|e| e.to_string())? {
                let payload_str: String = row.get(0).map_err(|e| e.to_string())?;
                let mut release: Value =
                    serde_json::from_str(&payload_str).map_err(|e| e.to_string())?;
                let approved: i64;
                if let Some(selected_ids) = sel {
                    let id_set: std::collections::HashSet<String> =
                        selected_ids.iter().cloned().collect();
                    if let Some(tracks) = release.get("tracks").and_then(|v| v.as_array()) {
                        let filtered: Vec<Value> = tracks
                            .iter()
                            .filter(|t| {
                                t.get("id")
                                    .map(|i| id_set.contains(i.to_string().trim_matches('"')))
                                    .unwrap_or(false)
                            })
                            .cloned()
                            .collect();
                        approved = if filtered.is_empty() { 0 } else { 1 };
                        if filtered.len() != id_set.len() {
                            return Err("Some selected tracks are no longer in the cached release; refresh its details".into());
                        }
                        release["selected_tracks"] = json!(filtered);
                    } else {
                        approved = 0;
                    }
                } else {
                    release["selected_tracks"] = Value::Null;
                    approved = 1;
                }
                let updated_payload = serde_json::to_string(&release).map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE queue SET payload = ?, approved = ?, updated = ? WHERE id = ?",
                    (
                        updated_payload.as_str(),
                        approved,
                        now_iso.as_str(),
                        ident.as_str(),
                    ),
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        self.bump_revision();
        Ok(())
    }

    pub async fn queue_decision(&self, ids: &[String], decision: &str) -> Result<(), String> {
        let conn = self.connect()?;
        let now_iso = chrono::Utc::now().to_rfc3339();
        for id in ids {
            if decision == "removed" {
                conn.execute("DELETE FROM queue WHERE id = ?", (id.as_str(),))
                    .await
                    .map_err(|e| e.to_string())?;
            } else if decision == "ignored" {
                conn.execute(
                    "INSERT INTO queue (id, payload, approved, decision, updated) VALUES (?, '{}', 0, 'ignored', ?) ON CONFLICT(id) DO UPDATE SET approved = 0, decision = 'ignored', updated = excluded.updated",
                    (id.as_str(), now_iso.as_str()),
                )
                .await
                .map_err(|e| e.to_string())?;
            } else {
                conn.execute(
                    "UPDATE queue SET decision = ?, approved = 0, updated = ? WHERE id = ?",
                    (decision, now_iso.as_str(), id.as_str()),
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        self.bump_revision();
        Ok(())
    }

    pub async fn queue_redownload(
        &self,
        release_id: &str,
        track_id: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT payload FROM queue WHERE id = ? AND decision = 'downloaded'",
                (release_id,),
            )
            .await
            .map_err(|e| e.to_string())?;
        let row = rows
            .next()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Choose a completed download to redownload")?;
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let mut release: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if let Some(track_id) = track_id {
            let tracks = release["tracks"]
                .as_array()
                .ok_or("Load cached track details first")?;
            let selected: Vec<Value> = tracks
                .iter()
                .filter(|track| track["id"].as_str() == Some(track_id))
                .cloned()
                .collect();
            if selected.len() != 1 {
                return Err("Selected track is absent from the cached release".into());
            }
            release["selected_tracks"] = json!(selected);
        } else {
            release["selected_tracks"] = Value::Null;
        }
        release["redownload"] = json!(true);
        let payload = serde_json::to_string(&release).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute("UPDATE queue SET payload = ?, approved = 1, decision = 'queued', updated = ? WHERE id = ?",
            (payload.as_str(), now.as_str(), release_id)).await.map_err(|e| e.to_string())?;
        self.bump_revision();
        Ok(())
    }

    pub async fn queue_add(
        &self,
        selection: &HashMap<String, Option<Vec<String>>>,
    ) -> Result<(), String> {
        let conn = self.connect()?;
        let now_iso = chrono::Utc::now().to_rfc3339();

        for (ident, sel_tracks) in selection {
            let settings = self
                .get_preference("desktop")
                .await?
                .or(self.get_preference("ui").await?)
                .unwrap_or(Value::Null);
            let market = settings["market"].as_str().unwrap_or("GB");
            let mut cat_stmt = conn
                .query("SELECT payload FROM catalogue WHERE market = ?", (market,))
                .await
                .map_err(|e| e.to_string())?;
            let mut found_release: Option<Value> = None;
            while let Some(row) = cat_stmt.next().await.map_err(|e| e.to_string())? {
                if let Ok(Some(cat_str)) = row.get::<Option<String>>(0) {
                    if let Ok(cat_val) = serde_json::from_str::<Value>(&cat_str) {
                        if let Some(releases) = cat_val.get("releases").and_then(|v| v.as_array()) {
                            for r in releases {
                                if r.get("id")
                                    .map(|i| i.to_string().trim_matches('"').to_string())
                                    .as_deref()
                                    == Some(ident.as_str())
                                {
                                    found_release = Some(r.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
                if found_release.is_some() {
                    break;
                }
            }

            if found_release.is_none() {
                found_release = self
                    .get_preference(&format!("tag-review:{market}:{ident}"))
                    .await?;
            }
            let mut release = found_release.ok_or_else(|| {
                format!("Release {ident} has no cached metadata; refresh it before queueing")
            })?;

            if let Some(ids) = sel_tracks {
                if ids.is_empty() {
                    return Err("Select at least one audio track".into());
                }
                let id_set: std::collections::HashSet<String> = ids.iter().cloned().collect();
                if let Some(tracks) = release.get("tracks").and_then(|v| v.as_array()) {
                    let filtered: Vec<Value> = tracks
                        .iter()
                        .filter(|t| {
                            t.get("id")
                                .map(|i| id_set.contains(i.to_string().trim_matches('"')))
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    if filtered.len() != id_set.len() {
                        return Err("Some selected tracks are no longer in the cached release; refresh its details".into());
                    }
                    release["selected_tracks"] = json!(filtered);
                } else {
                    return Err("Load release details before selecting individual tracks".into());
                }
            } else {
                release["selected_tracks"] = Value::Null;
            }
            release["url"] = json!(format!("https://tidal.com/album/{}", ident));

            let payload_str = serde_json::to_string(&release).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO queue (id, payload, approved, decision, updated) VALUES (?, ?, 1, 'queued', ?) ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, approved = 1, decision = 'queued', updated = excluded.updated",
                (ident.as_str(), payload_str.as_str(), now_iso.as_str()),
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        self.bump_revision();
        Ok(())
    }

    pub async fn queue_export(
        &self,
        format: &str,
        decision: Option<&str>,
    ) -> Result<String, String> {
        let dec = decision.unwrap_or("queued");
        let conn = self.connect()?;
        let mut stmt = conn
            .query(
                "SELECT id, payload, approved FROM queue WHERE decision = ? ORDER BY id",
                (dec,),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        while let Some(row) = stmt.next().await.map_err(|e| e.to_string())? {
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let payload_str: Option<String> = row.get(1).ok().flatten();
            let approved: i64 = row.get(2).unwrap_or(0);
            if let Some(p) = payload_str
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
            {
                items.push((id, p, approved != 0));
            }
        }

        if format == "csv" {
            let mut csv = String::from("Artist,Release,Year,Type,Tracks,ID,URL\n");
            for (id, rel, _) in items {
                let artist = rel.get("artist").and_then(|v| v.as_str()).unwrap_or("");
                let release = rel.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let year = rel.get("date").and_then(|v| v.as_str()).unwrap_or("");
                let rel_type = rel.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let tracks = rel.get("track_count").and_then(|v| v.as_u64()).unwrap_or(0);
                csv.push_str(&format!(
                    "\"{}\",\"{}\",\"{}\",\"{}\",{},\"{}\",\"https://tidal.com/album/{}\"\n",
                    artist.replace('"', "\"\""),
                    release.replace('"', "\"\""),
                    year,
                    rel_type,
                    tracks,
                    id,
                    id
                ));
            }
            Ok(csv)
        } else {
            let list: Vec<Value> = items
                .into_iter()
                .map(|(id, rel, app)| {
                    json!({
                        "id": id,
                        "release": rel,
                        "approved": app
                    })
                })
                .collect();
            serde_json::to_string_pretty(&list).map_err(|e| e.to_string())
        }
    }

    pub fn clean_evidence_str(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let parsed_text = if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
            match val {
                Value::String(s) => s,
                Value::Array(arr) => {
                    let items: Vec<String> = arr
                        .iter()
                        .filter_map(|v| match v {
                            Value::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .collect();
                    if !items.is_empty() {
                        items.join(" · ")
                    } else {
                        serde_json::to_string(&arr).unwrap_or_default()
                    }
                }
                Value::Object(map) => {
                    if let Some(msg) = map
                        .get("message")
                        .or_else(|| map.get("note"))
                        .or_else(|| map.get("evidence"))
                        .and_then(|v| v.as_str())
                    {
                        msg.to_string()
                    } else if let Some(reasons) = map.get("reasons").and_then(|v| v.as_array()) {
                        reasons
                            .iter()
                            .filter_map(|r| r.as_str())
                            .collect::<Vec<_>>()
                            .join(" · ")
                    } else {
                        trimmed.trim_matches('"').to_string()
                    }
                }
                _ => trimmed.trim_matches('"').to_string(),
            }
        } else {
            trimmed.trim_matches('"').to_string()
        };

        parsed_text
            .replace("\\u2014", "—")
            .replace("\\u2013", "–")
            .replace("\\u00b7", "·")
            .replace("\\u2026", "…")
            .replace("\\\"", "\"")
            .replace("&amp;", "&")
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn get_artist_rows(
        &self,
        root: Option<&str>,
        filter: Option<&str>,
        search: Option<&str>,
        sort: Option<&str>,
        direction: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<TablePage<Value>, String> {
        let conn = self.connect()?;
        let clean_root = root.map(|r| r.trim_end_matches('/').to_string());
        let (sql, params_vec): (&str, Vec<String>) = match clean_root.as_deref() {
            Some(r) if !r.is_empty() => (
                "SELECT metadata FROM local_files WHERE present = 1 AND metadata IS NOT NULL AND (root = ? OR root = ?)",
                vec![r.to_string(), format!("{}/", r)],
            ),
            _ => (
                "SELECT metadata FROM local_files WHERE present = 1 AND metadata IS NOT NULL",
                vec![],
            ),
        };
        let mut file_stmt = if params_vec.is_empty() {
            conn.query(sql, ()).await.map_err(|e| e.to_string())?
        } else {
            conn.query(sql, (params_vec[0].as_str(), params_vec[1].as_str()))
                .await
                .map_err(|e| e.to_string())?
        };

        let mut artist_tracks: HashMap<String, usize> = HashMap::new();
        let mut artist_albums: HashMap<String, std::collections::HashSet<String>> = HashMap::new();

        while let Some(row) = file_stmt.next().await.map_err(|e| e.to_string())? {
            if let Ok(Some(meta_str)) = row.get::<Option<String>>(0) {
                if let Ok(meta) = serde_json::from_str::<Value>(&meta_str) {
                    let art =
                        extract_album_artist(&meta).unwrap_or_else(|| "Unknown artist".to_string());
                    if is_compilation_artist(&art) { continue; }
                    let alb = extract_tag_str(&meta, &["album", "release"]).unwrap_or_default();
                    *artist_tracks.entry(art.clone()).or_insert(0) += 1;
                    artist_albums.entry(art).or_default().insert(alb);
                }
            }
        }

        let mut map_stmt = conn
            .query(
                "SELECT artist, tidal_id, status, evidence FROM mappings",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut mappings: HashMap<String, (String, String, String)> = HashMap::new();
        let mut mappings_lower: HashMap<String, (String, String, String)> = HashMap::new();
        while let Some(row) = map_stmt.next().await.map_err(|e| e.to_string())? {
            let art: String = row.get(0).map_err(|e| e.to_string())?;
            let tid: String = row.get(1).unwrap_or_default();
            let st: String = row.get(2).unwrap_or_default();
            let ev: String = row.get(3).unwrap_or_default();
            mappings_lower.insert(art.to_lowercase(), (tid.clone(), st.clone(), ev.clone()));
            mappings.insert(art, (tid, st, ev));
        }

        let mut rows = Vec::new();
        for (artist, tracks) in artist_tracks {
            let (online_id, raw_status, evidence) = mappings
                .get(&artist)
                .or_else(|| mappings_lower.get(&artist.to_lowercase()))
                .cloned()
                .unwrap_or_else(|| (String::new(), "Unresolved".to_string(), String::new()));
            let releases = artist_albums.get(&artist).map(|s| s.len()).unwrap_or(0);
            let clean_evidence = Self::clean_evidence_str(&evidence);
            let display_status = match raw_status.to_lowercase().as_str() {
                "auto" => "Auto-matched",
                "confirmed" => "Confirmed",
                "review" => "Needs review",
                _ => {
                    if online_id.is_empty() {
                        "Unresolved"
                    } else {
                        &raw_status
                    }
                }
            };
            rows.push(json!({
                "id": artist,
                "artist": artist,
                "tracks": tracks,
                "release": releases,
                "status": display_status,
                "online_id": online_id,
                "evidence": clean_evidence,
            }));
        }

        // Apply filters
        if let Some(f) = filter {
            let f_lower = f.trim().to_lowercase();
            match f_lower.as_str() {
                "unresolved" => {
                    rows.retain(|r| {
                        let st = r
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let oid = r.get("online_id").and_then(|v| v.as_str()).unwrap_or("");
                        st.contains("unresolved") || oid.is_empty()
                    });
                }
                "matched" | "confirmed" => {
                    rows.retain(|r| {
                        let st = r
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let oid = r.get("online_id").and_then(|v| v.as_str()).unwrap_or("");
                        !oid.is_empty()
                            && (st.contains("confirmed")
                                || st.contains("auto")
                                || st.contains("matched"))
                    });
                }
                "review" => {
                    rows.retain(|r| {
                        let st = r
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let ev = r
                            .get("evidence")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        st.contains("review")
                            || st.contains("candidate")
                            || st.contains("ambiguous")
                            || ev.contains("confirm identity")
                            || ev.contains("review")
                    });
                }
                _ => {} // "all" or any other
            }
        }

        if let Some(q) = search {
            let q_trimmed = q.trim().to_lowercase();
            if !q_trimmed.is_empty() {
                rows.retain(|r| {
                    r.get("artist")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q_trimmed)
                });
            }
        }

        let sort_col = sort.unwrap_or("artist");
        let desc = direction
            .map(|d| d.eq_ignore_ascii_case("desc"))
            .unwrap_or(false);
        rows.sort_by(|a, b| {
            let ord = match sort_col {
                "tracks" => {
                    let a_val = a.get("tracks").and_then(|v| v.as_u64()).unwrap_or(0);
                    let b_val = b.get("tracks").and_then(|v| v.as_u64()).unwrap_or(0);
                    a_val.cmp(&b_val)
                }
                "release" => {
                    let a_val = a.get("release").and_then(|v| v.as_u64()).unwrap_or(0);
                    let b_val = b.get("release").and_then(|v| v.as_u64()).unwrap_or(0);
                    a_val.cmp(&b_val)
                }
                "status" => {
                    let a_val = a.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    let b_val = b.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    a_val.to_lowercase().cmp(&b_val.to_lowercase())
                }
                "online_id" => {
                    let a_val = a.get("online_id").and_then(|v| v.as_str()).unwrap_or("");
                    let b_val = b.get("online_id").and_then(|v| v.as_str()).unwrap_or("");
                    a_val.cmp(b_val)
                }
                _ => {
                    let a_val = a.get("artist").and_then(|v| v.as_str()).unwrap_or("");
                    let b_val = b.get("artist").and_then(|v| v.as_str()).unwrap_or("");
                    a_val.to_lowercase().cmp(&b_val.to_lowercase())
                }
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });

        let total = rows.len();
        let page_rows = if offset < total {
            rows.into_iter().skip(offset).take(limit).collect()
        } else {
            Vec::new()
        };

        Ok(TablePage {
            rows: page_rows,
            total,
            offset,
            revision: self.revision.load(std::sync::atomic::Ordering::SeqCst),
            preview_id: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn get_favourite_rows(
        &self,
        root: Option<&str>,
        filter: &str,
        search: &str,
        sort: &str,
        direction: &str,
        offset: usize,
        limit: usize,
    ) -> Result<TablePage<Value>, String> {
        let revision = self.revision.load(std::sync::atomic::Ordering::SeqCst);
        let cache_key = root.unwrap_or("").to_string();
        let cached = self
            .favourite_rows_cache
            .lock()
            .unwrap()
            .get(&cache_key)
            .filter(|(rev, _)| *rev == revision)
            .map(|(_, rows)| rows.clone());
        let mut rows = if let Some(base) = cached {
            base
        } else {
            let conn = self.connect()?;
            let mut local_counts: HashMap<String, (String, usize)> = HashMap::new();
            let mut files = conn
                .query(
                    "SELECT metadata FROM local_files WHERE present = 1 AND (? = '' OR root = ?)",
                    (root.unwrap_or(""), root.unwrap_or("")),
                )
                .await
                .map_err(|e| e.to_string())?;
            while let Some(file) = files.next().await.map_err(|e| e.to_string())? {
                let metadata: Option<String> = file.get(0).ok().flatten();
                if let Some(meta) = metadata
                    .as_deref()
                    .and_then(|text| serde_json::from_str::<Value>(text).ok())
                {
                    if let Some(artist) = extract_album_artist(&meta) {
                        if !is_compilation_artist(&artist) {
                            let key = artist.to_lowercase();
                            let item = local_counts.entry(key).or_insert((artist, 0));
                            item.1 += 1;
                        }
                    }
                }
            }
            drop(files);
            let mut stmt = conn
                .query("SELECT payload FROM favourite_artists", ())
                .await
                .map_err(|e| e.to_string())?;
            let mut rows = Vec::new();
            let mut favourited = HashSet::new();
            while let Some(row) = stmt.next().await.map_err(|e| e.to_string())? {
                if let Ok(Some(payload_str)) = row.get::<Option<String>>(0) {
                    if let Ok(items) = serde_json::from_str::<Vec<Value>>(&payload_str) {
                        for item in items {
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let id = item
                                .get("id")
                                .map(|v| v.to_string().trim_matches('"').to_string())
                                .unwrap_or_default();
                            let key = name.to_lowercase();
                            if name.is_empty() || !favourited.insert(key.clone()) {
                                continue;
                            }
                            let tracks =
                                local_counts.get(&key).map(|(_, count)| *count).unwrap_or(0);
                            rows.push(json!({
                                "id": format!("favourite:{id}"),
                                "artist": name,
                                "status": if tracks > 0 { "In library" } else { "Missing locally" },
                                "tracks": tracks,
                                "online_id": id
                            }));
                        }
                    }
                }
            }
            for (key, (name, count)) in local_counts {
                if !favourited.contains(&key) {
                    rows.push(json!({"id":format!("local:{key}"),"artist":name,"status":"Local only","tracks":count,"online_id":""}));
                }
            }
            let mut cache = self.favourite_rows_cache.lock().unwrap();
            cache.clear();
            cache.insert(cache_key, (revision, rows.clone()));
            rows
        };
        if filter != "all" && !filter.is_empty() {
            rows.retain(|row| row["status"].as_str() == Some(filter));
        }
        let query = search.trim().to_lowercase();
        if !query.is_empty() {
            rows.retain(|row| {
                row["artist"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query)
            });
        }
        rows.sort_by(|a, b| {
            let cmp = match sort {
                "tracks" => a["tracks"].as_u64().cmp(&b["tracks"].as_u64()),
                "status" => a["status"].as_str().cmp(&b["status"].as_str()),
                "online_id" => a["online_id"].as_str().cmp(&b["online_id"].as_str()),
                _ => a["artist"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&b["artist"].as_str().unwrap_or("").to_lowercase()),
            };
            let cmp = if direction == "desc" {
                cmp.reverse()
            } else {
                cmp
            };
            cmp.then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        let total = rows.len();
        Ok(TablePage {
            rows: rows.into_iter().skip(offset).take(limit).collect(),
            total,
            offset,
            revision: self.revision.load(std::sync::atomic::Ordering::SeqCst),
            preview_id: None,
        })
    }

    pub async fn get_detail(&self, args: &Value) -> Result<Value, String> {
        let conn = self.connect()?;
        if let Some(release_id) = args.get("release_id").and_then(|v| v.as_str()) {
            let settings = self
                .get_preference("desktop")
                .await?
                .or(self.get_preference("ui").await?)
                .unwrap_or(Value::Null);
            let market = args["market"]
                .as_str()
                .or(settings["market"].as_str())
                .unwrap_or("GB");
            if let Some(cached) = self
                .get_preference(&format!("tag-review:{market}:{release_id}"))
                .await?
            {
                return Ok(cached);
            }
            let mut cat_stmt = conn
                .query("SELECT payload FROM catalogue WHERE market = ?", (market,))
                .await
                .map_err(|e| e.to_string())?;
            while let Some(row) = cat_stmt.next().await.map_err(|e| e.to_string())? {
                if let Ok(Some(cat_str)) = row.get::<Option<String>>(0) {
                    if let Ok(cat_val) = serde_json::from_str::<Value>(&cat_str) {
                        if let Some(releases) = cat_val.get("releases").and_then(|v| v.as_array()) {
                            for r in releases {
                                if r.get("id")
                                    .map(|i| i.to_string().trim_matches('"').to_string())
                                    .as_deref()
                                    == Some(release_id)
                                {
                                    return Ok(r.clone());
                                }
                            }
                        }
                    }
                }
            }
            return Ok(json!({ "id": release_id, "title": "Release not found" }));
        }

        if let Some(artist) = args.get("artist").and_then(|v| v.as_str()) {
            let mut rev_stmt = conn
                .query(
                    "SELECT payload FROM match_reviews WHERE artist = ?",
                    (artist,),
                )
                .await
                .map_err(|e| e.to_string())?;
            let payload: Value =
                if let Some(row) = rev_stmt.next().await.map_err(|e| e.to_string())? {
                    row.get::<Option<String>>(0)
                        .ok()
                        .flatten()
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or(json!({}))
                } else {
                    json!({})
                };
            let mut linked=conn.query("SELECT tidal_id FROM mappings WHERE artist=? AND tidal_id IS NOT NULL AND tidal_id != '' UNION SELECT tidal_id FROM additional_mappings WHERE artist=? AND tidal_id IS NOT NULL AND tidal_id != ''",(artist,artist)).await.map_err(|e|e.to_string())?;
            let mut ids = vec![];
            while let Some(row) = linked.next().await.map_err(|e| e.to_string())? {
                ids.push(row.get::<String>(0).map_err(|e| e.to_string())?);
            }
            let mut payload = payload;
            if let Some(candidates) = payload["candidates"].as_array_mut() {
                for candidate in candidates {
                    if candidate["id"].is_null() { candidate["id"] = candidate["artist"]["id"].clone(); }
                    if candidate["name"].is_null() { candidate["name"] = candidate["artist"]["name"].clone(); }
                }
            }
            let mut local_files = Vec::new();
            let root = args["root"].as_str().unwrap_or("");
            let mut files = conn.query("SELECT path,metadata FROM local_files WHERE present=1 AND (?='' OR root=?)", (root,root)).await.map_err(|e| e.to_string())?;
            while let Some(row) = files.next().await.map_err(|e| e.to_string())? {
                let metadata = row.get::<Option<String>>(1).ok().flatten().and_then(|s| serde_json::from_str(&s).ok());
                let tags = crate::workflows::extract_tags_map(&metadata);
                if tags.get("albumartist").or(tags.get("artist")).is_some_and(|s| crate::matching::name_key(s) == crate::matching::name_key(artist)) {
                    local_files.push(json!({"path":row.get::<String>(0).map_err(|e|e.to_string())?,"title":tags.get("title"),"release":tags.get("album"),"performers":tags.get("artist"),"isrc":tags.get("isrc")}));
                }
            }
            return Ok(json!({"artist":artist,"ids":ids,"review":payload,"local_files":local_files}));
        }

        if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            let mut file_stmt = conn
                .query(
                    "SELECT path, metadata FROM local_files WHERE path = ?",
                    (path,),
                )
                .await
                .map_err(|e| e.to_string())?;
            if let Some(row) = file_stmt.next().await.map_err(|e| e.to_string())? {
                let p: String = row.get(0).map_err(|e| e.to_string())?;
                let meta_str: Option<String> = row.get(1).ok().flatten();
                let meta: Value = meta_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null);
                let settings = self.get_settings().await?;
                let market = args["market"]
                    .as_str()
                    .unwrap_or(settings["general"]["market"].as_str().unwrap_or("GB"));
                let mut links = conn
                    .query(
                        "SELECT payload FROM track_links WHERE path=? AND market=?",
                        (path, market),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let link = if let Some(row) = links.next().await.map_err(|e| e.to_string())? {
                    let raw: String = row.get(0).unwrap_or_default();
                    serde_json::from_str::<Value>(&raw).unwrap_or(json!({}))
                } else {
                    json!({})
                };
                let tags = crate::workflows::extract_tags_map(&Some(meta.clone()));
                let arrays: serde_json::Map<String, Value> =
                    tags.iter().map(|(k, v)| (k.clone(), json!([v]))).collect();
                let options:Vec<Value>=link["catalogue_options"].as_array().into_iter().flatten().map(|option|{
                    let mut value=option.clone();
                    if value["album"].is_null(){value["album"]=value["title"].clone();}
                    if value["artist"].is_null(){value["artist"]=json!(tags.get("albumartist").or(tags.get("artist")));}
                    if value["position_label"].is_null(){value["position_label"]=value["position"].clone();}
                    if let Some(evidence)=value["evidence"].as_array(){value["evidence"]=json!(evidence.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "));}
                    if value["structure"].is_null(){value["structure"]=json!({"compatible":value["compatible"].as_bool().unwrap_or(false),"reasons":value["evidence"]});}
                    value
                }).collect();

                let mut sources = Vec::new();
                let mut seen_sources = std::collections::HashSet::new();
                let placements = link["placements"].as_array().cloned().unwrap_or_default();
                for source in std::iter::once(&link["ids"])
                    .chain(placements.iter())
                    .chain(options.iter())
                {
                    let album = source["album_id"].as_str().or(source["id"].as_str());
                    let track = source["track_id"].as_str();
                    let (Some(album), Some(track)) = (album, track) else {
                        continue;
                    };
                    if !seen_sources.insert((album.to_string(), track.to_string())) {
                        continue;
                    }
                    let dj = self
                        .get_preference(&format!("dj-check:{market}:{track}"))
                        .await?;
                    let cached = self
                        .get_preference(&format!("tag-review:{market}:{album}"))
                        .await?
                        .unwrap_or(Value::Null);
                    let recording = cached["tracks"]
                        .as_array()
                        .and_then(|tracks| tracks.iter().find(|t| t["id"].as_str() == Some(track)))
                        .cloned()
                        .unwrap_or(Value::Null);
                    sources.push(json!({"album_id":album,"track_id":track,
                        "release":cached["title"].as_str().or(source["album"].as_str()),
                        "artist":cached["artist"],"date":cached["date"],"label":cached["label"],"upc":cached["upc"].as_str().or(cached["barcode"].as_str()),"genres":recording["genres"],"release_genres":cached["genres"],"provider_replacement_id":cached["replacement_id"],
                        "title":recording["title"],"isrc":recording["isrc"],
                        "bpm":dj.as_ref().and_then(|d|d.get("bpm")).unwrap_or(&recording["bpm"]),
                        "key":dj.as_ref().and_then(|d|d.get("key")).unwrap_or(&recording["key"]),
                        "dj_check":if dj.is_some(){"Checked; blank values were not supplied by this source"}else{"No saved additional metadata check"}}));
                }
                return Ok(
                    json!({"id":p,"path":p,"metadata":meta,"tags":arrays,"linked_ids":link["ids"],"dj_checks":sources,
                    "local_position":({
                        let position = |key: &str, total: &str| {
                            let raw = tags.get(key).map(String::as_str).unwrap_or("?");
                            let n = raw.split('/').next().unwrap_or("?");
                            let count = tags.get(total).map(String::as_str).or_else(|| raw.split('/').nth(1)).unwrap_or("?");
                            format!("{n}/{count}")
                        };
                        format!("Disc {} · Track {}",position("discnumber","disctotal"),position("tracknumber","tracktotal"))
                    }),
                    "catalogue_note":link["catalogue_note"],"catalogue_options":options}),
                );
            }
        }

        Ok(json!({}))
    }
}

fn extract_tag_str(val: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = val.get(*k) {
            if let Some(s) = v.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            } else if let Some(arr) = v.as_array() {
                if let Some(s) = arr.first().and_then(|x| x.as_str()) {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }
    if let Some(tags) = val.get("tags").and_then(|t| t.as_object()) {
        for k in keys {
            if let Some(v) = tags.get(*k) {
                if let Some(s) = v.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                } else if let Some(arr) = v.as_array() {
                    if let Some(s) = arr.first().and_then(|x| x.as_str()) {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_album_artist(val: &Value) -> Option<String> {
    // Search both metadata locations for the grouping tag before considering performer credits.
    extract_tag_str(val, &["album_artist", "albumartist"])
        .or_else(|| extract_tag_str(val, &["artist"]))
}

fn compare_release_dates(left: &str, right: &str) -> std::cmp::Ordering {
    let left_year = left.chars().take(4).collect::<String>();
    let right_year = right.chars().take(4).collect::<String>();
    let year_order = left_year.cmp(&right_year);
    if !year_order.is_eq() {
        return year_order;
    }
    // A year-only tag cannot prove that another release in that year is newer.
    if left.len() <= 4 || right.len() <= 4 {
        return std::cmp::Ordering::Equal;
    }
    left.cmp(right)
}

fn extract_tag_num(val: &Value, keys: &[&str]) -> Option<u32> {
    for k in keys {
        if let Some(v) = val.get(*k) {
            if let Some(n) = v.as_u64() {
                return Some(n as u32);
            } else if let Some(s) = v.as_str() {
                let part = s.split('/').next().unwrap_or(s).trim();
                if let Ok(n) = part.parse::<u32>() {
                    return Some(n);
                }
            } else if let Some(arr) = v.as_array() {
                if let Some(first) = arr.first() {
                    if let Some(n) = first.as_u64() {
                        return Some(n as u32);
                    } else if let Some(s) = first.as_str() {
                        let part = s.split('/').next().unwrap_or(s).trim();
                        if let Ok(n) = part.parse::<u32>() {
                            return Some(n);
                        }
                    }
                }
            }
        }
    }
    if let Some(tags) = val.get("tags").and_then(|t| t.as_object()) {
        for k in keys {
            if let Some(v) = tags.get(*k) {
                if let Some(n) = v.as_u64() {
                    return Some(n as u32);
                } else if let Some(s) = v.as_str() {
                    let part = s.split('/').next().unwrap_or(s).trim();
                    if let Ok(n) = part.parse::<u32>() {
                        return Some(n);
                    }
                } else if let Some(arr) = v.as_array() {
                    if let Some(first) = arr.first() {
                        if let Some(n) = first.as_u64() {
                            return Some(n as u32);
                        } else if let Some(s) = first.as_str() {
                            let part = s.split('/').next().unwrap_or(s).trim();
                            if let Ok(n) = part.parse::<u32>() {
                                return Some(n);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_tag_f64(val: &Value, keys: &[&str]) -> Option<f64> {
    for k in keys {
        if let Some(v) = val.get(*k) {
            if let Some(f) = v.as_f64() {
                return Some(f);
            } else if let Some(s) = v.as_str() {
                if let Ok(f) = s.trim().parse::<f64>() {
                    return Some(f);
                }
            } else if let Some(arr) = v.as_array() {
                if let Some(first) = arr.first() {
                    if let Some(f) = first.as_f64() {
                        return Some(f);
                    } else if let Some(s) = first.as_str() {
                        if let Ok(f) = s.trim().parse::<f64>() {
                            return Some(f);
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn link_stamp_matches(raw: &str, size: i64, mtime: i64) -> bool {
    let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(raw) else { return false; };
    let offset = if parts.len() >= 4 { 2 } else if parts.len() == 2 { 0 } else { return false; };
    parts[offset].as_i64() == Some(size) && parts[offset + 1].as_i64() == Some(mtime)
}

fn is_compilation_artist(artist: &str) -> bool {
    let lower = artist.trim().to_lowercase();
    matches!(
        lower.as_str(),
        "various artists" | "various artist" | "va" | "v a" | "v.a." | "v.a"
    )
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn recommendations_use_only_current_verified_remote_credit_anchors() {
        let folder =
            std::env::temp_dir().join(format!("tibrary-credit-anchors-{}", uuid::Uuid::new_v4()));
        let store = super::TursoDb::open(&folder.join("db")).await.unwrap();
        let conn = store.connect().unwrap();
        let credits = serde_json::json!([{"name":"Writer One","role":"Composer"},{"name":"Writer Two","role":"Producer"}]);
        let catalogue = serde_json::json!({"id":"artist","name":"Example","releases":[
            {"id":"owned","title":"Owned","artist":"Example","date":"2020-01-01","available":true,"tracks_loaded":true,"track_count":1,"tracks":[{"id":"anchor","title":"Owned song","credits":credits}]},
            {"id":"new","title":"New","artist":"Example","date":"2021-01-01","available":true,"primary_artist_verified":true,"tracks_loaded":true,"track_count":2,"tracks":[{"id":"new1","title":"New one","credits":credits},{"id":"new2","title":"New two","credits":credits}]}
        ]});
        conn.execute(
            "INSERT INTO catalogue(artist_id,market,payload) VALUES('artist','GB',?)",
            (catalogue.to_string(),),
        )
        .await
        .unwrap();
        let meta =
            serde_json::json!({"albumartist":"Example","album":"Owned","title":"Owned song"});
        conn.execute("INSERT INTO local_files(path,root,size,mtime,metadata,present) VALUES('/music/song.flac','/music',10,20,?,1)", (meta.to_string(),)).await.unwrap();
        let link =
            serde_json::json!({"status":"linked","ids":{"track_id":"anchor","album_id":"owned"}});
        conn.execute("INSERT INTO track_links(path,market,stamp,payload) VALUES('/music/song.flac','GB','[0,0,10,20]',?)", (link.to_string(),)).await.unwrap();
        conn.execute("INSERT OR IGNORE INTO mappings(artist,tidal_id,status) SELECT artist_id,artist_id,'confirmed' FROM catalogue", ()).await.unwrap();
        let rows = store.build_missing_rows("GB").await.unwrap();
        let row = rows.iter().find(|row| row.id == "new").unwrap();
        assert_eq!(row.recommendation, "Recommended");
        conn.execute("UPDATE local_files SET mtime=21", ())
            .await
            .unwrap();
        let rows = store.build_missing_rows("GB").await.unwrap();
        assert_ne!(
            rows.iter()
                .find(|row| row.id == "new")
                .unwrap()
                .recommendation,
            "Recommended"
        );
        drop(conn);
        drop(store);
        std::fs::remove_dir_all(folder).unwrap();
    }
    #[tokio::test]
    async fn clearing_one_activity_stream_preserves_the_others() {
        let path =
            std::env::temp_dir().join(format!("tibrary_log_streams_{}.db", uuid::Uuid::new_v4()));
        let db = super::TursoDb::open(&path).await.unwrap();
        db.log_activity(
            "2026-09-26T12:00:00Z",
            "Scanning download folder",
            "info",
            "scan",
        )
        .await
        .unwrap();
        db.log_activity(
            "2026-09-26T12:00:01Z",
            "Fetching catalogue",
            "info",
            "online",
        )
        .await
        .unwrap();
        db.log_activity(
            "2026-09-26T12:00:02Z",
            "Downloading track",
            "info",
            "download",
        )
        .await
        .unwrap();
        db.clear_log_stream("local").await.unwrap();
        let remaining = db.load_recent_logs(10).await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0]["category"], "online");
        assert_eq!(remaining[1]["category"], "download");
        drop(db);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn album_artist_precedes_performer_even_when_nested_and_year_is_not_a_full_date() {
        let metadata =
            serde_json::json!({"artist":"Guest singer","tags":{"albumartist":["Main artist"]}});
        assert_eq!(
            super::extract_album_artist(&metadata).as_deref(),
            Some("Main artist")
        );
        assert!(super::compare_release_dates("2025-09-15", "2025").is_eq());
        assert!(super::compare_release_dates("2026-01-01", "2025").is_gt());
    }
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn test_turso_db_schema_and_crud() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_tibrary_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("test_library.sqlite3");
        let store = TursoDb::open(&db_path)
            .await
            .expect("Failed to open TursoDb");

        // Roots
        store
            .add_root("/Volumes/Music/Library")
            .await
            .expect("Failed to add root");
        let roots = store.list_roots("GB").await.expect("Failed to list roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].root, "/Volumes/Music/Library");

        // Preferences
        let pref_val = json!({"theme": "dark", "market": "GB"});
        store
            .set_preference("ui", &pref_val)
            .await
            .expect("Failed to set preference");
        let read_back = store
            .get_preference("ui")
            .await
            .expect("Failed to get preference");
        assert_eq!(read_back, Some(pref_val));

        // Insert dummy file
        let conn = store.connect().expect("Failed to connect");
        let meta = json!({
            "artist": "Bicep",
            "album_artist": "Bicep",
            "album": "Isles",
            "title": "Atlas",
            "tidal_album_id": "12345",
            "tidal_track_id": "67890"
        });
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES (?, ?, ?, ?, ?, 1)",
            (
                "/Volumes/Music/Library/Bicep/Isles/01 Atlas.flac",
                "/Volumes/Music/Library",
                25000000i64,
                1700000000i64,
                serde_json::to_string(&meta).unwrap().as_str(),
            ),
        )
        .await
        .expect("Failed to insert local file");

        // Stats calculation with Turso
        let stats = store
            .get_stats("GB", None)
            .await
            .expect("Failed to get stats");
        assert_eq!(stats.track_count, 1);
        assert_eq!(stats.linked_tracks, 1);
        assert_eq!(stats.release_count, 1);
        assert_eq!(stats.linked_releases, 1);
        assert_eq!(stats.artists, 1);
        assert_eq!(stats.unresolved_artists, 1); // Not in mappings table yet

        // Page query
        let (files, total) = store
            .get_local_files_page(None, 10, 0)
            .await
            .expect("Failed to get page");
        assert_eq!(total, 1);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path,
            "/Volumes/Music/Library/Bicep/Isles/01 Atlas.flac"
        );

        // Clean up
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_python_turso_bidirectional_compat() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_tibrary_bi_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("shared.sqlite3");
        let _root_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");

        // 1. Python writes to the database using standard sqlite3
        let script = format!(
            r#"
import sqlite3, json
with sqlite3.connect('{db}') as db:
    db.execute("CREATE TABLE IF NOT EXISTS roots(root TEXT PRIMARY KEY, scanned_at TEXT, status TEXT)")
    db.execute("CREATE TABLE IF NOT EXISTS local_files(path TEXT PRIMARY KEY, root TEXT, size INTEGER, mtime INTEGER, metadata TEXT, error TEXT, present INTEGER DEFAULT 1)")
    db.execute("CREATE TABLE IF NOT EXISTS mappings(artist TEXT PRIMARY KEY, tidal_id TEXT, status TEXT, evidence TEXT, manual INTEGER DEFAULT 0)")
    db.execute("CREATE TABLE IF NOT EXISTS queue(id TEXT PRIMARY KEY, payload TEXT, approved INTEGER DEFAULT 0, decision TEXT DEFAULT 'queued', updated TEXT)")
    db.execute("INSERT INTO roots VALUES ('/Music/DJ', '2026-09-24T00:00:00Z', 'active')")
    meta = json.dumps({{'artist': 'Overmono', 'album_artist': 'Overmono', 'album': 'Good Lies', 'title': 'So U Kno', 'tidal_album_id': '111', 'tidal_track_id': '222'}})
    db.execute("INSERT INTO local_files VALUES ('/Music/DJ/Overmono/01.flac', '/Music/DJ', 1000, 2000, ?, NULL, 1)", (meta,))
    db.execute("INSERT INTO mappings VALUES ('Overmono', '999', 'confirmed', '[]', 1)")
    db.execute("INSERT INTO queue VALUES ('q1', '{{}}', 0, 'queued', '2026-09-24')")
"#,
            db = db_path.display()
        );

        let py_status = std::process::Command::new("python3")
            .arg("-c")
            .arg(&script)
            .status()
            .expect("Failed to execute Python seeding script");
        assert!(py_status.success(), "Python seeding failed");

        // 2. Turso opens and reads what Python wrote
        let store = TursoDb::open(&db_path)
            .await
            .expect("Turso failed to open Python-created DB");
        let roots = store
            .list_roots("GB")
            .await
            .expect("Turso failed to list roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].root, "/Music/DJ");
        assert_eq!(roots[0].tracks, 1);
        assert_eq!(roots[0].linked, 1);

        let stats = store
            .get_stats("GB", None)
            .await
            .expect("Turso failed to get stats");
        assert_eq!(stats.track_count, 1);
        assert_eq!(stats.linked_tracks, 1);
        assert_eq!(stats.artists, 1);
        assert_eq!(stats.unresolved_artists, 0); // Mapped!
        assert_eq!(stats.queued, 1);
        assert_eq!(stats.approved_queue, 0);

        // 3. Turso modifies the database (approve queue item, add new file)
        {
            let conn = store.connect().expect("Turso connect error");
            conn.execute("UPDATE queue SET approved = 1 WHERE id = 'q1'", ())
                .await
                .expect("Turso update failed");
            let new_meta = json!({"artist": "Four Tet", "album_artist": "Four Tet", "album": "Three", "title": "Loved"});
            conn.execute(
                "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES ('/Music/DJ/FourTet/01.flac', '/Music/DJ', 3000, 4000, ?, 1)",
                (serde_json::to_string(&new_meta).unwrap().as_str(),),
            ).await.expect("Turso insert failed");
        }
        drop(store);

        // 4. Python reads back the changes made by Turso
        let verify_script = format!(
            r#"
import sqlite3
with sqlite3.connect('{db}') as db:
    q = db.execute("SELECT approved FROM queue WHERE id = 'q1'").fetchone()
    assert q[0] == 1, f"Expected approved=1, got {{q[0]}}"
    f = db.execute("SELECT path FROM local_files WHERE path = '/Music/DJ/FourTet/01.flac'").fetchone()
    assert f is not None, "Turso-inserted file not found by Python"
"#,
            db = db_path.display()
        );

        let verify_status = std::process::Command::new("python3")
            .arg("-c")
            .arg(&verify_script)
            .status()
            .expect("Failed to execute Python verification script");
        assert!(
            verify_status.success(),
            "Python verification of Turso writes failed"
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_turso_get_link_rows() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_link_rows_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("links_test.sqlite3");
        let store = TursoDb::open(&db_path)
            .await
            .expect("Failed to open TursoDb");
        let conn = store.connect().expect("Failed to connect");

        store
            .add_root("/Music/FLAC")
            .await
            .expect("Failed to add root");

        // 1. Linked file
        let meta1 = json!({
            "artist": "Queen",
            "album": "A Night at the Opera",
            "title": "Bohemian Rhapsody",
            "track_number": 11,
            "track_total": 12,
            "disc_number": 1,
            "disc_total": 1,
            "bpm": 72.0,
            "initial_key": "Bb"
        });
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES ('/Music/FLAC/Queen/Bohemian.flac', '/Music/FLAC', 1000, 1000, ?, 1)",
            (serde_json::to_string(&meta1).unwrap().as_str(),),
        ).await.expect("Insert 1 failed");
        let link_payload1 = json!({
            "ids": { "track_id": "1001", "album_id": "5001" },
            "status": "linked",
            "catalogue_note": "Exact match verified"
        });
        conn.execute(
            "INSERT INTO track_links (path, market, stamp, payload) VALUES ('/Music/FLAC/Queen/Bohemian.flac', 'GB', '[0,0,1000,1000]', ?)",
            (serde_json::to_string(&link_payload1).unwrap().as_str(),),
        ).await.expect("Insert link 1 failed");

        // 2. Choice required file
        let meta2 = json!({
            "artist": "Queen",
            "album": "A Night at the Opera",
            "title": "You're My Best Friend",
            "track_number": 4,
            "track_total": 12
        });
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES ('/Music/FLAC/Queen/BestFriend.flac', '/Music/FLAC', 2000, 2000, ?, 1)",
            (serde_json::to_string(&meta2).unwrap().as_str(),),
        ).await.expect("Insert 2 failed");
        let link_payload2 = json!({
            "catalogue_options": [{ "id": 1 }, { "id": 2 }],
            "status": "review",
            "note": "Multiple release candidates"
        });
        conn.execute(
            "INSERT INTO track_links (path, market, stamp, payload) VALUES ('/Music/FLAC/Queen/BestFriend.flac', 'GB', '[]', ?)",
            (serde_json::to_string(&link_payload2).unwrap().as_str(),),
        ).await.expect("Insert link 2 failed");

        // 3. Unlinked file
        let meta3 = json!({
            "artist": "Queen",
            "album": "A Night at the Opera",
            "title": "'39",
            "track_number": 5,
            "track_total": 12
        });
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES ('/Music/FLAC/Queen/39.flac', '/Music/FLAC', 3000, 3000, ?, 1)",
            (serde_json::to_string(&meta3).unwrap().as_str(),),
        ).await.expect("Insert 3 failed");

        // 4. Ignored file
        let meta4 = json!({
            "artist": "Queen",
            "album": "A Night at the Opera",
            "title": "Love of My Life",
            "track_number": 9,
            "track_total": 12
        });
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES ('/Music/FLAC/Queen/LoveOfMyLife.flac', '/Music/FLAC', 4000, 4000, ?, 1)",
            (serde_json::to_string(&meta4).unwrap().as_str(),),
        ).await.expect("Insert 4 failed");
        conn.execute(
            "INSERT INTO ignored_local_files (path, ignored_at) VALUES ('/Music/FLAC/Queen/LoveOfMyLife.flac', '2026-01-01')",
            (),
        ).await.expect("Insert ignore failed");

        // Test filter: all
        let all_page = store
            .get_link_rows("GB", "/Music/FLAC", Some("all"), None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(all_page.total, 4);

        // Test filter: linked
        let linked_page = store
            .get_link_rows("GB", "/Music/FLAC", Some("linked"), None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(linked_page.total, 1);
        assert_eq!(linked_page.rows[0].title, "Bohemian Rhapsody");
        assert_eq!(linked_page.rows[0].status, "Linked");
        assert_eq!(linked_page.rows[0].online_id, "5001");
        assert_eq!(linked_page.rows[0].position, "Disc 01/01 · Track 11/12");
        assert_eq!(linked_page.rows[0].bpm, Some(72.0));
        assert_eq!(linked_page.rows[0].key.as_deref(), Some("Bb"));

        // Test filter: choice
        let choice_page = store
            .get_link_rows("GB", "/Music/FLAC", Some("choice"), None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(choice_page.total, 1);
        assert_eq!(choice_page.rows[0].title, "You're My Best Friend");
        assert_eq!(choice_page.rows[0].status, "Needs choice");
        assert_eq!(choice_page.rows[0].candidates, 2);

        // Test filter: unlinked (all unlinked tracks including those needing choice)
        let unlinked_page = store
            .get_link_rows(
                "GB",
                "/Music/FLAC",
                Some("unlinked"),
                None,
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(unlinked_page.total, 2);

        // Test filter: ignored
        let ignored_page = store
            .get_link_rows(
                "GB",
                "/Music/FLAC",
                Some("ignored"),
                None,
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(ignored_page.total, 1);
        assert_eq!(ignored_page.rows[0].title, "Love of My Life");
        assert!(ignored_page.rows[0].ignored);

        // Test search
        let search_page = store
            .get_link_rows(
                "GB",
                "/Music/FLAC",
                None,
                Some("Bohemian"),
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(search_page.total, 1);
        assert_eq!(search_page.rows[0].title, "Bohemian Rhapsody");

        // Test sorting by title asc
        let sort_page = store
            .get_link_rows(
                "GB",
                "/Music/FLAC",
                None,
                None,
                Some("title"),
                Some("asc"),
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(sort_page.rows[0].title, "'39");

        // Test pagination
        let paged = store
            .get_link_rows("GB", "/Music/FLAC", None, None, None, None, 1, 2)
            .await
            .unwrap();
        assert_eq!(paged.total, 4);
        assert_eq!(paged.rows.len(), 2);
        assert_eq!(paged.offset, 1);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_turso_get_missing_rows() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_missing_rows_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("missing_test.sqlite3");
        let store = TursoDb::open(&db_path)
            .await
            .expect("Failed to open TursoDb");
        let conn = store.connect().expect("Failed to connect");

        // Seed catalogue for Queen
        let cat_payload = json!({
            "name": "Queen",
            "id": "queen_id",
            "releases": [
                {
                    "id": "rel_1",
                    "title": "A Night at the Opera",
                    "artist": "Queen",
                    "date": "1975-11-21",
                    "type": "ALBUM",
                    "track_count": 2,
                    "available": true,
                    "tracks_loaded": true,
                    "tracks": [
                        { "id": "t1", "title": "Death on Two Legs", "disc_number": 1, "track_number": 1 },
                        { "id": "t2", "title": "Lazing on a Sunday Afternoon", "disc_number": 1, "track_number": 2 }
                    ]
                },
                {
                    "id": "rel_2",
                    "title": "News of the World",
                    "artist": "Queen",
                    "date": "1977-10-28",
                    "type": "ALBUM",
                    "track_count": 2,
                    "available": true,
                    "tracks": [
                        { "id": "t3", "title": "We Will Rock You", "disc_number": 1, "track_number": 1 },
                        { "id": "t4", "title": "We Are the Champions", "disc_number": 1, "track_number": 2 }
                    ]
                },
                {
                    "id": "rel_3",
                    "title": "The Game",
                    "artist": "Queen",
                    "date": "1980-06-30",
                    "type": "ALBUM",
                    "track_count": 10,
                    "available": true,
                    "tracks": []
                }
            ]
        });

        conn.execute(
            "INSERT INTO catalogue (artist_id, market, payload, fetched) VALUES ('queen_id', 'GB', ?, '2026-01-01')",
            (serde_json::to_string(&cat_payload).unwrap().as_str(),),
        ).await.expect("Insert catalogue failed");

        // Mark rel_1 as owned via track_links (2 tracks)
        for i in 1..=2 {
            store.apply_file_update(&format!("/path/{i}"),&format!("/path/{i}"),"/path",&json!({}),1,1).await.unwrap();
            let p = json!({ "ids": { "track_id": format!("t{}", i), "album_id": "rel_1" } });
            conn.execute(
                "INSERT INTO track_links (path, market, stamp, payload) VALUES (?, 'GB', '[0,0,1,1]', ?)",
                (
                    format!("/path/{}", i).as_str(),
                    serde_json::to_string(&p).unwrap().as_str(),
                ),
            )
            .await
            .expect("Insert track link failed");
        }

        // Mark rel_2 as queued in queue table
        conn.execute(
            "INSERT INTO queue (id, payload, approved, decision, updated) VALUES ('rel_2', '{}', 1, 'queued', '2026-01-01')",
            (),
        ).await.expect("Insert queue failed");

        // Query missing rows
        conn.execute("INSERT OR IGNORE INTO mappings(artist,tidal_id,status) SELECT artist_id,artist_id,'confirmed' FROM catalogue", ()).await.unwrap();
        // Cached search candidates must not become artists in Missing releases.
        let unrelated = json!({"name":"Alabama","releases":[{"id":"unrelated","title":"Unrelated album","artist":"Alabama","date":"2020-01-01","type":"ALBUM","track_count":1}]});
        conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES ('unmapped-search','GB',?)", (unrelated.to_string(),)).await.unwrap();
        let page = store
            .get_missing_rows("GB", None, None, None, None, None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(page.total, 3);

        // Filter: Queued
        let queued_page = store
            .get_missing_rows(
                "GB",
                None,
                None,
                Some("Queued"),
                None,
                None,
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(queued_page.total, 1);
        assert_eq!(queued_page.rows[0].release, "News of the World");
        assert!(queued_page.rows[0].approved);

        // Filter: Owned complete
        let owned_page = store
            .get_missing_rows(
                "GB",
                None,
                None,
                Some("Owned complete"),
                None,
                None,
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(owned_page.total, 1);
        assert_eq!(owned_page.rows[0].release, "A Night at the Opera");
        assert_eq!(owned_page.rows[0].children.len(), 2);
        assert_eq!(owned_page.rows[0].children[0].title, "Death on Two Legs");

        // Filter: Missing release
        let missing_page = store
            .get_missing_rows(
                "GB",
                None,
                None,
                Some("Missing release"),
                None,
                None,
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(missing_page.total, 1);
        assert_eq!(missing_page.rows[0].release, "The Game");

        // Search
        let search_page = store
            .get_missing_rows(
                "GB",
                None,
                None,
                None,
                None,
                Some("News"),
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(search_page.total, 1);
        assert_eq!(search_page.rows[0].release, "News of the World");

        // Sort by date desc (default) -> The Game (1980), News of the World (1977), A Night at the Opera (1975)
        assert_eq!(page.rows[0].release, "The Game");
        assert_eq!(page.rows[1].release, "News of the World");
        assert_eq!(page.rows[2].release, "A Night at the Opera");

        // Duplicate copies of one recording must not count as two owned tracks.
        conn.execute("UPDATE track_links SET payload=? WHERE path='/path/2'", (json!({"status":"linked","ids":{"album_id":"rel_1","track_id":"t1"}}).to_string(),)).await.unwrap();
        let rows = store.build_missing_rows("GB").await.unwrap();
        assert_eq!(rows.iter().find(|r| r.id == "rel_1").unwrap().status,"Owned partial");
        // A removed local file must not retain ownership through a stale link.
        conn.execute("UPDATE local_files SET present=0", ()).await.unwrap();
        let rows = store.build_missing_rows("GB").await.unwrap();
        assert_eq!(rows.iter().find(|r| r.id == "rel_1").unwrap().status,"Missing release");
        conn.execute("DELETE FROM track_links", ()).await.unwrap();
        for (path,number) in [("/path/1",1),("/path/2",4)] {
            store.apply_file_update(path,path,"/path",&json!({"albumartist":"Queen","album":"A Night at the Opera","date":"1975","tracknumber":number.to_string(),"tracktotal":"1","discnumber":"1","disctotal":"1"}),2,2).await.unwrap();
        }
        let rows = store.build_missing_rows("GB").await.unwrap();
        assert_eq!(rows.iter().find(|r| r.id == "rel_1").unwrap().status,"Owned partial", "Impossible totals must not imply a complete local edition");
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_linked_artist_ids_and_apply_update() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_artist_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("artist_test.sqlite3");
        let store = TursoDb::open(&db_path)
            .await
            .expect("Failed to open TursoDb");
        let conn = store.connect().expect("Failed to connect");

        // Seed mappings
        conn.execute(
            "INSERT INTO mappings (artist, tidal_id, status) VALUES ('Queen', '1234', 'confirmed')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO mappings (artist, tidal_id, status) VALUES ('David Bowie', '5678', 'auto')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO mappings (artist, tidal_id, status) VALUES ('Various Artists', '9999', 'confirmed')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO mappings (artist, tidal_id, status) VALUES ('Unlinked', '0000', 'unresolved')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO additional_mappings (artist, tidal_id) VALUES ('Queen', '4321')",
            (),
        )
        .await
        .unwrap();

        let ids = store.get_linked_artist_ids().await.unwrap();
        assert_eq!(ids, vec!["1234", "4321", "5678"]);

        // Test apply_file_update
        let meta = json!({ "title": "Under Pressure", "artist": "Queen" });
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES ('/old/path.flac', '/root', 100, 100, '{}', 1)",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO track_links (path, market, stamp, payload) VALUES ('/old/path.flac', 'GB', '[]', '{}')",
            (),
        ).await.unwrap();

        store
            .apply_file_update("/old/path.flac", "/new/path.flac", "/root", &meta, 200, 200)
            .await
            .unwrap();

        // Verify old file marked not present
        let mut old_stmt = conn
            .query(
                "SELECT present FROM local_files WHERE path = '/old/path.flac'",
                (),
            )
            .await
            .unwrap();
        let old_present: i64 = old_stmt.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(old_present, 0);

        // Verify new file inserted with updated metadata
        let mut new_stmt = conn
            .query(
                "SELECT present, size, metadata FROM local_files WHERE path = '/new/path.flac'",
                (),
            )
            .await
            .unwrap();
        let row = new_stmt.next().await.unwrap().unwrap();
        let new_present: i64 = row.get(0).unwrap();
        let new_size: i64 = row.get(1).unwrap();
        let new_meta_str: String = row.get(2).unwrap();
        assert_eq!(new_present, 1);
        assert_eq!(new_size, 200);
        assert!(new_meta_str.contains("Under Pressure"));

        // Verify track_links moved to new path
        let mut link_stmt = conn
            .query(
                "SELECT path FROM track_links WHERE path = '/new/path.flac'",
                (),
            )
            .await
            .unwrap();
        assert!(link_stmt.next().await.unwrap().is_some());

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_state_settings_and_queue_flow() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_state_queue_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("state_queue_test.sqlite3");
        let store = TursoDb::open(&db_path)
            .await
            .expect("Failed to open TursoDb");

        // 1. Settings
        let initial_settings = store.get_settings().await.unwrap();
        assert_eq!(initial_settings["general"]["market"], "GB");

        let updated = store
            .save_settings("ui", &json!({ "market": "US", "theme": "light" }))
            .await
            .unwrap();
        assert_eq!(updated["general"]["market"], "US");
        assert_eq!(updated["general"]["theme"], "light");

        // 2. Queue add & query
        let conn = store.connect().unwrap();
        let cat_payload = json!({
            "name": "Radiohead",
            "releases": [{
                "id": "album_100",
                "title": "Kid A",
                "artist": "Radiohead",
                "date": "2000-10-02",
                "type": "ALBUM",
                "track_count": 10,
                "tracks": [
                    { "id": "t1", "title": "Everything in Its Right Place", "track_number": "1", "disc_number": 1 },
                    { "id": "t2", "title": "Kid A", "track_number": "2", "disc_number": 1 }
                ]
            }]
        });
        conn.execute(
            "INSERT INTO catalogue (artist_id, market, payload) VALUES ('art_1', 'US', ?)",
            (serde_json::to_string(&cat_payload).unwrap().as_str(),),
        )
        .await
        .unwrap();

        let mut sel = HashMap::new();
        sel.insert("album_100".to_string(), None);
        store.queue_add(&sel).await.unwrap();
        assert!(store
            .queue_add(&HashMap::from([(
                "album_100".into(),
                Some(vec!["unknown".into()])
            )]))
            .await
            .is_err());

        let queue_rows = store
            .get_queue_rows("queue", None, None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(queue_rows.total, 1);
        assert_eq!(queue_rows.rows[0].release, "Kid A");
        assert!(queue_rows.rows[0].approved);

        // 3. Queue export
        let json_export = store.queue_export("json", None).await.unwrap();
        assert!(json_export.contains("Kid A"));

        let csv_export = store.queue_export("csv", None).await.unwrap();
        assert!(csv_export.contains("Kid A"));
        assert!(csv_export.contains("Radiohead"));

        // 4. Queue select specific tracks
        let mut select_tracks = HashMap::new();
        select_tracks.insert("album_100".to_string(), Some(vec!["t1".to_string()]));
        store.queue_select(&select_tracks).await.unwrap();

        let updated_queue = store
            .get_queue_rows("queue", None, None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(updated_queue.rows[0].selected, Some(vec!["t1".to_string()]));

        // 5. State
        let state = store.get_state(None, &[], None).await.unwrap();
        assert_eq!(state["stats"]["queued"], 1);
        assert_eq!(state["stats"]["approved_queue"], 1);

        // 6. Queue decision mark downloaded
        store
            .queue_decision(&["album_100".to_string()], "downloaded")
            .await
            .unwrap();
        store
            .queue_redownload("album_100", Some("t2"))
            .await
            .unwrap();
        let requeued = store
            .get_queue_rows("queue", None, None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(requeued.total, 1);
        assert_eq!(requeued.rows[0].selected, Some(vec!["t2".to_string()]));
        store
            .queue_decision(&["album_100".to_string()], "downloaded")
            .await
            .unwrap();
        let queue_after = store
            .get_queue_rows("queue", None, None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(queue_after.total, 0);

        let dl_rows = store
            .get_queue_rows("downloaded", None, None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(dl_rows.total, 1);
        assert_eq!(dl_rows.rows[0].release, "Kid A");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_clean_evidence_str() {
        assert_eq!(TursoDb::clean_evidence_str(""), "");
        assert_eq!(
            TursoDb::clean_evidence_str("\"6 releases and 48 track titles/identifiers match\""),
            "6 releases and 48 track titles/identifiers match"
        );
        assert_eq!(
            TursoDb::clean_evidence_str("\"6 local release titles match; track details not checked \\u2014 confirm identity\""),
            "6 local release titles match; track details not checked — confirm identity"
        );
        assert_eq!(
            TursoDb::clean_evidence_str("12 local \\u00b7 4 online"),
            "12 local · 4 online"
        );
    }

    #[tokio::test]
    async fn test_artist_rows_filters_and_sorting() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_artist_filters_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("artist_filters.sqlite3");
        let store = TursoDb::open(&db_path).await.unwrap();
        let conn = store.connect().unwrap();

        // Seed files for artists
        let artists = [
            ("The Beatles", "Abbey Road", "/music/beatles/01.flac"),
            ("The Beatles", "Let It Be", "/music/beatles/02.flac"),
            ("Radiohead", "OK Computer", "/music/radiohead/01.flac"),
            ("Pink Floyd", "The Wall", "/music/pinkfloyd/01.flac"),
            ("Unknown Band", "Demo", "/music/unknown/01.flac"),
            ("Various Artists", "Compilation A", "/music/compilation/01.flac"),
            ("V.A.", "Compilation B", "/music/compilation/02.flac"),
        ];
        for (artist, album, path) in artists {
            let meta = json!({ "artist": artist, "album": album });
            conn.execute(
                "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES (?, '/music', 100, 100, ?, 1)",
                (path, serde_json::to_string(&meta).unwrap().as_str()),
            ).await.unwrap();
        }

        // Seed mappings
        conn.execute(
            "INSERT INTO mappings (artist, tidal_id, status, evidence) VALUES ('The Beatles', '123', 'confirmed', '\"Exact discography match\"')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO mappings (artist, tidal_id, status, evidence) VALUES ('Radiohead', '456', 'auto', '\"10 releases match \\u2014 confirm identity\"')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO mappings (artist, tidal_id, status, evidence) VALUES ('Pink Floyd', '789', 'ambiguous', '\"Multiple candidate matches\"')",
            (),
        ).await.unwrap();
        conn.execute("INSERT INTO mappings (artist, tidal_id, status) VALUES ('Various Artists', 'compilation-id', 'confirmed')", ()).await.unwrap();
        // Unknown Band has no mapping -> status = Unresolved

        // 1. All filter
        let all_page = store
            .get_artist_rows(None, Some("all"), None, Some("artist"), Some("asc"), 0, 10)
            .await
            .unwrap();
        assert_eq!(all_page.total, 4);
        // Hiding compilation placeholders must not delete indexed files or saved associations.
        let mut kept = conn.query("SELECT tidal_id FROM mappings WHERE artist='Various Artists'", ()).await.unwrap();
        assert_eq!(kept.next().await.unwrap().unwrap().get::<String>(0).unwrap(), "compilation-id");
        assert!(!store.get_linked_artist_ids().await.unwrap().contains(&"compilation-id".to_string()));


        // 2. Unresolved filter
        let unresolved_page = store
            .get_artist_rows(None, Some("unresolved"), None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(unresolved_page.total, 1);
        assert_eq!(unresolved_page.rows[0]["artist"], "Unknown Band");

        // 3. Matched filter (confirmed and auto with valid tidal_id)
        let matched_page = store
            .get_artist_rows(None, Some("matched"), None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(matched_page.total, 2); // The Beatles and Radiohead

        // 4. Review filter (candidate/ambiguous or confirm identity)
        let review_page = store
            .get_artist_rows(None, Some("review"), None, None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(review_page.total, 2); // Radiohead (confirm identity) and Pink Floyd (ambiguous)

        // 5. Search
        let search_page = store
            .get_artist_rows(None, None, Some("beatles"), None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(search_page.total, 1);
        assert_eq!(search_page.rows[0]["artist"], "The Beatles");
        assert_eq!(search_page.rows[0]["evidence"], "Exact discography match");

        // 6. Sort tracks desc
        let sort_page = store
            .get_artist_rows(None, None, None, Some("tracks"), Some("desc"), 0, 10)
            .await
            .unwrap();
        assert_eq!(sort_page.rows[0]["artist"], "The Beatles");
        assert_eq!(sort_page.rows[0]["tracks"], 2);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_missing_rows_future_and_deduplication() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_missing_dedup_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("missing_dedup.sqlite3");
        let store = TursoDb::open(&db_path).await.unwrap();
        let conn = store.connect().unwrap();

        let cat_payload1 = json!({
            "name": "Asketa",
            "releases": [
                // Shared release between Asketa and Natan Chaim
                {
                    "id": "rel_shared",
                    "title": "Overdrive",
                    "artist": "Asketa",
                    "date": "2024-01-05",
                    "type": "SINGLE",
                    "track_count": 1,
                    "tracks": []
                },
                // Future release - should be excluded
                {
                    "id": "rel_future",
                    "title": "Future Track",
                    "artist": "Asketa",
                    "date": "2099-01-01",
                    "type": "SINGLE",
                    "track_count": 1,
                    "tracks": []
                }
            ]
        });
        let cat_payload2 = json!({
            "name": "Natan Chaim",
            "releases": [
                {
                    "id": "rel_shared",
                    "title": "Overdrive",
                    "artist": "Natan Chaim",
                    "date": "2024-01-05",
                    "type": "SINGLE",
                    "track_count": 1,
                    "tracks": []
                }
            ]
        });

        conn.execute(
            "INSERT INTO catalogue (artist_id, market, payload) VALUES ('asketa_id', 'GB', ?)",
            (serde_json::to_string(&cat_payload1).unwrap().as_str(),),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO catalogue (artist_id, market, payload) VALUES ('natan_id', 'GB', ?)",
            (serde_json::to_string(&cat_payload2).unwrap().as_str(),),
        )
        .await
        .unwrap();

        conn.execute("INSERT OR IGNORE INTO mappings(artist,tidal_id,status) SELECT artist_id,artist_id,'confirmed' FROM catalogue", ()).await.unwrap();
        let page = store
            .get_missing_rows("GB", None, None, None, None, None, None, None, 0, 10)
            .await
            .unwrap();
        // The future release is skipped, and the shared release is merged into 1 row at the album artist level!
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].artist, "Asketa & Natan Chaim");
        assert_eq!(page.rows[0].release, "Overdrive");
        assert_eq!(page.rows[0].recommendation, "Potential"); // Real badge, not "All recommendations"!

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn complete_local_standard_is_not_recommended_as_missing_deluxe() {
        let temp_dir =
            std::env::temp_dir().join(format!("tibrary_editions_{}", uuid::Uuid::new_v4()));
        let store = TursoDb::open(temp_dir.join("library.sqlite3"))
            .await
            .unwrap();
        let conn = store.connect().unwrap();
        for (number, title) in [(1, "Unbound"), (2, "Sequoia")] {
            let path = format!("/music/Klur/Unbound (2025)/0{number} - {title}.flac");
            let meta = json!({"album_artist":"Klur","artist":"Klur","album":"Unbound","title":title,"tracknumber":format!("{number}/2"),"tracktotal":"2","date":"2025-01-01"});
            conn.execute(
                "INSERT INTO local_files (path,root,metadata,present) VALUES (?, '/music', ?, 1)",
                (path, meta.to_string()),
            )
            .await
            .unwrap();
        }
        let catalogue = json!({"name":"Klur","releases":[{"id":"deluxe","artist":"Klur","title":"Unbound (Deluxe)","date":"2025-01-01","type":"ALBUM","track_count":3,"tracks":[]}]});
        conn.execute(
            "INSERT INTO catalogue (artist_id,market,payload) VALUES ('klur','GB',?)",
            (catalogue.to_string(),),
        )
        .await
        .unwrap();
        conn.execute("INSERT INTO mappings(artist,tidal_id,status) VALUES ('Klur','klur','confirmed')", ()).await.unwrap();
        let all = store
            .get_missing_rows(
                "GB",
                Some("All releases"),
                None,
                None,
                None,
                None,
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(all.rows[0].status, "Owned alternate edition");
        let missing = store
            .get_missing_rows(
                "GB",
                Some("All missing releases"),
                None,
                None,
                None,
                None,
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(missing.total, 0);
        store
            .set_preference("desktop", &json!({"treat_editions_as_owned":false}))
            .await
            .unwrap();
        store.invalidate_missing_rows();
        let reconsidered = store
            .get_missing_rows(
                "GB",
                Some("All releases"),
                None,
                None,
                None,
                None,
                None,
                None,
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(reconsidered.rows[0].status, "Owned partial");
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn unresolved_artist_detail_accepts_null_mapping_and_legacy_candidates() {
        let temp = std::env::temp_dir().join(format!("artist_detail_{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(temp.join("db")).await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO mappings(artist,tidal_id,status) VALUES('Sad Alex',NULL,'review')", ()).await.unwrap();
        conn.execute("INSERT INTO match_reviews(artist,payload) VALUES('Sad Alex',?)", (json!({"candidates":[{"artist":{"id":"8790541","name":"Sad Alex"}}]}).to_string(),)).await.unwrap();
        let detail = db.get_detail(&json!({"artist":"Sad Alex"})).await.unwrap();
        assert_eq!(detail["ids"], json!([]));
        assert_eq!(detail["review"]["candidates"][0]["id"], "8790541");
        let _ = std::fs::remove_dir_all(temp);
    }

    #[tokio::test]
    async fn favourites_use_album_artist_and_filter_pagination() {
        let temp_dir =
            std::env::temp_dir().join(format!("tibrary_favourites_{}", uuid::Uuid::new_v4()));
        let store = TursoDb::open(temp_dir.join("library.sqlite3"))
            .await
            .unwrap();
        let conn = store.connect().unwrap();
        let meta =
            json!({"album_artist":"North Assembly","artist":"Guest Artist","album":"Blue Hours"});
        conn.execute("INSERT INTO local_files (path,root,metadata,present) VALUES ('/music/song.flac','/music',?,1)", (meta.to_string(),)).await.unwrap();
        let cached =
            json!([{"id":"101","name":"North Assembly"},{"id":"102","name":"Away Artist"}]);
        conn.execute(
            "INSERT INTO favourite_artists (cache_id,payload) VALUES ('user',?)",
            (cached.to_string(),),
        )
        .await
        .unwrap();
        let local = store
            .get_favourite_rows(
                Some("/music"),
                "In library",
                "North",
                "tracks",
                "desc",
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(local.total, 1);
        assert_eq!(local.rows[0]["tracks"], 1);
        let missing = store
            .get_favourite_rows(
                Some("/music"),
                "Missing locally",
                "",
                "artist",
                "asc",
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(missing.total, 1);
        assert_eq!(missing.rows[0]["artist"], "Away Artist");
        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
