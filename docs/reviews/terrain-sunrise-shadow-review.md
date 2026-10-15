# Terrain sunrise and shadow publication review

The shared light projector preserves small rendered rotations through the
horizon. Sky disc radius and terrain penumbra consume the existing UsdLux angle;
there is no separate sky size constant. Scene depth owns camera-relative disc
occlusion. The canonical cache and physical-query contracts are in
[terrain layered rendering](../architecture/terrain-layered-rendering.md).

DRY discovery identified the existing incremental annotation uploader as the
owner for resident image updates. Its typed contract lives in `lunco-render`,
with sparse RGBA32Float and complete R8Unorm payloads consumed by
`lunco-render-bevy`; annotation and horizon producers share that reader.
Equal-size shadow updates preserve image and GPU texture identities. Source
identity and sampling quality fence completed worker products. Removal retires
pending uploads and acknowledgements.

Hook review: direction projection, angular comparison, texture lifetime and
queue writes are shared rendering math/resource invariants. Their inputs are
existing light, terrain and quality contracts, and their output is typed
presentation data. They remain in Rust/WGSL; authored Rhai retains quality policy.
Native workers prepare visibility without controlling deterministic physics.

Production verification on the normal binary:

- `sunrise_shadow_cache` passes nine authored checks, including repeated
  crossings at ±0.00001 radians, three acknowledged R8 updates into one observed
  GPU allocation, stable bindings overnight, and fixed physics cadence.
- `shadow_cache_sampling` passes its GPU panel, including the current night gate
  inside native cascade range and the unbound-heightfield case.
- `sun_disc_occlusion` passes the authored two-degree radius and opaque half-disc
  pixel gate. Authoring one degree produces the expected red failure panel;
  restoring two degrees restores the passing pixels.
- The existing Summer `route_surface_annotation` consumer passes twelve checks,
  including sparse uploads, retained binding and terminal invalid-input recovery.
- The live Sun material GPU driver passes five phases on a static heightfield
  and five on a streamed DEM, including a sub-tolerance sunrise and zero/positive
  authored angular size. Static heightfield owners and DEM owners share the
  terrain uniform feed. The shader compares its horizon side against the native
  light; CPU cache agreement alone is insufficient for this contract.
- Focused typed-upload coalescing/retirement and angular precision tests pass.
- Six headful Griffin day/night/return views complete. Sampled frame durations
  remain below 15 ms in those windows; this is bounded observation, not a Tracy
  attribution or an exhaustive stall guarantee. Physical transport is paused
  for those directed views; the authored cache gate supplies cadence evidence.

Evidence is under `.lunco/terrain-lines/sunrise-fix/` and
`.lunco/terrain-lines/final-pass/`. Owned API 4286 sessions
close through Exit or the recorder's normal ExitAfterRecording path. The full
render wrapper remains blocked by an unrelated missing `twin_search_paths`
scenario source; direct production fixtures supply the scoped GPU evidence.
Browser performance is unverified. A pending angular bake retains its last
completed presentation sample, not an exact current-Sun visibility claim.
