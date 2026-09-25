// TEST-ONLY. Runs against `scenes/tests/sun_tracker.usda`.
// The test sends a static `sun` ray and a moving `spacecraft_b` point target
// through one EnvironmentProbe. Both consumers expose only `target_mount_xyz`;
// the provider wire selects the source. It checks unit length, frame conversion,
// Modelica response, and the final joint motion through the production scene
// test binary.

const STEP_YAW = 1.0;      // radians the static sun ray is moved to (~57 deg)
const TOL_RAD = 0.15;      // servo convergence band (~8.6 deg)
const PORT_WAIT_S = 20.0;  // budget for the Modelica model to compile and publish

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [
        find_path("/SunTrackerTest/SolarTower/Controller"),
        find_path("/SunTrackerTest/SpacecraftController"),
    ], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.t = 0.0;
    // Wait for Modelica publication before sampling the direction triplets.
    this.phase = "await_ports";
    this.phase_start = 0.0;
    this.settle_s = 2.0;    // after the ports appear, let the model reach steady state
    this.track_s = 12.0;    // first-order servo: give it time to slew and settle

    this.ctrl = find_path("/SunTrackerTest/SolarTower/Controller");
    this.spacecraft_ctrl = find_path("/SunTrackerTest/SpacecraftController");
    this.spacecraft_target = find_path("/SunTrackerTest/SpacecraftTarget");
    this.hinge = find_path("/SunTrackerTest/SolarTower/Hinge");
    this.mast = find_path("/SunTrackerTest/SolarTower/Mast");
    this.head = find_path("/SunTrackerTest/SolarTower/YawHead");

    this.az_before = ();
    this.direction_y_before = ();
    this.direction_z_before = ();
    this.yaw_before = ();
    this.mast_forward_before = ();
    this.head_forward_before = ();
    this.spacecraft_x_before = ();
    this.spacecraft_y_before = ();
    this.spacecraft_z_before = ();
    this.spacecraft_yaw_before = ();
    this.spacecraft_x_after = ();
    this.spacecraft_y_after = ();
    this.spacecraft_z_after = ();
    this.spacecraft_yaw_after = ();

    print("[direction] ── static ray and spacecraft target ───────────────");
    print("[direction] sun controller gid=" + this.ctrl + " hinge gid=" + this.hinge +
          " spacecraft controller gid=" + this.spacecraft_ctrl);
}

