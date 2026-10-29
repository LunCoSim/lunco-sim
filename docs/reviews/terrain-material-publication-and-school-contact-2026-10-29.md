# Terrain material publication and School rover contact

`lunco-render-bevy::shader_look` constructs each material with the loaded
fragment shader's cached `ShaderSchemas` layout before binding its handle.
This applies to first bindings, shared LOD band/stitch replacements, private
structural replacements, and shader asset repair. Unreflected sources remain
pending through the existing readiness contract. Layout selection is a render
invariant; it adds no Rhai policy, shader variant, dependency patch, or terrain
visibility mechanism. Shared material identity and batching are preserved.

The inline regression `newly_keyed_material_uses_loaded_layout_before_binding`
checks the Add observer before another asset-event sweep can run, then changes
the material key and checks its replacement. Both must carry the current source
layout and packed nonzero values; a ready replacement retains readiness. The
negative baseline fails at the first binding. The repaired test and
`cargo build --locked -p lunco-luncosim -j 4` pass.

Summer Space School's authored DEM collider lattice is depth 9 with 65 nodes per
tile side, independent of graphics quality. On its 998.078 m Apollo 15 crop this
gives 0.030459 m contact spacing and a 0.060918 m Nyquist gate, retaining its
authored 0.18–0.65 m synthetic crater radii. These features are procedural
relief, not additional measured lunar data. The authoring change was made with
`OpenFile`, `ApplyUsdOps`, `SaveDocument`, and composed readback. It changes only
the two integer collider parameters in the Twin's `traverse.usda`.

The Twin owns `sim/scenes/tests/terrain_contact.usda`, its Rhai observer, and
`scripts/test_terrain_contact.sh`. The fixture was created through USD document
operations and references the production workshop. Its 441 near-rover native
non-solid downward rays compare actual contacts with the analytic oracle; a
short ray ending above ground supplies the negative case. The initial negative
diagnostic grid measured 0.106833 m maximum disagreement. The retained 10 mm
gate also rejects a generated depth-6/33-node fixture at 0.012760 m, with a
terminal failed verdict and exit code 1. The production headless scene test
passes all three checks at tick 1, with 0.000686 m maximum error and exit code 0.
The generated negative fixture stays outside the maintained test catalogue.

Normal windowed acceptance used this checkout's rebuilt production binary and
owned API 4791. After the classroom scenario and physical reference frame
settled, eight initial ground captures, forty rapid material-key captures and
one hundred moving-camera captures passed the clear-patch pixel check. The
configured forward binding then drove the actual rover 8.136 m. The same contact
observer passed afterward with 0.005366 m maximum disagreement. Readiness is
true with no world hold; runtime diagnostics contain zero errors or warnings,
and the accepted GPU logs are clean. Captures include native vehicle shadows.

Evidence is under `target/summer-terrain-fix/`: `layout-baseline.log`,
`layout-final.log`, `build.log`, `scene-test-final.log`,
`coarse-negative-verdict-final.log`, `acceptance/pixel-validation.json`, and
`drive-final/validation.json`. The accepted windowed run and the headless scene
test exit normally; owned ports 4791–4793 are released. Other simulations
were active, so frame timings are workload-qualified and establish no FPS
improvement. Existing workbench edits remain outside this repair.
