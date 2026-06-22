# lunco-client

The primary application crate for LunCoSim, containing executable binaries and high-level application logic.

## What This Crate Does

This crate aggregates all domain-specific crates (`lunco-celestial`, `lunco-mobility`, `lunco-usd`, etc.) into functional application targets.

- **Binary Entrypoints** — Native and Web builds for different simulation scenarios.
- **App Configuration** — Plugin orchestration and global resource initialization.
- **Web Support** — Special handling for `wasm32-unknown-unknown` (JS interop, console hooks, RNG).
- **Environment Integration** — Connects the simulation to the `lunco-api` and `lunco-cosim` orchestration.

## Binaries

| Name | Path | Purpose |
|---|---|---|
| **`sandbox`** | `src/bin/sandbox.rs` | Main sandbox for testing USD-based rover mobility |
| **`lunco_client_web`** | `src/bin/lunco_client_web.rs` | Web-targeted build of the LunCoSim client |
| **`model_viewer`** | `src/bin/model_viewer.rs` | Isolated viewer for USD assets and materials |

## Architecture

`lunco-client` serves as the **Integration Layer** (Level 5) in the project hierarchy.

- **Level 1 (Foundation)**: `lunco-core`, `lunco-assets`
- **Level 2 (Domain Logic)**: `lunco-celestial`, `lunco-mobility`, `lunco-usd`
- **Level 3 (Software)**: `lunco-fsw`, `lunco-obc`, `lunco-controller`
- **Level 4 (Workflow)**: `lunco-ui`, `lunco-workbench`
- **Level 5 (Application)**: `lunco-client` (this crate)

## Web Build

The web target uses `wasm-bindgen` and `web-sys` to bridge Bevy's systems with browser APIs. It requires `getrandom` with the `js` feature for RNG support.

## Usage

```bash
# Run the sandbox natively
cargo run -p lunco-client --bin sandbox

# Build for web (using scripts/build_web.sh)
./scripts/build_web.sh
```
