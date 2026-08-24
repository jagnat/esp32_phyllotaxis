# Phyllotaxis WASM sketch starter

Edit `src/sketch.rs`; the raw host bindings and ABI glue are already wrapped in
safe types. The board renders 90 RGB LEDs at a 60fps target.

## Start

```sh
cp .phyllo.env.example .phyllo.env
chmod 600 .phyllo.env
```

Rustup reads `rust-toolchain.toml` and installs stable Rust plus the WASM target.
Put the board hostname or IP in `.phyllo.env`. That file is ignored by Git. You
can instead point `PHYLLO_ENV_FILE` at another environment file. Pick an empty
slot from `./phyllo list`, then:

```sh
./phyllo upload
./phyllo run
```

Other commands are `build`, `health`, `list`, `delete`, and `main`. Uploading
again to the same slot atomically replaces that sketch; deleting erases it.

## Sketch API

`render(input, frame)` is called at about 60fps:

- `frame.set(index, Rgb)` and `frame.fill(Rgb)` write pixels;
- `phyllo::position(index)` gives physical x/y coordinates on the unit disk;
- `input.phase(period_us)` makes year-safe looping animation time;
- `input.dt_us()`, `elapsed_us()`, and `frame_index()` expose timing;
- `input.audio()` exposes four arrays of six audio bands when AUDIO is enabled;
- encoder/button methods work when INPUT is enabled;
- `phyllo::hsv`, `agc`, and `random` provide common utilities.
- `phyllo::return_to_main()` hands control back to the unattended display.

Set `CAPABILITIES` to `NONE`, `AUDIO`, `INPUT`, or `AUDIO | INPUT`. A sketch
with INPUT requires a five-second button hold to exit; otherwise a click exits.

The host gamma-corrects and power-limits the finished RGB frame. Keep release
modules at or below 32,704 bytes and rendering within the 120K fuel budget.

## Storage identity

The persistent ID is the numeric slot, 0 through 99. A slot also has a display
name and menu color. Names are metadata rather than unique IDs. Every occupied
slot appears as its own physical-menu item. `./phyllo list` returns every
occupied slot; `upload` replaces the configured slot, and `delete` removes it.
