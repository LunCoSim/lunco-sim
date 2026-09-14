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
| System structure and requirement intent | standard SysML v2 `.sysml`/`.kerml` | `sysml` feature, opt-in | parts, ports, connections, requirements, satisfy/verify links, scalar literals |
| Scene identity, topology, geometry and authored physical facts | USD | standard runtime path | prim paths, schemas, relationships, dimensions, materials, physics topology |
| Continuous equations and domain state | Modelica | standard domain backend | propulsion, electrical, thermal and other continuous models |
| Scenario policy, observations, checks and verdicts | Rhai | default scenario backend | runtime observation, actuation, generic requirement evaluation and test policy |
| Python integration | Python backend | `python` feature, opt-in | explicit one-shot or integration requests only; not the normal scenario workflow |
| Generic parsing, resolution, transport and lifecycle mechanisms | Rust crates | feature-dependent | reusable substrate, not Twin-specific policy |

The source-of-truth rule is strict: SysML states what the system must be and
why; USD states what is authored and observable; Modelica states equations;
Rhai executes the observation and policy. A verification registry selects a
scene and script but does not duplicate requirement text or thresholds.

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

## What is implemented, and what is not

The current supported subset is source-backed and deterministic:

- standard textual SysML v2 syntax in `.sysml` and `.kerml` files;
- packages/imports, part definitions/usages, ports, connections, requirement
  definitions/usages with documentation and attributes;
- standard external references plus `satisfy` and verification `verify`
  memberships;
- qualified names, typed scalar literal projections, source spans, diagnostics,
  source files, and a deterministic `source_revision`;
- Twin-indexed source-set discovery through the existing asset manifest;
- a Twin-owned verification registry mapping a qualified SysML verification
  name to one scene, one Rhai observer, and an optional verdict channel;
- native Rhai maps from `sysml_report()` and
  `sysml_requirement_report()`; JSON forms are compatibility output for logs
  and external clients;
- the read-only `ValidateSysml` query and the
  `luncosim test --verification QUALIFIED_NAME` selector; and
- structured per-check evidence emitted by `report_structured_verdict`.

It does not provide a full SysML/KerML execution engine. Do not promise or
silently emulate interface definitions, arbitrary expressions and constraints,
parametrics, state machines, behaviors, allocations/refinements, a full SysML
editor, automatic UI source-set discovery, or a SysML-to-USD projection. If a
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

The result contains `ok`, `results`, `failures`, `requirement_count`,
`failure_count`, `verification`, `source_revision`, and `source_files`. Keep
the result and emitted evidence with the run. A missing observation is a
failure, not a passing empty set.

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
2. Confirm the binary feature set. SysML runtime integration is opt-in:
   `cargo build -p lunco-luncosim --bin luncosim --features sysml -j 4`.
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
