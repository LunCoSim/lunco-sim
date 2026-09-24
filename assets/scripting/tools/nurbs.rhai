//! Standard USD NURBS control-net editing helpers.
//!
//! `UsdGeomNurbsCurves` and `UsdGeomNurbsPatch` use the standard `point3f[]`
//! `points` attribute. The tool edits that attribute through the generic USD
//! command, so the authored value is journaled, undoable, replicated, and
//! re-composed by the normal USD projection.

fn ui_label() { "NURBS points" }
fn ui_hint() { "Edit NURBS points with set_point; create a reviewed collision proxy with propose_collision_proxy(doc, source, name, approximation, layer, scope)" }

fn on_click(context) {
    if context == () || context.button != "primary" || context.target_entity_id == () {
        return;
    }
    cmd("SelectEntity", #{
        entity_id: context.target_entity_id,
        inspector_part_entity_id: 0,
        extend: false,
        toggle: false,
        remove_only: false,
    });
}

fn point_literal(point) {
    return "(" + point[0].to_string() + ", "
        + point[1].to_string() + ", " + point[2].to_string() + ")";
}

fn points_literal(points) {
    let text = "[";
    for i in 0..points.len() {
        if i > 0 { text += ", "; }
        text += point_literal(points[i]);
    }
    return text + "]";
}

/// Replace the standard USD control net at `path` in the explicit document.
fn set_points(doc, path, points, parent_gen) {
    return cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ SetAttribute: #{
            edit_target: "@runtime@",
            path: path,
            name: "points",
            type_name: "point3f[]",
            value: points_literal(points),
        }},
    });
}

fn collision_point_literal(point) {
    return "(" + point[0].to_string() + ", "
        + point[1].to_string() + ", " + point[2].to_string() + ")";
}

fn collision_array_literal(values) {
    let text = "[";
    for i in 0..values.len() {
        if i > 0 { text += ", "; }
        text += values[i].to_string();
    }
    return text + "]";
}

fn collision_points_literal(points) {
    let text = "[";
    for i in 0..points.len() {
        if i > 0 { text += ", "; }
        text += collision_point_literal(points[i]);
    }
    return text + "]";
}

fn collision_attribute_op(edit_target, path, name, type_name, value) {
    #{ SetAttribute: #{
        edit_target: edit_target,
        path: path,
        name: name,
        type_name: type_name,
        value: value,
    }}
}

/// Read a composed NURBS patch and return a deterministic collision-mesh cook
/// plan using the source-based physical tessellation settings. Supported Avian approximation tokens are none, convexHull,
/// convexDecomposition, and boundingCube.
fn plan_collision_proxy(doc, source_path, proxy_name, approximation) {
    query("PlanNurbsCollisionProxy", #{
        doc_id: doc,
        path: source_path,
        proxy_name: proxy_name,
        approximation: approximation,
    })
}

/// Plan an explicitly resolved NURBS collision cook. All four tessellation
/// values are recorded on the resulting proxy and checked at physics admission.
fn plan_collision_proxy_with_tessellation(doc, source_path, proxy_name, approximation,
    u_subdivisions, v_subdivisions, trim_curve_samples, trim_grid_subdivisions) {
    query("PlanNurbsCollisionProxy", #{
        doc_id: doc,
        path: source_path,
        proxy_name: proxy_name,
        approximation: approximation,
        u_subdivisions: u_subdivisions,
        v_subdivisions: v_subdivisions,
        trim_curve_samples: trim_curve_samples,
        trim_grid_subdivisions: trim_grid_subdivisions,
    })
}

