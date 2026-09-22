//! Small in-process boundary: native comments stay native; I/O releases Python's GIL.
use lofty::{
    config::{ParseOptions, ParsingMode, WriteOptions},
    file::AudioFile,
    flac::FlacFile,
    ogg::{tag::VorbisComments, OggPictureStorage},
    picture::Picture,
};
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyDict, PyList},
};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Seek,
    path::PathBuf,
};

type Tags = BTreeMap<String, Vec<String>>;
fn error(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}
// Turn unexpected parser panics into ordinary job failures, not an unhandled
// Python BaseException that could leave a background job marked as running.
fn run_native<T: Send>(
    py: Python<'_>,
    operation: impl FnOnce() -> PyResult<T> + Send,
) -> PyResult<T> {
    py.detach(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).unwrap_or_else(|_| {
            Err(error(
                "Native audio operation failed; inspect this file before retrying",
            ))
        })
    })
}
fn options() -> ParseOptions {
    ParseOptions::new()
        .parsing_mode(ParsingMode::Strict)
        .implicit_conversions(false)
}

#[pyfunction]
#[pyo3(signature = (path, pictures=true))]
fn read_flac(py: Python<'_>, path: PathBuf, pictures: bool) -> PyResult<Py<PyDict>> {
    let (tags, info, covers) = run_native(py, move || -> PyResult<_> {
        let mut input = File::open(path)?;
        let audio =
            FlacFile::read_from(&mut input, options().read_cover_art(pictures)).map_err(error)?;
        let mut tags = Tags::new();
        if let Some(comments) = audio.vorbis_comments() {
            for (key, value) in comments.items() {
                tags.entry(key.to_ascii_lowercase())
                    .or_default()
                    .push(value.to_owned());
            }
        }
        let p = audio.properties();
        let info = (
            p.duration().as_secs_f64(),
            p.sample_rate(),
            p.bit_depth(),
            p.channels(),
        );
        let covers: Vec<Vec<u8>> = audio
            .pictures()
            .iter()
            .map(|(p, i)| p.as_flac_bytes(*i, false))
            .collect();
        Ok((tags, info, covers))
    })?;
    let result = PyDict::new(py);
    result.set_item("tags", tags)?;
    result.set_item("info", info)?;
    let list = PyList::empty(py);
    for data in covers {
        list.append(PyBytes::new(py, &data))?;
    }
    result.set_item("pictures", list)?;
    Ok(result.unbind())
}

#[pyfunction]
fn write_flac(
    py: Python<'_>,
    path: PathBuf,
    changes: Tags,
    pictures: Option<Vec<Vec<u8>>>,
) -> PyResult<()> {
    // Validate inputs before opening for writes. Only callers' staging files may be used.
    for key in changes.keys() {
        if key.is_empty() || !key.bytes().all(|c| (0x20..=0x7d).contains(&c) && c != b'=') {
            return Err(error("Invalid Vorbis comment key"));
        }
    }
    run_native(py, move || {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut audio = FlacFile::read_from(&mut file, options()).map_err(error)?;
        if audio.id3v2().is_some() {
            return Err(error(
                "Legacy ID3 block in FLAC; retained without modification",
            ));
        }
        if audio.vorbis_comments().is_none() {
            audio.set_vorbis_comments(VorbisComments::new());
        }
        let tags = audio.vorbis_comments_mut().unwrap();
        // VorbisComments::remove reorders unrelated entries; artist credit order matters.
        // Rebuild only the item vector in stable order, preserving native pictures/vendor.
        let existing: Vec<_> = tags.take_items().collect();
        for (key, value) in &existing {
            if !changes.contains_key(&key.to_ascii_lowercase()) {
                tags.push(key.clone(), value.clone());
            }
        }
        for (key, values) in changes {
            let spelling = existing
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&key))
                .map(|(k, _)| k.clone())
                .unwrap_or(key.to_ascii_uppercase());
            for value in values {
                tags.push(spelling.clone(), value);
            }
        }
        if let Some(pictures) = pictures {
            let decoded = pictures
                .iter()
                .map(|data| {
                    Picture::from_flac_bytes(data, false, ParsingMode::Strict).map_err(error)
                })
                .collect::<PyResult<Vec<_>>>()?;
            audio.remove_pictures();
            for (picture, info) in decoded {
                audio.insert_picture(picture, Some(info)).map_err(error)?;
            }
        }
        file.rewind()?;
        save_flac_metadata(&audio, &mut file)?;
        Ok(())
    })
}

