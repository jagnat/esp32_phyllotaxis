use crate::util::{NUM_LEDS, Rgb};

// Applied after gamma correction so these are direct output-channel gains.
// Full white becomes a dim candle-like amber: red is retained most strongly,
// green is reduced further, and blue is nearly eliminated.
const NIGHT_RED: u16 = 72;
const NIGHT_GREEN: u16 = 40;
const NIGHT_BLUE: u16 = 8;

fn scale(channel: u8, gain: u16) -> u8 {
    ((channel as u16 * gain + 127) / 255) as u8
}

/// Apply a dim, warm color-temperature shift suitable for nighttime use.
pub fn apply_night(leds: &mut [Rgb; NUM_LEDS]) {
    for led in leds {
        led.r = scale(led.r, NIGHT_RED);
        led.g = scale(led.g, NIGHT_GREEN);
        led.b = scale(led.b, NIGHT_BLUE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_mode_dims_and_warms_white() {
        let mut leds = [Rgb::new(255, 255, 255); NUM_LEDS];
        apply_night(&mut leds);
        assert_eq!((leds[0].r, leds[0].g, leds[0].b), (72, 40, 8));
    }

    #[test]
    fn night_mode_preserves_black() {
        let mut leds = [Rgb::new(0, 0, 0); NUM_LEDS];
        apply_night(&mut leds);
        assert!(leds.iter().all(|led| *led == Rgb::new(0, 0, 0)));
    }
}
