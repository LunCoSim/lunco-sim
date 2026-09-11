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

fn close_scalar(actual, expected) {
    actual != () && (actual - expected).abs() <= 0.0001
}

fn close_vector(actual, expected) {
    if actual == () || actual.len() != expected.len() { return false; }
    for i in 0..expected.len() {
        if !close_scalar(actual[i], expected[i]) { return false; }
    }
    true
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
    this.bundle_update_started = false;
    this.bundle_update_wait_generation = -1;
    this.bundle_update_checked = false;
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
                this.checks.push(t_true(plan.frame_paths == [BUNDLE + "/Mount", BUNDLE + "/DeployEnd"]
                        && plan.input_ports == ["inputs:deployment"],
                    "component bundle returns exact frame paths and standard input ports"));
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
                    "physics:mass",
                    "lunco:mount:plug",
                    "inputs:deployment",
                ],
                rels: [
                    "lunco:mount:frame",
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
                attrs: ["xformOp:translate", "xformOpOrder"],
            });
            let mount = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: BUNDLE + "/Mount",
                attrs: ["xformOp:translate", "xformOpOrder"],
            });
            let frame_catalog = assembly_builder::functional_frame_catalog(
                this.doc, "@root@", BUNDLE,
            );
            let invalid_frame_catalog = assembly_builder::functional_frame_catalog(
                this.doc, "@root@", BUNDLE + "/Missing",
            );
            let context = assembly_builder::authoring_context(
                this.doc, BUNDLE, "@root@",
            );
            let invalid_dispatch = assembly_builder::place_or_attach_plan(
                this.doc, "@root@", #{ mode: "unsupported" },
            );
            let invalid_component_plan = assembly_builder::place_or_attach_plan(
                this.doc, "@root@", #{
                    mode: "attach_component",
                    host_path: ASSEMBLY,
                    socket_kind: "panel",
                    joint_kind: "fixed",
                    asset: ASSET,
                    name: "MissingPanel",
                    joint_name: "MissingPanelJoint",
                },
            );
            this.checks.push(t_true(query_ok(bundle_root)
                    && bundle_root.attrs["physics:mass"] == 2.0
                    && bundle_root.attrs["lunco:mount:plug"] == "panel"
                    && bundle_root.attrs["inputs:deployment"] == 0.0
                    && bundle_root.relationships["lunco:mount:frame"][0] == BUNDLE + "/Mount",
                "component bundle uses standard mass, mount, transform, and input contracts"));
            this.checks.push(t_true(query_ok(visual)
                    && visual.attrs["size"] == 1.0
                    && visual.attrs["physics:collisionEnabled"] == true
                    && visual.relationships["material:binding"][0] == BUNDLE + "/Looks/Panel",
                "component bundle geometry preserves visual, collision, and material-binding roles"));
            this.checks.push(t_true(query_ok(mount)
                    && query_ok(actuator)
                    && actuator.type_name == "Xform"
                    && actuator.attrs["xformOpOrder"].len() > 0,
                "component bundle exposes explicit mount and actuator frame paths without custom metadata"));
            this.checks.push(t_true(query_ok(context)
                    && context.kind == "authoring_context"
                    && context.path == BUNDLE
                    && context.parent_path == ASSEMBLY
                    && context.edit_target == "@root@"
                    && context.generation == before.generation
                    && context.prim.type_name == "Xform"
                    && context.sockets.len() == 0
                    && context.plug != ()
                    && context.actions.contains("inspect")
                    && context.actions.contains("transform")
                    && !context.actions.contains("plan_component_attachment")
                    && !context.actions.contains("plan_existing_mount_realignment"),
                "authoring context exposes exact identity, generation, and available actions"));
            this.checks.push(t_true(query_ok(frame_catalog)
                    && frame_catalog.kind == "functional_frame_catalog"
                    && frame_catalog.edit_target == "@root@"
                    && frame_catalog.root_path == BUNDLE
                    && frame_catalog.generation == before.generation
                    && frame_catalog.frame_paths == [BUNDLE + "/Mount"]
                    && frame_catalog.socket_paths.len() == 0
                    && frame_catalog.frames.len() == 1
                    && frame_catalog.frames[0].role == "attachment"
                    && frame_catalog.frames[0].is_mount_frame
                    && !frame_catalog.frames[0].is_actuator_frame,
                "functional frame catalog reads the registered mount schema without custom frame metadata"));
            this.checks.push(t_true(!invalid_frame_catalog.ok,
                "functional frame catalog rejects an unavailable exact root"));
            this.checks.push(t_true(!invalid_dispatch.ok && !invalid_component_plan.ok,
                "unified placement/attachment planner rejects unsupported modes and missing sockets"));
            this.bundle_checked = true;
        }
        if this.bundle_checked && !this.bundle_update_started {
            let updated = #{
                units: "m",
                dimensions: [1.6, 0.12, 0.8],
                geometry: [
                    #{
                        name: "Panel",
                        kind: "Cube",
                        size: 1.0,
                        scale: [1.6, 0.06, 0.8],
                        render: true,
                        collision: true,
                        material: BUNDLE + "/Looks/Panel",
                    },
                    #{
                        name: "ActuatorBody",
                        kind: "Cylinder",
                        radius: 0.1,
                        height: 0.35,
                        axis: "Y",
                        translation: [0.0, 0.22, 0.0],
                        render: true,
                        collision: false,
                    },
                ],
                mass: #{
                    value: 2.2,
                    center_of_mass: [0.0, 0.01, 0.0],
                    diagonal_inertia: [0.45, 0.4, 0.45],
                },
                frames: [
                    #{
                        name: "Mount",
                        role: "attachment",
                        mount_kind: "panel",
                        translation: [0.1, 0.0, 0.0],
                    },
                    #{
                        name: "DeployEnd",
                        role: "actuator",
                        translation: [0.0, 0.22, 0.0],
                    },
                ],
                actuators: [#{
                    name: "deployment",
                    direction: "input",
                    unit: "rad",
                    frame: "DeployEnd",
                    default_value: 1.0,
                    limits: [0.0, 1.57],
                }],
                deployment: #{ state: "deployed", units: "rad", limits: [0.0, 1.57] },
                spawnable: false,
            };
            this.updated_bundle = updated;
            let bindings = [#{
                path: BUNDLE,
                name: "inputs:surface_area",
                type_name: "double",
                value: 1.6,
                expected: 1.6,
            }];
            let update = assembly_builder::component_bundle_update_plan(
                this.doc, "@root@", BUNDLE, updated, bindings,
            );
            let invalid_update = updated;
            invalid_update.geometry[0].kind = "Cylinder";
            let invalid = assembly_builder::component_bundle_update_plan(
                this.doc, "@root@", BUNDLE, invalid_update, bindings,
            );
            invalid_update.geometry[0].kind = "Cube";
            if !query_ok(update) || update.kind != "component_bundle_update"
                || update.no_op || update.ops.len() < 10 || query_ok(invalid) {
                fail(this, "generic component regeneration did not preserve topology and produce owned updates");
                return;
            }
            let proposal = assembly_edit::propose(
                this.doc, #{ Assembly: () }, "Regenerate generic panel component",
                update.ops, before.generation,
            );
            if !query_ok(proposal) {
                fail(this, "generic component regeneration proposal was rejected: " + proposal.error);
                return;
            }
            let review = assembly_edit::review_session(this.doc);
            if !query_ok(review) || review.proposals.len() != 1
                || review.proposals[0].diagnostics.len() != 0 {
                fail(this, "generic component regeneration proposal did not validate cleanly");
                return;
            }
            let committed = assembly_edit::commit_proposal(review.proposals[0].id);
            if !query_ok(committed) {
                fail(this, "generic component regeneration could not be committed: " + committed.error);
                return;
            }
            this.bundle_update_wait_generation = before.generation;
            this.bundle_update_started = true;
            return;
        }
        if this.bundle_update_started && !this.bundle_update_checked {
            if before.generation <= this.bundle_update_wait_generation {
                if this.elapsed > 10.0 { fail(this, "generic component regeneration did not reach its committed generation"); }
                return;
            }
            let bindings = [#{
                path: BUNDLE,
                name: "inputs:surface_area",
                type_name: "double",
                value: 1.6,
                expected: 1.6,
            }];
            let root = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: BUNDLE,
                attrs: ["physics:mass", "physics:centerOfMass",
                    "physics:diagonalInertia", "inputs:deployment", "inputs:surface_area"],
            });
            let panel = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: BUNDLE + "/Panel",
                attrs: ["xformOp:scale", "physics:collisionEnabled"],
                rels: ["material:binding"],
            });
            let mount = query("QueryUsdPrim", #{
                doc_id: this.doc,
                path: BUNDLE + "/Mount",
                attrs: ["xformOp:translate"],
            });
            if !query_ok(root)
                || !close_scalar(root.attrs["physics:mass"], 2.2)
                || !close_vector(root.attrs["physics:centerOfMass"], [0.0, 0.01, 0.0])
                || !close_vector(root.attrs["physics:diagonalInertia"], [0.45, 0.4, 0.45])
                || !close_scalar(root.attrs["inputs:deployment"], 1.0)
                || root.attrs["inputs:surface_area"] != 1.6
                || !query_ok(panel) || !close_vector(panel.attrs["xformOp:scale"], [1.6, 0.06, 0.8])
                || !query_ok(mount) || !close_vector(mount.attrs["xformOp:translate"], [0.1, 0.0, 0.0]) {
                if this.elapsed > 10.0 {
                    fail(this, "generic component regeneration did not reproject its owned values");
                }
                return;
            }
            let same = assembly_builder::component_bundle_update_plan(
                this.doc, "@root@", BUNDLE, this.updated_bundle, bindings,
            );
            let standard_manifest = #{
                path: BUNDLE,
                expected_type: "Xform",
                mass: #{
                    value: 2.2,
                    center_of_mass: [0.0, 0.01, 0.0],
                    diagonal_inertia: [0.45, 0.4, 0.45],
                },
                parts: [
                    #{
                        path: BUNDLE + "/Panel",
                        type_name: "Cube",
                        size: 1.0,
                        collision_enabled: true,
                        material: BUNDLE + "/Looks/Panel",
                    },
                    #{
                        path: BUNDLE + "/ActuatorBody",
                        type_name: "Cylinder",
                        radius: 0.1,
                        height: 0.35,
                    },
                ],
            };
            let invalid_standard = #{
                path: BUNDLE,
                parts: [#{ path: BUNDLE + "/Panel", type_name: "Cylinder" }],
            };
            let compliance = assembly_audit::standard_component_report(
                this.doc, standard_manifest,
            );
            let invalid_compliance = assembly_audit::standard_component_report(
                this.doc, invalid_standard,
            );
            this.checks.push(t_true(query_ok(same) && same.no_op && same.ops.len() == 0,
                "unchanged component regeneration is a true no-op"));
            this.checks.push(t_true(query_ok(compliance) && compliance.ok
                    && !query_ok(invalid_compliance),
                "standard USD compliance report accepts direct edits and rejects type drift"));
            this.checks.push(t_true(query_ok(root)
                    && close_scalar(root.attrs["physics:mass"], 2.2)
                    && close_vector(root.attrs["physics:centerOfMass"], [0.0, 0.01, 0.0])
                    && close_vector(root.attrs["physics:diagonalInertia"], [0.45, 0.4, 0.45])
                    && close_scalar(root.attrs["inputs:deployment"], 1.0)
                    && root.attrs["inputs:surface_area"] == 1.6,
                "regeneration updates standard mass properties and explicit inputs"));
            this.checks.push(t_true(query_ok(panel)
                    && close_vector(panel.attrs["xformOp:scale"], [1.6, 0.06, 0.8])
                    && panel.attrs["physics:collisionEnabled"] == true
                    && panel.relationships["material:binding"][0] == BUNDLE + "/Looks/Panel",
                "regeneration updates owned shape geometry while preserving collision and material"));
            this.checks.push(t_true(query_ok(mount)
                    && close_vector(mount.attrs["xformOp:translate"], [0.1, 0.0, 0.0]),
                "regeneration updates the named frame placement"));
            this.bundle_update_checked = true;
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
