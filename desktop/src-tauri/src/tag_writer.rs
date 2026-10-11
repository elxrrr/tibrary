use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagExt};
use std::collections::HashMap;
use std::path::Path;

pub fn write_tags(path: &Path, updates: &HashMap<String, String>) -> Result<(), String> {
    if updates.is_empty() {
        return Ok(());
    }

    // Use native Vorbis comments for FLAC: generic numeric tag conversion drops
    // leading zeroes and can discard provider-specific metadata.
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("flac"))
    {
        use lofty::file::AudioFile;
        let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut audio = lofty::flac::FlacFile::read_from(
            &mut file,
            lofty::config::ParseOptions::default().implicit_conversions(false),
        )
        .map_err(|e| e.to_string())?;
        drop(file);
        if audio.vorbis_comments().is_none() {
            audio.set_vorbis_comments(lofty::ogg::tag::VorbisComments::default());
        }
        let comments = audio.vorbis_comments_mut().unwrap();
        for (key, value) in updates {
            let lower = key.to_ascii_lowercase();
            let key = match lower.as_str() {
                "track" => "TITLE",
                "album_artist" => "ALBUMARTIST",
                "track_number" => "TRACKNUMBER",
                "track_total" => "TRACKTOTAL",
                "disc_number" => "DISCNUMBER",
                "disc_total" => "DISCTOTAL",
                "year" => "DATE",
                "initial_key" | "key" => "INITIALKEY",
                "recordlabel" => "LABEL",
                _ => key,
            }
            .to_ascii_uppercase();
            comments.remove(&key).for_each(drop);
            if !value.trim().is_empty() {
                comments.insert(key, value.clone());
            }
        }
        return audio
            .save_to_path(path, lofty::config::WriteOptions::default())
            .map_err(|e| e.to_string());
    }

    let mut tagged_file = Probe::open(path)
        .map_err(|e| format!("Failed to open file: {}", e))?
        .read()
        .map_err(|e| format!("Failed to read audio tags: {}", e))?;

    let tag_type = tagged_file.primary_tag_type();
    let tag = match tagged_file.tag_mut(tag_type) {
        Some(t) => t,
        None => {
            let new_tag = Tag::new(tag_type);
            tagged_file.insert_tag(new_tag);
            tagged_file.tag_mut(tag_type).unwrap()
        }
    };

    for (k, v) in updates {
        let key_lower = k.to_lowercase();
        let item_key = match key_lower.as_str() {
            "title" | "track" => Some(ItemKey::TrackTitle),
            "artist" => Some(ItemKey::TrackArtist),
            "albumartist" | "album_artist" => Some(ItemKey::AlbumArtist),
            "album" => Some(ItemKey::AlbumTitle),
            "tracknumber" | "track_number" => Some(ItemKey::TrackNumber),
            "tracktotal" | "track_total" => Some(ItemKey::TrackTotal),
            "discnumber" | "disc_number" => Some(ItemKey::DiscNumber),
            "disctotal" | "disc_total" => Some(ItemKey::DiscTotal),
            "year" | "date" => Some(ItemKey::RecordingDate),
            "genre" => Some(ItemKey::Genre),
            "bpm" => Some(ItemKey::Bpm),
            "initialkey" | "initial_key" | "key" => Some(ItemKey::InitialKey),
            "isrc" => Some(ItemKey::Isrc),
            "copyright" => Some(ItemKey::CopyrightMessage),
            "label" | "recordlabel" => Some(ItemKey::Label),
            _ => ItemKey::from_key(tag_type, k),
        };

        let Some(key) = item_key else {
            continue;
        };

        if v.trim().is_empty() {
            tag.remove_key(key);
        } else {
            tag.insert_text(key, v.clone());
        }
    }

    tag.save_to_path(path, lofty::config::WriteOptions::default())
        .map_err(|e| format!("Failed to save audio tags to {:?}: {}", path, e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::file::TaggedFileExt;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn flac_alias_updates_and_deletions_preserve_other_tags_and_audio() {
        use lofty::file::AudioFile;
        let path = std::env::temp_dir().join(format!("tag-aliases-{}.FLAC", uuid::Uuid::new_v4()));
        fs::write(&path, crate::stream_download::MINIMAL_FLAC).unwrap();
        let samples = |path: &Path| claxon::FlacReader::open(path).unwrap()
            .samples().collect::<Result<Vec<_>, _>>().unwrap();
        let original_audio = samples(&path);
        write_tags(&path, &HashMap::from([
            ("title".into(), "Old title".into()),
            ("label".into(), "Old label".into()),
            ("genre".into(), "Electronic".into()),
            ("PROVIDER_CUSTOM_ID".into(), "001234".into()),
        ])).unwrap();
        write_tags(&path, &HashMap::from([
            ("track".into(), "New title".into()),
            ("recordlabel".into(), "New label".into()),
            ("album_artist".into(), "Album artist".into()),
            ("track_number".into(), "04".into()),
            ("disc_number".into(), "02".into()),
            ("track_total".into(), "12".into()),
            ("disc_total".into(), "03".into()),
            ("year".into(), "2025-03-04".into()),
            ("key".into(), "8A".into()),
            ("GENRE".into(), " \t ".into()),
        ])).unwrap();
        let mut file = fs::File::open(&path).unwrap();
        let audio = lofty::flac::FlacFile::read_from(&mut file,
            lofty::config::ParseOptions::default().implicit_conversions(false)).unwrap();
        let comments = audio.vorbis_comments().unwrap();
        for (key, expected) in [
            ("TITLE", "New title"), ("LABEL", "New label"), ("ALBUMARTIST", "Album artist"),
            ("TRACKNUMBER", "04"), ("DISCNUMBER", "02"), ("TRACKTOTAL", "12"),
            ("DISCTOTAL", "03"), ("DATE", "2025-03-04"), ("INITIALKEY", "8A"),
            ("PROVIDER_CUSTOM_ID", "001234"),
        ] {
            assert_eq!(comments.get(key), Some(expected), "Tag {key}");
        }
        assert_eq!(comments.get("GENRE"), None);
        assert_eq!(comments.get("TRACK"), None);
        assert_eq!(comments.get("RECORDLABEL"), None);
        assert_eq!(samples(&path), original_audio);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_updates_leave_bytes_unchanged_and_bad_audio_returns_an_error() {
        let path = std::env::temp_dir().join(format!("tag-errors-{}.flac", uuid::Uuid::new_v4()));
        let invalid = b"This is not an audio file";
        fs::write(&path, invalid).unwrap();
        write_tags(&path, &HashMap::new()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), invalid);
        assert!(write_tags(&path, &HashMap::from([("title".into(), "Title".into())])).is_err());
        assert_eq!(fs::read(&path).unwrap(), invalid);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn mp4_tag_updates_dates_and_deletions_preserve_encoded_audio() {
        let path = std::env::temp_dir().join(format!("mp4-tag-edits-{}.m4a", uuid::Uuid::new_v4()));
        let fixture = include_bytes!("../tests/fixtures/silence.m4a");
        fs::write(&path, fixture).unwrap();
        let audio_payload = |bytes: &[u8]| {
            let mut offset = 0;
            let mut payload = Vec::new();
            while offset + 8 <= bytes.len() {
                let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
                let (size, header) = match size {
                    0 => (bytes.len() - offset, 8),
                    1 => (u64::from_be_bytes(bytes[offset + 8..offset + 16].try_into().unwrap()) as usize, 16),
                    _ => (size, 8),
                };
                assert!(size >= header && offset + size <= bytes.len());
                if &bytes[offset + 4..offset + 8] == b"mdat" {
                    payload.extend_from_slice(&bytes[offset + header..offset + size]);
                }
                offset += size;
            }
            assert!(!payload.is_empty(), "Fixture must contain encoded audio");
            payload
        };
        let before = audio_payload(fixture);
        write_tags(&path, &HashMap::from([
            ("track".into(), "New title".into()), ("artist".into(), "Track artist".into()),
            ("album_artist".into(), "Album artist".into()), ("album".into(), "Release".into()),
            ("track_number".into(), "04".into()), ("track_total".into(), "12".into()),
            ("disc_number".into(), "2".into()), ("disc_total".into(), "3".into()),
            ("year".into(), "2025-03-04".into()), ("bpm".into(), "123.5".into()),
            ("initial_key".into(), "8A".into()), ("genre".into(), "Electronic".into()),
            ("isrc".into(), "GBABC2500001".into()), ("recordlabel".into(), "Record label".into()),
            ("copyright".into(), "Copyright holder".into()),
        ])).unwrap();
        let tagged = Probe::open(&path).unwrap().read().unwrap();
        let tag = tagged.primary_tag().unwrap();
        for (key, expected) in [
            (ItemKey::TrackTitle, "New title"), (ItemKey::TrackArtist, "Track artist"),
            (ItemKey::AlbumArtist, "Album artist"), (ItemKey::AlbumTitle, "Release"),
            (ItemKey::TrackNumber, "4"), (ItemKey::TrackTotal, "12"),
            (ItemKey::DiscNumber, "2"), (ItemKey::DiscTotal, "3"),
            (ItemKey::RecordingDate, "2025-03-04"), (ItemKey::Bpm, "123.5"),
            (ItemKey::InitialKey, "8A"), (ItemKey::Isrc, "GBABC2500001"),
            (ItemKey::Label, "Record label"), (ItemKey::CopyrightMessage, "Copyright holder"),
        ] {
            assert_eq!(tag.get_string(key.clone()), Some(expected), "{key:?}");
        }
        write_tags(&path, &HashMap::from([("genre".into(), " ".into()), ("key".into(), "".into())])).unwrap();
        let tagged = Probe::open(&path).unwrap().read().unwrap();
        let tag = tagged.primary_tag().unwrap();
        assert_eq!(tag.get_string(ItemKey::Genre), None);
        assert_eq!(tag.get_string(ItemKey::InitialKey), None);
        assert_eq!(tag.get_string(ItemKey::TrackTitle), Some("New title"));
        assert_eq!(audio_payload(&fs::read(&path).unwrap()), before);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_write_and_read_tags() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tag_writer_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let flac_path = temp_dir.join("test_song.flac");

        // Create a minimal valid FLAC fixture directly from native bytes
        fs::write(&flac_path, crate::stream_download::MINIMAL_FLAC)
            .expect("Failed to write FLAC fixture");

        let mut updates = HashMap::new();
        updates.insert("title".to_string(), "Bohemian Rhapsody".to_string());
        updates.insert("artist".to_string(), "Queen".to_string());
        updates.insert("album".to_string(), "A Night at the Opera".to_string());
        updates.insert("tracknumber".to_string(), "11".to_string());
        updates.insert("initialkey".to_string(), "8A".to_string());
        updates.insert("bpm".to_string(), "72".to_string());

        write_tags(&flac_path, &updates).expect("Failed to write tags");

        // Verify with lofty read
        let tagged = Probe::open(&flac_path).unwrap().read().unwrap();
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag()).unwrap();

        assert_eq!(
            tag.get_string(ItemKey::TrackTitle),
            Some("Bohemian Rhapsody")
        );
        assert_eq!(tag.get_string(ItemKey::TrackArtist), Some("Queen"));
        assert_eq!(
            tag.get_string(ItemKey::AlbumTitle),
            Some("A Night at the Opera")
        );
        assert_eq!(tag.get_string(ItemKey::TrackNumber), Some("11"));
        assert_eq!(tag.get_string(ItemKey::InitialKey), Some("8A"));

        let _ = fs::remove_dir_all(temp_dir);
    }
}
