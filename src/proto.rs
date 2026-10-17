//! What the tray process and the window process tell each other: window
//! messages (WM_APP + n) with small numbers in wParam and lParam. Settings
//! travel through settings.json. Pure, no Windows calls.

use crate::settings;

const WM_APP: u32 = 0x8000;

/// To the tray, internal: the default playback device changed.
pub const TRAY_DEVICE: u32 = WM_APP + 1;
/// To the tray (posted): a window process is up. wParam = its pid, lParam = its message window.
pub const TRAY_HELLO: u32 = WM_APP + 2;
/// To the tray (sent): read settings.json again. Answer: a hotkey status.
pub const TRAY_RELOAD: u32 = WM_APP + 3;
/// To the tray (sent): wParam 1 = let go of the hotkey while a new one is
/// recorded, 0 = register the saved one again. Answer: a hotkey status.
pub const TRAY_HOTKEY: u32 = WM_APP + 4;
/// To the tray (sent): how is the hotkey? Answer: a hotkey status.
pub const TRAY_STATUS: u32 = WM_APP + 5;
/// To the tray (posted): Quit was clicked in the window.
pub const TRAY_QUIT: u32 = WM_APP + 6;
/// To the tray (posted): the window is on screen (wParam 1) or hidden (0).
pub const TRAY_SHOWN: u32 = WM_APP + 7;
/// To the tray, internal: a step of the cycle test (see `TestStep`).
pub const TRAY_TEST: u32 = WM_APP + 8;
/// To the window process (sent): wParam = `ask_code` (plus `HAS_POINT`),
/// lParam = `pack_point` of the tray icon. Answer: `OPEN_*`, or 0 if it is exiting.
pub const WINDOW_OPEN: u32 = WM_APP + 20;
/// In wParam of `WINDOW_OPEN`: lParam holds a point.
pub const HAS_POINT: usize = 0x10;

pub const OPEN_SHOWN: isize = 1;
pub const OPEN_HIDDEN: isize = 2;
pub const OPEN_NOTHING: isize = 3;

/// What the tray asks the window, as wParam.
pub fn ask_code(a: crate::lifetime::Ask) -> usize {
    use crate::lifetime::Ask;
    match a {
        Ask::Toggle => 0,
        Ask::Show => 1,
        Ask::Hide => 2,
    }
}

pub fn ask_from(code: usize) -> Option<crate::lifetime::Ask> {
    use crate::lifetime::Ask;
    match code & !HAS_POINT {
        0 => Some(Ask::Toggle),
        1 => Some(Ask::Show),
        2 => Some(Ask::Hide),
        _ => None,
    }
}

/// A screen point in one lParam (64-bit): x in the low 32 bits, y in the high ones.
pub fn pack_point(x: i32, y: i32) -> isize {
    (((y as u32 as u64) << 32) | x as u32 as u64) as isize
}

pub fn unpack_point(l: isize) -> (i32, i32) {
    let l = l as u64;
    (l as u32 as i32, (l >> 32) as u32 as i32)
}

/// The tray's hotkey, as one answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HotkeyStatus {
    /// settings.json has a hotkey the app does not know; the default is used.
    pub unknown: bool,
    /// No hotkey is registered: another program has it.
    pub taken: bool,
    /// Let go of while a new one is recorded.
    pub paused: bool,
    /// Answer to a reload: the hotkey in settings.json was refused (another
    /// program has it) and the one before is kept.
    pub refused: bool,
}

const ANSWERED: isize = 0x100;

pub fn encode(s: HotkeyStatus) -> isize {
    ANSWERED | s.unknown as isize | (s.taken as isize) << 1 | (s.paused as isize) << 2 | (s.refused as isize) << 3
}

/// None: no answer (the message failed or timed out).
pub fn decode(r: isize) -> Option<HotkeyStatus> {
    (r & ANSWERED != 0).then_some(HotkeyStatus {
        unknown: r & 1 != 0,
        taken: r & 2 != 0,
        paused: r & 4 != 0,
        refused: r & 8 != 0,
    })
}

