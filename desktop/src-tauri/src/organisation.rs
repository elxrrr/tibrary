use std::collections::HashMap;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const DEFAULT_LAYOUT: &str =
    "{albumartist}/{album} ({year})/{disc}/{disc_prefix}{tracknumber} - {title}";

// Clean tag text before adding template punctuation. Separators only join content;
// they must not produce a dangling dash at a tag or bracket boundary.
fn clean_tag(value: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for c in value.chars() {
        match c {
            '/' | '\\' | ':' | '|' => { separator = true; continue; }
            '<' | '>' | '*' | '?' | '"' | '\0'..='\x1f' | '\x7f' => continue,
            _ => {}
        }
        if separator && c.is_whitespace() { continue; }
        if separator {
            while out.ends_with(char::is_whitespace) { out.pop(); }
            if !out.is_empty() && !out.ends_with(['(', '[', '{']) && ![')', ']', '}'].contains(&c) {
                if !out.ends_with('-') { out.push_str(" -"); }
                out.push(' ');
            }
            separator = false;
        }
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ").nfc().collect()
}

pub fn safe_component(value: &str, _final_part: bool) -> Result<String, String> {
    let mut result = clean_tag(value);

    // Strip trailing spaces and dots
    // Windows disallows trailing dots/spaces on directories as well as files.
    result = result.trim_end_matches([' ', '.']).to_string();

    if result.is_empty() || result == "." || result == ".." {
        return Err("Tags produce an empty or unsafe filename component".to_string());
    }

    // Windows reserved device names
    let upper = result.to_ascii_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved.contains(&upper.as_str()) {
        result = format!("_{}", result);
    }

    Ok(result)
}

pub fn folder_operation(source: &str, target: Option<&str>) -> &'static str {
    let Some(target) = target else { return "No change"; };
    let source = Path::new(source);
    let target = Path::new(target);
    match (source.parent() != target.parent(), source.file_name() != target.file_name()) {
        (true, true) => "Move and rename file",
        (true, false) => "Move file",
        (false, true) => "Rename file",
        _ => "No change",
    }
}

