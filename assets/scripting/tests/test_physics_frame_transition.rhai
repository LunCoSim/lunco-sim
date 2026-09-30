// Run after replacing an already-mounted Twin's scene with First Drive.
// Admission must seed the new frame while the old fixed-step boundary is held.

const ROVER_PATH = "/FirstDrive/Rover";

let fails = [];
let rover_id = find_path(ROVER_PATH);
let rover = query("QueryPhysicsState", #{ id: rover_id });
let readiness = query("GetReadiness", #{});
let physics = query("PhysicsPerformance", #{});

fails.push(t_true(rover != () && rover.ok != false,
    "the replacement scene rover resolves through the mounted physics bridge"));
fails.push(t_eq(rover.physics_pose_seeded, true,
    "the replacement scene rover is seeded in the active physics frame"));
fails.push(t_eq(rover.physics_initialization_pending, false,
    "the replacement scene rover passed initial-pose validation"));
fails.push(t_eq(rover.physics_state_pending, false,
    "the replacement scene rover crossed physics admission"));
fails.push(t_eq(rover.physics_state_ready, true,
    "the replacement scene rover is physics-ready"));
fails.push(t_eq(rover.physics_admission_requested, false,
    "the replacement scene no longer holds its dynamic-admission request"));
fails.push(t_eq(rover.body_mode, "dynamic",
    "the replacement scene rover is admitted as dynamic"));
fails.push(t_true(readiness != () && readiness.ready == true
        && readiness.world_hold == false && to_int(readiness.pending_count) == 0,
    "the scene replacement releases the world readiness hold"));
fails.push(t_true(physics != () && physics.ok != false
        && to_int(physics.dynamic_bodies) > 0
        && to_int(physics.step_number) > 0,
    "physics advances with the admitted replacement scene"));

t_report(fails);
