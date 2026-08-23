use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use alloc::vec::Vec;
use defmt::{info, warn, Debug2Format};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Instant;
use libm::sinf;
use wasmi::{
    Caller, Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder,
    TrapCode, TypedFunc,
};

use crate::audio::AudioFeatures;
use crate::input;
use crate::sketches::{Sketch, SketchParams};
use crate::util::{LED_POSITIONS, NUM_LEDS, Rgb};

/// When false, the i2s task keeps draining DMA but skips the FFT/feature
/// pipeline, freeing that CPU for a guest that declared no audio caps.
pub static AUDIO_ENABLED: AtomicBool = AtomicBool::new(true);

pub const CAP_AUDIO_FEATURES: u32 = 1;
pub const CAP_AUDIO_BINS: u32 = 2;
pub const CAP_WANTS_INPUT: u32 = 4;

pub const ABI_VERSION: u32 = 1;
pub const NUM_BINS: usize = 256;
pub const FRAME_LEN: usize = NUM_LEDS * 3;

// Input block, little-endian:
//   0  u32      abi version
//   4  f32      t
//   8  f32      dt
//   12 i32      encoder position (absolute)
//   16 i32      encoder detents this frame
//   20 u8       buttons: bit0 = held, bit1 = released edge
//   21 u8[3]    pad
//   24 f32[6]   bands        \
//   48 f32[6]   transients    | present when CAP_AUDIO_FEATURES
//   72 f32[6]   sustain       | (zeroed otherwise)
//   96 f32[6]   super_sustain/
//   120 f32[256] fft magnitudes, only written when CAP_AUDIO_BINS
const INPUT_HDR_LEN: usize = 120;
pub const INPUT_LEN: usize = INPUT_HDR_LEN + NUM_BINS * 4;

const AGC_CHANNELS: usize = 16;
const MAX_GUEST_MEMORY: usize = 1 << 20;
// Per-render budget; instantiation (data segments, start fn) gets a larger one.
// wasmi runs ~8.4 fuel/us on this chip, so this caps a frame at ~18ms of
// execution; the reference demo sketch uses ~52k.
const FUEL_PER_FRAME: u64 = 150_000;
const FUEL_FOR_LOAD: u64 = 100_000_000;

pub struct InputFrame<'a> {
    pub t: f32,
    pub dt: f32,
    pub encoder_pos: i32,
    pub detents: i32,
    pub pressed: bool,
    pub released: bool,
    pub audio: &'a AudioFeatures,
    pub bins: Option<&'a [f32; NUM_BINS]>,
}

#[derive(Clone, Copy, Default)]
struct AgcChannel {
    noise_floor: f32,
    max: f32,
}

impl AgcChannel {
    // Same shape as AudioProcessor's per-band post-processing: slow-rising
    // noise floor with a smoothstep gate above it, then normalization by a
    // slowly decaying running maximum.
    fn step(&mut self, energy: f32) -> f32 {
        if !energy.is_finite() || energy < 0.0 {
            return 0.0;
        }
        const NOISE_FLOOR_RISE: f32 = 0.1 * 0.016;
        if energy < self.noise_floor {
            self.noise_floor = energy;
        } else {
            self.noise_floor = (self.noise_floor + NOISE_FLOOR_RISE).min(0.1);
        }
        let lo = self.noise_floor;
        let hi = lo * 2.0;
        let mut e = energy;
        if e <= lo {
            e = 0.0;
        } else if e < hi {
            let t = ((e - lo) / (hi - lo)).clamp(0.0, 1.0);
            e *= t * t * (3.0 - 2.0 * t);
        }
        if e > self.max {
            self.max = e;
        } else {
            self.max *= 0.9999;
        }
        self.max = self.max.max(0.01);
        (e / self.max).min(1.0)
    }
}

pub struct HostState {
    limits: StoreLimits,
    agc: [AgcChannel; AGC_CHANNELS],
    rng: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, defmt::Format)]
pub enum LoadError {
    Invalid,
    Instantiate,
    NoMemory,
    NoRender,
    BadLayout,
}

