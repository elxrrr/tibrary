use crate::db::TursoDb;
use crate::tidal::TidalArtist;
use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub user_id: Option<String>,
    pub expires_at: f64,
}

pub struct AccountClient {
    http: reqwest::Client,
}

impl Default for AccountClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self { http }
    }

    pub fn load_saved_session() -> Option<AccountSession> {
        #[cfg(target_os = "macos")]
        {
            // Query generic password for service Tibrary, account starting with collection-
            let output = std::process::Command::new("security")
                .args(["find-generic-password", "-s", "Tibrary", "-w"])
                .output()
                .ok()?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let trimmed = line.trim();
                    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                        if let Some(token) = val.get("access_token").and_then(|v| v.as_str()) {
                            let refresh = val.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());
                            let uid = val.get("user_id").or_else(|| val.get("userId")).and_then(|v| v.as_str()).map(|s| s.to_string());
                            let exp = val.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0);

                            return Some(AccountSession {
                                access_token: token.to_string(),
                                refresh_token: refresh,
                                user_id: uid,
                                expires_at: exp,
                            });
                        }
                    }
                }
            }
        }
        None
    }

    pub fn save_session(session: &AccountSession) -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            if let Ok(val) = serde_json::to_string(session) {
                let _ = std::process::Command::new("security")
                    .args(["add-generic-password", "-s", "Tibrary", "-a", "session", "-w", &val, "-U"])
                    .output();
            }
        }
        Ok(())
    }

    pub fn disconnect() -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("security")
                .args(["delete-generic-password", "-s", "Tibrary", "-a", "session"])
                .output();
        }
        Ok(())
    }

    pub async fn fetch_favourite_artists(
        &self,
        token: &str,
        user_id: &str,
    ) -> Result<Vec<TidalArtist>, String> {
        let url = format!(
            "https://openapi.tidal.com/v2/users/{}/relationships/favorites/artists",
            user_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token))
                .map_err(|e| format!("Invalid auth header: {}", e))?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.api+json"));

        let res = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| format!("Favorites request failed: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(format!("TIDAL API HTTP {}: {}", status, body));
        }

        let payload: Value = res
            .json()
            .await
            .map_err(|e| format!("Failed to parse favorites JSON: {}", e))?;

        let mut artists = Vec::new();
        if let Some(data) = payload.get("data").and_then(|v| v.as_array()) {
            for item in data {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = item
                    .get("attributes")
                    .and_then(|a| a.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if !id.is_empty() {
                    artists.push(TidalArtist { id, name });
                }
            }
        }

        Ok(artists)
    }

    pub async fn save_favourites_to_db(
        &self,
        db: &TursoDb,
        user_id: &str,
        artists: &[TidalArtist],
    ) -> Result<(), String> {
        let conn = db.connect()?;
        let json_payload = serde_json::to_string(artists).map_err(|e| e.to_string())?;
        let now_str = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT OR REPLACE INTO favourite_artists (cache_id, payload, fetched) VALUES (?, ?, ?)",
            (user_id, json_payload.as_str(), now_str.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn test_save_and_retrieve_favourites() {
        let temp_dir = std::env::temp_dir().join(format!(
            "fav_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("fav.sqlite3");
        let store = TursoDb::open(&db_path).await.expect("Failed to open TursoDb");
        let client = AccountClient::new();

        let artists = vec![
            TidalArtist {
                id: "123".to_string(),
                name: "Queen".to_string(),
            },
            TidalArtist {
                id: "456".to_string(),
                name: "David Bowie".to_string(),
            },
        ];

        client
            .save_favourites_to_db(&store, "user_1", &artists)
            .await
            .expect("Failed to save favourites");

        let conn = store.connect().unwrap();
        let mut stmt = conn
            .query("SELECT payload FROM favourite_artists WHERE cache_id = 'user_1'", ())
            .await
            .unwrap();

        let row = stmt.next().await.unwrap().unwrap();
        let payload_str: String = row.get(0).unwrap();
        let loaded: Vec<TidalArtist> = serde_json::from_str(&payload_str).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "Queen");
        assert_eq!(loaded[1].name, "David Bowie");

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
