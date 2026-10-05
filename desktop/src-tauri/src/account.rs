use crate::db::TursoDb;
use crate::tidal::TidalArtist;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
static SESSION_CACHE: OnceLock<Mutex<Option<(Instant, Option<AccountSession>)>>> = OnceLock::new();

/// Account country is catalogue state, not a user-selectable matching preference.
/// Keep it separately from credentials so cached browsing still uses the same
/// market when the account is disconnected or the network is unavailable.
const ACCOUNT_MARKET_KEY: &str = "subscriber-account-market";

pub fn normalize_market(country: &str) -> Option<String> {
    let country = country.trim();
    if country.len() != 2 || !country.bytes().all(|letter| letter.is_ascii_alphabetic()) {
        return None;
    }
    Some(match country.to_ascii_uppercase().as_str() {
        "UK" => "GB".to_string(),
        code => code.to_string(),
    })
}

fn account_country(details: &Value) -> Option<String> {
    ["countryCode", "country_code", "country"]
        .into_iter()
        .find_map(|key| details[key].as_str().and_then(normalize_market))
        .or_else(|| {
            ["user", "session", "account"]
                .into_iter()
                .find_map(|key| details.get(key).and_then(account_country))
        })
}

fn account_user(details: &Value) -> Option<String> {
    ["userId", "user_id"]
        .into_iter()
        .find_map(|key| {
            details[key]
                .as_str()
                .map(str::to_string)
                .or_else(|| details[key].as_u64().map(|id| id.to_string()))
        })
        .or_else(|| {
            ["user", "session", "account"]
                .into_iter()
                .find_map(|key| details.get(key).and_then(account_user))
        })
}

fn cached_market(saved: &Value, fallback: Option<&str>) -> String {
    saved["country_code"]
        .as_str()
        .and_then(normalize_market)
        .or_else(|| fallback.and_then(normalize_market))
        // Existing GB databases remain usable before the first account check.
        // Once confirmed, account country always takes precedence over this.
        .unwrap_or_else(|| "GB".into())
}

pub async fn catalogue_market(db: &TursoDb, fallback: Option<&str>) -> Result<String, String> {
    let saved = db
        .get_preference(ACCOUNT_MARKET_KEY)
        .await?
        .unwrap_or(Value::Null);
    Ok(cached_market(&saved, fallback))
}

/// Reuse the existing sign-in/connection-check session payload. No tokens or
/// personal profile fields are copied into catalogue preferences.
pub async fn save_account_market(
    db: &TursoDb,
    details: &Value,
    user_id: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(country) = account_country(details) else {
        return Ok(None);
    };
    let user = account_user(details).or_else(|| user_id.map(str::to_string));
    let previous = db.get_preference(ACCOUNT_MARKET_KEY).await?;
    db.set_preference(
        ACCOUNT_MARKET_KEY,
        &json!({
            "country_code": country,
            "user_id": user,
            "checked_at": Utc::now().to_rfc3339(),
        }),
    )
    .await?;
    if previous
        .as_ref()
        .is_none_or(|saved| saved["country_code"] != country || saved["user_id"] != json!(user))
    {
        db.bump_revision();
    }
    Ok(Some(country))
}

/// Resolve an older saved session once, then reuse its cached country for every
/// workflow. A failed check never discards the market used by saved links.
pub async fn ensure_account_market(
    db: &TursoDb,
    http: &reqwest::Client,
    cancel: Option<&AtomicBool>,
) -> Result<String, String> {
    check_account_market(db, http, cancel, false).await
}

/// Sign-in and an explicit connection check can refresh account country even
/// when the same account already has a cached country.
pub async fn refresh_account_market(
    db: &TursoDb,
    http: &reqwest::Client,
    cancel: Option<&AtomicBool>,
) -> Result<String, String> {
    check_account_market(db, http, cancel, true).await
}

