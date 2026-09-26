use crate::matching::title_key;
use crate::tidal::TidalRelease;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalTrackInfo {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub track_number: u32,
    pub disc_number: u32,
    pub isrc: Option<String>,
}

pub fn clean_isrc(isrc_opt: Option<&str>) -> Option<String> {
    isrc_opt.map(|s| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_uppercase()
    })
}

pub fn has_mix_keyword(title: &str) -> bool {
    let lower = title.to_lowercase();
    let keywords = [
        "remix",
        "mix",
        "extended",
        "instrumental",
        "radio",
        "club",
        "live",
        "edit",
        "acoustic",
    ];
    keywords
        .iter()
        .any(|&kw| lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == kw))
}

pub fn recording_matches(
    local_title: &str,
    local_duration: f64,
    local_isrc: Option<&str>,
    remote_title: &str,
    remote_duration: f64,
    remote_isrc: Option<&str>,
    strict: bool,
) -> bool {
    // 1. Duration check (tolerance: 3 seconds)
    if local_duration > 0.0
        && remote_duration > 0.0
        && (local_duration - remote_duration).abs() > 3.0
    {
        return false;
    }

    // 2. ISRC check
    let li = clean_isrc(local_isrc);
    let ri = clean_isrc(remote_isrc);
    if let (Some(ref l), Some(ref r)) = (&li, &ri) {
        if !l.is_empty() && !r.is_empty() && l != r {
            return false;
        }
    }

    // 3. Mix keyword check
    let lt_key = title_key(local_title);
    let rt_key = title_key(remote_title);
    if lt_key != rt_key && (has_mix_keyword(local_title) || has_mix_keyword(remote_title)) {
        return false;
    }

    if strict {
        if local_duration <= 0.0
            || remote_duration <= 0.0
            || local_title.is_empty()
            || remote_title.is_empty()
        {
            return false;
        }
        if lt_key != rt_key {
            return false;
        }
    }

    // If both ISRCs match, that's a verified match
    if let (Some(ref l), Some(ref r)) = (&li, &ri) {
        if !l.is_empty() && !r.is_empty() && l == r {
            return true;
        }
    }

    // Otherwise, match if normalized title and duration match
    !lt_key.is_empty() && lt_key == rt_key && local_duration > 0.0 && remote_duration > 0.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackAlignment {
    pub local_path: String,
    pub remote_track_id: String,
    pub disc_number: u32,
    pub track_number: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureMatchResult {
    pub compatible: bool,
    pub incomplete: bool,
    pub matched_count: usize,
    pub total_remote_tracks: usize,
    pub conflicts: Vec<String>,
    pub alignments: HashMap<String, TrackAlignment>,
    pub missing_remote_track_ids: Vec<String>,
}

pub fn structure_match(
    local_tracks: &[LocalTrackInfo],
    remote_release: &TidalRelease,
) -> StructureMatchResult {
    let mut remote_positions = HashMap::new();
    for t in &remote_release.tracks {
        remote_positions.insert((t.disc_number, t.track_number), t);
    }

    let mut conflicts = Vec::new();
    if remote_positions.len() != remote_release.tracks.len() {
        conflicts.push("Catalogue track positions are incomplete or duplicated".to_string());
    }

    let mut claimed_positions = HashSet::new();
    let mut alignments = HashMap::new();
    let mut unmatched_local = Vec::new();

    // Pass 1: exact position matches
    for local in local_tracks {
        let pos = (local.disc_number, local.track_number);
        if let Some(remote) = remote_positions.get(&pos) {
            if recording_matches(
                &local.title,
                local.duration,
                local.isrc.as_deref(),
                &remote.title,
                remote.duration,
                remote.isrc.as_deref(),
                false,
            ) && !claimed_positions.contains(&pos)
            {
                claimed_positions.insert(pos);
                alignments.insert(
                    local.path.clone(),
                    TrackAlignment {
                        local_path: local.path.clone(),
                        remote_track_id: remote.id.clone(),
                        disc_number: pos.0,
                        track_number: pos.1,
                    },
                );
                continue;
            }
        }
        unmatched_local.push(local);
    }

    // Pass 2: unique recording match for unmatched tracks
    for local in unmatched_local {
        let mut hits = Vec::new();
        for (&pos, &remote) in &remote_positions {
            if !claimed_positions.contains(&pos)
                && recording_matches(
                    &local.title,
                    local.duration,
                    local.isrc.as_deref(),
                    &remote.title,
                    remote.duration,
                    remote.isrc.as_deref(),
                    false,
                )
            {
                hits.push((pos, remote));
            }
        }

        if hits.len() == 1 {
            let (pos, remote) = hits[0];
            claimed_positions.insert(pos);
            alignments.insert(
                local.path.clone(),
                TrackAlignment {
                    local_path: local.path.clone(),
                    remote_track_id: remote.id.clone(),
                    disc_number: pos.0,
                    track_number: pos.1,
                },
            );
        } else if !hits.is_empty() {
            conflicts.push(format!("Multiple candidates for track {}", local.title));
        }
    }

    let missing_remote_track_ids: Vec<String> = remote_release
        .tracks
        .iter()
        .filter(|t| !claimed_positions.contains(&(t.disc_number, t.track_number)))
        .map(|t| t.id.clone())
        .collect();

    let matched_count = alignments.len();
    let total_remote_tracks = remote_release.tracks.len();
    let compatible = conflicts.is_empty() && total_remote_tracks > 0;
    let incomplete = compatible && matched_count > 0 && !missing_remote_track_ids.is_empty();

    StructureMatchResult {
        compatible,
        incomplete,
        matched_count,
        total_remote_tracks,
        conflicts,
        alignments,
        missing_remote_track_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tidal::TidalTrack;

    #[test]
    fn test_recording_matches_duration_and_title() {
        assert!(recording_matches(
            "Bohemian Rhapsody",
            354.0,
            None,
            "Bohemian Rhapsody",
            355.0,
            None,
            false
        ));

        // Duration > 3s difference fails
        assert!(!recording_matches(
            "Bohemian Rhapsody",
            350.0,
            None,
            "Bohemian Rhapsody",
            356.0,
            None,
            false
        ));

        // Conflicting mix name fails
        assert!(!recording_matches(
            "Song (Club Mix)",
            200.0,
            None,
            "Song (Acoustic Mix)",
            200.0,
            None,
            false
        ));
    }

    #[test]
    fn test_structure_match_exact() {
        let local = vec![
            LocalTrackInfo {
                path: "/music/01.flac".to_string(),
                title: "Track One".to_string(),
                artist: "Band".to_string(),
                album: "Album".to_string(),
                duration: 180.0,
                track_number: 1,
                disc_number: 1,
                isrc: None,
            },
            LocalTrackInfo {
                path: "/music/02.flac".to_string(),
                title: "Track Two".to_string(),
                artist: "Band".to_string(),
                album: "Album".to_string(),
                duration: 200.0,
                track_number: 2,
                disc_number: 1,
                isrc: None,
            },
        ];

        let release = TidalRelease {
            id: "rel_1".to_string(),
            artist: "Band".to_string(),
            title: "Album".to_string(),
            date: "2020-01-01".to_string(),
            r#type: "album".to_string(),
            available: Some(true),
            track_count: 2,
            explicit: false,
            copyright: None,
            label: None,
            quality: "LOSSLESS".to_string(),
            tracks: vec![
                TidalTrack {
                    id: "t1".to_string(),
                    title: "Track One".to_string(),
                    isrc: None,
                    track_number: 1,
                    disc_number: 1,
                    duration: 180.0,
                    bpm: None,
                    key: None,
                    key_scale: None,
                    copyright: None,
                    ..Default::default()
                },
                TidalTrack {
                    id: "t2".to_string(),
                    title: "Track Two".to_string(),
                    isrc: None,
                    track_number: 2,
                    disc_number: 1,
                    duration: 200.0,
                    bpm: None,
                    key: None,
                    key_scale: None,
                    copyright: None,
                    ..Default::default()
                },
            ],
            tracks_loaded: true,
            ..Default::default()
        };

        let result = structure_match(&local, &release);
        assert!(result.compatible);
        assert!(!result.incomplete);
        assert_eq!(result.matched_count, 2);
        assert!(result.missing_remote_track_ids.is_empty());
        assert_eq!(
            result
                .alignments
                .get("/music/01.flac")
                .unwrap()
                .remote_track_id,
            "t1"
        );
    }
}
