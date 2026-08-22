//! Lunica Overview — the first-run guided tour, authored in rhai.
//!
//! An ordinary tutorial scenario: `coach_step(steps, i)` renders each step's
//! spotlight + card, and `on_event` moves the `this.i` cursor on the card's
//! Back/Next/Skip/dot events — the SAME shared engine every tutorial uses.
//! Because a tutorial is just a script, you can edit this file and re-run it live
//! from the Tutorials menu (native reads it fresh; `RunScenario` hot-reloads).
//! On a first run the boot policy (`policy/boot.rhai`) starts this automatically.
//!
//! `steps()` is a FUNCTION (rhai named fns can't see script-level consts); `this`
//! is bound only in the hook the engine calls, not in helpers it calls.

fn steps() {
    [
        #{ anchor: "",                              focus: (),                       title: "Welcome to Lunica",     body: "Lunica is the LunCoSim Modelica workbench — model, compile, run, and plot dynamical systems. A 90-second tour. Use ◀ ▶ or the arrow keys; Esc to skip." },
        #{ anchor: "panel.lunco.workbench.twin_browser",      focus: "lunco.workbench.twin_browser",     title: "Twin Browser",          body: "Your workspace. A Twin is a folder of .mo files + a manifest. Bundled demos and the Modelica Standard Library are read-only — duplicate one to edit." },
        #{ anchor: "panel.modelica_welcome",        focus: "modelica_welcome",       title: "Learning Paths",        body: "The Welcome tab curates example arcs — Circuits, Control, Mechanics — each a ladder of MSL models with a goal. A great place to start exploring." },
        #{ anchor: "model_view.view_toggles",       focus: (),                       title: "Four Views of a Model", body: "Click Text, Diagram, Icon, and Docs. They are four projections of one model: the source, connected components, reusable symbol, and explanation." },
        #{ anchor: "model_view.compile_buttons",    focus: (),                       title: "Compile & Run",         body: "First click Compile to check the model. Then use Run for real-time stepping and live inputs, or Fast Run to integrate to t_end in the background and produce plots." },
        #{ anchor: "panel.modelica_plot",           focus: (),          title: "Graphs",                body: "Select a signal to trace, put fast and slow signals on appropriate axes, then snapshot the run so a later run can be overlaid. Drag Graphs beside the model if useful." },
        #{ anchor: "panel.modelica_experiments",    focus: "modelica_experiments",   title: "Experiments",           body: "Open Experiments, override a parameter without editing source, run once, then define a list of overrides for a reproducible sweep." },
        #{ anchor: "",                              focus: (),                       title: "Automate it",           body: "Open Help in the top menu for the command guidance. Every UI action is a typed Command. Drive Lunica from rhai scripts (like this tour!), the HTTP API, or an LLM agent over MCP." },
        #{ anchor: "",                              focus: (),                       title: "You're set",            body: "Reopen any lesson from the Tutorials menu, or press F1 for this overview. Pick a focused lesson next to go deeper." },
    ]
}

fn on_start(me) {
    this.i = 0;
    coach_step(steps(), 0);   // `coach_step` (HUD prelude) focuses the panel + draws the card
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext"      { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else if evt.name == "cmd:TutorialGoto" { this.i = evt.value; }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Tour complete!", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        coach_step(s, this.i);
    }
}