async fn check_account_market(
    db: &TursoDb,
    http: &reqwest::Client,
    cancel: Option<&AtomicBool>,
    force: bool,
) -> Result<String, String> {
    static MARKET_CHECK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static ATTEMPTS: OnceLock<
        Mutex<std::collections::HashMap<(std::path::PathBuf, Option<String>), Instant>>,
    > = OnceLock::new();
    let _check = MARKET_CHECK.lock().await;
    let preferences = db
        .get_preference("desktop")
        .await?
        .or(db.get_preference("ui").await?)
        .unwrap_or(Value::Null);
    let saved = db
        .get_preference(ACCOUNT_MARKET_KEY)
        .await?
        .unwrap_or(Value::Null);
    let fallback = cached_market(&saved, preferences["market"].as_str());
    let Some(session) = crate::stream_download::load_saved_token(db).await else {
        return Ok(fallback);
    };
    let cached_user = saved["user_id"].as_str();
    if !force
        && account_country(&saved).is_some()
        && (session.user_id.is_none() || cached_user == session.user_id.as_deref())
    {
        return Ok(fallback);
    }
    // Automated local tests must not query the real subscriber service.
    if std::env::var_os("TIBRARY_TEST_MODE").is_some() {
        return Ok(fallback);
    }
    // A temporarily offline service must not add another session request for
    // every catalogue action. Explicit sign-in/check actions bypass this gate.
    {
        let mut attempts = ATTEMPTS.get_or_init(Default::default).lock().unwrap();
        let key = (db.path.clone(), session.user_id.clone());
        if !force
            && attempts
                .get(&key)
                .is_some_and(|at| at.elapsed() < Duration::from_secs(300))
        {
            return Ok(fallback);
        }
        attempts.insert(key, Instant::now());
    }
    let token = match crate::stream_download::get_valid_token(db, http).await {
        Ok(token) => token,
        Err(_) => return Ok(fallback),
    };
    let response = match crate::network::get(
        http.get("https://api.tidal.com/v1/sessions")
            .bearer_auth(token),
        Duration::from_millis(350),
        2,
        cancel,
    )
    .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(_) => return Ok(fallback),
        Err(error) if error == "Cancelled" => return Err(error),
        Err(_) => return Ok(fallback),
    };
    let details = match response.json::<Value>().await {
        Ok(details) => details,
        Err(_) => return Ok(fallback),
    };
    Ok(
        save_account_market(db, &details, session.user_id.as_deref())
            .await?
            .unwrap_or(fallback),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub user_id: Option<String>,
    pub expires_at: f64,
}

pub struct AccountClient;

impl Default for AccountClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountClient {
    pub fn new() -> Self {
        Self
    }

