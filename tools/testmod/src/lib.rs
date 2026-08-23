#![no_std]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

const NUM_LEDS: usize = 90;
const INPUT_LEN: usize = 132;

const CAP_AUDIO_FEATURES: u32 = 1;

static mut INPUT: [u8; INPUT_LEN] = [0; INPUT_LEN];
static mut FRAME: [u8; NUM_LEDS * 3] = [0; NUM_LEDS * 3];

#[link(wasm_import_module = "phyllo")]
unsafe extern "C" {
    fn led_x(i: i32) -> f32;
    fn led_y(i: i32) -> f32;
}

#[unsafe(no_mangle)]
pub extern "C" fn abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn caps() -> u32 {
    CAP_AUDIO_FEATURES
}

#[unsafe(no_mangle)]
pub extern "C" fn input_ptr() -> i32 {
    core::ptr::addr_of!(INPUT) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn frame_ptr() -> i32 {
    core::ptr::addr_of!(FRAME) as i32
}

fn input_u64(off: usize) -> u64 {
    let b = unsafe { &*core::ptr::addr_of!(INPUT) };
    u64::from_le_bytes([
        b[off], b[off + 1], b[off + 2], b[off + 3],
        b[off + 4], b[off + 5], b[off + 6], b[off + 7],
    ])
}

fn hsv(h: f32, v: f32) -> [u8; 3] {
    let h = (h - libm::floorf(h)) * 6.0;
    let i = h as i32;
    let f = h - i as f32;
    let (r, g, b) = match i {
        0 => (1.0, f, 0.0),
        1 => (1.0 - f, 1.0, 0.0),
        2 => (0.0, 1.0, f),
        3 => (0.0, 1.0 - f, 1.0),
        4 => (f, 0.0, 1.0),
        _ => (1.0, 0.0, 1.0 - f),
    };
    [(r * v * 255.0) as u8, (g * v * 255.0) as u8, (b * v * 255.0) as u8]
}

// Smooth rotating rainbow spiral. Audio remains enabled in the host so this
// module is also a cadence test while the FFT pipeline is running.
#[unsafe(no_mangle)]
pub extern "C" fn render() {
    let elapsed_us = input_u64(8);
    let hue_phase = (elapsed_us % 20_000_000) as f32 / 20_000_000.0;
    let wave_phase = (elapsed_us % 2_094_395) as f32 / 2_094_395.0
        * 2.0 * core::f32::consts::PI;
    let frame = unsafe { &mut *core::ptr::addr_of_mut!(FRAME) };
    for i in 0..NUM_LEDS {
        let (x, y) = unsafe { (led_x(i as i32), led_y(i as i32)) };
        let r = libm::sqrtf(x * x + y * y);
        let th = libm::atan2f(y, x);
        let h = th / (2.0 * core::f32::consts::PI) + hue_phase + r * 0.3;
        let wave = 0.5 + 0.5 * libm::sinf(r * 9.0 - wave_phase);
        let v = 0.2 + 0.8 * wave;
        let c = hsv(h, v);
        frame[3 * i] = c[0];
        frame[3 * i + 1] = c[1];
        frame[3 * i + 2] = c[2];
    }
}
