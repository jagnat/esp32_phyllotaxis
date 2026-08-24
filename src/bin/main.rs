#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::dma::{DmaRxBuf, DmaTxBuf};
use esp_hal::dma_buffers;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::i2s::master::{Config, I2s, I2sRx};
use esp_hal::spi::master::{Config as SpiConfig, Spi, SpiDmaBus};
use esp_hal::time::{Duration as HalDuration, Rate};
use esp_hal::timer::timg::{MwdtStage, TimerGroup, Wdt};

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};

use static_cell::StaticCell;

use esp32_phyllotaxis::audio::*;
use esp32_phyllotaxis::input::{BUTTON_PRESSED, BUTTON_RELEASED, ENCODER_DELTA, encoder_task};
use esp32_phyllotaxis::net::{self, SKETCH_SELECT};
use esp32_phyllotaxis::sketches::*;
use esp32_phyllotaxis::ui_state::{UiInput, UiMode, UiState};
use esp32_phyllotaxis::util::*;

use defmt::error;
use defmt::info;
use defmt::unwrap;
use defmt::warn;
use esp_println as _;

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
    esp32_phyllotaxis::health::init(esp_hal::system::reset_reason());

    // PSRAM region is registered first so unconstrained allocations (wasmi's
    // module code, stacks, guest linear memory) land in external RAM; the
    // internal heap stays available for esp-radio, which allocates with the
    // Internal capability and can only use that region.
    // Board is an S3 N_R8: 8MB octal PSRAM. Auto-detect works but explicit is reliable.
    let psram_config = esp_hal::psram::PsramConfig {
        mode: esp_hal::psram::PsramMode::OctalSpi,
        ..Default::default()
    };
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram, psram_config);
    esp_alloc::heap_allocator!(size: 96 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let mut watchdog = timg0.wdt;
    watchdog.set_timeout(MwdtStage::Stage0, HalDuration::from_secs(5));
    watchdog.enable();
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);
    spawner.spawn(unwrap!(esp32_phyllotaxis::health::logger()));

    // Continuous (circular) I2S ring: 8 descriptors x CHUNK_SIZE (4092), ~128ms
    // at 32kHz stereo -- enough to ride out flash-write stalls (interrupts are
    // disabled per erased sector) without overrunning, since an overrun wedges
    // the transfer permanently. The clock must free-run so the INMP441 stays
    // settled -- one-shot reads stop BCLK between reads and re-capture the
    // mic's DC settling transient every window.
    let (rx_buffer, rx_descs, _, _) = dma_buffers!(8 * 4092, 0);
    let i2s = I2s::new(
        peripherals.I2S0,
        peripherals.DMA_CH0,
        Config::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(32000))
            .with_data_format(esp_hal::i2s::master::DataFormat::Data32Channel32),
    )
    .unwrap()
    .into_async();
    let i2s_rx = i2s
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

    esp32_phyllotaxis::gallery::init(peripherals.FLASH);
    spawner.spawn(unwrap!(render(spi, watchdog)));
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

const SCENE_TRANSITION_US: u64 = 220_000;
const MAIN_TARGET: usize = 0;

async fn gallery_menu() -> (Vec<usize>, Vec<Rgb>, Vec<usize>) {
    let mut targets = vec![MAIN_TARGET];
    let mut colors = vec![SketchBank::MAIN_COLOR];
    let mut attract = vec![MAIN_TARGET];
    if let Some(entries) = esp32_phyllotaxis::gallery::menu_entries().await {
        targets.reserve(entries.len());
        colors.reserve(entries.len());
        attract.reserve(entries.len());
        for entry in entries {
            let target = entry.slot + 1;
            targets.push(target);
            colors.push(Rgb::new(entry.color[0], entry.color[1], entry.color[2]));
            if entry.caps & esp32_phyllotaxis::wasm::CAP_WANTS_INPUT == 0 {
                attract.push(target);
            }
        }
    }
    (targets, colors, attract)
}

