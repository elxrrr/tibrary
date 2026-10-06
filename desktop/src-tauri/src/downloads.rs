use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::db::TursoDb;
use crate::stream_download::{
    self, apply_audio_tags, download_stream, fetch_album_tracks, fetch_cover_art,
    fetch_lyrics, format_download_path, get_playback_info, get_valid_token, publish_staged_files,
    TidalAlbumTrack, TrackDownloadMeta,
};

struct StagingDirectory(PathBuf);
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// JoinHandle drops detach tasks. Keep independent abort handles until the whole
// release finishes so every error/cancellation path stops pending transfers too.
#[derive(Default)]
struct DownloadTasks(Vec<tokio::task::AbortHandle>);
impl Drop for DownloadTasks {
    fn drop(&mut self) {
        for task in &self.0 { task.abort(); }
    }
}

fn preliminary_meta(track: &TidalAlbumTrack, album_id: &str, album: &str, album_artist: &str, track_total: u32, disc_total: u32, date: &Option<String>) -> TrackDownloadMeta {
    TrackDownloadMeta {
        track_id: track.id.clone(), album_id: album_id.to_string(), title: track.title.clone(),
        album: album.to_string(), album_artist: album_artist.to_string(),
        track_artists: if track.artists.is_empty() { vec![album_artist.to_string()] } else { track.artists.clone() },
        track_number: track.track_number, track_total, disc_number: track.volume_number, disc_total,
        date: date.clone(), isrc: track.isrc.clone(), copyright: track.copyright.clone(),
        bpm: track.bpm, musical_key: track.key.clone(), release_type: None, explicit: track.explicit,
        lyrics: None, unsynced_lyrics: None, album_replay_gain: None, album_peak_amplitude: None,
        track_replay_gain: None, track_peak_amplitude: None,
        credit_tags: crate::enrichment::credit_tags(&track.credits),
    }
}

fn existing_audio_path(output: &Path, destination: Option<&Value>, template: &str, meta: &TrackDownloadMeta) -> PathBuf {
    let relative = if let Some(dest) = destination {
        let mut path = PathBuf::from(dest.get("album_relative").and_then(Value::as_str).unwrap_or(""));
        if meta.disc_total > 1 { path.push(format!("Disc {}", meta.disc_number)); }
        path.push(format_download_path("{tracknumber} - {title}", meta, ".flac"));
        path
    } else { format_download_path(template, meta, ".flac") };
    output.join(relative)
}

fn nonempty_file(path: &Path) -> bool {
    path.is_file() && path.metadata().map(|metadata| metadata.len() > 0).unwrap_or(false)
}

fn preserve_existing_dj_tags(replacement: &Path, previous: &[PathBuf], explicit_replacement: bool) -> Result<(), String> {
    let fresh = crate::scanner::read_audio_metadata(replacement)?;
    let valid_bpm = |value: Option<&str>| value.and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0);
    let mut updates = std::collections::HashMap::new();
    let mut needs_bpm = valid_bpm(fresh.bpm.as_deref()).is_none();
    let mut needs_key = fresh.musical_key.as_deref().and_then(crate::musical_keys::camelot_key).is_none();
    if !needs_bpm && !needs_key && !explicit_replacement { return Ok(()); }
    for path in previous {
        let old = match crate::scanner::read_audio_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if explicit_replacement => return Err(format!("Could not verify replacement source {}: {error}", path.display())),
            Err(_) => continue,
        };
        let old_isrc = crate::release_matching::clean_isrc(old.isrc.as_deref());
        let new_isrc = crate::release_matching::clean_isrc(fresh.isrc.as_deref());
        let same_id = old.tidal_track_id.as_deref().filter(|id| !id.is_empty())
            .is_some_and(|id| fresh.tidal_track_id.as_deref() == Some(id));
        let same_isrc = old_isrc.as_ref().filter(|isrc| isrc.len() == 12)
            .is_some_and(|isrc| new_isrc.as_ref() == Some(isrc));
        let same_recording = (same_id || same_isrc || explicit_replacement) && crate::release_matching::recording_matches(
            &old.title, old.duration, old.isrc.as_deref(), &fresh.title, fresh.duration,
            fresh.isrc.as_deref(), true);
        if !same_recording {
            if explicit_replacement { return Err(format!("Replacement recording differs from {}; review its release link", path.display())); }
            continue;
        }
        if needs_bpm {
            if let Some(bpm) = valid_bpm(old.bpm.as_deref()) {
                updates.insert("bpm".into(), bpm.to_string());
                needs_bpm = false;
            }
        }
        if needs_key {
            if let Some(key) = old.musical_key.as_deref().and_then(crate::musical_keys::camelot_key) {
                updates.insert("initialkey".into(), key);
                needs_key = false;
            }
        }
        if !needs_bpm && !needs_key && !explicit_replacement { break; }
    }
    crate::tag_writer::write_tags(replacement, &updates)
}

fn replacement_sources(release: &Value, track: &str, root: &Path) -> Result<Vec<PathBuf>, String> {
    if release["replacement_audit"]["root"].as_str().is_none() { return Ok(Vec::new()); }
    let saved = &release["replacement_audit"]["source_paths"][track];
    let mut paths: Vec<_> = if let Some(path) = saved.as_str() { vec![PathBuf::from(path)] }
        else { saved.as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).collect() };
    paths.sort(); paths.dedup();
    if paths.is_empty() { return Err("Replacement selection no longer contains this track's source files; review the queue".into()); }
    let canonical_root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    for path in &paths {
        if !path.starts_with(root) || path.components().any(|part| matches!(part, std::path::Component::ParentDir))
            || !fs::symlink_metadata(path).map_err(|e|format!("Replacement source {}: {e}",path.display()))?.file_type().is_file()
            || !fs::canonicalize(path).map_err(|e| e.to_string())?.starts_with(&canonical_root) {
            return Err("Replacement source must be an existing file within the selected library".into());
        }
    }
    Ok(paths)
}

pub struct DownloadManager;

impl DownloadManager {
    pub async fn run_downloads(
        db: &TursoDb,
        app: Option<&AppHandle>,
        cancel_flag: Arc<AtomicBool>,
        _job_id: &str,
        progress_cb: impl Fn(String) + Send + Sync + 'static,
        monitor_cb: impl Fn(Value) + Send + Sync + 'static,
    ) -> Result<usize, String> {
        let progress_cb = Arc::new(progress_cb);
        let monitor_cb = Arc::new(monitor_cb);
        let conn = db.connect()?;
        let mut queue_stmt = conn
            .query(
                "SELECT id, payload FROM queue WHERE approved = 1 AND decision = 'queued' ORDER BY id",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        while let Some(row) = queue_stmt.next().await.map_err(|e| e.to_string())? {
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let payload_str: String = row.get(1).map_err(|e| e.to_string())?;
            if let Ok(release) = serde_json::from_str::<Value>(&payload_str) {
                items.push((id, release));
            }
        }

        if items.is_empty() {
            progress_cb("No approved items to download".to_string());
            return Ok(0);
        }

        let home_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));

