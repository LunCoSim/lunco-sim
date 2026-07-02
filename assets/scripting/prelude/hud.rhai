// ── Tutorial HUD: PERSISTENT instructions/objectives + widget spotlight ──────
//
// Unlike notify() toasts (which fade after a few seconds), these stay on screen
// until changed or cleared — so a tutorial can hold a hint up while the user
// acts, show a live objectives checklist, and point at a workbench widget.
// Rendered by lunco-workbench's tutorial_overlay (sandbox + lunica both load it).

/// Set the persistent one-line hint at the top of the HUD.
fn hint(msg)                   { cmd("SetHint", #{ text: msg }) }
/// Clear the hint line.
fn clear_hint()                { cmd("SetHint", #{ text: "" }) }
/// Spotlight a workbench widget by its HelpAnchors key, with a caption.
fn spotlight(anchor, caption)  { cmd("Spotlight", #{ anchor: anchor, text: caption }) }
/// Remove any active spotlight.
fn clear_spotlight()           { cmd("ClearSpotlight") }

// Guided TOUR (lunica-style coach marks). `coach(...)` shows one step — spotlight
// `anchor` + a card with title/body, progress dots (index/total), and Back/Next/
// Skip controls. The controls emit `cmd:TutorialNext`/`cmd:TutorialBack`/
// `cmd:TutorialSkip` on the bus; a tour reacts in `on_event` (see the
// tutorials/*/tour.rhai step-cursor pattern). `end_tour()` hides the coach.
fn coach(index, total, anchor, title, body) {
    cmd("SetTourStep", #{ index: index, total: total, anchor: anchor, title: title, body: body });
}
fn end_tour()                  { cmd("ClearTour") }

/// Glyph for an objective state: pending •, active ▸, complete ✓, failed ✗.
fn __obj_glyph(state) {
    if state == "complete" { "✓" }
    else if state == "failed" { "✗" }
    else if state == "active" { "▸" }
    else { "•" }
}

/// Publish an objectives checklist to the HUD. `list` is an array of
/// `#{ text, state }` (state ∈ "pending"|"active"|"complete"|"failed"). Use this
/// for hand-driven step lists; a declarative `mission(me)` publishes itself.
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

/// Enable/disable a simulation subsystem at runtime (progressive fidelity ramp).
/// `name` ∈ "thermal" | "comms-degradation" | "obstacle-field" (see lunco-core).
fn set_subsystem(name, on)     { cmd("SetSubsystemEnabled", #{ name: name, on: on }) }

// Event-source helpers. `evt.source` is the EMITTER's gid — which sensor/script
// fired — so ONE `on_event` can branch per source (e.g. many checkpoints all
// named `enter:checkpoint`, different behaviour each). `0` = global (raw input).
fn emitter(evt)                { evt.source }
fn from_entity(evt, id)        { evt.source == id }

/// Fire a command and return JUST its assigned data (the `.data` bag), or () if
/// it failed or reported none. The create-then-manipulate shortcut:
///   let id = cmd_data("SpawnThing", #{...}); if id != () { set_property(id, ...) }
fn cmd_data(name, params) {
    let r = cmd(name, params);
    if r.ok { r.data } else { () }
}

