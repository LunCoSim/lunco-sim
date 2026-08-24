// ── The linter, linted ──────────────────────────────────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/lint_selftest.usda`, which is authored
// wrong on purpose.
//
// WHAT IT GUARDS. The whole lint chain, end to end and in one place:
//
//   facts (Rust: lunco-usd-avian::physics_facts)
//     → hook `lint.usd`
//       → rules (rhai: assets/scripting/policy/lint_usd.rhai)
//         → LintReport
//           → the `LintReport` query
//
// Any link breaking gives the same symptom — no findings — and "clean scene" and
// "linter not running" are indistinguishable from the outside. That is why the
// scene contains known faults: this test asserts they are FOUND, by rule id.
//
// It also asserts the CORRECT construction stays silent. A nested body that is
// jointed is how every rover mounts a wheel; a rule that fires on it would bury
// every real finding in false positives, so `/LintSelftest/Vehicle/Wheel` must
// appear in no finding at all.
//
// NOTHING LINTS ON LOAD — the linter is a verb. This scenario calls it exactly as
// a user would:
//
//     cmd("RunLint", #{})            // or the tagged ExecuteCommand envelope over HTTP/MCP
//     query("LintReport")
//
// An editor or preflight command repeats the same pair after an authored change;
// this test deliberately invokes it once and has no per-tick lint monitor.
//
// HOW FAILURE IS SIGNALLED: `emit("LINT_SELFTEST", "PASS"|"FAIL")`.

/// Findings whose `rule` matches, as an array.
fn by_rule(findings, rule) {
    let out = [];
    for f in findings {
        if f.rule == rule { out.push(f); }
    }
    out
}

/// Whether any finding names `subject`.
fn mentions(findings, subject) {
    for f in findings {
        if f.subject == subject { return true; }
    }
    false
}

