// ── Tutorial HUD: PERSISTENT instructions/objectives + widget spotlight ──────
//
// Unlike notify() toasts (which fade after a few seconds), these stay on screen
// until changed or cleared — so a tutorial can hold a hint up while the user
// acts, show a live objectives checklist, and point at a workbench widget.
// Rendered by the optional lunco-workbench-guided-ui adapter (sandbox + lunica
// both load it).

/// Set the persistent one-line hint at the top of the HUD.
fn hint(msg)                   { cmd("SetHint", #{ text: msg }) }
/// Clear the hint line.
fn clear_hint()                { cmd("SetHint", #{ text: "" }) }
/// Resolve a user-facing control label for tutorial copy. An unbound intent is
/// explicit state, not a hardcoded key or a guessed default.
fn input_hint(binding)          {
    let label = input_binding(binding);
    if label == () { "unbound" } else { label }
}
/// Focus a singleton workbench panel when an interactive host exists.
/// Headless lesson gates verify policy and state without a panel to focus.
fn focus_panel(id)              { if !is_unattended() { cmd("FocusPanel", #{ id: id }) } }
/// Spotlight a workbench widget by its HelpAnchors key, with a caption.
fn spotlight(anchor, caption)  { cmd("Spotlight", #{ anchor: anchor, text: caption }) }
/// Remove any active spotlight.
fn clear_spotlight()           { cmd("ClearSpotlight") }

// Guided TOUR (lunica-style coach marks). `coach(...)` shows one step — spotlight
// `anchor` + a card with title/body, progress dots (index/total), and Back/Next/
// Skip controls. The controls emit `cmd:GuidedNext`/`cmd:GuidedBack`/
// `cmd:GuidedSkip` on the bus; a tour reacts in `on_event` (see the
// tutorials/*/tour.rhai step-cursor pattern). Action policy belongs in that
// Rhai event handler, not in this presentation command. `end_tour()` hides
// the coach.
fn coach(index, total, anchor, title, body) {
    cmd("SetTourStep", #{ index: index, total: total, anchor: anchor, title: title, body: body });
}
fn end_tour()                  { cmd("ClearTour") }

/// Render step `i` of a `steps` array of `#{ anchor, title, body, focus }`:
/// focus the named singleton panel (if `focus` is set) so its spotlight anchor
/// is on screen, then draw the coach card. Pairs with the standard on_event
/// cursor pattern (`this.i`) so a whole guided tour is just a `steps()` table +
/// two thin hooks. `focus` is optional — omit it (or set `()`), and no panel is
/// touched.
fn coach_step(steps, i) {
    let s = steps[i];
    // A headless scene test has no workbench panel to focus. The lesson still
    // renders and its event policy remains testable; focus is presentation
    // only, so it is omitted when no interactive host exists.
    if s.focus != () { focus_panel(s.focus); }
    coach(i, steps.len(), s.anchor, s.title, s.body);
}

/// Glyph for an objective state: pending •, active ▸, complete ✓, failed ✗.
fn __obj_glyph(state) {
    if state == "complete" { "✓" }
    else if state == "failed" { "✗" }
    else if state == "active" { "▸" }
    else { "•" }
}

/// Publish an objectives checklist to the HUD. `list` is an array of
/// `#{ text, state }` (state ∈ "pending"|"active"|"complete"|"failed"). Use this
/// for hand-driven step lists; a declarative `mission(me, ctx)` publishes itself.
fn objectives_hud(list) {
    let out = "";
    for o in list {
        let txt = if o.text != () { o.text } else { o.id };
        let st  = if o.state != () { o.state } else { "pending" };
        if out != "" { out += "\n"; }
        out += __obj_glyph(st) + " " + txt;
    }
    cmd("SetObjectives", #{ text: out });
}
/// Clear the objectives checklist.
fn clear_objectives()          { cmd("SetObjectives", #{ text: "" }) }

/// Replace the guided HUD action row. Each `actions` entry has `id`, `label`,
/// and `enabled`; button presses dispatch the id to one registered one-arg
/// Rhai tool hook. An empty list removes the row.
fn guided_hud_actions(tool, hook, actions) {
    cmd("SetGuidedHudActions", #{ tool: tool, hook: hook, actions: actions })
}
/// Clear the guided HUD action row.
fn clear_guided_hud_actions() { guided_hud_actions("", "", []) }

/// Per-Twin runtime camera HUD policy. The surface is visible by default when
/// the key is absent; projects and tutorials can opt out without a Rust field.
fn camera_status_visible()     {
    let value = get_twin_setting("ui.camera_status");
    if value == () { true } else { value == true }
}
fn show_camera_status(on)      { set_twin_setting("ui.camera_status", on) }

/// Read the current engine camera fact for authored Rhai policy. Selection is
/// still done with `set_camera(name)` from control.rhai; this helper never
/// guesses a camera from an entity name.
fn current_camera_name() {
    let name = get_exposure("camera-status", "active_name");
    if name == () || name == "" { "NO ACTIVE CAMERA" } else { name }
}

/// Convert an inertial mission-site direction to orbital camera angles.
/// Rhai's two-argument atan(y, x) preserves the quadrant like atan2.
fn mission_site_camera_angles(position) {
    if position == () || position.len() != 3 { return (); }
    let x = position[0];
    let y = position[1];
    let z = position[2];
    if !x.is_finite() || !y.is_finite() || !z.is_finite() { return (); }
    let radius_squared = x * x + y * y + z * z;
    if !radius_squared.is_finite() || radius_squared <= 0.0 { return (); }
    #{
        yaw_rad: atan(x, z),
        pitch_rad: -atan(y, sqrt(x * x + z * z))
    }
}

/// Read the persisted default duration for orbit-direction presentation moves.
fn mission_site_camera_animation_duration_s() {
    let duration_s = get_setting("CameraInputSettings.orbit_direction_animation_duration_s");
    if duration_s == () || type_of(duration_s) != "f64" || !duration_s.is_finite() || duration_s <= 0.0 {
        return ();
    }
    duration_s
}

/// Dispatch the authored camera-surface actions through the normal typed
/// command bridge. The runtime UI layer only transports the action string; it
/// does not know that these records happen to represent cameras.
fn handle_camera_ui_action(action) {
    let prefix = "camera.select.path.";
    if action.starts_with(prefix) {
        let result = cmd("SetUserCamera", #{ name: action.sub_string(prefix.len()) });
        if result == () || result.ok != true {
            notify_kind("Camera selection failed", "error");
        }
        return true;
    }
    if action == "camera.observe.avatar" {
        let result = cmd("ObserveAvatar", #{});
        if result == () || result.ok != true {
            notify_kind("Avatar camera is unavailable", "error");
        }
        return true;
    }
    if action == "camera.resume.director" {
        let result = cmd("ResumeCameraDirector", #{});
        if result == () || result.ok != true {
            notify_kind("Authored camera director is unavailable", "error");
        }
        return true;
    }
    if action == "camera.focus.mission-site" {
        let position = get_exposure("celestial-view", "site_orbit_position_m");
        if position == () || position.len() != 3 {
            notify_kind("The lunar mission location is unavailable", "warning");
            return true;
        }
        let angles = mission_site_camera_angles(position);
        if angles == () {
            notify_kind("The lunar mission location is invalid", "error");
            return true;
        }
        let duration_s = mission_site_camera_animation_duration_s();
        if duration_s == () {
            notify_kind("The orbit direction animation setting is invalid", "error");
            return true;
        }
        // Rhai chooses the target direction and animation duration. The
        // generic camera command preserves the current orbital radius.
        let result = cmd("AnimateOrbitCameraDirection", #{
            yaw_rad: angles.yaw_rad,
            pitch_rad: angles.pitch_rad,
            duration_s: duration_s
        });
        if result == () || result.ok != true {
            notify_kind("Could not frame the lunar mission location", "error");
        }
        return true;
    }
    false
}

/// Enable/disable a simulation subsystem at runtime (progressive fidelity ramp).
/// `name` ∈ "thermal" | "comms-degradation" | "obstacle-field" (see lunco-core).
fn set_subsystem(name, on)     { cmd("SetSubsystemEnabled", #{ name: name, on: on }) }

// Event-source helpers. `evt.source` is the EMITTER's gid — which sensor/script
// fired — so ONE `on_event` can branch per source (e.g. many checkpoints all
// named `enter:checkpoint`, different behaviour each). `0` = global (raw input).
fn emitter(evt)                { evt.source }
fn from_entity(evt, id)        { evt.source == id }

/// Fire a command and return JUST its result data (the `.data` bag), or () if
/// it failed or reported none. The create-then-manipulate shortcut:
///   let id = cmd_data("SpawnThing", #{...}); if id != () { set_property(id, ...) }
fn cmd_data(name, params) {
    let r = cmd(name, params);
    if r.ok { r.data } else { () }
}
