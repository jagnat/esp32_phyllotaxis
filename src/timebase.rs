use core::f32::consts::TAU;

pub const FRAME_PERIOD_US: u64 = 16_667;
pub const MAX_DT_US: u64 = 50_000;

/// Elapsed milliseconds for wrapping 32-bit telemetry clocks. This remains
/// correct across the roughly 49.7-day wrap as long as intervals stay below
/// half the counter range.
pub const fn elapsed_ms32(now: u32, then: u32) -> u32 {
    now.wrapping_sub(then)
}

pub fn clamp_dt_us(actual_us: u64) -> u32 {
    actual_us.min(MAX_DT_US) as u32
}

pub fn dt_seconds(dt_us: u32) -> f32 {
    dt_us as f32 / 1_000_000.0
}

/// Produce a bounded phase from an arbitrarily large integer timestamp.
pub fn phase_radians(elapsed_us: u64, period_us: u64) -> f32 {
    (elapsed_us % period_us) as f32 / period_us as f32 * TAU
}

pub fn phase_degrees(elapsed_us: u64, period_us: u64) -> f32 {
    (elapsed_us % period_us) as f32 / period_us as f32 * 360.0
}

pub fn wrap_radians(value: f32) -> f32 {
    let value = value % TAU;
    if value < 0.0 { value + TAU } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_still_advances_after_a_year() {
        let year_us = 365u64 * 24 * 60 * 60 * 1_000_000;
        let a = phase_radians(year_us, 20_000_000);
        let b = phase_radians(year_us + FRAME_PERIOD_US, 20_000_000);
        let advance = wrap_radians(b - a);
        assert!(a.is_finite() && b.is_finite());
        assert!(advance > 0.005 && advance < 0.006);
    }

    #[test]
    fn phase_is_bounded_near_u64_limit() {
        let phase = phase_radians(u64::MAX - 1, 4_188_790);
        assert!(phase.is_finite());
        assert!((0.0..TAU).contains(&phase));
    }

    #[test]
    fn delta_is_clamped_after_a_stall() {
        assert_eq!(clamp_dt_us(16_667), 16_667);
        assert_eq!(clamp_dt_us(5_000_000), 50_000);
    }

    #[test]
    fn millisecond_clock_handles_startup_and_wrap() {
        assert_eq!(elapsed_ms32(0, 0), 0);
        assert_eq!(elapsed_ms32(10, u32::MAX - 4), 15);
    }

    #[test]
    fn wrapped_accumulators_stay_bounded() {
        for value in [-1_000_000.0, -TAU, -0.1, 0.0, TAU, 1_000_000.0] {
            let phase = wrap_radians(value);
            assert!(phase.is_finite());
            assert!((0.0..TAU).contains(&phase));
        }
    }
}
