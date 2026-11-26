# Native CPU mesh preparation

The coordinated native Bevy revision `4cea221a4792adb3922abffd522b5c779b5b5c0f`
preserves the previously admitted shadow and presentation branches. The CPU
mesh instance owner uses a dense frame snapshot and one entity index; the mesh
allocator publishes uniform offsets and slab identities with the allocation.
Borrowed native phase data is encoded directly into the retained shader-layout
buffer. Dynamic uniform devices retain their owned array and offset semantics.

## Relative production evidence

Factory Site Layout, the same source, native desktop, stationary Avatar camera,
matching Tracy filter and sample period, equally sized settled render-frame
cohorts. The reference was application `a2adc5636` with its prior pinned native
renderer. The candidate used the source-matched local Bevy checkout before
publication. Concurrent compiler workloads were substantial in both captures;
these relative native CPU measurements are not uncontended product FPS.

| Native work | Candidate/reference CPU samples |
|---|---:|
| Shadow batching | 30% |
| Opaque batching | 31% |
| Shadow, opaque and shared uniform preparation | 44% |
| Extraction, preparation, shadow, opaque and buffer upload | 58% |
| Instance-buffer upload | 73% |

The combined preparation median and mean zone-work ratios were 44% and 48%.
Memory-copy samples represented less than 1% of the affected preparation group.
The overall render-schedule median was 84% of the reference; the mean was 96%,
under unequal external load. Extraction itself consumed 117% of its former
sampled work and remains a candidate for further investigation. The paired
viewport images agree in more than 99.99% of pixels, with no geometry or shadow
quality changes. Both captures completed with clear runtime diagnostics.

The native render-owner tests verify borrowed storage encoding without a clone,
identical shader-layout bytes, retained capacity, and dynamic-uniform offsets;
the allocation test verifies replacement and retirement. The PBR test checks
borrow identity, dense frame refresh and exact uniform/compare fields. The
production `shared_shadow_allocation` and `vertex_shadow_alignment` scenes
passed their authored verdicts and both positive pixel probes without red
disagreement. Run these exact graphics scenes directly if unrelated Twin scene
discovery blocks the full graphics runner. The maintained 2D and custom-phase
consumers use the same batching contract.

The normal release binary built from the published Git lockfile also completed
the native Factory Site Layout session with ready simulation, live telemetry,
clear runtime diagnostics and exact viewport pixel agreement with the reference.
The product run establishes visual and runtime qualification, not an unloaded
headline FPS claim.

The next owner investigation is reducing repeated extraction and final buffer
encoding/upload: retain exact mesh identity and the native per-view draw ranges;
shared indices are eligible only where batch contiguity and dynamic-uniform
offsets remain valid. No physics, telemetry or quality cadence is changed.

Raw captures and receipts are ignored under `target/perf/factory-20261125/`,
in `site-native-dense-borrowed-baseline` and `site-native-dense-borrowed-tracy`;
`dense-borrowed-relative.json` contains the relative comparison.