        // Get download settings
        let settings = db.get_preference("downloads").await?.unwrap_or_else(|| {
            json!({
                "output": home_dir.join("Downloads/Music").to_string_lossy().to_string(),
                "quality": "LOSSLESS",
                "cover_size": 1280
            })
        });

        let output_str = settings
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("~/Downloads/Music");
        let output_path = if output_str.starts_with('~') {
            home_dir.join(output_str.trim_start_matches("~/"))
        } else {
            PathBuf::from(output_str)
        };

        let default_quality = settings
            .get("quality")
            .and_then(|v| v.as_str())
            .unwrap_or("LOSSLESS");

        let cover_size_u32 = settings
            .get("cover_size")
            .and_then(|v| {
                if let Some(n) = v.as_u64() {
                    Some(n as u32)
                } else if let Some(s) = v.as_str() {
                    s.parse::<u32>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(1280);

        let skip_existing = settings
            .get("skip_existing")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let lyrics_embed = settings
            .get("lyrics_embed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let lyrics_file = settings
            .get("lyrics_file")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let cover_album_file = settings
            .get("cover_album_file")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let playlist_create = settings
            .get("playlist_create")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let replay_gain = settings
            .get("replay_gain")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let provider = db
            .get_preference("provider")
            .await?
            .unwrap_or_else(|| json!({}));
        let download_delay = provider
            .get("download_delay")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let delay_min = provider
            .get("download_delay_min_sec")
            .and_then(|v| v.as_f64())
            .unwrap_or(3.0);
        let delay_max = provider
            .get("download_delay_max_sec")
            .and_then(|v| v.as_f64())
            .unwrap_or(5.0);

        let layout = db.get_preference("organisation").await?.unwrap_or_else(|| {
            json!({
                "template": "{albumartist}/{album} ({year})/{disc}/{tracknumber} - {title}"
            })
        });
        let template = layout
            .get("template")
            .and_then(|v| v.as_str())
            .unwrap_or("{albumartist}/{album} ({year})/{disc}/{tracknumber} - {title}");

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .unwrap_or_default();

        let _token = match get_valid_token(db, &http).await {
            Ok(tok) => tok,
            Err(_) => {
                if let Some(app) = app {
                    let flow = stream_download::create_pkce_flow();
                    let login_url = flow.login_url.clone();
                    if let Some(backend) = app.try_state::<Arc<crate::Backend>>() {
                        *backend.pending_pkce.lock().unwrap() = Some(flow);
                    }
                    let _ = app.emit(
                        "backend-event",
                        json!({
                            "event": "authentication",
                            "url": login_url
                        }),
                    );
                }
                progress_cb(
                    "Authentication required. Please complete sign-in in your browser.".to_string(),
                );
                return Err(
                    "Tidal sign-in required. Follow the link to log into Tidal.".to_string()
                );
            }
        };

        progress_cb("Download account connected · preparing selected releases".to_string());

        let configuration = db.get_settings().await?;
        let market = configuration["general"]["market"].as_str().unwrap_or("GB");
        let roots = db.list_roots(market).await?;
        let mut completed_count = 0;
        let mut failed_count = 0;
        let mut processed_tracks=0usize;
        let mut planned_tracks:Vec<usize>=items.iter().map(|(_,release)|release["selected_tracks"].as_array().map(Vec::len)
            .or_else(||release["track_count"].as_u64().map(|n|n as usize)).unwrap_or(0)).collect();

        for (release_index, (ident, release)) in items.iter().enumerate() {
            progress_cb(format!("Downloading releases · {}/{} releases",release_index,items.len()));
            if cancel_flag.load(Ordering::Relaxed) {
                break;
            }

            let mut album_artist_name = release
                .get("artist")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let release_title = release
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            progress_cb(format!(
                "Downloading {} — {}",
                if album_artist_name.is_empty() {
                    "Release"
                } else {
                    &album_artist_name
                },
                release_title
            ));

            let destination = release.get("existing_destination");
            let item_output = if let Some(root) = release["replacement_audit"]["root"].as_str() {
                if !roots.iter().any(|library| library.root == root) || !Path::new(root).is_dir() {
                    return Err("Replacement library is offline or no longer registered".into());
                }
                PathBuf::from(root)
            } else if let Some(dest) = destination {
                let r_str = dest.get("root").and_then(|v| v.as_str()).unwrap_or("");
                let requested = if r_str.starts_with('~') {
                    home_dir.join(r_str.trim_start_matches("~/"))
                } else {
                    PathBuf::from(r_str)
                };
                if !requested.is_dir() {
                    return Err("Existing library destination is offline or invalid".to_string());
                }
                requested
            } else {
                output_path.clone()
            };

            fs::create_dir_all(&item_output).map_err(|e| e.to_string())?;

            let raw_quality = release
                .get("replacement_audit")
                .and_then(|v| v.get("quality"))
                .and_then(|v| v.as_str())
                .unwrap_or(default_quality);
            let item_quality = stream_download::normalize_quality(raw_quality);

            let token = get_valid_token(db, &http).await?;
            // Fetch album info and tracks
            let album_info = match crate::subscriber_metadata::album_info(db,&http, ident, &token, market).await {
                Ok(info) => info,
                Err(e) => {
                    failed_count += 1;
                    progress_cb(format!(
                        "Failed to fetch album details for {}: {}",
                        ident, e
                    ));
                    continue;
                }
            };


            if album_artist_name.is_empty() {
                if let Some(ref a) = album_info.artist_name {
                    album_artist_name = a.clone();
                }
            }

            let all_tracks =
                match fetch_album_tracks(db, &http, ident, &token, &cancel_flag, market).await {
                    Ok(t) => t,
                    Err(e) => {
                        if cancel_flag.load(Ordering::Relaxed) {
                            break;
                        }
                        failed_count += 1;
                        progress_cb(format!("Failed to fetch tracks for album {}: {}", ident, e));
                        continue;
                    }
                };

            // Publish the shared credited track list for linking and recommendations too.
            if let Err(error)=crate::actions::release(db,ident,market,false).await {
                progress_cb(format!("Release metadata cached for download; catalogue publication deferred: {error}"));
            }

            let selected = release.get("selected_tracks").and_then(|v| v.as_array());
            let selected_ids: Option<std::collections::HashSet<String>> = selected.map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        s.get("id")
                            .map(|v| v.to_string().trim_matches('"').to_string())
                    })
                    .collect()
            });

            let tracks: Vec<_> = all_tracks
                .iter()
                .filter(|t| {
                    if let Some(ref set) = selected_ids {
                        set.contains(&t.id)
                    } else {
                        true
                    }
                })
                .cloned()
                .collect();

            if selected_ids
                .as_ref()
                .is_some_and(|ids| ids.len() != tracks.len())
            {
                return Err("Selected tracks no longer match the online release; review the queue selection".into());
            }
            if tracks.is_empty() {
                failed_count += 1;
                progress_cb("No tracks to download for this release".to_string());
                continue;
            }
            for track in &tracks {
                replacement_sources(release, &track.id, &item_output)?;
            }

            // Staging directory inside target root
            let work_id = uuid::Uuid::new_v4();
            let stage_dir = item_output.join(format!(".tibrary-work-{}", work_id));
            fs::create_dir_all(&stage_dir).map_err(|e| e.to_string())?;
            let _staging_cleanup = StagingDirectory(stage_dir.clone());

            // Fetch cover art if configured
            let cover_data = if cover_size_u32 > 0 {
                if let Some(ref cover_id) = album_info.cover {
                    fetch_cover_art(&http, cover_id, cover_size_u32).await
                } else {
                    None
                }
            } else {
                None
            };

            let mut all_tracks_ok = true;
            let total_tracks = tracks.len();
            planned_tracks[release_index]=total_tracks;
            let planned_total=if planned_tracks.iter().all(|n|*n>0) {planned_tracks.iter().sum()} else {0};
            monitor_cb(json!({"kind":"batch","release_id":ident,"release":release_title,"artist":album_artist_name,"total_tracks":total_tracks,"completed_tracks":0,"status":"running"}));
            let mut relative_track_filenames = Vec::new();
            let mut existing_published_files = Vec::new();
            let mut replacement_moves = std::collections::HashMap::<PathBuf, PathBuf>::new();
            let mut album_dir_opt: Option<PathBuf> = None;

            // Fetch audio concurrently, then publish the complete release only after every
            // selected file has passed tagging and path checks. The semaphore bounds network
            // pressure and each track writes to its own staging file.
            let parallel = provider.get("download_concurrency").and_then(Value::as_u64).unwrap_or(2).clamp(1, 3) as usize;
            let provider_segment_concurrency = provider.get("segment_concurrency").and_then(Value::as_u64).unwrap_or(2).clamp(1, 4) as usize;
            let permits = Arc::new(tokio::sync::Semaphore::new(parallel));
            let mut prefetch = Vec::with_capacity(total_tracks);
            let mut pending_downloads = DownloadTasks::default();
            for (idx, track) in tracks.iter().enumerate() {
                let track_total = all_tracks.iter().filter(|other| other.volume_number == track.volume_number).count() as u32;
                let disc_total = album_info.number_of_volumes.unwrap_or(1) as u32;
                let album_title = if let Some(version) = album_info.version.as_ref().filter(|version| !version.is_empty() && !album_info.title.to_lowercase().contains(&version.to_lowercase())) {
                    format!("{} ({version})", album_info.title)
                } else { album_info.title.clone() };
                let meta = preliminary_meta(track, ident, &album_title, &album_artist_name, track_total, disc_total, &album_info.release_date);
                let existing = existing_audio_path(&item_output, destination, template, &meta);
                if skip_existing && release["redownload"] != true && (nonempty_file(&existing) || nonempty_file(&existing.with_extension("m4a"))) {
                    prefetch.push(None);
                    continue;
                }
                let http = http.clone();
                let track = track.clone();
                let token = token.clone();
                let market = market.to_string();
                let stage = stage_dir.clone();
                let cancel = cancel_flag.clone();
                let monitor = monitor_cb.clone();
                let log = progress_cb.clone();
                let permits = permits.clone();
                let release_id = ident.clone();
                let task = tokio::spawn(async move {
                    let _permit = permits.acquire_owned().await.map_err(|e| e.to_string())?;
                    if cancel.load(Ordering::Relaxed) { return Err("Download cancelled".into()); }
                    if idx > 0 { tokio::time::sleep(Duration::from_millis(350 * (idx % parallel) as u64)).await; }
                    monitor(json!({"kind":"track","id":track.id,"release_id":release_id,"title":track.title,"index":idx+1,"total_tracks":total_tracks,"status":"preparing","percent":0,"bytes":0}));
                    let stream = get_playback_info(&http, &track.id, &token, item_quality, &market).await?;
                    let extension = if stream.mime_type.contains("mp4") || stream.codec.to_lowercase().contains("mp4") || stream.codec.to_lowercase().contains("aac") { ".m4a" } else { ".flac" };
                    let source = stage.join(format!(".source-{}{}", track.id, extension));
                    let started = std::time::Instant::now();
                    let estimator = std::sync::Mutex::new(crate::progress::Estimate::default());
                    let last = std::sync::Mutex::new((std::time::Instant::now() - Duration::from_secs(1), 255u8));
                    let progress = |pct: u8, bytes: u64, total_bytes: Option<u64>| {
                        let mut latest = last.lock().unwrap();
                        if pct == latest.1 && latest.0.elapsed() < Duration::from_millis(250) { return; }
                        if latest.0.elapsed() < Duration::from_millis(250) && pct < 100 { return; }
                        *latest = (std::time::Instant::now(), pct);
                        let mut estimate = estimator.lock().unwrap();
                        estimate.observe(started.elapsed().as_secs_f64(),bytes,total_bytes.unwrap_or(u64::MAX),"bytes");
                        let speed = estimate.rate.unwrap_or(0.);
                        let estimated_total = total_bytes.or_else(|| if pct > 0 { Some(bytes.saturating_mul(100) / pct as u64) } else { None });
                        let remaining = estimated_total.and_then(|total| if speed > 0.0 { Some((total.saturating_sub(bytes) as f64 / speed).ceil() as u64) } else { None });
                        monitor(json!({"kind":"track","id":track.id,"release_id":release_id,"title":track.title,"index":idx+1,"total_tracks":total_tracks,"status":"downloading","percent":pct,"bytes":bytes,"total_bytes":total_bytes,"estimated_total_bytes":estimated_total,"bytes_per_second":speed,"eta_seconds":remaining}));
                        if pct == 100 { log(format!("Downloaded audio · {}", track.title)); }
                    };
                    let segment_concurrency = provider_segment_concurrency;
                    download_stream(&http, &stream, &source, &cancel, &progress, segment_concurrency).await?;
                    Ok::<_, String>((stream, source))
                });
                pending_downloads.0.push(task.abort_handle());
                prefetch.push(Some(task));
            }

            for (idx, track) in tracks.iter().enumerate() {
                if planned_total>0 {progress_cb(format!("Downloading audio · {}/{planned_total} tracks · {album_artist_name} — {release_title} · {}",processed_tracks+idx,track.title));}
                if cancel_flag.load(Ordering::Relaxed) {
                    all_tracks_ok = false;
                    break;
                }

                let track_total_on_vol = all_tracks
                    .iter()
                    .filter(|t| t.volume_number == track.volume_number)
                    .count() as u32;
                let disc_total = album_info.number_of_volumes.unwrap_or(1) as u32;

                // Album version in title if appropriate
                let full_album_title = if let Some(ref ver) = album_info.version {
                    if !ver.is_empty()
                        && !album_info
                            .title
                            .to_lowercase()
                            .contains(&ver.to_lowercase())
                    {
                        format!("{} ({})", album_info.title, ver)
                    } else {
                        album_info.title.clone()
                    }
                } else {
                    album_info.title.clone()
                };

                let prelim_meta = preliminary_meta(track, ident, &full_album_title, &album_artist_name, track_total_on_vol, disc_total, &album_info.release_date);

                if skip_existing && release["redownload"] != true {
                    let dst_flac = existing_audio_path(&item_output, destination, template, &prelim_meta);
                    let dst_m4a = dst_flac.with_extension("m4a");

                    let exists_flac = dst_flac.is_file()
                        && dst_flac.metadata().map(|m| m.len() > 0).unwrap_or(false);
                    let exists_m4a = dst_m4a.is_file()
                        && dst_m4a.metadata().map(|m| m.len() > 0).unwrap_or(false);

                    if exists_flac || exists_m4a {
                        progress_cb(format!(
                            "Track {}/{} · {} (already exists, skipped)",
                            idx + 1,
                            total_tracks,
                            track.title
                        ));
                        let actual_dst = if exists_flac { dst_flac } else { dst_m4a };
                        if let Some(fname) = actual_dst.file_name().and_then(|f| f.to_str()) {
                            relative_track_filenames.push(fname.to_string());
                        }
                        existing_published_files.push(actual_dst);
                        monitor_cb(json!({"kind":"track","id":track.id,"release_id":ident,"title":track.title,"index":idx+1,"total_tracks":total_tracks,"status":"already downloaded","percent":100}));
                        monitor_cb(json!({"kind":"batch","release_id":ident,"release":release_title,"artist":album_artist_name,"total_tracks":total_tracks,"completed_tracks":idx+1,"status":"running"}));
                        continue;
                    }
                }

                let Some(task) = prefetch[idx].take() else {
                    // An external editor may remove a file after the preflight
                    // skip decision. Leave the release queued for a fresh retry.
                    progress_cb(format!("Existing file changed during download · {} · retry this release", track.title));
                    all_tracks_ok = false;
                    break;
                };
                let result = task.await;
                let (stream_info, mut staged_source) = match result {
                    Ok(Ok(audio)) => audio,
                    Ok(Err(error)) => {
                        progress_cb(format!("Download failed for {}: {error}", track.title));
                        monitor_cb(json!({"kind":"track","id":track.id,"release_id":ident,"title":track.title,"index":idx+1,"total_tracks":total_tracks,"status":"failed","error":error}));
                        all_tracks_ok = false;
                        break;
                    }
                    Err(error) => {
                        progress_cb(format!("Download task stopped for {}: {error}", track.title));
                        all_tracks_ok = false;
                        break;
                    }
                };
                let ext = if staged_source.extension().and_then(|s| s.to_str()) == Some("m4a") { ".m4a" } else { ".flac" };

                let mut bytes = [0u8; 32];
                {
                    use std::io::Read;
                    let mut file = fs::File::open(&staged_source).map_err(|e| e.to_string())?;
                    let _ = file.read(&mut bytes).map_err(|e| e.to_string())?;
                }
                let mut detected_ext = stream_download::detect_audio_extension(&bytes, ext);
                let lossless = item_quality == "LOSSLESS" || item_quality == "HI_RES_LOSSLESS";
                if lossless && detected_ext == ".m4a" {
                    progress_cb(format!("Preparing lossless FLAC · {}", track.title));
                    let converted = stage_dir.join(format!("converted-{}.flac", track.id));
                    if let Err(error) =
                        crate::flac_container::remux_flac(&staged_source, &converted, &cancel_flag)
                            .await
                    {
                        progress_cb(format!("Download failed for {}: {}", track.title, error));
                        all_tracks_ok = false;
                        break;
                    }
                    fs::remove_file(&staged_source).map_err(|e| e.to_string())?;
                    staged_source = converted;
                    detected_ext = ".flac".into();
                } else if lossless && detected_ext != ".flac" {
                    return Err("The provider did not return lossless FLAC audio".into());
                }
                let correct_path =
                    staged_source.with_extension(detected_ext.trim_start_matches('.'));
                if correct_path != staged_source {
                    fs::rename(&staged_source, &correct_path).map_err(|e| e.to_string())?;
                    staged_source = correct_path;
                }

                // Fetch lyrics
                let (lyrics_synced, lyrics_unsynced) = if lyrics_embed || lyrics_file {
                    fetch_lyrics(&http, &track.id, &token, market).await
                } else {
                    (None, None)
                };

                let meta = TrackDownloadMeta {
                    track_id: track.id.clone(),
                    album_id: ident.clone(),
                    title: track.title.clone(),
                    album: full_album_title,
                    album_artist: album_artist_name.clone(), // STRICTLY ALBUM ARTIST!
                    track_artists: if !track.artists.is_empty() {
                        track.artists.clone()
                    } else {
                        vec![album_artist_name.clone()]
                    },
                    track_number: track.track_number,
                    track_total: track_total_on_vol,
                    disc_number: track.volume_number,
                    disc_total,
                    date: album_info.release_date.clone(),
                    isrc: track.isrc.clone(),
                    copyright: track.copyright.clone(),
                    credit_tags: crate::enrichment::credit_tags(&track.credits),
                    bpm: track.bpm,
                    musical_key: track.key.clone(),
                    release_type: None,
                    explicit: track.explicit,
                    lyrics: if lyrics_embed {
                        lyrics_synced.clone()
                    } else {
                        None
                    },
                    unsynced_lyrics: if lyrics_embed {
                        lyrics_unsynced.clone()
                    } else {
                        None
                    },
                    album_replay_gain: if replay_gain {
                        stream_info.album_replay_gain
                    } else {
                        None
                    },
                    album_peak_amplitude: if replay_gain {
                        stream_info.album_peak_amplitude
                    } else {
                        None
                    },
                    track_replay_gain: if replay_gain {
                        stream_info.track_replay_gain
                    } else {
                        None
                    },
                    track_peak_amplitude: if replay_gain {
                        stream_info.track_peak_amplitude
                    } else {
                        None
                    },
                };

                let tag_path = staged_source.clone();
                let tag_meta = meta.clone();
                let tag_cover = cover_data.clone();
                let previous_path = existing_audio_path(&item_output, destination, template, &meta);
                let source_paths = replacement_sources(release, &track.id, &item_output)?;
                let explicit_replacement = !source_paths.is_empty();
                let previous_files = if explicit_replacement { source_paths.clone() }
                    else if release["redownload"] == true || release.get("replacement_audit").is_some() {
                    vec![previous_path.clone(), previous_path.with_extension("m4a")]
                } else { Vec::new() };
                let tag_result = tokio::task::spawn_blocking(move || {
                    apply_audio_tags(&tag_path, &tag_meta, tag_cover.as_deref())?;
                    if !previous_files.is_empty() && !explicit_replacement { preserve_existing_dj_tags(&tag_path, &previous_files, false)?; }
                    Ok::<_, String>(())
                })
                    .await.map_err(|e| format!("Tag worker stopped: {e}"))?;
                if let Err(e) = tag_result {
                    progress_cb(format!("Tagging failed for {}: {}", track.title, e));
                    monitor_cb(json!({"kind":"track","id":track.id,"release_id":ident,"title":track.title,"index":idx+1,"total_tracks":total_tracks,"status":"failed","error":e}));
                    all_tracks_ok = false;
                    break;
                }

                // Determine final relative path
                let target_path_in_stage = if let Some(source) = source_paths.first() {
                    stage_dir.join(source.strip_prefix(&item_output).map_err(|e| e.to_string())?)
                        .with_extension(detected_ext.trim_start_matches('.'))
                } else if let Some(dest) = destination {
                    let rel_album = dest
                        .get("album_relative")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let disc_dir_name = if disc_total > 1 {
                        format!("Disc {}", track.volume_number)
                    } else {
                        String::new()
                    };
                    let filename =
                        format_download_path("{tracknumber} - {title}", &meta, &detected_ext);
                    let mut p = stage_dir.join(rel_album);
                    if !disc_dir_name.is_empty() {
                        p.push(disc_dir_name);
                    }
                    p.push(filename);
                    p
                } else {
                    let rel_path = format_download_path(template, &meta, &detected_ext);
                    stage_dir.join(rel_path)
                };

                if let Some(parent) = target_path_in_stage.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    album_dir_opt = Some(
                        if disc_total > 1 {
                            parent.parent().unwrap_or(parent)
                        } else {
                            parent
                        }
                        .to_path_buf(),
                    );
                }
                if target_path_in_stage.exists() {
                    return Err("Two downloaded tracks resolve to the same filename; adjust the folder template".into());
                }
                fs::rename(&staged_source, &target_path_in_stage)
                    .map_err(|e| format!("Could not stage downloaded track: {e}"))?;
                let mut replacement_files = Vec::new();
                for source in &source_paths {
                    let relative = source.strip_prefix(&item_output).map_err(|e| e.to_string())?;
                    let copy = stage_dir.join(relative).with_extension(detected_ext.trim_start_matches('.'));
                    if copy != target_path_in_stage {
                        if let Some(parent) = copy.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
                        let mut output = fs::OpenOptions::new().write(true).create_new(true).open(&copy).map_err(|e| e.to_string())?;
                        let mut input = fs::File::open(&target_path_in_stage).map_err(|e| e.to_string())?;
                        std::io::copy(&mut input, &mut output).map_err(|e| format!("Could not stage every selected replacement: {e}"))?;
                    }
                    replacement_moves.insert(item_output.join(copy.strip_prefix(&stage_dir).map_err(|e|e.to_string())?), source.clone());
                    replacement_files.push((copy, source.clone()));
                }
                if !replacement_files.is_empty() {
                    tokio::task::spawn_blocking(move || {
                        for (replacement, source) in replacement_files {
                            preserve_existing_dj_tags(&replacement, &[source], true)?;
                        }
                        Ok::<_, String>(())
                    }).await.map_err(|e| format!("Replacement verification worker stopped: {e}"))??;
                }

                // Companion .lrc file if requested
                if lyrics_file {
                    if let Some(lrc_text) = lyrics_synced.as_ref().or(lyrics_unsynced.as_ref()) {
                        let _ = stream_download::write_lrc_file(&target_path_in_stage, lrc_text);
                    }
                }

                if let Some(fname) = target_path_in_stage.file_name().and_then(|f| f.to_str()) {
                    relative_track_filenames.push(fname.to_string());
                }
                monitor_cb(json!({"kind":"track","id":track.id,"release_id":ident,"title":track.title,"index":idx+1,"total_tracks":total_tracks,"status":"staged","percent":100}));
                monitor_cb(json!({"kind":"batch","release_id":ident,"release":release_title,"artist":album_artist_name,"total_tracks":total_tracks,"completed_tracks":idx+1,"status":"running"}));

                // Delay pacing between tracks if enabled
                if download_delay && parallel == 1 && delay_min > 0.0 {
                    let wait_secs = {
                        let mut rng = rand::thread_rng();
                        rng.gen_range(delay_min..=delay_max.max(delay_min))
                    };
                    tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;
                }
            }

