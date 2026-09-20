# SysML-to-Modelica geometry constraint workflow

**Status:** Typed analysis, Rhai constraint generation, asynchronous Modelica
solve/readback, and typed Editor proposal foundations are implemented. The new
nonzero integration gate and any Griffin geometry move still require a fresh
production Editor/runtime run.
**Reviewed:** 2026-09-20
**Scope:** Generic Rust/Rhai/SysML/Modelica seams needed to place and size
Griffin components from a single authored source of truth.

## Ownership contract

The intended one-way flow is:

```text
SysML typed values + constraint usages
  -> generic Rust SysML analysis snapshot
  -> Rhai selects a constraint policy and assembles Modelica source
  -> Modelica solves/measures once
  -> Rhai turns the result into a reviewed typed USD edit plan
  -> Editor applies that plan to the explicit document/edit target/generation
```

Rust owns parsing, resolution, native value types, Modelica execution, and
typed Editor operations. It must not decide that a project-specific `Constraint`
means a particular Griffin part arrangement. Rhai owns policy selection,
binding, and generated-model assembly. SysML owns engineering values and
source references. Modelica owns equations. USD owns scene topology and
geometry. The Editor owns visible, journaled scene changes.

Keep policies small and composable rather than defining one universal CAD
solver. Structural SysML lint, requirement/provenance verification, and
geometry transformation are separate Rhai policy surfaces today. Geometry
transform families (for example, segment measurement, coincident interfaces,
symmetry, and axis alignment) can be separate Rhai policy functions that emit
reusable Modelica components. Shared Rust APIs provide typed source facts and
Modelica execution; they do not choose which policy applies.

No CAD/BREP kernel is needed for this layer. A small library of algebraic
relations (coincident points, segment length/midpoint, symmetry, axis alignment,
and fixed offsets) is enough for deterministic placement and sizing; it is not
a general-purpose sketch solver.

## Implemented foundation

- `AnalyzeSysml` exposes the existing source-backed typed analysis facts through
  the in-process `HookValue` API. Generic typed table and attribute-name
  selection keeps policy queries bounded; it does not filter constraints by
  meaning, infer engineering intent, or parse opaque constraint-expression
  text.
- `ValidateSysml` reports source diagnostics and structural `lint.sysml`
  findings. `AnalyzeSysml` now bypasses that policy and returns source facts
  independently; Rust no longer stores a second JSON-shaped semantic graph or
  projects requirement, provenance, collision, or Twin binding records into its
  validation report.
- `ReadActiveTwinContract` exposes the existing typed Twin manifest registries
  through the workspace API. `sysml_requirements.rhai` joins those manifest
  records with source identities and shapes evidence in Rhai.
- Policy boundaries are distinct: `lint.sysml` checks structural/source
  quality, `sysml_requirements.rhai` evaluates requirement and provenance
  contracts, and `sysml_modelica_constraints.rhai` selects geometry constraints
  and assembles Modelica. They share one generic analysis source rather than
  growing a project-specific Rust validator.
- `sysml_modelica_constraints.rhai` selects definitions/usages from the generic
  fact graph, follows standard SysML `BindingConnectorAsUsage` relationship
  ends to resolve each definition input to its authored part feature, requires
  standard `LengthValue[3]` inputs with explicit metre units before lowering
  them to the shared f64 Bevy/Rhai `Vec3`, and assembles a Modelica source
  wrapper. It can submit that source through the explicit Modelica Editor
  document API. Endpoint names are not repeated in the Rhai caller.
- `LunCo.Geometry.Segment3D` derives a directed segment's length, midpoint,
  and unit axis from its endpoints. Re-running the current source with
  `cargo run -p lunco-modelica-execution --bin modelica_run --
  assets/models/LunCo/Geometry/Segment3D.mo LunCo.Geometry.Segment3D --duration
  0.01 --dt 0.01` compiled and stepped successfully; both samples returned
  `length=1.0 m`, `midpoint=(0,0,0.5) m`, and `axis=(0,0,1)`. Distinct endpoints
  are an explicit Modelica assertion.
- `LunCo.Geometry.CoincidentPointTranslation3D` defines an algebraic solve for
  the translation that makes a moving point equal a fixed point. Its matching
  Rhai translator resolves both points from standard SysML binding connectors
  and generates a Modelica wrapper. The relation assumes fixed orientation;
  solving rotation remains out of scope. The standalone `modelica_run` CLI
  still only gives compile/step evidence for this zero-start component; the
  generated-wrapper path now uses the Editor's batch experiment API and checks
  a nonzero result instead of treating that CLI run as numerical proof.
- `RunExperiment` now completes its API command acknowledgement only after the
  Modelica owner has registered the run, returning that exact experiment id.
  Rhai no longer correlates a run by a mutable display label or `ListRuns`
  ordering. The generic ticket checks document id/source generation, polls
  without blocking using bounded exponential frame backoff, has a finite poll
  budget and cancellation, and reads finite `f64` timeseries from
  `GetExperimentResult`.
- `CreateNewScratchModel` now returns its exact allocated `doc_id` in the
  programmatic command result. Rhai no longer discovers a scratch document by
  diffing open-tab lists, which could select another document opened at the
  same time. The UI handler records that result only when an API command
  context is present, preserving ordinary UI invocation without API resources.
- `assembly_editor_proposal.rhai` now exercises one generated nonzero
  SysML→Modelica→typed placement proposal round trip. It reads back the
  uncommitted proposal by its returned proposal identity and operation list,
  checks the exact Editor document generation and Modelica source generation,
  rejects the fixture-only proposal, and closes its test-owned scratch model.
  This is an authored runtime gate, not a result observed in this work session.
