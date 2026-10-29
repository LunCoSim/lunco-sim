// @timing presentation
//! Inspect the Simulation — a guided tour of reading live state (the inspect-simulation skill).
//!
//! Coach-mark tour. Teaches selection → inspector, telemetry ports and plots,
//! and reading it all back over the API without touching the log.

fn steps() {
    [
        #{ anchor: "",                      title: "Inspect live state",   body: "This tour shows where to read pose, telemetry, and Modelica values. Press Next to move on; press Back to review." },
        #{ anchor: "panel.entity_list", focus: "entity_list", title: "Select an object",      body: "Select an object in the Entities panel. Its details become available in the other panels." },
        #{ anchor: "panel.sandbox_inspector", focus: "sandbox_inspector", title: "Read Inspector",        body: "Open Inspector to read the selected object's pose, components, and authored properties." },
        #{ anchor: "panel.telemetry_browser", focus: "telemetry_browser", title: "Read telemetry",        body: "Use Analyze to open Telemetry beside Graphs. Select a port such as throttle, altitude, or battery to read its live value." },
        #{ anchor: "",                      title: "Read state from code",  body: "Call `ListEntities` to get an entity's `api_id`. Use `ListPorts` to discover exact names, then pass selected names in `ReadPorts`'s required `port_names` array. Use `CosimStatus` and `GetBrokenConnections` to inspect the Modelica link." },
        #{ anchor: "",                      title: "Inspect, then act",     body: "Use selection, Inspector, and Telemetry to answer whether the simulation is working. Choose another lesson from Tutorials after finishing this tour." },
    ]
}

fn show(s, i) { coach_step(s, i); }

fn on_start(me, ctx) {
    // The optional world is selected by the catalog and mounted by the generic
    // launcher before this runs.
    cmd("ActivatePerspective", #{ id: "analysis" });
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Inspect the Simulation", "info");
}

fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    this = tutorials::tour_event(this, evt, steps(), ctx);
}
