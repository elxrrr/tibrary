use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

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
    #[serde(default, deserialize_with = "copyright_from_value")]
    pub copyright: Option<String>,
    #[serde(default, deserialize_with = "nullable_vec")]
    pub media_tags: Vec<String>,
    #[serde(default, deserialize_with = "nullable_vec")]
    pub audio_modes: Vec<String>,
    pub media_metadata: Value,
    #[serde(default, deserialize_with = "nullable_vec")]
    pub genres: Vec<String>,
    pub replacement_id: Option<String>,
    pub discovery_checked_at: Option<i64>,
    pub credits: Value,
    pub credits_complete: bool,
    pub credits_checked_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TidalRelease {
    pub id: String,
    #[serde(deserialize_with = "artist_name_from_value")]
    pub artist: String,
    pub title: String,
    pub date: String,
    pub r#type: String,
    pub available: Option<bool>,
    pub track_count: usize,
    pub explicit: bool,
    #[serde(default, deserialize_with = "copyright_from_value")]
    pub copyright: Option<String>,
    pub label: Option<String>,
    pub quality: String,
    #[serde(default, deserialize_with = "nullable_vec")]
    pub tracks: Vec<TidalTrack>,
    pub tracks_loaded: bool,
    pub upc: Option<String>,
    pub release_group_id: Option<String>,
    pub primary_type: Option<String>,
    #[serde(default, deserialize_with = "nullable_vec")]
    pub secondary_types: Vec<String>,
    #[serde(default, deserialize_with = "nullable_vec")]
    pub artist_credits: Vec<String>,
    pub primary_artist_verified: bool,
    #[serde(default, deserialize_with = "optional_bool_from_value")]
    pub official: Option<bool>,
    #[serde(default, deserialize_with = "nullable_vec")]
    pub genres: Vec<String>,
    pub replacement_id: Option<String>,
    pub discovery_checked_at: Option<i64>,
    pub original_release_date: Option<String>,
    #[serde(default, deserialize_with = "nullable_vec")]
    pub audio_modes: Vec<String>,
    pub media_metadata: Value,
}

fn nullable_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

fn artist_name_from_value<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::String(name) => name,
        Value::Object(artist) => artist
            .get("name")
            .or_else(|| artist.get("title"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Value::Null => String::new(),
        _ => String::new(),
    })
}

fn copyright_from_value<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text)),
        Value::Object(fields) => Ok(fields
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)),
        _ => Err(serde::de::Error::custom(
            "copyright must be text or an object with a text field",
        )),
    }
}

fn optional_bool_from_value<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Bool(flag) => Some(flag),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    })
}

#[cfg(test)]
mod release_payload_tests {
    #[test]
    fn discovery_relationships_do_not_leak_between_resources() {
        use super::{related_genres, replacement_id};
        use serde_json::json;
        let included = vec![
            json!({"type":"genres","id":"g","attributes":{"name":"Electronic"}}),
            json!({"type":"genres","id":"other","attributes":{"name":"Rock"}}),
        ];
        let resource = json!({"id":"1","relationships":{"genres":{"data":[{"type":"genres","id":"g"}]},"replacement":{"data":{"type":"albums","id":"2"}}}});
        assert_eq!(related_genres(&resource, &included), vec!["Electronic"]);
        assert_eq!(replacement_id(&resource).as_deref(), Some("2"));
        assert!(related_genres(&json!({}), &included).is_empty());
        assert!(replacement_id(
            &json!({"id":"1","relationships":{"replacement":{"data":{"id":"1"}}}})
        )
        .is_none());
    }

