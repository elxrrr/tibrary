use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit, StreamCipher};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
use rand::Rng;
use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::db::TursoDb;

// ============================================================================
// CONSTANTS & KEYS
// ============================================================================

/// Master key used to decrypt Tidal security tokens (AES-256-CBC).
pub const MASTER_KEY_B64: &str = "UIlTTEMmmLfGowo/UC60x2H45W6MdGgTRfo/umg4754=";

/// Tidal PKCE client credentials
pub const CLIENT_ID_PKCE: &str = "6BDSRdpK9hqEBTgU";
pub const CLIENT_SECRET_PKCE: &str = "xeuPmY7nbpZ9IIbLAcQ93shka1VNheUAqN6IcszjTG8=";
pub const PKCE_REDIRECT_URI: &str = "https://tidal.com/android/login/auth";

/// Tidal Dolby Atmos client credentials
pub const ATMOS_CLIENT_ID: &str = "7m7Ap0JC9j1cOM3n";
pub const ATMOS_CLIENT_SECRET: &str = "vRAdA108tlvkJpTsGZS8rGZ7xTlbJ0qaZ2K9saEzsgY=";

pub const API_AUTH_TOKEN: &str = "https://auth.tidal.com/v1/oauth2/token";
pub const API_PKCE_AUTH: &str = "https://login.tidal.com/authorize";
pub const API_V1_BASE: &str = "https://api.tidal.com/v1";
pub const RESOURCES_BASE: &str = "https://resources.tidal.com";

// ============================================================================
// CRYPTOGRAPHY & DECRYPTION
// ============================================================================

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
type Aes128Ctr = ctr::Ctr64BE<aes::Aes128>;

/// Decrypts a Tidal security token into an AES-128 key and 8-byte nonce.
/// The token is base64-encoded, containing a 16-byte IV followed by ciphertext
/// encrypted with the master key using AES-256-CBC.
pub fn decrypt_security_token(security_token_b64: &str) -> Result<([u8; 16], [u8; 8]), String> {
    let master_key = STANDARD
        .decode(MASTER_KEY_B64)
        .map_err(|e| format!("Invalid master key base64: {}", e))?;
    if master_key.len() != 32 {
        return Err("Master key must be 32 bytes (AES-256)".to_string());
    }

    let raw = STANDARD
        .decode(security_token_b64.trim())
        .map_err(|e| format!("Invalid security token base64: {}", e))?;

    if raw.len() < 32 {
        return Err("Security token too short (< 32 bytes)".to_string());
    }

    let iv = &raw[..16];
    let ciphertext = &raw[16..];

    if ciphertext.len() % 16 != 0 {
        return Err(format!(
            "Ciphertext length {} is not a multiple of block size 16",
            ciphertext.len()
        ));
    }

    let mut buf = ciphertext.to_vec();
    let decryptor = Aes256CbcDec::new_from_slices(&master_key, iv)
        .map_err(|e| format!("Failed to init AES-256-CBC decryptor: {}", e))?;

    let decrypted = decryptor
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|e| format!("Failed to decrypt security token: {}", e))?;

    if decrypted.len() < 24 {
        return Err("Decrypted security token too short (< 24 bytes)".to_string());
    }

    let mut key = [0u8; 16];
    key.copy_from_slice(&decrypted[..16]);

    let mut nonce = [0u8; 8];
    nonce.copy_from_slice(&decrypted[16..24]);

    Ok((key, nonce))
}

/// Decrypts stream audio bytes in place using AES-128-CTR.
/// The 128-bit counter block starts with the 8-byte nonce as prefix (upper 64 bits)
/// and a 64-bit big-endian counter starting at 0.
pub fn decrypt_stream_bytes(
    data: &mut [u8],
    key: &[u8; 16],
    nonce: &[u8; 8],
) -> Result<(), String> {
    let mut iv = [0u8; 16];
    iv[..8].copy_from_slice(nonce);

    let mut cipher = Aes128Ctr::new_from_slices(key, &iv)
        .map_err(|e| format!("Failed to init AES-128-CTR cipher: {}", e))?;

    cipher.apply_keystream(data);
    Ok(())
}

// ============================================================================
// TIDAL AUTHENTICATION & SESSION
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TidalToken {
    pub token_type: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: f64,
    pub user_id: Option<String>,
    pub is_pkce: bool,
}

#[derive(Debug, Clone)]
pub struct PkceFlow {
    pub login_url: String,
    pub code_verifier: String,
    pub client_unique_key: String,
}

/// Creates a PKCE login URL and returns the state needed to complete verification.
pub fn create_pkce_flow() -> PkceFlow {
    let mut rng = rand::thread_rng();

    // 64-bit random client unique key in hex
    let client_unique_key: String = format!("{:016x}", rng.gen::<u64>());

    // 32 random bytes -> urlsafe base64 without padding for code verifier
    let mut verifier_bytes = [0u8; 32];
    rng.fill(&mut verifier_bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);

    // Code challenge = SHA256(verifier) in urlsafe base64 without padding
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    let login_url = format!(
        "{}?response_type=code&redirect_uri={}&client_id={}&lang=EN&appMode=android&client_unique_key={}&code_challenge={}&code_challenge_method=S256&restrict_signup=true&state={}",
        API_PKCE_AUTH,
        urlencoding_encode(PKCE_REDIRECT_URI),
        urlencoding_encode(CLIENT_ID_PKCE),
        urlencoding_encode(&client_unique_key),
        urlencoding_encode(&code_challenge),
        urlencoding_encode(&client_unique_key)
    );

    PkceFlow {
        login_url,
        code_verifier,
        client_unique_key,
    }
}

/// Exchanges the redirected authorization code for an OAuth access token.
pub async fn exchange_pkce_code(
    http: &reqwest::Client,
    redirect_url: &str,
    code_verifier: &str,
    client_unique_key: &str,
) -> Result<TidalToken, String> {
    let returned =
        url::Url::parse(redirect_url).map_err(|_| "Paste the complete sign-in redirect URL")?;
    let expected = url::Url::parse(PKCE_REDIRECT_URI).map_err(|e| e.to_string())?;
    if returned.scheme() != expected.scheme()
        || returned.host_str() != expected.host_str()
        || returned.path() != expected.path()
    {
        return Err("This URL is not the expected account sign-in redirect".into());
    }
    if extract_query_param(redirect_url, "state").as_deref() != Some(client_unique_key) {
        return Err("Sign-in response belongs to a different or expired sign-in attempt".into());
    }
    let code = extract_query_param(redirect_url, "code")
        .ok_or_else(|| "Missing 'code' query parameter in redirect URL".to_string())?;

    let params = [
        ("code", code.as_str()),
        ("client_id", CLIENT_ID_PKCE),
        ("grant_type", "authorization_code"),
        ("redirect_uri", PKCE_REDIRECT_URI),
        ("scope", "r_usr+w_usr+w_sub"),
        ("code_verifier", code_verifier),
        ("client_unique_key", client_unique_key),
    ];

    let res = http
        .post(API_AUTH_TOKEN)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("PKCE token exchange request failed: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!(
            "PKCE exchange rejected (HTTP {}): {}",
            status, body
        ));
    }

    let val: Value = res
        .json()
        .await
        .map_err(|e| format!("Invalid PKCE JSON response: {}", e))?;

    let access_token = val
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing access_token in response".to_string())?
        .to_string();

    let refresh_token = val
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let user_id = val
        .get("user")
        .and_then(|u| u.get("userId"))
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .or_else(|| {
            val.get("userId")
                .and_then(|v| v.as_i64().map(|n| n.to_string()))
        });

    let expires_in = val
        .get("expires_in")
        .and_then(|v| v.as_f64())
        .unwrap_or(604800.0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    Ok(TidalToken {
        token_type: val
            .get("token_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Bearer")
            .to_string(),
        access_token,
        refresh_token,
        expires_at: now + expires_in - 60.0,
        user_id,
        is_pkce: true,
    })
}

