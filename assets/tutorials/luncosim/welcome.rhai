//! LunCoSim UI orientation — a short guided tour of the simulator workbench.
//!
//! Every anchored card points at a real HelpAnchors rect published by the
//! current workbench. The payload supplies a live rover scene so the panels
//! have meaningful content while the learner looks around.

fn steps() {
    [
        #{ anchor: "", focus: (), title: "Simulation time", body: "Click the pause button beside Time to stop the simulation while you inspect the scene. Click the play button in the same spot when you want physics to advance again." },
        #{ anchor: "panel.entity_list", focus: "entity_list", title: "Find the rover", body: "Open Entities in the left dock. Click the rover row to select it; expand the row to see its child parts." },
        #{ anchor: "panel.telemetry_browser", focus: "telemetry_browser", title: "Read live state", body: "Open Telemetry in the left dock, then drag a rover speed or wheel signal from it into the Graphs window at the bottom to plot the signal as the simulation runs." },
        #{ anchor: "panel.sandbox_inspector", focus: "sandbox_inspector", title: "Inspect the selection", body: "Select the model in the scene. The Inspector in the right dock then shows the selected model's authored properties and current state." },
        #{ anchor: "panel.command_deck", focus: "command_deck", title: "Check control authority", body: "Open Command Deck in the right dock. It tells you whether the rover is free, human-driven, or on autopilot." },
        #{ anchor: "", focus: (), title: "Next: choose a perspective", body: "The next lesson explains ◉ View, ⚒ Build, and ∿ Lunica.\n\nClick Next in the Tutorial complete dialog when you are ready." },
    ]
}

fn on_start(me, ctx) {
    this.i = 0;
    coach_step(steps(), 0);
    notify_kind("Tutorial: Read the Sim UI", "info");
}

fn on_event(me, evt, ctx) {
    let s = steps();
    if evt.name == "cmd:GuidedNext" { this.i += 1; }
    else if evt.name == "cmd:GuidedBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:GuidedSkip" { this.i = s.len(); }
    else if evt.name == "cmd:GuidedGoto" { this.i = evt.value; }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("UI review complete — next is View, Build, Editor & Lunica", "success");
        emit("MISSION_COMPLETE", 0);
    } else { coach_step(s, this.i); }
}
