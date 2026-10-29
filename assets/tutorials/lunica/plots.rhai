//! Lesson 5 — Plots & Results. Reading a run: graphs, diagnostics, console.

fn steps() {
    [
        #{ anchor: "",                              focus: (),                       title: "Read plots and results",  body: "This lesson shows where to read simulation output. Press Next to continue." },
        #{ anchor: "panel.modelica_plot",           focus: (),          title: "Compare signals",           body: "Select two signals in Graphs. Put fast and slow signals on separate axes, then snapshot the run and overlay another one." },
        #{ anchor: "panel.modelica_diagnostics",    focus: "modelica_diagnostics",   title: "Open a diagnostic",      body: "Compile the model and select a diagnostic. When a source location exists, the source view jumps to that line." },
        #{ anchor: "panel.modelica_console",        focus: "modelica_console",       title: "Read the console",          body: "Read compile progress, solver messages, and script output. Use the message context to tell whether the issue is in source, setup, or runtime." },
        #{ anchor: "",                              focus: (),                       title: "Explain a result",        body: "Choose one plotted signal. Identify its units and time interval, then describe what its shape says about the model. Finish this tour to choose your next lesson." },
    ]
}

fn on_start(me, ctx) {
    cmd("OpenClass", #{ qualified: "CascadedRCFilter" });
    this.i = 0;
    coach_step(steps(), 0);
}

fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    this = tutorials::tour_event(this, evt, steps(), ctx);
}
