# lunco-render

Shared **render-look configuration** for LunCoSim.

The single, render-capable home that sits *below* every 3D crate
(`lunco-celestial`, `lunco-usd-bevy`, `lunco-environment`, the binaries) so they
agree on "what the scene's look is" **by construction** instead of by
copy-paste.

It depends only on `lunco-core` + the lightweight `bevy_light` component types —
never `bevy_pbr` — so it forms no dependency cycle and never drags the render
pipeline into the slim web / Modelica binaries.

## What it owns

- **`sun::LunarSunShadow`** — the canonical lunar sun-shadow spec (cascade
  split + shadow-map atlas + depth/normal biases). Shared by the sandbox,
  celestial, and USD render paths; binaries override individual biases for their
  look (e.g. the sandbox's hard-shadow tuning).

## Roadmap

Intended home for the rest of the render-look surface: exposure / earthshine,
anti-aliasing, sky / Earth, and the `RenderSettings` window backing. Only the
sun-shadow spec lives here today.
