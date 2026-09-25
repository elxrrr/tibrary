use std::collections::{HashMap, HashSet};

const MINOR: &[&str] = &[
    "G#m", "D#m", "A#m", "Fm", "Cm", "Gm", "Dm", "Am", "Em", "Bm", "F#m", "C#m",
];
const MAJOR: &[&str] = &[
    "B", "F#", "C#", "G#", "D#", "A#", "F", "C", "G", "D", "A", "E",
];

const FLATS: &[(&str, &str)] = &[
    ("Cb", "B"),
    ("Db", "C#"),
    ("Eb", "D#"),
    ("Fb", "E"),
    ("Gb", "F#"),
    ("Ab", "G#"),
    ("Bb", "A#"),
    ("B#", "C"),
    ("E#", "F"),
];

pub const KEY_TAGS: &[&str] = &["initialkey", "key", "tkey"];

pub fn canonical_key(value: &str) -> Option<String> {
    let text = value
        .trim()
        .replace('♯', "#")
        .replace('♭', "b");

    // Check Camelot format e.g. "8A", "11B", "1a"
    let len = text.len();
    if (2..=3).contains(&len) {
        let (num_part, letter_part) = text.split_at(len - 1);
        if let Ok(num) = num_part.parse::<usize>() {
            if (1..=12).contains(&num) {
                let letter = letter_part.to_ascii_uppercase();
                if letter == "A" {
                    return Some(MINOR[num - 1].to_string());
                } else if letter == "B" {
                    return Some(MAJOR[num - 1].to_string());
                }
            }
        }
    }

    // Normalize words: sharp -> #, flat -> b
    let lower = text.to_lowercase();
    let replaced = lower.replace("sharp", "#").replace("flat", "b");
    let trimmed = replaced.trim();

    // Check note letter
    let mut chars = trimmed.chars();
    let first = chars.next()?.to_ascii_uppercase();
    if !('A'..='G').contains(&first) {
        return None;
    }

    let mut accidental = "";
    let rest = chars.as_str().trim_start();
    let after_accidental = if let Some(stripped) = rest.strip_prefix('#') {
        accidental = "#";
        stripped.trim_start()
    } else if let Some(stripped) = rest.strip_prefix('b') {
        accidental = "b";
        stripped.trim_start()
    } else {
        rest
    };

    let note = format!("{}{}", first, accidental);
    let canonical_note = FLATS
        .iter()
        .find(|(f, _)| *f == note)
        .map(|(_, m)| m.to_string())
        .unwrap_or(note);

    let is_minor = after_accidental.starts_with("min")
        || after_accidental == "m"
        || after_accidental.starts_with("minor");

    if is_minor {
        Some(format!("{}m", canonical_note))
    } else {
        Some(canonical_note)
    }
}

pub fn camelot_key(value: &str) -> Option<String> {
    let note = canonical_key(value)?;
    if let Some(pos) = MINOR.iter().position(|&m| m == note) {
        return Some(format!("{}A", pos + 1));
    }
    if let Some(pos) = MAJOR.iter().position(|&m| m == note) {
        return Some(format!("{}B", pos + 1));
    }
    None
}

pub fn key_changes(tags: &HashMap<String, Vec<String>>) -> (HashMap<String, Vec<String>>, String) {
    let mut present = Vec::new();
    for key in KEY_TAGS {
        if let Some(vals) = tags.get(*key) {
            for v in vals {
                if !v.trim().is_empty() {
                    present.push(v.clone());
                }
            }
        }
    }

    if present.is_empty() {
        return (HashMap::new(), String::new());
    }

    let mut camelot_values = HashSet::new();
    for p in present {
        match camelot_key(&p) {
            Some(c) => {
                camelot_values.insert(c);
            }
            None => {
                return (
                    HashMap::new(),
                    "Musical key notation is unknown or conflicting; unchanged".to_string(),
                );
            }
        }
    }

    if camelot_values.len() != 1 {
        return (
            HashMap::new(),
            "Musical key notation is unknown or conflicting; unchanged".to_string(),
        );
    }

    let target_key = camelot_values.into_iter().next().unwrap();
    let current_initial_key = tags.get("initialkey");

    if current_initial_key.map(|v| v.as_slice()) == Some(std::slice::from_ref(&target_key)) {
        (HashMap::new(), String::new())
    } else {
        let mut changes = HashMap::new();
        changes.insert("initialkey".to_string(), vec![target_key]);
        (changes, String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camelot_key_conversion() {
        // Camelot direct
        assert_eq!(camelot_key("8A"), Some("8A".to_string()));
        assert_eq!(camelot_key("11B"), Some("11B".to_string()));
        assert_eq!(camelot_key("1B"), Some("1B".to_string()));

        // Standard notation
        assert_eq!(camelot_key("Am"), Some("8A".to_string()));
        assert_eq!(camelot_key("A minor"), Some("8A".to_string()));
        assert_eq!(camelot_key("C"), Some("8B".to_string()));
        assert_eq!(camelot_key("C major"), Some("8B".to_string()));

        // Flats and sharps
        assert_eq!(camelot_key("Bb"), Some("6B".to_string()));
        assert_eq!(camelot_key("A#"), Some("6B".to_string()));
        assert_eq!(camelot_key("G#m"), Some("1A".to_string()));
        assert_eq!(camelot_key("Abm"), Some("1A".to_string()));

        // Invalid
        assert_eq!(camelot_key("invalid"), None);
        assert_eq!(camelot_key(""), None);
    }

    #[test]
    fn test_key_changes() {
        let mut tags = HashMap::new();
        tags.insert("key".to_string(), vec!["Am".to_string()]);

        let (changes, issue) = key_changes(&tags);
        assert!(issue.is_empty());
        assert_eq!(changes.get("initialkey"), Some(&vec!["8A".to_string()]));

        // Already correct
        tags.insert("initialkey".to_string(), vec!["8A".to_string()]);
        let (changes2, _) = key_changes(&tags);
        assert!(changes2.is_empty());
    }
}
