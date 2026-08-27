//! View, Build & Lunica — choose the workbench layout for the task at hand.
//!
//! This is a UI-only tour. It deliberately has no scene payload and no
//! authored perspective requirement: the learner must be able to switch
//! between all three perspectives while the cards remain active.

fn steps() {
    [
        #{ anchor: "", title: "Choose your workbench", body: "The title bar has three perspective tabs: ◉ View, ⚒ Build, and ∿ Lunica.\n\nClick the tab named in each of the next three cards. Then use Next to continue." },
        #{ anchor: "", title: "◉ View — observe and operate", body: "Click ◉ View now. The 3D scene fills the window and most docks disappear.\n\nUse View to watch a mission, move the camera, or drive a vessel." },
        #{ anchor: "", title: "⚒ Build — author the scene", body: "Click ⚒ Build now. The Entities and Telemetry panels appear on the left; Inspector and Spawn appear on the right.\n\nUse Build to place objects, select them, and inspect or edit the USD-authored scene." },
        #{ anchor: "", title: "∿ Lunica — engineer the model", body: "Click ∿ Lunica now. The Twin Browser and Modelica workbench replace the scene-authoring docks.\n\nUse Lunica to open, edit, compile, run, and plot Modelica models." },
        #{ anchor: "", title: "One workflow, three perspectives", body: "Click ◉ View, then use Next. A common workflow is ⚒ Build the scene, ∿ Lunica the continuous model, and ◉ View the running mission.\n\nSwitching perspectives keeps the same Twin and simulation; it only changes the tools on screen." },
        #{ anchor: "", title: "Choose by intent", body: "Use View for seeing and operating. Use Build for the USD scene. Use Lunica for Modelica equations, parameters, and results.\n\nThese tabs choose the workbench layout. The top View menu and the 3D camera controls are separate tools." },
    ]
}

fn on_start(me) {
    this.i = 0;
    coach_step(steps(), 0);
    notify_kind("Tutorial: View, Build & Lunica", "info");
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
        notify_kind("Perspective tour complete — next is Authority Handoff", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        coach_step(s, this.i);
    }
}
