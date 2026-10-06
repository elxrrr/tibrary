//! Conservative manual propagation and read-only, whole-release scan evidence.
use crate::{
    db::{LocalFileRecord, TursoDb},
    tidal::TidalRelease,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// A scan takes one snapshot of existing links and their shared metadata cache.
/// Reading an anchor never authorizes a manual one-track choice to mutate peers.
pub struct ScanAnchorCache {
    pub saved_links: HashMap<String, (Value, String)>,
    pub ignored_paths: HashSet<String>,
    active_links: HashMap<String, HashMap<String, String>>,
    releases: HashMap<String, TidalRelease>,
    live: HashMap<String, Value>,
}

pub struct ScanAnchoredRelease {
    pub release: TidalRelease,
    pub structure: crate::release_matching::StructureMatchResult,
    pub evidence: String,
    pub conflicting_isrc_paths: HashSet<String>,
    pub primary_anchor_count: usize,
}

fn positive_id(value: &Value) -> Option<String> {
    crate::tidal::resource_id(value)
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .map(|n| n.to_string())
}

impl ScanAnchorCache {
    pub async fn load(db: &TursoDb, market: &str, root: &str) -> Result<Self, String> {
        let conn = db.connect()?;
        let mut rows = conn.query(
            "SELECT f.path,l.payload,l.stamp,f.size,f.mtime,i.path FROM local_files f LEFT JOIN track_links l ON l.path=f.path AND l.market=? LEFT JOIN ignored_local_files i ON i.path=f.path WHERE f.root=? AND f.present=1",
            (market, root),
        ).await.map_err(|e| e.to_string())?;
        let mut result = Self {
            saved_links: HashMap::new(),
            ignored_paths: HashSet::new(),
            active_links: HashMap::new(),
            releases: HashMap::new(),
            live: HashMap::new(),
        };
        let mut release_ids = HashSet::new();
        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            let path: String = row.get(0).map_err(|e| e.to_string())?;
            if row.get::<Option<String>>(5).ok().flatten().is_some() {
                result.ignored_paths.insert(path.clone());
            }
            let Some(raw) = row.get::<Option<String>>(1).ok().flatten() else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            let stamp: String = row
                .get::<Option<String>>(2)
                .ok()
                .flatten()
                .unwrap_or_default();
            let active = !result.ignored_paths.contains(&path)
                && crate::db::link_stamp_matches(
                    &stamp,
                    row.get(3).unwrap_or(0),
                    row.get(4).unwrap_or(0),
                )
                && (value["status"] == "linked"
                    || value["status"] == "resolved"
                    || value["status"].is_null());
            if active {
                let mut placements = HashMap::new();
                let mut rejected = HashSet::new();
                let mut had_placement = false;
                for placement in std::iter::once(&value["ids"])
                    .chain(value["placements"].as_array().into_iter().flatten())
                {
                    if let (Some(album), Some(track)) = (
                        positive_id(&placement["album_id"]),
                        positive_id(&placement["track_id"]),
                    ) {
                        had_placement = true;
                        release_ids.insert(album.clone());
                        if placements.get(&album).is_some_and(|prior| prior != &track) {
                            placements.remove(&album);
                            rejected.insert(album.clone());
                        }
                        if !rejected.contains(&album) {
                            placements.insert(album, track);
                        }
                    }
                }
                if had_placement {
                    result.active_links.insert(path.clone(), placements);
                }
            }
            result.saved_links.insert(path, (value, stamp));
        }
        drop(rows);
        if !release_ids.is_empty() {
            let keys: Vec<_> = release_ids
                .iter()
                .flat_map(|id| {
                    [
                        format!("tag-review:{market}:{id}"),
                        format!("release-live:{market}:{id}"),
                    ]
                })
                .collect();
            for (key, value) in crate::db::preference_snapshots_for_keys(&conn, keys).await? {
                let id = key.rsplit(':').next().unwrap_or_default().to_string();
                if key.starts_with("release-live:") {
                    result.live.insert(id, value);
                } else if let Ok(release) = serde_json::from_value::<TidalRelease>(value) {
                    if release.id == id {
                        result.releases.insert(id, release);
                    }
                }
            }
        }
        Ok(result)
    }

    /// Reuse the caller's already-loaded market catalogue; no second scan or API.
    pub fn merge_catalogues<'a>(&mut self, releases: impl IntoIterator<Item = &'a TidalRelease>) {
        let ids: HashSet<_> = self
            .active_links
            .values()
            .flat_map(|links| links.keys().cloned())
            .collect();
        for release in releases {
            if ids.contains(&release.id)
                && self.releases.get(&release.id).is_none_or(|r| {
                    r.title.is_empty()
                        || (!r.tracks_loaded && release.tracks_loaded)
                        || (r.tracks.is_empty() && !release.tracks.is_empty())
                })
            {
                self.releases.insert(release.id.clone(), release.clone());
            }
        }
    }

    pub fn observe_checked_availability(&mut self, id: &str, available: bool) {
        self.live.insert(id.to_owned(), serde_json::json!({"available":available,"checked_at":chrono::Utc::now().timestamp(),"source":"album_lookup"}));
    }

    /// Preserve live N-ary editions in the fast path without checking all IDs
    /// or making any API request. An absent observation remains unconfirmed.
    pub async fn extend_live_cache(
        &mut self,
        db: &TursoDb,
        market: &str,
        ids: &[String],
    ) -> Result<(), String> {
        let keys: Vec<_> = ids
            .iter()
            .filter(|id| !self.live.contains_key(*id))
            .map(|id| format!("release-live:{market}:{id}"))
            .collect();
        if keys.is_empty() {
            return Ok(());
        }
        let conn = db.connect()?;
        for (key, value) in crate::db::preference_snapshots_for_keys(&conn, keys).await? {
            self.live
                .insert(key.rsplit(':').next().unwrap_or_default().to_owned(), value);
        }
        Ok(())
    }

    fn candidate_ids(&self, tracks: &[crate::release_matching::LocalTrackInfo]) -> HashSet<String> {
        let mut linked = tracks.iter().filter_map(|t| self.active_links.get(&t.path));
        let Some(first) = linked.next() else {
            return HashSet::new();
        };
        let mut ids: HashSet<_> = first.keys().cloned().collect();
        for links in linked {
            ids.retain(|id| links.contains_key(id));
        }
        ids
    }

    pub fn candidate_releases(
        &self,
        tracks: &[crate::release_matching::LocalTrackInfo],
    ) -> Vec<TidalRelease> {
        let mut ids: Vec<_> = self.candidate_ids(tracks).into_iter().collect();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| self.releases.get(&id))
            .filter(|r| {
                self.latest_observation(r)["available"] != false
                    && tracks.iter().all(|t| {
                        crate::matching::title_key(&t.album) == crate::matching::title_key(&r.title)
                    })
            })
            .cloned()
            .collect()
    }

    fn latest_observation(&self, release: &TidalRelease) -> Value {
        let from_release = serde_json::json!({"available":release.available,"checked_at":release.availability_checked_at,"source":release.availability_source});
        self.live
            .get(&release.id)
            .filter(|saved| !crate::availability::observation_wins(&from_release, saved))
            .cloned()
            .unwrap_or(from_release)
    }

    pub fn is_confirmed_live(&self, id: &str) -> bool {
        let observation = self
            .releases
            .get(id)
            .map(|r| self.latest_observation(r))
            .or_else(|| self.live.get(id).cloned());
        let Some(observation) = observation else {
            return false;
        };
        Self::confirmed_observation(&observation)
    }

    pub fn is_release_confirmed_live(&self, release: &TidalRelease) -> bool {
        Self::confirmed_observation(&self.latest_observation(release))
    }

    fn confirmed_observation(observation: &Value) -> bool {
        let now = chrono::Utc::now().timestamp();
        observation["available"] == true
            && observation["source"] == "album_lookup"
            && observation["checked_at"]
                .as_i64()
                .is_some_and(|at| at > 0 && at <= now && now - at < 7 * 86400)
    }

    pub fn match_group(
        &self,
        tracks: &[crate::release_matching::LocalTrackInfo],
        totals: &HashMap<String, (u32, u32)>,
    ) -> Vec<ScanAnchoredRelease> {
        self.candidate_releases(tracks)
            .into_iter()
            .filter_map(|release| self.match_release(tracks, totals, release))
            .collect()
    }

    fn match_release(
        &self,
        tracks: &[crate::release_matching::LocalTrackInfo],
        totals: &HashMap<String, (u32, u32)>,
        release: TidalRelease,
    ) -> Option<ScanAnchoredRelease> {
        use crate::release_matching::{
            clean_isrc, recording_matches, StructureMatchResult, TrackAlignment,
        };
        if tracks.is_empty() || !release.tracks_loaded || !self.is_confirmed_live(&release.id) {
            return None;
        }
        let mut remote = HashMap::new();
        let mut ids = HashSet::new();
        let mut counts = HashMap::<u32, u32>::new();
        for track in &release.tracks {
            if track.disc_number == 0
                || track.track_number == 0
                || positive_id(&Value::String(track.id.clone())).is_none()
                || !ids.insert(track.id.clone())
                || remote
                    .insert((track.disc_number, track.track_number), track)
                    .is_some()
            {
                return None;
            }
            *counts.entry(track.disc_number).or_default() += 1;
        }
        let max_disc = counts.keys().copied().max()?;
        if !(1..=max_disc).all(|disc| {
            counts
                .get(&disc)
                .is_some_and(|count| (1..=*count).all(|index| remote.contains_key(&(disc, index))))
        }) {
            return None;
        }
        let mut local_positions = HashSet::new();
        let mut alignments = HashMap::new();
        let mut conflicting_isrc_paths = HashSet::new();
        let mut primary_anchor_count = 0;
        let mut exact_anchor_isrcs = HashSet::new();
        let complete = tracks.len() == release.tracks.len();
        for local in tracks {
            let pos = (local.disc_number, local.track_number);
            if local.track_number == 0 || !local_positions.insert(pos) {
                return None;
            }
            let target = remote.get(&pos)?;
            // Exact title/mix and positive durations are always required. ISRC
            // disagreement is evaluated separately against whole-release proof.
            let matches = |r: &&crate::tidal::TidalTrack| {
                recording_matches(
                    &local.title,
                    local.duration,
                    None,
                    &r.title,
                    r.duration,
                    None,
                    true,
                )
            };
            if !matches(target) || release.tracks.iter().filter(|r| matches(r)).count() != 1 {
                return None;
            }
            let (declared_tracks, declared_discs) =
                totals.get(&local.path).copied().unwrap_or_default();
            let local_max = tracks
                .iter()
                .filter(|t| t.disc_number == local.disc_number)
                .map(|t| t.track_number)
                .max()
                .unwrap_or(0);
            let local_discs = tracks.iter().map(|t| t.disc_number).max().unwrap_or(0);
            let valid_total = declared_tracks > 0 && declared_tracks >= local_max;
            let valid_discs = declared_discs > 0 && declared_discs >= local_discs;
            if (valid_total && counts[&local.disc_number] != declared_tracks)
                || (valid_discs && max_disc != declared_discs)
                || (!complete && (!valid_total || (!valid_discs && max_disc != 1)))
            {
                return None;
            }
            let local_isrc = clean_isrc(local.isrc.as_deref()).filter(|s| !s.is_empty());
            let remote_isrc = clean_isrc(target.isrc.as_deref()).filter(|s| !s.is_empty());
            if local_isrc
                .as_ref()
                .zip(remote_isrc.as_ref())
                .is_some_and(|(l, r)| l != r)
            {
                conflicting_isrc_paths.insert(local.path.clone());
            }
            if let Some(linked) = self.active_links.get(&local.path) {
                if linked.get(&release.id) != Some(&target.id) {
                    return None;
                }
                if self.saved_links.get(&local.path).and_then(|(saved, _)| positive_id(&saved["ids"]["album_id"])) == Some(release.id.clone()) {
                    primary_anchor_count += 1;
                }
                if local_isrc
                    .as_ref()
                    .zip(remote_isrc.as_ref())
                    .is_some_and(|(l, r)| l == r)
                {
                    exact_anchor_isrcs.insert(local_isrc.clone().unwrap());
                }
            }
            alignments.insert(
                local.path.clone(),
                TrackAlignment {
                    local_path: local.path.clone(),
                    remote_track_id: target.id.clone(),
                    disc_number: pos.0,
                    track_number: pos.1,
                },
            );
        }

        let exact_anchors = exact_anchor_isrcs.len();
        if !conflicting_isrc_paths.is_empty()
            && (!complete
                || exact_anchors < 2
                || tracks.iter().any(|t| {
                    crate::matching::title_key(&t.artist)
                        != crate::matching::title_key(&release.artist)
                }))
        {
            return None;
        }
        let missing: Vec<_> = release
            .tracks
            .iter()
            .filter(|t| !local_positions.contains(&(t.disc_number, t.track_number)))
            .map(|t| t.id.clone())
            .collect();
        Some(ScanAnchoredRelease {
            evidence: if conflicting_isrc_paths.is_empty() {
                "Saved linked recordings confirm the release; titles, durations, positions and totals match".into()
            } else {
                format!("Complete release verified against {exact_anchors} independently linked ISRC matches; {} differing local ISRC tag(s) retained", conflicting_isrc_paths.len())
            },
            structure: StructureMatchResult {
                compatible: true,
                incomplete: !missing.is_empty(),
                matched_count: tracks.len(),
                total_remote_tracks: release.tracks.len(),
                conflicts: Vec::new(),
                alignments,
                missing_remote_track_ids: missing,
            },
            release,
            conflicting_isrc_paths,
            primary_anchor_count,
        })
    }
}

