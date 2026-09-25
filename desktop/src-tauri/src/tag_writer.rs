use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagExt};
use std::collections::HashMap;
use std::path::Path;

pub fn write_tags(path: &Path, updates: &HashMap<String, String>) -> Result<(), String> {
    if updates.is_empty() {
        return Ok(());
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
    use std::process::Command;
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

        // Create a minimal valid FLAC fixture with soundfile in Python
        let py_script = format!(
            r#"
import soundfile as sf
import numpy as np
data = np.zeros((1000, 2), dtype='float32')
sf.write('{path}', data, 44100, format='FLAC')
"#,
            path = flac_path.display()
        );
        let root_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let py_status = Command::new(root_dir.join(".venv/bin/python"))
            .arg("-c")
            .arg(&py_script)
            .status()
            .expect("Failed to run python");
        assert!(py_status.success(), "Failed to create FLAC fixture");

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

        assert_eq!(tag.get_string(ItemKey::TrackTitle), Some("Bohemian Rhapsody"));
        assert_eq!(tag.get_string(ItemKey::TrackArtist), Some("Queen"));
        assert_eq!(tag.get_string(ItemKey::AlbumTitle), Some("A Night at the Opera"));
        assert_eq!(tag.get_string(ItemKey::TrackNumber), Some("11"));
        assert_eq!(tag.get_string(ItemKey::InitialKey), Some("8A"));

        let _ = fs::remove_dir_all(temp_dir);
    }
}