/// Parse-and-shape check without instantiating: cheap enough for the upload path.
pub fn validate(bytes: &[u8]) -> Result<(), LoadError> {
    let engine = Engine::default();
    let module = Module::new(&engine, bytes).map_err(|_| LoadError::Invalid)?;
    let mut memory = false;
    let mut render = false;
    let mut input_ptr = false;
    let mut frame_ptr = false;
    for export in module.exports() {
        match export.name() {
            "memory" => memory = true,
            "render" => render = true,
            "input_ptr" => input_ptr = true,
            "frame_ptr" => frame_ptr = true,
            _ => {}
        }
    }
    if !memory {
        return Err(LoadError::NoMemory);
    }
    if !render {
        return Err(LoadError::NoRender);
    }
    if !(input_ptr && frame_ptr) {
        return Err(LoadError::BadLayout);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, defmt::Format)]
pub enum FrameError {
    Overrun,
    Trap,
    Io,
}

pub struct SketchInstance {
    store: Store<HostState>,
    memory: Memory,
    render: TypedFunc<(), ()>,
    preview: Option<TypedFunc<(), ()>>,
    input_ptr: u32,
    frame_ptr: u32,
    pub caps: u32,
}

impl SketchInstance {
    pub fn load(bytes: &[u8], seed: u32) -> Result<SketchInstance, LoadError> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(|e| {
            info!("wasm: module rejected: {}", Debug2Format(&e));
            LoadError::Invalid
        })?;

        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_GUEST_MEMORY)
            .memories(1)
            .tables(1)
            .instances(1)
            .build();
        let mut store = Store::new(
            &engine,
            HostState {
                limits,
                agc: [AgcChannel::default(); AGC_CHANNELS],
                rng: seed | 1,
            },
        );
        store.limiter(|s| &mut s.limits);
        let _ = store.set_fuel(FUEL_FOR_LOAD);

        let mut linker = Linker::<HostState>::new(&engine);
        let _ = linker.func_wrap("phyllo", "led_x", |i: i32| -> f32 {
            LED_POSITIONS.get(i as usize).map_or(0.0, |p| p[0])
        });
        let _ = linker.func_wrap("phyllo", "led_y", |i: i32| -> f32 {
            LED_POSITIONS.get(i as usize).map_or(0.0, |p| p[1])
        });
        let _ = linker.func_wrap(
            "phyllo",
            "agc",
            |mut caller: Caller<'_, HostState>, ch: i32, energy: f32| -> f32 {
                match caller.data_mut().agc.get_mut(ch as usize) {
                    Some(c) => c.step(energy),
                    None => 0.0,
                }
            },
        );
        let _ = linker.func_wrap("phyllo", "rand", |mut caller: Caller<'_, HostState>| -> f32 {
            let s = &mut caller.data_mut().rng;
            let mut x = *s;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *s = x;
            (x >> 8) as f32 / (1u32 << 24) as f32
        });

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| {
                info!("wasm: instantiate failed: {}", Debug2Format(&e));
                LoadError::Instantiate
            })?;

        let memory = instance
            .get_memory(&store, "memory")
            .ok_or(LoadError::NoMemory)?;
        let render = instance
            .get_typed_func::<(), ()>(&store, "render")
            .map_err(|_| LoadError::NoRender)?;
        let preview = instance.get_typed_func::<(), ()>(&store, "preview").ok();

        let caps = match instance.get_typed_func::<(), u32>(&store, "caps") {
            Ok(f) => f.call(&mut store, ()).map_err(|_| LoadError::Instantiate)?,
            Err(_) => 0,
        };
        let input_ptr = Self::call_ptr(&instance, &mut store, "input_ptr")?;
        let frame_ptr = Self::call_ptr(&instance, &mut store, "frame_ptr")?;

        let mem_size = memory.data_size(&store);
        if input_ptr as usize + INPUT_LEN > mem_size || frame_ptr as usize + FRAME_LEN > mem_size {
            return Err(LoadError::BadLayout);
        }

        Ok(SketchInstance {
            store,
            memory,
            render,
            preview,
            input_ptr,
            frame_ptr,
            caps,
        })
    }

    fn call_ptr(
        instance: &wasmi::Instance,
        store: &mut Store<HostState>,
        name: &str,
    ) -> Result<u32, LoadError> {
        instance
            .get_typed_func::<(), u32>(&*store, name)
            .map_err(|_| LoadError::BadLayout)?
            .call(store, ())
            .map_err(|_| LoadError::BadLayout)
    }

    pub fn has_preview(&self) -> bool {
        self.preview.is_some()
    }

    pub fn run_frame(
        &mut self,
        input: &InputFrame,
        out: &mut [u8; FRAME_LEN],
        preview: bool,
    ) -> Result<(), FrameError> {
        let mut buf = [0u8; INPUT_LEN];
        let len = self.encode_input(input, &mut buf);
        self.memory
            .write(&mut self.store, self.input_ptr as usize, &buf[..len])
            .map_err(|_| FrameError::Io)?;

        let _ = self.store.set_fuel(FUEL_PER_FRAME);
        let f = if preview {
            self.preview.unwrap_or(self.render)
        } else {
            self.render
        };
        match f.call(&mut self.store, ()) {
            Ok(()) => {}
            Err(e) if e.as_trap_code() == Some(TrapCode::OutOfFuel) => {
                return Err(FrameError::Overrun)
            }
            Err(_) => return Err(FrameError::Trap),
        }

        self.memory
            .read(&self.store, self.frame_ptr as usize, out)
            .map_err(|_| FrameError::Io)?;
        Ok(())
    }

    fn encode_input(&self, input: &InputFrame, buf: &mut [u8; INPUT_LEN]) -> usize {
        buf[0..4].copy_from_slice(&ABI_VERSION.to_le_bytes());
        buf[4..8].copy_from_slice(&input.t.to_le_bytes());
        buf[8..12].copy_from_slice(&input.dt.to_le_bytes());
        buf[12..16].copy_from_slice(&input.encoder_pos.to_le_bytes());
        buf[16..20].copy_from_slice(&input.detents.to_le_bytes());
        buf[20] = input.pressed as u8 | (input.released as u8) << 1;
        buf[21..24].fill(0);
        let mut i = 24;
        if self.caps & CAP_AUDIO_FEATURES != 0 {
            let a = input.audio;
            for arr in [&a.bands, &a.transients, &a.sustain, &a.super_sustain] {
                for v in arr {
                    buf[i..i + 4].copy_from_slice(&v.to_le_bytes());
                    i += 4;
                }
            }
        } else {
            buf[24..INPUT_HDR_LEN].fill(0);
        }
        if self.caps & CAP_AUDIO_BINS != 0 {
            let mut i = INPUT_HDR_LEN;
            if let Some(bins) = input.bins {
                for v in bins {
                    buf[i..i + 4].copy_from_slice(&v.to_le_bytes());
                    i += 4;
                }
            } else {
                buf[INPUT_HDR_LEN..].fill(0);
            }
            INPUT_LEN
        } else {
            INPUT_HDR_LEN
        }
    }
}