#[pymodule]
fn _lofty(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(digest_flac, m)?)?;
    m.add_function(wrap_pyfunction!(mqa_signal, m)?)?;
    m.add("LOFTY_VERSION", "0.25.3")?;
    m.add_function(wrap_pyfunction!(read_flac, m)?)?;
    m.add_function(wrap_pyfunction!(write_flac, m)?)?;
    m.add_function(wrap_pyfunction!(read_other, m)?)?;
    m.add_function(wrap_pyfunction!(write_mp4, m)?)?;
    Ok(())
}

use lofty::{
    file::TaggedFileExt,
    mp4::{Atom, AtomData, AtomIdent, Ilst, Mp4File},
    picture::PictureInformation,
    probe::Probe,
    tag::{Accessor, ItemKey, TagType},
};
use std::borrow::Cow;

fn mp4_ident(key: &str) -> AtomIdent<'static> {
    let fourcc = match key {
        "title" => Some(*b"\xa9nam"),
        "album" => Some(*b"\xa9alb"),
        "artist" => Some(*b"\xa9ART"),
        "albumartist" => Some(*b"aART"),
        "date" => Some(*b"\xa9day"),
        "genre" => Some(*b"\xa9gen"),
        "composer" => Some(*b"\xa9wrt"),
        "copyright" => Some(*b"cprt"),
        "bpm" => Some(*b"tmpo"),
        "lyrics" => Some(*b"\xa9lyr"),
        "url" => Some(*b"\xa9url"),
        "explicit" => Some(*b"rtng"),
        _ => None,
    };
    if let Some(code) = fourcc {
        return AtomIdent::Fourcc(code);
    }
    let name = match key {
        "label" => "LABEL",
        "isrc" => "ISRC",
        "upc" => "UPC",
        "originaldate" => "ORIGINALDATE",
        "releasetype" => "MusicBrainz Album Type",
        _ => key,
    };
    AtomIdent::Freeform {
        mean: Cow::Borrowed("com.apple.iTunes"),
        name: Cow::Owned(name.to_owned()),
    }
}

#[pyfunction]
#[pyo3(signature=(path, pictures=true))]
fn read_other(py: Python<'_>, path: PathBuf, pictures: bool) -> PyResult<Py<PyDict>> {
    let (tags, info, covers) = run_native(py, move || -> PyResult<_> {
        let mut tags = Tags::new();
        let audio = Probe::open(&path)
            .map_err(error)?
            .options(options().read_cover_art(pictures))
            .read()
            .map_err(error)?;
        let prop = audio.properties();
        let info = (
            prop.duration().as_secs_f64(),
            prop.sample_rate().unwrap_or(0),
            prop.bit_depth().unwrap_or(0),
            prop.channels().unwrap_or(0),
        );
        let mut covers = Vec::new();
        if let Some(tag) = audio.primary_tag().or_else(|| audio.first_tag()) {
            for item in tag.items() {
                if let Some(key) = if item.key() == ItemKey::IntegerBpm {
                    Some("BPM")
                } else {
                    item.key().map_key(TagType::VorbisComments)
                } {
                    if let Some(value) = item.value().text().or_else(|| item.value().locator()) {
                        tags.entry(key.to_ascii_lowercase())
                            .or_default()
                            .push(value.to_owned());
                    }
                }
            }
            for p in tag.pictures() {
                let info = PictureInformation::from_picture(p).unwrap_or_default();
                covers.push(p.as_flac_bytes(info, false));
            }
        }
        // Preserve arbitrary textual MP4 freeforms, including our durable recording IDs.
        if audio.file_type() == lofty::file::FileType::Mp4 {
            let file = Mp4File::read_from(&mut File::open(&path)?, options().read_cover_art(false))
                .map_err(error)?;
            if let Some(ilst) = file.ilst() {
                for atom in ilst {
                    if let AtomIdent::Freeform { mean, name } = atom.ident() {
                        if mean.as_ref() != "com.apple.iTunes" {
                            continue;
                        }
                        let key = if name.as_ref() == "MusicBrainz Album Type" {
                            "releasetype".to_owned()
                        } else {
                            name.to_ascii_lowercase()
                        };
                        let values: Vec<String> = atom
                            .data()
                            .filter_map(|data| match data {
                                AtomData::UTF8(v) | AtomData::UTF16(v) => Some(v.clone()),
                                _ => None,
                            })
                            .collect();
                        if !values.is_empty() {
                            tags.insert(key, values);
                        }
                    }
                }
            }
        }
        Ok((tags, info, covers))
    })?;
    let result = PyDict::new(py);
    result.set_item("tags", tags)?;
    result.set_item("info", info)?;
    let list = PyList::empty(py);
    for data in covers {
        list.append(PyBytes::new(py, &data))?;
    }
    result.set_item("pictures", list)?;
    Ok(result.unbind())
}

