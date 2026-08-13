#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use esp_hal::clock::CpuClock;
use esp_hal::dma_buffers;
use esp_hal::dma::{DmaRxBuf, DmaTxBuf};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::spi::master::{Config as SpiConfig, Spi, SpiDmaBus};
use esp_hal::Async;
use esp_hal::i2s::master::{Config, I2s, I2sRx};

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer, Instant};
use embassy_sync::signal::Signal;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use static_cell::StaticCell;

use esp32_phyllotaxis::util::*;
use esp32_phyllotaxis::sketches::*;
use esp32_phyllotaxis::audio::*;
use esp32_phyllotaxis::input::{encoder_task, BUTTON_PRESSED, BUTTON_RELEASED, ENCODER_DELTA};
use esp32_phyllotaxis::net::{self, SKETCH_SELECT};

use esp_println as _;
use defmt::info;
use defmt::error;
use defmt::unwrap;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

// WS2812 driven over SPI MOSI at 3.2 MHz: each WS2812 bit expands to 4 SPI bits
// (0 -> 1000 = 312ns high, 1 -> 1100 = 625ns high, 1.25us period). The whole frame
// streams by DMA, so LED timing is immune to interrupt latency from wifi/FFT.
const WS_BYTES_PER_LED: usize = 12;
// >280us of low after the frame latches the LEDs.
const WS_RESET_BYTES: usize = 120;
const SPI_FRAME: usize = NUM_LEDS * WS_BYTES_PER_LED + WS_RESET_BYTES;

static AUDIO: Signal<CriticalSectionRawMutex, AudioFeatures> = Signal::new();

// I2S frame = [L 32-bit][R 32-bit] = 8 bytes, little-endian in the DMA buffer.
// 24-bit mic data is left-justified in the 32-bit word; `>> 8` recovers the signed sample.
fn decode_left_sample(buf: &[u8], i: usize) -> i32 {
    let o = 8 * i;
    let w = u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    (w as i32) >> 8
}

fn encode(rgb: &[Rgb; NUM_LEDS], tx: &mut [u8; SPI_FRAME]) {
    fn expand(bits: u8) -> u8 {
        let hi = if bits & 2 != 0 { 0b1100 } else { 0b1000 };
        let lo = if bits & 1 != 0 { 0b1100 } else { 0b1000 };
        (hi << 4) | lo
    }
    let mut i = 0;
    for led in rgb {
        let grb = [led.g, led.r, led.b];
        for byte in grb {
            for pair in (0..4).rev() {
                tx[i] = expand((byte >> (2 * pair)) & 0b11);
                i += 1;
            }
        }
    }
    tx[i..].fill(0);
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32s3 -o unstable-hal -o embassy -o defmt

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 96 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    // Continuous (circular) I2S ring: 4 descriptors x CHUNK_SIZE (4092). The clock must
    // free-run so the INMP441 stays settled -- one-shot reads stop BCLK between reads and
    // re-capture the mic's DC settling transient every window.
    let (rx_buffer, rx_descs, _, _) = dma_buffers!(4 * 4092, 0);
    let i2s = I2s::new(
        peripherals.I2S0,
        peripherals.DMA_CH0,
        Config::new_tdm_philips().with_sample_rate(Rate::from_hz(32000)).with_data_format(esp_hal::i2s::master::DataFormat::Data32Channel32)
    ).unwrap().into_async();
    let mut i2s_rx = i2s
        .i2s_rx
        .with_bclk(peripherals.GPIO7)
        .with_ws(peripherals.GPIO6)
        .with_din(peripherals.GPIO5)
        .build(rx_descs);
    
    info!("Embassy initialized!");

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

    spawner.spawn(unwrap!(render(spi)));
    spawner.spawn(unwrap!(i2s_receive(i2s_rx, rx_buffer)));

    let rng = esp_hal::rng::Rng::new();
    let seed = ((rng.random() as u64) << 32) | rng.random() as u64;
    net::start(spawner, peripherals.WIFI, seed);

    // EC11: A/B and switch all short to ground, so pull-ups.
    let pull_up = InputConfig::default().with_pull(Pull::Up);
    spawner.spawn(unwrap!(encoder_task(
        Input::new(peripherals.GPIO15, pull_up),
        Input::new(peripherals.GPIO16, pull_up),
        Input::new(peripherals.GPIO17, pull_up),
    )));

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}

