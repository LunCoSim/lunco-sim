// Production asset acceptance for the bundled multi-class Modelica example.
//
// Keep this at the authored scenario boundary: the public contract is that a
// shipped model can be resolved and preflighted through the same query surface
// used by tools and the application. Parser and annotation implementation
// details are owned by their focused unit tests, not by this scene test.

fn on_start(me) {
    let report = query("ValidateAsset", #{
        path: "lunco://models/AnnotatedRocketStage.mo",
    });
    let failures = [];

    failures.push(t_true(report != (),
        "ValidateAsset did not return a report for AnnotatedRocketStage.mo"));
    if report != () {
        failures.push(t_true(report.kind == "modelica",
            "AnnotatedRocketStage.mo was classified as " + report.kind));
        failures.push(t_true(report.ok == true,
            "AnnotatedRocketStage.mo failed Modelica preflight"));
        failures.push(t_true(report.errors.len() == 0,
            "AnnotatedRocketStage.mo returned parse errors"));
        failures.push(t_true(report.info != () && report.info.model != (),
            "Modelica preflight did not report a model name"));
        if report.info != () && report.info.model != () {
            failures.push(t_true(report.info.model == "AnnotatedRocketStage.RocketStage",
                "preflight selected " + report.info.model +
                ", expected AnnotatedRocketStage.RocketStage"));
        }
    }

    report_verdict(failures, "ANNOTATED ROCKET STAGE", "ANNOTATED_ROCKET_STAGE");
}
