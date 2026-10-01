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
  relationships, including source-evidence coverage, formal acceptance
  constraints, and identifier-like values encoded as strings;
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

The generic constraint compilation boundary is now explicit. A resolved
supported constraint can be compiled by `lunco-sysml-ir`, carrying source
spans, typed parameters, multiplicity, dependencies, diagnostics, and a
deterministic fingerprint. `lunco-sysml-modelica` lowers valid IR to
standalone Modelica and admits it through Rumoca. In-process Rhai may use a
native `SysmlModel`; API/MCP callers should use the structured path functions
`sysml_constraint_ir`, `sysml_modelica_constraint`, and
`sysml_evaluate_constraint`. These are provider-neutral seams, not a Griffin
relation library and not a claim that Rumoca executes KerML.

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

`report_structured_verdict` emits a `<CHANNEL>_EVIDENCE` map with
`schema_version: 2` and the standard `pass`, `fail`, `inconclusive`, or `error`
verdict. Boolean-only checks map true/false to pass/fail; constraint checks
retain the neutral evaluator's four states. Per-requirement summaries count
passes, failures, inconclusive checks, and errors independently. The evidence
contains the verification key, typed check results, numeric source revision,
ordered `source_files`, and optional observer `metrics`. The telemetry verdict
uses the same four states; generic scene tests may continue to emit PASS/FAIL.
Component observers should put clock/root/package facts in `metrics`; they
must not copy requirement thresholds there.

The requirements presentation model rebuilds after changes to its Twin,
analysis, source-document registry, or evidence inputs. Idle document-event
publishing must preserve registry change detection, so opening USD previews
does not repeatedly reconstruct the SysML model on subsequent frames.

The windowed **SysML Requirements** panel consumes inline check tables and
bounded result events. It associates each retained check with its requirement,
channel, verification, source revision, and simulation tick, then presents its
ID, kind, component/path, status, explanation, and actual/expected values when
present. It retains up to 64 display details per requirement, prioritizing
non-pass checks; aggregate requirement counts remain the status source for
larger reports. The production scene-test runner returns report schema 2 with
the standard verdict, a separate runner status, and the source revision observed
at run start and completion. The UI rejects mismatched source revisions and
parses this process-boundary payload into the same evidence model used for live
Twin telemetry; it does not infer verdicts or check details from output text.

The Requirements view separates the inventory of requirement usages,
definitions, and source files from its status breakdown. Actionable counters
filter to missing formal `require` criteria, missing `verify` links, unmapped
Twin tests, or mapped tests without a current result. The virtualized table can
be sorted by identity, kind, source, evidence, Twin test, model coverage, or
overall status; the evidence, execution, and coverage columns remain distinct.
The table uses the full center pane; a docked Requirement details panel follows
the selected row in the lower-right Editor pane. It summarizes the criterion,
declared subjects, explicit `satisfy` links, resolved verification cases, Twin
mappings, and evidence, with direct source navigation and a shortcut to the
full Traceability view. When a `verify` link has no Twin test mapping, the
detail can open the active Twin's `twin.toml` in the source viewer. Test
execution remains available only for cases present in the Twin verification
registry.

**Run selected requirement tests** executes the unique mapped cases linked from
the selected requirement. **Run all mapped tests** executes each unique
Twin-mapped case linked from any requirement once, sequentially, and reports
progress. Completed suite results retain their source revision and outcome
counts; **Show failures** filters to failed requirements, and **Rerun failed**
starts only failed cases. Each failed structured check can open its associated
requirement declaration. The structured report includes runner-level reasons
for readiness failures, runtime faults, and exhausted limits. Bounded captured
process output remains available for diagnostics. Unmapped links and
requirements without `verify` links remain coverage gaps and are not executed.

The **Traceability** view maps one selected requirement through declared
subject types, explicit standard `satisfy` relationships, resolved `verify`
cases, Twin test registration, run status, and structured evidence. Subject
types are displayed separately from satisfy relationships; missing links stay
visible and are never inferred. Source-backed requirement, model, and
verification nodes open at their analyzed lines, and mapped tests can run from
the map. The **Structure** view provides a filterable, navigable hierarchy of
packages, parts, items, interfaces, ports, and connections from the same
analysis revision. It supports source navigation without claiming to provide
full BDD/IBD diagram authoring.

