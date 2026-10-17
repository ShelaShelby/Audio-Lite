//! Where the mixer popup goes and how a click maps to a volume. Pure.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Top-left corner for a `w` x `h` popup opened from a tray icon at `anchor`
/// (usually the mouse position, on the taskbar). The popup sits next to the
/// taskbar, whichever edge it is on, `gap` pixels inside the work area, and is
/// always kept inside the work area. If the work area is too small, the popup's
/// top-left corner stays visible.
pub fn popup_position(work: Rect, w: i32, h: i32, anchor: (i32, i32), gap: i32) -> (i32, i32) {
    let (ax, ay) = anchor;
    let x = if ax >= work.right {
        work.right - gap - w // taskbar on the right
    } else if ax < work.left {
        work.left + gap // taskbar on the left
    } else {
        ax - w / 2
    };
    let y = if ay >= work.bottom {
        work.bottom - gap - h // taskbar at the bottom
    } else if ay < work.top {
        work.top + gap // taskbar at the top
    } else {
        ay - h / 2
    };
    let clamp = |v: i32, lo: i32, hi: i32| v.min(hi).max(lo);
    (clamp(x, work.left, work.right - w), clamp(y, work.top, work.bottom - h))
}

/// The volume (0-100) for a click at `x` on a slider track from `left` to `right`.
pub fn percent_at(x: i32, left: i32, right: i32) -> u32 {
    if right <= left {
        return 0;
    }
    let p = ((x - left) as f32 * 100.0 / (right - left) as f32).round();
    p.clamp(0.0, 100.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };

    #[test]
    fn taskbar_at_bottom() {
        let work = Rect { bottom: 1032, ..SCREEN };
        assert_eq!(popup_position(work, 300, 200, (1700, 1050), 8), (1550, 824));
        // Tray icon near the right corner: pushed back inside.
        assert_eq!(popup_position(work, 300, 200, (1910, 1050), 8), (1620, 824));
    }

    #[test]
    fn taskbar_at_top() {
        let work = Rect { top: 48, ..SCREEN };
        assert_eq!(popup_position(work, 300, 200, (1800, 20), 8), (1620, 56));
    }

    #[test]
    fn taskbar_on_the_left() {
        let work = Rect { left: 60, ..SCREEN };
        assert_eq!(popup_position(work, 300, 200, (30, 1000), 8), (68, 880));
        assert_eq!(popup_position(work, 300, 200, (30, 500), 8), (68, 400));
    }

    #[test]
    fn taskbar_on_the_right() {
        let work = Rect { right: 1860, ..SCREEN };
        assert_eq!(popup_position(work, 300, 200, (1890, 1060), 8), (1552, 880));
        assert_eq!(popup_position(work, 300, 200, (1890, 10), 8), (1552, 0));
    }

    #[test]
    fn second_monitor_with_negative_coordinates() {
        let work = Rect { left: -1280, top: 0, right: 0, bottom: 984 };
        assert_eq!(popup_position(work, 300, 200, (-50, 1000), 8), (-300, 776));
    }

    #[test]
    fn tiny_work_area_keeps_top_left_visible() {
        let work = Rect { left: 100, top: 100, right: 300, bottom: 200 };
        assert_eq!(popup_position(work, 300, 200, (290, 210), 8), (100, 100));
        assert_eq!(popup_position(work, 300, 200, (0, 0), 8), (100, 100));
    }

    #[test]
    fn always_inside_when_it_fits() {
        let work = Rect { bottom: 1032, ..SCREEN };
        for ax in [-100, 0, 500, 1919, 3000] {
            for ay in [-100, 0, 500, 1031, 1060, 3000] {
                let (x, y) = popup_position(work, 320, 400, (ax, ay), 8);
                assert!(x >= work.left && x + 320 <= work.right, "x {x} for {ax},{ay}");
                assert!(y >= work.top && y + 400 <= work.bottom, "y {y} for {ax},{ay}");
            }
        }
    }

    #[test]
    fn slider_clicks() {
        assert_eq!(percent_at(100, 100, 300), 0);
        assert_eq!(percent_at(200, 100, 300), 50);
        assert_eq!(percent_at(300, 100, 300), 100);
        assert_eq!(percent_at(50, 100, 300), 0);
        assert_eq!(percent_at(999, 100, 300), 100);
        assert_eq!(percent_at(5, 10, 10), 0);
    }

    #[test]
    fn rect_helpers() {
        let r = Rect { left: 10, top: 20, right: 30, bottom: 50 };
        assert!(r.contains(10, 20) && !r.contains(30, 20) && !r.contains(10, 50));
    }
}
