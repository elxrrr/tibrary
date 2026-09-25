use crate::db::LocalFileRecord;
use crate::musical_keys::key_changes;
use crate::organisation::format_layout;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRowPlan {
    pub path: String,
    pub root: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub current_tags: HashMap<String, String>,
    pub changes: HashMap<String, String>,
    pub target: Option<String>,
    pub issues: Vec<String>,
}

pub fn extract_tags_map(metadata: &Option<Value>) -> HashMap<String, String> {
    let mut tags = HashMap::new();
    let Some(ref val) = metadata else {
        return tags;
    };

    if let Some(obj) = val.as_object() {
        for (k, v) in obj {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Array(arr) => arr
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => continue,
            };
            if !s.trim().is_empty() {
                tags.insert(k.to_lowercase(), s);
            }
        }
    }
    tags
}

pub fn plan_workflow(
    rows: &[LocalFileRecord],
    action: &str,
    template: Option<&str>,
) -> Vec<WorkflowRowPlan> {
    let mut plans = Vec::new();

    for row in rows {
        let current_tags = extract_tags_map(&row.metadata);
        let mut changes = HashMap::new();
        let mut issues = Vec::new();
        let mut target = None;

        let title = current_tags
            .get("title")
            .cloned()
            .unwrap_or_else(|| {
                Path::new(&row.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown Title")
                    .to_string()
            });

        let artist = current_tags
            .get("albumartist")
            .or_else(|| current_tags.get("album_artist"))
            .or_else(|| current_tags.get("artist"))
            .cloned()
            .unwrap_or_else(|| "Unknown Artist".to_string());

        let album = current_tags
            .get("album")
            .cloned()
            .unwrap_or_else(|| "Unknown Album".to_string());

        match action {
            "keys" => {
                let tags_vec: HashMap<String, Vec<String>> = current_tags
                    .iter()
                    .map(|(k, v)| (k.clone(), vec![v.clone()]))
                    .collect();
                let (key_edits, issue) = key_changes(&tags_vec);
                for (k, vals) in key_edits {
                    if let Some(first_val) = vals.first() {
                        changes.insert(k, first_val.clone());
                    }
                }
                if !issue.is_empty() {
                    issues.push(issue);
                }
            }
            "numbers" => {
                // Track number zero-padding
                if let Some(tn) = current_tags.get("tracknumber").or_else(|| current_tags.get("track")) {
                    let num_part = tn.split('/').next().unwrap_or(tn).trim();
                    if let Ok(n) = num_part.parse::<u32>() {
                        let formatted = format!("{:02}", n);
                        if num_part != formatted {
                            changes.insert("tracknumber".to_string(), formatted);
                        }
                    }
                }
                // Disc number normalization
                if let Some(dn) = current_tags.get("discnumber").or_else(|| current_tags.get("disc")) {
                    let num_part = dn.split('/').next().unwrap_or(dn).trim();
                    if let Ok(n) = num_part.parse::<u32>() {
                        let formatted = format!("{:02}", n);
                        if num_part != formatted {
                            changes.insert("discnumber".to_string(), formatted);
                        }
                    }
                }
            }
            "lyrics" => {
                for k in &["lyrics", "unsyncedlyrics", "syncedlyrics"] {
                    if current_tags.contains_key(*k) {
                        changes.insert(k.to_string(), "".to_string());
                    }
                }
            }
            "organise" => {
                let ext = Path::new(&row.path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("flac");

                if let Ok(target_path) = format_layout(Path::new(&row.root), &current_tags, template, ext) {
                    let target_str = target_path.display().to_string();
                    if target_str != row.path {
                        target = Some(target_str);
                    }
                }
            }
            _ => {}
        }

        if !changes.is_empty() || target.is_some() {
            plans.push(WorkflowRowPlan {
                path: row.path.clone(),
                root: row.root.clone(),
                title,
                artist,
                album,
                current_tags,
                changes,
                target,
                issues,
            });
        }
    }

    plans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_workflow_keys_and_numbers() {
        let mut meta = serde_json::Map::new();
        meta.insert("title".to_string(), Value::String("Song".to_string()));
        meta.insert("artist".to_string(), Value::String("Artist".to_string()));
        meta.insert("album".to_string(), Value::String("Album".to_string()));
        meta.insert("tracknumber".to_string(), Value::String("1".to_string()));
        meta.insert("initialkey".to_string(), Value::String("G Major".to_string()));

        let rows = vec![LocalFileRecord {
            path: "/root/song.flac".to_string(),
            root: "/root".to_string(),
            size: 1000,
            mtime: 1000,
            metadata: Some(Value::Object(meta)),
            error: None,
            present: true,
        }];

        // Test numbers workflow
        let num_plans = plan_workflow(&rows, "numbers", None);
        assert_eq!(num_plans.len(), 1);
        assert_eq!(num_plans[0].changes.get("tracknumber").unwrap(), "01");

        // Test keys workflow (G Major -> 9B)
        let key_plans = plan_workflow(&rows, "keys", None);
        assert_eq!(key_plans.len(), 1);
        assert_eq!(key_plans[0].changes.get("initialkey").unwrap(), "9B");
    }

    #[test]
    fn test_plan_workflow_organise() {
        let mut meta = serde_json::Map::new();
        meta.insert("title".to_string(), Value::String("Bohemian Rhapsody".to_string()));
        meta.insert("artist".to_string(), Value::String("Queen".to_string()));
        meta.insert("album".to_string(), Value::String("A Night at the Opera".to_string()));
        meta.insert("date".to_string(), Value::String("1975".to_string()));
        meta.insert("tracknumber".to_string(), Value::String("11".to_string()));

        let rows = vec![LocalFileRecord {
            path: "/root/unorganized.flac".to_string(),
            root: "/root".to_string(),
            size: 1000,
            mtime: 1000,
            metadata: Some(Value::Object(meta)),
            error: None,
            present: true,
        }];

        let org_plans = plan_workflow(&rows, "organise", None);
        assert_eq!(org_plans.len(), 1);
        assert!(org_plans[0].target.as_ref().unwrap().contains("Queen/A Night at the Opera (1975)/11 - Bohemian Rhapsody.flac"));
    }
}
