//! Which device comes next when the hotkey is pressed. Pure, no Windows calls.

/// One active playback device.
#[derive(Clone, Debug, PartialEq, Eq)]
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
}
