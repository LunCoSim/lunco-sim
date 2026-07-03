//! Lesson 4 — Experiments & Sweeps. Parameter overrides without editing source.

fn steps() {
    [
        #{ anchor: "",                            focus: (),                       title: "Experiments",       body: "How to explore a design space without touching the model source. ◀ ▶ or arrow keys." },
        #{ anchor: "panel.modelica_experiments",  focus: "modelica_experiments",   title: "Override parameters", body: "Set any parameter to a new value here — no source edit. A one-shot Fast Run applies the overrides once and plots the result." },
        #{ anchor: "panel.modelica_experiments",  focus: "modelica_experiments",   title: "Sweeps",            body: "A scheduled run sweeps a LIST of overrides (e.g. R = 100, 200, 400) and collects every result, so you compare curves side by side." },
        #{ anchor: "panel.modelica_plot",         focus: (),          title: "Compare runs",      body: "Snapshot a run in Graphs to freeze its curves, then run again with new overrides and overlay — the fast feedback loop of design." },
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
