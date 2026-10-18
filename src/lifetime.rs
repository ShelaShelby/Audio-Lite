//! When the window process shows, hides and exits. A hidden window waits
//! `linger` (30 s) for another tray click, then its process exits; nothing runs
//! while it waits. A "warm" process, started while the pointer is over the
//! tray icon, waits `WARM` (5 s) after the pointer was last seen there.
//! Pure, no Windows calls.

use std::time::{Duration, Instant};

/// How long a hidden window waits for the next tray click.
pub const LINGER: Duration = Duration::from_secs(30);
/// A warm process (or a hidden one about to exit) stays at least this long
/// after the pointer was last over the tray icon.
pub const WARM: Duration = Duration::from_secs(5);
/// A tray click that follows a click-away hide must not reopen the window.
pub const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// What the tray asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// A tray click: show if hidden, hide if shown.
    Toggle,
    Show,
    Hide,
}

/// What the window does about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Show,
    Hide,
    Nothing,
}

#[derive(Debug)]
pub struct Lifetime {
    linger: Duration,
    visible: bool,
    /// When it was hidden (None while shown, or before it was shown once).
    hidden_at: Option<Instant>,
    /// When it last hid itself because it lost focus.
    click_away_at: Option<Instant>,
    /// The pointer was over the tray icon: no exit before this.
    warm_until: Option<Instant>,
    exiting: bool,
}

impl Lifetime {
    /// A new window process: not shown yet, and no exit planned until it was
    /// shown and hidden again (or, if it is warm, until `warm` ran out).
    pub fn new(linger: Duration) -> Self {
        Lifetime { linger, visible: false, hidden_at: None, click_away_at: None, warm_until: None, exiting: false }
    }

    /// Shown (or about to be, as soon as its first frame is painted).
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// The pointer is over the tray icon: stay at least `WARM` from `now`.
    pub fn warm(&mut self, now: Instant) {
        if let Some(t) = now.checked_add(WARM) {
            self.warm_until = Some(self.warm_until.map_or(t, |w| w.max(t)));
        }
    }

    pub fn shown(&mut self) {
        self.visible = true;
        self.hidden_at = None;
    }

    /// Hidden (X, Esc, a tray click, or `click_away`: it lost focus).
    pub fn hidden(&mut self, now: Instant, click_away: bool) {
        if self.visible || self.hidden_at.is_none() {
            self.hidden_at = Some(now);
        }
        self.visible = false;
        if click_away {
            self.click_away_at = Some(now);
        }
    }

    /// The answer to the tray. `None`: the process is exiting, so the tray has
    /// to start a new one.
    pub fn ask(&self, what: Ask, now: Instant) -> Option<Do> {
        if self.exiting {
            return None;
        }
        let just_hid = self.click_away_at.is_some_and(|t| now.saturating_duration_since(t) < REOPEN_GUARD);
        Some(match (what, self.visible) {
            (Ask::Hide, true) | (Ask::Toggle, true) => Do::Hide,
            (Ask::Show, false) => Do::Show,
            (Ask::Toggle, false) if !just_hid => Do::Show,
            _ => Do::Nothing,
        })
    }

    /// When the process exits if nothing happens before (None also for a
    /// linger too long for the clock: then it never exits by itself).
    pub fn deadline(&self) -> Option<Instant> {
        if self.exiting || self.visible {
            return None;
        }
        match (self.hidden_at, self.warm_until) {
            // A linger too long for the clock: it never exits by itself.
            (Some(t), w) => t.checked_add(self.linger).map(|d| w.map_or(d, |w| d.max(w))),
            (None, w) => w,
        }
    }

