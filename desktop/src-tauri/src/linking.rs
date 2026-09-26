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
    link_library_mode(db, market, root, cancel, progress, selected, editions_only, false).await
}

pub async fn link_library_mode(
    db: &TursoDb, market: &str, root: &str, cancel: Arc<AtomicBool>, progress: impl Fn(String),
    selected: Option<&HashSet<String>>, editions_only: bool, cached_only: bool,
) -> Result<LinkSummary, String> {
    let conn = db.connect()?;
    let general = db.get_settings().await?["general"].clone();
    let include_unofficial = general["recommend_bootlegs"].as_bool().unwrap_or(false);
    let include_compilations = general["recommend_compilations"].as_bool().unwrap_or(false);
    let fuzzy_review = general["fuzzy_release_matching"].as_bool().unwrap_or(false);

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
    let mut local_upcs = HashMap::new();
    let mut local_genres: HashMap<String, HashSet<String>> = HashMap::new();
    let mut local_credits = HashMap::new();
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

        local_credits.insert(
            path.clone(),
            crate::recommendations::local_credit_names(&meta),
        );
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
        if let Some(genres) = tags.get("genre") { local_genres.insert(path.clone(),genres.split(';').map(crate::matching::name_key).filter(|g| !g.is_empty()).collect()); }
        if let Some(upc) = tags.get("upc").or(tags.get("barcode")) { local_upcs.insert(path.clone(),upc.clone()); }
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
                    crate::db::link_stamp_matches(&stamp, size, mtime),
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

    let group_count = groups.values().filter(|tracks| tracks.iter().any(|t| eligible.contains(&t.path))).count();
    let mut group_idx = 0;

    for ((art_key, alb_key, _folder), group_tracks) in groups {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        if !group_tracks.iter().any(|t| eligible.contains(&t.path)) {
            continue;
        }
        group_idx += 1;
        progress(format!("Checking release · {}/{} releases · {} — {} · {} local tracks · cached candidates first", group_idx, group_count, group_tracks[0].artist, group_tracks[0].album, group_tracks.len()));

        // Look up online releases for this artist
        let mut candidate_releases = Vec::new();
        if let Some(artist_ids) = mappings.get(&art_key) {
            for aid in artist_ids {
                if let Some(rels) = catalogues_by_artist.get(aid) {
                    for r in rels {
                        let candidate_title = title_key(&r.title);
                        let related_title = fuzzy_review
                            && alb_key.len() >= 8
                            && candidate_title.len() >= 8
                            && (candidate_title.starts_with(&format!("{alb_key} "))
                                || alb_key.starts_with(&format!("{candidate_title} ")));
                        if candidate_title == alb_key || related_title {
                            candidate_releases.push(r.clone());
                        }
                    }
                }
            }
        }

        // Provider replacement IDs are candidate edges, never identity proof.
        // Keep the original placement and require the normal recording/position checks.
        let replacement_ids: Vec<_> = candidate_releases.iter().filter_map(|r| r.replacement_id.clone()).collect();
        for id in replacement_ids {
            if candidate_releases.iter().any(|r| r.id == id) { continue; }
            if let Some(target) = catalogues_by_artist.values().flatten().find(|r| r.id == id && title_key(&r.artist) == art_key) {
                candidate_releases.push(target.clone());
            }
        }
        // A local repair must never trigger network requests or erase a prior choice
        // when the catalogue lacks any candidate's full track list.
        if cached_only && (candidate_releases.is_empty() || candidate_releases.iter().any(|r| !r.tracks_loaded)) { continue; }
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
                conn.execute(
                    "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
                    (track.path.as_str(), market, stamp.as_str(), payload_str.as_str()),
                ).await.map_err(|e| format!("Could not save track link: {e}"))?;
                progress(format!("Unmatched · {}/{} tracks checked · {} — {} · {} · no release in linked artist cache", summary.linked + summary.review + summary.unmatched, total, track.artist, track.title, track.album));
            }
            continue;
        }

        // Hydrate only relevant editions, reusing the shared release cache.
        for release in &mut candidate_releases {
            if cancel.load(Ordering::Relaxed) {
                return Ok(summary);
            }
            if !release.tracks_loaded {
                progress(format!("Loading release details · {group_idx}/{group_count} releases · {} — {} · release ID {}", release.artist, release.title, release.id));
                let details = crate::actions::release(db, &release.id, market, false).await?;
                *release = serde_json::from_value(details).map_err(|e| e.to_string())?;
            }
        }
        // Match against candidates
        let mut scored_candidates = Vec::new();
        let mut credit_scores = HashMap::new();
        let mut barcode_scores = HashMap::new();
        let mut genre_scores = HashMap::new();
        for rel in &candidate_releases {
            let mut res = structure_match(&group_tracks, rel);
            let unofficial = rel.official == Some(false)
                || rel.secondary_types.iter().any(|kind| {
                    ["bootleg", "promo", "unofficial"]
                        .iter()
                        .any(|flag| kind.eq_ignore_ascii_case(flag))
                })
                || rel.title.to_ascii_lowercase().contains("bootleg");
            let compilation = rel.r#type.eq_ignore_ascii_case("compilation")
                || rel
                    .secondary_types
                    .iter()
                    .any(|kind| kind.eq_ignore_ascii_case("compilation"));
            if title_key(&rel.title) != alb_key {
                res.compatible = false;
                res.conflicts
                    .push("Related title; manual review required".into());
            }
            if unofficial && !include_unofficial {
                res.compatible = false;
                res.conflicts.push("Unofficial or promotional edition; enable in settings to allow automatic matching".into());
            }
            if compilation && !include_compilations {
                res.compatible = false;
                res.conflicts
                    .push("Compilation; enable in settings to allow automatic matching".into());
            }
            for track in &group_tracks {
                if track.track_number > 0
                    && res.alignments.get(&track.path).is_some_and(|alignment| {
                        alignment.track_number != track.track_number
                            || alignment.disc_number != track.disc_number
                    })
                {
                    res.compatible = false;
                    res.conflicts
                        .push("Track or disc positions differ from local tags".into());
                }
                let (total, discs) = totals[&track.path];
                let remote_total = rel
                    .tracks
                    .iter()
                    .filter(|t| t.disc_number == track.disc_number)
                    .count() as u32;
                let remote_discs = rel.tracks.iter().map(|t| t.disc_number).max().unwrap_or(1);
                let minimum_tracks = group_tracks.iter().filter(|t| t.disc_number == track.disc_number).map(|t| t.track_number).max().unwrap_or(0);
                let minimum_discs = group_tracks.iter().map(|t| t.disc_number).max().unwrap_or(1);
                if (total >= minimum_tracks && total > 0 && remote_total != total)
                    || (discs >= minimum_discs && discs > 0 && remote_discs != discs)
                {
                    res.compatible = false;
                    res.conflicts
                        .push("Release totals differ from local tags".into());
                }
            }
            let shared: usize = group_tracks
                .iter()
                .map(|track| {
                    let Some(local) = local_credits.get(&track.path) else {
                        return 0;
                    };
                    let Some(alignment) = res.alignments.get(&track.path) else {
                        return 0;
                    };
                    let Some(remote) = rel
                        .tracks
                        .iter()
                        .find(|track| track.id == alignment.remote_track_id)
                    else {
                        return 0;
                    };
                    crate::recommendations::credit_names(&remote.credits)
                        .intersection(local)
                        .count()
                })
                .sum();
            let remote_genres: HashSet<_> = rel.genres.iter().chain(rel.tracks.iter().flat_map(|t| t.genres.iter())).map(|g| crate::matching::name_key(g)).collect();
            genre_scores.insert(rel.id.clone(), group_tracks.iter().filter_map(|t| local_genres.get(&t.path)).map(|g| g.intersection(&remote_genres).count()).sum::<usize>());
            barcode_scores.insert(rel.id.clone(), rel.upc.as_ref().is_some_and(|upc| !upc.is_empty() && group_tracks.iter().any(|t| local_upcs.get(&t.path) == Some(upc)) && group_tracks.iter().all(|t| local_upcs.get(&t.path).is_none_or(|local| local == upc))));
            credit_scores.insert(rel.id.clone(), shared);
            scored_candidates.push((rel, res));
        }

        // Sort candidates by matched_count desc, conflicts len asc
        scored_candidates.sort_by(|a, b| {
            b.1.compatible
                .cmp(&a.1.compatible)
                .then_with(|| b.1.matched_count.cmp(&a.1.matched_count))
                .then_with(|| a.1.conflicts.len().cmp(&b.1.conflicts.len()))
                .then_with(|| barcode_scores[&b.0.id].cmp(&barcode_scores[&a.0.id]))
                .then_with(|| credit_scores[&b.0.id].cmp(&credit_scores[&a.0.id]))
                .then_with(|| a.0.replacement_id.is_some().cmp(&b.0.replacement_id.is_some()))
                .then_with(|| genre_scores[&b.0.id].cmp(&genre_scores[&a.0.id]))
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
                    payload["catalogue_options"] = json!(scored_candidates.iter().filter_map(|(r,s)|s.alignments.get(&track.path).map(|a|json!({"id":r.id,"title":r.title,"track_id":a.remote_track_id,"tracks":r.track_count,"artist":r.artist,"album":r.title,"position_label":format!("Disc {} · Track {}/{}",a.disc_number,a.track_number,r.track_count),"evidence":({let mut reasons=s.conflicts.clone(); if credit_scores[&r.id]>0 { reasons.push(format!("{} shared local contributor credits support this release",credit_scores[&r.id])); } reasons.join("; ")}),"structure":{"compatible":s.compatible,"reasons":s.conflicts},"compatible":s.compatible}))).collect::<Vec<_>>());
                    let payload_str = payload.to_string();
                    conn.execute(
                        "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
                        (track.path.as_str(), market, stamp.as_str(), payload_str.as_str()),
                    ).await.map_err(|e| format!("Could not save track link: {e}"))?;
                } else {
                    summary.review += 1;
                    let payload = json!({
                        "status": "review",
                        "catalogue_note": "Track position unverified on candidate release",
                        "catalogue_options": cand_options,
                        "checked_at": chrono::Utc::now().timestamp(),
                    });
                    let payload_str = payload.to_string();
                    conn.execute(
                        "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
                        (track.path.as_str(), market, stamp.as_str(), payload_str.as_str()),
                    ).await.map_err(|e| format!("Could not save track link: {e}"))?;
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
                conn.execute(
                    "INSERT OR REPLACE INTO track_links (path, market, stamp, payload) VALUES (?, ?, ?, ?)",
                    (track.path.as_str(), market, stamp.as_str(), payload_str.as_str()),
                ).await.map_err(|e| format!("Could not save track link: {e}"))?;
            }
        }
        for track in group_tracks.iter().filter(|t| eligible.contains(&t.path)) {
            let mut saved = conn.query("SELECT payload FROM track_links WHERE path=? AND market=?", (track.path.as_str(), market)).await.map_err(|e|e.to_string())?;
            if let Some(row) = saved.next().await.map_err(|e|e.to_string())? {
                let raw: String = row.get(0).map_err(|e|e.to_string())?;
                let result: Value = serde_json::from_str(&raw).map_err(|e|e.to_string())?;
                let outcome = match result["status"].as_str() { Some("linked") => "Linked", Some("review") => "Needs review", _ => "Unmatched" };
                progress(format!("{outcome} · {}/{} tracks checked · {} — {} · {} · {}", summary.linked + summary.review + summary.unmatched, total, track.artist, track.title, track.album, result["catalogue_note"].as_str().or(result["note"].as_str()).unwrap_or("Recording checked")));
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
        let mut cat = TidalCatalogue {
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
                    ..Default::default()
                }],
                tracks_loaded: true,
                ..Default::default()
            }],
        };
        let mut other = cat.releases[0].clone();
        other.id = "other_edition".into();
        other.tracks[0].id = "other_track".into();
        cat.releases[0].tracks[0].credits = json!([{"name":"Freddie Mercury","role":"Composer"}]);
        cat.releases.insert(0, other);
        let mut conflict = cat.releases[1].clone();
        conflict.id = "wrong_position".into();
        conflict.tracks[0].track_number = 2;
        cat.releases.insert(0, conflict);
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
            "composer": "Freddie Mercury",
            "track_number": 1,
            "disc_number": 1
        });
        conn.execute(
            "INSERT INTO local_files (path, root, size, mtime, metadata, present) VALUES ('/music/bohemian.flac', '/music', 100, 100, ?, 1)",
            (serde_json::to_string(&meta).unwrap().as_str(),),
        ).await.unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let events = std::sync::Mutex::new(Vec::new());
        let summary = link_library(&store, "GB", "/music", cancel, |message| events.lock().unwrap().push(message))
            .await
            .unwrap();

        assert_eq!(summary.total, 1);
        assert!(events.lock().unwrap().iter().any(|event| event.starts_with("Linked · 1/1 tracks checked")));
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
        assert_eq!(p["placements"].as_array().unwrap().len(), 2);
        assert!(p["catalogue_options"][0]["evidence"]
            .as_str()
            .unwrap()
            .contains("shared local contributor"));

        conn.execute("UPDATE track_links SET stamp='[99,123,100,100,456]' WHERE path='/music/bohemian.flac'", ()).await.unwrap();
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
