use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
pub static RUNNING_SKETCH: AtomicU32 = AtomicU32::new(0);
pub static UI_MODE: AtomicU32 = AtomicU32::new(0);
pub static MENU_CURSOR: AtomicU32 = AtomicU32::new(u32::MAX);
pub static AUDIO_DMA_POPS: AtomicU32 = AtomicU32::new(0);
pub static LAST_AUDIO_DMA_MS: AtomicU32 = AtomicU32::new(0);
pub static AUDIO_CAPTURE_STARTED: AtomicBool = AtomicBool::new(false);
pub static REBOOT_REQUESTED: AtomicBool = AtomicBool::new(false);
static REBOOT_REASON: AtomicU32 = AtomicU32::new(0);

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

pub fn note_audio_dma_progress() {
    AUDIO_DMA_POPS.fetch_add(1, Ordering::Relaxed);
    LAST_AUDIO_DMA_MS.store(Instant::now().as_millis() as u32, Ordering::Relaxed);
}

pub fn audio_capture_started() {
    LAST_AUDIO_DMA_MS.store(Instant::now().as_millis() as u32, Ordering::Relaxed);
    AUDIO_CAPTURE_STARTED.store(true, Ordering::Relaxed);
}

pub fn audio_dma_stale() -> bool {
    if !AUDIO_CAPTURE_STARTED.load(Ordering::Relaxed) {
        return false;
    }
    let last = LAST_AUDIO_DMA_MS.load(Ordering::Relaxed);
    crate::timebase::elapsed_ms32(Instant::now().as_millis() as u32, last) > 10_000
}

pub fn request_reboot(reason: u32) {
    REBOOT_REASON.store(reason, Ordering::Relaxed);
    REBOOT_REQUESTED.store(true, Ordering::Release);
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
            "{{\"firmware\":\"{}\",",
            "\"uptime_ms\":{},\"reset_reason\":\"{}\",",
            "\"active_sketch\":{},\"running_sketch\":{},",
            "\"ui_mode\":{},\"menu_cursor\":{},\"rendered_frames\":{},",
            "\"missed_frame_deadlines\":{},\"audio_dma_errors\":{},",
            "\"wasm_load_max_us\":{},\"wasm_frame_max_us\":{},",
            "\"wasm_frame_overruns\":{},\"audio_dma_pops\":{},",
            "\"reboot_requested\":{},\"reboot_reason\":{},",
            "\"led_budget_ma\":{},\"led_requested_ma\":{},",
            "\"led_output_ma\":{},\"led_scale_permille\":{},",
            "\"heap_internal_free\":{},",
            "\"heap_external_free\":{}}}\n"
        ),
        env!("CARGO_PKG_VERSION"),
        uptime_ms,
        reset_reason_name(),
        ACTIVE_SKETCH.load(Ordering::Relaxed),
        RUNNING_SKETCH.load(Ordering::Relaxed),
        UI_MODE.load(Ordering::Relaxed),
        MENU_CURSOR.load(Ordering::Relaxed),
        RENDERED_FRAMES.load(Ordering::Relaxed),
        MISSED_FRAME_DEADLINES.load(Ordering::Relaxed),
        AUDIO_DMA_ERRORS.load(Ordering::Relaxed),
        WASM_LOAD_MAX_US.load(Ordering::Relaxed),
        WASM_FRAME_MAX_US.load(Ordering::Relaxed),
        WASM_FRAME_OVERRUNS.load(Ordering::Relaxed),
        AUDIO_DMA_POPS.load(Ordering::Relaxed),
        REBOOT_REQUESTED.load(Ordering::Relaxed),
        REBOOT_REASON.load(Ordering::Relaxed),
        crate::power::LED_BUDGET_MA.load(Ordering::Relaxed),
        crate::power::LED_REQUESTED_MA.load(Ordering::Relaxed),
        crate::power::LED_OUTPUT_MA.load(Ordering::Relaxed),
        crate::power::LED_SCALE_PERMILLE.load(Ordering::Relaxed),
        crate::heap_internal_free(),
        crate::heap_external_free(),
    )
}

#[embassy_executor::task]
pub async fn logger() {
    loop {
        defmt::info!(
            "health uptime={}s reset={} sketch={} run={} ui={} cursor={} frames={} missed={} audio_errs={} audio_pops={} wasm_load_max={}us wasm_frame_max={}us wasm_overruns={} led={}/{}mA scale={}/1000 heap_int={} heap_ext={}",
            Instant::now().as_secs(),
            reset_reason_name(),
            ACTIVE_SKETCH.load(Ordering::Relaxed),
            RUNNING_SKETCH.load(Ordering::Relaxed),
            UI_MODE.load(Ordering::Relaxed),
            MENU_CURSOR.load(Ordering::Relaxed),
            RENDERED_FRAMES.load(Ordering::Relaxed),
            MISSED_FRAME_DEADLINES.load(Ordering::Relaxed),
            AUDIO_DMA_ERRORS.load(Ordering::Relaxed),
            AUDIO_DMA_POPS.load(Ordering::Relaxed),
            WASM_LOAD_MAX_US.load(Ordering::Relaxed),
            WASM_FRAME_MAX_US.load(Ordering::Relaxed),
            WASM_FRAME_OVERRUNS.load(Ordering::Relaxed),
            crate::power::LED_OUTPUT_MA.load(Ordering::Relaxed),
            crate::power::LED_BUDGET_MA.load(Ordering::Relaxed),
            crate::power::LED_SCALE_PERMILLE.load(Ordering::Relaxed),
            crate::heap_internal_free(),
            crate::heap_external_free(),
        );
        Timer::after(Duration::from_secs(30)).await;
    }
}
