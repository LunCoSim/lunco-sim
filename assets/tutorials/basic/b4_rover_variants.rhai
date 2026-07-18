//! B4 — Rover Variants (Basic tutorial track).
//!
//! A coach tour that puts the three rovers side by side on the practice yard so
//! the student can drive them back to back and FEEL what centre of mass, motor
//! torque and grip do. `on_start` spawns the easy and awful variants flanking the
//! medium rover already in the scene; the cards explain each, and the student is
//! free to possess and drive any of them (up the ramp) between cards.

fn steps() {
    [
        #{ anchor: "",                      title: "Three rovers, three feels", body: "Same six wheels, very different behaviour. Three rovers are lined up on the yard — green Explorer LT, amber Standard, red Hauler. Possess each (click, or F) and drive it up the ramp; the cards explain what you're feeling." },
        #{ anchor: "panel.center",          title: "Explorer LT (green)",       body: "Low centre of mass, strong motors, high grip. Planted and forgiving — it climbs the ramp easily and shrugs off a careless turn. The trainer: start here." },
        #{ anchor: "panel.center",          title: "Standard (amber)",          body: "Neutral centre of mass, moderate power. The honest baseline — it climbs fine and turns fine, but a sloppy cross-slope line will still get away from you." },
        #{ anchor: "panel.center",          title: "Hauler (red)",              body: "Top-heavy, weak motors, low grip — deliberately awful. It struggles up the ramp and tips at the smallest across-slope steer. If you can drive this one well, you can drive anything." },
        #{ anchor: "panel.right_inspector", title: "It's all in the USD",        body: "Select any rover: the Inspector shows the exact knobs — centre of mass, mass, torque, friction. The variants are just those numbers overridden on a shared base. Author your own by changing them." },
        #{ anchor: "",                      title: "Pick your rover",           body: "You've felt the range from forgiving to punishing. On a scored run, the rover you draw sets your whole plan. Next: join a team and take one out for real." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
    load_scene("lunco://tutorials/basic/driving_basics.usda");
    // The medium rover is already in the scene; flank it with the easy and awful
    // variants so all three are lined up at the SW corner.
    cmd("SpawnEntity", #{ entry_id: "rover_easy",  position: [-35.0, 1.0, -9.0] });
    cmd("SpawnEntity", #{ entry_id: "rover_awful", position: [-35.0, 1.0,  9.0] });
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
        notify_kind("Variants felt — on to joining a team", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
