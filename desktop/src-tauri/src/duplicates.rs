use crate::db::{LinkRow, LocalFileRecord, TursoDb};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalTrack {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub isrc: Option<String>,
    pub duration: f64,
    pub track_number: u32,
    pub disc_number: u32,
    pub size: i64,
    pub mtime: i64,
    pub bpm: Option<String>,
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRelease {
    pub id: String,
    pub folder: String,
    pub artist: String,
    pub title: String,
    pub date: String,
    pub tracks: Vec<LocalTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateCluster {
    pub cluster_id: String,
    pub master: LocalRelease,
    pub redundant: Vec<LocalRelease>,
    pub is_chained: bool,
    pub chain_summary: String,
    pub total_redundant_tracks: usize,
    pub total_recoverable_bytes: i64,
}

pub fn normalize_text(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn track_matches(source: &LocalTrack, target: &LocalTrack) -> bool {
    if normalize_text(&source.artist) != normalize_text(&target.artist) {
        return false;
    }
    crate::release_matching::recording_matches(
        &source.title,
        source.duration,
        source.isrc.as_deref(),
        &target.title,
        target.duration,
        target.isrc.as_deref(),
        true,
    )
}

pub fn is_release_contained(source: &LocalRelease, target: &LocalRelease) -> bool {
    if source.folder == target.folder || source.tracks.is_empty() {
        return false;
    }

    // Target must have at least as many tracks as source
    if target.tracks.len() < source.tracks.len() {
        return false;
    }

    // Artist compatibility check
    let s_artist = normalize_text(&source.artist);
    let t_artist = normalize_text(&target.artist);
    if !s_artist.is_empty()
        && !t_artist.is_empty()
        && s_artist != t_artist
        && !s_artist.contains(&t_artist)
        && !t_artist.contains(&s_artist)
    {
        return false;
    }

    // Every track in source must match a distinct track in target
    let mut claimed_target_indices = HashSet::new();
    for s_track in &source.tracks {
        let mut found = false;
        for (idx, t_track) in target.tracks.iter().enumerate() {
            if !claimed_target_indices.contains(&idx) && track_matches(s_track, t_track) {
                claimed_target_indices.insert(idx);
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }

    true
}

pub fn extract_release_folder(file_path: &str) -> String {
    let path = Path::new(file_path);
    let parent = match path.parent() {
        Some(p) => p,
        None => return file_path.to_string(),
    };

    let parent_name = parent
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // If file is inside a multi-disc subfolder (e.g. "Disc 1", "CD 2"), group under the album directory
    if parent_name.starts_with("disc ")
        || parent_name.starts_with("disc_")
        || parent_name.starts_with("cd ")
        || parent_name.starts_with("cd_")
        || parent_name.starts_with("cd") && parent_name.len() <= 5
    {
        if let Some(grandparent) = parent.parent() {
            return grandparent.display().to_string();
        }
    }

    parent.display().to_string()
}

pub fn parse_local_releases(files: &[LocalFileRecord]) -> Vec<LocalRelease> {
    let mut releases = HashMap::<String, LocalRelease>::new();
    for file in files.iter().filter(|f| f.present) {
        let tags = crate::workflows::extract_tags_map(&file.metadata);
        let text = |key: &str| tags.get(key).cloned().unwrap_or_default();
        let number = |key: &str| {
            text(key)
                .split('/')
                .next()
                .unwrap_or("")
                .trim()
                .parse::<u32>()
                .unwrap_or(0)
        };
        let folder = extract_release_folder(&file.path);
        let artist = tags
            .get("albumartist")
            .or(tags.get("artist"))
            .cloned()
            .unwrap_or_default();
        let release = releases
            .entry(folder.clone())
            .or_insert_with(|| LocalRelease {
                id: format!("{:x}", md5_hash(&folder)),
                folder,
                artist,
                title: text("album"),
                date: text("date"),
                tracks: vec![],
            });
        release.tracks.push(LocalTrack {
            path: file.path.clone(),
            title: text("title"),
            artist: tags
                .get("track_artist")
                .or(tags.get("artist"))
                .cloned()
                .unwrap_or_default(),
            album: text("album"),
            isrc: tags.get("isrc").cloned(),
            duration: file
                .metadata
                .as_ref()
                .and_then(|m| m["duration"].as_f64())
                .unwrap_or(0.),
            track_number: number("tracknumber"),
            disc_number: number("discnumber").max(1),
            size: file.size,
            mtime: file.mtime,
            bpm: tags.get("bpm").cloned(),
            key: tags.get("initialkey").or(tags.get("key")).cloned(),
        });
    }
    let mut releases: Vec<_> = releases.into_values().collect();
    for release in &mut releases {
        release
            .tracks
            .sort_by_key(|t| (t.disc_number, t.track_number, t.title.clone()));
    }
    releases.sort_by(|a, b| a.folder.cmp(&b.folder));
    releases
}

fn md5_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

fn is_better_master(a: &LocalRelease, b: &LocalRelease) -> bool {
    let size_a: i64 = a.tracks.iter().map(|t| t.size).sum();
    let size_b: i64 = b.tracks.iter().map(|t| t.size).sum();
    if size_a != size_b {
        return size_a > size_b;
    }
    // Deterministic tie breaker: prefer the alphabetically earlier folder path
    a.folder < b.folder
}

pub fn find_duplicate_clusters(files: &[LocalFileRecord]) -> Vec<DuplicateCluster> {
    let releases = parse_local_releases(files);
    if releases.len() < 2 {
        return Vec::new();
    }

    // Build containment map: child_idx -> Vec<parent_idx> (releases that contain child)
    let mut parents: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();

    for i in 0..releases.len() {
        for j in 0..releases.len() {
            if i == j {
                continue;
            }
            if is_release_contained(&releases[i], &releases[j]) {
                // If both releases contain each other (exact duplicate releases),
                // break symmetry so the graph is a strict DAG: only the worse release has the better release as parent
                if is_release_contained(&releases[j], &releases[i]) {
                    if is_better_master(&releases[j], &releases[i]) {
                        parents.entry(i).or_default().push(j);
                        children.entry(j).or_default().push(i);
                    }
                } else {
                    parents.entry(i).or_default().push(j);
                    children.entry(j).or_default().push(i);
                }
            }
        }
    }

    if parents.is_empty() {
        return Vec::new();
    }

    // Find maximal master releases (releases that are not contained in any other release)
    // For each release i that has parents: find its ultimate ancestor(s)
    let mut master_to_redundant: HashMap<usize, HashSet<usize>> = HashMap::new();

    for &child_idx in parents.keys() {
        // Find all maximal ancestors of child_idx
        let mut visited = HashSet::new();
        let mut queue = vec![child_idx];
        let mut maximal_ancestors = HashSet::new();

        while let Some(curr) = queue.pop() {
            if !visited.insert(curr) {
                continue;
            }
            if let Some(par_list) = parents.get(&curr) {
                for &p in par_list {
                    if !parents.contains_key(&p) {
                        // p has no parents; it is maximal!
                        maximal_ancestors.insert(p);
                    } else {
                        queue.push(p);
                    }
                }
            }
        }

        // If there are multiple maximal ancestors, pick the one with the most tracks / largest size
        if let Some(&best_master) = maximal_ancestors.iter().max_by_key(|&&m| {
            let size: i64 = releases[m].tracks.iter().map(|t| t.size).sum();
            (releases[m].tracks.len(), releases[m].date.clone(), size)
        }) {
            master_to_redundant
                .entry(best_master)
                .or_default()
                .insert(child_idx);
        }
    }

    let mut clusters = Vec::new();

    for (master_idx, redundant_indices) in master_to_redundant {
        let master = releases[master_idx].clone();
        let mut redundant_list: Vec<LocalRelease> = redundant_indices
            .into_iter()
            .map(|idx| releases[idx].clone())
            .collect();

        // Sort redundant releases by track count ascending (e.g. Single -> EP)
        redundant_list.sort_by_key(|r| r.tracks.len());

        // Check if there is an internal chain among the redundant releases
        // (e.g. R1 is contained in R2, and R2 is contained in Master)
        let mut is_chained = false;
        if redundant_list.len() > 1 {
            for i in 0..redundant_list.len() {
                for j in (i + 1)..redundant_list.len() {
                    if is_release_contained(&redundant_list[i], &redundant_list[j]) {
                        is_chained = true;
                        break;
                    }
                }
                if is_chained {
                    break;
                }
            }
        }

        let total_redundant_tracks: usize = redundant_list.iter().map(|r| r.tracks.len()).sum();
        let total_recoverable_bytes: i64 = redundant_list
            .iter()
            .flat_map(|r| &r.tracks)
            .map(|t| t.size)
            .sum();

        let chain_summary = if is_chained {
            let names: Vec<String> = redundant_list
                .iter()
                .map(|r| format!("{} ({} trk)", r.title, r.tracks.len()))
                .collect();
            format!(
                "Chained duplicate: {} → {} ({} trk)",
                names.join(" → "),
                master.title,
                master.tracks.len()
            )
        } else if redundant_list.len() > 1 {
            format!(
                "Multi-release duplicate: {} releases ({} tracks) absorbed into {} ({} trk)",
                redundant_list.len(),
                total_redundant_tracks,
                master.title,
                master.tracks.len()
            )
        } else if let Some(first) = redundant_list.first() {
            format!(
                "Duplicate release: {} ({} tracks) fully contained in {} ({} trk)",
                first.title,
                first.tracks.len(),
                master.title,
                master.tracks.len()
            )
        } else {
            String::new()
        };

        let cluster_id = format!("cluster:{}", master.id);

        clusters.push(DuplicateCluster {
            cluster_id,
            master,
            redundant: redundant_list,
            is_chained,
            chain_summary,
            total_redundant_tracks,
            total_recoverable_bytes,
        });
    }

    // Sort clusters so chained / multi-release clusters appear first
    clusters.sort_by(|a, b| {
        b.is_chained
            .cmp(&a.is_chained)
            .then_with(|| b.redundant.len().cmp(&a.redundant.len()))
            .then_with(|| b.total_redundant_tracks.cmp(&a.total_redundant_tracks))
    });

    clusters
}

/// Each retained release owns its redundant releases; no catalogue request is needed to expand it.
pub fn clusters_to_group_rows(clusters: &[DuplicateCluster]) -> Vec<serde_json::Value> {
    use serde_json::json;
    clusters.iter().map(|cluster| {
        let retained=&cluster.master;
        let covered=retained.tracks.iter().filter(|track|cluster.redundant.iter().any(|r|r.tracks.iter().any(|t|track_matches(t,track)))).count();
        let children:Vec<_>=cluster.redundant.iter().map(|r|json!({
            "id":format!("{}::{}",cluster.cluster_id,r.folder),"artist":r.artist,"release":r.title,"title":r.title,
            "path":r.folder,"date":r.date,"tracks":r.tracks.len(),"duplicates":r.tracks.len(),"gained":0,
            "target":retained.folder,"status":"Duplicate","evidence":format!("All {} recordings are present in {} with matching mix, duration and performer credits",r.tracks.len(),retained.title),
            "changes":"Move reviewed duplicate files to Trash","affected":true
        })).collect();
        json!({"id":cluster.cluster_id,"artist":retained.artist,"release":retained.title,"title":retained.title,"path":retained.folder,
            "date":retained.date,"tracks":retained.tracks.len(),"duplicates":cluster.total_redundant_tracks,"gained":retained.tracks.len()-covered,
            "target":retained.folder,"status":if cluster.is_chained {"Chained duplicate"}else{"Duplicate"},
            "evidence":format!("Keep this release; review {} redundant releases · {:.1} MB recoverable",children.len(),cluster.total_recoverable_bytes as f64 / 1_000_000.),
            "expanded_available":true,"children":children,"affected":true})
    }).collect()
}

pub fn clusters_to_link_rows(clusters: &[DuplicateCluster]) -> Vec<LinkRow> {
    let mut rows = Vec::new();

    for cluster in clusters {
        let is_multi = cluster.redundant.len() > 1;

        for (idx, red) in cluster.redundant.iter().enumerate() {
            let row_id = format!("{}::{}", cluster.cluster_id, red.folder);

            let release_label = if cluster.is_chained {
                format!(
                    "{} [Chained: {} of {}]",
                    red.title,
                    idx + 1,
                    cluster.redundant.len()
                )
            } else if is_multi {
                format!(
                    "{} [Cluster: {} of {}]",
                    red.title,
                    idx + 1,
                    cluster.redundant.len()
                )
            } else {
                red.title.clone()
            };

            let status_badge = if cluster.is_chained {
                "Chained duplicate".to_string()
            } else if is_multi {
                "Multi-duplicate".to_string()
            } else {
                "Duplicate".to_string()
            };

            let target_label = format!(
                "{} ({} tracks)",
                cluster.master.title,
                cluster.master.tracks.len()
            );

            let gained = if cluster.master.tracks.len() > red.tracks.len() {
                format!("+{} tracks", cluster.master.tracks.len() - red.tracks.len())
            } else {
                "0 tracks".to_string()
            };

            let changes_desc = format!(
                "Move {} duplicate files to Trash (all present in {})",
                red.tracks.len(),
                cluster.master.title
            );

            rows.push(LinkRow {
                id: row_id,
                artist: red.artist.clone(),
                release: release_label,
                title: format!("{} tracks", red.tracks.len()),
                path: red.folder.clone(),
                position: gained,
                status: status_badge,
                evidence: cluster.chain_summary.clone(),
                affected: true,
                target: target_label,
                changes: changes_desc,
                bpm: None,
                key: None,
                candidates: cluster.redundant.len(),
                online_id: cluster.cluster_id.clone(),
                ignored: false,
            });
        }
    }

    rows
}

pub async fn trash_file_or_directory(path_str: &str) -> Result<(), String> {
    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let esc = path_str.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "tell application \"Finder\" to delete POSIX file \"{}\"",
            esc
        );
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                return Ok(());
            }
        }
    }

    // Fallback: move to ~/.Trash if exists
    if let Some(home) = std::env::var_os("HOME") {
        let trash_dir = Path::new(&home).join(".Trash");
        if trash_dir.is_dir() {
            if let Some(file_name) = path.file_name() {
                let mut target = trash_dir.join(file_name);
                if target.exists() {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    target = trash_dir.join(format!("{}_{}", ts, file_name.to_string_lossy()));
                }
                if std::fs::rename(path, &target).is_ok() {
                    return Ok(());
                }
            }
        }
    }

    // If Trash failed, return error to never lose files unexpectedly
    Err(format!("Could not safely move '{}' to Trash.", path_str))
}

pub async fn consolidate_redundant_releases(
    db: &TursoDb,
    files_to_delete: &[String],
    folders_to_clean: &[String],
) -> Result<usize, String> {
    let mut deleted_count = 0;

    for file_path in files_to_delete {
        if Path::new(file_path).exists() {
            trash_file_or_directory(file_path).await?;
            deleted_count += 1;
        }
        db.remove_local_file(file_path).await?;
    }

    // Clean up empty folders
    for folder in folders_to_clean {
        let p = Path::new(folder);
        if p.is_dir() {
            // Check if folder has any remaining audio files
            let is_empty = match std::fs::read_dir(p) {
                Ok(entries) => {
                    let remaining: Vec<_> = entries
                        .filter_map(|e| e.ok())
                        .filter(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            name != ".DS_Store" && name != "Thumbs.db"
                        })
                        .collect();
                    remaining.is_empty()
                }
                Err(_) => false,
            };

            if is_empty {
                let _ = trash_file_or_directory(folder).await;
            }
        }
    }

    db.bump_revision();
    Ok(deleted_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_track(path: &str, title: &str, isrc: &str, dur: f64) -> LocalTrack {
        LocalTrack {
            path: path.to_string(),
            title: title.to_string(),
            artist: "Daft Punk".to_string(),
            album: "Album".to_string(),
            isrc: if isrc.is_empty() {
                None
            } else {
                Some(isrc.to_string())
            },
            duration: dur,
            track_number: 1,
            disc_number: 1,
            size: 1024,
            mtime: 1000,
            bpm: None,
            key: None,
        }
    }

    #[test]
    fn test_chained_duplicate_cluster_detection() {
        // Release A: 2-track single (Tracks 1, 2)
        let release_a = LocalRelease {
            id: "rel_a".to_string(),
            folder: "/music/Daft Punk/Get Lucky (Single)".to_string(),
            artist: "Daft Punk".to_string(),
            title: "Get Lucky (Single)".to_string(),
            date: "2013".to_string(),
            tracks: vec![
                make_test_track(
                    "/music/Daft Punk/Get Lucky (Single)/01 Get Lucky.flac",
                    "Get Lucky",
                    "USXX1",
                    248.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Get Lucky (Single)/02 Get Lucky (Radio).flac",
                    "Get Lucky (Radio Edit)",
                    "USXX2",
                    180.0,
                ),
            ],
        };

        // Release B: 4-track EP (Tracks 1, 2, 3, 4 - contains Release A + 2 remixes)
        let release_b = LocalRelease {
            id: "rel_b".to_string(),
            folder: "/music/Daft Punk/Get Lucky (EP)".to_string(),
            artist: "Daft Punk".to_string(),
            title: "Get Lucky (EP)".to_string(),
            date: "2013".to_string(),
            tracks: vec![
                make_test_track(
                    "/music/Daft Punk/Get Lucky (EP)/01 Get Lucky.flac",
                    "Get Lucky",
                    "USXX1",
                    248.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Get Lucky (EP)/02 Get Lucky (Radio).flac",
                    "Get Lucky (Radio Edit)",
                    "USXX2",
                    180.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Get Lucky (EP)/03 Remix 1.flac",
                    "Get Lucky (Remix 1)",
                    "USXX3",
                    300.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Get Lucky (EP)/04 Remix 2.flac",
                    "Get Lucky (Remix 2)",
                    "USXX4",
                    320.0,
                ),
            ],
        };

        // Release C: 12-track Deluxe Album (contains all 4 tracks of Release B + 8 other album tracks!)
        let release_c = LocalRelease {
            id: "rel_c".to_string(),
            folder: "/music/Daft Punk/Random Access Memories (Deluxe)".to_string(),
            artist: "Daft Punk".to_string(),
            title: "Random Access Memories (Deluxe)".to_string(),
            date: "2013".to_string(),
            tracks: vec![
                make_test_track(
                    "/music/Daft Punk/Random Access Memories (Deluxe)/01 Give Life.flac",
                    "Give Life Back to Music",
                    "USXX5",
                    274.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Random Access Memories (Deluxe)/02 Get Lucky.flac",
                    "Get Lucky",
                    "USXX1",
                    248.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Random Access Memories (Deluxe)/03 Get Lucky (Radio).flac",
                    "Get Lucky (Radio Edit)",
                    "USXX2",
                    180.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Random Access Memories (Deluxe)/04 Remix 1.flac",
                    "Get Lucky (Remix 1)",
                    "USXX3",
                    300.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Random Access Memories (Deluxe)/05 Remix 2.flac",
                    "Get Lucky (Remix 2)",
                    "USXX4",
                    320.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Random Access Memories (Deluxe)/06 Lose Yourself.flac",
                    "Lose Yourself to Dance",
                    "USXX6",
                    353.0,
                ),
            ],
        };

        assert!(
            is_release_contained(&release_a, &release_b),
            "Release A must be contained in Release B"
        );
        assert!(
            is_release_contained(&release_b, &release_c),
            "Release B must be contained in Release C"
        );
        assert!(
            is_release_contained(&release_a, &release_c),
            "Release A must be contained in Release C (transitive)"
        );

        // Convert to LocalFileRecord list and find clusters
        let mut records = Vec::new();
        for rel in &[&release_a, &release_b, &release_c] {
            for t in &rel.tracks {
                records.push(LocalFileRecord {
                    path: t.path.clone(),
                    root: "/music".to_string(),
                    size: t.size,
                    mtime: t.mtime,
                    metadata: Some(serde_json::json!({
                        "title": t.title,
                        "artist": t.artist,
                        "album": rel.title,
                        "isrc": t.isrc,
                        "duration": t.duration,
                    })),
                    error: None,
                    present: true,
                });
            }
        }

        let clusters = find_duplicate_clusters(&records);
        assert_eq!(clusters.len(), 1, "Must find exactly 1 duplicate cluster");

        let cluster = &clusters[0];
        assert_eq!(cluster.master.title, "Random Access Memories (Deluxe)");
        assert_eq!(
            cluster.redundant.len(),
            2,
            "Must absorb both Release A and Release B"
        );
        assert!(
            cluster.is_chained,
            "Cluster must be marked as chained duplicate"
        );
        assert_eq!(
            cluster.total_redundant_tracks, 6,
            "Total redundant tracks must be 2 + 4 = 6"
        );

        let groups = clusters_to_group_rows(&clusters);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["release"], "Random Access Memories (Deluxe)");
        assert_eq!(groups[0]["children"].as_array().unwrap().len(), 2);
        assert_eq!(groups[0]["duplicates"], 6);
        assert_eq!(groups[0]["gained"], 2);
        let rows = clusters_to_link_rows(&clusters);
        assert_eq!(
            rows.len(),
            2,
            "Must produce 2 rows for the 2 redundant releases"
        );
        assert_eq!(rows[0].status, "Chained duplicate");
        assert!(rows[0].evidence.contains("Chained duplicate"));
        assert_eq!(rows[0].target, "Random Access Memories (Deluxe) (6 tracks)");
    }

    #[test]
    fn test_exact_duplicate_detection() {
        let release_orig = LocalRelease {
            id: "rel_1".to_string(),
            folder: "/music/Daft Punk/Discovery".to_string(),
            artist: "Daft Punk".to_string(),
            title: "Discovery".to_string(),
            date: "2001".to_string(),
            tracks: vec![
                make_test_track(
                    "/music/Daft Punk/Discovery/01 One More Time.flac",
                    "One More Time",
                    "USXX1",
                    320.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Discovery/02 Aerodynamic.flac",
                    "Aerodynamic",
                    "USXX2",
                    210.0,
                ),
            ],
        };
        let release_copy = LocalRelease {
            id: "rel_2".to_string(),
            folder: "/music/Daft Punk/Discovery (Copy)".to_string(),
            artist: "Daft Punk".to_string(),
            title: "Discovery".to_string(),
            date: "2001".to_string(),
            tracks: vec![
                make_test_track(
                    "/music/Daft Punk/Discovery (Copy)/01 One More Time.flac",
                    "One More Time",
                    "USXX1",
                    320.0,
                ),
                make_test_track(
                    "/music/Daft Punk/Discovery (Copy)/02 Aerodynamic.flac",
                    "Aerodynamic",
                    "USXX2",
                    210.0,
                ),
            ],
        };

        let mut records = Vec::new();
        for rel in &[&release_orig, &release_copy] {
            for t in &rel.tracks {
                records.push(LocalFileRecord {
                    path: t.path.clone(),
                    root: "/music".to_string(),
                    size: t.size,
                    mtime: t.mtime,
                    metadata: Some(serde_json::json!({
                        "title": t.title,
                        "artist": t.artist,
                        "album": rel.title,
                        "isrc": t.isrc,
                        "duration": t.duration,
                    })),
                    error: None,
                    present: true,
                });
            }
        }

        let clusters = find_duplicate_clusters(&records);
        assert_eq!(
            clusters.len(),
            1,
            "Must find 1 duplicate cluster for exact duplicate releases"
        );
        assert_eq!(clusters[0].redundant.len(), 1);
        assert_eq!(
            clusters[0].redundant[0].folder,
            "/music/Daft Punk/Discovery (Copy)"
        );
        assert_eq!(clusters[0].master.folder, "/music/Daft Punk/Discovery");
    }
}
