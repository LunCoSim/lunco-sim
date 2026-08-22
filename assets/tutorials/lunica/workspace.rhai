//! Lesson 1 — Your Workspace. Where models live, and how to find examples.
//! Coach-mark tour authored in rhai; `coach_step` (HUD prelude) draws each step,
//! `on_event` advances the `this.i` cursor. No model needed.

fn steps() {
    [
        #{ anchor: "",                         focus: (),                     title: "Your Workspace",   body: "This lesson shows where your models live and how to find examples to learn from. ◀ ▶ or the arrow keys; Esc to leave." },
        #{ anchor: "panel.lunco.workbench.twin_browser", focus: "lunco.workbench.twin_browser",   title: "Twin Browser",     body: "A Twin is a folder of .mo files plus a manifest. Expand it, select a model, and open it in the model view; this browser is the source of your workspace documents." },
        #{ anchor: "panel.lunco.workbench.twin_browser", focus: "lunco.workbench.twin_browser",   title: "Read-only libraries", body: "Bundled demos and the Modelica Standard Library are read-only. Right-click a class and choose Duplicate when you need an editable copy in your Twin." },
        #{ anchor: "panel.modelica_welcome",   focus: "modelica_welcome",     title: "Learning Paths",   body: "Open a path such as Circuits, Control, or Mechanics, choose a class, and run it. Each step names a concrete signal or behavior to inspect." },
        #{ anchor: "",                         focus: (),                     title: "That's the map",   body: "Next: open a model and meet its four views. Continue when you're ready - or reopen any lesson from the Tutorials menu." },
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
