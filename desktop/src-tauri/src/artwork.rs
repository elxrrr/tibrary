//! Small display-only covers. Metadata, audio files and catalogue revisions are
//! deliberately untouched; the UI requests only rows that are on screen.
use crate::db::TursoDb;
use base64::Engine;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

// TIDAL currently supports 160px; 128px returns HTTP 403. Never probe it per row.
const THUMBNAIL_SIZE: u16 = 160;
const FAILURE_COOLDOWN: i64 = 300;
const MEMORY_LIMIT: usize = 96;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct ThumbnailService {
    http: Option<reqwest::Client>,
    slots: Semaphore,
    gates: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
    images: Mutex<HashMap<String, (Instant, Value)>>,
    unavailable: Mutex<HashMap<String, Instant>>,
    network_paused: Mutex<Option<Instant>>,
    #[cfg(test)]
    test_origin: Option<String>,
}

impl Default for ThumbnailService {
    fn default() -> Self {
        Self {
            // One connection pool shared by every visible-row request. Refuse
            // redirects so a cover can never send requests outside the CDN.
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .pool_max_idle_per_host(4)
                .build()
                .ok(),
            slots: Semaphore::new(4),
            gates: Mutex::new(HashMap::new()),
            images: Mutex::new(HashMap::new()),
            unavailable: Mutex::new(HashMap::new()),
            network_paused: Mutex::new(None),
            #[cfg(test)]
            test_origin: None,
        }
    }
}

impl ThumbnailService {
    pub(crate) async fn thumbnail(
        &self,
        db: &TursoDb,
        release: &str,
        market: Option<&str>,
    ) -> Value {
        if release.is_empty() || release.len() > 20 || !release.bytes().all(|c| c.is_ascii_digit())
        {
            return Value::Null;
        }
        let market = match market.and_then(crate::account::normalize_market) {
            Some(market) => market,
            None => {
                let preferences = db
                    .get_preference("desktop")
                    .await
                    .ok()
                    .flatten()
                    .or(db.get_preference("ui").await.ok().flatten())
                    .unwrap_or(Value::Null);
                preferences["market"]
                    .as_str()
                    .and_then(crate::account::normalize_market)
                    .unwrap_or_else(|| "GB".into())
            }
        };
        let market = crate::account::catalogue_market(db, Some(&market))
            .await
            .unwrap_or(market);
        let missing_key = format!(
            "{}:{market}:{release}:{}",
            db.path.display(),
            db.revision.load(std::sync::atomic::Ordering::Relaxed)
        );
        if self.is_unavailable(&missing_key) {
            return Value::Null;
        }
        let Some(cover) = cached_cover(db, release, &market).await else {
            self.remember_unavailable(missing_key, Duration::from_secs(60));
            return Value::Null;
        };
        self.cover_thumbnail(&db.path, &cover).await
    }

    /// The same fixed CDN and cache serve artist portraits and playlist covers.
    /// Only a validated image UUID is accepted, never a remote URL or API path.
    pub(crate) async fn catalogue_image(&self, db: &TursoDb, image_id: &str) -> Value {
        self.cover_thumbnail(&db.path, image_id).await
    }

    fn gate(&self, key: &str) -> Arc<AsyncMutex<()>> {
        let mut gates = self.gates.lock().unwrap();
        if let Some(gate) = gates.get(key).and_then(Weak::upgrade) {
            return gate;
        }
        if gates.len() >= 64 {
            gates.retain(|_, gate| gate.strong_count() > 0);
        }
        let gate = Arc::new(AsyncMutex::new(()));
        gates.insert(key.into(), Arc::downgrade(&gate));
        gate
    }

    fn image(&self, key: &str) -> Option<Value> {
        let mut images = self.images.lock().unwrap();
        let (used, image) = images.get_mut(key)?;
        *used = Instant::now();
        Some(image.clone())
    }

