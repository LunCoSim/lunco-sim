// ── Sandbox ramp placement regression ───────────────────────────────────────
//
// The interactive sandbox owns the ramp geometry and the rover spawn.  This
// focused test composes that real scene, verifies a rover authored inside the
// ramp is paused in place, and drives a second rover from the flat approach
// across the ramp to check its authored grade in the observed world pose.

const SETTLE_S = 2.0;
const DRIVE_S = 28.0;
// This is a contact regression, not a hill-climbing performance target.  The
// rover must reach the ramp's rising face and gain a measurable height; the
// remaining high-grade traverse is covered by the dedicated driving lesson.
const MIN_PROGRESS_M = 15.0;
const MIN_RISE_M = 1.0;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path("/SandboxRampPlacement/Skid_Raycast_1")],
       entity_reads: [find_path("/SandboxRampPlacement/Skid_Raycast_2")],
       entity_writes: [],
       query_reads: ["QueryPhysicsState", "QueryUsdPrim", "RuntimeDiagnostics"],
       required_inputs: [] }
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn verdict(pass, reason) {
    if pass {
        emit("SANDBOX_RAMP_PLACEMENT", "PASS");
    } else {
        print("SANDBOX_RAMP_PLACEMENT FAIL: " + reason);
        emit("SANDBOX_RAMP_PLACEMENT", "FAIL");
    }
}

fn on_start(me, ctx) {
    this.t = 0.0;
    this.done = false;
    this.rover = find_path("/SandboxRampPlacement/Skid_Raycast_1");
    this.ramp = find_path("/SandboxRampPlacement/Ramp");
    this.start = ();
    this.min_y = 0.0;
    this.max_x = 0.0;
    this.possession = ();
    this.drive_command = ();
    this.slope_rover = find_path("/SandboxRampPlacement/Skid_Raycast_2");
    this.slope_rover_checked = false;
    this.slope_rover_paused_in_place = false;
    if this.rover >= 0 && this.ramp >= 0 {
        this.start = world_pos(this.rover);
        if this.start != () {
            this.min_y = this.start[1];
            this.max_x = this.start[0];
        }
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.t += dt();

    if !this.slope_rover_checked && this.slope_rover >= 0 {
        let state = query("QueryPhysicsState", #{ id: this.slope_rover });
        if query_ok(state) && state.physics_initialization_pending == false {
            let pose = query("QueryUsdPrim", #{
                path: "/SandboxRampPlacement/Skid_Raycast_2", attrs: [],
            });
            let diagnostics = query("RuntimeDiagnostics", #{});
            let has_warning = false;
            if query_ok(diagnostics) {
                for finding in diagnostics.findings {
                    if finding.code == "physics-initialization-paused"
                        && finding.severity == "warning"
                        && finding.subject == "/SandboxRampPlacement/Skid_Raycast_2" {
                        has_warning = true;
                    }
                }
            }
            this.slope_rover_paused_in_place =
                state.physics_initialization_paused == true
                && state.physics_object_paused == true
                && state.physics_state_pending == false
                && state.physics_initialization_penetration_m > 0.0
                && state.rigid_body_disabled == true
                && state.collider_disabled == true
                && query_ok(pose) && pose.position_frame == "active_physics"
                && (pose.world_position[0] - 15.0).abs() <= 0.001
                && (pose.world_position[1] - 5.0).abs() <= 0.001
                && (pose.world_position[2] - 10.0).abs() <= 0.001
                && has_warning;
            this.slope_rover_checked = true;
            print("[sandbox-ramp] authored slope rover paused in place="
                + this.slope_rover_paused_in_place);
        }
    }

    if this.rover < 0 || this.ramp < 0 || this.start == () {
        this.done = true;
        verdict(false, "focused sandbox ramp fixture is missing its rover, ramp, or start pose");
        return;
    }

    if this.t < SETTLE_S {
        return;
    }

    if this.possession == () {
        this.possession = cmd("AcquireControl", #{
            target: this.rover, bind_camera: false,
        });
    }
    let command = drive(this.rover, 0.55, 0.0);
    if this.drive_command == () { this.drive_command = command; }
    let p = world_pos(this.rover);
    if p != () {
        if p[1] < this.min_y { this.min_y = p[1]; }
        if p[0] > this.max_x { this.max_x = p[0]; }
    }

    if this.t >= SETTLE_S + DRIVE_S {
        this.done = true;
        drive(this.rover, 0.0, 0.0);
        if this.possession == () || this.possession.ok != true ||
           this.drive_command == () || this.drive_command.ok != true ||
           this.drive_command.status != "applied" {
            verdict(false, "rover control claim or throttle command was not applied");
            return;
        }
        if p == () {
            verdict(false, "rover lost its world pose during the ramp traverse");
            return;
        }
        let rise = p[1] - this.start[1];
        let progress = p[0] - this.start[0];
        print("[sandbox-ramp] start=" + this.start + " final=" + p +
              " progress_x=" + progress + " rise_y=" + rise +
              " min_y=" + this.min_y + " max_x=" + this.max_x);
        if !this.slope_rover_checked || !this.slope_rover_paused_in_place {
            verdict(false, "the rover authored inside the ramp was not paused in place with a diagnostic");
            return;
        }
        verdict(progress > MIN_PROGRESS_M && rise > MIN_RISE_M,
            "rover did not climb the authored ramp (progress_x=" + progress +
            ", rise_y=" + rise + ")");
    }
}