    #[test]
    fn compound_credits_stay_with_their_track_and_preserve_roles() {
        let payload = serde_json::json!({"data":[{"id":"one","type":"tracks"},{"id":"two","type":"tracks"}],"included":[
            {"id":"one","type":"tracks","attributes":{"title":"Song"},"relationships":{"credits":{"data":[{"id":"writer","type":"credits"}],"links":{"next":"?page[cursor]=next"}}}},
            {"id":"two","type":"tracks","attributes":{"title":"Other"},"relationships":{"credits":{"data":[]}}},
            {"id":"writer","type":"credits","attributes":{"name":"Example Person","role":"Composer"},"relationships":{"category":{"data":{"id":"writing"}},"artist":{"data":{"id":"artist-id"}}}},
            {"id":"unrelated","type":"credits","attributes":{"name":"Unrelated Person","role":"Producer"}}
        ]});
        let tracks = super::parse_release_tracks(&payload).unwrap();
        assert_eq!(tracks[0].credits.as_array().unwrap().len(), 1);
        assert_eq!(tracks[0].credits[0]["role"], "Composer");
        assert_eq!(tracks[0].credits[0]["roleId"], "writing");
        assert_eq!(tracks[0].credits[0]["artist_id"], "artist-id");
        assert!(!tracks[0].credits_complete);
        assert_eq!(tracks[1].credits, serde_json::json!([]));
        assert!(tracks[1].credits_complete);
    }
    #[test]
    fn release_artist_accepts_cached_object_or_string() {
        let object: super::TidalRelease = serde_json::from_value(serde_json::json!({
            "id":"123", "artist":{"id":"42","name":"Album Artist"}, "title":"Example"
        }))
        .unwrap();
        assert_eq!(object.artist, "Album Artist");
        let string: super::TidalRelease = serde_json::from_value(serde_json::json!({
            "id":"123", "artist":"Album Artist", "title":"Example"
        }))
        .unwrap();
        assert_eq!(string.artist, "Album Artist");
    }
    #[test]
    fn cached_catalogue_copyright_accepts_provider_text_object() {
        let release: super::TidalRelease = serde_json::from_value(serde_json::json!({
            "id":"378495652", "artist":"Example", "title":"Example release",
            "copyright":{"text":"Armada Music B.V."},
            "tracks":[{"id":"378495656","title":"Example track","copyright":{"text":"Armada Music B.V."}}]
        })).unwrap();
        assert_eq!(release.copyright.as_deref(), Some("Armada Music B.V."));
        assert_eq!(
            release.tracks[0].copyright.as_deref(),
            Some("Armada Music B.V.")
        );
    }
    #[test]
    fn release_tracks_keep_optional_audio_and_credit_metadata() {
        let payload = serde_json::json!({
            "data":[{"id":"track-1","type":"tracks","meta":{"trackNumber":1,"volumeNumber":1}}],
            "included":[{"id":"track-1","type":"tracks","attributes":{
                "title":"Example", "duration":"PT3M10S", "isrc":"GBABC1234567",
                "mediaTags":["HIRES_LOSSLESS"], "audioModes":["STEREO"],
                "mediaMetadata":{"sampleRate":96000,"bitDepth":24},
                "credits":[{"roleId":"producer","name":"Example Producer"}]
            }}]
        });
        let tracks = super::parse_release_tracks(&payload).unwrap();
        assert_eq!(tracks[0].isrc.as_deref(), Some("GBABC1234567"));
        assert_eq!(tracks[0].media_tags, vec!["HIRES_LOSSLESS"]);
        assert_eq!(tracks[0].media_metadata["bitDepth"], 24);
        assert_eq!(tracks[0].credits[0]["roleId"], "producer");
        let cached: super::TidalTrack =
            serde_json::from_value(serde_json::json!({"id":"legacy","title":"Legacy"})).unwrap();
        assert!(cached.audio_modes.is_empty());
    }
    #[test]
    fn saved_catalogue_fixture_deserializes() {
        let Ok(path) = std::env::var("TIBRARY_CATALOGUE_JSON") else {
            return;
        };
        let raw = std::fs::read_to_string(path).unwrap();
        let catalogue: super::TidalCatalogue = serde_json::from_str(&raw).unwrap();
        assert!(!catalogue.releases.is_empty());
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TidalCatalogue {
    pub id: String,
    pub name: String,
    pub releases: Vec<TidalRelease>,
}

#[cfg(test)]
#[test]
fn retry_delay_accepts_seconds_dates_and_fallback() {
    assert_eq!(retry_delay(Some("12"), 0), Duration::from_secs(12));
    assert_eq!(retry_delay(Some("invalid"), 1), Duration::from_secs(4));
    let future = (Utc::now() + chrono::Duration::seconds(30)).to_rfc2822();
    assert!((29..=30).contains(&retry_delay(Some(&future), 0).as_secs()));
}

#[cfg(test)]
fn retry_delay(header: Option<&str>, attempt: usize) -> Duration {
    crate::network::retry_after(header, attempt)
}

#[derive(Clone)]
pub struct TidalClient {
    db: TursoDb,
    http: reqwest::Client,
    request_spacing: Duration,
    attempts: usize,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl TidalClient {
    pub async fn from_db(db: &TursoDb) -> Result<Self, String> {
        let settings = db.get_preference("provider").await?.unwrap_or(Value::Null);
        Ok(Self {
            db: db.clone(),
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            http: crate::network::client(
                settings["request_timeout_sec"]
                    .as_u64()
                    .unwrap_or(20)
                    .clamp(5, 120),
            )?,
            request_spacing: Duration::from_millis(
                settings["request_interval_ms"]
                    .as_u64()
                    .unwrap_or(350)
                    .clamp(100, 10000),
            ),
            attempts: settings["request_attempts"]
                .as_u64()
                .unwrap_or(2)
                .clamp(1, 5) as usize,
        })
    }

    pub fn with_cancel(mut self, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cancel = cancel;
        self
    }

    pub async fn authenticate(&mut self) -> Result<String, String> {
        crate::stream_download::get_valid_token(&self.db, &self.http).await
    }

    /// All network requests use the single subscriber session. Paths are internal,
    /// never taken from response links, so bearer tokens cannot leave the API host.
    async fn request(
        &mut self,
        path: &str,
        market: &str,
        params: &[(&str, String)],
    ) -> Result<Value, String> {
        if market.len() != 2 || !market.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err("Choose a two-letter country code in Settings".into());
        }
        if path.contains('?') || path.contains('#') || path.contains("..") {
            return Err("Invalid catalogue path".into());
        }
        let token = self.authenticate().await?;
        let response = crate::network::get(
            self.http
                .get(format!("https://api.tidal.com/v1/{path}"))
                .query(&[("countryCode", market)])
                .query(params)
                .bearer_auth(token),
            self.request_spacing,
            self.attempts,
            Some(self.cancel.as_ref()),
        )
        .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "Subscriber catalogue {path} failed (HTTP {})",
                status.as_u16()
            ));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Invalid subscriber catalogue response: {e}"))
    }

