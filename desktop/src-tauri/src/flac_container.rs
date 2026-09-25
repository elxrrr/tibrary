//! Lossless container conversion. FFmpeg copies encoded FLAC packets; it never re-encodes.
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub fn ffmpeg_path() -> PathBuf {
    for path in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"] {
        if Path::new(path).is_file() {
            return PathBuf::from(path);
        }
    }
    PathBuf::from("ffmpeg")
}

pub async fn remux_flac(source: &Path, target: &Path, cancel: &AtomicBool) -> Result<(), String> {
    if target.exists() {
        return Err("Conversion destination already exists".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Conversion cancelled".into());
    }
    let error_path = target.with_extension("ffmpeg.log");
    let errors = std::fs::File::create(&error_path).map_err(|e| e.to_string())?;
    let result = async {
        let mut child = Command::new(ffmpeg_path())
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-n", "-i"])
            .arg(source).args(["-map", "0:a:0", "-vn", "-c:a", "copy", "-f", "flac"])
            .arg(target).stdin(Stdio::null()).stdout(Stdio::null()).stderr(errors)
            .spawn().map_err(|e| format!("Cannot run FFmpeg for lossless container conversion: {e}. Install FFmpeg and retry."))?;
        let start = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(120) {
                let _ = child.kill(); let _ = child.wait();
                return Err("Lossless container conversion cancelled or timed out".to_string());
            }
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(_)) => {
                    let details = std::fs::read_to_string(&error_path).unwrap_or_default();
                    return Err(format!("Lossless container conversion failed: {}", details.chars().take(600).collect::<String>()));
                }
                Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(e) => { let _ = child.kill(); let _ = child.wait(); return Err(e.to_string()); }
            }
        }
    }.await;
    let _ = std::fs::remove_file(error_path);
    if result.is_err() {
        let _ = std::fs::remove_file(target);
    }
    result
}