            for task in prefetch.into_iter().flatten() {
                task.abort();
                let _ = task.await;
            }
            if !all_tracks_ok {
                failed_count += 1;
                monitor_cb(json!({"kind":"batch","release_id":ident,"release":release_title,"artist":album_artist_name,"total_tracks":total_tracks,"status":"failed"}));
                let _ = fs::remove_dir_all(&stage_dir);
                if cancel_flag.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }

            // Save companion cover.jpg in album directory if cover_album_file is true
            if cover_album_file {
                if let Some(ref cover_bytes) = cover_data {
                    if let Some(ref album_dir) = album_dir_opt {
                        let cover_path = album_dir.join("cover.jpg");
                        if !cover_path.exists() {
                            let _ = fs::write(cover_path, cover_bytes);
                        }
                    }
                }
            }

            // Save _playlist.m3u8 if playlist_create is true
            if playlist_create && !relative_track_filenames.is_empty() {
                if let Some(ref album_dir) = album_dir_opt {
                    let _ = stream_download::write_m3u8_playlist(
                        album_dir,
                        "_playlist",
                        &relative_track_filenames,
                    );
                }
            }

            // Atomically publish staged files to destination library
            let publish_stage = stage_dir.clone();
            let publish_output = item_output.clone();
            let is_redownload = release["redownload"] == true;
            let publish_result = tokio::task::spawn_blocking(move || {
                if is_redownload {
                    stream_download::publish_redownloaded_files(&publish_stage, &publish_output)
                } else {
                    publish_staged_files(&publish_stage, &publish_output).map(|files| (files, Vec::new()))
                }
            }).await.map_err(|e| format!("Publish worker stopped: {e}"))?;
            let (mut published, previous_files) = match publish_result {
                Ok(p) => p,
                Err(e) => {
                    let _ = fs::remove_dir_all(&stage_dir);
                    failed_count += 1;
                    progress_cb(format!("Publishing failed for {}: {}", release_title, e));
                    monitor_cb(json!({"kind":"batch","release_id":ident,"release":release_title,"artist":album_artist_name,"total_tracks":total_tracks,"status":"failed","error":e}));
                    continue;
                }
            };
            let backup_root = previous_files.first().and_then(|path| {
                let folder = path.strip_prefix(&item_output).ok()?.components().next()?;
                let name = folder.as_os_str().to_string_lossy();
                if name.starts_with(".tibrary-redownload-backup-") { Some(item_output.join(folder.as_os_str())) } else { None }
            });
            published.extend(existing_published_files);
            let _ = fs::remove_dir_all(&stage_dir);