/// Refreshes an expired or expiring Tidal token.
pub async fn refresh_token(
    http: &reqwest::Client,
    refresh_token_str: &str,
    is_pkce: bool,
) -> Result<TidalToken, String> {
    let (client_id, client_secret) = if is_pkce {
        (CLIENT_ID_PKCE, Some(CLIENT_SECRET_PKCE))
    } else {
        (
            "fX2JxdmntZWK0ixT",
            Some("1Nn9AfDAjxrgJFJbKNWLeAyKGVGmINuXPPLHVXAvxAg="),
        )
    };

    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token_str),
        ("client_id", client_id),
    ];
    if let Some(sec) = client_secret {
        params.push(("client_secret", sec));
    }

    let res = http
        .post(API_AUTH_TOKEN)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Token refresh request failed: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!(
            "Token refresh rejected (HTTP {}): {}",
            status, body
        ));
    }

    let val: Value = res
        .json()
        .await
        .map_err(|e| format!("Invalid refresh JSON response: {}", e))?;

    let access_token = val
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing access_token in response".to_string())?
        .to_string();

    let new_refresh = val
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| Some(refresh_token_str.to_string()));

    let expires_in = val
        .get("expires_in")
        .and_then(|v| v.as_f64())
        .unwrap_or(604800.0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    Ok(TidalToken {
        token_type: val
            .get("token_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Bearer")
            .to_string(),
        access_token,
        refresh_token: new_refresh,
        expires_at: now + expires_in - 60.0,
        user_id: None,
        is_pkce,
    })
}

/// Loads a saved Tidal token from database preferences or filesystem token.json.
pub async fn load_saved_token(db: &TursoDb) -> Option<TidalToken> {
    if db
        .get_preference("account-disconnected")
        .await
        .ok()
        .flatten()
        == Some(json!(true))
    {
        return None;
    }
    // 1. Try DB preference "tidal_token"
    if let Ok(Some(val)) = db.get_preference("tidal_token").await {
        if let Ok(tok) = serde_json::from_value::<TidalToken>(val) {
            return Some(tok);
        }
    }

    if std::env::var_os("TIBRARY_TEST_MODE").is_some() {
        return None;
    }
    // 2. Try macOS keychain or session
    if let Some(session) = crate::account::AccountClient::load_saved_session() {
        return Some(TidalToken {
            token_type: "Bearer".to_string(),
            access_token: session.access_token,
            refresh_token: session.refresh_token,
            expires_at: session.expires_at,
            user_id: session.user_id,
            is_pkce: true,
        });
    }

    // 3. Try ~/.config/tidaler/token.json or ~/.config/tidaler-dev/token.json
    let home = std::env::var("HOME").ok()?;
    let candidate_paths = [
        PathBuf::from(&home).join(".config/tidaler/token.json"),
        PathBuf::from(&home).join(".config/tidaler-dev/token.json"),
    ];

    for path in &candidate_paths {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(val) = serde_json::from_str::<Value>(&content) {
                    if let Some(access) = val.get("access_token").and_then(|v| v.as_str()) {
                        let refresh = val
                            .get("refresh_token")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let exp_str = val
                            .get("expiry_time")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let expires_at =
                            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(exp_str) {
                                dt.timestamp() as f64
                            } else {
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs_f64()
                                    + 86400.0
                            };
                        return Some(TidalToken {
                            token_type: val
                                .get("token_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Bearer")
                                .to_string(),
                            access_token: access.to_string(),
                            refresh_token: refresh,
                            expires_at,
                            user_id: val
                                .get("user_id")
                                .and_then(|v| v.as_str().map(|s| s.to_string())),
                            is_pkce: true,
                        });
                    }
                }
            }
        }
    }

    None
}

/// Save the account in the OS credential store; retire legacy plaintext tokens.
pub async fn save_token(db: &TursoDb, token: &TidalToken) -> Result<(), String> {
    let session = crate::account::AccountSession {
        access_token: token.access_token.clone(),
        refresh_token: token.refresh_token.clone(),
        user_id: token.user_id.clone(),
        expires_at: token.expires_at,
    };
    crate::account::AccountClient::save_session(&session).map_err(|e| e.to_string())?;
    db.set_preference("tidal_token", &Value::Null).await?;

    db.set_preference("account-disconnected", &json!(false))
        .await?;

    Ok(())
}

/// Retrieves a valid access token, auto-refreshing if necessary.
pub async fn get_valid_token(db: &TursoDb, http: &reqwest::Client) -> Result<String, String> {
    let mut token = load_saved_token(db)
        .await
        .ok_or_else(|| "No Tidal login token found. Authentication required.".to_string())?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    if now >= token.expires_at {
        if let Some(ref ref_token) = token.refresh_token {
            let refreshed = refresh_token(http, ref_token, token.is_pkce).await?;
            let mut refreshed = refreshed;
            if refreshed.refresh_token.is_none() {
                refreshed.refresh_token = token.refresh_token.clone();
            }
            if refreshed.user_id.is_none() {
                refreshed.user_id = token.user_id.clone();
            }
            token = refreshed;
            save_token(db, &token).await?;
        } else {
            return Err("Tidal login token expired and no refresh token available.".to_string());
        }
    }

    Ok(token.access_token)
}

// ============================================================================
// PLAYBACK INFO & MANIFEST
// ============================================================================

#[derive(Debug, Clone)]
pub struct PlaybackStreamInfo {
    pub urls: Vec<String>,
    pub codec: String,
    pub mime_type: String,
    pub is_encrypted: bool,
    pub security_token: Option<String>,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
    pub album_replay_gain: Option<f64>,
    pub album_peak_amplitude: Option<f64>,
    pub track_replay_gain: Option<f64>,
    pub track_peak_amplitude: Option<f64>,
}

pub fn normalize_quality(q: &str) -> &'static str {
    let lower = q.to_lowercase();
    if lower.contains("hi_res") || lower.contains("24") || lower.contains("192") || lower.contains("hires") {
        "HI_RES_LOSSLESS"
    } else if lower.contains("lossless") || lower.contains("16") || lower.contains("44") || lower.contains("flac") {
        "LOSSLESS"
    } else if lower.contains("low") || lower.contains("96") {
        "LOW"
    } else if lower.contains("high") || lower.contains("320") || lower.contains("mp3") || lower.contains("aac") {
        "HIGH"
    } else {
        "LOSSLESS"
    }
}

