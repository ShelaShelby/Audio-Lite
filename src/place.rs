//! Where the window goes on the screen. Pure, no Windows calls.

/// Gap between the window and the screen edge (and the taskbar), in pixels.
pub const EDGE: i32 = 8;

/// A screen rectangle in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Top left corner for a window of `size` in the work area `wa`: in the corner
/// nearest to the tray icon at `at` (bottom right without it), `EDGE` from the
/// edges, and never above or left of the work area.
pub fn place(wa: Area, size: (i32, i32), at: Option<(i32, i32)>) -> (i32, i32) {
    // In i64: no overflow for any input.
    let (w, h) = (i64::from(size.0), i64::from(size.1));
    let (l, t, r, b) = (i64::from(wa.left), i64::from(wa.top), i64::from(wa.right), i64::from(wa.bottom));
    let edge = i64::from(EDGE);
    let (left, top) = match at {
        Some((x, y)) => (2 * i64::from(x) < l + r, 2 * i64::from(y) < t + b),
        None => (false, false),
    };
    let x = if left { l + edge } else { (r - w - edge).max(l) };
    let y = if top { t + edge } else { (b - h - edge).max(t) };
    let fit = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    (fit(x), fit(y))
}

/// The farthest the middle of a tray icon is from the screen edge in a taskbar
/// (a tall taskbar at 200 % or more is still well below this).
const MAX_BAR_HALF: i64 = 160;

