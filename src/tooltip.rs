//! The tray tooltip text. Windows keeps at most 127 characters (UTF-16 units)
//! plus the end mark; tray-icon copies 128 units without one, so a longer text
//! would show garbage or nothing. Pure, no Windows calls.

/// The most UTF-16 units Windows shows in a tray tooltip.
pub const MAX_UNITS: usize = 127;
const ELLIPSIS: char = '…';

/// `s` cut to at most `max` UTF-16 units, never inside a character, with "…"
/// at the end if anything was cut.
pub fn cut(s: &str, max: usize) -> String {
    if s.encode_utf16().count() <= max {
        return s.to_string();
    }
    let room = max.saturating_sub(ELLIPSIS.len_utf16());
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        if used + c.len_utf16() > room {
            break;
        }
        used += c.len_utf16();
        out.push(c);
    }
    if max >= ELLIPSIS.len_utf16() {
        out.push(ELLIPSIS);
    }
    out
}

/// The first line (the device), then the problems, each on its own line, in at
/// most `MAX_UNITS` units. A long device name is cut first, so the problems
/// still show.
pub fn text(first: &str, rest: &[&str]) -> String {
    let rest = rest.iter().map(|l| l.replace(['\r', '\n'], " ")).collect::<Vec<_>>();
    let first = first.replace(['\r', '\n'], " ");
    let rest_units: usize = rest.iter().map(|l| l.encode_utf16().count() + 1).sum();
    // The device keeps at least 32 units; the problems get the rest.
    let first = cut(&first, MAX_UNITS.saturating_sub(rest_units).max(32));
    let mut tip = first;
    for l in &rest {
        tip.push('\n');
        tip.push_str(l);
    }
    cut(&tip, MAX_UNITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> usize {
        s.encode_utf16().count()
    }

    #[test]
    fn short_text_is_unchanged() {
        assert_eq!(text("Audio Lite: Speakers", &[]), "Audio Lite: Speakers");
        assert_eq!(text("A", &["B", "C"]), "A\nB\nC");
    }

    #[test]
    fn long_device_names_are_cut_and_the_problem_stays() {
        let name = format!("Audio Lite: {}", "Very long headset name ".repeat(20));
        let problem = "Ctrl+Alt+A is already used by another program.";
        let t = text(&name, &[problem]);
        assert!(units(&t) <= MAX_UNITS);
        assert!(t.ends_with(problem));
        assert!(t.starts_with("Audio Lite: Very long"));
        assert!(t.contains('…'));
    }

    #[test]
    fn never_too_long_and_never_splits_a_character() {
        let pieces = ["", "a", "Ä", "😀", "語", "\n", "Lautsprecher (Realtek(R) Audio)", "e\u{301}"];
        for n in 0..200 {
            for p in pieces {
                let s = p.repeat(n);
                for max in [0, 1, 2, 3, 64, MAX_UNITS] {
                    let c = cut(&s, max);
                    assert!(units(&c) <= max, "{n} {p:?} {max}");
                    assert!(s.starts_with(c.trim_end_matches('…')) || p == "…");
                }
                let t = text(&s, &[&s, "Problem"]);
                assert!(units(&t) <= MAX_UNITS);
                // The device line has no line breaks of its own.
                assert!(!t.split('\n').next().unwrap_or("").contains('\r'));
            }
        }
    }

    #[test]
    fn line_breaks_in_names_stay_on_one_line() {
        assert_eq!(text("Audio Lite: TV\nRoom", &["x\r\ny"]), "Audio Lite: TV Room\nx  y");
    }
}
