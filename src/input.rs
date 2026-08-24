use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};

use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::Input;

/// Accumulated detents since last read; consumers `swap(0)` to drain.
pub static ENCODER_DELTA: AtomicI32 = AtomicI32::new(0);
/// Absolute detent count since boot; never reset.
pub static ENCODER_POS: AtomicI32 = AtomicI32::new(0);
/// Set on a debounced button release; consumers `swap(false)` to drain.
pub static BUTTON_RELEASED: AtomicBool = AtomicBool::new(false);
/// Debounced button level; true while held down.
pub static BUTTON_PRESSED: AtomicBool = AtomicBool::new(false);
/// Raw (undebounced) pin levels for diagnostics: bit2 = A, bit1 = B, bit0 = button.
pub static RAW_PINS: AtomicU8 = AtomicU8::new(0);

// Quadrature transition table, indexed by (prev_ab << 2) | ab. Invalid
// transitions (both lines changed) contribute 0, so contact bounce cancels
// out instead of miscounting.
const QDEC: [i8; 16] = [0, -1, 1, 0, 1, 0, 0, -1, -1, 0, 0, 1, 0, 1, -1, 0];

const DEBOUNCE: Duration = Duration::from_millis(20);
const STARTUP_SETTLE: Duration = Duration::from_millis(100);

fn ab(a: &Input, b: &Input) -> u8 {
    ((a.is_high() as u8) << 1) | (b.is_high() as u8)
}

/// Polls an EC11 rotary encoder (A/B to ground, internal pull-ups) and its
/// push switch. One detent = one full quadrature cycle (4 transitions),
/// committed only at the rest position (both lines high).
#[embassy_executor::task]
pub async fn encoder_task(a: Input<'static>, b: Input<'static>, btn: Input<'static>) {
    // Do not turn power-up transients (or a button held while powering on) into
    // a release/detent event. Arm only after every input has remained at its
    // pull-up idle level for a short, continuous interval.
    let mut idle_since = Instant::now();
    loop {
        let cur = ab(&a, &b);
        let raw = btn.is_high();
        RAW_PINS.store((cur << 1) | raw as u8, Ordering::Relaxed);
        if cur == 0b11 && raw {
            if Instant::now() - idle_since >= STARTUP_SETTLE {
                break;
            }
        } else {
            idle_since = Instant::now();
        }
        Timer::after(Duration::from_millis(1)).await;
    }

    ENCODER_DELTA.store(0, Ordering::Relaxed);
    BUTTON_RELEASED.store(false, Ordering::Relaxed);
    BUTTON_PRESSED.store(false, Ordering::Relaxed);

    let mut prev = 0b11;
    let mut accum: i8 = 0;

    let mut btn_stable = true;
    let mut btn_last = true;
    let mut btn_change = Instant::now();

    loop {
        let cur = ab(&a, &b);
        let raw = btn.is_high();
        RAW_PINS.store((cur << 1) | raw as u8, Ordering::Relaxed);
        if cur != prev {
            accum += QDEC[((prev << 2) | cur) as usize];
            prev = cur;
            if cur == 0b11 {
                if accum >= 4 {
                    ENCODER_DELTA.fetch_add(1, Ordering::Relaxed);
                    ENCODER_POS.fetch_add(1, Ordering::Relaxed);
                } else if accum <= -4 {
                    ENCODER_DELTA.fetch_sub(1, Ordering::Relaxed);
                    ENCODER_POS.fetch_sub(1, Ordering::Relaxed);
                }
                accum = 0;
            }
        }

        if raw != btn_last {
            btn_last = raw;
            btn_change = Instant::now();
        } else if raw != btn_stable && Instant::now() - btn_change >= DEBOUNCE {
            btn_stable = raw;
            // Switch shorts to ground when pressed; high edge = release.
            BUTTON_PRESSED.store(!raw, Ordering::Relaxed);
            if raw {
                BUTTON_RELEASED.store(true, Ordering::Relaxed);
            }
        }

        Timer::after(Duration::from_millis(1)).await;
    }
}