static DEMO_MODULE: &[u8] = include_bytes!("../tools/testmod/testmod.wasm");

static PENDING: Mutex<CriticalSectionRawMutex, RefCell<Option<Vec<u8>>>> =
    Mutex::new(RefCell::new(None));

/// Hand a module to the wasm sketch slot; it loads it on its next frame.
pub fn submit_module(bytes: Vec<u8>) {
    PENDING.lock(|p| *p.borrow_mut() = Some(bytes));
}

pub fn preload_demo() {
    submit_module(DEMO_MODULE.to_vec());
}

const OVERRUN_LIMIT: u32 = 240;

pub struct WasmSketch {
    inst: Option<SketchInstance>,
    frame: [u8; FRAME_LEN],
    dead: bool,
    overruns: u32,
}

impl Default for WasmSketch {
    fn default() -> Self {
        Self {
            inst: None,
            frame: [0; FRAME_LEN],
            dead: false,
            overruns: 0,
        }
    }
}

impl WasmSketch {
    pub fn wants_input(&self) -> bool {
        !self.dead
            && self
                .inst
                .as_ref()
                .is_some_and(|i| i.caps & CAP_WANTS_INPUT != 0)
    }

    fn breath(leds: &mut [Rgb; NUM_LEDS], t: f32, color: Rgb) {
        let breath = (sinf(t * 1.5) + 1.0) * 0.5;
        for led in leds.iter_mut() {
            *led = Rgb::new(0, 0, 0).lerp(color, 0.1 + 0.25 * breath);
        }
    }
}

