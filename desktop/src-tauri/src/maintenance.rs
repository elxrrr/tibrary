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

pub async fn apply_file_item(
    db: &TursoDb,
    root: &str,
    item: &FileApplyItem,
) -> Result<PathBuf, String> {
    let source_path = PathBuf::from(&item.path);
    if !source_path.exists() {
        return Err(format!("Source file does not exist: {}", item.path));
    }

    // 1. Update tags if any
    if !item.tags.is_empty() {
        write_tags(&source_path, &item.tags)?;
    }

    // 2. Move file if target is specified and different
    let final_path = if let Some(ref target_str) = item.target {
        let target_path = PathBuf::from(target_str);
        if target_path != source_path {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if let Err(e) = fs::rename(&source_path, &target_path) {
                // If rename across filesystem boundaries fails, copy and remove
                fs::copy(&source_path, &target_path).map_err(|e2| {
                    format!("Failed to move file (rename: {}, copy: {})", e, e2)
                })?;
                let _ = fs::remove_file(&source_path);
            }
            target_path
        } else {
            source_path.clone()
        }
    } else {
        source_path.clone()
    };

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

    Ok(final_path)
}

pub async fn apply_batch(
    db: &TursoDb,
    root: &str,
    items: &[FileApplyItem],
) -> ApplyResult {
    let mut applied = 0;
    let mut failed = 0;
    let mut errors = Vec::new();

    for item in items {
        match apply_file_item(db, root, item).await {
            Ok(_) => applied += 1,
            Err(e) => {
                failed += 1;
                errors.push(format!("{}: {}", item.path, e));
            }
        }
    }

    ApplyResult {
        applied,
        failed,
        errors,
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
        let store = TursoDb::open(&db_path).await.expect("Failed to open TursoDb");

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
        let mut old_stmt = conn.query(
            "SELECT present FROM local_files WHERE path = ?",
            (source_file.display().to_string().as_str(),),
        ).await.unwrap();
        let old_present: i64 = old_stmt.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(old_present, 0);

        let mut new_stmt = conn.query(
            "SELECT present, metadata FROM local_files WHERE path = ?",
            (target_file.display().to_string().as_str(),),
        ).await.unwrap();
        let row = new_stmt.next().await.unwrap().unwrap();
        let new_present: i64 = row.get(0).unwrap();
        let new_meta: String = row.get(1).unwrap();
        assert_eq!(new_present, 1);
        assert!(new_meta.contains("New Title"));

        let _ = fs::remove_dir_all(temp_dir);
    }
}
