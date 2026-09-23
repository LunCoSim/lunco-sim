# lunco-celestial-ephemeris

Concrete **high-fidelity ephemeris provider** for `lunco-celestial`.

This is the "heavy half" of the celestial split. `lunco-celestial` defines the
`EphemerisProvider` trait + a default resource; this crate supplies a real
implementation backed by analytical theories and scene-selected vector data.

## What it provides

- **`CelestialEphemerisProvider`** — concrete `EphemerisProvider`. Combines
  built-in analytical modules (VSOP2013 Earth/Sun/EMB, ELP/MPP02 Moon, via the
  `celestial-ephemeris` / `celestial-time` / `celestial-core` crates) with
  scene-requested vector datasets (JPL Horizons CSV) held behind
  `Arc<RwLock<…>>` for `position()`.
- **`EphemerisPlugin`** — apps that need real planetary positions add this; it
  **overwrites** the `EphemerisResource` installed by the semantic/runtime
  setup and parses dataset text delivered by the generic asset runtime.

## Vector data is selected by authored scene policy

This crate opens no sockets, selects no assets, and builds no URLs. The
application's authored asset lifecycle policy selects a dataset for a scene;
the generic asset runtime reads it only when that scene completes loading.
`assets/manifests/ephemeris.toml` carries transport (`url`, `dest`) for
`lunco-assets` and an `[<key>.ephemeris]` sub-table (`naif_id`, `center`) for
this parser. The generic runtime resolves the declaration to its canonical
asset URI and loads UTF-8 text through `TextAsset`. Downloading still requires a user request in Settings ▸
Downloadable data. If the active scene needs an uninstalled dataset, the
generic asset runtime reports the requirement and waits for the user to
download it.

## Platform note

Does **not** build on Windows MSVC: a transitive dependency
(`celestial-eop-data`'s `build.rs`) shells out to the Unix `date` command. The
split keeps the rest of `lunco-celestial` portable while the high-fidelity
provider is opt-in.

## Usage

```rust
app.add_plugins(lunco_celestial_spatial::CelestialPlugin);
app.add_plugins(lunco_celestial_ephemeris::EphemerisPlugin); // overrides the default provider
```

## Status

Working. Analytical positions plus scene-requested datasets parsed from generic
runtime asset deliveries. Scene-selected vectors are retired at scene
teardown.
