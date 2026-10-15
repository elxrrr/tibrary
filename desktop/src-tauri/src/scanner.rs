use crate::db::TursoDb;
use lofty::{
    config::{ParseOptions, ParsingMode},
    file::{AudioFile, TaggedFileExt},
    flac::FlacFile,
    probe::Probe,
    tag::ItemKey,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub(crate) const AUDIO_EXTS: &[&str] = &[
    "flac", "m4a", "mp4", "alac", "mp3", "aif", "aiff", "aifc", "wav", "wave",
];

type PendingWrite = (String, String, i64, i64, Option<String>, Option<String>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioMetadata {
    pub artist: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    pub album: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub musical_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracktotal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discnumber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disctotal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub releasetype: Option<String>,
    pub duration: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u8>,
    #[serde(default)]
    pub audio_properties_version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copyright: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tidal_track_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tidal_album_id: Option<String>,
    pub artist_grouping_version: i64,
    pub tags: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScanSummary {
    pub read: usize,
    pub unchanged: usize,
    pub errors: usize,
    pub missing: usize,
    pub removed: usize,
    pub restored: usize,
    pub properties_refreshed: usize,
    pub status: String,
}

fn mtime_ns(m: &fs::Metadata) -> i64 {
    match m.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos().min(i64::MAX as u128) as i64,
            Err(e) => -(e.duration().as_nanos().min(i64::MAX as u128) as i64),
        },
        Err(_) => 0,
    }
}

pub fn read_audio_metadata(path: &Path) -> Result<AudioMetadata, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let mut tags_map: HashMap<String, Vec<String>> = HashMap::new();
    let duration: f64;
    let (sample_rate, bit_depth);

    if ext == "flac" {
        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        let options = ParseOptions::new()
            .implicit_conversions(false)
            .parsing_mode(ParsingMode::Strict)
            .read_cover_art(false);
        let audio = FlacFile::read_from(&mut file, options).map_err(|e| e.to_string())?;
        duration = (audio.properties().duration().as_secs_f64() * 1000.0).round() / 1000.0;
        sample_rate = Some(audio.properties().sample_rate());
        bit_depth = Some(audio.properties().bit_depth());
        if let Some(comments) = audio.vorbis_comments() {
            for (k, v) in comments.items() {
                tags_map
                    .entry(k.to_ascii_lowercase())
                    .or_default()
                    .push(v.to_string());
            }
        }
    } else {
        let options = ParseOptions::new()
            .implicit_conversions(false)
            .parsing_mode(ParsingMode::Strict)
            .read_cover_art(false);
        let audio = Probe::open(path)
            .map_err(|e| e.to_string())?
            .options(options)
            .read()
            .map_err(|e| e.to_string())?;
        duration = (audio.properties().duration().as_secs_f64() * 1000.0).round() / 1000.0;
        sample_rate = audio.properties().sample_rate();
        bit_depth = audio.properties().bit_depth();
        if let Some(tag) = audio.primary_tag().or_else(|| audio.first_tag()) {
            for item in tag.items() {
                let mapped = if item.key() == ItemKey::IntegerBpm {
                    Some("bpm".to_string())
                } else {
                    item.key()
                        .map_key(lofty::tag::TagType::VorbisComments)
                        .map(|k| k.to_ascii_lowercase())
                };
                if let Some(key) = mapped {
                    if let Some(value) = item.value().text().or_else(|| item.value().locator()) {
                        tags_map.entry(key).or_default().push(value.to_string());
                    }
                }
            }
        }
        // The generic tag model has no provider-ID keys. Read these MP4
        // freeform atoms explicitly so AAC downloads retain release links.
        if matches!(ext.as_str(), "m4a" | "mp4" | "alac") {
            use lofty::mp4::{AtomData, AtomIdent, Mp4File};
            let mut file = fs::File::open(path).map_err(|error|error.to_string())?;
            let native = Mp4File::read_from(&mut file, options).map_err(|error|error.to_string())?;
            if let Some(tag) = native.ilst() {
                for name in ["TIDAL_TRACK_ID", "TIDAL_ALBUM_ID"] {
                    let key = AtomIdent::Freeform { mean:"com.apple.iTunes".into(), name:name.into() };
                    if let Some(atom) = tag.get(&key) {
                        for value in atom.data() {
                            if let AtomData::UTF8(value) | AtomData::UTF16(value) = value {
                                tags_map.entry(name.to_ascii_lowercase()).or_default().push(value.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    let get_first = |keys: &[&str]| -> Option<String> {
        for &k in keys {
            if let Some(vals) = tags_map.get(k) {
                if let Some(first) = vals.iter().find(|s| !s.trim().is_empty()) {
                    return Some(first.trim().to_string());
                }
            }
        }
        None
    };

    let album_artist = get_first(&["albumartist", "tpe2", "aart"]);
    let track_artist = get_first(&["artist", "tpe1"]);
    let artist = album_artist
        .clone()
        .or_else(|| track_artist.clone())
        .unwrap_or_else(|| "Unknown artist".to_string());

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown title")
        .to_string();
    let title = get_first(&["title", "tit2"]).unwrap_or(file_stem);
    let album = get_first(&["album", "talb"]).unwrap_or_else(|| "Unknown release".to_string());

    Ok(AudioMetadata {
        artist,
        track_artist,
        album_artist,
        album,
        title,
        date: get_first(&["date", "tdrc", "year"]),
        isrc: get_first(&["isrc", "tsrc"]),
        bpm: get_first(&["bpm", "tempo", "tbpm", "tmpo"]),
        musical_key: get_first(&["key", "initialkey", "tkey"]),
        track: get_first(&["tracknumber", "trck"]),
        tracktotal: get_first(&["tracktotal", "totaltracks"]),
        discnumber: get_first(&["discnumber", "tpos"]),
        disctotal: get_first(&["disctotal", "totaldiscs"]),
        label: get_first(&["label", "organization", "publisher", "tpub"]),
        releasetype: get_first(&["releasetype", "musicbrainz album type"]),
        duration,
        sample_rate,
        bit_depth,
        audio_properties_version: 1,
        copyright: get_first(&["copyright", "tcop"]),
        tidal_track_id: get_first(&["tidal_track_id", "tidaltrackid"]),
        tidal_album_id: get_first(&["tidal_album_id", "tidalalbumid"]),
        artist_grouping_version: 3,
        tags: tags_map.clone(),
    })
}

pub async fn scan_library(
    db: &TursoDb,
    root: impl AsRef<Path>,
    cancelled: Arc<AtomicBool>,
    progress: impl Fn(&str) + Send + Sync,
) -> Result<ScanSummary, String> {
    scan_library_with_options(db, root, cancelled, false, progress).await
}

pub async fn scan_library_with_options(
    db: &TursoDb,
    root: impl AsRef<Path>,
    cancelled: Arc<AtomicBool>,
    force: bool,
    progress: impl Fn(&str) + Send + Sync,
) -> Result<ScanSummary, String> {
    let root_path = root.as_ref().to_path_buf();
    let root_str = root_path
        .to_str()
        .ok_or_else(|| "Invalid UTF-8 in root path".to_string())?
        .to_string();

    if !root_path.is_dir() {
        return Err("Library folder does not exist or is offline".to_string());
    }

    let original_library = crate::library_access::library_identity(&root_path)?;
    progress(&format!("Opening library · {}", root_str));
    let conn = db.connect()?;
    let _ = conn.execute("PRAGMA busy_timeout = 10000", ()).await;

    conn.execute(
        "INSERT OR IGNORE INTO roots (root, status) VALUES (?, 'scanning')",
        (root_str.as_str(),),
    )
    .await
    .map_err(|e| e.to_string())?;

    // Load existing cached files for this root
    let mut cached_rows = conn
        .query(
            "SELECT path, size, mtime, metadata, error, present FROM local_files WHERE root = ?",
            (root_str.as_str(),),
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut cached_files: HashMap<String, (i64, i64, bool, bool)> = HashMap::new();
    let mut legacy_properties = HashMap::new();
    while let Some(row) = cached_rows.next().await.map_err(|e| e.to_string())? {
        let path: String = row.get(0).map_err(|e| e.to_string())?;
        let size: i64 = row.get(1).map_err(|e| e.to_string())?;
        let mtime: i64 = row.get(2).map_err(|e| e.to_string())?;
        let error_opt: Option<String> = row.get::<Option<String>>(4).unwrap_or(None);
        let saved_metadata: Option<String> = row.get::<Option<String>>(3).unwrap_or(None);
        let parsed_metadata = saved_metadata.as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .filter(serde_json::Value::is_object);
        let has_error = error_opt
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
            || parsed_metadata.is_none();
        let present: i64 = row.get(5).unwrap_or(1);
        if let Some(metadata) = parsed_metadata {
            if metadata["audio_properties_version"] != 1 {
                legacy_properties.insert(path.clone(), metadata);
            }
        }
        cached_files.insert(path, (size, mtime, has_error, present != 0));
    }

    progress(&format!(
        "Loaded {} cached files · traversing directory tree",
        cached_files.len()
    ));

    let allowed_exts: HashSet<&str> = AUDIO_EXTS.iter().copied().collect();
    let mut pending_dirs = vec![root_path.clone()];
    let mut paths = Vec::new();
    let mut last_report = Instant::now();
    // Enumerate once before reading tags, so the processing denominator is real.
    while let Some(dir) = pending_dirs.pop() {
        if cancelled.load(Ordering::Relaxed) {break;}
        let entries=fs::read_dir(&dir).map_err(|e|format!("Cannot read folder {}: {e}; missing-file status unchanged",dir.display()))?;
        for entry in entries {
            if cancelled.load(Ordering::Relaxed) {break;}
            let entry=entry.map_err(|e|format!("Folder enumeration failed: {e}"))?;
            let kind=entry.file_type().map_err(|e|e.to_string())?;
            if kind.is_symlink() {continue;}
            // Tag updates also stage individual files beside the destination.
            // They are never library tracks, even when a scan overlaps an edit.
            if entry.file_name().to_string_lossy().starts_with(".tibrary-") {continue;}
            if kind.is_dir() {
                pending_dirs.push(entry.path());
            } else if kind.is_file() && allowed_exts.contains(entry.path().extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase().as_str()) {paths.push(entry.path());}
        }
        if last_report.elapsed() >= Duration::from_millis(250) {
            progress(&format!("Discovering library · {} audio files found · {}",paths.len(),dir.display()));
            last_report=Instant::now();
            tokio::task::yield_now().await;
        }
    }
    let total=paths.len();
    progress(&format!("Reading local tags · 0/{total} files"));
    let mut seen_paths = HashSet::new();
    let mut summary = ScanSummary::default();
    let mut pending_writes: Vec<PendingWrite> = Vec::new();
    let mut restored_paths = Vec::new();
    if cancelled.load(Ordering::Relaxed) {summary.status="cancelled".into();}
    for (position,path) in paths.into_iter().enumerate() {
            if cancelled.load(Ordering::Relaxed) {summary.status="cancelled".into();break;}
            if last_report.elapsed() >= Duration::from_millis(250) {
                progress(&format!("Reading local tags · {position}/{total} files · {} read · {} unchanged",summary.read,summary.unchanged));
                last_report=Instant::now();
                tokio::task::yield_now().await;
            }
            let path_str=path.to_str().ok_or("Unsupported non-UTF8 music path")?.to_owned();
            seen_paths.insert(path_str.clone());
            let metadata=match fs::metadata(&path) {Ok(m)=>m,Err(_)=>{summary.errors+=1;continue;}};
            let size = metadata.len() as i64;
            let mtime = mtime_ns(&metadata);

            // Incremental check: if size and mtime match and no prior error, skip reading tags!
            if let Some(&(cached_size, cached_mtime, has_error, is_present)) =
                cached_files.get(&path_str)
            {
                if !force && cached_size == size && cached_mtime == mtime && !has_error {
                    summary.unchanged += 1;
                    // Older indices omitted audio quality. Fill this once from
                    // properties only, preserving every saved tag and link.
                    if let Some(mut saved) = legacy_properties.remove(&path_str) {
                        let source = path.clone();
                        let properties = tokio::task::spawn_blocking(move || {
                            let audio = Probe::open(&source).map_err(|error|error.to_string())?
                                .options(ParseOptions::new().read_tags(false).read_cover_art(false))
                                .read().map_err(|error|error.to_string())?;
                            Ok::<_,String>((audio.properties().sample_rate(), audio.properties().bit_depth()))
                        }).await.map_err(|error|error.to_string())?;
                        if let Ok((rate, depth)) = properties {
                            saved["sample_rate"] = serde_json::json!(rate);
                            saved["bit_depth"] = serde_json::json!(depth);
                            saved["audio_properties_version"] = serde_json::json!(1);
                            pending_writes.push((path_str.clone(), root_str.clone(), size, mtime, Some(saved.to_string()), None));
                            summary.properties_refreshed += 1;
                            if pending_writes.len() >= 64 {
                                crate::library_access::require_same_library(&root_path, &original_library)?;
                                flush_writes(db, &conn, &mut pending_writes).await?;
                            }
                        }
                    }
                    if !is_present {
                        restored_paths.push(path_str);
                    }
                    continue;
                }
            }

            // Needs tag read
            summary.read += 1;
            let read = tokio::task::spawn_blocking(move || read_audio_metadata(&path))
                .await
                .map_err(|e| format!("Tag reader stopped unexpectedly: {e}"))?;
            let (meta_json, err_str) = match read {
                Ok(m) => (serde_json::to_string(&m).ok(), None),
                Err(e) => (None, Some(e)),
            };

            if err_str.is_some() {
                summary.errors += 1;
            }

            pending_writes.push((path_str, root_str.clone(), size, mtime, meta_json, err_str));

            if pending_writes.len() >= 64 {
                crate::library_access::require_same_library(&root_path, &original_library)?;
                flush_writes(db, &conn, &mut pending_writes).await?;
            }

    }
    if summary.status != "cancelled" {progress(&format!("Reading local tags · {total}/{total} files · saving index"));}

    // Do not publish I/O errors caused by a drive disappearing mid-read.
    crate::library_access::require_same_library(&root_path, &original_library)?;
    // Flush any remaining writes
    if !pending_writes.is_empty() {
        flush_writes(db, &conn, &mut pending_writes).await?;
    }

    // Restore any files that reappeared
    for p in restored_paths {
        conn.execute(
            "UPDATE local_files SET present = 1 WHERE path = ?",
            (p.as_str(),),
        )
        .await
        .map_err(|e| e.to_string())?;
        summary.restored += 1;
        db.note_local_change();
    }

    if cancelled.load(Ordering::Relaxed) { summary.status = "cancelled".into(); }
    if summary.status != "cancelled" {
        // Publish missing-file decisions together, only while the same drive is present.
        crate::library_access::require_same_library(&root_path, &original_library)?;
        conn.execute("BEGIN IMMEDIATE", ()).await.map_err(|error| error.to_string())?;
        let reconcile: Result<bool, String> = async {
            for (path_str, (_, _, _, was_present)) in &cached_files {
                if cancelled.load(Ordering::Relaxed) { return Ok(false); }
                if !seen_paths.contains(path_str) {
                    summary.missing += 1;
                    if !was_present { continue; }
                    conn.execute("UPDATE local_files SET present = 0 WHERE path = ?", (path_str.as_str(),))
                        .await.map_err(|error|error.to_string())?;
                    summary.removed += 1;
                }
            }
            crate::library_access::require_same_library(&root_path, &original_library)?;
            if cancelled.load(Ordering::Relaxed) { return Ok(false); }
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute("UPDATE roots SET status = 'complete', scanned_at = ? WHERE root = ?", (now.as_str(), root_str.as_str()))
                .await.map_err(|error|error.to_string())?;
            Ok(true)
        }.await;
        match reconcile {
            Ok(true) => {
                if let Err(error) = conn.execute("COMMIT", ()).await {
                    let _ = conn.execute("ROLLBACK", ()).await;
                    return Err(error.to_string());
                }
                summary.status = "complete".into();
                if summary.removed > 0 { db.note_local_change(); }
            }
            Ok(false) => {
                conn.execute("ROLLBACK", ()).await.map_err(|error|error.to_string())?;
                summary.status = "cancelled".into();
                summary.missing = 0;
                summary.removed = 0;
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(error);
            }
        }
    }

    progress(&format!(
        "Scan {} · {} tags read · {} unchanged · {} audio properties cached · {} errors · {} missing",
        summary.status, summary.read, summary.unchanged, summary.properties_refreshed, summary.errors, summary.missing
    ));

    Ok(summary)
}

async fn flush_writes(
    db: &TursoDb,
    conn: &turso::Connection,
    pending: &mut Vec<PendingWrite>,
) -> Result<(), String> {
    if pending.is_empty() {
        return Ok(());
    }
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(|e| e.to_string())?;
    let result:Result<(),String> = async {
    for (path, root, size, mtime, meta, err) in pending.iter() {
        conn.execute(
            "INSERT OR REPLACE INTO local_files (path, root, size, mtime, metadata, error, present) VALUES (?, ?, ?, ?, ?, ?, 1)",
            (
                path.as_str(),
                root.as_str(),
                *size,
                *mtime,
                meta.as_deref().unwrap_or(""),
                err.as_deref().unwrap_or(""),
            ),
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
    }.await;
    if let Err(e) = result {
        let _ = conn.execute("ROLLBACK", ()).await;
        return Err(e);
    }
    conn.execute("COMMIT", ())
        .await
        .map_err(|e| e.to_string())?;
    // Invalidate every derived view as soon as an indexed batch commits. This
    // also covers cancellation/errors after a partially completed scan.
    db.note_local_change();
    pending.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn legacy_audio_properties_are_cached_once_without_changing_tags_or_links() {
        let temp = std::env::temp_dir().join(format!("tibrary-audio-properties-{}", uuid::Uuid::new_v4()));
        let root = temp.join("Music");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("track.flac");
        fs::write(&path, crate::stream_download::MINIMAL_FLAC).unwrap();
        let original = fs::read(&path).unwrap();
        let db = TursoDb::open(temp.join("db")).await.unwrap();
        let scan = || scan_library(&db,&root,Arc::new(AtomicBool::new(false)),|_|{});
        scan().await.unwrap();
        let mut metadata = db.get_local_files_page(None,10,0).await.unwrap().0[0].metadata.clone().unwrap();
        for key in ["sample_rate","bit_depth","audio_properties_version"] { metadata.as_object_mut().unwrap().remove(key); }
        // Cached tags are deliberately different: property repair must not reread them.
        metadata["title"] = serde_json::json!("Cached title");
        let conn = db.connect().unwrap();
        conn.execute("UPDATE local_files SET metadata=? WHERE path=?",(metadata.to_string(),path.to_string_lossy().to_string())).await.unwrap();
        db.save_track_link(path.to_str().unwrap(),"GB","fixture",r#"{"status":"linked","ids":{"album_id":"12","track_id":"13"}}"#).await.unwrap();
        let result = scan().await.unwrap();
        assert_eq!(result.read,0);
        assert_eq!(result.properties_refreshed,1);
        let metadata = db.get_local_files_page(None,10,0).await.unwrap().0[0].metadata.clone().unwrap();
        assert_eq!(metadata["title"],"Cached title");
        assert_eq!(metadata["sample_rate"],44_100);
        assert_eq!(metadata["bit_depth"],16);
        let mut links = conn.query("SELECT payload FROM track_links WHERE path=? AND market='GB'", (path.to_string_lossy().to_string(),)).await.unwrap();
        let linked: String = links.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&linked).unwrap()["ids"]["track_id"],"13");
        drop(links);
        assert_eq!(scan().await.unwrap().properties_refreshed,0);
        assert_eq!(fs::read(&path).unwrap(),original);
        drop(conn);drop(db);fs::remove_dir_all(temp).unwrap();
    }

    #[tokio::test]
    async fn drive_disconnection_or_replacement_during_scan_preserves_cached_inventory() {
        let temp = std::env::temp_dir().join(format!("tibrary-scan-disconnect-{}", uuid::Uuid::new_v4()));
        let root = temp.join("drive");
        let unplugged = temp.join("unplugged");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("track.flac");
        fs::write(&file, crate::stream_download::MINIMAL_FLAC).unwrap();
        let db = TursoDb::open(temp.join("index.sqlite3")).await.unwrap();
        scan_library(&db, &root, Arc::new(AtomicBool::new(false)), |_|{}).await.unwrap();
        let revision = db.local_revision.load(Ordering::SeqCst);
        for replace_mount in [false, true] {
            let result = scan_library(&db, &root, Arc::new(AtomicBool::new(false)), |message| {
                if message.starts_with("Reading local tags · 0/") {
                    fs::rename(&root, &unplugged).unwrap();
                    if replace_mount { fs::create_dir(&root).unwrap(); }
                }
            }).await;
            assert!(result.is_err());
            let (saved, count) = db.get_local_files_page(None, 10, 0).await.unwrap();
            assert_eq!(count, 1);
            assert_eq!(saved[0].path, file.to_string_lossy());
            assert_eq!(db.local_revision.load(Ordering::SeqCst), revision);
            if replace_mount { fs::remove_dir(&root).unwrap(); }
            fs::rename(&unplugged, &root).unwrap();
        }
        let resumed = scan_library(&db, &root, Arc::new(AtomicBool::new(false)), |_|{}).await.unwrap();
        assert_eq!(resumed.unchanged, 1);
        assert_eq!(resumed.read, 0);
        drop(db);
        fs::remove_dir_all(temp).unwrap();
    }

    #[tokio::test]
    async fn test_native_scanner_pipeline() {
        let temp_dir = std::env::temp_dir().join(format!(
            "turso_scan_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let music_dir = temp_dir.join("music");
        fs::create_dir_all(&music_dir).unwrap();

        let db_path = temp_dir.join("scan.sqlite3");
        let store = TursoDb::open(&db_path).await.expect("Failed to open DB");

        let song_path = music_dir.join("test_track.flac");
        // Create a minimal valid FLAC fixture directly from native bytes
        fs::write(&song_path, crate::stream_download::MINIMAL_FLAC)
            .expect("Failed to write flac fixture");

        let cancel = Arc::new(AtomicBool::new(false));
        let initial_revision = store.revision.load(Ordering::SeqCst);
        let progress_messages = Arc::new(std::sync::Mutex::new(Vec::new()));
        let pm = progress_messages.clone();

        let summary = scan_library(&store, &music_dir, cancel.clone(), move |msg| {
            pm.lock().unwrap().push(msg.to_string());
        })
        .await
        .expect("Scan failed");

        assert_eq!(summary.status, "complete");
        assert_eq!(summary.read, 1);
        assert_eq!(summary.unchanged, 0);
        assert!(store.revision.load(Ordering::SeqCst) > initial_revision);

        // Verify row was stored in Turso
        let (files, total) = store
            .get_local_files_page(None, 10, 0)
            .await
            .expect("Failed to get files");
        assert_eq!(total, 1);
        assert_eq!(files[0].path, song_path.to_str().unwrap());

        // Re-scan: verify incremental skip (read=0, unchanged=1)
        let unchanged_revision = store.revision.load(Ordering::SeqCst);
        let summary2 = scan_library(&store, &music_dir, cancel, |_| {})
            .await
            .expect("Second scan failed");
        assert_eq!(summary2.read, 0);
        assert_eq!(summary2.unchanged, 1);
        assert_eq!(store.revision.load(Ordering::SeqCst), unchanged_revision);
        assert!(progress_messages.lock().unwrap().iter().any(|m|m.contains("0/1 files")));
        assert!(progress_messages.lock().unwrap().iter().any(|m|m.contains("1/1 files")));
        // A cancelled enumeration must not publish missing-file decisions.
        fs::remove_file(&song_path).unwrap();
        let stopped=scan_library(&store,&music_dir,Arc::new(AtomicBool::new(true)),|_|{}).await.unwrap();
        assert_eq!(stopped.status,"cancelled");
        assert_eq!(store.get_local_files_page(None,10,0).await.unwrap().1,1);
        let final_scan=scan_library(&store,&music_dir,Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
        assert_eq!(final_scan.missing,1);
        assert_eq!(final_scan.removed,1);
        assert!(store.revision.load(Ordering::SeqCst) > unchanged_revision);
        assert_eq!(store.get_local_files_page(None,10,0).await.unwrap().1,0);
        let absent_revision = store.revision.load(Ordering::SeqCst);
        let repeated=scan_library(&store,&music_dir,Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
        assert_eq!(repeated.missing,1);
        assert_eq!(repeated.removed,0);
        assert_eq!(store.revision.load(Ordering::SeqCst),absent_revision);
        // Reappearance can restore presence entirely from cached tags.
        fs::write(&song_path,crate::stream_download::MINIMAL_FLAC).unwrap();
        let current=fs::metadata(&song_path).unwrap();
        store.connect().unwrap().execute("UPDATE local_files SET size=?,mtime=? WHERE path=?",(current.len() as i64,mtime_ns(&current),song_path.to_str().unwrap())).await.unwrap();
        let restored=scan_library(&store,&music_dir,Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
        assert_eq!(restored.read,0);
        assert_eq!(restored.restored,1);
        assert!(store.revision.load(Ordering::SeqCst)>absent_revision);
        assert_eq!(store.get_local_files_page(None,10,0).await.unwrap().1,1);
        // A matching file stamp is insufficient when its tag cache is absent.
        store.connect().unwrap().execute("UPDATE local_files SET metadata=NULL WHERE path=?",(song_path.to_str().unwrap(),)).await.unwrap();
        let recovered=scan_library(&store,&music_dir,Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
        assert_eq!(recovered.read,1);
        assert!(store.get_local_files_page(None,10,0).await.unwrap().0[0].metadata.is_some());
        // A nonempty but malformed/incorrectly typed legacy cache is also a
        // cache miss, never a reason to keep skipping the file indefinitely.
        for invalid_metadata in ["malformed JSON", "null", "[]", "\"legacy value\""] {
            store.connect().unwrap().execute("UPDATE local_files SET metadata=? WHERE path=?", (invalid_metadata,song_path.to_str().unwrap())).await.unwrap();
            let recovered=scan_library(&store,&music_dir,Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
            assert_eq!(recovered.read,1, "Must repair {invalid_metadata}");
            assert!(store.get_local_files_page(None,10,0).await.unwrap().0[0].metadata.as_ref().unwrap().is_object());
        }
        let _ = fs::remove_dir_all(temp_dir);
    }
}
