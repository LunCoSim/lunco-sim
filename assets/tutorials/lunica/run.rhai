//! Lesson 3 — Compile & Run. Interactive vs Fast Run, and live inputs.

fn steps() {
    [
        #{ anchor: "",                            focus: (),                     title: "Compile & Run",      body: "Time to make a model move. We opened CascadedRCFilter — let's simulate it. ◀ ▶ or arrow keys." },
        #{ anchor: "model_view.compile_buttons",  focus: (),                     title: "Two run modes",      body: "Click Run for an interactive compile-and-step session; use the input controls while it runs. Click Fast Run for a headless integration from 0 to t_end that produces a completed result." },
        #{ anchor: "panel.modelica_inspector",    focus: "modelica_inspector",   title: "Live Inputs",        body: "During Run, select an input in Telemetry and drag its slider. Observe the signal change at simulation rate; parameters remain model configuration, not live inputs." },
        #{ anchor: "panel.modelica_plot",         focus: (),        title: "Watch it move",      body: "Choose a signal for Graphs while Run is active, then repeat with Fast Run to compare a complete 0 → t_end trajectory." },
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
