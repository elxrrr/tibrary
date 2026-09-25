use crate::db::TursoDb;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

pub struct DownloadManager;

impl DownloadManager {
    pub async fn run_downloads(
        db: &TursoDb,
        app: &AppHandle,
        cancel_flag: Arc<AtomicBool>,
        _job_id: &str,
        progress_cb: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<usize, String> {
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
                items.push(json!({
                    "id": id,
                    "release": release,
                }));
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

        let layout = db.get_preference("organisation").await?.unwrap_or_else(|| {
            json!({
                "template": "{album_artist}/{album}/{track_number} {title}"
            })
        });

        let request = json!({
            "output": output_path.to_string_lossy().to_string(),
            "settings": settings,
            "layout": layout,
            "items": items,
        });

        // Locate python runtime and download_bridge.py
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir.join("../..");
        let python_bin = repo_root.join(".venv/bin/python");
        let bridge_script = repo_root.join("app/library_manager/download_bridge.py");

        if !python_bin.exists() {
            return Err("Python runtime not found in .venv. Ensure environment is set up.".to_string());
        }
        if !bridge_script.exists() {
            return Err("download_bridge.py not found.".to_string());
        }

        let mut child = Command::new(&python_bin)
            .arg("-u")
            .arg(&bridge_script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PYTHONPATH", repo_root.join("app"))
            .env("PYTHONUNBUFFERED", "1")
            .spawn()
            .map_err(|e| format!("Failed to spawn download process: {}", e))?;

        let pid = child.id();

        // Spawn cancellation watcher
        let cancel_flag_watcher = cancel_flag.clone();
        std::thread::spawn(move || {
            while !cancel_flag_watcher.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .output();
        });

        let mut stdin = child.stdin.take().ok_or("Failed to open stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to open stdout")?;

        // Send request JSON
        let req_bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        stdin.write_all(&req_bytes).map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;
        drop(stdin); // finished writing request

        let lines = BufReader::new(stdout).lines();
        let mut completed_count = 0;
        let roots = db.list_roots("GB").await?;

        for line in lines {
            if cancel_flag.load(Ordering::Relaxed) {
                break;
            }
            let Ok(line) = line else { break };

            if let Ok(evt) = serde_json::from_str::<Value>(&line) {
                let event_type = evt.get("event").and_then(|v| v.as_str()).unwrap_or("");
                match event_type {
                    "log" => {
                        if let Some(msg) = evt.get("message").and_then(|v| v.as_str()) {
                            progress_cb(msg.to_string());
                        }
                    }
                    "completed" => {
                        completed_count += 1;
                        if let Some(ident) = evt.get("id").and_then(|v| v.as_str()) {
                            let now_iso = chrono::Utc::now().to_rfc3339();
                            let _ = conn.execute(
                                "UPDATE queue SET decision = 'downloaded', approved = 0, updated = ? WHERE id = ?",
                                (now_iso.as_str(), ident),
                            ).await;
                        }
                        if let Some(files) = evt.get("files").and_then(|v| v.as_array()) {
                            for f in files {
                                if let Some(p_str) = f.as_str() {
                                    let path = Path::new(p_str);
                                    if path.exists() {
                                        for r in &roots {
                                            let r_path = Path::new(&r.root);
                                            if path.starts_with(r_path) {
                                                if let Ok(meta) = std::fs::metadata(path) {
                                                    let size = meta.len() as i64;
                                                    let mtime = meta.modified()
                                                        .ok()
                                                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                                        .map(|d| d.as_nanos() as i64)
                                                        .unwrap_or(0);
                                                    if let Ok(meta_obj) = crate::scanner::read_audio_metadata(path) {
                                                        let meta_str = serde_json::to_string(&meta_obj).unwrap_or_default();
                                                        let _ = conn.execute(
                                                            "INSERT OR REPLACE INTO local_files (path, root, size, mtime, metadata, present) VALUES (?, ?, ?, ?, ?, 1)",
                                                            (p_str, r.root.as_str(), size, mtime, meta_str.as_str()),
                                                        ).await;
                                                    }
                                                }
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "auth" => {
                        if let Some(url) = evt.get("url").and_then(|v| v.as_str()) {
                            let _ = app.emit("backend-event", json!({
                                "event": "authentication",
                                "url": url
                            }));
                        }
                    }
                    "error" => {
                        if let Some(msg) = evt.get("message").and_then(|v| v.as_str()) {
                            progress_cb(format!("Error: {}", msg));
                        }
                    }
                    _ => {}
                }
            }
        }

        let _ = child.wait();
        Ok(completed_count)
    }
}