    fn remember_image(&self, key: String, image: Value) {
        let mut images = self.images.lock().unwrap();
        if images.len() >= MEMORY_LIMIT {
            let oldest = images
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                images.remove(&oldest);
            }
        }
        images.insert(key, (Instant::now(), image));
    }

    fn is_unavailable(&self, key: &str) -> bool {
        let mut unavailable = self.unavailable.lock().unwrap();
        if unavailable
            .get(key)
            .is_some_and(|until| *until > Instant::now())
        {
            return true;
        }
        unavailable.remove(key);
        false
    }

    fn remember_unavailable(&self, key: String, duration: Duration) {
        let mut unavailable = self.unavailable.lock().unwrap();
        if unavailable.len() >= 256 {
            unavailable.retain(|_, until| *until > Instant::now());
            if unavailable.len() >= 256 {
                let oldest = unavailable
                    .iter()
                    .min_by_key(|(_, until)| **until)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    unavailable.remove(&oldest);
                }
            }
        }
        unavailable.insert(key, Instant::now() + duration);
    }

    async fn cover_thumbnail(&self, database: &Path, raw_cover: &str) -> Value {
        let Some(cover) = valid_cover(raw_cover) else {
            return Value::Null;
        };
        let folder = thumbnail_folder(database);
        let key = format!("{}:{cover}", folder.display());
        if let Some(image) = self.image(&key) {
            return image;
        }
        let gate = self.gate(&key);
        let _cover = gate.lock().await;
        if let Some(image) = self.image(&key) {
            return image;
        }
        let size = THUMBNAIL_SIZE;
        let path = folder.join(format!("{cover}-{size}.jpg"));
        if let Some(bytes) = read_image(&path, size).await {
            let image = image_value(&cover, size, &bytes);
            self.remember_image(key, image.clone());
            return image;
        }
        let failure_path = folder.join(format!("{cover}.unavailable"));
        if self.is_unavailable(&key) || disk_cooldown(&failure_path).await {
            return Value::Null;
        }
        if self
            .network_paused
            .lock()
            .unwrap()
            .is_some_and(|until| until > Instant::now())
        {
            return Value::Null;
        }

        // This lane has no relationship to metadata jobs or local scanning.
        let Ok(_slot) = self.slots.acquire().await else {
            return Value::Null;
        };
        if self
            .network_paused
            .lock()
            .unwrap()
            .is_some_and(|until| until > Instant::now())
        {
            return Value::Null;
        }
        match self.fetch(&cover, size).await {
            Fetch::Image(bytes) => {
                let image = image_value(&cover, size, &bytes);
                let _ = atomic_write(&folder.join(format!("{cover}-{size}.jpg")), &bytes).await;
                let _ = tokio::fs::remove_file(&failure_path).await;
                self.remember_image(key, image.clone());
                return image;
            }
            Fetch::Failed => {},
            Fetch::Pause(duration) => {
                *self.network_paused.lock().unwrap() = Some(Instant::now() + duration);
            }
        }
        self.remember_unavailable(key, Duration::from_secs(FAILURE_COOLDOWN as u64));
        let until = (chrono::Utc::now().timestamp() + FAILURE_COOLDOWN).to_string();
        let _ = atomic_write(&failure_path, until.as_bytes()).await;
        Value::Null
    }

    async fn fetch(&self, cover: &str, size: u16) -> Fetch {
        let Some(http) = self.http.as_ref() else {
            return Fetch::Failed;
        };
        let Some(url) = cover_url(cover, size) else {
            return Fetch::Failed;
        };
        #[cfg(test)]
        let url = self
            .test_origin
            .as_ref()
            .map(|origin| url.replacen("https://resources.tidal.com", origin, 1))
            .unwrap_or(url);
        let Ok(mut response) = http
            .get(url)
            .header(reqwest::header::ACCEPT, "image/jpeg")
            .send()
            .await
        else {
            return Fetch::Pause(Duration::from_secs(30));
        };
        let status = response.status().as_u16();
        if status == 429 || status >= 500 {
            let delay = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60)
                .clamp(30, 900);
            return Fetch::Pause(Duration::from_secs(delay));
        }
        if status != 200 {
            return Fetch::Failed;
        }
        let jpeg = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("image/jpeg"))
            });
        if !jpeg
            || response
                .content_length()
                .is_some_and(|size_bytes| size_bytes > max_bytes(size) as u64)
        {
            return Fetch::Failed;
        }
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if bytes.len() + chunk.len() <= max_bytes(size) => {
                    bytes.extend_from_slice(&chunk)
                }
                Ok(None) => break,
                _ => return Fetch::Failed,
            }
        }
        if valid_jpeg(&bytes, size) {
            Fetch::Image(bytes)
        } else {
            Fetch::Failed
        }
    }
}

