// Production-Rhai contract for the shared f64 geometry surface. Model and
// scene acceptance stays in Twin-owned scenario files; this checks only the
// generic native geometry mechanism exposed to authored tools.

import "assembly_audit" as assembly_audit;

fn _revolved_mesh_signed_volume(mesh) {
    let volume = 0.0;
    let cursor = 0;
    for face_count in mesh.face_vertex_counts {
        let first = mesh.points[mesh.face_vertex_indices[cursor]];
        for corner in 1..face_count - 1 {
            let b = mesh.points[mesh.face_vertex_indices[cursor + corner]];
            let c = mesh.points[mesh.face_vertex_indices[cursor + corner + 1]];
            volume += (first.x * (b.y * c.z - b.z * c.y)
                + first.y * (b.z * c.x - b.x * c.z)
                + first.z * (b.x * c.y - b.y * c.x)) / 6.0;
        }
        cursor += face_count;
    }
    volume
}

let checks = [];

let scaled = vec3(1.0, -2.0, 3.5) * 0.5;
let scaled_left = 0.5 * vec3(1.0, -2.0, 3.5);
checks.push(t_true(type_of(scaled) == "Vec3" && scaled.x == 0.5
    && scaled.y == -1.0 && scaled.z == 1.75,
    "native f64 Vec3 supports typed scalar multiplication"));
checks.push(t_true(type_of(scaled_left) == "Vec3" && scaled_left.x == 0.5
    && scaled_left.y == -1.0 && scaled_left.z == 1.75,
    "native f64 Vec3 supports scalar multiplication in either operand order"));

let profile = [
    vec2(0.0, -1.0), vec2(1.0, -0.5), vec2(1.0, 0.5),
    vec2(0.0, 1.0), vec2(-1.0, 0.5), vec2(-1.0, -0.5),
];
let mesh = extrude_profile(profile, 0.16, profile_plane_xz());
checks.push(t_true(mesh.vertex_count == 6,
    "typed hex profile retains six vertices per cap"));
checks.push(t_true(mesh.face_count == 8,
    "extrusion has two caps and six side faces"));
checks.push(t_true(mesh.points.len() == 12 && mesh.face_vertex_indices.len() == 36,
    "closed prism has complete indexed topology"));
checks.push(t_true(mesh.face_varying_normals.len() == 36,
    "profile builder returns one unit normal per face corner"));
checks.push(t_true(mesh.face_varying_normals[0].y < -0.999
    && mesh.face_varying_normals[6].y > 0.999,
    "XZ caps have outward normals in the expected Y directions"));
checks.push(t_true(mesh.points[0].z > 0.999 && mesh.points[6].y > 0.079,
    "XZ profile mapping preserves the right-handed local frame"));

let foot_radius = 0.52;
let hemisphere_profile = [vec2(0.0, 0.0), vec2(foot_radius, 0.0)];
for station in 1..24 {
    let angle = 1.5707963267948966 * station / 24.0;
    hemisphere_profile.push(vec2(foot_radius * angle.cos(),
        foot_radius * angle.sin()));
}
hemisphere_profile.push(vec2(0.0, foot_radius));
let hemisphere = revolve_profile(hemisphere_profile, 64);
let hemisphere_volume = _revolved_mesh_signed_volume(hemisphere);
let expected_hemisphere_volume = 2.0 * 3.141592653589793
    * foot_radius * foot_radius * foot_radius / 3.0;
checks.push(t_true(type_of(hemisphere) == "RevolvedProfileMesh"
    && hemisphere.vertex_count > 1400 && hemisphere.face_count > 1500
    && hemisphere.face_vertex_indices.len() == hemisphere.face_varying_normals.len()
    && abs(hemisphere_volume / expected_hemisphere_volume - 1.0) < 0.01,
    "typed profile revolution builds a closed hemisphere at the requested radius with outward volume"));

