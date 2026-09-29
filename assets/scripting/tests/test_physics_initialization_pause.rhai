// A measured terrain penetration must pause only its articulated object.
// Every paused subject stays at its authored pose while independent bodies
// remain admitted and scene readiness continues.

const RED_BALLOON_PATH = "/SandboxScene/RedBalloon";
const SKID_BATTERY_PATH = "/SandboxScene/Skid_Battery_Thermal_1";
const SKID_PHYSICAL_PATH = "/SandboxScene/Skid_Physical_1";
const JOINT_TEST_BODY_PATH = "/SandboxScene/JointTestB";

let fails = [];
let red_balloon_id = find(RED_BALLOON_PATH);
let red_balloon = query("QueryPhysicsState", #{ id: red_balloon_id });
let red_balloon_prim = query("QueryUsdPrim", #{ path: RED_BALLOON_PATH, attrs: [] });
let skid_battery_id = find_path(SKID_BATTERY_PATH);
let skid_battery = query("QueryPhysicsState", #{ id: skid_battery_id });
let skid_battery_prim = query("QueryUsdPrim", #{ path: SKID_BATTERY_PATH, attrs: [] });
let skid_physical_id = find_path(SKID_PHYSICAL_PATH);
let skid_physical = query("QueryPhysicsState", #{ id: skid_physical_id });
let joint_test_body_id = find(JOINT_TEST_BODY_PATH);
let joint_test_body = query("QueryPhysicsState", #{ id: joint_test_body_id });
let diagnostics = query("RuntimeDiagnostics", #{});
let physics = query("PhysicsPerformance", #{});
let readiness = query("GetReadiness", #{});

let red_balloon_pause_diagnostic = false;
let skid_battery_pause_diagnostic = false;
let paused_subjects = [];
let physics_initialization_errors = 0;
if diagnostics != () && diagnostics.ok != false {
    for finding in diagnostics.findings {
        if finding.producer == "physics-initialization"
            && finding.severity == "error" {
            physics_initialization_errors += 1;
        }
        if finding.code == "physics-initialization-paused"
            && finding.severity == "warning" {
            paused_subjects.push(finding.subject);
            if finding.subject == RED_BALLOON_PATH {
                red_balloon_pause_diagnostic = true;
            }
            if finding.subject == SKID_BATTERY_PATH {
                skid_battery_pause_diagnostic = true;
            }
        }
    }
}

fails.push(t_true(to_int(red_balloon_id) >= 0 && red_balloon != () && red_balloon.ok != false,
    "the authored RedBalloon body is queryable"));
fails.push(t_eq(red_balloon.physics_initialization_pending, false,
    "RedBalloon no longer blocks physics admission"));
fails.push(t_eq(red_balloon.physics_initialization_paused, true,
    "the penetrating RedBalloon is paused by the initialization policy"));
fails.push(t_eq(red_balloon.physics_initialization_invalid, true,
    "RedBalloon remains marked as an invalid authored initial state"));
fails.push(t_eq(red_balloon.physics_object_paused, true,
    "the reusable physics object pause covers RedBalloon"));
fails.push(t_true(red_balloon.physics_initialization_penetration_m > 0.0,
    "RedBalloon retains its measured terrain penetration"));
fails.push(t_eq(red_balloon.physics_state_pending, false,
    "RedBalloon has left the world readiness gate"));
fails.push(t_eq(red_balloon.rigid_body_disabled, true,
    "RedBalloon remains disabled while paused"));
fails.push(t_eq(red_balloon.collider_disabled, true,
    "the RedBalloon body collider is disabled while paused"));
fails.push(t_true(red_balloon_prim != () && red_balloon_prim.ok != false
        && red_balloon_prim.position_frame == "active_physics",
    "the paused RedBalloon keeps a queryable active-frame pose"));
fails.push(t_near(red_balloon_prim.world_position[0], 10.0, 0.001,
    "pause preserves the authored RedBalloon X position"));
fails.push(t_near(red_balloon_prim.world_position[1], 5.0, 0.001,
    "pause preserves the authored RedBalloon height instead of moving it up"));
fails.push(t_near(red_balloon_prim.world_position[2], 0.0, 0.001,
    "pause preserves the authored RedBalloon Z position"));
fails.push(t_true(red_balloon_pause_diagnostic,
    "the paused RedBalloon remains visible in runtime diagnostics"));
fails.push(t_true(skid_battery != () && skid_battery.ok != false,
    "the authored Skid battery body is queryable"));
fails.push(t_eq(skid_battery.physics_initialization_pending, false,
    "the Skid battery no longer blocks physics admission"));
fails.push(t_eq(skid_battery.physics_admission_requested, false,
    "the Skid battery no longer requests dynamic admission while paused"));
fails.push(t_eq(skid_battery.physics_initialization_paused, true,
    "the Skid battery is paused by the initialization policy"));
fails.push(t_eq(skid_battery.physics_object_paused, true,
    "the reusable physics object pause covers the Skid battery"));
fails.push(t_eq(skid_battery.physics_state_pending, false,
    "the Skid battery has left the world readiness gate"));
fails.push(t_eq(skid_battery.rigid_body_disabled, true,
    "the Skid battery remains disabled while paused"));
fails.push(t_eq(skid_battery.collider_disabled, true,
    "the Skid battery body collider is disabled while paused"));
fails.push(t_true(skid_battery_pause_diagnostic,
    "the paused Skid battery remains visible in runtime diagnostics"));
fails.push(t_true(skid_battery_prim != () && skid_battery_prim.ok != false
        && skid_battery_prim.position_frame == "active_physics",
    "the paused Skid battery keeps a queryable pose in the active physics frame"));
fails.push(t_near(skid_battery_prim.world_position[0], -15.0, 0.001,
    "pause preserves the authored Skid battery X position"));
fails.push(t_near(skid_battery_prim.world_position[1], 1.0, 0.001,
    "pause preserves the authored Skid battery height instead of moving it up"));
fails.push(t_near(skid_battery_prim.world_position[2], 0.0, 0.001,
    "pause preserves the authored Skid battery Z position"));
fails.push(t_true(paused_subjects.len() > 0,
    "the measured penetration is visible as a paused subject"));
for subject in paused_subjects {
    let body_id = find_path(subject);
    let body = query("QueryPhysicsState", #{ id: body_id });
    fails.push(t_true(body != () && body.ok != false
            && body.physics_initialization_paused == true
            && body.physics_object_paused == true
            && body.physics_initialization_pending == false
            && body.physics_admission_requested == false
            && body.physics_state_pending == false
            && body.rigid_body_disabled == true
            && body.collider_disabled == true,
        "each policy-paused subject is disabled and retired from readiness: " + subject));
}
fails.push(t_true(skid_physical != () && skid_physical.ok != false
        && skid_physical.physics_pose_seeded == true
        && skid_physical.physics_state_ready == true
        && skid_physical.physics_admission_requested == false
        && skid_physical.body_mode == "dynamic",
    "the formerly held Skid physical body finishes joint admission"));
fails.push(t_true(joint_test_body != () && joint_test_body.ok != false
        && joint_test_body.physics_pose_seeded == true
        && joint_test_body.physics_state_ready == true
        && joint_test_body.physics_admission_requested == false
        && joint_test_body.body_mode == "dynamic",
    "the formerly held JointTest body finishes joint admission"));
fails.push(t_true(readiness != () && readiness.ready == true
        && readiness.world_hold == false && to_int(readiness.pending_count) == 0,
    "paused constraints no longer keep the world readiness hold open"));
fails.push(t_eq(physics_initialization_errors, 0,
    "handled initial penetrations no longer hold the world as errors"));
fails.push(t_true(physics != () && physics.ok != false
        && to_int(physics.step_number) > 0,
    "unaffected scene physics advances after invalid bodies are paused"));

t_report(fails);
