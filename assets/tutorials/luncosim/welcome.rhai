//! LunCoSim UI review — a guided coach-mark tour of the simulator workbench.
//!
//! Every card points at a real HelpAnchors rect published by the current
//! workbench. The payload supplies a live rover scene so the panels have
//! meaningful content while the learner looks around.

fn steps() {
    [
        #{ anchor: "", focus: (), title: "Mission time", body: "Open Time in the menu bar. Time controls are global: pause before inspecting a scene, resume when you want physics to advance, and change rate only when the experiment calls for it." },
        #{ anchor: "panel.side_browser", focus: (), title: "Entities", body: "Use the left dock's Entities panel as the authored scene hierarchy. Select the rover here when the viewport is busy; expanding a row reveals its meaningful child parts." },
        #{ anchor: "panel.side_browser", focus: (), title: "Telemetry", body: "Use the left dock's Telemetry tab for live state, not a screenshot: browse the rover's exposed ports and choose signals to inspect or plot." },
        #{ anchor: "panel.right_inspector", focus: (), title: "Inspector", body: "Use the right dock's Inspector to review authored properties for the selected prim. This is where you review what the scene says, rather than guessing from the render." },
        #{ anchor: "panel.right_inspector", focus: "command_deck", title: "Command Deck", body: "Open Command Deck from the right dock. It reports whether the rover is free, human-driven, or on autopilot, and exposes the authority handoff." },
        #{ anchor: "panel.right_inspector", focus: (), title: "Spawn", body: "Use the right dock's Spawn panel as an authoring action: choose a catalog entry, then click the scene to place it. " + input_hint("cancel") + " cancels placement." },
        #{ anchor: "", focus: (), title: "One command surface", body: "The workbench buttons, Rhai scripts, HTTP API and agents use the same typed command path. The UI is a client of the simulation, not a second control system." },
        #{ anchor: "", focus: (), title: "Ready for mechanics", body: "You have seen where to read scene facts, live signals, and control authority. Next, click the rover, click Possess in the Inspector or Command Deck, then use Release or " + input_hint("cancel") + " to perform the handoff yourself." },
    ]
}

fn on_start(me) {
    this.i = 0;
    coach_step(steps(), 0);
    notify_kind("Tutorial: Read the Sim UI", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else if evt.name == "cmd:TutorialGoto" { this.i = evt.value; }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("UI review complete — next is Authority Handoff", "success");
        emit("MISSION_COMPLETE", 0);
    } else { coach_step(s, this.i); }
}
