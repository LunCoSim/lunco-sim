//! Lesson 3 — Compile & Run. Interactive vs Fast Run, and live inputs.

fn steps() {
    [
        #{ anchor: "",                            focus: (),                     title: "Compile and run",      body: "The CascadedRCFilter model is open. Press Next to learn the two run modes." },
        #{ anchor: "model_view.compile_buttons",  focus: (),                     title: "Choose a run mode",      body: "Click Run for interactive stepping and live inputs. Click Fast Run to integrate from 0 to `t_end` in the background." },
        #{ anchor: "panel.modelica_inspector",    focus: "modelica_inspector",   title: "Change a live input",        body: "During Run, select an input in Telemetry and drag its slider. Watch the signal change. Parameters stay part of the model configuration." },
        #{ anchor: "panel.modelica_plot",         focus: (),        title: "Read the result",      body: "Select a signal in Graphs while Run is active. Then use Fast Run and compare the complete trajectory." },
        #{ anchor: "",                            focus: (),                     title: "Sweep parameters next",         body: "Click Continue to learn how to test several parameter values without editing source." },
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
