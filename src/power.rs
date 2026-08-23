use core::sync::atomic::{AtomicU32, Ordering};

use crate::power_math::{SCALE_ONE, estimated_ma, scale_channel, target_scale_q16};
use crate::util::{NUM_LEDS, Rgb};

/// Production ceiling. This is deliberately far below the conservative
/// 5.49A estimate for 90 pixels at full white while leaving normal sketches
/// enough headroom to retain their intended contrast.
pub static LED_BUDGET_MA: AtomicU32 = AtomicU32::new(1_500);
pub static LED_REQUESTED_MA: AtomicU32 = AtomicU32::new(0);
pub static LED_OUTPUT_MA: AtomicU32 = AtomicU32::new(0);
pub static LED_SCALE_PERMILLE: AtomicU32 = AtomicU32::new(0);

// At 60fps, a limited scene takes at least ~0.8s to rise to full output.
const SCALE_RISE_PER_FRAME: u32 = SCALE_ONE / 50;

pub struct LedPowerLimiter {
    scale_q16: u32,
}

impl LedPowerLimiter {
    pub const fn new() -> Self {
        Self { scale_q16: 0 }
    }

    pub fn apply(&mut self, leds: &mut [Rgb; NUM_LEDS]) {
        let requested_sum = leds
            .iter()
            .map(|led| led.r as u32 + led.g as u32 + led.b as u32)
            .sum::<u32>();
        let budget_ma = LED_BUDGET_MA.load(Ordering::Relaxed);
        let target = target_scale_q16(requested_sum, NUM_LEDS as u32, budget_ma);
        self.scale_q16 = if target < self.scale_q16 {
            target
        } else {
            self.scale_q16
                .saturating_add(SCALE_RISE_PER_FRAME)
                .min(target)
        };

        for led in leds.iter_mut() {
            led.r = scale_channel(led.r, self.scale_q16);
            led.g = scale_channel(led.g, self.scale_q16);
            led.b = scale_channel(led.b, self.scale_q16);
        }

        let output_sum = leds
            .iter()
            .map(|led| led.r as u32 + led.g as u32 + led.b as u32)
            .sum::<u32>();
        LED_REQUESTED_MA.store(
            estimated_ma(requested_sum, NUM_LEDS as u32),
            Ordering::Relaxed,
        );
        LED_OUTPUT_MA.store(
            estimated_ma(output_sum, NUM_LEDS as u32),
            Ordering::Relaxed,
        );
        LED_SCALE_PERMILLE.store(
            self.scale_q16 * 1_000 / SCALE_ONE,
            Ordering::Relaxed,
        );
    }
}
