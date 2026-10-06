//! Resolve tracks inside an established release, never by borrowing a single's placement.
use crate::release_anchor::ScanAnchoredRelease;
use crate::release_matching::{
    clean_isrc, recording_matches, CreditAliases, LocalTrackInfo, StructureMatchResult,
    TrackAlignment,
};
use crate::tidal::TidalRelease;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(crate) fn resolve(
    tracks: &[LocalTrackInfo],
    totals: &HashMap<String, (u32, u32)>,
    release: &TidalRelease,
    aliases: &CreditAliases,
    links: &HashMap<String, HashMap<String, String>>,
    saved: &HashMap<String, (Value, String)>,
) -> Option<ScanAnchoredRelease> {
    if tracks.is_empty() || !release.tracks_loaded || release.available == Some(false) {
        return None;
    }
    let remote: HashMap<_, _> = release
        .tracks
        .iter()
        .map(|track| ((track.disc_number, track.track_number), track))
        .collect();
    let remote_ids: HashSet<_> = release.tracks.iter().map(|track| &track.id).collect();
    if remote.len() != release.tracks.len()
        || remote_ids.len() != release.tracks.len()
        || remote_ids
            .iter()
            .any(|id| id.parse::<u64>().ok().is_none_or(|id| id == 0))
    {
        return None;
    }

    // Current peer choices must belong to this edition and this exact position.
    // A mismatched existing choice requires review rather than a majority vote.
    let mut strong = HashSet::new();
    let mut anchor_recordings = HashSet::new();
    let mut primary_anchor_count = 0;
    for local in tracks {
        let target = remote.get(&(local.disc_number, local.track_number))?;
        if let Some(placements) = links.get(&local.path) {
            if placements.get(&release.id) != Some(&target.id) {
                return None;
            }
            if saved.get(&local.path).is_some_and(|(payload, _)| {
                crate::tidal::resource_id(&payload["ids"]["album_id"]) == release.id
            }) {
                primary_anchor_count += 1;
            }
        }
        let local_isrc = clean_isrc(local.isrc.as_deref()).filter(|isrc| !isrc.is_empty());
        let remote_isrc = clean_isrc(target.isrc.as_deref()).filter(|isrc| !isrc.is_empty());
        let equal_isrc = local_isrc
            .as_ref()
            .zip(remote_isrc.as_ref())
            .is_some_and(|(a, b)| a == b);
        let chosen_without_conflicting_isrc = links.contains_key(&local.path)
            && local_isrc
                .as_ref()
                .zip(remote_isrc.as_ref())
                .is_none_or(|(a, b)| a == b);
        if (equal_isrc || chosen_without_conflicting_isrc)
            && duration_agrees(local.duration, target.duration)
            && aliases.titles_match(&local.title, target, false)
        {
            let identity = remote_isrc
                .clone()
                .map(|isrc| format!("isrc:{isrc}"))
                .unwrap_or_else(|| format!("track:{}", target.id));
            strong.insert(identity.clone());
            if links.contains_key(&local.path) {
                anchor_recordings.insert(identity);
            }
        }
    }
    let anchored = anchor_recordings.len() >= 2;
    let cardinality =
        crate::release_cardinality::validate_totals_with_context(tracks, totals, release, anchored);
    if !cardinality.valid() {
        return None;
    }
    let majority = strong.len() >= 2 && strong.len() * 3 >= tracks.len() * 2;
    // Majority evidence can identify a complete physical release. Incomplete
    // albums need existing sibling choices to explain missing/impossible totals.
    if (!majority && !anchored) || (!cardinality.complete && !anchored) {
        return None;
    }
    let artist_agrees = tracks.iter().all(|track| {
        crate::matching::title_key(&track.artist) == crate::matching::title_key(&release.artist)
    });

    let mut alignments = HashMap::new();
    let mut conflicting_isrc_paths = HashSet::new();
    let mut contextual_paths = HashMap::new();
    let mut review_reasons = HashMap::new();
    for local in tracks {
        let target = remote[&(local.disc_number, local.track_number)];
        let local_isrc = clean_isrc(local.isrc.as_deref()).filter(|isrc| !isrc.is_empty());
        let remote_isrc = clean_isrc(target.isrc.as_deref()).filter(|isrc| !isrc.is_empty());
        let isrc_conflict = local_isrc
            .as_ref()
            .zip(remote_isrc.as_ref())
            .is_some_and(|(a, b)| a != b);
        let equal_isrc = local_isrc
            .as_ref()
            .zip(remote_isrc.as_ref())
            .is_some_and(|(a, b)| a == b);
        let title_agrees = aliases.titles_match(&local.title, target, equal_isrc);
        let unique = release
            .tracks
            .iter()
            .filter(|other| {
                duration_agrees(local.duration, other.duration)
                    && aliases.titles_match(&local.title, other, equal_isrc)
                    && (!equal_isrc || clean_isrc(other.isrc.as_deref()) == local_isrc)
            })
            .count()
            == 1;
        let may_reconcile_isrc = cardinality.complete && (anchored || artist_agrees);
        let reason = if !duration_agrees(local.duration, target.duration) {
            Some(format!("Same release identified by siblings; duration differs by {:.2} seconds (local {:.2}s · online {:.2}s)", (local.duration-target.duration).abs(), local.duration, target.duration))
        } else if !title_agrees {
            Some(
                "Same release identified by siblings; the title or mix identity still conflicts"
                    .into(),
            )
        } else if !unique {
            Some("Same release identified by siblings; this recording is not unique within the release".into())
        } else if isrc_conflict && !may_reconcile_isrc {
            Some("Same release identified by siblings; an incomplete album cannot settle a conflicting ISRC".into())
        } else {
            None
        };
        if let Some(reason) = reason {
            review_reasons.insert(local.path.clone(), reason);
            continue;
        }
        if isrc_conflict {
            conflicting_isrc_paths.insert(local.path.clone());
            contextual_paths.insert(local.path.clone(), "Local ISRC differs; the complete release and verified sibling recordings establish this placement".into());
        } else if !recording_matches(
            &local.title,
            local.duration,
            local.isrc.as_deref(),
            &target.title,
            target.duration,
            target.isrc.as_deref(),
            true,
        ) {
            contextual_paths.insert(local.path.clone(), "Cached contributor credits and the same-release recording identity explain the title difference".into());
        }
        alignments.insert(
            local.path.clone(),
            TrackAlignment {
                local_path: local.path.clone(),
                remote_track_id: target.id.clone(),
                disc_number: target.disc_number,
                track_number: target.track_number,
            },
        );
    }
    let placed: HashSet<_> = alignments
        .values()
        .map(|alignment| alignment.remote_track_id.as_str())
        .collect();
    let missing_remote_track_ids = release
        .tracks
        .iter()
        .filter(|track| !placed.contains(track.id.as_str()))
        .map(|track| track.id.clone())
        .collect();
    let evidence = format!("{} of {} independent recordings establish this release; {} verified existing sibling link(s){}", strong.len(), tracks.len(), anchor_recordings.len(), if cardinality.uses_release_total { "; tags use the whole-release track total across discs" } else { "" });
    Some(ScanAnchoredRelease {
        release: release.clone(),
        structure: StructureMatchResult {
            compatible: true,
            incomplete: !cardinality.complete,
            matched_count: alignments.len(),
            total_remote_tracks: release.tracks.len(),
            conflicts: Vec::new(),
            alignments,
            missing_remote_track_ids,
        },
        evidence,
        conflicting_isrc_paths,
        primary_anchor_count,
        contextual_paths,
        review_reasons,
    })
}

fn duration_agrees(local: f64, remote: f64) -> bool {
    local.is_finite()
        && remote.is_finite()
        && local > 0.
        && remote > 0.
        && (local - remote).abs() <= 3.
}

/// Optional metadata sources must describe the same full audio edition, not a
/// single or a later rolling album which merely contains the local recordings.
pub(crate) fn equivalent_editions(a: &TidalRelease, b: &TidalRelease) -> bool {
    a.tracks_loaded
        && b.tracks_loaded
        && !a.tracks.is_empty()
        && a.tracks.len() == b.tracks.len()
        && crate::matching::title_key(&a.artist) == crate::matching::title_key(&b.artist)
        && crate::matching::title_key(&a.title) == crate::matching::title_key(&b.title)
        && a.tracks.iter().all(|track| {
            b.tracks
                .iter()
                .filter(|other| {
                    other.disc_number == track.disc_number
                        && other.track_number == track.track_number
                })
                .count()
                == 1
                && b.tracks.iter().any(|other| {
                    other.disc_number == track.disc_number
                        && other.track_number == track.track_number
                        && duration_agrees(track.duration, other.duration)
                        && crate::matching::title_key(&track.title)
                            == crate::matching::title_key(&other.title)
                })
        })
}
