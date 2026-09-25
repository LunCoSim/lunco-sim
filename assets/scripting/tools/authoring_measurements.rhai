// Generic authored measurement policy.
//
// USD owns geometry, transforms, collision representation, units and
// provenance. This library only consumes the typed QueryUsdPrim surface and
// evaluates caller-supplied requirements. It is deliberately independent of
// vehicles, routes, Twin names, and a custom measurement schema.

import "engineering_units" as engineering_units;
import "mechanical_relations" as mechanical_relations;

fn query_ok(record) {
    record != () && record.ok != false
}

fn unit_scale(units) {
    engineering_units::linear_scale(units)
}

fn unavailable(subject, kind, message, source) {
    #{
        ok: false,
        status: "unavailable",
        subject: subject,
        kind: kind,
        message: message,
        source: source,
    }
}

fn distance3(a, b) {
    if a == () || b == () { return (); }
    if !vec3_is_finite(a) || !vec3_is_finite(b) { return (); }
    vlen(vsub(vec3_from(a), vec3_from(b)))
}

fn verdict(actual, expected, tolerance) {
    let relation = mechanical_relations::equal(actual, expected, tolerance);
    if relation == () { return "unavailable"; }
    relation.status
}

fn distance_requirement(doc, spec) {
    let from = spec.from;
    let to = spec.to;
    let units = if spec.units != () { spec.units } else { "m" };
    let scale = unit_scale(units);
    let subject = from + " ↔ " + to;
    if scale == () {
        return unavailable(subject, "distance", "unsupported units: " + units, "Rhai");
    }
    // Read both endpoints from one composed-stage snapshot. This avoids
    // reopening the stage and rechecking the document generation for every
    // endpoint when a component has many dimensional rules.
    let request = #{paths: [from, to]};
    if doc != () { request.doc_id = doc; }
    let records = query("QueryUsdPrims", request);
    if !query_ok(records) || records.records == () || records.records.len() != 2 {
        return unavailable(subject, "distance", "one or both USD subjects are unavailable", "QueryUsdPrims");
    }
    let a = records.records[0];
    let b = records.records[1];
    let meters = distance3(a.world_position, b.world_position);
    let actual = if meters != () { meters / scale } else { () };
    let expected = if spec.expected != () { spec.expected } else { () };
    let tolerance = if spec.tolerance != () { spec.tolerance } else { () };
    let status = verdict(actual, expected, tolerance);
    #{
        ok: status == "passed",
        status: status,
        kind: "distance",
        subject: subject,
        from: from,
        to: to,
        frame: "canonical_stage",
        units: units,
        actual: actual,
        expected: expected,
        tolerance: tolerance,
        method: "QueryUsdPrims.world_position",
        source: "USD",
    }
}

fn _axis_value(value, axis) {
    if !vec3_is_finite(value) { return (); }
    if axis == "x" { return vcomponent(value, 0); }
    if axis == "y" { return vcomponent(value, 1); }
    if axis == "z" { return vcomponent(value, 2); }
    ()
}

/// Check a point against a plane using only native typed vectors. The plane
/// normal and expected distance are authored by the caller; Rust does not
/// choose an axis or a tolerance.
fn plane_distance_requirement(doc, spec) {
    let point_path = spec.point;
    let plane_path = spec.plane;
    let units = if spec.units != () { spec.units } else { "m" };
    let scale = unit_scale(units);
    let subject = point_path + " ↔ plane " + plane_path;
    if scale == () {
        return unavailable(subject, "plane_distance", "unsupported units: " + units, "Rhai");
    }
    let request = #{paths: [point_path, plane_path]};
    if doc != () { request.doc_id = doc; }
    let records = query("QueryUsdPrims", request);
    if !query_ok(records) || records.records == () || records.records.len() != 2 {
        return unavailable(subject, "plane_distance", "point or plane subject is unavailable", "QueryUsdPrims");
    }
    let normal = if spec.normal != () { vec3_from(spec.normal) } else { vec3(0.0, 1.0, 0.0) };
    if !vec3_is_valid(normal) {
        return unavailable(subject, "plane_distance", "plane normal is invalid", "Rhai");
    }
    let point = records.records[0].world_position;
    let plane = records.records[1].world_position;
    if !vec3_is_finite(point) || !vec3_is_finite(plane) {
        return unavailable(subject, "plane_distance", "point or plane position is invalid", "QueryUsdPrims");
    }
    let actual = mechanical_relations::plane_signed_distance(point, plane, normal) / scale;
    let expected = if spec.expected != () { spec.expected } else { 0.0 };
    let tolerance = if spec.tolerance != () { spec.tolerance } else { 0.0 };
    let status = verdict(actual, expected, tolerance);
    #{ok: status == "passed", status: status, kind: "plane_distance",
      subject: subject, point: point_path, plane: plane_path, frame: "canonical_stage",
      units: units, actual: actual, expected: expected, tolerance: tolerance,
      normal: normal, method: "QueryUsdPrims.world_position", source: "USD+Rhai"}
}

