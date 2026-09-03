//! Inspect the Simulation — a guided tour of reading live state (the inspect-simulation skill).
//!
//! Coach-mark tour. Teaches selection → inspector, telemetry ports and plots,
//! and reading it all back over the API without touching the log.

fn steps() {
    [
        #{ anchor: "",                      title: "Inspect live state",   body: "This tour shows where to read pose, telemetry, and Modelica values. Press Next to move on; press Back to review." },
        #{ anchor: "panel.center",          title: "Select an object",      body: "Select an object in Entities or the USD prim tree. Its details become available in the other panels." },
        #{ anchor: "panel.sandbox_inspector", title: "Read Inspector",        body: "Open Inspector to read the selected object's pose, components, and authored properties." },
        #{ anchor: "panel.telemetry_browser", title: "Read telemetry",        body: "Open Telemetry in the left dock. Select a port such as throttle, altitude, or battery to read its live value." },
        #{ anchor: "",                      title: "Read state from code",  body: "Call `ListEntities` to get an entity's `api_id`. Use `ListPorts` to find its ports, then `ReadPorts` to read values. Use `CosimStatus` and `GetBrokenConnections` to inspect the Modelica link." },
        #{ anchor: "",                      title: "Inspect, then act",     body: "Use selection, Inspector, and Telemetry to answer whether the simulation is working. Click Continue to return to the tutorial list." },
    ]
}

fn show(s, i) { let step = s[i]; coach(i, s.len(), step.anchor, step.title, step.body); }

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Inspect the Simulation", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Inspect the Simulation complete", "success");
        emit("MISSION_COMPLETE", 0);
    } else { show(s, this.i); }
}
