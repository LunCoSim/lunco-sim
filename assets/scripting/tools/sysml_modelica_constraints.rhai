// Rhai-owned policy bridge from source-backed SysML constraints to Modelica.
// Rust supplies the resolved typed graph. This library selects relevant
// definitions/usages and generates Modelica only at the Modelica source edge.

import "modelica_editor" as modelica_editor;
import "assembly_builder" as assembly_builder;
import "assembly_edit" as assembly_edit;
import "engineering_units" as engineering_units;
import "numerical_settings" as numerical_settings;

fn analyze(path) {
    let report = sysml_analysis(path);
    if map_is(report) && report.ok == true {
        let model = sysml_model(path);
        if type_of(model) == "SysmlModel" { report.model = model; }
    }
    report
}

/// Request only the geometry attributes this policy consumes. The generic
/// projection filters the immutable SysML snapshot before constructing Rhai
/// values; this policy owns the selected names and needs no Rust rebuild when
/// the design inputs change.
fn analyze_selected(path, attribute_names) {
    let report = sysml_analysis(path);
    if report == () || report.ok != true || report.analysis == () { return report; }
    let selected = [];
    for attribute in report.analysis.attributes {
        for name in attribute_names {
            if attribute.qualified_name == name || attribute.name == name {
                selected.push(attribute);
                break;
            }
        }
    }
    report.analysis.attributes = selected;
    let model = sysml_model(path);
    if type_of(model) == "SysmlModel" { report.model = model; }
    report
}

fn _short_name(qualified_name) {
    let parts = qualified_name.split("::");
    parts[parts.len() - 1]
}

fn _constraint_by_name(report, qualified_name, kind) {
    for record in report.analysis.constraints {
        if record.element.qualified_name == qualified_name
            && (kind == "" || record.element.kind == kind) { return record; }
    }
    ()
}

/// Compile one selected constraint through the native neutral IR and the
/// Rumoca Modelica boundary. Empty constraint definitions are legal semantic
/// declarations (and remain useful for binding discovery), but they are not
/// executable Modelica obligations until they have a body.
fn _modelica_backend(report, qualified_name) {
    if report.path == () || !string_is(report.path) {
        return #{ok: false, error: "SysML source path is unavailable"};
    }
    sysml_modelica_constraint(report.path, qualified_name)
}

fn _element_name(report, handle) {
    for element in report.analysis.elements {
        if element.handle == handle { return element.qualified_name; }
    }
    "<unmapped SysML element>"
}

fn _feature_for_owner_name(report, owner_handle, local_name) {
    for attribute in report.analysis.attributes {
        if attribute.owner_handle == owner_handle && attribute.name == local_name {
            return attribute.handle;
        }
    }
    for element in report.analysis.elements {
        if element.owner_handle == owner_handle
            && _short_name(element.qualified_name) == local_name
            && type_of(element.feature_handle) == "SysmlFeatureHandle" {
            return element.feature_handle;
        }
    }
    ()
}

fn _attribute_for_feature(report, feature_handle) {
    for attribute in report.analysis.attributes {
        if attribute.handle == feature_handle { return attribute; }
    }
    ()
}

fn _contains_handle(handles, candidate) {
    for handle in handles { if handle == candidate { return true; } }
    false
}

/// Select definitions, usages and relationship edges from resolved native
/// handles. Qualified names remain an explicit caller selector/display edge;
/// no graph join is performed on text.
fn select(report, requested) {
    if !map_is(report) || report.ok != true
        || report.analysis == () || report.analysis.constraints == () {
        return #{ok: false, error: "SysML analysis is unavailable"};
    }
    if !array_is(requested) || requested.is_empty() {
        return #{ok: false, error: "at least one qualified constraint definition is required"};
    }

    for identity in requested {
        if !string_is(identity) || !identity.contains("::") {
            return #{ok: false, error: "constraint identities must be qualified SysML names"};
        }
    }

    let definitions = [];
    let definition_handles = [];
    for identity in requested {
        let definition = _constraint_by_name(report, identity, "ConstraintDefinition");
        if definition == () { continue; }
        let backend = _modelica_backend(report, identity);
        if backend.ok != true && backend.ir.constraint != ()
            && backend.ir.constraint.expressions.len() > 0 {
            return #{ok: false, error: "constraint IR cannot cross the Rumoca Modelica boundary: "
                + backend.error};
        }
        definition.modelica_backend = backend;
        let handle = definition.element.handle;
        if _contains_handle(definition_handles, handle) { continue; }
        definitions.push(definition);
        definition_handles.push(handle);
    }
    if definitions.is_empty() {
        return #{ok: false, error: "no matching SysML constraint definition"};
    }

    let usages = [];
    let usage_handles = [];
    let related = [];
    let related_references = [];
    for reference in report.analysis.references {
        if !_contains_handle(definition_handles, reference.target) { continue; }
        for record in report.analysis.constraints {
            if record.element.kind == "ConstraintUsage"
                && record.element.handle == reference.from
                && !_contains_handle(usage_handles, reference.from) {
                usages.push(record);
                usage_handles.push(reference.from);
                related_references.push(reference);
            }
        }
    }
    for relationship in report.analysis.relationships {
        for usage in usages {
            if relationship.element.owner_handle == usage.element.handle {
                related.push(relationship);
                break;
            }
        }
    }

    let definition_edges = [];
    for relationship in report.analysis.relationships {
        let related_to_definition = false;
        let related_to_usage = _contains_handle(usage_handles, relationship.element.handle);
        for property in relationship.properties {
            for target in property.targets {
                if _contains_handle(definition_handles, target) { related_to_definition = true; }
            }
        }
        if related_to_definition && !related_to_usage { definition_edges.push(relationship); }
    }
    related += definition_edges;

    #{ok: true, definitions: definitions, usages: usages,
      relationships: related, references: related_references}
}

