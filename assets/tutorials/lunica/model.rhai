//! Lesson 2 — Open & View a Model. The four views + graphical composition.
//! Opens the bundled `CascadedRCFilter` demo so the model-view steps have real
//! content to spotlight.

fn steps() {
    [
        #{ anchor: "",                                  focus: (),                            title: "Open & View a Model", body: "We opened the CascadedRCFilter demo for you. Let's look at the ways to view and edit a Modelica model. ◀ ▶ or arrow keys." },
        #{ anchor: "model_view.view_toggles",           focus: (),                            title: "Four Views",          body: "One model, four live views: 📝 Text source, 🔗 Diagram (wired components), 🎨 Icon (its annotation symbol), and 📖 Docs. Switch freely — they stay in sync." },
        #{ anchor: "panel.modelica_component_palette",  focus: "modelica_component_palette",  title: "Component Palette",   body: "In Diagram view, drag components from the palette onto the canvas and wire their ports. This is graphical composition — build a model with no text." },
        #{ anchor: "panel.modelica_diagram_inspector",  focus: "modelica_diagram_inspector",  title: "Diagram Inspector",   body: "Select a component or connection to edit its parameters and modifiers here. The Text source updates as you type — both views are one model." },
        #{ anchor: "",                                  focus: (),                            title: "Now run it",          body: "You can read and build models. Next: compile and run one. Pick “Compile & Run” from the 🎓 Tutorials menu." },
    ]
}

fn on_start(me) {
    cmd("OpenClass", #{ qualified: "CascadedRCFilter" });   // give the model-view steps content
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
