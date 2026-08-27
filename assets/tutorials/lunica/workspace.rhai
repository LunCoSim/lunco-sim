//! Lesson 1 — Your Workspace. Where models live, and how to find examples.
//! Coach-mark tour authored in rhai; `coach_step` (HUD prelude) draws each step,
//! `on_event` advances the `this.i` cursor. No model needed.

fn steps() {
    [
        #{ anchor: "",                         focus: (),                     title: "Find your workspace",   body: "This lesson shows where models live and where to find examples. Press Next to continue." },
        #{ anchor: "panel.lunco.workbench.twin_browser", focus: "lunco.workbench.twin_browser",   title: "Open a Twin",     body: "In Twin Browser, expand a Twin and select a model. A Twin contains `.mo` files and a manifest." },
        #{ anchor: "panel.lunco.workbench.twin_browser", focus: "lunco.workbench.twin_browser",   title: "Duplicate before editing", body: "Bundled demos and the Modelica Standard Library are read-only. Right-click a class and choose Duplicate to make an editable copy." },
        #{ anchor: "panel.modelica_welcome",   focus: "modelica_welcome",     title: "Follow a Learning Path",   body: "Open Learning Paths and choose Circuits, Control, or Mechanics. Select a class and follow its signal or behaviour goal." },
        #{ anchor: "",                         focus: (),                     title: "Open a model next",   body: "Click Continue to learn the four model views." },
    ]
}

fn on_start(me) {
    this.i = 0;
    coach_step(steps(), 0);
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
        notify_kind("Lesson complete!", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        coach_step(s, this.i);
    }
}