/// Read exactly three length quantities, resolve their UCUM/SI units through
/// the shared native quantity adapter, then lower them to the shared f64
/// Bevy/Rhai Vec3. Unit conversion is never inferred from a bare number.
/// A qualified name is accepted only as a public selector; all later joins
/// use the attribute's native feature and owner handles.
fn position_m(report, feature_selector) {
    if !map_is(report) || report.analysis == ()
        || report.analysis.attributes == () {
        return #{ok: false, error: "SysML analysis has no attribute records"};
    }
    let selected = ();
    for attribute in report.analysis.attributes {
        if type_of(feature_selector) == "SysmlFeatureHandle"
            && attribute.handle == feature_selector { selected = attribute; break; }
        if string_is(feature_selector)
            && attribute.qualified_name == feature_selector { selected = attribute; break; }
    }
    if selected != () {
        let attribute = selected;
        let qualified_name = attribute.qualified_name;
        if attribute.declared_type == ()
            || attribute.declared_type.is_quantity != true
            || attribute.declared_type.quantity_kind == ()
            || attribute.declared_type.quantity_kind.qualified_name != "ISQBase::LengthValue"
            || attribute.declared_type.dimensions.len() != 1
            || attribute.declared_type.dimensions[0] != 3 {
            return #{ok: false, error: "SysML position must be a fixed three-element length quantity: " + qualified_name};
        }
        if attribute.value == () || attribute.value.kind != "vector"
            || attribute.value.elements == () || attribute.value.elements.len() != 3 {
            return #{ok: false, error: "SysML position is not a three-element length collection: " + qualified_name};
        }
        let values = [];
        for element in attribute.value.elements {
            if element.number_value == () || element.unit == () {
                return #{ok: false, error: "SysML position components must be finite quantities: " + qualified_name};
            }
            let source_unit = engineering_units::ucum_unit(element.unit);
            let metre_unit = engineering_units::ucum_unit("m");
            let source_quantity = if source_unit == () || metre_unit == () {
                ()
            } else { quantity(element.number_value, source_unit) };
            let value = if source_quantity == () { () } else {
                quantity_value_in(source_quantity, metre_unit)
            };
            if value == () {
                return #{ok: false, error: "SysML position components use unsupported or incompatible units: " + qualified_name};
            }
            values.push(value);
        }
        return #{ok: true, value: vec3(values[0], values[1], values[2]),
          feature: attribute.handle, owner: attribute.owner_handle,
          display_name: attribute.qualified_name};
    }
    #{ok: false, error: "SysML vector feature is missing from the selected source snapshot"}
}

/// Resolve one constraint-definition input through a standard SysML binding
/// connector. The relationship's typed ends carry the definition feature and
/// the bound part feature; the reference graph ties the connector to the
/// particular constraint usage in whose context it is declared.
fn bound_vector(report, usage_selector, input_name) {
    if !map_is(report) || report.analysis == ()
        || report.analysis.relationships == () || report.analysis.references == () {
        return #{ok: false, error: "SysML analysis has no binding relationship facts"};
    }
    let usage = if map_is(usage_selector) { usage_selector }
        else { _constraint_by_name(report, usage_selector, "ConstraintUsage") };
    if usage == () { return #{ok: false, error: "constraint usage selector did not resolve"}; }
    let usage_handle = usage.element.handle;
    let usage_name = usage.element.qualified_name;
    let definition = _definition_for_usage(report, usage_handle);
    if definition == () {
        return #{ok: false, error: "constraint usage has no unique source definition: " + usage_name};
    }
    let formal_handle = _feature_for_owner_name(
        report, definition.element.handle, input_name);
    if formal_handle == () {
        return #{ok: false, error: "constraint definition input is not a resolved feature: " + input_name};
    }
    let candidates = [];
    for relationship in report.analysis.relationships {
        if relationship.element.kind != "BindingConnector" { continue; }
        let belongs_to_usage = relationship.element.owner_handle == usage_handle;
        let binds_formal = false;
        for reference in report.analysis.references {
            if reference.from != relationship.element.handle { continue; }
            if reference.target == usage_handle { belongs_to_usage = true; }
            if reference.target == formal_handle.element { binds_formal = true; }
        }
        if !belongs_to_usage || !binds_formal { continue; }

        let related_features = ();
        for property in relationship.properties {
            if property.name == "relatedFeature" { related_features = property.feature_targets; }
        }
        if !array_is(related_features) || related_features.len() != 2 {
            return #{ok: false, error: "binding connector must resolve to exactly two feature ends: "
                + relationship.element.qualified_name};
        }
        let bound_feature = ();
        for target in related_features {
            if target.element != formal_handle.element { bound_feature = target; }
        }
        if bound_feature == () {
            return #{ok: false, error: "binding connector does not bind a distinct part feature: "
                + relationship.element.qualified_name};
        }
        candidates.push(#{binding: relationship.element.handle, feature: bound_feature});
    }
    if candidates.len() != 1 {
        return #{ok: false, error: "expected one binding from " + usage_name + "." + input_name
            + ", found " + candidates.len()};
    }
    let resolved = position_m(report, candidates[0].feature);
    if resolved.ok != true { return resolved; }
    #{ok: true, binding: candidates[0].binding,
      feature: candidates[0].feature, feature_name: resolved.display_name,
      owner: resolved.owner, value: resolved.value}
}

fn _definition_for_usage(report, usage_handle) {
    for reference in report.analysis.references {
        if reference.from != usage_handle { continue; }
        for constraint in report.analysis.constraints {
            if constraint.element.kind != "ConstraintDefinition"
                || constraint.element.handle != reference.target { continue; }
            return constraint;
        }
    }
    ()
}

