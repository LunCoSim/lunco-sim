// Test-only observation of the production physics and Modelica recorders.

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListTelemetryChannels", "QueryTelemetryHistory"], required_inputs: []}
}

fn history_is_flat(samples) {
    if samples.len() < 3 { return false; }
    for sample in samples {
        if sample.v != samples[0].v { return false; }
    }
    true
}

fn verify_recorded_history(channel, history, label, rate) {
    let ordered = true;
    let paced = true;
    for i in 1..history.samples.len() {
        let elapsed = history.samples[i].t - history.samples[i - 1].t;
        ordered = ordered && elapsed > 0.0;
        paced = paced && rate != () && rate > 0.0 && elapsed >= 1.0 / rate - 1.0e-9;
    }
    [t_true(channel.active, label + " has a live publisher"),
     t_true(history.samples.len() >= 3, label + " retains multiple simulation-time points"),
     t_true(history_is_flat(history.samples), label + " keeps steady values"),
     t_true(ordered, label + " timestamps advance strictly"),
     t_true(paced, label + " obeys the configured recording rate"),
     t_true(history.samples.len().to_float() <= channel.retention.to_float(), label + " respects retention")]
}

fn on_start(me, ctx) {
    if ctx.rate_hz == () || ctx.rate_hz <= 0.0 { throw "the configured recording rate is required"; }
    this.rate = ctx.rate_hz;
    this.done = false;
    this.attempts = 0;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.attempts += 1;
    if this.attempts % 20 != 0 { return; }
    // The sandbox's physics masses and vacuum-balloon density are steady.
    // Exact channel discovery keeps the observer's input bounded.
    let channels = query("ListTelemetryChannels", #{name: "mass"}).channels;
    channels.append(query("ListTelemetryChannels", #{name: "airDensity"}).channels);
    let selected = #{};
    for channel in channels {
        let label = "";
        if channel.name == "mass" { label = "physics"; }
        if channel.model_class != () && channel.model_class != "" { label = "Modelica"; }
        if label != "" && !selected.contains(label) && channel.samples.to_float() >= 3.0 {
            let history = query("QueryTelemetryHistory", #{key: channel.key, limit: 10});
            if history_is_flat(history.samples) { selected[label] = #{channel: channel, history: history}; }
        }
        if selected.contains("physics") && selected.contains("Modelica") { break; }
    }
    if (!selected.contains("physics") || !selected.contains("Modelica")) && this.attempts < 1800 { return; }
    this.done = true;
    let checks = [t_true(selected.contains("physics"), "a steady physics channel is admitted"),
                  t_true(selected.contains("Modelica"), "a steady Modelica channel is admitted")];
    for label in ["physics", "Modelica"] {
        if selected.contains(label) {
            let item = selected[label];
            checks.append(verify_recorded_history(item.channel, item.history, label, this.rate));
            if label == "physics" {
                checks.push(t_true(item.channel.unit == "kg" && item.channel.provenance == "avian",
                    "physics mass keeps its owner metadata"));
                checks.push(t_true(item.channel.group_path != () && item.channel.group_path != "" &&
                                   item.channel.description != () && item.channel.description != "",
                    "physics mass keeps its USD group and description"));
            }
        }
    }
    // An absent identity must not manufacture retained samples.
    let missing = query("ListTelemetryChannels", #{name: "__missing_recording_history__"});
    checks.push(t_true(missing.count.to_float() == 0.0 && missing.channels.len() == 0,
        "an unknown channel has no manufactured recording"));
    report_verdict(checks, "TELEMETRY RECORDING HISTORY", "TELEMETRY_RECORDING_HISTORY");
}
