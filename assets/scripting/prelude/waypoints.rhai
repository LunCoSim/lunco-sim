// ── Waypoint state presentation ────────────────────────────────────────────
//
// The engine emits a typed waypoint.reached payload. The marker's authored
// USD API supplies the inactive color; this helper applies that policy through
// the runtime USD layer. No mission path or waypoint count belongs here.

/// Return the composed marker path from a waypoint arrival event.
fn waypoint_event_path(evt) {
    if evt.name != "waypoint.reached" || evt.value == () { return (); }
    if evt.value.path == () || evt.value.state != "reached" { return (); }
    evt.value.path
}

/// Return the ordered route index from a waypoint arrival event.
fn waypoint_event_index(evt) {
    if evt.name != "waypoint.reached" || evt.value == () { return (); }
    if evt.value.state != "reached" || evt.value.index == () || evt.value.index < 0 { return (); }
    evt.value.index
}

fn waypoint_color_literal(value) {
    "(" + value[0] + ", " + value[1] + ", " + value[2] + ")"
}

/// Apply the authored inactive look to one reached waypoint.
///
/// The helper resolves the composed material binding, so a referenced waypoint
/// remains a referenced asset and only the session runtime layer changes. It
/// returns false when the event payload or authored USD contract is invalid.
fn apply_waypoint_reached(evt) {
    let path = waypoint_event_path(evt);
    if path == () { return false; }

    let marker = query("QueryUsdPrim", #{
        path: path,
        attrs: ["lunco:waypoint:inactiveColor"],
        children: true,
    });
    if marker == () || marker.attrs == () { return false; }
    let color = marker.attrs["lunco:waypoint:inactiveColor"];
    if color == () || color.len() != 3 { return false; }

    let visual_path = ();
    let visual = ();
    if marker.children == () { return false; }
    for child in marker.children {
        let candidate = query("QueryUsdPrim", #{
            path: child,
            rels: ["material:binding"],
        });
        if candidate != () && candidate.relationships != ()
            && candidate.relationships["material:binding"] != ()
            && candidate.relationships["material:binding"].len() == 1 {
            visual_path = child;
            visual = candidate;
            break;
        }
    }
    if visual_path == () || visual == () {
        return false;
    }

    let color_literal = waypoint_color_literal(color);
    let display = cmd("SetUsdAttribute", #{
        path: visual_path,
        name: "primvars:displayColor",
        type_name: "color3f[]",
        value: "[" + color_literal + "]",
    });
    let material = visual.relationships["material:binding"][0];
    let material_info = query("QueryUsdPrim", #{
        path: material,
        children: true,
    });
    if material_info == () || material_info.children == () { return false; }
    let shader_path = ();
    for child in material_info.children {
        let shader = query("QueryUsdPrim", #{
            path: child,
            attrs: ["info:id"],
        });
        if shader != () && shader.attrs != ()
            && shader.attrs["info:id"] == "UsdPreviewSurface" {
            shader_path = child;
            break;
        }
    }
    if shader_path == () { return false; }
    let emission = cmd("SetUsdAttribute", #{
        path: shader_path,
        name: "inputs:emissiveColor",
        type_name: "color3f",
        value: color_literal,
    });
    display.ok == true && emission.ok == true
}
