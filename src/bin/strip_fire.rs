//! Power test for the 199-pixel RGB SK6812 string on GPIO4 (level-shifted):
//! fire colors only (deep red through orange to yellow), full brightness, no
//! knob. Hue ripples along the strip so it looks alive, but every pixel stays
//! at full value so the load is representative of a bright fire sketch.
//! Estimated strip current is logged every few seconds.
//!
//!   cargo run --release --bin strip_fire -- --port COM3

#![no_std]
#![no_main]

extern crate alloc;

use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::dma::{DmaRxBuf, DmaTxBuf};
use esp_hal::dma_buffers;
use esp_hal::spi::master::{Config as SpiConfig, Spi, SpiDmaBus};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use libm::sinf;
use static_cell::StaticCell;

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
/// Hue band in degrees: 0 is red, ~50 is yellow on the rainbow wheel.
const HUE_MIN: f32 = 2.0;
const HUE_MAX: f32 = 48.0;
/// Spatial wavelength of the ripple in pixels, and its speed.
const RIPPLE_PIXELS: f32 = 18.0;
const RIPPLE_HZ: f32 = 0.6;
const FRAME_MS: u64 = 16;
const LOG_EVERY_MS: u64 = 5_000;

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

    info!("strip_fire: {} pixels, fire hues at full brightness on GPIO4", NUM_LEDS);
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
    let mut last_log_ms = 0u64;
    let mut first = true;
    loop {
        // Bound the phase before going to f32 so long uptimes stay precise.
        let period_ms = (1000.0 / RIPPLE_HZ) as u64;
        let now_ms = started.elapsed().as_millis();
        let t = (now_ms % period_ms) as f32 / period_ms as f32 * core::f32::consts::TAU;

        for (i, led) in leds.iter_mut().enumerate() {
            let x = i as f32 / RIPPLE_PIXELS * core::f32::consts::TAU;
            // Two ripples of different scale so it does not read as a sine wave.
            let wave = 0.5 + 0.35 * sinf(x - t) + 0.15 * sinf(2.7 * x + 1.9 * t);
            let hue = HUE_MIN + (HUE_MAX - HUE_MIN) * wave.clamp(0.0, 1.0);
            *led = hsv_to_rgb_rainbow(Hsv::new(hue, 1.0, 1.0)).apply_gamma();
        }

        if first || now_ms - last_log_ms >= LOG_EVERY_MS {
            info!(
                "est. {} mA (after gamma)",
                estimated_ma(channel_sum(leds), NUM_LEDS as u32)
            );
            last_log_ms = now_ms;
            first = false;
        }

        encode(leds, tx);
        if let Err(e) = spi.write_async(&tx[..]).await {
            error!("spi tx err: {}", e);
        }
        Timer::after(Duration::from_millis(FRAME_MS)).await;
    }
}