/// Queries Tidal's playbackinfopostpaywall endpoint and parses manifest.
pub async fn get_playback_info(
    http: &reqwest::Client,
    track_id: &str,
    token: &str,
    quality: &str,
    market: &str,
) -> Result<PlaybackStreamInfo, String> {
    let norm_quality = normalize_quality(quality);
    let url = format!(
        "{}/tracks/{}/playbackinfopostpaywall?audioquality={}&playbackmode=STREAM&assetpresentation=FULL",
        API_V1_BASE, track_id, norm_quality
    );

    let res = http
        .get(&url)
        .query(&[("countryCode", market)])
        .header(AUTHORIZATION, format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| format!("Playback info request failed: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!(
            "Playback info rejected (HTTP {}): {}",
            status, body
        ));
    }

    let val: Value = res
        .json()
        .await
        .map_err(|e| format!("Invalid playback info JSON: {}", e))?;

    let manifest_mime = val
        .get("manifestMimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let manifest_b64 = val
        .get("manifest")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing manifest in playback info response".to_string())?;

    let manifest_bytes = STANDARD
        .decode(manifest_b64.trim())
        .map_err(|e| format!("Invalid base64 manifest: {}", e))?;

    let album_replay_gain = val.get("albumReplayGain").and_then(|v| v.as_f64());
    let album_peak_amplitude = val.get("albumPeakAmplitude").and_then(|v| v.as_f64());
    let track_replay_gain = val.get("trackReplayGain").and_then(|v| v.as_f64());
    let track_peak_amplitude = val.get("trackPeakAmplitude").and_then(|v| v.as_f64());
    let bit_depth = val
        .get("bitDepth")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let sample_rate = val
        .get("sampleRate")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    if manifest_mime.contains("bts") {
        // BTS JSON Manifest
        let bts: Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| format!("Invalid BTS manifest JSON: {}", e))?;

        let urls: Vec<String> = bts
            .get("urls")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if urls.is_empty() {
            return Err("No stream URLs found in BTS manifest".to_string());
        }

        let codec = bts
            .get("codecs")
            .and_then(|v| v.as_str())
            .unwrap_or("flac")
            .to_string();
        let mime = bts
            .get("mimeType")
            .and_then(|v| v.as_str())
            .unwrap_or("audio/flac")
            .to_string();
        let enc_type = bts
            .get("encryptionType")
            .and_then(|v| v.as_str())
            .unwrap_or("NONE");
        let is_encrypted = enc_type != "NONE";
        let security_token = bts
            .get("keyId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(PlaybackStreamInfo {
            urls,
            codec,
            mime_type: mime,
            is_encrypted,
            security_token,
            bit_depth,
            sample_rate,
            album_replay_gain,
            album_peak_amplitude,
            track_replay_gain,
            track_peak_amplitude,
        })
    } else if manifest_mime.contains("dash") || manifest_mime.contains("xml") {
        // MPEG-DASH MPD Manifest
        let xml_str = String::from_utf8_lossy(&manifest_bytes);
        let urls = parse_mpd_manifest(&xml_str)?;

        if urls.is_empty() {
            return Err("No stream segments extracted from MPD manifest".to_string());
        }

        Ok(PlaybackStreamInfo {
            urls,
            codec: "flac".to_string(),
            mime_type: "audio/flac".to_string(),
            is_encrypted: false,
            security_token: None,
            bit_depth,
            sample_rate,
            album_replay_gain,
            album_peak_amplitude,
            track_replay_gain,
            track_peak_amplitude,
        })
    } else {
        Err(format!(
            "Unsupported stream manifest MIME type: {}",
            manifest_mime
        ))
    }
}

/// Parses an MPEG-DASH MPD XML manifest to extract initialization and segment URLs.
pub fn parse_mpd_manifest(xml: &str) -> Result<Vec<String>, String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut init_url = String::new();
    let mut media_template = String::new();
    let mut segment_count = 0usize;
    let mut direct_urls = Vec::new();

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name.ends_with("SegmentTemplate") {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        if key == "initialization" {
                            init_url = val;
                        } else if key == "media" {
                            media_template = val;
                        }
                    }
                } else if name.ends_with("S") {
                    // Segment timeline element: attributes: d, r
                    let mut repeat = 0usize;
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        if key == "r" {
                            repeat = val.parse::<usize>().unwrap_or(0);
                        }
                    }
                    segment_count += 1 + repeat;
                } else if name.ends_with("Initialization") {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        if key == "sourceURL" {
                            init_url = val;
                        }
                    }
                } else if name.ends_with("SegmentURL") {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        if key == "media" {
                            direct_urls.push(val);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("Error parsing MPD XML: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    if !direct_urls.is_empty() {
        let mut all = Vec::new();
        if !init_url.is_empty() {
            all.push(init_url);
        }
        all.extend(direct_urls);
        return Ok(all);
    }

    if !media_template.is_empty() {
        let mut all = Vec::new();
        if !init_url.is_empty() {
            all.push(init_url);
        }
        let total = if segment_count > 0 { segment_count } else { 1 };
        for i in 0..total {
            let replaced = if media_template.contains("$Number$") {
                media_template.replace("$Number$", &i.to_string())
            } else if let Some(start_idx) = media_template.find("$Number%") {
                if let Some(rel_end) = media_template[start_idx + 1..].find('$') {
                    let end_idx = start_idx + 1 + rel_end;
                    let spec = &media_template[start_idx..=end_idx];
                    let digits = spec
                        .chars()
                        .filter(|c| c.is_ascii_digit())
                        .collect::<String>();
                    let width = digits.parse::<usize>().unwrap_or(1);
                    media_template.replace(spec, &format!("{:0width$}", i, width = width))
                } else {
                    media_template.replace("$Number$", &i.to_string())
                }
            } else {
                media_template.replace("$Number$", &i.to_string())
            };
            all.push(replaced);
        }
        return Ok(all);
    }

    Err("Could not extract stream URLs from MPD manifest".to_string())
}

// ============================================================================
// AUDIO DOWNLOAD ENGINE
// ============================================================================

/// Downloads audio stream segments into destination file, handling decryption if needed.
pub async fn download_stream(
    http: &reqwest::Client,
    stream_info: &PlaybackStreamInfo,
    dest_path: &Path,
    cancel_flag: &Arc<AtomicBool>,
    progress_cb: &(dyn Fn(u8) + Send + Sync),
) -> Result<(), String> {
    if dest_path.exists() {
        let _ = fs::remove_file(dest_path);
    }

    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut dest_file = File::create(dest_path).map_err(|e| e.to_string())?;
    let total_urls = stream_info.urls.len();

    // Key and nonce for stream decryption if needed
    let crypto_keys = if stream_info.is_encrypted {
        if let Some(ref sec_token) = stream_info.security_token {
            Some(decrypt_security_token(sec_token)?)
        } else {
            return Err("Stream is marked encrypted but missing security token".to_string());
        }
    } else {
        None
    };

    let mut _downloaded_bytes_total = 0usize;
    for (idx, url) in stream_info.urls.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            let _ = fs::remove_file(dest_path);
            return Err("Download cancelled".to_string());
        }

        let mut res = http
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Segment download error (url {}): {}", idx, e))?;

        if !res.status().is_success() {
            let _ = fs::remove_file(dest_path);
            return Err(format!(
                "Segment download failed with HTTP {}",
                res.status()
            ));
        }

        let mut seg_bytes = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
            if cancel_flag.load(Ordering::Relaxed) {
                let _ = fs::remove_file(dest_path);
                return Err("Download cancelled".to_string());
            }
            seg_bytes.extend_from_slice(&chunk);
            _downloaded_bytes_total += chunk.len();
        }

        dest_file
            .write_all(&seg_bytes)
            .map_err(|e| format!("Failed writing segment to file: {}", e))?;

        let pct = (((idx + 1) as f64 / total_urls as f64) * 100.0).min(100.0) as u8;
        progress_cb(pct);
    }

    dest_file.flush().map_err(|e| e.to_string())?;
    drop(dest_file);

    // If stream was encrypted, decrypt entire assembled file
    if let Some((key, nonce)) = crypto_keys {
        let mut all_bytes = fs::read(dest_path).map_err(|e| e.to_string())?;
        decrypt_stream_bytes(&mut all_bytes, &key, &nonce)?;
        fs::write(dest_path, &all_bytes).map_err(|e| e.to_string())?;
    }

    Ok(())
}

