//! Lesson 5 — Plots & Results. Reading a run: graphs, diagnostics, console.

fn steps() {
    [
        #{ anchor: "",                              focus: (),                       title: "Plots & Results",  body: "A simulation is only as good as what you can read off it. Let's tour the output surfaces. ◀ ▶ or arrow keys." },
        #{ anchor: "panel.modelica_plot",           focus: (),          title: "Graphs",           body: "Select two signals, put fast and slow signals on separate axes, and snapshot the run. Then overlay a later run to make the comparison visible." },
        #{ anchor: "panel.modelica_diagnostics",    focus: "modelica_diagnostics",   title: "Diagnostics",      body: "Compile the model, open a diagnostic with a severity, and click it. The source view should jump to the reported line when a source location is available." },
        #{ anchor: "panel.modelica_console",        focus: "modelica_console",       title: "Console",          body: "Read compile progress, solver messages, and script output here. Use the message context to decide whether a problem is in source, setup, or runtime." },
        #{ anchor: "",                              focus: (),                       title: "Last stop",        body: "You can model, run, sweep, and read results. Finish with “Automate” — drive all of this from scripts. Continue when you're ready." },
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
