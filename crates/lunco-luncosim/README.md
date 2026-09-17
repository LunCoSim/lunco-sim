# lunco-luncosim

The thin **luncosim** process shell for LunCoSim. It dispatches headless mode
to `lunco-luncosim-core` and GUI mode to `lunco-luncosim-ui`. The headless
server depends on the core package directly.

## What This Crate Does

The app lives in `src/lib.rs` as the process entry point `pub fn run()`. Headless
launching is owned by `lunco-luncosim-core`; this package only dispatches to it
for `--no-ui` and `LUNCO_NO_UI`. Window/render composition and presentation
live in `lunco-luncosim-ui`, which owns:

- **`lunco_luncosim_core::LunCoSimCorePlugin`** — sim / physics / cosim / USD /
  networking / API. Headless-safe and shared with the server.
- **`lunco_luncosim_ui::run_gui`** and **`LunCoSimUiPlugin`** — window/render
  setup, egui workbench, picking, the
  in-scene editor, materials, panels, and authored-camera presentation. Added
  only when windowed; a scene without an authored camera contract remains
  visibly camera-less with an owning diagnostic rather than receiving an
  engine-created camera. USD loading completion is independent of presentation.

GUI = `lunco_luncosim_ui::run_gui` composing `LunCoSimCorePlugin + LunCoSimUiPlugin`. The server and
`lunco-scene-runner` use `LunCoSimCorePlugin + LunCoSimHeadlessPlugin` from the
core package.

## Binaries

`cargo run -p lunco-luncosim` runs the LunCoSim GUI (the `luncosim` bin in
`src/bin/luncosim.rs`, which calls `lunco_luncosim::run()`). Its `test` and
`test-component` subcommands delegate to the production `lunco-scene-runner`
package. The headless
`luncosim-server` bin lives in the sibling `lunco-luncosim-server` crate and
calls `lunco_luncosim_core::run_headless()`.

The `luncosim rhai` subcommand is a terminal adapter from
`lunco-rhai-repl`. It never embeds a Rhai engine: the running simulator
executes the reflected `RunRhai` command, while `lunco-api-client` owns the
native API transport. Use `--api PORT` for the documented loopback endpoint or
`--api-url URL` for an explicitly configured HTTP API base URL. Build with the
`rhai-tls` feature when that URL uses HTTPS.

| Name | Purpose |
|---|---|
| `luncosim` | The windowed GUI app |
| `luncosim test` | The headless runner for authored USD + Rhai scene tests (`scripts/run_scene_tests.sh`) |
| `luncosim test-component` | Select one manifest-owned component and run its declared USD/Rhai verification harness |

For a long-running, render-free host, use `luncosim-server`. Add
`--headless-max-speed` to run that same production simulation loop without a
wall-clock wait; it advances one fixed simulation duration per update and still
honours the co-simulation barrier. The deterministic `luncosim test` runner is
already manually clocked and runs at the speed the CPU permits.

`test-component` is the component-level entry point:

```bash
luncosim test-component --twin ./my-twin --component mobility.wheel
```

The command resolves `[[components]]` in `twin.toml`, validates its requirement
and verification bindings, then delegates to the same deterministic runner as
`test --scene`. The selected Rhai verification owns component-specific asset
loading and observations through the typed USD authoring/query tools; the CLI
does not duplicate component geometry or requirements.

## Project Hierarchy

`lunco-luncosim` serves as an **Integration Layer** (Level 5) in the project hierarchy.

- **Level 1 (Foundation)**: `lunco-core`, `lunco-assets-core`
- **Level 2 (Domain Logic)**: `lunco-celestial`, `lunco-mobility`, `lunco-usd-commands`
- **Level 3 (Software)**: `lunco-obc`, `lunco-controller`
- **Level 4 (Workflow)**: `lunco-ui`, `lunco-workbench`
- **Level 5 (Application)**: `lunco-luncosim-core`, `lunco-luncosim` (this crate),
  `luncosim`, `lunco-luncosim-server`

## Features

- `ui` (default) — winit windowing backend, render-effect features, and every
  UI crate (egui workbench, material/blueprint editors, doc/theme/ui).
- `api-transport` (default) — compiles the API contracts and native HTTP transport in.
- `networking` (opt-in) — multiplayer over WebTransport (lightyear). Enable it
  explicitly with `--features networking`; ordinary GUI and test runs do not
  bind multiplayer ports.
- `server` — lean headless build: API + networking host, NO `ui`. Build with
  `--no-default-features --features server`. Skips `celestial`.
- `celestial` — bundled Earth texture + Artemis-II ephemeris (10s of MB).
- `recording`, `tracy`, `net-diag` — opt-in diagnostics/tools.

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

- Native uses mimalloc as the global allocator in `lunco-luncosim-core` to avoid
  glibc's global-lock contention against avian's contact-graph rebuild.
- The workspace bevy baseline is `default-features = false`, so
  `reflect_auto_register` is OFF (it overflowed clang's link command line).
  Scene component types are explicitly registered by `UsdVisualPlugin` — see
  `crates/lunco-usd-bevy/src/lib.rs`.
- `luncosim://` deep-link scheme handling + single-instance gate is native +
  `networking` only; filesystem writes route through `lunco-storage`.
