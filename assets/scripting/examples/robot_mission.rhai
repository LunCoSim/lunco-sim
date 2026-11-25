// A production-style robot mission: no on_tick loop. The task tree owns
// sequencing, while the mission declaration owns success/failure reporting.
// Attach this to the mounted rover whose pose and drive ports control the survey.

const SITE_A = [12.0, 0.0,  0.0];
const SITE_B = [12.0, 0.0, 25.0];
const BASE   = [ 0.0, 0.0,  0.0];
const SPEED  = 1.0;
const RADIUS = 2.0;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [me], entity_writes: [me],
       query_reads: [], required_inputs: [] }
}

const PROGRESS_ID = "robot-survey-v1";
fn mission_progress() { #{ id: PROGRESS_ID, save: true, restore: true } }

// Each terminal objective records one arrival. A resumed task starts at
// the first outstanding leg and never re-emits arrivals for finished legs.
fn task(me, ctx) {
    let legs = [];
    for leg in [
        ["site_a", SITE_A, "ROBOT_SITE_A_REACHED"],
        ["site_b", SITE_B, "ROBOT_SITE_B_REACHED"],
        ["return", BASE, "ROBOT_MISSION_COMPLETE"],
    ] {
        if mission_checkpoint_read(PROGRESS_ID, "objective:" + leg[0]) == "complete" { continue; }
        let point = leg[1];
        let signal = leg[2];
        legs.push(step(|m| nav_to(m, point, SPEED, RADIUS),
                       |m| arrived(m, point, RADIUS)));
        legs.push(once(|m| emit(signal, true)));
    }
    seq(legs)
}

fn mission(me, ctx) {
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
