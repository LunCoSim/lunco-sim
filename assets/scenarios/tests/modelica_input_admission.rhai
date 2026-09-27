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
    this.ticks = 0;
    this.done = false;
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;

    let stream = query("ReadSessionInputStream", #{});
    let matching = ();
    if stream["state"] == "recording" {
        for record in stream["records"] {
            let payload = record["payload"];
            if payload["kind"] == "modelica_input_change"
                && payload["value"]["correlation_id"].to_string() == this.correlation_id.to_string() {
                matching = record;
                break;
            }
        }
    }

    if matching != () {
        let payload = matching["payload"]["value"];
        let producer = matching["producer"];
        let ports = query("ReadPorts", #{ api_id: this.target_gid });
        let live_value = ();
        if ports != () && ports["ports"] != () {
            for port in ports["ports"] {
                if port["name"] == this.name && port["direction"] == "in" {
                    live_value = port["value"];
                    break;
                }
            }
        }

        let stopped = cmd("StopSessionInputCapture", #{});
        let complete = query("ReadSessionInputStream", #{});
        let expected = this.admission;
        let checks = [
            t_true(stream["state"] == "recording",
                "Modelica input was not captured while the stream was active"),
            t_true(matching["target"].to_string() == this.target_gid.to_string(),
                "capture target differs from the live acknowledgement"),
            t_true(matching["scene_generation"].to_string() == expected["scene_generation"].to_string()
                && matching["effective_tick"].to_string() == expected["effective_tick"].to_string()
                && matching["sequence"].to_string() == expected["sequence"].to_string(),
                "capture admission stamp differs from the live acknowledgement"),
            t_true(producer["kind"] == "api_transport"
                && producer["value"]["producer_id"].to_string() == this.producer_id.to_string(),
                "capture did not retain the stable API producer identity"),
            t_true(payload["name"] == this.name && payload["value"] == this.value,
                "capture did not retain the declared input name and exact f64 value"),
            t_true(payload["correlation_id"].to_string() == this.correlation_id.to_string(),
                "capture correlation differs from the command acknowledgement"),
            t_true(sim_tick() >= expected["effective_tick"],
                "the assertion ran before the admitted fixed tick"),
            t_true(live_value == this.value,
                "the Modelica input port did not reflect the admitted value"),
            t_true(stopped["ok"] == true && complete["state"] == "complete",
                "the captured Modelica input stream did not complete cleanly"),
        ];
        this.done = true;
        report_verdict(checks, "MODELICA INPUT ADMISSION", "MODELICA_INPUT_ADMISSION");
        return;
    }

    if this.ticks >= 1200 {
        this.done = true;
        report_verdict(["no matching live Modelica input reached its admitted fixed tick"],
            "MODELICA INPUT ADMISSION", "MODELICA_INPUT_ADMISSION");
    }
}
