// @timing presentation
//! The persisted experiment report resumes in the participant's saved Twin.
//! Saving is verified from the document owner and indexed manifest; opening or
//! cancelling a picker never counts as a completed save.

fn report_saved(me) {
    if this.projected_generation != usd_document_generation(this.doc) { return false; }
    let measurements = query("QueryUsdPrim", #{doc_id: this.doc, path: "/RoverLab/Results/Report",
        attrs: ["inputs:baseline_speed_mps", "inputs:changed_speed_mps"]}).attrs;
    for key in ["inputs:baseline_speed_mps", "inputs:changed_speed_mps"] {
        if measurements[key] == () || !measurements[key].is_finite() || measurements[key] < 0.0 {
            throw "workshop report has an invalid measurement slot: " + key;
        }
        // The template explicitly authors zero for an unmeasured slot.
        if measurements[key] == 0.0 { return false; }
    }
    let twin = query("ReadActiveTwinContract", #{});
    if !twin.open || !twin.manifest_present { return false; }
    let documents = query("ListOpenDocuments", #{});
    for doc in documents.open_documents {
        if doc.doc_id == this.doc {
            if doc.dirty || doc.origin.kind != "file" || !doc.origin.writable
                || doc.runtime_context.kind != "local_twin" || doc.runtime_context.twin != twin.twin_id {
                return false;
            }
            // Save-As changes the file origin before the new Twin is admitted.
            // Completion requires that exact saved file in the active index.
            for file in query("ListTwin", #{}).files {
                if file.absolute_path == doc.origin.path { return true; }
            }
            return false;
        }
    }
    false
}

fn report_complete(me) {
    let report = query("QueryUsdPrim", #{doc_id: this.doc, path: "/RoverLab/Results/Report", attrs: [
        "inputs:baseline_speed_mps", "inputs:changed_speed_mps", "inputs:baseline_mass_kg",
        "inputs:changed_mass_kg", "inputs:prediction_code", "inputs:practice"]}).attrs;
    for key in ["inputs:baseline_speed_mps", "inputs:changed_speed_mps", "inputs:baseline_mass_kg", "inputs:changed_mass_kg"] {
        if report[key] == () || !report[key].is_finite() || report[key] <= 0.0 {
            throw "saved workshop report is missing a positive measured value: " + key;
        }
    }
    if ![-1.0, 0.0, 1.0].contains(report["inputs:prediction_code"])
        || ![0.0, 1.0].contains(report["inputs:practice"]) {
        throw "saved workshop report has an invalid prediction or practice flag";
    }
    let prediction = if report["inputs:prediction_code"] < 0.0 { "lower" } else if report["inputs:prediction_code"] > 0.0 { "higher" } else { "similar" };
    let title = if report["inputs:practice"] > 0.5 { "Practice result saved · earlier tasks were unassessed" }
        else { "Lunar Rover Lab complete · your Twin is saved" };
    hint("lunar_rover_lab", title + "\n\nBaseline: " + r4(report["inputs:baseline_speed_mps"]).to_string()
        + " m/s at " + r4(report["inputs:baseline_mass_kg"]).to_string() + " kg.\nChanged drive demand: "
        + r4(report["inputs:changed_speed_mps"]).to_string() + " m/s at "
        + r4(report["inputs:changed_mass_kg"]).to_string() + " kg.\nPrediction: "
        + prediction + ".\n\nExplain what changed, what stayed constant, and what the model leaves out. Reopen this Twin to revisit the saved report.");
    guided_hud_actions("lunar_rover_lab", "tutorials", "action", [
        #{id: "rover_lab_result:driving", label: "Next: Moon Driving Basics", enabled: true},
    ]);
    emit("ROVER_LAB_SAVED", report);
}

fn on_start(me, ctx) {
    this.doc = assembly_edit::document_for_entity(me);
    this.projected_generation = usd_document_generation(this.doc);
    hud_perspectives("lunar_rover_lab", []);
    if !is_unattended() { cmd("SetGuidedHudLayout", #{hud: "lunar_rover_lab", width: 440.0, text_scale: 1.15}); }
}

fn on_event(me, evt, ctx) {
    if evt.name == "usd.document.projected" && evt.value.doc_id == this.doc {
        this.projected_generation = evt.value.generation;
    }
    if evt.name == "tutorial.action" && evt.value == "rover_lab_result:driving" {
        tutorials::start(#{source_asset: "lunco://tutorials/basic/b1_driving_basics.rhai",
            scene_asset: "lunco://tutorials/basic/driving_basics.usda", reload_policy: "Restart"});
    }
}

fn task(me, ctx) { seq([wait_until(Fn("report_saved")), once(Fn("report_complete"))]) }