fn _modelica_real(value) {
    let rendered = value.to_string();
    if !rendered.contains(".") && !rendered.contains("e") && !rendered.contains("E") {
        rendered += ".0";
    }
    rendered
}

fn _modelica_vec3(value) {
    "{" + _modelica_real(value.x) + ", "
        + _modelica_real(value.y) + ", " + _modelica_real(value.z) + "}"
}

/// Assemble a one-shot Modelica solve/readback model for one SegmentMember
/// usage. Its standard SysML binding connectors select both endpoint features;
/// callers never duplicate the feature mapping in Rhai arguments.
fn assemble_segment(report, usage_name) {
    if !map_is(report) || report.analysis == () {
        return #{ok: false, error: "SysML analysis is unavailable"};
    }
    let usage = _constraint_by_name(report, usage_name, "ConstraintUsage");
    if usage == () {
        return #{ok: false, error: "segment constraint usage selector did not resolve: " + usage_name};
    }
    let definition = _definition_for_usage(report, usage.element.handle);
    if definition == () || _short_name(definition.element.qualified_name) != "SegmentMember" {
        return #{ok: false, error: "constraint usage is not linked to the SegmentMember geometry policy: " + usage_name};
    }
    let selection = select(report, [definition.element.qualified_name]);
    if selection.ok != true { return selection; }
    let semantic_backend = selection.definitions[0].modelica_backend;
    let start = bound_vector(report, usage_name, "start");
    if start.ok != true { return start; }
    let finish = bound_vector(report, usage_name, "finish");
    if finish.ok != true { return finish; }

    let source = "model SysmlSegmentConstraintSolve\n"
        + "  LunCo.Geometry.Segment3D segment(\n"
        + "    point_a=" + _modelica_vec3(start.value) + ",\n"
        + "    point_b=" + _modelica_vec3(finish.value) + ");\n"
        + "  output Real segmentLength(unit=\"m\");\n"
        + "  output Real midpoint[3](each unit=\"m\");\n"
        + "  output Real axis[3];\n"
        + "equation\n"
        + "  segmentLength = segment.length;\n"
        + "  midpoint = segment.midpoint;\n"
        + "  axis = segment.axis;\n"
        + "end SysmlSegmentConstraintSolve;\n";
    #{ok: true, class: "SysmlSegmentConstraintSolve", source: source,
      source_revision_hex: report.analysis.source_revision_hex,
      constraint: definition.element.qualified_name,
      usage: usage_name, modelica_class: "LunCo.Geometry.Segment3D",
      start_feature: start.feature, end_feature: finish.feature,
      start_feature_name: start.feature_name, end_feature_name: finish.feature_name,
      start_binding: start.binding, end_binding: finish.binding,
      start_binding_name: _element_name(report, start.binding),
      end_binding_name: _element_name(report, finish.binding),
      constraint_ir: semantic_backend.ir,
      rumoca_modelica_source: semantic_backend.source,
      start: start.value, finish: finish.value}
}

/// Apply the assembled source to one explicit Modelica Editor document. The
/// Modelica owner enforces the inspected generation and keeps the replacement
/// undoable; scene geometry remains untouched until a separate reviewed USD
/// placement plan consumes the solved result.
fn apply_segment(doc_id, parent_generation, report, usage_name) {
    let generated = assemble_segment(report, usage_name);
    if generated.ok != true { return generated; }
    let edit = modelica_editor::replace_source(
        doc_id, generated.source, parent_generation);
    #{ok: edit.ok == true, generated: generated, edit: edit}
}

fn _is_feature_expression(expression, feature) {
    type_of(expression) == "SysmlExpression"
        && type_of(expression.feature) == "SysmlFeatureHandle"
        && expression.feature == feature
}

fn _unwrap_expression_group(expression) {
    let current = expression;
    while type_of(current) == "SysmlExpression"
        && current.feature == () && current.operator == ()
        && current.integer_value == () && current.real_value == ()
        && current.boolean_value == () && current.string_value == ()
        && current.unsupported == () && current.children.len() == 1 {
        current = current.children[0];
    }
    current
}

fn _coincident_translation_equation(report, definition) {
    let moving = _feature_for_owner_name(report, definition.element.handle, "movingPoint");
    let translation = _feature_for_owner_name(report, definition.element.handle, "translation");
    let fixed = _feature_for_owner_name(report, definition.element.handle, "fixedPoint");
    if moving == () || translation == () || fixed == () { return (); }
    for expression in definition.expressions {
        if type_of(expression) != "SysmlExpression"
            || type_of(expression.operator) != "SysmlExpressionOperator"
            || !expression.operator.is_equal() || expression.children.len() != 2 { continue; }
        let lhs = _unwrap_expression_group(expression.children[0]);
        let rhs = _unwrap_expression_group(expression.children[1]);
        let sum = lhs;
        let target = rhs;
        if type_of(sum) == "SysmlExpression"
            && type_of(sum.operator) == "SysmlExpressionOperator"
            && sum.operator.is_add() && sum.children.len() == 2
            && _is_feature_expression(target, fixed) {
            let first = _unwrap_expression_group(sum.children[0]);
            let second = _unwrap_expression_group(sum.children[1]);
            if _is_feature_expression(first, moving)
                && _is_feature_expression(second, translation) {
                return #{ expression: expression, lhs: expression.children[0],
                    rhs: expression.children[1], moving: moving,
                    translation: translation, fixed: fixed };
            }
            if _is_feature_expression(first, translation)
                && _is_feature_expression(second, moving) {
                return #{ expression: expression, lhs: expression.children[0],
                    rhs: expression.children[1], moving: moving,
                    translation: translation, fixed: fixed };
            }
        }
    }
    ()
}

