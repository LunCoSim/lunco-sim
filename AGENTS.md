# LunCoSim agent guide

Compact operating contract. Read `skills/README.md`, `docs/crates-index.md`,
`docs/principles.md`, the architecture index, and the relevant open review first.

## Design

- Read owning source before accepting a bug claim; comments describe intent.
- Check OpenUSD, Modelica, Avian, Bevy, or a maintained crate before adding a
  schema, resolver, field, or duplicate mechanism.
- No legacy paths, shims, aliases, duplicate spellings, or writes without readers.
  Replace the old mechanism and remove its traces in the same change.
- Use composed USD reads for runtime behaviour and authored-layer reads for
  authoring/document questions. USD owns scene facts and standard fields such as
  `doc`, `metersPerUnit`, `UsdShade`, and `UsdPhysics`.
- Modelica owns continuous equations/state; behaviour trees own sequencing; Rhai
  owns scenario glue/policy; Rust owns engine mechanisms, kinematics, dynamics,
  and hot paths. Production Rhai must not use `on_tick` except for test verdicts.
- A movable mounted part needs both a rigid body and a joint; internal geometry
  must not become an unconnected body. The `nested-body-no-joint` lint guards this.
- Generated USD schema files come from `scripts/gen_schema.py`; edit source schema
  and regenerate. Update `docs/crates-index.md` for crate changes.

## Tests and runtime

- Test scenes are under `assets/scenes/tests/`, scenarios under
  `assets/scenarios/tests/`. A green gate needs a negative fixture and a real verdict.
- Use only the production `sandbox` binary for scene tests and visual validation.
  Full scene reload is supported; partial object/reference reload remains TODO.
- Establish a behaviour baseline before physics/vehicle changes and rerun it.
  Capture real exit codes and inspect verdicts.
- Use focused tests first, then production sandbox. Use `-j 4`, repository
  `target/`, and regular `sccache`; never use managed temporary build directories
  or custom temporary files. Avoid overlapping Cargo builds.

## Sandbox lifecycle

- Every controllable launch uses an explicit API port:
  `target/debug/sandbox --api 4101` (or another free port).
- Reuse the existing session for asset, shader, and Rhai reloads through its API.
  When a replacement is required, stop the previous session through API `Exit`,
  verify its process and API port are gone, then launch the replacement. Never
  overlap GUI/API sessions or reuse a port owned by the previous process.
- Keep one production process while iterating. Use `/api/commands` for scene
  reloads, telemetry, screenshots, and tests. Use `ReloadShader` or `RunScenario`
  for live edits instead of relaunching.

## Performance, UI, and persistence

- Per-frame work is only for continuous rendering, physics, animation, and input.
  Otherwise use observers, asset events, change detection, revisions, or hashes.
- UI colours, spacing, and rounding come from `lunco-theme`; UI dispatches typed
  commands and does not mutate domain state directly.
- Heavy parsing, baking, mesh generation, and I/O must not block the UI thread.
- Persist through `lunco-settings` and `lunco-storage`. Runtime persistence loads
  are off by default and saves are independent; corrupt optional state cannot stop
  authored scene loading. Do not use JSON for internal change detection.

## Handoff

- Search with `rg` and exclude `target/`.
- Run `git diff --check`, focused formatting/checks, and relevant runtime tests.
- Fix warnings introduced by the change. Report exact tests, runtime/API checks,
  sandbox port/session state, and remaining blockers. Never claim unobserved results.


<claude-mem-context>
# Memory Context

# [main] recent context, 2026-07-28 2:11pm GMT+7

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (14,111t read) | 641,738t work | 98% savings

