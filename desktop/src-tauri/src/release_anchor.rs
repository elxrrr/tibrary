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
    aliases: crate::release_matching::CreditAliases,
}

pub struct ScanAnchoredRelease {
    pub release: TidalRelease,
    pub structure: crate::release_matching::StructureMatchResult,
    pub evidence: String,
    pub conflicting_isrc_paths: HashSet<String>,
    pub primary_anchor_count: usize,
    pub contextual_paths: HashMap<String, String>,
    pub review_reasons: HashMap<String, String>,
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
    #[cfg(test)]
    pub async fn load(db: &TursoDb, market: &str, root: &str) -> Result<Self, String> {
        let mut result = Self::load_link_state(db, market, root).await?;
        let paths: Vec<_> = result.active_links.keys().cloned().collect();
        result
            .hydrate_for_paths(db, market, paths.iter().map(String::as_str))
            .await?;
        Ok(result)
    }

    /// Eligibility needs only link, file-stamp and ignore state. Defer large
    /// release/credit payloads until the requested local release groups exist.
    pub async fn load_link_state(db: &TursoDb, market: &str, root: &str) -> Result<Self, String> {
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
            aliases: crate::release_matching::CreditAliases::new(),
        };
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
        Ok(result)
    }

    /// Retain every linked sibling in the eligible groups, while unrelated
    /// library releases need neither metadata hydration nor live-cache reads.
    /// Global catalogue aliases are still supplied separately by the caller.
    pub async fn hydrate_for_tracks<'a>(
        &mut self,
        db: &TursoDb,
        market: &str,
        tracks: impl IntoIterator<Item = &'a crate::release_matching::LocalTrackInfo>,
    ) -> Result<(), String> {
        self.hydrate_for_paths(
            db,
            market,
            tracks.into_iter().map(|track| track.path.as_str()),
        )
        .await
    }

    async fn hydrate_for_paths<'a>(
        &mut self,
        db: &TursoDb,
        market: &str,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), String> {
        let paths: HashSet<_> = paths.into_iter().collect();
        self.active_links
            .retain(|path, _| paths.contains(path.as_str()));
        let release_ids: HashSet<_> = self
            .active_links
            .values()
            .flat_map(|links| links.keys().cloned())
            .collect();
        self.releases.retain(|id, _| release_ids.contains(id));
        self.live.retain(|id, _| release_ids.contains(id));
        if !release_ids.is_empty() {
            let conn = db.connect()?;
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
                    self.live.insert(id, value);
                } else if let Ok(release) = serde_json::from_value::<TidalRelease>(value) {
                    if release.id == id {
                        self.releases.insert(id, release);
                    }
                }
            }
        }
        self.aliases.extend_releases(self.releases.values());
        Ok(())
    }

    /// Reuse the caller's already-loaded market catalogue; no second scan or API.
    pub fn merge_catalogues<'a>(&mut self, releases: impl IntoIterator<Item = &'a TidalRelease>) {
        let releases: Vec<_> = releases.into_iter().collect();
        self.aliases.extend_releases(releases.iter().copied());
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

    pub fn begin_catalogue_references(&mut self) {
        self.aliases.begin_batch();
    }

    pub fn finish_catalogue_references(&mut self) {
        self.aliases.finish_batch();
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

    /// Preserve current sibling choices throughout ordinary and contextual
    /// matching. An additional edition can supply metadata only when its full
    /// audio sequence is equivalent to the chosen edition.
    pub fn respects_existing_choices(
        &self,
        tracks: &[crate::release_matching::LocalTrackInfo],
        candidate: &TidalRelease,
        structure: &crate::release_matching::StructureMatchResult,
    ) -> bool {
        tracks.iter().all(|local| {
            let Some(placements) = self.active_links.get(&local.path) else {
                return true;
            };
            let Some(alignment) = structure.alignments.get(&local.path) else {
                return false;
            };
            if let Some(chosen) = placements.get(&candidate.id) {
                return chosen == &alignment.remote_track_id;
            }
            placements.iter().any(|(id, chosen)| {
                let Some(release) = self.releases.get(id) else {
                    return false;
                };
                self.is_release_confirmed_live(release)
                    && crate::release_context::equivalent_editions(release, candidate)
                    && release.tracks.iter().any(|track| {
                        track.id == *chosen
                            && track.disc_number == alignment.disc_number
                            && track.track_number == alignment.track_number
                    })
            })
        })
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
        self.evaluate_release(tracks, totals, &release)
    }

    pub(crate) fn evaluate_release(
        &self,
        tracks: &[crate::release_matching::LocalTrackInfo],
        totals: &HashMap<String, (u32, u32)>,
        release: &TidalRelease,
    ) -> Option<ScanAnchoredRelease> {
        if !self.is_release_confirmed_live(release) {
            return None;
        }
        crate::release_context::resolve(
            tracks,
            totals,
            release,
            &self.aliases,
            &self.active_links,
            &self.saved_links,
        )
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
            "SELECT path,payload,stamp FROM track_links WHERE market=?",
            (market,),
        )
        .await
        .map_err(|e| e.to_string())?;
    let files_by_path: HashMap<_, _> = files
        .iter()
        .filter(|file| file.present)
        .map(|file| (file.path.as_str(), file))
        .collect();
    let mut anchors = HashMap::new();
    let mut current_links = HashMap::new();
    while let Some(row) = query.next().await.map_err(|e| e.to_string())? {
        let path: String = row.get(0).map_err(|e| e.to_string())?;
        let Some(file) = files_by_path.get(path.as_str()) else {
            continue;
        };
        let raw: String = row.get(1).map_err(|e| e.to_string())?;
        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
            let stamp: String = row.get(2).unwrap_or_default();
            if !crate::db::link_stamp_matches(&stamp, file.size, file.mtime)
                || !(value["status"] == "linked"
                    || value["status"] == "resolved"
                    || value["status"].is_null())
                || positive_id(&value["ids"]["album_id"]).is_none()
                || positive_id(&value["ids"]["track_id"]).is_none()
            {
                continue;
            }
            if value["manual"] == true {
                anchors.insert(path.clone(), value.clone());
            }
            current_links.insert(path, value);
        }
    }
    drop(query);
    // Read the relevant release snapshots together, rather than repeating a
    // database lookup for every album while preparing number corrections.
    let keys: Vec<_> = groups
        .values()
        .filter(|peers| {
            peers.iter().any(|file| anchors.contains_key(&file.path))
                || peers
                    .iter()
                    .filter(|file| current_links.contains_key(&file.path))
                    .count()
                    >= 2
        })
        .flat_map(|peers| {
            peers.iter().filter_map(|file| {
                current_links
                    .get(&file.path)
                    .and_then(|saved| positive_id(&saved["ids"]["album_id"]))
                    .map(|id| format!("tag-review:{market}:{id}"))
            })
        })
        .collect();
    let cached_releases: HashMap<String, TidalRelease> =
        crate::db::preference_snapshots_for_keys(&conn, keys)
            .await?
            .into_iter()
            .filter_map(|(_, value)| serde_json::from_value::<TidalRelease>(value).ok())
            .map(|release| (release.id.clone(), release))
            .collect();
    let mut aliases = crate::release_matching::CreditAliases::new();
    aliases.extend_releases(cached_releases.values());
    let mut result = Vec::new();
    for (_, mut peers) in groups {
        let manual_chosen: Vec<_> = peers
            .iter()
            .filter_map(|p| anchors.get(&p.path).map(|a| (&p.path, a)))
            .collect();
        let allow_link_propagation = manual_chosen
            .iter()
            .any(|(_, anchor)| anchor["scope"] == "release");
        let require_independent_anchors = manual_chosen.is_empty();
        let chosen: Vec<_> = if require_independent_anchors {
            peers
                .iter()
                .filter_map(|file| {
                    current_links
                        .get(&file.path)
                        .map(|saved| (&file.path, saved))
                })
                .collect()
        } else {
            manual_chosen
        };
        if require_independent_anchors && chosen.len() < 2 {
            continue;
        }
        let ids: HashSet<_> = chosen
            .iter()
            .filter_map(|(_, a)| positive_id(&a["ids"]["album_id"]))
            .collect();
        if ids.len() != 1 {
            continue;
        }
        let id = ids.iter().next().unwrap();
        let mut release = match cached_releases.get(id) {
            Some(release) => release.clone(),
            None => {
                let raw = db
                    .get_detail(&serde_json::json!({"release_id":id,"market":market}))
                    .await?;
                let Ok(release) = serde_json::from_value::<TidalRelease>(raw) else {
                    continue;
                };
                aliases.extend_releases(std::iter::once(&release));
                release
            }
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
        let positions: HashSet<_> = peers.iter().map(position).collect();
        let paths: HashSet<_> = peers.iter().map(|file| file.path.as_str()).collect();
        let remote_positions: HashSet<_> = release
            .tracks
            .iter()
            .map(|track| (track.disc_number, track.track_number))
            .collect();
        let remote_ids: HashSet<_> = release
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect();
        let remote_discs = release
            .tracks
            .iter()
            .map(|track| track.disc_number)
            .max()
            .unwrap_or(0);
        let remote_valid = remote_positions.len() == release.tracks.len()
            && remote_ids.len() == release.tracks.len()
            && release.tracks.iter().all(|track| {
                track.disc_number > 0 && track.track_number > 0 && !track.id.is_empty()
            })
            && (1..=remote_discs).all(|disc| {
                let count = release
                    .tracks
                    .iter()
                    .filter(|track| track.disc_number == disc)
                    .count() as u32;
                count > 0 && (1..=count).all(|index| remote_positions.contains(&(disc, index)))
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
        if !remote_valid
            || !contiguous
            || positions.len() != peers.len()
            || paths.len() != peers.len()
            || peers.iter().any(|file| position(file).1 == 0)
        {
            continue;
        }
        let linked_target_agrees = |file: &LocalFileRecord, track: &crate::tidal::TidalTrack| {
            current_links.get(&file.path).is_none_or(|saved| {
                positive_id(&saved["ids"]["album_id"]).as_ref() == Some(id)
                    && positive_id(&saved["ids"]["track_id"]).as_deref() == Some(track.id.as_str())
            })
        };
        let strict_recording_matches =
            |file: &LocalFileRecord, track: &crate::tidal::TidalTrack| {
                let tags = crate::workflows::extract_tags_map(&file.metadata);
                let duration = file
                    .metadata
                    .as_ref()
                    .and_then(|m| m["duration"].as_f64())
                    .unwrap_or(0.0);
                duration.is_finite()
                    && track.duration.is_finite()
                    && crate::release_matching::recording_matches(
                        tags.get("title").map(String::as_str).unwrap_or(""),
                        duration,
                        tags.get("isrc").map(String::as_str),
                        &track.title,
                        track.duration,
                        track.isrc.as_deref(),
                        true,
                    )
            };
        // Preserve existing exact sequence/disc-layout corrections when an ISRC
        // is absent. Reordering recordings needs a stronger unique ISRC bijection.
        let sequence_valid = peers.iter().zip(&release.tracks).all(|(file, track)| {
            strict_recording_matches(file, track)
                && linked_target_agrees(file, track)
                && release
                    .tracks
                    .iter()
                    .filter(|other| strict_recording_matches(file, other))
                    .count()
                    == 1
        });
        let mut ordered = Vec::new();
        if sequence_valid {
            ordered = peers;
        } else {
            let mut by_online_position = HashMap::new();
            for file in peers {
                let tags = crate::workflows::extract_tags_map(&file.metadata);
                let local_isrc =
                    crate::release_matching::clean_isrc(tags.get("isrc").map(String::as_str))
                        .filter(|isrc| !isrc.is_empty());
                let duration = file
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata["duration"].as_f64())
                    .unwrap_or(0.0);
                let matches: Vec<_> = release
                    .tracks
                    .iter()
                    .filter(|track| {
                        let online_isrc =
                            crate::release_matching::clean_isrc(track.isrc.as_deref())
                                .filter(|isrc| !isrc.is_empty());
                        local_isrc
                            .as_ref()
                            .zip(online_isrc.as_ref())
                            .is_some_and(|(a, b)| a == b)
                            && duration.is_finite()
                            && track.duration.is_finite()
                            && duration > 0.0
                            && track.duration > 0.0
                            && (duration - track.duration).abs() <= 3.0
                            && aliases.titles_match(
                                tags.get("title").map(String::as_str).unwrap_or(""),
                                track,
                                false,
                            )
                    })
                    .collect();
                if matches.len() != 1
                    || !linked_target_agrees(&file, matches[0])
                    || by_online_position
                        .insert((matches[0].disc_number, matches[0].track_number), file)
                        .is_some()
                {
                    by_online_position.clear();
                    break;
                }
            }
            if by_online_position.len() == release.tracks.len() {
                ordered = release
                    .tracks
                    .iter()
                    .map(|track| {
                        by_online_position
                            .remove(&(track.disc_number, track.track_number))
                            .unwrap()
                    })
                    .collect();
            }
        }
        let independent_anchors: HashSet<_> = ordered
            .iter()
            .zip(&release.tracks)
            .filter_map(|(file, track)| {
                if !current_links.contains_key(&file.path) {
                    return None;
                }
                let tags = crate::workflows::extract_tags_map(&file.metadata);
                let local =
                    crate::release_matching::clean_isrc(tags.get("isrc").map(String::as_str))
                        .filter(|isrc| !isrc.is_empty())?;
                let online = crate::release_matching::clean_isrc(track.isrc.as_deref())
                    .filter(|isrc| !isrc.is_empty())?;
                (local == online).then_some(local)
            })
            .collect();
        if ordered.len() == release.tracks.len()
            && (!require_independent_anchors || independent_anchors.len() >= 2)
        {
            result.push(AnchoredRelease {
                release,
                files: ordered,
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
        let mut deferred = ScanAnchorCache::load_link_state(&db, "GB", "/music")
            .await
            .unwrap();
        assert!(deferred.releases.is_empty() && deferred.live.is_empty());
        deferred
            .hydrate_for_tracks(&db, "GB", local.iter())
            .await
            .unwrap();
        assert_eq!(deferred.releases.len(), 1);
        assert_eq!(deferred.match_group(&local, &totals).len(), 1);
        let saved_count = deferred.saved_links.len();
        deferred
            .hydrate_for_tracks(&db, "GB", [&local[1]])
            .await
            .unwrap();
        assert!(
            deferred.active_links.is_empty()
                && deferred.releases.is_empty()
                && deferred.live.is_empty()
        );
        assert_eq!(
            deferred.saved_links.len(),
            saved_count,
            "Scoped hydration must preserve the original eligibility snapshot"
        );
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
            .contains("2 verified existing sibling link(s)"));
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
        db.set_preference(
            "release-live:GB:910002",
            &json!({"available":true,"checked_at":now-60,"source":"album_lookup"}),
        )
        .await
        .unwrap();
        let mut other_cache = ScanAnchorCache::load(&db, "GB", "/music").await.unwrap();
        other_cache
            .extend_live_cache(&db, "GB", &["910002".to_string()])
            .await
            .unwrap();
        let other = TidalRelease {
            id: "910002".into(),
            available: Some(false),
            availability_checked_at: Some(now),
            availability_source: Some("artist_list".into()),
            ..Default::default()
        };
        assert!(!other_cache.is_release_confirmed_live(&other));

        // Incomplete inventory supports ordinary strict recording matches, but
        // cannot explain a contradictory ISRC using whole-release context.
        let strict_subset = vec![local[0].clone(), local[5].clone()];
        let matched = cache.match_group(&strict_subset, &totals);
        assert_eq!(matched.len(), 1);
        assert!(matched[0].structure.incomplete);
        assert_eq!(matched[0].structure.missing_remote_track_ids.len(), 12);
        let partial = cache.match_group(&local[..6], &totals);
        assert_eq!(partial.len(), 1);
        assert!(!partial[0].structure.alignments.contains_key(&local[1].path));
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
        let partial = cache.match_group(&changed, &totals);
        assert_eq!(partial.len(), 1);
        assert!(!partial[0].structure.alignments.contains_key(&local[1].path));
        assert!(partial[0].review_reasons[&local[1].path].contains("duration differs"));
        changed = local.clone();
        changed[1].title.push_str(" (Extended Mix)");
        let partial = cache.match_group(&changed, &totals);
        assert_eq!(partial.len(), 1);
        assert!(!partial[0].structure.alignments.contains_key(&local[1].path));
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
        assert_eq!(
            wrong_links.match_group(&local, &totals).len(),
            1,
            "A strong complete-release majority can establish context without two prior choices"
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
    async fn complete_reordered_release_offers_reviewed_numbers_from_unique_recordings() {
        let directory =
            std::env::temp_dir().join(format!("reordered-numbers-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(directory.join("db")).await.unwrap();
        let remote_titles = [
            "Heaven (Lenno Remix)",
            "Favorite Sound (BRKLYN Remix)",
            "Favorite Sound (Win and Woo Remix)",
            "Buzzing (Codeko Remix)",
            "See You on the Other Side (Upmost Remix)",
        ];
        let release = TidalRelease {
            id: "448619412".into(),
            artist: "Artist".into(),
            title: "Remixes".into(),
            tracks_loaded: true,
            tracks: remote_titles
                .iter()
                .enumerate()
                .map(|(index, title)| crate::tidal::TidalTrack {
                    id: format!("44861941{}", index + 3),
                    title: (*title).into(),
                    duration: 180.0 + index as f64,
                    isrc: Some(format!("GBTEST26000{index}")),
                    disc_number: 1,
                    track_number: index as u32 + 1,
                    artists: vec![
                        json!({"name":"Maty Noyes","id":6699993}),
                        json!({"name":"Echosmith","id":4624156}),
                        json!({"name":"Nevve","id":8014921}),
                    ],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let local_to_remote = [0usize, 3, 4, 1, 2];
        let local_titles = [
            "Heaven (feat. Maty Noyes) [Lenno Remix]",
            "Buzzing (with Nevve) [Codeko Remix]",
            "See You on the Other Side [Upmost Remix]",
            "Favorite Sound (with Echosmith) [BRKLYN Remix]",
            "Favorite Sound (with Echosmith) [Win and Woo Remix]",
        ];
        let mut files = Vec::new();
        for (index, remote_index) in local_to_remote.iter().enumerate() {
            let remote = &release.tracks[*remote_index];
            let path = format!("/fixture/Artist/Remixes/{:02}.flac", index + 1);
            let metadata = json!({"albumartist":"Artist","album":"Remixes","title":local_titles[index],"duration":remote.duration+0.2,"isrc":remote.isrc,"tracknumber":format!("{:02}",index+1),"tracktotal":"01","discnumber":"01","disctotal":"01"});
            db.apply_file_update(&path, &path, "/fixture", &metadata, 10, 20)
                .await
                .unwrap();
            files.push(LocalFileRecord {
                path,
                root: "/fixture".into(),
                size: 10,
                mtime: 20,
                metadata: Some(metadata),
                error: None,
                present: true,
            });
        }
        db.set_preference("tag-review:GB:448619412", &json!(release))
            .await
            .unwrap();
        let manual = json!({"status":"linked","manual":true,"scope":"track","ids":{"album_id":448619412,"track_id":448619417}});
        db.save_track_link(&files[2].path, "GB", "[0,0,10,20]", &manual.to_string())
            .await
            .unwrap();
        let groups = verified(&db, &files, "GB").await.unwrap();
        assert_eq!(groups.len(), 1);
        assert!(!groups[0].allow_link_propagation);
        assert_eq!(
            groups[0]
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                files[0].path.as_str(),
                files[3].path.as_str(),
                files[4].path.as_str(),
                files[1].path.as_str(),
                files[2].path.as_str()
            ]
        );
        let plans = crate::workflows::plan_cached(&db, &files, "numbers", None)
            .await
            .unwrap();
        for (index, file) in files.iter().enumerate() {
            let plan = plans.iter().find(|plan| plan.path == file.path).unwrap();
            assert_eq!(plan.changes["tracktotal"], "05");
            if local_to_remote[index] + 1 != index + 1 {
                assert_eq!(
                    plan.changes["tracknumber"],
                    format!("{:02}", local_to_remote[index] + 1)
                );
            }
            assert!(
                plan.target.is_none(),
                "Numbering review leaves all files in place"
            );
        }
        assert_eq!(
            propagate_files(&db, &files, "GB").await.unwrap(),
            0,
            "A one-track choice never authorizes sibling propagation"
        );
        let connection = db.connect().unwrap();
        let mut row = connection
            .query(
                "SELECT payload FROM track_links WHERE path=?",
                (files[2].path.as_str(),),
            )
            .await
            .unwrap();
        let saved: Value =
            serde_json::from_str(&row.next().await.unwrap().unwrap().get::<String>(0).unwrap())
                .unwrap();
        assert_eq!(saved, manual, "Reviewing numbering is read-only");
        drop(row);
        let automatic =
            json!({"status":"linked","ids":{"album_id":"448619412","track_id":"448619417"}});
        db.save_track_link(&files[2].path, "GB", "[0,0,10,20]", &automatic.to_string())
            .await
            .unwrap();
        assert!(
            verified(&db, &files, "GB").await.unwrap().is_empty(),
            "One automatic recording cannot establish a numbering correction"
        );
        db.save_track_link(
            &files[1].path,
            "GB",
            "[0,0,10,20]",
            &json!({"status":"linked","ids":{"album_id":"448619412","track_id":"448619416"}})
                .to_string(),
        )
        .await
        .unwrap();
        let automatic_groups = verified(&db, &files, "GB").await.unwrap();
        assert_eq!(
            automatic_groups.len(),
            1,
            "Two independent automatic recording links support a reviewed full-release correction"
        );
        assert!(!automatic_groups[0].allow_link_propagation);
        files[1].metadata.as_mut().unwrap()["isrc"] = json!("WRONG");
        assert!(
            verified(&db, &files, "GB").await.unwrap().is_empty(),
            "Reordering requires every recording to match uniquely by ISRC"
        );
        files[1].metadata.as_mut().unwrap()["isrc"] = json!(release.tracks[3].isrc);
        connection
            .execute(
                "UPDATE track_links SET stamp='[0,0,10,19]' WHERE path=?",
                (files[2].path.as_str(),),
            )
            .await
            .unwrap();
        assert!(
            verified(&db, &files, "GB").await.unwrap().is_empty(),
            "Stale manual links cannot establish numbering corrections"
        );
        drop(connection);
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
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
