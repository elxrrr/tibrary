//! Publish a completed file without replacing another file at its destination.
use std::{io, path::Path};

/// Source and destination must be on the same filesystem. On macOS an exclusive
/// rename also works on removable filesystems that do not support hard links.
/// Success consumes the stage (best effort on the hard-link fallback).
pub fn publish_staged(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Source path contains a NUL byte",
            )
        })?;
        let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Destination path contains a NUL byte",
            )
        })?;
        // SAFETY: Both paths are owned, NUL-terminated C strings valid throughout
        // the call. RENAME_EXCL rejects every existing destination atomically.
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::hard_link(source, target)?;
        // Once publication succeeds, a cleanup error must not misreport the
        // destination as absent. Its enclosing stage directory can retry cleanup.
        let _ = std::fs::remove_file(source);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_directory() -> std::path::PathBuf {
        let temp =
            std::env::temp_dir().join(format!("exclusive-publication-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp).unwrap();
        temp
    }

    #[test]
    fn successful_publication_moves_complete_bytes_and_missing_source_publishes_nothing() {
        let temp = temp_directory();
        let stage = temp.join("stage.flac");
        let target = temp.join("published.flac");
        fs::write(&stage, crate::stream_download::MINIMAL_FLAC).unwrap();
        publish_staged(&stage, &target).unwrap();
        assert!(!stage.exists());
        assert_eq!(
            fs::read(&target).unwrap(),
            crate::stream_download::MINIMAL_FLAC
        );
        let missing_target = temp.join("missing.flac");
        assert!(publish_staged(&stage, &missing_target).is_err());
        assert!(!missing_target.exists());
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn files_directories_and_dangling_symlinks_are_never_replaced() {
        let temp = temp_directory();
        let stage = temp.join("stage.flac");
        let file = temp.join("existing.flac");
        let directory = temp.join("existing-directory");
        fs::write(&stage, b"New audio").unwrap();
        fs::write(&file, b"Existing audio").unwrap();
        fs::create_dir_all(&directory).unwrap();
        for target in [&file, &directory] {
            assert!(publish_staged(&stage, target).is_err());
            assert_eq!(fs::read(&stage).unwrap(), b"New audio");
        }
        assert_eq!(fs::read(&file).unwrap(), b"Existing audio");
        assert!(directory.is_dir());
        #[cfg(unix)]
        {
            let link = temp.join("dangling.flac");
            std::os::unix::fs::symlink(temp.join("missing"), &link).unwrap();
            assert!(publish_staged(&stage, &link).is_err());
            assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
            assert_eq!(fs::read(&stage).unwrap(), b"New audio");
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn concurrent_publications_have_one_winner_and_preserve_the_losing_stage() {
        let temp = temp_directory();
        let target = temp.join("song.flac");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for bytes in [b"First recording", b"Other recording"] {
            let stage = temp.join(format!("{}.flac", uuid::Uuid::new_v4()));
            fs::write(&stage, bytes).unwrap();
            let target = target.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let success = publish_staged(&stage, &target).is_ok();
                if success {
                    assert!(!stage.exists());
                    assert_eq!(fs::read(target).unwrap(), bytes);
                } else {
                    assert_eq!(fs::read(stage).unwrap(), bytes);
                }
                success
            }));
        }
        let winners = workers
            .into_iter()
            .map(|worker| worker.join().unwrap() as usize)
            .sum::<usize>();
        assert_eq!(winners, 1);
        fs::remove_dir_all(temp).unwrap();
    }
}
