use core::sync::atomic::{AtomicU32, Ordering};

use alloc::{format, string::String};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::rtc_cntl::SocResetReason;

pub static RENDERED_FRAMES: AtomicU32 = AtomicU32::new(0);
pub static MISSED_FRAME_DEADLINES: AtomicU32 = AtomicU32::new(0);
pub static AUDIO_DMA_ERRORS: AtomicU32 = AtomicU32::new(0);
pub static WASM_LOAD_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static WASM_FRAME_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static WASM_FRAME_OVERRUNS: AtomicU32 = AtomicU32::new(0);
pub static ACTIVE_SKETCH: AtomicU32 = AtomicU32::new(0);

const RESET_UNKNOWN: u32 = u32::MAX;
static RESET_REASON: AtomicU32 = AtomicU32::new(RESET_UNKNOWN);

pub fn init(reset_reason: Option<SocResetReason>) {
    RESET_REASON.store(
        reset_reason.map_or(RESET_UNKNOWN, |reason| reason as u32),
        Ordering::Relaxed,
    );
}

pub fn record_max(counter: &AtomicU32, value: u64) {
    counter.fetch_max(value.min(u32::MAX as u64) as u32, Ordering::Relaxed);
}

pub fn reset_reason_name() -> &'static str {
    match RESET_REASON.load(Ordering::Relaxed) {
        0x01 => "power_on",
        0x03 => "software",
        0x05 => "deep_sleep",
        0x07 => "core_mwdt0",
        0x08 => "core_mwdt1",
        0x09 => "core_rtc_wdt",
        0x0b => "cpu_mwdt0",
        0x0c => "cpu_software",
        0x0d => "cpu_rtc_wdt",
        0x0f => "brownout",
        0x10 => "system_rtc_wdt",
        0x11 => "cpu_mwdt1",
        0x12 => "super_wdt",
        0x13 => "clock_glitch",
        0x14 => "efuse_crc",
        0x15 => "usb_uart",
        0x16 => "usb_jtag",
        0x17 => "power_glitch",
        _ => "unknown",
    }
}

pub fn json() -> String {
    let uptime_ms = Instant::now().as_millis();
    format!(
        concat!(
            "{{\"api_version\":2,\"firmware\":\"{}\",",
            "\"uptime_ms\":{},\"reset_reason\":\"{}\",",
            "\"active_sketch\":{},\"rendered_frames\":{},",
            "\"missed_frame_deadlines\":{},\"audio_dma_errors\":{},",
            "\"wasm_load_max_us\":{},\"wasm_frame_max_us\":{},",
            "\"wasm_frame_overruns\":{},\"heap_internal_free\":{},",
            "\"heap_external_free\":{}}}\n"
        ),
        env!("CARGO_PKG_VERSION"),
        uptime_ms,
        reset_reason_name(),
        ACTIVE_SKETCH.load(Ordering::Relaxed),
        RENDERED_FRAMES.load(Ordering::Relaxed),
        MISSED_FRAME_DEADLINES.load(Ordering::Relaxed),
        AUDIO_DMA_ERRORS.load(Ordering::Relaxed),
        WASM_LOAD_MAX_US.load(Ordering::Relaxed),
        WASM_FRAME_MAX_US.load(Ordering::Relaxed),
        WASM_FRAME_OVERRUNS.load(Ordering::Relaxed),
        crate::heap_internal_free(),
        crate::heap_external_free(),
    )
}

#[embassy_executor::task]
pub async fn logger() {
    loop {
        defmt::info!(
            "health uptime={}s reset={} sketch={} frames={} missed={} audio_errs={} wasm_load_max={}us wasm_frame_max={}us wasm_overruns={} heap_int={} heap_ext={}",
            Instant::now().as_secs(),
            reset_reason_name(),
            ACTIVE_SKETCH.load(Ordering::Relaxed),
            RENDERED_FRAMES.load(Ordering::Relaxed),
            MISSED_FRAME_DEADLINES.load(Ordering::Relaxed),
            AUDIO_DMA_ERRORS.load(Ordering::Relaxed),
            WASM_LOAD_MAX_US.load(Ordering::Relaxed),
            WASM_FRAME_MAX_US.load(Ordering::Relaxed),
            WASM_FRAME_OVERRUNS.load(Ordering::Relaxed),
            crate::heap_internal_free(),
            crate::heap_external_free(),
        );
        Timer::after(Duration::from_secs(30)).await;
    }
}
