//! B5 — Join a Team Session (Basic tutorial track).
//!
//! A coach-mark tour (Next/Back cards) that walks a team through connecting to
//! the workshop server and taking their rover. No scene of its own — it explains
//! the multiplayer flow the teams use on the day. Guaranteed-completable (no scene
//! deps), so it is safe to run on any client. Pattern: sandbox_intro.rhai.

fn steps() {
    [
        #{ anchor: "",                      title: "Joining your team",     body: "On the day you'll each be on a laptop, 1–2 drivers per team. This 60-second tour shows how to get onto the shared world and take your rover. Next / Back below — Skip anytime." },
        #{ anchor: "",                      title: "Connect to the server", body: "Open Network in the top menu bar and enter the workshop server address and access key supplied by the instructor. You join as an authenticated Observer first, so you can watch before requesting control." },
        #{ anchor: "panel.center",          title: "Find your rover",       body: "Your team's rover is the one in your team colour at your start marker. Hold " + input_hint("look_button") + " and drag to look around the world." },
        #{ anchor: "panel.right_inspector", title: "Take control",          body: "Select your rover, then click Possess in the Inspector or Command Deck. One driver holds control at a time; a teammate can take over after you release." },
        #{ anchor: "panel.right_inspector", title: "Check your rover",      body: "With the rover selected, the Inspector shows its class and tuning — which variant you drew (Explorer LT / Standard / Hauler) changes how it climbs and tips." },
        #{ anchor: "panel.side_browser",    title: "Watch your telemetry",  body: "Use Telemetry in the left dock to inspect the selected rover's live ports. The current client shows its own readable state; team-wide authority and peer dashboards are not part of this lesson." },
        #{ anchor: "",                      title: "You're on the team",     body: "Connect through Network, select the right rover, request possession, read its live ports, and use Disconnect or Leave Server when the session ends. A scored run requires a separate event setup." },
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
