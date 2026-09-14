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
       kind: "attribute", path: "/Twin/FLIP/Wheel_FL", attr: "radius",
       expected_attr: "visualWheelRadiusM", tolerance: 0.001 }
]);
report_verdict(result.failures, "VISUAL REQUIREMENTS", "VISUAL_REQUIREMENTS");
```

For inspection and tooling, the same snapshot is available as native Rhai
maps (no stringify/parse round trip):

```rhai
let report = sysml_requirement_report();
let all_declarations = sysml_report();
```

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
```

`luncosim test --scene tests/visual.usda --verification Project::VerifyVisual`
checks this registry mapping (qualified SysML name, Twin-relative scene and
Rhai observer, and verdict channel) before constructing the simulation. The
registry is metadata, not another requirement source; thresholds and units
remain in SysML literals.

Supported observations are `exists`, `children`, `attribute`,
`attribute_component`, `extent_component`, `bounds_component`,
`attribute_equals`, `relationship`, and `coverage`. `expected_attr` reads a
literal SysML attribute by its unique source attribute name, so numeric limits
are not copied into a Rhai script. The compact bridge uses short attribute
names; callers must resolve collisions through the full report instead of
silently choosing one. Every check carries a component and requirement ID,
producing a per-component evidence record with the source revision and exact
USD path.

This is deliberately a subset of SysML v2 verification semantics: requirement
definitions/usages, subjects, attributes, and verification-case `verify`
memberships. It does not pretend to be a full KerML execution engine. The
subset is sufficient for deterministic system-level acceptance while remaining
portable to headless tests and interactive Twin review.
