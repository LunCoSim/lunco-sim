# lunco-celestial-ephemeris

Concrete **high-fidelity ephemeris provider** for `lunco-celestial`.

This is the "heavy half" of the celestial split. `lunco-celestial` defines the
`EphemerisProvider` trait + a default resource; this crate supplies a real
implementation backed by analytical theories and external mission data.

## What it provides

- **`CelestialEphemerisProvider`** — concrete `EphemerisProvider`. Combines
  built-in analytical modules (VSOP2013 Earth/Sun/EMB, ELP/MPP02 Moon, via the
  `celestial-ephemeris` / `celestial-time` / `celestial-core` crates) with a
  local cache of external JPL Horizons CSV mission data (`Arc<RwLock<…>>` so a
  background fetch can fill it in).
- **`EphemerisPlugin`** — apps that need real planetary positions add this; it
  **overwrites** the `EphemerisResource` installed by
  `lunco_celestial::CelestialPlugin` and kicks off the background Horizons fetch.

## Platform note

Does **not** build on Windows MSVC: a transitive dependency
(`celestial-eop-data`'s `build.rs`) shells out to the Unix `date` command. The
split exists precisely so the rest of `lunco-celestial` stays portable while the
high-fidelity provider is opt-in.

## Usage

```rust
app.add_plugins(lunco_celestial::CelestialPlugin);
app.add_plugins(lunco_celestial_ephemeris::EphemerisPlugin); // overrides the default provider
```

## Status

Working. Analytical positions + Horizons CSV cache; embedded-ephemeris
constructor (`new_with_embedded_ephemeris`) for bundled mission data.
