//! Volume as the user sees it (0-100) and as Windows stores it (0.0-1.0).

/// 0.0-1.0 to 0-100, rounded to the nearest step. Out of range or NaN is clamped (NaN gives 0).
pub fn to_percent(scalar: f32) -> u32 {
    if scalar.is_nan() {
        return 0;
    }
    (scalar.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// A volume Windows accepts (0.0-1.0), or None for NaN (Windows refuses it).
pub fn to_scalar(v: f32) -> Option<f32> {
    (!v.is_nan()).then(|| v.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_to_percent_rounds_and_clamps() {
        assert_eq!(to_percent(0.0), 0);
        assert_eq!(to_percent(1.0), 100);
        assert_eq!(to_percent(0.494), 49);
        assert_eq!(to_percent(0.495), 50);
        assert_eq!(to_percent(0.999), 100);
        assert_eq!(to_percent(-0.3), 0);
        assert_eq!(to_percent(1.7), 100);
        assert_eq!(to_percent(f32::NAN), 0);
        assert_eq!(to_percent(f32::INFINITY), 100);
    }

    #[test]
    fn round_trip_every_step() {
        for p in 0..=100 {
            assert_eq!(to_percent(p as f32 / 100.0), p);
        }
    }

    /// Every percent, and values just around it, as Windows may store them
    /// (it keeps volumes as floats that are rarely exact).
    #[test]
    fn every_step_and_its_neighbours() {
        for p in 0..=100u32 {
            let exact = p as f32 / 100.0;
            for d in [-0.0049f32, -0.001, -f32::EPSILON, 0.0, f32::EPSILON, 0.001, 0.0049] {
                let v = exact + d;
                let want = if v <= 0.0 {
                    0
                } else if v >= 1.0 {
                    100
                } else {
                    p
                };
                assert_eq!(to_percent(v), want, "{v}");
                let s = to_scalar(v).unwrap();
                assert!((0.0..=1.0).contains(&s));
                assert_eq!(to_percent(s), want);
            }
            // The slider gives 0-100; the window turns it into a scalar and back.
            assert_eq!(to_percent(to_scalar(p as f32 / 100.0).unwrap()), p);
        }
    }

    #[test]
    fn scalars_windows_refuses() {
        assert_eq!(to_scalar(f32::NAN), None);
        assert_eq!(to_scalar(-1.0), Some(0.0));
        assert_eq!(to_scalar(f32::INFINITY), Some(1.0));
        assert_eq!(to_scalar(f32::NEG_INFINITY), Some(0.0));
        assert_eq!(to_scalar(0.0), Some(0.0));
        assert_eq!(to_scalar(1.0), Some(1.0));
    }
}
