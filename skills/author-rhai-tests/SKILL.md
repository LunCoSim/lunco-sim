---
name: author-rhai-tests
description: >
  Author and review LunCoSim behavioral, asset-backed, component, mission,
  visual, and requirements-verification tests. Use when a test observes a
  Twin, USD stage, Modelica participant, runtime asset, or authored policy;
  keep the test in the Twin's Rhai scenario and run it through production.
  Use Rust tests only for generic engine mechanisms that Rhai cannot observe.
---

# Author tests in the right layer

Use this skill whenever a test is intended to prove what a mission Twin does,
what an authored component looks like, whether a USD relationship is wired,
whether a Modelica participant responds, or whether a public runtime command
or query produces the required result.

## Ownership rule

The default decision is simple: if the test includes an authored asset or
runtime behavior, author it in Rhai beside the Twin asset and execute it with
the production `luncosim` scene-test/runtime surface. This includes USD,
SysML/KerML, Modelica, terrain, materials, component files, scene paths, and
visual evidence. A Rust implementation does not make an observable behavior a
Rust-owned test.

Rust tests are reserved for generic mechanisms that Rhai cannot observe
without already depending on the mechanism under test: pure math/lowering,
parser and schema contracts, serialization, generic path/identity resolution,
and lifecycle/resource seams. Keep those fixtures inline or temporary and
generic; do not embed a repository or Twin asset path in a Rust test. A Rust
resolver test may prove the generic TwinRoots contract, but it must not become
a second asset acceptance runner.

The source-of-truth split is:

| Fact or behavior | Authoritative source | Test owner |
| --- | --- | --- |
| Prim identity, topology, transforms, dimensions, materials, physics schemas | USD | Rhai scene gate |
| Requirement intent, traceability, scalar limits, verification names | SysML/KerML | Rhai verifier reading the mounted SysML report |
| Continuous equations and participant state | Modelica | Rhai observer through the public runtime surface |
| Sequence, stimulus, policy, verdict, visual inspection rubric | Rhai | Rhai |
| Generic parser, resolver, serializer, or engine invariant | Rust owner crate | Rust unit/integration test |

Do not copy a threshold, prim path, clock value, or component parameter into a
test when it is already authored in SysML or USD. Load the authoritative
source, then use the generic helpers in
`assets/scripting/tools/sysml_requirements.rhai` and the public query/command
surface to evaluate it.

For Modelica observable publication, use the required `ModelicaVariables.values`
reflection surface and public output ports. `modelica_variable_projection`
checks snapshot retention while paused, then resumes the solver after a public
input command to compare the native snapshot, output port and accepted result.
`modelica_variables_missing` removes
required state and uses the runner's expected-runtime-fault verdict for
`modelica-variables-missing`; a stopped clock must not leave the test waiting
for another tick. Buffered-result rejection remains a low-level lifecycle seam.

## Authoring workflow

1. Split the behavior into the smallest independently verifiable component
   (bus, leg, wheel, ramp, panel, tank, joint, controller, or mission phase).
   Give the component its own authored requirement/verification mapping and
   Rhai observer when the contract is independently useful.
2. Inspect the owning USD/SysML/Modelica source and existing tool libraries
   before adding helpers. Put reusable mechanics in a namespaced Rhai library;
   keep the observer short and declarative. If a helper needs engine state that
   the public API cannot expose, add one generic Rust capability at its owner,
   then consume it from Rhai.
   Preserve unsigned document, generation and admission identities. Use `()`
   for a missing document rather than comparing its `u64` id with a negative
   sentinel. Compare identity strings exactly; use checked `parse_int` only
   when a bounded unsigned tick must be compared with signed `sim_tick()`.
   A helper that changes a state map must return it and the caller must assign
   it back to `this`; a terminal verdict must stop the observer's later work.
