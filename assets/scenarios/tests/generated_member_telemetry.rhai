// Production telemetry ownership checks, invoked against a generated network
// and by the targetless metadata-Scope negative scene.

// The API driver supplies the generated owner's typed alias facts. Its source
// query also carries the full generated document, which exceeds the REPL's
// string limit; live channel ownership and sampling are read here.
fn verify_generated_member_telemetry(network_path, member_path, member_output, channel_name, aliases) {
    let network = find_path(network_path);
    let member = find_path(member_path);
    let alias_count = 0;
    for alias in aliases {
        if alias[0] == member_path && alias[1] == member_output { alias_count += 1; }
    }
    let channels = query("ListTelemetryChannels", #{ name: channel_name });
    let matches = [];
    if channels != () && channels.channels != () {
        for channel in channels.channels {
            if channel.name == channel_name { matches.push(channel); }
        }
    }
    let checks = [
        t_true(network >= 0 && member >= 0 && network != member,
            "generated network and authored member have distinct runtime identities"),
        t_true(alias_count == 1, "the generated owner publishes exactly one member-output alias"),
        t_true(matches.len() == 1, "the authored telemetry declaration creates one retained channel"),
    ];
    if matches.len() == 1 {
        let channel = matches[0];
        checks.push(t_true(channel.source.to_string() == network.to_string()
                && channel.source.to_string() != member.to_string(),
            "the generated wrapper owns the member's retained telemetry"));
        checks.push(t_true(channel.active && channel.samples.to_float() > 0.0 && channel.unit == "W",
            "the authored channel retains live samples and its unit"));
    }
    let missing = query("ListTelemetryChannels", #{ name: "__unregistered_telemetry_probe__" });
    checks.push(t_true(missing.count.to_float() == 0.0 && missing.channels.len() == 0,
        "an unknown exact channel name returns an empty catalog subset"));
    report_verdict(checks, "GENERATED MEMBER TELEMETRY", "GENERATED_MEMBER_TELEMETRY");
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["RuntimeDiagnostics", "ListTelemetryChannels"], required_inputs: [] }
}

fn on_start(me, ctx) { this.done = false; this.ticks = 0; }

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks < 3 { return; }
    this.done = true;
    let diagnostics = query("RuntimeDiagnostics");
    let target_errors = [];
    if diagnostics != () && diagnostics.findings != () {
        for finding in diagnostics.findings {
            if finding.producer == "usd-telemetry" && finding.code == "telemetry-target" {
                target_errors.push(finding);
            }
        }
    }
    let channel_count = 0;
    let channels = query("ListTelemetryChannels", #{ name: "unbound_probe" });
    if channels != () && channels.channels != () {
        for channel in channels.channels {
            if channel.name == "unbound_probe" { channel_count += 1; }
        }
    }
    let checks = [
        t_true(find_path("/World/UnboundTelemetry") >= 0,
            "the targetless metadata Scope is present in the live scene"),
        t_true(target_errors.len() == 1,
            "a targetless metadata Scope produces one telemetry-target diagnostic"),
        t_true(channel_count == 0, "invalid declaration creates no retained telemetry channel"),
    ];
    let invalid_name = query("ListTelemetryChannels", #{ name: true });
    checks.push(t_true(invalid_name != () && invalid_name.ok == false
            && invalid_name.code == 422 && invalid_name.status == "invalid"
            && invalid_name.error.contains("ListTelemetryChannels: name must be a string"),
        "an invalid catalog filter returns a terminal query diagnostic"));
    if target_errors.len() == 1 {
        checks.push(t_true(target_errors[0].subject == "/World/UnboundTelemetry"
                && target_errors[0].severity == "error",
            "the diagnostic names the invalid declaration and remains an error"));
    }
    report_verdict(checks, "TELEMETRY TARGET NEGATIVE", "TELEMETRY_TARGET_NEGATIVE");
}
