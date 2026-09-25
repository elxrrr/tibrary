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

const AUDIO_EXTS: &[&str] = &[
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

    if ext == "flac" {
        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        let options = ParseOptions::new()
            .implicit_conversions(false)
            .parsing_mode(ParsingMode::Strict)
            .read_cover_art(false);
        let audio = FlacFile::read_from(&mut file, options).map_err(|e| e.to_string())?;
        duration = (audio.properties().duration().as_secs_f64() * 1000.0).round() / 1000.0;
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
    while let Some(row) = cached_rows.next().await.map_err(|e| e.to_string())? {
        let path: String = row.get(0).map_err(|e| e.to_string())?;
        let size: i64 = row.get(1).map_err(|e| e.to_string())?;
        let mtime: i64 = row.get(2).map_err(|e| e.to_string())?;
        let error_opt: Option<String> = row.get::<Option<String>>(4).unwrap_or(None);
        let has_error = error_opt
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let present: i64 = row.get(5).unwrap_or(1);
        cached_files.insert(path, (size, mtime, has_error, present != 0));
    }

    progress(&format!(
        "Loaded {} cached files · traversing directory tree",
        cached_files.len()
    ));

    let allowed_exts: HashSet<&str> = AUDIO_EXTS.iter().copied().collect();
    let mut pending_dirs = vec![root_path.clone()];
    let mut seen_paths = HashSet::new();

    let mut summary = ScanSummary::default();
    let mut pending_writes: Vec<PendingWrite> = Vec::new();
    let mut restored_paths = Vec::new();
    let mut last_report = Instant::now();

    while let Some(dir) = pending_dirs.pop() {
        if cancelled.load(Ordering::Relaxed) {
            summary.status = "cancelled".to_string();
            break;
        }

        if last_report.elapsed() >= Duration::from_millis(250) {
            progress(&format!("Checking folder · {}", dir.display()));
            last_report = Instant::now();
        }

        let entries = match fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            if cancelled.load(Ordering::Relaxed) {
                summary.status = "cancelled".to_string();
                break;
            }

            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_symlink() {
                continue;
            }

            let path = entry.path();
            if file_type.is_dir() {
                pending_dirs.push(path);
                continue;
            }

            if !file_type.is_file() {
                continue;
            }

            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if !allowed_exts.contains(ext.as_str()) {
                continue;
            }

            let path_str = match path.to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };

            seen_paths.insert(path_str.clone());

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            let size = metadata.len() as i64;
            let mtime = mtime_ns(&metadata);

            // Incremental check: if size and mtime match and no prior error, skip reading tags!
            if let Some(&(cached_size, cached_mtime, has_error, is_present)) =
                cached_files.get(&path_str)
            {
                if !force && cached_size == size && cached_mtime == mtime && !has_error {
                    summary.unchanged += 1;
                    if !is_present {
                        restored_paths.push(path_str);
                    }
                    continue;
                }
            }

            // Needs tag read
            summary.read += 1;
            let (meta_json, err_str) = match read_audio_metadata(&path) {
                Ok(m) => (serde_json::to_string(&m).ok(), None),
                Err(e) => (None, Some(e)),
            };

            if err_str.is_some() {
                summary.errors += 1;
            }

            pending_writes.push((path_str, root_str.clone(), size, mtime, meta_json, err_str));

            if pending_writes.len() >= 64 {
                flush_writes(&conn, &mut pending_writes).await?;
            }

            if last_report.elapsed() >= Duration::from_millis(250) {
                progress(&format!(
                    "Scanned {} files · {} read · {} unchanged",
                    seen_paths.len(),
                    summary.read,
                    summary.unchanged
                ));
                last_report = Instant::now();
            }
        }
    }

    // Flush any remaining writes
    if !pending_writes.is_empty() {
        flush_writes(&conn, &mut pending_writes).await?;
    }

    // Restore any files that reappeared
    for p in restored_paths {
        conn.execute(
            "UPDATE local_files SET present = 1 WHERE path = ?",
            (p.as_str(),),
        )
        .await
        .ok();
    }

    if summary.status != "cancelled" {
        summary.status = "complete".to_string();
        // Mark missing files that exist in DB for this root but were not seen
        let mut missing_count = 0;
        for path_str in cached_files.keys() {
            if !seen_paths.contains(path_str) {
                missing_count += 1;
                conn.execute(
                    "UPDATE local_files SET present = 0 WHERE path = ?",
                    (path_str.as_str(),),
                )
                .await
                .ok();
            }
        }
        summary.missing = missing_count;

        let now_str = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE roots SET status = 'complete', scanned_at = ? WHERE root = ?",
            (now_str.as_str(), root_str.as_str()),
        )
        .await
        .ok();
    }

    progress(&format!(
        "Scan {} · {} tags read · {} unchanged · {} errors · {} missing",
        summary.status, summary.read, summary.unchanged, summary.errors, summary.missing
    ));

    Ok(summary)
}

async fn flush_writes(
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
    pending.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

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

        // Verify row was stored in Turso
        let (files, total) = store
            .get_local_files_page(None, 10, 0)
            .await
            .expect("Failed to get files");
        assert_eq!(total, 1);
        assert_eq!(files[0].path, song_path.to_str().unwrap());

        // Re-scan: verify incremental skip (read=0, unchanged=1)
        let summary2 = scan_library(&store, &music_dir, cancel, |_| {})
            .await
            .expect("Second scan failed");
        assert_eq!(summary2.read, 0);
        assert_eq!(summary2.unchanged, 1);

        let _ = fs::remove_dir_all(temp_dir);
    }
}
