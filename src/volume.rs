//! Volume as the user sees it (0-100) and as Windows stores it (0.0-1.0).

/// 0-100 to 0.0-1.0. Values outside 0-100 are clamped.
pub fn to_scalar(percent: i32) -> f32 {
    percent.clamp(0, 100) as f32 / 100.0
}

/// 0.0-1.0 to 0-100, rounded to the nearest step. Out of range or NaN is clamped (NaN gives 0).
pub fn to_percent(scalar: f32) -> u32 {
    if scalar.is_nan() {
        return 0;
    }
    (scalar.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// `percent` moved by `delta` steps, kept inside 0-100.
pub fn step(percent: u32, delta: i32) -> u32 {
    (percent as i32 + delta).clamp(0, 100) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_to_scalar() {
        assert_eq!(to_scalar(0), 0.0);
        assert_eq!(to_scalar(50), 0.5);
        assert_eq!(to_scalar(100), 1.0);
        assert_eq!(to_scalar(-5), 0.0);
        assert_eq!(to_scalar(250), 1.0);
    }

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
            assert_eq!(to_percent(to_scalar(p)), p as u32);
        }
    }

    #[test]
    fn steps_stay_in_range() {
        assert_eq!(step(50, 2), 52);
        assert_eq!(step(99, 2), 100);
        assert_eq!(step(1, -2), 0);
        assert_eq!(step(100, 4), 100);
    }
}
