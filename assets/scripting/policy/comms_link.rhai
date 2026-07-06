// ── Comms link-availability policy (doc 43) ──────────────────────────────────
//
// Decides whether a sight-line between two antennas is a usable link.
// Registered under `comms.link.connected` and consulted by the connectivity
// layer for EVERY antenna pair after it resolves the geometry. This is DATA,
// not Rust — tune link rules here (or override per scenario with
// `register_hook("comms.link.connected", "my_rule", src)`) without a rebuild.
//
// `ctx` is a map:
//   a, b            antenna names
//   range_m         distance between the antennas (meters)
//   elev_a, elev_b  elevation of the peer above each local horizon (degrees;
//                   orbiting antennas read 90 — no horizon)
//   min_elev_a/_b   each antenna's authored elevation mask (degrees; −90 = off)
//   occluded        a celestial body blocks the sight-line
//   occluded_by     that body's name ("" if clear)
//   max_range_m     the pair's combined range limit
//
// Return true = link up. The Rust fallback applies this same canonical rule
// if this script is missing or fails to compile.

fn link_connected(ctx) {
    // A body between the endpoints kills the link outright.
    if ctx.occluded {
        return false;
    }

    // Beyond either antenna's usable range.
    if ctx.range_m > ctx.max_range_m {
        return false;
    }

    // Elevation masks: surface stations refuse to track below their mask
    // (terrain, RF noise near the horizon). −90 disables the mask.
    if ctx.elev_a < ctx.min_elev_a || ctx.elev_b < ctx.min_elev_b {
        return false;
    }

    true
}
