//! Your sketch lives here. The runtime and raw ABI are in `phyllo.rs`.

use crate::phyllo::{self, Frame, Input, LED_COUNT};

/// Use NONE, AUDIO, INPUT, or AUDIO | INPUT.
pub const CAPABILITIES: u32 = phyllo::capabilities::NONE;

/// Called at about 60fps. Write all 90 pixels each frame.
pub fn render(input: &Input<'_>, frame: &mut Frame<'_>) {
    // `phase` safely loops integer time before converting it to f32.
    let time = input.phase(8_000_000);

    for led in 0..LED_COUNT {
        let point = phyllo::position(led);
        let radius = libm::sqrtf(point.x * point.x + point.y * point.y);
        let hue = time + radius * 0.35 + point.y * 0.08;
        let pulse = 0.45 + 0.35 * libm::sinf((time - radius) * core::f32::consts::TAU);
        frame.set(led, phyllo::hsv(hue, 0.85, pulse));
    }

    // Available when requested in CAPABILITIES:
    // input.audio(); input.encoder_delta(); input.encoder_position();
    // input.button_held(); input.button_released();
    // Also: phyllo::agc(channel, value), random(), return_to_main().
}
