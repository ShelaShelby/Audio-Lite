//! What the window shows, and the copy of it kept in cache.json so a new
//! window process can paint the last known devices and volumes at once,
//! before Windows audio has answered. Pure, no Windows calls.

use crate::cycle::Device;
use crate::volume;
use serde::{Deserialize, Serialize};

/// The file next to settings.json.
pub const CACHE_FILE: &str = "cache.json";
/// Larger files are not read (nothing the app writes comes close).
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// More rows than this are dropped from the cache (the live list has them all).
const MAX_ROWS: usize = 200;
/// Longer ids, keys and names are not taken from the cache.
const MAX_TEXT: usize = 2048;

/// What a slider controls.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// The default device's master volume.
    Master,
    /// One app row, by its key (see `mixer::Row::key`).
    App(String),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppLevel {
    pub key: String,
    pub name: String,
    /// 0.0-1.0
    pub volume: f32,
    pub muted: bool,
}

/// What the window shows. Written by the audio thread, and by the window for
/// its own volume and mute changes, so the window shows them before they are applied.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    pub devices: Vec<Device>,
    pub default_id: Option<String>,
    pub default_name: String,
    /// Master volume (0.0-1.0) and mute of the default device.
    pub master: Option<(f32, bool)>,
    pub apps: Vec<AppLevel>,
    /// No device, or Windows audio not available.
    #[serde(skip)]
    pub error: Option<String>,
    #[serde(skip)]
    pub apps_error: Option<String>,
    /// The last switching problem, until the next switch works.
    #[serde(skip)]
    pub note: Option<String>,
}

impl Model {
    pub fn level(&self, t: &Target) -> Option<f32> {
        match t {
            Target::Master => self.master.map(|m| m.0),
            Target::App(k) => self.apps.iter().find(|a| &a.key == k).map(|a| a.volume),
        }
    }

    pub fn muted(&self, t: &Target) -> Option<bool> {
        match t {
            Target::Master => self.master.map(|m| m.1),
            Target::App(k) => self.apps.iter().find(|a| &a.key == k).map(|a| a.muted),
        }
    }

    pub fn set_level(&mut self, t: &Target, v: f32) {
        match t {
            Target::Master => {
                if let Some(m) = &mut self.master {
                    m.0 = v;
                }
            }
            Target::App(k) => {
                if let Some(a) = self.apps.iter_mut().find(|a| &a.key == k) {
                    a.volume = v;
                }
            }
        }
    }

    pub fn set_muted(&mut self, t: &Target, on: bool) {
        match t {
            Target::Master => {
                if let Some(m) = &mut self.master {
                    m.1 = on;
                }
            }
            Target::App(k) => {
                if let Some(a) = self.apps.iter_mut().find(|a| &a.key == k) {
                    a.muted = on;
                }
            }
        }
    }
}

/// The text of cache.json for `m` (devices, the default one, volumes; no problems).
pub fn to_cache(m: &Model) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

fn short(s: &str) -> bool {
    s.len() <= MAX_TEXT
}