pub struct AnchoredRelease {
    pub release: TidalRelease,
    pub files: Vec<LocalFileRecord>,
    // A manual track placement can support reviewed numbering proposals, but
    // must not silently authorize linking the other files in its release.
    allow_link_propagation: bool,
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
        let allow_link_propagation = chosen
            .iter()
            .any(|(_, anchor)| anchor["scope"] == "release");
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
                allow_link_propagation,
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
        if !group.allow_link_propagation {
            continue;
        }
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
    use crate::release_matching::LocalTrackInfo;
    use serde_json::json;

    #[tokio::test]
    async fn scan_uses_existing_recordings_for_complete_release_without_changing_links() {
        let dir = std::env::temp_dir().join(format!("scan-anchor-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let mut local = Vec::new();
        let mut totals = HashMap::new();
        let mut release = TidalRelease {
            id: "555276547".into(),
            artist: "Flight Facilities".into(),
            title: "Down to Earth".into(),
            tracks_loaded: true,
            track_count: 15,
            available: Some(true),
            ..Default::default()
        };
        for n in 1..=14 {
            let title = match n {
                1 => "Intro".to_string(),
                2 => "Two Bodies".to_string(),
                6 => "Apollo".to_string(),
                _ => format!("Recording {n}"),
            };
            let isrc = match n {
                1 => "AUFF01400613".to_string(),
                2 => "AUFF01400582".to_string(),
                6 => "AUFF01400617".to_string(),
                _ => format!("AUFF014000{n:02}"),
            };
            let duration = if n == 2 { 368.0 } else { 180.0 };
            release.tracks.push(crate::tidal::TidalTrack {
                id: (555276550 + n).to_string(),
                title: title.clone(),
                duration,
                isrc: Some(isrc.clone()),
                disc_number: 1,
                track_number: n,
                ..Default::default()
            });
            let path = format!("/music/Flight Facilities/Down to Earth (2014)/{n:02}.flac");
            let local_isrc = if n == 2 {
                "QZFZ62138178".to_string()
            } else {
                isrc
            };
            let local_duration = if n == 2 { 368.346 } else { duration };
            let metadata = json!({"albumartist":release.artist,"album":release.title,"title":title,"isrc":local_isrc,"duration":local_duration,"discnumber":"01/01","tracknumber":format!("{n:02}/14")});
            db.apply_file_update(&path, &path, "/music", &metadata, 10, 20)
                .await
                .unwrap();
            totals.insert(path.clone(), (14, 1));
            local.push(LocalTrackInfo {
                path,
                title,
                artist: release.artist.clone(),
                album: release.title.clone(),
                duration: local_duration,
                isrc: Some(local_isrc),
                disc_number: 1,
                track_number: n,
            });
        }
        db.set_preference(
            "tag-review:GB:555276547",
            &serde_json::to_value(&release).unwrap(),
        )
        .await
        .unwrap();
        db.set_preference("release-live:GB:555276547",&json!({"available":true,"checked_at":chrono::Utc::now().timestamp()-5*86400,"source":"album_lookup"})).await.unwrap();
        for index in [0, 5] {
            db.save_track_link(&local[index].path,"GB","[0,0,10,20]",&json!({"status":"linked","manual":true,"scope":"track","ids":{"album_id":555276547,"track_id":release.tracks[index].id}}).to_string()).await.unwrap();
        }
        let review =
            json!({"status":"review","note":"Track position unverified on candidate release"});
        db.save_track_link(&local[1].path, "GB", "[0,0,10,20]", &review.to_string())
            .await
            .unwrap();
        let revision = db.revision.load(std::sync::atomic::Ordering::SeqCst);
        let cache = ScanAnchorCache::load(&db, "GB", "/music").await.unwrap();
        let result = cache.match_group(&local, &totals);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].structure.alignments[&local[1].path].remote_track_id,
            "555276552"
        );
        assert_eq!(
            result[0].structure.total_remote_tracks, 14,
            "Provider count can include a video; audio positions own cardinality"
        );
        assert_eq!(
            result[0].conflicting_isrc_paths,
            HashSet::from([local[1].path.clone()])
        );
        assert!(result[0]
            .evidence
            .contains("2 independently linked ISRC matches"));
        assert!(!crate::release_matching::recording_matches(
            &local[1].title,
            local[1].duration,
            local[1].isrc.as_deref(),
            &release.tracks[1].title,
            release.tracks[1].duration,
            release.tracks[1].isrc.as_deref(),
            true
        ));
        assert_eq!(
            ScanAnchorCache::load(&db, "GB", "/music")
                .await
                .unwrap()
                .saved_links[&local[1].path]
                .0,
            review
        );
        assert_eq!(
            db.revision.load(std::sync::atomic::Ordering::SeqCst),
            revision,
            "An evidence scan cannot write links"
        );
        assert!(ScanAnchorCache::load(&db, "US", "/music")
            .await
            .unwrap()
            .match_group(&local, &totals)
            .is_empty());

        // An additional cached edition can carry a newer withdrawal than its
        // stored positive check, even though it is not itself an anchor.
        let now = chrono::Utc::now().timestamp();
        db.set_preference("release-live:GB:910002",&json!({"available":true,"checked_at":now-60,"source":"album_lookup"})).await.unwrap();
        let mut other_cache = ScanAnchorCache::load(&db,"GB","/music").await.unwrap();
        other_cache.extend_live_cache(&db,"GB",&["910002".to_string()]).await.unwrap();
        let other = TidalRelease {id:"910002".into(),available:Some(false),availability_checked_at:Some(now),availability_source:Some("artist_list".into()),..Default::default()};
        assert!(!other_cache.is_release_confirmed_live(&other));

        // Incomplete inventory supports ordinary strict recording matches, but
        // cannot explain a contradictory ISRC using whole-release context.
        let strict_subset = vec![local[0].clone(), local[5].clone()];
        let matched = cache.match_group(&strict_subset, &totals);
        assert_eq!(matched.len(), 1);
        assert!(matched[0].structure.incomplete);
        assert_eq!(matched[0].structure.missing_remote_track_ids.len(), 12);
        assert!(cache.match_group(&local[..6], &totals).is_empty());
        let mut summary_only = ScanAnchorCache::load(&db, "GB", "/music").await.unwrap();
        summary_only
            .releases
            .get_mut(&release.id)
            .unwrap()
            .tracks_loaded = false;
        assert_eq!(summary_only.candidate_releases(&local).len(), 1);
        assert!(summary_only.match_group(&local, &totals).is_empty());
        summary_only.merge_catalogues(std::iter::once(&release));
        assert_eq!(summary_only.match_group(&local, &totals).len(), 1);
        // Broken 04/1 totals do not veto complete physical/audio-set proof.
        let broken: HashMap<_, _> = totals.keys().map(|path| (path.clone(), (1, 1))).collect();
        assert_eq!(cache.match_group(&local, &broken).len(), 1);
        let wrong: HashMap<_, _> = totals.keys().map(|path| (path.clone(), (15, 1))).collect();
        assert!(cache.match_group(&local, &wrong).is_empty());

        let mut changed = local.clone();
        changed[1].duration += 3.0;
        assert!(cache.match_group(&changed, &totals).is_empty());
        changed = local.clone();
        changed[1].title.push_str(" (Extended Mix)");
        assert!(cache.match_group(&changed, &totals).is_empty());
        changed = local.clone();
        changed[1].track_number = 3;
        assert!(cache.match_group(&changed, &totals).is_empty());
        let mut wrong_links = ScanAnchorCache::load(&db, "GB", "/music").await.unwrap();
        wrong_links
            .active_links
            .get_mut(&local[5].path)
            .unwrap()
            .insert(release.id.clone(), "999".into());
        assert!(wrong_links.match_group(&local, &totals).is_empty());
        wrong_links = ScanAnchorCache::load(&db, "GB", "/music").await.unwrap();
        wrong_links.active_links.remove(&local[5].path);
        assert!(
            wrong_links.match_group(&local, &totals).is_empty(),
            "One exact anchor cannot waive a conflicting ISRC"
        );
        wrong_links.observe_checked_availability(&release.id, false);
        assert!(wrong_links.match_group(&strict_subset, &totals).is_empty());
        let mut rolling = ScanAnchorCache::load(&db, "GB", "/music").await.unwrap();
        rolling
            .releases
            .get_mut(&release.id)
            .unwrap()
            .tracks
            .push(crate::tidal::TidalTrack {
                id: "555276599".into(),
                title: "Bonus".into(),
                disc_number: 1,
                track_number: 15,
                duration: 180.0,
                ..Default::default()
            });
        assert!(
            rolling.match_group(&local, &totals).is_empty(),
            "A later rolling edition cannot absorb an initial-release link"
        );
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

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
            &json!({"path":files[0].path,"album_id":123,"track_id":1,"market":"GB"}),
        )
        .await
        .unwrap();
        assert_eq!(verified(&db, &files, "GB").await.unwrap().len(), 1);
        // Selecting one track retains safe numbering proposals, but neither
        // immediate nor later maintenance propagation may link its siblings.
        assert_eq!(propagate(&db, &files[0].path, "GB").await.unwrap(), 0);
        assert_eq!(propagate_files(&db, &files, "GB").await.unwrap(), 0);
        let detail = db
            .get_detail(&json!({"path":files[0].path,"market":"GB"}))
            .await
            .unwrap();
        assert_eq!(detail["linked_ids"]["album_id"], "123");
        assert_eq!(detail["linked_ids"]["track_id"], "1");
        assert!(db
            .get_detail(&json!({"path":files[1].path,"market":"GB"}))
            .await
            .unwrap()["linked_ids"]
            .is_null());
        let plans = crate::workflows::plan_cached(&db, &files, "numbers", None)
            .await
            .unwrap();
        assert_eq!(plans.len(), 4);
        for plan in plans {
            assert_eq!(plan.changes["tracktotal"], "04");
            assert_eq!(plan.changes["disctotal"], "01");
            assert!(plan.target.is_none());
        }
        db.choose_track_link(&json!({"path":files[0].path,"album_id":"123","track_id":"1","market":"GB","scope":"release"})).await.unwrap();
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
