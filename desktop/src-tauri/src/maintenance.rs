use crate::db::TursoDb;
use crate::scanner::read_audio_metadata;
use crate::tag_writer::write_tags;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileApplyItem {
    pub path: String,
    pub target: Option<String>,
    #[serde(default)]
    pub artwork: Option<String>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApplyResult {
    pub applied: usize,
    pub failed: usize,
    pub errors: Vec<String>,
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

/// Local inspections share the library database, keyed by the indexed file stamp.
pub(crate) async fn load_inspections(
    db: &TursoDb,
    prefix: &str,
) -> Result<HashMap<String, serde_json::Value>, String> {
    let conn = db.connect()?;
    let mut rows = conn
        .query("SELECT key,payload FROM app_preferences WHERE key LIKE ?", (format!("{prefix}%"),))
        .await.map_err(|e| e.to_string())?;
    let mut saved = HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        let payload: String = row.get(1).map_err(|e| e.to_string())?;
        if let (Some(path), Ok(value)) = (key.strip_prefix(prefix), serde_json::from_str(&payload)) {
            saved.insert(path.to_string(), value);
        }
    }
    Ok(saved)
}

pub(crate) fn valid_inspection(value: &serde_json::Value, size: i64, mtime: i64) -> bool {
    value["size"].as_i64() == Some(size) && value["mtime"].as_i64() == Some(mtime)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ArtworkInspection {
    pub front_covers: Vec<(u32, u32)>,
}

impl ArtworkInspection {
    pub fn has_standard_cover(&self) -> bool {
        self.front_covers.contains(&(1280, 1280))
    }
}

/// Empty artwork is a successful inspection; read failures are never cached.
pub(crate) async fn inspect_artwork(
    db: &TursoDb,
    file: &crate::db::LocalFileRecord,
    saved: Option<&serde_json::Value>,
    force: bool,
) -> Result<(ArtworkInspection, bool), String> {
    if !force {
        if let Some(value) = saved.filter(|value| valid_inspection(value, file.size, file.mtime) && value["schema"] == 1) {
            if let Ok(result) = serde_json::from_value(value["result"].clone()) {
                return Ok((result, true));
            }
        }
    }
    let path = file.path.clone();
    let result = tokio::task::spawn_blocking(move || {
        use lofty::{file::TaggedFileExt, picture::{PictureInformation, PictureType}};
        let audio = lofty::probe::Probe::open(&path).map_err(|e| e.to_string())?
            .options(lofty::config::ParseOptions::new().read_properties(false))
            .read().map_err(|e| e.to_string())?;
        let front_covers = audio.tags().iter().flat_map(|tag| tag.pictures())
            .filter(|picture| picture.pic_type() == PictureType::CoverFront)
            .filter_map(|picture| PictureInformation::from_picture(picture).ok())
            .map(|info| (info.width, info.height)).collect();
        Ok::<_, String>(ArtworkInspection { front_covers })
    }).await.map_err(|e| e.to_string())??;
    db.set_preference(&format!("artwork-inspection:{}", file.path),
        &serde_json::json!({"schema":1,"size":file.size,"mtime":file.mtime,"result":result})).await?;
    Ok((result, false))
}

pub async fn apply_file_item(
    db: &TursoDb,
    root: &str,
    item: &FileApplyItem,
) -> Result<PathBuf, String> {
    let source_path = PathBuf::from(&item.path);
    if !source_path.exists() {
        return Err(format!("Source file does not exist: {}", item.path));
    }

    let library = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let canonical = fs::canonicalize(&source_path).map_err(|e| e.to_string())?;
    if !canonical.starts_with(&library)
        || fs::symlink_metadata(&source_path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err(
            "File must be inside the selected library and cannot be a symbolic link".into(),
        );
    }
    let final_path = item
        .target
        .as_ref()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| source_path.clone());
    if final_path
        .components()
        .any(|c| c == std::path::Component::ParentDir)
        || !(final_path.starts_with(&library) || final_path.starts_with(PathBuf::from(root)))
    {
        return Err("Destination must remain inside the selected library".into());
    }
    if final_path != source_path && final_path.exists() {
        return Err("Destination already exists; no files changed".into());
    }
    let before = fs::metadata(&source_path).map_err(|e| e.to_string())?;
    // These operations rewrite metadata/containers only; the audio is retained.
    // Preserve inspections only when their evidence was unaffected by the edit.
    let mqa_tags_changed = item.tags.keys().any(|key| {
        let key = key.to_ascii_lowercase();
        key.contains("mqa") || matches!(key.as_str(), "encoder" | "encodedby" | "originalsamplerate")
    });
    let mut preserved = Vec::new();
    for (prefix, keep) in [("mqa-audit:", !mqa_tags_changed), ("artwork-inspection:", item.artwork.is_none())] {
        if keep {
            if let Some(value) = db.get_preference(&format!("{prefix}{}", item.path)).await? {
                if valid_inspection(&value, before.len() as i64, mtime_ns(&before)) {
                    preserved.push((prefix, value));
                }
            }
        }
    }
    let parent = final_path.parent().ok_or("Invalid destination")?;
    let ancestor = parent
        .ancestors()
        .find(|p| p.exists())
        .ok_or("Invalid destination")?;
    if !fs::canonicalize(ancestor)
        .map_err(|e| e.to_string())?
        .starts_with(&library)
    {
        return Err("Destination resolves outside library".into());
    }
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    if !fs::canonicalize(parent)
        .map_err(|e| e.to_string())?
        .starts_with(&library)
    {
        return Err("Destination resolves outside library".into());
    }
    let staged = parent.join(format!(
        ".tibrary-{}.{}",
        uuid::Uuid::new_v4(),
        source_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("flac")
    ));
    let publish = (|| -> Result<(), String> {
        fs::copy(&source_path, &staged).map_err(|e| e.to_string())?;
        write_tags(&staged, &item.tags)?;
        if let Some(artwork) = &item.artwork {
            use lofty::{
                file::{AudioFile, TaggedFileExt},
                ogg::OggPictureStorage,
                picture::{Picture, PictureType},
            };
            let bytes = fs::read(artwork).map_err(|e| e.to_string())?;
            let mut pic = Picture::from_reader(&mut std::io::Cursor::new(bytes))
                .map_err(|e| e.to_string())?;
            pic.set_pic_type(PictureType::CoverFront);
            if staged
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("flac"))
            {
                let mut input = fs::File::open(&staged).map_err(|e| e.to_string())?;
                let mut audio = lofty::flac::FlacFile::read_from(
                    &mut input,
                    lofty::config::ParseOptions::default().implicit_conversions(false),
                )
                .map_err(|e| e.to_string())?;
                drop(input);
                audio.remove_picture_type(PictureType::CoverFront);
                audio.insert_picture(pic, None).map_err(|e| e.to_string())?;
                audio
                    .save_to_path(&staged, lofty::config::WriteOptions::default())
                    .map_err(|e| e.to_string())?;
            } else {
                let mut audio = lofty::probe::Probe::open(&staged)
                    .map_err(|e| e.to_string())?
                    .read()
                    .map_err(|e| e.to_string())?;
                let kind = audio.primary_tag_type();
                if audio.tag(kind).is_none() {
                    audio.insert_tag(lofty::tag::Tag::new(kind));
                }
                let tag = audio.tag_mut(kind).unwrap();
                tag.remove_picture_type(PictureType::CoverFront);
                tag.push_picture(pic);
                audio
                    .save_to_path(&staged, lofty::config::WriteOptions::default())
                    .map_err(|e| e.to_string())?;
            }
        }
        read_audio_metadata(&staged)?;
        let current = fs::metadata(&source_path).map_err(|e| e.to_string())?;
        if current.len() != before.len() || mtime_ns(&current) != mtime_ns(&before) {
            return Err("File changed during operation; refresh the preview".into());
        }
        if final_path == source_path {
            fs::rename(&staged, &final_path).map_err(|e| e.to_string())?;
        } else {
            // An atomic no-overwrite publication; collisions never replace another track.
            fs::hard_link(&staged, &final_path).map_err(|e| e.to_string())?;
            fs::remove_file(&source_path).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    let _ = fs::remove_file(&staged);
    publish?;

    // 3. Read metadata of final file
    let meta = read_audio_metadata(&final_path)?;
    let fs_meta = fs::metadata(&final_path).map_err(|e| e.to_string())?;
    let size = fs_meta.len() as i64;
    let mtime = mtime_ns(&fs_meta);
    let meta_val = serde_json::to_value(&meta).map_err(|e| e.to_string())?;

    // 4. Update Turso DB
    db.apply_file_update(
        &source_path.display().to_string(),
        &final_path.display().to_string(),
        root,
        &meta_val,
        size,
        mtime,
    )
    .await?;

    for (prefix, mut value) in preserved {
        value["size"] = serde_json::json!(size);
        value["mtime"] = serde_json::json!(mtime);
        if prefix == "mqa-audit:" {
            value["result"]["path"] = serde_json::json!(final_path.display().to_string());
        }
        if let Err(error) = db.set_preference(&format!("{prefix}{}", final_path.display()), &value).await {
            eprintln!("File updated; cached inspection will be repeated: {error}");
        } else if source_path != final_path {
            if let Ok(conn) = db.connect() {
                let _ = conn.execute("DELETE FROM app_preferences WHERE key=?", (format!("{prefix}{}", item.path),)).await;
            }
        }
    }

    Ok(final_path)
}

pub async fn apply_batch(db: &TursoDb, root: &str, items: &[FileApplyItem]) -> ApplyResult {
    let mut applied = 0;
    let mut failed = 0;
    let mut errors = Vec::new();
    let mut repaired = std::collections::HashSet::new();

    for item in items {
        match apply_file_item(db, root, item).await {
            Ok(path) => { applied += 1; if item.tags.keys().any(|k| matches!(k.as_str(), "tracktotal" | "disctotal" | "tracknumber" | "discnumber")) { repaired.insert(path.display().to_string()); } },
            Err(e) => {
                failed += 1;
                errors.push(format!("{}: {}", item.path, e));
            }
        }
    }

    if !repaired.is_empty() { refresh_number_links(db, root, &repaired).await; }
    ApplyResult {
        applied,
        failed,
        errors,
    }
}

pub async fn refresh_number_links(db: &TursoDb, root: &str, paths: &std::collections::HashSet<String>) {
    let settings = db.get_settings().await.unwrap_or_default();
    let market = settings["general"]["market"].as_str().unwrap_or("GB");
    if let Ok(files) = crate::actions::files(db, root).await {
        let keys: std::collections::HashSet<_> = files.iter().filter(|f|paths.contains(&f.path)).map(|f|crate::workflows::release_key(f,&crate::workflows::extract_tags_map(&f.metadata))).collect();
        let peers: Vec<_> = files.into_iter().filter(|f|keys.contains(&crate::workflows::release_key(f,&crate::workflows::extract_tags_map(&f.metadata)))).collect();
        if let Err(error) = crate::release_anchor::propagate_files(db,&peers,market).await { eprintln!("Cached sibling link refresh: {error}"); }
    }
    if let Err(error) = crate::linking::link_library_mode(db, market, root,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), |_| {}, Some(paths), false, true).await {
        eprintln!("Cached link refresh after tag correction: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::ItemKey;
    use std::time::SystemTime;

    #[tokio::test]
    async fn local_inspections_are_reused_and_follow_reviewed_tag_only_moves() {
        let temp = std::env::temp_dir().join(format!("local-inspection-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp).unwrap();
        let path = temp.join("original.flac");
        fs::write(&path, crate::stream_download::MINIMAL_FLAC).unwrap();
        let db = TursoDb::open(temp.join("library.db")).await.unwrap();
        crate::scanner::scan_library(&db, &temp, std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), |_| {}).await.unwrap();
        let file = crate::actions::files(&db, temp.to_str().unwrap()).await.unwrap().remove(0);
        let (first, reused) = inspect_artwork(&db, &file, None, false).await.unwrap();
        assert!(!reused);
        assert!(!first.has_standard_cover());
        let saved = db.get_preference(&format!("artwork-inspection:{}", file.path)).await.unwrap().unwrap();
        // A reused inspection must not need to reopen the audio container.
        let held = temp.join("held.flac");
        fs::rename(&path, &held).unwrap();
        assert_eq!(inspect_artwork(&db, &file, Some(&saved), false).await.unwrap(), (first.clone(), true));
        assert!(inspect_artwork(&db, &file, Some(&saved), true).await.is_err());
        let mut changed = file.clone();
        changed.mtime += 1;
        assert!(inspect_artwork(&db, &changed, Some(&saved), false).await.is_err());
        fs::rename(&held, &path).unwrap();
        let audit = serde_json::json!({"size":file.size,"mtime":file.mtime,"result":{"path":file.path,"detected":false,"status":"No signal found"}});
        db.set_preference(&format!("mqa-audit:{}", file.path), &audit).await.unwrap();
        let target = temp.join("moved.flac");
        apply_file_item(&db, temp.to_str().unwrap(), &FileApplyItem {
            path: file.path.clone(), target: Some(target.display().to_string()), artwork: None,
            tags: HashMap::from([("title".into(), "New title".into())]),
        }).await.unwrap();
        let moved = crate::actions::files(&db, temp.to_str().unwrap()).await.unwrap().remove(0);
        let saved = db.get_preference(&format!("artwork-inspection:{}", moved.path)).await.unwrap().unwrap();
        assert!(valid_inspection(&saved, moved.size, moved.mtime));
        assert_eq!(inspect_artwork(&db, &moved, Some(&saved), false).await.unwrap(), (first, true));
        let audit = db.get_preference(&format!("mqa-audit:{}", moved.path)).await.unwrap().unwrap();
        assert!(valid_inspection(&audit, moved.size, moved.mtime));
        assert_eq!(audit["result"]["path"], serde_json::json!(moved.path));
        assert!(db.get_preference(&format!("mqa-audit:{}", file.path)).await.unwrap().is_none());
        use base64::Engine;
        let cover = temp.join("cover.png");
        fs::write(&cover, base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/a9sAAAAASUVORK5CYII=").unwrap()).unwrap();
        apply_file_item(&db, temp.to_str().unwrap(), &FileApplyItem {
            path: moved.path.clone(), target: None, artwork: Some(cover.display().to_string()), tags: HashMap::new(),
        }).await.unwrap();
        let updated = crate::actions::files(&db, temp.to_str().unwrap()).await.unwrap().remove(0);
        let old_artwork = db.get_preference(&format!("artwork-inspection:{}", updated.path)).await.unwrap().unwrap();
        assert!(!valid_inspection(&old_artwork, updated.size, updated.mtime), "Changing embedded artwork must invalidate its prior inspection");
        let (artwork, reused) = inspect_artwork(&db, &updated, Some(&old_artwork), false).await.unwrap();
        assert!(!reused);
        assert_eq!(artwork.front_covers, vec![(1, 1)]);
        apply_file_item(&db, temp.to_str().unwrap(), &FileApplyItem {
            path: updated.path.clone(), target: None, artwork: None,
            tags: HashMap::from([("encoder".into(), "MQA encoder".into())]),
        }).await.unwrap();
        let changed = crate::actions::files(&db, temp.to_str().unwrap()).await.unwrap().remove(0);
        let old_audit = db.get_preference(&format!("mqa-audit:{}", changed.path)).await.unwrap().unwrap();
        assert!(!valid_inspection(&old_audit, changed.size, changed.mtime), "Changing MQA evidence tags must invalidate its prior audit");
        fs::remove_dir_all(&temp).unwrap();
    }

    #[tokio::test]
    async fn test_apply_file_item() {
        let temp_dir = std::env::temp_dir().join(format!(
            "maintenance_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("maint.sqlite3");
        let store = TursoDb::open(&db_path)
            .await
            .expect("Failed to open TursoDb");

        let source_file = temp_dir.join("old_song.flac");
        let target_dir = temp_dir.join("Artist").join("Album");
        let target_file = target_dir.join("01 - New Title.flac");

        // Create a minimal valid FLAC fixture directly from native bytes
        fs::write(&source_file, crate::stream_download::MINIMAL_FLAC)
            .expect("Failed to write FLAC fixture");

        // Seed old file in local_files
        let conn = store.connect().unwrap();
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES (?, ?, 100, 100, '{}', 1)",
            (source_file.display().to_string().as_str(), temp_dir.display().to_string().as_str()),
        ).await.unwrap();

        let mut tags = HashMap::new();
        tags.insert("title".to_string(), "New Title".to_string());
        tags.insert("artist".to_string(), "The Beatles".to_string());
        tags.insert("album".to_string(), "Abbey Road".to_string());
        tags.insert("tracknumber".to_string(), "1".to_string());

        let item = FileApplyItem {
            path: source_file.display().to_string(),
            target: Some(target_file.display().to_string()),
            artwork: None,
            tags,
        };

        let res_path = apply_file_item(&store, &temp_dir.display().to_string(), &item)
            .await
            .expect("Failed to apply file item");

        assert_eq!(res_path, target_file);
        assert!(!source_file.exists());
        assert!(target_file.exists());

        // Verify tags on the target file
        let tagged = Probe::open(&target_file).unwrap().read().unwrap();
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag()).unwrap();
        assert_eq!(tag.get_string(ItemKey::TrackTitle), Some("New Title"));
        assert_eq!(tag.get_string(ItemKey::TrackArtist), Some("The Beatles"));

        // Verify DB updates
        let mut old_stmt = conn
            .query(
                "SELECT present FROM local_files WHERE path = ?",
                (source_file.display().to_string().as_str(),),
            )
            .await
            .unwrap();
        let old_present: i64 = old_stmt.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(old_present, 0);

        let mut new_stmt = conn
            .query(
                "SELECT present, metadata FROM local_files WHERE path = ?",
                (target_file.display().to_string().as_str(),),
            )
            .await
            .unwrap();
        let row = new_stmt.next().await.unwrap().unwrap();
        let new_present: i64 = row.get(0).unwrap();
        let new_meta: String = row.get(1).unwrap();
        assert_eq!(new_present, 1);
        assert!(new_meta.contains("New Title"));

        let _ = fs::remove_dir_all(temp_dir);
    }
}
