# Wasm sketch ABI (v2)

A sketch is a wasm module (any language; `tools/testmod/` is a Rust example).
The host runs it under wasmi with fuel metering; a trap or sustained overruns
disable the sketch without affecting the board.

## Guest exports

| export        | type         | required | notes                                    |
|---------------|--------------|----------|------------------------------------------|
| `abi_version` | `() -> u32`  | yes      | must return `2`                          |
| `memory`      | memory       | yes      |                                          |
| `render`      | `() -> ()`   | yes      | called once per frame (60fps target)     |
| `input_ptr`   | `() -> i32`  | yes      | address of the input block               |
| `frame_ptr`   | `() -> i32`  | yes      | address of the 270-byte RGB frame buffer |
| `caps`        | `() -> u32`  | no       | capability bits, default 0               |
| `preview`     | `() -> ()`   | no       | gallery preview; falls back to `render`  |

## Capability bits

- `1` AUDIO_FEATURES — input block carries bands/transients/sustain/super_sustain
- `4` WANTS_INPUT — encoder + button belong to the sketch; exiting requires a
  5s button hold (otherwise any click exits)

Without `AUDIO_FEATURES`, the host parks the FFT pipeline while the sketch is
active, freeing CPU for the guest. Raw FFT bins are not part of ABI v2.

## Input block (little-endian, written by host before each `render`)

| offset | type      | field                                   |
|--------|-----------|-----------------------------------------|
| 0      | u32       | ABI version (`2`)                       |
| 4      | u32       | frame delta, microseconds; capped at 50,000 |
| 8      | u64       | elapsed microseconds since host render start |
| 16     | u64       | rendered frame index                    |
| 24     | i32       | encoder position (absolute)             |
| 28     | i32       | encoder detents this frame              |
| 32     | u8        | buttons: bit0 held, bit1 released edge  |
| 33     | u8[3]     | pad                                     |
| 36     | f32[6]    | bands            (AUDIO_FEATURES)       |
| 60     | f32[6]    | transients       (AUDIO_FEATURES)       |
| 84     | f32[6]    | sustain          (AUDIO_FEATURES)       |
| 108    | f32[6]    | super_sustain    (AUDIO_FEATURES)       |

The input block is 132 bytes. Guests should reduce integer elapsed time into a
bounded animation phase before converting it to `f32`; converting the full
uptime to `f32` reintroduces long-running precision loss.

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
- 150K fuel per frame; overrun skips the frame, ~2s of consecutive overruns
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
