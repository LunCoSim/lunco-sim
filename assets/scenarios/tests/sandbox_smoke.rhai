// ── Sandbox smoke contract ──────────────────────────────────────────────────
//
// `sandbox_scene.usda` is an interactive arena. It deliberately has no
// scenario and therefore correctly returns NO-VERDICT when passed directly to
// `luncosim test`; loading an arena is not a test assertion. This script is the
// explicit release/CI contract for the same composed scene.
//
// It checks independent, observable invariants:
//   * the Modelica oscillator compiled and published a real signal;
//   * the fixed-joint demo retained its authored separation;
//   * the composed mount-frame demo and cosim sink have live world positions.
// Python demos are not part of this contract because Python is an optional
// runtime and an unavailable optional interpreter must not turn the smoke gate
// into a false pass or an unrelated failure.

const WAIT_S = 30.0;

fn query_ok(record) {
    record != () && record.ok != false
}

fn close_vec(actual, expected, tolerance) {
    if actual == () || expected == () || actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if (actual[i] - expected[i]).abs() > tolerance { return false; }
    }
    true
}

fn rover_topology_ok(path) {
    let record = query("QueryUsdPrim", #{ path: path, topology: true });
    if !query_ok(record) || record.topology == () { return false; }
    if record.topology.selection.scope != path
        || record.topology.projection.source != "live_stage"
        || record.topology.binding.rigid_body != true
        || record.topology.parts == () {
        return false;
    }
    let chassis = false;
    for part in record.topology.parts {
        if part.path == path + "/Chassis" {
            chassis = part.visual == true && part.collider == true
                && part.collision_enabled == true
                && part.body_owner == path;
        }
    }
    chassis
}

fn rover_contract(path, expected_raycast, expected_physical) {
    let entity = find(path);
    if entity == () || entity < 0 { return false; }
    let wheels = vessel_wheel_summary(entity);
    let controls = get(entity, "throttle") != ()
        && get(entity, "steer") != ()
        && get(entity, "brake") != ()
        && get(entity, "drive_left") != ()
        && get(entity, "drive_right") != ();
    wheels.raycast == expected_raycast
        && wheels.physical == expected_physical
        && wheels.wheels == expected_raycast + expected_physical
        && (if expected_raycast > 0 {
            wheels.raycast_radius != ()
                && wheels.suspension_rest != ()
                && wheels.suspension_spring != ()
                && wheels.suspension_damping != ()
        } else {
            wheels.physical_radius != ()
        })
        && vessel_wheels_match(entity, expected_raycast, expected_physical,
            0.4, 0.7, 15000.0, 3000.0, 1.5)
        && controls
        && rover_topology_ok(path)
}

fn wiring_contract() {
    let generated = query("GeneratedModelicaSource", #{
        network_root: "/SandboxScene/Skid_Battery_Thermal_1",
    });
    let network = false;
    if generated != () && generated.networks != () && generated.networks.len() == 1 {
        let n = generated.networks[0];
        network = n.projection_error == ()
            && n.boundary_inputs.contains("throttle")
            && n.boundary_inputs.contains("steer")
            && n.boundary_outputs.contains("soc")
            && n.members.len() >= 5;
    }
    let binding = query("BindingStatus");
    let oscillator_to_amplifier = false;
    let amplifier_to_target = false;
    if query_ok(binding) && binding.connections != () {
        for edge in binding.connections {
            if edge.source.usd_path == "/SandboxScene/Oscillator"
                && edge.source_port == "signal"
                && edge.sink.usd_path == "/SandboxScene/Amplifier"
                && edge.sink_port == "signal" {
                oscillator_to_amplifier = true;
            }
            if edge.source.usd_path == "/SandboxScene/Amplifier"
                && edge.source_port == "scaled"
                && edge.sink.usd_path == "/SandboxScene/CosimTarget"
                && edge.sink_port == "force_y" {
                amplifier_to_target = true;
            }
        }
    }
    network && oscillator_to_amplifier && amplifier_to_target
}

