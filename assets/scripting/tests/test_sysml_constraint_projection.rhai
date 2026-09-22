// Production Rhai proof that selected SysML facts stay typed and that exact
// attribute reads expose the native wrappers used by geometry policies.

import "sysml_modelica_constraints" as geometry_constraints;

let path = "lunco://scripting/tests/fixtures/sysml_constraint_projection.sysml";
let report = geometry_constraints::analyze_selected(path,
    ["upperMount", "lowerMount", "movingAnchor", "fixedAnchor",
     "movingAnchorSecond", "fixedAnchorSecond",
     "materialDensity", "vesselPressure", "elasticModulus"]);
let checks = [];
checks.push(t_true(type_of(report) == "map" && report.ok == true,
    "the standard SysML constraint fixture resolves through native source analysis"));
checks.push(t_true(report.analysis.constraints.len() == 5,
    "the native source snapshot exposes constraint definitions and usages"));
let selected_attributes = [];
for attribute in report.analysis.attributes {
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::upperMount"
        || attribute.qualified_name == "ConstraintProjection::StrutAssembly::lowerMount"
        || attribute.qualified_name == "ConstraintProjection::StrutAssembly::movingAnchor"
        || attribute.qualified_name == "ConstraintProjection::StrutAssembly::fixedAnchor"
        || attribute.qualified_name == "ConstraintProjection::StrutAssembly::movingAnchorSecond"
        || attribute.qualified_name == "ConstraintProjection::StrutAssembly::fixedAnchorSecond" {
        selected_attributes.push(attribute.qualified_name);
    }
}
checks.push(t_true(selected_attributes.len() == 6,
    "the Rhai policy selects the requested geometry inputs"));

let definition = false;
let usage = false;
let definition_handle = ();
let usage_handle = ();
let coincidence_definition = ();
for constraint in report.analysis.constraints {
    if constraint.element.qualified_name == "ConstraintProjection::SegmentMember"
        && constraint.element.kind == "ConstraintDefinition" {
        definition = true;
        definition_handle = constraint.element.handle;
    }
    if constraint.element.qualified_name == "ConstraintProjection::StrutAssembly::loadPath"
        && constraint.element.kind == "ConstraintUsage" {
        usage = true;
        usage_handle = constraint.element.handle;
    }
    if constraint.element.qualified_name == "ConstraintProjection::CoincidentPointTranslation"
        && constraint.element.kind == "ConstraintDefinition" {
        coincidence_definition = constraint;
    }
}
checks.push(t_true(definition && usage,
    "generic analysis preserves source-backed constraint identities"));

let type_link = false;
for reference in report.analysis.references {
    if type_of(reference.from) == "SysmlElementHandle"
        && type_of(reference.target) == "SysmlElementHandle"
        && reference.from == usage_handle && reference.target == definition_handle {
        type_link = true;
    }
}
checks.push(t_true(type_link,
    "Rhai receives native typed handles for source-backed constraint typing references"));

let expression = coincidence_definition.expressions[0];
checks.push(t_true(type_of(expression) == "SysmlExpression"
    && type_of(expression.operator) == "SysmlExpressionOperator"
    && expression.operator.is_equal() && expression.children.len() == 2
    && type_of(expression.children[0]) == "SysmlExpression"
    && expression.children[0].operator.is_add()
    && type_of(expression.children[0].children[0].feature) == "SysmlFeatureHandle"
    && type_of(expression.children[0].children[1].feature) == "SysmlFeatureHandle"
    && type_of(expression.children[1].feature) == "SysmlFeatureHandle"
    && expression.source.file.ends_with("sysml_constraint_projection.sysml")
    && expression.source.end > expression.source.start,
    "Rhai receives the resolved source-spanned constraint expression tree and feature handles"));

let selected = geometry_constraints::select(report,
    ["ConstraintProjection::SegmentMember"]);
checks.push(t_true(selected.ok == true && selected.definitions.len() == 1
    && selected.usages.len() == 1
    && selected.usages[0].element.qualified_name
        == "ConstraintProjection::StrutAssembly::loadPath",
    "Rhai policy selects the definition and its typed usage from the generic graph"));

let coincidence_backend = sysml_modelica_constraint(path,
    "ConstraintProjection::CoincidentPointTranslation");
