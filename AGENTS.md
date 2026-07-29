# LunCoSim agent guide

Compact operating contract. Read `skills/README.md`, `docs/crates-index.md`,
`docs/principles.md`, the architecture index, and the relevant open review first.

## Design

- Read owning source before accepting a bug claim; comments describe intent.
- Check OpenUSD, Modelica, Avian, Bevy, or a maintained crate before adding a
  schema, resolver, field, or duplicate mechanism.
- No legacy paths, shims, aliases, duplicate spellings, or writes without readers.
  Replace the old mechanism and remove its traces in the same change. Do not
  retain a fallback, compatibility branch, or migration path that preserves an
  incorrect former behaviour. A default is permitted only when it is the
  documented semantic default of the authoritative input (for example, an
  omitted USD attribute), is exercised by a reader, and does not alter an
  explicit authored value.
- Use composed USD reads for runtime behaviour and authored-layer reads for
  authoring/document questions. USD owns scene facts and standard fields such as
  `doc`, `metersPerUnit`, `UsdShade`, and `UsdPhysics`.
- Asset identity, traversal, and storage belong to `lunco-assets`; USD-related
  runtime crates do not call `std::fs` for asset bytes. They use the asset
  boundary, while `lunco-usd-compose` supplies USD dependency interpretation
  and OpenUSD assembly (sublayers, references, payloads, variants), re-exported
  by `lunco-usd`. A composed stage is inert:
  it must not launch Modelica, Rhai, behaviour trees, physics, or rendering.
  Those are separate downstream projections. A tutorial projects metadata from
  a supplied stage; it never opens layers itself.
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
- Use only the production `luncosim` binary for scene tests and visual validation.
  Build it in this worktree, then invoke `target/debug/luncosim` directly; do not
  use an old `sandbox` executable name or hide a rebuild behind `cargo run`.
  Full scene reload is supported; partial object/reference reload remains TODO.
- Establish a behaviour baseline before physics/vehicle changes and rerun it.
  Capture real exit codes and inspect verdicts.
- Use focused tests first, then production luncosim. Use `-j 4`, repository
  `target/`, and regular `sccache`; never use managed temporary build directories
  or custom temporary files. Avoid overlapping Cargo builds.

## Sandbox lifecycle

- Every controllable launch uses an explicit API port:
  `target/debug/luncosim --api 4101` (or another free port).
- Networking is opt-in (`cargo build -p lunco-luncosim --features networking`);
  normal local builds and scene tests must not start a multiplayer host.
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
  luncosim port/session state, and remaining blockers. Never claim unobserved results.


<claude-mem-context>
# Memory Context

# [main] recent context, 2026-07-30 6:10am GMT+7

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (17,269t read) | 522,674t work | 97% savings