enum Fetch {
    Image(Vec<u8>),
    Failed,
    Pause(Duration),
}

fn thumbnail_folder(database: &Path) -> PathBuf {
    database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("thumbnail-cache")
}

fn valid_cover(value: &str) -> Option<String> {
    if value.len() != 36
        || value.bytes().enumerate().any(|(index, ch)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                ch != b'-'
            } else {
                !ch.is_ascii_hexdigit()
            }
        })
    {
        return None;
    }
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil())
        .map(|id| id.to_string())
}

fn cover_url(cover: &str, size: u16) -> Option<String> {
    if size != THUMBNAIL_SIZE {
        return None;
    }
    Some(format!(
        "https://resources.tidal.com/images/{}/{}x{}.jpg",
        valid_cover(cover)?.replace('-', "/"),
        size,
        size
    ))
}

fn matching_album_cover(value: &Value, release: &str) -> Option<String> {
    let id = crate::tidal::resource_id(&value["id"]);
    if !id.is_empty() && id != release {
        return None;
    }
    value["cover"].as_str().and_then(valid_cover)
}

async fn cached_cover(db: &TursoDb, release: &str, market: &str) -> Option<String> {
    // A stale cover UUID is still useful; do not refresh metadata to paint a row.
    for prefix in ["subscriber-summary", "tag-review", "browse-cover"] {
        if let Some(value) = db
            .get_preference(&format!("{prefix}:{market}:{release}"))
            .await
            .ok()
            .flatten()
        {
            if let Some(cover) = matching_album_cover(&value, release) {
                return Some(cover);
            }
        }
    }
    let value = db
        .get_preference(&format!("subscriber-items:{market}:{release}"))
        .await
        .ok()
        .flatten()?;
    if let Some(cover) = matching_album_cover(&value["album"], release) {
        return Some(cover);
    }
    value["items"]
        .as_array()?
        .iter()
        .take(1000)
        .find_map(|item| {
            let album = item
                .get("album")
                .or_else(|| item.get("item").and_then(|track| track.get("album")))?;
            // Raw track caches may contain an album from another edition.
            (crate::tidal::resource_id(&album["id"]) == release)
                .then(|| matching_album_cover(album, release))
                .flatten()
        })
}

fn max_bytes(_size: u16) -> usize {
    96 * 1024
}

fn image_value(cover: &str, size: u16, bytes: &[u8]) -> Value {
    json!({"src":format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)), "cover_id":cover,"size":size})
}

// Inspect the JPEG's bounded segment headers before allowing the webview to
// decode it. Reject HTML/error bodies, truncated files and oversized dimensions.
fn valid_jpeg(bytes: &[u8], size: u16) -> bool {
    if bytes.len() > max_bytes(size)
        || bytes.len() < 20
        || !bytes.starts_with(&[0xff, 0xd8])
        || !bytes.ends_with(&[0xff, 0xd9])
    {
        return false;
    }
    let mut offset = 2;
    let mut dimensions = None;
    while offset + 3 < bytes.len() {
        if bytes[offset] != 0xff {
            return false;
        }
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        let Some(&marker) = bytes.get(offset) else {
            return false;
        };
        offset += 1;
        if marker == 0xd9 {
            return false;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if offset + 2 > bytes.len() {
            return false;
        }
        let length = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        if length < 2 || offset + length > bytes.len() {
            return false;
        }
        if matches!(marker, 0xc0 | 0xc1 | 0xc2) {
            if length < 8 {
                return false;
            }
            dimensions = Some((
                u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]),
                u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]),
            ));
        }
        if marker == 0xda {
            return dimensions == Some((size, size)) && offset + length < bytes.len() - 2;
        }
        offset += length;
    }
    false
}