checks.push(t_true(coincidence_backend.ok == true
    && coincidence_backend.ir.valid == true
    && coincidence_backend.ir.status == "valid"
    && coincidence_backend.ir.constraint.parameters.len() == 3
    && coincidence_backend.ir.constraint.parameters[0].direction == "in"
    && coincidence_backend.ir.constraint.parameters[0].type.value == "Quantity"
    && coincidence_backend.ir.constraint.parameters[0].type.lower == 3
    && coincidence_backend.ir.constraint.parameters[0].type.upper == 3
    && coincidence_backend.source.contains("model SysmlConstraint_")
    && coincidence_backend.source.contains("Real movingPoint[3]")
    && coincidence_backend.source.contains("equation"),
    "the neutral constraint IR exposes typed KerML parameters and the Rumoca-backed Modelica source"));

let binding_links = [];
for relationship in report.analysis.relationships {
    if relationship.element.kind != "BindingConnector" { continue; }
    if !relationship.element.qualified_name.starts_with(
        "ConstraintProjection::StrutAssembly::loadPath::") { continue; }
    let endpoints = [];
    for property in relationship.properties {
        if property.name == "relatedFeature" { endpoints = property.feature_targets; }
    }
    if endpoints.len() == 2
        && type_of(endpoints[0]) == "SysmlFeatureHandle"
        && type_of(endpoints[1]) == "SysmlFeatureHandle" {
        binding_links.push(relationship);
    }
}
checks.push(t_true(binding_links.len() == 2,
    "the standard SysML binding connectors resolve to native typed feature handles"));

let upper = ();
let lower = ();
let material_density = ();
let vessel_pressure = ();
let elastic_modulus = ();
let moving_anchor = ();
let fixed_anchor = ();
let moving_anchor_second = ();
let fixed_anchor_second = ();
for attribute in report.analysis.attributes {
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::upperMount" {
        upper = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::lowerMount" {
        lower = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::materialDensity" {
        material_density = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::vesselPressure" {
        vessel_pressure = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::elasticModulus" {
        elastic_modulus = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::movingAnchor" {
        moving_anchor = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::fixedAnchor" {
        fixed_anchor = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::movingAnchorSecond" {
        moving_anchor_second = attribute;
    }
    if attribute.qualified_name == "ConstraintProjection::StrutAssembly::fixedAnchorSecond" {
        fixed_anchor_second = attribute;
    }
}
checks.push(t_true(type_of(moving_anchor_second.handle) == "SysmlFeatureHandle"
    && type_of(fixed_anchor_second.owner_handle) == "SysmlElementHandle"
    && moving_anchor_second.owner_handle == fixed_anchor_second.owner_handle,
    "SysML attributes expose stable typed feature and component-owner handles"));
checks.push(t_true(type_of(upper.declared_type) == "SysmlType"
    && type_of(upper.declared_type.category) == "SysmlTypeCategory"
    && upper.declared_type.is_collection == true
    && upper.declared_type.is_quantity == true
    && type_of(upper.declared_type.quantity_kind) == "SysmlTypeRef"
    && upper.declared_type.quantity_kind.qualified_name == "ISQBase::LengthValue"
    && upper.declared_type.dimensions.len() == 1
    && upper.declared_type.dimensions[0] == 3
    && upper.value.kind == "vector" && upper.value.elements.len() == 3
    && upper.value.elements[0].kind == "quantity"
    && upper.value.elements[0].unit == "m"
    && type_of(upper.value.elements[0].number_value) == "f64"
    && upper.value.elements[0].number_value == 0.0
    && lower.value.kind == "vector" && lower.value.elements[0].unit == "m"
    && lower.value.elements[0].number_value == 0.5,
    "typed facts preserve resolved length identity, fixed cardinality, metre units, and numeric values"));
checks.push(t_true(material_density.declared_type.is_quantity == true
    && type_of(material_density.declared_type.quantity_kind) == "SysmlTypeRef"
    && material_density.declared_type.quantity_kind.qualified_name == "ISQMechanics::MassDensityValue"
    && vessel_pressure.declared_type.is_quantity == true
    && type_of(vessel_pressure.declared_type.quantity_kind) == "SysmlTypeRef"
    && vessel_pressure.declared_type.quantity_kind.qualified_name == "ISQMechanics::PressureValue"
    && elastic_modulus.declared_type.is_quantity == true
    && type_of(elastic_modulus.declared_type.quantity_kind) == "SysmlTypeRef"
    && elastic_modulus.declared_type.quantity_kind.qualified_name == "ISQMechanics::ModulusOfElasticityValue",
    "standard material-property quantity types resolve from SysML type inheritance"));

