// TEST-ONLY. Builds a four-wheel rover reference into an empty authored
// assembly through the dynamic Rhai builder and the normal review/journal
// boundary, then checks the composed vehicle contract.

const ROOT = "/AssemblyComponentBuilderTest";
const ASSEMBLY = ROOT + "/Assembly";
const FLIP = ASSEMBLY + "/FLIP";
const ASSET = "lunco://vessels/rovers/skid_rover.usda";
const WHEELS = [
    FLIP + "/Suspension_FL/Wheel_FL",
    FLIP + "/Suspension_FR/Wheel_FR",
    FLIP + "/Suspension_RL/Wheel_RL",
    FLIP + "/Suspension_RR/Wheel_RR",
];

fn query_ok(record) {
    record != () && record.ok != false
}

fn prim_exists(doc, path) {
    let record = query("QueryUsdPrim", #{
        doc: doc,
        path: path,
        children: false,
    });
    query_ok(record)
}

fn variants_authored(doc) {
    let snapshot = assembly_edit::sync_document(doc, ());
    if !query_ok(snapshot) || snapshot.layers == () || snapshot.layers.root == () {
        return false;
    }
    let source = snapshot.layers.root.source;
    source.contains("drivetrain") && source.contains("raycast")
        && source.contains("generation") && source.contains("thermal")
}

fn array_has(values, expected) {
    if values == () { return false; }
    for value in values {
        if value == expected { return true; }
    }
    false
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "ASSEMBLY COMPONENT BUILDER", "ASSEMBLY_COMPONENT_BUILDER");
    }
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" {
            let root = query("QueryUsdPrim", #{
                doc: item.doc_id,
                path: ROOT,
                children: false,
            });
            if query_ok(root) { return item.doc_id; }
        }
    }
    -1
}

fn variants() {
    [
        #{ variant_set: "drivetrain", variant: "raycast" },
        #{ variant_set: "power", variant: "infinite" },
        #{ variant_set: "generation", variant: "none" },
        #{ variant_set: "thermal", variant: "none" },
    ]
}

fn wheel_record(doc, path) {
    query("QueryUsdPrim", #{
        doc: doc,
        path: path,
        attrs: [
            "physxVehicleWheelAttachment:index",
            "physxVehicleWheel:radius",
            "physxVehicleWheel:width",
            "lunco:suspension:restLength",
        ],
        children: false,
    })
}

