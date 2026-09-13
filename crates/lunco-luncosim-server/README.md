# lunco-luncosim-server

Headless launcher for the LunCo luncosim.

The same simulation runtime as the `luncosim` GUI, linked through the
headless-safe `lunco-luncosim-core` package. The GUI shell is not a dependency:
no winit, egui, workbench, or render/UI code is compiled for this binary.
`src/main.rs` is a 3-line launcher that calls
`lunco_luncosim_core::run_headless()`.

```rust
fn main() -> lunco_luncosim_core::AppExit {
    lunco_luncosim_core::run_headless()
}
```

`run_headless()` starts the sim + physics + cosim + networking host through
`ScheduleRunnerPlugin`. The core package has no GUI feature to unify.

## Why a separate crate

This package is a deliberately thin launcher around the production
`lunco-luncosim-core` runtime. Keeping the launcher separate makes the server
binary select the core package's empty default feature set while the GUI shell
retains its independent desktop feature set.

```bash
cargo run -p lunco-luncosim-server     # headless, NO flags needed
cargo run -p lunco-luncosim-server -- --headless-max-speed --scene path/to/scene.usda
```

`--headless-max-speed` is a wall-clock execution mode for the production
simulation loop: it uses the same fixed timestep, port propagation, worker
transport, and causal barrier, but does not sleep between updates. It is not a
fake physics-rate multiplier and does not release a participant whose causal
step is still in flight. Use the API `Exit` command to stop a long-running
session, or use `luncosim test` when a bounded deterministic verdict is needed.

For the GUI, run `cargo run -p lunco-luncosim`.
