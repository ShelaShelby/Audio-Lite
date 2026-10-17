//! Volume writes, latest wins. The window only stores the newest wanted value
//! per slider; the audio thread takes them out at most once per `gap` (about 60
//! times a second). A value marked final (the slider was let go, a click on the
//! track, an arrow key) is written at once. Pure, no Windows calls.

use std::time::{Duration, Instant};

/// About 60 writes a second.
pub const GAP: Duration = Duration::from_millis(16);

pub struct Coalescer<K> {
    pending: Vec<(K, f32)>,
    urgent: bool,
    last: Option<Instant>,
    gap: Duration,
}

impl<K: PartialEq> Coalescer<K> {
    pub fn new(gap: Duration) -> Self {
        Coalescer { pending: Vec::new(), urgent: false, last: None, gap }
    }

    /// Remember `value` for `key`, replacing a value that was not written yet.
    pub fn set(&mut self, key: K, value: f32) {
        match self.pending.iter_mut().find(|(k, _)| *k == key) {
            Some(p) => p.1 = value,
            None => self.pending.push((key, value)),
        }
    }

    /// Like `set`, and the next `take` writes it without waiting for the gap.
    pub fn set_final(&mut self, key: K, value: f32) {
        self.set(key, value);
        self.urgent = true;
    }

    /// The values not written yet.
    pub fn pending(&self) -> &[(K, f32)] {
        &self.pending
    }

    /// When `take` will return something: `None` if nothing waits.
    pub fn due(&self, now: Instant) -> Option<Instant> {
        if self.pending.is_empty() {
            return None;
        }
        match self.last {
            Some(t) if !self.urgent => Some((t + self.gap).max(now)),
            _ => Some(now),
        }
    }

    /// The values to write now (empty if nothing waits or the gap is not over).
    pub fn take(&mut self, now: Instant) -> Vec<(K, f32)> {
        if self.due(now).is_none_or(|t| t > now) {
            return Vec::new();
        }
        self.last = Some(now);
        self.urgent = false;
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn nothing_waiting() {
        let mut c: Coalescer<u8> = Coalescer::new(GAP);
        let now = Instant::now();
        assert_eq!(c.due(now), None);
        assert!(c.take(now).is_empty());
    }

    #[test]
    fn first_value_is_written_at_once() {
        let mut c = Coalescer::new(GAP);
        let now = Instant::now();
        c.set(1, 0.5);
        assert_eq!(c.due(now), Some(now));
        assert_eq!(c.take(now), vec![(1, 0.5)]);
        assert!(c.pending().is_empty());
    }

    #[test]
    fn latest_value_wins() {
        let mut c = Coalescer::new(GAP);
        let t0 = Instant::now();
        c.set(1, 0.1);
        c.take(t0);
        for v in [0.2, 0.3, 0.4] {
            c.set(1, v);
        }
        c.set(2, 0.9);
        // Inside the gap: nothing yet, then only the newest value per key.
        assert!(c.take(t0 + 5 * MS).is_empty());
        assert_eq!(c.due(t0 + 5 * MS), Some(t0 + GAP));
        assert_eq!(c.take(t0 + GAP), vec![(1, 0.4), (2, 0.9)]);
    }

    #[test]
    fn at_most_one_write_per_gap() {
        let mut c = Coalescer::new(GAP);
        let t0 = Instant::now();
        let mut writes = 0;
        // A drag that sends a new value every millisecond for one second.
        for ms in 0..1000u32 {
            let now = t0 + ms * MS;
            c.set(0, ms as f32 / 1000.0);
            writes += c.take(now).len();
        }
        assert!((60..=63).contains(&writes), "{writes} writes");
        // The newest value is never lost.
        let last = t0 + 1000 * MS + GAP;
        assert_eq!(c.take(last), vec![(0, 0.999)]);
    }

    #[test]
    fn final_value_skips_the_gap() {
        let mut c = Coalescer::new(GAP);
        let t0 = Instant::now();
        c.set(1, 0.2);
        c.take(t0);
        c.set(1, 0.3);
        c.set_final(1, 0.35);
        assert_eq!(c.due(t0 + MS), Some(t0 + MS));
        assert_eq!(c.take(t0 + MS), vec![(1, 0.35)]);
        // Back to the normal pace afterwards.
        c.set(1, 0.4);
        assert!(c.take(t0 + 2 * MS).is_empty());
    }

    /// Random drags on three sliders: writes never come closer than the gap
    /// unless a value is final, and the newest value of each slider is always
    /// the last one written.
    #[test]
    fn random_drags_keep_the_pace_and_lose_nothing() {
        let mut seed = 0xdead_beef_cafe_f00du64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..200 {
            let mut c = Coalescer::new(GAP);
            let t0 = Instant::now();
            let mut now = t0;
            let mut want = [None::<f32>; 3];
            let mut written = [None::<f32>; 3];
            let mut last_take: Option<Instant> = None;
            let mut urgent = false;
            for _ in 0..500 {
                now += Duration::from_micros(next(8000));
                let k = next(3) as usize;
                let v = next(1001) as f32 / 1000.0;
                if next(10) == 0 {
                    c.set_final(k, v);
                    urgent = true;
                } else {
                    c.set(k, v);
                }
                want[k] = Some(v);
                let due = c.due(now).unwrap();
                assert!(due >= now);
                let out = c.take(now);
                if !out.is_empty() {
                    if let (Some(t), false) = (last_take, urgent) {
                        assert!(now - t >= GAP);
                    }
                    last_take = Some(now);
                    urgent = false;
                    for (k, v) in out {
                        written[k] = Some(v);
                    }
                }
            }
            // After the gap everything that waits is written.
            let out = c.take(now + GAP);
            for (k, v) in out {
                written[k] = Some(v);
            }
            assert_eq!(written, want);
            assert_eq!(c.due(now + GAP), None);
        }
    }
}
