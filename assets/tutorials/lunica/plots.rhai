//! Lesson 5 — Plots & Results. Reading a run: graphs, diagnostics, console.

fn steps() {
    [
        #{ anchor: "",                              focus: (),                       title: "Plots & Results",  body: "A simulation is only as good as what you can read off it. Let's tour the output surfaces. ◀ ▶ or arrow keys." },
        #{ anchor: "panel.modelica_plot",           focus: (),          title: "Graphs",           body: "Multi-axis plots, one set per document. Pick signals to trace, put fast + slow signals on separate axes, and snapshot a run to overlay against later ones." },
        #{ anchor: "panel.modelica_diagnostics",    focus: "modelica_diagnostics",   title: "Diagnostics",      body: "Compile and solver problems land here with severities. Click a diagnostic to jump straight to the offending line in the source." },
        #{ anchor: "panel.modelica_console",        focus: "modelica_console",       title: "Console",          body: "The running log — compile progress, solver messages, and any script output stream here. Your first stop when something looks off." },
        #{ anchor: "",                              focus: (),                       title: "Last stop",        body: "You can model, run, sweep, and read results. Finish with “Automate” — drive all of this from scripts. 🎓 Tutorials menu." },
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