let min_x = foot_radius;
let min_y = foot_radius;
let min_z = foot_radius;
let max_x = -foot_radius;
let max_y = -foot_radius;
let max_z = -foot_radius;
for point in hemisphere.points {
    if point.x < min_x { min_x = point.x; }
    if point.y < min_y { min_y = point.y; }
    if point.z < min_z { min_z = point.z; }
    if point.x > max_x { max_x = point.x; }
    if point.y > max_y { max_y = point.y; }
    if point.z > max_z { max_z = point.z; }
}
let face_offset = 0;
let oriented_faces = true;
for face_count in hemisphere.face_vertex_counts {
    let first_index = hemisphere.face_vertex_indices[face_offset];
    let normal = hemisphere.face_varying_normals[face_offset];
    let center_dot = normal.x * hemisphere.points[first_index].x
        + normal.y * hemisphere.points[first_index].y
        + normal.z * hemisphere.points[first_index].z;
    if center_dot < -0.000001 { oriented_faces = false; }
    face_offset += face_count;
}
checks.push(t_true(abs(min_x + foot_radius) < 0.000001
    && min_y == 0.0 && abs(min_z + foot_radius) < 0.000001
    && abs(max_x - foot_radius) < 0.000001
    && abs(max_y - foot_radius) < 0.000001
    && abs(max_z - foot_radius) < 0.000001 && oriented_faces,
    "revolved hemisphere has the dimensional envelope and outward right-handed face normals"));

let a = oriented_bounds3(vec3(0.0, 0.0, 0.0), vec3(1.0, 0.2, 0.2),
    quat_identity());
let b = oriented_bounds3(vec3(0.0, 0.0, 1.5), vec3(1.0, 0.2, 0.2),
    quat_identity());
let separated = bounds_relation(a, b);
checks.push(t_true(separated.separated && separated.greatest_axis_margin > 1.0,
    "native OBB SAT reports a positive signed separation margin"));

let precision_a = oriented_bounds3(vec3(100000000.0, 0.0, 0.0), vec3(0.5, 0.5, 0.5),
    quat_identity());
let precision_b = oriented_bounds3(vec3(100000001.25, 0.0, 0.0), vec3(0.5, 0.5, 0.5),
    quat_identity());
let precision_relation = bounds_relation(precision_a, precision_b);
checks.push(t_true(precision_relation.separated
    && abs(precision_relation.greatest_axis_margin - 0.25) < 0.000000001
    && precision_relation.axis_from_a_to_b.x == 1.0,
    "OBB SAT preserves f64 separation at large world coordinates"));

let overlap_a = oriented_bounds3(vec3(0.0, 0.0, 0.0), vec3(1.0, 0.2, 0.2),
    quat_identity());
let overlap_b = oriented_bounds3(vec3(0.0, 0.0, 0.1), vec3(1.0, 0.2, 0.2),
    quat_from_euler_xyz_deg(vec3(0.0, 90.0, 0.0)));
let overlap_relation = bounds_relation(overlap_a, overlap_b);
checks.push(t_true(!overlap_relation.separated
    && overlap_relation.greatest_axis_margin < 0.0,
    "OBB SAT detects a rotated intersection"));

let pair = assembly_audit::oriented_box_clearance("panel-to-equipment",
    vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 0.2), quat_identity(),
    vec3(0.0, 0.0, 1.5), vec3(1.0, 1.0, 0.2), quat_identity(),
    0.5);
checks.push(t_true(pair.ok && pair.status == "clear" && pair.signed_axis_margin_m > 0.5,
    "Rhai audit returns structured caller-policy clearance evidence"));

let report = assembly_audit::oriented_clearance_report([
    #{label: "equipment-clearance", center_a: vec3(0.0, 0.0, 0.0),
      dimensions_a: vec3(1.0, 1.0, 0.2), rotation_a: quat_identity(),
      center_b: vec3(0.0, 0.0, 1.5), dimensions_b: vec3(1.0, 1.0, 0.2),
      rotation_b: quat_identity(), required_clearance_m: 0.5},
]);
checks.push(t_true(report.ok && report.check_count == 1 && report.failure_count == 0
    && report.checks[0].label == "equipment-clearance"
    && report.checks[0].status == "clear"
    && report.checks[0].separating_axis.z > 0.99,
    "batch clearance report retains structured identity, status, margin and axis evidence"));

let b_far = oriented_bounds3(vec3(3.0, 0.0, 0.0), vec3(0.5, 0.5, 0.5),
    quat_from_euler_xyz_deg(vec3(0.0, 45.0, 0.0)));
let rotated = bounds_relation(a, b_far);
checks.push(t_true(rotated.separated && rotated.axis_from_a_to_b.x > 0.9,
    "OBB SAT handles a rotated component and returns its separating axis"));
t_report(checks);
