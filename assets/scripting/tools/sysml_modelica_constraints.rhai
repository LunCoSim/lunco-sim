// Rhai-owned policy bridge from source-backed SysML constraints to Modelica.
// Rust supplies the resolved typed graph. This library selects relevant
// definitions/usages and generates Modelica only at the Modelica source edge.

import "modelica_editor" as modelica_editor;
import "assembly_builder" as assembly_builder;
import "assembly_edit" as assembly_edit;

fn analyze(path) {
    query("AnalyzeSysml", #{path: path,
        tables: ["constraints", "references", "relationships"]})
}

/// Load only the geometry attributes that the current solve policy consumes.
fn analyze_selected(path, attribute_names) {
    query("AnalyzeSysml", #{path: path,
        tables: ["constraints", "references", "relationships", "attributes"],
        attribute_names: attribute_names})
}

fn _short_name(qualified_name) {
    let parts = qualified_name.split("::");
    parts[parts.len() - 1]
}

/// Select definitions, usages and relationship edges for the requested
/// semantic constraint identities. This selection is intentionally Rhai
/// policy, not part of the Rust source projection.
fn select(report, requested) {
    if type_of(report) != "map" || report.ok != true
        || report.analysis == () || report.analysis.constraints == () {
        return #{ok: false, error: "SysML analysis is unavailable"};
    }
    if type_of(requested) != "array" || requested.is_empty() {
        return #{ok: false, error: "at least one qualified constraint definition is required"};
    }

    for identity in requested {
        if type_of(identity) != "string" || !identity.contains("::") {
            return #{ok: false, error: "constraint identities must be qualified SysML names"};
        }
    }

    let definitions = [];
    let definition_names = #{};
    for record in report.analysis.constraints {
        if record.element.kind != "ConstraintDefinition" { continue; }
        let qualified_name = record.element.qualified_name;
        let matched = false;
        for identity in requested {
            if qualified_name == identity { matched = true; }
        }
        if !matched { continue; }
        if definition_names[qualified_name] == true { continue; }
        definitions.push(record);
        definition_names[qualified_name] = true;
    }
    if definitions.is_empty() {
        return #{ok: false, error: "no matching SysML constraint definition"};
    }

    let usages = [];
    let usage_names = #{};
    let related = [];
    let related_references = [];
    for reference in report.analysis.references {
        if definition_names[reference.target] != true { continue; }
        let usage_name = reference.from;
        for record in report.analysis.constraints {
            if record.element.kind == "ConstraintUsage"
                && record.element.qualified_name == usage_name
                && usage_names[usage_name] != true {
                usages.push(record);
                usage_names[usage_name] = true;
                related_references.push(reference);
            }
        }
    }
    for relationship in report.analysis.relationships {
        if usage_names[relationship.element.qualified_name] == true {
            related.push(relationship);
        }
    }

    let definition_edges = [];
    for relationship in report.analysis.relationships {
        let related_to_definition = false;
        let related_to_usage = usage_names[relationship.element.qualified_name] == true;
        for property in relationship.properties {
            for target in property.targets {
                if definition_names[target] == true { related_to_definition = true; }
            }
        }
        if related_to_definition && !related_to_usage { definition_edges.push(relationship); }
    }
    related += definition_edges;

    #{ok: true, definitions: definitions, usages: usages,
      relationships: related, references: related_references}
}

/// Read exactly three length quantities authored in metres, then lower them
/// to the shared f64 Bevy/Rhai Vec3. Unit conversion is not inferred here.
fn position_m(report, qualified_name) {
    if type_of(report) != "map" || report.analysis == ()
        || report.analysis.attributes == () {
        return #{ok: false, error: "SysML analysis has no attribute records"};
    }
    for attribute in report.analysis.attributes {
        if attribute.qualified_name != qualified_name { continue; }
        if attribute.declared_type == ()
            || attribute.declared_type.quantity_kind != "Length"
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
            if element.number_value == () || element.unit != "m" {
                return #{ok: false, error: "SysML position components must be finite metre quantities: " + qualified_name};
            }
            values.push(element.number_value);
        }
        return #{ok: true, value: vec3(values[0], values[1], values[2])};
    }
    #{ok: false, error: "SysML vector attribute is missing: " + qualified_name}
}