#[embassy_executor::task]
async fn render(
    mut spi: SpiDmaBus<'static, Async>,
    mut watchdog: Wdt<esp_hal::peripherals::TIMG0<'static>>,
) {
    let started = Instant::now();
    let mut last = started;
    let mut bank = SketchBank::new();
    let mut next = started;
    let mut frame_index = 0u64;
    let mut led_buf = [Rgb::new(0, 0, 0); NUM_LEDS];
    let mut last_audio = AudioFeatures::default();
    let mut tx_buf = [0u8; SPI_FRAME];
    let mut power_limiter = esp32_phyllotaxis::power::LedPowerLimiter::new();
    let (menu_targets, mut menu_colors, attract_targets) = gallery_menu().await;
    let mut gallery_revision = esp32_phyllotaxis::gallery::revision();
    let mut ui = UiState::new(
        &menu_targets,
        esp32_phyllotaxis::gallery::SLOTS + 1,
        MAIN_TARGET,
    );
    ui.set_attract_slots(&attract_targets);
    let mut loaded_gallery_slot = None;
    let mut last_scene = [Rgb::new(0, 0, 0); NUM_LEDS];
    let mut transition_from = last_scene;
    let mut transition_started_us: Option<u64> = None;
    loop {
        if esp32_phyllotaxis::health::audio_dma_stale() {
            error!("audio DMA made no progress for 10s; rebooting");
            esp32_phyllotaxis::health::request_reboot(2);
        }
        if esp32_phyllotaxis::health::REBOOT_REQUESTED
            .swap(false, core::sync::atomic::Ordering::Acquire)
        {
            led_buf.fill(Rgb::new(0, 0, 0));
            encode(&led_buf, &mut tx_buf);
            let _ = spi.write_async(&tx_buf).await;
            Timer::after(Duration::from_millis(20)).await;
            esp_hal::system::software_reset();
        }

        let now = Instant::now();
        let dt_us = esp32_phyllotaxis::timebase::clamp_dt_us((now - last).as_micros());
        last = now;

        if let Some(a) = AUDIO.try_take() {
            last_audio = a;
        }

        let detents = ENCODER_DELTA.swap(0, core::sync::atomic::Ordering::Relaxed);
        let released = BUTTON_RELEASED.swap(false, core::sync::atomic::Ordering::Relaxed);
        let pressed = BUTTON_PRESSED.load(core::sync::atomic::Ordering::Relaxed);
        let selected = SKETCH_SELECT.swap(-1, core::sync::atomic::Ordering::Relaxed);
        let guest_return = esp32_phyllotaxis::wasm::RETURN_TO_MAIN
            .swap(false, core::sync::atomic::Ordering::Acquire);
        let revision = esp32_phyllotaxis::gallery::revision();
        if revision != gallery_revision {
            let (targets, colors, attract) = gallery_menu().await;
            ui.set_menu_slots(&targets);
            ui.set_attract_slots(&attract);
            menu_colors = colors;
            loaded_gallery_slot = None;
            gallery_revision = revision;
        }
        let elapsed_us = (now - started).as_micros();
        let ui_frame = ui.update(
            elapsed_us,
            UiInput {
                detents,
                pressed,
                released,
            },
            bank.input_claimed(),
            if guest_return {
                Some(MAIN_TARGET)
            } else {
                (selected >= 0).then_some(selected as usize)
            },
        );
        if ui_frame.visual_changed {
            transition_from = last_scene;
            transition_started_us = Some(elapsed_us);
        }
        let mut render_target = ui_frame.render_slot;
        let mut running_target = ui_frame.running_slot;
        if render_target == MAIN_TARGET {
            bank.set(SketchBank::MAIN_SLOT);
        } else {
            let gallery_slot = render_target - 1;
            if loaded_gallery_slot != Some(gallery_slot) {
                match esp32_phyllotaxis::gallery::with(|gallery| gallery.read(gallery_slot)).await {
                    Some(Some(bytes)) => {
                        esp32_phyllotaxis::wasm::submit_module(bytes);
                        loaded_gallery_slot = Some(gallery_slot);
                    }
                    _ => {
                        warn!("gallery: selected slot {} is unavailable", gallery_slot);
                        ui.force_select(MAIN_TARGET);
                        render_target = MAIN_TARGET;
                        running_target = MAIN_TARGET;
                        loaded_gallery_slot = None;
                    }
                }
            }
            bank.set(if render_target == MAIN_TARGET {
                SketchBank::MAIN_SLOT
            } else {
                SketchBank::WASM_SLOT
            });
        }
        esp32_phyllotaxis::health::ACTIVE_SKETCH
            .store(bank.active() as u32, core::sync::atomic::Ordering::Relaxed);
        esp32_phyllotaxis::health::RUNNING_SKETCH
            .store(running_target as u32, core::sync::atomic::Ordering::Relaxed);
        esp32_phyllotaxis::health::UI_MODE.store(
            (ui_frame.mode == UiMode::Menu) as u32,
            core::sync::atomic::Ordering::Relaxed,
        );
        esp32_phyllotaxis::health::MENU_CURSOR.store(
            ui_frame
                .menu_cursor
                .map_or(u32::MAX, |cursor| cursor as u32),
            core::sync::atomic::Ordering::Relaxed,
        );

        // Only the wasm sketch may park the audio pipeline, and only while active.
        if bank.active() != SketchBank::WASM_SLOT {
            esp32_phyllotaxis::wasm::AUDIO_ENABLED
                .store(true, core::sync::atomic::Ordering::Relaxed);
        }

        let params = SketchParams {
            dt_us,
            elapsed_us,
            frame_index,
            audio: last_audio,
            detents: ui_frame.sketch_input.detents,
            pressed: ui_frame.sketch_input.pressed,
            released: ui_frame.sketch_input.released,
            preview: ui_frame.mode == UiMode::Menu,
        };
        bank.render(&mut led_buf, &params);
        if let Some(position) = ui_frame.menu_position {
            esp32_phyllotaxis::menu::render(&mut led_buf, position, &menu_colors, elapsed_us);
        }
        if let Some(transition_start) = transition_started_us {
            let transition_elapsed = elapsed_us.saturating_sub(transition_start);
            if esp32_phyllotaxis::menu::fade_through_black(
                &mut led_buf,
                &transition_from,
                transition_elapsed,
                SCENE_TRANSITION_US,
            ) {
                transition_started_us = None;
            }
        }
        last_scene = led_buf;
        apply_gamma(&mut led_buf);
        power_limiter.apply(&mut led_buf);

        encode(&led_buf, &mut tx_buf);

        // A failed frame is transient; log it rather than panicking the render loop.
        if let Err(e) = spi.write_async(&tx_buf).await {
            error!("spi tx err: {}", e);
        }
        watchdog.feed();
        esp32_phyllotaxis::health::RENDERED_FRAMES
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        frame_index = frame_index.wrapping_add(1);

        next += Duration::from_micros(esp32_phyllotaxis::timebase::FRAME_PERIOD_US);
        let finished = Instant::now();
        if finished > next {
            let lag = finished - next;
            let missed = 1 + lag.as_micros() / esp32_phyllotaxis::timebase::FRAME_PERIOD_US;
            esp32_phyllotaxis::health::MISSED_FRAME_DEADLINES.fetch_add(
                missed.min(u32::MAX as u64) as u32,
                core::sync::atomic::Ordering::Relaxed,
            );
            if lag > Duration::from_millis(50) {
                defmt::warn!("render stalled {}ms", lag.as_millis());
            }
            // Drop accumulated backlog. A subsequent fast frame can return to
            // the fixed cadence instead of running permanently behind.
            next = finished;
        } else {
            Timer::at(next).await;
        }
    }
}

