//! settings.json: the hotkey and the devices the hotkey skips.
//! A missing or broken file gives the defaults. Pure parsing, no Windows calls.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "settings.json";
pub const APP_DIR: &str = "Audio Lite";
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+A";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Settings {
    /// For example "Ctrl+Alt+A".
    pub hotkey: String,
    /// Device names the hotkey skips (unticked "In hotkey cycle" in the window).
    pub exclude: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { hotkey: DEFAULT_HOTKEY.to_string(), exclude: Vec::new() }
    }
}

/// Parse the file contents. `None` (no file) or broken JSON gives the defaults.
/// Each field is read on its own: a field with the wrong type gets its default
/// and the other one is still used. Unknown fields are ignored.
pub fn parse(text: Option<&str>) -> Settings {
    let mut s = Settings::default();
    let Some(Value::Object(map)) = text.and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
    else {
        return s;
    };
    if let Some(Value::String(h)) = map.get("hotkey") {
        s.hotkey = h.clone();
    }
    if let Some(Value::Array(list)) = map.get("exclude") {
        s.exclude =
            list.iter().filter_map(Value::as_str).filter(|n| !n.trim().is_empty()).map(str::to_string).collect();
    }
    s
}

/// The text of the file. `Ok(None)`: there is no file (or no path). `Err`: it
/// exists but could not be read right now; then the settings in use are kept.
pub fn read(path: Option<&Path>) -> std::io::Result<Option<String>> {
    let Some(path) = path else { return Ok(None) };
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn to_json(s: &Settings) -> String {
    serde_json::to_string_pretty(s).unwrap_or_default() + "\n"
}

/// Write the file, making its folder if needed. Only called after the user
/// changed something in the window. Written to a temporary file first and then
/// renamed over the old one, so a crash or a reader never sees half a file.
pub fn save(path: &Path, s: &Settings) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(to_json(s).as_bytes())?;
        f.sync_all()
    });
    let res = written.and_then(|()| std::fs::rename(&tmp, path));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
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
        assert_eq!(parse(Some("null")), Settings::default());
        assert_eq!(parse(Some("\u{feff}")), Settings::default());
        // Truncated in the middle of a write.
        assert_eq!(parse(Some(r#"{ "hotkey": "Ctrl+Shift+F9", "exclude": ["T"#)), Settings::default());
    }

    #[test]
    fn a_wrong_field_keeps_the_other_one() {
        let s = parse(Some(r#"{ "hotkey": 5, "exclude": ["TV", 3, null, "", "  ", "Monitor"] }"#));
        assert_eq!(s, Settings { hotkey: DEFAULT_HOTKEY.into(), exclude: vec!["TV".into(), "Monitor".into()] });
        let s = parse(Some(r#"{ "hotkey": "Alt+F1", "exclude": {"a": 1} }"#));
        assert_eq!(s, Settings { hotkey: "Alt+F1".into(), exclude: vec![] });
    }

    /// The files 0.1.0 to 0.4.0 wrote (and the README example) load unchanged.
    #[test]
    fn old_versions_load() {
        let old = "{\n  \"hotkey\": \"Ctrl+Alt+A\",\n  \"exclude\": [\"LG TV\", \"Realtek Digital Output\"]\n}\n";
        let s = parse(Some(old));
        assert_eq!(s.hotkey, "Ctrl+Alt+A");
        assert_eq!(s.exclude, vec!["LG TV", "Realtek Digital Output"]);
        assert_eq!(parse(Some(&to_json(&s))), s);
        assert_eq!(parse(Some("{}")), Settings::default());
    }

    /// Random bytes and cut-off files never panic.
    #[test]
    fn garbage_never_panics() {
        let good = to_json(&Settings { hotkey: "Ctrl+Shift+F9".into(), exclude: vec!["Lautsprecher Ä".into()] });
        for end in 0..=good.len() {
            if good.is_char_boundary(end) {
                let _ = parse(Some(&good[..end]));
            }
        }
        let mut seed = 0x1234_5678_9abc_def0u64;
        let alphabet = b"{}[]\":,. \n\\tu0123456789abcdefnrlsxeTFhotkyclud+-";
        for _ in 0..20_000 {
            let mut text = String::new();
            for _ in 0..(seed % 40) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push(alphabet[(seed % alphabet.len() as u64) as usize] as char);
            }
            let s = parse(Some(&text));
            assert!(!s.hotkey.is_empty() || text.contains("\"\""));
            seed = seed.wrapping_add(1);
        }
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
    fn save_makes_the_folder_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("audio-lite-test-{}", std::process::id()));
        let path = dir.join(APP_DIR).join(FILE_NAME);
        let s = Settings { hotkey: "Ctrl+Shift+F9".into(), exclude: vec!["TV".into()] };
        save(&path, &s).unwrap();
        assert_eq!(parse(std::fs::read_to_string(&path).ok().as_deref()), s);
        // Saving again replaces the file and leaves no temporary file behind.
        let s2 = Settings { hotkey: "Alt+F1".into(), exclude: vec![] };
        save(&path, &s2).unwrap();
        assert_eq!(parse(read(Some(&path)).unwrap().as_deref()), s2);
        let names: Vec<_> =
            std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from(FILE_NAME)]);
        assert_eq!(read(Some(&dir.join("missing.json"))).unwrap(), None);
        assert_eq!(read(None).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
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
