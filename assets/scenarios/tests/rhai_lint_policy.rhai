// The authored Rhai lint policy owns this contract. The Rust validator supplies
// generic file facts; this observer exercises the public ValidateAsset surface.

fn has_error(report, needle) {
    if report == () || report.errors == () { return false; }
    for error in report.errors {
        if error.contains(needle) { return true; }
    }
    false
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.1 { return; }

    let report = query("ValidateAsset", #{
        path: "lunco://scenarios/tests/fixtures/invalid_build_profile_gate.rhai"
    });
    this.done = true;
    report_verdict([
        t_true(report != (), "ValidateAsset returned no Rhai report"),
        t_true(report != () && report.ok == false,
               "the build-profile fixture was accepted by Rhai lint"),
        t_true(has_error(report, "rhai-build-profile-gate"),
               "Rhai lint did not report rule `rhai-build-profile-gate`"),
    ], "RHAI LINT POLICY", "RHAI_LINT_POLICY");
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report_verdict(["Rhai lint policy stopped before validation"],
            "RHAI LINT POLICY", "RHAI_LINT_POLICY");
    }
}
