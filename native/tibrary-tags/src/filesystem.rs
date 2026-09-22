//! Read-only inventory and cancellable staging I/O. Never publishes or removes files.
use crate::{error, run_native};
use pyo3::{prelude::*, types::PyBytes};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

pub type InventoryEntry = (PathBuf, u64, i64);

#[cfg(unix)]
fn modified_ns(meta: &fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    meta.mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(meta.mtime_nsec())
}
#[cfg(not(unix))]
fn modified_ns(meta: &fs::Metadata) -> i64 {
    use std::time::UNIX_EPOCH;
    match meta
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
    {
        Ok(d) => d.as_nanos().min(i64::MAX as u128) as i64,
        Err(e) => -(e.duration().as_nanos().min(i64::MAX as u128) as i64),
    }
}

#[pyfunction]
pub fn inventory(
    py: Python<'_>,
    root: PathBuf,
    extensions: HashSet<String>,
    cancelled: Py<PyAny>,
    progress: Py<PyAny>,
) -> PyResult<(Vec<InventoryEntry>, bool)> {
    run_native(py, move || {
        // Root is resolved by the service. Never traverse directory/file symlinks.
        if !fs::symlink_metadata(&root)?.is_dir() {
            return Err(std::io::Error::other("Choose an available library folder").into());
        }
        let mut pending = vec![root];
        let mut result = Vec::new();
        let mut last_report = Instant::now() - Duration::from_secs(1);
        let mut visited = 0usize;
        while let Some(directory) = pending.pop() {
            if Python::attach(|py| cancelled.call0(py)?.extract::<bool>(py))? {
                return Ok((result, false));
            }
            if last_report.elapsed() >= Duration::from_millis(250) {
                Python::attach(|py| {
                    progress.call1(py, (format!("Checking folder · {}", directory.display()),))
                })?;
                last_report = Instant::now();
            }
            // Avoid retaining DirEntry handles across directories, or following a
            // directory replaced by a symlink after it was queued.
            if !fs::symlink_metadata(&directory)?.is_dir() {
                return Err(std::io::Error::other(
                    "Library folder changed during scan; refresh again",
                )
                .into());
            }
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                visited += 1;
                if visited.is_multiple_of(128)
                    && Python::attach(|py| cancelled.call0(py)?.extract::<bool>(py))?
                {
                    return Ok((result, false));
                }
                let kind = entry.file_type()?;
                if kind.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if kind.is_dir() {
                    pending.push(path);
                    continue;
                }
                if !kind.is_file() {
                    continue;
                }
                let extension = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !extensions.contains(&extension) {
                    continue;
                }
                let meta = entry.metadata()?;
                if meta.file_type().is_symlink() {
                    continue;
                }
                result.push((path, meta.len(), modified_ns(&meta)));
            }
        }
        Ok((result, true))
    })
}

fn audio_offset(file: &mut File, check: &Py<PyAny>) -> PyResult<u64> {
    let mut header = [0u8; 4];
    file.read_exact(&mut header)?;
    if &header != b"fLaC" {
        return Err(error("Unsupported FLAC header"));
    }
    let length = file.metadata()?.len();
    loop {
        Python::attach(|py| check.call0(py))?;
        file.read_exact(&mut header)?;
        let size = u32::from_be_bytes([0, header[1], header[2], header[3]]);
        let end = file
            .stream_position()?
            .checked_add(u64::from(size))
            .ok_or_else(|| error("Invalid FLAC metadata"))?;
        if end > length {
            return Err(error("Truncated FLAC metadata"));
        }
        file.seek(SeekFrom::Start(end))?;
        if header[0] & 128 != 0 {
            return Ok(end);
        }
    }
}

#[pyfunction]
pub fn copy_flac_verified(
    py: Python<'_>,
    source: PathBuf,
    destination: PathBuf,
    check_cancelled: Py<PyAny>,
) -> PyResult<Py<PyBytes>> {
    let digest = run_native(py, move || {
        Python::attach(|py| check_cancelled.call0(py))?;
        let mut input = File::open(&source)?;
        let audio_start = audio_offset(&mut input, &check_cancelled)?;
        input.rewind()?;
        // Caller supplies its own empty mkstemp file; refuse all other destinations.
        let target = fs::symlink_metadata(&destination)?;
        if !target.is_file() || target.len() != 0 {
            return Err(error("Staging destination must be an empty regular file"));
        }
        let mut options = OpenOptions::new();
        options.write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut output = options.open(&destination)?;
        if output.metadata()?.len() != 0 {
            return Err(error("Staging destination changed; original retained"));
        }
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut position = 0u64;
        loop {
            Python::attach(|py| check_cancelled.call0(py))?;
            let size = input.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            output.write_all(&buffer[..size])?;
            let start = audio_start.saturating_sub(position).min(size as u64) as usize;
            hasher.update(&buffer[start..size]);
            position += size as u64;
        }
        Ok(hasher.finalize().to_vec())
    })?;
    Ok(PyBytes::new(py, &digest).unbind())
}