/// Check that one authored subject is below another along an explicitly
/// selected axis. This is a reusable relation policy, not a Griffin rule.
fn under_requirement(doc, spec) {
    let child_path = spec.child;
    let parent_path = spec.parent;
    let axis = if spec.axis != () { spec.axis } else { "y" };
    let units = if spec.units != () { spec.units } else { "m" };
    let scale = unit_scale(units);
    let subject = child_path + " under " + parent_path + " along " + axis;
    if scale == () {
        return unavailable(subject, "under", "unsupported units: " + units, "Rhai");
    }
    let request = #{paths: [child_path, parent_path]};
    if doc != () { request.doc_id = doc; }
    let records = query("QueryUsdPrims", request);
    if !query_ok(records) || records.records == () || records.records.len() != 2 {
        return unavailable(subject, "under", "child or parent subject is unavailable", "QueryUsdPrims");
    }
    let child = _axis_value(records.records[0].world_position, axis);
    let parent = _axis_value(records.records[1].world_position, axis);
    if child == () || parent == () {
        return unavailable(subject, "under", "axis is invalid or position is unavailable", "QueryUsdPrims");
    }
    let minimum = if spec.minimum != () { spec.minimum } else { 0.0 };
    let tolerance = if spec.tolerance != () { spec.tolerance } else { 0.0 };
    let actual = mechanical_relations::axis_delta(
        records.records[1].world_position, records.records[0].world_position, axis) / scale;
    let relation = mechanical_relations::at_least(actual, minimum, tolerance);
    let status = if relation == () { "unavailable" } else { relation.status };
    #{ok: status == "passed", status: status, kind: "under", subject: subject,
      child: child_path, parent: parent_path, axis: axis, frame: "canonical_stage",
      units: units, actual: actual, minimum: minimum, tolerance: tolerance,
      method: "QueryUsdPrims.world_position", source: "USD+Rhai"}
}

fn bounds_requirement(doc, spec) {
    let path = spec.path;
    let units = if spec.units != () { spec.units } else { "m" };
    let scale = unit_scale(units);
    if scale == () {
        return unavailable(path, "extent", "unsupported units: " + units, "Rhai");
    }
    let request = #{path: path, collision_bounds: true};
    if doc != () { request.doc_id = doc; }
    let record = query("QueryUsdPrim", request);
    if !query_ok(record) || record.collision_bounds == () {
        return unavailable(path, "extent", "collision representation has no composed bounds", "QueryUsdPrim");
    }
    let bounds = record.collision_bounds;
    let extents = bounds.half_extents;
    let axis = if spec.axis != () { spec.axis } else { 0 };
    if extents == () || axis < 0 || axis > 2 {
        return unavailable(path, "extent", "collision bounds have no requested axis", "QueryUsdPrim");
    }
    let actual = extents[axis] * 2.0 / scale;
    let expected = if spec.expected != () { spec.expected } else { () };
    let tolerance = if spec.tolerance != () { spec.tolerance } else { () };
    let status = verdict(actual, expected, tolerance);
    #{
        ok: status == "passed",
        status: status,
        kind: "extent",
        subject: path,
        axis: axis,
        representation: "collision",
        frame: bounds.frame,
        units: units,
        actual: actual,
        expected: expected,
        tolerance: tolerance,
        bounds: bounds,
        method: "QueryUsdPrim.collision_bounds",
        source: "USD",
    }
}

fn _point2_less(a, b) {
    a[0] < b[0] || (a[0] == b[0] && a[1] < b[1])
}

fn _cross2(origin, a, b) {
    (a[0] - origin[0]) * (b[1] - origin[1])
        - (a[1] - origin[1]) * (b[0] - origin[0])
}

