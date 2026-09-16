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
| Python integration | Python backend | `python` feature, opt-in | explicit one-shot or integration requests only; not the normal scenario workflow |
| Generic parsing, resolution, transport and lifecycle mechanisms | Rust crates | feature-dependent | reusable substrate, not Twin-specific policy |

The source-of-truth rule is strict: SysML states what the system must be and
why; USD states what is authored and observable; Modelica states equations;
Rhai executes the observation and policy. A verification registry selects a
scene and script but does not duplicate requirement text or thresholds.

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
snapshot and evaluate composed USD evidence. Keep both in Rhai; Rust supplies
typed facts and lifecycle, not mission- or component-specific assertions.

Relevant implementation and design references:

- [`24-domain-sysml.md`](../../docs/architecture/24-domain-sysml.md) — domain
  boundary and supported subset.
- [`sysml-requirement-verification.md`](../../docs/architecture/sysml-requirement-verification.md)
  — current Rhai bridge and verification contract.
- [`sysmlv2-embedding-and-asset-resolution.md`](../../docs/reviews/sysmlv2-embedding-and-asset-resolution.md)
  — source-set, resolution, registry and evidence review.
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
- **Failure behavior:** For safety, reliability, or resilience requirements,
  specify the trigger, detection deadline, required response, and recovery or
  safe state. “The system shall handle errors” is incomplete.

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
- qualified names, typed scalar literal projections (`value.number_value` is a
  native finite Rhai number while `value.number` preserves authored text), source spans, diagnostics,
  source files, and a deterministic `source_revision`;
- Twin-indexed source-set discovery through the existing asset manifest, with
  `SysmlPlugin` opening the checked set automatically after `TwinAssetMounted`;
- a Twin-owned verification registry mapping a qualified SysML verification
  name to one scene, one Rhai observer, and an optional verdict channel;
- native Rhai maps from `sysml_report()` and
  `sysml_requirement_report()`; JSON forms are compatibility output for logs
  and external clients;
- the read-only `ValidateSysml` query and the
  `luncosim test --verification QUALIFIED_NAME` selector; and
- structured per-check evidence emitted by `report_structured_verdict`.  The
  summary event keeps small reports inline; larger reports emit one bounded
  `*_EVIDENCE_RESULT` event per observation with a stable `result_index`.
  Consumers must group by channel and source revision, then order by that
  index.  This preserves the complete typed table without exceeding Rhai's
  bounded value budget.  Check records may use qualified or validated short
  requirement/verification names; short names are preferred in repeated
  arrays to avoid duplicating package prefixes.

It does not provide a full SysML/KerML execution engine. Do not promise or
silently emulate interface definitions, arbitrary expressions and constraints,
parametrics, state machines, behaviors, allocations/refinements, a full SysML
editor, full UI source-set browsing, or a SysML-to-USD projection. If a
request needs one of those, report the exact bounded gap after checking the
current owner and dependencies.

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

For the complete Twin source set, use `ValidateSysml` so the manifest roots,
indexed files, verification registry and source revision are checked together.
Against a running production session:

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H 'content-type: application/json' \
  -d '{"type":"ExecuteCommand","command":"ValidateSysml","params":{"path":"twin://my_twin","compact":true}}'
```

`ValidateSysml` accepts either a filesystem path or a `twin://` URI. For an
active Twin, the generic tool uses:

```rhai
let source = sysml_requirements::source();
```

That call is read-only. It fails visibly when there is no active Twin, no
indexed SysML source, a parser diagnostic, a registry error, or a registry
verification name that does not exist in the source set. Do not add a fallback
that lets a test run against copied requirements.

The compact report includes `requirements`, `attributes` (currently keyed by
the source attribute's short name),
`requirement_records`, `verification_cases`, `verification_records`,
`verification_registry`, `verification_registry_errors`, `source_files`,
`source_revision`, and `source_revision_hex`. Use native report maps in Rhai;
use JSON only at an external/logging boundary. The full non-compact report
also exposes `attributes_qualified` and `attribute_collisions` for tooling.

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
    #{ id: "VIS-002", component: "wheel_FL",
       requirement: "Project::WheelRadius",
       verification: "Project::VerifyVisual",
       kind: "attribute", path: "/Twin/FLIP/Wheel_FL",
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
| `exists` | prim exists, optionally has type and visibility | `path`, `expected_type`, `visible` |
| `children` | required child prims exist and are visible | `paths`, `visible` |
| `attribute` | scalar USD attribute is near a SysML literal | `path`, `attr`, `expected_attr`, `tolerance` |
| `attribute_component` | one vector component is near a SysML literal | `path`, `attr`, `index`, `expected_attr`, `tolerance` |
| `extent_component` | derived geometry extent component is near a SysML literal | `path`, `index`, `expected_attr`, `tolerance` |
| `bounds_component` | composed USD geometry bound component is near a SysML literal | `path`, `index`, `expected_attr`, `tolerance` |
| `attribute_equals` | direct literal equality for an observed attribute | `path`, `attr`, `expected` |
| `relationship` | authored relationship has the required target | `path`, `relationship`, `target` |

For numeric limits, use `expected_attr` to read the literal from the
authoritative SysML report. The current compact evaluator resolves that field
by the source attribute's short name; if the full report shows a collision,
use a unique authored attribute name or extend the authoritative bridge before
using the check. Do not silently choose between colliding attributes.
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
Run one positive and one deliberately failing observation through the
production scene-test binary. Add an unavailable/stale-source case when the
contract needs to prove fail-closed behavior. `--validate` alone cannot prove
any of these runtime facts.

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
| Verification registry fails | Inspect `Twin::verification_registry_errors()` and the exact Twin-relative scene/script paths; fix the manifest or indexed files. |
| A short attribute name collides | Inspect `attribute_collisions`; use a unique authored attribute name or extend the authoritative bridge before evaluating it. Do not let a new lookup rule choose silently. |
| The selected case passes without evidence | Confirm the observer emitted structured evidence and that the result was not an empty/unavailable report; `--verification` only selects and validates the mapping. |
| A request needs arbitrary KerML constraints or a full editor | State that this is outside the implemented subset after citing the domain review; do not emulate it with a hidden Rhai parser. |
| A request mentions Python | Treat Python as optional integration only. Use `--features python` and the Python-specific contract if explicitly requested; keep normal examples in Rhai/Modelica. |

When reporting a gap, use a bounded result: found and usable, implemented but
unwired, present on another branch/version, not found in the searched scope,
or externally blocked. Never generalize from an empty `skills/` search or from
an executable built without the optional `sysml` feature.
