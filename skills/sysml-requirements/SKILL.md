---
name: sysml-requirements
description: >
  Author, inspect, validate, or verify SysML v2 requirements in a LunCoSim
  Twin. Use for `.sysml`/`.kerml`, requirements management, satisfy/verify
  traceability, `ValidateSysml`, `sysml_requirements`, source revisions, or
  `luncosim test --verification`. Do not use it to claim full KerML execution
  or to replace USD, Modelica, or Rhai ownership.
---

# Manage SysML v2 requirements

SysML v2 is implemented as a portable, source-backed requirements and system-
structure boundary. It is not a second runtime physics engine and it is not a
copy of the Twin's USD or Rhai state. Use this skill when the request mentions
requirements, verification cases, SysML, KerML, traceability, or a missing
capability that may already exist behind the optional SysML feature.

## Capability matrix and ownership

Check the feature set and owner before saying that a capability is missing:

| Concern | Authoritative owner | Normal availability | Use it for |
|---|---|---|---|
| System structure and requirement intent | standard SysML v2 `.sysml`/`.kerml` | default in the production app/core/server; `--no-default-features` remains available for a deliberately lean build | parts, ports, connections, requirements, satisfy/verify links, scalar literals |
| Scene identity, topology, geometry and authored physical facts | USD | standard runtime path | prim paths, schemas, relationships, dimensions, materials, physics topology |
| Continuous equations and domain state | Modelica | standard domain backend | propulsion, electrical, thermal and other continuous models |
| Scenario policy, observations, checks and verdicts | Rhai | default scenario backend | runtime observation, actuation, generic requirement evaluation and test policy |
| Bounded resolved-constraint IR | `lunco-sysml-ir` | default with SysML support | typed source-linked constraint compilation, diagnostics, fingerprints, and provider-neutral evaluation |
| Bounded SysML-to-Modelica lowering | `lunco-sysml-modelica` through the Rumoca boundary | default with SysML support | typed scalar/fixed-array equation lowering; not SysML resolution or engineering intent |
| Python integration | Python backend | `python` feature, opt-in | explicit one-shot or integration requests only; not the normal scenario workflow |
| Generic parsing, resolution, transport and lifecycle mechanisms | Rust crates | feature-dependent | reusable substrate, not Twin-specific policy |

The source-of-truth rule is strict: SysML states what the system must be and
why; USD states what is authored and observable; Modelica states continuous
equations; supported SysML constraint expressions execute through the neutral
IR. Rhai selects providers, queries observations, orchestrates behavior, and
formats evidence. A verification registry selects a scene and script but does
not duplicate requirement text or thresholds.

