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
    let (w, h) = size;
    let (left, top) = match at {
        Some((x, y)) => (2 * x < wa.left + wa.right, 2 * y < wa.top + wa.bottom),
        None => (false, false),
    };
    let x = if left { wa.left + EDGE } else { (wa.right - w - EDGE).max(wa.left) };
    let y = if top { wa.top + EDGE } else { (wa.bottom - h - EDGE).max(wa.top) };
    (x, y)
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
}
