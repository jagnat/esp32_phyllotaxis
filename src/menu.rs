use libm::sinf;

use crate::menu_layout::{is_reserved, relative_to_selected, GROUPS};
use crate::util::{NUM_LEDS, Rgb};

#[inline(never)]
fn indicator_color(base: Rgb, relative: i32, pulse: f32) -> Rgb {
    match relative {
        0 => base.lerp(Rgb::new(255, 255, 255), 0.60 * pulse),
        -1 | 1 => Rgb::new(0, 0, 0).lerp(base, 0.34),
        _ => Rgb::new(0, 0, 0).lerp(base, 0.14),
    }
}

/// Reserve five four-pixel phyllotaxis arms for a moving menu viewport. The
/// arms show previous-2, previous-1, hovered, next-1, and next-2 sketches.
#[inline(never)]
pub fn render(
    leds: &mut [Rgb; NUM_LEDS],
    position: i64,
    sketch_colors: &[Rgb],
    elapsed_us: u64,
) {
    if sketch_colors.is_empty() {
        return;
    }
    let pulse = 0.86
        + 0.14
            * ((sinf(crate::timebase::phase_radians(elapsed_us, 1_500_000)) + 1.0)
                * 0.5);
    for (i, led) in leds.iter_mut().enumerate() {
        *led = if is_reserved(i) {
            Rgb::new(0, 0, 0)
        } else {
            Rgb::new(0, 0, 0).lerp(*led, 0.65)
        };
    }

    let selected_position = position.rem_euclid(GROUPS.len() as i64) as usize;
    for (group_position, group) in GROUPS.iter().enumerate() {
        let relative = relative_to_selected(group_position, selected_position);
        let sketch_index = (position + relative as i64)
            .rem_euclid(sketch_colors.len() as i64) as usize;
        let color = indicator_color(sketch_colors[sketch_index], relative, pulse);
        for index in *group {
            leds[index] = color;
        }
    }
}

/// Briefly fade the old scene to black, then bring up the newly rendered one.
/// Kept out of the async render future to avoid inflating its state and to
/// sidestep an Xtensa LLVM vector-constant selection bug.
#[inline(never)]
pub fn fade_through_black(
    leds: &mut [Rgb; NUM_LEDS],
    from: &[Rgb; NUM_LEDS],
    elapsed_us: u64,
    duration_us: u64,
) -> bool {
    if duration_us == 0 || elapsed_us >= duration_us {
        return true;
    }

    let midpoint = (duration_us / 2).max(1);
    if elapsed_us < midpoint {
        let t = elapsed_us as f32 / midpoint as f32;
        let brightness = 1.0 - t * t * (3.0 - 2.0 * t);
        for (led, previous) in leds.iter_mut().zip(from.iter()) {
            *led = Rgb::new(0, 0, 0).lerp(*previous, brightness);
        }
    } else {
        let fade_in_us = duration_us - midpoint;
        let t = (elapsed_us - midpoint) as f32 / fade_in_us as f32;
        let brightness = t * t * (3.0 - 2.0 * t);
        for led in leds.iter_mut() {
            *led = Rgb::new(0, 0, 0).lerp(*led, brightness);
        }
    }
    false
}
