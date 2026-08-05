//! Lander & Rover Mission — a coaching tutorial over a self-orchestrating scene.
//!
//! ## One source, layered orchestration
//! The lesson IS this script (run by the launcher on the tutorial host). It loads
//! `lander_ops.usda`, whose OWN embedded scenarios fly the mission:
//!   • `/LanderTest/Scenario` → `rover_surface_ops.rhai`: powered descent →
//!     `lander_touchdown` → detach the joint → `rover_deployed` → consume the
//!     three marker Sensor events, announcing `waypoint_N_reached` / `course_complete`.
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
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
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

fn lander_touchdown_message(me) {
    hint("Touchdown. The mission is about to cut the rover loose.");
}

fn lander_deployed_message(me) {
    hint("The rover is driving itself. Take it: click the rover, or press F.");
}

fn lander_human_takeover(me) {
    hint("You have the rover. Drive it through the glowing waypoint gates with W / A / S / D.");
}

fn lander_human_has_control(me) {
    human_drives(find("/LanderTest/SkidRover"))
}

fn lander_course_finished(me) {
    notify_kind("Course complete — mission accomplished!", "success");
}

fn lander_autopilot_takeover(me) {
    cmd("PossessVessel", #{ target: find("/LanderTest/SkidRover") });
    notify_kind("[debug] auto-takeover — driving the course", "info");
}

fn lander_drive_gate_1(me) {
    nav_to(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget1")), 0.7, 4.0)
}

fn lander_gate_1_done(me) {
    arrived(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget1")), 4.0)
}

fn lander_drive_gate_2(me) {
    nav_to(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget2")), 0.7, 4.0)
}

fn lander_gate_2_done(me) {
    arrived(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget2")), 4.0)
}

fn lander_drive_gate_3(me) {
    nav_to(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget3")), 0.7, 4.0)
}

fn lander_gate_3_done(me) {
    arrived(find("/LanderTest/SkidRover"), world_pos(find("/LanderTest/RoverTarget3")), 4.0)
}

fn lander_brake(me) {
    brake(find("/LanderTest/SkidRover"))
}

fn mission(me) {
    // Scene paths as LOCALS so the closures capture them (named fns can't see
    // module consts; closures capture enclosing locals) — same idiom as
    // first_drive.rhai. Each objective completes on a real event / world state.
    [
        // 1 — passive: the lander's own supervisor emits `lander_touchdown` when it
        // settles on its legs. We just wait for that event (no polling the descent).
        objective("touchdown", #{
            text: "Watch the powered descent land the rover",
            requires_event: "lander_touchdown",
            on_complete: Fn("lander_touchdown_message"),
        }),
        // 2 — passive: the scene detaches the joint and announces `rover_deployed`.
        // Its own step in the checklist, because from the student's seat "the rover
        // is free" and "the lander has landed" are two different moments, and the
        // next instruction only makes sense after this one.
        objective("deploy", #{
            text: "Wait for the rover to be released",
            requires: ["touchdown"],
            requires_event: "rover_deployed",
            on_complete: Fn("lander_deployed_message"),
        }),
        // 3 — take over. Completes when a PERSON holds the rover, however they got
        // it. NOT an event latch: possessing the lander must not satisfy this.
        objective("takeover", #{
            text: "Take control of the deployed rover (click it or press F)",
            requires: ["deploy"],
            done: Fn("lander_human_has_control"),
            on_complete: Fn("lander_human_takeover"),
        }),
        // 4 — drive the course home. Gated on the takeover, then completes on
        // the scene's authoritative final-marker event.
        objective("finish", #{
            text: "Drive the rover to the final waypoint",
            requires: ["takeover"],
            requires_event: "waypoint_3_reached",
            on_complete: Fn("lander_course_finished"),
        }),
    ]
}

fn on_event(me, evt) {
    // Coach the course from the scene's own gate events — the scenario counts the
    // gates, the tutorial narrates them.
    if evt.name == "waypoint_1_reached" {
        hint("One gate down. Continue to the next marker.");
    } else if evt.name == "waypoint_2_reached" {
        hint("Two down. Continue to the final marker.");
    } else if evt.name == "MISSION_COMPLETE" {
        // The engine emits MISSION_COMPLETE once every objective is done.
        hint("Mission complete! You've flown a descent and driven a rover. Free-roam, or replay any lesson from the 🎓 Tutorials menu.");
        notify_kind("Lander & Rover Mission complete 🎉", "success");
    }
}

// UNATTENDED self-completion — runs alongside the mission for CI/regression (the
// "if unattended, autopilot" pattern; see first_drive.rhai). With no window it
// waits for the rover to deploy, then possesses it, satisfying `takeover` WITHOUT
// a keypress. Possession makes the rover's autopilot stand down, so the automated
// run drives the rest of the course itself. With a window open it returns () and
// a human plays.
fn task(me) {
    if !is_unattended() { return; }
    seq([
        wait_for("rover_deployed"),
        wait(0.5),
        once(Fn("lander_autopilot_takeover")),
        // The autopilot yielded on possession, so nothing is steering. Drive the
        // three gates ourselves, exactly as a student would.
        step(Fn("lander_drive_gate_1"), Fn("lander_gate_1_done")),
        step(Fn("lander_drive_gate_2"), Fn("lander_gate_2_done")),
        step(Fn("lander_drive_gate_3"), Fn("lander_gate_3_done")),
        #{ act: Fn("lander_brake"), secs: 1.0 },
    ])
}
