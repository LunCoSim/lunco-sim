//! Lesson 4 — Experiments & Sweeps. Parameter overrides without editing source.

fn steps() {
    [
        #{ anchor: "",                            focus: (),                       title: "Run an experiment",       body: "Test a design without editing the model source. Press Next to begin." },
        #{ anchor: "panel.modelica_experiments",  focus: "modelica_experiments",   title: "Override one parameter", body: "Choose a parameter, enter a new value, and start Fast Run. The source stays unchanged." },
        #{ anchor: "panel.modelica_experiments",  focus: "modelica_experiments",   title: "Run a sweep",            body: "Enter several values, such as R = 100, 200, and 400. Start the sweep to compare the resulting trajectories." },
        #{ anchor: "panel.modelica_plot",         focus: (),          title: "Compare the runs",      body: "Snapshot the first run. Change the override, run again, and overlay the new result. Note which change caused the difference." },
        #{ anchor: "",                            focus: (),                       title: "Keep it reproducible",      body: "An experiment stores the model, overrides, and solver settings. Click Continue to learn how to read the results." },
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
