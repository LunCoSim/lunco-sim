// Production acceptance for the shared articulated antenna on dynamic,
// static, and independently authored hosts. Each joint must attach and slew.
// The passive antenna control must contain no rigid bodies or joint mechanism.

// The mounts under test. Every one is addressed as the ANTENNA prim the scene
// authors plus the joint the COMPONENT owns, so this file never restates the
// component's interior layout beyond the one prim that is its interface.
fn mounts() {
    [
        #{ id: "rover",  path: "/AntennaMountTest/Rover/Comms" },
        #{ id: "mast",   path: "/AntennaMountTest/Mast/Antenna" },
        #{ id: "bare",   path: "/AntennaMountTest/Bare/Antenna" },
    ]
}

fn simulation_dependencies(me, ctx) {
    let reads = [];
    for mount in mounts() {
        reads.push(find_path(mount.path + "/YawJoint"));
    }
    #{ modelica_entities: [], entity_reads: reads, entity_writes: [], query_reads: ["QueryUsdPrim"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.t = 0.0;
    // Joints resolve over several ticks — avian must admit both bodies into its
    // island graph, and the physics-transform bridge must have written a real
    // pose — so a verdict taken early measures load order, not the mount. The
    // tracker also needs a few ticks of Goldstone direction before the dish moves at
    // all, which is what `slewed` below is reading.
    this.settle_s = 12.0;
    this.phase = "settle";
    // Peak |angle| per mount, accumulated across the run. A running maximum, not
    // the final sample: a dish that slews and then settles has still slewed, and
    // reading only the last tick would call that motionless.
    this.peak = #{};
    for m in mounts() { this.peak[m.id] = 0.0; }
    print("[mount] ── one antenna, three hosts, one parked control ─────");
}

fn on_tick(me, ctx) {
    if this.phase == "done" { return; }
    this.t += dt();

    for m in mounts() {
        let a = get(find_path(m.path + "/YawJoint"), "angle");
        if a != () && a.abs() > this.peak[m.id] { this.peak[m.id] = a.abs(); }
    }
    if this.t < this.settle_s { return; }
    this.phase = "done";

    // Snapshot every fact the verdict reads, so a failure is diagnosable from the
    // log alone rather than needing the scene reloaded under a debugger.
    let seen = [];
    for m in mounts() {
        let j = find_path(m.path + "/YawJoint");
        seen.push(#{
            id: m.id,
            host: find_path(m.path),
            joint: j,
            angle: get(j, "angle"),
            peak: this.peak[m.id],
        });
    }
    let parked_path = "/AntennaMountTest/Parked/Antenna/YawJoint";
    let parked_prim = query("QueryUsdPrim", #{ path: parked_path });
    let parked_head = query("QueryUsdPrim", #{
        path: "/AntennaMountTest/Parked/Antenna/YawHead", schemas: true,
    });
    verdict(#{
        mounts: seen,
        parked_host: find_path("/AntennaMountTest/Parked/Antenna"),
        parked_joint: find_path(parked_path),
        parked_joint_absent: parked_prim.ok == false,
        parked_body_absent: parked_head.api_schemas != ()
            && !parked_head.api_schemas.contains("PhysicsRigidBodyAPI"),
    });
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   host present    a mount asserted against a host that failed to compose
//                   would report the component broken when the SCENE is.
//   joint present   the component's own prim came in with the reference.
//   angle port      the port exists ONLY on an entity carrying a live
//                   `RevoluteJoint`, so asking for it IS asking whether the
//                   joint attached. This is the check the whole file is for.
//   slewed          and it MOVED. A joint can attach and still be inert if the
//                   controller never reaches it; without this the test passes on
//                   a dish welded at its authored pose.
//   control parked  the joint that was switched off must have NO angle port and
//                   must not have moved.
fn verdict(s) {
    let f = [];

    for m in s.mounts {
        f.push(t_true(m.host != () && m.host >= 0,
            "mount `" + m.id + "`: the host prim is not in the scene — this " +
            "scene failed to compose, so nothing below is about the antenna"));

        f.push(t_true(m.joint != () && m.joint >= 0,
            "mount `" + m.id + "`: the component's `YawJoint` prim is missing. " +
            "The antenna owns its own mount joint; if the prim is absent the " +
            "reference did not compose."));

        f.push(t_present(m.angle,
            "mount `" + m.id + "`: YawJoint `angle` port. The port exists only " +
            "on an entity carrying a live RevoluteJoint, so this means the " +
            "joint never ATTACHED — the dish is frozen and nothing says so."));

        // 1 mrad. Station tracking is a slow slew, so the bar is only meant to
        // separate "moving" from "welded": a mount that never attached reads
        // exactly 0.000, not a small number.
        f.push(t_true(m.peak > 0.001,
            "mount `" + m.id + "`: the dish never slewed (peak |angle| = " +
            r4(m.peak) + " rad). The joint exists but nothing drives it, so the " +
            "antenna is decorative — which is indistinguishable from a working " +
            "one in every other test."));
    }

    // ── THE CONTROL ──
    f.push(t_true(s.parked_host != () && s.parked_host >= 0,
        "the parked control's host is not in the scene, so the control proves " +
        "nothing — an absent mount and an absent HOST read identically here."));

    f.push(t_true(s.parked_joint_absent && s.parked_body_absent,
        "the passive control must not declare an articulated joint or rigid body."));

    f.push(t_true(s.parked_joint == -1,
        "the passive control projected a joint into live simulation."));

    print("[mount] ── results ─────────────────────────────────────────");
    for m in s.mounts {
        print("[mount]   " + m.id + "  angle=" + m.angle + "  peak=" + r4(m.peak));
    }
    print("[mount]   parked  body_absent=" + s.parked_body_absent + "  runtime entity=" +
          s.parked_joint + " (must be absent)");

    report_verdict(f, "ANTENNA MOUNT", "ANTENNA_MOUNT");
}
