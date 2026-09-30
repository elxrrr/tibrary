use crate::db::LocalFileRecord;
use crate::musical_keys::key_changes;
use crate::organisation::format_layout;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use unicode_normalization::UnicodeNormalization;

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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PlanScope {
    database: PathBuf,
    roots: Vec<String>,
    action: String,
}

struct CachedPlan {
    fingerprint: [u8; 32],
    rows: Vec<WorkflowRowPlan>,
    used: Instant,
}

// Only the latest plan for each operation is retained. These are derived views
// of the shared index, so they do not need another durable copy of every tag.
const MAX_CACHED_OPERATIONS: usize = 16;
const MAX_CACHED_PLAN_ROWS: usize = 80_000;
static PLAN_CACHE: OnceLock<Mutex<HashMap<PlanScope, CachedPlan>>> = OnceLock::new();

fn plan_scope(db: &crate::db::TursoDb, rows: &[LocalFileRecord], action: &str) -> PlanScope {
    let mut roots: Vec<_> = rows.iter().map(|row| row.root.as_str()).collect();
    roots.sort_unstable();
    roots.dedup();
    PlanScope { database: db.path.clone(), roots: roots.into_iter().map(str::to_owned).collect(), action: action.to_owned() }
}

fn plan_fingerprint(rows: &[LocalFileRecord], template: Option<&str>, online_revision: Option<u64>) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&(template, online_revision)).unwrap_or_default());
    let mut ordered: Vec<_> = rows.iter().collect();
    ordered.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    for row in ordered {
        // Include metadata itself: other workflows may update the index without
        // changing the on-disk file's size or timestamp.
        digest.update(serde_json::to_vec(&(
            &row.path, &row.root, row.size, row.mtime, row.present,
            &row.error, &row.metadata,
        )).unwrap_or_default());
    }
    digest.finalize().into()
}

fn cached_plan(scope: &PlanScope, fingerprint: &[u8; 32]) -> Option<Vec<WorkflowRowPlan>> {
    let mut cache = PLAN_CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
    let saved = cache.get_mut(scope).filter(|entry| &entry.fingerprint == fingerprint)?;
    saved.used = Instant::now();
    Some(saved.rows.clone())
}

fn save_plan(scope: PlanScope, fingerprint: [u8; 32], rows: &[WorkflowRowPlan]) {
    if rows.len() > MAX_CACHED_PLAN_ROWS { return; }
    let mut cache = PLAN_CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
    cache.insert(scope, CachedPlan { fingerprint, rows: rows.to_vec(), used: Instant::now() });
    while cache.len() > MAX_CACHED_OPERATIONS
        || cache.values().map(|entry| entry.rows.len()).sum::<usize>() > MAX_CACHED_PLAN_ROWS
    {
        let Some(oldest) = cache.iter().min_by_key(|(_, entry)| entry.used).map(|(key, _)| key.clone()) else { break; };
        cache.remove(&oldest);
    }
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
    if let Some(raw) = val.get("tags").and_then(|v| v.as_object()) {
        for (key, value) in raw {
            let text = value.as_str().map(str::to_owned).or_else(|| {
                value.as_array().map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
            });
            if let Some(text) = text {
                tags.insert(key.to_lowercase(), text);
            }
        }
    }
    for (canonical, alias) in [
        ("albumartist", "album_artist"),
        ("artist", "track_artist"),
        ("tracknumber", "track"),
        ("tracknumber", "track_number"),
        ("discnumber", "disc_number"),
        ("tracktotal", "track_total"),
        ("disctotal", "disc_total"),
        ("initialkey", "musical_key"),
    ] {
        if let Some(value) = tags.get(alias).cloned() {
            tags.entry(canonical.into()).or_insert(value);
        }
    }
    tags
}

fn number(tags: &HashMap<String, String>, key: &str) -> u32 {
    tags.get(key).and_then(|s| s.split('/').next()).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}
