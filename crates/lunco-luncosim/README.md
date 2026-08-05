# lunco-luncosim

The ground-physics **luncosim** test bed for LunCoSim — ground mobility + physics,
loaded from USD: a USD scene + Avian physics + the in-scene edit tools, exposed as
the `luncosim` binary. It is the composition root that aggregates the domain crates
(`lunco-core`, `lunco-celestial`, `lunco-mobility`, `lunco-usd`, `lunco-controller`,
`lunco-environment`, terrain, scripting, …) into a runnable app. (The full mission
simulator is the separate `luncosim` crate; the headless variant is
`lunco-luncosim-server`.)

## What This Crate Does

The app lives in `src/lib.rs` as `pub fn run()` / `run_headless()`, the single
shared entry point for both the windowed GUI and the headless server. It is
built from three named plugins composed by a tiny shell:

- **`SandboxCorePlugin`** — sim / physics / cosim / USD / networking / API.
  Headless-safe, added unconditionally.
- **`ui::SandboxUiPlugin`** (`ui` feature) — egui workbench, picking, the
  in-scene editor, materials, panels, fallback camera. Added only when windowed.
- **`SandboxHeadlessPlugin`** — the `ScheduleRunner` plus the Modelica/spawn
  cores a server needs in the UI plugin's place. Added only when headless.

GUI = `SandboxCorePlugin + SandboxUiPlugin`; headless =
`SandboxCorePlugin + SandboxHeadlessPlugin`. Both binaries compose the SAME
`SandboxCorePlugin`, so they can never drift.

## Binaries

`cargo run -p lunco-luncosim` runs the LunCoSim GUI (the `luncosim` bin in
`src/bin/luncosim.rs`, which just calls `lunco_luncosim::run()`). The headless
`luncosim-server` bin lives in the sibling `lunco-luncosim-server` crate and calls
`run_headless()`.

| Name | Purpose |
|---|---|
| `luncosim` | The windowed GUI app |
| `luncosim test` | The headless runner for authored USD + Rhai scene tests (`scripts/run_scene_tests.sh`) |

## Project Hierarchy

`lunco-luncosim` serves as an **Integration Layer** (Level 5) in the project hierarchy.

- **Level 1 (Foundation)**: `lunco-core`, `lunco-assets`
- **Level 2 (Domain Logic)**: `lunco-celestial`, `lunco-mobility`, `lunco-usd`
- **Level 3 (Software)**: `lunco-obc`, `lunco-controller`
- **Level 4 (Workflow)**: `lunco-ui`, `lunco-workbench`
- **Level 5 (Application)**: `lunco-luncosim` (this crate), `luncosim`, `lunco-luncosim-server`

## Features

- `ui` (default) — winit windowing backend, render-effect features, and every
  UI crate (egui workbench, material/blueprint editors, doc/theme/ui).
- `lunco-api` (default) — compiles the API in; native HTTP transport.
- `networking` (opt-in) — multiplayer over WebTransport (lightyear). Enable it
  explicitly with `--features networking`; ordinary GUI and test runs do not
  bind multiplayer ports.
- `server` — lean headless build: API + networking host, NO `ui`. Build with
  `--no-default-features --features server`. Skips `celestial`.
- `celestial` — bundled Earth texture + Artemis-II ephemeris (10s of MB).
- `recording`, `tracy`, `net-diag`, `drive-diag` — opt-in diagnostics/tools.

The simulation-facing asset and component features (`mesh`/`light`/`window`)
stay enabled in headless builds. Headless mode omits `RenderPlugin` and its
render-world consumers while USD visual sync remains available to the simulator.

## Builds

```bash
# Windowed GUI (single-player by default; HTTP API remains opt-in at runtime via --api)
cargo run -p lunco-luncosim --bin luncosim

# Windowed GUI with multiplayer support
cargo run -p lunco-luncosim --bin luncosim --features networking

# Lean headless multiplayer server
cargo build -p lunco-luncosim --bin luncosim --no-default-features --features server

# Web (single desktop+web source via lib.rs run())
./scripts/build_web.sh build luncosim   # served at dist/luncosim/
```

The wasm build sets its own feature set (`--no-default-features`), with
`#[cfg(target_arch = "wasm32")]` blocks in the lib handling JS interop, panic
hooks, RNG, and the `?workspace=…&open=…` URL boot path.

## Notes

- Native uses mimalloc as the global allocator (set in `lib.rs`) to avoid
  glibc's global-lock contention against avian's contact-graph rebuild.
- The workspace bevy baseline is `default-features = false`, so
  `reflect_auto_register` is OFF (it overflowed clang's link command line).
  Scene component types are explicitly registered by `UsdBevyPlugin` — see
  `crates/lunco-usd-bevy/src/lib.rs`.
- `luncosim://` deep-link scheme handling + single-instance gate is native +
  `networking` only; filesystem writes route through `lunco-storage`.
