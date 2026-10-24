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

The astronomy crates are pinned to maintained upstream
[`Celestial`](https://github.com/gaker/celestial). Its position-only analytic
evaluators return ICRS astronomical units, including the geocentric Moon.
The provider converts native vectors into typed ecliptic coordinates and reuses
results only for the exact requested epoch. Failed evaluations remain unavailable.

`lunco-time` uses Celestial's checked UTC/TAI/TT/TDB conversions, leap-second
validation and ISO 8601 formatting. UTC input requires a complete date and time,
for example `2026-10-24T12:30:00.000Z`. The portable application clock supplies
Unix seconds and nanoseconds; an invalid conversion remains an error at its
policy or UI consumer.
