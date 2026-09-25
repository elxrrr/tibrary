use lofty::file::{AudioFile, TaggedFileExt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqaAuditResult {
    pub path: String,
    pub status: String,
    pub detected: bool,
    pub bits: Option<u8>,
    pub rate: Option<u32>,
    pub original_rate: Option<u32>,
    pub studio: Option<bool>,
    pub evidence: String,
}

/// Detect the established AudioAuditor / MQA-identifier repeated 36-bit stereo signal.
/// `samples` contains `left ^ right` 32-bit unsigned integers.
pub fn mqa_signal(samples: &[u32]) -> Option<(u32, bool, u8, usize)> {
    for bit in 16..24 {
        let mut window = 0u64;
        let mut hits = BTreeMap::<u32, Vec<u32>>::new();
        let mut next = 0usize;
        for index in 0..samples.len() {
            window = ((window << 1) | u64::from((samples[index] >> bit) & 1)) & 0xfffffffff;
            if index < 35 || index < next || window != 0xBE0498C88 || index + 34 > samples.len() {
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
                return Some((
                    rate,
                    values.iter().all(|v| *v > 8),
                    bit as u8,
                    values.len(),
                ));
            }
        }
    }
    None
}

pub fn audit_file(path: &Path) -> MqaAuditResult {
    let path_str = path.display().to_string();
    let mut result = MqaAuditResult {
        path: path_str.clone(),
        status: "No signal found".to_string(),
        detected: false,
        bits: None,
        rate: None,
        original_rate: None,
        studio: None,
        evidence: "No MQA signal in the first 3 seconds".to_string(),
    };

    // 1. Inspect tags via lofty
    if let Ok(tagged_file) = lofty::probe::Probe::open(path).and_then(|p| p.read()) {
        let props = tagged_file.properties();
        result.bits = props.bit_depth();
        result.rate = props.sample_rate();

        if let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) {
            let mut encoder = String::new();
            let mut original_sample_rate = String::new();

            for item in tag.items() {
                let key_str = item
                    .key()
                    .map_key(lofty::tag::TagType::VorbisComments)
                    .or_else(|| item.key().map_key(lofty::tag::TagType::Id3v2))
                    .map(|s| s.to_lowercase())
                    .unwrap_or_else(|| format!("{:?}", item.key()).to_lowercase());
                if key_str == "encoder" || key_str == "mqaencoder" {
                    if let Some(val) = item.value().text() {
                        encoder.push_str(val);
                        encoder.push(' ');
                    }
                }
                if key_str == "originalsamplerate" {
                    if let Some(val) = item.value().text() {
                        original_sample_rate = val.trim().to_string();
                    }
                }
            }

            if encoder.to_lowercase().contains("mqa") {
                result.status = "MQA tags".to_string();
                result.detected = true;
                result.evidence = "MQA encoder metadata; audio signal not confirmed".to_string();
            } else if !original_sample_rate.is_empty() {
                result.status = "Metadata clue".to_string();
                result.evidence = "Original sample rate tag alone does not establish MQA".to_string();
            }

            if let Ok(rate) = original_sample_rate.parse::<u32>() {
                result.original_rate = Some(rate);
            }
        }
    }

    // 2. Scan audio PCM samples if FLAC
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    if ext == "flac" {
        if let Ok(file) = File::open(path) {
            if let Ok(mut reader) = claxon::FlacReader::new(file) {
                let streaminfo = reader.streaminfo();
                result.bits = Some(streaminfo.bits_per_sample as u8);
                result.rate = Some(streaminfo.sample_rate);

                if streaminfo.channels != 2 {
                    result.evidence.push_str(" · non-stereo file");
                    return result;
                }

                let max_frames = (streaminfo.sample_rate as usize) * 3;
                let shift = 32 - streaminfo.bits_per_sample;

                let mut samples = Vec::with_capacity(max_frames);
                let mut sample_iter = reader.samples();

                for _ in 0..max_frames {
                    let left = match sample_iter.next() {
                        Some(Ok(s)) => (s as u32) << shift,
                        _ => break,
                    };
                    let right = match sample_iter.next() {
                        Some(Ok(s)) => (s as u32) << shift,
                        _ => break,
                    };
                    samples.push(left ^ right);
                }

                if let Some((rate, studio, bit, hits)) = mqa_signal(&samples) {
                    result.status = "MQA signal".to_string();
                    result.detected = true;
                    result.original_rate = Some(rate);
                    result.studio = Some(studio);
                    result.evidence = format!(
                        "Repeated 36-bit stereo signal · bit {} · {} frames",
                        bit, hits
                    );
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mqa_signal_detection() {
        // Construct a synthetic 36-bit sync signal matching MqaDetector / AudioAuditor:
        // Sync: 0xBE0498C88 (36 bits)
        // bit 20 of left ^ right
        let sync_pattern = 0xBE0498C88u64;
        let mut samples = vec![0u32; 350];
        let bit = 18;

        for rep in 0..3 {
            let offset = rep * 100;
            // Write 36-bit sync word
            for i in 0..36 {
                let bit_val = ((sync_pattern >> (35 - i)) & 1) as u32;
                samples[offset + i] |= bit_val << bit;
            }
            // Code field is at (sync_end + 3..sync_end + 7)
            let sync_end = offset + 35;
            let code = 0b0011u32; // rate code
            for (j, i) in (3..7).enumerate() {
                let b = (code >> (3 - j)) & 1;
                samples[sync_end + i] |= b << bit;
            }
            // Provenance field is at (sync_end + 29..sync_end + 34)
            let prov = 0b10101u32; // > 8 -> studio
            for (j, i) in (29..34).enumerate() {
                let b = (prov >> (4 - j)) & 1;
                samples[sync_end + i] |= b << bit;
            }
        }

        let detected = mqa_signal(&samples);
        assert!(detected.is_some(), "Expected MQA sync detection");
        let (rate, studio, det_bit, hits) = detected.unwrap();
        assert_eq!(det_bit, bit as u8);
        assert!(hits >= 3);
        assert!(studio);
        assert!(rate > 0);
    }
}
