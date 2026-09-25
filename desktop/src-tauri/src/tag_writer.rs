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
                "album_artist" => "ALBUMARTIST",
                "track_number" => "TRACKNUMBER",
                "track_total" => "TRACKTOTAL",
                "disc_number" => "DISCNUMBER",
                "disc_total" => "DISCTOTAL",
                "year" => "DATE",
                "initial_key" | "key" => "INITIALKEY",
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
            "year" | "date" => Some(ItemKey::Year),
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
