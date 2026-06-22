# rust-analyzer on this ESP32-S3 (Xtensa) project

How to get working `rust-analyzer` (hover, goto-definition, diagnostics) for `esp_hal`
and the rest of the dependency tree. Editor used here is Neovim, but the root cause and
the core fix are editor-independent.

## Symptom

- Your own crate (`src/*`) resolves and autocompletes fine.
- **Every** `esp_hal` symbol is dead: hover says *"No information available"*, goto-def says
  *"No locations found"* — for all of esp-hal, not just one module.

## Root cause

rust-analyzer loads the dependency graph by running `cargo metadata`. Recent rust-analyzer
invokes it with **`--lockfile-path <tmp> -Zunstable-options`** (so it doesn't touch your
`Cargo.lock`). It decides to pass that flag based on the cargo **version string**.

The esp toolchain's cargo reports `1.95.0-nightly` but is a fork that **does not accept
`--lockfile-path`**:

```
error: unexpected argument '--lockfile-path' found
```

So rust-analyzer is fooled by the version, `cargo metadata` fails, and rust-analyzer
**silently falls back to `--no-deps`** — which drops every dependency from the crate graph.
That is why the local crate works but all of `esp_hal` is invisible.

Diagnose it yourself with:

```sh
rust-analyzer analysis-stats . 2>&1 | grep -i lockfile
# WARN `cargo metadata` failed ... returning succeeded result with `--no-deps`
# error: unexpected argument '--lockfile-path' found
```

> Note: rust-analyzer invokes cargo by an absolute path with `RUSTUP_TOOLCHAIN` pre-set, so
> overriding `$CARGO` or `$PATH` does **not** redirect it. The toolchain's `cargo` binary is
> the only interception point.

## The fix

### 1. Shim the esp toolchain's `cargo` to drop the unsupported flag (the essential fix)

`--lockfile-path` only exists to keep rust-analyzer from touching `Cargo.lock`; stripping it
just reverts to in-place metadata, which is harmless. The shim is transparent to normal
builds (they never pass the flag).

```sh
ESPBIN=~/.rustup/toolchains/esp/bin
mv "$ESPBIN/cargo" "$ESPBIN/cargo-real"          # keep the real cargo
cat > "$ESPBIN/cargo" <<'EOF'
#!/usr/bin/env bash
# Strip `--lockfile-path <val>` (unsupported by the esp cargo fork) then forward
# everything else to the real cargo. Lets a newer rust-analyzer's `cargo metadata`
# succeed so dependencies (esp-hal, ...) load. Transparent to normal builds.
here="$(cd "$(dirname "$0")" && pwd)"
args=(); skip=0
for a in "$@"; do
  if [ "$skip" = 1 ]; then skip=0; continue; fi
  case "$a" in
    --lockfile-path)   skip=1; continue ;;
    --lockfile-path=*) continue ;;
    *) args+=("$a") ;;
  esac
done
exec "$here/cargo-real" "${args[@]}"
EOF
chmod +x "$ESPBIN/cargo"
```

Verify: `rust-analyzer analysis-stats .` no longer prints the lockfile warning, and
`cargo metadata --filter-platform xtensa-esp32s3-none-elf ...` resolves ~149 packages
including `esp-hal`.

**Revert:** `rm "$ESPBIN/cargo" && mv "$ESPBIN/cargo-real" "$ESPBIN/cargo"`

### 2. Point rust-analyzer at the esp proc-macro server

espup ships **no** `rust-analyzer` binary, but it does ship a matching proc-macro server at
`~/.rustup/toolchains/esp/libexec/rust-analyzer-proc-macro-srv`. Use your normal
rust-analyzer as the engine and only repoint the proc-macro server, otherwise esp-hal's
proc-macros (`esp_app_desc!`, `#[esp_rtos::main]`, ...) fail to expand:

```jsonc
// rust-analyzer settings
"rust-analyzer.procMacro.server": "<home>/.rustup/toolchains/esp/libexec/rust-analyzer-proc-macro-srv"
```

### 3. Make sure the analysis target is Xtensa

rust-analyzer reads `build.target = "xtensa-esp32s3-none-elf"` from `.cargo/config.toml`
(already committed), so it analyzes for the chip rather than the host — without this,
esp-hal's arch/chip-`cfg`'d code (and whole modules like `rmt`, gated on build-script cfgs
such as `rmt_driver_supported`) is compiled out. Setting it explicitly is belt-and-braces:

```jsonc
"rust-analyzer.cargo.target": "xtensa-esp32s3-none-elf",
"rust-analyzer.cargo.buildScripts.enable": true,   // default; needed for cfg-gated modules
"rust-analyzer.check.allTargets": false             // host test/bench can't build for no_std/Xtensa
```

For the Neovim implementation that applies §2–§3 **only** to esp projects (detected via
`channel = "esp"` in `rust-toolchain.toml`), see the `rust_analyzer` block in
`~/.config/nvim/lua/plugins/lsp.lua`.

## Maintenance caveat

`espup` / `rustup` reinstalling the esp toolchain will **overwrite the shim** (§1). If
`esp_hal` suddenly goes blank again after a toolchain update, just re-run the §1 script.

This is fundamentally a version skew (esp cargo behind upstream on `--lockfile-path`). The
clean long-term resolutions are a newer esp toolchain whose cargo accepts the flag, or an
upstream rust-analyzer fix — until then, the shim holds.
```
