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
        #{ anchor: "",                              focus: (),                       title: "Welcome to Lunica",     body: "Lunica is the Modelica workbench. Use it to model, compile, run, and plot dynamical systems. Press Next to begin." },
        #{ anchor: "panel.lunco.workbench.twin_browser",      focus: "lunco.workbench.twin_browser",     title: "Find a model",           body: "Open Twin Browser and select a model. A Twin contains `.mo` files and a manifest. Duplicate read-only demos before editing them." },
        #{ anchor: "panel.modelica_welcome",        focus: "modelica_welcome",       title: "Start with a path",       body: "Open Learning Paths. Choose Circuits, Control, or Mechanics, then select a class to see its goal and example." },
        #{ anchor: "model_view.view_toggles",       focus: (),                       title: "View one model four ways", body: "Click Text, Diagram, Icon, and Docs. Text is the source; Diagram shows connections; Icon is the reusable symbol; Docs explains the model." },
        #{ anchor: "model_view.compile_buttons",    focus: (),                       title: "Compile, then run",       body: "Click Compile to check the model. Then click Run for live stepping, or Fast Run to integrate to `t_end` and produce plots." },
        #{ anchor: "panel.modelica_plot",           focus: (),          title: "Read a graph",                body: "Select a signal in Graphs. Snapshot the run, change the setup, and overlay the next run to compare results." },
        #{ anchor: "panel.modelica_experiments",    focus: "modelica_experiments",   title: "Test a parameter",           body: "Open Experiments. Override a parameter, run once, then define several values for a reproducible sweep." },
        #{ anchor: "",                              focus: (),                       title: "Automate the work",           body: "Open Help for command guidance. Rhai scripts, the HTTP API, and MCP clients can send the same typed commands as the UI." },
        #{ anchor: "",                              focus: (),                       title: "Choose your next lesson",            body: "Reopen this tour from Tutorials, or choose a focused lesson to practise one part of the workflow." },
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
