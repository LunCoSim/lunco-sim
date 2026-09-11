// TEST-ONLY. Builds generic authored component and reference plans into an
// empty assembly through the normal review/journal boundary.

const ROOT = "/AssemblyComponentBuilderTest";
const ASSEMBLY = ROOT + "/Assembly";
const BUNDLE = ASSEMBLY + "/PanelBundle";
const INSTANCE = ASSEMBLY + "/Ramp";
const ASSET = "lunco://props/ramp.usda";

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

fn on_start(me) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.wait_generation = -1;
    this.bundle_wait_generation = -1;
    this.bundle_checked = false;
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
        let parameter_plan = assembly_builder::parameter_plan(
            "root", BUNDLE,
            [
                #{ name: "primvars:width", type_name: "float", value: "1.2" },
                #{ name: "primvars:length", type_name: "float", value: "0.8" },
            ],
        );
        if !query_ok(parameter_plan) || parameter_plan.kind != "parameter_edit"
            || parameter_plan.ops == () || parameter_plan.ops.len() != 2 {
            fail(this, "generic parameter planner did not return two typed operations");
            return;
        }
        let duplicate_parameter = assembly_builder::parameter_plan(
            "root", BUNDLE,
            [
                #{ name: "primvars:width", type_name: "float", value: "1.2" },
                #{ name: "primvars:width", type_name: "float", value: "0.8" },
            ],
        );
        if query_ok(duplicate_parameter) {
            fail(this, "generic parameter planner accepted duplicate properties");
            return;
        }
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
        if prim_exists(this.doc, INSTANCE) {
            this.wait_generation = before.generation - 1;
            this.phase = 1;
            return;
        }
        let plan = assembly_builder::referenced_instance_plan(
            this.doc, "@root@", ASSEMBLY, "Ramp", "Xform", ASSET,
            [0.0, 1.0, 0.0], [0.0, 0.0, 0.0], (), (), (),
        );
        let missing_parent = assembly_builder::referenced_instance_plan(
            this.doc, "@root@", ASSEMBLY + "/Missing", "Bad", "Xform", ASSET,
            (), (), (), (), (),
        );
        let invalid_asset = assembly_builder::referenced_instance_plan(
            this.doc, "@root@", ASSEMBLY, "BadAsset", "Xform",
            "components/mobility/wheel.usda", (), (), (), (), (),
        );
        if !plan.ok || plan.kind != "referenced_instance" || plan.ops.len() != 3
            || missing_parent.ok || invalid_asset.ok {
            fail(this, "component-instance plan did not build or reject its explicit contract");
            return;
        }

        let proposal = assembly_edit::propose(
            this.doc, #{ Assembly: () }, "Build generic component instance",
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

    let instance = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: INSTANCE,
        children: true,
    });

    if this.phase == 1 {
        if before.generation <= this.wait_generation {
            if this.elapsed > 10.0 { fail(this, "component reference did not reach its committed generation"); }
            return;
        }
        if !query_ok(instance) || instance.children == () || instance.children.len() == 0 {
            if this.elapsed > 10.0 {
                fail(this, "referenced component root never received its composed children");
            }
            return;
        }
        let duplicate_identity = assembly_builder::referenced_instance_plan(
            this.doc, "@root@", ASSEMBLY, "Ramp", "Xform", ASSET,
            (), (), (), (), (),
        );
        if duplicate_identity.ok {
            fail(this, "duplicate component identity was not rejected");
            return;
        }
        let root = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ASSEMBLY,
            children: true,
        });

        this.done = true;
        this.checks.push(t_true(query_ok(root) && root.children != () && array_has(root.children, INSTANCE),
            "empty authored assembly gained the requested generic component"));
        this.checks.push(t_true(query_ok(instance) && instance.type_name == "Xform",
            "referenced component root remains a composed Xform"));
        this.checks.push(t_true(instance.children.len() > 0,
            "reference composition exposes the component children"));
        this.checks.push(t_true(!duplicate_identity.ok,
            "duplicate component identity is rejected before a second proposal"));
        report_verdict(this.checks, "ASSEMBLY COMPONENT BUILDER", "ASSEMBLY_COMPONENT_BUILDER");
    }
}
