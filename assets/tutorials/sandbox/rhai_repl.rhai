//! Lesson 5 — Rhai Editor & REPL. Learn how to write scripts in the editor and use the REPL.

fn steps() {
    [
        #{ anchor: "",             focus: (),              title: "Rhai policy",          body: "Use Rhai for event-driven sequencing, mission objectives, and scenario glue. Use Modelica or Rust for continuous equations. Press Next to continue." },
        #{ anchor: "panel.center", focus: "rhai_editor",   title: "Edit a scenario",      body: "The Behaviour editor opens `.rhai` files inside LunCoSim. These scripts attach to USD entities and define their scenario behaviour." },
        #{ anchor: "panel.bottom", focus: "rhai_repl",     title: "Try the live REPL",    body: "Open the Rhai REPL in the bottom dock. Type `print(3 + 4)` and run it. You can also dispatch typed commands to a live rover." },
        #{ anchor: "panel.bottom", focus: "rhai_repl",     title: "Find a command",       body: "Open Help and search the command reference. Copy a typed command into the REPL, then read its result." },
        #{ anchor: "",             focus: (),              title: "Automate the next step", body: "You can edit a scenario and test it live. Click Continue to return to the tutorial list." },
    ]
}

fn show(s, i) {
    coach_step(s, i);
}

fn on_start(me) {
    // The curriculum payload is mounted before this script starts, so the REPL
    // examples have a live entity to talk to.
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Rhai Editor & REPL", "info");
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
        notify_kind("Rhai Editor & REPL tutorial complete!", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