fn _convex_footprint(vertices, travel_axis, lateral_axis, travel_sign) {
    if vertices == () || vertices.len() < 3 { return (); }
    let points = [];
    for vertex in vertices {
        if type_of(vertex) != "array" || vertex.len() != 3
            || !vec3_is_finite(vertex) {
            return ();
        }
        points.push([vertex[travel_axis] * travel_sign, vertex[lateral_axis]]);
    }
    for left in 0..points.len() {
        for right in left + 1..points.len() {
            if _point2_less(points[right], points[left]) {
                let swap = points[left];
                points[left] = points[right];
                points[right] = swap;
            }
        }
    }
    let lower = [];
    for point in points {
        while lower.len() >= 2
            && _cross2(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 1.0e-12 {
            lower.pop();
        }
        lower.push(point);
    }
    let upper = [];
    for offset in 0..points.len() {
        let point = points[points.len() - offset - 1];
        while upper.len() >= 2
            && _cross2(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 1.0e-12 {
            upper.pop();
        }
        upper.push(point);
    }
    if lower.len() < 2 || upper.len() < 2 { return (); }
    lower.pop();
    upper.pop();
    for point in upper { lower.push(point); }
    if lower.len() < 3 { return (); }
    lower
}

fn _footprint_lateral_interval(polygon) {
    if polygon == () || polygon.len() < 3 { return (); }
    let low = polygon[0][1];
    let high = polygon[0][1];
    for point in polygon {
        if point[1] < low { low = point[1]; }
        if point[1] > high { high = point[1]; }
    }
    [low, high]
}

fn _footprint_travel_interval(polygon) {
    if polygon == () || polygon.len() < 3 { return (); }
    let low = polygon[0][0];
    let high = polygon[0][0];
    for point in polygon {
        if point[0] < low { low = point[0]; }
        if point[0] > high { high = point[0]; }
    }
    [low, high]
}

fn _footprint_travel_section(polygon, lateral) {
    if polygon == () || polygon.len() < 3 { return (); }
    let low = 1.0e30;
    let high = -1.0e30;
    for index in 0..polygon.len() {
        let a = polygon[index];
        let b = polygon[(index + 1) % polygon.len()];
        let dz = b[1] - a[1];
        if dz.abs() <= 1.0e-12 {
            if (lateral - a[1]).abs() <= 1.0e-9 {
                if a[0] < low { low = a[0]; }
                if a[0] > high { high = a[0]; }
                if b[0] < low { low = b[0]; }
                if b[0] > high { high = b[0]; }
            }
        } else if lateral >= a[1].min(b[1]) - 1.0e-9
            && lateral <= a[1].max(b[1]) + 1.0e-9 {
            let amount = (lateral - a[1]) / dz;
            if amount >= -1.0e-9 && amount <= 1.0 + 1.0e-9 {
                let travel = a[0] + amount * (b[0] - a[0]);
                if travel < low { low = travel; }
                if travel > high { high = travel; }
            }
        }
    }
    if low > high { () } else { [low, high] }
}

fn _section_candidates(first, second, low, high) {
    let candidates = [low, high];
    for polygon in [first, second] {
        for point in polygon {
            if point[1] >= low - 1.0e-9 && point[1] <= high + 1.0e-9 {
                let found = false;
                for candidate in candidates {
                    if (candidate - point[1]).abs() <= 1.0e-9 { found = true; }
                }
                if !found { candidates.push(point[1]); }
            }
        }
    }
    candidates
}

fn _minimum_cross_section_overlap(a, b, lateral_low, lateral_high) {
    let candidates = _section_candidates(a, b, lateral_low, lateral_high);
    let minimum = 1.0e30;
    for lateral in candidates {
        if lateral < lateral_low - 1.0e-9 || lateral > lateral_high + 1.0e-9 { continue; }
        let section_a = _footprint_travel_section(a, lateral);
        let section_b = _footprint_travel_section(b, lateral);
        if section_a == () || section_b == () { return (); }
        let overlap = section_a[1].min(section_b[1])
            - section_a[0].max(section_b[0]);
        if overlap < minimum { minimum = overlap; }
    }
    if minimum == 1.0e30 { () } else { minimum }
}

fn _maximum_side_gap(inner_shape, outer_shape, lateral_low, lateral_high) {
    let candidates = _section_candidates(inner_shape, outer_shape,
        lateral_low, lateral_high);
    let maximum_abs = -1.0;
    let signed_at_maximum = 0.0;
    for lateral in candidates {
        if lateral < lateral_low - 1.0e-9 || lateral > lateral_high + 1.0e-9 { continue; }
        let inner_section = _footprint_travel_section(inner_shape, lateral);
        let outer_section = _footprint_travel_section(outer_shape, lateral);
        if inner_section == () || outer_section == () { return (); }
        let gap = outer_section[0] - inner_section[1];
        if gap.abs() > maximum_abs {
            maximum_abs = gap.abs();
            signed_at_maximum = gap;
        }
    }
    if maximum_abs < 0.0 { () } else { signed_at_maximum }
}

/// Measure a deck/transition/ramp interface from cooked collider parts in one
/// QueryUsdPrims snapshot. This relation needs one convex part per input; a
/// multi-part decomposition has no single convex footprint.
fn interface_geometry_observation(doc, spec) {
    if spec == () {
        return unavailable("<incomplete interface>", "interface_geometry",
            "measurement specification is required", "Rhai");
    }
    let deck_path = spec.deck_path;
    let transition_path = spec.transition_path;
    let ramp_path = spec.ramp_path;
    if type_of(deck_path) != "string" || type_of(transition_path) != "string"
        || type_of(ramp_path) != "string" || deck_path == ""
        || transition_path == "" || ramp_path == "" {
        return unavailable("<incomplete interface>", "interface_geometry",
            "three non-empty USD prim paths are required", "Rhai");
    }
    let subject = deck_path + " ↔ " + transition_path + " ↔ " + ramp_path;
    let side = spec.side;
    if side != "port" && side != "starboard" {
        return unavailable(subject, "interface_geometry",
            "side must be port or starboard to select the hinge edge", "Rhai");
    }
    let travel_axis = if spec.travel_axis != () { spec.travel_axis } else { 0 };
    let face_axis = if spec.face_axis != () { spec.face_axis } else { 1 };
    let lateral_axis = if spec.lateral_axis != () { spec.lateral_axis } else { 2 };
    if type_of(travel_axis) != "i64" || type_of(face_axis) != "i64"
        || type_of(lateral_axis) != "i64" || travel_axis < 0 || travel_axis > 2
        || face_axis < 0 || face_axis > 2 || lateral_axis < 0 || lateral_axis > 2
        || travel_axis == face_axis
        || travel_axis == lateral_axis || face_axis == lateral_axis {
        return unavailable(subject, "interface_geometry",
            "travel, face, and lateral axes must be distinct indices from 0 to 2", "Rhai");
    }
    let request = #{paths: [deck_path, transition_path, ramp_path],
        collision_geometry: true};
    if doc != () { request.doc_id = doc; }
    let records = query("QueryUsdPrims", request);
    if !query_ok(records) || records.records == () || records.records.len() != 3 {
        return unavailable(subject, "interface_geometry",
            "deck, transition, or ramp exact collision geometry is unavailable", "QueryUsdPrims");
    }
    let deck_record = records.records[0];
    let transition_record = records.records[1];
    let ramp_record = records.records[2];
    let deck_geometry = deck_record.collision_geometry;
    let transition_geometry = transition_record.collision_geometry;
    let ramp_geometry = ramp_record.collision_geometry;
    if deck_geometry == () || transition_geometry == () || ramp_geometry == ()
        || deck_geometry.frame != "canonical_stage"
        || transition_geometry.frame != "canonical_stage"
        || ramp_geometry.frame != "canonical_stage" {
        return unavailable(subject, "interface_geometry",
            "all three exact collider point sets must be in canonical stage coordinates", "QueryUsdPrims");
    }
    for geometry in [deck_geometry, transition_geometry, ramp_geometry] {
        if geometry.shape != "convex_hull" || geometry.vertices == () {
            return unavailable(subject, "interface_geometry",
                "interface measurement requires exactly one cooked convex collider part per prim", "QueryUsdPrims");
        }
    }
    let travel_sign = if side == "port" { 1.0 } else { -1.0 };
    let deck = _convex_footprint(deck_geometry.vertices, travel_axis,
        lateral_axis, travel_sign);
    let transition = _convex_footprint(transition_geometry.vertices,
        travel_axis, lateral_axis, travel_sign);
    let ramp = _convex_footprint(ramp_geometry.vertices, travel_axis,
        lateral_axis, travel_sign);
    if deck == () || transition == () || ramp == () {
        return unavailable(subject, "interface_geometry",
            "collider vertices do not form finite convex footprints", "Rhai");
    }
    let deck_lateral = _footprint_lateral_interval(deck);
    let transition_lateral = _footprint_lateral_interval(transition);
    let ramp_lateral = _footprint_lateral_interval(ramp);
    let transition_travel = _footprint_travel_interval(transition);
    if deck_lateral == () || transition_lateral == () || ramp_lateral == ()
        || transition_travel == () {
        return unavailable(subject, "interface_geometry",
            "collider footprint extents could not be derived", "Rhai");
    }
    let overlap_lateral_low = deck_lateral[0].max(transition_lateral[0]);
    let overlap_lateral_high = deck_lateral[1].min(transition_lateral[1]);
    let seam_lateral_low = ramp_lateral[0].max(transition_lateral[0]);
    let seam_lateral_high = ramp_lateral[1].min(transition_lateral[1]);
    if overlap_lateral_high < overlap_lateral_low
        || seam_lateral_high < seam_lateral_low {
        return unavailable(subject, "interface_geometry",
            "deck-transition or transition-ramp footprints have no lateral intersection", "Rhai");
    }
    let overlap = _minimum_cross_section_overlap(deck, transition,
        overlap_lateral_low, overlap_lateral_high);
    let signed_hinge_gap = _maximum_side_gap(transition, ramp,
        seam_lateral_low, seam_lateral_high);
    if overlap == () || signed_hinge_gap == () {
        return unavailable(subject, "interface_geometry",
            "cross-section overlap or hinge seam could not be measured", "Rhai");
    }
    let lateral_coverage = overlap_lateral_high - overlap_lateral_low;
    let deck_lateral_span = deck_lateral[1] - deck_lateral[0];
    let deck_top = -1.0e30;
    let transition_top = -1.0e30;
    for vertex in deck_geometry.vertices {
        if vertex[face_axis] > deck_top { deck_top = vertex[face_axis]; }
    }
    for vertex in transition_geometry.vertices {
        if vertex[face_axis] > transition_top { transition_top = vertex[face_axis]; }
    }
    #{
        ok: true,
        status: "observed",
        kind: "interface_geometry",
        subject: subject,
        deck_path: deck_path,
        transition_path: transition_path,
        ramp_path: ramp_path,
        side: side,
        frame: "canonical_stage",
        units: "m",
        axes: #{travel: travel_axis, face: face_axis, lateral: lateral_axis},
        snapshot: "single QueryUsdPrims response",
        footprints: #{deck: deck, transition: transition, ramp: ramp},
        measured_overlap_m: overlap,
        measured_transition_length_m: transition_travel[1] - transition_travel[0],
        deck_top_step_m: (deck_top - transition_top).abs(),
        signed_hinge_gap_m: signed_hinge_gap,
        hinge_gap_m: signed_hinge_gap.abs(),
        lateral_coverage_m: lateral_coverage,
        deck_lateral_span_m: deck_lateral_span,
        transition_lateral_span_m: transition_lateral[1] - transition_lateral[0],
        method: "QueryUsdPrims.cooked_collider_parts+convex_cross_sections",
        source: "USD",
    }
}

