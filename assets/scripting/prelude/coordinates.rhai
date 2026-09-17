// ── Coordinate points ───────────────────────────────────────────────────────
//
// A bare [x, y, z] is a vector. It is not enough information for a position:
// the same numbers can be render-relative, active-physics, or document-local.
// Point helpers keep the frame tag beside the values while remaining ordinary
// Rhai data, so authored tools do not need a Rust type or a JSON codec.
//
// The host converts a pointer hit into `world_position` before this prelude
// runs. That field is a point3 in the `active_physics` frame. Its companion
// `render_position` is presentation-only and must never be sent to a spatial
// authoring command.

/// Construct a finite point with an explicit semantic frame.
fn point3(values, frame) {
    if type_of(frame) != "string" || frame == "" || vlen(values) == () { return (); }
    #{ kind: "point3", values: values, frame: frame }
}

/// Read a point's numeric values, or `()` when the value is not a point3.
fn point_values(point) {
    if type_of(point) != "map" || point.kind != "point3" { return (); }
    let values = point.values;
    if vlen(values) == () { return (); }
    values
}

/// Read a point's frame tag, or `()` when the value is not a point3.
fn point_frame(point) {
    if type_of(point) != "map" || point.kind != "point3" { return (); }
    let frame = point.frame;
    if type_of(frame) != "string" || frame == "" { return (); }
    frame
}

/// Validate a point without guessing or repairing its frame.
fn point_valid(point) {
    point_values(point) != () && point_frame(point) != ()
}

/// True when both points use the same explicit frame.
fn point_same_frame(a, b) {
    point_valid(a) && point_valid(b) && point_frame(a) == point_frame(b)
}

/// Add a frame-free offset to a point.
fn point_offset(point, offset) {
    if !point_valid(point) || vlen(offset) == () { return (); }
    point3(vadd(point_values(point), offset), point_frame(point))
}

/// Subtract two points in the same frame and return a frame-free offset.
fn point_delta(a, b) {
    if !point_same_frame(a, b) { return (); }
    vsub(point_values(a), point_values(b))
}

/// Distance between two points in the same frame, or `()` for a frame error.
fn point_distance(a, b) {
    let delta = point_delta(a, b);
    if delta == () { return (); }
    vlen(delta)
}

/// The standard frame-tagged position read for an entity.
fn world_point(id) {
    let values = world_pos(id);
    if values == () { return (); }
    point3(values, "active_physics")
}

/// Resolve a scene-pointer hit for an authored tool.
///
/// This returns a small result map so a tool can show a useful reason when a
/// frame is not admitted yet. It never falls back to `render_position`.
fn pointer_point(context) {
    if context == () { return #{ ok: false, error: "pointer context is unavailable" }; }
    let point = context.world_position;
    if point == () {
        let error = context.position_error;
        if error == () { error = "the pointer did not hit a spatial surface"; }
        return #{ ok: false, error: error };
    }
    if !point_valid(point) {
        return #{ ok: false, error: "pointer position is not a valid point3" };
    }
    if point_frame(point) != "active_physics" {
        return #{ ok: false,
            error: "pointer position is not in the active physics frame" };
    }
    #{ ok: true, point: point }
}

/// Resolve the canonical analytic terrain point under a scene pointer.
///
/// Streamed terrain tiles keep their mesh transform local to their BigSpace
/// cell, so `world_position` can be a mesh-local hit even though the terrain
/// oracle has a valid point. Terrain-aware tools must use this helper; it
/// uses the explicit `surface_world_position` field and never interprets a
/// render-space or arbitrary mesh hit as an active-physics terrain point.
/// A missing surface hit is a real miss, not a value to repair or guess.
fn pointer_surface_point(context) {
    if context == () { return #{ ok: false, error: "pointer context is unavailable" }; }
    let point = context.surface_world_position;
    if point == () {
        return #{ ok: false, error: "pointer did not hit terrain" };
    }
    if !point_valid(point) || point_frame(point) != "active_physics" {
        return #{ ok: false, error: "terrain surface point is not in the active physics frame" };
    }
    #{ ok: true, point: point }
}
