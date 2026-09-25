use crate::musical_keys::camelot_key;
use crate::tidal::{TidalRelease, TidalTrack};
use std::collections::HashMap;

pub fn tag_aliases(key: &str) -> &'static [&'static str] {
    match key {
        "bpm" | "tempo" => &["bpm", "tempo"],
        "initialkey" | "key" | "tkey" => &["initialkey", "key", "tkey"],
        "upc" | "barcode" => &["upc", "barcode"],
        "label" | "recordlabel" => &["label", "recordlabel"],
        "lyrics" | "unsyncedlyrics" => &["lyrics", "unsyncedlyrics"],
        "disctotal" | "totaldiscs" => &["disctotal", "totaldiscs"],
        "tracktotal" | "totaltracks" => &["tracktotal", "totaltracks"],
        _ => &[],
    }
}

pub fn has_any_tag(tags: &HashMap<String, String>, key: &str) -> bool {
    let aliases = tag_aliases(key);
    if aliases.is_empty() {
        tags.contains_key(key) && !tags[key].trim().is_empty()
    } else {
        aliases.iter().any(|&a| tags.contains_key(a) && !tags[a].trim().is_empty())
    }
}

pub fn compute_missing_tags(
    local_tags: &HashMap<String, String>,
    release: &TidalRelease,
    track: &TidalTrack,
) -> HashMap<String, String> {
    let mut missing = HashMap::new();

    // Title & Artist
    if !has_any_tag(local_tags, "title") && !track.title.trim().is_empty() {
        missing.insert("title".to_string(), track.title.clone());
    }
    if !has_any_tag(local_tags, "artist") && !release.artist.trim().is_empty() {
        missing.insert("artist".to_string(), release.artist.clone());
    }
    if !has_any_tag(local_tags, "album") && !release.title.trim().is_empty() {
        missing.insert("album".to_string(), release.title.clone());
    }
    if !has_any_tag(local_tags, "albumartist") && !release.artist.trim().is_empty() {
        missing.insert("albumartist".to_string(), release.artist.clone());
    }

    // Identifiers
    if !has_any_tag(local_tags, "isrc") {
        if let Some(ref isrc) = track.isrc {
            if !isrc.trim().is_empty() {
                missing.insert("isrc".to_string(), isrc.clone());
            }
        }
    }
    if !has_any_tag(local_tags, "tidal_track_id") {
        missing.insert("tidal_track_id".to_string(), track.id.clone());
    }
    if !has_any_tag(local_tags, "tidal_album_id") {
        missing.insert("tidal_album_id".to_string(), release.id.clone());
    }
    if !has_any_tag(local_tags, "url") && track.id.chars().all(|c| c.is_ascii_digit()) {
        missing.insert("url".to_string(), format!("https://tidal.com/track/{}", track.id));
    }

    // Numbers & Totals
    if !has_any_tag(local_tags, "tracknumber") && track.track_number > 0 {
        missing.insert("tracknumber".to_string(), format!("{:02}", track.track_number));
    }
    if !has_any_tag(local_tags, "discnumber") && track.disc_number > 0 {
        missing.insert("discnumber".to_string(), format!("{:02}", track.disc_number));
    }
    if !has_any_tag(local_tags, "tracktotal") && release.track_count > 0 {
        // If single disc or known count
        let on_disc = release.tracks.iter().filter(|t| t.disc_number == track.disc_number).count();
        let total = if on_disc > 0 { on_disc } else { release.track_count };
        missing.insert("tracktotal".to_string(), format!("{:02}", total));
    }

    // Date
    if !has_any_tag(local_tags, "date") && !release.date.trim().is_empty() {
        missing.insert("date".to_string(), release.date.clone());
    }

    // Label & Copyright
    if !has_any_tag(local_tags, "label") {
        if let Some(ref label) = release.label {
            if !label.trim().is_empty() {
                missing.insert("label".to_string(), label.clone());
            }
        }
    }
    if !has_any_tag(local_tags, "copyright") {
        let c = track.copyright.as_deref().or(release.copyright.as_deref());
        if let Some(cr) = c {
            if !cr.trim().is_empty() {
                missing.insert("copyright".to_string(), cr.to_string());
            }
        }
    }

    // Release Type
    if !has_any_tag(local_tags, "releasetype") && !release.r#type.trim().is_empty() {
        missing.insert("releasetype".to_string(), release.r#type.to_lowercase());
    }

    // DJ Metadata: BPM
    if !has_any_tag(local_tags, "bpm") {
        if let Some(bpm_val) = track.bpm {
            if bpm_val > 0.0 && bpm_val.is_finite() {
                missing.insert("bpm".to_string(), format!("{}", bpm_val.round()));
            }
        }
    }

    // DJ Metadata: Key -> Camelot key
    if !has_any_tag(local_tags, "initialkey") {
        if let Some(ref key_str) = track.key {
            let full_key = if let Some(ref scale) = track.key_scale {
                format!("{} {}", key_str, scale)
            } else {
                key_str.clone()
            };
            if let Some(cam) = camelot_key(&full_key) {
                missing.insert("initialkey".to_string(), cam);
            }
        }
    }

    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_missing_dj_tags() {
        let local_tags = HashMap::new();
        let release = TidalRelease {
            id: "123".to_string(),
            artist: "Daft Punk".to_string(),
            title: "Discovery".to_string(),
            date: "2001-03-12".to_string(),
            r#type: "album".to_string(),
            available: Some(true),
            track_count: 14,
            explicit: false,
            copyright: Some("2001 Daft Life".to_string()),
            label: Some("Virgin".to_string()),
            quality: "LOSSLESS".to_string(),
            tracks: Vec::new(),
            tracks_loaded: true,
        };
        let track = TidalTrack {
            id: "456".to_string(),
            title: "One More Time".to_string(),
            isrc: Some("FRZ110000001".to_string()),
            track_number: 1,
            disc_number: 1,
            duration: 320.0,
            bpm: Some(123.0),
            key: Some("G".to_string()),
            key_scale: Some("major".to_string()),
            copyright: None,
        };

        let missing = compute_missing_tags(&local_tags, &release, &track);
        assert_eq!(missing.get("title").unwrap(), "One More Time");
        assert_eq!(missing.get("artist").unwrap(), "Daft Punk");
        assert_eq!(missing.get("bpm").unwrap(), "123");
        assert_eq!(missing.get("initialkey").unwrap(), "9B"); // G Major is 9B in Camelot
        assert_eq!(missing.get("isrc").unwrap(), "FRZ110000001");
        assert_eq!(missing.get("tidal_track_id").unwrap(), "456");
        assert_eq!(missing.get("tidal_album_id").unwrap(), "123");
    }
}
