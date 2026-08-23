//! Lesson 6 — Automate. Drive Lunica from scripts, the HTTP API, and MCP.
//!
//! This lesson is itself proof of the point: the whole tour you're reading is a
//! rhai scenario running in the workbench. Every UI action is a typed Command
//! that `cmd(name, #{params})` can call — the same reflect-dispatched core the
//! HTTP API and MCP use — so scripting is a first-class way to drive the app,
//! independent of the API server.

fn steps() {
    [
        #{ anchor: "",                        focus: (),                   title: "Automate Lunica",   body: "This tour IS a rhai script running live in the workbench. Anything you can click, a script can do. Use the Back and Next controls." },
        #{ anchor: "",                        focus: (),                   title: "One command bus",   body: "Use the live REPL or open Help in the top menu for the command surface. Dispatch one typed command, for example cmd(\"OpenClass\", #{ qualified: \"CascadedRCFilter\" }). Check the model view afterward; the UI and HTTP/API clients use the same command." },
        #{ anchor: "",                        focus: (),                   title: "HTTP API & MCP",    body: "Use the API/MCP guidance under Help. Send an ExecuteCommand request to /api/commands with a command and named params, then inspect the response. MCP exposes the same typed operation to an agent; it is not a separate simulation path." },
        #{ anchor: "panel.modelica_journal",  focus: "modelica_journal",   title: "Journal",           body: "Commands are dispatched through one typed bus. The document journal records authored document operations; it is not a universal replay log for every transient UI or runtime command." },
        #{ anchor: "",                        focus: (),                   title: "Make it yours",     body: "These tutorials live in assets/tutorials/lunica/*.rhai. Edit one, re-run from the Tutorials menu, and it hot-reloads. Author your own the same way." },
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
