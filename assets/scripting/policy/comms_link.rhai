// ── Comms link availability = a COMPOSITION OF CONSTRAINTS (docs 43 + 10) ─────
//
// A sight-line between two antennas is a usable link IFF every constraint in
// `constraints()` passes. This is the STK-"Access" model: the core computes
// generic geometry; the LINK is composed here, as authored data — extend it by
// adding a constraint, never by touching Rust.
//
// Registered under `comms.link.connected`; consulted for EVERY antenna pair
// after the connectivity layer resolves the geometry. Tune here, or override
// per scenario with `register_hook("comms.link.connected", "link_connected", src)`.
//
// `ctx` is a map the connectivity layer fills for the pair:
//   a, b            antenna names
//   range_m         distance between the antennas (meters)
//   elev_a, elev_b  elevation of the peer above each local horizon (degrees;
//                   orbiting antennas read 90 — no horizon)
//   min_elev_a/_b   each antenna's authored elevation mask (degrees; −90 = off)
//   occluded        a celestial body blocks the sight-line (analytic sphere)
//   occluded_by     that body's name ("" if clear)
//   max_range_m     the pair's combined range limit
//
// Return true = link up. The Rust builtin applies the SAME canonical rule
// (range + elevation mask + body occlusion) if this script is missing or fails,
// so the shipped constraint set below is byte-for-byte that fallback.

// ── The constraint set ───────────────────────────────────────────────────────
// Each constraint is `(ctx) -> bool`. Ship a standard set; ADD YOUR OWN by
// appending a closure to this list (see the examples at the bottom). Order is
// cheap-first so common rejects short-circuit.
fn constraints() {
    [
        // c_range — within the pair's combined usable range.
        |ctx| ctx.range_m <= ctx.max_range_m,

        // c_body — no celestial body (Moon/Earth) across the sight-line.
        //          Closed-form ray–sphere, computed in the core.
        |ctx| !ctx.occluded,

        // c_elevation — each surface antenna sees the peer above its mask.
        //               Orbiting antennas read elev 90 (no horizon); a −90 mask
        //               disables the check for that endpoint.
        |ctx| ctx.elev_a >= ctx.min_elev_a && ctx.elev_b >= ctx.min_elev_b,

        // c_terrain — local relief (the rille wall) cuts the sight-line. Marches
        //             the generic `TerrainRaycast` geometry query between the two
        //             endpoints' site-local positions (published by the
        //             connectivity layer). Skips links with no site DEM and pure
        //             space-to-space links; a missing provider/DEM → no cut.
        |ctx| {
            if ctx.a_local == () || ctx.b_local == () { return true; }  // no site
            if !ctx.a_on_site && !ctx.b_on_site { return true; }        // space link
            let hit = query("TerrainRaycast", #{ origin: ctx.a_local, target: ctx.b_local });
            if hit == () { return true; }                               // no provider/DEM
            !hit.hit
        },

        // ── Add your own (examples — leave commented to keep the default rule) ─
        // A fade/link-margin gate (Friis, or defer to CommsLink.mo):
        // |ctx| friis_margin_db(ctx.range_m) > 0.0,
        // A directional keep-out cone, a rain window, a duty cycle, …:
        // |ctx| !in_keepout(ctx),
    ]
}

fn link_connected(ctx) {
    // Each constraint is isolated: a throwing constraint is logged and SKIPPED
    // (fail-open), never collapsing the whole policy to the Rust builtin — the
    // hard safety rules (range/occlusion/mask) are themselves constraints here.
    for c in constraints() {
        let ok = true;
        try {
            ok = c.call(ctx);
        } catch (err) {
            print("[comms] link constraint error (skipped): " + err);
            ok = true;
        }
        if ok != true {
            return false;
        }
    }
    true
}