The panel's overall requirement roll-up is `VERIFIED` only when the analyzed
source has no parser/resolver diagnostics, a formal `require` criterion exists,
every `verify` link maps to a Twin test, current requirement evidence passes,
and all mapped tests pass. `FAILED` is reserved for an observed standard fail;
`INCONCLUSIVE` means evidence is insufficient, `ERROR` means evaluation failed,
and `RUN ERROR` means the runner could not establish a trustworthy result.
`INVALID MODEL`, `STALE`, `RUNNING`, and `INCOMPLETE` keep source diagnostics,
old evidence, active work, and missing proof visible. This is a summary of
authored criteria, emitted evidence, and linked tests; it is not full KerML
constraint execution.

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

`ValidateSysml` applies the reloadable `lint.sysml` policy to the same resolved
source snapshot. In addition to documentation, subject, and `verify`-link
checks, it reports requirement usages without a typed source-catalog link to a
populated source locator, usages without an inherited or owned `require`
acceptance predicate (`assume` memberships provide context but do not count as
pass criteria), and identifier-like `String` values that appear to encode
enumerators or member identities. Findings identify the qualified element and
source file/byte offset. These are authoring warnings: they surface migration
work without preventing an incomplete source set from opening. A bounded
choice belongs in an enum; a named system member belongs in typed part usages
or references. Neither should be stored as a delimited `String` list.
The API returns the same findings as structured records with `domain`, `rule`,
`severity`, `subject`, and `message`; `warnings` remains the concise display
form for existing callers.

The source-evidence check follows typed references in the analyzed model: a
requirement-evidence usage references its requirement and one or more source
records, and each source record supplies a non-empty typed locator. The
`verify` relationship establishes SysML verification coverage. The Twin
verification registry and `luncosim test --verification` separately establish
that a concrete observer is bound to a fixture; source lint does not claim
that a verification case has been executed.

These functions are policy-neutral fact queries. The Twin-aware report shape,
source/verification joins, and any selected requirement set are authored in
`sysml_requirements.rhai`; changing that policy does not require rebuilding
Rust. `sysml_requirements::source()` pages Twin-wide requirement and
verification facts, reduces them to qualified identities and verification
links, and keeps detailed records available through `source_with_selection()`.
Rust changes are reserved for new generic SysML semantics, typed value
lowering, or transport-selection capabilities. Numeric literals retain their
authored source identity and provide a validated native finite `number_value`;
consumers use that value instead of reparsing literal text in Rhai. The same
projection now includes finite unitless numeric source constants, resolved
across package/local references and numeric tuple members. The AST preserves
the authored expression and uses its existing resolved-expression projection
with the shared semantic unit-factor arithmetic. Runtime adapters all read
that projection; no Twin-side arithmetic parser or second numeric table is
needed. Invalid or unsupported initializers remain unresolved, and native
`SysmlModel.value` reports the qualified datum in its error.

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
the evidence reports canonical names and the coverage decision compares
snapshot-scoped SysML element handles. The authored name is used only to
select a source element; it is not the stored coverage key. The
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
memberships, plus the bounded source-linked constraint IR and Modelica
lowering path. Resolved calls to the implemented standard numeric, trig,
sequence-size, Boolean aggregate, and primitive `ToString` functions compile
into typed IR and evaluate against provider observations. The
`TrigFunctions::pi` constant, homogeneous scalar sequence literals, and
one-based sequence indexing also compile through that IR;
`sysml_standard_constants()` lists the constant.
`sysml_standard_functions()` exposes
the operation catalog and backend availability to Rhai policy, while
`sysml_constraint_operators()` lists the typed operators accepted by the IR.
The `constraint_ir`, `modelica_constraint`, and `evaluate_constraint` model
tools provide the three corresponding constraint paths. Modelica capability
reporting shares its lowering table with the backend, which reports an explicit
error when a call is outside its subset. This is not a full KerML execution
engine: user-defined function bodies/defaults, feature-chain navigation,
feature-valued and multidimensional collections, aggregates beyond the listed
standard subset, full quantity dimensional conversion, resolved
operator-function overload dispatch, behavioral execution, and automatic
requirement-to-USD projection remain outside the subset.
