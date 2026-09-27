// Production API acceptance for fixed-tick admission of a held semantic input.

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path("/PhysicalRoverControls/SkidPhysical")],
       entity_reads: [], entity_writes: [],
       query_reads: ["ReadSessionInputStream"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.target = find_path("/PhysicalRoverControls/SkidPhysical");
    this.expected_producer_id = 8181;
    this.elapsed = 0.0;
    this.done = false;
    if this.target < 0 {
        this.done = true;
        report_verdict(["fixed-tick input target did not compose"],
            "SIMULATE INTENT INPUT ADMISSION", "SIMULATE_INTENT_INPUT_ADMISSION");
        return;
    }

    let started = cmd("StartSessionInputCapture", #{});
    if !started.ok {
        this.done = true;
        report_verdict(["session input capture did not start: " + started.error],
            "SIMULATE INTENT INPUT ADMISSION", "SIMULATE_INTENT_INPUT_ADMISSION");
    }
}

fn on_event(me, evt, ctx) {
    if this.done || evt.name != "intent.hold" || evt.source != this.target {
        return;
    }
    let value = evt.value;
    if value["intent"] != "action" || value["held"] != true {
        return;
    }

    let checks = [];
    checks.push(t_true(type_of(value["correlation_id"]) == "u64" &&
                       value["correlation_id"].to_string() != "0",
        "intent.hold did not retain the command correlation"));
    checks.push(t_true(type_of(value["target_gid"]) == "u64" &&
                       value["target_gid"].to_string() == this.target.to_string(),
        "intent.hold did not retain its stable target identity"));
    checks.push(t_true(type_of(value["scene_generation"]) == "u64" &&
                       type_of(value["effective_tick"]) == "u64" &&
                       type_of(value["input_sequence"]) == "u64",
        "intent.hold did not expose its fixed-tick admission stamp"));
    checks.push(t_true(value["producer_kind"] == "api_transport" &&
                       type_of(value["producer_id"]) == "u64" &&
                       value["producer_id"].to_string() == this.expected_producer_id.to_string(),
        "intent.hold did not retain the stable API producer identity"));
    checks.push(t_true(evt.sim_tick.to_string() == value["effective_tick"].to_string(),
        "intent.hold telemetry did not land at its admitted fixed tick"));

    let recording = query("ReadSessionInputStream", #{});
    checks.push(t_true(recording["state"] == "recording",
        "session input capture was not active during admitted input: " + recording.to_string()));
    let matching_record = false;
    if type_of(recording["records"]) == "array" {
        for record in recording["records"] {
            let payload = record["payload"];
            let producer = record["producer"];
            if payload["kind"] == "simulated_intent_change"
                && producer["kind"] == "api_transport"
                && producer["value"]["producer_id"].to_string() == this.expected_producer_id.to_string()
                && record["target"].to_string() == value["target_gid"].to_string()
                && record["scene_generation"].to_string() == value["scene_generation"].to_string()
                && record["effective_tick"].to_string() == value["effective_tick"].to_string()
                && record["sequence"].to_string() == value["input_sequence"].to_string()
                && payload["value"]["intent"] == value["intent"]
                && payload["value"]["held"] == value["held"]
                && payload["value"]["correlation_id"].to_string() == value["correlation_id"].to_string() {
                matching_record = true;
            }
        }
    }
    checks.push(t_true(matching_record,
        "session input capture did not retain the admitted API intent payload and producer"));
    let scenario_release = cmd("SimulateIntent", #{
        intent: "action",
        held: false,
        target: this.target,
    });
    checks.push(t_true(scenario_release.ok,
        "Twin Rhai could not submit its actor-scoped held input without producer_id"));
    let supplied_actor_identity = cmd("SimulateIntent", #{
        intent: "action",
        held: false,
        target: this.target,
        producer_id: this.expected_producer_id,
    });
    checks.push(t_true(!supplied_actor_identity.ok &&
                       supplied_actor_identity.status == "rejected" &&
                       supplied_actor_identity.error.contains("Twin Rhai semantic input"),
        "Twin Rhai did not reject a caller ID in favor of its stable actor identity"));
    let stopped = cmd("StopSessionInputCapture", #{});
    checks.push(t_true(stopped.ok && stopped.data["state"] == "complete",
        "session input capture did not stop with a complete record stream"));
    let complete = query("ReadSessionInputStream", #{});
    checks.push(t_true(complete["state"] == "complete" &&
                       type_of(complete["records"]) == "array",
        "completed session input stream was not readable through the typed API: " + complete.to_string()));
    this.done = true;
    report_verdict(checks, "SIMULATE INTENT INPUT ADMISSION",
        "SIMULATE_INTENT_INPUT_ADMISSION");
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed >= 30.0 {
        this.done = true;
        report_verdict(["no admitted API held-input event arrived within thirty simulation seconds"],
            "SIMULATE INTENT INPUT ADMISSION", "SIMULATE_INTENT_INPUT_ADMISSION");
    }
}