let upper_native = sysml_value(path,
    "ConstraintProjection::StrutAssembly::upperMount");
let lower_native = sysml_value(path,
    "ConstraintProjection::StrutAssembly::lowerMount");
let upper_attribute = sysml_attribute(path,
    "ConstraintProjection::StrutAssembly::upperMount");
checks.push(t_true(upper_attribute.ok == true && upper_attribute.found == true
    && type_of(upper_attribute.attribute) == "SysmlAttribute"
    && type_of(upper_attribute.attribute.declared_type) == "SysmlType"
    && upper_attribute.attribute.qualified_name
        == "ConstraintProjection::StrutAssembly::upperMount"
    && type_of(upper_attribute.attribute.typed_value) == "array"
    && type_of(upper_attribute.attribute.typed_value[0]) == "Quantity",
    "Rhai receives the source attribute and resolved type as native Rust-backed values"));
checks.push(t_true(upper_native.ok == true && upper_native.found == true
    && type_of(upper_native.value) == "array"
    && type_of(upper_native.value[0]) == "Quantity"
    && type_of(upper_native.value[0].kind) == "SysmlTypeRef"
    && upper_native.value[0].kind.qualified_name == "ISQBase::LengthValue"
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
    && type_of(assembled.start_binding) == "SysmlElementHandle"
    && type_of(assembled.end_binding) == "SysmlElementHandle"
    && type_of(assembled.start_feature) == "SysmlFeatureHandle"
    && type_of(assembled.end_feature) == "SysmlFeatureHandle"
    && assembled.start_feature == upper.handle
    && assembled.end_feature == lower.handle
    && assembled.start_binding_name.starts_with(assembled.usage + "::")
    && assembled.end_binding_name.starts_with(assembled.usage + "::")
    && assembled.start_feature_name == upper.qualified_name
    && assembled.end_feature_name == lower.qualified_name
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
    && coincidence.modelica_class == "SysmlCoincidentPointTranslationSetSolve"
    && coincidence.equation_origin == "resolved SysML constraint expression tree"
    && coincidence.numerical_backend == "Rumoca Modelica DAE"
    && type_of(coincidence.moving_feature) == "SysmlFeatureHandle"
    && type_of(coincidence.fixed_feature) == "SysmlFeatureHandle"
    && coincidence.moving_feature == moving_anchor.handle
    && coincidence.fixed_feature == fixed_anchor.handle
    && type_of(coincidence.moving_binding) == "SysmlElementHandle"
    && type_of(coincidence.fixed_binding) == "SysmlElementHandle"
    && coincidence.moving_binding_name.starts_with(coincidence.usage + "::")
    && coincidence.fixed_binding_name.starts_with(coincidence.usage + "::")
    && coincidence.source.contains("movingPoint1[1]")
    && coincidence.source.contains("translation[1]")
    && coincidence.source.contains("fixedPoint1[1]")
    && coincidence.source.contains("movingPoint1[1] + translation[1]")
    && coincidence.source.contains("relationResiduals[1]"),
    "Rhai lowers the resolved SysML equation into Rumoca Modelica scalar equations"));

let translation_set = geometry_constraints::assemble_coincident_translation_set(report,
    ["ConstraintProjection::StrutAssembly::alignMount",
     "ConstraintProjection::StrutAssembly::alignMountSecond"]);
checks.push(t_true(translation_set.ok == true
    && translation_set.relation_count == 2
    && translation_set.relations.len() == 2
    && translation_set.initial_translation_dof == 3
    && translation_set.constraint_jacobian_rank == 3
    && translation_set.remaining_translation_dof == 0
    && translation_set.closure_equation_count == 3
    && translation_set.relation_residual_outputs.len() == 6
    && translation_set.experiment_outputs.len() == 9
    && translation_set.source.contains("movingPoint2[1]")
    && translation_set.source.contains("fixedPoint2[3]")
    && translation_set.source.contains("relationResiduals[6]"),
    "Rhai assembles two bound datum relations into one three-DOF Rumoca model with six residual outputs"));

t_report(checks);