fn _modelica_scalar_expression(expression, axis, context) {
    if type_of(expression) != "SysmlExpression" {
        return #{ok: false, error: "constraint expression node is not typed SysML"};
    }
    if type_of(expression.feature) == "SysmlFeatureHandle" {
        if expression.feature == context.moving_feature {
            return #{ok: true, source: context.moving_name + "[" + axis.to_string() + "]"};
        }
        if expression.feature == context.translation_feature {
            return #{ok: true, source: "translation[" + axis.to_string() + "]"};
        }
        if expression.feature == context.fixed_feature {
            return #{ok: true, source: context.fixed_name + "[" + axis.to_string() + "]"};
        }
        return #{ok: false, error: "constraint references a feature outside the bound translation relation"};
    }
    if expression.integer_value != () {
        return #{ok: true, source: expression.integer_value.to_string()};
    }
    if expression.real_value != () {
        let real_value = f64_only(expression.real_value);
        if real_value == () {
            return #{ok: false, error: "constraint contains a non-finite real literal"};
        }
        return #{ok: true, source: _modelica_real(real_value)};
    }
    if expression.boolean_value != () {
        return #{ok: true, source: if expression.boolean_value { "true" } else { "false" }};
    }
    if expression.unsupported != () {
        return #{ok: false, error: "unsupported SysML expression at "
            + expression.source.file + ":" + expression.source.start.to_string()};
    }
    let children = expression.children;
    if children.len() == 1 {
        let inner = _modelica_scalar_expression(children[0], axis, context);
        if inner.ok != true { return inner; }
        if expression.operator == () {
            return #{ok: true, source: "(" + inner.source + ")"};
        }
        let symbol = expression.operator.modelica_symbol();
        if !string_is(symbol) {
            return #{ok: false, error: "SysML unary operator has no supported Modelica spelling"};
        }
        return #{ok: true, source: "(" + symbol + " " + inner.source + ")"};
    }
    if children.len() == 2 {
        if type_of(expression.operator) != "SysmlExpressionOperator"
            || expression.operator.is_equal() {
            return #{ok: false, error: "nested equality is not a scalar Modelica expression"};
        }
        let symbol = expression.operator.modelica_symbol();
        if !string_is(symbol) {
            return #{ok: false, error: "SysML binary operator has no supported Modelica spelling"};
        }
        let lhs = _modelica_scalar_expression(children[0], axis, context);
        if lhs.ok != true { return lhs; }
        let rhs = _modelica_scalar_expression(children[1], axis, context);
        if rhs.ok != true { return rhs; }
        return #{ok: true, source: "(" + lhs.source + " " + symbol + " " + rhs.source + ")"};
    }
    #{ok: false, error: "constraint expression is not a supported scalar Modelica expression"}
}