fn on_tick(me, ctx) {
    if this.phase == "done" { return; }

    if this.ctrl == () || this.ctrl < 0 || this.hinge == () || this.hinge < 0 ||
       this.mast == () || this.mast < 0 || this.head == () || this.head < 0 ||
       this.spacecraft_ctrl == () || this.spacecraft_ctrl < 0 ||
       this.spacecraft_target == () || this.spacecraft_target < 0 {
        fail_fast("sun tracker assembly prims not found (ctrl=" + this.ctrl +
                  " hinge=" + this.hinge + " mast=" + this.mast + " head=" +
                  this.head + " spacecraft_ctrl=" + this.spacecraft_ctrl +
                  " spacecraft_target=" + this.spacecraft_target +
                  ") — wrong scene?", "SUN TRACKER", "SUN_TRACKER");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let leg = this.t - this.phase_start;

    if this.phase == "await_ports" {
        if get(this.ctrl, "target_mount_x") != () && get(this.ctrl, "yaw") != () &&
           get(this.spacecraft_ctrl, "target_mount_x") != () &&
           get(this.spacecraft_ctrl, "yaw") != () {
            print("[sun] ports live at t=" + r2(this.t) + "s");
            this.phase = "settle";
            this.phase_start = this.t;
        } else if this.t > PORT_WAIT_S {
            fail_fast("Controller published no target_mount_x/yaw within " +
                      PORT_WAIT_S + "s — SunTracker.mo never compiled, or its " +
                      "outputs are not exposed as ports", "SUN TRACKER", "SUN_TRACKER");
            this.phase = "done";
        }
    } else if this.phase == "settle" {
        if leg >= this.settle_s {
            this.az_before = get(this.ctrl, "target_mount_x");
            this.direction_y_before = get(this.ctrl, "target_mount_y");
            this.direction_z_before = get(this.ctrl, "target_mount_z");
            this.yaw_before = get(this.ctrl, "yaw");
            this.mast_forward_before = world_forward(this.mast);
            this.head_forward_before = world_forward(this.head);
            this.spacecraft_x_before = get(this.spacecraft_ctrl, "target_mount_x");
            this.spacecraft_y_before = get(this.spacecraft_ctrl, "target_mount_y");
            this.spacecraft_z_before = get(this.spacecraft_ctrl, "target_mount_z");
            this.spacecraft_yaw_before = get(this.spacecraft_ctrl, "yaw");
            let moved = cmd("MoveEntity", #{ entity_id: this.spacecraft_target,
                                                translation: [0.0, 0.0, -10.0] });
            if moved == () || moved.ok == false {
                fail_fast("could not move the tagged spacecraft direction target",
                    "SUN TRACKER", "SUN_TRACKER");
                this.phase = "done";
                return;
            }
            print("[sun] before the step: target_mount_x=" + r4(this.az_before) +
                  " yaw=" + r4(this.yaw_before) + " angle=" + r4(get(this.hinge, "angle")));
            this.phase = "spacecraft_track";
            this.phase_start = this.t;
        }
    } else if this.phase == "spacecraft_track" {
        if leg >= this.track_s {
            this.spacecraft_x_after = get(this.spacecraft_ctrl, "target_mount_x");
            this.spacecraft_y_after = get(this.spacecraft_ctrl, "target_mount_y");
            this.spacecraft_z_after = get(this.spacecraft_ctrl, "target_mount_z");
            this.spacecraft_yaw_after = get(this.spacecraft_ctrl, "yaw");
            cmd("SetEnvironmentLight", #{ sun_yaw: STEP_YAW });
            print("[sun] step: SetEnvironmentLight sun_yaw=" + STEP_YAW + " rad");
            this.phase = "sun_track";
            this.phase_start = this.t;
        } else if leg % 4.0 < dt() {
            print("[direction] spacecraft vector=" +
                  r4(get(this.spacecraft_ctrl, "target_mount_x")) + "," +
                  r4(get(this.spacecraft_ctrl, "target_mount_y")) + "," +
                  r4(get(this.spacecraft_ctrl, "target_mount_z")) +
                  " yaw=" + r4(get(this.spacecraft_ctrl, "yaw")));
        }
    } else if this.phase == "sun_track" {
        if leg >= this.track_s {
            this.phase = "done";
            verdict(#{
                az_before: this.az_before,
                direction_y_before: this.direction_y_before,
                direction_z_before: this.direction_z_before,
                spacecraft_x_before: this.spacecraft_x_before,
                spacecraft_y_before: this.spacecraft_y_before,
                spacecraft_z_before: this.spacecraft_z_before,
                spacecraft_yaw_before: this.spacecraft_yaw_before,
                spacecraft_x_after: this.spacecraft_x_after,
                spacecraft_y_after: this.spacecraft_y_after,
                spacecraft_z_after: this.spacecraft_z_after,
                spacecraft_yaw_after: this.spacecraft_yaw_after,
                yaw_before: this.yaw_before,
                az: get(this.ctrl, "target_mount_x"),
                direction_y: get(this.ctrl, "target_mount_y"),
                direction_z: get(this.ctrl, "target_mount_z"),
                yaw: get(this.ctrl, "yaw"),
                angle: get(this.hinge, "angle"),
                mast_forward: world_forward(this.mast),
                head_forward: world_forward(this.head),
                mast_forward_before: this.mast_forward_before,
                head_forward_before: this.head_forward_before,
            });
        } else if leg % 4.0 < dt() {
            print("[sun] t=" + r2(this.t) + "s  target_mount_x=" + r4(get(this.ctrl, "target_mount_x")) +
                  " yaw=" + r4(get(this.ctrl, "yaw")) + " angle=" + r4(get(this.hinge, "angle")));
        }
    }
}