/// Resolve one constraint-definition input through a standard SysML binding
/// connector. The relationship's typed ends carry the definition feature and
/// the bound part feature; the reference graph ties the connector to the
/// particular constraint usage in whose context it is declared.
fn bound_vector(report, usage_name, input_name) {
    if type_of(report) != "map" || report.analysis == ()
        || report.analysis.relationships == () || report.analysis.references == () {
        return #{ok: false, error: "SysML analysis has no binding relationship facts"};
    }
    let definition = _definition_for_usage(report, usage_name);
    if definition == () {
        return #{ok: false, error: "constraint usage has no unique source definition: " + usage_name};
    }
    let formal_name = definition.element.qualified_name + "::" + input_name;
    let candidates = [];
    for relationship in report.analysis.relationships {
        if relationship.element.kind != "BindingConnectorAsUsage" { continue; }
        let belongs_to_usage = relationship.element.qualified_name
            .starts_with(usage_name + "::");
        let binds_formal = false;
        for reference in report.analysis.references {
            if reference.from != relationship.element.qualified_name { continue; }
            if reference.target == usage_name { belongs_to_usage = true; }
            if reference.target == formal_name { binds_formal = true; }
        }
        if !belongs_to_usage || !binds_formal { continue; }

        let related_features = ();
        for property in relationship.properties {
            if property.name == "relatedFeature" { related_features = property.targets; }
        }
        if type_of(related_features) != "array" || related_features.len() != 2 {
            return #{ok: false, error: "binding connector must resolve to exactly two feature ends: "
                + relationship.element.qualified_name};
        }
        let bound_feature = "";
        for target in related_features {
            if target != formal_name { bound_feature = target; }
        }
        if bound_feature == "" {
            return #{ok: false, error: "binding connector does not bind a distinct part feature: "
                + relationship.element.qualified_name};
        }
        candidates.push(#{binding: relationship.element.qualified_name, feature: bound_feature});
    }
    if candidates.len() != 1 {
        return #{ok: false, error: "expected one binding from " + usage_name + "." + input_name
            + ", found " + candidates.len()};
    }
    let resolved = position_m(report, candidates[0].feature);
    if resolved.ok != true { return resolved; }
    #{ok: true, binding: candidates[0].binding,
      feature: candidates[0].feature, value: resolved.value}
}

