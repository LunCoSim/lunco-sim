//! B4 — Rover Variants (Basic tutorial track).
//!
//! A coach tour that compares tire friction and motor torque across three
//! profiles on one shared chassis.
//! The B4 payload authors the easy and awful variants flanking the medium rover;
//! the cards explain each, and the student may possess and drive any of them
//! between cards.

// Teaching guidance follows the learner across workbench perspectives.
hud_perspectives([]);

fn steps() {
    [
        #{ anchor: "",                      title: "Compare three rovers",      body: "The green Explorer LT, amber Standard, and red Hauler start side by side on flat ground. In Entities, the amber Standard is named Rover. Select each rover, then expand RockerL to inspect Wheel_FL's tire variant and Motor_FL's stall torque; the next cards give you the values to compare." },
        #{ anchor: "panel.center",          title: "Explorer LT (green)",       body: "All three profiles share the same chassis, mass, centre of mass, inertia, and suspension. Explorer uses cleated tires (μ=1.8) and 0.9 N·m motor stall torque. The tire coefficient μ describes the tire's authored friction setting." },
        #{ anchor: "panel.center",          title: "Standard (amber)",          body: "Select the amber Rover. This is the Standard profile: worn tires (μ=1.0) and 0.4 N·m motor stall torque. Compare it with Hauler to isolate tire friction while motor torque stays the same." },
        #{ anchor: "panel.center",          title: "Hauler (red)",              body: "Hauler uses bald tires (μ=0.8) with the same 0.4 N·m motor stall torque as Standard. To compare driving, take each rover to the central ramp in turn and follow the same line. The red paint is a label, not a heavier or higher-mounted load." },
        #{ anchor: "panel.sandbox_inspector", title: "Read a controlled comparison", body: "The Explorer-to-Standard comparison changes both tire friction and motor torque. The Standard-to-Hauler comparison changes tire friction only. Record which values changed before explaining the drive." },
        #{ anchor: "",                      title: "Choose a rover",             body: "Choose a profile for your next drive and justify it with the authored tire friction or motor stall torque. These profiles do not change mass or centre of mass. Finish this tour to choose your next lesson." },
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
