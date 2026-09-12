// ── Navigation (generic bridge plus authored Modelica setpoints) ────────────
//
// Rhai owns mission composition and waypoint selection. `nav_to` adapts the
// generic navigation bridge for subjects that expose that capability; route
// programs for authored Modelica drive laws use `modelica_waypoint` so the
// continuous controller remains the owner's responsibility.

const NAV_DEFAULT_SPEED  = 0.6;
const NAV_DEFAULT_RADIUS = 3.0;

/// Publish one world waypoint to the subject's authored Modelica guidance law.
/// This is a target setpoint, not a direct actuator calculation.
fn modelica_waypoint(vessel, target, speed, radius, turn_only) {
    let s = if speed == () { NAV_DEFAULT_SPEED } else { speed };
    let r = if radius == () { NAV_DEFAULT_RADIUS } else { radius };
    let t = if turn_only == () { 0.0 } else { turn_only };
    cmd("SetPorts", #{ target: vessel,
        writes: [
            ["autopilot_enable", 1.0],
            ["autopilot_target_x", target[0]],
            ["autopilot_target_z", target[2]],
            ["autopilot_speed", s],
            ["autopilot_radius", r],
            ["autopilot_turn_only", t],
            ["throttle", 0.0],
            ["steer", 0.0],
            ["brake", 0.0],
        ], seq: 0, tick: 0 })
}

/// Disable waypoint guidance and hold the subject's authored brake.
fn modelica_waypoint_stop(vessel) {
    cmd("SetPorts", #{ target: vessel,
        writes: [
            ["autopilot_enable", 0.0],
            ["throttle", 0.0],
            ["steer", 0.0],
            ["brake", 1.0],
        ], seq: 0, tick: 0 });
}

/// Drive `entity` toward world point `target` at `speed`. Returns true once
/// within `radius` and brakes. `nav_command` returns `()` when the authoritative
/// pose or authored steering capability is unavailable; that is a visible safe
/// stop, not a guessed vehicle class or a legacy steering fallback.
fn nav_to(entity, target, speed, radius) {
    let command = nav_command(entity, target, speed, radius);
    if command == () {
        brake(entity);
        return false;
    }
    if command.arrived || command.brake > 0.5 {
        brake(entity);
        return command.arrived;
    }
    drive(entity, command.throttle, command.steer);
    false
}