#[embassy_executor::task]
async fn render(mut spi: SpiDmaBus<'static, Async>) {
    let mut last = Instant::now();
    let mut t: f32 = 0.0;
    let mut bank = SketchBank::new();
    let mut next = Instant::now();
    let mut led_buf = [Rgb::new(0,0,0); NUM_LEDS];
    let mut last_audio = AudioFeatures::default();
    let mut tx_buf = [0u8; SPI_FRAME];
    loop {
        next += Duration::from_millis(1000/120);
        let now = Instant::now();
        let dt = (now - last).as_millis() as f32 / 1000.0;
        last = now;
        t += dt;

        if let Some(a) = AUDIO.try_take() { last_audio = a; }

        let detents = ENCODER_DELTA.swap(0, core::sync::atomic::Ordering::Relaxed);
        let released = BUTTON_RELEASED.swap(false, core::sync::atomic::Ordering::Relaxed);
        let pressed = BUTTON_PRESSED.load(core::sync::atomic::Ordering::Relaxed);
        if released { bank.step(1); }
        let selected = SKETCH_SELECT.swap(-1, core::sync::atomic::Ordering::Relaxed);
        if selected >= 0 { bank.set(selected as usize); }

        let params = SketchParams { dt, t, audio: last_audio, detents, pressed, released };
        bank.render(&mut led_buf, &params);
        apply_gamma(&mut led_buf);

        encode(&led_buf, &mut tx_buf);

        // A failed frame is transient; log it rather than panicking the render loop.
        if let Err(e) = spi.write_async(&tx_buf).await {
            error!("spi tx err: {}", e);
        }
        
        Timer::at(next).await;
    }
}

// One FFT window = 512 frames x 8 bytes/frame = 4096 bytes (multiple of the 8-byte frame,
// so window boundaries stay frame-aligned).
const WINDOW: usize = 512 * 8;

// Staging: holds carry-over (< WINDOW) plus one pop (<= ring's available). Sized generously
// so `pop` (which errors if the buffer is smaller than what's available) never underflows.
static STAGE: StaticCell<[u8; 16384]> = StaticCell::new();

#[embassy_executor::task]
async fn i2s_receive(i2s_rx: I2sRx<'static, Async>, buf: &'static mut [u8; 4 * 4092]) {
    let mut proc = AudioProcessor::new();
    let mut transfer = match i2s_rx.read_dma_circular_async(buf) {
        Ok(t) => t,
        Err(e) => {
            error!("i2s circular init err: {}", e);
            return;
        }
    };
    let stage = STAGE.init([0u8; 16384]);
    let mut fill = 0usize;
    loop {
        let n = match transfer.pop(&mut stage[fill..]).await {
            Ok(n) => n,
            Err(e) => {
                error!("i2s pop err: {}", e);
                fill = 0; // resync
                continue;
            }
        };
        fill += n;

        while fill >= WINDOW {
            // DC block: the INMP441 carries a DC offset that leaks into the lowest FFT bins
            // via the Hamming window's main lobe. Subtract the window mean.
            let mut sum = 0i64;
            for i in 0..512 {
                sum += decode_left_sample(&stage[..WINDOW], i) as i64;
            }
            let dc = (sum as f32 / 512.0) / 8388608.0;
            let feats = proc.process(|i| decode_left_sample(&stage[..WINDOW], i) as f32 / 8388608.0 - dc);
            AUDIO.signal(feats);
            stage.copy_within(WINDOW..fill, 0);
            fill -= WINDOW;
        }
    }
}