fn on_start(me) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.wait_generation = -1;
    this.variant_ops = 0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "empty component-builder fixture was not discoverable"); }
            return;
        }
    }

    let before = assembly_edit::describe(this.doc);
    if !query_ok(before) || before.generation == () {
        fail(this, "empty assembly did not expose a document generation");
        return;
    }

    if this.phase == 0 {
        // Applying the reviewed reference reprojects the document and can
        // reload this authored scenario. Preserve the one real commit by
        // continuing from the composed instance instead of proposing it a
        // second time; the source fixture itself remains empty.
        if prim_exists(this.doc, FLIP) {
            this.wait_generation = before.generation - 1;
            if variants_authored(this.doc) {
                this.variant_ops = 4;
                this.phase = 2;
            } else {
                this.phase = 1;
            }
            return;
        }
        let plan = assembly_builder::flip_rover_instance_plan(
            this.doc, "@root@", ASSEMBLY, "FLIP", ASSET,
            [0.0, 1.0, 0.0], [0.0, 0.0, 0.0], WHEELS,
        );
        let duplicate_wheel = assembly_builder::flip_rover_instance_plan(
            this.doc, "@root@", ASSEMBLY, "Duplicate", ASSET,
            [0.0, 1.0, 0.0], [0.0, 0.0, 0.0],
            [WHEELS[0], WHEELS[0], WHEELS[2], WHEELS[3]],
        );
        let outside_wheel = assembly_builder::flip_rover_instance_plan(
            this.doc, "@root@", ASSEMBLY, "Outside", ASSET,
            [0.0, 1.0, 0.0], [0.0, 0.0, 0.0],
            [WHEELS[0], ROOT + "/NotAWheel", WHEELS[2], WHEELS[3]],
        );
        let missing_parent = assembly_builder::referenced_instance_plan(
            this.doc, "@root@", ASSEMBLY + "/Missing", "Bad", "Xform", ASSET,
            (), (), (), (), (),
        );
        let invalid_asset = assembly_builder::referenced_instance_plan(
            this.doc, "@root@", ASSEMBLY, "BadAsset", "Xform",
            "components/mobility/wheel.usda", (), (), (), (), (),
        );
        let missing_variant_target = assembly_builder::select_variants_plan(
            this.doc, "@root@", ASSEMBLY + "/Missing", variants(),
        );
        if !plan.ok || plan.kind != "flip_rover_instance" || plan.ops.len() != 3
            || duplicate_wheel.ok || outside_wheel.ok || missing_parent.ok
            || invalid_asset.ok || missing_variant_target.ok {
            fail(this, "component-instance plan did not build or reject its explicit contract");
            return;
        }

        let proposal = assembly_edit::propose(
            this.doc, #{ Assembly: () }, "Build FLIP component instance",
            plan.ops, before.generation,
        );
        if !query_ok(proposal) {
            fail(this, "component-instance proposal was rejected: " + proposal.error);
            return;
        }
        let review = assembly_edit::review_session(this.doc);
        if !query_ok(review) || review.proposals.len() != 1
            || review.proposals[0].diagnostics.len() != 0 {
            fail(this, "component-instance proposal did not validate cleanly");
            return;
        }
        let committed = assembly_edit::commit_proposal(review.proposals[0].id);
        if !query_ok(committed) {
            fail(this, "component-instance proposal could not be committed: " + committed.error);
            return;
        }
        this.wait_generation = before.generation;
        this.phase = 1;
        return;
    }

    let vehicle = query("QueryUsdPrim", #{
        doc: this.doc,
        path: FLIP,
        children: true,
    });

    if this.phase == 1 {
        if before.generation <= this.wait_generation {
            if this.elapsed > 10.0 { fail(this, "component reference did not reach its committed generation"); }
            return;
        }
        if !query_ok(vehicle) || vehicle.children == () || vehicle.children.len() == 0 {
            if this.elapsed > 10.0 {
                fail(this, "referenced vehicle root never received its composed children");
            }
            return;
        }
        let variant_plan = assembly_builder::select_variants_plan(
            this.doc, "@root@", FLIP, variants(),
        );
        if !variant_plan.ok || variant_plan.kind != "variant_selection"
            || variant_plan.ops.len() != 4 {
            fail(this, "second-phase rover variant plan was not constructible");
            return;
        }
        let proposal = assembly_edit::propose(
            this.doc, #{ Assembly: () }, "Configure FLIP component variants",
            variant_plan.ops, before.generation,
        );
        if !query_ok(proposal) {
            fail(this, "variant-selection proposal was rejected: " + proposal.error);
            return;
        }
        let review = assembly_edit::review_session(this.doc);
        if !query_ok(review) || review.proposals.len() != 1
            || review.proposals[0].diagnostics.len() != 0 {
            fail(this, "variant-selection proposal did not validate cleanly");
            return;
        }
        let committed = assembly_edit::commit_proposal(review.proposals[0].id);
        if !query_ok(committed) {
            fail(this, "variant-selection proposal could not be committed: " + committed.error);
            return;
        }
        this.variant_ops = variant_plan.ops.len();
        this.wait_generation = before.generation;
        this.phase = 2;
        return;
    }

    if before.generation <= this.wait_generation {
        if this.elapsed > 10.0 { fail(this, "variant-selection commit did not reproject"); }
        return;
    }

    let root = query("QueryUsdPrim", #{
        doc: this.doc,
        path: ASSEMBLY,
        children: true,
    });
    vehicle = query("QueryUsdPrim", #{
        doc: this.doc,
        path: FLIP,
        children: true,
    });
    if !query_ok(vehicle) || vehicle.children == () || vehicle.children.len() == 0 {
        if this.elapsed > 10.0 {
            fail(this, "variant-selected vehicle lost its composed children");
        }
        return;
    }
    let wheels = [];
    for wheel_path in WHEELS {
        wheels.push(wheel_record(this.doc, wheel_path));
    }
    let duplicate_identity = assembly_builder::referenced_instance_plan(
        this.doc, "@root@", ASSEMBLY, "FLIP", "Xform", ASSET,
        (), (), (), (), (),
    );
    let wheel_contract = true;
    for i in 0..4 {
        let wheel = wheels[i];
        if !query_ok(wheel) {
            wheel_contract = false;
            continue;
        }
        if wheel.attrs == () {
            wheel_contract = false;
            continue;
        }
        if wheel.type_name != "Cylinder"
            || wheel.attrs == ()
            || wheel.attrs["physxVehicleWheelAttachment:index"] != i
            || (wheel.attrs["physxVehicleWheel:radius"] - 0.4).abs() > 0.0001
            || (wheel.attrs["physxVehicleWheel:width"] - 0.3).abs() > 0.0001
            || wheel.attrs["lunco:suspension:restLength"] == () {
            wheel_contract = false;
        }
    }

    this.done = true;
    report_verdict([
        t_true(query_ok(root) && root.children != () && array_has(root.children, FLIP),
            "empty authored assembly gained the requested FLIP instance"),
        t_true(query_ok(vehicle) && vehicle.type_name == "Xform",
            "referenced vehicle root remains a composed Xform"),
        t_true(this.variant_ops == 4,
            "variant choices are applied through a second reviewed typed plan"),
        t_true(wheel_contract,
            "all four composed wheel components expose type, index, dimensions, and suspension"),
        t_true(!duplicate_identity.ok,
            "duplicate component identity is rejected before a second proposal"),
    ], "ASSEMBLY COMPONENT BUILDER", "ASSEMBLY_COMPONENT_BUILDER");
}
