//! Lander & Rover Mission — a coaching tutorial over a self-orchestrating scene.
//!
//! ## One source, layered orchestration
//! The lesson IS this script (run by the launcher on the tutorial host). It loads
//! `lander_test.usda`, whose OWN embedded scenarios fly the mission:
//!   • `/LanderTest/Scenario` → `rover_surface_ops.rhai`: powered descent →
//!     `lander_touchdown` → safe the engine → detach the joint → `rover_deployed`
//!     → reveal a 3-gate course one dome at a time.
//!   • `/LanderTest/SkidRover` → `rover_autopilot.rhai`: drives the course after
//!     `rover_deployed`, and YIELDS to the player on any keypress.
//! Both emit their phase events on the global bus, so this tutorial just *watches*:
//! it turns those events into a student-facing objectives checklist and coaches
//! the hand-off. No duplicated mission logic — the scene owns the physics, the
//! tutorial owns the pedagogy.
//!
//! The declarative `mission(me)` advances on the REAL things happening — the
//! lander's touchdown event, the student taking control, the rover reaching the
//! final gate — never on a timer. The engine auto-publishes the checklist to the
//! HUD and emits `MISSION_COMPLETE` when every objective is done (→ chain end).

const ROVER  = "/LanderTest/SkidRover";
const LANDER = "/LanderTest/Lander";
const GOAL   = "/LanderTest/RoverTarget3";   // the final gate of the 3-gate course

fn on_start(me) {
    // One source: the tutorial loads its own environment. The scene's embedded
    // scenarios do the flying; the mission() below coaches over their events.
    load_scene("scenes/sandbox/lander_test.usda");
    hint("A powered descent is landing a rover. Watch it touch down — then you'll take control and drive.");
    notify_kind("Tutorial: Lander & Rover Mission", "info");
}

fn mission(me) {
    // Scene paths as LOCALS so the `done` closure captures them (named fns can't
    // see module consts; closures capture enclosing locals) — same idiom as
    // first_drive.rhai. Each objective completes on a real event / world state.
    let rover = "/LanderTest/SkidRover";
    let goal  = "/LanderTest/RoverTarget3";
    [
        // 1 — passive: the lander's own script emits `lander_touchdown` when it
        // settles. We just wait for that event (no polling the descent).
        objective("touchdown", #{
            text: "Watch the powered descent land the rover",
            requires_event: "lander_touchdown",
            on_complete: |m| hint("Touchdown! The rover is being released. Take control — click it or press F."),
        }),
        // 2 — take over. The rover deploys under autopilot; the student seizes it
        // (click / F, or any key hands off from the autopilot). Either path fires a
        // PossessVessel command, which lands on the bus as `cmd:PossessVessel`.
        objective("takeover", #{
            text: "Take control of the deployed rover (click it or press F)",
            requires: ["touchdown"],
            requires_event: "cmd:PossessVessel",
            on_complete: |m| hint("You have the rover. Drive it through the glowing waypoint gates with W / A / S / D."),
        }),
        // 3 — drive the course home. Gated on the takeover, then completes on a
        // plain distance read to the final gate (robust whether you or the
        // autopilot's last leg gets there). dwell 0.4s so a fly-through won't count.
        objective("finish", #{
            text: "Drive the rover to the final waypoint",
            requires: ["takeover"],
            done: |m| { let d = distance(find(rover), find(goal)); d >= 0.0 && d < 6.0 },
            dwell: 0.4,
            on_complete: |m| notify_kind("Course complete — mission accomplished!", "success"),
        }),
    ]
}

fn on_event(me, evt) {
    // The engine emits MISSION_COMPLETE once every objective is done.
    if evt.name == "MISSION_COMPLETE" {
        hint("Mission complete! You've flown a descent and driven a rover. Free-roam, or replay any lesson from the 🎓 Tutorials menu.");
        notify_kind("Lander & Rover Mission complete 🎉", "success");
    }
}

// DEBUG self-completion — runs alongside the mission for CI/regression (the "if
// DEBUG, autopilot" pattern; see first_drive.rhai). In a debug build (or with
// LUNCO_SCENARIO_DEBUG=1) it waits for the rover to deploy, then possesses it —
// satisfying the `takeover` objective WITHOUT a keypress, so the scene's autopilot
// keeps driving the course to the final gate and the lesson auto-passes. In
// release it returns () and a human plays.
fn task(me) {
    if !is_debug() { return; }
    seq([
        wait_for("rover_deployed"),
        wait(0.5),
        once(|m| {
            cmd("PossessVessel", #{ target: find("/LanderTest/SkidRover") });
            notify_kind("[debug] auto-takeover — autopilot finishes the course", "info");
        }),
    ])
}