// One FFT window = 512 frames x 8 bytes/frame = 4096 bytes (multiple of the 8-byte frame,
// so window boundaries stay frame-aligned).
const WINDOW: usize = 512 * 8;

// Staging: holds carry-over (< WINDOW) plus one pop (<= the full ring). `pop`
// errors if the buffer is smaller than what's available, so this must exceed
// the ring size.
static STAGE: StaticCell<[u8; 8 * 4092 + 4096]> = StaticCell::new();

#[embassy_executor::task]
async fn i2s_receive(i2s_rx: I2sRx<'static, Async>, buf: &'static mut [u8; 8 * 4092]) {
    // Radio bring-up (RF calibration / association) can swallow the GDMA
    // in-done interrupt long enough to overrun the ring, which wedges the
    // circular transfer for good -- so don't start it until wifi is settled
    // (or clearly isn't coming).
    if esp32_phyllotaxis::net::WIFI_STARTED.load(core::sync::atomic::Ordering::Relaxed) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !esp32_phyllotaxis::net::NET_UP.load(core::sync::atomic::Ordering::Relaxed)
            && Instant::now() < deadline
        {
            Timer::after(Duration::from_millis(100)).await;
        }
        Timer::after(Duration::from_secs(1)).await;
        info!("starting audio capture");
    }

    let mut proc = AudioProcessor::new();
    let mut transfer = match i2s_rx.read_dma_circular_async(buf) {
        Ok(t) => t,
        Err(e) => {
            error!("i2s circular init err: {}", e);
            esp32_phyllotaxis::health::AUDIO_DMA_ERRORS
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            esp32_phyllotaxis::health::request_reboot(1);
            return;
        }
    };
    esp32_phyllotaxis::health::audio_capture_started();
    let stage = STAGE.init([0u8; 8 * 4092 + 4096]);
    let mut fill = 0usize;
    loop {
        let n = match transfer.pop(&mut stage[fill..fill + 8 * 4092]).await {
            Ok(n) => n,
            Err(e) => {
                // An overrun (Late) wedges the circular transfer permanently;
                // esp-hal has no restart API, so reboot the peripheral cleanly.
                error!("i2s pop err: {}; requesting reboot", e);
                esp32_phyllotaxis::health::AUDIO_DMA_ERRORS
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                esp32_phyllotaxis::health::request_reboot(1);
                return;
            }
        };
        esp32_phyllotaxis::health::note_audio_dma_progress();
        fill += n;

        if !esp32_phyllotaxis::wasm::AUDIO_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
            fill = 0;
            continue;
        }

        while fill >= WINDOW {
            // DC block: the INMP441 carries a DC offset that leaks into the lowest FFT bins
            // via the Hamming window's main lobe. Subtract the window mean.
            let mut sum = 0i64;
            for i in 0..512 {
                sum += decode_left_sample(&stage[..WINDOW], i) as i64;
            }
            let dc = (sum as f32 / 512.0) / 8388608.0;
            let feats =
                proc.process(|i| decode_left_sample(&stage[..WINDOW], i) as f32 / 8388608.0 - dc);
            AUDIO.signal(feats);
            stage.copy_within(WINDOW..fill, 0);
            fill -= WINDOW;
        }
    }
}
