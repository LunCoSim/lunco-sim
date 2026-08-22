// A production-style robot mission: no on_tick loop. The task tree owns
// sequencing, while the mission declaration owns success/failure reporting.
// Attach this to a rover or to a mission-control entity and edit the points.
// If the control entity is not movable, add `subject: "/World/Rover"` to the
// timeline form instead; this example keeps the task subject as `me`.

const SITE_A = [12.0, 0.0,  0.0];
const SITE_B = [12.0, 0.0, 25.0];
const BASE   = [ 0.0, 0.0,  0.0];
const SPEED  = 1.0;
const RADIUS = 2.0;

// Supply the USD document that owns the rover prim. The default scene in the
// headless/API example is DocumentId 1; editor scripts should use the id from
// ListOpenDocuments instead.
fn mission_document(me) { 1 }

// The checkpoint is written only when a phase completes. On hot reload or
// restart, task(me) resumes from the last authored phase rather than replaying
// already-completed work.
fn task(me) {
    let doc = mission_document(me);
    let phase = mission_checkpoint_read(me, "phase");

    if phase == "site_b" {
        return seq([
            step(|m| nav_to(m, BASE, SPEED, RADIUS),
                 |m| arrived(m, BASE, RADIUS)),
            once(|m| {
                mission_checkpoint(m, doc, "phase", "complete");
                emit("ROBOT_MISSION_COMPLETE", true);
            }),
        ]);
    }

    if phase == "site_a" {
        return seq([
            step(|m| nav_to(m, SITE_B, SPEED, RADIUS),
                 |m| arrived(m, SITE_B, RADIUS)),
            once(|m| {
                mission_checkpoint(m, doc, "phase", "site_b");
                emit("ROBOT_SITE_B_REACHED", true);
            }),
            step(|m| nav_to(m, BASE, SPEED, RADIUS),
                 |m| arrived(m, BASE, RADIUS)),
            once(|m| {
                mission_checkpoint(m, doc, "phase", "complete");
                emit("ROBOT_MISSION_COMPLETE", true);
            }),
        ]);
    }

    seq([
        step(|m| nav_to(m, SITE_A, SPEED, RADIUS),
             |m| arrived(m, SITE_A, RADIUS)),
        once(|m| {
            mission_checkpoint(m, doc, "phase", "site_a");
            emit("ROBOT_SITE_A_REACHED", true);
        }),
        step(|m| nav_to(m, SITE_B, SPEED, RADIUS),
             |m| arrived(m, SITE_B, RADIUS)),
        once(|m| {
            mission_checkpoint(m, doc, "phase", "site_b");
            emit("ROBOT_SITE_B_REACHED", true);
        }),
        step(|m| nav_to(m, BASE, SPEED, RADIUS),
             |m| arrived(m, BASE, RADIUS)),
        once(|m| {
            mission_checkpoint(m, doc, "phase", "complete");
            emit("ROBOT_MISSION_COMPLETE", true);
        }),
    ])
}

fn mission(me) {
    [
        objective("site_a", #{
            text: "Survey site A",
            requires_event: "ROBOT_SITE_A_REACHED",
        }),
        objective("site_b", #{
            text: "Survey site B",
            requires_event: "ROBOT_SITE_B_REACHED",
            requires: ["site_a"],
        }),
        objective("return", #{
            text: "Return to base",
            requires_event: "ROBOT_MISSION_COMPLETE",
            requires: ["site_b"],
        }),
    ]
}