/// Resolve and assemble one or more coincident-point usages that place the
/// same component by one shared translation. The first relation anchors the
/// three unknown DOFs. Every additional relation contributes a three-axis
/// closure residual, so inconsistent datums are detected instead of silently
/// overconstraining or moving the component to satisfy only the last mate.
fn assemble_coincident_translation_set(report, usage_selectors) {
    if !map_is(report) || report.analysis == () {
        return #{ok: false, error: "SysML analysis is unavailable"};
    }
    if !array_is(usage_selectors) || usage_selectors.is_empty() {
        return #{ok: false, error: "at least one coincident-translation usage selector is required"};
    }

    let definitions = [];
    let usages = [];
    let moving_points = [];
    let fixed_points = [];
    let moving_features = [];
    let fixed_features = [];
    let moving_bindings = [];
    let fixed_bindings = [];
    let moving_feature_names = [];
    let fixed_feature_names = [];
    let usage_handles = [];
    let definition_handle = ();
    let common_moving_owner = ();
    let common_fixed_owner = ();

    for usage_selector in usage_selectors {
        if !string_is(usage_selector) {
            return #{ok: false, error: "usage selectors must be qualified SysML names"};
        }
        let usage = _constraint_by_name(report, usage_selector, "ConstraintUsage");
        if usage == () {
            return #{ok: false, error: "coincident-translation usage is missing: " + usage_selector};
        }
        if _contains_handle(usage_handles, usage.element.handle) {
            return #{ok: false, error: "a coincident-translation usage was selected more than once"};
        }
        let definition = _definition_for_usage(report, usage.element.handle);
        if definition == ()
            || _short_name(definition.element.qualified_name) != "CoincidentPointTranslation" {
            return #{ok: false, error: "usage is not typed by CoincidentPointTranslation: " + usage_selector};
        }
        if definition_handle == () { definition_handle = definition.element.handle; }
        else if definition.element.handle != definition_handle {
            return #{ok: false, error: "all relations in one solve must share the same resolved constraint definition"};
        }

        let moving = bound_vector(report, usage, "movingPoint");
        if moving.ok != true { return moving; }
        let fixed = bound_vector(report, usage, "fixedPoint");
        if fixed.ok != true { return fixed; }
        if moving.owner == () || fixed.owner == () {
            return #{ok: false, error: "bound datum features must have resolved semantic owners"};
        }
        if common_moving_owner == () { common_moving_owner = moving.owner; }
        else if moving.owner != common_moving_owner {
            return #{ok: false, error: "the relation set has more than one moving component owner"};
        }
        if common_fixed_owner == () { common_fixed_owner = fixed.owner; }
        else if fixed.owner != common_fixed_owner {
            return #{ok: false, error: "the relation set has more than one fixed-frame owner"};
        }

        definitions.push(definition);
        usages.push(usage);
        usage_handles.push(usage.element.handle);
        moving_points.push(moving.value);
        fixed_points.push(fixed.value);
        moving_features.push(moving.feature);
        fixed_features.push(fixed.feature);
        moving_bindings.push(moving.binding);
        fixed_bindings.push(fixed.binding);
        moving_feature_names.push(moving.feature_name);
        fixed_feature_names.push(fixed.feature_name);
    }

    let definition = definitions[0];
    let equation = _coincident_translation_equation(report, definition);
    if equation == () {
        return #{ok: false, error: "resolved SysML constraint body is not movingPoint + translation == fixedPoint"};
    }

    let source = "model SysmlCoincidentPointTranslationSetSolve\n";
    for relation_index in 0..usages.len() {
        let modelica_index = relation_index + 1;
        source += "  parameter Real movingPoint" + modelica_index.to_string()
            + "[3](each unit=\"m\") = "
            + _modelica_vec3(moving_points[relation_index]) + ";\n";
        source += "  parameter Real fixedPoint" + modelica_index.to_string()
            + "[3](each unit=\"m\") = "
            + _modelica_vec3(fixed_points[relation_index]) + ";\n";
    }
    source += "  output Real translation[3](each unit=\"m\");\n";
    let closure_count = 3 * (usages.len() - 1);
    source += "  output Real relationResiduals[" + (3 * usages.len()).to_string()
        + "](each unit=\"m\");\n";
    source += "equation\n";
    let relation_residual_outputs = [];
    for relation_index in 0..usages.len() {
        let modelica_index = relation_index + 1;
        // Expression leaves identify the formal features owned by the
        // ConstraintDefinition; the bound instance features above are a
        // separate identity space used for source provenance and placement.
        let context = #{moving_feature: equation.moving,
            translation_feature: equation.translation,
            fixed_feature: equation.fixed,
            moving_name: "movingPoint" + modelica_index.to_string(),
            fixed_name: "fixedPoint" + modelica_index.to_string()};
        for axis in 1..4 {
            let lhs = _modelica_scalar_expression(equation.lhs, axis, context);
            if lhs.ok != true { return lhs; }
            let rhs = _modelica_scalar_expression(equation.rhs, axis, context);
            if rhs.ok != true { return rhs; }
            if relation_index == 0 {
                source += "  " + lhs.source + " = " + rhs.source + ";\n";
            }
            let residual_index = relation_index * 3 + axis;
            let output_name = "relationResiduals[" + residual_index.to_string() + "]";
            relation_residual_outputs.push(output_name);
            source += "  " + output_name + " = (" + lhs.source + ") - ("
                + rhs.source + ");\n";
        }
    }
    source += "end SysmlCoincidentPointTranslationSetSolve;\n";

    let relation_evidence = [];
    for index in 0..usages.len() {
        relation_evidence.push(#{
            usage: usages[index].element.qualified_name,
            usage_handle: usages[index].element.handle,
            moving_feature: moving_features[index],
            fixed_feature: fixed_features[index],
            moving_feature_name: moving_feature_names[index],
            fixed_feature_name: fixed_feature_names[index],
            moving_binding: moving_bindings[index],
            fixed_binding: fixed_bindings[index],
            moving_point: moving_points[index],
            fixed_point: fixed_points[index]
        });
    }
    let experiment_outputs = ["translation[1]", "translation[2]", "translation[3]"];
    experiment_outputs += relation_residual_outputs;
    #{ok: true, class: "SysmlCoincidentPointTranslationSetSolve", source: source,
      source_revision_hex: report.analysis.source_revision_hex,
      source_fingerprint: report.analysis.source_fingerprint,
      constraint: definition.element.qualified_name,
      usage: usages[0].element.qualified_name,
      usages: usages.map(|record| record.element.qualified_name),
      relation_count: usages.len(), relations: relation_evidence,
      modelica_class: "SysmlCoincidentPointTranslationSetSolve",
      equation_origin: "resolved SysML constraint expression tree",
      numerical_backend: "Rumoca Modelica DAE",
      moving_point: moving_points[0], fixed_point: fixed_points[0],
      moving_points: moving_points, fixed_points: fixed_points,
      moving_owner: common_moving_owner, fixed_owner: common_fixed_owner,
      initial_translation_dof: 3, constraint_jacobian_rank: 3,
      remaining_translation_dof: 0, closure_equation_count: closure_count,
      relation_residual_outputs: relation_residual_outputs,
      experiment_outputs: experiment_outputs}
}

/// Compatibility-shaped single relation helper; it delegates to the same
/// relation-set policy and exposes display strings only in its report fields.
fn assemble_coincident_translation(report, usage_name) {
    let generated = assemble_coincident_translation_set(report, [usage_name]);
    if generated.ok != true { return generated; }
    let relation = generated.relations[0];
    generated.moving_feature = relation.moving_feature;
    generated.fixed_feature = relation.fixed_feature;
    generated.moving_feature_name = relation.moving_feature_name;
    generated.fixed_feature_name = relation.fixed_feature_name;
    generated.moving_binding = relation.moving_binding;
    generated.fixed_binding = relation.fixed_binding;
    generated.moving_binding_name = _element_name(report, relation.moving_binding);
    generated.fixed_binding_name = _element_name(report, relation.fixed_binding);
    generated
}

/// Apply the generated translation model to one explicit Modelica Editor
/// document. This does not mutate USD or choose a scene edit target.
fn apply_coincident_translation(doc_id, parent_generation, report, usage_name) {
    let generated = assemble_coincident_translation(report, usage_name);
    if generated.ok != true { return generated; }
    let edit = modelica_editor::replace_source(
        doc_id, generated.source, parent_generation);
    #{ok: edit.ok == true, generated: generated, edit: edit}
}

