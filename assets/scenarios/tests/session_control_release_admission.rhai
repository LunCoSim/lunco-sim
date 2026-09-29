// Production API acceptance for ordered port writes and releases.
// @timing simulation

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [find_path("/RocketEngineObservables/Engine")], entity_reads: [], entity_writes: [],
       query_reads: ["ReadSessionInputStream", "ReadPorts"], required_inputs: [] }
}

fn on_start(me, params) {
    this.target_gid = params["target_gid"];
    this.write_producer_id = params["write_producer_id"];
    this.write_correlation_id = params["write_correlation_id"];
    this.write_admission = params["write_admission"];
    this.write_value = params["write_value"];
    this.port_producer_id = params["port_producer_id"];
    this.port_correlation_id = params["port_correlation_id"];
    this.port_admission = params["port_admission"];
    this.control_producer_id = params["control_producer_id"];
    this.control_correlation_id = params["control_correlation_id"];
    this.control_admission = params["control_admission"];
    this.expected_writes = [["throttle", this.write_value]];
    this.ticks = 0;
    this.done = false;
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;

    let stream = query("ReadSessionInputStream", #{});
    let port_write = ();
    let port_release = ();
    let control_release = ();
    if stream["state"] == "recording" {
        for record in stream["records"] {
            let payload = record["payload"];
            if payload["kind"] == "port_input_writes"
                && payload["value"]["correlation_id"].to_string() == this.write_correlation_id.to_string() {
                port_write = record;
            }
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

    if port_write != () && port_release != () && control_release != () {
        let write_payload = port_write["payload"]["value"];
        let release_payload = port_release["payload"]["value"];
        let control_payload = control_release["payload"]["value"];
        let write_stamp = this.write_admission;
        let release_stamp = this.port_admission;
        let control_stamp = this.control_admission;
        if sim_tick() >= control_stamp["effective_tick"] {
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
                    "admitted writes and releases were not captured while the stream was active"),
                t_true(port_write["target"].to_string() == this.target_gid.to_string()
                    && port_write["scene_generation"].to_string() == write_stamp["scene_generation"].to_string()
                    && port_write["effective_tick"].to_string() == write_stamp["effective_tick"].to_string()
                    && port_write["sequence"].to_string() == write_stamp["sequence"].to_string(),
                    "SetPorts target or admission stamp differs from its acknowledgement"),
                t_true(port_write["producer"]["kind"] == "api_transport"
                    && port_write["producer"]["value"]["producer_id"].to_string() == this.write_producer_id.to_string()
                    && write_payload["writes"] == this.expected_writes
                    && write_payload["correlation_id"].to_string() == this.write_correlation_id.to_string(),
                    "SetPorts capture did not retain its producer, value, and correlation"),
                t_true(port_release["target"].to_string() == this.target_gid.to_string()
                    && port_release["scene_generation"].to_string() == release_stamp["scene_generation"].to_string()
                    && port_release["effective_tick"].to_string() == release_stamp["effective_tick"].to_string()
                    && port_release["sequence"].to_string() == release_stamp["sequence"].to_string(),
                    "ReleasePort target or admission stamp differs from its acknowledgement"),
                t_true(port_release["producer"]["kind"] == "api_transport"
                    && port_release["producer"]["value"]["producer_id"].to_string() == this.port_producer_id.to_string()
                    && release_payload["name"] == "throttle"
                    && release_payload["correlation_id"].to_string() == this.port_correlation_id.to_string(),
                    "ReleasePort capture did not retain its producer, port, and correlation"),
                t_true(control_release["target"].to_string() == this.target_gid.to_string()
                    && control_release["scene_generation"].to_string() == control_stamp["scene_generation"].to_string()
                    && control_release["effective_tick"].to_string() == control_stamp["effective_tick"].to_string()
                    && control_release["sequence"].to_string() == control_stamp["sequence"].to_string(),
                    "ReleaseControl target or admission stamp differs from its acknowledgement"),
                t_true(control_release["producer"]["kind"] == "api_transport"
                    && control_release["producer"]["value"]["producer_id"].to_string() == this.control_producer_id.to_string()
                    && control_payload["correlation_id"].to_string() == this.control_correlation_id.to_string(),
                    "ReleaseControl capture did not retain its producer and correlation"),
                t_true(port_write["effective_tick"].to_string() == port_release["effective_tick"].to_string()
                    && port_release["effective_tick"].to_string() == control_release["effective_tick"].to_string()
                    && port_write["sequence"] < port_release["sequence"]
                    && port_release["sequence"] < control_release["sequence"],
                    "same-tick inputs did not commit in their admitted shared sequence order"),
                t_true(live_throttle == this.write_value && this.write_value != 0.0,
                    "ReleaseControl changed the latest explicit throttle write"),
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
        report_verdict(["ordered port writes and releases did not reach their admitted fixed tick"],
            "SESSION CONTROL RELEASE ADMISSION", "SESSION_INPUT_ADMISSION");
    }
}
