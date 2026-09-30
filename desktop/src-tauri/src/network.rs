//! Shared adaptive scheduling for idempotent GETs. OAuth POSTs are never replayed.
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};

pub const METADATA_CONCURRENCY: usize = 3;

#[cfg(test)]
static THROTTLES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct Pace {
    interval: Duration,
    next: Instant,
    successes: u8,
    floor: Duration,
}
impl Pace {
    fn new(initial: Duration, floor: Duration) -> Self {
        Self {
            interval: initial.max(floor),
            next: Instant::now(),
            successes: 0,
            floor,
        }
    }
    fn success(&mut self) {
        self.successes += 1;
        if self.successes >= 8 {
            self.interval = self.interval.mul_f64(0.85).max(self.floor);
            self.successes = 0;
        }
    }
    fn slow(&mut self, cooldown: Duration) {
        self.successes = 0;
        self.interval = (self.interval * 2)
            .max(Duration::from_millis(250))
            .min(Duration::from_secs(10));
        self.next = self
            .next
            .max(Instant::now() + cooldown.min(Duration::from_secs(86400)));
    }
}
struct Lane {
    pace: Mutex<Pace>,
    slots: Semaphore,
}
fn lane(key: String, initial: Duration, api: bool) -> Arc<Lane> {
    static LANES: OnceLock<std::sync::Mutex<HashMap<String, Arc<Lane>>>> = OnceLock::new();
    LANES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(key)
        .or_insert_with(|| {
            Arc::new(Lane {
                pace: Mutex::new(Pace::new(
                    initial,
                    if api {
                        Duration::from_millis(100)
                    } else {
                        Duration::ZERO
                    },
                )),
                slots: Semaphore::new(if api { METADATA_CONCURRENCY } else { 8 }),
            })
        })
        .clone()
}
fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}
pub fn retry_after(value: Option<&str>, attempt: usize) -> Duration {
    let seconds = value
        .and_then(|v| {
            v.trim().parse::<u64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(v)
                    .ok()
                    .map(|at| (at.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64)
            })
        })
        .unwrap_or(2u64.saturating_pow(attempt.min(5) as u32 + 1));
    Duration::from_secs(seconds.min(86400))
}