    /// The exit timer fired. True: exit now; every later `ask` gets `None`.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.deadline().is_some_and(|t| t <= now) {
            self.exiting = true;
        }
        self.exiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn no_exit_before_it_was_shown_and_hidden() {
        let mut l = Lifetime::new(LINGER);
        let t0 = Instant::now();
        assert_eq!(l.deadline(), None);
        assert!(!l.expire(t0 + LINGER * 10));
        // A click while it is still opening shows it.
        assert_eq!(l.ask(Ask::Toggle, t0), Some(Do::Show));
        l.shown();
        assert_eq!(l.deadline(), None);
        assert!(!l.expire(t0 + LINGER * 10));
    }

    #[test]
    fn exits_after_linger_once_hidden() {
        let mut l = Lifetime::new(LINGER);
        let t0 = Instant::now();
        l.shown();
        l.hidden(t0, false);
        assert_eq!(l.deadline(), Some(t0 + LINGER));
        assert!(!l.expire(t0 + LINGER - MS));
        assert_eq!(l.ask(Ask::Toggle, t0 + LINGER - MS), Some(Do::Show));
        assert!(l.expire(t0 + LINGER));
        // Exiting: every request is refused, so the tray starts a new process.
        assert_eq!(l.ask(Ask::Toggle, t0 + LINGER), None);
        assert_eq!(l.ask(Ask::Show, t0 + LINGER), None);
        assert_eq!(l.deadline(), None);
    }

    #[test]
    fn showing_again_cancels_the_exit() {
        let mut l = Lifetime::new(LINGER);
        let t0 = Instant::now();
        l.shown();
        l.hidden(t0, false);
        l.shown();
        assert_eq!(l.deadline(), None);
        assert!(!l.expire(t0 + LINGER * 2));
        // Hidden again: the full linger starts again.
        let t1 = t0 + LINGER * 2;
        l.hidden(t1, false);
        assert_eq!(l.deadline(), Some(t1 + LINGER));
        // Hiding a hidden window does not move the deadline.
        l.hidden(t1 + MS * 500, false);
        assert_eq!(l.deadline(), Some(t1 + LINGER));
    }

    #[test]
    fn toggle_and_reopen_guard() {
        let mut l = Lifetime::new(LINGER);
        let t0 = Instant::now();
        l.shown();
        assert_eq!(l.ask(Ask::Toggle, t0), Some(Do::Hide));
        assert_eq!(l.ask(Ask::Show, t0), Some(Do::Nothing));
        // The click on the tray icon took the focus away: the window hid itself,
        // and the same click must not open it again.
        l.hidden(t0, true);
        assert_eq!(l.ask(Ask::Toggle, t0 + 100 * MS), Some(Do::Nothing));
        assert_eq!(l.ask(Ask::Toggle, t0 + REOPEN_GUARD), Some(Do::Show));
        // An explicit show is not a click, and a hide of a hidden window does nothing.
        assert_eq!(l.ask(Ask::Show, t0 + MS), Some(Do::Show));
        assert_eq!(l.ask(Ask::Hide, t0 + MS), Some(Do::Nothing));
        // Hidden by X or Esc: the next click opens it at once.
        l.shown();
        let t1 = t0 + REOPEN_GUARD * 2;
        l.hidden(t1, false);
        assert_eq!(l.ask(Ask::Toggle, t1 + MS), Some(Do::Show));
    }

    #[test]
    fn huge_linger_never_panics() {
        let mut l = Lifetime::new(Duration::MAX);
        let t0 = Instant::now();
        l.shown();
        l.hidden(t0, false);
        assert_eq!(l.deadline(), None);
        assert!(!l.expire(t0 + LINGER));
        assert_eq!(l.ask(Ask::Toggle, t0 + LINGER), Some(Do::Show));
    }

    #[test]
    fn warm_process_exits_after_warm_unless_shown() {
        let mut l = Lifetime::new(LINGER);
        let t0 = Instant::now();
        l.warm(t0);
        assert!(!l.visible());
        assert_eq!(l.deadline(), Some(t0 + WARM));
        // The pointer is still there a second later: 5 s from then.
        l.warm(t0 + 1000 * MS);
        assert_eq!(l.deadline(), Some(t0 + 1000 * MS + WARM));
        // An older report never shortens it.
        l.warm(t0);
        assert_eq!(l.deadline(), Some(t0 + 1000 * MS + WARM));
        assert!(!l.expire(t0 + WARM));
        assert!(l.expire(t0 + 1000 * MS + WARM));
        assert_eq!(l.ask(Ask::Toggle, t0 + 10 * WARM), None);
    }

    #[test]
    fn a_click_on_a_warm_process_shows_it_and_then_linger_counts() {
        let mut l = Lifetime::new(LINGER);
        let t0 = Instant::now();
        l.warm(t0);
        assert_eq!(l.ask(Ask::Toggle, t0 + MS), Some(Do::Show));
        l.shown();
        assert_eq!(l.deadline(), None);
        assert!(!l.expire(t0 + WARM * 2));
        let t1 = t0 + WARM * 3;
        l.hidden(t1, false);
        assert_eq!(l.deadline(), Some(t1 + LINGER));
        // The pointer over the icon near the end of the linger: at least 5 s more.
        let t2 = t1 + LINGER - MS;
        l.warm(t2);
        assert_eq!(l.deadline(), Some(t2 + WARM));
        assert!(!l.expire(t1 + LINGER));
    }

    #[test]
    fn a_cold_process_is_visible_from_the_start() {
        let mut l = Lifetime::new(LINGER);
        let t0 = Instant::now();
        l.shown();
        assert!(l.visible());
        // Hover reports while it opens never end it.
        l.warm(t0);
        assert_eq!(l.deadline(), None);
        assert!(!l.expire(t0 + WARM * 10));
    }

    /// Random asks and events: the answers always fit the state, and once it
    /// is exiting it stays exiting.
    #[test]
    fn random_sequences_keep_the_rules() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..500 {
            let mut l = Lifetime::new(Duration::from_millis(1000));
            let t0 = Instant::now();
            let mut now = t0;
            let mut shown = false;
            let mut exiting = false;
            for _ in 0..200 {
                now += Duration::from_millis(next() % 700);
                match next() % 7 {
                    6 => l.warm(now),
                    0 => {
                        l.shown();
                        shown = true;
                    }
                    1 | 2 => {
                        l.hidden(now, next() % 2 == 0);
                        shown = false;
                    }
                    3 => {
                        if l.expire(now) {
                            exiting = true;
                        }
                    }
                    _ => {
                        let ask = [Ask::Toggle, Ask::Show, Ask::Hide][(next() % 3) as usize];
                        match l.ask(ask, now) {
                            None => assert!(exiting),
                            Some(d) => {
                                assert!(!exiting);
                                if shown {
                                    assert_ne!(d, Do::Show);
                                } else {
                                    assert_ne!(d, Do::Hide);
                                }
                                if ask == Ask::Hide {
                                    assert_ne!(d, Do::Show);
                                }
                            }
                        }
                    }
                }
                if exiting {
                    assert_eq!(l.deadline(), None);
                    assert!(l.expire(now));
                }
                if shown {
                    assert_eq!(l.deadline(), None);
                }
            }
        }
    }
}