### Jul 28, 2026
S7157 Diagnose and fix balloon_netforce_flows_through_cosim_pipeline test failure preventing main branch fastforward (Jul 28, 6:56 AM)
48007 6:57a 🔵 Root cause found: ModelicaModel is paused, blocking all aerodynamic computations
48008 " 🔵 Model pause default: resume_after_compile field controls post-compile state
48009 " 🔴 Fixed balloon test: added resume_after_compile = true to unblock model stepping
S7158 Fix pre-existing balloon_netforce_flows_through_cosim_pipeline test failure blocking main branch fastforward (Jul 28, 6:57 AM)
48010 6:58a 🔵 resume_after_compile fix applied but test still fails identically
48011 " 🔵 Pause fix worked but revealed air density input is zero, blocking aerodynamic calculation
48012 " 🔵 Balloon.mo defaults rho0 (air density) to 0.0, intentionally zeroing all aerodynamic forces
48013 6:59a 🔴 Fixed balloon test: set rho0 = 1.225 to provide Earth-like atmosphere for aerodynamic forces
S7159 Fix pre-existing test failures in balloon integration tests by correcting model initialization and stepping assumptions (Jul 28, 6:59 AM)
48014 7:00a 🔵 balloon_netforce test now passes; two fixes successful but balloon_flies_up_test now fails
48015 " 🔵 balloon_flies_up_test has same root cause: rho0 defaults to 0.0, needs same fix
48016 " 🔵 balloon_flies_up_test uses identical unfixed model setup pattern from original test
48017 " 🔴 Applied same two fixes to balloon_flies_up_test: resume_after_compile and rho0=1.225
S7160 Fix pre-existing balloon integration test failures blocking main branch fastforward; two tests had architectural mismatches in model setup (Jul 28, 7:00 AM)
48018 7:01a 🔵 balloon_flies_up_test: one test now passes, one still fails; rho0 fix incomplete
48019 7:02a 🔵 balloon_flies_up_with_flat_gravity: model computes correctly (11.6 N) but physics assertion fails
48020 " 🔵 Fix applied uniform rho0=1.225 to tests with different gravity constants; violates physics
48021 " 🔵 flat_gravity test expects ~48 N buoyancy but model computes 11.6 N; temperature scaling likely culprit
48022 " 🔴 Added missing gravity=9.81 input to balloon test model setup
S7161 Fix pre-existing test failures and fastforward main branch; identified and resolved three architectural mismatches in balloon integration tests (Jul 28, 7:03 AM)
48023 7:03a 🔵 All tests passing: exit=0; balloon_netforce and balloon_flies_up both pass with all fixes applied
48024 7:04a ✅ Updated synthesis review to document balloon test fixes and three other findings (D5–D7)
S7162 Fastforward to main branch and verify test suite; fix second-pass diagnostics and state management issues in lunco-cosim and lunco-modelica (Jul 28, 7:04 AM)
S7163 Fix all issues in lunco-sim workspace (fix all request) (Jul 28, 7:07 AM)
48025 9:58a 🔵 Raycast spin floating-origin invariance test requirements
48026 " 🔵 Raycast wheel spin test harness setup and parameters
48027 " 🔵 Recent wheel_spin refactor: fixed-timestep requirement and friction cone fix
48028 9:59a 🔵 Fixed-clock test harness and passing control test
48029 " 🔵 Fixed-clock test infrastructure design rationale
48030 10:00a 🔴 Test harness bug: first app.update() runs FixedUpdate zero times
48031 " ✅ Document app_on_fixed_clock accumulator off-by-one behavior
48033 " 🔄 Refactor member class handling from verification to active resolution
48032 " ✅ Clarify app_on_fixed_clock documentation wording
S7164 Prioritize remaining work on rover simulator system — identify next actions after torque channel implementation and testing (Jul 28, 10:00 AM)
48034 10:01a 🔄 Thread resolved member classes through network reading pipeline
48035 " 🔄 Wire class resolver into projector pipeline with outcome handling
48036 10:03a 🔵 Syntax error in domain_projection.rs from refactoring
48037 " 🔴 Fixed syntax error in synthesize function call
48038 " ✅ Class resolver integration compiles successfully
48039 " 🔵 Unit tests require pending_sources field initialization
48040 10:04a 🔴 Updated test DomainNetwork constructors with pending_sources field
48041 " 🔵 Integration test requires MemberClasses parameter for read_network
48042 10:05a 🟣 Add path-derived-only mode to MemberClasses for offline testing
48043 " 🟣 Add integration tests for member class resolution behavior
48044 " ✅ Member class resolver integration complete and all tests passing
48045 10:06a 🟣 Integrate manual port holds into connection propagation
48047 " 🟣 Implement PortHolds resource for manual port control
48048 " ✅ Complete PortHolds integration with real-time expiry
S7165 Investigate and fix multiple bugs across LunCo: starfield rendering, terrain shadows, Modelica parsing on Windows, co-sim wiring, schema units, render robustness, Earth direction, and time handling. (Jul 28, 10:07 AM)
48050 10:10a 🟣 SetPort commands now hold control over WIRED inputs with auto-expiring locks
48051 10:11a 🔵 SetPortProvider TODO in lunco-usd-sim aligns with hold mechanism just implemented
48052 " 🟣 SetPort and ReleasePort API commands now fully support hold mechanism with configurable durations
48054 10:12a ✅ Add metersPerUnit=1 to USD layer headers
48055 " 🔄 Extract MaterialBindingAPI application into reusable function
48053 " 🟣 Added comprehensive test for hold outranking wire and auto-expiration behavior
48056 " 🔴 DomeLight ambient now applies exposure scaling correctly
48057 " 🟣 Material binding now guaranteed to be schema-legal
48058 10:14a 🔵 Disk full blocks test run for lunco-cosim crate
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


Access 523k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>