### Jul 28, 2026
47881 12:23a 🔴 Rover jitter fixed via gear differential constraint unification
47882 12:25a ✅ Rover drive test initialized with gear constraint at throttle 0.6
47883 12:26a ✅ Jitter data collection in progress for gear-constrained rover
47884 " 🔴 Rover jitter quantified and fixed: gear constraint reduces height bobbing 57x
47885 " 🔵 Gear constraint introduces performance cost: 65% higher frame time
47886 12:27a ✅ Performance A/B test configured: gear constraint vs baseline
47887 12:28a ✅ Full mobility test suite launched for regression validation
47888 12:29a 🔵 Performance A/B test completed: gear constraint costs 25% frame rate
47889 " ✅ Environment variable gate added for gear constraint A/B testing
47890 " ✅ Final A/B validation run: identical scene with gear constraint toggled
S7121 Debug and fix rover jitter during movement caused by competing coordinate systems. Implement holonomic gear constraint solver and characterize performance with self-timing instrumentation. (Jul 28, 12:31 AM)
47892 12:32a 🔵 Identical-scene A/B test shows gear constraint is performance WIN, not cost
47893 " ✅ Instrumentation added for constraint solver self-timing
47894 " ✅ Self-timing instrumentation completed for constraint solver performance profiling
S7127 Pull latest changes and debug rover jitter caused by two systems fighting over coordinates (Jul 28, 12:33 AM)
S7128 Status check: "what's left?" on rover suspension physics work. User asked for remaining tasks and blockers after shimmer/jitter fix was implemented. (Jul 28, 12:57 AM)
S7130 Status check and shimmer fix validation: restore jitter probe diagnostic, create paired measurement harness, and run before/after test of gear-joint suspension change (Jul 28, 5:58 AM)
47924 5:58a ✅ Jitter probe diagnostic re-added for suspension smoothness measurement
47925 5:59a ✅ Jitter probe plugin integrated into sandbox core initialization
47926 " 🔵 Paired before/after measurement harness for shimmer fix validation
S7132 Debug gear-joint suspension constraint implementation that fails catastrophically under drive load, compared to penalty-spring baseline (Jul 28, 5:59 AM)
47927 6:03a 🔵 Paired A/B measurement completed with suspicious data disparity
47928 " 🔵 AFTER measurement crashed after 7.8 seconds; gear-joint implementation unstable under drive load
47929 " 🔵 AFTER run silent failure: rover never accelerated, process stopped without panic or error
47930 " 🔵 Both BEFORE and AFTER runs failed test; AFTER crashed much faster
S7151 Fix starfield circle-in-sky rendering and terrain shadow issues in summer-space-school scene; improve performance and visuals; review recent commits (Jul 28, 6:04 AM)
47964 6:25a 🔵 Starfield rendering uses view-ray emissive dome, not surface shading
47965 " 🔵 Starfield parameters exposed via shader metadata annotations for live hot-reload
47966 " 🔵 Terrain vertices include morph targets for geomorphing LOD transitions
47967 " 🔵 Headless server mode built via Cargo feature flags, not separate binary
47968 " 🔵 NoFrustumCulling used for sky/starfield in big_space and trajectory rendering
47969 6:27a 🔴 Starfield dome culled when using shader materials—added double_sided attribute preservation
47970 " 🟣 Per-instance vertex shader support via info:wgsl:vertexAsset USD attribute
47976 6:41a 🔵 Unreachable public item warnings in lunco-celestial and lunco-sandbox crates
47977 " ✅ Restrict JitterProbePlugin visibility from pub to pub(crate)
47978 " 🔵 Port 3001 already in use on localhost
47979 6:42a 🔵 Existing sandbox process occupies port 3001
47980 " ✅ Sandbox process terminated via API exit command
S7153 Fix starfield circle-in-sky rendering and terrain shadow issues in summer-space-school scene; improve performance and visuals; review recent commits (Jul 28, 6:42 AM)
47983 6:45a ✅ Headless server build completed successfully
S7156 Fix starfield appearing as circle in sky; investigate and improve bad terrain shadows in summer-space-school; review latest changed commits for regressions (Jul 28, 6:46 AM)
47984 6:47a 🔵 Sandbox API does not support DiscoverSchema command
47985 6:48a 🔵 DiscoverSchema is implemented in code but not exposed by running sandbox API
47986 " 🔵 Sandbox API missing query and queries endpoints
47987 " 🔵 Sandbox API schema endpoint is accessible and responsive
47988 " 🔵 Sandbox API exposes 181 commands including scene loading and screenshots
47989 6:49a 🔵 Sandbox API command signatures for scene loading and imaging
47990 " ✅ Initiated loading of summer-space-school twin via API
47992 " 🔵 OpenTwin command accepted but twin not reflected in open documents
47994 " 🔵 Sandbox API lacks command result polling and status query mechanisms
47996 " 🔵 Summer-space-school twin loaded with terrain layers and rendering pipeline active
47997 6:50a ✅ Captured screenshot of summer-space-school scene rendering
47998 " 🔵 Clock hierarchy in luncosim time domain
47999 6:51a 🔵 SetClock command successfully isolates sky rendering at extreme timescale
48000 " 🔵 Sky lighting cycles correctly through day-night; starfield circle issue is rendering-layer problem
48002 6:55a 🔵 No Pipeline or Shader Validation Errors in Build Output
48003 " 🔴 Fixed Starfield Sky Dome Appearing as Circle (Backface Culling Issue)
48004 " 🔴 Fixed Terrain Shadow Quality in Summer School Scene
S7165 Investigate and fix multiple bugs across LunCo: starfield rendering, terrain shadows, Modelica parsing on Windows, co-sim wiring, schema units, render robustness, Earth direction, and time handling. (Jul 28, 6:56 AM)
S7166 Investigate and fix 6 Windows bug-report items (B-04/B-06, B-11/B-09, B-07/B-10) affecting Modelica MSL gating, Windows .mo parsing, co-sim antenna wiring, USD schema units, render robustness, and celestial/time handling. Four async agents launched to cover four distinct clusters. (Jul 28, 10:16 AM)
**Investigated**: **Four parallel diagnostic agents deployed:**
1. **B-04/B-06** (MSL gating + Windows strip): Located MSL background install trigger at `msl_remote.rs:758` (unconditional spawn with no networking-mode gating). Confirmed bound-input strip at `lib.rs:1134` calls `ast_extract::strip_input_defaults_with_report()` which parses FILE CONTENT not path. Rumoca grammar supports CRLF at scanner level (`modelica_parser.rs:131`). Path prefix `\\?\` is diagnostic only; actual parse happens on file bytes read via `std::fs::read_to_string()`.

2. **B-11/B-09** (YawJoint + schema): Found antenna.usda wires at lines 262 (`inputs:angle.connect = </outputs:az>`) and 274 (`inputs:angle.connect = </outputs:el>`). Rocker_bogie.usda shows correct pattern: declares `float outputs:drive_left = 0.0` on chassis chassis (line 116). B-09 schema lookup uses tuple keys `(schema, name)` where schema="UsdGeomCylinder" but warning references instance "Cylinder_1.radius"; likely instance-vs-class keying mismatch at `schema.rs:420`.

3. **P0 Shadow OOM**: Full read of `render_robustness.rs` (578 lines). Ladder correctly escalates: Healthy → ShadowMapsOff (disables all DirectionalLight/SpotLight/PointLight shadow_maps_enabled) → GaveUp (sets Camera.is_active=false). Lines 396-413 show escalation logic. Issue: frames still submit at ~600/s after camera deactivation because render graph continues (likely shadow passes, egui, or other systems not gated on Camera.is_active).

4. **B-07/B-10** (Earth + time): Sandbox scene HAS solar_system.usda reference at line 46 and site anchor (lat/lon/height/body at lines 27-30), so CELESTIAL opt-in is present. B-07 EarthDirectionWorld degenerate message not found in grep (likely in lunco-environment/src/earth.rs, not yet read). B-10 discontinuity detector not yet located; re-anchor message implies time jump logic exists but order/reset unclear.

**Learned**: - **MSL offline**: No networking mode consulted before spawn_native_install(). Code path: msl_remote.rs:758 → spawn_native_install() with no check of `MslSettings` or offline flag.
- **Bound-input parsing**: Failure is in file content parse not path; CRLF supported by Rumoca lexer but file must parse through `rumoca_phase_parse::parse_to_ast()` → `ast_extract::strip_input_defaults_with_report()`.
- **Render mitigation**: Ladder state machine is pure and works correctly, but presentation loop doesn't honor Camera.is_active=false; suggests render graph continues submitting other passes or presentation queue has independent logic.
- **YawJoint wires**: Authored correctly in antenna.usda. Port declaration mismatch likely in cosim runtime registry keying (class "PhysicsRevoluteJoint" vs instance "YawJoint").
- **Schema lookup**: CORE_LINEAR_UNITS entries keyed by `(schema_class_name, property_name)` tuples; lookup in `apply_core_linear_units()` at schema.rs:420 tries to match by stringified schema+name, but warning shows instance name "Cylinder_1.radius" suggesting instance-level lookup happening somewhere else.
- **Sandbox scene**: Celestial reference is present and correct; issue is likely wiring/publisher side not scene authoring.

**Completed**: None. All work is diagnostic; no code changes applied yet. Four agents (afc45bbf4bc11095e, aff002da57f60cf2b, aa03e745418fe6e70, a01b313984e5623d3) launched async and still running.

**Next Steps**: **Immediately upon agent completion:**
1. **B-04 (MSL gating)**: Add networking-mode gate before spawn_native_install() at msl_remote.rs:758. Check for `MslSettings` and abort if networking mode is None.
2. **B-06 (Windows .mo)**: Determine if strip failure is CRLF or BOM by inspecting first few bytes of failing .mo file; if BOM, strip in read_to_string() result; if CRLF, ensure rumoca parser handles line-ending normalization.
3. **B-11 (YawJoint)**: Fix cosim port registry keying — likely need to register "angle" port explicitly on PhysicsRevoluteJoint, or fix wire to use correct port name.
4. **B-09 (schema units)**: Change CORE_LINEAR_UNITS lookup from instance-name keying to class-name keying, or add UsdGeomCylinder/Capsule entries to vendored schema.
5. **P0 (render death)**: Upstream fix: clamp DirectionalLight shadow-casting count at startup via light_policy or limit shadow map size. Downstream: fix frame submission loop to stop when presentation stopped (investigate why Camera.is_active=false doesn't halt render graph).
6. **B-07 (Earth direction)**: Locate EarthDirectionWorld degenerate check in lunco-environment; verify wiring is correct and add NaN guard to port publisher.
7. **B-10 (epoch jump)**: Find re-anchor and discontinuity-detector code; reorder so discontinuity check runs AFTER re-anchor and state is reset properly.

All fixes are minimal and targeted. No starfield/terrain shadow work until P0 render chain is unblocked.


Access 642k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>