#[pyfunction]
fn write_mp4(
    py: Python<'_>,
    path: PathBuf,
    changes: Tags,
    pictures: Option<Vec<Vec<u8>>>,
) -> PyResult<()> {
    run_native(py, move || {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut audio = Mp4File::read_from(&mut file, options()).map_err(error)?;
        if audio.ilst().is_none() {
            audio.set_ilst(Ilst::new());
        }
        let tags = audio.ilst_mut().unwrap();
        for (key, values) in changes {
            if ["tracknumber", "tracktotal", "discnumber", "disctotal"].contains(&key.as_str()) {
                let value = values
                    .first()
                    .map(|v| v.parse::<u16>().map_err(error))
                    .transpose()?
                    .map(u32::from);
                match (key.as_str(), value) {
                    ("tracknumber", Some(v)) => tags.set_track(v),
                    ("tracktotal", Some(v)) => tags.set_track_total(v),
                    ("discnumber", Some(v)) => tags.set_disk(v),
                    ("disctotal", Some(v)) => tags.set_disk_total(v),
                    ("tracknumber", None) => tags.remove_track(),
                    ("tracktotal", None) => tags.remove_track_total(),
                    ("discnumber", None) => tags.remove_disk(),
                    ("disctotal", None) => tags.remove_disk_total(),
                    _ => (),
                }
                continue;
            }
            if key == "bpm" {
                let integer = AtomIdent::Fourcc(*b"tmpo");
                let decimal = AtomIdent::Freeform {
                    mean: Cow::Borrowed("com.apple.iTunes"),
                    name: Cow::Borrowed("BPM"),
                };
                tags.retain(|atom| atom.ident() != &integer && atom.ident() != &decimal);
                if let Some(value) = values.first() {
                    let bpm = value.parse::<f64>().map_err(error)?;
                    if !bpm.is_finite() || bpm <= 0.0 {
                        return Err(error("Invalid BPM"));
                    }
                    if bpm.fract() == 0.0 && bpm <= u16::MAX as f64 {
                        tags.insert(Atom::new(integer, AtomData::UnsignedInteger(bpm as u32)));
                    } else {
                        tags.insert(Atom::new(decimal, AtomData::UTF8(value.clone())));
                    }
                }
                continue;
            }
            let ident = mp4_ident(&key);
            tags.retain(|atom| atom.ident() != &ident);
            let values = values
                .into_iter()
                .map(|value| {
                    if key == "explicit" {
                        Ok(AtomData::UnsignedInteger(
                            value.parse::<u16>().map_err(error)?.into(),
                        ))
                    } else {
                        Ok(AtomData::UTF8(value))
                    }
                })
                .collect::<PyResult<Vec<_>>>()?;
            if let Some(atom) = Atom::from_collection(ident, values) {
                tags.insert(atom);
            }
        }
        if let Some(pictures) = pictures {
            let decoded = pictures
                .iter()
                .map(|data| {
                    Picture::from_flac_bytes(data, false, ParsingMode::Strict).map_err(error)
                })
                .collect::<PyResult<Vec<_>>>()?;
            tags.remove_pictures();
            for (picture, _) in decoded {
                tags.insert_picture(picture);
            }
        }
        file.rewind()?;
        audio
            .save_to(&mut file, WriteOptions::default())
            .map_err(error)?;
        Ok(())
    })
}