- `test_sysml_constraint_projection.rhai` positively checks the authored
  bindings, automatic endpoint identities, declared `Length` cardinality,
  preserved `Quantity` values, and explicit metre validation before native
  vector lowering. Production Rhai assertions still need to run on a fresh
  production process; port 37432 belongs to a stale, deleted executable image
  and is left untouched.
- `luncosim --validate` rejected the outer-scope binding form
  `bind loadPath.start = upperMount;` at its dotted path, while the nested
  constraint-usage form parses. Keep standard binding connectors and track
  full chained-end syntax as a parser capability; do not invent an endpoint
  mapping mini-language. The normative notation is in the
  [OMG SysML v2 language specification](https://www.omg.org/spec/SysML/2.0/Language/PDF).
- Production Rhai test `test_sysml_policy_isolation.rhai` passes three checks:
  source validation applies structural lint, while `AnalyzeSysml` remains
  available to independent policies over the same successfully parsed source.

Prior checks prove generic analysis transport, Rhai-owned selection, a numerical
`Segment3D` run, and compile/step viability for the coincidence component. The
new Rust acknowledgement path and Rhai integration gate have not yet run in a
fresh production host: current-session validation was intentionally not
attempted because the only known API process is the stale deleted executable on
port 37432. Therefore no nonzero generated-wrapper result or Griffin USD edit
is claimed here.

## Remaining high-impact gaps

| Priority | Gap | Needed capability / owner |
|---|---|---|
| P0 | The nonzero generated-wrapper solve/readback path and deferred run-identity acknowledgement are implemented, but not production-verified in this session; general rotational placement remains unsolved. | Run `assembly_editor_proposal` in a fresh current production host and confirm its positive numerical and proposal assertions. Add fixed-distance, symmetry, and axis-alignment relations only as actual Griffin source inputs require. |
| P0 | Binding-based endpoint resolution and the typed `RunExperiment` acknowledgement are authored but the end-to-end acceptance gate has not run against a fresh production process. | Confirm each binding resolves exactly one vector feature, the returned experiment id addresses the same result, and the proposal preserves both source and USD generations. Extend generic AST projection only if the live typed graph is insufficient. |
| P1 | The only local Editor/API process is a stale deleted executable that must remain untouched, so current production Rhai tests cannot run. | Start/use a fresh current production session when available; do not kill or trust the stale process. Keep the portable `modelica_run` checks separate from Editor/Twin acceptance. |
| P1 | The current parser rejects the standard dotted endpoint form when a binding is declared in the containing part, although the nested usage form parses. | Verify the pinned parser's intended subset; if this is a supported SysML form, add generic chained-end resolution and a positive Rhai fixture. Keep source expressions and metamodel relationships authoritative. |
| P1 | A Rhai-generated static solve currently goes through a scratch Editor document, source replacement, compile polling, `RunExperiment`, and result readback; there is no standalone typed in-memory source-to-solve entry point. | First run the authored Editor integration gate on a fresh production process. If the document/tab lifecycle is too stateful for repeated design solves, add a generic typed one-shot compile/solve/readback service in Modelica core and call it from Rhai; keep constraint interpretation and model assembly in Rhai. |
| P1 | The fixture gate authors and inspects a reviewed typed solve-result proposal, but no Griffin result has been reviewed/applied in the visible Editor. | In the live Editor, inspect the real Griffin document, edit target, component path, local-frame convention, and generation; build and review the source-bound placement plan there. Commit only after visual and requirement review. |
| P1 | The active-Twin manifest join is implemented, but the current headless acceptance test has no Twin mounted and verifies only the closed-session response. | Add a Twin-backed production Rhai gate for source discovery, component/verification binding, and provenance once the headless fixture can open and activate a Twin. |
| P2 | Fact selection is bounded by table and attribute selectors but not paginated. | Add paging only if representative source sets demonstrate that selected facts still exceed the Rhai value budget. |

## Griffin-specific model input still required

Before a solve can move a Griffin part, its SysML model needs real endpoint
features and constraints, not just numeric render offsets. For each strut, the
source must identify the body-side joint centre, foot-side joint centre, local
frame, and any authoritative length or angle. A constraint usage must bind to
those features through resolvable SysML references. The generated Modelica
instance must derive centre, span, and axis from those source values. The
result must then be compared against the authored USD local frame before any
Editor operation is proposed.

For the landing foot, encode the user's specified hemispherical pad as a typed
shape choice/parameterized component, not a cylinder renamed in a script. The
strut bottom endpoint must meet the foot's load interface; the foot contact
surface must meet the support datum. A visual clearance check should prove the
strut/pad envelopes do not interpenetrate. The currently observed historical
strut/plate/pad coordinates are diagnosis clues only, not accepted source data.

Each Griffin dimension still needs its existing source/rationale record or a
clearly labeled study assumption. Constraint equations must not create a second
dimension table inside Rhai or Modelica; generated parameters are compiled
from the selected SysML snapshot and carry its source revision into the result.

## Next sequence

1. Keep Rust generic and preserve the current separation among structural
   lint, requirement/provenance, and geometry-transformation Rhai policies.
2. Run the new coincidence-translation Modelica solve and production Rhai
   binding/proposal gate; then add one geometric relation translator at a time.
3. Author actual Griffin endpoint features and constraints through the SysML
   Editor, then use the Modelica runner to derive strut placement and foot
   interface geometry.
4. Review and apply only the resulting typed operations in the visible USD
   Editor perspective; run the Griffin requirement and landing/first-operations
   scene tests afterward.
