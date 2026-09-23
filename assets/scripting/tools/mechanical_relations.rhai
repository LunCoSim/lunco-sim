// Extensible mechanical/CAD relation vocabulary over native Rhai values.
//
// This is an authored policy library, not a Rust domain model. It deliberately
// knows nothing about Twin names, USD paths, SysML selectors, or Modelica
// components. Callers provide already-resolved canonical values and their
// tolerances; the library returns residual/evidence records that a policy can
// attach to a SysML requirement, USD observation, or Modelica result.
//
// Construction/solve operations remain in spatial_relations.rhai. This module
// answers whether a resolved arrangement satisfies a mechanical relation.

// Rhai integer literals are accepted at the script edge, then immediately
// canonicalized to the native f64 representation used by the math bridge.
fn _f64(value) {
    f64_from(value)
}

fn _finite_scalar(value) {
    _f64(value) != ()
}

fn _valid_tolerance(value) {
    _finite_scalar(value) && value >= 0.0
}

fn _valid_vector(value) {
    // Numerical validity belongs to the shared native f64 vector bridge. Do
    // not duplicate an epsilon here: this library's tolerance is authored by
    // the requirement/policy caller.
    vec3_is_valid(value)
}

fn _result(relation, residual, tolerance, actual, expected, units) {
    let residual_f64 = _f64(residual);
    let tolerance_f64 = _f64(tolerance);
    if residual_f64 == () || tolerance_f64 == () || tolerance_f64 < 0.0 {
        return ();
    }
    let actual_f64 = if _finite_scalar(actual) { _f64(actual) } else { actual };
    let expected_f64 = if _finite_scalar(expected) { _f64(expected) } else { expected };
    #{relation: relation,
      status: if residual_f64.abs() <= tolerance_f64 { "passed" } else { "failed" },
      residual: residual_f64, tolerance: tolerance_f64,
      actual: actual_f64, expected: expected_f64, units: units}
}

fn equal(actual, expected, tolerance) {
    if !_finite_scalar(actual) || !_finite_scalar(expected) { return (); }
    _result("equal", (actual - expected).abs(), tolerance, actual, expected, "")
}

fn at_least(actual, minimum, tolerance) {
    if !_finite_scalar(actual) || !_finite_scalar(minimum) { return (); }
    let residual = if minimum > actual { minimum - actual } else { 0.0 };
    _result("at_least", residual, tolerance,
        actual, minimum, "")
}

fn at_most(actual, maximum, tolerance) {
    if !_finite_scalar(actual) || !_finite_scalar(maximum) { return (); }
    let residual = if actual > maximum { actual - maximum } else { 0.0 };
    _result("at_most", residual, tolerance,
        actual, maximum, "")
}

