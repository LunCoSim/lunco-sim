//! Script a Rover — a guided tour of behaviour scripting (the author-scenario skill).
//!
//! Coach-mark tour. Explains what a scenario is and how to attach one, using
//! this very tutorial as the live example — a tutorial *is* a rhai scenario.

fn steps() {
    [
        #{ anchor: "",                   title: "Script a Rover",      body: "Behaviour in LunCoSim is rhai — hot-reloadable scripts attached to entities. This tutorial you're reading is itself one. Next / Back below." },
        #{ anchor: "",                   title: "Lifecycle hooks",     body: "A scenario defines on_start (once), on_tick (every step), and on_event (a signal arrived). Per-entity state lives on `this`." },
        #{ anchor: "",                   title: "High-level verbs",    body: "You don't hand-code control loops. Prelude verbs do the work: nav_to(target), drive(rover, fwd, steer), arrived(...), emit(event). Navigation, missions, and reactions are all rhai." },
        #{ anchor: "panel.side_browser", title: "Attach & run",        body: "Attach a script by adding a `Scope` child with `prepend apiSchemas = [\"LunCoProgramAPI\"]` and `uniform asset info:sourceAsset = @scenarios/patrol.rhai@`; delete the scope and the behaviour goes with it. Per-instance settings are typed `inputs:` on that scope. Or run one live through RunScenario; re-running hot-reloads it without a rebuild." },
        #{ anchor: "panel.bottom",       title: "Watch it work",       body: "print() and telemetry land in the Console below. Scripts sense and command the world through the SAME API the UI and AI agents use." },
        #{ anchor: "",                   title: "Go deeper",           body: "See the Scripting Guide and the author-scenario skill for the full verb surface, sequencing, and mission objectives." },
    ]
}

fn show(s, i) { let step = s[i]; coach(i, s.len(), step.anchor, step.title, step.body); }

fn on_start(me) {
    load_scene("lunco://tutorials/sandbox/first_drive.usda");
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
        notify_kind("Script a Rover complete 🎉", "success");
        emit("MISSION_COMPLETE", 0);
    } else { show(s, this.i); }
}
