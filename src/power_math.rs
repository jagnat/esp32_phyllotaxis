pub const SCALE_ONE: u32 = 1 << 16;
pub const PIXEL_IDLE_MA: u32 = 1;
pub const CHANNEL_FULL_MA: u32 = 20;

pub fn estimated_ma(channel_sum: u32, led_count: u32) -> u32 {
    led_count * PIXEL_IDLE_MA + channel_sum * CHANNEL_FULL_MA / 255
}

pub fn target_scale_q16(channel_sum: u32, led_count: u32, budget_ma: u32) -> u32 {
    let idle_ma = led_count * PIXEL_IDLE_MA;
    if channel_sum == 0 {
        return SCALE_ONE;
    }
    if budget_ma <= idle_ma {
        return 0;
    }
    let available_channel_ma = budget_ma - idle_ma;
    let scale = (available_channel_ma as u64 * 255 * SCALE_ONE as u64)
        / (channel_sum as u64 * CHANNEL_FULL_MA as u64);
    scale.min(SCALE_ONE as u64) as u32
}

pub fn scale_channel(channel: u8, scale_q16: u32) -> u8 {
    ((channel as u32 * scale_q16) >> 16).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_white_matches_conservative_ws2812_model() {
        assert_eq!(estimated_ma(90 * 3 * 255, 90), 5_490);
    }

    #[test]
    fn limiter_respects_budget() {
        let sum = 90 * 3 * 255;
        for budget in [250, 500, 1_000, 1_500] {
            let scale = target_scale_q16(sum, 90, budget);
            let scaled_sum = 90 * 3 * scale_channel(255, scale) as u32;
            assert!(estimated_ma(scaled_sum, 90) <= budget + 2);
        }
    }

    #[test]
    fn dim_frames_are_not_scaled_down() {
        assert_eq!(target_scale_q16(100, 90, 500), SCALE_ONE);
    }
}
