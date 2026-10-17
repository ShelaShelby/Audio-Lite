//! When the window process shows, hides and exits. A hidden window waits
//! `linger` (30 s) for another tray click, then its process exits; nothing runs
//! while it waits. Pure, no Windows calls.

use std::time::{Duration, Instant};

/// How long a hidden window waits for the next tray click.
pub const LINGER: Duration = Duration::from_secs(30);
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
    exiting: bool,
}

impl Lifetime {
    /// A new window process: not shown yet, and no exit planned until it was
    /// shown and hidden again.
    pub fn new(linger: Duration) -> Self {
        Lifetime { linger, visible: false, hidden_at: None, click_away_at: None, exiting: false }
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

    /// When the process exits if nothing happens before.
    pub fn deadline(&self) -> Option<Instant> {
        match (self.exiting, self.visible, self.hidden_at) {
            (false, false, Some(t)) => Some(t + self.linger),
            _ => None,
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
}
