//! Lesson 2 — Open & View a Model. The four views + graphical composition.
//! Opens the bundled `CascadedRCFilter` demo so the model-view steps have real
//! content to spotlight.

fn steps() {
    [
        #{ anchor: "",                                  focus: (),                            title: "Open & View a Model", body: "We opened the CascadedRCFilter demo for you. Let's look at the ways to view and edit a Modelica model. ◀ ▶ or arrow keys." },
        #{ anchor: "model_view.view_toggles",           focus: (),                            title: "Four Views",          body: "Click Text, Diagram, Icon, and Docs in turn. Text shows declarations, Diagram shows connected components, Icon shows the reusable symbol, and Docs shows the model explanation." },
        #{ anchor: "panel.modelica_component_palette",  focus: "modelica_component_palette",  title: "Component Palette",   body: "In Diagram view, drag one component from the palette onto the canvas, then drag between compatible ports to create a connection. This is graphical composition." },
        #{ anchor: "panel.modelica_diagram_inspector",  focus: "modelica_diagram_inspector",  title: "Diagram Inspector",   body: "Select the component or connection you just placed. Edit one parameter or modifier here, then switch to Text to see the same authored model." },
        #{ anchor: "",                                  focus: (),                            title: "Now run it",          body: "You can read and build models. Next: compile and run one. Continue when you're ready." },
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
