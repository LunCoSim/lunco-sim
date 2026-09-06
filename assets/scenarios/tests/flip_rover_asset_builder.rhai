// TEST-ONLY. Builds the dedicated FLIP composition from an empty authored
// frame through dynamic Rhai plans. The recipe composes maintained rover
// components and adds local geometry through typed frame/shape plans; this
// test does not embed a generated USDA model or a Rust-side authoring path.

const ROOT = "/FlipRoverAssetBuilderTest";
const PARENT = ROOT + "/AssetFrame";
const FLIP = PARENT + "/FLIP";
const CHASSIS = FLIP + "/Chassis";
const FRONT_CAMERA = FLIP + "/FrontCamera";
const PAYLOAD_DECK = FLIP + "/PayloadDeck";
const SENSOR_MAST = FLIP + "/SensorMast";
const SOLAR_PANEL = FLIP + "/SolarPanel";
const BASE_ASSET = "lunco://vessels/rovers/skid_rover.usda";

fn wheel_paths() {
    let flip = "/FlipRoverAssetBuilderTest/AssetFrame/FLIP";
    [
        flip + "/Suspension_FL/Wheel_FL",
        flip + "/Suspension_FR/Wheel_FR",
        flip + "/Suspension_RL/Wheel_RL",
        flip + "/Suspension_RR/Wheel_RR",
    ]
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn close(actual, expected, epsilon) {
    actual != () && expected != () && (actual - expected).abs() <= epsilon
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "FLIP ROVER ASSET BUILDER",
            "FLIP_ROVER_ASSET_BUILDER");
    }
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" {
            let frame = query("QueryUsdPrim", #{
                doc: item.doc_id,
                path: PARENT,
                children: true,
            });
            if query_ok(frame) { return item.doc_id; }
        }
    }
    -1
}

fn variants() {
    [
        #{ variant_set: "drivetrain", variant: "raycast" },
        #{ variant_set: "power", variant: "battery" },
        #{ variant_set: "generation", variant: "solar" },
        #{ variant_set: "thermal", variant: "none" },
    ]
}

fn wheel_manifest() {
    let wheels = wheel_paths();
    [
        #{ station: FLIP + "/Suspension_FL", wheel: wheels[0], index: 0,
            translation: [-0.95, -0.55, -1.05], radius: 0.45, width: 0.28, moi: 3.4 },
        #{ station: FLIP + "/Suspension_FR", wheel: wheels[1], index: 1,
            translation: [0.95, -0.55, -1.05], radius: 0.45, width: 0.28, moi: 3.4 },
        #{ station: FLIP + "/Suspension_RL", wheel: wheels[2], index: 2,
            translation: [-0.95, -0.55, 1.05], radius: 0.45, width: 0.28, moi: 3.4 },
        #{ station: FLIP + "/Suspension_RR", wheel: wheels[3], index: 3,
            translation: [0.95, -0.55, 1.05], radius: 0.45, width: 0.28, moi: 3.4 },
    ]
}

fn variants_authored(doc) {
    let snapshot = assembly_edit::sync_document(doc, ());
    if !query_ok(snapshot) || snapshot.layers == () || snapshot.layers.root == () {
        return false;
    }
    let source = snapshot.layers.root.source;
    source.contains("drivetrain") && source.contains("raycast")
        && source.contains("power") && source.contains("battery")
        && source.contains("generation") && source.contains("solar")
        && source.contains("thermal")
}

fn detail_authored(doc, path) {
    let root = query("QueryUsdPrim", #{
        doc: doc,
        path: path,
        attrs: ["physics:mass"],
        children: false,
    });
    let solar = query("QueryUsdPrim", #{
        doc: doc,
        path: path + "/SolarPanel",
        attrs: ["lunco:installation"],
        children: false,
    });
    query_ok(root) && root.attrs != () && root.attrs["physics:mass"] != ()
        && query_ok(solar) && solar.attrs != ()
        && solar.attrs["lunco:installation"] != ()
}

