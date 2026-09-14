# SysML v2 requirements and Rhai verification

LunCoSim treats SysML v2 as the normative requirement source, USD as the
identity/topology source, Modelica as the continuous-equation source, and Rhai
as the executable test policy. A requirement is not accepted merely because a
source parser found its declaration: an authored verification case must cover
the requirement, and a Twin test must observe the composed USD/Modelica stage.

The generic `sysml_requirements` Rhai tool provides the small bridge:

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

Supported observations are `exists`, `children`, `attribute`,
`attribute_component`, `attribute_equals`, and `coverage`. `expected_attr`
must name a literal SysML attribute, so numeric limits are not copied into a
Rhai script. Every check carries a component and requirement ID, producing a
per-component evidence record with the source revision and exact USD path.

This is deliberately a subset of SysML v2 verification semantics: requirement
definitions/usages, subjects, attributes, and verification-case `verify`
memberships. It does not pretend to be a full KerML execution engine. The
subset is sufficient for deterministic system-level acceptance while remaining
portable to headless tests and interactive Twin review.
