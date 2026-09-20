// Production Rhai proof that generic SysML source analysis reaches Rhai as
// typed records and that the existing native vector adapter remains usable.

import "sysml_modelica_constraints" as geometry_constraints;

let path = "assets/scripting/tests/fixtures/sysml_constraint_projection.sysml";
let report = geometry_constraints::analyze_selected(path,
    ["upperMount", "lowerMount", "movingAnchor", "fixedAnchor"]);
let checks = [];
checks.push(t_true(type_of(report) == "map" && report.ok == true,
    "the standard SysML constraint fixture resolves through generic source analysis"));
checks.push(t_true(report.analysis.constraints.len() == 4,
    "the source query exposes generic constraint definitions and usages"));
let selected_attributes = [];
for attribute in report.analysis.attributes {
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::upperMount"
        || attribute.qualified_name == "ConstraintProjection::StrutAssembly::lowerMount"
        || attribute.qualified_name == "ConstraintProjection::StrutAssembly::movingAnchor"
        || attribute.qualified_name == "ConstraintProjection::StrutAssembly::fixedAnchor" {
        selected_attributes.push(attribute.qualified_name);
    }
}
checks.push(t_true(selected_attributes.len() == 4,
    "generic fact selection returns the four requested typed geometry inputs"));

let definition = false;
let usage = false;
for constraint in report.analysis.constraints {
    if constraint.element.qualified_name == "ConstraintProjection::SegmentMember"
        && constraint.element.kind == "ConstraintDefinition" {
        definition = true;
    }
    if constraint.element.qualified_name == "ConstraintProjection::StrutAssembly::loadPath"
        && constraint.element.kind == "ConstraintUsage" {
        usage = true;
    }
}
checks.push(t_true(definition && usage,
    "generic analysis preserves source-backed constraint identities"));

let type_link = false;
for reference in report.analysis.references {
    if reference.from == "ConstraintProjection::StrutAssembly::loadPath"
        && reference.target == "ConstraintProjection::SegmentMember" {
        type_link = true;
    }
}
checks.push(t_true(type_link,
    "Rhai receives source-backed constraint typing references for policy selection"));

let selected = geometry_constraints::select(report,
    ["ConstraintProjection::SegmentMember"]);
checks.push(t_true(selected.ok == true && selected.definitions.len() == 1
    && selected.usages.len() == 1
    && selected.usages[0].element.qualified_name
        == "ConstraintProjection::StrutAssembly::loadPath",
    "Rhai policy selects the definition and its typed usage from the generic graph"));

let binding_links = [];
for relationship in report.analysis.relationships {
    if relationship.element.kind != "BindingConnectorAsUsage" { continue; }
    let endpoints = [];
    for property in relationship.properties {
        if property.name == "relatedFeature" { endpoints = property.targets; }
    }
    if endpoints.len() == 2 { binding_links.push(relationship); }
}
checks.push(t_true(binding_links.len() == 2,
    "the standard SysML binding connectors resolve to two typed feature ends"));

let upper = ();
let lower = ();
for attribute in report.analysis.attributes {
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::upperMount" {
        upper = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::lowerMount" {
        lower = attribute;
    }
}
checks.push(t_true(upper.declared_type.quantity_kind == "Length"
    && upper.declared_type.dimensions.len() == 1
    && upper.declared_type.dimensions[0] == 3
    && upper.value.kind == "vector" && upper.value.elements.len() == 3
    && upper.value.elements[0].kind == "quantity"
    && upper.value.elements[0].unit == "m"
    && type_of(upper.value.elements[0].number_value) == "f64"
    && upper.value.elements[0].number_value == 0.0
    && lower.value.kind == "vector" && lower.value.elements[0].unit == "m"
    && lower.value.elements[0].number_value == 0.5,
    "typed facts preserve length kind, fixed cardinality, metre units, and numeric values"));

let upper_native = sysml_value(path,
    "ConstraintProjection::StrutAssembly::upperMount");