3. Make positive conformance evidence the default: prove that the required
   component, topology, relationship, datum, or runtime outcome is present and
   correct. Do not write a negative test merely to assert that an obsolete
   implementation name, old shape, or superseded path is absent; update the
   positive requirement and observe the required result instead.

   Add a negative case only when rejection or safe failure is itself a real
   contract, for example malformed source, non-finite values, missing
   safety-critical relationships, invalid units or clocks, stale-generation
   mutation, unsupported commands, or a required fail-safe response. Such a
   case must be bounded, non-destructive, and reach the public
   diagnostic/verdict boundary without crashing or silently substituting a
   default. A historical regression example alone is not sufficient reason
   to add a negative test.
4. For requirements, read the mounted SysML snapshot and evaluate composed USD
   evidence. Keep the requirement ID/limit in SysML and emit structured check
   evidence (name, measured value, units, criterion, source revision, and
   pass/fail) from Rhai.

   For support-gated motion, distinguish authored geometry from runtime state:
   `QueryPhysicsState.support_footprint_count` is only the number of declared
   probes, while `support_contact_count` is the latest evaluated contact count
   and `support_sample_tick` proves freshness. Treat a missing (`null`) contact
   count as unavailable evidence and fail the check; never infer contact from a
   non-empty footprint.

   For collider acceptance, read `QueryPhysicsState.collider_present` and
   `collider_source_paths` after admission. Authored `PhysicsCollisionAPI` and
   geometry queries alone do not prove that the intended live body contains a
   shape. Declare each queried body in `entity_reads`. Use
   `contact_debug(body, other)` for exact narrow-phase pair evidence, including
   the other body's wheel hierarchy; terrain contacts cannot establish rover-leg
   contact. For compound hardware, read `contact_debug(...).parts` and require
   `source1` or `source2` to name an admitted source, with a real applied
   `normal_impulse`. The enclosing body's response alone cannot identify its
   contacting member. Observe impact ticks without gaps, assert native penetration and
   geometric clearance, and verify final support on every required pad.
5. For visual requirements, define the camera, lighting/time contract,
   reference artifact, measurable geometry/placement rubric, and capture
   window. A screenshot is evidence only when the observer records the exact
   camera/source revision and verdict; visual inspection must not be reduced to
   an unbounded “looks good” assertion.
6. Run the narrowest production gate. Use `--validate` only as parse/preflight
   evidence; it is not a behavior or visual verdict. For a live Editor session,
   use `RunRhai`/`run_rhai_test.sh` or an attached `RunScenario` and preserve
   the current process and camera. Do not rebuild Rust for a Rhai-only change.
   A terminal runtime fault ends stepping immediately. Declare
   `expect_runtime_fault(kind)` before triggering an intended negative case;
   unexpected kinds fail with the owner's diagnostic.
   Scene discovery is recursive and uses the owning Twin's prepared asset-path
   context, including USD search paths. Require `luncosim test --list` to resolve
   the complete catalog before a suite pass. Give independent Editor fixtures nested
   one-scene directories so each windowed run mounts its own Twin and preview
   state. The production test wrappers give each process a run-scoped
   `LUNCOSIM_CONFIG`, isolating saved workspace/session state as well as
   ephemeral settings and runtime overlays.
7. Repeat with deterministic clocks and explicit seeds. Record the clock
   contract, timestep/substeps, source revision, and finite-state result. A
   repeatability check compares the same sampled evidence, not merely a zero
   exit code. For an owner-only hook, exercise valid context through its real
   production owner and rejection from an authored off-cycle invocation. Use
   `RunRhai`'s `Application/Repl/Evaluation` route for deliberate policy
   inspection; it does not stand in for the live owner context.
   World-bound `RunRhai` requests drain one per application update in FIFO
   order, and excess queue submissions or over-budget invocations return
   terminal errors. Keep public command behavior assertions in authored Rhai;
   test only the generic batch and FIFO seam in Rust.

