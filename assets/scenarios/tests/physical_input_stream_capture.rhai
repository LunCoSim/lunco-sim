// Headful production acceptance for physical keyboard input admission.
// Run on physical_rover_controls.usda with RunScenarioAsset so the configured
// key follows InjectWindowInput -> Bevy input mapping -> vessel controller ->
// the bounded fixed-tick SessionInputStream.

const AVATAR_PATH = "/PhysicalRoverControls/Avatar";
const TARGET_PATH = "/PhysicalRoverControls/SkidPhysical";

fn simulation_dependencies(me, ctx) {
    let entity_writes = [];
    let entity_reads = [];
    let modelica_entities = [];
    let avatar = find_path(AVATAR_PATH);
    let target = find_path(TARGET_PATH);
    if avatar >= 0 {
        entity_reads.push(avatar);
        entity_writes.push(avatar);
    }
    if target >= 0 {
        modelica_entities.push(target);
        entity_reads.push(target);
        entity_writes.push(target);
    }
    #{ modelica_entities: modelica_entities, entity_reads: entity_reads,
       entity_writes: entity_writes,
       query_reads: ["ReadSessionInputStream"], required_inputs: [] }
}

fn state_is_valid(me) { this.failure == "" }

fn prepare_capture(me) {
    this.avatar = find_path(AVATAR_PATH);
    this.target = find_path(TARGET_PATH);
    this.forward_key = input_binding("forward");
    if this.avatar < 0 || this.target < 0 {
        this.failure = "physical rover fixture avatar or skid target did not compose";
        return;
    }
    if this.forward_key == () {
        this.failure = "the active input profile has no forward key binding";
        return;
    }

    let started = cmd("StartSessionInputCapture", #{});
    if started == () || !started.ok {
        this.failure = "session input capture did not start: " + started;
        return;
    }
    this.capture_started = true;

    let possession = cmd("AcquireControl", #{
        source: this.avatar,
        target: this.target,
        bind_camera: false,
    });
    if possession == () || !possession.ok {
        this.failure = "fixture avatar could not acquire the skid control surface: " + possession;
    }
}

fn press_forward(me) {
    let result = input_key_press(this.forward_key);
    if result == () || !result.ok {
        this.failure = "configured forward key press was rejected: " + result;
        return;
    }
    this.key_down = true;
}

fn release_forward(me) {
    let result = input_key_release(this.forward_key);
    if result == () || !result.ok {
        this.failure = "configured forward key release was rejected: " + result;
        return;
    }
    this.key_down = false;
}

fn inspect_capture(me) {
    let stopped = cmd("StopSessionInputCapture", #{});
    if stopped == () || !stopped.ok || stopped.data["state"] != "complete" {
        this.failure = "session input capture did not finish cleanly: " + stopped;
        return;
    }
    this.capture_started = false;

    let stream = query("ReadSessionInputStream", #{});
    if stream == () || stream["state"] != "complete"
        || type_of(stream["records"]) != "array" {
        this.failure = "completed session input records were unavailable: " + stream;
        return;
    }

    for record in stream["records"] {
        let payload = record["payload"];
        let producer = record["producer"];
        if payload["kind"] != "physical_intent_frame"
            || producer["kind"] != "physical_controller"
            || record["target"].to_string() != this.target.to_string() {
            continue;
        }
        let intent_ids = payload["value"]["intent_ids"];
        if type_of(intent_ids) != "array" || !intent_ids.contains("forward") {
            continue;
        }
        if type_of(record["scene_generation"]) != "u64"
            || record["scene_generation"].to_string() == "0"
            || type_of(record["effective_tick"]) != "u64"
            || record["effective_tick"].to_string() == "0"
            || type_of(record["sequence"]) != "u64"
            || record["sequence"].to_string() == "0"
            || type_of(producer["value"]["session_id"]) != "u64" {
            this.failure = "captured physical forward frame has no committed admission stamp";
            return;
        }
        this.forward_frame = true;
        break;
    }

    if !this.forward_frame {
        this.failure = "no physical-controller frame retained the configured forward intent for the skid target";
    }
}

fn cleanup_capture(state) {
    if state.forward_key != () {
        input_key_release(state.forward_key);
        state.key_down = false;
    }
    let stream = query("ReadSessionInputStream", #{});
    if stream != () && stream["state"] == "recording" {
        cmd("StopSessionInputCapture", #{});
    }
    state.capture_started = false;
}

fn task(me, ctx) {
    reactive_seq([
        check(Fn("state_is_valid")),
        seq([
            once(Fn("prepare_capture")),
            check(Fn("state_is_valid")),
            once(Fn("press_forward")),
            check(Fn("state_is_valid")),
            wait(0.25),
            once(Fn("release_forward")),
            check(Fn("state_is_valid")),
            wait(0.1),
            once(Fn("inspect_capture")),
            check(Fn("state_is_valid")),
            once(Fn("report_pass")),
        ]),
    ])
}

fn report_pass(me) {
    if this.done { return; }
    this.done = true;
    report_verdict([t_true(true,
        "configured physical forward input was retained with a stable target and fixed-tick admission stamp")],
        "PHYSICAL INPUT STREAM CAPTURE", "PHYSICAL_INPUT_STREAM_CAPTURE");
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.failure = "";
    this.avatar = -1;
    this.target = -1;
    this.forward_key = ();
    this.capture_started = false;
    this.key_down = false;
    this.forward_frame = false;
    subscribe("TASK_FAILED");
}

fn on_event(me, evt, ctx) {
    if evt.name == "TASK_FAILED" && !this.done {
        let message = if this.failure == "" {
            "physical input capture task sequence failed"
        } else { this.failure };
        this.done = true;
        cleanup_capture(this);
        report_verdict([t_true(false, message)],
            "PHYSICAL INPUT STREAM CAPTURE", "PHYSICAL_INPUT_STREAM_CAPTURE");
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 10.0 {
        this.done = true;
        cleanup_capture(this);
        let message = if this.failure == "" {
            "physical input capture did not finish within ten simulation seconds"
        } else { this.failure };
        report_verdict([t_true(false, message)],
            "PHYSICAL INPUT STREAM CAPTURE", "PHYSICAL_INPUT_STREAM_CAPTURE");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        cleanup_capture(this);
        report_verdict([t_true(false,
            "scenario stopped before physical input capture completed")],
            "PHYSICAL INPUT STREAM CAPTURE", "PHYSICAL_INPUT_STREAM_CAPTURE");
    }
}
