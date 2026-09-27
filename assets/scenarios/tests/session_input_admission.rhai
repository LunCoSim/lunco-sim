// Production API acceptance for live Modelica input admission and fixed-tick application.
// @timing simulation

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [find_path("/RocketEngineObservables/Engine")], entity_reads: [], entity_writes: [],
       query_reads: ["ReadSessionInputStream", "ReadPorts"], required_inputs: [] }
}

fn on_start(me, params) {
    this.name = params["name"];
    this.value = params["value"];
    this.target_gid = params["target_gid"];
    this.producer_id = params["producer_id"];
    this.correlation_id = params["correlation_id"];
    this.admission = params["admission"];
    this.port_producer_id = params["port_producer_id"];
    this.port_correlation_id = params["port_correlation_id"];
    this.port_admission = params["port_admission"];
    this.port_value = params["port_value"];
    this.ticks = 0;
    this.done = false;
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;

    let stream = query("ReadSessionInputStream", #{});
    let matching = ();
    let port_matching = ();
    if stream["state"] == "recording" {
        for record in stream["records"] {
            let payload = record["payload"];
            if payload["kind"] == "modelica_input_change"
                && payload["value"]["correlation_id"].to_string() == this.correlation_id.to_string() {
                matching = record;
            }
            if payload["kind"] == "port_input_writes"
                && payload["value"]["correlation_id"].to_string() == this.port_correlation_id.to_string() {
                port_matching = record;
            }
        }
    }

    if matching != () && port_matching != () {
        let payload = matching["payload"]["value"];
        let producer = matching["producer"];
        let port_payload = port_matching["payload"]["value"];
        let port_producer = port_matching["producer"];
        let observed_port_value = ();
        if port_payload["writes"] != () {
            for write in port_payload["writes"] {
                if write[0] == "throttle" {
                    observed_port_value = write[1];
                    break;
                }
            }
        }
        let ports = query("ReadPorts", #{ api_id: this.target_gid });
        let live_port_value = ();
        if ports != () && ports["ports"] != () {
            for port in ports["ports"] {
                if port["direction"] == "in" {
                    if port["name"] == "throttle" {
                        live_port_value = port["value"];
                    }
                }
            }
        }

        let stopped = cmd("StopSessionInputCapture", #{});
        let complete = query("ReadSessionInputStream", #{});
        let expected = this.admission;
        let expected_port = this.port_admission;
        let checks = [
            t_true(stream["state"] == "recording",
                "external simulation inputs were not captured while the stream was active"),
            t_true(matching["target"].to_string() == this.target_gid.to_string(),
                "Modelica capture target differs from the live acknowledgement"),
            t_true(matching["scene_generation"].to_string() == expected["scene_generation"].to_string()
                && matching["effective_tick"].to_string() == expected["effective_tick"].to_string()
                && matching["sequence"].to_string() == expected["sequence"].to_string(),
                "Modelica capture admission stamp differs from the live acknowledgement"),
            t_true(producer["kind"] == "api_transport"
                && producer["value"]["producer_id"].to_string() == this.producer_id.to_string(),
                "Modelica capture did not retain the stable API producer identity"),
            t_true(payload["name"] == this.name && payload["value"] == this.value,
                "Modelica capture did not retain the declared input name and exact f64 value"),
            t_true(payload["correlation_id"].to_string() == this.correlation_id.to_string(),
                "Modelica capture correlation differs from the command acknowledgement"),
            t_true(port_matching["target"].to_string() == this.target_gid.to_string()
                && port_matching["scene_generation"].to_string() == expected_port["scene_generation"].to_string()
                && port_matching["effective_tick"].to_string() == expected_port["effective_tick"].to_string()
                && port_matching["sequence"].to_string() == expected_port["sequence"].to_string(),
                "SetPorts capture target or admission stamp differs from its acknowledgement"),
            t_true(port_producer["kind"] == "api_transport"
                && port_producer["value"]["producer_id"].to_string() == this.port_producer_id.to_string(),
                "SetPorts capture did not retain its stable API producer identity"),
            t_true(observed_port_value == this.port_value,
                "SetPorts capture did not retain the exact ordered port write"),
            t_true(port_payload["correlation_id"].to_string() == this.port_correlation_id.to_string(),
                "SetPorts capture correlation differs from its command acknowledgement"),
            t_true(sim_tick() >= expected["effective_tick"] && sim_tick() >= expected_port["effective_tick"],
                "the assertions ran before both admitted fixed ticks"),
            t_true(live_port_value == this.port_value,
                "the later generic port write did not remain applied at its admitted fixed tick"),
            t_true(stopped["ok"] == true && complete["state"] == "complete",
                "the captured simulation input stream did not complete cleanly"),
        ];
        this.done = true;
        report_verdict(checks, "SESSION INPUT ADMISSION", "SESSION_INPUT_ADMISSION");
        return;
    }

    if this.ticks >= 1200 {
        this.done = true;
        report_verdict(["no matching live Modelica input reached its admitted fixed tick"],
            "SESSION INPUT ADMISSION", "SESSION_INPUT_ADMISSION");
    }
}
