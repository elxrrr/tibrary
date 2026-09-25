use base64::prelude::*;
use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

use crate::db::TursoDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TidalArtist {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TidalTrackSearchResult {
    pub id: String,
    pub title: String,
    pub isrc: Option<String>,
    pub album_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TidalTrack {
    pub id: String,
    pub title: String,
    pub isrc: Option<String>,
    pub track_number: u32,
    pub disc_number: u32,
    pub duration: f64,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub key_scale: Option<String>,
    pub copyright: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TidalRelease {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub date: String,
    pub r#type: String,
    pub available: Option<bool>,
    pub track_count: usize,
    pub explicit: bool,
    pub copyright: Option<String>,
    pub label: Option<String>,
    pub quality: String,
    pub tracks: Vec<TidalTrack>,
    pub tracks_loaded: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TidalCatalogue {
    pub id: String,
    pub name: String,
    pub releases: Vec<TidalRelease>,
}

type CachedToken = (String, String, String, f64);
type CachedCredentials = (std::time::Instant, Option<(String, String)>);
static CREDENTIAL_CACHE: std::sync::Mutex<Option<CachedCredentials>> = std::sync::Mutex::new(None);
static TOKEN_CACHE: std::sync::Mutex<Option<CachedToken>> = std::sync::Mutex::new(None);

pub fn catalogue_next(current: &str, next: &str) -> Result<String, String> {
    let current = Url::parse(current).map_err(|e| e.to_string())?;
    let mut target = current.join(next).map_err(|e| e.to_string())?;
    if target.scheme() != "https"
        || target.host_str() != Some("openapi.tidal.com")
        || target.port().is_some_and(|p| p != 443)
        || !target.username().is_empty()
        || target.password().is_some()
    {
        return Err("Rejected unexpected catalogue pagination destination".into());
    }
    if !target.path().starts_with("/v2/") {
        target.set_path(&format!("/v2{}", target.path()));
    }
    if !target.query_pairs().any(|(k, _)| k == "countryCode") {
        if let Some((_, market)) = current.query_pairs().find(|(k, _)| k == "countryCode") {
            target.query_pairs_mut().append_pair("countryCode", &market);
        }
    }
    Ok(target.to_string())
}

pub struct TidalClient {
    pub client_id: String,
    pub client_secret: String,
    http: reqwest::Client,
    token: Option<String>,
    expires_at: f64,
    request_spacing: Duration,
    attempts: usize,
}

impl TidalClient {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            http,
            token: None,
            expires_at: 0.0,
            request_spacing: Duration::from_millis(350),
            attempts: 3,
        }
    }

    pub async fn from_db(db: &TursoDb) -> Result<Self, String> {
        let mut client =
            Self::from_env_or_keychain().ok_or("Configure catalogue credentials in Settings")?;
        let settings = db.get_preference("provider").await?.unwrap_or(Value::Null);
        client.request_spacing = Duration::from_millis(
            settings["request_interval_ms"]
                .as_u64()
                .unwrap_or(750)
                .clamp(100, 10000),
        );
        client.attempts = settings["request_attempts"]
            .as_u64()
            .unwrap_or(2)
            .clamp(1, 5) as usize;
        let timeout = settings["request_timeout_sec"]
            .as_u64()
            .unwrap_or(20)
            .clamp(5, 120);
        client.http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(client)
    }

    pub fn from_env_or_keychain() -> Option<Self> {
        // 1. Environment variables
        if let (Ok(id), Ok(sec)) = (
            std::env::var("TIDAL_CLIENT_ID"),
            std::env::var("TIDAL_CLIENT_SECRET"),
        ) {
            let id = id.trim().to_string();
            let sec = sec.trim().to_string();
            if !id.is_empty() && !sec.is_empty() {
                return Some(Self::new(id, sec));
            }
        }

        if std::env::var_os("TIBRARY_TEST_MODE").is_some() {
            return None;
        }

        {
            let cache = CREDENTIAL_CACHE.lock().unwrap();
            if let Some((checked, credentials)) = cache.as_ref() {
                if checked.elapsed() < Duration::from_secs(30) {
                    return credentials
                        .as_ref()
                        .map(|(id, secret)| Self::new(id, secret));
                }
            }
        }
        let result = Self::read_keychain();
        *CREDENTIAL_CACHE.lock().unwrap() = Some((
            std::time::Instant::now(),
            result
                .as_ref()
                .map(|c| (c.client_id.clone(), c.client_secret.clone())),
        ));
        result
    }

    fn read_keychain() -> Option<Self> {
        // 2. macOS Keychain
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("security")
                .args([
                    "find-generic-password",
                    "-s",
                    "Tibrary",
                    "-a",
                    "catalogue-client",
                    "-w",
                ])
                .output()
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if let Ok(val) = serde_json::from_str::<Value>(stdout.trim()) {
                        if let (Some(id), Some(sec)) = (
                            val.get("id").and_then(|v| v.as_str()),
                            val.get("secret").and_then(|v| v.as_str()),
                        ) {
                            let id = id.trim().to_string();
                            let sec = sec.trim().to_string();
                            if !id.is_empty() && !sec.is_empty() {
                                return Some(Self::new(id, sec));
                            }
                        }
                    }
                    // Migrate the split credential format written by the early Rust UI.
                    if !stdout.trim().is_empty() && !stdout.trim().starts_with('{') {
                        if let Ok(secret) = std::process::Command::new("security")
                            .args([
                                "find-generic-password",
                                "-s",
                                "Tibrary",
                                "-a",
                                "catalogue-secret",
                                "-w",
                            ])
                            .output()
                        {
                            if secret.status.success() {
                                return Some(Self::new(
                                    stdout.trim(),
                                    String::from_utf8_lossy(&secret.stdout).trim(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    pub fn save_credentials(client: &str, secret: &str) -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            if client.trim().is_empty() || secret.trim().is_empty() {
                return Err(std::io::Error::other("Client ID and secret are required"));
            }
            let payload =
                serde_json::json!({"id":client.trim(),"secret":secret.trim()}).to_string();
            let output = std::process::Command::new("security")
                .args([
                    "add-generic-password",
                    "-U",
                    "-s",
                    "Tibrary",
                    "-a",
                    "catalogue-client",
                    "-w",
                    &payload,
                ])
                .output()?;
            if !output.status.success() {
                return Err(std::io::Error::other(
                    "macOS Keychain could not save the credentials",
                ));
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (client, secret);
        }
        *CREDENTIAL_CACHE.lock().unwrap() = None;
        *TOKEN_CACHE.lock().unwrap() = None;
        Ok(())
    }

    pub fn forget_credentials() -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("security")
                .args([
                    "delete-generic-password",
                    "-s",
                    "Tibrary",
                    "-a",
                    "catalogue-client",
                ])
                .output();
            let _ = std::process::Command::new("security")
                .args([
                    "delete-generic-password",
                    "-s",
                    "Tibrary",
                    "-a",
                    "catalogue-secret",
                ])
                .output();
        }
        *CREDENTIAL_CACHE.lock().unwrap() = None;
        *TOKEN_CACHE.lock().unwrap() = None;
        Ok(())
    }

    pub async fn authenticate(&mut self) -> Result<String, String> {
        let auth_val = format!("{}:{}", self.client_id, self.client_secret);
        let encoded = BASE64_STANDARD.encode(auth_val.as_bytes());

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {}", encoded))
                .map_err(|e| format!("Invalid auth header: {}", e))?,
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        let res = self
            .http
            .post("https://auth.tidal.com/v1/oauth2/token")
            .headers(headers)
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(|e| format!("Authentication request failed: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(format!(
                "TIDAL authentication rejected (HTTP {}): {}",
                status, body
            ));
        }

        let val: Value = res
            .json()
            .await
            .map_err(|e| format!("Invalid auth JSON response: {}", e))?;

        let token = val
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing access_token in response".to_string())?
            .to_string();

        let expires_in = val
            .get("expires_in")
            .and_then(|v| v.as_f64())
            .unwrap_or(3600.0);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        self.expires_at = now + expires_in - 30.0; // Refresh 30s before expiry
        self.token = Some(token.clone());
        *TOKEN_CACHE.lock().unwrap() = Some((
            self.client_id.clone(),
            self.client_secret.clone(),
            token.clone(),
            self.expires_at,
        ));

        Ok(token)
    }

    pub async fn get_token(&mut self) -> Result<String, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        if let Some(ref t) = self.token {
            if now < self.expires_at {
                return Ok(t.clone());
            }
        }

        if let Some((id, secret, token, expiry)) = TOKEN_CACHE.lock().unwrap().as_ref() {
            if id == &self.client_id && secret == &self.client_secret && now < *expiry {
                self.token = Some(token.clone());
                self.expires_at = *expiry;
                return Ok(token.clone());
            }
        }
        self.authenticate().await
    }

    pub async fn get_json(&mut self, url: &str) -> Result<Value, String> {
        let parsed = Url::parse(url).map_err(|e| e.to_string())?;
        if parsed.scheme() != "https"
            || parsed.host_str() != Some("openapi.tidal.com")
            || !parsed.path().starts_with("/v2/")
        {
            return Err("Rejected unexpected catalogue URL".into());
        }
        static PACER: std::sync::OnceLock<tokio::sync::Mutex<std::time::Instant>> =
            std::sync::OnceLock::new();
        for attempt in 0..self.attempts {
            let mut last = PACER
                .get_or_init(|| {
                    tokio::sync::Mutex::new(std::time::Instant::now() - Duration::from_secs(1))
                })
                .lock()
                .await;
            let wait = self.request_spacing.saturating_sub(last.elapsed());
            tokio::time::sleep(wait).await;
            *last = std::time::Instant::now();
            drop(last);
            let token = self.get_token().await?;
            let res = self
                .http
                .get(url)
                .bearer_auth(token)
                .header(ACCEPT, "application/vnd.api+json")
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = res.status();
            if status.is_success() {
                return res
                    .json()
                    .await
                    .map_err(|e| format!("Invalid catalogue response: {e}"));
            }
            if status.as_u16() == 401 && attempt == 0 {
                self.token = None;
                *TOKEN_CACHE.lock().unwrap() = None;
                continue;
            }
            if (status.as_u16() == 429 || status.is_server_error()) && attempt + 1 < self.attempts {
                let wait = res
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(4 * (attempt as u64 + 1));
                if wait > 60 {
                    return Err(format!("Service rate limit: retry after {wait} seconds"));
                }
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            let body: Value = res.json().await.unwrap_or(Value::Null);
            let detail = body["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["detail"].as_str().or(e["title"].as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "Catalogue {} failed (HTTP {}){}",
                parsed.path(),
                status.as_u16(),
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail.chars().take(400).collect::<String>())
                }
            ));
        }
        Err("Catalogue retry limit reached".into())
    }

    pub async fn search_artists(
        &mut self,
        query: &str,
        market: &str,
    ) -> Result<Vec<TidalArtist>, String> {
        let mut url =
            Url::parse("https://openapi.tidal.com/v2/searchResults").map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("filter[query]", query)
            .append_pair("countryCode", market);

        let payload = self.get_json(url.as_str()).await?;
        let data = match payload.get("data") {
            Some(Value::Array(arr)) => arr.clone(),
            Some(Value::Object(_)) => vec![payload["data"].clone()],
            _ => return Ok(Vec::new()),
        };

        let mut artists = Vec::new();
        for item in data {
            let search_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if search_id.is_empty() {
                continue;
            }

            let rel_url = format!(
                "https://openapi.tidal.com/v2/searchResults/{}/relationships/artists?countryCode={}&include=artists",
                urlencoding_encode(search_id),
                market
            );

            {
                let rel_payload = self.get_json(&rel_url).await?;
                if let Some(included) = rel_payload.get("included").and_then(|v| v.as_array()) {
                    for inc in included {
                        if inc.get("type").and_then(|v| v.as_str()) == Some("artists") {
                            let id = inc
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = inc
                                .get("attributes")
                                .and_then(|a| a.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !id.is_empty() && !name.is_empty() {
                                artists.push(TidalArtist { id, name });
                            }
                        }
                    }
                }
            }
        }

        Ok(artists)
    }

    pub async fn search_tracks(
        &mut self,
        query: &str,
        market: &str,
    ) -> Result<Vec<TidalTrackSearchResult>, String> {
        let mut url =
            Url::parse("https://openapi.tidal.com/v2/searchResults").map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("filter[query]", query)
            .append_pair("countryCode", market);

        let payload = self.get_json(url.as_str()).await?;
        let data = match payload.get("data") {
            Some(Value::Array(arr)) => arr.clone(),
            Some(Value::Object(_)) => vec![payload["data"].clone()],
            _ => return Ok(Vec::new()),
        };

        let mut results = Vec::new();
        for item in data {
            let search_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if search_id.is_empty() {
                continue;
            }

            let rel_url = format!(
                "https://openapi.tidal.com/v2/searchResults/{}/relationships/tracks?countryCode={}&include=tracks",
                urlencoding_encode(search_id),
                market
            );

            {
                let rel_payload = self.get_json(&rel_url).await?;
                if let Some(included) = rel_payload.get("included").and_then(|v| v.as_array()) {
                    for inc in included {
                        if inc.get("type").and_then(|v| v.as_str()) == Some("tracks") {
                            let id = inc
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let title_val = inc
                                .get("attributes")
                                .and_then(|a| a.get("title"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let isrc = inc
                                .get("attributes")
                                .and_then(|a| a.get("isrc"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let mut album_ids = Vec::new();
                            if let Some(albums) = inc
                                .get("relationships")
                                .and_then(|r| r.get("albums"))
                                .and_then(|a| a.get("data"))
                                .and_then(|d| d.as_array())
                            {
                                for alb in albums {
                                    if let Some(aid) = alb.get("id").and_then(|v| v.as_str()) {
                                        album_ids.push(aid.to_string());
                                    }
                                }
                            }
                            if !id.is_empty() {
                                results.push(TidalTrackSearchResult {
                                    id,
                                    title: title_val,
                                    isrc,
                                    album_ids,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    pub async fn get_artist_catalogue(
        &mut self,
        artist_id: &str,
        market: &str,
        detailed: bool,
    ) -> Result<TidalCatalogue, String> {
        let artist_url = format!(
            "https://openapi.tidal.com/v2/artists/{}?countryCode={}",
            artist_id, market
        );
        let artist_payload = self.get_json(&artist_url).await?;
        let artist_name = artist_payload
            .get("data")
            .and_then(|d| d.get("attributes"))
            .and_then(|a| a.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut next_url = Some(format!(
            "https://openapi.tidal.com/v2/artists/{}/relationships/albums?countryCode={}&include=albums",
            artist_id, market
        ));

        let mut releases = Vec::new();
        let mut visited = std::collections::HashSet::new();
        while let Some(current_url) = next_url.take() {
            if !visited.insert(current_url.clone()) {
                return Err("Catalogue pagination repeated a page".into());
            }
            let payload = self.get_json(&current_url).await?;

            if let Some(included) = payload.get("included").and_then(|v| v.as_array()) {
                for item in included {
                    if item.get("type").and_then(|v| v.as_str()) != Some("albums") {
                        continue;
                    }

                    let rel_id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let attrs = item.get("attributes").cloned().unwrap_or(Value::Null);

                    let title_raw = attrs.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    let version_raw = attrs.get("version").and_then(|v| v.as_str());
                    let title = format_title(title_raw, version_raw);

                    let date = attrs
                        .get("releaseDate")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let rel_type = attrs
                        .get("albumType")
                        .or_else(|| attrs.get("type"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("ALBUM")
                        .to_string();

                    let track_count = attrs
                        .get("numberOfItems")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;

                    let availability = attrs.get("availability").and_then(|v| v.as_array());
                    let is_available = availability.map(|arr| {
                        arr.iter()
                            .any(|s| s.as_str() == Some("STREAM") || s.as_str() == Some("DJ"))
                    });

                    let explicit = attrs
                        .get("explicit")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let copyright = attrs
                        .get("copyright")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let label = attrs
                        .get("recordLabel")
                        .or_else(|| attrs.get("label"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let quality = attrs
                        .get("mediaTags")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();

                    let mut release = TidalRelease {
                        id: rel_id.clone(),
                        artist: artist_name.clone(),
                        title,
                        date,
                        r#type: rel_type,
                        available: is_available,
                        track_count,
                        explicit,
                        copyright,
                        label,
                        quality,
                        tracks: Vec::new(),
                        tracks_loaded: false,
                    };

                    if detailed {
                        {
                            let tracks = self.get_release_details(&rel_id, market).await?;
                            release.track_count = tracks.len();
                            release.tracks = tracks;
                            release.tracks_loaded = true;
                        }
                    }

                    releases.push(release);
                }
            }

            next_url = payload["links"]["next"]
                .as_str()
                .or_else(|| payload["links"]["next"]["href"].as_str())
                .map(|next| catalogue_next(&current_url, next))
                .transpose()
                .map_err(|e| e.to_string())?;
        }

        Ok(TidalCatalogue {
            id: artist_id.to_string(),
            name: artist_name,
            releases,
        })
    }

    pub async fn get_release_details(
        &mut self,
        release_id: &str,
        market: &str,
    ) -> Result<Vec<TidalTrack>, String> {
        let mut next=Some(format!("https://openapi.tidal.com/v2/albums/{release_id}/relationships/items?countryCode={market}&include=items"));
        let mut tracks = Vec::new();
        let mut visited = std::collections::HashSet::new();
        while let Some(url) = next {
            if !visited.insert(url.clone()) {
                return Err("Catalogue pagination repeated a page".into());
            }
            let payload = self.get_json(&url).await?;
            tracks.extend(parse_release_tracks(&payload)?);
            next = payload["links"]["next"]
                .as_str()
                .or_else(|| payload["links"]["next"]["href"].as_str())
                .map(|n| catalogue_next(&url, n))
                .transpose()
                .map_err(|e| e.to_string())?;
        }
        tracks.sort_by_key(|t| (t.disc_number, t.track_number));
        tracks.dedup_by(|a, b| {
            a.id == b.id && a.disc_number == b.disc_number && a.track_number == b.track_number
        });
        Ok(tracks)
    }

    pub async fn save_catalogue_to_db(
        &self,
        db: &TursoDb,
        market: &str,
        catalogue: &TidalCatalogue,
    ) -> Result<(), String> {
        let conn = db.connect()?;
        let mut payload = serde_json::to_value(catalogue).map_err(|e| e.to_string())?;
        let mut previous = conn
            .query(
                "SELECT payload FROM catalogue WHERE artist_id=? AND market=?",
                (catalogue.id.as_str(), market),
            )
            .await
            .map_err(|e| e.to_string())?;
        if let Some(row) = previous.next().await.map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            if let Ok(old) = serde_json::from_str::<Value>(&raw) {
                if let (Some(fresh), Some(prior)) = (
                    payload["releases"].as_array_mut(),
                    old["releases"].as_array(),
                ) {
                    for release in fresh {
                        if let Some(cached) = prior.iter().find(|r| r["id"] == release["id"]) {
                            if release["tracks_loaded"] != true && cached["tracks_loaded"] == true {
                                release["tracks"] = cached["tracks"].clone();
                                release["tracks_loaded"] = json!(true);
                                release["track_count"] = cached["track_count"].clone();
                            }
                        }
                    }
                }
            }
        }
        drop(previous);
        let json_payload = payload.to_string();
        let now_str = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT OR REPLACE INTO catalogue (artist_id, market, payload, fetched) VALUES (?, ?, ?, ?)",
            (catalogue.id.as_str(), market, json_payload.as_str(), now_str.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;

        db.bump_revision();
        Ok(())
    }
}

pub fn parse_release_tracks(payload: &Value) -> Result<Vec<TidalTrack>, String> {
    let included = payload["included"]
        .as_array()
        .ok_or("Release details missing included items")?;
    let data = payload["data"]
        .as_array()
        .ok_or("Release details missing item order")?;
    let mut tracks = vec![];
    for reference in data {
        if reference["type"] != "tracks" {
            continue;
        }
        let item = included
            .iter()
            .find(|i| i["id"] == reference["id"] && i["type"] == "tracks")
            .ok_or("Incomplete release track details")?;
        let a = &item["attributes"];
        let m = &reference["meta"];
        tracks.push(TidalTrack {
            id: item["id"].as_str().ok_or("Missing track ID")?.into(),
            title: format_title(a["title"].as_str().unwrap_or(""), a["version"].as_str()),
            isrc: a["isrc"].as_str().map(str::to_owned),
            track_number: m["trackNumber"]
                .as_u64()
                .or_else(|| a["trackNumber"].as_u64())
                .unwrap_or(0) as u32,
            disc_number: m["volumeNumber"]
                .as_u64()
                .or_else(|| a["volumeNumber"].as_u64())
                .unwrap_or(1) as u32,
            duration: a["duration"]
                .as_str()
                .and_then(parse_iso8601_duration)
                .unwrap_or(0.),
            bpm: a["bpm"].as_f64(),
            key: a["key"].as_str().map(str::to_owned),
            key_scale: a["keyScale"].as_str().map(str::to_owned),
            copyright: a["copyright"]
                .as_str()
                .or_else(|| a["copyright"]["text"].as_str())
                .map(str::to_owned),
        });
    }
    Ok(tracks)
}

pub fn format_title(title: &str, version: Option<&str>) -> String {
    if let Some(ver) = version {
        let trimmed = ver.trim();
        if !trimmed.is_empty() && !title.to_lowercase().contains(&trimmed.to_lowercase()) {
            return format!("{} ({})", title, trimmed);
        }
    }
    title.to_string()
}

pub fn parse_iso8601_duration(value: &str) -> Option<f64> {
    if !value.starts_with("PT") {
        return None;
    }
    let rest = &value[2..];
    let mut total_secs = 0.0;
    let mut current_num = String::new();

    for c in rest.chars() {
        if c.is_ascii_digit() || c == '.' {
            current_num.push(c);
        } else {
            let n: f64 = current_num.parse().ok()?;
            current_num.clear();
            match c {
                'H' => total_secs += n * 3600.0,
                'M' => total_secs += n * 60.0,
                'S' => total_secs += n,
                _ => return None,
            }
        }
    }
    Some(total_secs)
}

fn urlencoding_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn pagination_preserves_api_prefix_and_market() {
        let current =
            "https://openapi.tidal.com/v2/artists/123/relationships/albums?countryCode=GB";
        assert_eq!(
            super::catalogue_next(current, "/artists/123/relationships/albums?page=2").unwrap(),
            "https://openapi.tidal.com/v2/artists/123/relationships/albums?page=2&countryCode=GB"
        );
        assert!(super::catalogue_next(current, "https://example.com/page").is_err());
        assert!(super::catalogue_next(current, "http://openapi.tidal.com/page").is_err());
    }

    #[test]
    fn release_relationship_positions_exclude_video() {
        let payload = serde_json::json!({"data":[{"id":"t","type":"tracks","meta":{"trackNumber":5,"volumeNumber":2}},{"id":"v","type":"videos"}],"included":[{"id":"t","type":"tracks","attributes":{"title":"Song","duration":"PT3M","trackNumber":1}}]});
        let tracks = super::parse_release_tracks(&payload).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!((tracks[0].disc_number, tracks[0].track_number), (2, 5));
        assert_eq!(tracks[0].duration, 180.0);
    }

    use super::*;

    #[test]
    fn test_format_title() {
        assert_eq!(format_title("Bohemian Rhapsody", None), "Bohemian Rhapsody");
        assert_eq!(
            format_title("Bohemian Rhapsody", Some("2011 Remaster")),
            "Bohemian Rhapsody (2011 Remaster)"
        );
        assert_eq!(
            format_title("Bohemian Rhapsody (Remastered)", Some("Remastered")),
            "Bohemian Rhapsody (Remastered)"
        );
    }

    #[test]
    fn test_parse_iso8601_duration() {
        assert_eq!(parse_iso8601_duration("PT3M45S"), Some(225.0));
        assert_eq!(parse_iso8601_duration("PT1H2M3S"), Some(3723.0));
        assert_eq!(parse_iso8601_duration("PT45S"), Some(45.0));
        assert_eq!(parse_iso8601_duration("PT3M45.5S"), Some(225.5));
        assert_eq!(parse_iso8601_duration("INVALID"), None);
    }

    #[test]
    #[ignore = "Explicit credential-store integration check"]
    fn test_from_env_or_keychain() {
        // Test loading client from Keychain / env
        let client = TidalClient::from_env_or_keychain();
        if let Some(c) = client {
            assert!(!c.client_id.is_empty());
            assert!(!c.client_secret.is_empty());
        }
    }

    #[tokio::test]
    #[ignore = "Explicit live catalogue check; uses saved credentials and API quota"]
    async fn test_online_search_artists() {
        if let Some(mut client) = TidalClient::from_env_or_keychain() {
            let artists = client.search_artists("100 gecs", "GB").await.unwrap();
            assert!(!artists.is_empty(), "Should find artists for '100 gecs'");
            println!("Found {} artists: {:?}", artists.len(), artists);
            let gecs = artists.iter().find(|a| a.name.to_lowercase() == "100 gecs");
            assert!(gecs.is_some(), "Should find 100 gecs artist");
            let artist = gecs.unwrap();
            let cat = client
                .get_artist_catalogue(&artist.id, "GB", false)
                .await
                .unwrap();
            assert!(
                !cat.releases.is_empty(),
                "Should find releases for 100 gecs"
            );
            println!("Found {} releases for {}:", cat.releases.len(), cat.name);
            for r in &cat.releases {
                println!("  - [{}] {} ({})", r.id, r.title, r.date);
            }
        }
    }
}