fn between(actual, minimum, maximum, tolerance) {
    if !_finite_scalar(actual) || !_finite_scalar(minimum)
        || !_finite_scalar(maximum) || minimum > maximum { return (); }
    let residual = if actual < minimum { minimum - actual }
        else if actual > maximum { actual - maximum }
        else { 0.0 };
    _result("between", residual, tolerance, actual,
        #{minimum: minimum, maximum: maximum}, "")
}

fn distance(a, b) {
    if !vec3_is_finite(a) || !vec3_is_finite(b) { return (); }
    vlen(vsub(a, b))
}

/// Transform a point by a planar rigid rotation about Z and a translation.
/// Coordinates keep the caller's length unit; the angle is in radians.
fn planar_rigid_transform_point(origin, local_point, rotation_rad) {
    if !vec3_is_finite(origin) || !vec3_is_finite(local_point)
        || f64_from(rotation_rad) == () {
        return ();
    }
    let cosine = cos(rotation_rad);
    let sine = sin(rotation_rad);
    let x = vcomponent(origin, 0)
        + vcomponent(local_point, 0) * cosine
        - vcomponent(local_point, 1) * sine;
    let y = vcomponent(origin, 1)
        + vcomponent(local_point, 0) * sine
        + vcomponent(local_point, 1) * cosine;
    vec3(x, y, vcomponent(origin, 2) + vcomponent(local_point, 2))
}

/// Return the travel-direction offset that miter-cuts a slab edge so its toe
/// face becomes horizontal after a planar Z rotation. `thickness` and the
/// returned run use the caller's length unit; the angle is in radians.
fn planar_horizontal_face_bevel_run(thickness, rotation_rad) {
    if !_finite_scalar(thickness) || thickness <= 0.0
        || !_finite_scalar(rotation_rad) { return (); }
    let sine = sin(rotation_rad).abs();
    if sine == 0.0 { return (); }
    thickness * cos(rotation_rad).abs() / sine
}

fn distance_requirement(a, b, expected, tolerance) {
    let actual = distance(a, b);
    if actual == () || !_finite_scalar(expected) { return (); }
    _result("distance", (actual - expected).abs(), tolerance,
        actual, expected, "m")
}

fn coincident(a, b, tolerance) {
    distance_requirement(a, b, 0.0, tolerance)
}

fn plane_signed_distance(point, origin, normal) {
    if !_valid_vector(normal) || !vec3_is_finite(point)
        || !vec3_is_finite(origin) { return (); }
    vdot(vsub(point, origin), vnorm(normal))
}

fn plane_distance_requirement(point, origin, normal, expected, tolerance) {
    let actual = plane_signed_distance(point, origin, normal);
    if actual == () || !_finite_scalar(expected) { return (); }
    _result("plane_distance", (actual - expected).abs(), tolerance,
        actual, expected, "m")
}

fn _angle_rad(a, b, oriented) {
    if !_valid_vector(a) || !_valid_vector(b) { return (); }
    if oriented { return vangle_rad(a, b); }
    let cosine = vcosine(a, b);
    if cosine == () { return (); }
    acos(cosine.abs())
}

fn angle_requirement(a, b, expected_rad, tolerance_rad) {
    let actual = _angle_rad(a, b, true);
    if actual == () || !_finite_scalar(expected_rad) { return (); }
    _result("angle", (actual - expected_rad).abs(), tolerance_rad,
        actual, expected_rad, "rad")
}

// Parallel accepts anti-parallel directions as parallel. Use
// same_direction when orientation matters.
fn parallel(a, b, tolerance_rad) {
    let actual = _angle_rad(a, b, false);
    if actual == () { return (); }
    _result("parallel", actual, tolerance_rad, actual, 0.0, "rad")
}

fn same_direction(a, b, tolerance_rad) {
    let actual = _angle_rad(a, b, true);
    if actual == () { return (); }
    _result("same_direction", actual, tolerance_rad, actual, 0.0, "rad")
}

fn perpendicular(a, b, tolerance_rad) {
    let actual = _angle_rad(a, b, true);
    if actual == () { return (); }
    _result("perpendicular", (actual - PI() / 2.0).abs(), tolerance_rad,
        actual, PI() / 2.0, "rad")
}

fn collinear(a, b, point, tolerance_m) {
    if !_valid_vector(vsub(b, a)) || !vec3_is_finite(a) || !vec3_is_finite(b)
        || !vec3_is_finite(point) { return (); }
    let line = vsub(b, a);
    let distance_to_line = vlen(vcross(vsub(point, a), vnorm(line)));
    _result("collinear", distance_to_line, tolerance_m,
        distance_to_line, 0.0, "m")
}

fn coplanar(a, b, c, point, tolerance_m) {
    if !vec3_is_finite(a) || !vec3_is_finite(b)
        || !vec3_is_finite(c) || !vec3_is_finite(point) { return (); }
    let normal = vcross(vsub(b, a), vsub(c, a));
    if !_valid_vector(normal) { return (); }
    let distance_to_plane = plane_signed_distance(point, a, normal);
    _result("coplanar", distance_to_plane.abs(), tolerance_m,
        distance_to_plane, 0.0, "m")
}

fn line_parallel(origin_a, direction_a, origin_b, direction_b, tolerance_rad) {
    let relation = parallel(direction_a, direction_b, tolerance_rad);
    if relation == () { return (); }
    relation.relation = "line_parallel";
    relation.origin_distance_m = distance(origin_a, origin_b);
    relation
}

fn plane_parallel(normal_a, normal_b, tolerance_rad) {
    let relation = parallel(normal_a, normal_b, tolerance_rad);
    if relation == () { return (); }
    relation.relation = "plane_parallel";
    relation
}

fn plane_perpendicular(normal_a, normal_b, tolerance_rad) {
    let relation = perpendicular(normal_a, normal_b, tolerance_rad);
    if relation == () { return (); }
    relation.relation = "plane_perpendicular";
    relation
}

fn axis_delta(parent, child, axis) {
    if !vec3_is_finite(parent) || !vec3_is_finite(child) { return (); }
    if axis == "x" { return vcomponent(parent, 0) - vcomponent(child, 0); }
    if axis == "y" { return vcomponent(parent, 1) - vcomponent(child, 1); }
    if axis == "z" { return vcomponent(parent, 2) - vcomponent(child, 2); }
    ()
}

fn under(child, parent, axis, minimum, tolerance) {
    let actual = axis_delta(parent, child, axis);
    if actual == () || !_finite_scalar(minimum) { return (); }
    let relation = at_least(actual, minimum, tolerance);
    if relation == () { return (); }
    relation.relation = "under";
    relation.units = "m";
    relation
}

fn _project_to_axis(point, origin, axis) {
    if !_valid_vector(axis) || !vec3_is_finite(point)
        || !vec3_is_finite(origin) { return (); }
    let unit_axis = vnorm(axis);
    let along = vscale(unit_axis, vdot(vsub(point, origin), unit_axis));
    vadd(origin, along)
}

fn _mirror_across_plane(point, origin, normal) {
    let signed = plane_signed_distance(point, origin, normal);
    if signed == () { return (); }
    vsub(point, vscale(vnorm(normal), 2.0 * signed))
}

fn _half_turn_about_axis(point, origin, axis) {
    let projected = _project_to_axis(point, origin, axis);
    if projected == () { return (); }
    vsub(vscale(projected, 2.0), point)
}

// Check that actual is the reflection of source across a plane.
fn mirrored_about_plane(actual, source, plane_origin, plane_normal, tolerance_m) {
    let mirrored = _mirror_across_plane(source, plane_origin, plane_normal);
    if mirrored == () { return (); }
    let relation = coincident(actual, mirrored, tolerance_m);
    if relation == () { return (); }
    relation.relation = "mirrored_about_plane";
    relation.mirrored = mirrored;
    relation
}

// Pair symmetry about a plane is the same physical relation as mirroring, but
// the name makes authored CAD intent natural when no source/target direction
// is important.
fn symmetric_about_plane(first, second, plane_origin, plane_normal, tolerance_m) {
    let relation = mirrored_about_plane(second, first,
        plane_origin, plane_normal, tolerance_m);
    if relation == () { return (); }
    relation.relation = "symmetric_about_plane";
    relation
}

// Axial symmetry is a half-turn about a line. This is distinct from mirror
// symmetry, which uses a plane; the distinction matters for cylindrical parts.
fn symmetric_about_axis(first, second, axis_origin, axis_direction, tolerance_m) {
    let rotated = _half_turn_about_axis(first, axis_origin, axis_direction);
    if rotated == () { return (); }
    let relation = coincident(second, rotated, tolerance_m);
    if relation == () { return (); }
    relation.relation = "symmetric_about_axis";
    relation.mirrored = rotated;
    relation
}

fn midpoint(first, second) {
    if !vec3_is_finite(first) || !vec3_is_finite(second) { return (); }
    vscale(vadd(first, second), 0.5)
}

fn midpoint_on_plane(first, second, plane_origin, plane_normal, tolerance_m) {
    let center = midpoint(first, second);
    if center == () { return (); }
    let relation = plane_distance_requirement(center, plane_origin,
        plane_normal, 0.0, tolerance_m);
    if relation == () { return (); }
    relation.relation = "midpoint_on_plane";
    relation
}

fn symmetric_pairs_about_plane(first_points, second_points,
        plane_origin, plane_normal, tolerance_m) {
    if !array_is(first_points) || !array_is(second_points)
        || first_points.len() != second_points.len() { return (); }
    let relations = [];
    let maximum = 0.0;
    let status = "passed";
    for index in 0..first_points.len() {
        let relation = symmetric_about_plane(first_points[index],
            second_points[index], plane_origin, plane_normal, tolerance_m);
        if relation == () { return (); }
        relations.push(relation);
        if relation.residual > maximum { maximum = relation.residual; }
        if relation.status != "passed" { status = "failed"; }
    }
    #{relation: "symmetric_pairs_about_plane", status: status,
      residual: maximum, tolerance: tolerance_m, relations: relations,
      pair_count: first_points.len()}
}