/// Turn a cooked NURBS proxy plan into ordinary typed USD edits. The proxy is
/// invisible to rendering, has standard USD Physics collision schemas, and
/// records a typed source relationship plus its reproducible cook settings.
fn collision_proxy_ops(plan, edit_target) {
    if plan == () { return []; }
    let path = plan.proxy_path;
    let ops = [];
    if plan.create_prim {
        ops.push(#{ AddPrim: #{
            edit_target: edit_target,
            parent_path: plan.source_path,
            name: plan.proxy_name,
            type_name: "Mesh",
            reference: (),
            reference_prim_path: "",
        }});
    }

    ops.push(collision_attribute_op(edit_target, path, "points", "point3f[]",
        collision_points_literal(plan.points)));
    ops.push(collision_attribute_op(edit_target, path, "faceVertexCounts", "int[]",
        collision_array_literal(plan.face_vertex_counts)));
    ops.push(collision_attribute_op(edit_target, path, "faceVertexIndices", "int[]",
        collision_array_literal(plan.face_vertex_indices)));
    ops.push(collision_attribute_op(edit_target, path, "subdivisionScheme", "token", "\"none\""));
    ops.push(collision_attribute_op(edit_target, path, "orientation", "token", "\"rightHanded\""));
    ops.push(collision_attribute_op(edit_target, path, "purpose", "token", "\"proxy\""));
    ops.push(collision_attribute_op(edit_target, path, "visibility", "token", "\"invisible\""));
    ops.push(collision_attribute_op(edit_target, path, "physics:collisionEnabled", "bool", "true"));
    ops.push(collision_attribute_op(edit_target, path, "physics:approximation", "token",
        "\"" + plan.approximation + "\""));
    ops.push(collision_attribute_op(edit_target, path, "lunco:derived:geometryFingerprint", "uint64",
        plan.geometry_fingerprint.to_string()));
    ops.push(collision_attribute_op(edit_target, path, "lunco:derived:nurbsUSubdivisions", "int",
        plan.u_subdivisions.to_string()));
    ops.push(collision_attribute_op(edit_target, path, "lunco:derived:nurbsVSubdivisions", "int",
        plan.v_subdivisions.to_string()));
    ops.push(collision_attribute_op(edit_target, path, "lunco:derived:trimCurveSamples", "int",
        plan.trim_curve_samples.to_string()));
    ops.push(collision_attribute_op(edit_target, path, "lunco:derived:trimGridSubdivisions", "int",
        plan.trim_grid_subdivisions.to_string()));
    ops.push(#{ SetRelationship: #{
        edit_target: edit_target,
        path: path,
        name: "lunco:derived:source",
        targets: [plan.source_path],
    }});
    ops.push(#{ SetApiSchemas: #{
        edit_target: edit_target,
        path: path,
        schemas: plan.schemas,
    }});
    return ops;
}

/// Create a normal Editor proposal to author or refresh a derived collision
/// proxy. Review and commit the returned proposal through assembly_edit.
fn propose_collision_proxy(doc, source_path, proxy_name, approximation, edit_target, scope) {
    let plan = plan_collision_proxy(doc, source_path, proxy_name, approximation);
    if plan == () { return plan; }
    return _propose_collision_plan(doc, plan, edit_target, scope);
}

/// Propose a collision proxy with explicit, reproducible subdivisions.
fn propose_collision_proxy_with_tessellation(doc, source_path, proxy_name, approximation,
    u_subdivisions, v_subdivisions, trim_curve_samples, trim_grid_subdivisions,
    edit_target, scope) {
    let plan = plan_collision_proxy_with_tessellation(doc, source_path, proxy_name,
        approximation, u_subdivisions, v_subdivisions, trim_curve_samples,
        trim_grid_subdivisions);
    if plan == () { return plan; }
    return _propose_collision_plan(doc, plan, edit_target, scope);
}

fn _propose_collision_plan(doc, plan, edit_target, scope) {
    let ops = collision_proxy_ops(plan, edit_target);
    return assembly_edit::propose(doc, scope,
        "Derive NURBS collision proxy " + plan.proxy_path,
        ops, plan.generation);
}

/// Edit one control point without losing the other points.
/// Returns false when the prim or index is invalid.
fn set_point(doc, path, index, point) {
    let prim = query("QueryUsdPrim", #{ doc_id: doc, path: path, attrs: ["points"] });
    if type_of(prim) == "()" || type_of(prim.attrs["points"]) == "()" {
        return false;
    }
    let points = prim.attrs["points"];
    if index < 0 || index >= points.len() {
        return false;
    }
    points[index] = point;
    set_points(doc, path, points, prim.generation);
    return true;
}
