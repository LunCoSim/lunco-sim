//! SS2 — Follow the ДЗЗ Route (Space School track).
//!
//! The teams don't invent a path — the remote-sensing (ДЗЗ) section analyses the
//! slope map and hands them a route that stays inside the rover's safe angle
//! (task §3.3). This lesson: receive that route, sanity-check it, and fly it to
//! the POI and back.
//!
//! The route is an inline array of [x, z] ground points here — which IS the
//! day-one import form (a scenario reads a coordinate list). A CSV/GeoJSON→array
//! converter is an optional small core add later (see 06_CORE_VS_EXTERNAL_SPLIT.md
//! §3); nothing in this lesson changes when it lands.
//! NEEDS (polish, optional): on-terrain polyline renderer for the route line.

/// The ДЗЗ route: NW start → down to the centre POI, kept off the SE highland.
/// Ground [x, z]; height is taken from the terrain when the beads are placed.
fn route() {
    [ [-380.0, -380.0], [-280.0, -250.0], [-170.0, -150.0], [-80.0, -70.0], [0.0, 0.0] ]
}

fn on_start(me) {
    load_scene("tutorials/school/traverse.usda");
    notify_kind("Space School 2/4 · Follow the ДЗЗ Route", "info");
    hint("The remote-sensing team sent a route (the glowing beads). Take control of your rover and follow the beads down to the Point of Interest — then bring it back to the start.");
    // Drop a bead marker at each route point so the line is visible on the ground.
    let r = route();
    for p in r {
        cmd("SpawnEntity", #{ entry_id: "waypoint", position: [p[0], 1.0, p[1]] });
    }
}

fn mission(me) {
    let rover = "/Traverse/Rover";
    let poi   = "/Traverse/POI";
    let start = [-380.0, 1.0, -380.0];
    [
        objective("possess", #{
            text: "Take control of your rover",
            requires_event: "cmd:PossessVessel",
            on_complete: |m| hint("Follow the beads. They mark a line the ДЗЗ team checked against the slope map — stay on it and you stay inside the rover's safe angle. Sanity-check as you go: if a bead sits on ground that looks too steep for YOUR rover, trust your eyes."),
        }),
        objective("reach_poi", #{
            text: "Follow the route down to the Point of Interest",
            requires: ["possess"],
            done: |m| { distance(find(rover), find(poi)) < 8.0 },
            dwell: 0.5,
            on_complete: |m| hint("At the POI. Now the second half — and the half teams forget: get back. Retrace the route to the NW start."),
        }),
        objective("return_home", #{
            text: "Bring the rover back to the NW start",
            requires: ["reach_poi"],
            done: |m| { arrived(find(rover), start, 12.0) },
            dwell: 0.5,
            on_complete: |m| notify_kind("There and back — a complete traverse.", "success"),
        }),
    ]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" {
        hint("You flew an imported route to the POI and back. Next: what happens on the floor when the link to Earth drops.");
        notify_kind("Route flown 🎉 — on to Radio-Shadow", "success");
    }
}

// DEBUG autopilot — possess and drive the route to the POI, then reverse it home.
fn task(me) {
    if !is_debug() { return; }
    let rover = "/Traverse/Rover";
    let r = route();
    seq([
        wait(1.0),
        once(|m| cmd("PossessVessel", #{ target: find(rover) })),
        wait(0.5),
        // Outbound: nav through each route point to the POI.
        step(|m| nav_to(find(rover), [r[1][0], 1.0, r[1][1]], 0.7, 10.0), |m| arrived(find(rover), [r[1][0], 1.0, r[1][1]], 10.0)),
        step(|m| nav_to(find(rover), [r[2][0], 1.0, r[2][1]], 0.7, 10.0), |m| arrived(find(rover), [r[2][0], 1.0, r[2][1]], 10.0)),
        step(|m| nav_to(find(rover), [r[3][0], 1.0, r[3][1]], 0.7, 10.0), |m| arrived(find(rover), [r[3][0], 1.0, r[3][1]], 10.0)),
        step(|m| nav_to(find(rover), [r[4][0], 1.0, r[4][1]], 0.5, 7.0),  |m| arrived(find(rover), [r[4][0], 1.0, r[4][1]], 7.0)),
        wait(1.0),
        // Return: reverse back to the start.
        step(|m| nav_to(find(rover), [r[2][0], 1.0, r[2][1]], 0.7, 10.0), |m| arrived(find(rover), [r[2][0], 1.0, r[2][1]], 10.0)),
        step(|m| nav_to(find(rover), [r[0][0], 1.0, r[0][1]], 0.7, 11.0), |m| arrived(find(rover), [r[0][0], 1.0, r[0][1]], 11.0)),
        #{ act: |m| brake(find(rover)), secs: 1.5 },
    ])
}
