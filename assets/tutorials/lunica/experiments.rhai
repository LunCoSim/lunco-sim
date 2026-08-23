//! Lesson 4 — Experiments & Sweeps. Parameter overrides without editing source.

fn steps() {
    [
        #{ anchor: "",                            focus: (),                       title: "Experiments",       body: "How to explore a design space without touching the model source. Use the Back and Next controls." },
        #{ anchor: "panel.modelica_experiments",  focus: "modelica_experiments",   title: "Override parameters", body: "Choose a parameter, enter a new value, and start a one-shot Fast Run. The source stays unchanged and the result is recorded as an experiment." },
        #{ anchor: "panel.modelica_experiments",  focus: "modelica_experiments",   title: "Sweeps",            body: "Define a list of values, such as R = 100, 200, 400, and start the sweep. Compare the resulting trajectories rather than judging one run by eye." },
        #{ anchor: "panel.modelica_plot",         focus: (),          title: "Compare runs",      body: "Snapshot a run in Graphs, change the override, run again, and overlay the new result. Record which parameter change caused the difference." },
        #{ anchor: "",                            focus: (),                       title: "Reproducible",      body: "An experiment captures model + overrides + solver settings, so a run is repeatable and shareable. Next: “Plots & Results”." },
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