/// Replace one explicit scratch/Modelica document with the generated
/// constraint wrapper, then create a caller-owned async solve ticket. The
/// caller polls `poll_coincident_translation` on later ticks; no engine frame
/// is blocked while Rumoca compiles or the experiment runner publishes data.
fn begin_coincident_translation(doc_id, parent_generation, report, usage_name) {
    let generated = assemble_coincident_translation(report, usage_name);
    if generated.ok != true { return generated; }
    let before = query("GetDocumentSource", #{ doc_id: doc_id });
    if before == () || before.ok == false || before.generation != parent_generation {
        return #{ ok: false, stage: "source", retryable: false,
            error: "Modelica source generation changed before constraint replacement",
            generated: generated, inspection: before };
    }
    let edit = modelica_editor::replace_source(
        doc_id, generated.source, parent_generation);
    if edit == () || edit.ok != true {
        return #{ ok: false, stage: "source", retryable: false,
            error: "Modelica Editor rejected the generated constraint source",
            generated: generated, edit: edit };
    }
    if edit.status != "pending" && edit.status != "applied" {
        return #{ ok: false, stage: "source", retryable: false,
            error: "Modelica source replacement did not reach the applied state",
            generated: generated, edit: edit };
    }
    if edit.status == "pending" && edit.id == () {
        return #{ ok: false, stage: "source", retryable: false,
            error: "pending Modelica source replacement omitted its command identity",
            generated: generated, edit: edit };
    }
    let state = if edit.status == "pending" { "source_apply" } else { "source_readback" };
    #{ ok: true, pending: true, complete: false, stage: state,
       generated: generated, edit: edit,
       ticket: #{ stage: state, doc_id: doc_id,
           parent_generation: parent_generation, command_id: edit.id,
           poll_count: 0, edit: edit } }
}

fn _coincident_translation_experiment_ticket(doc_id, parent_generation, generated, edit, after) {
    if after == () || after.ok == false || after.source != generated.source
        || after.generation == () || after.generation <= parent_generation {
        let actual_generation = if after == () { "unavailable" }
            else if after.generation == () { "unavailable" }
            else { after.generation.to_string() };
        let actual_length = if after == () || after.source == () { -1 }
            else { after.source.len() };
        return #{ ok: false, stage: "source_readback", retryable: false,
            error: "Modelica Editor source readback mismatched generated wrapper (expected generation > "
                + parent_generation.to_string() + ", actual generation " + actual_generation
                + ", expected bytes " + generated.source.len().to_string()
                + ", actual bytes " + actual_length.to_string() + ")",
            generated: generated, edit: edit, inspection: after };
    }
    // `label` is for the Experiments UI only; execution identity comes from
    // the deferred RunExperiment acknowledgement, never from name matching.
    let label = "SysML geometry solve: " + generated.source_revision_hex
        + " / " + generated.usage;
    let ticket = modelica_editor::experiment_ticket(
        doc_id, generated.class, after.generation, label,
        generated.experiment_outputs,
        0.0, 0.01, 0.01, 1800);
    if ticket == () || ticket.ok != true {
        return #{ ok: false, stage: "ticket", retryable: false,
            error: "Modelica Editor could not create a valid experiment ticket",
            generated: generated, edit: edit, source_generation: after.generation,
            ticket: ticket };
    }
    #{ ok: true, pending: true, generated: generated, edit: edit,
       source_generation: after.generation, ticket: ticket.ticket }
}

/// Read the completed Modelica translation series without text/CSV parsing.
/// Every requested output must be a finite f64 sample and the solved point
/// must coincide with the SysML-bound target within a metre-scale tolerance.
fn coincident_translation_readback(generated, completed) {
    if !map_is(generated) || generated.ok != true
        || !map_is(completed) || completed.ok != true
        || completed.complete != true || completed.ticket == ()
        || completed.result == () {
        return #{ ok: false, stage: "readback", error: "a completed generated Modelica experiment is required" };
    }
    let numerical_policy = numerical_settings::active();
    let residual_tolerance = numerical_settings::require(
        numerical_policy, "solver_abs");
    if residual_tolerance == () {
        return #{ ok: false, stage: "readback", error:
            "Twin setting numerics.solver.residual_abs is required as a finite, non-negative f64" };
    }
    let result = completed.result;
    let source = query("GetDocumentSource", #{ doc_id: completed.ticket.doc_id });
    if source == () || source.ok == false
        || source.generation != completed.ticket.source_generation {
        return #{ ok: false, stage: "readback",
            error: "Modelica source generation changed before placement readback (expected "
                + completed.ticket.source_generation.to_string() + ", observed "
                + if source == () || source.generation == () { "unavailable" }
                  else { source.generation.to_string() },
            expected_generation: completed.ticket.source_generation,
            observed_generation: if source == () { () } else { source.generation },
            source_snapshot: source };
    }
    if result.state != "done" || result.times == () || result.times.is_empty()
        || result.series == () {
        return #{ ok: false, stage: "readback", error: "completed experiment has no numeric trajectory" };
    }
    let components = [];
    for axis in 1..4 {
        let name = "translation[" + axis.to_string() + "]";
        let samples = result.series[name];
        if !array_is(samples) || samples.is_empty() {
            return #{ ok: false, stage: "readback", error: "missing Modelica translation output " + name };
        }
        let value = f64_only(samples[samples.len() - 1]);
        if value == () {
            return #{ ok: false, stage: "readback", error: "translation output is not finite f64 data: " + name };
        }
        components.push(value);
    }
    let translation = vec3(components[0], components[1], components[2]);
    let relation_residuals = [];
    let all_closed = true;
    for relation_index in 0..generated.relations.len() {
        let relation = generated.relations[relation_index];
        let residual_components = [];
        for axis in 1..4 {
            let sample_index = relation_index * 3 + axis;
            let name = "relationResiduals[" + sample_index.to_string() + "]";
            let samples = result.series[name];
            if !array_is(samples) || samples.is_empty() {
                return #{ ok: false, stage: "readback",
                    error: "missing Modelica relation residual output " + name };
            }
            let value = f64_only(samples[samples.len() - 1]);
            if value == () {
                return #{ ok: false, stage: "readback",
                    error: "Modelica relation residual is not finite f64 data: " + name };
            }
            residual_components.push(value);
        }
        let residual = vec3(residual_components[0], residual_components[1], residual_components[2]);
        let satisfied = vlen(residual) <= residual_tolerance;
        if !satisfied { all_closed = false; }
        relation_residuals.push(#{usage: relation.usage,
            usage_handle: relation.usage_handle,
            moving_feature: relation.moving_feature,
            fixed_feature: relation.fixed_feature,
            residual: residual, residual_m: vlen(residual),
            residual_tolerance_m: residual_tolerance,
            satisfied: satisfied});
    }
    let dof = #{initial: generated.initial_translation_dof,
        rank: generated.constraint_jacobian_rank,
        remaining: generated.remaining_translation_dof,
        closure_equations: generated.closure_equation_count};
    if !all_closed {
        return #{ok: false, stage: "equation_check",
            error: "the shared translation does not close every source-bound SysML relation",
            translation: translation, relation_residuals: relation_residuals,
            residual_tolerance_m: residual_tolerance, dof: dof};
    }
    #{ ok: true, kind: "coincident_translation_set_solution",
       translation: translation, units: "m", residual: relation_residuals[0].residual,
       relation_residuals: relation_residuals, relation_count: generated.relation_count,
       residual_tolerance_m: residual_tolerance,
       dof: dof,
       moving_point: generated.moving_point, fixed_point: generated.fixed_point,
       constraint: generated.constraint, usage: generated.usage, usages: generated.usages,
       source_revision_hex: generated.source_revision_hex,
       modelica_doc_id: completed.ticket.doc_id,
       source_generation: completed.ticket.source_generation,
       modelica_class: generated.class, experiment_id: completed.ticket.experiment_id,
       sample_time: result.times[result.times.len() - 1], result: result }
}

