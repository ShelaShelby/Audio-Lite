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
    if text == "Alt+F4" {
        return Err("Alt+F4 stays for closing windows.");
    }
    match settings::parse_hotkey(&text) {
        Some(_) => Ok(text),
        None if !(mods.ctrl || mods.alt) && settings::parse_hotkey(&format!("Ctrl+{key}")).is_some() => {
            Err("Shift alone only works with F1 to F24; add Ctrl or Alt.")
        }
        None => Err("Use a letter, a digit or F1 to F24."),
    }
}

/// Why Ctrl+C, Ctrl+X and Ctrl+V cannot be the hotkey.
pub const CLIPBOARD: &str = "Ctrl+C, Ctrl+X and Ctrl+V stay for copy and paste.";

/// `combo` for the key as the layout names it, or else for the key at the same
/// place (`physical`): Shift+1 is "!" on many layouts, Ctrl+Alt+E may be "€".
pub fn combo_of(mods: Mods, key: &str, physical: Option<&str>) -> Result<String, &'static str> {
    combo(mods, key).or_else(|e| match physical {
        // The key at that place says best what is wrong with it.
        Some(p) if p != key => combo(mods, p),
        _ => Err(e),
    })
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
    fn layout_symbols_fall_back_to_the_key_position() {
        let ctrl_shift = Mods { ctrl: true, shift: true, ..Mods::default() };
        assert_eq!(combo_of(ctrl_shift, "!", Some("1")), Ok("Ctrl+Shift+1".to_string()));
        assert_eq!(combo_of(CTRL_ALT, "€", Some("E")), Ok("Ctrl+Alt+E".to_string()));
        assert_eq!(combo_of(CTRL_ALT, "Q", Some("A")), Ok("Ctrl+Alt+Q".to_string()), "the layout wins");
        assert!(combo_of(CTRL_ALT, "Space", Some("Space")).is_err());
        assert!(combo_of(CTRL_ALT, "Space", None).is_err());
        assert!(combo_of(Mods::default(), "!", Some("1")).is_err());
    }

    /// Every key name egui can give, with every set of modifiers: never a panic,
    /// and what is accepted always registers as the same keys.
    #[test]
    fn every_key_and_modifier() {
        let mut names: Vec<String> = ('A'..='Z').chain('0'..='9').map(String::from).collect();
        names.extend((0..=40).map(|n| format!("F{n}")));
        names.extend(
            ["Escape", "Tab", "Num1", "€", "ä", "", " ", "+", "Ctrl", "Alt", "Shift", "Win", "F", "F01", "FF"]
                .map(String::from),
        );
        for bits in 0..8 {
            let m = Mods { ctrl: bits & 1 != 0, alt: bits & 2 != 0, shift: bits & 4 != 0 };
            for n in &names {
                if let Ok(text) = combo(m, n) {
                    let (mods, vk) = settings::parse_hotkey(&text).unwrap();
                    assert_ne!(mods, 0);
                    assert_eq!(mods & settings::MOD_CONTROL != 0, m.ctrl);
                    assert_eq!(mods & settings::MOD_ALT != 0, m.alt);
                    assert_eq!(mods & settings::MOD_SHIFT != 0, m.shift);
                    assert!((0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) || (0x70..=0x87).contains(&vk));
                } else {
                    let shift_only = bits == 4;
                    assert!(
                        bits == 0 || shift_only || !(n.len() == 1 && n.chars().all(|c| c.is_ascii_alphanumeric())),
                        "{n}"
                    );
                }
            }
        }
    }

    /// Shift alone with a letter or digit would take that character away from typing.
    #[test]
    fn shift_alone_needs_an_f_key() {
        let shift = Mods { shift: true, ..Mods::default() };
        assert_eq!(combo(shift, "A"), Err("Shift alone only works with F1 to F24; add Ctrl or Alt."));
        assert_eq!(combo_of(shift, "!", Some("1")), Err("Shift alone only works with F1 to F24; add Ctrl or Alt."));
        assert_eq!(combo(shift, "F12"), Ok("Shift+F12".to_string()));
        assert_eq!(combo(shift, "Space"), Err("Use a letter, a digit or F1 to F24."));
    }

    #[test]
    fn alt_f4_stays_for_closing_windows() {
        let alt = Mods { alt: true, ..Mods::default() };
        assert_eq!(combo(alt, "F4"), Err("Alt+F4 stays for closing windows."));
        assert_eq!(combo(alt, "f4"), Err("Alt+F4 stays for closing windows."));
        assert_eq!(combo(CTRL_ALT, "F4"), Ok("Ctrl+Alt+F4".to_string()));
    }

    #[test]
    fn pretty_text() {
        assert_eq!(pretty("Ctrl+Alt+A"), "Ctrl + Alt + A");
        assert_eq!(pretty("Win+F2"), "Win + F2");
    }
}