    async fn pages(
        &mut self,
        path: &str,
        market: &str,
        params: &[(&str, String)],
    ) -> Result<Vec<Value>, String> {
        let mut result = Vec::new();
        let mut offset = 0;
        let mut seen = std::collections::HashSet::new();
        loop {
            let mut query = params.to_vec();
            query.extend([("limit", "100".into()), ("offset", offset.to_string())]);
            let payload = self.request(path, market, &query).await?;
            let page = payload["items"]
                .as_array()
                .ok_or("Catalogue response has no items")?;
            let total = payload["totalNumberOfItems"].as_u64();
            if page.is_empty() {
                if total.is_some_and(|n| n > offset as u64) {
                    return Err("Incomplete catalogue page; saved data retained".into());
                }
                break;
            }
            let signature = serde_json::to_string(page).map_err(|e| e.to_string())?;
            if !seen.insert(signature) {
                return Err("Catalogue repeated a page; saved data retained".into());
            }
            offset += page.len();
            result.extend(page.clone());
            if total.is_some_and(|n| offset as u64 >= n) || (total.is_none() && page.len() < 100) {
                break;
            }
            if offset > 100_000 {
                return Err("Catalogue pagination exceeded safety limit".into());
            }
        }
        Ok(result)
    }

    /// Batch release summaries feed the existing normalized resource contract.
    /// This avoids reimplementing availability, artist-credit and recommendation
    /// rules while keeping all HTTP traffic on the subscriber API.
    pub async fn albums(
        &mut self,
        ids: &[String],
        market: &str,
        force: bool,
    ) -> Result<Value, String> {
        let mut raw = Vec::new();
        let mut pending = Vec::new();
        let now = Utc::now().timestamp();
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                return Err("Invalid release ID".into());
            }
            if !seen.insert(id.clone()) {
                continue;
            }
            let cached = self
                .db
                .get_preference(&format!("subscriber-summary:{market}:{id}"))
                .await?;
            if let Some(value) = cached.filter(|v| {
                !force
                    && (v["artist"].is_object() || v["artists"].is_array())
                    && v["title"].is_string()
                    && v["checked_at"]
                        .as_i64()
                        .is_some_and(|at| (0..30 * 86400).contains(&(now - at)))
            }) {
                raw.push(value);
            } else {
                pending.push(id.clone());
            }
        }
        for batch in pending.chunks(20) {
            let payload = match self
                .request("albums", market, &[("ids", batch.join(","))])
                .await
            {
                Ok(payload) => payload,
                Err(error)
                    if ["(HTTP 400)", "(HTTP 404)", "(HTTP 500)"]
                        .iter()
                        .any(|code| error.ends_with(code)) =>
                {
                    // A stale ID can poison the subscriber batch. Isolate it,
                    // but never reinterpret authentication/throttling as absence.
                    // On a genuine outage the first failed single stops the job.
                    let mut items = Vec::new();
                    for id in batch {
                        match self.request(&format!("albums/{id}"), market, &[]).await {
                            Ok(item) => items.push(item),
                            Err(error) if error.ends_with("(HTTP 404)") => {}
                            Err(error) => return Err(error),
                        }
                    }
                    json!(items)
                }
                Err(error) => return Err(error),
            };
            let items = payload
                .as_array()
                .or_else(|| payload["items"].as_array())
                .ok_or("Invalid release summaries")?;
            for item in items {
                let id = resource_id(&item["id"]);
                if !batch.contains(&id) {
                    continue;
                }
                let mut item = item.clone();
                item["checked_at"] = json!(now);
                self.cache_summary(&item, market).await?;
                raw.push(item);
            }
        }
        Ok(album_resources(&raw))
    }

    async fn cache_summary(&self, raw: &Value, market: &str) -> Result<(), String> {
        let id = resource_id(&raw["id"]);
        self.db
            .set_preference(&format!("subscriber-summary:{market}:{id}"), raw)
            .await?;
        let ids: Vec<_> = album_artists(raw)
            .iter()
            .map(|a| resource_id(&a["id"]))
            .collect();
        if !ids.is_empty() {
            self.db.set_preference(&format!("release-artists:{market}:{id}"),&json!({"ids":ids,"names":album_artists(raw).iter().map(|a|a["name"].clone()).collect::<Vec<_>>(),"checked_at":raw["checked_at"],"source":"subscriber album credits"})).await?;
        }
        if let Some(available) = subscriber_available(raw) {
            self.db
                .set_preference(
                    &format!("release-live:{market}:{id}"),
                    &json!({"available":available,"checked_at":raw["checked_at"]}),
                )
                .await?;
        }
        Ok(())
    }

    /// Optional catalogue fields are accessible with the same subscriber token.
    /// Empty successful results are cached; rejected/failed optional access backs
    /// off globally so a library run cannot repeat a failure for every album.
    pub async fn discovery(
        &mut self,
        ids: &[String],
        market: &str,
        force: bool,
    ) -> Result<std::collections::HashMap<String, Value>, String> {
        let mut result = std::collections::HashMap::new();
        let mut pending = Vec::new();
        let now = Utc::now().timestamp();
        for id in ids {
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                return Err("Invalid release ID".into());
            }
            let saved = self
                .db
                .get_preference(&format!("subscriber-discovery:{market}:{id}"))
                .await?;
            if let Some(value) = saved.filter(|v| {
                !force
                    && v["checked_at"]
                        .as_i64()
                        .is_some_and(|at| (0..30 * 86400).contains(&(now - at)))
            }) {
                result.insert(id.clone(), value);
            } else if !pending.contains(id) {
                pending.push(id.clone());
            }
        }
        let pause_key = format!("subscriber-discovery-paused:{market}");
        if !force
            && self
                .db
                .get_preference(&pause_key)
                .await?
                .is_some_and(|v| v["until"].as_i64().is_some_and(|at| at > now))
        {
            return Ok(result);
        }
        for batch in pending.chunks(20) {
            let response = async {
                let token = self.authenticate().await?;
                let response = crate::network::get(
                    self.http
                        .get("https://openapi.tidal.com/v2/albums")
                        .query(&[
                            ("countryCode", market),
                            ("filter[id]", batch.join(",").as_str()),
                            ("include", "genres,replacement"),
                        ])
                        .bearer_auth(token)
                        .header("Accept", "application/vnd.api+json"),
                    self.request_spacing,
                    self.attempts,
                    Some(self.cancel.as_ref()),
                )
                .await?;
                if !response.status().is_success() {
                    return Err(format!(
                        "Optional catalogue fields: HTTP {}",
                        response.status().as_u16()
                    ));
                }
                let payload: Value = response.json().await.map_err(|e| e.to_string())?;
                if !payload["data"].is_array() {
                    return Err("Invalid optional catalogue metadata".to_string());
                }
                Ok(payload)
            }
            .await;
            let payload = match response {
                Ok(payload) => payload,
                Err(error) if self.cancel.load(std::sync::atomic::Ordering::Relaxed) => return Err(error),
                Err(error) => {
                    self.db
                        .set_preference(&pause_key, &json!({"until":now+3600,"message":error}))
                        .await?;
                    break;
                }
            };
            let included = payload["included"].as_array().cloned().unwrap_or_default();
            for id in batch {
                let resource = payload["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["id"].as_str() == Some(id));
                let value = if let Some(resource) = resource {
                    let attrs = &resource["attributes"];
                    json!({"checked_at":now,"genres":related_genres(resource,&included),"replacement_id":replacement_id(resource),
                        "label":attrs["recordLabel"].as_str().or(attrs["recordLabel"]["name"].as_str()),
                        "official":attrs["official"],"original_release_date":attrs["originalReleaseDate"],"source":"subscriber"})
                } else {
                    json!({"checked_at":now,"source":"subscriber","not_returned":true})
                };
                self.db
                    .set_preference(&format!("subscriber-discovery:{market}:{id}"), &value)
                    .await?;
                result.insert(id.clone(), value);
            }
        }
        Ok(result)
    }

    pub async fn search_artists(
        &mut self,
        query: &str,
        market: &str,
    ) -> Result<Vec<TidalArtist>, String> {
        let payload = self
            .request(
                "search/artists",
                market,
                &[("query", query.into()), ("limit", "50".into())],
            )
            .await?;
        let items = payload["items"]
            .as_array()
            .ok_or("Invalid artist search response")?;
        Ok(items
            .iter()
            .filter_map(|v| {
                let id = resource_id(&v["id"]);
                let name = v["name"].as_str()?;
                (!id.is_empty()).then(|| TidalArtist {
                    id,
                    name: name.into(),
                })
            })
            .collect())
    }

    pub async fn search_tracks(
        &mut self,
        query: &str,
        market: &str,
    ) -> Result<Vec<TidalTrackSearchResult>, String> {
        let payload = self
            .request(
                "search/tracks",
                market,
                &[("query", query.into()), ("limit", "100".into())],
            )
            .await?;
        let items = payload["items"]
            .as_array()
            .ok_or("Invalid track search response")?;
        Ok(items
            .iter()
            .filter_map(|v| {
                let id = resource_id(&v["id"]);
                let album = resource_id(&v["album"]["id"]);
                (!id.is_empty() && !album.is_empty()).then(|| TidalTrackSearchResult {
                    id,
                    title: format_title(v["title"].as_str().unwrap_or(""), v["version"].as_str()),
                    isrc: v["isrc"].as_str().map(str::to_owned),
                    album_ids: vec![album],
                })
            })
            .collect())
    }

    pub async fn releases_for_isrc(
        &mut self,
        isrc: &str,
        market: &str,
        release_title: &str,
    ) -> Result<Vec<String>, String> {
        // ISRC is a filter on /tracks; text search treats it as words and can
        // return unrelated recordings. Always verify the returned identifier.
        let items = self
            .pages("tracks", market, &[("isrc", isrc.into())])
            .await?;
        let mut ids: Vec<_> = items
            .iter()
            .filter(|v| {
                v["isrc"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(isrc))
            })
            .filter(|v| {
                let candidate =
                    crate::matching::title_key(v["album"]["title"].as_str().unwrap_or(""));
                let local = crate::matching::title_key(release_title);
                // Nested album summaries can omit the edition suffix. Retain
                // these candidates; full release structure still decides links.
                candidate.is_empty()
                    || candidate == local
                    || (candidate.len() >= 3 && local.starts_with(&format!("{candidate} ")))
            })
            .map(|v| resource_id(&v["album"]["id"]))
            .filter(|s| !s.is_empty())
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    async fn artist_name(&mut self, artist_id: &str, market: &str) -> Result<String, String> {
        let key = format!("subscriber-artist:{market}:{artist_id}");
        let saved = self.db.get_preference(&key).await?;
        let now = Utc::now().timestamp();
        if let Some(value) = &saved {
            if value["checked_at"].as_i64().is_some_and(|at| (0..30 * 86400).contains(&(now-at))) {
                if let Some(name) = value["name"].as_str().filter(|name| !name.is_empty()) { return Ok(name.to_owned()); }
            }
        } else {
            // Bootstrap the name cache from recent catalogue snapshots after upgrade.
            // Once expired, fetch a fresh profile rather than extending stale names forever.
            let conn = self.db.connect()?;
            let mut rows = conn.query("SELECT json_extract(payload,'$.name'),fetched FROM catalogue WHERE artist_id=? AND market=?", (artist_id,market)).await.map_err(|e|e.to_string())?;
            let prior = rows.next().await.map_err(|e|e.to_string())?.and_then(|row| {
                let name = row.get::<String>(0).ok()?;
                let fetched = row.get::<String>(1).ok()?;
                let at = chrono::DateTime::parse_from_rfc3339(&fetched).ok()?.timestamp();
                (!name.is_empty() && (0..30*86400).contains(&(now-at))).then_some((name,at))
            });
            drop(rows);
            if let Some((name,at)) = prior {
                self.db.set_preference(&key,&json!({"name":name,"checked_at":at})).await?;
                return Ok(name);
            }
        }
        let artist = self.request(&format!("artists/{artist_id}"),market,&[]).await?;
        let name = artist["name"].as_str().filter(|name| !name.is_empty()).ok_or("Artist name missing")?.to_owned();
        self.db.set_preference(&key,&json!({"name":name,"checked_at":now})).await?;
        Ok(name)
    }

    pub async fn get_artist_catalogue(
        &mut self,
        artist_id: &str,
        market: &str,
        detailed: bool,
    ) -> Result<TidalCatalogue, String> {
        if artist_id.is_empty() || !artist_id.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Invalid artist ID".into());
        }
        let name = self.artist_name(artist_id,market).await?;
        // Main catalogue excludes guest compilation appearances by default.
        // Users can explicitly include them in Release matching & recommendations.
        let settings = self.db.get_preference("desktop").await?.or(self.db.get_preference("ui").await?).unwrap_or(Value::Null);
        let filters = if settings["recommend_compilations"] == true {
            vec!["", "EPSANDSINGLES", "COMPILATIONS"]
        } else {
            vec!["", "EPSANDSINGLES"]
        };
        let mut releases = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for filter in filters {
            let params = if filter.is_empty() {
                vec![]
            } else {
                vec![("filter", filter.to_owned())]
            };
            let items = self
                .pages(&format!("artists/{artist_id}/albums"), market, &params)
                .await?;
            for mut raw in items {
                let id = resource_id(&raw["id"]);
                if id.is_empty() || !seen.insert(id.clone()) {
                    continue;
                }
                raw["checked_at"] = json!(Utc::now().timestamp());
                self.cache_summary(&raw, market).await?;
                let mut release = release_from_subscriber(&raw, artist_id);
                if detailed {
                    release.tracks = self.get_release_details(&id, market).await?;
                    release.tracks_loaded = true;
                    release.track_count = release.tracks.len();
                }
                releases.push(release);
            }
        }
        let ids = releases.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        let extra = self.discovery(&ids, market, false).await?;
        for release in &mut releases {
            if let Some(fields) = extra.get(&release.id) {
                let mut value = serde_json::to_value(&*release).map_err(|e| e.to_string())?;
                merge_discovery(&mut value, fields);
                *release = serde_json::from_value(value).map_err(|e| e.to_string())?;
            }
        }
        Ok(TidalCatalogue {
            id: artist_id.into(),
            name,
            releases,
        })
    }

    pub async fn get_release_details(
        &mut self,
        id: &str,
        market: &str,
    ) -> Result<Vec<TidalTrack>, String> {
        let raw = crate::subscriber_metadata::load(
            &self.db,
            &self.http,
            id,
            market,
            self.cancel.clone(),
            false,
        )
        .await?;
        crate::subscriber_metadata::tracks(&raw)
    }

    pub async fn save_catalogue_to_db(
        &self,
        db: &TursoDb,
        market: &str,
        catalogue: &TidalCatalogue,
    ) -> Result<(), String> {
        let conn = db.connect()?;
        conn.execute("BEGIN IMMEDIATE", ()).await.map_err(|e|e.to_string())?;
        let result = async {
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
                                for field in [
                                    "upc",
                                    "original_release_date",
                                    "audio_modes",
                                    "media_metadata",
                                    "quality",
                                    "copyright",
                                    "label",
                                    "official",
                                    "release_group_id",
                                    "primary_type",
                                    "secondary_types",
                                    "artist_credits",
                                    "genres",
                                    "replacement_id",
                                ] {
                                    let absent = release[field].is_null()
                                        || release[field].as_str().is_some_and(str::is_empty)
                                        || release[field].as_array().is_some_and(Vec::is_empty);
                                    if absent && !cached[field].is_null() {
                                        release[field] = cached[field].clone();
                                    }
                                }
                                if release["discovery_checked_at"].is_null() {
                                    release["discovery_checked_at"] =
                                        cached["discovery_checked_at"].clone();
                                }
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

            TursoDb::index_catalogue(&conn,&catalogue.id,market,&payload).await?;
            conn.execute("COMMIT", ()).await.map_err(|e|e.to_string())?;
            Ok::<_,String>(())
        }.await;
        if result.is_err() { let _ = conn.execute("ROLLBACK", ()).await; }
        result?;
        db.bump_revision();
        Ok(())
    }
}

