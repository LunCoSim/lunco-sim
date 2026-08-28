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
  celestial, and USD render paths; callers construct it from the authoritative
  Graphics profile and may then apply authored scene overrides.
- **`RenderingQualitySettings`** — the persisted Graphics section and its
  `RenderingQuality::{Low, Balanced, High}` presets. `High` is the highest
  shipped renderer budget: it covers shadow maps and casters, the horizon-shadow
  cache, camera MSAA/bloom, sky cubemap resolution, lunar terrain caches/LOD,
  rock density, and geometric tessellation. Its interactive CDLOD terrain
  envelope is intentionally the same bounded envelope as `Balanced`; the extra
  High budget is spent on lighting, derived maps, rocks, and tessellation so
  terrain geometry cannot consume the frame. The settings are consumed by the
  render-capable crates; they do not select or replace USD-authored shader
  sources.

## Remaining roadmap

The remaining render-look surface includes deeper exposure/earthshine controls,
additional authored sky/Earth looks, and further Graphics UI refinements. New
quality controls belong in `RenderingQualitySettings` here and must have a real
consumer in the render-capable crates.