/// The model in cache.json, or None if the file is missing, broken or odd.
/// Volumes are clamped, overlong texts and rows beyond `MAX_ROWS` are dropped.
pub fn from_cache(text: Option<&str>) -> Option<Model> {
    let mut m: Model = serde_json::from_str(text?.trim_start_matches('\u{feff}')).ok()?;
    m.devices.retain(|d| !d.id.is_empty() && short(&d.id) && short(&d.name));
    m.devices.truncate(MAX_ROWS);
    m.apps.retain(|a| !a.key.is_empty() && short(&a.key) && short(&a.name));
    m.apps.truncate(MAX_ROWS);
    for a in &mut m.apps {
        a.volume = volume::to_scalar(a.volume)?;
    }
    if let Some((v, muted)) = m.master {
        m.master = Some((volume::to_scalar(v)?, muted));
    }
    if m.default_id.as_deref().is_some_and(|id| !short(id)) || !short(&m.default_name) {
        return None;
    }
    // Error texts are never cached: with no device there is nothing to show early.
    (!m.devices.is_empty()).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Model {
        Model {
            devices: vec![
                Device { id: "{0.0.0}.{a}".into(), name: "Speakers (Realtek(R) Audio)".into() },
                Device { id: "{0.0.0}.{b}".into(), name: "Kopfhörer 😀".into() },
            ],
            default_id: Some("{0.0.0}.{a}".into()),
            default_name: "Speakers (Realtek(R) Audio)".into(),
            master: Some((0.42, false)),
            apps: vec![
                AppLevel {
                    key: "exe:c:\\b\\chrome.exe".into(),
                    name: "Google Chrome".into(),
                    volume: 1.0,
                    muted: false,
                },
                AppLevel { key: "system".into(), name: "System sounds".into(), volume: 0.3, muted: true },
            ],
            error: Some("not cached".into()),
            apps_error: Some("not cached".into()),
            note: Some("not cached".into()),
        }
    }

    #[test]
    fn round_trip_without_problems() {
        let m = sample();
        let back = from_cache(Some(&to_cache(&m))).unwrap();
        assert_eq!(back, Model { error: None, apps_error: None, note: None, ..m });
    }

    #[test]
    fn missing_or_broken_files_are_ignored() {
        assert_eq!(from_cache(None), None);
        assert_eq!(from_cache(Some("")), None);
        assert_eq!(from_cache(Some("{")), None);
        assert_eq!(from_cache(Some("[]")), None);
        assert_eq!(from_cache(Some("null")), None);
        assert_eq!(from_cache(Some("{}")), None, "no devices: nothing to show");
        assert_eq!(from_cache(Some(r#"{"devices": 5}"#)), None);
        assert_eq!(from_cache(Some(r#"{"devices":[{"id":"a","name":"A"}],"master":[0.5]}"#)), None);
        let good = to_cache(&sample());
        for end in 0..good.len() {
            if good.is_char_boundary(end) {
                assert_eq!(from_cache(Some(&good[..end])), None, "{end}");
            }
        }
    }

    #[test]
    fn odd_values_are_cleaned() {
        let text = r#"{"devices":[{"id":"a","name":"A"},{"id":"","name":"no id"}],
            "master":[7.5,true],
            "apps":[{"key":"k","name":"n","volume":-3,"muted":false},{"key":"","name":"x","volume":0.5,"muted":false}],
            "unknown": 1}"#;
        let m = from_cache(Some(text)).unwrap();
        assert_eq!(m.devices.len(), 1);
        assert_eq!(m.master, Some((1.0, true)));
        assert_eq!(m.apps, vec![AppLevel { key: "k".into(), name: "n".into(), volume: 0.0, muted: false }]);
        assert_eq!(m.default_id, None);
        let mut big = sample();
        big.apps = (0..1000)
            .map(|i| AppLevel { key: format!("pid:{i}"), name: format!("P{i}"), volume: 0.5, muted: false })
            .collect();
        big.devices[1].name = "x".repeat(MAX_TEXT + 1);
        let m = from_cache(Some(&to_cache(&big))).unwrap();
        assert_eq!(m.apps.len(), MAX_ROWS);
        assert_eq!(m.devices.len(), 1);
    }

    #[test]
    fn notepad_bom_is_fine() {
        let text = format!("\u{feff}{}", to_cache(&sample()));
        assert!(from_cache(Some(&text)).is_some());
    }

    /// Random bytes never panic.
    #[test]
    fn garbage_never_panics() {
        let alphabet = br#"{}[]":,. -0123456789eE+truefalsnl"devicesmasterappskeynamevolumeid"#;
        let mut seed = 0x0bad_5eed_1234_5678u64;
        for _ in 0..20_000 {
            let mut text = String::new();
            for _ in 0..(seed % 60) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push(alphabet[(seed % alphabet.len() as u64) as usize] as char);
            }
            if let Some(m) = from_cache(Some(&text)) {
                assert!(m.master.is_none_or(|(v, _)| (0.0..=1.0).contains(&v)));
            }
            seed = seed.wrapping_add(1);
        }
    }

    #[test]
    fn levels_and_mutes_by_target() {
        let mut m = sample();
        let chrome = Target::App("exe:c:\\b\\chrome.exe".into());
        assert_eq!(m.level(&Target::Master), Some(0.42));
        m.set_level(&chrome, 0.25);
        m.set_muted(&Target::Master, true);
        assert_eq!(m.level(&chrome), Some(0.25));
        assert_eq!(m.muted(&Target::Master), Some(true));
        // A row that is gone: nothing to read, nothing written.
        let gone = Target::App("gone".into());
        m.set_level(&gone, 0.1);
        assert_eq!(m.level(&gone), None);
        assert_eq!(m.muted(&gone), None);
    }
}