fn normalized_credit(resource: &Value) -> Value {
    let mut credit = resource
        .get("attributes")
        .cloned()
        .unwrap_or_else(|| resource.clone());
    if !credit.is_object() {
        return Value::Null;
    }
    if let Some(id) = resource.get("id") {
        credit["id"] = id.clone();
    }
    if let Some(relationships) = resource.get("relationships") {
        credit["relationships"] = relationships.clone();
        if let Some(id) = relationships["artist"]["data"]["id"].as_str() {
            credit["artist_id"] = json!(id);
        }
        if let Some(id) = relationships["category"]["data"]["id"].as_str() {
            credit["roleId"] = json!(id);
        }
    }
    credit
}

fn relationship_next(relationship: &Value) -> Option<String> {
    relationship["links"]["next"]
        .as_str()
        .or_else(|| relationship["links"]["next"]["href"].as_str())
        .filter(|link| !link.is_empty())
        .map(str::to_owned)
}

fn included_credits(item: &Value, included: &[Value]) -> (Value, bool) {
    if let Some(credits) = item["attributes"]["credits"].as_array() {
        return (
            json!(credits.iter().map(normalized_credit).collect::<Vec<_>>()),
            true,
        );
    }
    let relationship = &item["relationships"]["credits"];
    let Some(refs) = relationship["data"].as_array() else {
        return (Value::Null, false);
    };
    let credits: Vec<_> = refs
        .iter()
        .filter_map(|reference| {
            included
                .iter()
                .find(|resource| resource["type"] == "credits" && resource["id"] == reference["id"])
        })
        .map(normalized_credit)
        .collect();
    let complete = credits.len() == refs.len() && relationship_next(relationship).is_none();
    (json!(credits), complete)
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
        let (credits, credits_complete) = included_credits(item, included);
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
            media_tags: string_list(a.get("mediaTags")),
            audio_modes: string_list(a.get("audioModes")),
            media_metadata: a.get("mediaMetadata").cloned().unwrap_or(Value::Null),
            genres: related_genres(item, included),
            replacement_id: replacement_id(item),
            discovery_checked_at: item["relationships"]
                .get("genres")
                .map(|_| Utc::now().timestamp()),
            credits,
            credits_complete,
            credits_checked_at: credits_complete.then(|| Utc::now().timestamp()),
        });
    }
    Ok(tracks)
}

