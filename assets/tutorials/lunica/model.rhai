//! Lesson 2 — Open & View a Model. The four views + graphical composition.
//! Opens the bundled `CascadedRCFilter` demo so the model-view steps have real
//! content to spotlight.

fn steps() {
    [
        #{ anchor: "",                                  focus: (),                            title: "Open a model",          body: "The CascadedRCFilter demo is open. Use the Back and Next controls to explore it." },
        #{ anchor: "model_view.view_toggles",           focus: (),                            title: "Switch views",           body: "Click Text, Diagram, Icon, and Docs. Each view shows the same model from a different angle." },
        #{ anchor: "panel.modelica_component_palette",  focus: "modelica_component_palette",  title: "Add a component",         body: "In Diagram view, drag a component from the palette onto the canvas. Connect compatible ports by dragging between them." },
        #{ anchor: "panel.modelica_diagram_inspector",  focus: "modelica_diagram_inspector",  title: "Edit the component",      body: "Select the component or connection. Change one parameter in Diagram Inspector, then switch to Text to see the authored change." },
        #{ anchor: "",                                  focus: (),                            title: "Check an edit",          body: "Identify the same component in Text and Diagram. If you edited it, review the source before compiling. Finish this tour to choose your next lesson." },
    ]
}

fn on_start(me, ctx) {
    cmd("OpenClass", #{ qualified: "CascadedRCFilter" });   // give the model-view steps content
    this.i = 0;
    coach_step(steps(), 0);
}

fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    this = tutorials::tour_event(this, evt, steps(), ctx);
}
