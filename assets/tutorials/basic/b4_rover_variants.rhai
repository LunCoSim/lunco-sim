//! B4 — Rover Variants (Basic tutorial track).
//!
//! A coach tour that puts the three rovers side by side on the practice yard so
//! the student can compare what centre of mass, motor torque and grip do.
//! The B4 payload authors the easy and awful variants flanking the medium rover;
//! the cards explain each, and the student may possess and drive any of them
//! between cards.

// Teaching guidance follows the learner across workbench perspectives.
hud_perspectives([]);

fn steps() {
    [
        #{ anchor: "",                      title: "Compare three rovers",      body: "Three rovers are lined up on the yard: green Explorer LT, amber Standard, and red Hauler. Select each rover and read its Inspector values." },
        #{ anchor: "panel.center",          title: "Explorer LT (green)",       body: "Select the green Explorer LT. Notice its low centre of mass, strong motors, and high grip. Use it when you want forgiving handling." },
        #{ anchor: "panel.center",          title: "Standard (amber)",          body: "Select the amber Standard. Compare its mass and motor values with the Explorer LT. It is the balanced baseline for testing." },
        #{ anchor: "panel.center",          title: "Hauler (red)",              body: "Select the red Hauler. Notice its high centre of mass, weaker motors, and lower grip. Drive it carefully: across-slope steering can tip it." },
        #{ anchor: "panel.sandbox_inspector", title: "Explain the difference",    body: "Select a rover and read centre of mass, mass, torque, and friction in Inspector. Those authored values explain how each rover handles." },
        #{ anchor: "",                      title: "Choose a rover",             body: "Pick the rover that fits your next task. Use the next-lesson button to learn how to join a team session." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me, ctx) {
    // The optional world is selected by the catalog and mounted by the generic
    // launcher before this runs.
    // All three profile references are composed by the scene before
    // this script starts. The script owns lesson state only; scene composition
    // stays in USD and cannot be lost to catalog-load timing.
    this.i = 0;
    notify_kind("Basic · Rover Variants", "info");
    show(steps(), 0);
}

fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    this = tutorials::tour_event(this, evt, steps(), ctx);
}
