# esp32_phyllotaxis — port status & plan

Audio-reactive LED project ported from **STM32F411 + RTIC** to **ESP32-S3 + Embassy**.
The original lives in the sibling repo **`../rust_leds`** and is the reference
implementation — paths below (e.g. `../rust_leds/src/bin/minimal.rs:213`) point at the
exact code to translate.

## Stack (as scaffolded)

This project was generated with:

```
esp-generate --chip esp32s3 -o unstable-hal -o embassy -o defmt
```

Resulting toolchain (esp-generate 1.3.0):

| Topic | Value |
|---|---|
| MCU | ESP32-S3 (Xtensa, hardware FPU) |
| Edition / rustc | 2024 / 1.88, `channel = "esp"` (Xtensa fork) |
| HAL | `esp-hal ~1.1.0` (`unstable` feature on) |
| Async | **Embassy via `esp-rtos 0.3`** — `#[esp_rtos::main]`, started with `esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0)`. NOTE: **not** `esp-hal-embassy`/`#[esp_hal_embassy::main]` (older guides reference that). |
| Logging | `defmt` over USB-Serial-JTAG via `esp-println` (`defmt-espflash`), decoded by `espflash`. |
| Panic | plain `#[panic_handler]` in `main.rs` (no `esp-backtrace` in this option set). |
| Target / runner | `xtensa-esp32s3-none-elf`, `espflash flash --monitor` (see `.cargo/config.toml`). |

## Decisions (made — do not re-litigate)

- **LED output:** WS2812 driven by the **RMT peripheral directly** via esp-hal 1.1's `rmt`
  API with a hand-built pulse encoder (one `PulseCode` per bit, GRB order, ≥50 µs reset).
  `esp-hal-smartled` + `smart-leds` were ruled OUT (2026-06): the newest smartled (0.17.0,
  and even git) still pins `esp-hal = "~1.0"`, while `esp-rtos 0.3` / `esp-bootloader 0.5`
  require `esp-hal = "~1.1"`, so they can't coexist and esp-hal can't be downgraded.
  The SPI bit-encoding has already been **deleted** from `src/util.rs`.
- **Audio input:** I2S RX + DMA, INMP441-class mic, 24-bit-in-32-bit Philips frame.
  Keep `decode_left_sample` and the `/ 8388608.0` normalization unchanged
  (reference: `../rust_leds/src/bin/minimal.rs:57`).

## DONE in this repo

- [x] Scaffolded esp32-s3 / embassy / defmt project (builds the blinky once toolchain installed).
- [x] Copied pure DSP modules: `src/audio.rs`, `src/sketches.rs`, `src/util.rs` (unchanged logic).
- [x] `src/util.rs`: removed dead WS2812-SPI path (`TX_BUFFER_SIZE`, `to_spi_bits`,
      `fill_transfer_buffer`, `SPI_PIXEL_LUT`). Kept `Rgb`/`Hsv`/`hsv_to_rgb_rainbow`/
      `apply_gamma`/`GAMMA8`/`lerp`/`add`/`LED_POSITIONS`/`NUM_LEDS`.
- [x] `src/lib.rs`: declares `pub mod util; pub mod audio; pub mod sketches;`.
- [x] `Cargo.toml`: added `microfft = "0.6"`, `libm = "0.2"`.

## TODO (the actual port work)

### 0. Toolchain (prereq, not yet installed)
- `cargo install espup && espup install` (installs the Xtensa Rust fork, ~2 GB).
- `cargo install espflash`.
- `. ~/export-esp.sh` (or per espup instructions) so the `esp` toolchain is on PATH.
- Sanity: `cargo run --release` should flash the blinky and stream `defmt` logs.

### 1. Resolve the remaining deps (uncomment block in `Cargo.toml`)
- `smart-leds` + `esp-hal-smartled` — **verify a release tracks esp-hal ~1.1** before
  committing a version; the smartled crate has lagged esp-hal majors before. If no
  compatible release exists, fall back to driving WS2812 from the RMT directly with
  `esp-hal`'s `rmt` API and a hand-built pulse encoder.
- `embassy-sync` — pick the version matching `embassy-executor 0.10` / `embassy-time 0.5`.
- `bytemuck` (`default-features = false`) for the I2S byte/u16 cast.

### 2. Add the `Rgb -> RGB8` conversion (in `src/util.rs`, once smart-leds is a dep)
```rust
impl From<Rgb> for smart_leds::RGB8 {
    fn from(c: Rgb) -> Self { smart_leds::RGB8::new(c.r, c.g, c.b) }
}
```

### 3. Rewrite the glue in `src/bin/main.rs`
Replace the blinky. Architecture carries over 1:1 from the reference; only HAL calls change.

RTIC → Embassy mapping (reference: `../rust_leds/src/bin/minimal.rs`):

