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
3. Write a positive and a negative case. The negative case must reach the
   public diagnostic/verdict boundary and must fail loudly without crashing or
   silently substituting a default. Missing assets, bad relationships,
   non-finite values, wrong units, and wrong clocks are real failures.
4. For requirements, read the mounted SysML snapshot and evaluate composed USD
   evidence. Keep the requirement ID/limit in SysML and emit structured check
   evidence (name, measured value, units, criterion, source revision, and
   pass/fail) from Rhai.
5. For visual requirements, define the camera, lighting/time contract,
   reference artifact, measurable geometry/placement rubric, and capture
   window. A screenshot is evidence only when the observer records the exact
   camera/source revision and verdict; visual inspection must not be reduced to
   an unbounded “looks good” assertion.
6. Run the narrowest production gate. Use `--validate` only as parse/preflight
   evidence; it is not a behavior or visual verdict. For a live Editor session,
   use `RunRhai`/`run_rhai_test.sh` or an attached `RunScenario` and preserve
   the current process and camera. Do not rebuild Rust for a Rhai-only change.
7. Repeat with deterministic clocks and explicit seeds. Record the clock
   contract, timestep/substeps, source revision, and finite-state result. A
   repeatability check compares the same sampled evidence, not merely a zero
   exit code.

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
- Does the test load the real Twin/USD/Modelica source rather than recreate it?
- Are requirements and dimensions read from SysML/USD instead of duplicated?
- Is the component independently scoped and its evidence structured?
- Is there a meaningful negative case with a named, non-crashing diagnostic?
- Are units, coordinate frame, camera/time contract, and deterministic clocks
  explicit where they affect the result?
- Is the test short enough to reuse libraries rather than becoming a batch
  builder or a second runtime in Rhai?
- Did the run use the production binary/API and prove a real verdict?

Defer to [`sysml-requirements`](../sysml-requirements/SKILL.md) for SysML
source-set and traceability rules, [`interactive-component-authoring`](../interactive-component-authoring/SKILL.md)
for the one-component Editor loop, and [`validate-assets`](../validate-assets/SKILL.md)
for parse/lint preflight.
