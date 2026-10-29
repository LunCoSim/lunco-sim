//! Classroom workshop over one bounded, bundled practice yard.
//! Rhai owns the lesson and timed experiment; Modelica and native physics own
//! all motion. The final report is authored into the participant's USD document.

fn simulation_dependencies(me, ctx) {
    let rover = find_path("/RoverLab/Rover");
    if rover < 0 { throw "Lunar Rover Lab requires its training rover"; }
    #{modelica_entities: [rover], entity_reads: vessel_descendants(find_path("/RoverLab")),
      entity_writes: [rover], query_reads: ["QueryPhysicsState", "QueryUsdPrim",
        "SensorOccupants", "ListOpenDocuments", "InspectUsdDocument", "ListTelemetryChannels", "QueryTelemetryHistory"], required_inputs: []}
}

fn lab_require(result) {
    if result == () || result.ok != true {
        throw "Lunar Rover Lab command rejected: " + result.to_string();
    }
    result
}

fn lab_speed(rover) {
    let state = query("QueryPhysicsState", #{id: rover});
    if state.physics_initialization_invalid || state.rigid_body_disabled
        || !state.computed_mass_finite { throw "the training rover has invalid physics state"; }
    vlen(state.linear_velocity_mps)
}

fn lab_within(entity, ancestor) {
    let current = entity;
    for depth in 0..32 {
        if current == ancestor { return true; }
        current = parent(current);
        if current == () || current < 0 { return false; }
    }
    false
}

fn lab_camera(doc) {
    let pose = query("QueryUsdPrim", #{doc_id: doc, path: "/RoverLab/Avatar",
        attrs: ["xformOp:translate", "lunco:cameraLookAt"]}).attrs;
    lab_require(cmd("SetActiveCamera", #{name: "/RoverLab/Avatar"}));
    lab_require(cmd("SetCameraLookAt", #{camera: find_path("/RoverLab/Avatar"),
        eye: pose["xformOp:translate"], target: pose["lunco:cameraLookAt"]}));
}

fn lab_button(id, label) { #{id: "rover_lab:" + id, label: label, enabled: true} }

fn lab_show(s) {
    let labels = ["1/6 · Drive and stop at the green marker", "2/6 · Read a measured speed signal",
        "3/6 · Predict and run a comparison", "4/6 · Explain the measurements",
        "5/6 · Move the orange block into the blue bay", "6/6 · Save your personal Twin"];
    let text = labels[s.phase] + "\n\n";
    let actions = [];
    if s.phase == 0 {
        text += "Take control. Drive with " + input_hint("forward") + "/" + input_hint("backward")
            + ", steer with " + input_hint("left") + "/" + input_hint("right")
            + ", and brake with " + input_hint("brake") + ". Stop inside the green marker, then check your stop.";
        actions.push(lab_button("control", "Take control"));
        actions.push(lab_button("check_stop", "Check my stop"));
    } else if s.phase == 1 {
        text += "Open the speed graph. Find a non-zero sample from your drive, then check the signal. Speed is measured in m/s.";
        actions.push(lab_button("control", "Take control"));
        actions.push(lab_button("graph", "Open speed graph"));
        actions.push(lab_button("check_signal", "Check my signal"));
    } else if s.phase == 2 {
        text += "We will increase drive demand from 0.25 to 0.65. Both trials use the same rover and mass, start from rest on uniform flat ground, and run for 3 simulation seconds. Predict the final speed.";
        if s.prediction == "" {
            actions.push(lab_button("predict_lower", "Lower speed"));
            actions.push(lab_button("predict_similar", "Similar speed"));
            actions.push(lab_button("predict_higher", "Higher speed"));
        } else if !s.running {
            text += "\nYour prediction: " + s.prediction + ". This is an illustrative model experiment, not calibrated hardware data.";
            actions.push(lab_button("run", "Run both trials"));
        } else { text += "\nExperiment running. The simulation owns the rover until both trials stop."; }
    } else if s.phase == 3 {
        text += "Baseline: " + r4(s.baseline_speed).to_string() + " m/s at " + r4(s.baseline_mass).to_string()
            + " kg total.\nChanged drive demand: " + r4(s.variant_speed).to_string() + " m/s at "
            + r4(s.variant_mass).to_string() + " kg total.\nWhich result does the evidence support? Differences below 0.02 m/s count as similar.";
        actions.push(lab_button("explain_lower", "Changed run is slower"));
        actions.push(lab_button("explain_similar", "Speeds are similar"));
        actions.push(lab_button("explain_higher", "Changed run is faster"));
    } else if s.phase == 4 {
        text += "In Build, select BuildBlock in Entities. Move the orange block to the centre of the blue bay (X 9, Y 0.5, Z 3 m). The block's authored position is checked.";
        actions.push(lab_button("build", "Open Build"));
    } else {
        text += "Your two measurements and block placement are prepared in this scene. Save as Twin into a new folder. The saved report confirms completion after the new Twin opens; cancelling the picker leaves this step unfinished.";
        actions.push(lab_button("save", "Save as Twin…"));
    }
    if s.message != "" { text += "\n\n" + s.message; }
    hint("lunar_rover_lab", text);
    if !s.running {
        actions.push(lab_button("recover", "Right rover"));
        actions.push(lab_button("camera", "Return camera"));
        actions.push(lab_button("help", "More help"));
        if s.phase < 5 { actions.push(lab_button("checkpoints", "Instructor checkpoints")); }
    }
    guided_hud_actions("lunar_rover_lab", "tutorials", "action", actions);
    emit("ROVER_LAB_STEP", #{step: s.phase + 1, practice: s.practice});
}

fn on_start(me, ctx) {
    this.rover = find_path("/RoverLab/Rover");
    this.sensor = find_path("/RoverLab/Flag/Trigger");
    this.doc = assembly_edit::document_for_entity(this.rover);
    this.projected_generation = usd_document_generation(this.doc);
    this.phase = 0; this.practice = false; this.arrived = false; this.graph_open = false;
    this.prediction = ""; this.running = false; this.message = "";
    this.baseline_speed = (); this.variant_speed = (); this.baseline_mass = (); this.variant_mass = ();
    let saved = query("QueryUsdPrim", #{doc_id: this.doc, path: "/RoverLab/Results/Report", attrs: ["inputs:baseline_speed_mps", "inputs:changed_speed_mps"]}).attrs;
    this.finished = saved["inputs:baseline_speed_mps"] > 0.0 && saved["inputs:changed_speed_mps"] > 0.0;
    if this.finished { return; }
    if ctx.checkpoint != () {
        if ctx.checkpoint < 0 || ctx.checkpoint > 2 { throw "workshop checkpoint must be 0, 1, or 2"; }
        this.phase = ctx.checkpoint; this.practice = ctx.checkpoint != 0;
        this.message = "Instructor practice checkpoint: earlier tasks are unassessed.";
    }
    hud_perspectives("lunar_rover_lab", []);
    if !is_unattended() {
        lab_require(cmd("SetGuidedHudLayout", #{hud: "lunar_rover_lab", width: 440.0, text_scale: 1.15}));
        lab_require(cmd("ActivatePerspective", #{id: "sandbox_view"}));
        lab_camera(this.doc);
    }
    // Existing contact is authoritative too; startup does not replay old enters.
    let occupancy = query("SensorOccupants", #{sensor: this.sensor});
    for entity in occupancy.entrants {
        if lab_within(entity, this.rover) { this.arrived = true; }
    }
    lab_show(this);
}

fn lab_signal(rover) {
    let channels = query("ListTelemetryChannels", #{name: "linear_speed"});
    for channel in channels.channels {
        if channel.owner.kind == "api" && channel.owner.api_id == rover { return channel; }
    }
    throw "the training rover's linear_speed channel is unavailable";
}

fn lab_signal_assessment(rover) {
    let channel = lab_signal(rover);
    let history = query("QueryTelemetryHistory", #{key: channel.key, limit: 20000});
    let peak = 0.0;
    for sample in history.samples { peak = max(peak, sample.v); }
    #{samples: history.samples.len(), peak_mps: peak}
}

fn on_event(me, evt, ctx) {
    if this.finished { return; }
    if evt.name == "usd.document.projected" && evt.value.doc_id == this.doc {
        this.projected_generation = evt.value.generation;
    }
    if sensor_entered(evt, this.sensor) && lab_within(evt.value, this.rover) { this.arrived = true; }
    if evt.name != "tutorial.action" || !string_is(evt.value) || !evt.value.starts_with("rover_lab:") { return; }
    let action = evt.value.sub_string(10);
    this.message = "";
    if action == "camera" && !this.running { lab_camera(this.doc); }
    else if action == "recover" && !this.running {
        lab_require(cmd("RecoverVessel", #{entity_id: this.rover}));
        this.message = "The rover was righted at its current location. Your lesson progress remains.";
    } else if action == "control" && this.phase <= 1 { lab_require(possess(this.rover)); }
    else if action == "check_stop" && this.phase == 0 {
        let occupancy = query("SensorOccupants", #{sensor: this.sensor});
        let inside = false;
        for entity in occupancy.entrants { if lab_within(entity, this.rover) { inside = true; } }
        if inside && this.arrived && controller(this.rover) != () && lab_speed(this.rover) < 0.15 {
            this.phase = 1;
        } else { this.message = "Take control of this rover, enter the green marker, and brake to below 0.15 m/s. Selection alone does not give driving control."; }
    } else if action == "graph" && this.phase == 1 {
        let channel = lab_signal(this.rover);
        if !is_unattended() {
            lab_require(cmd("ActivatePerspective", #{id: "analysis"}));
            lab_require(cmd("NewPlotPanel", #{title: "Training rover speed · m/s",
                signals: [channel.name], target_gid: this.rover}));
        }
        this.graph_open = true;
    } else if action == "check_signal" && this.phase == 1 {
        let assessment = lab_signal_assessment(this.rover);
        emit("ROVER_LAB_SIGNAL", #{opened: this.graph_open, samples: assessment.samples, peak_mps: assessment.peak_mps});
        if this.graph_open && assessment.samples >= 3 && assessment.peak_mps > 0.1 { this.phase = 2; }
        else { this.message = "Open the speed graph and collect at least three samples including motion above 0.1 m/s. Recorded " + assessment.samples.to_string() + " samples; peak " + assessment.peak_mps.to_string() + " m/s. A flat zero trace does not demonstrate driving."; }
    } else if action.starts_with("predict_") && this.phase == 2 && !this.running {
        let prediction = action.sub_string(8);
        if !["lower", "similar", "higher"].contains(prediction) { throw "unknown workshop prediction"; }
        this.prediction = prediction;
    } else if action == "run" && this.phase == 2 && !this.running && this.prediction != "" {
        this.running = true;
        emit("ROVER_LAB_RUN", 0);
    } else if action.starts_with("explain_") && this.phase == 3 {
        let difference = this.variant_speed - this.baseline_speed;
        let observed = if abs(difference) < 0.02 { "similar" } else if difference < 0.0 { "lower" } else { "higher" };
        if action.sub_string(8) == observed {
            this.phase = 4; emit("ROVER_LAB_INTERPRETED", 0);
            if !is_unattended() { lab_require(cmd("ActivatePerspective", #{id: "rover_build"})); }
        } else { this.message = "Compare the two measured speeds again. Your initial prediction can be wrong; your explanation should match the evidence."; }
    } else if action == "build" && this.phase == 4 {
        if !is_unattended() { lab_require(cmd("ActivatePerspective", #{id: "rover_build"})); }
    } else if action == "save" && this.phase == 5 { lab_require(cmd("SaveAsTwin", #{})); }
    else if action == "help" {
        this.message = "Driving: take control, then use the displayed bindings. Lost view: Return camera. Rolled over: Right rover. Build: enter X 9, Y 0.5, Z 3 in the block's translation. Saving: choose a new empty folder.";
    } else if action == "checkpoints" && !this.running {
        hint("lunar_rover_lab", "Instructor practice: begin at a lesson checkpoint in this yard. Earlier tasks remain unassessed. Restart whole lab discards unsaved practice edits.");
        guided_hud_actions("lunar_rover_lab", "tutorials", "action", [
            lab_button("checkpoint_0", "1 · Drive"), lab_button("checkpoint_1", "2 · Telemetry"),
            lab_button("checkpoint_2", "3 · Comparison"), lab_button("restart", "Restart whole lab (discard edits)"), lab_button("cancel_checkpoint", "Keep working")]);
        return;
    } else if action.starts_with("checkpoint_") && !this.running {
        let checkpoint = parse_int(action.sub_string(11));
        if checkpoint < 0 || checkpoint > 2 { throw "unknown workshop checkpoint"; }
        lab_require(cmd("RunScenarioAsset", #{target: me, source_asset: "lunco://tutorials/sandbox/rover_lab.rhai",
            reload_policy: "Restart",
            params: #{checkpoint: checkpoint}}));
        return;
    } else if action == "restart" && !this.running {
        lab_require(cmd("RestartScene", #{reset_document: true}));
        return;
    }
    lab_show(this);
}

fn lab_prepare_trial(s) {
    lab_require(cmd("ReleaseControlSource", #{source: find_path("/RoverLab/Avatar")}));
    lab_require(brake(s.rover));
    lab_require(cmd("RecoverVessel", #{entity_id: s.rover}));
}

fn lab_prepare_baseline(me) { lab_prepare_trial(this); }
fn lab_prepare_variant(me) { lab_prepare_trial(this); }
fn lab_comparison_requested(me) { this.running }
fn lab_interpreted(me) { this.phase == 4 }
fn lab_trial_ready(me) {
    let state = query("QueryPhysicsState", #{id: this.rover});
    state.physics_state_ready && !state.physics_initialization_invalid && !state.rigid_body_disabled
}
fn lab_drive_baseline(me) { lab_require(drive(this.rover, 0.25, 0.0)); }
fn lab_drive_variant(me) { lab_require(drive(this.rover, 0.65, 0.0)); }

fn lab_sample_baseline(me) {
    this.baseline_speed = lab_speed(this.rover);
    this.baseline_mass = query("QueryPhysicsState", #{id: this.rover}).mass_kg;
    if this.baseline_speed <= 0.01 { throw "baseline trial did not move the rover"; }
    lab_require(brake(this.rover));
    emit("ROVER_LAB_SAMPLE", #{trial: "baseline", speed_mps: this.baseline_speed, mass_kg: this.baseline_mass});
}

fn lab_sample_variant(me) {
    this.variant_speed = lab_speed(this.rover);
    this.variant_mass = query("QueryPhysicsState", #{id: this.rover}).mass_kg;
    if this.variant_speed <= 0.01 || abs(this.variant_mass - this.baseline_mass) > 0.001 {
        throw "changed-demand trial did not preserve mass and produce a moving rover";
    }
    lab_require(brake(this.rover));
    this.running = false; this.phase = 3;
    emit("ROVER_LAB_SAMPLE", #{trial: "changed_demand", speed_mps: this.variant_speed, mass_kg: this.variant_mass});
    lab_show(this);
}

fn lab_block_placed(me) {
    if this.projected_generation != usd_document_generation(this.doc) { return false; }
    let authored = query("InspectUsdDocument", #{doc_id: this.doc, path: "/RoverLab/BuildBlock"});
    let persistent = false;
    for attribute in authored.prim.attributes {
        if attribute.name == "xformOp:translate" { persistent = attribute.authored.root; }
    }
    if !persistent { return false; }
    let block = query("QueryUsdPrim", #{doc_id: this.doc, path: "/RoverLab/BuildBlock", attrs: ["xformOp:translate"]});
    let translation = block.attrs["xformOp:translate"];
    translation != () && vlen(vsub(translation, [9.0, 0.5, 3.0])) <= 0.75
}

fn lab_prepare_report(me) {
    this.phase = 5;
    let report = "/RoverLab/Results/Report";
    let ops = [];
    for pair in [["baseline_speed_mps", this.baseline_speed], ["changed_speed_mps", this.variant_speed],
        ["baseline_mass_kg", this.baseline_mass], ["changed_mass_kg", this.variant_mass]] {
        ops.push(#{SetAttribute: #{edit_target: "@root@", path: report, name: "inputs:" + pair[0], type_name: "double", value: pair[1].to_string()}});
    }
    let prediction_code = if this.prediction == "lower" { -1.0 } else if this.prediction == "higher" { 1.0 } else { 0.0 };
    ops.push(#{SetAttribute: #{edit_target: "@root@", path: report, name: "inputs:prediction_code", type_name: "double", value: prediction_code.to_string()}});
    ops.push(#{SetAttribute: #{edit_target: "@root@", path: report, name: "inputs:practice", type_name: "double", value: if this.practice { "1.0" } else { "0.0" }}});
    lab_require(cmd("ApplyUsdOps", #{doc_id: this.doc, label: "Record the classroom experiment", ops: ops}));
    emit("ROVER_LAB_READY_TO_SAVE", #{practice: this.practice});
    lab_show(this);
}

fn task(me, ctx) {
    seq([
        wait_until(Fn("lab_comparison_requested")), once(Fn("lab_prepare_baseline")),
        wait_until(Fn("lab_trial_ready")), wait(1.0), once(Fn("lab_drive_baseline")),
        wait(3.0), once(Fn("lab_sample_baseline")), wait(1.0),
        once(Fn("lab_prepare_variant")), wait_until(Fn("lab_trial_ready")),
        wait(1.0), once(Fn("lab_drive_variant")), wait(3.0), once(Fn("lab_sample_variant")),
        wait_until(Fn("lab_interpreted")), wait_until(Fn("lab_block_placed")), once(Fn("lab_prepare_report")),
    ])
}
