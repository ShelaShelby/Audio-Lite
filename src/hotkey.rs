//! A key combination recorded in the window, checked and turned into the text
//! stored in settings.json ("Ctrl+Alt+A"). Pure, no Windows calls.

use crate::settings;

/// The modifier keys held down with the key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// "Ctrl+Alt+A" for Ctrl and Alt held with the key named `key` ("A", "7", "F9").
/// Refused without a modifier, or for a key the hotkey cannot use.
pub fn combo(mods: Mods, key: &str) -> Result<String, &'static str> {
    if !(mods.ctrl || mods.alt || mods.shift) {
        return Err("Hold Ctrl, Alt or Shift with the key.");
    }
    let mut text = String::new();
    for (on, name) in [(mods.ctrl, "Ctrl"), (mods.alt, "Alt"), (mods.shift, "Shift")] {
        if on {
            text.push_str(name);
            text.push('+');
        }
    }
    text.push_str(&key.trim().to_ascii_uppercase());
    match settings::parse_hotkey(&text) {
        Some(_) => Ok(text),
        None => Err("Use a letter, a digit or F1 to F24."),
    }
}

/// "Ctrl + Alt + A" for showing in the window.
pub fn pretty(text: &str) -> String {
    text.split('+').map(str::trim).collect::<Vec<_>>().join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_ALT: Mods = Mods { ctrl: true, alt: true, shift: false };

    #[test]
    fn valid_combos() {
        assert_eq!(combo(CTRL_ALT, "A"), Ok("Ctrl+Alt+A".to_string()));
        assert_eq!(combo(Mods { shift: true, ..Mods::default() }, "f9"), Ok("Shift+F9".to_string()));
        assert_eq!(combo(Mods { ctrl: true, alt: true, shift: true }, "7"), Ok("Ctrl+Alt+Shift+7".to_string()));
        // What is stored parses back to the same key.
        let text = combo(CTRL_ALT, "B").unwrap();
        assert_eq!(settings::parse_hotkey(&text), Some((settings::MOD_CONTROL | settings::MOD_ALT, 'B' as u32)));
    }

    #[test]
    fn no_modifier_is_refused() {
        assert!(combo(Mods::default(), "A").is_err());
        assert!(combo(Mods::default(), "F5").is_err());
    }

    #[test]
    fn unusable_keys_are_refused() {
        for key in ["Space", "Enter", "Escape", "F25", "", "Up", "Minus"] {
            assert!(combo(CTRL_ALT, key).is_err(), "{key}");
        }
    }

    #[test]
    fn pretty_text() {
        assert_eq!(pretty("Ctrl+Alt+A"), "Ctrl + Alt + A");
        assert_eq!(pretty("Win+F2"), "Win + F2");
    }
}