// The verdict requires observed input samples, a changed bearing, Modelica
// response, and matching joint motion.
fn verdict(s) {
    let f = [];
    f.push(t_present(s.az, "Controller target_mount_x port"));
    f.push(t_present(s.direction_y, "Controller target_mount_y port"));
    f.push(t_present(s.direction_z, "Controller target_mount_z port"));
    f.push(t_present(s.direction_y_before, "Controller target_mount_y initial sample"));
    f.push(t_present(s.direction_z_before, "Controller target_mount_z initial sample"));
    f.push(t_present(s.yaw, "Controller yaw port"));
    f.push(t_present(s.angle, "Hinge angle port"));
    f.push(t_present(s.mast_forward, "Mast world orientation"));
    f.push(t_present(s.head_forward, "YawHead world orientation"));
    f.push(t_present(s.spacecraft_x_before, "spacecraft target_mount_x before move"));
    f.push(t_present(s.spacecraft_y_before, "spacecraft target_mount_y before move"));
    f.push(t_present(s.spacecraft_z_before, "spacecraft target_mount_z before move"));
    f.push(t_present(s.spacecraft_yaw_before, "spacecraft yaw before target move"));
    f.push(t_present(s.spacecraft_x_after, "spacecraft target_mount_x after move"));
    f.push(t_present(s.spacecraft_y_after, "spacecraft target_mount_y after move"));
    f.push(t_present(s.spacecraft_z_after, "spacecraft target_mount_z after move"));
    f.push(t_present(s.spacecraft_yaw_after, "spacecraft yaw after target move"));

    if s.az != () && s.az_before != () {
        f.push(t_true((s.az - s.az_before).abs() > 0.1,
            "the environment probe did not see its source change: target_mount_x went from " +
            r4(s.az_before) + " to " + r4(s.az) + " after a " + STEP_YAW +
            " rad step"));
    }
    if s.az_before != () && s.direction_y_before != () && s.direction_z_before != ()
       && s.az != () && s.direction_y != () && s.direction_z != () {
        let before_norm_squared = norm_squared([s.az_before, s.direction_y_before, s.direction_z_before]);
        let after_norm_squared = norm_squared([s.az, s.direction_y, s.direction_z]);
        f.push(t_true((before_norm_squared - 1.0).abs() < 1.0e-10,
            "direction provider did not publish a unit vector before the step: norm²=" + before_norm_squared));
        f.push(t_true((after_norm_squared - 1.0).abs() < 1.0e-10,
            "direction provider did not publish a unit vector after the step: norm²=" + after_norm_squared));
    }
    if s.yaw != () && s.yaw_before != () {
        f.push(t_true((s.yaw - s.yaw_before).abs() > 0.2,
            "the panel never slewed: yaw went from " + r4(s.yaw_before) + " to " +
            r4(s.yaw) + " rad across the step. A controller already sitting where " +
            "the sun ended up agrees with it for free."));
    }

    if s.spacecraft_x_before != () && s.spacecraft_y_before != () &&
       s.spacecraft_z_before != () && s.spacecraft_x_after != () &&
       s.spacecraft_y_after != () && s.spacecraft_z_after != () {
        let before_norm_squared = norm_squared([s.spacecraft_x_before, s.spacecraft_y_before, s.spacecraft_z_before]);
        let after_norm_squared = norm_squared([s.spacecraft_x_after, s.spacecraft_y_after, s.spacecraft_z_after]);
        f.push(t_true((before_norm_squared - 1.0).abs() < 1.0e-10,
            "arbitrary spacecraft bearing was not unit length before motion: norm²=" + before_norm_squared));
        f.push(t_true((after_norm_squared - 1.0).abs() < 1.0e-10,
            "arbitrary spacecraft bearing was not unit length after motion: norm²=" + after_norm_squared));
        f.push(t_true(s.spacecraft_x_before > 0.99 && s.spacecraft_y_before.abs() < 1.0e-6 &&
                      s.spacecraft_z_before.abs() < 1.0e-6,
            "the first spacecraft target should be +X from the probe; got " +
            s.spacecraft_x_before + "," + s.spacecraft_y_before + "," + s.spacecraft_z_before));
        f.push(t_true(s.spacecraft_z_after < -0.99 && s.spacecraft_x_after.abs() < 1.0e-6 &&
                      s.spacecraft_y_after.abs() < 1.0e-6,
            "moving the target spacecraft to -Z did not update its observer-frame bearing; got " +
            s.spacecraft_x_after + "," + s.spacecraft_y_after + "," + s.spacecraft_z_after));
    }
    if s.spacecraft_yaw_before != () && s.spacecraft_yaw_after != () {
        f.push(t_true((s.spacecraft_yaw_after - s.spacecraft_yaw_before).abs() > 0.2,
            "the generic spacecraft target did not drive its Modelica consumer"));
    }

    if s.angle != () && s.yaw != () {
        f.push(t_true((s.angle - s.yaw).abs() < TOL_RAD,
            "the joint did not follow the controller: hinge angle=" + r4(s.angle) +
            " against yaw=" + r4(s.yaw) + " rad. The cross-prim " +
            "`inputs:angle.connect` is not reaching the joint's motor — the " +
            "controller can be perfect and the panel still never move."));
    }

    if s.mast_forward != () && s.mast_forward_before != () {
        f.push(t_true(vdot(vnorm(s.mast_forward), vnorm(s.mast_forward_before)) > 0.9999,
            "the fixed mast rotated with the assembly; the pole must remain static while " +
            "the panel head turns on the revolute joint"));
    }
    if s.head_forward != () && s.head_forward_before != () {
        f.push(t_true(vdot(vnorm(s.head_forward), vnorm(s.head_forward_before)) < 0.995,
            "the solar-panel head did not rotate; the panel must be parented to the " +
            "revolute-joint body1"));
    }

    print("[sun] ── results ───────────────────────────────────────────");
    print("[sun]   target_mount_x  " + r4(s.az_before) + " -> " + r4(s.az));
    print("[sun]   yaw          " + r4(s.yaw_before) + " -> " + r4(s.yaw) + " rad");
    print("[sun]   hinge angle  " + r4(s.angle) + " rad");

    report_verdict(f, "SUN TRACKER", "SUN_TRACKER");
}
