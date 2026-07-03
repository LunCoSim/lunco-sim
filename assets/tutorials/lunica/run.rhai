//! Lesson 3 — Compile & Run. Interactive vs Fast Run, and live inputs.

fn steps() {
    [
        #{ anchor: "",                            focus: (),                     title: "Compile & Run",      body: "Time to make a model move. We opened CascadedRCFilter — let's simulate it. ◀ ▶ or arrow keys." },
        #{ anchor: "model_view.compile_buttons",  focus: (),                     title: "Two run modes",      body: "🚀 Interactive — compile, then step in real time; sliders drive inputs live and signals stream into Graphs.\n⏩ Fast Run — batch, headless: integrate 0 → t_end then dump results. Best for sweeps and reproducibility." },
        #{ anchor: "panel.modelica_inspector",    focus: "modelica_inspector",   title: "Live Inputs",        body: "During an Interactive run, Telemetry lists every parameter and input. Drag a slider to drive an input at simulation rate and watch the model respond." },
        #{ anchor: "panel.modelica_plot",         focus: (),        title: "Watch it move",      body: "Toggle a signal to trace and it appears in Graphs as the sim runs. Try Fast Run for the whole 0 → t_end curve at once." },
        #{ anchor: "",                            focus: (),                     title: "Go further",         body: "Next: sweep parameters without editing source. Continue when you're ready." },
    ]
}

fn on_start(me) {
    cmd("OpenClass", #{ qualified: "CascadedRCFilter" });
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
