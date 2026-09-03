//! B5 — Join a Team Session (Basic tutorial track).
//!
//! A coach-mark tour (Next/Back cards) that walks a team through connecting to
//! the workshop server and taking their rover. No scene of its own — it explains
//! the multiplayer flow the teams use on the day. Guaranteed-completable (no scene
//! deps), so it is safe to run on any client. Pattern: sandbox_intro.rhai.

fn steps() {
    [
        #{ anchor: "",                      title: "Join your team",        body: "This tour shows the team-session flow. Use Next to move through the steps; use Back to review one." },
        #{ anchor: "",                      title: "Connect to the server", body: "Open Network in the top menu. Enter the server address and access key from your instructor, then connect." },
        #{ anchor: "panel.center",          title: "Find your rover",       body: "Find the rover in your team colour at its start marker. Hold " + input_hint("look_button") + " and drag to look around." },
        #{ anchor: "panel.command_deck",    title: "Take control",          body: "Select your rover. Click Possess in Command Deck. Only one driver controls a rover at a time." },
        #{ anchor: "panel.sandbox_inspector", title: "Check the setup",       body: "With the rover selected, read its class and tuning in Inspector. Explorer LT, Standard, and Hauler handle differently." },
        #{ anchor: "panel.telemetry_browser", title: "Read live telemetry",   body: "Open Telemetry in the left dock. Select a port and read the rover's current values while it is moving." },
        #{ anchor: "",                      title: "End the session",        body: "When the session ends, open Network and click Disconnect or Leave Server. Click Continue when you are ready for the Space School track." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
    this.i = 0;
    notify_kind("Basic · Join a Team Session", "info");
    show(steps(), 0);
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Basics complete - ready for the Space School track", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
