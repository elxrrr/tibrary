//! Live filesystem access is separate from the saved library inventory.
//! An absent drive never means its cached tracks have been deleted.
use crate::db::TursoDb;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

pub(crate) fn require_library(root: &str) -> Result<(), String> {
    let path = Path::new(root);
    if root.trim().is_empty() || !path.is_absolute() {
        return Err("Choose a registered library first".into());
    }
    if !path.is_dir() || std::fs::read_dir(path).is_err() {
        return Err(format!("Library is disconnected or unavailable: {root}. Reconnect the drive to read or change its files. Cached library and online tools remain available."));
    }
    Ok(())
}

/// Detect disconnect/replacement of the root while a scan is in progress.
#[derive(PartialEq, Eq)]
pub(crate) struct LibraryIdentity {
    canonical: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

pub(crate) fn library_identity(root: &Path) -> Result<LibraryIdentity, String> {
    require_library(&root.to_string_lossy())?;
    let canonical = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&canonical).map_err(|error| error.to_string())?;
        Ok(LibraryIdentity {
            canonical,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    Ok(LibraryIdentity { canonical })
}

pub(crate) fn require_same_library(root: &Path, original: &LibraryIdentity) -> Result<(), String> {
    if &library_identity(root)? != original {
        return Err("Library drive changed during the scan; cached missing-file status was preserved. Reconnect the original drive and scan again.".into());
    }
    Ok(())
}

pub(crate) fn requires_local_files(kind: &str) -> bool {
    matches!(
        kind,
        "scan" | "mqa" | "artwork" | "apply" | "workflow" | "consolidate"
    )
}

async fn registered_roots(db: &TursoDb) -> Result<Vec<String>, String> {
    let conn = db.connect()?;
    let mut rows = conn
        .query("SELECT root FROM roots ORDER BY root", ())
        .await
        .map_err(|error| error.to_string())?;
    let mut roots = Vec::new();
    while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
        roots.push(row.get(0).map_err(|error| error.to_string())?);
    }
    Ok(roots)
}

fn destination_access(path: &Path, roots: &[String]) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err("Choose an absolute download folder without parent-directory shortcuts".into());
    }
    for root in roots {
        if path.starts_with(root) {
            require_library(root)?;
        }
    }
    // Never create a directory with the name of an unmounted macOS volume.
    if let Ok(relative) = path.strip_prefix("/Volumes") {
        if let Some(volume) = relative.components().next() {
            require_library(&Path::new("/Volumes").join(volume).to_string_lossy())?;
        }
    }
    let ancestor = path
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or("Download folder is unavailable; choose a folder on a connected drive")?;
    if !ancestor.is_dir() || std::fs::read_dir(ancestor).is_err() {
        return Err(
            "Download folder is not accessible; choose a folder on a connected drive".into(),
        );
    }
    if std::fs::metadata(ancestor)
        .map_err(|error| error.to_string())?
        .permissions()
        .readonly()
    {
        return Err("Download folder is read-only; choose a writable folder".into());
    }
    Ok(())
}

pub(crate) async fn require_download_destination(db: &TursoDb, path: &Path) -> Result<(), String> {
    destination_access(path, &registered_roots(db).await?)
}

pub(crate) async fn download_folder(db: &TursoDb) -> Result<PathBuf, String> {
    let settings = db.get_preference("downloads").await?.unwrap_or(Value::Null);
    Ok(settings["output"]
        .as_str()
        .map(|path| {
            if let Some(relative) = path.strip_prefix("~/") {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default()
                    .join(relative)
            } else {
                PathBuf::from(path)
            }
        })
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join("Downloads/Music")
        }))
}

pub(crate) async fn snapshot(db: &TursoDb) -> Result<Value, String> {
    let roots = registered_roots(db).await?;
    let output = download_folder(db).await?;
    let error = crate::downloads::validate_download_destinations(db)
        .await
        .err();
    Ok(json!({"roots":roots.iter().map(|root| {
        let error = require_library(root).err();
        json!({"root":root,"available":error.is_none(),"unavailable_reason":error})
    }).collect::<Vec<_>>(),"download_destination":{"path":output,"available":error.is_none(),"reason":error}}))
}

