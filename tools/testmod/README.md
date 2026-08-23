# Rust sketch example

Install Rust's WASM target once, then make an optimized module:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release
cp target/wasm32-unknown-unknown/release/phyllo_testmod.wasm testmod.wasm
```

Always use `--release`. The current example is about 6.3 KiB in release mode;
the debug build is several megabytes and exceeds the 32,704-byte upload limit.

The local `.cargo/config.toml` selects the WASM target and a 16 KiB shadow
stack compatible with the host's 1 MiB guest-memory ceiling. See
[`../../docs/WASM_ABI.md`](../../docs/WASM_ABI.md) for the full contract.