pub fn replacement_id(resource: &Value) -> Option<String> {
    let data = &resource["relationships"]["replacement"]["data"];
    let reference = data.as_array().and_then(|a| a.first()).unwrap_or(data);
    reference["id"]
        .as_str()
        .filter(|id| !id.is_empty() && Some(*id) != resource["id"].as_str())
        .map(str::to_owned)
}

pub fn related_genres(resource: &Value, included: &[Value]) -> Vec<String> {
    let mut values = Vec::new();
    for reference in resource["relationships"]["genres"]["data"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(name) = included
            .iter()
            .find(|v| v["type"] == "genres" && v["id"] == reference["id"])
            .and_then(|v| v["attributes"]["name"].as_str())
        {
            if !name.trim().is_empty()
                && !values
                    .iter()
                    .any(|v: &String| v.eq_ignore_ascii_case(name.trim()))
            {
                values.push(name.trim().to_string());
            }
        }
    }
    values
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
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
}

fn resource_id(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|v| v.to_string()))
        .unwrap_or_default()
}

fn album_artists(raw: &Value) -> Vec<Value> {
    let mut artists = Vec::new();
    // Preserve the explicit album-level primary artist first, never a track guest.
    for artist in
        std::iter::once(&raw["artist"]).chain(raw["artists"].as_array().into_iter().flatten())
    {
        let id = resource_id(&artist["id"]);
        if !id.is_empty() && !artists.iter().any(|a: &Value| resource_id(&a["id"]) == id) {
            artists.push(artist.clone());
        }
    }
    artists
}