pub(crate) async fn decorate_state(db: &TursoDb, state: &mut Value) -> Result<(), String> {
    let access = snapshot(db).await?;
    if let Some(roots) = state["roots"].as_array_mut() {
        for root in roots {
            if let Some(current) = access["roots"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["root"] == root["root"])
            {
                root["available"] = current["available"].clone();
                root["unavailable_reason"] = current["unavailable_reason"].clone();
            }
        }
    }
    state["download_destination"] = access["download_destination"].clone();
    Ok(())
}

/// Check file-dependent requests before authentication or worker creation.
/// Database-only linking, previews and queue decisions use the saved inventory.
pub(crate) async fn guard_rpc(db: &TursoDb, method: &str, args: &Value) -> Result<(), String> {
    let input = args
        .get("args")
        .filter(|value| value.is_object())
        .unwrap_or(args);
    let kind = args["kind"].as_str().unwrap_or("");
    if method == "job.start" && kind == "download" {
        return crate::downloads::validate_download_destinations(db).await;
    }
    let requires_root = method == "turso.scan"
        || method == "turso.maintenance.apply"
        || (method == "job.start" && requires_local_files(kind));
    if requires_root {
        let selected = input["root"].as_str().or_else(|| args["root"].as_str());
        let root = if let Some(root) = selected {
            root.to_owned()
        } else if method == "turso.scan" || kind == "scan" {
            registered_roots(db)
                .await?
                .into_iter()
                .next()
                .unwrap_or_default()
        } else {
            String::new()
        };
        require_library(&root)?;
    }
    if matches!(method, "turso.tags.write" | "turso.mqa.audit") {
        let path = args["path"].as_str().ok_or("Choose a local file")?;
        let conn = db.connect()?;
        let mut rows = conn
            .query("SELECT root FROM local_files WHERE path=?", (path,))
            .await
            .map_err(|error| error.to_string())?;
        if let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
            require_library(&row.get::<String>(0).map_err(|error| error.to_string())?)?;
        }
        if !Path::new(path).is_file() {
            return Err(
                "Local file is unavailable; reconnect its drive before reading or changing it"
                    .into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disconnected_cache_preserves_online_access_and_allows_an_independent_download_folder()
    {
        let temp = std::env::temp_dir().join(format!("tibrary-access-{}", uuid::Uuid::new_v4()));
        let root = temp.join("removable/Music");
        let downloads = temp.join("internal/downloads");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(temp.join("internal")).unwrap();
        let db = TursoDb::open(temp.join("db")).await.unwrap();
        db.add_root(root.to_str().unwrap()).await.unwrap();
        db.set_preference("downloads", &json!({"output":downloads}))
            .await
            .unwrap();
        assert_eq!(snapshot(&db).await.unwrap()["roots"][0]["available"], true);
        std::fs::rename(temp.join("removable"), temp.join("disconnected")).unwrap();
        let offline = snapshot(&db).await.unwrap();
        assert_eq!(offline["roots"][0]["available"], false);
        assert_eq!(offline["download_destination"]["available"], true);
        for kind in ["scan", "mqa", "artwork", "apply", "consolidate"] {
            assert!(
                guard_rpc(&db, "job.start", &json!({"kind":kind,"args":{"root":root}}))
                    .await
                    .unwrap_err()
                    .contains("disconnected")
            );
        }
        for kind in [
            "preview",
            "local_duplicates",
            "review_consolidation",
            "link",
            "deep_apply",
            "metadata",
            "discography",
            "cached_releases",
            "queue_mqa",
            "check_replacements",
            "download",
        ] {
            guard_rpc(&db, "job.start", &json!({"kind":kind,"args":{"root":root}}))
                .await
                .unwrap();
        }
        assert!(require_download_destination(&db, &root.join("downloads"))
            .await
            .is_err());
        assert!(require_download_destination(
            &db,
            &Path::new("/Volumes")
                .join(format!("tibrary-unmounted-{}", uuid::Uuid::new_v4()))
                .join("Music")
        )
        .await
        .is_err());
        assert!(
            require_download_destination(&db, &temp.join("internal/../removable/Music"))
                .await
                .is_err()
        );
        assert!(
            !downloads.exists(),
            "Availability checks never create download folders"
        );
        std::fs::rename(temp.join("disconnected"), temp.join("removable")).unwrap();
        assert_eq!(snapshot(&db).await.unwrap()["roots"][0]["available"], true);
        assert_eq!(db.list_roots("GB").await.unwrap().len(), 1);
        drop(db);
        std::fs::remove_dir_all(temp).unwrap();
    }
}