fn evaluate_requirement(doc, spec) {
    if spec == () || spec.kind == () {
        return unavailable("<missing requirement>", "unknown", "requirement kind is required", "Rhai");
    }
    if spec.kind == "distance" { return distance_requirement(doc, spec); }
    if spec.kind == "plane_distance" { return plane_distance_requirement(doc, spec); }
    if spec.kind == "under" { return under_requirement(doc, spec); }
    if spec.kind == "extent" { return bounds_requirement(doc, spec); }
    unavailable("<unknown requirement>", spec.kind, "unsupported requirement kind", "Rhai")
}

/// Evaluate every rule and retain every result. Findings contain only failed
/// or unavailable rules, while `checks` proves that no rule was skipped.
fn requirement_report(doc, requirements) {
    if doc == () {
        return #{ ok: false, status: "unavailable", error: "explicit document id is required" };
    }
    if requirements == () || requirements.len() == 0 {
        return #{ ok: false, status: "unavailable", error: "at least one requirement is required" };
    }
    let checks = [];
    let findings = [];
    for spec in requirements {
        let check = evaluate_requirement(doc, spec);
        checks.push(check);
        if check.status != "passed" { findings.push(check); }
    }
    #{
        ok: findings.len() == 0,
        status: if findings.len() == 0 { "passed" } else { "failed" },
        document: doc,
        checks: checks,
        findings: findings,
        evaluated: checks.len(),
        failed: findings.len(),
    }
}

/// Compact evidence contract for a UI/agent consumer. It keeps the actual
/// method, frame, units, tolerance, and generation owner-visible instead of
/// reducing a measurement to a bare boolean.
fn evidence_summary(report) {
    if report == () { return (); }
    #{
        ok: report.ok,
        status: report.status,
        evaluated: report.evaluated,
        failed: report.failed,
        checks: report.checks,
        findings: report.findings,
    }
}
