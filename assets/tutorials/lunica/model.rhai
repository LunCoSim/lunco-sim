//! Lesson 2 — Open & View a Model. The four views + graphical composition.
//! Opens the bundled `CascadedRCFilter` demo so the model-view steps have real
//! content to spotlight.

fn steps() {
    [
        #{ anchor: "",                                  focus: (),                            title: "Open a model",          body: "The CascadedRCFilter demo is open. Use the Back and Next controls to explore it." },
        #{ anchor: "model_view.view_toggles",           focus: (),                            title: "Switch views",           body: "Click Text, Diagram, Icon, and Docs. Each view shows the same model from a different angle." },
        #{ anchor: "panel.modelica_component_palette",  focus: "modelica_component_palette",  title: "Add a component",         body: "In Diagram view, drag a component from the palette onto the canvas. Connect compatible ports by dragging between them." },
        #{ anchor: "panel.modelica_diagram_inspector",  focus: "modelica_diagram_inspector",  title: "Edit the component",      body: "Select the component or connection. Change one parameter in Diagram Inspector, then switch to Text to see the authored change." },
        #{ anchor: "",                                  focus: (),                            title: "Compile it next",          body: "You can now open and edit a model. Click Continue to learn how to compile and run it." },
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
