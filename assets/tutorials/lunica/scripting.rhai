//! Lesson 6 — Automate. Drive Lunica from scripts, the HTTP API, and MCP.
//!
//! This lesson is itself proof of the point: the whole tour you're reading is a
//! rhai scenario running in the workbench. Every UI action is a typed Command
//! that `cmd(name, #{params})` can call — the same reflect-dispatched core the
//! HTTP API and MCP use — so scripting is a first-class way to drive the app,
//! independent of the API server.

fn steps() {
    [
        #{ anchor: "",                        focus: (),                   title: "Automate Lunica",   body: "This tour is a Rhai script running in the workbench. Scripts can send the same typed commands as the UI. Press Next to continue." },
        #{ anchor: "",                        focus: (),                   title: "Use the command bus",   body: "Open Help or the live REPL. Send a typed command such as `OpenClass` with a qualified class name, then check the model view." },
        #{ anchor: "",                        focus: (),                   title: "Use the API or MCP",    body: "Follow the API or MCP guidance under Help. Send `ExecuteCommand` to `/api/commands`, then inspect the response." },
        #{ anchor: "panel.modelica_journal",  focus: "modelica_journal",   title: "Read the journal",           body: "Open Journal to review authored document operations. It does not record every transient UI or runtime command." },
        #{ anchor: "",                        focus: (),                   title: "Edit a tutorial",     body: "Tutorial scripts live in `assets/tutorials/lunica`. Edit one, reopen it from Tutorials, and test the change." },
    ]
}

fn on_start(me) {
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
        notify_kind("You finished the Lunica curriculum!", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        coach_step(s, this.i);
    }
}