pub fn format_layout(
    root: &Path,
    tags: &HashMap<String, String>,
    template: Option<&str>,
    extension: &str,
) -> Result<PathBuf, String> {
    let tpl = template.unwrap_or(DEFAULT_LAYOUT);

    let artist = tags
        .get("albumartist")
        .or_else(|| tags.get("album_artist"))
        .or_else(|| tags.get("artist"))
        .map(|s| s.as_str())
        .unwrap_or("Unknown Artist");

    let album = tags
        .get("album")
        .map(|s| s.as_str())
        .unwrap_or("Unknown Album");

    let title = tags
        .get("title")
        .map(|s| s.as_str())
        .unwrap_or("Unknown Title");

    let year = tags
        .get("year")
        .or_else(|| tags.get("date"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let year = year.get(..4).filter(|s| s.chars().all(|c| c.is_ascii_digit())).unwrap_or("");

    let disc_num: u32 = tags
        .get("discnumber")
        .or_else(|| tags.get("disc_number"))
        .and_then(|s| s.split('/').next()?.trim().parse().ok())
        .unwrap_or(1);

    let track_num: u32 = tags
        .get("tracknumber")
        .or_else(|| tags.get("track_number"))
        .and_then(|s| s.split('/').next()?.trim().parse().ok())
        .unwrap_or(1);

    let disc_total: u32 = tags
        .get("disctotal")
        .or_else(|| tags.get("disc_total"))
        .and_then(|s| s.split('/').next()?.trim().parse().ok())
        .unwrap_or(1);

    let disc_relevant = disc_total > 1 || disc_num > 1;

    let artist = clean_tag(artist);
    let album = clean_tag(album);
    let title = clean_tag(title);
    if artist.is_empty() || album.is_empty() || title.is_empty() {
        return Err("Artist, release or title produces an empty filename after removing unsafe characters".into());
    }
    let parts: Vec<&str> = tpl.split('/').collect();
    let mut resolved_path = root.to_path_buf();

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let mut part_str = part.to_string();
        // The album tag remains untouched; do not append the same year twice.
        if !year.is_empty() && album.trim_end().ends_with(&format!("({year})")) {
            part_str = part_str.replace("{album} ({year})", "{album}");
        }

        if !disc_relevant && part_str.contains("{disc}") {
            continue;
        }

        let disc_folder = if disc_relevant {
            format!("Disc {:02}", disc_num)
        } else {
            String::new()
        };

        let disc_prefix = if disc_relevant {
            format!("{:02}.", disc_num)
        } else {
            String::new()
        };

        let track_str = format!("{:02}", track_num);

        if year.is_empty() { part_str = part_str.replace(" ({year})", ""); }
        part_str = part_str
            .replace("{albumartist}", &artist)
            .replace("{artist}", tags.get("artist").map(String::as_str).unwrap_or(&artist))
            .replace("{discnumber}", &format!("{disc_num:02}"))
            .replace("{album}", &album)
            .replace("{title}", &title)
            .replace("{year}", year)
            .replace("{disc}", &disc_folder)
            .replace("{disc_prefix}", &disc_prefix)
            .replace("{tracknumber}", &track_str);

        // Trim template whitespace without removing literal punctuation from tags.
        part_str = part_str.trim().to_string();

        let safe = safe_component(&part_str, is_last)?;
        resolved_path.push(safe);
    }

    // Append extension
    let ext = extension.trim_start_matches('.');
    let filename = resolved_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let with_ext = format!("{}.{}", filename, ext);
    resolved_path.set_file_name(with_ext);

    Ok(resolved_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_component() {
        assert_eq!(safe_component("Queen", true).unwrap(), "Queen");
        assert_eq!(
            safe_component("AC/DC", true).unwrap(),
            "AC - DC"
        );
        assert_eq!(
            safe_component("A:B?C*D", true).unwrap(),
            "A - BCD"
        );
        assert_eq!(safe_component("CON", true).unwrap(), "_CON");
        assert_eq!(safe_component("Parallel / S", true).unwrap(), "Parallel - S");
        assert_eq!(safe_component("Belong - Mirth", true).unwrap(), "Belong - Mirth");
        assert_eq!(safe_component("Babsy.", false).unwrap(), "Babsy");
        assert_eq!(safe_component("Ending with dot.", true).unwrap(), "Ending with dot");
    }

    #[test]
    fn test_format_layout() {
        let root = Path::new("/Music");
        let mut tags = HashMap::new();
        tags.insert("albumartist".to_string(), "Queen".to_string());
        tags.insert("album".to_string(), "A Night at the Opera".to_string());
        tags.insert("title".to_string(), "Bohemian Rhapsody".to_string());
        tags.insert("date".to_string(), "1975-11-21".to_string());
        tags.insert("tracknumber".to_string(), "11/12".to_string());
        tags.insert("discnumber".to_string(), "1".to_string());
        tags.insert("disctotal".to_string(), "1".to_string());

        let path = format_layout(root, &tags, None, "flac").unwrap();
        assert_eq!(
            path,
            PathBuf::from("/Music/Queen/A Night at the Opera (1975)/11 - Bohemian Rhapsody.flac")
        );
    }

    #[test]
    fn real_library_punctuation_preserves_text_without_extra_spaces_or_dashes() {
        for (tag, expected) in [
            ("> album title goes here <", "album title goes here"),
            ("W:/2016ALBUM/", "W - 2016ALBUM"),
            ("FoV_v2.0 [CHEDI.hack::]", "FoV_v2.0 [CHEDI.hack]"),
            ("//M\\\\", "M"), ("https://", "https"),
            ("while(1<2)", "while(12)"),
            ("when da snacks match da fit (:prayer hands:)", "when da snacks match da fit (prayer hands)"),
            ("Acid Disk 2 ", "Acid Disk 2"),
            ("Chopped &  Screwed", "Chopped & Screwed"),
        ] {
            let tags = HashMap::from([("albumartist".into(), "Artist".into()), ("album".into(), tag.into()), ("date".into(), "2012".into()), ("title".into(), "Resonator ()".into())]);
            assert_eq!(format_layout(Path::new("/Music"), &tags, None, "flac").unwrap(), PathBuf::from(format!("/Music/Artist/{expected} (2012)/01 - Resonator ().flac")));
        }
    }

    #[test]
    fn folder_operations_describe_actual_changes() {
        assert_eq!(folder_operation("/a/old.flac", Some("/b/new.flac")), "Move and rename file");
        assert_eq!(folder_operation("/a/song.flac", Some("/b/song.flac")), "Move file");
        assert_eq!(folder_operation("/a/old.flac", Some("/a/new.flac")), "Rename file");
        assert_eq!(folder_operation("/a/song.flac", None), "No change");
    }

    #[test]
    fn album_year_suffix_is_not_duplicated() {
        let mut tags = HashMap::from([("albumartist".into(), "Elohim".into()), ("album".into(), "Elohim (2016)".into()), ("date".into(), "2016-05-20".into()), ("title".into(), "Song".into())]);
        let path = format_layout(Path::new("/Music"), &tags, None, "flac").unwrap();
        assert_eq!(path, PathBuf::from("/Music/Elohim/Elohim (2016)/01 - Song.flac"));
        tags.insert("date".into(), "2020".into());
        assert!(format_layout(Path::new("/Music"), &tags, None, "flac").unwrap().to_string_lossy().contains("Elohim (2016) (2020)"));
    }

    #[test]
    fn test_format_layout_multi_disc() {
        let root = Path::new("/Music");
        let mut tags = HashMap::new();
        tags.insert("albumartist".to_string(), "Pink Floyd".to_string());
        tags.insert("album".to_string(), "The Wall".to_string());
        tags.insert("title".to_string(), "Hey You".to_string());
        tags.insert("year".to_string(), "1979".to_string());
        tags.insert("tracknumber".to_string(), "1".to_string());
        tags.insert("discnumber".to_string(), "2".to_string());
        tags.insert("disctotal".to_string(), "2".to_string());

        let path = format_layout(root, &tags, None, "flac").unwrap();
        assert_eq!(
            path,
            PathBuf::from("/Music/Pink Floyd/The Wall (1979)/Disc 02/02.01 - Hey You.flac")
        );
    }
}
