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

// A wrong Album Artist must not override exact recording evidence. This only
// supplies candidates; whole-release totals and positions still decide linking.
fn exact_recording_release(tracks: &[LocalTrackInfo], release: &TidalRelease) -> bool {
    release.available != Some(false) && release.tracks_loaded && !tracks.is_empty()
        && tracks.iter().all(|local| {
            let Some(isrc) = local.isrc.as_deref().filter(|s| !s.trim().is_empty()) else { return false; };
            release.tracks.iter().any(|remote| remote.isrc.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(isrc))
                && crate::release_matching::recording_matches(&local.title, local.duration, Some(isrc), &remote.title, remote.duration, remote.isrc.as_deref(), true))
        })
}

// Physical release boundaries must be identical for scan scoping and matching.
fn release_folder(path: &str, root: &str) -> String {
    let mut folder = std::path::Path::new(path)
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
    folder.display().to_string()
}

fn candidate_option(
    release: &TidalRelease,
    matched: &crate::release_matching::StructureMatchResult,
    local: &LocalTrackInfo,
    shared_credits: usize,
    context: Option<&crate::release_anchor::ScanAnchoredRelease>,
) -> Option<Value> {
    let path = local.path.as_str();
    let review_placement = context.and_then(|context| context.review_reasons.get(path)).and_then(|_| {
        release.tracks.iter().find(|track| track.disc_number == local.disc_number && track.track_number == local.track_number)
    }).map(|track| crate::release_matching::TrackAlignment {local_path:local.path.clone(),remote_track_id:track.id.clone(),disc_number:track.disc_number,track_number:track.track_number});
    let alignment = matched.alignments.get(path).or(review_placement.as_ref())?;
    let safe = matched.compatible && matched.alignments.contains_key(path);
    let mut evidence = matched.conflicts.clone();
    if let Some(context) = context {
        evidence.push(context.evidence.clone());
        if context.conflicting_isrc_paths.contains(path) {
            evidence.push("Local ISRC differs; verified sibling recordings and the complete release's titles, mixes, durations and positions agree".into());
        }
        if let Some(reason) = context.contextual_paths.get(path).or(context.review_reasons.get(path)) { evidence.push(reason.clone()); }
    }
    if shared_credits > 0 { evidence.push(format!("{shared_credits} shared local contributor credits support this release")); }
    let disc_total = release.tracks.iter().map(|track| track.disc_number).max().unwrap_or(1);
    let track_total = release.tracks.iter().filter(|track| track.disc_number == alignment.disc_number).count();
    Some(json!({"id":release.id,"title":release.title,"track_id":alignment.remote_track_id,"tracks":release.track_count,"artist":release.artist,"album":release.title,
        "position_label":format!("Disc {:02}/{disc_total:02} · Track {:02}/{track_total:02}",alignment.disc_number,alignment.track_number),
        "evidence":evidence.join("; "),"structure":{"compatible":safe,"reasons":evidence},"compatible":safe}))
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

    // Read saved decisions first; a completed or empty scope needs no catalogue
    // decoding, and linked peers remain evidence rather than rewrite targets.
    let mut anchor_cache = crate::release_anchor::ScanAnchorCache::load_link_state(db, market, root).await?;

    // Read the cheap manifest first. A small selected/review scope should not
    // deserialize every library file merely to discover which tracks need work.
    let mut file_stmt = conn
        .query(
            "SELECT path, mtime, size FROM local_files WHERE root = ? AND present = 1 AND metadata IS NOT NULL",
            (root,),
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut manifest = Vec::new();
    let mut eligible = HashSet::new();

    while let Some(row) = file_stmt.next().await.map_err(|e| e.to_string())? {
        if cancel.load(Ordering::Relaxed) { return Ok(LinkSummary::default()); }
        let path: String = row.get(0).unwrap_or_default();
        let mtime: i64 = row.get(1).unwrap_or(0);
        let size: i64 = row.get(2).unwrap_or(0);
        let (old, stamp_current) = anchor_cache.saved_links.get(&path)
            .map(|(payload, stamp)| (payload, crate::db::link_stamp_matches(stamp, size, mtime)))
            .unwrap_or((&Value::Null, false));
        let is_ignored = anchor_cache.ignored_paths.contains(&path);
        let linked = stamp_current
            && (old["status"] == "linked"
                || old["status"] == "resolved"
                || (old["status"].is_null() && old["ids"]["track_id"].as_str().is_some()));
        if !is_ignored
            && !(cached_only && linked)
            && selected.map(|s| s.contains(&path)).unwrap_or(!linked)
            && (!editions_only
                || (!linked
                    && old["catalogue_options"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty())))
        {
            eligible.insert(path.clone());
        }
        manifest.push(path);
    }

    drop(file_stmt);
    drop(map_stmt);
    drop(add_stmt);
    let total = eligible.len();
    progress(format!("Linking recordings · 0/{total} tracks · cached links retained"));
    if total == 0 {
        return Ok(LinkSummary::default());
    }

    let eligible_folders: HashSet<_> = eligible.iter().map(|path| release_folder(path, root)).collect();
    let peer_paths: Vec<_> = manifest.into_iter()
        .filter(|path| eligible_folders.contains(&release_folder(path, root))).collect();
    let mut local_tracks = Vec::with_capacity(peer_paths.len());
    let mut file_stamps = HashMap::new();
    let mut totals = HashMap::new();
    let mut local_upcs = HashMap::new();
    let mut local_genres: HashMap<String, HashSet<String>> = HashMap::new();
    let mut local_credits = HashMap::new();
    // Include linked and ignored peers as physical release evidence, while only
    // the eligible paths may be written. Bounded primary-key reads avoid a full
    // metadata scan and retain the current root/presence safeguards.
    for paths in peer_paths.chunks(256) {
        if cancel.load(Ordering::Relaxed) { return Ok(LinkSummary { total, ..Default::default() }); }
        let placeholders = vec!["?"; paths.len()].join(",");
        let mut parameters = paths.to_vec();
        parameters.push(root.to_string());
        let mut rows = conn.query(
            format!("SELECT path,metadata,mtime,size FROM local_files WHERE path IN ({placeholders}) AND root=? AND present=1 AND metadata IS NOT NULL"),
            parameters,
        ).await.map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
            let path: String = row.get(0).unwrap_or_default();
            let meta_str: Option<String> = row.get(1).ok().flatten();
            let mtime: i64 = row.get(2).unwrap_or(0);
            let size: i64 = row.get(3).unwrap_or(0);
            file_stamps.insert(path.clone(), format!("[0,0,{},{}]", size, mtime));
            let meta: Value = meta_str.as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or(Value::Null);
            local_credits.insert(path.clone(), crate::recommendations::local_credit_names(&meta));
            let tags = crate::workflows::extract_tags_map(&Some(meta.clone()));
            let text = |key: &str| tags.get(key).cloned().unwrap_or_default();
            let number = |key: &str| text(key).split('/').next().unwrap_or("").trim().parse::<u32>().unwrap_or(0);
            let title = text("title");
            let artist = tags.get("albumartist").cloned().unwrap_or_else(|| text("artist"));
            let album = text("album");
            let duration = meta["duration"].as_f64().unwrap_or(0.0);
            let track_number = number("tracknumber");
            let disc_number = number("discnumber").max(1);
            let isrc = tags.get("isrc").cloned();
            let total = |key: &str, position: &str| {
                let declared = number(key);
                if declared > 0 { declared } else { text(position).split('/').nth(1).unwrap_or("").parse().unwrap_or(0) }
            };
            if let Some(genres) = tags.get("genre") { local_genres.insert(path.clone(),genres.split(';').map(crate::matching::name_key).filter(|genre| !genre.is_empty()).collect()); }
            if let Some(upc) = tags.get("upc").or(tags.get("barcode")) { local_upcs.insert(path.clone(),upc.clone()); }
            totals.insert(path.clone(), (total("tracktotal", "tracknumber"), total("disctotal", "discnumber")));
            local_tracks.push(LocalTrackInfo { path, title, artist, album, duration, track_number, disc_number, isrc });
        }
    }

    // 4. Group local tracks by (artist_key, album_key)
    let mut groups: HashMap<(String, String, String), Vec<LocalTrackInfo>> = HashMap::new();
    for track in local_tracks {
        let art_key = title_key(&track.artist);
        let alb_key = title_key(&track.album);
        let folder = release_folder(&track.path, root);
        groups
            .entry((art_key, alb_key, folder))
            .or_default()
            .push(track);
    }

    let mut summary = LinkSummary {
        total: eligible.len(),
        ..Default::default()
    };

    groups.retain(|_, tracks| tracks.iter().any(|track| eligible.contains(&track.path)));
    anchor_cache.hydrate_for_tracks(db, market, groups.values().flatten()).await?;
    let relevant_artists: HashSet<_> = groups.keys().filter_map(|(artist, _, _)| mappings.get(artist))
        .flatten().cloned().collect();
    let mut catalogues_by_artist: HashMap<String, Vec<TidalRelease>> = HashMap::new();
    let mut exact_fallbacks: HashMap<String, Vec<TidalRelease>> = HashMap::new();
    // Stream global credit references into a compact identity index. Retain full
    // release objects only for the current artists and exact wrong-artist fixes.
    // This preserves globally ambiguous aliases without retaining every album.
    progress(format!("Loading cached release evidence · {total} unresolved tracks · retaining relevant artist catalogues and shared contributor identities"));
    anchor_cache.begin_catalogue_references();
    let mut cat_stmt = conn.query("SELECT artist_id,payload FROM catalogue WHERE market=?", (market,)).await.map_err(|e|e.to_string())?;
    while let Some(row) = cat_stmt.next().await.map_err(|e|e.to_string())? {
        if cancel.load(Ordering::Relaxed) { return Ok(summary); }
        let artist_id: String = row.get(0).unwrap_or_default();
        let Some(raw) = row.get::<Option<String>>(1).ok().flatten() else { continue; };
        let Ok(catalogue) = serde_json::from_str::<TidalCatalogue>(&raw) else { continue; };
        anchor_cache.merge_catalogues(catalogue.releases.iter());
        if relevant_artists.contains(&artist_id) {
            catalogues_by_artist.insert(artist_id, catalogue.releases);
        } else {
            for release in catalogue.releases {
                let album_key = title_key(&release.title);
                if groups.iter().any(|((_, local_album, _), tracks)| *local_album == album_key && exact_recording_release(tracks, &release)) {
                    exact_fallbacks.entry(album_key).or_default().push(release);
                }
            }
        }
    }
    drop(cat_stmt);
    anchor_cache.finish_catalogue_references();
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
        progress(format!("Checking release · {}/{} releases · {} — {} · {} local tracks · cached candidates first", group_idx, group_count, group_tracks[0].artist, group_tracks[0].album, group_tracks.len()));

        let mut anchored_matches = anchor_cache.match_group(&group_tracks, &totals);
        let cached_anchored = !anchored_matches.is_empty();
        if cached_anchored {
            progress(format!("Reusing verified release · {group_idx}/{group_count} releases · {} — {} · existing track links and complete cached recordings agree", group_tracks[0].artist, group_tracks[0].album));
        }

        // Look up online releases for this artist
        let mut candidate_releases = anchor_cache.candidate_releases(&group_tracks);
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
                        if (candidate_title == alb_key || related_title)
                            && !candidate_releases.iter().any(|candidate| candidate.id == r.id) {
                            candidate_releases.push(r.clone());
                        }
                    }
                }
            }
        }

        // Fall back across cached artist catalogues only with exact ISRC and
        // title/duration evidence for every local recording, never artist-name similarity.
        if candidate_releases.is_empty() {
            for release in catalogues_by_artist.values().flatten().chain(exact_fallbacks.get(&alb_key).into_iter().flatten()) {
                if title_key(&release.title) == alb_key && exact_recording_release(&group_tracks, release)
                    && !candidate_releases.iter().any(|r| r.id == release.id) {
                    candidate_releases.push(release.clone());
                }
            }
        }
        if candidate_releases.is_empty() && !cached_only && selected.is_some() {
            if let Some(anchor) = group_tracks.iter().find(|t| t.isrc.as_deref().is_some_and(|s| !s.trim().is_empty())) {
                let key = format!("recording-isrc-releases:{market}:{}:{}", anchor.isrc.as_deref().unwrap(), title_key(&anchor.title));
                let cached = db.get_preference(&key).await?;
                let ids: Vec<String> = if let Some(value) = cached.filter(|v| v["checked_at"].as_i64().is_some_and(|at| chrono::Utc::now().timestamp() - at < 86400)) {
                    serde_json::from_value(value["ids"].clone()).unwrap_or_default()
                } else {
                    progress(format!("Searching exact recording · {} — {} · checking ISRC before considering another artist", anchor.artist, anchor.title));
                    let mut client = crate::tidal::TidalClient::from_db(db).await?;
                    let ids = client.releases_for_isrc(anchor.isrc.as_deref().unwrap(), market, &anchor.album).await?;
                    db.set_preference(&key, &json!({"ids":ids,"checked_at":chrono::Utc::now().timestamp()})).await?;
                    ids
                };
                for id in ids {
                    if cancel.load(Ordering::Relaxed) { return Ok(summary); }
                    let release: TidalRelease = serde_json::from_value(crate::actions::release(db, &id, market, false).await?).map_err(|e| e.to_string())?;
                    if title_key(&release.title) == alb_key && exact_recording_release(&group_tracks, &release) {
                        candidate_releases.push(release);
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
        if cached_anchored {
            // This release is already proved from current market data. Keep
            // other complete live cached editions for multi-linking, without
            // fetching irrelevant or incomplete editions to finish one track.
            let ids: Vec<_> = candidate_releases.iter().map(|release| release.id.clone()).collect();
            anchor_cache.extend_live_cache(db, market, &ids).await?;
            candidate_releases.retain(|release| release.tracks_loaded && anchor_cache.is_release_confirmed_live(release));
        } else if !cached_only && !candidate_releases.is_empty() {
            progress(format!("Checking live release placements · {group_idx}/{group_count} · {}", group_tracks[0].album));
            let ids: Vec<_> = candidate_releases.iter().map(|r|r.id.clone()).collect();
            let availability = crate::availability::check(db,&ids,market).await?;
            for (id, available) in &availability {
                anchor_cache.observe_checked_availability(id, *available == Some(true));
            }
            candidate_releases.retain(|r| availability.get(&r.id).copied().flatten()==Some(true));
            if candidate_releases.is_empty() {
                summary.review += group_tracks.iter().filter(|t|eligible.contains(&t.path)).count();
                progress(format!("No live placements confirmed · {} · existing links retained", group_tracks[0].album));
                continue;
            }
        } else {
            candidate_releases.retain(|r|r.available != Some(false));
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
                    "note": "No verified release found; check release tags and recording identifiers",
                    "checked_at": chrono::Utc::now().timestamp(),
                });
                let stamp = file_stamps
                    .get(&track.path)
                    .cloned()
                    .unwrap_or_else(|| "[]".to_string());
                let payload_str = payload.to_string();
                db.save_track_link(&track.path, market, &stamp, &payload_str).await?;
                progress(format!("Unmatched · {}/{} tracks checked · {} — {} · {} · no verified release found", summary.linked + summary.review + summary.unmatched, total, track.artist, track.title, track.album));
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
        // Loaded details and credits enter one release-context matcher, including
        // complete releases established by a majority of exact recordings.
        anchor_cache.merge_catalogues(candidate_releases.iter());
        let candidate_ids: Vec<_> = candidate_releases.iter().map(|release| release.id.clone()).collect();
        anchor_cache.extend_live_cache(db, market, &candidate_ids).await?;
        anchored_matches = candidate_releases.iter().filter_map(|release| anchor_cache.evaluate_release(&group_tracks, &totals, release)).collect();
        let bound_release = anchored_matches.iter().max_by_key(|context| (context.primary_anchor_count, context.structure.matched_count)).map(|context| &context.release);
        // Match against candidates
        let mut scored_candidates = Vec::new();
        let mut credit_scores = HashMap::new();
        let mut barcode_scores = HashMap::new();
        let mut genre_scores = HashMap::new();
        for rel in &candidate_releases {
            let mut res = anchored_matches.iter().find(|matched| matched.release.id == rel.id)
                .map(|matched| matched.structure.clone())
                .unwrap_or_else(|| structure_match(&group_tracks, rel));
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
            }
            let cardinality = crate::release_cardinality::validate_totals_with_context(
                &group_tracks, &totals, rel,
                anchored_matches.iter().any(|context| context.release.id == rel.id),
            );
            if !cardinality.valid() {
                res.compatible = false;
                res.conflicts.extend(cardinality.conflicts);
            }
            if !anchor_cache.respects_existing_choices(&group_tracks, rel, &res)
                || bound_release.is_some_and(|bound| bound.id != rel.id
                    && !crate::release_context::equivalent_editions(bound, rel)) {
                res.compatible = false;
                res.conflicts.push("A sibling establishes another release edition; this placement cannot split the local release".into());
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
                .then_with(|| {
                    let support = |id: &str| anchored_matches.iter().find(|matched| matched.release.id == id).map(|matched| matched.primary_anchor_count).unwrap_or(0);
                    support(&b.0.id).cmp(&support(&a.0.id))
                })
                .then_with(|| b.1.matched_count.cmp(&a.1.matched_count))
                .then_with(|| a.1.conflicts.len().cmp(&b.1.conflicts.len()))
                .then_with(|| barcode_scores[&b.0.id].cmp(&barcode_scores[&a.0.id]))
                .then_with(|| credit_scores[&b.0.id].cmp(&credit_scores[&a.0.id]))
                .then_with(|| a.0.replacement_id.is_some().cmp(&b.0.replacement_id.is_some()))
                .then_with(|| genre_scores[&b.0.id].cmp(&genre_scores[&a.0.id]))
        });

        let (best_rel, best_struct) = &scored_candidates[0];

        if best_struct.matched_count > 0 {
            let best_context = anchored_matches.iter().find(|context| context.release.id == best_rel.id);
            let is_perfect = best_struct.compatible
                && (best_struct.matched_count == group_tracks.len() || best_context.is_some());
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
                    if is_perfect {
                        if let Some(context) = anchored_matches.iter().find(|matched| matched.release.id == best_rel.id) {
                            payload["catalogue_note"] = json!("Verified cached release from existing track links");
                            payload["release_context"] = json!({"evidence":context.evidence,"isrc_conflict":context.conflicting_isrc_paths.contains(&track.path),"track_evidence":context.contextual_paths.get(&track.path)});
                            if context.primary_anchor_count == 0 { payload["catalogue_note"] = json!("Verified release from matching sibling recordings"); }
                        }
                    }
                    if !is_perfect {
                        payload.as_object_mut().unwrap().remove("ids");
                    } else {
                        payload["placements"] = json!(scored_candidates
                            .iter()
                            .filter(|(release, s)| s.compatible && (s.matched_count == group_tracks.len() || anchored_matches.iter().any(|context| context.release.id == release.id)))
                            .filter_map(|(r, s)| s
                                .alignments
                                .get(&track.path)
                                .map(|a| json!({"album_id":r.id,"track_id":a.remote_track_id})))
                            .collect::<Vec<_>>());
                    }
                    payload["catalogue_options"] = json!(scored_candidates.iter().filter_map(|(release, matched)| candidate_option(
                        release, matched, track, credit_scores[&release.id],
                        anchored_matches.iter().find(|context| context.release.id == release.id),
                    )).collect::<Vec<_>>());
                    let payload_str = payload.to_string();
                    db.save_track_link(&track.path, market, &stamp, &payload_str).await?;
                } else {
                    summary.review += 1;
                    let payload = json!({
                        "status": "review",
                        "catalogue_note": best_context.and_then(|context| context.review_reasons.get(&track.path)).map(String::as_str).unwrap_or("Track position unverified on candidate release"),
                        "catalogue_options": if best_context.is_some() {
                            scored_candidates.iter().filter_map(|(release, matched)| candidate_option(release, matched, track, credit_scores[&release.id], anchored_matches.iter().find(|context| context.release.id == release.id))).collect::<Vec<_>>()
                        } else { cand_options.clone() },
                        "checked_at": chrono::Utc::now().timestamp(),
                    });
                    let payload_str = payload.to_string();
                    db.save_track_link(&track.path, market, &stamp, &payload_str).await?;
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
                db.save_track_link(&track.path, market, &stamp, &payload_str).await?;
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
    async fn cached_sibling_evidence_links_only_requested_tracks_and_retains_existing_choices() {
        let dir = std::env::temp_dir().join(format!("linked-context-{}", uuid::Uuid::new_v4()));
        let store = TursoDb::open(dir.join("db")).await.unwrap();
        let mut release = TidalRelease {
            id:"910001".into(), artist:"North Assembly".into(), title:"Blue Hours".into(),
            available:Some(true), tracks_loaded:true, track_count:4, ..Default::default()
        };
        let mut paths = Vec::new();
        for index in 1..=4 {
            let path = format!("/music/North Assembly/Blue Hours/{index:02}.flac");
            let title = format!("Recording {index}");
            let isrc = format!("GBTEST24000{index}");
            release.tracks.push(TidalTrack {id:format!("9100010{index}"),title:title.clone(),duration:180.,isrc:Some(isrc.clone()),disc_number:1,track_number:index,..Default::default()});
            let tags = json!({"albumartist":"North Assembly","album":"Blue Hours","title":title,"composer":"Writer","duration":180.2,"isrc":if index == 2 {"GBOTHER240002".to_string()} else {isrc},"tracknumber":format!("{index:02}/04"),"discnumber":"01/01"});
            store.apply_file_update(&path,&path,"/music",&tags,10,20).await.unwrap();
            paths.push(path);
        }
        store.set_preference("tag-review:GB:910001",&json!(release)).await.unwrap();
        store.set_preference("release-live:GB:910001",&json!({"available":true,"checked_at":chrono::Utc::now().timestamp(),"source":"album_lookup"})).await.unwrap();
        let anchor = |index:usize,status:&str| json!({"status":status,"manual":true,"scope":"track","ids":{"album_id":"910001","track_id":release.tracks[index].id}});
        let first = anchor(0,"linked");
        let third = anchor(2,"resolved");
        let review = json!({"status":"review","catalogue_options":[{"id":"910001","title":"Blue Hours"}]});
        for (path,payload) in [(&paths[0],&first),(&paths[2],&third),(&paths[1],&review),(&paths[3],&review)] {
            store.save_track_link(path,"GB","[0,0,10,20]",&payload.to_string()).await.unwrap();
        }
        let selected = HashSet::from([paths[1].clone()]);
        let events = std::sync::Mutex::new(Vec::new());
        let summary = link_library_scoped(&store,"GB","/music",Arc::new(AtomicBool::new(false)),|message|events.lock().unwrap().push(message),Some(&selected),false).await.unwrap();
        assert_eq!((summary.total,summary.linked,summary.review,summary.unmatched),(1,1,0,0));
        assert!(events.lock().unwrap().iter().any(|event|event.starts_with("Reusing verified release")));
        let conn = store.connect().unwrap();
        let payload = async |path:&str| {
            let mut row = conn.query("SELECT payload FROM track_links WHERE path=? AND market='GB'",(path,)).await.unwrap();
            serde_json::from_str::<Value>(&row.next().await.unwrap().unwrap().get::<String>(0).unwrap()).unwrap()
        };
        let linked = payload(&paths[1]).await;
        assert_eq!(linked["status"],"linked");
        assert_eq!(linked["ids"]["track_id"],"91000102");
        assert_eq!(linked["release_context"]["isrc_conflict"],true);
        assert!(linked["catalogue_options"][0]["evidence"].as_str().unwrap().contains("Local ISRC differs"));
        assert_eq!(payload(&paths[0]).await,first);
        assert_eq!(payload(&paths[2]).await,third);
        assert_eq!(payload(&paths[3]).await,review,"A selected scan may use every peer as evidence but only writes its requested paths");

        // More optional credits on another valid edition must not split the
        // primary release chosen by existing peers. It remains a metadata source.
        let mut alternate = release.clone();
        alternate.id = "910002".into();
        for (index, track) in alternate.tracks.iter_mut().enumerate() {
            track.id = format!("9100020{}",index+1);
            track.credits = json!([{"name":"Writer","role":"Composer"}]);
        }
        alternate.tracks[1].isrc = Some("GBOTHER240002".into());
        conn.execute("INSERT INTO mappings(artist,tidal_id,status) VALUES('North Assembly','900001','confirmed')",()).await.unwrap();
        conn.execute("INSERT INTO catalogue(artist_id,market,payload,fetched) VALUES('900001','GB',?,'2026-10-06')",(json!(TidalCatalogue {id:"900001".into(),name:"North Assembly".into(),releases:vec![alternate]}).to_string(),)).await.unwrap();
        store.set_preference("release-live:GB:910002",&json!({"available":true,"checked_at":chrono::Utc::now().timestamp(),"source":"album_lookup"})).await.unwrap();

        // Correct-tags follow-up uses this same matcher without network access.
        let repaired = HashSet::from([paths[3].clone()]);
        let followup = link_library_mode(&store,"GB","/music",Arc::new(AtomicBool::new(false)),|_|{},Some(&repaired),false,true).await.unwrap();
        assert_eq!((followup.total,followup.linked),(1,1));
        assert_eq!(payload(&paths[3]).await["ids"]["track_id"],"91000104");
        assert_eq!(payload(&paths[3]).await["placements"].as_array().unwrap().len(),2,"Compatible cached editions still supply N-ary metadata sources");
        let repeated = link_library(&store,"GB","/music",Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
        assert_eq!(repeated.total,0,"Linked and resolved peers must not be rechecked");
        assert_eq!(payload(&paths[0]).await,first);
        assert_eq!(payload(&paths[2]).await,third);
        drop(conn); drop(store); std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wrong_artist_fallback_requires_exact_recordings() {
        let tracks = vec![LocalTrackInfo { path:"song.flac".into(), title:"High Hopes".into(), artist:"Wrong Artist".into(), album:"High Hopes".into(), duration:180.0, track_number:1, disc_number:1, isrc:Some("USP6L2100619".into()) }];
        let mut release = TidalRelease { title:"High Hopes".into(), artist:"Correct Artist".into(), available:Some(true), tracks_loaded:true, tracks:vec![TidalTrack {title:"High Hopes".into(), duration:180.0, isrc:Some("USP6L2100619".into()), ..Default::default()}], ..Default::default() };
        assert!(exact_recording_release(&tracks, &release));
        release.tracks[0].duration = 190.0;
        assert!(!exact_recording_release(&tracks, &release));
        release.tracks[0].duration = 180.0;
        release.tracks[0].isrc = None;
        assert!(!exact_recording_release(&tracks, &release));
        release.tracks[0].isrc = Some("USP6L2100619".into());
        release.available = Some(false);
        assert!(!exact_recording_release(&tracks, &release));
    }

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
                id: "1001".to_string(),
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
        other.id = "1002".into();
        other.tracks[0].id = "other_track".into();
        cat.releases[0].tracks[0].credits = json!([{"name":"Freddie Mercury","role":"Composer"}]);
        cat.releases.insert(0, other);
        let mut conflict = cat.releases[1].clone();
        conflict.id = "1003".into();
        conflict.tracks[0].track_number = 2;
        cat.releases.insert(0, conflict);
        for release in &cat.releases {
            store.set_preference(&format!("release-live:GB:{}",release.id),&json!({"available":true,"checked_at":chrono::Utc::now().timestamp(),"source":"album_lookup"})).await.unwrap();
        }
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
        assert_eq!(p["ids"]["album_id"], "1001");
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
