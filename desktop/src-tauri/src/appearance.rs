//! The native accent is read, never written. AppKit objects stay on the main
//! thread; callers and the web UI only receive ordinary serializable values.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SystemAccent {
    pub name: String,
    pub hex: String,
    pub rgb: [u8; 3],
}

impl Default for SystemAccent {
    fn default() -> Self {
        Self::new("Multicolour", [0, 122, 255])
    }
}

impl SystemAccent {
    fn new(name: &str, rgb: [u8; 3]) -> Self {
        Self {
            name: name.to_owned(),
            hex: format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]),
            rgb,
        }
    }
}

/// Resolve the actual control accent, including macOS Multicolour. A short
/// cache avoids repeated AppKit dispatch when several views ask at once; a UI
/// poll or window-focus refresh picks up subsequent System Settings changes.
pub async fn system_accent(app: Option<&tauri::AppHandle>) -> SystemAccent {
    #[cfg(target_os = "macos")]
    {
        macos::system_accent(app).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        SystemAccent::default()
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::SystemAccent;
    use objc2::{rc::autoreleasepool, MainThreadMarker};
    use objc2_app_kit::{NSColor, NSColorSpace};
    use objc2_foundation::{NSString, NSUserDefaults};
    use std::{
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };

    const CACHE_AGE: Duration = Duration::from_secs(2);
    static CACHE: OnceLock<Mutex<Option<(Instant, SystemAccent)>>> = OnceLock::new();

    fn cached(fresh_only: bool) -> Option<SystemAccent> {
        CACHE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()
            .and_then(|cache| {
                cache.as_ref().and_then(|(time, accent)| {
                    (!fresh_only || time.elapsed() < CACHE_AGE).then(|| accent.clone())
                })
            })
    }

    fn remember(accent: &SystemAccent) {
        if let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(None)).lock() {
            *cache = Some((Instant::now(), accent.clone()));
        }
    }

    pub async fn system_accent(app: Option<&tauri::AppHandle>) -> SystemAccent {
        if let Some(accent) = cached(true) {
            return accent;
        }
        if let Some(marker) = MainThreadMarker::new() {
            let accent = read_native(marker);
            remember(&accent);
            return accent;
        }
        // The command-line test/RPC runner has no AppKit event loop. Never
        // initialize AppKit from its worker threads or wait for a missing loop.
        if let Some(app) = app {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            if app
                .run_on_main_thread(move || {
                    if let Some(marker) = MainThreadMarker::new() {
                        let accent = read_native(marker);
                        remember(&accent);
                        let _ = sender.send(accent);
                    }
                })
                .is_ok()
            {
                if let Ok(Ok(accent)) = tokio::time::timeout(Duration::from_secs(1), receiver).await
                {
                    return accent;
                }
            }
        }
        cached(false).unwrap_or_default()
    }

    fn read_native(_main_thread: MainThreadMarker) -> SystemAccent {
        autoreleasepool(|_| {
            // controlAccentColor is dynamic and may be grayscale/P3. Resolve
            // it in sRGB before accessing RGB components or serializing CSS.
            let colour = NSColor::controlAccentColor();
            let Some(colour) = colour.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace()) else {
                return SystemAccent::default();
            };
            let rgb = [
                channel(colour.redComponent()),
                channel(colour.greenComponent()),
                channel(colour.blueComponent()),
            ];
            let defaults = NSUserDefaults::standardUserDefaults();
            let key = NSString::from_str("AppleAccentColor");
            // This optional preference hint is only for the human-readable
            // label. The actual colour always comes from the public AppKit
            // API, so new/unknown preference values cannot break rendering.
            let selection = defaults
                .stringForKey(&key)
                .and_then(|value| value.to_string().parse::<i64>().ok());
            SystemAccent::new(selection_name(selection), rgb)
        })
    }

    fn channel(value: f64) -> u8 {
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    fn selection_name(value: Option<i64>) -> &'static str {
        match value {
            Some(-1) => "Graphite",
            Some(0) => "Red",
            Some(1) => "Orange",
            Some(2) => "Yellow",
            Some(3) => "Green",
            Some(4) => "Blue",
            Some(5) => "Purple",
            Some(6) => "Pink",
            // Multicolour is normally absent; -2 is accepted as well. An
            // unknown future choice retains its native colour, labelled macOS.
            None | Some(-2) => "Multicolour",
            _ => "macOS",
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn native_preference_names_include_multicolour_and_graphite() {
            assert_eq!(selection_name(None), "Multicolour");
            assert_eq!(selection_name(Some(-2)), "Multicolour");
            assert_eq!(selection_name(Some(-1)), "Graphite");
            for (value, name) in [
                (0, "Red"),
                (1, "Orange"),
                (2, "Yellow"),
                (3, "Green"),
                (4, "Blue"),
                (5, "Purple"),
                (6, "Pink"),
            ] {
                assert_eq!(selection_name(Some(value)), name);
            }
            assert_eq!(selection_name(Some(7)), "macOS");
        }

        #[test]
        fn native_rgb_is_bounded_and_serialized_as_css() {
            let rgb = [channel(-0.01), channel(0.5), channel(1.2)];
            assert_eq!(rgb, [0, 128, 255]);
            assert_eq!(SystemAccent::new("Blue", rgb).hex, "#0080ff");
        }
    }
}
