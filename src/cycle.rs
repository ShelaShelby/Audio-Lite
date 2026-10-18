//! Which device comes next when the hotkey is pressed. Pure, no Windows calls.

/// One active playback device.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Device {
    /// Windows endpoint id (stable, used to set the default).
    pub id: String,
    /// Friendly name, as shown in Windows Sound settings.
    pub name: String,
}

/// True if `name` is in the exclude list (case-insensitive, surrounding spaces ignored).
pub fn is_excluded(name: &str, excluded: &[String]) -> bool {
    let name = name.trim().to_lowercase();
    excluded.iter().any(|e| e.trim().to_lowercase() == name)
}

/// Tick or untick "In hotkey cycle" for the device `name`. Unticked devices are
/// kept as names in the exclude list of settings.json. Returns true if the list changed.
pub fn set_in_cycle(excluded: &mut Vec<String>, name: &str, on: bool) -> bool {
    let before = excluded.len();
    if on {
        excluded.retain(|e| !is_excluded(name, std::slice::from_ref(e)));
    } else if !is_excluded(name, excluded) {
        excluded.push(name.trim().to_string());
    }
    excluded.len() != before
}

/// The devices the hotkey goes through, in order (the ones ticked "In hotkey cycle").
pub fn cycle_list<'a>(devices: &'a [Device], excluded: &[String]) -> Vec<&'a Device> {
    devices.iter().filter(|d| !is_excluded(&d.name, excluded)).collect()
}