fn symmetric_pairs_about_axis(first_points, second_points,
        axis_origin, axis_direction, tolerance_m) {
    if !array_is(first_points) || !array_is(second_points)
        || first_points.len() != second_points.len() { return (); }
    let relations = [];
    let maximum = 0.0;
    let status = "passed";
    for index in 0..first_points.len() {
        let relation = symmetric_about_axis(first_points[index],
            second_points[index], axis_origin, axis_direction, tolerance_m);
        if relation == () { return (); }
        relations.push(relation);
        if relation.residual > maximum { maximum = relation.residual; }
        if relation.status != "passed" { status = "failed"; }
    }
    #{relation: "symmetric_pairs_about_axis", status: status,
      residual: maximum, tolerance: tolerance_m, relations: relations,
      pair_count: first_points.len()}
}

fn axis_coincident(origin_a, direction_a, origin_b, direction_b,
        tolerance_m, tolerance_rad) {
    let direction = parallel(direction_a, direction_b, tolerance_rad);
    if direction == () { return (); }
    let offset = vsub(origin_b, origin_a);
    let perpendicular_offset = vsub(offset,
        vscale(vnorm(direction_a), vdot(offset, vnorm(direction_a))));
    let position = _result("axis_coincident", vlen(perpendicular_offset),
        tolerance_m, vlen(perpendicular_offset), 0.0, "m");
    if position == () { return (); }
    #{relation: "axis_coincident", status: if direction.status == "passed"
        && position.status == "passed" { "passed" } else { "failed" },
      residual: position.residual, tolerance: tolerance_m,
      direction: direction, position: position}
}
