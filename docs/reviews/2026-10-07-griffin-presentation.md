# Griffin presentation acceptance — 2026-10-07

The production binary in this checkout opened the exact canonical Twin scene:
`/home/rod/Documents/models/lunar-base-model/twins/astrobotic-griffin-1/scenes/griffin_1_surface_ops.usda`.
Its authored surface-operations verdict passed all 16 checks, including landing,
ordered physical ramp deployment, adapter retirement, native DEM wheel contact,
and all route destinations. The owned window used API 4117 with throttling and
vsync disabled. The warm repeat used the supported time transport at 2x after
its initial descent interval; this is mission acceptance, not an FPS benchmark.

## Modelica ownership

All workspace Rumoca dependencies resolve to fork commit
`c3ef5b308054778b2cfd90b4408ce4c856aadfc6`, pushed to the LunCoSim fork's main
and `fix/stream-connection-semantics` branches. Local vendored copies and their
Cargo patches are removed. Source provenance, scoped input initialization,
cooperative cancellation, output budgets, and output producers share that pin.
The structural phase retains equations producing outputs also used as state
derivatives. Prepared solver cache version 7 excludes prior eliminated graphs.
Ten focused fork mechanism tests passed before the production integration.

The source-root compiler publishes parsed namespace identities through the
existing operation-fenced worker acknowledgement. Compile admission resolves
names to the unique scoped owner. Standard `within;` denotes global scope; it
must not generate an empty namespace prefix. The actual shared `LunCo` library
was admitted as `["LunCo"]`, and every Griffin Modelica system compiled and
prepared successfully. Source and verification registry errors were empty;
the sampled runtime diagnostics had zero errors and zero warnings.

Cold preparation reached readiness after approximately 127 seconds, including
114.44 seconds lowering MainPropulsion. Warm MainPropulsion preparation took
34.58 ms, AttitudePropulsion 9.28 ms, AttitudeActuation 8.73 ms, and FLIP 1.64 ms.
The warm scene reached readiness before the first driver sample. No readiness
hold, physics admission, or clock boundary was bypassed.

## Twin physics and visible trails

Twin commit `a547140` removes FLIP's global linear damping in lunar vacuum.
Tire forces retain ground resistance. With the attached payload, the damping
force previously produced an overturning moment during pitched ascent. The
unchanged normal pilot acceptance now passes all 9 checks; minimum upright Y
was 0.996059 and maximum recovery angular rate 0.032515 rad/s. Diagnostic
isolation retains explicit native poses and attachment retirement boundaries.
Pilot recovery has unique requirement `GPP-013` and source provenance.

The actual surface mission published all four wheel trails without errors.
The final rendered capture shows both tracks reaching FLIP. This bounded run
did not reproduce the reported cutoff and makes no claim of fixing an unseen
renderer defect. Source histories and screenshots are paired in the evidence.

## Evidence and limits

- `target/griffin-completion/presentation-final.log`: `TESTS_OK 16`, surface
  operations PASS, successful Modelica compilation and cache preparation.
- `target/griffin-completion/presentation-driver.log`: readiness transitions,
  typed HUD actions, native vehicle/trail samples, and verified API shutdown.
- `target/griffin-completion/presentation-027.{json,png}`: final moving vehicle
  and rendered tracks. Earlier captures cover ramp egress and the route.
- `target/griffin-completion/pilot-vacuum-damping.log`: normal pilot PASS 9.
- `target/griffin-completion/presentation-RuntimeDiagnostics.json` and
  `presentation-ReadActiveTwinContract.json`: sampled diagnostics and contract.
- `target/griffin-completion/rumoca-owner-tests.log`: ten fork mechanism tests.
- `target/griffin-completion/presentation-rebuild.log`: final production build
  with `networking,sysml` passed.

Windows-native, browser/wasm, full-suite, aggregate-output production regression,
and separate package-wrapper namespace acceptance were not run in this demo
completion phase. Authored aggregate-output coverage is supplied for follow-up.
Performance outside preparation timings remains unaccepted.
