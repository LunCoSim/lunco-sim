//! B4 — Rover Variants (Basic tutorial track).
//!
//! A coach tour that puts the three rovers side by side on the practice yard so
//! the student can compare what centre of mass, motor torque and grip do.
//! The B4 payload authors the easy and awful variants flanking the medium rover;
//! the cards explain each, and the student may possess and drive any of them
//! between cards.

fn steps() {
    [
        #{ anchor: "",                      title: "Compare three rovers",      body: "Three rovers are lined up on the yard: green Explorer LT, amber Standard, and red Hauler. Select each rover and read its Inspector values." },
        #{ anchor: "panel.center",          title: "Explorer LT (green)",       body: "Select the green Explorer LT. Notice its low centre of mass, strong motors, and high grip. Use it when you want forgiving handling." },
        #{ anchor: "panel.center",          title: "Standard (amber)",          body: "Select the amber Standard. Compare its mass and motor values with the Explorer LT. It is the balanced baseline for testing." },
        #{ anchor: "panel.center",          title: "Hauler (red)",              body: "Select the red Hauler. Notice its high centre of mass, weaker motors, and lower grip. Drive it carefully: across-slope steering can tip it." },
        #{ anchor: "panel.right_inspector", title: "Explain the difference",    body: "Select a rover and read centre of mass, mass, torque, and friction in Inspector. Those authored values explain how each rover handles." },
        #{ anchor: "",                      title: "Choose a rover",             body: "Pick the rover that fits your next task. Click Continue to learn how to join a team session." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    // All three profile references are composed by the curriculum payload before
    // this script starts. The script owns lesson state only; scene composition
    // stays in USD and cannot be lost to catalog-load timing.
    this.i = 0;
    notify_kind("Basic · Rover Variants", "info");
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
        hint("You have compared the rover variants. Click Continue in the Tutorial complete dialog to begin Join a Team Session.");
        notify_kind("Variants felt — choose Continue for Join a Team Session", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