/// The hotkey the tray uses for the text in settings.json, and whether that text was unknown.
pub fn wanted_hotkey(text: &str) -> (String, bool) {
    match settings::parse_hotkey(text) {
        Some(_) => (text.to_string(), false),
        None => (settings::DEFAULT_HOTKEY.to_string(), true),
    }
}

/// The hotkey to show and the problem with it, if any, for the text in
/// settings.json and the tray's status (None: the tray did not answer).
pub fn hotkey_view(text: &str, status: Option<HotkeyStatus>) -> (String, Option<String>) {
    let (used, unknown) = wanted_hotkey(text);
    let Some(s) = status else {
        return (used, Some("The tray part of Audio Lite did not answer.".into()));
    };
    let mut problems = Vec::new();
    if unknown || s.unknown {
        problems.push(format!("Unknown hotkey \"{text}\" in settings.json, using {used}."));
    }
    if s.taken {
        problems.push(format!("{used} is already used by another program."));
    }
    (used, (!problems.is_empty()).then(|| problems.join(" ")))
}

/// Steps of the cycle test, as wParam of `TRAY_TEST`.
pub const TEST_CLICK: usize = 1;
pub const TEST_HIDE: usize = 2;
/// A real tray click (toggle).
pub const TEST_TOGGLE: usize = 3;
/// Kill every window process (stress test).
pub const TEST_KILL: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifetime::Ask;

    #[test]
    fn points_survive_packing() {
        for (x, y) in [(0, 0), (1, -1), (-1920, 1050), (i32::MIN, i32::MAX), (3839, -2160)] {
            assert_eq!(unpack_point(pack_point(x, y)), (x, y));
        }
    }

    #[test]
    fn ask_codes_round_trip() {
        for a in [Ask::Toggle, Ask::Show, Ask::Hide] {
            assert_eq!(ask_from(ask_code(a)), Some(a));
        }
        assert_eq!(ask_from(ask_code(Ask::Hide) | HAS_POINT), Some(Ask::Hide));
        assert_eq!(ask_from(9), None);
    }

    #[test]
    fn status_round_trip_and_no_answer() {
        for bits in 0..16 {
            let s = HotkeyStatus {
                unknown: bits & 1 != 0,
                taken: bits & 2 != 0,
                paused: bits & 4 != 0,
                refused: bits & 8 != 0,
            };
            assert_eq!(decode(encode(s)), Some(s));
            assert_ne!(encode(s), 0);
        }
        // SendMessageTimeout gives 0 when the tray is gone or does not answer.
        assert_eq!(decode(0), None);
        assert_eq!(decode(1), None);
    }

    #[test]
    fn hotkey_texts() {
        let ok = Some(HotkeyStatus::default());
        assert_eq!(hotkey_view("Ctrl+Shift+F9", ok), ("Ctrl+Shift+F9".to_string(), None));
        let taken = Some(HotkeyStatus { taken: true, ..Default::default() });
        assert_eq!(
            hotkey_view("Ctrl+Alt+A", taken),
            ("Ctrl+Alt+A".to_string(), Some("Ctrl+Alt+A is already used by another program.".to_string()))
        );
        let (used, err) = hotkey_view("Hyper+Q", ok);
        assert_eq!(used, settings::DEFAULT_HOTKEY);
        assert_eq!(err.as_deref(), Some("Unknown hotkey \"Hyper+Q\" in settings.json, using Ctrl+Alt+A."));
        let (used, err) = hotkey_view("Alt+F1", None);
        assert_eq!(used, "Alt+F1");
        assert!(err.is_some());
        assert_eq!(wanted_hotkey("Alt+F1"), ("Alt+F1".to_string(), false));
        assert_eq!(wanted_hotkey(""), (settings::DEFAULT_HOTKEY.to_string(), true));
    }
}
