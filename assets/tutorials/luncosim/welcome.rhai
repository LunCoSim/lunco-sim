//! LunCoSim UI orientation — a short guided tour of the simulator workbench.
//!
//! Every anchored card points at a real HelpAnchors rect published by the
//! current workbench. The payload supplies a live rover scene so the panels
//! have meaningful content while the learner looks around.

fn steps() {
    [
        #{ anchor: "", focus: (), title: "Simulation time", body: "Open Time in the menu bar. Click Pause before you inspect the scene; click Resume when you want physics to advance." },
        #{ anchor: "panel.side_browser", focus: (), title: "Find the rover", body: "Open Entities in the left dock. Click the rover row to select it; expand the row to see its child parts." },
        #{ anchor: "panel.side_browser", focus: (), title: "Read live state", body: "Open Telemetry in the left dock. Select the rover's speed or wheel signal to inspect it while the simulation runs." },
        #{ anchor: "panel.right_inspector", focus: (), title: "Inspect the selection", body: "Look at Inspector in the right dock. It shows the selected prim's authored properties and current state." },
        #{ anchor: "panel.right_inspector", focus: "command_deck", title: "Check control authority", body: "Open Command Deck in the right dock. It tells you whether the rover is free, human-driven, or on autopilot." },
        #{ anchor: "", focus: (), title: "Next: choose a perspective", body: "The next lesson explains ◉ View, ⚒ Build, and ∿ Lunica.

Click Next in the Tutorial complete dialog when you are ready." },
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
        notify_kind("UI review complete — next is View, Build & Lunica", "success");
        emit("MISSION_COMPLETE", 0);
    } else { coach_step(s, this.i); }
}
