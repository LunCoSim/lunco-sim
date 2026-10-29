// @timing presentation
//! View, Build, Analyze, Editor & Lunica — choose the workbench layout for the task at hand.
//!
//! This is a UI-only tour. It deliberately has no scene payload and no
//! authored perspective requirement: the learner must be able to switch
//! between all five perspectives while the cards remain active.

fn steps() {
    [
        #{ anchor: "menu.perspective.sandbox_view", title: "Choose your workbench", body: "The highlighted title-bar tab is ◉ View. The five task tabs are ◉ View, ⚒ Build, ↗ Analyze, ✎ Editor, and ∿ Lunica.\n\nClick the tab named in each of the next five cards. Then use Next to continue." },
        #{ anchor: "menu.perspective.sandbox_view", title: "◉ View — observe and operate", body: "The highlighted tab is ◉ View. The View workspace fills the window and most docks disappear. A visual scene appears here when this Twin has one.\n\nUse View to watch a mission, move the camera, or drive a vessel." },
        #{ anchor: "menu.perspective.rover_build", title: "⚒ Build — author the scene", body: "Click the highlighted ⚒ Build tab. Entities and Twin sources appear on the left; Inspector, Environment and Spawn appear on the right.\n\nUse Build to place objects, select them, and inspect or edit the USD-authored scene." },
        #{ anchor: "menu.perspective.analysis", title: "↗ Analyze — compare signals", body: "Click the highlighted ↗ Analyze tab. Telemetry and Entities appear on the left, Graphs in the center and Ports on the right.\n\nCheck a telemetry channel to plot it. Compare curves, inspect units and current values, then return to the same running Twin in View." },
        #{ anchor: "menu.perspective.editor", title: "✎ Editor — work on an assembly", body: "Click the highlighted ✎ Editor tab. Open an assembly from the Twin browser, browse its USD prim tree, and inspect or edit the selected part.\n\nEditor keeps authored-document edits together with the isolated USD preview." },
        #{ anchor: "menu.perspective.modelica_analyze", title: "∿ Lunica — engineer the model", body: "Click the highlighted ∿ Lunica tab. The Twin browser and Modelica workbench replace the scene-authoring docks.\n\nUse Lunica to open, edit, compile, run, and plot Modelica models." },
        #{ anchor: "menu.perspective.sandbox_view", title: "One workflow, five perspectives", body: "Click the highlighted ◉ View tab, then use Next. A common workflow is ⚒ Build the scene, ✎ Editor assemblies, ∿ Lunica continuous models, ↗ Analyze signals, and ◉ View the running mission.\n\nSwitching perspectives keeps the same Twin and simulation; it only changes the tools on screen." },
        #{ anchor: "", title: "Choose by intent", body: "Use View for seeing and operating. Use Build for scene composition. Use Analyze for telemetry, plots and current port values. Use ✎ Editor to inspect, select, and edit USD assemblies and parts. Use Lunica for Modelica equations, parameters, and results.\n\nEnter Editor from the ✎ Editor perspective tab in the title bar. View → Perspectives offers the same task choices. View → Analyze contains optional analysis panels, and Help → Perspective help explains each task. Twins can ship additional task layouts assembled in Rhai." },
    ]
}

fn on_start(me, ctx) {
    this.i = 0;
    coach_step(steps(), 0);
    notify_kind("Tutorial: View, Build, Analyze, Editor & Lunica", "info");
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
        notify_kind("Perspective tour complete — next is Authority Handoff", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        coach_step(s, this.i);
    }
}
