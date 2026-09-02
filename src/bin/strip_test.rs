//! LED counter for an RGB SK6812/WS2812 strip on GPIO4 (level-shifted).
//!
//! Exactly one pixel is lit white. The EC11 encoder moves it one LED per
//! detent, or ten per detent while the button is held, and every change logs
//! the 0-based index and the 1-based LED number over defmt. Walk the pixel to
//! the last physical LED and read the log to get the strip length.
//!
//! Flash and watch in one go:
//!   cargo run --release --bin strip_test -- --port COM3

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
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

use esp32_phyllotaxis::input::{BUTTON_PRESSED, ENCODER_DELTA, encoder_task};

use defmt::{error, info, unwrap};
use esp_println as _;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!();

/// Upper bound on the cursor. Over-counting is harmless: positions past the
/// end of the physical strip are simply dark.
const NUM_LEDS: usize = 300;
const LEVEL: u8 = 128;
/// Detents move the cursor this far while the button is held.
const FAST_STEP: i32 = 10;
const POLL_MS: u64 = 10;

// WS2812/SK6812 over SPI MOSI at 3.2 MHz: each LED bit becomes 4 SPI bits
// (0 -> 1000 = 312ns high, 1 -> 1100 = 625ns high), so 12 SPI bytes per
// GRB pixel, then >280us of low to latch.
const SPI_BYTES_PER_LED: usize = 12;
const WS_RESET_BYTES: usize = 120;
const SPI_FRAME: usize = NUM_LEDS * SPI_BYTES_PER_LED + WS_RESET_BYTES;

fn expand(bits: u8) -> u8 {
    let hi = if bits & 2 != 0 { 0b1100 } else { 0b1000 };
    let lo = if bits & 1 != 0 { 0b1100 } else { 0b1000 };
    (hi << 4) | lo
}

/// Frame with only `lit` on, in white.
fn encode(tx: &mut [u8; SPI_FRAME], lit: usize) {
    let mut at = 0;
    for i in 0..NUM_LEDS {
        let level = if i == lit { LEVEL } else { 0 };
        for byte in [level, level, level] {
            for pair in (0..4).rev() {
                tx[at] = expand((byte >> (2 * pair)) & 0b11);
                at += 1;
            }
        }
    }
    tx[at..].fill(0);
}

static TX: StaticCell<[u8; SPI_FRAME]> = StaticCell::new();

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
        "strip_test: one pixel on GPIO4; encoder moves it, hold button for x{} steps",
        FAST_STEP
    );
    spawner.spawn(unwrap!(run(spi)));

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn run(mut spi: SpiDmaBus<'static, Async>) {
    let tx = TX.init([0u8; SPI_FRAME]);
    let mut cursor: usize = 0;
    let mut dirty = true;
    loop {
        let detents = ENCODER_DELTA.swap(0, Ordering::Relaxed);
        if detents != 0 {
            let step = if BUTTON_PRESSED.load(Ordering::Relaxed) { FAST_STEP } else { 1 };
            cursor = (cursor as i32 + detents * step).clamp(0, NUM_LEDS as i32 - 1) as usize;
            dirty = true;
        }
        if dirty {
            encode(tx, cursor);
            if let Err(e) = spi.write_async(&tx[..]).await {
                error!("spi tx err: {}", e);
            }
            info!("cursor index={} (LED number {})", cursor, cursor + 1);
            dirty = false;
        }
        Timer::after(Duration::from_millis(POLL_MS)).await;
    }
}