use sha2::{Digest, Sha256};
use std::io::{Cursor, Read, SeekFrom, Write};

/// Hash only encoded FLAC frames, with bounded memory and cancellation at each MiB.
#[pyfunction]
fn digest_flac(py: Python<'_>, path: PathBuf, check_cancelled: Py<PyAny>) -> PyResult<Py<PyBytes>> {
    let digest = run_native(py, move || -> PyResult<Vec<u8>> {
        let mut file = File::open(path)?;
        let mut header = [0u8; 4];
        file.read_exact(&mut header)?;
        if &header != b"fLaC" {
            return Err(error("Unsupported FLAC header"));
        }
        let length = file.metadata()?.len();
        loop {
            Python::attach(|py| check_cancelled.call0(py))?;
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
                break;
            }
        }
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            Python::attach(|py| check_cancelled.call0(py))?;
            let size = file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            hasher.update(&buffer[..size]);
        }
        Ok(hasher.finalize().to_vec())
    })?;
    Ok(PyBytes::new(py, &digest).unbind())
}

/// Detect the established MQA audit's repeated 36-bit stereo signal in PCM.
#[pyfunction]
fn mqa_signal(py: Python<'_>, pcm: Vec<u8>) -> PyResult<Option<(u32, bool, u8, usize)>> {
    if !pcm.len().is_multiple_of(8) {
        return Err(error("Expected interleaved stereo 32-bit PCM"));
    }
    run_native(py, move || {
        let samples: Vec<u32> = pcm
            .as_chunks::<8>()
            .0
            .iter()
            .map(|frame| {
                u32::from_le_bytes(frame[0..4].try_into().unwrap())
                    ^ u32::from_le_bytes(frame[4..8].try_into().unwrap())
            })
            .collect();
        for bit in 16..24 {
            let mut window = 0u64;
            let mut hits = BTreeMap::<u32, Vec<u32>>::new();
            let mut next = 0usize;
            for index in 0..samples.len() {
                window = ((window << 1) | u64::from((samples[index] >> bit) & 1)) & 0xfffffffff;
                if index < 35 || index < next || window != 0xBE0498C88 || index + 34 > samples.len()
                {
                    continue;
                }
                next = index + 36;
                let field = |start: usize, end: usize| {
                    (start..end).fold(0u32, |v, i| (v << 1) | ((samples[i] >> bit) & 1))
                };
                let code = field(index + 3, index + 7);
                let provenance = field(index + 29, index + 34);
                let values = hits.entry(code).or_default();
                values.push(provenance);
                if values.len() >= 3 {
                    let factor = 1u32 << ((code >> 1) & 7);
                    let rate = if code & 1 != 0 { 48000 } else { 44100 }
                        * factor
                        * if factor > 16 { 2 } else { 1 };
                    return Ok(Some((
                        rate,
                        values.iter().all(|v| *v > 8),
                        bit,
                        values.len(),
                    )));
                }
            }
        }
        Ok(None)
    })
}