async fn read_image(path: &Path, size: u16) -> Option<Vec<u8>> {
    if tokio::fs::metadata(path).await.ok()?.len() > max_bytes(size) as u64 {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    valid_jpeg(&bytes, size).then_some(bytes)
}

async fn disk_cooldown(path: &Path) -> bool {
    if !tokio::fs::metadata(path)
        .await
        .ok()
        .is_some_and(|metadata| metadata.len() <= 32)
    {
        return false;
    }
    let Some(until) = tokio::fs::read_to_string(path)
        .await
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
    else {
        return false;
    };
    let remaining = until.saturating_sub(chrono::Utc::now().timestamp());
    (1..=FAILURE_COOLDOWN).contains(&remaining)
}

async fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    if let Err(error) = tokio::fs::write(&temporary, bytes).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    const COVER: &str = "a1b2c3d4-1234-5678-9abc-def012345678";

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("tibrary-thumbnails-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn db(&self) -> PathBuf {
            self.0.join("library.db")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Reply {
        status: u16,
        body: Vec<u8>,
        headers: String,
        delay: Duration,
    }
    impl Reply {
        fn jpeg() -> Self {
            Self {
                status: 200,
                body: jpeg(),
                headers: "Content-Type: image/jpeg\r\n".into(),
                delay: Duration::ZERO,
            }
        }
        fn status(status: u16) -> Self {
            Self {
                status,
                body: Vec::new(),
                headers: String::new(),
                delay: Duration::ZERO,
            }
        }
    }
    struct Server {
        origin: String,
        requests: Arc<Mutex<Vec<String>>>,
        maximum: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Server {
        fn new(reply: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let maximum = Arc::new(AtomicUsize::new(0));
            let active = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let (saved_requests, saved_maximum, saved_stop) =
                (requests.clone(), maximum.clone(), stop.clone());
            let reply = Arc::new(reply);
            let thread = std::thread::spawn(move || {
                let mut workers = Vec::new();
                while !saved_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let (requests, maximum, active, reply) = (
                                saved_requests.clone(),
                                saved_maximum.clone(),
                                active.clone(),
                                reply.clone(),
                            );
                            workers.push(std::thread::spawn(move || {
                                // BSD sockets can inherit the listener's
                                // nonblocking mode. Read the complete request
                                // before replying; closing with unread headers
                                // can reset an otherwise valid JPEG response.
                                stream.set_nonblocking(false).unwrap();
                                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                                let mut request = Vec::new();
                                while !request.windows(4).any(|bytes|bytes == b"\r\n\r\n") {
                                    let mut chunk = [0; 4096];
                                    let length = stream.read(&mut chunk).expect("Local HTTP fixture request headers");
                                    if length == 0 { return; }
                                    request.extend_from_slice(&chunk[..length]);
                                    assert!(request.len() <= 16_384,"Local HTTP fixture headers exceed limit");
                                }
                                let path = String::from_utf8_lossy(&request).split_whitespace().nth(1).unwrap_or("").to_owned();
                                requests.lock().unwrap().push(path.clone());
                                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                                maximum.fetch_max(count, Ordering::SeqCst);
                                let reply = reply(&path);
                                std::thread::sleep(reply.delay);
                                let headers = format!("HTTP/1.1 {} Test\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n", reply.status, reply.headers, reply.body.len());
                                let _ = stream.write_all(headers.as_bytes());
                                let _ = stream.write_all(&reply.body);
                                active.fetch_sub(1, Ordering::SeqCst);
                            }));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) => panic!("Test server failed: {error}"),
                    }
                }
                for worker in workers {
                    worker.join().unwrap();
                }
            });
            Self {
                origin,
                requests,
                maximum,
                stop,
                thread: Some(thread),
            }
        }
        fn service(&self) -> ThumbnailService {
            ThumbnailService {
                test_origin: Some(self.origin.clone()),
                ..Default::default()
            }
        }
        fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.thread.take().unwrap().join().unwrap();
        }
    }

    fn jpeg() -> Vec<u8> {
        // Solid 160px JPEG fixture generated once; no image library or live CDN is needed.
        base64::engine::general_purpose::STANDARD.decode("/9j/4AAQSkZJRgABAQAASABIAAD/4QCARXhpZgAATU0AKgAAAAgABAEaAAUAAAABAAAAPgEbAAUAAAABAAAARgEoAAMAAAABAAIAAIdpAAQAAAABAAAATgAAAAAAAABIAAAAAQAAAEgAAAABAAOgAQADAAAAAQABAACgAgAEAAAAAQAAAKCgAwAEAAAAAQAAAKAAAAAA/+0AOFBob3Rvc2hvcCAzLjAAOEJJTQQEAAAAAAAAOEJJTQQlAAAAAAAQ1B2M2Y8AsgTpgAmY7PhCfv/AABEIAKAAoAMBIgACEQEDEQH/xAAfAAABBQEBAQEBAQAAAAAAAAAAAQIDBAUGBwgJCgv/xAC1EAACAQMDAgQDBQUEBAAAAX0BAgMABBEFEiExQQYTUWEHInEUMoGRoQgjQrHBFVLR8CQzYnKCCQoWFxgZGiUmJygpKjQ1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4eLj5OXm5+jp6vHy8/T19vf4+fr/xAAfAQADAQEBAQEBAQEBAAAAAAAAAQIDBAUGBwgJCgv/xAC1EQACAQIEBAMEBwUEBAABAncAAQIDEQQFITEGEkFRB2FxEyIygQgUQpGhscEJIzNS8BVictEKFiQ04SXxFxgZGiYnKCkqNTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqCg4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2dri4+Tl5ufo6ery8/T19vf4+fr/2wBDAAICAgICAgMCAgMFAwMDBQYFBQUFBggGBgYGBggKCAgICAgICgoKCgoKCgoMDAwMDAwODg4ODg8PDw8PDw8PDw//2wBDAQICAgQEBAcEBAcQCwkLEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBD/3QAEAAr/2gAMAwEAAhEDEQA/APn+iiivy8/RAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigD//Q+f6KKK/Lz9ECiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKAP/9H5/ooor8vP0QKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooA//0vn+iiivy8/RAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigD//T+f6KKK/Lz9ECiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKAP/9T5/ooor8vP0QKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooA//1fn+iiivy8/RAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigD//W+f6KKK/Lz9ECiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKAP/9f5/ooor8vP0QKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooA//0Pn+iiivy8/RAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigD//Z").unwrap()
    }

    #[test]
    fn only_fixed_thumbnail_urls_and_bounded_jpegs_are_accepted() {
        assert_eq!(
            cover_url(COVER, 160).unwrap(),
            "https://resources.tidal.com/images/a1b2c3d4/1234/5678/9abc/def012345678/160x160.jpg"
        );
        for invalid in [
            "../../secret",
            "https://example.com/image.jpg",
            "urn:uuid:a1b2c3d4-1234-5678-9abc-def012345678",
            "a1b2c3d4123456789abcdef012345678",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(cover_url(invalid, 160).is_none());
        }
        for size in [0, 80, 128, 320, 1280] {
            assert!(cover_url(COVER, size).is_none());
        }
        assert!(valid_jpeg(&jpeg(), 160));
        assert!(!valid_jpeg(&jpeg(), 128));
        assert!(!valid_jpeg(b"<html>CDN error</html>", 160));
        let mut truncated = jpeg();
        truncated.pop();
        assert!(!valid_jpeg(&truncated, 160));
        let mut oversized = jpeg();
        oversized.resize(max_bytes(160) + 1, 0);
        assert!(!valid_jpeg(&oversized, 160));
    }

    #[tokio::test]
    async fn visible_thumbnail_is_cached_across_service_and_database_reopen() {
        let temp = Temp::new();
        let db = TursoDb::open(temp.db()).await.unwrap();
        db.set_preference(
            "subscriber-summary:GB:12",
            &json!({"id":12,"cover":COVER,"checked_at":1}),
        )
        .await
        .unwrap();
        let before = db.revision.load(Ordering::SeqCst);
        let server = Server::new(|_| Reply::jpeg());
        let service = server.service();
        let first = service.catalogue_image(&db, COVER).await;
        assert!(service.catalogue_image(&db, "https://example.com/private.jpg").await.is_null());
        assert!(service.catalogue_image(&db, "../../outside").await.is_null());
        assert_eq!(service.thumbnail(&db, "12", Some("GB")).await, first);
        assert_eq!(first["size"], 160);
        assert_eq!(first["cover_id"], COVER);
        assert!(first["src"]
            .as_str()
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));
        assert_eq!(service.thumbnail(&db, "12", Some("GB")).await, first);
        assert_eq!(server.count(), 1);
        assert_eq!(db.revision.load(Ordering::SeqCst), before);
        assert!(thumbnail_folder(&db.path)
            .join(format!("{COVER}-160.jpg"))
            .is_file());
        assert!(std::fs::read_dir(thumbnail_folder(&db.path))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
        drop(db);
        let reopened = TursoDb::open(temp.db()).await.unwrap();
        assert_eq!(
            server
                .service()
                .thumbnail(&reopened, "12", Some("GB"))
                .await,
            first
        );
        assert_eq!(server.count(), 1, "Reopening must reuse the persisted JPEG");
        assert!(server
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|path| path.ends_with("/160x160.jpg")));
    }

    #[tokio::test]
    async fn concurrent_requests_deduplicate_and_limit_the_network_to_four() {
        let temp = Temp::new();
        let server = Server::new(|_| {
            let mut reply = Reply::jpeg();
            reply.delay = Duration::from_millis(60);
            reply
        });
        let service = Arc::new(server.service());
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..12 {
            let (service, database) = (service.clone(), temp.db());
            requests.spawn(async move { service.cover_thumbnail(&database, COVER).await });
        }
        while let Some(result) = requests.join_next().await {
            assert_eq!(result.unwrap()["size"], 160);
        }
        assert_eq!(server.count(), 1, "One shared cover gets one request");
        for _ in 0..12 {
            let (service, database, cover) =
                (service.clone(), temp.db(), uuid::Uuid::new_v4().to_string());
            requests.spawn(async move { service.cover_thumbnail(&database, &cover).await });
        }
        while let Some(result) = requests.join_next().await {
            assert_eq!(result.unwrap()["size"], 160);
        }
        assert_eq!(server.count(), 13);
        assert!(server.maximum.load(Ordering::SeqCst) <= 4);
        assert!(server.maximum.load(Ordering::SeqCst) > 1);
    }

    #[tokio::test]
    async fn failures_and_bad_bodies_return_placeholders_and_persist_a_cooldown() {
        for failure in [0, 1, 2, 3, 4, 5] {
            let temp = Temp::new();
            let server = Server::new(move |_| match failure {
                0 => Reply::status(404),
                1 => Reply::status(403),
                2 => Reply {
                    body: b"<html>Error</html>".to_vec(),
                    ..Reply::jpeg()
                },
                3 => Reply {
                    body: vec![0; max_bytes(160) + 1],
                    ..Reply::jpeg()
                },
                4 => Reply {
                    headers: "Content-Type: text/html\r\n".into(),
                    ..Reply::jpeg()
                },
                _ => Reply {
                    headers: "Location: https://example.com/arbitrary\r\n".into(),
                    ..Reply::status(302)
                },
            });
            let service = server.service();
            assert!(service.cover_thumbnail(&temp.db(), COVER).await.is_null());
            assert!(service.cover_thumbnail(&temp.db(), COVER).await.is_null());
            assert!(server
                .service()
                .cover_thumbnail(&temp.db(), COVER)
                .await
                .is_null());
            assert_eq!(
                server.count(),
                1,
                "No fallback/retry loop for failure {failure}"
            );
            assert!(!thumbnail_folder(&temp.db())
                .join(format!("{COVER}-160.jpg"))
                .exists());
        }
    }

    #[tokio::test]
    async fn throttling_pauses_other_covers_and_corrupt_cache_is_refetched() {
        let temp = Temp::new();
        let server = Server::new(|_| Reply {
            headers: "Retry-After: 600\r\n".into(),
            ..Reply::status(429)
        });
        let service = server.service();
        assert!(service.cover_thumbnail(&temp.db(), COVER).await.is_null());
        assert!(service
            .cover_thumbnail(&temp.db(), &uuid::Uuid::new_v4().to_string())
            .await
            .is_null());
        assert_eq!(server.count(), 1);
        let temp = Temp::new();
        atomic_write(
            &thumbnail_folder(&temp.db()).join(format!("{COVER}-160.jpg")),
            b"broken JPEG",
        )
        .await
        .unwrap();
        let server = Server::new(|_| Reply::jpeg());
        assert_eq!(
            server.service().cover_thumbnail(&temp.db(), COVER).await["size"],
            160
        );
        assert_eq!(server.count(), 1);
    }

    #[tokio::test]
    async fn absent_or_mismatched_metadata_never_triggers_metadata_or_image_requests() {
        let temp = Temp::new();
        let db = TursoDb::open(temp.db()).await.unwrap();
        let server = Server::new(|_| Reply::jpeg());
        let service = server.service();
        db.set_preference("subscriber-summary:GB:12", &json!({"id":99,"cover":COVER}))
            .await
            .unwrap();
        for id in [
            "12",
            "13",
            "https://example.com",
            "../12",
            "",
            "123456789012345678901",
        ] {
            assert!(service.thumbnail(&db, id, Some("GB")).await.is_null());
        }
        assert_eq!(server.count(), 0);
        db.set_preference(
            "subscriber-items:GB:14",
            &json!({"items":[{"album":{"id":15,"cover":COVER}}]}),
        )
        .await
        .unwrap();
        assert!(service.thumbnail(&db, "14", Some("GB")).await.is_null());
        assert_eq!(server.count(), 0);
        db.set_preference(
            "subscriber-items:GB:15",
            &json!({"items":[{"album":{"id":15,"cover":COVER}}]}),
        )
        .await
        .unwrap();
        assert_eq!(
            service.thumbnail(&db, "15", Some("GB")).await["cover_id"],
            COVER
        );
        assert_eq!(server.count(), 1);
    }

    #[tokio::test]
    async fn thumbnail_rpc_runs_during_startup_and_never_waits_on_the_job_gate() {
        let temp = Temp::new();
        let db = TursoDb::open(temp.db()).await.unwrap();
        db.set_preference("subscriber-summary:GB:12", &json!({"id":12,"cover":COVER}))
            .await
            .unwrap();
        atomic_write(
            &thumbnail_folder(&temp.db()).join(format!("{COVER}-160.jpg")),
            &jpeg(),
        )
        .await
        .unwrap();
        let backend = Arc::new(crate::Backend::new());
        backend.startup_scan_pending.store(true, Ordering::SeqCst);
        let _busy = backend.dispatch_gate.lock().await;
        let revision = db.revision.load(Ordering::SeqCst);
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            crate::handle_rpc_call(
                None,
                &backend,
                &db,
                "artwork.thumbnail".into(),
                json!({"release_id":"12","market":"GB"}),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response["size"], 160);
        assert_eq!(db.revision.load(Ordering::SeqCst), revision);
        assert!(backend.logs.snapshot().is_empty());
        assert!(backend.view_cache.lock().unwrap().is_empty());
    }
}
