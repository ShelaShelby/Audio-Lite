//! Volume as the user sees it (0-100) and as Windows stores it (0.0-1.0).

/// 0.0-1.0 to 0-100, rounded to the nearest step. Out of range or NaN is clamped (NaN gives 0).
pub fn to_percent(scalar: f32) -> u32 {
    if scalar.is_nan() {
        return 0;
    }
    (scalar.clamp(0.0, 1.0) * 100.0).round() as u32
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
}