/// The work area without the taskbar. Normally Windows leaves the taskbar out
/// of it already, and the tray icon at `at` is outside. A taskbar that hides
/// itself is not left out: then the icon is inside, near one edge, and that
/// edge is moved in by twice the icon's distance from it (the taskbar's
/// height), so the window does not cover the taskbar while it is shown.
pub fn avoid_taskbar(wa: Area, at: Option<(i32, i32)>) -> Area {
    let Some((x, y)) = at else { return wa };
    let (x, y) = (i64::from(x), i64::from(y));
    let (mut l, mut t, mut r, mut b) =
        (i64::from(wa.left), i64::from(wa.top), i64::from(wa.right), i64::from(wa.bottom));
    if !(l <= x && x < r && t <= y && y < b) {
        return wa;
    }
    // Distance to each edge: left, right, top, bottom.
    let sides = [x - l, r - x, y - t, b - y];
    let (side, d) = sides.iter().copied().enumerate().min_by_key(|&(_, d)| d).unwrap_or((3, 0));
    if d > MAX_BAR_HALF {
        return wa;
    }
    let (w, h) = (r - l, b - t);
    match side {
        0 => l += (2 * d).min(w / 2),
        1 => r -= (2 * d).min(w / 2),
        2 => t += (2 * d).min(h / 2),
        _ => b -= (2 * d).min(h / 2),
    }
    let fit = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    Area { left: fit(l), top: fit(t), right: fit(r), bottom: fit(b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WA: Area = Area { left: 0, top: 0, right: 1920, bottom: 1040 };

    #[test]
    fn bottom_right_by_default_and_for_a_bottom_taskbar() {
        assert_eq!(place(WA, (360, 500), None), (1920 - 360 - EDGE, 1040 - 500 - EDGE));
        assert_eq!(place(WA, (360, 500), Some((1800, 1060))), (1552, 532));
    }

    #[test]
    fn follows_a_taskbar_at_the_top_or_left() {
        let top = Area { top: 40, bottom: 1080, ..WA };
        assert_eq!(place(top, (360, 500), Some((1800, 20))), (1552, 40 + EDGE));
        let left = Area { left: 60, ..WA };
        assert_eq!(place(left, (360, 500), Some((30, 1000))), (60 + EDGE, 532));
    }

    #[test]
    fn second_monitor_left_of_the_main_one() {
        let wa = Area { left: -1920, top: 0, right: 0, bottom: 1040 };
        assert_eq!(place(wa, (360, 500), Some((-100, 1060))), (-360 - EDGE, 532));
    }

    #[test]
    fn too_tall_stays_inside_the_top_left() {
        assert_eq!(place(WA, (2000, 2000), None), (0, 0));
    }

    #[test]
    fn extreme_values_never_overflow() {
        let all = [i32::MIN, -1, 0, 1, 1920, i32::MAX];
        for &a in &all {
            for &b in &all {
                let wa = Area { left: a, top: b, right: b, bottom: a };
                for at in [None, Some((a, b)), Some((b, a))] {
                    let _ = place(wa, (a, b), at);
                }
            }
        }
    }

    /// Every taskbar side: the window lands in the corner next to the icon, inside the work area.
    #[test]
    fn every_corner_on_every_monitor() {
        for wa in [
            WA,
            Area { left: -1920, top: -300, right: 0, bottom: 780 },
            Area { left: 0, top: 0, right: 800, bottom: 600 },
        ] {
            let (w, h) = (360, 500);
            for (ax, ay, left, top) in [
                (wa.left, wa.top, true, true),
                (wa.right, wa.bottom, false, false),
                (wa.left, wa.bottom, true, false),
                (wa.right, wa.top, false, true),
            ] {
                let (x, y) = place(wa, (w, h), Some((ax, ay)));
                assert!(x >= wa.left && y >= wa.top);
                assert_eq!(x == wa.left + EDGE, left || (wa.right - w - EDGE) == wa.left + EDGE);
                assert_eq!(y == wa.top + EDGE, top || (wa.bottom - h - EDGE) == wa.top + EDGE);
            }
        }
    }

    #[test]
    fn a_normal_taskbar_is_already_outside_the_work_area() {
        // Bottom taskbar, icon below the work area: nothing changes.
        assert_eq!(avoid_taskbar(WA, Some((1800, 1060))), WA);
        assert_eq!(avoid_taskbar(WA, None), WA);
        // Far inside (not near an edge): not a taskbar.
        assert_eq!(avoid_taskbar(WA, Some((900, 500))), WA);
    }

    #[test]
    fn an_auto_hidden_taskbar_is_left_out_on_every_side() {
        let full = Area { left: 0, top: 0, right: 1920, bottom: 1080 };
        // Icon 24 px from the bottom: a 48 px taskbar.
        let a = avoid_taskbar(full, Some((1800, 1056)));
        assert_eq!(a, Area { bottom: 1032, ..full });
        let (x, y) = place(a, (360, 500), Some((1800, 1056)));
        assert_eq!((x, y), (1920 - 360 - EDGE, 1032 - 500 - EDGE));
        assert_eq!(avoid_taskbar(full, Some((1800, 24))), Area { top: 48, ..full });
        assert_eq!(avoid_taskbar(full, Some((30, 900))), Area { left: 60, ..full });
        assert_eq!(avoid_taskbar(full, Some((1890, 900))), Area { right: 1860, ..full });
        // A second monitor left of the main one.
        let left = Area { left: -1920, top: 0, right: 0, bottom: 1080 };
        assert_eq!(avoid_taskbar(left, Some((-100, 1056))), Area { bottom: 1032, ..left });
    }

    #[test]
    fn avoid_taskbar_never_overflows_or_inverts() {
        let all = [i32::MIN, -1, 0, 1, 100, 1920, i32::MAX];
        for &a in &all {
            for &b in &all {
                let wa = Area { left: a.min(b), top: a.min(b), right: a.max(b), bottom: a.max(b) };
                for at in [None, Some((a, b)), Some((b, a)), Some((a, a)), Some((0, 0))] {
                    let r = avoid_taskbar(wa, at);
                    assert!(r.left <= r.right && r.top <= r.bottom, "{wa:?} {at:?}");
                    assert!(r.left >= wa.left && r.right <= wa.right && r.top >= wa.top && r.bottom <= wa.bottom);
                }
            }
        }
    }
}
