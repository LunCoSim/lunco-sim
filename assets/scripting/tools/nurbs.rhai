//! Standard USD NURBS control-net editing helpers.
//!
//! `UsdGeomNurbsCurves` and `UsdGeomNurbsPatch` use the standard `point3f[]`
//! `points` attribute. The tool edits that attribute through the generic USD
//! command, so the authored value is journaled, undoable, replicated, and
//! re-composed by the normal USD projection.

fn ui_label() { "NURBS points" }
fn ui_hint() { "Click a NURBS prim to select it; use set_point(path, index, p) from Rhai" }

fn on_click(id) {
    cmd("SelectEntity", #{ entity_id: id, extend: false, toggle: false });
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

/// Replace the standard USD control net at `path`.
fn set_points(path, points) {
    cmd("SetUsdAttribute", #{
        path: path,
        name: "points",
        type_name: "point3f[]",
        value: points_literal(points)
    });
}

/// Edit one control point without losing the other points.
/// Returns false when the prim or index is invalid.
fn set_point(path, index, point) {
    let prim = query("QueryUsdPrim", #{ path: path, attrs: ["points"] });
    if type_of(prim) == "()" || type_of(prim.attrs["points"]) == "()" {
        return false;
    }
    let points = prim.attrs["points"];
    if index < 0 || index >= points.len() {
        return false;
    }
    points[index] = point;
    set_points(path, points);
    return true;
}
