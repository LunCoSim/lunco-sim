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
//     cmd("RunLint", #{})            // or: {"command":"RunLint"} over HTTP/MCP
//     query("LintReport")
//
// and a scenario wanting a LIVE linter just calls the same pair on a cadence.
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
    this.t = 0.0;
    // One second of settling before linting: the stage must be composed and the
    // scene loaded, and a verb that runs on tick one would be testing the loader's
    // timing rather than the rules.
    this.lint_at = 1.0;
    this.phase = "wait";

    print("[lint-selftest] ── the linter, linted ─────────────────────────");

    // `/LintSelftest/MultiSourceNetwork` wires `inputs:left`/`inputs:right` into a
    // network whose only member declares neither — a REAL dangling wire, authored
    // on purpose so the lint rule has something to find. Declare it, so
    // `scene_test`'s never-landed gate reads the fixture as a fixture. Declaring
    // it also asserts it: if the malformation is ever tidied away, this scene
    // fails rather than silently testing nothing.
    expect_fault("left");
    expect_fault("right");
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

    // A joint pointing at a non-body.
    let bad = by_rule(findings, "joint-target-not-a-body");
    f.push(t_true(bad.len() == 1,
        "joint-target-not-a-body fired " + bad.len() + " time(s), expected exactly 1"));

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

    // ── THE ELECTRICAL ISLAND REFERENCE RULES ───────────────────────────────
    // A LunCo electrical device carries one `Pin` and there is no `Ground`, so the
    // node potential is set by whichever member states `p.v`. Zero of them leaves
    // `v` free and the DAE structurally singular; two over-determine the node.
    // Both compile. Neither is reported by the solver as what it is — the first
    // surfaces as an unrelated member's current failing to solve, the second as an
    // island that publishes nothing — which is why they are lint rules at all, and
    // why they are asserted BY SUBJECT here.

    let no_ref = by_rule(findings, "electrical-island-has-no-reference");
    f.push(t_true(mentions(no_ref, "/LintSelftest/PanelOnlyNetwork"),
        "electrical-island-has-no-reference never named /LintSelftest/PanelOnlyNetwork " +
        "— a solar panel alone on a bus is a current source with nothing setting the " +
        "node voltage, which is the exact island this rule exists to catch"));
    // Asserted by subject rather than by count because `/LintSelftest/DanglingElectrical`
    // — a lone DCMotor, authored for the dangling-connector rule — is reference-less
    // for the same real reason and trips this too. That is a true positive, not
    // double-reporting: the same stage being wrong in two ways is two findings.
    f.push(t_true(mentions(no_ref, "/LintSelftest/DanglingElectrical"),
        "electrical-island-has-no-reference skipped /LintSelftest/DanglingElectrical, " +
        "which is a lone motor on a bus — if the rule only fires on panels it is " +
        "matching the device rather than the missing reference"));
    if no_ref.len() > 0 {
        f.push(t_true(no_ref[0].severity == "error",
            "electrical-island-has-no-reference is severity " + no_ref[0].severity +
            ", expected error — a structurally singular island never publishes a port"));
    }

    // A built-in `info:id` program that authors `connectors:p`
    // (`/LintSelftest/InvalidProgramNetwork`) contributes no equations at all and is
    // already reported by `invalid-network-program-source`. Calling it a
    // reference-less electrical island would be a second, wronger diagnosis of one
    // prim, so the rule requires a `.mo` source — and this pins that.
    f.push(t_true(!mentions(no_ref, "/LintSelftest/InvalidProgramNetwork"),
        "electrical-island-has-no-reference named /LintSelftest/InvalidProgramNetwork, " +
        "whose only member is a built-in kernel with no Modelica source — it is not " +
        "an electrical island and already has its own finding"));

    let over = by_rule(findings, "electrical-island-over-determined");
    f.push(t_true(over.len() == 1,
        "electrical-island-over-determined fired " + over.len() + " time(s), expected " +
        "exactly 1 (/LintSelftest/TwoBatteryNetwork, two devices each stating p.v)"));
    if over.len() == 1 {
        f.push(t_true(over[0].subject == "/LintSelftest/TwoBatteryNetwork",
            "electrical-island-over-determined named " + over[0].subject +
            ", expected the two-battery network"));
    }

    // ONE NODE IS THE PREMISE, and `/LintSelftest/DisconnectedElectrical` is the
    // fixture that proves the rule knows it: two batteries in one scope, but on two
    // DISCONNECTED islands, so each pins its own node and neither is
    // over-determined. Counting per scope instead of per node flags it — MEASURED,
    // before `island_count` gated the rule. The split itself is already an error
    // from `disconnected-component-network`; saying it twice, wrongly, is worse
    // than saying it once.
    f.push(t_true(!mentions(over, "/LintSelftest/DisconnectedElectrical"),
        "electrical-island-over-determined named /LintSelftest/DisconnectedElectrical, " +
        "whose two batteries sit on two disconnected nodes — one source each, which " +
        "is correct. The rule is counting per scope where it must count per node"));

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

    report_verdict(f, "LINT SELFTEST", "LINT_SELFTEST");
}
