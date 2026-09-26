//! Conservative propagation from an explicitly chosen, complete release.
use crate::{
    db::{LocalFileRecord, TursoDb},
    tidal::TidalRelease,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub struct AnchoredRelease {
    pub release: TidalRelease,
    pub files: Vec<LocalFileRecord>,
}

pub async fn verified(
    db: &TursoDb,
    files: &[LocalFileRecord],
    market: &str,
) -> Result<Vec<AnchoredRelease>, String> {
    let mut groups: HashMap<_, Vec<LocalFileRecord>> = HashMap::new();
    for file in files.iter().filter(|f| f.present) {
        let tags = crate::workflows::extract_tags_map(&file.metadata);
        groups
            .entry(crate::workflows::release_key(file, &tags))
            .or_default()
            .push(file.clone());
    }
    let conn = db.connect()?;
    let mut query = conn
        .query(
            "SELECT path,payload FROM track_links WHERE market=?",
            (market,),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut anchors = HashMap::new();
    while let Some(row) = query.next().await.map_err(|e| e.to_string())? {
        let raw: String = row.get(1).map_err(|e| e.to_string())?;
        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
            if value["manual"] == true && value["status"] == "linked" {
                anchors.insert(row.get::<String>(0).map_err(|e| e.to_string())?, value);
            }
        }
    }
    drop(query);
    let mut result = Vec::new();
    for (_, mut peers) in groups {
        let chosen: Vec<_> = peers
            .iter()
            .filter_map(|p| anchors.get(&p.path).map(|a| (&p.path, a)))
            .collect();
        let ids: HashSet<_> = chosen
            .iter()
            .filter_map(|(_, a)| a["ids"]["album_id"].as_str())
            .collect();
        if ids.len() != 1 {
            continue;
        }
        let id = *ids.iter().next().unwrap();
        let raw = match db
            .get_preference(&format!("tag-review:{market}:{id}"))
            .await?
        {
            Some(v) => v,
            None => {
                db.get_detail(&serde_json::json!({"release_id":id,"market":market}))
                    .await?
            }
        };
        let Ok(mut release) = serde_json::from_value::<TidalRelease>(raw) else {
            continue;
        };
        if !release.tracks_loaded || release.tracks.len() != peers.len() || peers.is_empty() {
            continue;
        }
        release
            .tracks
            .sort_by_key(|t| (t.disc_number, t.track_number));
        if !peers.iter().all(|p| {
            crate::matching::title_key(
                &crate::workflows::extract_tags_map(&p.metadata)
                    .get("album")
                    .cloned()
                    .unwrap_or_default(),
            ) == crate::matching::title_key(&release.title)
        }) {
            continue;
        }
        peers.sort_by_key(position);
        let mut positions = HashSet::new();
        let mut remote_ids = HashSet::new();
        let mut remote_positions = HashSet::new();
        let valid = peers.iter().zip(&release.tracks).all(|(file, track)| {
            let pos = position(file);
            let tags = crate::workflows::extract_tags_map(&file.metadata);
            let matches = |t: &crate::tidal::TidalTrack| {
                crate::release_matching::recording_matches(
                    tags.get("title").map(String::as_str).unwrap_or(""),
                    file.metadata
                        .as_ref()
                        .and_then(|m| m["duration"].as_f64())
                        .unwrap_or(0.0),
                    tags.get("isrc").map(String::as_str),
                    &t.title,
                    t.duration,
                    t.isrc.as_deref(),
                    true,
                )
            };
            pos.1 > 0
                && track.disc_number > 0
                && track.track_number > 0
                && remote_positions.insert((track.disc_number, track.track_number))
                && track.track_number
                    <= release
                        .tracks
                        .iter()
                        .filter(|t| t.disc_number == track.disc_number)
                        .count() as u32
                && positions.insert(pos)
                && !track.id.is_empty()
                && remote_ids.insert(track.id.clone())
                && matches(track)
                && release.tracks.iter().filter(|t| matches(t)).count() == 1
                && anchors
                    .get(&file.path)
                    .is_none_or(|a| a["ids"]["track_id"].as_str() == Some(track.id.as_str()))
        });
        // Only complete, contiguous local discs establish reliable per-disc totals.
        let contiguous = peers.iter().all(|p| {
            let (disc, _) = position(p);
            let indices: HashSet<_> = peers
                .iter()
                .filter(|p| position(p).0 == disc)
                .map(|p| position(p).1)
                .collect();
            (1..=indices.len() as u32).all(|n| indices.contains(&n))
        });
        if valid && contiguous {
            result.push(AnchoredRelease {
                release,
                files: peers,
            });
        }
    }
    Ok(result)
}

pub fn position(file: &LocalFileRecord) -> (u32, u32) {
    let tags = crate::workflows::extract_tags_map(&file.metadata);
    let number = |key: &str| {
        tags.get(key)
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    (number("discnumber").max(1), number("tracknumber"))
}

pub async fn propagate(db: &TursoDb, path: &str, market: &str) -> Result<usize, String> {
    let conn = db.connect()?;
    let mut q = conn
        .query(
            "SELECT root FROM local_files WHERE path=? AND present=1",
            (path,),
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = q.next().await.map_err(|e| e.to_string())? else {
        return Ok(0);
    };
    let root: String = row.get(0).map_err(|e| e.to_string())?;
    drop(q);
    let files = crate::actions::files(db, &root).await?;
    let tags = files.iter().find(|f| f.path == path).map(|f| {
        crate::workflows::release_key(f, &crate::workflows::extract_tags_map(&f.metadata))
    });
    let peers: Vec<_> = files
        .into_iter()
        .filter(|f| {
            Some(crate::workflows::release_key(
                f,
                &crate::workflows::extract_tags_map(&f.metadata),
            )) == tags
        })
        .collect();
    propagate_files(db, &peers, market).await
}

pub async fn propagate_files(
    db: &TursoDb,
    peers: &[LocalFileRecord],
    market: &str,
) -> Result<usize, String> {
    let conn = db.connect()?;
    let mut count = 0;
    for group in verified(db, peers, market).await? {
        for (file, track) in group.files.iter().zip(&group.release.tracks) {
            let mut q = conn
                .query(
                    "SELECT payload FROM track_links WHERE path=? AND market=?",
                    (file.path.as_str(), market),
                )
                .await
                .map_err(|e| e.to_string())?;
            let prior = if let Some(r) = q.next().await.map_err(|e| e.to_string())? {
                serde_json::from_str::<Value>(&r.get::<String>(0).unwrap_or_default())
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            drop(q);
            let mut ignored = conn
                .query(
                    "SELECT path FROM ignored_local_files WHERE path=?",
                    (file.path.as_str(),),
                )
                .await
                .map_err(|e| e.to_string())?;
            if ignored.next().await.map_err(|e| e.to_string())?.is_some()
                || prior["status"] == "linked"
                || prior["ids"]["track_id"].as_str().is_some()
            {
                continue;
            }
            drop(ignored);
            let payload = serde_json::json!({"status":"linked","manual":false,"ids":{"album_id":group.release.id,"track_id":track.id},"note":"Complete release verified against manually chosen placement; online numbering available in Correct tags"});
            db.save_track_link(
                &file.path,
                market,
                &serde_json::json!([0, 0, file.size, file.mtime]).to_string(),
                &payload.to_string(),
            )
            .await?;
            count += 1;
        }
    }
    if count > 0 {
        db.bump_revision();
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn complete_split_release_proposes_online_numbers_and_links_only_safe_siblings() {
        let dir = std::env::temp_dir().join(format!("anchor-test-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let mut files = vec![];
        for n in 1..=4 {
            let disc = (n - 1) / 2 + 1;
            let index = (n - 1) % 2 + 1;
            let path = format!("/music/Artist/Album/Disc {disc:02}/{index:02}.flac");
            let metadata = json!({"albumartist":"Artist","album":"Album","title":format!("Song {n}"),"duration":180.0,"isrc":format!("ISRC{n}"),"discnumber":format!("{disc:02}"),"disctotal":"02","tracknumber":format!("{index:02}"),"tracktotal":"01"});
            db.apply_file_update(&path, &path, "/music", &metadata, 1, 1)
                .await
                .unwrap();
            files.push(LocalFileRecord {
                path,
                root: "/music".into(),
                size: 1,
                mtime: 1,
                metadata: Some(metadata),
                error: None,
                present: true,
            });
        }
        let release = json!({"id":"123","artist":"Artist","title":"Album","tracks_loaded":true,"tracks":(1..=4).map(|n|json!({"id":n.to_string(),"title":format!("Song {n}"),"duration":180.0,"isrc":format!("ISRC{n}"),"disc_number":1,"track_number":n})).collect::<Vec<_>>()});
        db.set_preference("tag-review:GB:123", &release)
            .await
            .unwrap();
        db.choose_track_link(
            &json!({"path":files[0].path,"album_id":"123","track_id":"1","market":"GB"}),
        )
        .await
        .unwrap();
        assert_eq!(verified(&db, &files, "GB").await.unwrap().len(), 1);
        let plans = crate::workflows::plan_cached(&db, &files, "numbers", None)
            .await
            .unwrap();
        assert_eq!(plans.len(), 4);
        for plan in plans {
            assert_eq!(plan.changes["tracktotal"], "04");
            assert_eq!(plan.changes["disctotal"], "01");
            assert!(plan.target.is_none());
        }
        db.set_local_files_ignored(&[files[3].path.clone()], true)
            .await
            .unwrap();
        assert_eq!(propagate(&db, &files[0].path, "GB").await.unwrap(), 2);
        assert_eq!(propagate(&db, &files[0].path, "GB").await.unwrap(), 0);
        files[2].metadata.as_mut().unwrap()["isrc"] = json!("DIFFERENT");
        assert!(verified(&db, &files, "GB").await.unwrap().is_empty());
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
