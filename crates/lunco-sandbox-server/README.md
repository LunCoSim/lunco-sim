# lunco-sandbox-server

Headless launcher for the LunCo sandbox.

The **same application** as the `sandbox` GUI bin — it links the same
`lunco-sandbox` library — but built without the GUI (no winit / egui) and with
the API + networking host. `src/main.rs` is a 3-line launcher that calls
`lunco_sandbox::run_headless()`.

```rust
fn main() -> lunco_sandbox::AppExit {
    lunco_sandbox::run_headless()
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
cargo run -p lunco-sandbox-server     # headless, NO flags needed
```

A windowed build of this same bin is available for symmetry/debugging, but for
the GUI you'd normally just run `-p lunco-sandbox`.