    pub fn load_saved_session() -> Option<AccountSession> {
        if std::env::var_os("TIBRARY_TEST_MODE").is_some() {
            return None;
        }
        let cache = SESSION_CACHE.get_or_init(|| Mutex::new(None));
        let mut entry = cache.lock().unwrap();
        if let Some((at, session)) = &*entry {
            if at.elapsed() < Duration::from_secs(30) {
                return session.clone();
            }
        }
        let session = Self::load_uncached_session();
        *entry = Some((Instant::now(), session.clone()));
        session
    }
    fn load_uncached_session() -> Option<AccountSession> {
        if std::env::var_os("TIBRARY_TEST_MODE").is_some() {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            // Read the subscriber account only, never another credential in this service.
            let output = std::process::Command::new("security")
                .args([
                    "find-generic-password",
                    "-s",
                    "Tibrary",
                    "-a",
                    "session",
                    "-w",
                ])
                .output()
                .ok()?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let trimmed = line.trim();
                    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                        if let Some(token) = val.get("access_token").and_then(|v| v.as_str()) {
                            let refresh = val
                                .get("refresh_token")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let uid = val
                                .get("user_id")
                                .or_else(|| val.get("userId"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let exp = val
                                .get("expires_at")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);

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
        if std::env::var_os("TIBRARY_TEST_MODE").is_none() {
            let value = serde_json::to_string(session)?;
            let result = std::process::Command::new("security")
                .args([
                    "add-generic-password",
                    "-s",
                    "Tibrary",
                    "-a",
                    "session",
                    "-w",
                    &value,
                    "-U",
                ])
                .output()?;
            if !result.status.success() {
                return Err(std::io::Error::other(
                    "Unable to save account securely in Keychain",
                ));
            }
        }
        *SESSION_CACHE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some((Instant::now(), Some(session.clone())));
        Ok(())
    }

    pub fn disconnect() -> std::io::Result<()> {
        *SESSION_CACHE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = None;
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("security")
                .args(["delete-generic-password", "-s", "Tibrary", "-a", "session"])
                .output();
        }
        Ok(())
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

    #[test]
    fn account_country_is_normalized_and_overrides_legacy_market() {
        assert_eq!(
            account_country(&json!({"user":{"countryCode":" gb "}})),
            Some("GB".into())
        );
        assert_eq!(
            account_country(&json!({"session":{"country_code":"uk"}})),
            Some("GB".into())
        );
        assert_eq!(
            account_country(&json!({"countryCode":"United Kingdom"})),
            None
        );
        assert_eq!(
            account_user(&json!({"user":{"userId":42}})),
            Some("42".into())
        );
        assert_eq!(
            cached_market(&json!({"country_code":"NZ"}), Some("GB")),
            "NZ"
        );
        assert_eq!(cached_market(&Value::Null, Some("US")), "US");
        assert_eq!(cached_market(&Value::Null, Some("invalid")), "GB");
    }

    #[tokio::test]
    async fn account_market_cache_survives_offline_and_missing_profile_fields() {
        let dir = std::env::temp_dir().join(format!("account_market_{}", uuid::Uuid::new_v4()));
        let db = TursoDb::open(dir.join("market.sqlite3")).await.unwrap();
        db.set_preference("desktop", &json!({"market":"US"}))
            .await
            .unwrap();
        assert_eq!(catalogue_market(&db, Some("US")).await.unwrap(), "US");
        let revision = db.revision.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            save_account_market(&db, &json!({"countryCode":"gb","userId":42}), None)
                .await
                .unwrap(),
            Some("GB".into())
        );
        assert!(db.revision.load(std::sync::atomic::Ordering::SeqCst) > revision);
        db.set_preference("account-disconnected", &json!(true))
            .await
            .unwrap();
        assert_eq!(catalogue_market(&db, Some("US")).await.unwrap(), "GB");
        let settings = db.get_settings().await.unwrap();
        assert_eq!(settings["general"]["market"], "GB");
        assert_eq!(settings["general"]["highlight_colour"], "system");
        assert_eq!(
            save_account_market(&db, &json!({"userId":42}), None)
                .await
                .unwrap(),
            None
        );
        assert_eq!(catalogue_market(&db, Some("US")).await.unwrap(), "GB");
        assert_eq!(
            save_account_market(&db, &json!({"user":{"countryCode":"NZ"}}), Some("84"))
                .await
                .unwrap(),
            Some("NZ".into())
        );
        let cached = db
            .get_preference(ACCOUNT_MARKET_KEY)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached["user_id"], "84");
        assert!(cached.get("access_token").is_none());
        assert_eq!(catalogue_market(&db, Some("US")).await.unwrap(), "NZ");
        db.set_preference("desktop", &json!({"market":"US","highlight_colour":"grey"}))
            .await.unwrap();
        let settings = db.get_settings().await.unwrap();
        assert_eq!(settings["general"]["market"], "NZ");
        assert_eq!(settings["general"]["highlight_colour"], "graphite");
        let _ = std::fs::remove_dir_all(dir);
    }

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
        let store = TursoDb::open(&db_path)
            .await
            .expect("Failed to open TursoDb");
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
            .query(
                "SELECT payload FROM favourite_artists WHERE cache_id = 'user_1'",
                (),
            )
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
