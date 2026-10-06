//! Shared interpretation of local track/disc totals against loaded audio tracks.
//!
//! Some taggers repeat the whole-release track total on every disc; others store
//! a per-disc total. The whole-release convention is only safe to infer from a
//! complete, unique set of positions, never from a partial collection of files.

use crate::release_matching::LocalTrackInfo;
use crate::tidal::TidalRelease;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub(crate) struct ReleaseCardinality {
    /// Every loaded audio position is present exactly once locally.
    pub complete: bool,
    /// All local tags use one consistent whole-release total on multiple discs.
    pub uses_release_total: bool,
    pub conflicts: Vec<String>,
}

impl ReleaseCardinality {
    pub fn valid(&self) -> bool {
        self.conflicts.is_empty()
    }
}

#[cfg(test)]
pub(crate) fn validate_totals(
    tracks: &[LocalTrackInfo],
    totals: &HashMap<String, (u32, u32)>,
    release: &TidalRelease,
) -> ReleaseCardinality {
    validate_totals_with_context(tracks, totals, release, false)
}

/// Established same-release evidence may explain missing or impossible totals
/// on an incomplete local album. Plausible totals that describe another edition
/// still conflict, even with that context.
pub(crate) fn validate_totals_with_context(
    tracks: &[LocalTrackInfo],
    totals: &HashMap<String, (u32, u32)>,
    release: &TidalRelease,
    established_release: bool,
) -> ReleaseCardinality {
    let mut result = ReleaseCardinality {
        complete: false,
        uses_release_total: false,
        conflicts: Vec::new(),
    };
    let mut conflict = |message: &str| {
        if !result.conflicts.iter().any(|saved| saved == message) {
            result.conflicts.push(message.into());
        }
    };
    if tracks.is_empty() || !release.tracks_loaded || release.tracks.is_empty() {
        conflict("Complete online audio track positions are required to validate release totals");
        return result;
    }

    let mut remote_positions = HashSet::new();
    let mut remote_counts = HashMap::<u32, u32>::new();
    for track in &release.tracks {
        if track.disc_number == 0
            || track.track_number == 0
            || !remote_positions.insert((track.disc_number, track.track_number))
        {
            conflict("Online track/disc positions are missing or duplicated");
        }
        *remote_counts.entry(track.disc_number).or_default() += 1;
    }
    let remote_discs = remote_counts.keys().copied().max().unwrap_or(0);
    if !(1..=remote_discs).all(|disc| {
        remote_counts.get(&disc).is_some_and(|count| {
            (1..=*count).all(|index| remote_positions.contains(&(disc, index)))
        })
    }) {
        conflict("Online audio track/disc positions are not contiguous");
    }

    let mut local_positions = HashSet::new();
    let mut local_paths = HashSet::new();
    let mut local_max_by_disc = HashMap::<u32, u32>::new();
    for track in tracks {
        let position = (track.disc_number, track.track_number);
        if track.disc_number == 0
            || track.track_number == 0
            || !local_positions.insert(position)
            || !local_paths.insert(&track.path)
        {
            conflict("Local track/disc positions are missing or duplicated");
        }
        if !remote_positions.contains(&position) {
            conflict("Local track/disc positions differ from the online release");
        }
        local_max_by_disc
            .entry(track.disc_number)
            .and_modify(|maximum| *maximum = (*maximum).max(track.track_number))
            .or_insert(track.track_number);
    }
    // Do not use duplicate rows, missing remote positions, or summaries including
    // a video in track_count as proof of a complete physical audio release.
    if !result.conflicts.is_empty() {
        return result;
    }
    result.complete = local_positions == remote_positions && tracks.len() == release.tracks.len();
    let loaded_audio_count = release.tracks.len() as u32;
    result.uses_release_total = result.complete
        && remote_discs > 1
        && tracks.iter().all(|track| {
            totals.get(&track.path).map(|(total, _)| *total) == Some(loaded_audio_count)
        });

    let local_max_disc = local_max_by_disc.keys().copied().max().unwrap_or(0);
    for track in tracks {
        let (declared_tracks, declared_discs) =
            totals.get(&track.path).copied().unwrap_or_default();
        let plausible_tracks =
            declared_tracks > 0 && declared_tracks >= local_max_by_disc[&track.disc_number];
        let plausible_discs = declared_discs > 0 && declared_discs >= local_max_disc;
        let remote_total = remote_counts[&track.disc_number];

        if plausible_tracks {
            if declared_tracks != remote_total && !result.uses_release_total {
                result.conflicts.push(format!(
                    "Disc {:02} track total differs: local {} · online {}",
                    track.disc_number, declared_tracks, remote_total
                ));
            }
        } else if !result.complete && !established_release {
            result.conflicts.push(format!(
                "Disc {:02} track total is missing or below the observed local positions",
                track.disc_number
            ));
        }
        if plausible_discs {
            if declared_discs != remote_discs {
                result.conflicts.push(format!(
                    "Disc total differs: local {} · online {}",
                    declared_discs, remote_discs
                ));
            }
        } else if !result.complete && !established_release && remote_discs != 1 {
            result
                .conflicts
                .push("Disc total is missing or below the observed local disc positions".into());
        }
    }
    result.conflicts.sort();
    result.conflicts.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tidal::TidalTrack;

    fn fixture(disc_totals: &[u32]) -> (Vec<LocalTrackInfo>, TidalRelease) {
        let mut local = Vec::new();
        let mut remote = Vec::new();
        for (disc, count) in disc_totals.iter().enumerate() {
            for index in 1..=*count {
                let disc_number = disc as u32 + 1;
                let title = format!("Disc {disc_number} track {index}");
                local.push(LocalTrackInfo {
                    path: format!("/fixture/{disc_number}-{index}.flac"),
                    title: title.clone(),
                    artist: "Artist".into(),
                    album: "Release".into(),
                    duration: 180.0,
                    track_number: index,
                    disc_number,
                    isrc: None,
                });
                remote.push(TidalTrack {
                    id: format!("{disc_number}{index:03}"),
                    title,
                    duration: 180.0,
                    track_number: index,
                    disc_number,
                    ..Default::default()
                });
            }
        }
        let release = TidalRelease {
            tracks_loaded: true,
            track_count: remote.len() + 1, // A provider count can include a video.
            tracks: remote,
            ..Default::default()
        };
        (local, release)
    }

    fn totals(tracks: &[LocalTrackInfo], count: u32, discs: u32) -> HashMap<String, (u32, u32)> {
        tracks
            .iter()
            .map(|track| (track.path.clone(), (count, discs)))
            .collect()
    }

    #[test]
    fn complete_multidisc_release_accepts_consistent_whole_release_or_per_disc_totals() {
        let (local, release) = fixture(&[3, 2]);
        let whole = validate_totals(&local, &totals(&local, 5, 2), &release);
        assert!(whole.valid(), "{:?}", whole.conflicts);
        assert!(whole.complete && whole.uses_release_total);

        let per_disc: HashMap<_, _> = local
            .iter()
            .map(|track| {
                (
                    track.path.clone(),
                    (if track.disc_number == 1 { 3 } else { 2 }, 2),
                )
            })
            .collect();
        let ordinary = validate_totals(&local, &per_disc, &release);
        assert!(ordinary.valid());
        assert!(ordinary.complete && !ordinary.uses_release_total);
        let mut mixed = totals(&local, 5, 2);
        mixed.insert(local[0].path.clone(), (3, 2));
        assert!(!validate_totals(&local, &mixed, &release).valid());
    }

    #[test]
    fn incomplete_or_duplicate_files_do_not_prove_the_whole_release_total_convention() {
        let (local, release) = fixture(&[3, 2]);
        let global = totals(&local, 5, 2);
        assert!(!validate_totals(&local[..4], &global, &release).valid());
        assert!(!validate_totals_with_context(&local[..4], &global, &release, true).valid());
        let mut duplicated = local.clone();
        duplicated[4] = local[3].clone();
        let result = validate_totals(&duplicated, &global, &release);
        assert!(!result.valid());
        assert!(!result.complete && !result.uses_release_total);
    }

    #[test]
    fn plausible_totals_keep_standalone_and_rolling_releases_separate() {
        let (local, release) = fixture(&[3]);
        assert!(!validate_totals(&local[..2], &totals(&local, 2, 1), &release).valid());
        assert!(
            !validate_totals_with_context(&local[..1], &totals(&local, 1, 1), &release, true)
                .valid()
        );
        assert!(!validate_totals(&local, &totals(&local, 4, 1), &release).valid());
        assert!(!validate_totals(&local, &totals(&local, 3, 2), &release).valid());
        let partial = validate_totals(&local[..2], &totals(&local, 3, 1), &release);
        assert!(partial.valid() && !partial.complete);
    }

    #[test]
    fn impossible_totals_need_complete_positions_or_established_same_release_context() {
        let (local, release) = fixture(&[4]);
        let broken = totals(&local, 1, 1);
        assert!(validate_totals(&local, &broken, &release).valid());
        assert!(!validate_totals(&local[..3], &broken, &release).valid());
        assert!(validate_totals_with_context(&local[..3], &broken, &release, true).valid());
        assert!(!validate_totals(&local[..2], &HashMap::new(), &release).valid());
    }
}
