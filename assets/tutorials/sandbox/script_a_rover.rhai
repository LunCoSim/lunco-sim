//! Script a Rover — a guided tour of behaviour scripting (the author-scenario skill).
//!
//! Coach-mark tour. Explains what a scenario is and how to attach one, using
//! this very tutorial as the live example — a tutorial *is* a rhai scenario.

fn steps() {
    [
        #{ anchor: "",                   title: "Script a rover",       body: "Rhai scripts add scenario behaviour to entities. This tutorial is a Rhai scenario too. Press Next to continue." },
        #{ anchor: "",                   title: "React to events",       body: "Use `on_start` for setup and `on_event` for reactions. Use mission and task primitives for progression. Keep per-entity state on `this`." },
        #{ anchor: "",                   title: "Choose the right owner", body: "Use Modelica for continuous equations. Use Rhai for policy and sequencing with verbs such as `nav_to`, `arrived`, `emit`, and mission objectives." },
        #{ anchor: "panel.rhai_editor", focus: "rhai_editor", title: "Attach or run a script", body: "To attach a script, add a `Scope` with `LunCoProgramAPI` and set its `info:sourceAsset` to the `.rhai` file. To run one live, use RunScenario. Re-running reloads the script without a rebuild." },
        #{ anchor: "panel.telemetry_browser", focus: "telemetry_browser", title: "Read the result",        body: "Open Telemetry to read the script's output. Scripts, the UI, and AI agents use the same command surface." },
        #{ anchor: "",                   title: "Try it yourself",       body: "Open the Scripting Guide for the full verb and mission reference. Click Continue to return to the tutorial list." },
    ]
}

fn show(s, i) { let step = s[i]; coach(i, s.len(), step.anchor, step.title, step.body); }

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Script a Rover", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Script a Rover complete", "success");
        emit("MISSION_COMPLETE", 0);
    } else { show(s, this.i); }
}
