// ── MoonDAO connectivity demo (doc 43) — scenes/sandbox/comms_demo_test.usda ──
//
// A scripted tour of the rover↔relay↔Earth comms stack:
//   • reads the live link PORTS the connectivity layer publishes on every
//     antenna (`comms:<peer>:connected|range_m|elevation_deg`,
//     `comms:route_earth:connected|hops`) — the same surface Modelica wires,
//     the API and MCP read;
//   • flies the camera surface → Moon orbit → Earth → back (the site-anchored
//     solar hierarchy makes it one continuous space);
//   • re-shapes the link POLICY live via `register_hook` — a "solar storm"
//     rule that cuts every direct-to-Earth path, forcing relay routing — then
//     restores the canonical rule. Policy is rhai DATA, not Rust.

// rhai fns don't see top-level consts (and `global::` doesn't reach the
// scenario module) — constants are FUNCTIONS returning literals.
fn rover_path() { "/CommsDemoScene/Rover" }
fn rover_ant_path() { "/CommsDemoScene/Rover/Comms" }

/// One antenna's Earth-route status as a short human line.
fn route_line(path) {
    let e = find(path);
    if e == () { return path + ": not found"; }
    let up = get(e, "comms:route_earth:connected");
    let hops = get(e, "comms:route_earth:hops");
    if up == () { return path + ": no comms state yet"; }
    if up > 0.5 {
        path + ": EARTH LINK UP (" + hops + " hop(s))"
    } else {
        path + ": no route to Earth"
    }
}

/// Log + toast the rover's link table (each DSN site + the relay + the mast).
fn link_report(tag) {
    let e = find(rover_ant_path());
    if e == () { return; }
    print("[comms-demo] " + tag);
    for peer in ["commsdemoscene_basemast", "commsdemoscene_relaysat",
                 "commsdemoscene_dss_goldstone", "commsdemoscene_dss_canberra",
                 "commsdemoscene_dss_madrid"] {
        let up = get(e, "comms:" + peer + ":connected");
        if up == () { continue; }
        let range = get(e, "comms:" + peer + ":range_m");
        let state = if up > 0.5 { "UP  " } else { "down" };
        print("[comms-demo]   " + state + " " + peer + "  " + (range / 1000.0).to_int() + " km");
    }
    print("[comms-demo]   " + route_line(rover_ant_path()));
    notify_kind(route_line(rover_ant_path()), "info");
}

/// "Solar storm": direct surface↔Earth links drop; only short local links and
/// satellite relays survive. Same ctx contract as policy/comms_link.rhai.
/// (Back-tick strings = rhai's multi-line literals.)
fn storm_rule() { `
fn link_connected(ctx) {
    if ctx.occluded { return false; }
    if ctx.range_m > ctx.max_range_m { return false; }
    if ctx.elev_a < ctx.min_elev_a || ctx.elev_b < ctx.min_elev_b { return false; }
    // Storm: anything reaching across cislunar space to a DSN dish is out.
    if ctx.range_m > 100000000.0
        && (ctx.a.contains("DSS") || ctx.b.contains("DSS")) { return false; }
    true
}` }

/// The canonical rule (mirror of assets/scripting/policy/comms_link.rhai) to
/// restore after the storm beat.
fn canonical_rule() { `
fn link_connected(ctx) {
    if ctx.occluded { return false; }
    if ctx.range_m > ctx.max_range_m { return false; }
    if ctx.elev_a < ctx.min_elev_a || ctx.elev_b < ctx.min_elev_b { return false; }
    true
}` }

fn task(me) {
    seq([
        // ── Beat 1: on the ridge — who can the rover talk to right now? ──
        once(|m| {
            notify_kind("Shackleton ridge: rover online.", "info");
            link_report("surface link table:");
        }),
        wait(4.0),

        // ── Beat 2: pull out to lunar orbit (one continuous space) ──
        once(|m| {
            notify_kind("Pulling out to lunar orbit…", "info");
            focus(find("Moon Body (Rotating)"));
        }),
        wait(8.0),

        // ── Beat 3: across cislunar space to Earth (the DSN end) ──
        once(|m| {
            notify_kind("The far end: Earth's Deep Space Network.", "info");
            focus(find("Earth Body (Rotating)"));
        }),
        wait(8.0),

        // ── Beat 4: back to the rover ──
        once(|m| {
            focus(find(rover_path()));
            notify_kind("Back on the surface.", "info");
        }),
        wait(4.0),

        // ── Beat 5: POLICY IS DATA — a solar storm cuts the direct DSN paths ──
        once(|m| {
            register_hook("comms.link.connected", "link_connected", storm_rule());
            notify_kind("Solar storm: direct Earth links degraded.", "warning");
        }),
        wait(2.0),
        once(|m| link_report("storm link table:")),
        wait(4.0),

        // ── Beat 6: storm passes — canonical rule restored ──
        once(|m| {
            register_hook("comms.link.connected", "link_connected", canonical_rule());
            notify_kind("Storm passed — links restored.", "info");
        }),
        wait(2.0),
        once(|m| link_report("restored link table:")),
    ])
}
