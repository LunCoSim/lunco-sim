# luncosim

The **flagship lunar-mission simulator** — the full-stack windowed app.

## What it is

`luncosim` assembles every simulation plugin into one cohesive Bevy app:

- Celestial bodies + ephemeris, solar-system-scale `big_space`, and an orbital
  camera that auto-focuses Earth on boot.
- The whole **FSW / Hardware / Mobility / Robotics / Avatar** stack, running
  under the workbench UI.
- Global coordinate propagation via `big_space` (high-precision grid entities +
  low-precision children) — no custom transform propagation.

Unlike `sandbox` (which gates its render/windowing stack behind the `ui`
feature for its headless server), `luncosim` is **always windowed** — the full
render + windowing stack is unconditional and `bevy_egui` drives the workbench
chrome.

## CLI Usage

```bash
cargo run -p luncosim
```

Single source for desktop + web: the wasm build uses `lunco-web`'s
`WebReadyPlugin` to dismiss the HTML loader once the first frame paints (no-op
on native). Build the web bundle with `scripts/build_web.sh build luncosim`.

## Cf. the other top-level binaries

- [`sandbox`](../sandbox/README.md) (`lunco-sandbox`) — ground-physics test bed.
- [`lunica`](../lunica/README.md) (`lunco-modelica`) — Modelica workbench.

## See also

- [Applications index](../README.md) — every binary at a glance.
- [`architecture/00-overview.md`](../../architecture/00-overview.md) — how the stack fits together.
