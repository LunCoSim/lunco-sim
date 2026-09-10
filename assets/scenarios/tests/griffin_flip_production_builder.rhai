// TEST-ONLY. The whole observable production-shaped Griffin/FLIP construction
// test is authored in Rhai. Only the empty mission frame is static USD input.

const ROOT = "/GriffinFlipProductionBuilderTest";
const MISSION = ROOT + "/Mission";
const LANDER = MISSION + "/Lander";
const FLIP = MISSION + "/FLIP";
const ADAPTER = MISSION + "/FLIPPayloadAdapterJoint";
const PAYLOAD_DECK = FLIP + "/PayloadDeck";
const SENSOR_MAST = FLIP + "/SensorMast";
const SOLAR_PANEL = FLIP + "/SolarPanel";
const CHASSIS = FLIP + "/Chassis";
const FRONT_CAMERA = FLIP + "/FrontCamera";
const LANDER_ASSET = "lunco://vessels/landers/descent_lander.usda";
const FLIP_ASSET = "lunco://vessels/rovers/flip_rover.usda";
const WHEELS = [
    FLIP + "/Suspension_FL/Wheel_FL",
    FLIP + "/Suspension_FR/Wheel_FR",
    FLIP + "/Suspension_RL/Wheel_RL",
    FLIP + "/Suspension_RR/Wheel_RR",
];

fn query_ok(record) {
    record != () && record.ok != false
}

fn close(actual, expected, epsilon) {
    actual != () && expected != () && (actual - expected).abs() <= epsilon
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "GRIFFIN FLIP PRODUCTION BUILDER",
            "GRIFFIN_FLIP_PRODUCTION_BUILDER");
    }
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" {
            let root = query("QueryUsdPrim", #{
                doc_id: item.doc_id,
                path: MISSION,
                children: true,
            });
            if query_ok(root) { return item.doc_id; }
        }
    }
    -1
}

fn mission_manifest() {
    #{
        root_path: MISSION,
        lander_path: LANDER,
        flip_path: FLIP,
        adapter_path: ADAPTER,
        lander_asset: LANDER_ASSET,
        flip_asset: FLIP_ASSET,
        lander_translation: [0.0, 6.0, 0.0],
        lander_rotation: [0.0, 0.0, 0.0],
        flip_translation: [0.0, 2.0, 0.0],
        flip_rotation: [0.0, 0.0, 0.0],
        adapter_local_pos0: [0.0, -4.0, 0.0],
        adapter_local_pos1: [0.0, 0.0, 0.0],
        adapter_local_rot0: [1.0, 0.0, 0.0, 0.0],
        adapter_local_rot1: [1.0, 0.0, 0.0, 0.0],
        wheel_paths: WHEELS,
        minimum_separation: 1.0,
    }
}

