# lunco-sandbox

The LunCo sandbox application — ground mobility + physics, loaded from USD. This
is the composition root that aggregates the domain crates (`lunco-core`,
`lunco-celestial`, `lunco-mobility`, `lunco-usd`, `lunco-controller`,
`lunco-environment`, terrain, scripting, …) into a runnable app.

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

`cargo run -p lunco-sandbox` runs the GUI sandbox (the `sandbox` bin in
`src/bin/sandbox.rs`, which just calls `lunco_sandbox::run()`). The headless
`sandbox-server` bin lives in the sibling `lunco-sandbox-server` crate and calls
`run_headless()`.

Physics test harnesses live in `src/bin/` — run with
`cargo run -p lunco-sandbox --bin <name>`:

| Name | Purpose |
|---|---|
| `sandbox` | The windowed GUI app |
| `rover_jitter` | Headless probe — chassis buzz under drive torque (no `ui`) |
| `rover_turn` | Headless turning/steering probe (no `ui`) |
| `joint_minimal` | Minimal windowed joint test (needs `ui`) |

## Features

- `ui` (default) — winit windowing backend, render-effect features, and every
  UI crate (egui workbench, material/blueprint editors, doc/theme/ui).
- `lunco-api` (default) — compiles the API in; native HTTP transport.
- `networking` (default) — multiplayer over WebTransport (lightyear).
- `server` — lean headless build: API + networking host, NO `ui`. Build with
  `--no-default-features --features server`. Skips `celestial`.
- `celestial` — bundled Earth texture + Artemis-II ephemeris (10s of MB).
- `recording`, `tracy`, `net-diag`, `drive-diag` — opt-in diagnostics/tools.

The render asset-store features (`bevy_pbr`/`mesh`/`light`/`window`) stay always
on even headless: the `--no-ui` server runs `RenderPlugin` in `backends: None`
mode so USD visual sync can populate the meshes avian colliders key off.

## Builds

```bash
# Windowed GUI (HTTP API + multiplayer wire live by default)
cargo run -p lunco-sandbox --bin sandbox

# Lean headless multiplayer server
cargo build -p lunco-sandbox --bin sandbox --no-default-features --features server

# Web (single desktop+web source via lib.rs run())
./scripts/build_web.sh build sandbox   # served at dist/sandbox/
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
