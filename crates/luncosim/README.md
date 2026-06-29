# luncosim

**LunCoSim — the full lunar-mission simulator.** The flagship windowed app.

Celestial bodies + ephemeris, solar-system-scale `big_space`, an orbital camera
(auto-focus Earth on boot), and the whole FSW / Hardware / Mobility / Robotics /
Avatar stack under the workbench. It assembles all simulation plugins into one
cohesive Bevy app — asset sourcing, plugin init, and `big_space` global
coordinate propagation.

Cf. the other top-level binaries:
- `sandbox` (`lunco-sandbox`) — ground-physics test bed.
- `lunica` (`lunco-modelica`) — Modelica workbench.

## Shape

A single `src/main.rs`; bin name = crate name = `luncosim`:

```bash
cargo run -p luncosim
```

Unlike `lunco-sandbox` (which gates its render/windowing stack behind `ui` for
its headless server), `luncosim` is **always a windowed GUI app** — the full
render + windowing stack is unconditional, and `bevy_egui` drives the workbench
chrome (no `default-features = false`; avatar's `ui` feature stays on).

Single desktop + web source: the wasm build uses `lunco-web`'s `WebReadyPlugin`
to dismiss the HTML loader once the first frame paints (no-op on native).

## Notes

Transform propagation relies entirely on `big_space`'s built-in systems
(`propagate_high_precision` for Grid entities, `propagate_low_precision` for
children) — the old custom propagation system was removed because it fought
big_space and corrupted `GlobalTransform` (the root cause of surface-mode camera
roll).