fn on_start(me) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.wait_generation = -1;
    this.invalid_adapter = false;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 {
                fail(this, "production Griffin/FLIP fixture was not discoverable");
            }
            return;
        }
    }

    let document = assembly_edit::describe(this.doc);
    if !query_ok(document) || document.generation == () {
        fail(this, "production Griffin/FLIP fixture did not expose a generation");
        return;
    }
    if (this.phase == 1 && document.generation <= this.wait_generation) {
        if this.elapsed > 12.0 {
            fail(this, "production Griffin/FLIP composition did not reach a new generation");
        }
        return;
    }
    if this.phase == 1 {
        let mission = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: MISSION,
            children: true,
        });
        let lander = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: LANDER,
            attrs: ["xformOp:translate", "xformOp:rotateXYZ"],
            children: true,
        });
        let flip = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: FLIP,
            attrs: ["xformOp:translate", "xformOp:rotateXYZ"],
            children: true,
        });
        if !query_ok(mission) || !query_ok(lander) || lander.children == ()
            || lander.children.len() == 0 || !query_ok(flip)
            || flip.children == () || flip.children.len() == 0 {
            if this.elapsed > 12.0 {
                fail(this, "production Griffin/FLIP reference bodies did not materialize");
            }
            return;
        }
        let bad_adapter_manifest = mission_manifest();
        bad_adapter_manifest.adapter_path = LANDER;
        let invalid_adapter = griffin_flip_builder::griffin_mission_adapter_plan(
            this.doc, "@root@", bad_adapter_manifest,
        );
        let adapter_plan = griffin_flip_builder::griffin_mission_adapter_plan(
            this.doc, "@root@", mission_manifest(),
        );
        if !adapter_plan.ok || adapter_plan.kind != "griffin_mission_adapter"
            || adapter_plan.ops.len() != 8 || invalid_adapter.ok
            || !invalid_adapter.error.contains("distinct") {
            fail(this, "production Griffin/FLIP adapter plan did not build or reject its explicit contract");
            return;
        }
        this.invalid_adapter = !invalid_adapter.ok;
        let proposal = assembly_edit::propose(
            this.doc, "Assembly", "Attach production Griffin and FLIP adapter",
            adapter_plan.ops, document.generation,
        );
        if !query_ok(proposal) {
            fail(this, "production Griffin/FLIP adapter proposal was rejected: " + proposal.error);
            return;
        }
        let review = assembly_edit::review_session(this.doc);
        if !query_ok(review) || review.proposals.len() != 1
            || review.proposals[0].diagnostics.len() != 0 {
            fail(this, "production Griffin/FLIP adapter proposal did not validate cleanly");
            return;
        }
        let committed = assembly_edit::commit_proposal(review.proposals[0].id);
        if !query_ok(committed) {
            fail(this, "production Griffin/FLIP adapter could not be committed: " + committed.error);
            return;
        }
        this.wait_generation = document.generation;
        this.phase = 2;
        return;
    }

    if this.phase == 2 && document.generation <= this.wait_generation {
        if this.elapsed > 16.0 {
            fail(this, "production Griffin/FLIP adapter did not reach a new generation");
        }
        return;
    }
    if this.phase == 2 {
        let mission = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: MISSION,
            children: true,
        });
        let lander = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: LANDER,
            children: true,
        });
        let flip = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: FLIP,
            attrs: [
                "lunco:mission:payload", "lunco:mission:vehicle_family",
                "lunco:configuration", "lunco:parameter_status",
                "physics:mass",
                "xformOp:translate", "xformOp:rotateXYZ",
            ],
            children: true,
        });
        let payload_deck = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: PAYLOAD_DECK,
            children: false,
        });
        let sensor_mast = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: SENSOR_MAST,
            children: true,
        });
        let solar_panel = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: SOLAR_PANEL,
            attrs: ["lunco:installation", "inputs:area", "inputs:efficiency"],
            children: false,
        });
        let chassis = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: CHASSIS,
            children: false,
        });
        let front_camera = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: FRONT_CAMERA,
            children: false,
        });
        let adapter = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ADAPTER,
            attrs: [
                "physics:localPos0", "physics:localPos1",
                "physics:localRot0", "physics:localRot1",
            ],
            rels: ["physics:body0", "physics:body1"],
        });
        let wheel_ready = true;
        let wheel_contract = true;
        for i in 0..WHEELS.len() {
            let wheel = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: WHEELS[i],
                attrs: [
                    "physxVehicleWheelAttachment:index",
                    "physxVehicleWheel:radius",
                    "physxVehicleWheel:width",
                ],
                children: false,
            });
            if !query_ok(wheel) || wheel.attrs == () {
                wheel_ready = false;
                continue;
            }
            if wheel.type_name != "Cylinder"
                || wheel.attrs["physxVehicleWheelAttachment:index"] != i
                || !close(wheel.attrs["physxVehicleWheel:radius"], 0.45, 0.001)
                || !close(wheel.attrs["physxVehicleWheel:width"], 0.28, 0.001) {
                wheel_contract = false;
            }
        }
        if !wheel_ready {
            if this.elapsed > 20.0 {
                fail(this, "production FLIP wheel references did not expose their authored metadata");
            }
            return;
        }
        let flip_identity = query_ok(flip) && flip.attrs != ()
            && flip.attrs["lunco:mission:payload"] == "Astrolab FLIP"
            && flip.attrs["lunco:mission:vehicle_family"] == "FLEX Lunar Innovation Platform"
            && flip.attrs["lunco:configuration"].contains("study proxy")
            && flip.attrs["lunco:parameter_status"].contains("simulator assumptions")
            && close(flip.attrs["physics:mass"], 450.0, 0.001);
        let flip_topology = query_ok(payload_deck) && payload_deck.type_name == "Cube"
            && query_ok(sensor_mast) && sensor_mast.type_name == "Xform"
            && sensor_mast.children != () && sensor_mast.children.len() == 2
            && query_ok(solar_panel) && solar_panel.attrs != ()
            && close(solar_panel.attrs["inputs:area"], 4.8, 0.001)
            && close(solar_panel.attrs["inputs:efficiency"], 0.30, 0.001)
            && solar_panel.attrs["lunco:installation"].contains("body-mounted")
            && query_ok(chassis) && chassis.type_name == "Cube"
            && query_ok(front_camera) && front_camera.type_name == "Camera";
        this.done = true;
        report_verdict([
            t_true(query_ok(mission) && mission.children.contains(LANDER)
                && mission.children.contains(FLIP)
                && mission.children.contains(ADAPTER),
                "dynamic plans materialize the lander, FLIP, and adapter under the mission frame"),
            t_true(query_ok(lander) && lander.children.len() > 0,
                "maintained descent_lander reference composes visible child structure"),
            t_true(query_ok(flip) && flip.children.len() > 0,
                "dedicated FLIP reference composes visible child structure"),
            t_true(flip_identity,
                "dedicated FLIP reference preserves its authored mission identity and study boundary"),
            t_true(flip_topology,
                "dedicated FLIP reference composes its payload deck, sensor mast, and solar proxy"),
            t_true(lander.attrs != ()
                && vector_close(lander.attrs["xformOp:translate"], [0.0, 6.0, 0.0], 0.001)
                && vector_close(lander.attrs["xformOp:rotateXYZ"], [0.0, 0.0, 0.0], 0.001)
                && flip.attrs != ()
                && vector_close(flip.attrs["xformOp:translate"], [0.0, 2.0, 0.0], 0.001)
                && vector_close(flip.attrs["xformOp:rotateXYZ"], [0.0, 0.0, 0.0], 0.001),
                "dynamic reference plans preserve the explicit lander and FLIP placements"),
            t_true(wheel_contract,
                "the composed FLIP wheel paths retain the four authored wheel contracts"),
            t_true(query_ok(adapter)
                && adapter.relationships != ()
                && adapter.relationships["physics:body0"] != ()
                && adapter.relationships["physics:body1"] != ()
                && adapter.relationships["physics:body0"][0] == LANDER
                && adapter.relationships["physics:body1"][0] == FLIP,
                "the adapter is an explicit fixed joint between the composed bodies"),
            t_true(adapter.attrs != ()
                && vector_close(adapter.attrs["physics:localPos0"], [0.0, -4.0, 0.0], 0.001)
                && vector_close(adapter.attrs["physics:localPos1"], [0.0, 0.0, 0.0], 0.001),
                "the adapter keeps its explicit local anchors"),
            t_true(this.invalid_asset && this.invalid_overlap
                && this.invalid_wheels && this.invalid_identity
                && this.invalid_adapter,
                "invalid asset, placement, wheel, and identity contracts fail before commit"),
        ], "GRIFFIN FLIP PRODUCTION BUILDER", "GRIFFIN_FLIP_PRODUCTION_BUILDER");
        return;
    }

    let valid = griffin_flip_builder::griffin_mission_assembly_plan(
        this.doc, "@root@", mission_manifest(),
    );
    let bad_asset_manifest = mission_manifest();
    bad_asset_manifest.flip_asset = "lunco://vessels/rovers/ackermann_rover.usda";
    let invalid_asset = griffin_flip_builder::griffin_mission_assembly_plan(
        this.doc, "@root@", bad_asset_manifest,
    );
    let bad_overlap_manifest = mission_manifest();
    bad_overlap_manifest.flip_translation = bad_overlap_manifest.lander_translation;
    let invalid_overlap = griffin_flip_builder::griffin_mission_assembly_plan(
        this.doc, "@root@", bad_overlap_manifest,
    );
    let bad_wheels_manifest = mission_manifest();
    bad_wheels_manifest.wheel_paths[1] = bad_wheels_manifest.wheel_paths[0];
    let invalid_wheels = griffin_flip_builder::griffin_mission_assembly_plan(
        this.doc, "@root@", bad_wheels_manifest,
    );
    let bad_identity_manifest = mission_manifest();
    bad_identity_manifest.adapter_path = LANDER;
    let invalid_identity = griffin_flip_builder::griffin_mission_assembly_plan(
        this.doc, "@root@", bad_identity_manifest,
    );
    if !valid.ok || valid.kind != "griffin_mission_assembly"
        || valid.ops.len() != 6
        || !invalid_asset.error.contains("dedicated")
        || !invalid_overlap.error.contains("minimum separation")
        || !invalid_wheels.error.contains("unique")
        || !invalid_identity.error.contains("distinct") {
        fail(this, "production Griffin/FLIP plan did not build or reject its explicit contracts");
        return;
    }
    this.invalid_asset = !invalid_asset.ok;
    this.invalid_overlap = !invalid_overlap.ok;
    this.invalid_wheels = !invalid_wheels.ok;
    this.invalid_identity = !invalid_identity.ok;
    let proposal = assembly_edit::propose(
        this.doc, "Assembly", "Build production Griffin and FLIP assembly",
        valid.ops, document.generation,
    );
    if !query_ok(proposal) {
        fail(this, "production Griffin/FLIP proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals.len() != 1
        || review.proposals[0].diagnostics.len() != 0 {
        fail(this, "production Griffin/FLIP proposal did not validate cleanly");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "production Griffin/FLIP proposal could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = document.generation;
    this.phase = 1;
}

fn vector_close(actual, expected, epsilon) {
    if actual == () || expected == () || actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if (actual[i] - expected[i]).abs() > epsilon { return false; }
    }
    true
}