fn total(tags: &HashMap<String, String>, key: &str, position: &str) -> u32 {
    let separate = number(tags, key);
    if separate > 0 { separate } else {
        tags.get(position).and_then(|s| s.split('/').nth(1)).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
    }
}
pub(crate) fn release_key(row: &LocalFileRecord, tags: &HashMap<String, String>) -> (String, String, String) {
    let mut folder = Path::new(&row.path).parent().unwrap_or(Path::new(&row.root));
    if folder.file_name().is_some_and(|s| s.to_string_lossy().to_lowercase().starts_with("disc ")) {
        folder = folder.parent().unwrap_or(folder);
    }
    (folder.display().to_string(), tags.get("albumartist").or(tags.get("artist")).cloned().unwrap_or_default(), tags.get("album").cloned().unwrap_or_default())
}

/// Uses the shared catalogue snapshot only: opening Correct tags never fetches online data.
pub async fn plan_cached(db: &crate::db::TursoDb, rows: &[LocalFileRecord], action: &str, template: Option<&str>) -> Result<Vec<WorkflowRowPlan>, String> {
    let scope = plan_scope(db, rows, action);
    let online_revision = (action == "numbers").then(|| db.revision.load(std::sync::atomic::Ordering::SeqCst));
    let fingerprint = plan_fingerprint(rows, template, online_revision);
    if let Some(saved) = cached_plan(&scope, &fingerprint) { return Ok(saved); }
    let mut plans = plan_workflow(rows, action, template);
    if action != "numbers" {
        save_plan(scope, fingerprint, &plans);
        return Ok(plans);
    }
    let market = db.get_settings().await?["general"]["market"].as_str().unwrap_or("GB").to_string();
    let conn = db.connect()?;
    let mut query = conn.query("SELECT payload FROM catalogue WHERE market=?", (market.as_str(),)).await.map_err(|e| e.to_string())?;
    let mut releases = Vec::new();
    while let Some(row) = query.next().await.map_err(|e| e.to_string())? {
        if let Ok(cat) = serde_json::from_str::<crate::tidal::TidalCatalogue>(&row.get::<String>(0).unwrap_or_default()) {
            releases.extend(cat.releases.into_iter().filter(|r| r.tracks_loaded));
        }
    }
    let mut groups: HashMap<_, Vec<_>> = HashMap::new();
    let mut keys = HashMap::new();
    for row in rows.iter().filter(|r| r.present) {
        let tags = extract_tags_map(&row.metadata);
        let key = release_key(row, &tags);
        keys.insert(row.path.as_str(), key.clone());
        groups.entry(key).or_default().push(crate::release_matching::LocalTrackInfo {
            path: row.path.clone(), title: tags.get("title").cloned().unwrap_or_default(),
            artist: tags.get("albumartist").or(tags.get("artist")).cloned().unwrap_or_default(),
            album: tags.get("album").cloned().unwrap_or_default(),
            duration: row.metadata.as_ref().and_then(|m| m["duration"].as_f64()).unwrap_or(0.0),
            track_number: number(&tags,"tracknumber"), disc_number: number(&tags,"discnumber").max(1), isrc: tags.get("isrc").cloned(),
        });
    }
    let mut by_title: HashMap<_,Vec<_>> = HashMap::new();
    for release in &releases { by_title.entry(crate::matching::title_key(&release.title)).or_default().push(release); }
    for plan in plans.iter_mut().filter(|p| !p.issues.is_empty()) {
        let Some(key) = keys.get(plan.path.as_str()) else { continue };
        let peers = &groups[key];
        let release_candidates = by_title.get(&crate::matching::title_key(&plan.album)).cloned().unwrap_or_default();
        let candidates: Vec<_> = release_candidates.into_iter().filter(|r| {
            crate::matching::title_key(&r.title) == crate::matching::title_key(&plan.album)
                && crate::matching::name_key(&r.artist) == crate::matching::name_key(&plan.artist)
                && peers.iter().all(|p| p.track_number > 0 && r.tracks.iter().any(|t|
                    t.track_number == p.track_number && t.disc_number == p.disc_number
                    && (p.duration > 0.0 || p.isrc.as_ref().is_some_and(|s| !s.trim().is_empty()))
                    && crate::release_matching::recording_matches(&p.title, p.duration, p.isrc.as_deref(), &t.title, t.duration, t.isrc.as_deref(), true)))
        }).collect();
        for (position, total_key) in [("tracknumber", "tracktotal"), ("discnumber", "disctotal")] {
            let disc = number(&plan.current_tags, "discnumber").max(1);
            let max_position = peers.iter().filter(|p| position == "discnumber" || p.disc_number == disc)
                .map(|p| if position == "discnumber" { p.disc_number } else { p.track_number }).max().unwrap_or(0);
            if total(&plan.current_tags, total_key, position) >= max_position || number(&plan.changes, total_key) >= max_position { continue; }
            let counts: std::collections::HashSet<u32> = candidates.iter().map(|r| if position == "discnumber" {
                r.tracks.iter().map(|t| t.disc_number).max().unwrap_or(1)
            } else { r.tracks.iter().filter(|t| t.disc_number == disc).count() as u32 }).collect();
            if counts.len() == 1 {
                let count = *counts.iter().next().unwrap();
                if count >= max_position {
                    plan.changes.insert(total_key.into(), format!("{count:02}"));
                    plan.issues.retain(|issue| !issue.starts_with(&format!("{total_key} ")));
                    plan.issues.push(format!("{total_key}: {count:02} confirmed by cached release(s) {} with matching recordings and positions", candidates.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join(", ")));
                }
            }
        }
    }
    // Explicitly chosen complete releases can reconcile alternate local disc layouts.
    // Merely sharing one recording is insufficient: every file must match uniquely.
    for group in crate::release_anchor::verified(db, rows, &market).await? {
        let discs = group.release.tracks.iter().map(|t|t.disc_number).max().unwrap_or(1);
        for (file, track) in group.files.iter().zip(&group.release.tracks) {
            let tags = extract_tags_map(&file.metadata);
            let count = group.release.tracks.iter().filter(|t|t.disc_number == track.disc_number).count() as u32;
            let corrected = [("tracknumber", track.track_number),("discnumber",track.disc_number),("tracktotal",count),("disctotal",discs)];
            let changes: HashMap<String,String> = corrected.into_iter().filter_map(|(k,n)| { let v=format!("{n:02}"); (tags.get(k)!=Some(&v)).then(||(k.to_string(),v)) }).collect();
            if changes.is_empty() { continue; }
            let index = plans.iter().position(|p|p.path == file.path).unwrap_or_else(|| {
                plans.push(WorkflowRowPlan {path:file.path.clone(),root:file.root.clone(),title:tags.get("title").cloned().unwrap_or_default(),artist:tags.get("albumartist").or(tags.get("artist")).cloned().unwrap_or_default(),album:tags.get("album").cloned().unwrap_or_default(),current_tags:tags.clone(),changes:HashMap::new(),target:None,issues:vec![]});
                plans.len()-1
            });
            let plan=&mut plans[index];
            plan.changes.extend(changes);
            plan.issues.retain(|i| !i.starts_with("tracktotal") && !i.starts_with("disctotal"));
            plan.issues.push(format!("Complete recording sequence verified against chosen release {}: use online Disc {:02}/{:02}, Track {:02}/{:02}; files stay in place",group.release.id,track.disc_number,discs,track.track_number,count));
        }
    }
    // A concurrent catalogue write must not publish an old plan as current.
    if online_revision == Some(db.revision.load(std::sync::atomic::Ordering::SeqCst)) {
        save_plan(scope, fingerprint, &plans);
    }
    Ok(plans)
}