// ============================================================================
// METADATA & COVER TAGGING
// ============================================================================

#[derive(Debug, Clone, Default)]
pub struct TrackDownloadMeta {
    pub track_id: String,
    pub album_id: String,
    pub title: String,
    pub album: String,
    pub album_artist: String,
    pub track_artists: Vec<String>,
    pub track_number: u32,
    pub track_total: u32,
    pub disc_number: u32,
    pub disc_total: u32,
    pub date: Option<String>,
    pub isrc: Option<String>,
    pub copyright: Option<String>,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub release_type: Option<String>,
    pub explicit: bool,
    pub lyrics: Option<String>,
    pub unsynced_lyrics: Option<String>,
    pub album_replay_gain: Option<f64>,
    pub album_peak_amplitude: Option<f64>,
    pub track_replay_gain: Option<f64>,
    pub track_peak_amplitude: Option<f64>,
}

/// Applies tags and embeds front cover art into the downloaded audio file.
pub fn apply_audio_tags(
    file_path: &Path,
    meta: &TrackDownloadMeta,
    cover_data: Option<&[u8]>,
) -> Result<(), String> {
    use lofty::ogg::tag::VorbisComments;
    use lofty::ogg::OggPictureStorage;

    let is_flac = file_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("flac"))
        .unwrap_or(false);

    if is_flac {
        let mut comments = VorbisComments::new();
        comments.insert("TITLE".to_string(), meta.title.clone());
        comments.insert("ALBUM".to_string(), meta.album.clone());
        comments.insert(
            "ARTIST".to_string(),
            if !meta.track_artists.is_empty() {
                meta.track_artists.join(", ")
            } else {
                meta.album_artist.clone()
            },
        );
        // CRITICAL: ALBUMARTIST strictly set to album artist!
        comments.insert("ALBUMARTIST".to_string(), meta.album_artist.clone());
        comments.insert(
            "TRACKNUMBER".to_string(),
            format!("{:02}", meta.track_number),
        );
        comments.insert("TRACKTOTAL".to_string(), format!("{:02}", meta.track_total));
        comments.insert("DISCNUMBER".to_string(), format!("{:02}", meta.disc_number));
        comments.insert("DISCTOTAL".to_string(), format!("{:02}", meta.disc_total));

        if let Some(ref d) = meta.date {
            comments.insert("DATE".to_string(), d.clone());
            comments.insert("ORIGINALDATE".to_string(), d.clone());
        }
        if let Some(ref isrc) = meta.isrc {
            comments.insert("ISRC".to_string(), isrc.clone());
        }
        if let Some(ref c) = meta.copyright {
            comments.insert("COPYRIGHT".to_string(), c.clone());
        }
        if let Some(bpm) = meta.bpm {
            if bpm > 0.0 {
                comments.insert("BPM".to_string(), format!("{:.0}", bpm));
            }
        }
        if let Some(ref key) = meta.musical_key {
            if let Some(key) = crate::musical_keys::camelot_key(key) {
                comments.insert("INITIALKEY".to_string(), key);
            }
        }
        if let Some(ref rt) = meta.release_type {
            comments.insert("RELEASETYPE".to_string(), rt.clone());
        }

        // Tidal IDs
        comments.insert("TIDAL_TRACK_ID".to_string(), meta.track_id.clone());
        comments.insert("TIDAL_ALBUM_ID".to_string(), meta.album_id.clone());

        // Lyrics
        if let Some(ref lyr) = meta.lyrics {
            comments.insert("LYRICS".to_string(), lyr.clone());
        }
        if let Some(ref unlyr) = meta.unsynced_lyrics {
            comments.insert("UNSYNCEDLYRICS".to_string(), unlyr.clone());
        }

        // ReplayGain
        if let Some(g) = meta.album_replay_gain {
            comments.insert("REPLAYGAIN_ALBUM_GAIN".to_string(), format!("{:.2} dB", g));
        }
        if let Some(p) = meta.album_peak_amplitude {
            comments.insert("REPLAYGAIN_ALBUM_PEAK".to_string(), format!("{:.6}", p));
        }
        if let Some(g) = meta.track_replay_gain {
            comments.insert("REPLAYGAIN_TRACK_GAIN".to_string(), format!("{:.2} dB", g));
        }
        if let Some(p) = meta.track_peak_amplitude {
            comments.insert("REPLAYGAIN_TRACK_PEAK".to_string(), format!("{:.6}", p));
        }

        if let Some(cover_bytes) = cover_data {
            let pic = Picture::unchecked(cover_bytes.to_vec())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Jpeg)
                .build();
            let _ = comments.insert_picture(pic, None);
        }

        comments
            .save_to_path(file_path, WriteOptions::default())
            .map_err(|e| format!("Failed saving Vorbis comments to FLAC: {}", e))?;

        return Ok(());
    }

    let mut tagged_file = Probe::open(file_path)
        .map_err(|e| format!("Failed opening file for tagging: {}", e))?
        .read()
        .map_err(|e| format!("Failed reading file tags: {}", e))?;

    let tag_type = tagged_file.primary_tag_type();
    let tag = match tagged_file.tag_mut(tag_type) {
        Some(t) => t,
        None => {
            let new_tag = Tag::new(tag_type);
            tagged_file.insert_tag(new_tag);
            tagged_file.tag_mut(tag_type).unwrap()
        }
    };

    // 1. Basic essential tags
    tag.set_title(meta.title.clone());
    tag.set_album(meta.album.clone());
    tag.set_artist(if !meta.track_artists.is_empty() {
        meta.track_artists.join(", ")
    } else {
        meta.album_artist.clone()
    });

    // CRITICAL: ALBUMARTIST must be set strictly to the album artist!
    tag.insert_text(ItemKey::AlbumArtist, meta.album_artist.clone());

    tag.set_track(meta.track_number);
    tag.set_track_total(meta.track_total);
    tag.set_disk(meta.disc_number);
    tag.set_disk_total(meta.disc_total);

    if let Some(ref d) = meta.date {
        tag.insert_text(ItemKey::RecordingDate, d.clone());
        tag.insert_text(ItemKey::OriginalReleaseDate, d.clone());
    }

    if let Some(ref isrc) = meta.isrc {
        tag.insert_text(ItemKey::Isrc, isrc.clone());
    }

    if let Some(ref c) = meta.copyright {
        tag.insert_text(ItemKey::CopyrightMessage, c.clone());
    }

    if let Some(bpm) = meta.bpm {
        if bpm > 0.0 {
            tag.insert_text(ItemKey::Bpm, format!("{:.0}", bpm));
        }
    }

    if let Some(ref key) = meta.musical_key {
        if let Some(key) = crate::musical_keys::camelot_key(key) {
            tag.insert_text(ItemKey::InitialKey, key);
        }
    }

    if let Some(cover_bytes) = cover_data {
        let pic = Picture::unchecked(cover_bytes.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Jpeg)
            .build();
        tag.push_picture(pic);
    }

    tag.save_to_path(file_path, WriteOptions::default())
        .map_err(|e| format!("Failed saving tags to audio file: {}", e))?;

    Ok(())
}

