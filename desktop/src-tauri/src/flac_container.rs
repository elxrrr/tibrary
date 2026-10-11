//! Lossless container conversion. FFmpeg copies encoded FLAC packets; it never re-encodes.
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

struct ConversionFiles {
    staged: PathBuf,
    log: PathBuf,
}

impl Drop for ConversionFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.staged);
        let _ = std::fs::remove_file(&self.log);
    }
}

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
    // FFmpeg writes to a private stage. Publishing only after conversion prevents
    // a destination created concurrently from being overwritten or deleted.
    let parent = target.parent().ok_or("Invalid conversion destination")?;
    let id = uuid::Uuid::new_v4();
    let temporary = ConversionFiles {
        staged: parent.join(format!(".tibrary-remux-{id}.flac")),
        log: parent.join(format!(".tibrary-remux-{id}.log")),
    };
    let errors = std::fs::OpenOptions::new().write(true).create_new(true)
        .open(&temporary.log).map_err(|e| e.to_string())?;
    let result = async {
        let mut child = tokio::process::Command::new(ffmpeg_path())
            .kill_on_drop(true)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-n", "-i"])
            .arg(source).args(["-map", "0:a:0", "-vn", "-c:a", "copy", "-f", "flac"])
            .arg(&temporary.staged).stdin(Stdio::null()).stdout(Stdio::null()).stderr(errors)
            .spawn().map_err(|e| format!("Cannot run FFmpeg for lossless container conversion: {e}. Install FFmpeg and retry."))?;
        let start = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(120) {
                let _ = child.kill().await; let _ = child.wait().await;
                return Err("Lossless container conversion cancelled or timed out".to_string());
            }
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Conversion cancelled".into());
                    }
                    return crate::file_publication::publish_staged(&temporary.staged, target)
                        .map_err(|e| format!("Cannot publish converted audio without replacing an existing file: {e}"));
                },
                Ok(Some(_)) => {
                    let details = std::fs::read_to_string(&temporary.log).unwrap_or_default();
                    return Err(format!("Lossless container conversion failed: {}", details.chars().take(600).collect::<String>()));
                }
                Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(e) => { let _ = child.kill().await; let _ = child.wait().await; return Err(e.to_string()); }
            }
        }
    }.await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_conversion_and_existing_destination_leave_files_unchanged() {
        let temp = std::env::temp_dir().join(format!("remux-guards-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let source = temp.join("source.flac");
        let target = temp.join("target.flac");
        std::fs::write(&source, crate::stream_download::MINIMAL_FLAC).unwrap();
        assert!(remux_flac(&source, &target, &AtomicBool::new(true)).await.unwrap_err().contains("cancelled"));
        assert!(!target.exists());
        std::fs::write(&target, b"Existing audio").unwrap();
        assert!(remux_flac(&source, &target, &AtomicBool::new(false)).await.unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read(&source).unwrap(), crate::stream_download::MINIMAL_FLAC);
        assert_eq!(std::fs::read(&target).unwrap(), b"Existing audio");
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 2);
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[tokio::test]
    #[ignore = "Requires FFmpeg; run with cargo test flac_container::tests -- --include-ignored"]
    async fn remux_preserves_decoded_audio_and_unrelated_files_and_cleans_failed_stages() {
        let temp = std::env::temp_dir().join(format!("remux-audio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let source = temp.join("source.flac");
        let target = temp.join("target.flac");
        let user_log = target.with_extension("ffmpeg.log");
        std::fs::write(&source, crate::stream_download::MINIMAL_FLAC).unwrap();
        std::fs::write(&user_log, b"An existing user file").unwrap();
        remux_flac(&source, &target, &AtomicBool::new(false)).await.unwrap();
        let samples = |path: &Path| claxon::FlacReader::open(path).unwrap()
            .samples().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(samples(&source), samples(&target));
        assert_eq!(std::fs::read(&source).unwrap(), crate::stream_download::MINIMAL_FLAC);
        assert_eq!(std::fs::read(&user_log).unwrap(), b"An existing user file");
        std::fs::write(&source, b"Invalid audio").unwrap();
        let failed_target = temp.join("failed.flac");
        assert!(remux_flac(&source, &failed_target, &AtomicBool::new(false)).await.is_err());
        assert!(!failed_target.exists());
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 3);
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    #[ignore = "Requires FFmpeg; run with cargo test flac_container::tests -- --include-ignored"]
    async fn destination_created_during_conversion_is_never_deleted() {
        let temp = std::env::temp_dir().join(format!("remux-collision-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let source = temp.join("source.flac");
        let target = temp.join("target.flac");
        assert!(std::process::Command::new("mkfifo").arg(&source).status().unwrap().success());
        let cancel = AtomicBool::new(false);
        // FFmpeg cannot finish reading the pipe before the competing destination
        // is created. This exercises the publication race deterministically.
        let conversion = remux_flac(&source, &target, &cancel);
        let input = source.clone();
        let collision = async {
            std::fs::write(&target, b"Concurrent download").unwrap();
            tokio::task::spawn_blocking(move || std::fs::write(input, crate::stream_download::MINIMAL_FLAC))
                .await.unwrap().unwrap();
        };
        let (result, _) = tokio::join!(conversion, collision);
        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"Concurrent download");
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 2);
        std::fs::remove_dir_all(temp).unwrap();
    }
}
