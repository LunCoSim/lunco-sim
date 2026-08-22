//! Script a Rover — a guided tour of behaviour scripting (the author-scenario skill).
//!
//! Coach-mark tour. Explains what a scenario is and how to attach one, using
//! this very tutorial as the live example — a tutorial *is* a rhai scenario.

fn steps() {
    [
        #{ anchor: "",                   title: "Script a Rover",      body: "Behaviour in LunCoSim is rhai — hot-reloadable scripts attached to entities. This tutorial you're reading is itself one. Next / Back below." },
        #{ anchor: "",                   title: "Event-driven lifecycle", body: "A scenario uses on_start for setup, on_event for reactions, and declarative mission/task primitives for progression. Production scenarios do not install an on_tick control loop. Per-entity state lives on `this`." },
        #{ anchor: "",                   title: "High-level verbs",    body: "You don't hand-code continuous control loops. Modelica owns continuous equations; Rhai supplies policy and sequencing through verbs such as nav_to, arrived, emit, and mission objectives." },
        #{ anchor: "panel.side_browser", title: "Attach & run",        body: "Attach a script by adding a `Scope` child with `prepend apiSchemas = [\"LunCoProgramAPI\"]` and `uniform asset info:sourceAsset = @lunco://scenarios/patrol.rhai@`; delete the scope and the behaviour goes with it. Per-instance settings are authored as `lunco:param:*` attributes. Or run one live through RunScenario; re-running hot-reloads it without a rebuild." },
        #{ anchor: "panel.side_browser", title: "Watch it work",       body: "Read the script's live output and telemetry in the workbench status and telemetry surfaces. Scripts sense and command the world through the SAME API the UI and AI agents use." },
        #{ anchor: "",                   title: "Go deeper",           body: "See the Scripting Guide and the author-scenario skill for the full verb surface, sequencing, and mission objectives." },
    ]
}

fn show(s, i) { let step = s[i]; coach(i, s.len(), step.anchor, step.title, step.body); }

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Script a Rover", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Script a Rover complete", "success");
        emit("MISSION_COMPLETE", 0);
    } else { show(s, this.i); }
}
