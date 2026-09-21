# SysML v2 requirements and Rhai verification

LunCoSim treats SysML v2 as the normative requirement source, USD as the
identity/topology source, Modelica as the continuous-equation source, and Rhai
as the executable test policy. A requirement is not accepted merely because a
source parser found its declaration: an authored verification case must cover
the requirement, and a Twin test must observe the composed USD/Modelica stage.

The generic `sysml_requirements` Rhai tool provides the small bridge. It reads
the active Twin's indexed SysML source set through `sysml_requirements::source()`;
it does not embed a second copy of requirements in the test:

The bridge is split by responsibility. `ValidateSysml` performs source
validation, runs the structural `lint.sysml` policy, and returns diagnostics
plus source identity. It does not evaluate Twin-manifest binding policy or
runtime requirement observations. `AnalyzeSysml` exposes
the parser's typed, source-backed fact tables, with optional table, name,
and bounded-page selection. `ReadActiveTwinContract` exposes the active Twin's
component and verification bindings. These are generic Rust mechanisms; Rhai
policies decide what the facts mean and join Twin bindings to SysML identities.
The current policy layers are deliberately independent:

- `lint.sysml` checks structural quality of requirements and verification
  relationships;
- `sysml_requirements.rhai` applies requirement, source-provenance, and
  verification policy; and
- `sysml_modelica_constraints.rhai` selects geometry constraints and assembles
  Modelica source from typed SysML values.

Each policy requests only the fact tables it needs. A different Twin can add a
separate Rhai policy without adding project rules to Rust or changing the
generic SysML projection. The geometry tool currently demonstrates selection
and source assembly. Its supported `CoincidentPointTranslation` path can also
replace an explicit scratch Modelica document, dispatch one bounded async
solve, poll the deferred acknowledgement and run by their exact identities,
and validate native finite `f64` readback against the SysML-bound relation.
The result is a source-revision-bound, generation-checked typed USD placement
plan and proposal; it does not auto-commit a Griffin edit or claim that a
visual/runtime acceptance gate has passed. Arbitrary SysML constraint
execution remains out of scope.

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

For bounded generic inspection, `sysml_report(path)` exposes `AnalyzeSysml`
facts as native Rhai values (no stringify/parse round trip). Twin-scale callers
should request selected pages through the generic query:

```rhai
let first_page = query("AnalyzeSysml", #{
    path: "twin://my-twin", tables: ["requirements", "verifications"],
    offset: 0, limit: 16
});
let page = first_page.analysis.page.tables.requirements;
// Continue at offset 16 while any selected table reports has_more == true.
```

Every selected table reports `offset`, `limit`, `total`, `returned`, and
`has_more` under `analysis.page.tables`. The top-level page also carries the
source revision. A Rhai assembler must check that revision on each page and
fail if it changes; do not concatenate pages from different source snapshots.

These functions are policy-neutral fact queries. The Twin-aware report shape,
source/verification joins, and any selected requirement set are authored in
`sysml_requirements.rhai`; changing that policy does not require rebuilding
Rust. `sysml_requirements::source()` pages Twin-wide requirement and
verification facts, reduces them to qualified identities and verification
links, and keeps detailed records available through `source_with_selection()`.
Rust changes are reserved for new generic SysML semantics, typed value
lowering, or transport-selection capabilities. Numeric literals retain their
authored source identity and provide a validated native finite `number_value`;
consumers use that value instead of reparsing literal text in Rhai.

For one native typed literal, `sysml_value(path, qualified_name)` and
`sysml_value_from_report(report, qualified_name)` return a tagged map:
`{ok: true, found: true, value: <native value>}` on success and
`{ok: false, found: false, error: <message>}` on failure. A source report
that intentionally omitted an attribute returns `ok: true,
found: false`; `sysml_requirements::native_value` then performs the selected
typed query. Missing attributes and failed source resolution remain explicit
errors. The bridge records a scene-scoped `RuntimeDiagnostics` warning, while
the script receives the structured result and can produce a failed check
without terminating the application. A successful read clears only that
source path's query warning; warnings for other sources remain visible.

External clients receive serialized reports from the API boundary; Rhai keeps
the report native and typed. A Twin declares the execution binding separately
in `twin.toml`:

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
are explicit failures. Its Rhai source policy joins `AnalyzeSysml` facts with
`ReadActiveTwinContract`; neither query makes a project-specific binding
decision.

`luncosim test --scene tests/visual.usda --verification Project::VerifyVisual`
checks this registry mapping (qualified SysML name, Twin-relative scene and
Rhai observer, and verdict channel) before constructing the simulation. The
registry is metadata, not another requirement source; thresholds and units
remain in SysML literals.

Supported observations are `assert`, `exists`, `children`, `attribute`,
`attribute_component`, `extent_component`, `bounds_component`,
`attribute_equals`, `relationship`, and `coverage`. `expected_attr` reads a
literal SysML attribute by its qualified source name, so numeric limits are not
copied into a Rhai script. `AnalyzeSysml` can select only required fact tables,
page a selected table, and, when attributes are requested, only named
attributes. The requirements
policy preserves qualified identity and reports ambiguous short names instead
of silently selecting one component's literal. Every check carries a component
and requirement ID, producing a per-component evidence record with the source
revision and exact USD path.

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