pub fn plan_workflow(
    rows: &[LocalFileRecord],
    action: &str,
    template: Option<&str>,
) -> Vec<WorkflowRowPlan> {
    let mut plans = Vec::new();
    let mut folder_years: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
    let mut indexed: HashMap<_, Vec<_>> = HashMap::new();
    for row in rows.iter().filter(|r| r.present) {
        let tags = extract_tags_map(&row.metadata);
        let key = release_key(row, &tags);
        for date in [tags.get("year"), tags.get("date")].into_iter().flatten() {
            if let Some(year) = date.trim().get(..4).filter(|y| y.chars().all(|c| c.is_ascii_digit())) {
                folder_years.entry(key.0.clone()).or_default().insert(year.to_owned());
            }
        }
        indexed.entry(key).or_default().push(tags);
    }

    for row in rows.iter().filter(|r| r.present) {
        let current_tags = extract_tags_map(&row.metadata);
        let mut changes = HashMap::new();
        let mut issues = Vec::new();
        let mut target = None;

        let title = current_tags.get("title").cloned().unwrap_or_else(|| {
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
            "dates" => {
                if let Some(date) = current_tags.get("date") {
                    let clean = date.trim().replace(['/', '.'], "-");
                    let clean = clean.split('T').next().unwrap_or(&clean);
                    let valid = (clean.len() == 4 && clean.parse::<u32>().is_ok())
                        || chrono::NaiveDate::parse_from_str(clean, "%Y-%m-%d").is_ok();
                    if valid && clean != date {
                        changes.insert("date".into(), clean.into());
                    }
                }
            }
            "numbers" => {
                for (number, total) in [("tracknumber", "tracktotal"), ("discnumber", "disctotal")]
                {
                    if let Some(value) = current_tags.get(number) {
                        let parts: Vec<_> = value.split('/').collect();
                        if let Ok(n) = parts[0].trim().parse::<u32>() {
                            if n > 0 {
                                let formatted = format!("{n:02}");
                                if &formatted != value {
                                    changes.insert(number.into(), formatted);
                                }
                            }
                        }
                        if !current_tags.contains_key(total) && parts.len() == 2 {
                            if let Ok(n) = parts[1].trim().parse::<u32>() {
                                if n > 0 {
                                    changes.insert(total.into(), format!("{n:02}"));
                                }
                            }
                        }
                    }
                    if let Some(value) = current_tags.get(total) {
                        if let Ok(n) = value.parse::<u32>() {
                            if n > 0 && value != &format!("{n:02}") {
                                changes.insert(total.into(), format!("{n:02}"));
                            }
                        }
                    }
                }
                let key = release_key(row, &current_tags);
                for (position, total_key) in [("tracknumber", "tracktotal"), ("discnumber", "disctotal")] {
                    let disc = number(&current_tags, "discnumber").max(1);
                    let peers: Vec<_> = indexed[&key].iter().filter(|t| position == "discnumber" || number(t,"discnumber").max(1) == disc).collect();
                    let max_position = peers.iter().map(|t| number(t,position)).max().unwrap_or(0);
                    let saved = total(&current_tags, total_key, position);
                    if max_position > saved {
                        let known: std::collections::HashSet<_> = peers.iter().map(|t| total(t,total_key,position)).filter(|n| *n >= max_position).collect();
                        if known.len() == 1 {
                            let count = *known.iter().next().unwrap();
                            changes.insert(total_key.into(), format!("{count:02}"));
                            issues.push(format!("{total_key} {saved} is below position {max_position}; sibling tags agree on {count:02}"));
                        } else {
                            issues.push(format!("{total_key} {saved} is below position {max_position}; a verified release total is needed (local files may be incomplete)"));
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

                if let Ok(target_path) =
                    format_layout(Path::new(&row.root), &current_tags, template, ext)
                {
                    let target_str = target_path.display().to_string();
                    // APFS spelling differences alone do not require moving audio.
                    if target_str.nfc().collect::<String>().to_lowercase() != row.path.nfc().collect::<String>().to_lowercase() {
                        let years = folder_years.get(&release_key(row, &current_tags).0).cloned().unwrap_or_default();
                        if template.unwrap_or(crate::organisation::DEFAULT_LAYOUT).contains("{year}") && years.len() > 1 {
                            issues.push(format!("Conflicting release years in saved tags: {}. No move proposed: correct the release dates first so the album stays together.", years.into_iter().collect::<Vec<_>>().join(", ")));
                        } else {
                        let source_spaces = row.path.chars().filter(|c| *c == ' ').count();
                        let target_spaces = target_str.chars().filter(|c| *c == ' ').count();
                        if source_spaces > target_spaces && row.path.split_whitespace().collect::<Vec<_>>().join(" ") == target_str {
                            issues.push(format!("Remove {} extra space(s); words and tags stay unchanged", source_spaces - target_spaces));
                        } else if row.path.replace('\u{a0}', " ") == target_str {
                            issues.push("Replace non-breaking space with a normal space; words and tags stay unchanged".into());
                        }
                        let source = Path::new(&row.path).strip_prefix(&row.root).unwrap_or(Path::new(&row.path));
                        let destination = target_path.strip_prefix(&row.root).unwrap_or(&target_path);
                        if source.parent() != destination.parent() {
                            issues.push(format!("Folder: {} → {}", source.parent().unwrap_or(Path::new("")).display(), destination.parent().unwrap_or(Path::new("")).display()));
                        }
                        if source.file_name() != destination.file_name() {
                            issues.push(format!("Filename: {} → {}", source.file_name().unwrap_or_default().to_string_lossy(), destination.file_name().unwrap_or_default().to_string_lossy()));
                        }
                        target = Some(target_str);
                        }
                    }
                }
            }
            _ => {}
        }

        if !changes.is_empty() || target.is_some() || !issues.is_empty() {
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

    fn sample(n: u32, total: u32) -> LocalFileRecord {
        LocalFileRecord { path: format!("/root/Artist/Album/{n}.flac"), root: "/root".into(), size: 1, mtime: 1,
            metadata: Some(serde_json::json!({"albumartist":"Artist","album":"Album","title":format!("Song {n}"),"duration":180.0,"tracknumber":format!("{n:02}/{total}"),"discnumber":"01","disctotal":"01"})), error:None, present:true }
    }
    #[test]
    fn impossible_totals_use_sibling_evidence_not_file_count() {
        let plans = plan_workflow(&[sample(1,12), sample(4,1)], "numbers", None);
        assert_eq!(plans.iter().find(|p| p.path.ends_with("4.flac")).unwrap().changes["tracktotal"], "12");
        let plans = plan_workflow(&[sample(1,1), sample(4,1)], "numbers", None);
        assert!(plans.iter().all(|p| !p.issues.is_empty()));
        assert!(plans.iter().all(|p| p.changes.get("tracktotal").is_none_or(|v| v == "01")));
        let plans = plan_workflow(&[sample(1,12), sample(2,14), sample(4,1)], "numbers", None);
        assert_ne!(plans.iter().find(|p| p.path.ends_with("4.flac")).unwrap().changes.get("tracktotal").map(String::as_str), Some("12"));
    }
    #[tokio::test]
    async fn unchanged_local_plans_are_reused_and_changed_tags_or_templates_invalidate() {
        let dir = std::env::temp_dir().join(format!("tibrary-plan-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::TursoDb::open(&dir.join("test.sqlite")).await.unwrap();
        let mut rows = vec![sample(1, 12)];
        rows[0].metadata.as_mut().unwrap()["date"] = serde_json::json!("2024");
        let first = plan_cached(&db, &rows, "organise", None).await.unwrap();
        let scope = plan_scope(&db, &rows, "organise");
        let cached_pointer = || PLAN_CACHE.get().unwrap().lock().unwrap()[&scope].rows.as_ptr() as usize;
        let initial_pointer = cached_pointer();
        // Unrelated online work changes the global revision but not this local plan.
        db.bump_revision();
        let second = plan_cached(&db, &rows, "organise", None).await.unwrap();
        assert_eq!(first[0].target, second[0].target);
        assert_eq!(cached_pointer(), initial_pointer, "unchanged plans should retain their cached allocation");

        // Index metadata changes must invalidate even with the same mtime/size.
        rows[0].metadata.as_mut().unwrap()["title"] = serde_json::json!("Different title");
        let changed = plan_cached(&db, &rows, "organise", None).await.unwrap();
        assert!(changed[0].target.as_deref().unwrap().contains("Different title"));
        assert_ne!(cached_pointer(), initial_pointer);
        let template = Some("{albumartist}/{album}/{tracknumber} - {title}");
        let new_template = plan_cached(&db, &rows, "organise", template).await.unwrap();
        assert!(!new_template[0].target.as_deref().unwrap().contains("(2024)"));
        assert_ne!(changed[0].target, new_template[0].target);

        rows[0].present = false;
        assert!(plan_cached(&db, &rows, "organise", template).await.unwrap().is_empty());
        PLAN_CACHE.get().unwrap().lock().unwrap().remove(&scope);
        drop(db); std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn cached_exact_recordings_repair_totals_but_conflicting_editions_do_not() {
        let dir = std::env::temp_dir().join(format!("tibrary-numbers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::TursoDb::open(&dir.join("test.sqlite")).await.unwrap();
        let release = serde_json::json!({"id":"r","artist":"Artist","title":"Album","tracks_loaded":true,"tracks":(1..=12).map(|n| serde_json::json!({"id":n.to_string(),"title":format!("Song {n}"),"duration":180.0,"track_number":n,"disc_number":1})).collect::<Vec<_>>()});
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO catalogue(artist_id,market,payload) VALUES('a','GB',?)", (serde_json::json!({"releases":[release.clone()]}).to_string(),)).await.unwrap();
        let plans = plan_cached(&db, &[sample(1,1),sample(4,1)], "numbers", None).await.unwrap();
        assert!(plans.iter().all(|p| p.changes["tracktotal"] == "12"));
        conn.execute("INSERT INTO mappings(artist,tidal_id,status) VALUES('Artist','a','confirmed')", ()).await.unwrap();
        let local = vec![sample(1,1),sample(4,1)];
        for file in &local {
            db.apply_file_update(&file.path, &file.path, &file.root, file.metadata.as_ref().unwrap(), 1, 1).await.unwrap();
        }
        let paths: std::collections::HashSet<_> = local.iter().map(|f| f.path.clone()).collect();
        crate::maintenance::refresh_number_links(&db, "/root", &paths).await;
        let mut linked = conn.query("SELECT payload FROM track_links", ()).await.unwrap();
        let mut count = 0;
        while let Some(row) = linked.next().await.unwrap() {
            let payload: Value = serde_json::from_str(&row.get::<String>(0).unwrap()).unwrap();
            assert_eq!(payload["status"], "linked", "{payload}"); count += 1;
        }
        assert_eq!(count, 2);
        drop(linked);
        let mut other = release.clone(); other["id"] = serde_json::json!("other"); other["tracks"].as_array_mut().unwrap().pop();
        conn.execute("UPDATE catalogue SET payload=?", (serde_json::json!({"releases":[release,other]}).to_string(),)).await.unwrap();
        // The production catalogue writer publishes a revision after committing.
        db.bump_revision();
        let plans = plan_cached(&db, &[sample(1,1),sample(4,1)], "numbers", None).await.unwrap();
        assert!(plans.iter().all(|p| p.changes["tracktotal"] == "01"));
        drop(conn); drop(db); std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn test_plan_workflow_keys_and_numbers() {
        let mut meta = serde_json::Map::new();
        meta.insert("title".to_string(), Value::String("Song".to_string()));
        meta.insert("artist".to_string(), Value::String("Artist".to_string()));
        meta.insert("album".to_string(), Value::String("Album".to_string()));
        meta.insert("tracknumber".to_string(), Value::String("1".to_string()));
        meta.insert(
            "initialkey".to_string(),
            Value::String("G Major".to_string()),
        );

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
    fn organise_does_not_split_release_with_conflicting_year_tags() {
        let rows: Vec<_> = ["2022", "2023"].iter().enumerate().map(|(i, year)| LocalFileRecord {
            path: format!("/music/Artist/Release (2023)/0{} - Song.flac", i + 1),
            root: "/music".into(), size: 1, mtime: 1, present: true, error: None,
            metadata: Some(serde_json::json!({"albumartist":"Artist", "album":"Release", "title":"Song", "year":year, "date":"2023", "tracknumber":i+1})),
        }).collect();
        let plans = plan_workflow(&rows, "organise", None);
        assert_eq!(plans.len(), 1);
        assert!(plans[0].target.is_none());
        assert!(plans[0].issues[0].contains("Conflicting release years"));
    }

    #[test]
    fn test_plan_workflow_organise() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "title".to_string(),
            Value::String("Bohemian Rhapsody".to_string()),
        );
        meta.insert("artist".to_string(), Value::String("Queen".to_string()));
        meta.insert(
            "album".to_string(),
            Value::String("A Night at the Opera".to_string()),
        );
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
        assert!(org_plans[0]
            .target
            .as_ref()
            .unwrap()
            .contains("Queen/A Night at the Opera (1975)/11 - Bohemian Rhapsody.flac"));
    }
}
