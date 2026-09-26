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

The astronomy crates are pinned to the LunCoSim-maintained
[`celestial` fork](https://github.com/LunCoSim/celestial). Its workspace uses a
pinned `celestial-eop-data` fork, which formats its build-time UTC date with
Chrono and does not invoke a system `date` executable.