pub async fn get(
    request: reqwest::RequestBuilder,
    initial: Duration,
    attempts: usize,
    cancel: Option<&AtomicBool>,
) -> Result<reqwest::Response, String> {
    let built = request
        .try_clone()
        .ok_or("Request cannot be retried")?
        .build()
        .map_err(|_| "Invalid network request")?;
    if built.method() != reqwest::Method::GET {
        return Err("Adaptive requests support GET only".into());
    }
    let host = built.url().host_str().unwrap_or("");
    let api = host == "api.tidal.com" || host == "openapi.tidal.com";
    let scheduler = lane(
        format!(
            "{}:{}",
            host,
            built.url().port_or_known_default().unwrap_or(443)
        ),
        if api { initial } else { Duration::ZERO },
        api,
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    let _slot = loop {
        if cancelled(cancel) {
            return Err("Cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("Network request waiting limit reached".into());
        }
        if let Ok(slot) = scheduler.slots.try_acquire() {
            break slot;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    for attempt in 0..attempts.clamp(1, 5) {
        loop {
            if cancelled(cancel) {
                return Err("Cancelled".into());
            }
            if Instant::now() >= deadline {
                return Err("Network retry waiting limit reached".into());
            }
            let mut pace = scheduler.pace.lock().await;
            let wait = pace.next.saturating_duration_since(Instant::now());
            if wait > Duration::from_secs(60) {
                return Err(format!(
                    "Service cooldown · retry after {} seconds",
                    wait.as_secs()
                ));
            }
            if wait.is_zero() {
                pace.next = Instant::now() + pace.interval;
                break;
            }
            drop(pace);
            tokio::time::sleep(wait.min(Duration::from_millis(200))).await;
        }
        let pending = request
            .try_clone()
            .ok_or("Request cannot be retried")?
            .send();
        tokio::pin!(pending);
        let response = loop {
            tokio::select! {
                response=&mut pending => break response,
                _=tokio::time::sleep(Duration::from_millis(100)) => {
                    if cancelled(cancel) {return Err("Cancelled".into());}
                    if Instant::now()>=deadline {return Err("Network request deadline reached".into());}
                }
            }
        };
        match response {
            Ok(response) => {
                let status = response.status();
                #[cfg(test)]
                if status.as_u16() == 429 {
                    THROTTLES.fetch_add(1, Ordering::Relaxed);
                }
                if status.is_success() {
                    scheduler.pace.lock().await.success();
                    return Ok(response);
                }
                if status.as_u16() == 429 || status.is_server_error() {
                    let delay = retry_after(
                        response
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok()),
                        attempt,
                    );
                    scheduler.pace.lock().await.slow(delay);
                    if attempt + 1 < attempts.clamp(1, 5) && delay <= Duration::from_secs(60) {
                        continue;
                    }
                }
                return Ok(response);
            }
            Err(error) => {
                scheduler.pace.lock().await.slow(Duration::from_secs(2));
                if !(error.is_timeout() || error.is_connect())
                    || attempt + 1 >= attempts.clamp(1, 5)
                {
                    return Err("Network request failed or timed out; cached data retained".into());
                }
            }
        }
    }
    Err("Network retry limit reached".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pacing_accelerates_gradually_and_backs_off() {
        let mut p = Pace::new(Duration::from_millis(750), Duration::from_millis(100));
        for _ in 0..8 {
            p.success();
        }
        assert!(p.interval < Duration::from_millis(750));
        p.slow(Duration::from_secs(2));
        assert!(p.interval > Duration::from_millis(750));
        assert!(p.next > Instant::now());
        for _ in 0..240 {
            p.success();
        }
        assert_eq!(p.interval, Duration::from_millis(100));
        assert_eq!(retry_after(Some("0"), 0), Duration::ZERO);
    }
    #[tokio::test]
    async fn throttled_get_retries_but_cancelled_get_does_not_send() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for status in ["429 Too Many Requests", "200 OK"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0; 4096];
                stream.read(&mut buffer).unwrap();
                write!(stream,"HTTP/1.1 {status}\r\nRetry-After: 0\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
            }
        });
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        assert!(get(
            http.get(format!("http://{address}")),
            Duration::ZERO,
            2,
            None
        )
        .await
        .unwrap()
        .status()
        .is_success());
        server.join().unwrap();
        let cancel = AtomicBool::new(true);
        assert_eq!(
            get(
                http.get(format!("http://{address}")),
                Duration::ZERO,
                2,
                Some(&cancel)
            )
            .await
            .unwrap_err(),
            "Cancelled"
        );
    }
}


#[cfg(test)]
#[tokio::test]
#[ignore = "Explicit bounded live concurrency benchmark, six album GETs plus warmup"]
async fn live_metadata_concurrency() {
    let dir = std::env::temp_dir().join(format!("tibrary-concurrency-{}", uuid::Uuid::new_v4()));
    let db = crate::db::TursoDb::open(dir.join("db")).await.unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let token = crate::stream_download::get_valid_token(&db, &http)
        .await
        .unwrap();
    let ids = [
        "234657671",
        "140303440",
        "447957706",
    ];
    async fn fetch(
        http: reqwest::Client,
        token: String,
        id: &'static str,
    ) -> Result<usize, String> {
        let response = get(
            http.get(format!("https://api.tidal.com/v1/albums/{id}/items/credits"))
                .query(&[("countryCode", "GB"), ("limit", "100"), ("offset", "0")])
                .bearer_auth(token),
            Duration::from_millis(350),
            2,
            None,
        )
        .await?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        let value: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
        Ok(value["items"].as_array().ok_or("Missing tracks")?.len())
    }
    fetch(http.clone(), token.clone(), ids[0]).await.unwrap();
    for (interval_ms,concurrency) in [(350,1),(350,3)] {
        let scheduler = lane("api.tidal.com:443".into(), Duration::from_millis(350), true);
        // Equal starting pace for each trial; never bypass a service cooldown.
        {
            let mut pace = scheduler.pace.lock().await;
            pace.interval = Duration::from_millis(interval_ms);
            pace.successes = 0;
        }
        let before = THROTTLES.load(Ordering::Relaxed);
        let start = Instant::now();
        let mut tasks = tokio::task::JoinSet::new();
        let mut ids = ids.into_iter();
        let mut tracks = 0;
        for _ in 0..concurrency {
            if let Some(id) = ids.next() {
                tasks.spawn(fetch(http.clone(), token.clone(), id));
            }
        }
        while let Some(result) = tasks.join_next().await {
            tracks += result.unwrap().unwrap();
            if let Some(id) = ids.next() {
                tasks.spawn(fetch(http.clone(), token.clone(), id));
            }
        }
        println!(
            "spacing_ms={interval_ms} concurrency={concurrency} releases=3 tracks={tracks} elapsed_ms={} http_429={}",
            start.elapsed().as_millis(),
            THROTTLES.load(Ordering::Relaxed) - before
        );
        if THROTTLES.load(Ordering::Relaxed) > before {
            break;
        }
    }
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

// Reuse connection pools across short-lived workflow clients. Authentication is
// attached per request, never stored in default headers on the shared client.
pub fn client(timeout: u64) -> Result<reqwest::Client, String> {
    type Clients = std::collections::HashMap<u64, reqwest::Client>;
    static CLIENTS: std::sync::OnceLock<std::sync::Mutex<Clients>> = std::sync::OnceLock::new();
    let mut clients = CLIENTS.get_or_init(Default::default).lock().unwrap();
    if let Some(client) = clients.get(&timeout) {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout))
        .build()
        .map_err(|e| e.to_string())?;
    clients.insert(timeout, client.clone());
    Ok(client)
}