let lower_native = sysml_value(path,
    "ConstraintProjection::StrutAssembly::lowerMount");
checks.push(t_true(upper_native.ok == true && upper_native.found == true
    && type_of(upper_native.value) == "array"
    && type_of(upper_native.value[0]) == "Quantity"
    && upper_native.value[0].unit == "m" && upper_native.value[1].value == 1.0
    && upper_native.value[2].value == 0.0,
    "SysML-to-Rhai preserves each standard length quantity and its unit"));
checks.push(t_true(lower_native.ok == true && lower_native.found == true
    && type_of(lower_native.value) == "array"
    && type_of(lower_native.value[0]) == "Quantity"
    && lower_native.value[0].value == 0.5 && lower_native.value[1].value == -1.0
    && lower_native.value[2].value == 0.0,
    "typed length collections retain all three source quantities"));

let upper_vector = geometry_constraints::position_m(report,
    "ConstraintProjection::StrutAssembly::upperMount");
let lower_vector = geometry_constraints::position_m(report,
    "ConstraintProjection::StrutAssembly::lowerMount");
checks.push(t_true(upper_vector.ok == true && type_of(upper_vector.value) == "Vec3"
    && lower_vector.ok == true && type_of(lower_vector.value) == "Vec3"
    && upper_vector.value.x == 0.0 && upper_vector.value.y == 1.0
    && lower_vector.value.x == 0.5 && lower_vector.value.y == -1.0,
    "the geometry policy lowers only explicitly metre-typed coordinates to native f64 Vec3"));

let assembled = geometry_constraints::assemble_segment(report,
    "ConstraintProjection::StrutAssembly::loadPath");
checks.push(t_true(assembled.ok == true
    && assembled.source_revision_hex == report.analysis.source_revision_hex
    && assembled.constraint == "ConstraintProjection::SegmentMember"
    && assembled.usage == "ConstraintProjection::StrutAssembly::loadPath"
    && assembled.modelica_class == "LunCo.Geometry.Segment3D"
    && assembled.start_binding.starts_with(assembled.usage + "::")
    && assembled.end_binding.starts_with(assembled.usage + "::")
    && assembled.start_feature == "ConstraintProjection::StrutAssembly::upperMount"
    && assembled.end_feature == "ConstraintProjection::StrutAssembly::lowerMount"
    && assembled.source.contains("point_a={0.0, 1.0, 0.0}")
    && assembled.source.contains("point_b={0.5, -1.0, 0.0}")
    && assembled.source.contains("segmentLength = segment.length;")
    && assembled.source.contains("axis = segment.axis;"),
    "Rhai assembles a Modelica constraint wrapper from typed endpoints"));

let coincidence = geometry_constraints::assemble_coincident_translation(report,
    "ConstraintProjection::StrutAssembly::alignMount");
checks.push(t_true(coincidence.ok == true
    && coincidence.source_revision_hex == report.analysis.source_revision_hex
    && coincidence.constraint == "ConstraintProjection::CoincidentPointTranslation"
    && coincidence.usage == "ConstraintProjection::StrutAssembly::alignMount"
    && coincidence.modelica_class == "LunCo.Geometry.CoincidentPointTranslation3D"
    && coincidence.moving_feature == "ConstraintProjection::StrutAssembly::movingAnchor"
    && coincidence.fixed_feature == "ConstraintProjection::StrutAssembly::fixedAnchor"
    && coincidence.moving_binding.starts_with(coincidence.usage + "::")
    && coincidence.fixed_binding.starts_with(coincidence.usage + "::")
    && coincidence.source.contains("moving_point={0.0, 0.0, 0.0}")
    && coincidence.source.contains("fixed_point={1.25, -0.5, 2.0}")
    && coincidence.source.contains("translation = relation.translation;"),
    "Rhai assembles a Modelica coincidence-translation solve from typed bound points"));

t_report(checks);
