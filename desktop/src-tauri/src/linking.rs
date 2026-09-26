use crate::db::TursoDb;
use crate::matching::title_key;
use crate::release_matching::{structure_match, LocalTrackInfo};
use crate::tidal::{TidalCatalogue, TidalRelease};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LinkSummary {
    pub total: usize,
    pub linked: usize,
    pub review: usize,
    pub unmatched: usize,
}

pub async fn link_library(
    db: &TursoDb,
    market: &str,
    root: &str,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(String),
) -> Result<LinkSummary, String> {
    link_library_scoped(db, market, root, cancel, progress, None, false).await
}

pub async fn link_library_scoped(
    db: &TursoDb,
    market: &str,
    root: &str,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(String),
    selected: Option<&HashSet<String>>,
    editions_only: bool,
) -> Result<LinkSummary, String> {
    let conn = db.connect()?;

    // 1. Load mappings: artist -> Vec<tidal_id>
    let mut mappings: HashMap<String, HashSet<String>> = HashMap::new();
    let mut map_stmt = conn
        .query(
            "SELECT artist, tidal_id FROM mappings WHERE status IN ('confirmed', 'auto')",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;

    while let Some(row) = map_stmt.next().await.map_err(|e| e.to_string())? {
        let artist: String = row.get(0).unwrap_or_default();
        let tid: String = row.get(1).unwrap_or_default();
        if !artist.is_empty() && !tid.is_empty() {
            mappings.entry(title_key(&artist)).or_default().insert(tid);
        }
    }

    let mut add_stmt = conn
        .query("SELECT artist, tidal_id FROM additional_mappings", ())
        .await
        .map_err(|e| e.to_string())?;

    while let Some(row) = add_stmt.next().await.map_err(|e| e.to_string())? {
        let artist: String = row.get(0).unwrap_or_default();
        let tid: String = row.get(1).unwrap_or_default();
        if !artist.is_empty() && !tid.is_empty() {
            mappings.entry(title_key(&artist)).or_default().insert(tid);
        }
    }

    // 2. Load catalogue releases by artist_id
    let mut catalogues_by_artist: HashMap<String, Vec<TidalRelease>> = HashMap::new();
    let mut cat_stmt = conn
        .query(
            "SELECT artist_id, payload FROM catalogue WHERE market = ?",
            (market,),
        )
        .await
        .map_err(|e| e.to_string())?;

    while let Some(row) = cat_stmt.next().await.map_err(|e| e.to_string())? {
        let artist_id: String = row.get(0).unwrap_or_default();
        let payload_str: Option<String> = row.get(1).ok().flatten();
        if let Some(s) = payload_str {
            if let Ok(cat) = serde_json::from_str::<TidalCatalogue>(&s) {
                catalogues_by_artist.insert(artist_id, cat.releases);
            }
        }
    }

    // 3. Load local files
    let mut file_stmt = conn
        .query(
            "SELECT path, metadata, mtime, size FROM local_files WHERE root = ? AND present = 1 AND metadata IS NOT NULL",
            (root,),
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut local_tracks = Vec::new();
    let mut file_stamps = HashMap::new();
    let mut totals = HashMap::new();
    let mut eligible = HashSet::new();

    while let Some(row) = file_stmt.next().await.map_err(|e| e.to_string())? {
        let path: String = row.get(0).unwrap_or_default();
        let meta_str: Option<String> = row.get(1).ok().flatten();
        let mtime: i64 = row.get(2).unwrap_or(0);
        let size: i64 = row.get(3).unwrap_or(0);

        file_stamps.insert(path.clone(), format!("[0,0,{},{}]", size, mtime));

        let meta: Value = meta_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);

        let tags = crate::workflows::extract_tags_map(&Some(meta.clone()));
        let text = |key: &str| tags.get(key).cloned().unwrap_or_default();
        let number = |key: &str| {
            text(key)
                .split('/')
                .next()
                .unwrap_or("")
                .trim()
                .parse::<u32>()
                .unwrap_or(0)
        };
        let title = text("title");
        let artist = tags
            .get("albumartist")
            .cloned()
            .unwrap_or_else(|| text("artist"));
        let album = text("album");
        let duration = meta["duration"].as_f64().unwrap_or(0.0);
        let track_number = number("tracknumber");
        let disc_number = number("discnumber").max(1);
        let isrc = tags.get("isrc").cloned();
        let total = |key: &str, num: &str| {
            let n = number(key);
            if n > 0 {
                n
            } else {
                text(num)
                    .split('/')
                    .nth(1)
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(0)
            }
        };
        totals.insert(
            path.clone(),
            (
                total("tracktotal", "tracknumber"),
                total("disctotal", "discnumber"),
            ),
        );
        let mut previous = conn
            .query(
                "SELECT payload,stamp FROM track_links WHERE path=? AND market=?",
                (path.as_str(), market),
            )
            .await
            .map_err(|e| e.to_string())?;
        let (old, stamp_current) =
            if let Some(row) = previous.next().await.map_err(|e| e.to_string())? {
                let raw: String = row.get(0).unwrap_or_default();
                let stamp: String = row.get(1).unwrap_or_default();
                (
                    serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null),
                    serde_json::from_str::<Value>(&stamp).ok() == Some(json!([0, 0, size, mtime])),
                )
            } else {
                (Value::Null, false)
            };
        let mut ignored = conn
            .query(
                "SELECT path FROM ignored_local_files WHERE path=?",
                (path.as_str(),),
            )
            .await
            .map_err(|e| e.to_string())?;
        let is_ignored = ignored.next().await.map_err(|e| e.to_string())?.is_some();
        let linked = stamp_current
            && (old["status"] == "linked"
                || (old["status"].is_null() && old["ids"]["track_id"].as_str().is_some()));
        if !is_ignored
            && selected.map(|s| s.contains(&path)).unwrap_or(!linked)
            && (!editions_only
                || (!linked
                    && old["catalogue_options"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty())))
        {
            eligible.insert(path.clone());
        }

        local_tracks.push(LocalTrackInfo {
            path,
            title,
            artist,
            album,
            duration,
            track_number,
            disc_number,
            isrc,
        });
    }

    let total = eligible.len();
    if total == 0 {
        return Ok(LinkSummary::default());
    }

    // 4. Group local tracks by (artist_key, album_key)
    let mut groups: HashMap<(String, String, String), Vec<LocalTrackInfo>> = HashMap::new();
    for track in local_tracks {
        let art_key = title_key(&track.artist);
        let alb_key = title_key(&track.album);
        let mut folder = std::path::Path::new(&track.path)
            .parent()
            .unwrap_or(std::path::Path::new(root));
        if folder
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
            .starts_with("disc ")
        {
            folder = folder.parent().unwrap_or(folder);
        }
        groups
            .entry((art_key, alb_key, folder.display().to_string()))
            .or_default()
            .push(track);
    }

    let mut summary = LinkSummary {
        total: eligible.len(),
        ..Default::default()
    };

    let group_count = groups.len();
    let mut group_idx = 0;

    for ((art_key, alb_key, _folder), group_tracks) in groups {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        if !group_tracks.iter().any(|t| eligible.contains(&t.path)) {
            continue;
        }
        group_idx += 1;
        if group_idx % 5 == 0 || group_idx == group_count {
            progress(format!(
                "Linking releases · {}/{} albums",
                group_idx, group_count
            ));
        }

        // Look up online releases for this artist
        let mut candidate_releases = Vec::new();
        if let Some(artist_ids) = mappings.get(&art_key) {
            for aid in artist_ids {
                if let Some(rels) = catalogues_by_artist.get(aid) {
                    for r in rels {
                        if title_key(&r.title) == alb_key {
                            candidate_releases.push(r.clone());
                        }
                    }
                }
            }
        }

        if candidate_releases.is_empty() {
            // Unmatched
            for track in group_tracks.iter().filter(|t| eligible.contains(&t.path)) {
                summary.unmatched += 1;
                let payload = json!({
                    "status": "unmatched",
                    "note": "No matching release in linked artist catalogues",
                    "checked_at": chrono::Utc::now().timestamp(),
                });
                let stamp = file_stamps
                    .get(&track.path)
                    .cloned()
                    .unwrap_or_else(|| "[]".to_string());
                let payload_str = payload.to_string();
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
                    (track.path.as_str(), market, stamp.as_str(), payload_str.as_str()),
                ).await;
            }
            continue;
        }

        // Hydrate only relevant editions, reusing the shared release cache.
        for release in &mut candidate_releases {
            if cancel.load(Ordering::Relaxed) {
                return Ok(summary);
            }
            if !release.tracks_loaded {
                progress(format!("Loading release details · {}", release.title));
                let details = crate::actions::release(db, &release.id, market, false).await?;
                *release = serde_json::from_value(details).map_err(|e| e.to_string())?;
            }
        }
        // Match against candidates
        let mut scored_candidates = Vec::new();
        for rel in &candidate_releases {
            let mut res = structure_match(&group_tracks, rel);
            for track in &group_tracks {
                let (total, discs) = totals[&track.path];
                let remote_total = rel
                    .tracks
                    .iter()
                    .filter(|t| t.disc_number == track.disc_number)
                    .count() as u32;
                let remote_discs = rel.tracks.iter().map(|t| t.disc_number).max().unwrap_or(1);
                if (total >= track.track_number && total > 0 && remote_total != total)
                    || (discs >= track.disc_number && discs > 0 && remote_discs != discs)
                {
                    res.compatible = false;
                    res.conflicts
                        .push("Release totals differ from local tags".into());
                }
            }
            scored_candidates.push((rel, res));
        }

        // Sort candidates by matched_count desc, conflicts len asc
        scored_candidates.sort_by(|a, b| {
            b.1.compatible
                .cmp(&a.1.compatible)
                .then_with(|| b.1.matched_count.cmp(&a.1.matched_count))
                .then_with(|| a.1.conflicts.len().cmp(&b.1.conflicts.len()))
        });

        let (best_rel, best_struct) = &scored_candidates[0];

        if best_struct.matched_count > 0 {
            let is_perfect =
                best_struct.compatible && best_struct.matched_count == group_tracks.len();
            let status = if is_perfect { "linked" } else { "review" };

            let cand_options: Vec<Value> = scored_candidates
                .iter()
                .map(|(r, s)| {
                    json!({
                        "id": r.id,
                        "title": r.title,
                        "date": r.date,
                        "tracks": r.track_count,
                        "matched": s.matched_count,
                    })
                })
                .collect();

            for track in group_tracks.iter().filter(|t| eligible.contains(&t.path)) {
                let stamp = file_stamps
                    .get(&track.path)
                    .cloned()
                    .unwrap_or_else(|| "[]".to_string());
                if let Some(align) = best_struct.alignments.get(&track.path) {
                    if is_perfect {
                        summary.linked += 1;
                    } else {
                        summary.review += 1;
                    }
                    let mut payload = json!({
                        "status": status,
                        "ids": {
                            "track_id": align.remote_track_id,
                            "album_id": best_rel.id,
                        },
                        "catalogue_note": if is_perfect { "Verified exact release match" } else { "Partial release match" },
                        "catalogue_options": cand_options,
                        "checked_at": chrono::Utc::now().timestamp(),
                    });
                    if !is_perfect {
                        payload.as_object_mut().unwrap().remove("ids");
                    } else {
                        payload["placements"] = json!(scored_candidates
                            .iter()
                            .filter(|(_, s)| s.compatible && s.matched_count == group_tracks.len())
                            .filter_map(|(r, s)| s
                                .alignments
                                .get(&track.path)
                                .map(|a| json!({"album_id":r.id,"track_id":a.remote_track_id})))
                            .collect::<Vec<_>>());
                    }
                    payload["catalogue_options"] = json!(scored_candidates.iter().filter_map(|(r,s)|s.alignments.get(&track.path).map(|a|json!({"id":r.id,"title":r.title,"track_id":a.remote_track_id,"tracks":r.track_count,"artist":r.artist,"album":r.title,"position_label":format!("Disc {} · Track {}/{}",a.disc_number,a.track_number,r.track_count),"evidence":s.conflicts.join("; "),"structure":{"compatible":s.compatible,"reasons":s.conflicts},"compatible":s.compatible}))).collect::<Vec<_>>());
                    let payload_str = payload.to_string();
                    let _ = conn.execute(
                        "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
                        (track.path.as_str(), market, stamp.as_str(), payload_str.as_str()),
                    ).await;
                } else {
                    summary.review += 1;
                    let payload = json!({
                        "status": "review",
                        "catalogue_note": "Track position unverified on candidate release",
                        "catalogue_options": cand_options,
                        "checked_at": chrono::Utc::now().timestamp(),
                    });
                    let payload_str = payload.to_string();
                    let _ = conn.execute(
                        "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
                        (track.path.as_str(), market, stamp.as_str(), payload_str.as_str()),
                    ).await;
                }
            }
        } else {
            for track in group_tracks.iter().filter(|t| eligible.contains(&t.path)) {
                summary.unmatched += 1;
                let payload = json!({
                    "status": "unmatched",
                    "note": "Tracks did not align with candidate releases",
                    "checked_at": chrono::Utc::now().timestamp(),
                });
                let stamp = file_stamps
                    .get(&track.path)
                    .cloned()
                    .unwrap_or_else(|| "[]".to_string());
                let payload_str = payload.to_string();
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
                    (track.path.as_str(), market, stamp.as_str(), payload_str.as_str()),
                ).await;
            }
        }
    }

    db.bump_revision();
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tidal::TidalTrack;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn test_link_library_pipeline() {
        let temp_dir = std::env::temp_dir().join(format!(
            "linking_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("link.sqlite3");
        let store = TursoDb::open(&db_path)
            .await
            .expect("Failed to open TursoDb");
        let conn = store.connect().unwrap();

        // 1. Seed mapping
        conn.execute(
            "INSERT INTO mappings (artist, tidal_id, status) VALUES ('Queen', 'queen_id', 'confirmed')",
            (),
        ).await.unwrap();

        // 2. Seed catalogue
        let cat = TidalCatalogue {
            id: "queen_id".to_string(),
            name: "Queen".to_string(),
            releases: vec![TidalRelease {
                id: "album_1".to_string(),
                artist: "Queen".to_string(),
                title: "A Night at the Opera".to_string(),
                date: "1975-11-21".to_string(),
                r#type: "album".to_string(),
                available: Some(true),
                track_count: 1,
                explicit: false,
                copyright: None,
                label: None,
                quality: "LOSSLESS".to_string(),
                tracks: vec![TidalTrack {
                    id: "track_101".to_string(),
                    title: "Bohemian Rhapsody".to_string(),
                    isrc: None,
                    track_number: 1,
                    disc_number: 1,
                    duration: 355.0,
                    bpm: None,
                    key: None,
                    key_scale: None,
                    copyright: None,
                }],
                tracks_loaded: true,
                ..Default::default()
            }],
        };
        conn.execute(
            "INSERT INTO catalogue (artist_id, market, payload, fetched) VALUES ('queen_id', 'GB', ?, '2026-01-01')",
            (serde_json::to_string(&cat).unwrap().as_str(),),
        ).await.unwrap();

        // 3. Seed local file
        let meta = json!({
            "title": "Bohemian Rhapsody",
            "artist": "Queen",
            "album": "A Night at the Opera",
            "duration": 355.0,
            "track_number": 1,
            "disc_number": 1
        });
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES ('/music/bohemian.flac', '/music', 100, 100, ?, 1)",
            (serde_json::to_string(&meta).unwrap().as_str(),),
        ).await.unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let summary = link_library(&store, "GB", "/music", cancel, |_| {})
            .await
            .unwrap();

        assert_eq!(summary.total, 1);
        assert_eq!(summary.linked, 1);
        assert_eq!(summary.unmatched, 0);

        // Verify track_links in DB
        let mut stmt = conn
            .query(
                "SELECT payload FROM track_links WHERE path = '/music/bohemian.flac'",
                (),
            )
            .await
            .unwrap();
        let row = stmt.next().await.unwrap().unwrap();
        let payload_str: String = row.get(0).unwrap();
        let p: Value = serde_json::from_str(&payload_str).unwrap();
        assert_eq!(p["status"], "linked");
        assert_eq!(p["ids"]["track_id"], "track_101");
        assert_eq!(p["ids"]["album_id"], "album_1");

        let repeated = link_library(
            &store,
            "GB",
            "/music",
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(
            repeated.total, 0,
            "unchanged linked files are not rechecked"
        );
        let selected = HashSet::from(["/music/bohemian.flac".to_string()]);
        let explicit = link_library_scoped(
            &store,
            "GB",
            "/music",
            Arc::new(AtomicBool::new(false)),
            |_| {},
            Some(&selected),
            false,
        )
        .await
        .unwrap();
        assert_eq!(explicit.linked, 1);
        let editions = link_library_scoped(
            &store,
            "GB",
            "/music",
            Arc::new(AtomicBool::new(false)),
            |_| {},
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(editions.total, 0);

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