// ============================================================================
// LYRICS & COVER FETCHER
// ============================================================================

/// Fetches lyrics from Tidal API for a given track ID.
pub async fn fetch_lyrics(
    http: &reqwest::Client,
    track_id: &str,
    token: &str,
    market: &str,
) -> (Option<String>, Option<String>) {
    let url = format!("{}/tracks/{}/lyrics", API_V1_BASE, track_id);
    if let Ok(res) = http
        .get(&url)
        .query(&[("countryCode", market)])
        .header(AUTHORIZATION, format!("Bearer {}", token))
        .send()
        .await
    {
        if res.status().is_success() {
            if let Ok(val) = res.json::<Value>().await {
                let synced = val
                    .get("subtitles")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let unsynced = val
                    .get("lyrics")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                return (synced, unsynced);
            }
        }
    }
    (None, None)
}

/// Fetches cover art JPEG bytes for an album cover UUID.
pub async fn fetch_cover_art(
    http: &reqwest::Client,
    cover_uuid: &str,
    cover_size: u32,
) -> Option<Vec<u8>> {
    let uuid_formatted = cover_uuid.replace('-', "/");
    let url = if cover_size >= 1280 {
        format!("{}/images/{}/1280x1280.jpg", RESOURCES_BASE, uuid_formatted)
    } else if cover_size >= 640 {
        format!("{}/images/{}/640x640.jpg", RESOURCES_BASE, uuid_formatted)
    } else {
        format!("{}/images/{}/320x320.jpg", RESOURCES_BASE, uuid_formatted)
    };

    if let Ok(res) = http.get(&url).send().await {
        if res.status().is_success() {
            return res.bytes().await.ok().map(|b| b.to_vec());
        }
    }

    // Fallback to origin.jpg
    let origin_url = format!("{}/images/{}/origin.jpg", RESOURCES_BASE, uuid_formatted);
    if let Ok(res) = http.get(&origin_url).send().await {
        if res.status().is_success() {
            return res.bytes().await.ok().map(|b| b.to_vec());
        }
    }

    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TidalAlbumInfo {
    pub id: String,
    pub title: String,
    pub artist_name: Option<String>,
    pub cover: Option<String>,
    pub number_of_tracks: Option<usize>,
    pub number_of_volumes: Option<usize>,
    pub release_date: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TidalAlbumTrack {
    pub id: String,
    pub title: String,
    pub track_number: u32,
    pub volume_number: u32,
    pub duration: f64,
    pub isrc: Option<String>,
    pub copyright: Option<String>,
    pub explicit: bool,
    pub artists: Vec<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub audio_modes: Vec<String>,
}

/// Fetches album details from Tidal API.
pub async fn fetch_album_info(
    http: &reqwest::Client,
    album_id: &str,
    token: &str,
    market: &str,
) -> Result<TidalAlbumInfo, String> {
    let url = format!("{}/albums/{}", API_V1_BASE, album_id);
    let res = http
        .get(&url)
        .query(&[("countryCode", market)])
        .header(AUTHORIZATION, format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| format!("Album info request failed: {}", e))?;

    if !res.status().is_success() {
        return Err(format!("Album info failed with HTTP {}", res.status()));
    }

    let val: Value = res.json().await.map_err(|e| e.to_string())?;
    let id = val
        .get("id")
        .map(|v| v.to_string())
        .unwrap_or_default()
        .trim_matches('"')
        .to_string();
    let title = val
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let cover = val
        .get("cover")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let number_of_tracks = val
        .get("numberOfTracks")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let number_of_volumes = val
        .get("numberOfVolumes")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let release_date = val
        .get("releaseDate")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let version = val
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let artist_name = val
        .get("artist")
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            val.get("artists")
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    Ok(TidalAlbumInfo {
        id,
        title,
        artist_name,
        cover,
        number_of_tracks,
        number_of_volumes,
        release_date,
        version,
    })
}

/// Fetches all tracks of an album from Tidal API, handling pagination.
pub async fn fetch_album_tracks(
    http: &reqwest::Client,
    album_id: &str,
    token: &str,
    cancel_flag: &Arc<AtomicBool>,
    market: &str,
) -> Result<Vec<TidalAlbumTrack>, String> {
    let mut tracks = Vec::new();
    let mut offset = 0usize;
    let limit = 100usize;

    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }

        let url = format!(
            "{}/albums/{}/tracks?limit={}&offset={}",
            API_V1_BASE, album_id, limit, offset
        );
        let res = http
            .get(&url)
            .query(&[("countryCode", market)])
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| format!("Tracks request failed: {}", e))?;

        if !res.status().is_success() {
            return Err(format!("Tracks request failed with HTTP {}", res.status()));
        }

        let val: Value = res.json().await.map_err(|e| e.to_string())?;
        let items = val.get("items").and_then(|v| v.as_array());

        let Some(items) = items else { break };
        if items.is_empty() {
            break;
        }

        for item in items {
            let id = item
                .get("id")
                .map(|v| v.to_string())
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            let title = crate::tidal::format_title(
                item.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                item.get("version").and_then(|v| v.as_str()),
            );
            let track_number = item
                .get("trackNumber")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let volume_number = item
                .get("volumeNumber")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;
            let duration = item.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let isrc = item
                .get("isrc")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let copyright = item
                .get("copyright")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let explicit = item
                .get("explicit")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let bpm = item.get("bpm").and_then(|v| v.as_f64());
            let key = item.get("key").and_then(|v| v.as_str()).map(|key| {
                match item["keyScale"].as_str() {
                    Some(scale) => format!("{key} {scale}"),
                    None => key.to_string(),
                }
            });

            let mut artists = Vec::new();
            if let Some(arr) = item.get("artists").and_then(|v| v.as_array()) {
                for a in arr {
                    if let Some(name) = a.get("name").and_then(|v| v.as_str()) {
                        artists.push(name.to_string());
                    }
                }
            } else if let Some(a) = item
                .get("artist")
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
            {
                artists.push(a.to_string());
            }

            let mut audio_modes = Vec::new();
            if let Some(arr) = item.get("audioModes").and_then(|v| v.as_array()) {
                for m in arr {
                    if let Some(ms) = m.as_str() {
                        audio_modes.push(ms.to_string());
                    }
                }
            }

            tracks.push(TidalAlbumTrack {
                id,
                title,
                track_number,
                volume_number,
                duration,
                isrc,
                copyright,
                explicit,
                artists,
                bpm,
                key,
                audio_modes,
            });
        }

        let total = val
            .get("totalNumberOfItems")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        offset += items.len();
        if offset >= total || items.len() < limit {
            break;
        }
    }

    Ok(tracks)
}

