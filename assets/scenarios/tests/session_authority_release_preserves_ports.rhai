// Production API acceptance for authority release clearing controller holds
// without writing replacement endpoint values.
// @timing simulation

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [find_path("/TerrainOnly/Avatar")], entity_writes: [],
       query_reads: ["ReadSessionInputStream", "ReadPorts"], required_inputs: [] }
}

fn on_start(me, params) {
    this.target_gid = params["target_gid"];
    this.producer_id = params["producer_id"];
    this.correlation_id = params["correlation_id"];
    this.admission = params["admission"];
    this.expected_writes = [
        ["forward", 0.7], ["side", -0.25], ["up", 0.4], ["speed_boost", 1.0],
    ];
    this.ticks = 0;
    this.done = false;
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks >= 1200 {
        this.done = true;
        report_verdict(["the admitted endpoint write did not reach the authority-release verifier"],
            "SESSION AUTHORITY RELEASE PRESERVES PORTS", "SESSION_INPUT_ADMISSION");
        return;
    }
    let stream = query("ReadSessionInputStream", #{});
    if stream == () || stream["records"] == () { return; }
    let port_write = ();
    let lifecycle_release = ();
    for record in stream["records"] {
        let payload = record["payload"];
        if payload["kind"] == "port_input_writes"
            && record["producer"]["kind"] == "api_transport"
            && record["producer"]["value"]["producer_id"].to_string()
                == this.producer_id.to_string()
            && payload["value"]["correlation_id"].to_string()
                == this.correlation_id.to_string() {
            port_write = record;
        }
        if payload["kind"] == "control_inputs_released"
            && record["target"].to_string() == this.target_gid.to_string() {
            lifecycle_release = record;
        }
    }
    if port_write == () || lifecycle_release == () { return; }

    let ports = query("ReadPorts", #{ api_id: this.target_gid });
    if ports == () || ports["ports"] == () { return; }
    let values = #{};
    for port in ports["ports"] {
        if port["direction"] != "in" { continue; }
        values[port["name"]] = port["value"];
    }
    let captured_writes = port_write["payload"]["value"]["writes"];
    let admission_matches = port_write["target"].to_string()
            == this.target_gid.to_string()
        && port_write["scene_generation"].to_string()
            == this.admission["scene_generation"].to_string()
        && port_write["effective_tick"].to_string()
            == this.admission["effective_tick"].to_string()
        && port_write["sequence"].to_string()
            == this.admission["sequence"].to_string();
    let lifecycle_release_ordered = lifecycle_release["target"].to_string()
            == this.target_gid.to_string()
        && lifecycle_release["producer"]["kind"] == "runtime_lifecycle"
        && (lifecycle_release["effective_tick"] > port_write["effective_tick"]
            || (lifecycle_release["effective_tick"] == port_write["effective_tick"]
                && lifecycle_release["sequence"] > port_write["sequence"]));
    let producer_matches = port_write["producer"]["kind"] == "api_transport"
        && port_write["producer"]["value"]["producer_id"].to_string()
            == this.producer_id.to_string()
        && port_write["payload"]["value"]["correlation_id"].to_string()
            == this.correlation_id.to_string();
    let ports_preserved = values["forward"] == 0.7
        && values["side"] == -0.25
        && values["up"] == 0.4
        && values["speed_boost"] == 1.0;
    let stopped = cmd("StopSessionInputCapture", #{});
    let complete = query("ReadSessionInputStream", #{});
    let checks = [
        t_true(stream["state"] == "recording",
            "authority release was observed while input capture remained active"),
        t_true(admission_matches,
            "the endpoint input write retained its admitted fixed-tick identity"),
        t_true(producer_matches && captured_writes == this.expected_writes,
            "the captured endpoint input write differs from the API request"),
        t_true(lifecycle_release_ordered,
            "authority release must enqueue an ordered controller-input release"),
        t_true(ports_preserved,
            "authority release changed endpoint inputs: " + values.to_string()),
        t_true(stopped["ok"] == true && complete["state"] == "complete",
            "the authority-release capture did not complete cleanly"),
    ];
    this.done = true;
    report_verdict(checks, "SESSION AUTHORITY RELEASE PRESERVES PORTS", "SESSION_INPUT_ADMISSION");
    return;
}
