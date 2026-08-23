# Wasm sketch ABI (v1)

A sketch is a wasm module (any language; `tools/testmod/` is a Rust example).
The host runs it under wasmi with fuel metering; a trap or sustained overruns
disable the sketch without affecting the board.

## Guest exports

| export        | type         | required | notes                                    |
|---------------|--------------|----------|------------------------------------------|
| `memory`      | memory       | yes      |                                          |
| `render`      | `() -> ()`   | yes      | called once per frame (120fps target)    |
| `input_ptr`   | `() -> i32`  | yes      | address of the input block               |
| `frame_ptr`   | `() -> i32`  | yes      | address of the 270-byte RGB frame buffer |
| `caps`        | `() -> u32`  | no       | capability bits, default 0               |
| `preview`     | `() -> ()`   | no       | gallery preview; falls back to `render`  |

## Capability bits

- `1` AUDIO_FEATURES — input block carries bands/transients/sustain/super_sustain
- `2` AUDIO_BINS — input block carries 256 raw FFT magnitudes
- `4` WANTS_INPUT — encoder + button belong to the sketch; exiting requires a
  5s button hold (otherwise any click exits)

No audio bits at all parks the FFT pipeline, freeing CPU for the sketch.

## Input block (little-endian, written by host before each `render`)

| offset | type      | field                                   |
|--------|-----------|-----------------------------------------|
| 0      | u32       | ABI version (1)                         |
| 4      | f32       | t (seconds)                             |
| 8      | f32       | dt                                      |
| 12     | i32       | encoder position (absolute)             |
| 16     | i32       | encoder detents this frame              |
| 20     | u8        | buttons: bit0 held, bit1 released edge  |
| 21     | u8[3]     | pad                                     |
| 24     | f32[6]    | bands            (AUDIO_FEATURES)       |
| 48     | f32[6]    | transients       (AUDIO_FEATURES)       |
| 72     | f32[6]    | sustain          (AUDIO_FEATURES)       |
| 96     | f32[6]    | super_sustain    (AUDIO_FEATURES)       |
| 120    | f32[256]  | FFT magnitudes   (AUDIO_BINS)           |

Frame buffer: 90 LEDs x 3 bytes RGB. The host gamma-corrects after readback.

## Host imports (module `"phyllo"`)

- `led_x(i: i32) -> f32`, `led_y(i: i32) -> f32` — LED positions, unit circle
- `agc(channel: i32, energy: f32) -> f32` — stateful normalizer (noise-floor
  tracking + decaying max, same shape as the native audio pipeline); 16
  channels per instance, call once per channel per frame
- `rand() -> f32` — uniform [0, 1)

## Limits

- module ≤ 32704 bytes (32KB slot minus header)
- linear memory ≤ 1MB
- 4M fuel per frame; overrun skips the frame, ~2s of consecutive overruns
  disables the sketch ("too slow" red breath)

## HTTP API

```sh
curl http://<ip>/api/sketches                                  # list slots
curl -X POST --data-binary @sketch.wasm \
     http://<ip>/api/sketches/<slot>/<name>                    # upload (validated)
curl -X POST http://<ip>/api/run/<slot>                        # load + switch to it
curl -X DELETE http://<ip>/api/sketches/<slot>                 # free a slot
```

Sketches persist in the `sketches` flash partition (see `partitions.csv`;
flash once with the new table via `cargo run --release`).
