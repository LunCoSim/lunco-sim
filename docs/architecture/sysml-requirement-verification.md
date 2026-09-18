# SysML v2 requirements and Rhai verification

LunCoSim treats SysML v2 as the normative requirement source, USD as the
identity/topology source, Modelica as the continuous-equation source, and Rhai
as the executable test policy. A requirement is not accepted merely because a
source parser found its declaration: an authored verification case must cover
the requirement, and a Twin test must observe the composed USD/Modelica stage.

The generic `sysml_requirements` Rhai tool provides the small bridge. It reads
the active Twin's indexed SysML source set through `sysml_requirements::source()`;
it does not embed a second copy of requirements in the test:

```rhai
let source = sysml_requirements::source();
let result = sysml_requirements::evaluate(source, [
    #{ id: "GV-001", component: "camera",
       requirement: "Project::gv001", verification: "Project::VerifyVisual",
       kind: "exists", path: "/Twin/VisualCamera",
       expected_type: "Camera", visible: true },
    #{ id: "GV-002", component: "wheel_FL",
       requirement: "Project::gv004", verification: "Project::VerifyVisual",
       kind: "attribute", path: "/Twin/Vehicle/Wheel_FL", attr: "radius",
       expected_attr: "Project::Rover::visualWheelRadiusM", tolerance: 0.001 }
]);
report_structured_verdict(result, "VISUAL REQUIREMENTS", "VISUAL_REQUIREMENTS");
```

`report_structured_verdict` emits the compatibility `TESTS_OK`/`TESTS_FAIL`
envelope and a `<CHANNEL>_EVIDENCE` map with `schema_version: 1`. The evidence
contains the verification key, requirement/check results and failures, the
source revision in both backward-compatible and explicit `source_revision_hex`
fields, the ordered `source_files` list, and optional observer `metrics`.
Component observers should put clock/root/package facts in `metrics`; they
must not copy requirement thresholds there.

For inspection and tooling, the same snapshot is available as native Rhai
maps (no stringify/parse round trip):

```rhai
let report = sysml_requirement_report();
let all_declarations = sysml_report();
```

Numeric literals in those maps carry both representations: `value.number` is
the lossless authored text, while `value.number_value` is the validated native
finite number used by requirement policy. Consumers must use `number_value`;
reparsing the text in Rhai is not part of the bridge contract.

The `sysml_report_json()` and `sysml_requirement_report_json()` functions are
compatibility paths for logs and external clients. A Twin declares the
execution binding separately in `twin.toml`:

```toml
[verification]
[[verification.cases]]
name = "Project::VerifyVisual"
scene = "tests/visual.usda"
script = "scenarios/tests/visual.rhai"
verdict_channel = "VISUAL_REQUIREMENTS"

[[components]]
name = "rover.wheels"
requirements = "requirements/rover_wheels.sysml"
verification = "RoverWheelRequirements::Verify"
usd_path = "/World/Rover/Wheels"
```

`[[components]]` is the optional ownership layer for a componentized Twin.
It requires one indexed SysML/KerML requirement document per component and
binds that document to one unique verification case. The case above owns the
fixture and Rhai observer, so a component cannot pass by accidentally using a
sibling's test or by sharing a stale source file. The registry is generic and
does not add domain names or duplicate thresholds.

The generic `sysml_requirements::component_binding(report, name)` helper
exposes this same manifest selection to Rhai observers. It returns the exact
component and verification records only when both registries are valid and the
name is unique; unavailable, unknown, duplicate, or unregistered selections
are explicit failures. Rust's `Twin::component_verification` is the shared
low-level selector used by CLI/runtime callers, so no caller needs a second
lookup path.

`luncosim test --scene tests/visual.usda --verification Project::VerifyVisual`
checks this registry mapping (qualified SysML name, Twin-relative scene and
Rhai observer, and verdict channel) before constructing the simulation. The
registry is metadata, not another requirement source; thresholds and units
remain in SysML literals.

Supported observations are `assert`, `exists`, `children`, `attribute`,
`attribute_component`, `extent_component`, `bounds_component`,
`attribute_equals`, `relationship`, and `coverage`. `expected_attr` reads a
literal SysML attribute by its qualified source name, so numeric limits are not
copied into a Rhai script. The compact bridge exposes one qualified attribute
map and intentionally omits a duplicate short-name map; every collision is
therefore explicit rather than silently selecting one component's literal. For
`attributes: []`, it also omits the collision table; for a selected short name
it returns only that name's collision record. Thus the lazy source request
stays bounded while an ambiguous short selector remains an explicit failure
rather than silently selecting one component's literal.
Every check carries a component and requirement ID, producing a per-component
evidence record with the source revision and exact USD path.

Check tables may use either a qualified requirement/verification name or a
short local identity. Before the first observation, `evaluate` resolves both
through the mounted source report and rewrites the records to their canonical
qualified names. A missing or colliding short identity is a failed check, not
a guessed package prefix. This lets a Twin keep repeated arrays compact while
the evidence and coverage index retain one unambiguous SysML identity. The
helpers are available to authored code as
`sysml_requirements::requirement_name(report, id)` and
`sysml_requirements::verification_name(report, id)` when a test needs the
canonical name before constructing a check.

An `assert` check is the pathless counterpart for a source-derived predicate.
The Twin Rhai observer computes the predicate from typed SysML values (for
example, deriving a ramp width from another component's wheel stations), then
passes `{ kind: "assert", ok: ..., actual: ..., expected: ..., error: ... }` to
the same evaluator. Coverage and structured failure handling remain identical;
the evaluator never invents a value or turns a missing predicate into a pass.

This is deliberately a subset of SysML v2 verification semantics: requirement
definitions/usages, subjects, attributes, and verification-case `verify`
memberships. It does not pretend to be a full KerML execution engine. The
subset is sufficient for deterministic system-level acceptance while remaining
portable to headless tests and interactive Twin review.
