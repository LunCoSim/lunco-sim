// Celestial scenes must not fall back to a static render light for Modelica.
// The authored fixture intentionally has a DistantLight, a celestial body, a
// connected solar program input, and no root geodetic site anchor.

fn findings_for_rule(findings, rule) {
    let out = [];
    for finding in findings {
        if finding.rule == rule { out.push(finding); }
    }
    out
}

fn has_diagnostic(findings, code) {
    if findings == () { return false; }
    for finding in findings {
        if finding.code == code { return true; }
    }
    false
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.phase = "start";
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    if this.phase == "start" {
        if this.elapsed < 0.1 { return; }
        cmd("RunLint", #{});
        this.phase = "wait";
        this.elapsed = 0.0;
        return;
    }

    if this.phase == "wait" {
        let report = query("LintReport");
        if report == () {
            this.done = true;
            report_verdict(["RunLint produced no LintReport"],
                "CELESTIAL SOLAR SOURCE", "CELESTIAL_SOLAR_SOURCE");
            return;
        }
        if report.pending == true {
            if this.elapsed > 10.0 {
                this.done = true;
                report_verdict(["RunLint did not finish its composed-stage pass"],
                    "CELESTIAL SOLAR SOURCE", "CELESTIAL_SOLAR_SOURCE");
            }
            return;
        }

        let site = findings_for_rule(report.findings, "celestial-solar-site-required");
        let static_source = findings_for_rule(report.findings, "static-solar-light-cardinality");
        let diagnostics = query("RuntimeDiagnostics");
        let findings = if diagnostics == () { () } else { diagnostics.findings };
        this.done = true;
        report_verdict([
            t_true(site.len() == 1,
                "expected one celestial-solar-site-required finding, got " + site.len()),
            t_true(site.len() == 1 && site[0].subject == "/World"
                && site[0].severity == "error"
                && site[0].message.contains("lunco:anchor:lat")
                && site[0].message.contains("lunco:anchor:lon"),
                "the missing site finding must be an actionable root error naming both anchor fields"),
            t_true(static_source.len() == 0,
                "the authored DistantLight must not be selected as a static source in celestial mode"),
            t_true(has_diagnostic(findings, "solar-source-missing"),
                "missing celestial SunState must remain visible as a solar-source-missing diagnostic"),
        ], "CELESTIAL SOLAR SOURCE", "CELESTIAL_SOLAR_SOURCE");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report_verdict(["celestial solar source check stopped before the report settled"],
            "CELESTIAL SOLAR SOURCE", "CELESTIAL_SOLAR_SOURCE");
    }
}
