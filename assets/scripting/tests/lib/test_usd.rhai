// ── USD read helpers + GENERIC geometry predicates ──────────────────────────
//
// TEST-ONLY (see test_assert.rhai for why this is not in the prelude).
//
// Everything here is asset-AGNOSTIC: it knows about USD, never about a
// particular habitat. Facts about a specific model — "semiRadial must be 7.345",
// "the apex is at 11.5 m" — belong in that model's own repo, next to the asset
// they describe. The split is mechanism here, policy there.
//
// All reads go through the `QueryUsdPrim` API query, so these helpers work
// unchanged from rhai, Python, raw HTTP or MCP — the query is the shared
// surface, not a rhai binding.

/// Every authored attribute on `path`, as a map. One round trip.
fn usd_prim(path) {
    return query("QueryUsdPrim", #{ path: path });
}

/// Named attributes only. Much cheaper on prims carrying big arrays — a trimmed
/// NurbsPatch holds thousands of control points, and pulling all of them to read
/// one radius is pure waste. Prefer this in a loop.
fn usd_attrs(path, names) {
    return query("QueryUsdPrim", #{ path: path, attrs: names });
}

/// One attribute value, or `()` when the prim or the attribute is absent.
/// The `()` is deliberate: it flows into `t_near`/`t_eq` as a FAILURE rather
/// than defaulting to zero and quietly passing.
fn usd_attr(path, name) {
    let r = query("QueryUsdPrim", #{ path: path, attrs: [name] });
    if type_of(r) == "()" { return (); }
    return r.attrs[name];
}

/// The prim's USD type name (`"NurbsPatch"`, `"Mesh"`, `"Xform"`, …), or `()`.
fn usd_type(path) {
    let r = query("QueryUsdPrim", #{ path: path, attrs: [] });
    if type_of(r) == "()" { return (); }
    return r.type_name;
}

/// Grid-absolute world position of the entity this prim spawned, or `()` when
/// the prim did not spawn one. Same frame `MoveEntity` accepts.
fn usd_world_pos(path) {
    let r = query("QueryUsdPrim", #{ path: path, attrs: [] });
    if type_of(r) == "()" { return (); }
    return r.world_position;
}

// ── Control-net geometry ────────────────────────────────────────────────────
// A NurbsPatch's `points` are ALREADY absolute in the prim's own space — unlike
// Cube/Cylinder/Sphere, which are origin-centred. That asymmetry is a standing
// authoring trap: reaching for a half-height translate to "sit the shape on the
// ground" is correct for a Cylinder and wrong for a NurbsPatch, where it detaches
// the surface from its siblings. `t_no_local_translate` below exists for it.

/// `[min, max]` of the control net along axis `i` (0=x, 1=y, 2=z), or `()`.
fn usd_extent_axis(path, i) {
    let pts = usd_attr(path, "points");
    if type_of(pts) == "()" || pts.len() == 0 { return (); }
    let lo = pts[0][i];
    let hi = pts[0][i];
    for p in pts {
        if p[i] < lo { lo = p[i]; }
        if p[i] > hi { hi = p[i]; }
    }
    return [lo, hi];
}

/// `[min, max]` of the control net in y — the height band a surface occupies.
fn usd_extent_y(path) {
    return usd_extent_axis(path, 1);
}

/// Minimum distance of any control point from the y axis.
///
/// For a rational-circle net this is the SURFACE radius: the corner control
/// points of a quarter-circle sit at r*sqrt(2), so a max would report 1.414x the
/// true radius. The minimum is the on-curve value.
fn usd_radial_min(path) {
    let pts = usd_attr(path, "points");
    if type_of(pts) == "()" || pts.len() == 0 { return (); }
    let best = ();
    for p in pts {
        let d = (p[0] * p[0] + p[2] * p[2]).sqrt();
        if type_of(best) == "()" || d < best { best = d; }
    }
    return best;
}

// ── Generic predicates ──────────────────────────────────────────────────────

/// Two surfaces span the same band in y — the faces of one wall, the skins of
/// one panel. This is the check that catches an offset applied to one face and
/// not its partner.
fn t_coaxial_y(path_a, path_b, tol, what) {
    let a = usd_extent_y(path_a);
    let b = usd_extent_y(path_b);
    if type_of(a) == "()" || type_of(b) == "()" {
        return what + ": missing points on " + path_a + " or " + path_b;
    }
    let lo = t_near(a[0], b[0], tol, what + " (base y)");
    if lo != "" { return lo; }
    return t_near(a[1], b[1], tol, what + " (top y)");
}

/// A prim carries NO local translate.
///
/// For a point-based gprim (NurbsPatch, Mesh, BasisCurves) the points are
/// already absolute in component space, so a local translate double-counts and
/// silently detaches the prim from siblings authored in the same frame. Assert
/// this on any surface whose position is meant to come from its control net.
fn t_no_local_translate(path, what) {
    let t = usd_attr(path, "xformOp:translate");
    return t_absent(t, what + " (xformOp:translate on " + path + ")");
}

/// A prim exists and has the expected USD type. Cheap first line in any suite —
/// a renamed or retyped prim otherwise shows up as a confusing cascade of
/// MISSING attribute failures further down.
fn t_prim_is(path, expected_type, what) {
    let ty = usd_type(path);
    if type_of(ty) == "()" {
        return what + ": prim " + path + " not found";
    }
    return t_eq(ty, expected_type, what + " (" + path + " type)");
}