For cross-run determinism, keep profile selection and state comparison in the
authored Rhai test. It reads the runner's typed parameters, selects a matching
profile from `scripts/tests/fixtures/deterministic-physics-reference.json`, and
requests only the expected row for each selected state. Rust decodes that JSON
at the `luncosim test --determinism-reference PATH` process boundary and serves
typed rows through the existing `query(...)` bridge. Rhai compares the six
selected physics checkpoints, the sparse Modelica checkpoints, articulated
checkpoints, and the explicit final stage with exact equality
(`numeric_tolerance=0`). It also deliberately alters one selected physics row
and verifies the comparison rejects it. It retains only the selected tick
numbers and the first mismatch message; it does not accumulate a state trace or
emit a result bundle.
`report_verdict` and the scene-test process exit code are the completion
contract. The Bash and PowerShell matrices in
`scripts/test-deterministic-physics-profiles.sh` and
`scripts/test-deterministic-physics-profiles.ps1` invoke the production test
for each profile and stop on a nonzero exit; they do not parse logs or compare
JSON. Rhai print/log lines remain diagnostics.

When refreshing that reference, capture an identified pre-change production
artifact, then require its uninstrumented authored gate to replay every profile
before testing the candidate against those same rows. Preserve profile inputs,
checkpoint ticks, identities, zero tolerance and the deliberate mismatch check.
Review drift from an older fixture against the current physics/Modelica owners;
a fresh capture alone is not proof that the candidate preserves behavior.
Record pool width from `clock_snapshot().physics_compute_threads` with
`physics_profile_known=true`; CLI `threads=0` alone does not observe the width.

For an intentional owner-contract repair that changes initialized state, first
demonstrate the fault and trace state differences to the authored inputs and
equations. Compare complete selected rows across Compute widths and jitter
before replacing the reference, then replay every profile with diagnostic
instrumentation removed. Keep zero tolerance and the negative control, identify
the artifact and inputs, and report the behavior change explicitly; paired
determinism does not imply preservation of the previous trajectory.

A finish helper must update the actor's actual state before reporting. For a
script-defined receiver method, use `this` in its body and call
`this.finish(...)`; passing the state map as an ordinary function argument
copies it. Verify a runtime gate emits one terminal verdict and stops querying
or mutating its completed scenario during subsequent ticks and scene replacement.

A fresh-scene startup test asserts that `on_start` observes tick 0 and the first
`on_tick` observes tick 1; startup readiness must hold the shared clock until
those callbacks can begin in order.
For USD Modelica networks, the startup gate also covers member-source resolution,
network synthesis, and generated port-surface publication; the binding epoch
must not classify authored connections while that interface is pending. Solver
compilation publishes the initialized time-zero state; do not advance Modelica
or physics clocks to prime a first exchange before opening the scenario gate.
The first live exchange uses the shared fixed tick and its normal causal barrier.
Keep the production scene-test's connection diagnostics in the pass condition.
Do not normalize startup ticks or reset elapsed time to make a trace begin at
zero. The production fixed runner completes a started causal cycle and retains
its remaining fixed time while an owner hold is active; verify startup stays at
tick 0 with no fixed elapsed time or overstep before readiness.
Never subtract a scenario start tick or translate traces to a relative tick
sequence. Match equivalent authored subjects by their USD-owned stable facts,
and omit ECS allocation and wall-clock timing from the comparison.

For observable multi-actor ordering, attach scenarios to distinct authored
hosts and assert the same-pass handoff in Rhai. Keep Rust coverage for the
generic identity key and reverse-completion commit mechanism; a Rust assertion
alone does not prove the production script path.

When a public query reports asynchronous analysis, a test may sample that
query from its test-only `on_tick` until the exact requested generation reaches
a terminal state. Assert `pending` as retryable and inspect diagnostics only
after `ready`; do not use elapsed wall time to decide which result is current.
If a running scenario needs those facts before it can initialize, declare the
owner and identity in `simulation_dependencies(...).required_inputs` and assert
from `on_start` that the committed source revision is available. The generic
scenario lifecycle test may verify Pending-to-Ready hold/release mechanics;
the authored scene gate must verify the domain key and source revision.