// ============================================================================
// LAYOUT & SAFE PUBLISHING
// ============================================================================

/// Sanitizes a single filename or folder path component.
pub fn safe_component(val: &str) -> String {
    let mut cleaned = String::new();
    for ch in val.chars() {
        match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => cleaned.push(' '),
            c if c.is_control() => {}
            c => cleaned.push(c),
        }
    }
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c| c == ' ' || c == '.');
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Generates the destination relative file path according to the layout template.
pub fn format_download_path(template: &str, meta: &TrackDownloadMeta, extension: &str) -> PathBuf {
    let ext = if extension.starts_with('.') {
        extension.to_string()
    } else {
        format!(".{}", extension)
    };

    let year = meta
        .date
        .as_ref()
        .and_then(|d| d.split('-').next())
        .unwrap_or("");

    let disc_text = if meta.disc_total > 1 {
        format!("Disc {}", meta.disc_number)
    } else {
        String::new()
    };

    let track_pad = if meta.track_total >= 100 {
        format!("{:03}", meta.track_number)
    } else {
        format!("{:02}", meta.track_number)
    };

    let album_artist_safe = safe_component(&meta.album_artist);
    let track_artist_safe = if !meta.track_artists.is_empty() {
        safe_component(&meta.track_artists.join(", "))
    } else {
        album_artist_safe.clone()
    };
    let album_safe = safe_component(&meta.album);
    let title_safe = safe_component(&meta.title);

    let mut result_parts = Vec::new();
    for segment in template.split('/') {
        let mut part = segment.to_string();
        part = part.replace("{album_artist}", &album_artist_safe);
        part = part.replace("{albumartist}", &album_artist_safe);
        part = part.replace("{artist}", &track_artist_safe);
        part = part.replace("{album}", &album_safe);
        part = part.replace("{album_title}", &album_safe);
        part = part.replace("{title}", &title_safe);
        part = part.replace("{track_title}", &title_safe);
        part = part.replace("{track_number}", &track_pad);
        part = part.replace("{tracknumber}", &track_pad);
        part = part.replace("{year}", year);
        part = part.replace("{disc}", &disc_text);
        part = part.replace(
            "{disc_prefix}",
            &if meta.disc_total > 1 {
                format!("{:02}.", meta.disc_number)
            } else {
                String::new()
            },
        );
        part = part.replace("{discnumber}", &format!("{:02}", meta.disc_number));

        let sanitized = safe_component(&part);
        if !sanitized.is_empty() && sanitized != "_" {
            result_parts.push(sanitized);
        }
    }

    if result_parts.is_empty() {
        result_parts.push(format!("{} - {}{}", track_pad, title_safe, ext));
    } else {
        let last_idx = result_parts.len() - 1;
        result_parts[last_idx] = format!("{}{}", result_parts[last_idx], ext);
    }

    let mut path = PathBuf::new();
    for p in result_parts {
        path.push(p);
    }
    path
}

/// Publishes downloaded files safely into target root.
/// Existing files with identical content are safely acknowledged;
/// different existing files result in an error to avoid data loss.
pub fn publish_staged_files(stage_dir: &Path, target_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut published = Vec::new();

    fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk_files(&path, out)?;
            } else if path.is_file() {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut staged_files = Vec::new();
    walk_files(stage_dir, &mut staged_files).map_err(|e| e.to_string())?;

    for src in staged_files {
        let rel = src
            .strip_prefix(stage_dir)
            .map_err(|e| format!("Staged file not within stage dir: {}", e))?;
        let dst = target_root.join(rel);

        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        if dst.exists() {
            // Check SHA-256
            let src_bytes = fs::read(&src).map_err(|e| e.to_string())?;
            let dst_bytes = fs::read(&dst).map_err(|e| e.to_string())?;
            if sha256_digest(&src_bytes) == sha256_digest(&dst_bytes) {
                published.push(dst);
                continue;
            } else {
                return Err(format!(
                    "Destination already exists with different contents: {}",
                    dst.display()
                ));
            }
        }

        // Try hardlink first, then rename/move
        if fs::hard_link(&src, &dst).is_err() {
            fs::copy(&src, &dst).map_err(|e| e.to_string())?;
        }
        published.push(dst);
    }

    Ok(published)
}

// ============================================================================
// HELPERS
// ============================================================================

