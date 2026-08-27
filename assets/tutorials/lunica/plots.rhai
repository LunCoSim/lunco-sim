//! Lesson 5 — Plots & Results. Reading a run: graphs, diagnostics, console.

fn steps() {
    [
        #{ anchor: "",                              focus: (),                       title: "Read plots and results",  body: "This lesson shows where to read simulation output. Press Next to continue." },
        #{ anchor: "panel.modelica_plot",           focus: (),          title: "Compare signals",           body: "Select two signals in Graphs. Put fast and slow signals on separate axes, then snapshot the run and overlay another one." },
        #{ anchor: "panel.modelica_diagnostics",    focus: "modelica_diagnostics",   title: "Open a diagnostic",      body: "Compile the model and select a diagnostic. When a source location exists, the source view jumps to that line." },
        #{ anchor: "panel.modelica_console",        focus: "modelica_console",       title: "Read the console",          body: "Read compile progress, solver messages, and script output. Use the message context to tell whether the issue is in source, setup, or runtime." },
        #{ anchor: "",                              focus: (),                       title: "Automate the workflow",        body: "You can model, run, sweep, and read results. Click Continue to learn how to automate those actions." },
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
