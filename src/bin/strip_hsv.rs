//! Power test for the 199-pixel RGB SK6812 string on GPIO4 (level-shifted):
//! a full-saturation rainbow spread along the strip, hue drifting at 16 deg/s
//! like the original radial_hsv sketch, at whatever brightness the EC11 sets.
//!
//! Boots at a low brightness. Each detent moves value by 1/32 (hold the button
//! for 1/4 steps); the level and the estimated strip current are logged on
//! every change so you can watch the supply as the load ramps to full.
//!
//!   cargo run --release --bin strip_hsv -- --port COM3

#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::Ordering;

use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::dma::{DmaRxBuf, DmaTxBuf};
use esp_hal::dma_buffers;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::spi::master::{Config as SpiConfig, Spi, SpiDmaBus};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use static_cell::StaticCell;

use esp32_phyllotaxis::input::{BUTTON_PRESSED, ENCODER_DELTA, encoder_task};
use esp32_phyllotaxis::power_math::estimated_ma;
use esp32_phyllotaxis::util::{Hsv, Rgb, hsv_to_rgb_rainbow};

use defmt::{error, info, unwrap};
use esp_println as _;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!();

const NUM_LEDS: usize = 199;
/// Brightness steps per detent, out of 32; hold the button for 8.
const STEPS: i32 = 32;
const FAST_STEP: i32 = 8;
const START_STEP: i32 = 32;
const HUE_DEG_PER_SEC: f32 = 16.0;
const FRAME_MS: u64 = 16;

const SPI_BYTES_PER_LED: usize = 12;
const WS_RESET_BYTES: usize = 120;
const SPI_FRAME: usize = NUM_LEDS * SPI_BYTES_PER_LED + WS_RESET_BYTES;

fn expand(bits: u8) -> u8 {
    let hi = if bits & 2 != 0 { 0b1100 } else { 0b1000 };
    let lo = if bits & 1 != 0 { 0b1100 } else { 0b1000 };
    (hi << 4) | lo
}

fn encode(leds: &[Rgb; NUM_LEDS], tx: &mut [u8; SPI_FRAME]) {
    let mut at = 0;
    for led in leds {
        for byte in [led.g, led.r, led.b] {
            for pair in (0..4).rev() {
                tx[at] = expand((byte >> (2 * pair)) & 0b11);
                at += 1;
            }
        }
    }
    tx[at..].fill(0);
}

fn channel_sum(leds: &[Rgb; NUM_LEDS]) -> u32 {
    leds.iter()
        .map(|led| led.r as u32 + led.g as u32 + led.b as u32)
        .sum()
}

static TX: StaticCell<[u8; SPI_FRAME]> = StaticCell::new();
static LEDS: StaticCell<[Rgb; NUM_LEDS]> = StaticCell::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let (spi_rx, spi_rx_descs, spi_tx, spi_tx_descs) = dma_buffers!(4, SPI_FRAME);
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_hz(3_200_000)),
    )
    .unwrap()
    .with_mosi(peripherals.GPIO4)
    .with_dma(peripherals.DMA_CH1)
    .with_buffers(
        DmaRxBuf::new(spi_rx_descs, spi_rx).unwrap(),
        DmaTxBuf::new(spi_tx_descs, spi_tx).unwrap(),
    )
    .into_async();

    // EC11: A/B and switch all short to ground, so pull-ups.
    let pull_up = InputConfig::default().with_pull(Pull::Up);
    spawner.spawn(unwrap!(encoder_task(
        Input::new(peripherals.GPIO15, pull_up),
        Input::new(peripherals.GPIO16, pull_up),
        Input::new(peripherals.GPIO17, pull_up),
    )));

    info!(
        "strip_hsv: {} pixels rainbow on GPIO4; encoder sets brightness in 1/{} steps",
        NUM_LEDS, STEPS
    );
    spawner.spawn(unwrap!(run(spi)));

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn run(mut spi: SpiDmaBus<'static, Async>) {
    let tx = TX.init([0u8; SPI_FRAME]);
    let leds = LEDS.init([Rgb::new(0, 0, 0); NUM_LEDS]);
    let started = Instant::now();
    let mut step = START_STEP;
    let mut announce = true;
    loop {
        let detents = ENCODER_DELTA.swap(0, Ordering::Relaxed);
        if detents != 0 {
            let by = if BUTTON_PRESSED.load(Ordering::Relaxed) { FAST_STEP } else { 1 };
            step = (step + detents * by).clamp(0, STEPS);
            announce = true;
        }
        let value = step as f32 / STEPS as f32;

        // Bound the phase before going to f32 so long uptimes stay precise.
        let period_ms = (360_000.0 / HUE_DEG_PER_SEC) as u64;
        let phase_ms = started.elapsed().as_millis() % period_ms;
        let drift = phase_ms as f32 * (HUE_DEG_PER_SEC / 1000.0);

        for (i, led) in leds.iter_mut().enumerate() {
            let hue = (i as f32 / NUM_LEDS as f32) * 360.0 + drift;
            let hue = if hue >= 360.0 { hue - 360.0 } else { hue };
            *led = hsv_to_rgb_rainbow(Hsv::new(hue, 1.0, value)).apply_gamma();
        }

        if announce {
            info!(
                "brightness {}/{} -> est. {} mA (after gamma)",
                step,
                STEPS,
                estimated_ma(channel_sum(leds), NUM_LEDS as u32)
            );
            announce = false;
        }

        encode(leds, tx);
        if let Err(e) = spi.write_async(&tx[..]).await {
            error!("spi tx err: {}", e);
        }
        Timer::after(Duration::from_millis(FRAME_MS)).await;
    }
}