/// Advance one source-application or Modelica solve ticket. Deferred document
/// edits are polled through the shared command-result API before the generated
/// wrapper is inspected or dispatched; pending remains non-blocking.
fn poll_coincident_translation(ticket, generated) {
    if !map_is(ticket) || ticket.stage == () {
        return #{ ok: false, stage: "ticket", retryable: false,
            error: "a source-application or Modelica experiment ticket is required" };
    }
    if ticket.stage == "source_apply" {
        ticket.poll_count += 1;
        let command = command_result(ticket.command_id);
        if command == () || command.status == "pending" {
            if ticket.poll_count > 300 {
                return #{ ok: false, stage: "source_apply", retryable: true,
                    error: "Modelica source replacement exceeded its bounded command-poll budget",
                    ticket: ticket, command: command };
            }
            return #{ ok: true, pending: true, complete: false,
                stage: "source_apply", ticket: ticket, command: command };
        }
        if command.ok != true || command.status != "applied" {
            let message = if command.error == () {
                "Modelica source replacement did not complete successfully"
            } else {
                command.error
            };
            return #{ ok: false, stage: "source_apply", retryable: false,
                error: message, ticket: ticket, command: command };
        }
        ticket.stage = "source_readback";
        ticket.poll_count = 0;
        ticket.edit = command;
        return #{ ok: true, pending: true, complete: false,
            stage: "source_readback", ticket: ticket, command: command };
    }
    if ticket.stage == "source_readback" {
        ticket.poll_count += 1;
        let after = query("GetDocumentSource", #{ doc_id: ticket.doc_id });
        if after != () && after.ok != false && after.generation != ()
            && after.generation > ticket.parent_generation
            && after.source == generated.source {
            let prepared = _coincident_translation_experiment_ticket(
                ticket.doc_id, ticket.parent_generation, generated, ticket.edit, after);
            if prepared.ok != true { return prepared; }
            ticket = prepared.ticket;
        } else {
            if after != () && after.ok != false && after.generation != ()
                && after.generation > ticket.parent_generation
                && after.source != generated.source {
                return #{ ok: false, stage: "source_readback", retryable: false,
                    error: "Modelica document advanced to a different source before constraint readback",
                    ticket: ticket, inspection: after };
            }
            if ticket.poll_count > 300 {
                let actual_generation = if after == () || after.generation == () {
                    "unavailable"
                } else {
                    after.generation.to_string()
                };
                return #{ ok: false, stage: "source_readback", retryable: true,
                    error: "Modelica source replacement exceeded its bounded readback budget (expected generation > "
                        + ticket.parent_generation.to_string() + ", actual generation "
                        + actual_generation + ")",
                    ticket: ticket, inspection: after };
            }
            return #{ ok: true, pending: true, complete: false,
                stage: "source_readback", ticket: ticket, inspection: after };
        }
    }
    let progress = modelica_editor::poll_experiment_ticket(ticket);
    if progress == () || progress.ok != true { return progress; }
    if progress.pending == true {
        return #{ ok: true, pending: true, complete: false,
            stage: progress.stage, ticket: progress.ticket,
            compile_state: progress.compile_state, run: progress.run };
    }
    let solution = coincident_translation_readback(generated, progress);
    if solution.ok != true { return solution; }
    #{ ok: true, pending: false, complete: true, stage: "complete",
       ticket: progress.ticket, run: progress.run,
       result: progress.result, solution: solution }
}

fn _vector_matches(value, expected, tolerance) {
    if value == () { return false; }
    if vec3_is_native(value) {
        return vlen(vsub(value, expected)) <= tolerance;
    }
    if !array_is(value) || value.len() != 3 { return false; }
    (value[0] - expected.x).abs() <= tolerance
        && (value[1] - expected.y).abs() <= tolerance
        && (value[2] - expected.z).abs() <= tolerance
}