impl Sketch for WasmSketch {
    fn render(&mut self, leds: &mut [Rgb; NUM_LEDS], p: &SketchParams) {
        if let Some(bytes) = PENDING.lock(|pending| pending.borrow_mut().take()) {
            let seed = p.t.to_bits() ^ input::ENCODER_POS.load(Ordering::Relaxed) as u32;
            match SketchInstance::load(&bytes, seed) {
                Ok(inst) => {
                    info!("wasm: loaded module ({} bytes, caps {:#x})", bytes.len(), inst.caps);
                    self.inst = Some(inst);
                    self.dead = false;
                    self.overruns = 0;
                }
                Err(e) => {
                    warn!("wasm: load failed: {}", e);
                    self.inst = None;
                    self.dead = true;
                }
            }
        }

        let Some(inst) = self.inst.as_mut() else {
            let color = if self.dead { Rgb::new(255, 0, 0) } else { Rgb::new(160, 0, 255) };
            Self::breath(leds, p.t, color);
            AUDIO_ENABLED.store(true, Ordering::Relaxed);
            return;
        };

        if self.dead {
            AUDIO_ENABLED.store(true, Ordering::Relaxed);
            Self::breath(leds, p.t, Rgb::new(255, 0, 0));
            return;
        }
        AUDIO_ENABLED.store(
            inst.caps & (CAP_AUDIO_FEATURES | CAP_AUDIO_BINS) != 0,
            Ordering::Relaxed,
        );

        let frame = InputFrame {
            t: p.t,
            dt: p.dt,
            encoder_pos: input::ENCODER_POS.load(Ordering::Relaxed),
            detents: p.detents,
            pressed: p.pressed,
            released: p.released,
            audio: &p.audio,
            bins: None,
        };
        match inst.run_frame(&frame, &mut self.frame, false) {
            Ok(()) => self.overruns = 0,
            Err(FrameError::Overrun) => {
                self.overruns += 1;
                if self.overruns >= OVERRUN_LIMIT {
                    warn!("wasm: sketch too slow, disabling");
                    self.dead = true;
                }
            }
            Err(e) => {
                warn!("wasm: sketch died: {}", e);
                self.dead = true;
            }
        }
        for (i, led) in leds.iter_mut().enumerate() {
            *led = Rgb::new(self.frame[3 * i], self.frame[3 * i + 1], self.frame[3 * i + 2]);
        }
    }
}

pub fn spike() {
    let t0 = Instant::now();
    let mut inst = match SketchInstance::load(DEMO_MODULE, 0x1234_5678) {
        Ok(i) => i,
        Err(e) => {
            warn!("wasm spike: load failed: {}", e);
            return;
        }
    };
    let load_us = t0.elapsed().as_micros();

    let audio = AudioFeatures::default();
    let input = InputFrame {
        t: 1.0,
        dt: 1.0 / 120.0,
        encoder_pos: 0,
        detents: 0,
        pressed: false,
        released: false,
        audio: &audio,
        bins: None,
    };
    let mut out = [0u8; FRAME_LEN];

    let t0 = Instant::now();
    let mut result = Ok(());
    for _ in 0..10 {
        result = inst.run_frame(&input, &mut out, false);
        if result.is_err() {
            break;
        }
    }
    let us = t0.elapsed().as_micros() / 10;
    // run_frame refills fuel each call, so what's missing now is one frame's cost.
    let fuel = FUEL_PER_FRAME.saturating_sub(inst.store.get_fuel().unwrap_or(0));

    info!(
        "wasm spike: load {}us, caps {:#x}, frame = {} in {}us ({} fuel), led0 = ({}, {}, {}), psram free {}",
        load_us,
        inst.caps,
        Debug2Format(&result),
        us,
        fuel,
        out[0],
        out[1],
        out[2],
        crate::heap_external_free()
    );
}