fn subscriber_available(raw: &Value) -> Option<bool> {
    match (
        raw["allowStreaming"].as_bool(),
        raw["streamReady"].as_bool(),
    ) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

/// Existing cache consumers share a JSON resource envelope. Missing optional
/// subscriber fields remain unknown; absence must not erase richer saved data.
fn album_resources(items: &[Value]) -> Value {
    let mut data = Vec::new();
    let mut included = Vec::new();
    for raw in items {
        let id = resource_id(&raw["id"]);
        let artists = album_artists(raw);
        let refs: Vec<_> = artists
            .iter()
            .map(|a| json!({"type":"artists","id":resource_id(&a["id"])}))
            .collect();
        for a in artists {
            let aid = resource_id(&a["id"]);
            if !included
                .iter()
                .any(|v: &Value| v["id"] == aid && v["type"] == "artists")
            {
                included.push(json!({"type":"artists","id":aid,"attributes":{"name":a["name"]}}));
            }
        }
        let mut attrs = raw.clone();
        attrs["albumType"] = raw["type"].clone();
        attrs["numberOfItems"] = raw["numberOfTracks"].clone(); // Audio only, never video count.
        attrs["barcodeId"] = raw["upc"].clone();
        attrs["recordLabel"] = raw
            .get("label")
            .or_else(|| raw.get("recordLabel"))
            .cloned()
            .unwrap_or(Value::Null);
        attrs["mediaTags"] = raw["mediaMetadata"]["tags"].clone();
        if let Some(available) = subscriber_available(raw) {
            attrs["availability"] = if available {
                json!(["STREAM"])
            } else {
                json!([])
            };
        }
        let mut relationships = json!({"artists":{"data":refs}});
        if let Some(replacement) = raw
            .get("replacementId")
            .or_else(|| raw.get("replacement").and_then(|v| v.get("id")))
        {
            let rid = resource_id(replacement);
            if !rid.is_empty() && rid != id {
                relationships["replacement"] = json!({"data":{"type":"albums","id":rid}});
            }
        }
        if let Some(genres) = raw["genres"].as_array() {
            let mut refs = Vec::new();
            for (index, g) in genres.iter().enumerate() {
                if let Some(name) = g.as_str().or_else(|| g["name"].as_str()) {
                    let gid = format!("{id}:genre:{index}");
                    refs.push(json!({"type":"genres","id":gid}));
                    included.push(json!({"type":"genres","id":gid,"attributes":{"name":name}}));
                }
            }
            relationships["genres"] = json!({"data":refs});
        }
        data.push(
            json!({"type":"albums","id":id,"attributes":attrs,"relationships":relationships}),
        );
    }
    json!({"data":data,"included":included,"source":"subscriber"})
}

fn release_from_subscriber(raw: &Value, artist_id: &str) -> TidalRelease {
    let artists = album_artists(raw);
    let resources = album_resources(&[raw.clone()]);
    let data = &resources["data"][0];
    let included = resources["included"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    TidalRelease {
        id: resource_id(&raw["id"]),
        artist: artists
            .first()
            .and_then(|a| a["name"].as_str())
            .unwrap_or("")
            .into(),
        title: format_title(raw["title"].as_str().unwrap_or(""), raw["version"].as_str()),
        date: raw["releaseDate"].as_str().unwrap_or("").into(),
        r#type: raw["type"].as_str().unwrap_or("ALBUM").into(),
        available: subscriber_available(raw),
        track_count: raw["numberOfTracks"].as_u64().unwrap_or(0) as usize,
        explicit: raw["explicit"].as_bool().unwrap_or(false),
        copyright: raw["copyright"].as_str().map(str::to_owned),
        label: raw["label"]
            .as_str()
            .or_else(|| raw["label"]["name"].as_str())
            .map(str::to_owned),
        quality: raw["audioQuality"].as_str().unwrap_or("").into(),
        upc: raw["upc"].as_str().map(str::to_owned),
        artist_credits: artists
            .iter()
            .filter_map(|a| a["name"].as_str().map(str::to_owned))
            .collect(),
        primary_artist_verified: artists
            .first()
            .is_some_and(|a| resource_id(&a["id"]) == artist_id),
        original_release_date: raw["originalReleaseDate"].as_str().map(str::to_owned),
        audio_modes: string_list(raw.get("audioModes")),
        media_metadata: raw["mediaMetadata"].clone(),
        genres: related_genres(data, &included),
        replacement_id: replacement_id(data),
        official: raw["official"].as_bool(),
        secondary_types: string_list(raw.get("secondaryTypes")),
        release_group_id: raw["releaseGroupId"].as_str().map(str::to_owned),
        // No claim about genres/replacements/official status when not supplied.
        ..Default::default()
    }
}

#[cfg(test)]
mod subscriber_tests {
    use super::*;
    #[test]
    fn missing_optional_lists_accept_cached_nulls_but_not_objects() {
        let release: TidalRelease = serde_json::from_value(
            json!({"id":"1","secondary_types":null,"audio_modes":null,"tracks":null}),
        )
        .unwrap();
        assert!(release.secondary_types.is_empty());
        assert!(release.tracks.is_empty());
        assert!(
            serde_json::from_value::<TidalRelease>(json!({"secondary_types":{"wrong":true}}))
                .is_err()
        );
    }

    #[test]
    fn album_adapter_preserves_primary_credit_audio_counts_and_unknowns() {
        let raw = json!({"id":12,"artist":{"id":4,"name":"Main"},"artists":[{"id":5,"name":"Guest"},{"id":4,"name":"Main"}],"numberOfTracks":12,"numberOfVideos":1,"allowStreaming":true,"streamReady":true,"upc":"0012","type":"ALBUM","title":"Release","audioQuality":"LOSSLESS"});
        let rel = release_from_subscriber(&raw, "4");
        assert_eq!(rel.track_count, 12);
        assert!(rel.primary_artist_verified);
        assert_eq!(rel.artist, "Main");
        let resources = album_resources(&[raw]);
        assert_eq!(
            resources["data"][0]["relationships"]["artists"]["data"][0]["id"],
            "4"
        );
        assert_eq!(
            crate::availability::available(&resources["data"][0]),
            Some(true)
        );
        assert!(!resources["data"][0]["relationships"]["genres"].is_object());
        assert_eq!(subscriber_available(&json!({"allowStreaming":true})), None);
        assert_eq!(
            subscriber_available(&json!({"allowStreaming":true,"streamReady":false})),
            Some(false)
        );
        let mut saved = json!({"genres":["Electronic"],"replacement_id":"42"});
        merge_discovery(
            &mut saved,
            &json!({"checked_at":10,"genres":[],"replacement_id":null}),
        );
        assert_eq!(saved["genres"], json!(["Electronic"]));
        assert_eq!(saved["replacement_id"], "42");
        merge_discovery(
            &mut saved,
            &json!({"checked_at":11,"genres":["House"],"replacement_id":"43"}),
        );
        assert_eq!(saved["genres"], json!(["House"]));
        assert_eq!(saved["replacement_id"], "43");
    }

    #[tokio::test]
    async fn subscriber_summary_cache_works_without_credentials_and_retains_rich_old_metadata() {
        let dir = std::env::temp_dir().join(format!("subscriber-cache-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let mut client = TidalClient::from_db(&db).await.unwrap();
        db.set_preference("subscriber-artist:GB:4", &json!({"name":"Main","checked_at":Utc::now().timestamp()})).await.unwrap();
        assert_eq!(client.artist_name("4","GB").await.unwrap(),"Main");
        let raw = json!({"id":12,"title":"Release","artist":{"id":4,"name":"Main"},"checked_at":Utc::now().timestamp(),"allowStreaming":true,"streamReady":true});
        client.cache_summary(&raw, "GB").await.unwrap();
        let payload = client.albums(&["12".into()], "GB", false).await.unwrap();
        assert_eq!(payload["data"][0]["id"], "12");
        assert!(db
            .get_preference("release-artists:US:12")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            db.get_preference("release-artists:GB:12")
                .await
                .unwrap()
                .unwrap()["ids"],
            json!(["4"])
        );
        let mut old = release_from_subscriber(&raw, "4");
        old.genres = vec!["Electronic".into()];
        old.replacement_id = Some("13".into());
        client
            .save_catalogue_to_db(
                &db,
                "GB",
                &TidalCatalogue {
                    id: "4".into(),
                    name: "Main".into(),
                    releases: vec![old],
                },
            )
            .await
            .unwrap();
        client
            .save_catalogue_to_db(
                &db,
                "GB",
                &TidalCatalogue {
                    id: "4".into(),
                    name: "Main".into(),
                    releases: vec![release_from_subscriber(&raw, "4")],
                },
            )
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        let mut rows = conn
            .query("SELECT payload FROM catalogue", ())
            .await
            .unwrap();
        let text: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
        let cached: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(cached["releases"][0]["genres"], json!(["Electronic"]));
        assert_eq!(cached["releases"][0]["replacement_id"], "13");
    }

    #[tokio::test]
    #[ignore = "Read-only subscriber workflow verification; disposable DB, saved account"]
    async fn live_subscriber_workflows() {
        let dir =
            std::env::temp_dir().join(format!("subscriber-workflows-{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("db")).await.unwrap();
        let mut client = TidalClient::from_db(&db).await.unwrap();
        let artists = client.search_artists("Canopy", "GB").await.unwrap();
        assert!(artists.iter().any(|a| a.id == "3870503"));
        println!("Artist search passed");
        let results = client.search_tracks("Cassie Me & U", "GB").await.unwrap();
        assert!(!results.is_empty());
        println!("Track search passed");
        let catalogue = client
            .get_artist_catalogue("3870503", "GB", false)
            .await
            .unwrap();
        assert!(catalogue.releases.iter().any(|r| r.id == "234657671"));
        client
            .save_catalogue_to_db(&db, "GB", &catalogue)
            .await
            .unwrap();
        println!("Discovery passed: {} releases", catalogue.releases.len());
        let summaries = client
            .albums(
                &["140303440".into(), "285803".into(), "999999999".into()],
                "GB",
                true,
            )
            .await
            .unwrap();
        assert_eq!(summaries["data"].as_array().unwrap().len(), 2);
        println!("Batch summaries passed");
        let optional = client
            .discovery(&["140303440".into()], "GB", false)
            .await
            .unwrap();
        assert!(optional.contains_key("140303440"));
        assert_eq!(optional["140303440"]["source"], "subscriber");
        println!("Optional v2 catalogue metadata with subscriber account passed");
        let value = crate::actions::release(&db, "140303440", "GB", false)
            .await
            .unwrap();
        assert_eq!(value["track_count"], 12);
        assert_eq!(value["artist"], "Cassie");
        let tracks: Vec<TidalTrack> = serde_json::from_value(value["tracks"].clone()).unwrap();
        assert!(tracks.iter().any(|t| t.bpm.is_some()));
        assert!(tracks
            .iter()
            .any(|t| !t.credits.as_array().unwrap().is_empty()));
        let repeat = crate::actions::release(&db, "140303440", "GB", false)
            .await
            .unwrap();
        assert_eq!(repeat["tracks"], value["tracks"]);
        println!("Audio-only tracks, credits, BPM/key and shared cache passed");
        let ids = client
            .releases_for_isrc(tracks[0].isrc.as_deref().unwrap(), "GB", "Cassie")
            .await
            .unwrap();
        assert!(ids.contains(&"140303440".into()));
        println!(
            "Exact ISRC lookup passed: {} matching album editions",
            ids.len()
        );
        let live = crate::availability::check(&db, &["140303440".into(), "285803".into()], "GB")
            .await
            .unwrap();
        assert_eq!(live["140303440"], Some(true));
        println!("Market availability passed");
        let state = std::sync::Arc::new(crate::Backend::new());
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let favourites = crate::actions::execute(&db, &state, "favourites", &json!({}), cancel)
            .await
            .unwrap();
        println!("Favourites passed: {}", favourites["artists"]);
        println!("Temporary test data: {}", dir.display());
    }
}

/// Preserve known metadata when optional fields are absent or empty upstream.
pub fn merge_discovery(value: &mut Value, fields: &Value) {
    for field in [
        "genres",
        "replacement_id",
        "label",
        "official",
        "original_release_date",
    ] {
        let incoming = &fields[field];
        if !incoming.is_null()
            && !incoming.as_array().is_some_and(Vec::is_empty)
            && !incoming.as_str().is_some_and(str::is_empty)
        {
            value[field] = incoming.clone();
        }
    }
    value["discovery_checked_at"] = fields["checked_at"].clone();
    value["subscriber_discovery_checked_at"] = fields["checked_at"].clone();
}