            // Keep previous copies until all new audio has been indexed. Moves
            // between extensions also migrate saved links and ignore choices.
            let mut index_ok = true;
            for path in &published {
                let Some(library) = roots.iter().filter(|library| path.starts_with(&library.root))
                    .max_by_key(|library| library.root.len()) else { continue; };
                if !path.extension().and_then(|extension| extension.to_str())
                    .is_some_and(|extension| crate::scanner::AUDIO_EXTS.contains(&extension.to_ascii_lowercase().as_str())) { continue; }
                let audio_path = path.clone();
                let inspection = tokio::task::spawn_blocking(move || {
                    let meta = crate::scanner::read_audio_metadata(&audio_path)?;
                    let stat = fs::metadata(&audio_path).map_err(|e| e.to_string())?;
                    let mtime = stat.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64).unwrap_or(0);
                    Ok::<_, String>((serde_json::to_value(meta).map_err(|e|e.to_string())?, stat.len() as i64, mtime))
                }).await.map_err(|e|e.to_string()).and_then(|result| result);
                let source = replacement_moves.get(path).unwrap_or(path);
                let update = match inspection {
                    Ok((metadata, size, mtime)) => db.apply_file_update(&source.to_string_lossy(), &path.to_string_lossy(), &library.root, &metadata, size, mtime).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = update {
                    index_ok = false;
                    progress_cb(format!("Audio saved; local index update needs retry · {} · {error}. Reopen the app to refresh the local index.", path.display()));
                }
            }
            if index_ok {
                let now_iso = chrono::Utc::now().to_rfc3339();
                if let Err(error) = conn.execute(
                    "UPDATE queue SET decision = 'downloaded', approved = 0, updated = ? WHERE id = ?",
                    (now_iso.as_str(), ident.as_str()),
                ).await {
                    index_ok = false;
                    progress_cb(format!("Audio saved; queue update needs retry · {release_title} · {error}"));
                }
            }
            if !index_ok {
                failed_count += 1;
                progress_cb(format!("Saved audio requires an index retry · {release_title} · previous copies retained"));
                monitor_cb(json!({"kind":"batch","release_id":ident,"release":release_title,"artist":album_artist_name,"status":"failed","error":"Audio saved; index or queue update needs retry"}));
                continue;
            }
            let mut retained_previous = false;
            for old in previous_files {
                if let Err(error) = crate::duplicates::trash_file_or_directory(&old.to_string_lossy()).await {
                    retained_previous = true;
                    progress_cb(format!("Previous copy retained for review · {} ({error})", old.display()));
                }
            }
            if !retained_previous {
                if let Some(folder) = backup_root { let _ = fs::remove_dir_all(folder); }
            }

