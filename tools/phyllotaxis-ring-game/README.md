# Ring game

A self-contained Phyllotaxis WASM sketch. A white comet circles the outside of
the board; release the button while it overlaps the green target. Passing the
target or pressing elsewhere costs a life. Five hits wins, three misses loses,
and each game resets after its result animation. If all three targets pass with
no button attempt at all, the loss animation returns to the board's idle display.

Build with:

```sh
cargo build --release
```

Then drag `target/wasm32-unknown-unknown/release/phyllotaxis_ring_game.wasm`
onto the board's upload page. Hold the board button for five seconds to leave
the game.
