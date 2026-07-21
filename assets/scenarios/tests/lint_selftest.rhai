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

    report_verdict(f, "LINT SELFTEST", "LINT_SELFTEST");
}
