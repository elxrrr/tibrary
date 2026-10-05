use crate::db::{LinkRow, LocalFileRecord, TursoDb};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static AUDIO_QUALITY_CACHE: OnceLock<Mutex<HashMap<(String, i64, i64), Option<(u32, u32)>>>> = OnceLock::new();

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
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
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

pub fn manifest_fingerprint(files: &[LocalFileRecord]) -> String {
    let mut rows: Vec<_> = files.iter().filter(|file| file.present).collect();
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hash = Sha256::new();
    for file in rows {
        hash.update(file.path.as_bytes());
        hash.update([0]);
        hash.update(file.size.to_le_bytes());
        hash.update(file.mtime.to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub async fn indexed_manifest_fingerprint(db: &TursoDb, root: &str) -> Result<String, String> {
    let conn = db.connect()?;
    let mut rows = conn
        .query(
            "SELECT path, size, mtime FROM local_files WHERE root=? AND present=1 ORDER BY path",
            (root,),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let path: String = row.get(0).map_err(|e| e.to_string())?;
        let size: i64 = row.get(1).unwrap_or_default();
        let mtime: i64 = row.get(2).unwrap_or_default();
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(size.to_le_bytes());
        hash.update(mtime.to_le_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
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

// FLAC STREAMINFO is the first metadata block. Reading its fixed 42-byte header
// avoids decoding audio or rescanning tags for an already indexed library.
fn flac_quality(path: &str) -> Option<(u32, u32)> {
    if !Path::new(path).extension()?.eq_ignore_ascii_case("flac") {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 42];
    file.read_exact(&mut header).ok()?;
    if &header[..4] != b"fLaC" || header[4] & 0x7f != 0 || header[5..8] != [0, 0, 34] {
        return None;
    }
    let packed = u64::from_be_bytes(header[18..26].try_into().ok()?);
    let sample_rate = ((packed >> 44) & 0xfffff) as u32;
    let bit_depth = (((packed >> 36) & 0x1f) + 1) as u32;
    (sample_rate > 0 && bit_depth >= 8).then_some((sample_rate, bit_depth))
}

fn quality(track: &LocalTrack) -> Option<(u32, u32)> {
    if let Some(known) = track.sample_rate.zip(track.bit_depth) { return Some(known); }
    let cache = AUDIO_QUALITY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (track.path.clone(), track.mtime, track.size);
    if let Ok(guard) = cache.lock() {
        if let Some(saved) = guard.get(&key) { return *saved; }
    }
    let inspected = flac_quality(&track.path);
    if let Ok(mut guard) = cache.lock() {
        if guard.len() > 100_000 { guard.clear(); }
        guard.insert(key, inspected);
    }
    inspected
}

fn fidelity_at_least(source: &LocalTrack, target: &LocalTrack) -> bool {
    let (Some((source_rate, source_depth)), Some((target_rate, target_depth))) =
        (quality(source), quality(target))
    else {
        return false;
    };
    target_rate >= source_rate && target_depth >= source_depth
}

fn assign_source(
    source_idx: usize,
    source: &LocalRelease,
    target: &LocalRelease,
    owners: &mut [Option<usize>],
    seen: &mut [bool],
) -> bool {
    for (idx, candidate) in target.tracks.iter().enumerate() {
        if seen[idx]
            || !track_matches(&source.tracks[source_idx], candidate)
            || !fidelity_at_least(&source.tracks[source_idx], candidate)
        {
            continue;
        }
        seen[idx] = true;
        if owners[idx].is_none()
            || assign_source(owners[idx].unwrap(), source, target, owners, seen)
        {
            owners[idx] = Some(source_idx);
            return true;
        }
    }
    false
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

    // An augmenting-path assignment handles repeated titles without letting two
    // source recordings claim the same target track.
    let mut owners = vec![None; target.tracks.len()];
    (0..source.tracks.len()).all(|idx| {
        assign_source(
            idx,
            source,
            target,
            &mut owners,
            &mut vec![false; target.tracks.len()],
        )
    })
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
            sample_rate: file
                .metadata
                .as_ref()
                .and_then(|m| m["sample_rate"].as_u64())
                .map(|n| n as u32),
            bit_depth: file
                .metadata
                .as_ref()
                .and_then(|m| m["bit_depth"].as_u64())
                .map(|n| n as u32),
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
    let qa = a.tracks.iter().filter_map(quality).min();
    let qb = b.tracks.iter().filter_map(quality).min();
    if qa != qb {
        return qa > qb;
    }
    // Deterministic tie breaker: prefer the alphabetically earlier folder path
    a.folder < b.folder
}

pub fn find_duplicate_clusters(files: &[LocalFileRecord]) -> Vec<DuplicateCluster> {
    find_duplicate_clusters_with_progress(files,&std::sync::atomic::AtomicBool::new(false),|_,_|{})
}

pub fn find_duplicate_clusters_with_progress(files: &[LocalFileRecord], cancel: &std::sync::atomic::AtomicBool, progress: impl Fn(usize,usize)) -> Vec<DuplicateCluster> {
    let releases = parse_local_releases(files);
    if releases.len() < 2 {
        return Vec::new();
    }

    // Build containment map: child_idx -> Vec<parent_idx> (releases that contain child)
    let mut parents: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut artist_groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, release) in releases.iter().enumerate() {
        artist_groups.entry(normalize_text(&release.artist)).or_default().push(index);
    }
    let groups: Vec<_> = artist_groups.into_iter().collect();
    for (position,(left_key, left)) in groups.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {return Vec::new();}
        progress(position,groups.len());
        for (right_key, right) in &groups {
            if !left_key.is_empty() && !right_key.is_empty()
                && left_key != right_key && !left_key.contains(right_key) && !right_key.contains(left_key) { continue; }
            for &i in left {
                for &j in right {
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {return Vec::new();}
                    if i == j || !is_release_contained(&releases[i], &releases[j]) { continue; }
                    // A complete clone has two possible parents. Keep only the
                    // higher-quality deterministic winner so the graph is acyclic.
                    if !is_release_contained(&releases[j], &releases[i])
                        || is_better_master(&releases[j], &releases[i]) {
                        parents.entry(i).or_default().push(j);
                    }
                }
            }
        }
    }

    progress(groups.len(),groups.len());
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

pub fn online_replacement_group_id(release_id: &str) -> String {
    format!("online-replacement:{release_id}")
}

/// A replacement release owns the local releases it can absorb. Cached plans
/// keep their original IDs so selections never become filesystem operations.
pub fn online_replacement_groups(
    plans: &[serde_json::Value],
    metadata: &HashMap<String, serde_json::Value>,
    local_dates: &HashMap<String, String>,
) -> Vec<serde_json::Value> {
    use serde_json::{json, Value};
    let mut groups = std::collections::BTreeMap::<String, Vec<Value>>::new();
    for plan in plans {
        let target = plan["online_id"].as_str().filter(|id|!id.is_empty()).map(str::to_owned)
            .unwrap_or_else(||format!("unverified:{:x}",Sha256::digest(plan["id"].to_string().as_bytes())));
        groups.entry(target).or_default().push(plan.clone());
    }
    groups.into_iter().map(|(target_id, mut children)| {
        children.sort_by(|left,right| crate::compare_table_cell(left,right,"release")
            .then_with(||crate::compare_table_cell(left,right,"path"))
            .then_with(||crate::compare_table_cell(left,right,"id")));
        let first = children[0].clone();
        let snapshot = metadata.get(&target_id)
            .or_else(||children.iter().find_map(|plan|plan.get("target_metadata").filter(|snapshot|snapshot.is_object())))
            .cloned().unwrap_or(Value::Null);
        let text = |field: &str, fallback: &str, last: &str| snapshot[field].as_str().filter(|text|!text.is_empty())
            .or_else(||first[fallback].as_str().filter(|text|!text.is_empty()))
            .unwrap_or(last).to_string();
        let title = text("title", "target", "Unknown replacement");
        let artist = text("artist", "target_artist", first["artist"].as_str().unwrap_or("Unknown artist"));
        let date = text("date", "target_date", "");
        let tracks = snapshot["tracks"].as_array().filter(|tracks|!tracks.is_empty()).map(Vec::len)
            .or_else(||children.iter().find_map(|plan|plan["target_tracks"].as_u64().filter(|count|*count > 0).map(|count|count as usize)))
            .or_else(||snapshot["track_count"].as_u64().filter(|count|*count > 0).map(|count|count as usize))
            .or_else(||children.iter().filter_map(|plan|Some(plan["tracks"].as_u64()? + plan["gained"].as_u64()?)).max().map(|count|count as usize));
        let id = online_replacement_group_id(&target_id);
        let mut matched = HashSet::<String>::new();
        let exact_coverage = children.iter().all(|plan|plan["matched_track_ids"].as_array().is_some_and(|ids| {
            let mut unique = HashSet::new();
            !ids.is_empty() && Some(ids.len() as u64) == plan["tracks"].as_u64()
                && ids.iter().all(|id|id.as_str().filter(|id|!id.is_empty()).is_some_and(|id| {
                    matched.insert(id.to_string()); unique.insert(id.to_string())
                }))
        }));
        let gained = if exact_coverage { tracks.map(|count|json!(count.saturating_sub(matched.len()))) }
            else if children.len() == 1 { tracks.zip(first["tracks"].as_u64())
                .map(|(total, covered)|json!(total.saturating_sub(covered as usize))) } else { None };
        let duplicates = children.iter().map(|plan|plan["duplicates"].as_u64().or_else(||plan["tracks"].as_u64()).unwrap_or(0)).sum::<u64>();
        for child in &mut children {
            child["parent_id"] = json!(id);
            child["target"] = json!(title);
            child["replacement_group"] = json!(false);
            if child["date"].as_str().is_none_or(|date|date.is_empty()) {
                child["date"] = json!(child["path"].as_str().and_then(|folder|local_dates.get(folder)).cloned().unwrap_or_default());
            }
        }
        let actionable = !target_id.is_empty() && target_id.bytes().all(|byte|byte.is_ascii_digit());
        let sources = children.len();
        let target_metadata = json!({"id":target_id,"artist":artist,"title":title,"date":date,
            "track_count":tracks,"type":snapshot["type"],"quality":snapshot["quality"],
            "label":snapshot["label"],"copyright":snapshot["copyright"],"upc":snapshot["upc"]});
        json!({"id":id,"artist":artist,"release":title,"title":title,"date":date,"tracks":tracks,
            "duplicates":duplicates,"gained":gained.unwrap_or(Value::Null),"target":title,"online_id":if actionable{Some(target_id)}else{None},
            "status":if actionable{"Larger online release"}else{"Needs review"},
            "evidence":if actionable {format!("{sources} local {} fully contained · {duplicates} duplicate files; originals retained until downloaded and reviewed",if sources==1{"release"}else{"releases"})}
                else {"Replacement identity is unverified; inspect the saved plan before queuing. Local files are retained.".to_string()},
            "changes":"Queue complete release; retain originals until downloaded and reviewed",
            "optimization_group":true,"replacement_group":true,"expanded_available":true,
            "plan_ids":children.iter().map(|plan|plan["id"].clone()).collect::<Vec<_>>(),
            "local_releases":sources,"target_metadata":target_metadata,"children":children,"affected":true})
    }).collect()
}

/// Expand selected aggregate IDs against the current saved plans. Unknown IDs
/// cannot queue a release, and each remote release is queued at most once.
pub fn selected_online_replacement_releases(plans: &[serde_json::Value], selected: &HashSet<String>) -> HashSet<String> {
    plans.iter().filter_map(|plan| {
        let release = plan["online_id"].as_str().filter(|id|!id.is_empty() && id.bytes().all(|byte|byte.is_ascii_digit()))?;
        let chosen = plan["id"].as_str().is_some_and(|id|selected.contains(id))
            || selected.contains(&online_replacement_group_id(release));
        chosen.then(||release.to_string())
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
        let output = tokio::time::timeout(std::time::Duration::from_secs(30),
            tokio::process::Command::new("osascript").kill_on_drop(true)
                .arg("-e").arg(&script).output()).await;

        if let Ok(Ok(out)) = output {
            if out.status.success() {
                return Ok(());
            }
        }
    }

    // Finder may have completed just before its response timed out.
    if !path.exists() { return Ok(()); }

    // Fallback: move to ~/.Trash if exists
    if let Some(home) = std::env::var_os("HOME") {
        let trash_dir = Path::new(&home).join(".Trash");
        if trash_dir.is_dir() {
            if let Some(file_name) = path.file_name() {
                // Concurrent replacements can have identical filenames. Never replace
                // an existing Trash entry with a timestamp collision.
                let target = trash_dir.join(format!("{}_{}", uuid::Uuid::new_v4(), file_name.to_string_lossy()));
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

    #[test]
    fn online_replacements_group_sources_with_exact_unique_recording_gains() {
        use serde_json::json;
        let plans = vec![
            json!({"id":"123::/Music/Single A","online_id":"123","artist":"Artist","release":"Single A",
                "path":"/Music/Single A","tracks":2,"duplicates":2,"gained":2,"target":"Deluxe",
                "matched_track_ids":["track-1","track-2"],"evidence":"Exact recordings"}),
            json!({"id":"123::/Music/Single B","online_id":"123","artist":"Artist","release":"Single B",
                "path":"/Music/Single B","tracks":1,"duplicates":1,"gained":3,"target":"Deluxe",
                "matched_track_ids":["track-1"],"target_tracks":4,"evidence":"Exact recording"}),
            json!({"id":"456::/Music/Other","online_id":"456","artist":"Artist","release":"Other",
                "path":"/Music/Other","tracks":1,"gained":2,"target":"Other Album"}),
        ];
        let metadata = HashMap::from([("123".into(),json!({"id":"123","artist":"Canonical Artist","title":"Deluxe","date":"2024-03-01",
            "track_count":5,"label":"Label","copyright":"Rights","quality":"LOSSLESS"}))]);
        let dates = HashMap::from([("/Music/Single A".into(),"2020".into()),("/Music/Single B".into(),"2021".into())]);
        let grouped = online_replacement_groups(&plans, &metadata, &dates);
        assert_eq!(grouped.len(), 2);
        let parent = &grouped[0];
        assert_eq!(parent["id"], "online-replacement:123");
        assert_eq!(parent["replacement_group"], true);
        assert_eq!(parent["artist"], "Canonical Artist");
        assert_eq!(parent["release"], "Deluxe");
        assert_eq!(parent["date"], "2024-03-01");
        assert_eq!(parent["tracks"], 4, "proven audio count takes precedence over a published count with video");
        assert_eq!(parent["duplicates"], 3);
        assert_eq!(parent["gained"], 2, "overlapping local recordings only count once toward the target");
        assert_eq!(parent["target_metadata"]["copyright"], "Rights");
        assert_eq!(parent["plan_ids"],json!([plans[0]["id"],plans[1]["id"]]));
        assert!(parent.get("path").is_none(), "online aggregate must never masquerade as a local folder");
        assert_eq!(parent["children"][0]["id"], plans[0]["id"]);
        assert_eq!(parent["children"][0]["date"], "2020");
        assert_eq!(parent["children"][0]["path"], plans[0]["path"]);
        assert!(parent["children"][0].get("children").is_none());
    }

    #[test]
    fn legacy_replacement_groups_keep_context_without_fabricating_overlapping_gains() {
        use serde_json::json;
        let plans = vec![
            json!({"id":"123::/A","online_id":"123","artist":"Artist","release":"A","path":"/A","tracks":2,"gained":3,"target":"Album"}),
            json!({"id":"123::/B","online_id":"123","artist":"Artist","release":"B","path":"/B","tracks":3,"gained":2,"target":"Album"}),
        ];
        let grouped = online_replacement_groups(&plans,&HashMap::new(),&HashMap::new());
        assert_eq!(grouped[0]["tracks"],5);
        assert_eq!(grouped[0]["duplicates"],5);
        assert!(grouped[0]["gained"].is_null(), "the union of legacy recordings is not proven");
        assert_eq!(grouped[0]["children"].as_array().unwrap().len(),2);
        assert_eq!(grouped[0]["children"][1]["id"], plans[1]["id"]);
        assert_eq!(grouped[0]["children"][1]["gained"],2);
        let invalid = online_replacement_groups(&[json!({"id":"invalid","online_id":"bad","release":"A","path":"/A","tracks":1})],&HashMap::new(),&HashMap::new());
        assert_eq!(invalid[0]["status"],"Needs review");
        assert!(invalid[0]["evidence"].as_str().unwrap().contains("unverified"));
    }

    #[test]
    fn replacement_selection_expands_only_current_group_or_original_plan_ids() {
        use serde_json::json;
        let plans = vec![
            json!({"id":"123::/A","online_id":"123"}),json!({"id":"123::/B","online_id":"123"}),
            json!({"id":"456::/C","online_id":"456"}),json!({"id":"invalid::/D","online_id":"bad"})
        ];
        let group = HashSet::from(["online-replacement:123".to_string()]);
        assert_eq!(selected_online_replacement_releases(&plans,&group), HashSet::from(["123".to_string()]));
        let child = HashSet::from(["123::/B".to_string()]);
        assert_eq!(selected_online_replacement_releases(&plans,&child), HashSet::from(["123".to_string()]));
        let mixed = HashSet::from(["online-replacement:123".to_string(),"456::/C".to_string(),"123::/A".to_string()]);
        assert_eq!(selected_online_replacement_releases(&plans,&mixed), HashSet::from(["123".to_string(),"456".to_string()]));
        let stale = HashSet::from(["online-replacement:999".to_string(),"invalid::/D".to_string()]);
        assert!(selected_online_replacement_releases(&plans,&stale).is_empty());
    }

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
            sample_rate: Some(44_100),
            bit_depth: Some(16),
        }
    }

    #[test]
    fn lower_quality_or_exclusive_mix_cannot_be_absorbed() {
        let source = LocalRelease {
            id: "single".into(),
            folder: "/music/single".into(),
            artist: "Daft Punk".into(),
            title: "Single".into(),
            date: "2020".into(),
            tracks: vec![make_test_track("source", "Song", "ISRC1", 200.0)],
        };
        let mut target = LocalRelease {
            id: "album".into(),
            folder: "/music/album".into(),
            artist: "Daft Punk".into(),
            title: "Album".into(),
            date: "2021".into(),
            tracks: vec![
                make_test_track("target", "Song", "ISRC1", 200.0),
                make_test_track("bonus", "Bonus", "ISRC2", 180.0),
            ],
        };
        assert!(is_release_contained(&source, &target));
        target.tracks[0].bit_depth = Some(16);
        let mut higher = source.clone();
        higher.tracks[0].bit_depth = Some(24);
        assert!(!is_release_contained(&higher, &target));
        let mut exclusive = source.clone();
        exclusive.tracks.push(make_test_track(
            "mix",
            "Song (Extended Mix)",
            "ISRC3",
            270.0,
        ));
        assert!(!is_release_contained(&exclusive, &target));
    }

    #[test]
    fn reads_flac_streaminfo_without_audio_decode() {
        let path = std::env::temp_dir().join(format!(
            "tibrary-streaminfo-{}-{}.flac",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut bytes = [0u8; 42];
        bytes[..4].copy_from_slice(b"fLaC");
        bytes[4] = 0x80; // final STREAMINFO block
        bytes[7] = 34;
        let packed = (96_000u64 << 44) | (1u64 << 41) | (23u64 << 36);
        bytes[18..26].copy_from_slice(&packed.to_be_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(flac_quality(path.to_str().unwrap()), Some((96_000, 24)));
        std::fs::remove_file(path).unwrap();
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
                        "sample_rate": t.sample_rate,
                        "bit_depth": t.bit_depth,
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
                        "sample_rate": t.sample_rate,
                        "bit_depth": t.bit_depth,
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
