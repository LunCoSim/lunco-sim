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

/// Apply the authored inactive look to one reached waypoint.
///
/// The USD simulation projection owns the runtime-only look transition. This
/// helper validates the typed event for callers but deliberately does not issue
/// `SetUsdAttribute`: authoring a material while a mission is running would
/// trigger a scene-wide visual refresh and tear down active co-simulation.
fn apply_waypoint_reached(evt) {
    let path = waypoint_event_path(evt);
    path != ()
}
