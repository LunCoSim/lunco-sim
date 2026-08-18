# lunco-luncosim-server

Headless launcher for the LunCo luncosim.

The **same application** as the `luncosim` GUI bin — it links the same
`lunco-luncosim` library — but built without the GUI (no winit / egui) and with
the API + networking host. `src/main.rs` is a 3-line launcher that calls
`lunco_luncosim::run_headless()`.

```rust
fn main() -> lunco_luncosim::AppExit {
    lunco_luncosim::run_headless()
}
```

`run_headless()` forces the windowless path (sim + physics + cosim + networking
host, driven by `ScheduleRunnerPlugin`). Forcing the mode — rather than
inferring it from an absent `ui` feature — keeps it headless even if a
`--workspace` build unifies the `ui` feature on.

## Why a separate crate

Cargo default features are **per-package**. A bin that should be
headless-by-default needs its own package: this crate sets
`default-features = false` (dropping `ui` = winit/egui/workbench) and adds the
`server` features (HTTP API + networking host). That is the whole reason it
exists.

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

A windowed build of this same bin is available for symmetry/debugging, but for
the GUI you'd normally just run `-p lunco-luncosim`.