| Reference (`minimal.rs`) | Here |
|---|---|
| `#[rtic::app] mod app` / `#[init]` (7, 79) | already `#[esp_rtos::main] async fn main` + init block |
| `#[task] async fn render` (213) | `#[embassy_executor::task] async fn render(...)` |
| `Mono::delay/delay_until` | `embassy_time::Timer` / `Ticker` |
| `systick_monotonic!` | provided by esp-rtos time driver |
| `#[shared] audio` + `.lock()` (228, 271) | `embassy_sync::signal::Signal<NoopRawMutex, AudioFeatures>` |
| `#[task(binds=DMA1_STREAM3)] i2s_dma` ISR (253) | `async fn audio` task that `await`s I2S DMA reads — no ISR, no triple-buffer dance |
| `#[task(binds=DMA2_STREAM2)] spi_complete` (246) | gone — RMT/`smart-leds` write owns completion |

Target shape (signatures illustrative — confirm against esp-hal 1.1 examples:
<https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples>, esp. `i2s_read` and `rmt`):

```rust
static AUDIO: Signal<NoopRawMutex, AudioFeatures> = Signal::new();

// replaces i2s_dma ISR — DMA completion wakes the task
#[embassy_executor::task]
async fn audio(mut i2s_rx: /* I2sRx */, buf: &'static mut [u16; AUDIO_BUF_LEN]) {
    let mut proc = AudioProcessor::new();
    loop {
        i2s_rx.read_dma_async(bytemuck::cast_slice_mut(buf)).await.unwrap();
        let feats = proc.process(|i| decode_left_sample(buf, i) as f32 / 8388608.0);
        AUDIO.signal(feats);
    }
}

// replaces render task (minimal.rs:213)
#[embassy_executor::task]
async fn render(mut led: /* SmartLedsAdapter */) {
    let mut bank = SketchBank::new();
    let mut led_buf = [Rgb::new(0,0,0); NUM_LEDS];
    let mut last_audio = AudioFeatures::default();
    let mut ticker = Ticker::every(Duration::from_millis(1000 / 120));
    let mut last = Instant::now();
    let mut t = 0.0f32;
    loop {
        if let Some(a) = AUDIO.try_take() { last_audio = a; }
        let now = Instant::now();
        let dt = (now - last).as_micros() as f32 / 1_000_000.0;
        last = now; t += dt;
        let p = SketchParams { dt, t, audio: last_audio, released: false };
        bank.render(&mut led_buf, &p);
        apply_gamma(&mut led_buf);
        led.write(led_buf.iter().copied().map(Into::into)).unwrap();
        ticker.next().await;
    }
}
```

Constants/helpers to bring from `minimal.rs`: `AUDIO_FRAMES`, `AUDIO_BUF_LEN`
(36-37), `decode_left_sample` (57). Put `decode_left_sample` in `audio.rs` or beside
the `audio` task.

### 4. Pins / hardware (assign free GPIOs)
| Signal | F411 ref | ESP32-S3 |
|---|---|---|
| WS2812 data | PA7 | any RMT-capable GPIO (level-shift to 5V if the strip needs it) |
| I2S mic SD (DIN) | PB15 | I2S data-in GPIO |
| I2S mic BCLK | PB10 | I2S BCLK GPIO |
| I2S mic WS/LRCL | PB12 | I2S WS GPIO |

Mic config must match `minimal.rs:133-138`: master RX, Philips, 24-bit data in 32-bit
channel, ~32 kHz, left channel decoded. Tie INMP441 `L/R` to the channel you decode.

## Bring-up checklist (order matters)
1. Blinky flashes; `defmt` logs appear over USB-JTAG.
2. RMT: static red fill (ref `minimal.rs:114`). **Verify color order** — the original packs
   GRB in `to_spi_bits`; `smart-leds` handles order itself, so confirm red==red before trusting sketches.
3. Render loop with audio stubbed to `AudioFeatures::default()`; confirm ~120 fps.
4. I2S audio: log raw `decode_left_sample` values; silence ≈ 0. **Highest-risk step** —
   verify bit depth / channel order / WS polarity against the mic datasheet before trusting the FFT.
5. Full chain; re-tune noise-floor constants (`audio.rs:9-12`) if the S3 floor differs.

## Gotchas
- DMA buffers must live in DMA-capable RAM with esp-hal alignment (use its `dma_buffers!`
  macro); do **not** reuse `cortex_m::singleton!`.
- Anything referencing `cortex_m`, `SYST`, NVIC, or `stm32f4xx_hal::pac` is gone.
- Hardware FPU is on — the f32 FFT runs native, no soft-float concern.
- Watch `#[deny(clippy::large_stack_frames)]` in `main.rs`: the FFT/LED buffers are sizable;
  keep big arrays in `static`/`StaticCell`, not on the task stack.

## Reference index (`../rust_leds`)
- `src/audio.rs` — DSP core (copied as-is)
- `src/sketches.rs` — sketches (copied as-is)
- `src/util.rs` — color/render utils (copied minus SPI encoding)
- `src/bin/minimal.rs` — STM32 glue to translate (init, render, I2S ISR, decode)
- `src/lib.rs` — STM32 panic/logger boilerplate (replaced by this template's `main.rs`)
