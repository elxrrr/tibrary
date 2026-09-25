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
    self, apply_audio_tags, download_stream, fetch_album_info, fetch_album_tracks, fetch_cover_art,
    fetch_lyrics, format_download_path, get_playback_info, get_valid_token, publish_staged_files,
    TrackDownloadMeta,
};

struct StagingDirectory(PathBuf);
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct DownloadManager;

impl DownloadManager {
    pub async fn run_downloads(
        db: &TursoDb,
        app: Option<&AppHandle>,
        cancel_flag: Arc<AtomicBool>,
        _job_id: &str,
        progress_cb: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<usize, String> {
        let progress_cb = Arc::new(progress_cb);
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

        progress_cb(
            "Tidal account connected · lossless audio · sequential track downloads".to_string(),
        );

        let configuration = db.get_settings().await?;
        let market = configuration["general"]["market"].as_str().unwrap_or("GB");
        let roots = db.list_roots(market).await?;
        let mut completed_count = 0;
        let mut failed_count = 0;

        for (ident, release) in &items {
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
            let item_output = if let Some(dest) = destination {
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
            let album_info = match fetch_album_info(&http, ident, &token, market).await {
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

            db.set_preference(
                &format!("subscriber-album:{market}:{ident}"),
                &json!(album_info),
            )
            .await?;

            if album_artist_name.is_empty() {
                if let Some(ref a) = album_info.artist_name {
                    album_artist_name = a.clone();
                }
            }

            let all_tracks =
                match fetch_album_tracks(&http, ident, &token, &cancel_flag, market).await {
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

            db.set_preference(
                &format!("subscriber-tracks:{market}:{ident}"),
                &json!(all_tracks),
            )
            .await?;
            for track in &all_tracks {
                db.set_preference(
                    &format!("dj-check:{market}:{}", track.id),
                    &json!({"id":track.id,"isrc":track.isrc,"bpm":track.bpm,"key":track.key}),
                )
                .await?;
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
            let mut relative_track_filenames = Vec::new();
            let mut existing_published_files = Vec::new();
            let mut album_dir_opt: Option<PathBuf> = None;

            for (idx, track) in tracks.iter().enumerate() {
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

                let prelim_meta = TrackDownloadMeta {
                    track_id: track.id.clone(),
                    album_id: ident.clone(),
                    title: track.title.clone(),
                    album: full_album_title.clone(),
                    album_artist: album_artist_name.clone(),
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
                    bpm: track.bpm,
                    musical_key: track.key.clone(),
                    release_type: None,
                    explicit: track.explicit,
                    lyrics: None,
                    unsynced_lyrics: None,
                    album_replay_gain: None,
                    album_peak_amplitude: None,
                    track_replay_gain: None,
                    track_peak_amplitude: None,
                };

                if skip_existing {
                    let rel_flac = if let Some(dest) = destination {
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
                            format_download_path("{tracknumber} - {title}", &prelim_meta, ".flac");
                        let mut p = PathBuf::from(rel_album);
                        if !disc_dir_name.is_empty() {
                            p.push(disc_dir_name);
                        }
                        p.push(filename);
                        p
                    } else {
                        format_download_path(template, &prelim_meta, ".flac")
                    };
                    let dst_flac = item_output.join(&rel_flac);
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
                        continue;
                    }
                }

                progress_cb(format!(
                    "Track {}/{} · {}",
                    idx + 1,
                    total_tracks,
                    track.title
                ));

                let stream_info =
                    match get_playback_info(&http, &track.id, &token, item_quality, market).await {
                        Ok(s) => s,
                        Err(e) => {
                            progress_cb(format!("Playback unavailable for {}: {}", track.title, e));
                            all_tracks_ok = false;
                            break;
                        }
                    };

                let ext = if stream_info.mime_type.contains("mp4")
                    || stream_info.codec.to_lowercase().contains("mp4")
                    || stream_info.codec.to_lowercase().contains("aac")
                {
                    ".m4a"
                } else {
                    ".flac"
                };

                let mut staged_source = stage_dir.join(format!(".source-{}{}", track.id, ext));

                // Download stream with progress reporting
                let cb_clone = progress_cb.clone();
                let p_cb = move |pct: u8| {
                    cb_clone(format!("Download progress · {}%", pct));
                };

                if let Err(e) =
                    download_stream(&http, &stream_info, &staged_source, &cancel_flag, &p_cb).await
                {
                    if cancel_flag.load(Ordering::Relaxed) {
                        all_tracks_ok = false;
                        break;
                    }
                    progress_cb(format!("Download failed for track {}: {}", track.title, e));
                    all_tracks_ok = false;
                    break;
                }

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

                if let Err(e) = apply_audio_tags(&staged_source, &meta, cover_data.as_deref()) {
                    progress_cb(format!("Tagging failed for {}: {}", track.title, e));
                    all_tracks_ok = false;
                    break;
                }

                // Determine final relative path
                let target_path_in_stage = if let Some(dest) = destination {
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

                // Companion .lrc file if requested
                if lyrics_file {
                    if let Some(lrc_text) = lyrics_synced.as_ref().or(lyrics_unsynced.as_ref()) {
                        let _ = stream_download::write_lrc_file(&target_path_in_stage, lrc_text);
                    }
                }

                if let Some(fname) = target_path_in_stage.file_name().and_then(|f| f.to_str()) {
                    relative_track_filenames.push(fname.to_string());
                }

                // Delay pacing between tracks if enabled
                if download_delay && delay_min > 0.0 {
                    let wait_secs = {
                        let mut rng = rand::thread_rng();
                        rng.gen_range(delay_min..=delay_max.max(delay_min))
                    };
                    tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;
                }
            }

            if !all_tracks_ok {
                failed_count += 1;
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
            let mut published = match publish_staged_files(&stage_dir, &item_output) {
                Ok(p) => p,
                Err(e) => {
                    let _ = fs::remove_dir_all(&stage_dir);
                    failed_count += 1;
                    progress_cb(format!("Publishing failed for {}: {}", release_title, e));
                    continue;
                }
            };
            published.extend(existing_published_files);

            let _ = fs::remove_dir_all(&stage_dir);

            // Update DB queue
            let now_iso = chrono::Utc::now().to_rfc3339();
            let _ = conn
                .execute(
                    "UPDATE queue SET decision = 'downloaded', approved = 0, updated = ? WHERE id = ?",
                    (now_iso.as_str(), ident.as_str()),
                )
                .await;

            // Index into local_files
            for p in published {
                let p_str = p.to_string_lossy().to_string();
                for r in &roots {
                    let r_path = Path::new(&r.root);
                    if p.starts_with(r_path) {
                        if let Ok(file_meta) = fs::metadata(&p) {
                            let size = file_meta.len() as i64;
                            let mtime = file_meta
                                .modified()
                                .ok()
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_nanos() as i64)
                                .unwrap_or(0);

                            if let Ok(meta_obj) = crate::scanner::read_audio_metadata(&p) {
                                let meta_str = serde_json::to_string(&meta_obj).unwrap_or_default();
                                let _ = conn.execute(
                                    "INSERT OR REPLACE INTO local_files (path, root, size, mtime, metadata, present) VALUES (?, ?, ?, ?, ?, 1)",
                                    (p_str.as_str(), r.root.as_str(), size, mtime, meta_str.as_str()),
                                ).await;
                            }
                        }
                        break;
                    }
                }
            }

            completed_count += 1;
        }

        db.bump_revision();
        if failed_count > 0 && !cancel_flag.load(Ordering::Relaxed) {
            return Err(format!("Downloaded {completed_count} releases; {failed_count} failed and remain queued. See Activity for details."));
        }
        Ok(completed_count)
    }
}
