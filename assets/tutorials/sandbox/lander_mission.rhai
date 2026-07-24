//! Lander & Rover Mission — a coaching tutorial over a self-orchestrating scene.
//!
//! ## One source, layered orchestration
//! The lesson IS this script (run by the launcher on the tutorial host). It loads
//! `lander_ops.usda`, whose OWN embedded scenarios fly the mission:
//!   • `/LanderTest/Scenario` → `rover_surface_ops.rhai`: powered descent →
//!     `lander_touchdown` → detach the joint → `rover_deployed` → reveal a 3-gate
//!     course one dome at a time, announcing `waypoint_N_reached` / `course_complete`.
//!   • `/LanderTest/SkidRover` → `lander_rover_patrol.btxml`: drives the course after
//!     `rover_deployed`, and yields the moment a human takes the rover.
//! Both emit their phase events on the global bus, so this tutorial just *watches*:
//! it turns those events into a student-facing objectives checklist and coaches
//! the hand-off. No duplicated mission logic — the scene owns the physics, the
//! tutorial owns the pedagogy.
//!
//! The declarative `mission(me)` advances on the REAL things happening — the
//! lander's touchdown event, the rover coming free, the student actually holding
//! the rover, the rover reaching the final gate — never on a timer. The engine
//! auto-publishes the checklist to the HUD and emits `MISSION_COMPLETE` when every
//! objective is done (→ chain end).
//!
//! ## Why `takeover` is a state read, not an event latch
//! `requires_event: "cmd:PossessVessel"` would complete on ANY possession — the
//! student clicking the LANDER mid-descent to fly it by hand would tick off "take
//! control of the rover". So `takeover` asks the question it actually means: is a
//! human driving THE ROVER right now? `controller(id)` answers it directly
//! ("AiAgent" = autopilot, "Owner"/"Operator" = a person), and it's true however
//! the student got there — click, F, or the autopilot handing over on a keypress.

fn on_start(me) {
    // One source: the tutorial loads its own environment. The scene's embedded
    // scenarios do the flying; the mission() below coaches over their events.
    load_scene("lunco://scenes/sandbox/lander_ops.usda");
    hint("A powered descent is landing a rover. Watch it touch down — then you'll take control and drive.");
    notify_kind("Tutorial: Lander & Rover Mission", "info");
}

/// True if a HUMAN session drives `id` — not the autopilot, not nobody. The same
/// test the rover's autopilot uses to decide when to stand down.
fn human_drives(id) {
    if id < 0 { return false; }
    let role = controller(id);
    role != () && role != "AiAgent"
}

fn mission(me) {
    // Scene paths as LOCALS so the closures capture them (named fns can't see
    // module consts; closures capture enclosing locals) — same idiom as
    // first_drive.rhai. Each objective completes on a real event / world state.
    let rover = "/LanderTest/SkidRover";
    let goal  = "/LanderTest/RoverTarget3";   // the final gate of the 3-gate course
    [
        // 1 — passive: the lander's own supervisor emits `lander_touchdown` when it
        // settles on its legs. We just wait for that event (no polling the descent).
        objective("touchdown", #{
            text: "Watch the powered descent land the rover",
            requires_event: "lander_touchdown",
            on_complete: |m| hint("Touchdown. The mission is about to cut the rover loose."),
        }),
        // 2 — passive: the scene detaches the joint and announces `rover_deployed`.
        // Its own step in the checklist, because from the student's seat "the rover
        // is free" and "the lander has landed" are two different moments, and the
        // next instruction only makes sense after this one.
        objective("deploy", #{
            text: "Wait for the rover to be released",
            requires: ["touchdown"],
            requires_event: "rover_deployed",
            on_complete: |m| hint("The rover is driving itself. Take it: click the rover, or press F."),
        }),
        // 3 — take over. Completes when a PERSON holds the rover, however they got
        // it. NOT an event latch: possessing the lander must not satisfy this.
        objective("takeover", #{
            text: "Take control of the deployed rover (click it or press F)",
            requires: ["deploy"],
            done: |m| human_drives(find(rover)),
            on_complete: |m| hint("You have the rover. Drive it through the glowing waypoint gates with W / A / S / D."),
        }),
        // 4 — drive the course home. Gated on the takeover, then completes on a
        // plain distance read to the final gate. dwell 0.4s so a fly-through won't
        // count.
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
    // Coach the course from the scene's own gate events — the scenario counts the
    // gates, the tutorial narrates them.
    if evt.name == "waypoint_1_reached" {
        hint("One gate down. The next dome just lit up — head for it.");
    } else if evt.name == "waypoint_2_reached" {
        hint("Two down. The final dome is revealed.");
    } else if evt.name == "MISSION_COMPLETE" {
        // The engine emits MISSION_COMPLETE once every objective is done.
        hint("Mission complete! You've flown a descent and driven a rover. Free-roam, or replay any lesson from the 🎓 Tutorials menu.");
        notify_kind("Lander & Rover Mission complete 🎉", "success");
    }
}

// DEBUG self-completion — runs alongside the mission for CI/regression (the "if
// DEBUG, autopilot" pattern; see first_drive.rhai). In a debug build (or with
// LUNCO_SCENARIO_DEBUG=1) it waits for the rover to deploy, then possesses it,
// satisfying `takeover` WITHOUT a keypress. Possession makes the rover's autopilot
// stand down, so the debug run drives the rest of the course itself. In release it
// returns () and a human plays.
fn task(me) {
    if !is_debug() { return; }
    seq([
        wait_for("rover_deployed"),
        wait(0.5),
        once(|m| {
            cmd("PossessVessel", #{ target: find("/LanderTest/SkidRover") });
            notify_kind("[debug] auto-takeover — driving the course", "info");
        }),
        // The autopilot yielded on possession, so nothing is steering. Drive the
        // three gates ourselves, exactly as a student would.
        step(|m| nav_to(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget1")), 0.7, 4.0),
             |m| arrived(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget1")), 4.0)),
        step(|m| nav_to(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget2")), 0.7, 4.0),
             |m| arrived(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget2")), 4.0)),
        step(|m| nav_to(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget3")), 0.7, 4.0),
             |m| arrived(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget3")), 4.0)),
        #{ act: |m| brake(find("/LanderTest/SkidRover")), secs: 1.0 },
    ])
}
