// Composed USD geometry audits for authored assemblies.
//
// These checks deliberately operate on the composed stage through the normal
// query/world-position surface. They do not infer design intent: the caller
// supplies the mating envelope that the asset contract requires. That keeps
// this library reusable without adding a custom USD schema or a Rust-side
// geometry policy.

const EPSILON = 0.001;

fn close_vec(actual, expected, epsilon) {
    if actual == () || expected == () || actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if (actual[i] - expected[i]).abs() > epsilon {
            return false;
        }
    }
    true
}

fn composed_transform_contract(path, expected_scale, expected_order, epsilon) {
    let record = query("QueryUsdPrim", #{
        path: path,
        attrs: ["xformOp:scale", "xformOpOrder"],
    });
    if record == () || record.ok == false || record.attrs == () {
        return false;
    }
    close_vec(record.attrs["xformOp:scale"], expected_scale, epsilon)
        && record.attrs["xformOpOrder"] == expected_order
}

/// Return the half extents of an axis-aligned UsdGeomCube in composed metres.
/// Only translation plus scale is accepted; rotated geometry needs an explicit
/// world-space contract instead of a guessed AABB.
fn axis_aligned_cube_half_extents(path) {
    let record = query("QueryUsdPrim", #{
        path: path,
        attrs: ["size", "xformOp:scale", "xformOpOrder"],
    });
    if record == () || record.ok == false || record.attrs == () {
        return ();
    }
    let size = record.attrs["size"];
    let scale = record.attrs["xformOp:scale"];
    let order = record.attrs["xformOpOrder"];
    if size == () || scale == () || order == () || order.len() == 0 {
        return ();
    }

    let has_scale = false;
    for op in order {
        if op == "xformOp:scale" {
            has_scale = true;
        } else if op != "xformOp:translate" {
            return ();
        }
    }
    if !has_scale || scale.len() != 3 {
        return ();
    }
    [
        0.5 * size * scale[0].abs(),
        0.5 * size * scale[1].abs(),
        0.5 * size * scale[2].abs(),
    ]
}

fn world_position(path) {
    let id = find(path);
    if id < 0 {
        return ();
    }
    world_pos(id)
}

/// Test two composed prim centres against supplied world-space half extents.
/// Extents are [X, Y, Z] metres and the primitive transforms must be
/// axis-aligned in the shared stage frame.
fn world_aabb_overlap(path_a, extents_a, path_b, extents_b, epsilon) {
    let a = world_position(path_a);
    let b = world_position(path_b);
    if a == () || b == () || extents_a == () || extents_b == ()
        || a.len() != 3 || b.len() != 3 || extents_a.len() != 3 || extents_b.len() != 3 {
        return false;
    }
    for i in 0..3 {
        if (a[i] - b[i]).abs() > extents_a[i] + extents_b[i] + epsilon {
            return false;
        }
    }
    true
}

/// Verify a host-to-mount-to-pivot seating chain in the composed stage.
/// `mount_extents` and `pivot_extents` are the authored contact envelopes,
/// not guessed dimensions hidden in the tool.
fn seat_pair(host_path, host_extents, mount_path, mount_extents, pivot_path, pivot_extents, epsilon) {
    world_aabb_overlap(host_path, host_extents, mount_path, mount_extents, epsilon)
        && world_aabb_overlap(mount_path, mount_extents, pivot_path, pivot_extents, epsilon)
}

/// Verify every pair in a list of seven-element pair records. Each record is
/// [host_path, host_extents, mount_path, mount_extents, pivot_path,
/// pivot_extents, epsilon].
fn seat_pairs(pairs) {
    for pair in pairs {
        if pair.len() != 7 || !seat_pair(
            pair[0], pair[1], pair[2], pair[3], pair[4], pair[5], pair[6]
        ) {
            return false;
        }
    }
    true
}
