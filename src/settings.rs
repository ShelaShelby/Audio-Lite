//! settings.json: the hotkey and the devices the hotkey skips.
//! A missing or broken file gives the defaults. Pure parsing, no Windows calls.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "settings.json";
pub const APP_DIR: &str = "Audio Lite";
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+A";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// For example "Ctrl+Alt+A".
    pub hotkey: String,
    /// Device names the hotkey skips (they stay in the tray menu).
    pub exclude: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { hotkey: DEFAULT_HOTKEY.to_string(), exclude: Vec::new() }
    }
}

/// Parse the file contents. `None` (no file) or broken JSON gives the defaults.
pub fn parse(text: Option<&str>) -> Settings {
    text.and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or_default()
}

pub fn to_json(s: &Settings) -> String {
    serde_json::to_string_pretty(s).unwrap_or_default() + "\n"
}

/// Where the file lives: next to the exe if a settings.json is already there
/// (portable use), otherwise `%APPDATA%\Audio Lite\settings.json`.
pub fn choose_path(exe_dir: Option<&Path>, appdata: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let portable = exe_dir.map(|d| d.join(FILE_NAME));
    if let Some(p) = portable.as_ref().filter(|p| exists(p)) {
        return Some(p.clone());
    }
    appdata.map(|a| a.join(APP_DIR).join(FILE_NAME)).or(portable)
}

/// Win32 modifier flags (same values as MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN).
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// Parse "Ctrl+Alt+A" into (modifier flags, virtual-key code).
/// Keys: A-Z, 0-9, F1-F24. At least one modifier is required.
pub fn parse_hotkey(text: &str) -> Option<(u32, u32)> {
    let mut mods = 0;
    let mut key = None;
    for part in text.split('+').map(|p| p.trim().to_ascii_uppercase()) {
        let flag = match part.as_str() {
            "CTRL" | "CONTROL" => MOD_CONTROL,
            "ALT" => MOD_ALT,
            "SHIFT" => MOD_SHIFT,
            "WIN" | "WINDOWS" => MOD_WIN,
            _ => 0,
        };
        if flag != 0 {
            mods |= flag;
            continue;
        }
        if key.is_some() {
            return None; // two keys
        }
        key = Some(key_code(&part)?);
    }
    if mods == 0 {
        return None;
    }
    Some((mods, key?))
}

fn key_code(name: &str) -> Option<u32> {
    let b = name.as_bytes();
    match b {
        [c] if c.is_ascii_uppercase() || c.is_ascii_digit() => Some(*c as u32),
        [b'F', rest @ ..] if !rest.is_empty() => {
            let n: u32 = std::str::from_utf8(rest).ok()?.parse().ok()?;
            (1..=24).contains(&n).then_some(0x70 + n - 1) // VK_F1 = 0x70
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_file() {
        let s = parse(Some(r#"{ "hotkey": "Ctrl+Shift+F9", "exclude": ["TV", "Monitor"] }"#));
        assert_eq!(s.hotkey, "Ctrl+Shift+F9");
        assert_eq!(s.exclude, vec!["TV", "Monitor"]);
    }

    #[test]
    fn missing_file_gives_defaults() {
        assert_eq!(parse(None), Settings::default());
        assert_eq!(parse(None).hotkey, "Ctrl+Alt+A");
    }

    #[test]
    fn broken_file_gives_defaults() {
        assert_eq!(parse(Some("{ hotkey: ")), Settings::default());
        assert_eq!(parse(Some("")), Settings::default());
        assert_eq!(parse(Some("[1, 2]")), Settings::default());
        assert_eq!(parse(Some(r#"{ "exclude": "TV" }"#)), Settings::default());
    }

    #[test]
    fn missing_fields_get_defaults() {
        let s = parse(Some(r#"{ "exclude": ["TV"], "unknown": 1 }"#));
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
        assert_eq!(s.exclude, vec!["TV"]);
    }

    #[test]
    fn notepad_bom_is_fine() {
        assert_eq!(parse(Some("\u{feff}{\"hotkey\":\"Alt+F1\"}")).hotkey, "Alt+F1");
    }

    #[test]
    fn round_trip() {
        let s = Settings { hotkey: "Win+F2".into(), exclude: vec!["A".into()] };
        assert_eq!(parse(Some(&to_json(&s))), s);
    }

    #[test]
    fn hotkeys() {
        assert_eq!(parse_hotkey("Ctrl+Alt+A"), Some((MOD_CONTROL | MOD_ALT, 'A' as u32)));
        assert_eq!(parse_hotkey(" ctrl + shift + f12 "), Some((MOD_CONTROL | MOD_SHIFT, 0x7B)));
        assert_eq!(parse_hotkey("Win+1"), Some((MOD_WIN, '1' as u32)));
        assert_eq!(parse_hotkey("A"), None);
        assert_eq!(parse_hotkey("Ctrl+Alt"), None);
        assert_eq!(parse_hotkey("Ctrl+A+B"), None);
        assert_eq!(parse_hotkey("Ctrl+F25"), None);
        assert_eq!(parse_hotkey("Ctrl+Space"), None);
        assert_eq!(parse_hotkey(""), None);
    }

    #[test]
    fn path_prefers_file_next_to_exe() {
        let exe = Path::new("C:\\Tools");
        let app = Path::new("C:\\Users\\a\\AppData\\Roaming");
        let next_to_exe = exe.join(FILE_NAME);
        assert_eq!(choose_path(Some(exe), Some(app), |p| p == next_to_exe), Some(next_to_exe.clone()));
        assert_eq!(choose_path(Some(exe), Some(app), |_| false), Some(app.join(APP_DIR).join(FILE_NAME)));
        assert_eq!(choose_path(Some(exe), None, |_| false), Some(next_to_exe));
        assert_eq!(choose_path(None, None, |_| false), None);
    }
}
