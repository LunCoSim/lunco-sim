// Production API acceptance for ordered port releases and safe-stop commits.
// @timing simulation

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [find_path("/RocketEngineObservables/Engine")], entity_reads: [], entity_writes: [],
       query_reads: ["ReadSessionInputStream", "ReadPorts"], required_inputs: [] }
}

fn on_start(me, params) {
    this.target_gid = params["target_gid"];
    this.port_producer_id = params["port_producer_id"];
    this.port_correlation_id = params["port_correlation_id"];
    this.port_admission = params["port_admission"];
    this.control_producer_id = params["control_producer_id"];
    this.control_correlation_id = params["control_correlation_id"];
    this.control_admission = params["control_admission"];
    this.ticks = 0;
    this.done = false;
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;

    let stream = query("ReadSessionInputStream", #{});
    let port_release = ();
    let control_release = ();
    if stream["state"] == "recording" {
        for record in stream["records"] {
            let payload = record["payload"];
            if payload["kind"] == "port_input_release"
                && payload["value"]["correlation_id"].to_string() == this.port_correlation_id.to_string() {
                port_release = record;
            }
            if payload["kind"] == "control_input_release"
                && payload["value"]["correlation_id"].to_string() == this.control_correlation_id.to_string() {
                control_release = record;
            }
        }
    }

    if port_release != () && control_release != () {
        let port_payload = port_release["payload"]["value"];
        let control_payload = control_release["payload"]["value"];
        let port_stamp = this.port_admission;
        let control_stamp = this.control_admission;
        if sim_tick() >= max(port_stamp["effective_tick"], control_stamp["effective_tick"]) {
            let ports = query("ReadPorts", #{ api_id: this.target_gid });
            let live_throttle = ();
            if ports != () && ports["ports"] != () {
                for port in ports["ports"] {
                    if port["direction"] == "in" && port["name"] == "throttle" {
                        live_throttle = port["value"];
                    }
                }
            }

            let stopped = cmd("StopSessionInputCapture", #{});
            let complete = query("ReadSessionInputStream", #{});
            let checks = [
                t_true(stream["state"] == "recording",
                    "port releases were not captured while the stream was active"),
                t_true(port_release["target"].to_string() == this.target_gid.to_string()
                    && port_release["scene_generation"].to_string() == port_stamp["scene_generation"].to_string()
                    && port_release["effective_tick"].to_string() == port_stamp["effective_tick"].to_string()
                    && port_release["sequence"].to_string() == port_stamp["sequence"].to_string(),
                    "ReleasePort target or admission stamp differs from its acknowledgement"),
                t_true(port_release["producer"]["kind"] == "api_transport"
                    && port_release["producer"]["value"]["producer_id"].to_string() == this.port_producer_id.to_string()
                    && port_payload["name"] == "throttle"
                    && port_payload["correlation_id"].to_string() == this.port_correlation_id.to_string(),
                    "ReleasePort capture did not retain producer, port, and correlation"),
                t_true(control_release["target"].to_string() == this.target_gid.to_string()
                    && control_release["scene_generation"].to_string() == control_stamp["scene_generation"].to_string()
                    && control_release["effective_tick"].to_string() == control_stamp["effective_tick"].to_string()
                    && control_release["sequence"].to_string() == control_stamp["sequence"].to_string(),
                    "ReleaseControl target or admission stamp differs from its acknowledgement"),
                t_true(control_release["producer"]["kind"] == "api_transport"
                    && control_release["producer"]["value"]["producer_id"].to_string() == this.control_producer_id.to_string()
                    && control_payload["correlation_id"].to_string() == this.control_correlation_id.to_string(),
                    "ReleaseControl capture did not retain producer and correlation"),
                t_true(live_throttle == 0.0,
                    "ReleaseControl did not apply the safe throttle value at its admitted tick"),
                t_true(stopped["ok"] == true && complete["state"] == "complete",
                    "the captured control-release stream did not complete cleanly"),
            ];
            this.done = true;
            report_verdict(checks, "SESSION CONTROL RELEASE ADMISSION", "SESSION_INPUT_ADMISSION");
            return;
        }
    }

    if this.ticks >= 1200 {
        this.done = true;
        report_verdict(["no matching control-release records reached their admitted fixed ticks"],
            "SESSION CONTROL RELEASE ADMISSION", "SESSION_INPUT_ADMISSION");
    }
}
