//! Minimal safe wrappers around Phyllotaxis WASM ABI v1.
#![allow(dead_code)]

pub const ABI_VERSION: u32 = 1;
pub const LED_COUNT: usize = 90;
pub const INPUT_BYTES: usize = 132;
pub const FRAME_BYTES: usize = LED_COUNT * 3;
pub const INPUT_CAPABILITY: u32 = 4;

#[derive(Clone, Copy, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn scale(self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        Self::new(
            (self.r as f32 * amount) as u8,
            (self.g as f32 * amount) as u8,
            (self.b as f32 * amount) as u8,
        )
    }
}

pub struct Frame<'a>(&'a mut [u8; FRAME_BYTES]);

impl<'a> Frame<'a> {
    pub fn new(bytes: &'a mut [u8; FRAME_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn set(&mut self, index: usize, color: Rgb) {
        if index < LED_COUNT {
            self.0[3 * index..3 * index + 3].copy_from_slice(&[color.r, color.g, color.b]);
        }
    }

    pub fn fill(&mut self, color: Rgb) {
        for index in 0..LED_COUNT {
            self.set(index, color);
        }
    }
}

pub struct Input<'a>(&'a [u8; INPUT_BYTES]);

impl<'a> Input<'a> {
    pub fn new(bytes: &'a [u8; INPUT_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn dt_seconds(&self) -> f32 {
        u32::from_le_bytes(self.0[4..8].try_into().unwrap()) as f32 / 1_000_000.0
    }

    pub fn elapsed_us(&self) -> u64 {
        u64::from_le_bytes(self.0[8..16].try_into().unwrap())
    }

    pub fn phase(&self, period_us: u64) -> f32 {
        (self.elapsed_us() % period_us) as f32 / period_us as f32
    }

    pub fn button_released(&self) -> bool {
        self.0[32] & 2 != 0
    }
}

#[derive(Clone, Copy)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[link(wasm_import_module = "phyllo")]
unsafe extern "C" {
    #[link_name = "led_x"]
    fn host_led_x(index: i32) -> f32;
    #[link_name = "led_y"]
    fn host_led_y(index: i32) -> f32;
    #[link_name = "rand"]
    fn host_rand() -> f32;
    #[link_name = "return_to_main"]
    fn host_return_to_main();
}

pub fn position(index: usize) -> Point {
    Point {
        x: unsafe { host_led_x(index as i32) },
        y: unsafe { host_led_y(index as i32) },
    }
}

pub fn random() -> f32 {
    unsafe { host_rand() }
}

pub fn return_to_main() {
    unsafe { host_return_to_main() }
}

pub fn hsv(hue: f32, saturation: f32, value: f32) -> Rgb {
    let h = (hue - libm::floorf(hue)) * 6.0;
    let i = h as i32;
    let f = h - i as f32;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - saturation * f);
    let t = value * (1.0 - saturation * (1.0 - f));
    let (r, g, b) = match i {
        0 => (value, t, p),
        1 => (q, value, p),
        2 => (p, value, t),
        3 => (p, q, value),
        4 => (t, p, value),
        _ => (value, p, q),
    };
    Rgb::new(
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}
