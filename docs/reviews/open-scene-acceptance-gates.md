# Open scene acceptance failures

Status: open. The 2026-10-16 Linux source-checkout sweep is not a clean
whole-runtime acceptance result. Keep failures and missing verdicts visible;
passing Twin retirement checks do not establish that every scene is usable.

## Reproduce

Build the production binary with `cargo build -p lunco-luncosim --bin luncosim -j 4`,
then run:

```sh
LUNCOSIM_BIN="$PWD/target/debug/luncosim" \
EDITOR_API_PORT=4800 RENDER_API_PORT=4305 \
./scripts/run_scene_tests.sh --no-build -j 1
```

The gate uses one process at a time, one compute thread, zero jitter,
36,000 simulation ticks, a 420-second startup budget and a 900-second scene
budget. Editor and graphics gates use their wrapper-owned bounds. Runtime
overlay I/O is disabled except in the separate manifest-backed persistence gate.
Logs are under `target/scene-tests/` and the render/editor wrapper log locations.

## Sweep result

The initial complete sequential sweep discovered 163 gates: 97 passed,
46 failed, 19 produced no verdict, and one timed out. This includes 141
headless, 11 graphics, one render-contract and 10 editor gates. Subsequent
focused corrections made `usd_material_edit_projection` and
`preview_domain_isolation` pass; the full sweep was not repeated.
The separate route persistence gate failed before warm restoration.
These totals describe bounded observed runs, not proof of a leak-free runtime.

## Established failures

| Boundary | Observed failure | Owner / next verification |
|---|---|---|
| Authored test identities | `model_state_invalidation`, `assembly_pattern_builder`, `assembly_workflow` and `editor_workflow` compare unsigned document IDs with signed negative sentinels. Rhai reports `Function not found: < (u64, i64)` before the intended behavior is exercised. | Update the authored observers to the typed optional-document contract; preserve unsigned identities. Rerun their real behavioral assertions. |
| Antenna source admission | `antenna_mount`, `comms_demo` and `solar_rover_energy_cycle` retain pending `AntennaTrackerController` sources after structural and visual projection, while the listed generated models have compiled. | Trace causal input admission at `lunco-usd-sim-cosim`; a timeout does not establish a compiler or asset-load failure. |
| Physics / participant admission | `sandbox_ramp_placement` also reports `avian-backend-pose-invalid` for a non-unit `ZSupport` rotation in the owned interactive cancellation probe. `battery_mounts` and `sandbox_ramp_placement` remain held at world-level USD physics admission after model compilation. `multi_rover_stress_negative` reaches participant timeout with disabled kinematic bodies and pending joints. | Inspect the physics admission and participant initialization owners; do not manufacture samples or bypass readiness. |
| Coupled initialization | `descent_lander_runtime` and `tutorial_lander_mission` report inconsistent `guidance_pitch`: solved approximately `0.7217`, observed `0`. The owner faults and pauses the participants. | Resolve the actual initialization contract at `lunco-usd-sim-cosim::initialization`. |
| Model compilation | `annotated_rocket_stage` rejects an unbalanced model: 28 equations and 27 unknowns. | Diagnose the authored/generated Modelica system through the maintained compiler. |
| Route persistence | The cold public waypoint edit adds a referenced marker and then cannot author its `/P0/Trigger` override: the compound transaction rejects the missing path. Warm restore is consequently untested. | Trace reference closure preparation and document composition at `lunco-usd-commands` / `lunco-usd-document`, preserving the single transaction. Reproduce with `python3 scripts/api/test_route_runtime_persistence.py`. |
| Editor acceptance | `assembly_property_editor`, `assembly_document_audit`, `referenced_part_edit`, `route_interaction` and `selection_ai_workflow` failed their authored editor gates. | Inspect each wrapper's editor log before assigning a cause; the passing preview gates do not cover these commands. |
| Graphics acceptance | `battery_mounts_visual` exceeded its 120-second wrapper bound (exit 124). | Inspect graphical readiness and the actual authored verdict; a timeout is not a pass. |
| Observer execution | `sensor` reaches Rhai's string-size limit; `rover_comparison` repeatedly indexes an empty array; `sun_tracker` reads an undeclared entity dependency. None produces a terminal verdict before the tick bound. | Repair the authored observation contracts and rerun; these are missing acceptance evidence, not passes. |

Startup runtime faults currently become startup timeout/no-verdict results in
the scene runner. Immediate terminal-fault completion applies during stepped
execution. The startup diagnostic remains available, but the test-process
classification and delay need review at `lunco-scene-runner`.

Physical-motion assertions and the remaining editor/render verdicts must also
be assessed from their individual logs. Do not attribute them to Twin cleanup
or dismiss them as pre-existing without a controlled comparison.