            processed_tracks+=total_tracks;
            if planned_total>0 {progress_cb(format!("Saved audio · {processed_tracks}/{planned_total} tracks · {album_artist_name} — {release_title}"));}
            completed_count += 1;
            for (idx, track) in tracks.iter().enumerate() {
                monitor_cb(json!({"kind":"track","id":track.id,"release_id":ident,"title":track.title,"index":idx+1,"total_tracks":total_tracks,"status":"complete","percent":100}));
            }
            monitor_cb(json!({"kind":"batch","release_id":ident,"release":release_title,"artist":album_artist_name,"total_tracks":total_tracks,"completed_tracks":total_tracks,"status":"complete"}));
        }

        db.bump_revision();
        if failed_count > 0 && !cancel_flag.load(Ordering::Relaxed) {
            return Err(format!("Downloaded {completed_count} releases; {failed_count} failed and remain queued. See Activity for details."));
        }
        Ok(completed_count)
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    fn replacements_preserve_missing_dj_values_only_for_the_same_recording() {
        let temp = std::env::temp_dir().join(format!("replacement-dj-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp).unwrap();
        let old = temp.join("old.flac");
        let replacement = temp.join("new.flac");
        let identity = std::collections::HashMap::from([
            ("title".into(), "Track".into()), ("isrc".into(), "GBAYE9900001".into()),
        ]);
        for path in [&old, &replacement] {
            fs::write(path, stream_download::MINIMAL_FLAC).unwrap();
            crate::tag_writer::write_tags(path, &identity).unwrap();
        }
        crate::tag_writer::write_tags(&old, &std::collections::HashMap::from([
            ("bpm".into(),"127.5".into()), ("initialkey".into(),"A minor".into()),
        ])).unwrap();
        preserve_existing_dj_tags(&replacement, &[old.clone()], false).unwrap();
        let metadata = crate::scanner::read_audio_metadata(&replacement).unwrap();
        assert_eq!(metadata.bpm.as_deref(), Some("127.5"));
        assert_eq!(metadata.musical_key.as_deref(), Some("8A"));
        crate::tag_writer::write_tags(&replacement, &std::collections::HashMap::from([
            ("bpm".into(),"133".into()), ("initialkey".into(),"9A".into()),
        ])).unwrap();
        preserve_existing_dj_tags(&replacement, &[old.clone()], false).unwrap();
        assert_eq!(crate::scanner::read_audio_metadata(&replacement).unwrap().bpm.as_deref(), Some("133"));
        for (title, isrc) in [("Track (Extended Mix)", "GBAYE9900001"), ("Track", "GBAYE9900002")] {
            crate::tag_writer::write_tags(&replacement, &std::collections::HashMap::from([
                ("title".into(),title.into()), ("isrc".into(),isrc.into()),
                ("bpm".into(),"".into()), ("initialkey".into(),"".into()),
            ])).unwrap();
            assert!(preserve_existing_dj_tags(&replacement, &[old.clone()], true).is_err());
            preserve_existing_dj_tags(&replacement, &[old.clone()], false).unwrap();
            let metadata = crate::scanner::read_audio_metadata(&replacement).unwrap();
            assert!(metadata.bpm.is_none());
            assert!(metadata.musical_key.is_none());
        }
        let other = temp.join("second.flac");
        fs::copy(&old, &other).unwrap();
        let release = json!({"replacement_audit":{"root":temp,"source_paths":{"1":[old,other]}}});
        assert_eq!(replacement_sources(&release, "1", &temp).unwrap().len(), 2);
        assert!(replacement_sources(&release, "missing", &temp).is_err());
        let unsafe_release = json!({"replacement_audit":{"root":temp,"source_paths":{"1":[temp.join("../outside.flac")]}}});
        assert!(replacement_sources(&unsafe_release, "1", &temp).is_err());
        fs::remove_dir_all(temp).unwrap();
    }

    #[tokio::test]
    async fn release_errors_abort_detached_and_awaited_download_workers() {
        let mut pending = DownloadTasks::default();
        let waiting = tokio::spawn(std::future::pending::<()>());
        let detached = tokio::spawn(std::future::pending::<()>());
        pending.0.extend([waiting.abort_handle(), detached.abort_handle()]);
        let detached_state = detached.abort_handle();
        drop(detached);
        drop(pending);
        assert!(waiting.await.unwrap_err().is_cancelled());
        tokio::task::yield_now().await;
        assert!(detached_state.is_finished());
    }
    #[tokio::test]
    #[ignore = "Uses the saved account and downloads one release to a temporary folder"]
    async fn isolated_one_track_download() {
        let temp = std::env::temp_dir().join(format!("tibrary-live-download-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp).unwrap();
        let db = TursoDb::open(&temp.join("db.sqlite3")).await.unwrap();
        db.set_preference("downloads", &json!({"output":temp.join("music"),"quality":"LOSSLESS","cover_size":0,"skip_existing":false,"parallel_downloads":2})).await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO queue (id,payload,approved,decision,updated) VALUES (?, ?, 1, 'queued', ?)",
            ("561743899", json!({"id":"561743899","title":"Sex","artist":"2hollis","track_count":1}).to_string().as_str(), chrono::Utc::now().to_rfc3339().as_str())).await.unwrap();
        let completed = DownloadManager::run_downloads(&db, None, Arc::new(AtomicBool::new(false)), "isolated-test", |_| {}, |_| {}).await.unwrap();
        assert_eq!(completed, 1);
        let rows = db.get_queue_rows("downloaded", None, None, None, None, 0, 10).await.unwrap();
        assert_eq!(rows.total, 1);
        assert!(temp.join("music").exists());
        drop(db);
        fs::remove_dir_all(temp).unwrap();
    }
    #[tokio::test]
    #[ignore = "Uses the saved account and downloads two tracks to a temporary folder"]
    async fn isolated_parallel_tracks_download() {
        let temp = std::env::temp_dir().join(format!("tibrary-parallel-download-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp).unwrap();
        let db = TursoDb::open(&temp.join("db.sqlite3")).await.unwrap();
        db.set_preference("downloads", &json!({"output":temp.join("music"),"quality":"LOSSLESS","cover_size":0,"skip_existing":false,"parallel_downloads":2})).await.unwrap();
        let selected = json!({"id":"482348849","title":"Sunk Cost Fallacy Deluxe","artist":"Fox Stevenson","track_count":20,
            "selected_tracks":[{"id":"482348850"},{"id":"482348851"}]});
        let conn = db.connect().unwrap();
        let payload = selected.to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute("INSERT INTO queue (id,payload,approved,decision,updated) VALUES (?, ?, 1, 'queued', ?)",
            ("482348849", payload.as_str(), now.as_str())).await.unwrap();
        let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let observed = events.clone();
        let download = DownloadManager::run_downloads(&db, None, Arc::new(AtomicBool::new(false)), "parallel-test", |_| {}, move |item| observed.lock().unwrap().push(item));
        let reads = async {
            for _ in 0..200 {
                if events.lock().unwrap().iter().any(|event| event["status"] == "downloading") { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let started = std::time::Instant::now();
            db.get_state(None, &[], None).await.unwrap();
            assert!(started.elapsed() < Duration::from_secs(2), "Local state query stalled during download");
        };
        let (downloaded, ()) = tokio::join!(download, reads);
        let completed = downloaded.unwrap();
        assert_eq!(completed, 1);
        let events = events.lock().unwrap();
        let active_ids: std::collections::HashSet<_> = events.iter().filter(|event| event["status"] == "downloading")
            .filter_map(|event| event["id"].as_str()).collect();
        assert_eq!(active_ids.len(), 2);
        drop(events);
        drop(db);
        fs::remove_dir_all(temp).unwrap();
    }
}

#[cfg(test)]
#[tokio::test]
#[ignore="Live one-track download into a disposable directory; saved subscriber account"]
async fn live_subscriber_download_pipeline() {
    let dir=std::env::temp_dir().join(format!("subscriber-download-{}",uuid::Uuid::new_v4()));
    let db=TursoDb::open(dir.join("db")).await.unwrap();
    let output=dir.join("music");
    db.set_preference("downloads",&json!({"output":output,"quality":"LOSSLESS","cover_size":1280,"skip_existing":true})).await.unwrap();
    let release=crate::actions::release(&db,"140303440","GB",false).await.unwrap();
    let track=release["tracks"].as_array().unwrap().iter().min_by(|a,b|a["duration"].as_f64().unwrap_or(9999.).total_cmp(&b["duration"].as_f64().unwrap_or(9999.))).unwrap();
    let mut payload=release.clone();payload["selected_tracks"]=json!([{"id":track["id"]}]);
    let conn=db.connect().unwrap();
    conn.execute("INSERT INTO queue(id,payload,approved,decision) VALUES(?,?,1,'queued')",("140303440",payload.to_string())).await.unwrap();
    let count=DownloadManager::run_downloads(&db,None,Arc::new(AtomicBool::new(false)),"test",|line|println!("{line}"),|_|{}).await.unwrap();
    assert_eq!(count,1);
    fn collect(path:&Path,files:&mut Vec<PathBuf>) { for entry in fs::read_dir(path).unwrap().flatten() {let path=entry.path();if path.is_dir(){collect(&path,files)}else if path.extension().is_some_and(|x|x=="flac"){files.push(path);}} }
    let mut files=Vec::new();collect(&output,&mut files);
    assert_eq!(files.len(),1);
    let metadata=crate::scanner::read_audio_metadata(&files[0]).unwrap();
    println!("Downloaded audio/tag validation: {}",serde_json::to_string(&metadata).unwrap().chars().take(700).collect::<String>());
    assert!(files[0].metadata().unwrap().len()>10000);
    let mut rows=conn.query("SELECT decision FROM queue WHERE id='140303440'",()).await.unwrap();
    let decision:String=rows.next().await.unwrap().unwrap().get(0).unwrap();assert_eq!(decision,"downloaded");
    assert_eq!(metadata.tracktotal.as_deref(),Some("12"));
    assert!(metadata.bpm.is_some());
    // Redownload through the same queue entry; replace only this disposable file.
    drop(rows);
    crate::tag_writer::write_tags(&files[0], &std::collections::HashMap::from([("comment".to_string(), "Disposable previous copy".to_string())])).unwrap();
    payload["redownload"] = json!(true);
    conn.execute("UPDATE queue SET payload=?,approved=1,decision='queued' WHERE id='140303440'", (payload.to_string(),)).await.unwrap();
    let replaced=DownloadManager::run_downloads(&db,None,Arc::new(AtomicBool::new(false)),"test-replace",|line|println!("{line}"),|_|{}).await.unwrap();
    assert_eq!(replaced,1);
    let mut queue_count=conn.query("SELECT COUNT(*) FROM queue",()).await.unwrap();
    assert_eq!(queue_count.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),1);
    drop(queue_count);
    assert!((crate::scanner::read_audio_metadata(&files[0]).unwrap().duration-metadata.duration).abs()<0.01);
    let cancel=Arc::new(AtomicBool::new(false));
    crate::scanner::scan_library(&db,&output,cancel.clone(),|_|{}).await.unwrap();
    db.choose_artist("Cassie",&["3924".into()]).await.unwrap();
    let client=crate::tidal::TidalClient::from_db(&db).await.unwrap();
    let album:crate::tidal::TidalRelease=serde_json::from_value(release).unwrap();
    client.save_catalogue_to_db(&db,"GB",&crate::tidal::TidalCatalogue{id:"3924".into(),name:"Cassie".into(),releases:vec![album]}).await.unwrap();
    let links=crate::linking::link_library_mode(&db,"GB",&output.to_string_lossy(),cancel,|line|println!("{line}"),None,false,true).await.unwrap();
    assert_eq!(links.linked,1);
    // Exercise real decoded audio through local repair and organisation as well.
    let root=output.to_string_lossy().to_string();
    let path=files[0].to_string_lossy().to_string();
    let wrong=output.join("Wrong filename.flac").to_string_lossy().to_string();
    let corrupt=crate::maintenance::FileApplyItem{path:path.clone(),target:Some(wrong.clone()),artwork:None,
        tags:std::collections::HashMap::from([("tracknumber".into(),"7/1".into()),("tracktotal".into(),"1".into()),("discnumber".into(),"1".into()),("disctotal".into(),"1".into())])};
    crate::maintenance::apply_file_item(&db,&root,&corrupt).await.unwrap();
    let indexed=db.get_local_files_page(Some(&root),usize::MAX,0).await.unwrap().0;
    let plans=crate::workflows::plan_cached(&db,&indexed,"numbers",None).await.unwrap();
    assert_eq!(plans[0].changes.get("tracktotal").map(String::as_str),Some("12"));
    crate::maintenance::apply_file_item(&db,&root,&crate::maintenance::FileApplyItem{path:wrong.clone(),target:None,artwork:None,tags:plans[0].changes.clone()}).await.unwrap();
    let indexed=db.get_local_files_page(Some(&root),usize::MAX,0).await.unwrap().0;
    let layout=crate::workflows::plan_cached(&db,&indexed,"organise",None).await.unwrap();
    let target=layout[0].target.clone().expect("wrong filename needs organising");
    crate::maintenance::apply_file_item(&db,&root,&crate::maintenance::FileApplyItem{path:wrong,target:Some(target.clone()),artwork:None,tags:std::collections::HashMap::new()}).await.unwrap();
    let repaired=crate::scanner::read_audio_metadata(Path::new(&target)).unwrap();
    assert_eq!(repaired.tracktotal.as_deref(),Some("12"));
    assert_eq!(repaired.bpm,metadata.bpm);
    assert!((repaired.duration-metadata.duration).abs()<0.01);
    // Review and remove a redundant copy via the same action used by the UI.
    let duplicate_folder=output.join("Disposable duplicate (2000)");
    fs::create_dir_all(&duplicate_folder).unwrap();
    let duplicate=duplicate_folder.join(format!("qa-{}.flac",uuid::Uuid::new_v4()));
    fs::copy(&target,&duplicate).unwrap();
    crate::tag_writer::write_tags(&duplicate,&std::collections::HashMap::from([
        ("album".into(),"Disposable duplicate".into()),("date".into(),"2000".into()),
    ])).unwrap();
    crate::scanner::scan_library(&db,&output,Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
    let backend=Arc::new(crate::Backend::new());
    let review=crate::actions::execute(&db,&backend,"review_consolidation",&json!({"root":root,"ids":[duplicate_folder.to_string_lossy()]}),Arc::new(AtomicBool::new(false))).await.unwrap();
    let preview=backend.previews.lock().unwrap().get(review["preview_id"].as_str().unwrap()).unwrap().clone();
    let removed=crate::actions::execute(&db,&backend,"consolidate",&json!({"root":root,"confirmed":true,"preview_id":review["preview_id"],"ids":[preview["rows"][0]["id"]]}),Arc::new(AtomicBool::new(false))).await.unwrap();
    assert_eq!(removed["completed"],1);
    assert!(!duplicate.exists());
    assert!(Path::new(&target).exists());
    assert_eq!(db.get_local_files_page(Some(&root),100,0).await.unwrap().1,1);
    // MQA replacements must replace every selected copy at its existing path,
    // using one fetched recording, rather than skipping it as already present.
    let second = output.join("Second copy").join("07 - Hope.flac");
    fs::create_dir_all(second.parent().unwrap()).unwrap();
    fs::copy(&target, &second).unwrap();
    for (path,key) in [(Path::new(&target),"12A"),(second.as_path(),"6A")] {
        crate::tag_writer::write_tags(path,&std::collections::HashMap::from([("initialkey".into(),key.into())])).unwrap();
    }
    crate::scanner::scan_library(&db,&output,Arc::new(AtomicBool::new(false)),|_|{}).await.unwrap();
    let local=db.get_local_files_page(Some(&root),100,0).await.unwrap().0;
    for file in &local {
        db.save_track_link(&file.path,"GB",&json!([0,0,file.size,file.mtime]).to_string(),
            &json!({"status":"linked","ids":{"album_id":"140303440","track_id":"140303447"}}).to_string()).await.unwrap();
        db.set_preference(&format!("mqa-audit:{}",file.path),&json!({"size":file.size,"mtime":file.mtime,"result":{"detected":true}})).await.unwrap();
    }
    let queued=crate::actions::execute(&db,&backend,"queue_mqa",&json!({"root":root,"ids":[target,second]}),Arc::new(AtomicBool::new(false))).await.unwrap();
    assert_eq!(queued["releases"],1);
    let count=DownloadManager::run_downloads(&db,None,Arc::new(AtomicBool::new(false)),"test-mqa",|line|println!("{line}"),|_|{}).await.unwrap();
    assert_eq!(count,1);
    for (path,key) in [(Path::new(&target),"12A"),(second.as_path(),"6A")] {
        let replaced=crate::scanner::read_audio_metadata(path).unwrap();
        assert!((replaced.duration-metadata.duration).abs()<0.01);
        assert_eq!(replaced.musical_key.as_deref(),Some(key));
        assert_eq!(replaced.tidal_track_id.as_deref(),Some("140303447"));
    }
    assert_eq!(db.get_local_files_page(Some(&root),100,0).await.unwrap().1,2);
    assert_eq!(db.get_stats("GB",Some(&root)).await.unwrap().linked_tracks,2);
    println!("Download, multi-copy MQA replacement, metadata, queue, scan, linking, number repair, organisation and reviewed duplicate removal passed in {}",dir.display());
}