/// Lofty encodes the metadata; our I/O layer reuses FLAC padding without rewriting audio.
/// This runs only on callers' temporary copies. Memory is bounded by metadata + 1 MiB.
fn save_flac_metadata(audio: &FlacFile, file: &mut File) -> PyResult<()> {
    file.rewind()?;
    let mut magic = [0; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"fLaC" {
        return Err(error("Unsupported FLAC header"));
    }
    let mut prefix = magic.to_vec();
    loop {
        let mut header = [0; 4];
        file.read_exact(&mut header)?;
        let size = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        if prefix.len() + size + 4 > 128 * 1024 * 1024 {
            return Err(error("FLAC metadata exceeds safety limit"));
        }
        prefix.extend_from_slice(&header);
        let start = prefix.len();
        prefix.resize(start + size, 0);
        file.read_exact(&mut prefix[start..])?;
        if header[0] & 128 != 0 {
            break;
        }
    }
    let old_length = prefix.len();
    let mut buffer = Cursor::new(prefix);
    audio
        .save_to(&mut buffer, WriteOptions::default())
        .map_err(error)?;
    let encoded = buffer.into_inner();
    // Remove padding blocks produced/retained by Lofty, then choose the exact available size.
    let mut blocks = Vec::<Vec<u8>>::new();
    let mut pos = 4;
    loop {
        if pos + 4 > encoded.len() {
            return Err(error("Invalid encoded FLAC metadata"));
        }
        let header = &encoded[pos..pos + 4];
        let size = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        if pos + 4 + size > encoded.len() {
            return Err(error("Truncated encoded FLAC metadata"));
        }
        if header[0] & 127 != 1 {
            let mut block = encoded[pos..pos + 4 + size].to_vec();
            block[0] &= 127;
            if header[0] & 127 == 4 {
                if let Some(comments) = audio.vorbis_comments() {
                    // Lofty's encoder omits empty values. Preserve them, including their
                    // position among repeated values, rather than silently dropping user tags.
                    if comments.items().any(|(_, v)| v.is_empty()) {
                        let mut data = Vec::new();
                        let vendor = comments.vendor().as_bytes();
                        data.extend((vendor.len() as u32).to_le_bytes());
                        data.extend(vendor);
                        data.extend((comments.items().len() as u32).to_le_bytes());
                        for (key, value) in comments.items() {
                            let item = format!("{key}={value}");
                            data.extend((item.len() as u32).to_le_bytes());
                            data.extend(item.as_bytes());
                        }
                        if data.len() > 0xffffff {
                            return Err(error("Vorbis comments exceed FLAC block limit"));
                        }
                        block = vec![4];
                        block.extend(&(data.len() as u32).to_be_bytes()[1..]);
                        block.extend(data);
                    }
                }
            }
            blocks.push(block);
        }
        pos += 4 + size;
        if header[0] & 128 != 0 {
            break;
        }
    }
    let compact_length = 4 + blocks.iter().map(Vec::len).sum::<usize>();
    let mut padding = if compact_length <= old_length
        && (compact_length == old_length || compact_length + 4 <= old_length)
    {
        old_length - compact_length
    } else {
        4096
    };
    while padding >= 4 {
        let size = (padding - 4).min(0xffffff);
        let mut block = vec![0; size + 4];
        block[0] = 1;
        block[1..4].copy_from_slice(&(size as u32).to_be_bytes()[1..]);
        blocks.push(block);
        padding -= size + 4;
    }
    if let Some(last) = blocks.last_mut() {
        last[0] |= 128;
    }
    let mut output = b"fLaC".to_vec();
    for block in blocks {
        output.extend(block);
    }
    if output.len() < old_length {
        return Err(error("Cannot safely reuse FLAC padding"));
    }
    let growth = (output.len() - old_length) as u64;
    if growth > 0 {
        let mut end = file.metadata()?.len();
        file.set_len(
            end.checked_add(growth)
                .ok_or_else(|| error("FLAC file too large"))?,
        )?;
        let mut chunk = vec![0; 1024 * 1024];
        while end > old_length as u64 {
            let size = (end - old_length as u64).min(chunk.len() as u64) as usize;
            let start = end - size as u64;
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(&mut chunk[..size])?;
            file.seek(SeekFrom::Start(start + growth))?;
            file.write_all(&chunk[..size])?;
            end = start;
        }
    }
    file.rewind()?;
    file.write_all(&output)?;
    Ok(())
}
