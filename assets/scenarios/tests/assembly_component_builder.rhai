// TEST-ONLY. Builds a four-wheel rover reference into an empty authored
// assembly through the dynamic Rhai builder and the normal review/journal
// boundary, then checks the composed vehicle contract.

const ROOT = "/AssemblyComponentBuilderTest";
const ASSEMBLY = ROOT + "/Assembly";
const BUNDLE = ASSEMBLY + "/PanelBundle";
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
        doc_id: doc,
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
                doc_id: item.doc_id,
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
        doc_id: doc,
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
    this.bundle_wait_generation = -1;
    this.bundle_checked = false;
    this.variant_ops = 0;
    this.checks = [];
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
        if !this.bundle_checked {
            if !prim_exists(this.doc, BUNDLE) {
                let bundle = #{
                    units: "m",
                    dimensions: [1.2, 0.1, 0.8],
                    geometry: [
                        #{
                            name: "Panel",
                            kind: "Cube",
                            size: 1.0,
                            scale: [1.2, 0.05, 0.8],
                            render: true,
                            collision: true,
                            material: BUNDLE + "/Looks/Panel",
                        },
                        #{
                            name: "ActuatorBody",
                            kind: "Cylinder",
                            radius: 0.1,
                            height: 0.3,
                            axis: "Y",
                            translation: [0.0, 0.2, 0.0],
                            render: true,
                            collision: false,
                        },
                    ],
                    mass: #{
                        value: 2.0,
                        center_of_mass: [0.0, 0.0, 0.0],
                        diagonal_inertia: [0.4, 0.4, 0.4],
                    },
                    frames: [
                        #{
                            name: "Mount",
                            role: "attachment",
                            mount_kind: "panel",
                            translation: [0.0, 0.0, 0.0],
                        },
                        #{
                            name: "DeployEnd",
                            role: "actuator",
                            translation: [0.0, 0.2, 0.0],
                        },
                    ],
                    actuators: [#{
                        name: "deployment",
                        direction: "input",
                        unit: "rad",
                        frame: "DeployEnd",
                        default_value: 0.0,
                        limits: [0.0, 1.57],
                    }],
                    deployment: #{ state: "stowed", units: "rad", limits: [0.0, 1.57] },
                    spawnable: false,
                };
                let facts = assembly_builder::component_bundle_facts(bundle);
                let invalid_units = assembly_builder::component_bundle_facts(#{
                    units: "mm", dimensions: [1.0, 1.0, 1.0], geometry: [],
                });
                let duplicate_names = assembly_builder::component_bundle_facts(#{
                    units: "m", dimensions: [1.0, 1.0, 1.0], geometry: [
                        #{ name: "Duplicate", kind: "Cube", size: 1.0 },
                        #{ name: "Duplicate", kind: "Cube", size: 1.0 },
                    ],
                });
                let plan = assembly_builder::component_bundle_plan(
                    "@root@", ASSEMBLY, "PanelBundle", bundle,
                );
                if !facts.ok || facts.visual.len() != 2 || facts.collision.len() != 1
                    || facts.frames.len() != 2 || facts.actuators.len() != 1
                    || facts.mount.mount_kind != "panel" || facts.mass.value != 2.0
                    || facts.deployment.state != "stowed" || !plan.ok
                    || plan.kind != "component_bundle" || plan.ops.len() == 0
                    || invalid_units.ok || duplicate_names.ok {
                    fail(this, "generic component bundle facts did not validate its contract");
                    return;
                }
                let proposal = assembly_edit::propose(
                    this.doc, #{ Assembly: () }, "Build generic panel component bundle",
                    plan.ops, before.generation,
                );
                if !query_ok(proposal) {
                    fail(this, "generic component bundle proposal was rejected: " + proposal.error);
                    return;
                }
                let review = assembly_edit::review_session(this.doc);
                if !query_ok(review) || review.proposals.len() != 1
                    || review.proposals[0].diagnostics.len() != 0 {
                    fail(this, "generic component bundle proposal did not validate cleanly");
                    return;
                }
                let committed = assembly_edit::commit_proposal(review.proposals[0].id);
                if !query_ok(committed) {
                    fail(this, "generic component bundle proposal could not be committed: " + committed.error);
                    return;
                }
                this.bundle_wait_generation = before.generation;
                return;
            }
            if this.bundle_wait_generation >= 0 && before.generation <= this.bundle_wait_generation {
                if this.elapsed > 10.0 { fail(this, "generic component bundle did not reach its committed generation"); }
                return;
            }
            let bundle_root = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: BUNDLE,
                attrs: [
                    "lunco:component:units",
                    "lunco:component:dimensions",
                    "physics:mass",
                    "lunco:mount:plug",
                    "lunco:deployment:state",
                    "lunco:deployment:limits",
                    "inputs:deployment",
                ],
                rels: [
                    "lunco:mount:frame",
                    "lunco:component:frames",
                    "lunco:component:actuatorFrames",
                ],
                children: true,
            });
            let visual = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: BUNDLE + "/Panel",
                attrs: ["size", "physics:collisionEnabled"],
                rels: ["material:binding"],
            });
            let actuator = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: BUNDLE + "/DeployEnd",
                attrs: [
                    "lunco:component:frameRole",
                    "lunco:component:actuatorName",
                    "lunco:component:actuatorDirection",
                    "lunco:component:actuatorUnit",
                    "lunco:component:actuatorLimits",
                ],
                rels: ["lunco:component:port"],
            });
            let mount = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: BUNDLE + "/Mount",
                attrs: ["lunco:component:frameRole"],
            });
            this.checks.push(t_true(query_ok(bundle_root)
                    && bundle_root.attrs["lunco:component:units"] == "m"
                    && bundle_root.attrs["physics:mass"] == 2.0
                    && bundle_root.attrs["lunco:mount:plug"] == "panel"
                    && bundle_root.attrs["lunco:deployment:state"] == "stowed"
                    && bundle_root.relationships["lunco:mount:frame"][0] == BUNDLE + "/Mount"
                    && bundle_root.relationships["lunco:component:frames"].len() == 2
                    && bundle_root.relationships["lunco:component:actuatorFrames"].len() == 1,
                "component bundle root exposes units, dimensions, mass, mount, deployment, and frame relationships"));
            this.checks.push(t_true(query_ok(visual)
                    && visual.attrs["size"] == 1.0
                    && visual.attrs["physics:collisionEnabled"] == true
                    && visual.relationships["material:binding"][0] == BUNDLE + "/Looks/Panel",
                "component bundle geometry preserves visual, collision, and material-binding roles"));
            this.checks.push(t_true(query_ok(mount)
                    && mount.attrs["lunco:component:frameRole"] == "attachment"
                    && query_ok(actuator)
                    && actuator.attrs["lunco:component:frameRole"] == "actuator"
                    && actuator.attrs["lunco:component:actuatorName"] == "deployment"
                    && actuator.attrs["lunco:component:actuatorLimits"] != (),
                "component bundle exposes explicit attachment and actuator endpoint frames"));
            this.bundle_checked = true;
        }
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
        doc_id: this.doc,
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
        doc_id: this.doc,
        path: ASSEMBLY,
        children: true,
    });
    vehicle = query("QueryUsdPrim", #{
        doc_id: this.doc,
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
    this.checks.push(t_true(query_ok(root) && root.children != () && array_has(root.children, FLIP),
        "empty authored assembly gained the requested FLIP instance"));
    this.checks.push(t_true(query_ok(vehicle) && vehicle.type_name == "Xform",
        "referenced vehicle root remains a composed Xform"));
    this.checks.push(t_true(this.variant_ops == 4,
        "variant choices are applied through a second reviewed typed plan"));
    this.checks.push(t_true(wheel_contract,
        "all four composed wheel components expose type, index, dimensions, and suspension"));
    this.checks.push(t_true(!duplicate_identity.ok,
        "duplicate component identity is rejected before a second proposal"));
    report_verdict(this.checks, "ASSEMBLY COMPONENT BUILDER", "ASSEMBLY_COMPONENT_BUILDER");
}