fn _definition_for_usage(report, usage_name) {
    let definition_name = "";
    for reference in report.analysis.references {
        if reference.from != usage_name { continue; }
        for constraint in report.analysis.constraints {
            if constraint.element.kind != "ConstraintDefinition"
                || constraint.element.qualified_name != reference.target { continue; }
            if definition_name != "" && definition_name != reference.target { return (); }
            definition_name = reference.target;
        }
    }
    if definition_name == "" { return (); }
    for constraint in report.analysis.constraints {
        if constraint.element.kind == "ConstraintDefinition"
            && constraint.element.qualified_name == definition_name {
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
    if type_of(report) != "map" || report.analysis == () {
        return #{ok: false, error: "SysML analysis is unavailable"};
    }
    let definition = _definition_for_usage(report, usage_name);
    if definition == () || _short_name(definition.element.qualified_name) != "SegmentMember" {
        return #{ok: false, error: "constraint usage is not linked to the SegmentMember geometry policy: " + usage_name};
    }
    let selection = select(report, [definition.element.qualified_name]);
    if selection.ok != true { return selection; }
    let start = bound_vector(report, usage_name, "start");
    if start.ok != true { return start; }
    let finish = bound_vector(report, usage_name, "end");
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
      start_binding: start.binding, end_binding: finish.binding,
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

/// Assemble a one-shot translation solve for a CoincidentPointTranslation
/// usage. The moving and fixed point inputs are resolved from its standard
/// SysML binding connectors rather than passed as duplicate caller data.
fn assemble_coincident_translation(report, usage_name) {
    if type_of(report) != "map" || report.analysis == () {
        return #{ok: false, error: "SysML analysis is unavailable"};
    }
    let definition = _definition_for_usage(report, usage_name);
    if definition == ()
        || _short_name(definition.element.qualified_name) != "CoincidentPointTranslation" {
        return #{ok: false, error: "constraint usage is not linked to the CoincidentPointTranslation geometry policy: " + usage_name};
    }
    let selection = select(report, [definition.element.qualified_name]);
    if selection.ok != true { return selection; }
    let moving = bound_vector(report, usage_name, "movingPoint");
    if moving.ok != true { return moving; }
    let fixed = bound_vector(report, usage_name, "fixedPoint");
    if fixed.ok != true { return fixed; }

    let source = "model SysmlCoincidentPointTranslationSolve\n"
        + "  LunCo.Geometry.CoincidentPointTranslation3D relation(\n"
        + "    moving_point=" + _modelica_vec3(moving.value) + ",\n"
        + "    fixed_point=" + _modelica_vec3(fixed.value) + ");\n"
        + "  output Real translation[3](each unit=\"m\");\n"
        + "equation\n"
        + "  translation = relation.translation;\n"
        + "end SysmlCoincidentPointTranslationSolve;\n";
    #{ok: true, class: "SysmlCoincidentPointTranslationSolve", source: source,
      source_revision_hex: report.analysis.source_revision_hex,
      constraint: definition.element.qualified_name,
      usage: usage_name,
      modelica_class: "LunCo.Geometry.CoincidentPointTranslation3D",
      moving_feature: moving.feature, fixed_feature: fixed.feature,
      moving_binding: moving.binding, fixed_binding: fixed.binding,
      moving_point: moving.value, fixed_point: fixed.value}
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
    let after = query("GetDocumentSource", #{ doc_id: doc_id });
    if after == () || after.ok == false || after.source != generated.source
        || after.generation == () || after.generation <= parent_generation {
        return #{ ok: false, stage: "source_readback", retryable: false,
            error: "Modelica Editor source readback did not match the generated wrapper",
            generated: generated, edit: edit, inspection: after };
    }
    // `label` is for the Experiments UI only; execution identity comes from
    // the deferred RunExperiment acknowledgement, never from name matching.
    let label = "SysML geometry solve: " + generated.source_revision_hex
        + " / " + usage_name;
    let ticket = modelica_editor::experiment_ticket(
        doc_id, generated.class, after.generation, label,
        ["translation[1]", "translation[2]", "translation[3]"],
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
    if type_of(generated) != "map" || generated.ok != true
        || type_of(completed) != "map" || completed.ok != true
        || completed.complete != true || completed.ticket == ()
        || completed.result == () {
        return #{ ok: false, stage: "readback", error: "a completed generated Modelica experiment is required" };
    }
    let result = completed.result;
    let source = query("GetDocumentSource", #{ doc_id: completed.ticket.doc_id });
    if source == () || source.ok != true
        || source.generation != completed.ticket.source_generation {
        return #{ ok: false, stage: "readback",
            error: "Modelica source generation changed before placement readback" };
    }
    if result.state != "done" || result.times == () || result.times.is_empty()
        || result.series == () {
        return #{ ok: false, stage: "readback", error: "completed experiment has no numeric trajectory" };
    }
    let components = [];
    for axis in 1..4 {
        let name = "translation[" + axis.to_string() + "]";
        let samples = result.series[name];
        if type_of(samples) != "array" || samples.is_empty() {
            return #{ ok: false, stage: "readback", error: "missing Modelica translation output " + name };
        }
        let value = samples[samples.len() - 1];
        if type_of(value) != "f64" || !value.is_finite() {
            return #{ ok: false, stage: "readback", error: "translation output is not finite f64 data: " + name };
        }
        components.push(value);
    }
    let translation = vec3(components[0], components[1], components[2]);
    let residual = vsub(vadd(generated.moving_point, translation), generated.fixed_point);
    if vlen(residual) > 1.0e-9 {
        return #{ ok: false, stage: "equation_check",
            error: "Modelica translation does not close the source-bound coincidence equation",
            translation: translation, residual: residual };
    }
    #{ ok: true, kind: "coincident_translation_solution",
       translation: translation, units: "m", residual: residual,
       moving_point: generated.moving_point, fixed_point: generated.fixed_point,
       constraint: generated.constraint, usage: generated.usage,
       source_revision_hex: generated.source_revision_hex,
       modelica_doc_id: completed.ticket.doc_id,
       source_generation: completed.ticket.source_generation,
       modelica_class: generated.class, experiment_id: completed.ticket.experiment_id,
       sample_time: result.times[result.times.len() - 1], result: result }
}

/// Advance one solve ticket. A pending result is an ordinary state, not a
/// failed compile and not a reason to block the simulation/editor thread.
fn poll_coincident_translation(ticket, generated) {
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

fn _vector_matches(value, expected) {
    if value == () { return false; }
    if type_of(value) == "Vec3" {
        return vlen(vsub(value, expected)) <= 1.0e-9;
    }
    if type_of(value) != "array" || value.len() != 3 { return false; }
    (value[0] - expected.x).abs() <= 1.0e-9
        && (value[1] - expected.y).abs() <= 1.0e-9
        && (value[2] - expected.z).abs() <= 1.0e-9
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
        || type_of(solution) != "map" || solution.ok != true
        || type_of(solution.translation) != "Vec3" || solution.units != "m" {
        return #{ ok: false, stage: "plan", error: "explicit document, edit target, path, generation, and metre-valued completed solution are required" };
    }
    let document = assembly_edit::describe(doc_id);
    if document == () || document.ok == false || document.generation != parent_generation {
        return #{ ok: false, stage: "plan", error: "USD document generation changed before placement planning", inspection: document };
    }
    let modelica_source = query("GetDocumentSource", #{ doc_id: solution.modelica_doc_id });
    if modelica_source == () || modelica_source.ok != true
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
    if rotation != () && !_vector_matches(rotation, vec3(0.0, 0.0, 0.0)) {
        return #{ ok: false, stage: "frame_contract", error: "coincident translation policy requires identity target rotation" };
    }
    let scale = if prim.attrs == () { () } else { prim.attrs["xformOp:scale"] };
    if scale != () && !_vector_matches(scale, vec3(1.0, 1.0, 1.0)) {
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
    if type_of(plan) != "map" || plan.ok != true || plan.ops == ()
        || label == () || type_of(label) != "string" || label == "" {
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
