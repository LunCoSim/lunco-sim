# lunco-celestial-ephemeris

Analytic natural-body position provider for `lunco-celestial`.

The semantic crate defines the `EphemerisProvider` trait and default resource.
This crate supplies the maintained VSOP2013 Earth/EMB and ELP/MPP02 Moon
models through `celestial-ephemeris`, `celestial-time`, and `celestial-core`.
It evaluates natural-body positions at the requested epoch and does not load
scene assets or evaluate spacecraft motion.

Apps that need the analytic natural-body model install:

```rust
app.add_plugins(lunco_celestial_spatial::CelestialPlugin);
app.add_plugins(lunco_celestial_ephemeris::EphemerisPlugin);
```

Scene-authored spacecraft motion uses ordinary USD transform `timeSamples`
through `lunco-usd-bevy-animation`.

This crate does not build on Windows MSVC because the transitive
`celestial-eop-data` build script shells out to the Unix `date` command. The
split keeps the rest of `lunco-celestial` portable.
