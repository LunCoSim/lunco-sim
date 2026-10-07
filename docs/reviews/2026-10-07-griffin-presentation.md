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

### Operator controls and sky

The live mission could complete with `program_active` and `autopilot_enable`
still set, suppressing manual drive and steering while the independent brake
worked. Its terminal branch now explicitly stops guidance. Its target-scoped
`intent.edge` subscription cancels the complete mission route branch on a
pressed or pulsed controlled-rover command, clearing only guidance flags and
preserving the controller's input. Route leaves cannot restart after this
interruption.

The actor-bound production handler regression passed four policy checks and
four admitted-port checks. Foreign sources, wrong targets and release edges
leave guidance unchanged; the exact pressed controlled-FLIP event interrupts
it and preserves all three manual input values. The temporary observer was
then detached, and native Space release restored all manual inputs to zero.

An unpossessed terrain click exhausted Rhai's aggregate string budget while
discovering route programs among FLIP's CAD hierarchy. Route discovery now
requests only topology and relationships, deduplicates pointer candidates,
and traverses branches in batches of 16. The actual live scene-interaction
owner accepted the unpossessed terrain context and created `FlipRoute/P4`.
The authored route gates also cover rejection when multiple routes are
ambiguous; the main windowed fixture was not rerun in this phase.

Native W, S, A, D and Space press/release each passed 11 authored assertions
against the exact ready Twin, including possession, other inputs at zero and
guidance disabled. The driver is `scripts/api/test_flip_manual_controls.py`;
its assertions are Twin-owned Rhai. Earlier W/A output observations also
recorded the wheel filter decaying after release and a 0.011729 rad/s final yaw
rate. The gate requires FLIP possession and fails visibly if it is absent.

The default scene references the shipped procedural sky component, authored
through the document API on its persistent root layer. The runtime composed
prim is spawned with `LunCoProceduralSkyAPI`; the rendered capture shows the
starfield and physical Sun disk supplied by the existing celestial Sun.
Runtime waypoints remain in their transient layer and were not saved.

- `target/griffin-completion/native-control-regression.log`: ten native input
  samples, all PASS, with 110 authored assertions.
- `target/griffin-completion/manual-native-input.json`: W/A owner values,
  motor outputs and native release settling.
- `target/griffin-completion/waypoint-no-selection.json`: successful live
  scene-interaction owner append without a controlled/selected path.
- `target/griffin-completion/mission-manual-handoff.log`: final policy PASS 4
  and committed-port PASS 4 using the exact production `on_event` handler.
- `target/griffin-completion/final-controls-handoff.json`: ready world,
  zero runtime diagnostics, disabled guidance and released manual inputs.
- `target/griffin-completion/sky-document-inspection.json` and
  `sky-controls.png`: exact document/layer inspection and rendered sky.

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

## Detachment and operator route ownership

After deployment, automatic mission driving and the operator route could both
write FLIP's guidance inputs. A HUD stop only stopped the operator route;
the mission continued writing until its route completed. The Twin mission now
retires its driving branch on the exact operator script host's `route_started`
or `route_stopped` event. It preserves the new operator program's writes.
Deployment also uses `AcquireControl` with `bind_camera: true`, handing the
local keyboard and camera to the same FLIP that the HUD controls.

The authored `griffin_operator_route_handoff.rhai` observer exercises HUD
start/stop while mission guidance is active after adapter retirement following
`RestartScene`. It requires 120 ticks of disabled guidance and native W
press/release without another possession. Runtime waypoints are supplied as
transient setup through the route editor, not saved to the authored scene.
The fresh-reload run passed all six authored checks in
`target/griffin-completion/detach-handoff-final.log` at 123.516667 scene seconds.
The production mission yielded on `route_started` before its automatic drive
could complete. The expanded exact-handler regression also passed ten policy
checks and four admitted-port checks in the same log. Both temporary observers
were stopped, test keys released, and the transport returned to 1x.

A separate repeated `OpenTwin` of the same active root exposed a mount-alias
failure: the new mount received a suffixed namespace while authored references
retained the canonical namespace. This is not resolved by the Twin control
change. A fresh application launch avoids that repeated-open path. No broad
Twin close/reopen acceptance is claimed.