fn urlencoding_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn extract_query_param(url_str: &str, param: &str) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(url_str) {
        for (k, v) in parsed.query_pairs() {
            if k == param {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Inspects file header magic bytes to detect actual audio container format.
pub fn detect_audio_extension(bytes: &[u8], fallback: &str) -> String {
    if bytes.len() >= 4 && &bytes[0..4] == b"fLaC" {
        return ".flac".to_string();
    }
    if bytes.len() >= 8
        && (&bytes[4..8] == b"ftyp" || (bytes.len() >= 12 && &bytes[8..12] == b"ftyp"))
    {
        return ".m4a".to_string();
    }
    if bytes.len() >= 3 && &bytes[0..3] == b"ID3" {
        return ".mp3".to_string();
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        return ".mp3".to_string();
    }
    if bytes.len() >= 4 && &bytes[0..4] == b"OggS" {
        return ".ogg".to_string();
    }
    if fallback.starts_with('.') {
        fallback.to_string()
    } else {
        format!(".{}", fallback)
    }
}

/// Writes synced or unsynced lyrics to a companion .lrc file.
pub fn write_lrc_file(audio_path: &Path, lyrics: &str) -> std::io::Result<PathBuf> {
    let lrc_path = audio_path.with_extension("lrc");
    fs::write(&lrc_path, lyrics)?;
    Ok(lrc_path)
}

/// Creates an M3U8 playlist in the specified album directory with ordered tracks.
pub fn write_m3u8_playlist(
    album_dir: &Path,
    playlist_name: &str,
    track_filenames: &[String],
) -> std::io::Result<PathBuf> {
    let playlist_path = album_dir.join(format!("{}.m3u8", playlist_name));
    let mut content = String::from("#EXTM3U\n");
    for track in track_filenames {
        content.push_str(track);
        content.push('\n');
    }
    fs::write(&playlist_path, content)?;
    Ok(playlist_path)
}

pub const MINIMAL_FLAC: &[u8] = &[
    102, 76, 97, 67, 0, 0, 0, 34, 16, 0, 16, 0, 0, 0, 14, 0, 0, 16, 10, 196, 66, 240, 0, 0, 17, 58,
    136, 44, 112, 41, 165, 9, 119, 105, 184, 91, 209, 118, 245, 117, 38, 132, 132, 0, 0, 40, 32, 0,
    0, 0, 114, 101, 102, 101, 114, 101, 110, 99, 101, 32, 108, 105, 98, 70, 76, 65, 67, 32, 49, 46,
    52, 46, 51, 32, 50, 48, 50, 51, 48, 54, 50, 51, 0, 0, 0, 0, 255, 248, 201, 24, 0, 194, 0, 0, 0,
    0, 0, 0, 184, 238, 255, 248, 121, 24, 1, 1, 57, 215, 0, 0, 0, 0, 0, 0, 173, 241,
];

// ============================================================================
// UNIT TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decrypt_security_token_vector() {
        // Known test vector generated with master key and Python PyCryptodome:
        // Key: b"0123456789abcdef"
        // Nonce: b"12345678"
        let token_b64 = "MDEyMzQ1Njc4OWFiY2RlZgDArlH3qgOnIOkznG4ENzPnHCVBG9d/7bZbQM1U5IA4";
        let (key, nonce) = decrypt_security_token(token_b64).expect("Failed to decrypt token");

        assert_eq!(&key, b"0123456789abcdef");
        assert_eq!(&nonce, b"12345678");
    }

    #[test]
    fn test_decrypt_stream_bytes_vector() {
        // Known test vector generated with Python PyCryptodome AES-128-CTR
        let key = b"0123456789abcdef";
        let nonce = b"12345678";
        let plaintext = b"Hello world, this is a test message 1234567890!";
        let ciphertext_hex = "4e7474a41b3f09fdda8f3877bbe70c1495c336b081b699e7ca02e4a84f59e9e1eef6c05647d5dfdf1fef1d73429f63";
        let mut ct_bytes = hex_decode(ciphertext_hex);

        decrypt_stream_bytes(&mut ct_bytes, key, nonce).expect("Failed CTR decrypt");
        assert_eq!(&ct_bytes, plaintext);
    }

    #[test]
    fn test_create_pkce_flow() {
        let flow = create_pkce_flow();
        assert!(!flow.login_url.is_empty());
        assert!(!flow.code_verifier.is_empty());
        assert!(!flow.client_unique_key.is_empty());
        assert!(flow.login_url.contains("https://login.tidal.com/authorize"));
        assert!(flow.login_url.contains(CLIENT_ID_PKCE));
    }

    #[test]
    fn test_safe_component_and_layout_path() {
        let meta = TrackDownloadMeta {
            track_id: "123".to_string(),
            album_id: "456".to_string(),
            title: "Harder, Better, Faster, Stronger".to_string(),
            album: "Discovery".to_string(),
            album_artist: "Daft Punk".to_string(),
            track_artists: vec!["Daft Punk".to_string()],
            track_number: 4,
            track_total: 14,
            disc_number: 1,
            disc_total: 1,
            date: Some("2001-03-12".to_string()),
            ..Default::default()
        };

        let path = format_download_path(
            "{album_artist}/{album}/{track_number} {title}",
            &meta,
            ".flac",
        );
        assert_eq!(
            path,
            PathBuf::from("Daft Punk/Discovery/04 Harder, Better, Faster, Stronger.flac")
        );
    }

    #[test]
    fn test_safe_component_special_chars() {
        assert_eq!(safe_component("AC/DC: Live!"), "AC DC Live!");
        assert_eq!(safe_component("Track..."), "Track");
        assert_eq!(safe_component("What? <Where>"), "What Where");
    }

    #[test]
    fn test_parse_mpd_manifest() {
        let mpd_sample = r#"<?xml version='1.0' encoding='UTF-8'?>
        <MPD xmlns="urn:mpeg:dash:schema:mpd:2011" mediaPresentationDuration="PT3M30S">
            <Period>
                <AdaptationSet contentType="audio" mimeType="audio/mp4">
                    <Representation codecs="flac" audioSamplingRate="44100">
                        <SegmentTemplate initialization="init.mp4" media="segment_$Number$.mp4">
                            <SegmentTimeline>
                                <S d="1000" r="2" />
                            </SegmentTimeline>
                        </SegmentTemplate>
                    </Representation>
                </AdaptationSet>
            </Period>
        </MPD>"#;

        let urls = parse_mpd_manifest(mpd_sample).expect("Failed to parse MPD sample");
        assert_eq!(urls.len(), 4); // init.mp4 + segment_0, segment_1, segment_2
        assert_eq!(urls[0], "init.mp4");
        assert_eq!(urls[1], "segment_0.mp4");
        assert_eq!(urls[2], "segment_1.mp4");
        assert_eq!(urls[3], "segment_2.mp4");
    }

    #[test]
    fn test_parse_mpd_manifest_formatted_number() {
        let mpd_sample = r#"<?xml version='1.0' encoding='UTF-8'?>
        <MPD xmlns="urn:mpeg:dash:schema:mpd:2011">
            <Period>
                <AdaptationSet contentType="audio" mimeType="audio/mp4">
                    <Representation codecs="flac">
                        <SegmentTemplate initialization="init.mp4" media="chunk_$Number%04d$.mp4">
                            <SegmentTimeline>
                                <S d="1000" r="1" />
                            </SegmentTimeline>
                        </SegmentTemplate>
                    </Representation>
                </AdaptationSet>
            </Period>
        </MPD>"#;

        let urls = parse_mpd_manifest(mpd_sample).expect("Failed to parse formatted MPD");
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "init.mp4");
        assert_eq!(urls[1], "chunk_0000.mp4");
        assert_eq!(urls[2], "chunk_0001.mp4");
    }

    #[test]
    fn test_flac_tagging_and_claxon_playback() {
        let temp_dir = std::env::temp_dir().join(format!(
            "flac_tag_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let flac_file = temp_dir.join("test.flac");
        fs::write(&flac_file, MINIMAL_FLAC).expect("Failed to write initial FLAC");

        // Verify initial playback validity with claxon
        let reader =
            claxon::FlacReader::open(&flac_file).expect("Claxon failed to open initial FLAC");
        assert_eq!(reader.streaminfo().channels, 2);
        assert_eq!(reader.streaminfo().sample_rate, 44100);

        let meta = TrackDownloadMeta {
            track_id: "887766".to_string(),
            album_id: "112233".to_string(),
            title: "Get Lucky".to_string(),
            album: "Random Access Memories".to_string(),
            album_artist: "Daft Punk".to_string(),
            track_artists: vec!["Daft Punk".to_string(), "Pharrell Williams".to_string()],
            track_number: 8,
            track_total: 13,
            disc_number: 1,
            disc_total: 1,
            date: Some("2013-05-17".to_string()),
            bpm: Some(116.0),
            musical_key: Some("8A".to_string()),
            lyrics: Some("[00:01.00] Like the legend of the phoenix".to_string()),
            album_replay_gain: Some(-9.2),
            track_replay_gain: Some(-8.7),
            ..Default::default()
        };

        // Dummy 1x1 JPEG for cover test
        let dummy_jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x01,
            0x00, 0x48, 0x00, 0x48, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x03, 0x02, 0x02,
            0x02, 0x02, 0x02, 0x03, 0x02, 0x02, 0x02, 0x03, 0x03, 0x03, 0x03, 0x04, 0x06, 0x04,
            0x04, 0x04, 0x04, 0x04, 0x08, 0x06, 0x06, 0x05, 0x06, 0x09, 0x08, 0x0A, 0x0A, 0x09,
            0x08, 0x09, 0x09, 0x0A, 0x0C, 0x0F, 0x0C, 0x0A, 0x0B, 0x0E, 0x0B, 0x09, 0x09, 0x0D,
            0x11, 0x0D, 0x0E, 0x0F, 0x10, 0x10, 0x11, 0x10, 0x0A, 0x0C, 0x12, 0x13, 0x12, 0x10,
            0x13, 0x0F, 0x10, 0x10, 0x10, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01,
            0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
            0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xDA, 0x00, 0x08, 0x01,
            0x01, 0x00, 0x00, 0x3F, 0x00, 0xBF, 0x00, 0xFF, 0xD9,
        ];

        apply_audio_tags(&flac_file, &meta, Some(&dummy_jpeg)).expect("Failed applying tags");

        // Verify tags using scanner::read_audio_metadata
        let parsed =
            crate::scanner::read_audio_metadata(&flac_file).expect("Failed reading back tags");
        assert_eq!(parsed.title, "Get Lucky");
        assert_eq!(parsed.album, "Random Access Memories");
        // Verify ALBUMARTIST is strictly Daft Punk!
        assert_eq!(parsed.album_artist.as_deref(), Some("Daft Punk"));
        assert_eq!(parsed.tidal_track_id.as_deref(), Some("887766"));
        assert_eq!(parsed.tidal_album_id.as_deref(), Some("112233"));
        assert_eq!(parsed.track.as_deref(), Some("08"));
        assert_eq!(parsed.tracktotal.as_deref(), Some("13"));

        // Verify playback validity with claxon after tagging
        let mut reader_after =
            claxon::FlacReader::open(&flac_file).expect("Claxon failed to open tagged FLAC");
        let mut sample_count = 0;
        for sample in reader_after.samples() {
            let _ = sample.expect("Corrupt audio frame");
            sample_count += 1;
        }
        assert!(sample_count > 0, "No audio samples decoded");

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_publish_staged_files_safe() {
        let temp_dir = std::env::temp_dir().join(format!(
            "publish_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let stage = temp_dir.join("stage");
        let dest = temp_dir.join("library");
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&dest).unwrap();

        let track_staged = stage.join("Artist/Album/01 Track.flac");
        fs::create_dir_all(track_staged.parent().unwrap()).unwrap();
        fs::write(&track_staged, b"audio-payload-123").unwrap();

        let published = publish_staged_files(&stage, &dest).expect("Initial publish failed");
        assert_eq!(published.len(), 1);
        let dest_file = dest.join("Artist/Album/01 Track.flac");
        assert!(dest_file.exists());
        assert_eq!(fs::read(&dest_file).unwrap(), b"audio-payload-123");

        // Re-publish same file -> succeeds idempotently
        let published2 = publish_staged_files(&stage, &dest).expect("Idempotent publish failed");
        assert_eq!(published2.len(), 1);

        // Re-publish a different file at the same relative path -> fails with error to prevent overwriting user file
        fs::remove_dir_all(&stage).unwrap();
        fs::create_dir_all(&stage).unwrap();
        let track_staged_new = stage.join("Artist/Album/01 Track.flac");
        fs::create_dir_all(track_staged_new.parent().unwrap()).unwrap();
        fs::write(&track_staged_new, b"DIFFERENT-audio-payload-456").unwrap();

        let err =
            publish_staged_files(&stage, &dest).expect_err("Should have failed due to collision");
        assert!(err.contains("Destination already exists with different contents"));

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_detect_audio_extension() {
        assert_eq!(detect_audio_extension(b"fLaC\x00\x00", ".m4a"), ".flac");
        assert_eq!(
            detect_audio_extension(b"\x00\x00\x00\x20ftypM4A \x00\x00", ".flac"),
            ".m4a"
        );
        assert_eq!(detect_audio_extension(b"ID3\x04\x00\x00", ".flac"), ".mp3");
        assert_eq!(detect_audio_extension(b"OggS\x00\x02", ".flac"), ".ogg");
        assert_eq!(detect_audio_extension(b"UNKNOWN_BYTES", ".flac"), ".flac");
    }

    #[test]
    fn test_write_lrc_file() {
        let temp_dir = std::env::temp_dir().join(format!(
            "lrc_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let audio_path = temp_dir.join("01 - Test Track.flac");
        fs::write(&audio_path, b"audio").unwrap();

        let lrc_path = write_lrc_file(&audio_path, "[00:12.00]Line 1\n[00:15.00]Line 2\n").unwrap();
        assert_eq!(lrc_path, temp_dir.join("01 - Test Track.lrc"));
        assert!(lrc_path.exists());
        assert_eq!(
            fs::read_to_string(&lrc_path).unwrap(),
            "[00:12.00]Line 1\n[00:15.00]Line 2\n"
        );

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_write_m3u8_playlist() {
        let temp_dir = std::env::temp_dir().join(format!(
            "m3u8_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let tracks = vec![
            "01 Track One.flac".to_string(),
            "02 Track Two.flac".to_string(),
        ];

        let pl_path = write_m3u8_playlist(&temp_dir, "_playlist", &tracks).unwrap();
        assert_eq!(pl_path, temp_dir.join("_playlist.m3u8"));
        assert!(pl_path.exists());
        let content = fs::read_to_string(&pl_path).unwrap();
        assert!(content.starts_with("#EXTM3U\n"));
        assert!(content.contains("01 Track One.flac\n02 Track Two.flac\n"));

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_normalize_quality() {
        assert_eq!(normalize_quality("FLAC (24/192khz)"), "HI_RES_LOSSLESS");
        assert_eq!(normalize_quality("HI_RES_LOSSLESS"), "HI_RES_LOSSLESS");
        assert_eq!(normalize_quality("FLAC (16/44.1khz)"), "LOSSLESS");
        assert_eq!(normalize_quality("LOSSLESS"), "LOSSLESS");
        assert_eq!(normalize_quality("MP3 (320kbps)"), "HIGH");
        assert_eq!(normalize_quality("HIGH"), "HIGH");
        assert_eq!(normalize_quality("MP3 (96kbps)"), "LOW");
        assert_eq!(normalize_quality("LOW"), "LOW");
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