/// Index of the device to switch to, or `None` if there is nothing to switch to.
///
/// - Starts after the current device and wraps around.
/// - Skips excluded devices. The current device may itself be excluded; it is
///   still used as the starting point.
/// - If the current device is unknown or not in the list, the first device
///   that is not excluded is chosen.
pub fn next_index(devices: &[Device], current_id: Option<&str>, excluded: &[String]) -> Option<usize> {
    let ok = |i: &usize| !is_excluded(&devices[*i].name, excluded);
    match current_id.and_then(|id| devices.iter().position(|d| d.id == id)) {
        Some(cur) => (1..devices.len()).map(|step| (cur + step) % devices.len()).find(ok),
        None => (0..devices.len()).find(ok),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devs(names: &[&str]) -> Vec<Device> {
        names.iter().map(|n| Device { id: format!("id-{n}"), name: n.to_string() }).collect()
    }

    fn ex(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn goes_to_the_next_device() {
        let d = devs(&["Speakers", "Headphones", "TV"]);
        assert_eq!(next_index(&d, Some("id-Speakers"), &[]), Some(1));
        assert_eq!(next_index(&d, Some("id-Headphones"), &[]), Some(2));
    }

    #[test]
    fn wraps_around() {
        let d = devs(&["Speakers", "Headphones", "TV"]);
        assert_eq!(next_index(&d, Some("id-TV"), &[]), Some(0));
    }

    #[test]
    fn one_device_means_nothing_to_do() {
        let d = devs(&["Speakers"]);
        assert_eq!(next_index(&d, Some("id-Speakers"), &[]), None);
    }

    #[test]
    fn one_device_that_is_not_the_default_is_chosen() {
        let d = devs(&["Speakers"]);
        assert_eq!(next_index(&d, None, &[]), Some(0));
    }

    #[test]
    fn zero_devices() {
        assert_eq!(next_index(&[], Some("id-x"), &[]), None);
        assert_eq!(next_index(&[], None, &[]), None);
    }

    #[test]
    fn skips_excluded_devices() {
        let d = devs(&["Speakers", "Headphones", "TV", "Monitor"]);
        let e = ex(&["headphones", " TV "]);
        assert_eq!(next_index(&d, Some("id-Speakers"), &e), Some(3));
        assert_eq!(next_index(&d, Some("id-Monitor"), &e), Some(0));
    }

    #[test]
    fn current_excluded_still_moves_on() {
        let d = devs(&["Speakers", "Headphones", "TV"]);
        assert_eq!(next_index(&d, Some("id-Headphones"), &ex(&["Headphones"])), Some(2));
    }

    #[test]
    fn all_others_excluded_means_nothing_to_do() {
        let d = devs(&["Speakers", "Headphones"]);
        assert_eq!(next_index(&d, Some("id-Speakers"), &ex(&["Headphones"])), None);
        assert_eq!(next_index(&d, None, &ex(&["Speakers", "Headphones"])), None);
    }

    #[test]
    fn current_missing_from_list_picks_first_allowed() {
        let d = devs(&["Speakers", "Headphones"]);
        assert_eq!(next_index(&d, Some("id-Gone"), &[]), Some(0));
        assert_eq!(next_index(&d, Some("id-Gone"), &ex(&["Speakers"])), Some(1));
        assert_eq!(next_index(&d, None, &[]), Some(0));
    }

    #[test]
    fn cycle_list_follows_the_checkboxes() {
        let d = devs(&["Speakers", "Headphones", "TV"]);
        let mut e = Vec::new();
        let names = |e: &[String]| cycle_list(&d, e).iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&e), vec!["Speakers", "Headphones", "TV"]);

        assert!(set_in_cycle(&mut e, "TV", false));
        assert!(!set_in_cycle(&mut e, "tv", false), "already out");
        assert_eq!(e, vec!["TV"]);
        assert_eq!(names(&e), vec!["Speakers", "Headphones"]);
        assert_eq!(next_index(&d, Some("id-Headphones"), &e), Some(0));

        assert!(set_in_cycle(&mut e, "Headphones", false));
        assert_eq!(names(&e), vec!["Speakers"]);
        assert_eq!(next_index(&d, Some("id-Speakers"), &e), None);

        // Ticking again removes every spelling of the name.
        e.push(" tv ".into());
        assert!(set_in_cycle(&mut e, "TV", true));
        assert_eq!(e, vec!["Headphones"]);
        assert!(!set_in_cycle(&mut e, "TV", true));
        assert_eq!(names(&e), vec!["Speakers", "TV"]);
    }

    #[test]
    fn exclude_ignores_case_including_non_ascii() {
        assert!(is_excluded("Lautsprecher ÄB", &ex(&["lautsprecher äb"])));
        assert!(!is_excluded("Speakers", &ex(&["Speaker"])));
    }

    /// Every combination of 0 to 4 devices, ticks and current device (also one
    /// that is gone): the answer is ticked, never the current one, the next
    /// one in order, and None only if there is nothing else to go to.
    #[test]
    fn every_combination_of_up_to_four_devices() {
        let all = ["Speakers", "Headphones", "TV", "Monitor"];
        for n in 0..=all.len() {
            let d = devs(&all[..n]);
            for ticks in 0..(1u32 << n) {
                let e: Vec<String> = (0..n).filter(|i| ticks & (1 << i) == 0).map(|i| all[i].to_string()).collect();
                let ticked = |i: usize| ticks & (1 << i) != 0;
                let currents: Vec<Option<String>> = std::iter::once(None)
                    .chain((0..n).map(|i| Some(format!("id-{}", all[i]))))
                    .chain([Some("id-Gone".into())])
                    .collect();
                for cur in &currents {
                    let got = next_index(&d, cur.as_deref(), &e);
                    let cur_i = cur.as_deref().and_then(|c| d.iter().position(|x| x.id == c));
                    let want = match cur_i {
                        Some(c) => (1..n).map(|s| (c + s) % n).find(|&i| ticked(i)),
                        None => (0..n).find(|&i| ticked(i)),
                    };
                    assert_eq!(got, want, "n={n} ticks={ticks:b} cur={cur:?}");
                    if let Some(i) = got {
                        assert!(ticked(i));
                        assert_ne!(Some(i), cur_i);
                    }
                }
                // Pressing the hotkey again and again visits every ticked device in order.
                let count = (0..n).filter(|&i| ticked(i)).count();
                if count >= 2 {
                    let mut cur = next_index(&d, None, &e).unwrap();
                    let mut seen = vec![cur];
                    for _ in 1..count {
                        cur = next_index(&d, Some(&d[cur].id), &e).unwrap();
                        seen.push(cur);
                    }
                    let mut sorted = seen.clone();
                    sorted.sort_unstable();
                    sorted.dedup();
                    assert_eq!(sorted.len(), count);
                    assert_eq!(next_index(&d, Some(&d[cur].id), &e), Some(seen[0]));
                }
            }
        }
    }

    #[test]
    fn unusual_names() {
        // Two devices with the same name: unticking one unticks both (names are
        // what settings.json keeps), and the hotkey still never picks the current one.
        let d = vec![
            Device { id: "a".into(), name: "Speakers".into() },
            Device { id: "b".into(), name: "Speakers".into() },
            Device { id: "c".into(), name: "".into() },
        ];
        assert_eq!(next_index(&d, Some("a"), &[]), Some(1));
        assert_eq!(next_index(&d, Some("c"), &ex(&["speakers"])), None);
        let mut e = Vec::new();
        assert!(set_in_cycle(&mut e, &"x".repeat(5000), false));
        assert!(is_excluded(&"X".repeat(5000), &e));
        // An empty name in the exclude list does not exclude everything.
        assert!(!is_excluded("TV", &ex(&[""])));
    }
}