fn on_start(me) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.wait_generation = -1;
    this.invalid_base = false;
    this.invalid_parent = false;
    this.invalid_shape = false;
    this.invalid_duplicate = false;
    this.invalid_detail = false;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 {
                fail(this, "FLIP asset-builder fixture was not discoverable");
            }
            return;
        }
    }

    let document = assembly_edit::describe(this.doc);
    if !query_ok(document) || document.generation == () {
        fail(this, "FLIP asset-builder fixture did not expose a generation");
        return;
    }

    if this.phase == 1 && document.generation <= this.wait_generation {
        if this.elapsed > 12.0 {
            fail(this, "FLIP asset skeleton did not reach a new generation");
        }
        return;
    }
    if this.phase == 1 {
        let root = query("QueryUsdPrim", #{
            doc: this.doc,
            path: FLIP,
            children: true,
        });
        if !query_ok(root) || root.children == () || root.children.len() == 0 {
            if this.elapsed > 12.0 {
                fail(this, "FLIP asset skeleton did not compose its children");
            }
            return;
        }
        let duplicate = griffin_flip_builder::flip_rover_asset_plan(
            this.doc, "@root@", PARENT, "FLIP", BASE_ASSET,
            [0.0, 1.0, 0.0], [0.0, 0.0, 0.0],
        );
        let variants = assembly_builder::select_variants_plan(
            this.doc, "@root@", FLIP, variants(),
        );
        let missing_target = assembly_builder::select_variants_plan(
            this.doc, "@root@", PARENT + "/Missing", variants(),
        );
        if duplicate.ok || !variants.ok || variants.ops.len() != 4
            || missing_target.ok {
            fail(this, "FLIP asset variant phase did not reject or build its explicit plans");
            return;
        }
        let proposal = assembly_edit::propose(
            this.doc, #{ Assembly: () }, "Select FLIP asset component variants",
            variants.ops, document.generation,
        );
        if !query_ok(proposal) {
            fail(this, "FLIP asset variant proposal was rejected: " + proposal.error);
            return;
        }
        let review = assembly_edit::review_session(this.doc);
        if !query_ok(review) || review.proposals.len() != 1
            || review.proposals[0].diagnostics.len() != 0 {
            fail(this, "FLIP asset variant proposal did not validate cleanly");
            return;
        }
        let committed = assembly_edit::commit_proposal(review.proposals[0].id);
        if !query_ok(committed) {
            fail(this, "FLIP asset variant proposal could not be committed: " + committed.error);
            return;
        }
        this.wait_generation = document.generation;
        this.phase = 2;
        return;
    }

    if this.phase == 2 && document.generation <= this.wait_generation {
        if this.elapsed > 16.0 {
            fail(this, "FLIP asset variants did not reach a new generation");
        }
        return;
    }
    if this.phase == 2 {
        let solar = query("QueryUsdPrim", #{
            doc: this.doc,
            path: SOLAR_PANEL,
            children: false,
        });
        if !query_ok(solar) {
            if this.elapsed > 16.0 {
                fail(this, "FLIP asset solar variant did not compose");
            }
            return;
        }
        let wheels = wheel_manifest();
        let bad_wheels = wheel_manifest();
        bad_wheels[1].wheel = bad_wheels[0].wheel;
        let invalid_detail = griffin_flip_builder::flip_rover_asset_detail_plan(
            this.doc, "@root@", FLIP, bad_wheels,
        );
        let detail = griffin_flip_builder::flip_rover_asset_detail_plan(
            this.doc, "@root@", FLIP, wheels,
        );
        if invalid_detail.ok || !invalid_detail.error.contains("unique")
            || !detail.ok || detail.ops.len() <= 20 {
            fail(this, "FLIP asset detail plan did not build or reject its explicit wheel contract");
            return;
        }
        let proposal = assembly_edit::propose(
            this.doc, #{ Assembly: () }, "Apply FLIP asset facts and alignment",
            detail.ops, document.generation,
        );
        if !query_ok(proposal) {
            fail(this, "FLIP asset detail proposal was rejected: " + proposal.error);
            return;
        }
        let review = assembly_edit::review_session(this.doc);
        if !query_ok(review) || review.proposals.len() != 1
            || review.proposals[0].diagnostics.len() != 0 {
            fail(this, "FLIP asset detail proposal did not validate cleanly");
            return;
        }
        let committed = assembly_edit::commit_proposal(review.proposals[0].id);
        if !query_ok(committed) {
            fail(this, "FLIP asset detail proposal could not be committed: " + committed.error);
            return;
        }
        this.wait_generation = document.generation;
        this.phase = 3;
        return;
    }

    if this.phase == 3 && document.generation <= this.wait_generation {
        if this.elapsed > 20.0 {
            fail(this, "FLIP asset detail commit did not reach a new generation");
        }
        return;
    }
    if this.phase == 3 {
        let wheels = wheel_paths();
        let invalid_base = griffin_flip_builder::flip_rover_asset_plan(
            this.doc, "@root@", PARENT, "BadBase", "lunco://vessels/rovers/flip_rover.usda",
            [0.0, 1.0, 0.0], [0.0, 0.0, 0.0],
        );
        let invalid_parent = assembly_builder::referenced_instance_plan(
            this.doc, "@root@", PARENT + "/Missing", "Bad", "Xform", BASE_ASSET,
            (), (), (), (), (),
        );
        let invalid_shape = assembly_builder::cylinder_shape_plan(
            "@root@", PARENT, "BadShape", -0.1, 0.5,
            (), (), (), false,
        );
        let duplicate = griffin_flip_builder::flip_rover_asset_plan(
            this.doc, "@root@", PARENT, "FLIP", BASE_ASSET,
            [0.0, 1.0, 0.0], [0.0, 0.0, 0.0],
        );
        let bad_wheels = wheel_manifest();
        bad_wheels[1].wheel = bad_wheels[0].wheel;
        let invalid_detail = griffin_flip_builder::flip_rover_asset_detail_plan(
            this.doc, "@root@", FLIP, bad_wheels,
        );
        let root = query("QueryUsdPrim", #{
            doc: this.doc,
            path: FLIP,
            attrs: [
                "lunco:mission:payload", "lunco:mission:vehicle_family",
                "lunco:configuration", "lunco:parameter_status", "physics:mass",
            ],
            children: true,
        });
        let chassis = query("QueryUsdPrim", #{ doc: this.doc, path: CHASSIS, children: false });
        let camera = query("QueryUsdPrim", #{ doc: this.doc, path: FRONT_CAMERA, children: false });
        let payload = query("QueryUsdPrim", #{ doc: this.doc, path: PAYLOAD_DECK, children: false });
        let mast = query("QueryUsdPrim", #{ doc: this.doc, path: SENSOR_MAST, children: true });
        let post = query("QueryUsdPrim", #{
            doc: this.doc, path: SENSOR_MAST + "/Post",
            attrs: ["radius", "height", "physics:collisionEnabled"], children: false,
        });
        let bar = query("QueryUsdPrim", #{
            doc: this.doc, path: SENSOR_MAST + "/SensorBar",
            attrs: ["size", "physics:collisionEnabled"], children: false,
        });
        let solar = query("QueryUsdPrim", #{
            doc: this.doc, path: SOLAR_PANEL,
            attrs: ["lunco:installation", "inputs:area", "inputs:efficiency"],
            children: false,
        });
        let wheel_contract = true;
        for i in 0..wheels.len() {
            let wheel = query("QueryUsdPrim", #{
                doc: this.doc,
                path: wheels[i],
                attrs: [
                    "physxVehicleWheelAttachment:index",
                    "physxVehicleWheel:radius", "physxVehicleWheel:width",
                    "physxVehicleWheel:moi",
                ],
                children: false,
            });
            if !query_ok(wheel) || wheel.attrs == ()
                || wheel.type_name != "Cylinder"
                || wheel.attrs["physxVehicleWheelAttachment:index"] != i
                || !close(wheel.attrs["physxVehicleWheel:radius"], 0.45, 0.001)
                || !close(wheel.attrs["physxVehicleWheel:width"], 0.28, 0.001)
                || !close(wheel.attrs["physxVehicleWheel:moi"], 3.4, 0.001) {
                wheel_contract = false;
            }
        }
        this.done = true;
        report_verdict([
            t_true(query_ok(root) && root.children != () && root.children.len() > 0,
                "dynamic FLIP asset recipe composes a referenced root and local authored frames"),
            t_true(query_ok(root) && root.attrs != ()
                && root.attrs["lunco:mission:payload"] == "Astrolab FLIP"
                && root.attrs["lunco:mission:vehicle_family"] == "FLEX Lunar Innovation Platform"
                && root.attrs["lunco:configuration"].contains("study proxy")
                && close(root.attrs["physics:mass"], 450.0, 0.001),
                "FLIP identity, study boundary, and mass are applied after composition"),
            t_true(query_ok(chassis) && chassis.type_name == "Cube",
                "the composed chassis remains the shared Cube component"),
            t_true(query_ok(camera) && camera.type_name == "Camera",
                "the composed rover camera remains queryable"),
            t_true(wheel_contract,
                "all four wheel stations retain their explicit FLIP dimensions and indexes"),
            t_true(query_ok(payload) && payload.type_name == "Cube"
                && payload.attrs["physics:collisionEnabled"] == true,
                "the dynamic payload deck is a physical authored shape"),
            t_true(query_ok(mast) && mast.type_name == "Xform"
                && mast.children != () && mast.children.len() == 2,
                "the dynamic sensor mast owns its two authored shape children"),
            t_true(query_ok(post) && post.attrs["physics:collisionEnabled"] == false
                && close(post.attrs["radius"], 0.06, 0.001)
                && close(post.attrs["height"], 0.55, 0.001)
                && query_ok(bar) && bar.attrs["physics:collisionEnabled"] == false,
                "visual-only mast shapes carry explicit collision policy"),
            t_true(query_ok(solar) && solar.attrs != ()
                && solar.attrs["lunco:installation"].contains("body-mounted")
                && close(solar.attrs["inputs:area"], 4.8, 0.001)
                && close(solar.attrs["inputs:efficiency"], 0.30, 0.001),
                "the selected solar component receives explicit FLIP installation facts"),
            t_true(!invalid_base.ok && !invalid_parent.ok && !invalid_shape.ok
                && !duplicate.ok && !invalid_detail.ok,
                "invalid base, parent, shape, identity, and wheel plans fail before proposal"),
        ], "FLIP ROVER ASSET BUILDER", "FLIP_ROVER_ASSET_BUILDER");
        return;
    }

    let invalid_base = griffin_flip_builder::flip_rover_asset_plan(
        this.doc, "@root@", PARENT, "BadBase", "lunco://vessels/rovers/flip_rover.usda",
        [0.0, 1.0, 0.0], [0.0, 0.0, 0.0],
    );
    let invalid_parent = assembly_builder::referenced_instance_plan(
        this.doc, "@root@", PARENT + "/Missing", "Bad", "Xform", BASE_ASSET,
        (), (), (), (), (),
    );
    let invalid_shape = assembly_builder::cylinder_shape_plan(
        "@root@", PARENT, "BadShape", -0.1, 0.5,
        (), (), (), false,
    );
    let existing = query("QueryUsdPrim", #{
        doc: this.doc,
        path: FLIP,
        children: false,
    });
    if query_ok(existing) {
        // The reviewed commit reprojects the document and reloads this
        // scenario. The source fixture is still empty, so resume from the
        // committed composed root instead of proposing it a second time.
        this.invalid_base = !invalid_base.ok;
        this.invalid_parent = !invalid_parent.ok;
        this.invalid_shape = !invalid_shape.ok;
        this.wait_generation = document.generation - 1;
        if detail_authored(this.doc, FLIP) {
            this.invalid_duplicate = true;
            this.invalid_detail = true;
            this.phase = 3;
        } else if variants_authored(this.doc) {
            this.phase = 2;
        } else {
            this.phase = 1;
        }
        return;
    }
    let valid = griffin_flip_builder::flip_rover_asset_plan(
        this.doc, "@root@", PARENT, "FLIP", BASE_ASSET,
        [0.0, 1.0, 0.0], [0.0, 0.0, 0.0],
    );
    if !valid.ok || valid.kind != "flip_rover_asset" || valid.ops.len() <= 20
        || invalid_base.ok || !invalid_base.error.contains("skid_rover")
        || invalid_parent.ok || invalid_shape.ok {
        fail(this, "FLIP asset plan did not build or reject its explicit construction contracts");
        return;
    }
    this.invalid_base = !invalid_base.ok;
    this.invalid_parent = !invalid_parent.ok;
    this.invalid_shape = !invalid_shape.ok;
    let proposal = assembly_edit::propose(
        this.doc, #{ Assembly: () }, "Build FLIP asset composition",
        valid.ops, document.generation,
    );
    if !query_ok(proposal) {
        fail(this, "FLIP asset proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals.len() != 1
        || review.proposals[0].diagnostics.len() != 0 {
        fail(this, "FLIP asset proposal did not validate cleanly");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "FLIP asset proposal could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = document.generation;
    this.phase = 1;
}
