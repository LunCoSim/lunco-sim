// ── Navigation (one host-owned command law) ─────────────────────────────────
//
// Rhai owns mission composition, while the language-neutral navigation bridge
// owns pose selection, authored steering capability, heading recovery,
// Ackermann travel state, arrival braking, and invalid-input handling. Keeping
// that law out of this prelude means native behaviour trees and every scripted
// mission cannot drift into different steering conventions.

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