/// Build a generation-bound typed Editor placement plan from the completed
/// solve. This translation-only policy requires an Xform target with identity
/// authored rotation and scale; the solver deliberately does not infer or
/// solve rotational placement. It sets the moving root origin from the
/// moving-local / target-parent-frame equation in CoincidentPointTranslation.
fn coincident_translation_placement_plan(doc_id, edit_target, path,
    parent_generation, solution) {
    if doc_id == () || doc_id == 0 || edit_target == () || edit_target == ""
        || path == () || !path.starts_with("/") || parent_generation == ()
        || !map_is(solution) || solution.ok != true
        || !vec3_is_native(solution.translation) || solution.units != "m" {
        return #{ ok: false, stage: "plan", error: "explicit document, edit target, path, generation, and metre-valued completed solution are required" };
    }
    let numerical_policy = numerical_settings::active();
    let scalar_tolerance = numerical_settings::require(
        numerical_policy, "scalar_abs");
    let angle_tolerance_rad = numerical_settings::require(
        numerical_policy, "angle_abs_rad");
    if scalar_tolerance == () || angle_tolerance_rad == () {
        return #{ ok: false, stage: "plan", error:
            "Twin settings numerics.comparison.scalar_abs and numerics.comparison.angle_abs_rad are required as finite, non-negative f64 values" };
    }
    let rotation_tolerance_deg = angle_tolerance_rad * 180.0 / PI();
    let document = assembly_edit::describe(doc_id);
    if document == () || document.ok == false || document.generation != parent_generation {
        return #{ ok: false, stage: "plan", error: "USD document generation changed before placement planning", inspection: document };
    }
    let modelica_source = query("GetDocumentSource", #{ doc_id: solution.modelica_doc_id });
    if modelica_source == () || modelica_source.ok == false
        || modelica_source.generation != solution.source_generation {
        return #{ ok: false, stage: "modelica_source",
            error: "Modelica source generation changed before placement planning",
            inspection: modelica_source };
    }
    let resolved = assembly_edit::resolve_target(doc_id, path, edit_target);
    if resolved == () || resolved.ok == false {
        return #{ ok: false, stage: "plan", error: "exact USD placement target did not resolve", target: resolved };
    }
    let prim = query("QueryUsdPrim", #{
        doc_id: doc_id,
        path: path,
        attrs: ["xformOp:rotateXYZ", "xformOp:scale"],
        children: false,
    });
    if prim == () || prim.ok == false || prim.type_name != "Xform" {
        return #{ ok: false, stage: "plan", error: "translation placement target must be an existing Xform", target: prim };
    }
    let rotation = if prim.attrs == () { () } else { prim.attrs["xformOp:rotateXYZ"] };
    if rotation != () && !_vector_matches(rotation, vec3(0.0, 0.0, 0.0), rotation_tolerance_deg) {
        return #{ ok: false, stage: "frame_contract", error: "coincident translation policy requires identity target rotation" };
    }
    let scale = if prim.attrs == () { () } else { prim.attrs["xformOp:scale"] };
    if scale != () && !_vector_matches(scale, vec3(1.0, 1.0, 1.0), scalar_tolerance) {
        return #{ ok: false, stage: "frame_contract", error: "coincident translation policy requires unit target scale" };
    }
    let plan = assembly_builder::place_plan(
        edit_target, path, solution.translation, (), ());
    if plan == () || plan.ok != true || plan.ops == () || plan.ops.is_empty() {
        return #{ ok: false, stage: "plan", error: "typed USD placement planner did not produce operations", plan: plan };
    }
    #{ ok: true, kind: "coincident_translation_placement",
       doc_id: doc_id, edit_target: edit_target, path: path,
       parent_generation: parent_generation, scope: #{ Assembly: () },
       source_revision_hex: solution.source_revision_hex,
       modelica_doc_id: solution.modelica_doc_id,
       source_generation: solution.source_generation,
       experiment_id: solution.experiment_id,
       constraint: solution.constraint, usage: solution.usage,
       translation: solution.translation, units: "m",
       frame_contract: "moving anchor local; target anchor in the USD parent frame; identity rotation and unit scale",
       ops: plan.ops }
}

/// Submit the typed plan to the visible USD Editor proposal/review owner and
/// read the accepted proposal back. This does not commit geometry.
fn propose_coincident_translation_plan(plan, label, activate_editor) {
    if !map_is(plan) || plan.ok != true || plan.ops == ()
        || label == () || !string_is(label) || label == "" {
        return #{ ok: false, stage: "proposal", error: "a valid typed placement plan and label are required" };
    }
    if activate_editor == true {
        let perspective = cmd("ActivatePerspective", #{ id: "editor" });
        if perspective == () || perspective.ok != true {
            return #{ ok: false, stage: "perspective", error: "Editor perspective could not be activated", command: perspective };
        }
    }
    let created = assembly_edit::propose(
        plan.doc_id, plan.scope, label, plan.ops, plan.parent_generation);
    if created == () || created.ok != true || created.data == ()
        || created.data.proposal == () {
        return #{ ok: false, stage: "proposal", error: "USD Editor did not return the created proposal identity", command: created };
    }
    let proposal_id = created.data.proposal;
    let review = assembly_edit::review_session(plan.doc_id);
    if review == () || review.ok == false || review.proposals == () {
        return #{ ok: false, stage: "review", error: "USD Editor review session did not expose the proposal", command: created, review: review };
    }
    let matching = ();
    for proposal in review.proposals {
        if proposal.id == proposal_id { matching = proposal; }
    }
    if matching == () || matching.state != "pending"
        || matching.ops == () || matching.ops.len() != plan.ops.len()
        || matching.affected_paths == () || matching.affected_paths.len() != 1
        || matching.affected_paths[0] != plan.path {
        return #{ ok: false, stage: "review", error: "review readback did not match the proposed typed placement", command: created, review: review };
    }
    #{ ok: true, stage: "reviewed", committed: false,
       plan: plan, proposal: matching, command: created, review: review }
}
