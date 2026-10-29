//! Lesson 5 — Rhai Editor & REPL. Learn how to write scripts in the editor and use the REPL.

fn steps() {
    [
        #{ anchor: "",             focus: (),              title: "Rhai policy",          body: "Use Rhai for event-driven sequencing, mission objectives, and scenario glue. Use Modelica or Rust for continuous equations. Press Next to continue." },
        #{ anchor: "panel.rhai_editor", focus: "rhai_editor",   title: "Edit a scenario",      body: "The Behaviour editor opens `.rhai` files inside LunCoSim. These scripts attach to USD entities and define their scenario behaviour." },
        #{ anchor: "panel.rhai_repl", focus: "rhai_repl",     title: "Try the live REPL",    body: "Open the Rhai REPL in the bottom dock. Type `print(3 + 4)` and run it. You can also dispatch typed commands to a live rover." },
        #{ anchor: "panel.rhai_repl", focus: "rhai_repl",     title: "Find a command",       body: "Open Help and search the command reference. Copy a typed command into the REPL, then read its result." },
        #{ anchor: "",             focus: (),              title: "Automate the next step", body: "You can edit a scenario and test it live. Choose another lesson from Tutorials after finishing this tour." },
    ]
}

fn show(s, i) {
    coach_step(s, i);
}

fn on_start(me, ctx) {
    // The catalog-selected scene is mounted before this script starts, so the REPL
    // examples have a live entity to talk to.
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Rhai Editor & REPL", "info");
}

fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    this = tutorials::tour_event(this, evt, steps(), ctx);
}