fn scene_contract() {
    let ground = query("QueryUsdPrim", #{
        path: "/SandboxScene/Ground",
        attrs: ["xformOp:scale", "physics:collisionEnabled"],
    });
    let ramp = query("QueryUsdPrim", #{
        path: "/SandboxScene/Ramp",
        attrs: ["xformOp:translate", "xformOp:scale", "physics:collisionEnabled"],
    });
    let piston = query("QueryUsdPrim", #{
        path: "/SandboxScene/Skid_Raycast_1/Suspension_FL/Wheel_FL/SuspensionPiston",
        attrs: ["lunco:suspensionVisual:role"],
    });
    let spring = query("QueryUsdPrim", #{
        path: "/SandboxScene/Skid_Raycast_1/Suspension_FL/Wheel_FL/SuspensionSpring",
        attrs: ["lunco:suspensionVisual:role"],
    });
    let casing = query("QueryUsdPrim", #{
        path: "/SandboxScene/Skid_Raycast_1/Suspension_FL/Wheel_FL/SuspensionCasing",
        attrs: ["lunco:suspensionVisual:role"],
    });
    let livery = query("QueryUsdPrim", #{
        path: "/SandboxScene/Skid_Raycast_1/Chassis",
        attrs: ["primvars:displayColor"],
    });
    let roots = [
        ["/SandboxScene/Skid_Raycast_1", 4, 0],
        ["/SandboxScene/Skid_Raycast_2", 4, 0],
        ["/SandboxScene/Skid_Physical_1", 0, 4],
        ["/SandboxScene/Ackermann_Raycast_1", 4, 0],
        ["/SandboxScene/Ackermann_Physical_1", 0, 4],
        ["/SandboxScene/Skid_Battery_Thermal_1", 4, 0],
    ];
    let rovers = true;
    for spec in roots {
        rovers = rovers && rover_contract(spec[0], spec[1], spec[2]);
    }
    let ground_ok = query_ok(ground) && ground.attrs["physics:collisionEnabled"] == true
        && close_vec(ground.attrs["xformOp:scale"], [4000.0, 1.0, 4000.0], 0.001);
    let ramp_ok = query_ok(ramp) && ramp.attrs["physics:collisionEnabled"] == true
        && close_vec(ramp.attrs["xformOp:translate"], [25.0, 7.91, 0.0], 0.001)
        && close_vec(ramp.attrs["xformOp:scale"], [60.0, 2.0, 80.0], 0.001);
    let suspension_visuals_ok = query_ok(piston) && query_ok(spring) && query_ok(casing)
        && piston.attrs["lunco:suspensionVisual:role"] == "piston"
        && spring.attrs["lunco:suspensionVisual:role"] == "spring"
        && casing.attrs["lunco:suspensionVisual:role"] == "casing";
    let livery_color = if query_ok(livery) { livery.attrs["primvars:displayColor"] } else { () };
    let livery_ok = livery_color != () && livery_color.len() == 1
        && close_vec(livery_color[0], [0.85, 0.15, 0.12], 0.001);
    ground_ok && ramp_ok && rovers && suspension_visuals_ok && livery_ok
        && wiring_contract()
}

fn on_start(me) {
    this.t = 0.0;
    this.done = false;
    this.osc = find("/SandboxScene/Oscillator");
    this.target = find("/SandboxScene/CosimTarget");
    this.joint_a = find("/SandboxScene/JointTestA");
    this.joint_b = find("/SandboxScene/JointTestB");
    this.base = find("/SandboxScene/Base");
    this.arm = find("/SandboxScene/Base/Arm");
    this.scene_checked = false;
    print("[sandbox-smoke] ── composed arena smoke contract ─────────────");
}

fn fail(message) {
    this.done = true;
    fail_fast(message, "SANDBOX SMOKE", "SANDBOX_SMOKE");
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();

    if this.osc < 0 || this.target < 0 || this.joint_a < 0 ||
       this.joint_b < 0 || this.base < 0 || this.arm < 0 {
        fail("required composed prim missing from sandbox smoke scene");
        return;
    }

    let signal = get(this.osc, "signal");
    if signal == () {
        if this.t >= WAIT_S {
            fail("Modelica oscillator did not publish `signal` within " + WAIT_S + "s");
        }
        return;
    }

    this.scene_checked = scene_contract();
    this.done = true;
    verdict(#{
        signal: signal,
        target_pos: world_pos(this.target),
        joint_a_pos: world_pos(this.joint_a),
        joint_b_pos: world_pos(this.joint_b),
        arm_pos: world_pos(this.arm),
        joint_gap: distance(this.joint_a, this.joint_b),
        scene_contract: this.scene_checked,
    });
}

fn verdict(s) {
    let f = [];
    f.push(t_range(s.signal, -1.01, 1.01,
        "Modelica oscillator signal outside authored amplitude"));
    f.push(t_bounded(s.target_pos, 1000.0,
        "cosim target left the composed arena"));
    f.push(t_bounded(s.joint_a_pos, 1000.0,
        "fixed-joint body A has no finite world position"));
    f.push(t_bounded(s.joint_b_pos, 1000.0,
        "fixed-joint body B has no finite world position"));
    f.push(t_bounded(s.arm_pos, 1000.0,
        "mounted arm has no finite world position"));
    f.push(t_rel(s.joint_gap, 3.0, 5.0,
        "fixed-joint bodies lost their authored 3 m separation"));
    f.push(t_true(s.scene_contract,
        "sandbox USD topology, wheel realization, authored collision, or Modelica wiring " +
        "did not survive the production projection"));

    print("[sandbox-smoke] ── results ─────────────────────────────────");
    print("[sandbox-smoke]   oscillator signal = " + r4(s.signal));
    print("[sandbox-smoke]   fixed-joint gap    = " + r4(s.joint_gap) + " m");
    report_verdict(f, "SANDBOX SMOKE", "SANDBOX_SMOKE");
}