For mission models with material-dependent structural, pressure, thermal, or
electrical behavior, follow the
[physical-material data gate](../interactive-component-authoring/references/mission-engineering-quality.md#physical-materials-and-engineering-properties).
Use typed material definitions/usages and unit-bearing properties with their
conditions and evidence; do not encode material identity or property arrays as
strings. The current typed SysML projection is not itself a material catalogue
or an automatic cross-domain binding—keep the limit visible until a generic
resolver and consumer path are demonstrated.

Model substrate and surface finish/coating as separate typed facts. Permit a
component face/region to select a sourced finish independently of its bulk
material. Keep visual `UsdShade` shader mapping distinct from optical/thermal
engineering properties: a shader preset is not a source for absorptance,
emittance, coating thickness, or Modelica thermal parameters. Record missing
finish catalog and rendering adapters as generic tool gaps, not per-component
string metadata.

For a mission Twin, apply the generic
[mission and engineering quality gates](../interactive-component-authoring/references/mission-engineering-quality.md)
alongside this parser/runtime contract. Requirements are the baselined bridge
from mission intent and ConOps to component and interface acceptance; record
provenance, assumptions, operating conditions, margins, and configuration
revision. Keep verification against requirements separate from validation in a
realistic nominal/off-nominal/recovery scenario, and make unresolved TBDs or
fault-response requirements visible failures rather than defaults.

The existing Rhai lint substrate is part of this path: `RunLint` executes the
domain policy in `assets/scripting/policy/lint_<domain>.rhai` over Rust-produced
facts, while Twin verification scripts use
`assets/scripting/tools/sysml_requirements.rhai` to read the mounted SysML
snapshot, bind composed USD observations, execute supported source constraints
through the neutral IR, and emit structured evidence. Keep provider and
component selection in Rhai; keep supported acceptance predicates in SysML;
Rust supplies generic typed mechanisms, not mission- or component-specific
assertions.

Use `ValidateSysml` with the mounted Twin URI to review the full indexed source
set and its structural lint findings. The `lint.sysml` policy flags requirement
usages without typed source evidence, usages without an inherited or owned
`require` acceptance predicate (`assume` memberships provide context but do
not count as pass criteria), and identifier-like `String` values that appear
to encode enumerators or member identities. Treat those findings as migration
work, not as permission to preserve the encoding. Use an enum for a closed
choice set; use typed part usages or references for named system members. A
`verify` membership is required for each usage, and the Twin's verification
registry must bind that case to its scene and observer.

For example, from an in-app Rhai tool with the Twin mounted:

```rhai
let report = query("ValidateSysml", #{path: "twin://astrobotic-griffin-1"});
```

Review `report.errors`, `report.warnings`, and the structured
`report.findings`. Use its stable `rule`, `severity`, `subject`, and `message`
fields instead of parsing warning prose; SysML subjects include the qualified
identity and source file/byte offset. Pay particular attention to
`sysml-requirement-*` and `sysml-string-encoded-identities`.

Relevant implementation and design references:

- [`24-domain-sysml.md`](../../docs/architecture/24-domain-sysml.md) — domain
  boundary and supported subset.
- [`sysml-requirement-verification.md`](../../docs/architecture/sysml-requirement-verification.md)
  — current Rhai bridge and verification contract.
- [`crates/lunco-twin/src/manifest.rs`](../../crates/lunco-twin/src/manifest.rs)
  — `[sysml]` and `[verification]` manifest ownership.
- [`assets/scripting/tools/sysml_requirements.rhai`](../../assets/scripting/tools/sysml_requirements.rhai)
  — generic evaluator implementation.

## Requirement quality gate

Treat an informal request as a stakeholder expectation until it has enough
information to become a requirement. A good requirement is singular, clear,
necessary, feasible, implementation-independent, traceable, and individually
verifiable. It states one subject and one normative behavior or characteristic,
usually with `shall`, plus the measure, units, limit or tolerance, operating
condition, and verification method needed to decide pass or fail. This follows
the NASA requirements guidance for statements that are clear, correct,
feasible, unambiguous, measurable/verifiable, and traceable:
[`NASA Appendix C`](https://www.nasa.gov/reference/appendix-c-how-to-write-a-good-requirement/)
and [`NPR 7123.1C`](https://nodis3.gsfc.nasa.gov/displayAll.cfm?Internal_ID=N_PR_7123_001C_&page_name=all).

Before authoring or baselining a requirement, check:

- **Subject and scope:** Which system, part, interface, or operating mode does
  it constrain? Name the subject and the condition or stimulus.
- **Observable measure:** What will be measured, inspected, demonstrated,
  analyzed, or tested? Include units, sampling rule, time window, population,
  and reference/baseline when they affect the result.
- **Pass criterion:** Give a numeric bound, range, tolerance, count, rate,
  state, or explicit finite acceptance rubric. “Good”, “stable”, “fast”,
  “intuitive”, “looks right”, and “make it look good” are not pass criteria.
- **Single thought:** Split `and`, `or`, and compound paragraphs into separate
  requirements unless the clauses are inseparable for verification.
- **Traceability and intent:** Record the requirement ID, source/parent goal,
  rationale, priority, and the qualified verification case. Keep design choices
  such as a particular class, algorithm, or shader implementation out unless
  they are themselves the required constraint.
- **Normative content vs. implementation status:** Requirement text states
  the required behavior or characteristic and how it will be accepted. Keep
  current implementation status, nonconformance, incomplete assets, proxy
  caveats, missing source data, historical milestones, and unresolved work in
  the owning gap/status report. Keep source history and provenance in the
  typed evidence/source catalog. Do not use requirement prose to say that the
  current model fails, that a parameter is only a proxy, or that a value is
  not publicly available. Write the positive desired outcome in the
  requirement; report actual-vs-required status separately under that
  requirement ID.
- **Time-dependent acceptance:** Identify the applicable mission phase or
  window, time scale, site/reference frame, and temporal envelope or sampling
  needed to accept a time-dependent requirement. Use one authoritative scene
  clock/epoch and derive environment state from its providers; keep a study
  epoch distinct from a mission schedule. Put unavailable schedule inputs and
  current verification coverage in the gap/status report.
- **Failure behavior:** For safety, reliability, or resilience requirements,
  specify the trigger, detection deadline, required response, and recovery or
  safe state. “The system shall handle errors” is incomplete.

Do not baseline an unmeasurable statement merely because its rationale is
important. If a threshold, reference, or acceptance condition is missing,
record the unresolved decision and required evidence in the gap/status report.
Keep the requirement concise; put its sources in the evidence catalog and its
verification evidence in the verification record.

For example, a solar-layout requirement should state the required installation
relationship and the mission Sun-envelope comparison used for acceptance.
Whether the current Twin has the right panel count, whether its existing roots
are only proxies, and which installation data are still missing belong in the
gap/status report, not in the requirement definition.

### Ask the user when the requirement is underspecified

It is correct to ask targeted questions before writing SysML. Ask only the
questions that block a measurable, verifiable statement, and show the proposed
rewritten requirement so the user can confirm the interpretation. Do not invent
numbers, tolerances, reference images, operating conditions, or priorities.

Use this compact question set as needed:

1. What exact subject is being constrained, and in which operating condition or
   scenario?
2. What observable quantity or finite outcome proves success? What are its
   units, range/threshold/tolerance, sampling rule, and time window?
3. What stimulus, initial state, environment, or reference baseline applies?
4. Should verification be by test, analysis, inspection, or demonstration, and
   what artifact or runtime evidence must be retained?
5. What stakeholder goal or parent requirement does it trace to, and what is the
   priority if it conflicts with another requirement?

If the user answers only “make it look good”, convert that into a clarification,
not a requirement. Ask for the approved reference and observable visual
criteria—for example camera/lighting/exposure, reference patches or objects,
allowed luminance/color deviation, frame window, and the inspection or image
analysis method. A valid visual requirement can be:

```text
Under the approved camera, sun direction, and exposure, the terrain surface
shall keep the mean luminance of each of three approved reference patches within
±5% over a 10-second stationary capture. Verification: image analysis of 300
frames plus visual inspection against the approved albedo reference.
```

The numbers and reference artifacts in that example are placeholders, not
defaults. Replace them with user-approved values before putting the statement
in SysML. A similarly testable motion requirement names the scenario and
window, for example: “During a 100 m straight run on a 10° slope at a commanded
1.0 m/s, the rover shall keep lateral error ≤0.5 m and speed within ±0.2 m/s for
at least 90% of samples after the first 5 s.”

Do not weaken a vague request by choosing a permissive tolerance merely to make
the first run pass. If the user cannot yet provide a bound, record it as an
open stakeholder expectation/TBD with an owner, rationale, and resolution
date; do not baseline it as a verified SysML requirement.

## What is implemented, and what is not

The current supported subset is source-backed and deterministic:

- standard textual SysML v2 syntax in `.sysml` and `.kerml` files;
- packages/imports, part definitions/usages, ports, connections, requirement
  definitions/usages with documentation and attributes;
- standard external references plus `satisfy` and verification `verify`
  memberships;
- qualified names, typed scalar/vector/array literal projections, source spans,
  diagnostics, source files, and a deterministic `source_revision`; numeric
  literals expose a validated native finite `number_value` alongside source
  identity;
- Twin-indexed source-set discovery through the existing asset manifest, with
  `SysmlPlugin` opening the checked set automatically after `TwinAssetMounted`;
- a Twin-owned verification registry mapping a qualified SysML verification
  name to one scene, one Rhai observer, and an optional verdict channel;
- three generic Rust query boundaries: `ValidateSysml` for source status,
  diagnostics, and the structural `lint.sysml` pass; `AnalyzeSysml` for
  policy-neutral selectable typed facts; and
  `ReadActiveTwinContract` for active-Twin component/verification bindings;
- Rhai policy layers that consume those same facts independently: `lint.sysml`
  for structural quality, `sysml_requirements.rhai` for requirement/source
  provenance and verification, and `sysml_modelica_constraints.rhai` for
  geometry-constraint selection and Modelica source assembly;
- `sysml_value(path, qualified_name)` and
  `sysml_value_from_report(report, qualified_name)` return tagged typed
  outcomes. Successful values remain native (`Vec3`, `Quat`, Transform,
  quantity, enumeration, or array); failed lookups return `ok: false` with an
  error and add a scene-scoped warning to `RuntimeDiagnostics`. A policy may
  request selected attributes through `AnalyzeSysml`; a successful read clears
  only the warning for the same source path;
- `sysml_model(path)` returns a read-only, revision-pinned `SysmlModel` backed
  by the cached Rust semantic snapshot. Use `model.value(qualified_name)` for
  repeated typed reads in one builder/verifier, and use
  `model.requirement(qualified_name)` or
  `model.verification(qualified_name)` for source-owned traceability. This
  keeps the parsed source set and its revision together and avoids rebuilding
  a dynamic report for every attribute. The Rhai `sysml_requirements::model()`
  helper opens the active Twin's `twin://` source; scripts must not read
  `.sysml` files directly or manufacture a second requirement-value table;
- the `luncosim test --verification QUALIFIED_NAME` selector; and
- structured per-check evidence emitted by `report_structured_verdict`.  The
  summary event keeps small reports inline; larger reports emit one bounded
  `*_EVIDENCE_RESULT` event per observation with a stable `result_index`.
  Consumers must group by channel and source revision, then order by that
  index.  This preserves the complete typed table without exceeding Rhai's
  bounded value budget.  Check records may use qualified or validated short
  requirement/verification names; short names are preferred in repeated
  arrays to avoid duplicating package prefixes.

The shared `sysml_requirements::evaluate` call resolves those short identities
once against the mounted source report and rewrites each check to the
canonical qualified name before observing USD or source-derived predicates.
Verification coverage is evaluated from resolved snapshot-scoped SysML
element handles, not by joining requirement and verification names into a
string key. The authored name selects the source element; the resolved handle
pair determines coverage.
Missing or colliding identities fail the evaluation; observers must not add a
package-prefix guess or a second registry. Use
`sysml_requirements::requirement_name(report, id)` and
`sysml_requirements::verification_name(report, id)` when a canonical identity
is needed before constructing additional evidence.

Unit-bearing vector components are preserved as arrays of native `Quantity`
values. A geometry policy must validate the declared quantity kind, fixed
cardinality, and each component's unit before lowering them to the shared
`Vec3`; never read only `number_value` and drop unit metadata. The shared
`lunco-engineering-values` seam performs dimension-safe conversion, while the
authored `engineering_units.rhai` catalog selects the supported UCUM-compatible
symbols. The current SysML-to-Modelica geometry adapter accepts a bounded
`LengthValue[3]` catalog and converts compatible entries to metres; it is not a
full UCUM parser and must fail closed for unsupported units.
Other physical-property quantities, including material properties, need the
same unit-preserving treatment. Do not reuse the geometry-only length adapter,
strip units, or assume an unimplemented material projection; capability-check
the property kind and consumer before generating a model.

For CAD/mechanical intent, keep the requirement and tolerance in SysML, then
call the reloadable `assets/scripting/tools/mechanical_relations.rhai` policy
with resolved native values. Its generic vocabulary includes distance,
coincidence, signed plane distance, under/clearance, parallelism,
perpendicularity, collinearity, coplanarity, mirroring, and plane/axis
symmetry. It returns residual/evidence records and knows neither Griffin names
nor USD paths. Do not add a product-specific relation predicate to Rust; add a
generic Rust numeric primitive only when the operation is shared, hot, and
not expressible safely with the Rhai standard math surface.

`source_with_attributes()` is a bounded value projection and intentionally does
not carry requirement/verification identity tables. Use
`source_with_selection()` when evidence must retain source-linked requirement
or verification identities. Identity and source revision remain Rust-owned.
For a supported source predicate, create a generic `constraint_check` and pass
it to `evaluate` or `evaluate_document`; Rhai selects explicit provider
observations and the neutral IR returns `pass`, `fail`, `inconclusive`, or
`error`. Do not collapse those states into a Twin-specific boolean assertion.

Keep normative requirement tolerances in SysML evidence. Runtime settings such
as `numerics.comparison.length_abs_m` and
`numerics.solver.residual_abs` are algorithm/solver policy, resolved once per
operation from the active Twin and passed through the call graph. They must not
silently replace a SysML acceptance tolerance or collapse different physical
dimensions into one epsilon.

It does not provide a full SysML/KerML execution engine. Parsed generic
elements, references, constraints, and relationships are source facts, not a
claim that their behavior is executed. The bounded constraint path now
compiles the resolved supported subset into `lunco-sysml-ir`; a Rhai policy
selects provider bindings; and `lunco-sysml-modelica` renders a typed
constraint into Modelica admitted through Rumoca. Native in-process policy can
retain a `SysmlModel` handle, while API/MCP callers use the structured
path-level functions `sysml_constraint_ir`, `sysml_modelica_constraint`, and
`sysml_evaluate_constraint` without serializing a native handle. Authored
source references use `lunco://`; a USD prim target remains a USD path.

The current IR supports typed scalar expressions, conditional expressions,
fixed primitive multiplicities, source-linked diagnostics, and deterministic
fingerprints. It does not yet support feature-chain navigation, reusable
constraint invocation/default arguments, collection/index/aggregate
expressions, full quantity conversion, redefinition/subsetting semantics,
null/invalid propagation across all providers, requirement membership
execution, applicability, or state/behavior execution. Do not recreate those
features with a hidden Rhai parser, Griffin-specific Rust, parallel arrays, or
qualified-name fallback tables. Add the generic AST/IR/provider mechanism and
then author the standard SysML construct.

`sysml_requirements::constraint_check` binds a check table to that supported
subset. Binding keys must match the source constraint parameter names; an
unknown key is an error and an omitted observation is unavailable. Evidence
retains the qualified constraint, source revision, fingerprint, provider
observations, expression results, diagnostics, and four-state verdict.
`evaluate_document(source, doc_id, checks)` gives the same generic checks an
explicit Editor document scope for USD queries. Generic observation kinds
include `not_exists` for absence requirements and typed source-vector component
selection through `expected_index`. The `tolerance` argument on a check is
numerical evaluator policy for equality comparisons; engineering acceptance
bounds must remain parameters of the SysML constraint itself.

The existing `CoincidentPointTranslation` policy can still run a bounded
asynchronous Modelica solve, read native finite `f64` results by exact
experiment identity, and produce a generation-bound typed USD placement plan.
The plan remains dry and requires Editor review/commit; no result is applied
automatically. This is a specific policy over the generic boundary, not
arbitrary SysML constraint execution. General constraint/parametric execution,
state and behavior execution, a dedicated SysML editor, full UI source-set
browsing, and automatic SysML-to-USD projection remain outside the current
runtime. If a request needs one of those, report the exact bounded gap after
checking the current owner and dependencies.

## Source organization

SysML files remain ordinary Twin files and must contain standard SysML syntax;
do not add LunCo-specific annotations to make runtime wiring work. The Twin
manifest owns source-set selection:

```toml
[sysml]
# Optional Twin-relative entry point; `.sysml` or `.kerml`.
root = "requirements/system.sysml"
# Additional Twin-relative roots. The indexed Twin files remain authoritative.
paths = ["requirements"]

[verification]
[[verification.cases]]
name = "Project::VerifyVisual"
scene = "tests/visual.usda"
script = "scenarios/tests/visual.rhai"
verdict_channel = "VISUAL_REQUIREMENTS"
```

Important source-set rules:

- `twin.toml` is configuration, not a replacement for SysML source text.
- The Twin's indexed files are the discovery authority; `[sysml].root` puts
  the entry point first and `[sysml].paths` narrows the indexed set.
- Paths in the verification registry are Twin-relative and are checked as
  indexed `.usda`/`.rhai` files. The registry selects execution; it does not
  define a requirement, a numeric limit, or a second backend.
- Use qualified names such as `Project::VerifyVisual` as stable keys for
  requirements and verification cases. Do not rely on a short name when
  packages can contain collisions.
- Use the existing `twin://<name>/...` and `lunco://...` identity schemes. Do
  not add a filesystem walker, a global source registry, or raw `std::fs` reads
  in a runtime crate.

## Validate before runtime

For an individual source, use the installed production executable. In the
commands below, set `LUNCOSIM_BIN` to the GitHub-installed `luncosim` command
or its absolute installed path. For a source checkout without an installed
command, build the production binary and set `LUNCOSIM_BIN` to that executable.

```bash
LUNCOSIM_BIN=luncosim
"$LUNCOSIM_BIN" --validate requirements/system.sysml
```

The same command accepts `.kerml` and can validate several assets in one call.
It parses and resolves the supplied source without constructing a window,
scene, physics world, or GPU. A successful pre-flight is not runtime proof.

For a Twin source set, `ValidateSysml` reports source validation status,
structural `lint.sysml` findings, diagnostics, indexed source files, and source
revision. Use `AnalyzeSysml` when a policy needs semantic facts; that query is
independent of structural-lint findings. Use `ReadActiveTwinContract` for the
active Twin's component/verification bindings. Rhai joins these inputs where
policy requires it. Against a running production session, the generic fact
query can be called from Rhai:

```rhai
let facts = query("AnalyzeSysml", #{
    path: "twin://my_twin",
    tables: ["requirements", "verifications"]
});
if facts.ok != true { throw(facts.errors); }
```

`AnalyzeSysml` currently discovers the Twin source set and obtains its analysis
synchronously. Use it for preflight, authoring, or explicit verification work;
do not call it from a high-rate `on_tick` hook. Revision-stamped async analysis
and admission are part of the cross-domain runtime contract in
[`62-deterministic-runtime-and-async-boundaries.md`](../../docs/architecture/62-deterministic-runtime-and-async-boundaries.md).

`ValidateSysml` and `AnalyzeSysml` accept either a filesystem path or a
`twin://` URI. `AnalyzeSysml` supports `elements`, `references`,
`relationships`, `constraints`, `attributes`, `requirements`, `verifications`,
and `diagnostics` tables. `attribute_names` narrows the attribute table to
qualified or local names; ambiguous local names remain visible to the policy
for explicit handling. Optional positive `limit` and non-negative `offset`
page every selected table independently. `analysis.page.tables` reports each
table's total, returned count, offset, limit, and `has_more`; every page also
carries the source revision. Check that revision before assembling pages.
Unpaged full analysis can exceed Rhai's bounded-value budget on Twin-scale
sources, so prefer selected tables, selected names, or bounded pages.

For editable Rhai runtime policy, `sysml_analysis(path)` provides a native
snapshot for small bounded sources. For Twin-scale sources use selected
`AnalyzeSysml` pages or `sysml_requirements::source()`, which pages the
requirement/verification tables, checks revision consistency, and retains only
the compact identities and verification links needed for policy joins.
Detailed selected requirement records remain available through
`source_with_selection()`. Use
`sysml_attribute(path, qualified_name)` when the policy needs the native
`SysmlAttribute` object for one exact-name source read, and `sysml_value` only
when it needs the typed literal rather than the attribute metadata. Keep
acceptance checks (dimensions, ranges, counts, material eligibility, and
geometry rules) in the authored policy so ordinary
design changes do not require a Rust rebuild. Rust should expose the source
semantics and reusable native conversion, not Griffin-specific pass/fail
decisions. The generic `AnalyzeSysml` selectors remain useful for clients that
need to limit language-neutral API payloads.

For the active Twin, the requirements policy composes source facts and the Twin
contract through:

```rhai
let source = sysml_requirements::source();
```

When a policy needs several literals, select them in one bounded request and
reuse the returned report:

```rhai
let source = sysml_requirements::source_with_attributes([
    "Project::Vehicle::massKg", "Project::Vehicle::wheelRadiusM"
]);
if source.ok != true { throw(source.error); }
let mass = sysml_requirements::number(source, "Project::Vehicle::massKg");
let radius = sysml_requirements::number(source, "Project::Vehicle::wheelRadiusM");
```

This requests the source-backed attributes once and reuses that typed Rhai
report. Keep it local to the evaluation or task-construction boundary; do not
turn it into a mutable global cache. Qualified selectors are preferred, and
ambiguous short selectors remain an explicit error.

`sysml_requirements::source()` is read-only. It fails visibly when there is no
active Twin, no indexed SysML source, a parser diagnostic, a registry error, or
a verification name that does not resolve in the source set, or when the
source revision changes during page assembly. It joins the
`AnalyzeSysml` result with `ReadActiveTwinContract` in Rhai; neither generic
query implements that policy. Use native typed Rhai maps inside the workflow;
JSON is reserved for explicit external transport/logging boundaries. Never
reconstruct a short-name map that could silently collapse colliding component
attributes.

For a component-owned observer, resolve the manifest binding through the
generic helper instead of repeating scene/script or qualified-verification
selection logic:

```rhai
let binding = sysml_requirements::component_binding(source, "vehicle.wheel");
if binding.ok != true { throw(binding.error); }
let component = binding.component;
let verification = binding.verification;
```

The helper fails closed on unavailable reports, registry errors, unknown or
duplicate components, and missing or duplicate verification mappings. It does
not replace the component's SysML checks; it only supplies the exact
Twin-owned identity selected by the manifest.

Repeated source queries reuse one immutable semantic snapshot only when the
caller revision, ordered source-set fingerprint, embedded standard-library
contents, and cache format all match. Do not treat a document generation or a
short revision number as a complete source identity: separate documents or
Twins can reuse those counters. The cache uses the shared `lunco-hash` fast
tier and does not add a second source registry or durable content store.

## Write the Rhai verification observer

The observer is the executable policy. It reads the canonical source snapshot,
names the SysML requirement and verification, observes the composed USD stage,
and emits the verdict:

```rhai
let source = sysml_requirements::source();
let result = sysml_requirements::evaluate(source, [
    #{ id: "VIS-001", component: "camera",
       requirement: "Project::CameraExists",
       verification: "Project::VerifyVisual",
       kind: "exists", path: "/Twin/VisualCamera",
       expected_type: "Camera", visible: true },
    #{ id: "VIS-002", component: "vehicle.wheel.front_left",
       requirement: "Project::WheelRadius",
       verification: "Project::VerifyVisual",
       kind: "attribute", path: "/Twin/Vehicle/WheelFrontLeft",
       attr: "radius", expected_attr: "visualWheelRadiusM",
       tolerance: 0.001 }
]);
report_structured_verdict(result, "VISUAL REQUIREMENTS", "VISUAL_REQUIREMENTS");
```

Every check requires `id`, `requirement`, `verification`, and `kind`. The
evaluator first proves that the requirement exists and that the selected
verification covers it. It then observes USD through the existing query path;
it does not open USD layers, mutate a stage, or reimplement the resolver.

Available generic check kinds are:

| Kind | Observation | Important fields |
|---|---|---|
| `coverage` | requirement/verification traceability only | no USD path |
| `assert` | source-derived predicate, including cross-component interface checks | `ok`, optional `actual`, `expected`, `error`; no USD path |
| `exists` | prim exists, optionally has type and visibility | `path`, `expected_type`, `visible` |
| `children` | required child prims exist and are visible | `paths`, `visible` |
| `attribute` | scalar USD attribute is near a SysML literal | `path`, `attr`, `expected_attr`, `tolerance` |
| `attribute_component` | one vector component is near a SysML literal | `path`, `attr`, `index`, `expected_attr`, `tolerance` |
| `extent_component` | derived geometry extent component is near a SysML literal | `path`, `index`, `expected_attr`, `tolerance` |
| `bounds_component` | composed USD geometry bound component is near a SysML literal | `path`, `index`, `expected_attr`, `tolerance` |
| `attribute_equals` | direct literal equality for an observed attribute | `path`, `attr`, `expected` |
| `relationship` | authored relationship has the required target | `path`, `relationship`, `target` |

For numeric limits, use `expected_attr` to read the literal from the
authoritative SysML source. Prefer a qualified attribute name; a short name is
accepted only when the selected source facts make it unique. Ambiguous names
are explicit failures, never a first-match choice.
`attribute_equals` is for direct equality such as strings or booleans and uses
its explicit `expected` value. Do not copy a threshold into Rhai, TOML, a UI
label, or a Rust constant. Do not infer a requirement from a screenshot or
from an ambiguous short-name lookup.

The result contains `ok`, `results`, `failures`, `check_count`,
`requirement_count`, `requirement_names`, `requirement_summary`,
`failure_count`, `verification`, `source_revision`, and `source_files`.
`check_count` is the number of concrete observations (for example, one
transform or attribute on one repeated part); `requirement_count` is the
number of unique SysML requirement usages represented by those observations.
Use `requirement_summary` for a compact per-requirement `{ checks, failures }`
view and keep the complete result table as evidence. A missing observation is a
failure, not a passing empty set. The next performance seam is a native batch
USD query; do not implement an ad-hoc Rhai cache that outlives one evaluation.

## Select and run a verification case

Use the same resolved production executable and a qualified name:

```bash
"$LUNCOSIM_BIN" test \
  --scene tests/visual.usda \
  --verification Project::VerifyVisual \
  --max-ticks 120
```

The selector is intentionally strict. Before constructing the simulation it
checks that:

1. the scene is enclosed by the Twin manifest;
2. the registry has exactly the requested qualified case;
3. the registry has no duplicate/unsafe/empty entries;
4. the mapped scene matches the requested scene; and
5. the declared verdict channel is used unless the CLI explicitly overrides it.

The selector does not execute SysML constraints for you. The mapped Rhai
observer still has to call `sysml_requirements::evaluate` and emit the verdict.
Make positive conformance evidence the default: run the authored requirement
observer through the production scene-test binary and prove the required
component/value/relationship. Add a failing or unavailable-source case only
when rejection or fail-closed behavior is itself an explicit contract (for
example, stale-source rejection or a safety-critical missing relationship).
`--validate` alone cannot prove any of these runtime facts.

## Development cycle

For a change to a Twin's requirements or verification:

1. Read the owning source and this skill, then search the current checkout for
   existing SysML vocabulary, evaluator checks, manifest fields, commands and
   tests. Do not add a second parser, registry, or report format.
2. Confirm the binary feature set. SysML is enabled by default in the
   production app and server:
   `cargo build -p lunco-luncosim --bin luncosim -j 4`.
   Use `--no-default-features` only when a deliberately lean build is needed.
   Rhai is the default scenario backend. Python is a separate opt-in
   `python` feature and is not used by this workflow.
3. Author standard SysML source and put source-set/execution selection in
   `twin.toml`; keep USD facts in USD and runtime observation in Rhai.
4. Run the parse-only source gate, then `ValidateSysml` for the mounted Twin.
5. Add or update the generic Rhai observer and run the production scene test
   with the qualified `--verification` selector.
6. Inspect structured evidence: requirement/verification identities, exact
   USD paths, actual/expected values, source files and `source_revision`.
7. Review the full diff for duplicated thresholds, stale qualified names,
   unindexed files, compatibility fallbacks and claims beyond the supported
   subset. Run `git diff --check` and the smallest relevant validation.

Keep behavior and policy tests in authored Rhai under
`assets/scenarios/tests/`. Reserve Rust tests for parser, resolution,
serialization and generic bridge mechanisms that the public Rhai/API surface
cannot observe. If the change is only `.sysml`, `.kerml`, `twin.toml`, Rhai, or
this skill, do not rebuild Rust unless the validation path itself changed.

## Common bounded diagnoses

| Observation | Correct diagnosis/check |
|---|---|
| `ValidateSysml` is unknown | Check that the production binary/session is current and that the validation plugin is registered; do not conclude SysML is absent from one source file. |
| `sysml_requirements::source()` is unavailable | Check the active Twin, the Rhai tool library, and the `sysml` feature; report the exact unwired boundary if one remains. |
| No requirements are returned | Check the Twin index, `[sysml].paths`, source extension and parser diagnostics; do not create a duplicate requirements file in Rhai. |
| Verification registry fails | Inspect the active Twin contract and exact Twin-relative scene/script paths; fix the manifest or indexed files. `ValidateSysml` reports source validation, not manifest policy. |
| A short attribute name collides | Use a qualified attribute identity or resolve the ambiguous source fact explicitly. Do not let a new lookup rule choose silently. |
| The selected case passes without evidence | Confirm the observer emitted structured evidence and that the result was not an empty/unavailable report; `--verification` only selects and validates the mapping. |
| A request needs arbitrary KerML constraints or a full editor | State that this is outside the implemented subset after citing the domain review; do not emulate it with a hidden Rhai parser. |
| A request mentions Python | Treat Python as optional integration only. Use `--features python` and the Python-specific contract if explicitly requested; keep normal examples in Rhai/Modelica. |

When reporting a gap, use a bounded result: found and usable, implemented but
unwired, present on another branch/version, not found in the searched scope,
or externally blocked. Never generalize from an empty `skills/` search or from
an executable built without the optional `sysml` feature.
