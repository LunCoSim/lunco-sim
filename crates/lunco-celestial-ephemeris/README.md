# lunco-celestial-ephemeris

Concrete **high-fidelity ephemeris provider** for `lunco-celestial`.

This is the "heavy half" of the celestial split. `lunco-celestial` defines the
`EphemerisProvider` trait + a default resource; this crate supplies a real
implementation backed by analytical theories and external mission data.

## What it provides

- **`CelestialEphemerisProvider`** — concrete `EphemerisProvider`. Combines
  built-in analytical modules (VSOP2013 Earth/Sun/EMB, ELP/MPP02 Moon, via the
  `celestial-ephemeris` / `celestial-time` / `celestial-core` crates) with
  external mission vectors (JPL Horizons CSV) held behind `Arc<RwLock<…>>`, so a
  dataset downloaded mid-session is visible to `position()` without a restart.
- **`EphemerisPlugin`** — apps that need real planetary positions add this; it
  **overwrites** the `EphemerisResource` installed by
  `lunco_celestial::CelestialPlugin`, registers this crate's `Assets.toml` with
  `lunco_assets::datasets`, and adopts each declared dataset once its file is on
  disk.

## Mission data is DECLARED, never fetched here

This crate opens no sockets and builds no URLs. `Assets.toml` declares each
mission dataset — transport (`url`, `dest`) for `lunco-assets`, and an
`[<key>.ephemeris]` sub-table (`naif_id`, `center`) for us. Downloading happens
only when a user asks (Settings ▸ Downloadable data); until then the mission
simply has no trajectory and `position()` answers `None`, which is the honest
answer offline.

It used to fetch from JPL at startup, driven by a second file
(`assets/missions/*.ephemeris.json`) that repeated the query. Both are gone —
see `docs/architecture/56-asset-resolution-and-cache.md`.

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

Working. Analytical positions + declared mission datasets; embedded-ephemeris
constructor (`new_with_embedded_ephemeris`) for bundled data on web.