For transient USD curve views, use `InspectUsdCurveView` to wait until
`completed_revision == requested_revision`, fail immediately on `state ==
"failed"`, and require `applied_revision == requested_revision` for a successful
mesh. Assert `local_visibility` and result vertex count rather than reading the
unchanged USD curve seed. A route with fewer than two active points must finish
with hidden local visibility and zero generated vertices; adding the second
point must produce an applied mesh revision.

For ordered asynchronous scene checks, author the sequence with the existing
Rhai task tree (`seq`, `once`, `wait_until`, `check`, and `sel`) rather than a
numeric `phase` switch in `on_tick`. Use named `Fn("callback")` leaves when a
step needs persistent test state; the task driver binds that state's `this` to
the callback. Prefer `wait_for`/`wait_for_from` when the owner publishes the
completion event; use `wait_until` only when no suitable event exists. `wait`
uses deterministic simulation time, not wall time. For an interruptible
sequence, put a cheap state guard in `reactive_seq` and let a failed `check`
cancel its running child; event handlers can update that guard's state, which
the task kernel observes on its next deterministic pass. Keep test-only
`on_tick` for a bounded fixed-step watchdog that reports the exact condition
that timed out. Do not add a separate Rust timer callback or phase runner: task
progression already owns deterministic waits and callback cadence. Any future
task deadline must specify its clock and cancellation/failure result as part of
the task contract. The shared `auto_tests.rhai` prelude already owns assertions
and terminal verdicts; do not add a parallel test DSL unless the Rhai task
surface demonstrably cannot express a required contract.

## Production commands

Resolve the production binary once:

```bash
export LUNCOSIM_BIN="${LUNCOSIM_BIN:-luncosim}"
./scripts/run_scene_tests.sh --no-build --exact <scene-name> -j 4
```

Use `-j 1` when diagnosing ordering or nondeterminism. Keep the production
binary and API session explicit; never substitute an old sandbox executable,
`cargo run`, or a temporary Rust runner. For same-session iteration, register
or reload the Twin Rhai library, run a minimal namespaced call, then execute
the observer through the API as described by
[`test-via-api`](../test-via-api/SKILL.md).

## Review checklist

- Is this observable behavior or an authored asset? If yes, it is Rhai-owned.
- If a test loads, composes, edits, or inspects a USD/Modelica asset, author it
  as a Twin Rhai scenario. Keep Rust tests for pure, asset-free engine
  primitives and routing predicates; do not embed fixture documents or asset
  identifiers in core tests.
- Does the test load the real Twin/USD/Modelica source rather than recreate it?
- Are requirements and dimensions read from SysML/USD instead of duplicated?
- Is the component independently scoped and its evidence structured?
- If a negative case exists, is rejection or safe failure an explicit contract,
  and does it have a named, non-crashing diagnostic? Do not require a negative
  case for ordinary conformance or obsolete-implementation cleanup.
- Are units, coordinate frame, camera/time contract, and deterministic clocks
  explicit where they affect the result?
- Is canonical numeric state kept as native `f64`/USD `double`, with any
  `f32`/`float` conversion explicit and limited to a renderer/GPU boundary or a
  USD field whose schema requires it?
- Is the test short enough to reuse libraries rather than becoming a batch
  builder or a second runtime in Rhai?
- Did the run use the production binary/API and prove a real verdict?

Defer to [`sysml-requirements`](../sysml-requirements/SKILL.md) for SysML
source-set and traceability rules, [`interactive-component-authoring`](../interactive-component-authoring/SKILL.md)
for the one-component Editor loop, and [`validate-assets`](../validate-assets/SKILL.md)
for parse/lint preflight.
