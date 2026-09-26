// Validate the generic direction contract through the production USD lint
// policy. Invalid test inputs stay in a read-only preflight fixture, not the
// mounted simulation stage.

fn errors_for_rule(errors, rule) {
    let out = [];
    let marker = "[usd/" + rule + "]";
    for error in errors {
        if error.contains(marker) { out.push(error); }
    }
    out
}

fn has_subject(errors, rule, subject) {
    for error in errors_for_rule(errors, rule) {
        if error.contains(subject) { return true; }
    }
    false
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ValidateAsset"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.1 { return; }

    let fixture = query("ValidateAsset", #{
        path: "lunco://scenes/fixtures/direction_contract_errors.usda"
    });
    let component = query("ValidateAsset", #{
        path: "lunco://components/power/solar_panel.usda"
    });
    this.done = true;
    if fixture == () || component == () {
        report_verdict(["ValidateAsset returned no direction contract report"],
            "DIRECTION TARGET CONTRACT", "DIRECTION_TARGET_CONTRACT");
        return;
    }

    let errors = fixture.errors;
    let duplicate = errors_for_rule(errors, "direction-target-id-duplicate");
    let invalid_id = errors_for_rule(errors, "direction-target-id-invalid");
    let triplets = errors_for_rule(errors, "direction-input-triplet-invalid");
    let precision = errors_for_rule(errors, "direction-input-precision");
    let source_specific = errors_for_rule(errors, "direction-input-name-non-generic");
    report_verdict([
        t_true(fixture.kind == "usd" && fixture.ok == false,
            "the malformed direction fixture must be rejected by USD lint"),
        t_true(!has_subject(errors, "direction-source-target-missing", "/World/ValidConsumer"),
            "an arbitrary unique target id must resolve without body-specific policy"),
        t_true(has_subject(errors, "direction-source-target-missing", "/World/MissingConsumer"),
            "a demanded id without a target must be reported"),
        t_true(has_subject(errors, "direction-source-target-ambiguous", "/World/DuplicateConsumer"),
            "a demanded id with two targets must be reported as ambiguous"),
        t_true(duplicate.len() == 1,
            "duplicate target identity must produce one actionable finding, got " + duplicate.len()),
        t_true(invalid_id.len() == 1,
            "invalid target identity must be rejected, got " + invalid_id.len()),
        t_true(triplets.len() >= 2,
            "incomplete and mismatched input triplets must be rejected, got " + triplets.len()),
        t_true(precision.len() == 1,
            "a float direction component must be rejected, got " + precision.len()),
        t_true(source_specific.len() == 1 &&
               has_subject(errors, "direction-input-name-non-generic", "/World/SolarConsumer"),
            "source identity must stay on the provider wire, got " + source_specific.len() + " source-specific consumer input finding(s)"),
        t_true(!has_subject(errors, "direction-input-name-non-generic", "/World/ValidConsumer"),
            "the generic target input triplet must remain valid"),
        t_true(component.ok == true &&
               errors_for_rule(component.errors, "direction-input-triplet-invalid").len() == 0,
            "a spawnable component's complete unconnected target interface must defer to its parent"),
    ], "DIRECTION TARGET CONTRACT", "DIRECTION_TARGET_CONTRACT");
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report_verdict(["direction target validation stopped before the report settled"],
            "DIRECTION TARGET CONTRACT", "DIRECTION_TARGET_CONTRACT");
    }
}
