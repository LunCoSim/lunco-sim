# Terrain contact and camera-detail cover

Summer's settled rover support rendered depth-6 terrain at approximately 0.244 m
posting spacing while native contact used depth 9, resolution 65, approximately
0.03046 m spacing. Both consumers read the same oracle, but independent band
filters removed visually the relief that still moved the wheels.

The terrain visualization owner now consumes the collider ring's canonical
support/build-ahead nodes as a minimum visual cover. Camera demand independently
reserves the finest tile under a nearby active camera and its edge neighbours.
The worker builds that required restricted tree first, reserves its split cost,
and spends the remaining graphics budget on discretionary camera LOD. Contact
and camera constraints preserve finer leaves and install morph pins after every
split. Coarser neighbours split first to retain 2:1 seams. Material morph-window
changes are consumed by resident tile looks when the support cover moves.
A committed cover must pass through morph publication even when its geometry
is already complete and resident. The idle path only applies after that commit
has reached the material reader, so a reused tile cannot retain an old unpinned
morph band as physics moves onto it.

The physical lattice supplies the nested streamed posting count. Canonical
contact tiles reuse `sample_heights_xz`; finer camera tiles sample
`SampledContactSurface`, a bounded worker-local view of those quantized native
triangle planes. Finer geometry therefore cannot introduce relief absent from
physics. Its parent lattice samples the same planes. Existing source/generation
fences and content-addressed tile caching remain the admission boundary; cache
format 16 distinguishes canonical depth and geometry. Retained physical-surface
CPU meshes are included in the existing LRU byte ceiling. Only GPU positions narrow
to f32. Physics selection, sampling, admission and cadence are unchanged and
never wait for visual workers or camera movement.

Routes and wheel marks continue through `SurfaceCurveAnnotation` and its shared
terrain-local index/texture. They inherit the actual terrain geometry, including
new fine tiles. Route overlays retain their unlit role; wheel marks retain tire
width and physical lighting. No raised ribbon mesh, extra ground sampler, or
per-frame history rebuild was introduced.

## Ownership and hook review

`lunco-terrain-core` owns node algebra and exact restricted-cover refinement;
`lunco-terrain-surface` owns canonical terrain samples, their presentation and
read-only `TerrainLodStatus` diagnostics. Existing graphics/camera settings own
tunable quality and near-detail radius. Geometry agreement, subdivision and seam
restriction are engine invariants, not changeable lifecycle or Twin policy, so
no Rhai policy hook is added. The production Rhai scenario owns acceptance and
uses typed public queries; JSON exists only in the external HTTP test driver.

## Verification

The focused `support_constraints_preserve_finer_camera_cover_and_restricted_edges`
geometry test checks coarse refinement, finer camera retention, post-split pins,
exact area, 2:1 seams, deterministic ordering and idempotence.

Run the production graphics gate with:

```sh
LUNCOSIM_BIN=target/debug/luncosim python3 scripts/api/test_terrain_contact_visual.py \
  --scene /home/rod/Documents/models/summer_space_school/space-school-twin/sim/scenes/rover_workshop.usda \
  --body-path /Traverse/Rover --route-path /Traverse/Route --port 4794 \
  --output target/terrain-contact-visual/final-acceptance
```

The authored verdict compares interior render triangles with native contact
rays within 1 mm and rejects a 2 cm displaced copy using those same contacts.
It checks annotation bindings and tire widths, real movement over tile
boundaries, reduced graphics budgets, camera detail finer than the physical
lattice, far-camera contact, and close-camera physical subdivision. Frame
history is collected during an ordinary 15-second drive. Screenshots and native
GPU logs are inspected separately. A concurrent optimization-worktree simulator
on port 4473 is left untouched and shares the machine/GPU.

Final production evidence: `target/terrain-contact-visual/final-acceptance/`.
All six phases passed 11 authored checks each (66 total), with zero runtime
errors/warnings and clean native GPU logs. The drive covered 10.965 m; 684 frame
samples measured median 20.648 ms, p95 24.488 ms, maximum 32.485 ms at 1280×720.
Concurrent GPU work means this is an observed window, not an isolated benchmark.
The far view was 116.356 m above local terrain and selected depth 4 under the
camera while native rover support retained depth 9. The close view was 3.030 m
above terrain and selected depth 10. Seven finer-mesh native-ray probes passed
in both fine-camera phases; worst render/contact error was 0.000003925 m.

The maintained Summer headless `test_terrain_contact.sh` also passed all three
checks: 441 native rays, maximum oracle/contact error 0.000686169 m, and the
short-ray negative case. Its owned API port 4792 and the graphics port 4794 were
released. No Twin source files were changed and no repository was pushed.
