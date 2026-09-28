// Production API acceptance for capturing authority-derived control safe-stops.
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
    this.ticks = 0;
    this.done = false;
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;

    let stream = query("ReadSessionInputStream", #{});
    let safe_stop = ();
    let command_write = ();
    if stream["state"] == "recording" {
        for record in stream["records"] {
            let payload = record["payload"];
            if payload["kind"] == "control_safe_stop"
                && record["producer"]["kind"] == "runtime_lifecycle" {
                safe_stop = record;
            }
            if payload["kind"] == "port_input_writes"
                && record["producer"]["kind"] == "api_transport"
                && record["producer"]["value"]["producer_id"].to_string()
                    == this.producer_id.to_string()
                && payload["value"]["correlation_id"].to_string()
                    == this.correlation_id.to_string() {
                command_write = record;
            }
        }
    }

    let safe_effective_tick = if safe_stop == () {
        0
    } else {
        safe_stop["effective_tick"].to_int()
    };
    if safe_stop != () && sim_tick() >= safe_effective_tick {
        let ports = query("ReadPorts", #{ api_id: this.target_gid });
        let forward = ();
        let side = ();
        let up = ();
        let speed_boost = ();
        if ports != () && ports["ports"] != () {
            for port in ports["ports"] {
                if port["direction"] == "in" && port["name"] == "forward" {
                    forward = port["value"];
                }
                if port["direction"] == "in" && port["name"] == "side" {
                    side = port["value"];
                }
                if port["direction"] == "in" && port["name"] == "up" {
                    up = port["value"];
                }
                if port["direction"] == "in" && port["name"] == "speed_boost" {
                    speed_boost = port["value"];
                }
            }
        }

        let stopped = cmd("StopSessionInputCapture", #{});
        let complete = query("ReadSessionInputStream", #{});
        let input_recorded = command_write != ();
        let command_tick = if input_recorded {
            command_write["effective_tick"].to_int()
        } else { 0 };
        let command_sequence = if input_recorded {
            command_write["sequence"].to_int()
        } else { 0 };
        let safe_sequence = safe_stop["sequence"].to_int();
        let record_ordered = input_recorded
            && (command_tick < safe_effective_tick
                || (command_tick == safe_effective_tick
                    && command_sequence < safe_sequence));
        let captured_writes = [];
        if input_recorded && command_write["payload"]["value"]["writes"] != () {
            captured_writes = command_write["payload"]["value"]["writes"];
        }
        let expected_writes = [
            ["forward", 0.7], ["side", -0.25], ["up", 0.4], ["speed_boost", 1.0],
        ];
        let checks = [
            t_true(stream["state"] == "recording",
                "lifecycle safe-stop was not captured while the stream was active"),
            t_true(safe_stop["target"].to_string() == this.target_gid.to_string()
                && safe_stop["scene_generation"].to_int() > 0
                && safe_effective_tick <= sim_tick()
                && safe_stop["sequence"].to_int() > 0,
                "lifecycle safe-stop did not retain its stable target and fixed-tick stamp"),
            t_true(safe_stop["producer"]["kind"] == "runtime_lifecycle",
                "authority safe-stop lost its lifecycle producer identity"),
            t_true(input_recorded
                && command_write["target"].to_string() == this.target_gid.to_string()
                && command_write["scene_generation"].to_string()
                    == this.admission["scene_generation"].to_string()
                && command_write["effective_tick"].to_string()
                    == this.admission["effective_tick"].to_string()
                && command_write["sequence"].to_string()
                    == this.admission["sequence"].to_string(),
                "the live API control write did not retain its admitted tick"),
            t_true(command_write["producer"]["kind"] == "api_transport"
                && command_write["producer"]["value"]["producer_id"].to_string()
                    == this.producer_id.to_string(),
                "the captured control write lost its API producer identity"),
            t_true(command_write["payload"]["value"]["correlation_id"].to_string()
                == this.correlation_id.to_string()
                && captured_writes == expected_writes,
                "the captured control write differs from the live API request"),
            t_true(record_ordered,
                "lifecycle order write=(" + command_tick + "," + command_sequence
                    + ") safe-stop=(" + safe_effective_tick + "," + safe_sequence + ")"),
            t_true(forward == 0.0 && side == 0.0 && up == 0.0 && speed_boost == 0.0,
                "safe-stop values were forward=" + forward + " side=" + side
                    + " up=" + up + " speed_boost=" + speed_boost),
            t_true(stopped["ok"] == true && complete["state"] == "complete",
                "the lifecycle safe-stop capture did not complete cleanly"),
        ];
        this.done = true;
        report_verdict(checks, "SESSION LIFECYCLE SAFE-STOP CAPTURE", "SESSION_INPUT_ADMISSION");
        return;
    }

    if this.ticks >= 1200 {
        this.done = true;
        report_verdict(["no lifecycle safe-stop record reached its effective fixed tick"],
            "SESSION LIFECYCLE SAFE-STOP CAPTURE", "SESSION_INPUT_ADMISSION");
    }
}
