//! Lesson 6 — Automate. Drive Lunica from scripts, the HTTP API, and MCP.
//!
//! This lesson is itself proof of the point: the whole tour you're reading is a
//! rhai scenario running in the workbench. Every UI action is a typed Command
//! that `cmd(name, #{params})` can call — the same reflect-dispatched core the
//! HTTP API and MCP use — so scripting is a first-class way to drive the app,
//! independent of the API server.

fn steps() {
    [
        #{ anchor: "",                        focus: (),                   title: "Automate Lunica",   body: "This tour IS a rhai script running live in the workbench. Anything you can click, a script can do. ◀ ▶ or arrow keys." },
        #{ anchor: "menu.help",               focus: (),                   title: "One command bus",   body: "Every UI action is a typed #[Command]. In a script: cmd(\"OpenClass\", #{ qualified: \"CascadedRCFilter\" }). The SAME commands back the HTTP API and MCP — one bus, three drivers." },
        #{ anchor: "menu.help",               focus: (),                   title: "HTTP API & MCP",    body: "POST { \"command\": ..., \"params\": ... } to /api/commands to drive Lunica headless (--no-ui), or let an LLM agent drive it over MCP — no bespoke bindings." },
        #{ anchor: "panel.modelica_journal",  focus: "modelica_journal",   title: "Journal",           body: "Every command — yours, a script's, the API's, or an agent's — is recorded here, giving a replayable, auditable history of the session." },
        #{ anchor: "",                        focus: (),                   title: "Make it yours",     body: "These tutorials live in assets/tutorials/lunica/*.rhai. Edit one, re-run from the 🎓 Tutorials menu, and it hot-reloads. Author your own the same way." },
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
        notify_kind("You finished the Lunica curriculum! 🎉", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        coach_step(s, this.i);
    }
}
