// ── The antenna mounts on anything, and nothing mounts it ───────────────────
//
// TEST-ONLY. Runs against `scenes/tests/antenna_mount.usda`.
//
// WHAT IT GUARDS. `components/comms/antenna.usda` owns both its hinges and
// mounts by ANCESTRY: its `YawJoint` names its own root — a plain `Xform`, not a
// body — which resolves to the nearest ancestor rigid body, i.e. whatever the
// component was parented under. The host authors a `translate` and nothing else.
//
// The failure this exists to catch has no symptom. A yaw joint that never
// attaches leaves the dish standing exactly where it was authored, pointing
// somewhere plausible, on a vehicle that otherwise behaves perfectly. Every
// other test in the project stayed green through it for as long as it existed:
// `rocker_bogie` passed 9/9 with a dead antenna. The only trace was one `warn!`
// about the `angle` port, in a log nothing reads.
//
// So the assertion is not "the prim is there" — it is that the joint ATTACHED
// and the dish is ACTUALLY SLEWING, on every kind of host at once.
//
// ── Why three hosts in one scene ────────────────────────────────────────────
//
//   Rover   dynamic body, antenna is a direct child
//   Mast    STATIC body, antenna is a SIBLING of the tower it stands on
//   Bare    a body the SCENE invents — nothing from the catalogue at all
//
// One host would prove one host. The mast is the case that actually broke: its
// tower is not an ancestor of the antenna, so the mount resolves only because
// the mast's ROOT carries `PhysicsRigidBodyAPI`. Take that off and this is the
// only thing in the project that notices. `Bare` proves the other direction —
// that the component needs nothing of a host but a body to hang under.
//
// ── THE CONTROL ─────────────────────────────────────────────────────────────
//
// `Parked` is the same component on the same kind of body with the joint
// switched off (`over "YawJoint" ( active = false )`). Without it, every assertion
// below passes just as well against an engine that joints anything that asks —
// including the things that asked not to be jointed. The control is what makes
// "it attached" mean "it attached BECAUSE it was authored to".
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("ANTENNA_MOUNT", "PASS"|"FAIL")` — the telemetry verdict `scene_test`
// reads for its exit code.

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

fn on_start(me) {
    this.t = 0.0;
    // Joints resolve over several ticks — avian must admit both bodies into its
    // island graph, and the physics-transform bridge must have written a real
    // pose — so a verdict taken early measures load order, not the mount. The
    // tracker also needs a few ticks of Earth direction before the dish moves at
    // all, which is what `slewed` below is reading.
    this.settle_s = 12.0;
    this.phase = "settle";
    // Peak |angle| per mount, accumulated across the run. A running maximum, not
    // the final sample: a dish that slews and then settles has still slewed, and
    // reading only the last tick would call that motionless.
    this.peak = #{};
    for m in mounts() { this.peak[m.id] = 0.0; }
    this.peak["parked"] = 0.0;
    print("[mount] ── one antenna, three hosts, one parked control ─────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();

    for m in mounts() {
        let a = get(find(m.path + "/YawJoint"), "angle");
        if a != () && a.abs() > this.peak[m.id] { this.peak[m.id] = a.abs(); }
    }
    let pa = get(find("/AntennaMountTest/Parked/Antenna/YawJoint"), "angle");
    if pa != () && pa.abs() > this.peak["parked"] { this.peak["parked"] = pa.abs(); }

    if this.t < this.settle_s { return; }
    this.phase = "done";

    // Snapshot every fact the verdict reads, so a failure is diagnosable from the
    // log alone rather than needing the scene reloaded under a debugger.
    let seen = [];
    for m in mounts() {
        let j = find(m.path + "/YawJoint");
        seen.push(#{
            id: m.id,
            host: find(m.path),
            joint: j,
            angle: get(j, "angle"),
            peak: this.peak[m.id],
        });
    }
    let pj = find("/AntennaMountTest/Parked/Antenna/YawJoint");
    verdict(#{
        mounts: seen,
        parked_host: find("/AntennaMountTest/Parked/Antenna"),
        parked_joint: pj,
        parked_angle: get(pj, "angle"),
        parked_peak: this.peak["parked"],
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

        // 1 mrad. Earth tracking is a slow slew, so the bar is only meant to
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

    f.push(t_true(s.parked_angle == (),
        "the parked control has an `angle` port, so its joint attached despite " +
        "being deactivated. Every positive case above is therefore " +
        "consistent with an engine that joints anything that asks, and this " +
        "file proves nothing about authoring."));

    f.push(t_true(s.parked_peak == 0.0,
        "the parked control's dish moved (peak |angle| = " + r4(s.parked_peak) +
        " rad) — a joint that was explicitly switched off is being simulated."));

    print("[mount] ── results ─────────────────────────────────────────");
    for m in s.mounts {
        print("[mount]   " + m.id + "  angle=" + m.angle + "  peak=" + r4(m.peak));
    }
    print("[mount]   parked  angle=" + s.parked_angle + " (must be ())  peak=" +
          r4(s.parked_peak) + " (must be 0)");

    report_verdict(f, "ANTENNA MOUNT", "ANTENNA_MOUNT");
}
