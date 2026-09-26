use std::collections::HashMap;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const DEFAULT_LAYOUT: &str =
    "{albumartist}/{album} ({year})/{disc}/{disc_prefix}{tracknumber} - {title}";

pub fn safe_component(value: &str, _final_part: bool) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err("Component is empty".to_string());
    }

    // Strip/replace illegal filename characters
    let mut cleaned = String::with_capacity(value.len());
    let mut after_separator = false;
    for c in value.chars() {
        if after_separator && c.is_whitespace() { continue; }
        after_separator = false;
        match c {
            '/' | '\\' | ':' | '|' => {
                while cleaned.ends_with(char::is_whitespace) { cleaned.pop(); }
                if !cleaned.ends_with(" -") && !cleaned.is_empty() { cleaned.push_str(" -"); }
                if !cleaned.is_empty() { cleaned.push(' '); }
                after_separator = true;
            }
            '*' | '?' | '"' | '<' | '>' | '\0'..='\x1f' | '\x7f' => {
                // skip illegal characters
            }
            _ => cleaned.push(c),
        }
    }

    let mut result: String = cleaned.trim().nfc().collect();

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

    let parts: Vec<&str> = tpl.split('/').collect();
    let mut resolved_path = root.to_path_buf();

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let mut part_str = part.to_string();

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

        part_str = part_str
            .replace("{albumartist}", artist)
            .replace("{album}", album)
            .replace("{title}", title)
            .replace("{year}", year)
            .replace("{disc}", &disc_folder)
            .replace("{disc_prefix}", &disc_prefix)
            .replace("{tracknumber}", &track_str);

        // Clean up empty parentheses e.g. " ()" if year is missing
        part_str = part_str.replace(" ()", "").trim().to_string();

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