fn on_start(me) {
    // The scene also contains deliberately dynamic, collider-less bodies so
    // `dynamic-body-no-collider` is exercised by the authored facts. Those
    // bodies must raise the real containment boundary when the simulation runs;
    // accept that terminal fault through the test harness rather than hiding it
    // or attempting recovery from the malformed fixture.
    expect_runtime_fault("physics-body-escaped");
    this.t = 0.0;
    // The production scene-test runner has already completed its explicit stage
    // and participant-readiness gates before lifecycle execution is enabled. Run
    // on the first fixed tick so the deliberate dynamic-body fixture can still
    // reach its declared terminal fault after the lint report is asserted.
    this.lint_at = 0.0;
    this.phase = "wait";

    print("[lint-selftest] ── the linter, linted ─────────────────────────");

    // The malformed network guides are lint inputs, not runtime participants, so
    // they must produce findings without producing live co-simulation faults.
    // The assertions below pin those findings by rule and subject; if the
    // malformation is tidied away, this scene still fails visibly.
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();
    if this.t < this.lint_at { return; }

    this.phase = "done";

    // 1. RUN it. Nothing has linted anything up to this point.
    cmd("RunLint", #{});

    // 2. READ it back.
    let report = query("LintReport");
    if report == () {
        fail_fast("query(\"LintReport\") returned () — the query provider is not " +
                  "registered, so nothing about the linter is being tested",
                  "LINT SELFTEST", "LINT_SELFTEST");
        return;
    }

    let findings = report.findings;
    print("[lint-selftest] " + report.errors + " error(s), " + report.warnings +
          " warning(s), " + findings.len() + " finding(s)");
    for f in findings {
        print("[lint-selftest]   " + f.severity + "  " + f.rule + "  " + f.subject);
    }

    let f = [];

    // The motor bug, by rule id and by subject.
    let nested = by_rule(findings, "nested-body-no-joint");
    f.push(t_true(nested.len() == 1,
        "nested-body-no-joint fired " + nested.len() + " time(s), expected exactly 1 " +
        "(/LintSelftest/Vehicle/LooseMotor)"));
    if nested.len() == 1 {
        f.push(t_true(nested[0].subject == "/LintSelftest/Vehicle/LooseMotor",
            "nested-body-no-joint named " + nested[0].subject + ", expected the loose motor"));
        f.push(t_true(nested[0].severity == "error",
            "nested-body-no-joint is severity " + nested[0].severity + ", expected error — " +
            "a part that falls off the vehicle is not a warning"));
    }

    // A joint pointing at a prim that resolves to NO body — /LintSelftest/NotABody
    // hangs under nothing that is a body, so the joint really is dropped.
    let bad = by_rule(findings, "joint-target-not-a-body");
    f.push(t_true(bad.len() == 1,
        "joint-target-not-a-body fired " + bad.len() + " time(s), expected exactly 1 " +
        "(/LintSelftest/BadJoint, whose target hangs under no body at all)"));

    // THE FALSE-POSITIVE GUARD, and the reason the rule reads "resolves to no
    // body" rather than "is not a body". A component names its OWN root Xform for
    // an endpoint because it cannot know the host it will be referenced onto;
    // UsdPhysics resolves that to the nearest ancestor body. MEASURED: the
    // stricter reading reported 73 shipped antennas, dishes and booms as silently
    // rigid.
    f.push(t_true(!mentions(findings, "/LintSelftest/MountHost/Mount/YawJoint"),
        "a finding names /LintSelftest/MountHost/Mount/YawJoint — it names a plain " +
        "Xform under a body, which resolves to that body. That is how every mounted " +
        "mechanism we ship attaches, and a rule that flags it flags all of them"));

    // A body that cannot touch anything. TWO are expected, and the second one is
    // the interesting one: the loose motor has no collider either, so it trips
    // this rule as well as `nested-body-no-joint`. Rules are independent
    // statements about the same stage, not a classification — the same prim
    // being wrong in two ways is two findings, and asserting "exactly one" here
    // would be asserting that rules suppress each other.
    let ghost = by_rule(findings, "dynamic-body-no-collider");
    f.push(t_true(ghost.len() == 2,
        "dynamic-body-no-collider fired " + ghost.len() + " time(s), expected 2 " +
        "(/LintSelftest/Ghost and the collider-less loose motor)"));
    f.push(t_true(mentions(ghost, "/LintSelftest/Ghost"),
        "dynamic-body-no-collider never named /LintSelftest/Ghost — the one prim in " +
        "this scene that exists only to trip it"));

    // A force-based drive with stiffness but no driven mass is not the
    // production implicit motor path. The finding must name the drive itself,
    // include the authoritative eight-substep contract, and remain separate
    // from the geometric sprung-foot finding.
    let unstable_drive = by_rule(findings, "conditionally-stable-joint-drive");
    f.push(t_true(unstable_drive.len() == 1,
        "conditionally-stable-joint-drive fired " + unstable_drive.len() +
        " time(s), expected exactly 1 (/LintSelftest/Leg/Strut_Spring)"));
    if unstable_drive.len() == 1 {
        f.push(t_true(unstable_drive[0].subject == "/LintSelftest/Leg/Strut_Spring",
            "conditionally-stable-joint-drive named " + unstable_drive[0].subject +
            ", expected the un-massed force drive"));
        f.push(t_true(unstable_drive[0].severity == "error",
            "conditionally-stable-joint-drive is severity " + unstable_drive[0].severity +
            ", expected error"));
        f.push(t_true(unstable_drive[0].message.contains("8 substeps"),
            "conditionally-stable-joint-drive did not report the authoritative 8-substep contract"));
    }

    let negative_stiffness = by_rule(findings, "joint-drive-negative-stiffness");
    f.push(t_true(negative_stiffness.len() == 1,
        "joint-drive-negative-stiffness fired " + negative_stiffness.len() +
        " time(s), expected exactly 1 (/LintSelftest/Leg/Strut_NegativeStiffness)"));
    if negative_stiffness.len() == 1 {
        f.push(t_true(negative_stiffness[0].subject == "/LintSelftest/Leg/Strut_NegativeStiffness",
            "joint-drive-negative-stiffness named " + negative_stiffness[0].subject +
            ", expected the negative-stiffness drive"));
        f.push(t_true(negative_stiffness[0].severity == "error",
            "joint-drive-negative-stiffness is severity " + negative_stiffness[0].severity +
            ", expected error"));
        f.push(t_true(negative_stiffness[0].message.contains("cannot be converted"),
            "joint-drive-negative-stiffness did not explain why adding mass cannot fix it"));
    }

    let invalid_gear = by_rule(findings, "invalid-gear-drive");
    f.push(t_true(invalid_gear.len() == 1,
        "invalid-gear-drive fired " + invalid_gear.len() +
        " time(s), expected exactly 1 (/LintSelftest/BadGear)"));
    if invalid_gear.len() == 1 {
        f.push(t_true(invalid_gear[0].subject == "/LintSelftest/BadGear",
            "invalid-gear-drive named " + invalid_gear[0].subject +
            ", expected the malformed gear drive"));
        f.push(t_true(invalid_gear[0].severity == "error",
            "invalid-gear-drive is severity " + invalid_gear[0].severity +
            ", expected error"));
    }

    // THE FALSE-POSITIVE GUARD. A jointed nested body is the normal wheel mount.
    f.push(t_true(!mentions(findings, "/LintSelftest/Vehicle/Wheel"),
        "a finding names /LintSelftest/Vehicle/Wheel — that is a correctly jointed " +
        "nested body, and a rule that fires on it fires on every rover we ship"));

    // The vehicle body itself is well-formed: collider on a child, mass authored.
    f.push(t_true(!mentions(findings, "/LintSelftest/Vehicle"),
        "a finding names /LintSelftest/Vehicle — its collider is on its Chassis child, " +
        "which is the compound-body shape every vehicle uses"));

    // An authored never-collide pair whose target does not exist. The rule is
    // ERROR, not warn: the author wrote down a protection they do not have, and
    // the runtime does not say so until 600 ticks in.
    let inert = by_rule(findings, "filtered-pair-target-missing");
    f.push(t_true(inert.len() == 1,
        "filtered-pair-target-missing fired " + inert.len() + " time(s), expected " +
        "exactly 1 (/LintSelftest/Bumper, which filters against a prim that is not " +
        "on this stage)"));
    if inert.len() == 1 {
        f.push(t_true(inert[0].subject == "/LintSelftest/Bumper",
            "filtered-pair-target-missing named " + inert[0].subject +
            ", expected the bumper"));
    }

    // A collision group whose collection matches no prim, and whose
    // `filteredGroups` names a body instead of a group. Both are ERRORs: the
    // author wrote down protection they do not have, and nothing at runtime says
    // so — the parts just keep colliding.
    let ghost_group = by_rule(findings, "collision-group-include-missing");
    f.push(t_true(ghost_group.len() == 1,
        "collision-group-include-missing fired " + ghost_group.len() + " time(s), " +
        "expected exactly 1 (/LintSelftest/GhostGroup groups a prim that is not " +
        "on this stage)"));

    let bad_filter = by_rule(findings, "collision-group-filter-not-a-group");
    f.push(t_true(bad_filter.len() == 1,
        "collision-group-filter-not-a-group fired " + bad_filter.len() + " time(s), " +
        "expected exactly 1 (/LintSelftest/GhostGroup filters against a body)"));

    // ── THE PHANTOM WIRE, AND THE DECLARATION THAT IS NOT ONE ───────────────
    // `connector-requires-network-scope` takes a `connectors:*.connect` that no
    // compiler network owns. Both halves are asserted, because the rule was
    // MEASURED reading the other way: keyed on the DECLARATION it reported 314
    // errors across the shipped tree — every motor of every rover — since
    // `components/mobility/motor.usda` declares its pin unconditionally and only
    // the `power = "battery"` variant wires it. None of those was fixable.
    let stranded = by_rule(findings, "connector-requires-network-scope");
    f.push(t_true(mentions(stranded, "/LintSelftest/StrandedMotor"),
        "connector-requires-network-scope never named /LintSelftest/StrandedMotor, " +
        "which connects its pin while sitting in no network scope — the wire reads " +
        "as authored in USD and no solver ever receives it"));
    f.push(t_true(!mentions(stranded, "/LintSelftest/StrandedBattery"),
        "connector-requires-network-scope named /LintSelftest/StrandedBattery, which " +
        "only DECLARES a pin. A bare connector is an interface — the USD spelling of " +
        "Modelica's `Pin p` — and flagging it flags every catalogue part we ship"));
    f.push(t_true(!mentions(stranded, "/LintSelftest/OutsideBattery"),
        "connector-requires-network-scope named /LintSelftest/OutsideBattery, another " +
        "bare declaration. It IS wrong — something inside a scope wires to it — but " +
        "that is dangling-network-connector's finding, said once, from the scope side"));

    // ── THE ELECTRICAL ISLAND REFERENCE RULES ───────────────────────────────
    // A LunCo electrical device carries one `Pin` and there is no `Ground`, so the
    // node potential is set by whichever member states `p.v`. Zero of them leaves
    // `v` free and the DAE structurally singular; two over-determine the node.
    // Both compile. Neither is reported by the solver as what it is — the first
    // surfaces as an unrelated member's current failing to solve, the second as an
    // island that publishes nothing — which is why they are lint rules at all, and
    // why they are asserted BY SUBJECT here.

    // THE FALSE-POSITIVE GUARD, and the one that matters most: one battery plus one
    // motor, connected, both collected — the shape of every shipped rover's
    // `Electrical` scope. A rule that cannot stay silent here fires on the fleet.
    f.push(t_true(!mentions(findings, "/LintSelftest/HealthyElectrical"),
        "a finding names /LintSelftest/HealthyElectrical — one voltage source and one " +
        "current-stating load is a CORRECT island, and a rule that flags it flags " +
        "every rover we ship"));
    f.push(t_true(!mentions(findings, "/LintSelftest/HealthyBattery"),
        "a finding names /LintSelftest/HealthyBattery, a correctly collected member"));
    f.push(t_true(!mentions(findings, "/LintSelftest/HealthyMotor"),
        "a finding names /LintSelftest/HealthyMotor, a correctly collected member"));

    // Do not emit the ordinary scene verdict here. The same fixture deliberately
    // contains the two dynamic collider-less bodies above, so the production
    // runner must continue until its declared terminal physics fault is raised.
    // A normal verdict would stop the runner before that boundary was observed.
    // Keep the assertion result explicit in the log; the runner owns the final
    // PASS/FAIL result for this negative scene through ExpectedRuntimeFaults.
    let real = [];
    for check in f {
        if check != "" { real.push(check); }
    }
    if !real.is_empty() {
        for check in real { print("  FAIL: " + check); }
        print("TESTS_FAIL " + real.len() + "/" + f.len());
        print("LINT SELFTEST: FAIL");
        emit("LINT_SELFTEST", "FAIL");
        notify_kind("LINT SELFTEST: FAIL", "error");
        return;
    }
    print("TESTS_OK " + f.len());
    print("LINT SELFTEST: PASS (lint assertions; awaiting expected terminal fault)");
}
