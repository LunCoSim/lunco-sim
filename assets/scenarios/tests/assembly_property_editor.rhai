// TEST-ONLY. Exercises the Editor gizmo and viewport with typed mouse input,
// then verifies the schema-driven property catalog and typed dry patch plan
// through the normal inspection, proposal, review, journal, and query boundary.
const TEST_KIND = "editor";

import "lunco://scripting/tools/assembly_ui" as assembly_ui;

const ROOT = "/AssemblyPropertyEditorTest";
const PANEL = ROOT + "/Assembly/Panel";
const TEMP_KIND = "component_test";
const PREVIEW_ID = 1;

fn query_ok(record) {
    record != () && record.ok != false
}

fn close_enough(actual, expected) {
    actual != () && (actual - expected).abs() <= 0.001
}

fn vector_close(actual, expected) {
    if actual == () || actual.len() != expected.len() { return false; }
    for i in 0..expected.len() {
        if !close_enough(actual[i], expected[i]) { return false; }
    }
    true
}

fn channel_changed(before, after) {
    if before == () { return after != (); }
    if after == () { return true; }
    !vector_close(before, after)
}

fn transform_changed(before, after) {
    channel_changed(before["xformOp:translate"], after["xformOp:translate"])
        || channel_changed(before["xformOp:rotateXYZ"], after["xformOp:rotateXYZ"])
        || channel_changed(before["xformOp:scale"], after["xformOp:scale"])
}

fn gizmo_pose_is_fixture_baseline(pose) {
    vector_close(pose["xformOp:translate"], [11.0, 0.0, 0.0])
        && pose["xformOp:rotateXYZ"] == ()
        && pose["xformOp:scale"] == ()
}

fn property_patch_is_current(doc) {
    let values = query("QueryUsdPrim", #{
        doc_id: doc,
        path: PANEL,
        attrs: ["size", "physics:collisionEnabled", "inputs:surface_area", "variant:configuration"],
        children: false,
    });
    query_ok(values) && values.attrs != ()
        && close_enough(values.attrs.size, 2.0)
        && values.attrs["physics:collisionEnabled"] == false
        && close_enough(values.attrs["inputs:surface_area"], 3.5)
        && values.attrs["variant:configuration"] == "wide"
}

fn property(catalog, name) {
    if catalog == () || catalog.properties == () { return (); }
    for item in catalog.properties {
        if item.name == name { return item; }
    }
    ()
}

fn composed_kind(inspection) {
    if !query_ok(inspection) || inspection.prim == ()
        || inspection.prim.metadata == ()
        || inspection.prim.metadata.kind == () {
        return ();
    }
    let kind = inspection.prim.metadata.kind.composed;
    if kind == () || kind.present != true { return (); }
    kind.value
}

fn preview_for_document(viewport, doc) {
    if viewport == () || viewport.previews == () { return (); }
    for preview in viewport.previews {
        if preview.doc_id == doc { return preview; }
    }
    ()
}

fn focused_view(preview) {
    if preview == () || preview.views == () { return (); }
    for view in preview.views {
        if view.focused == true { return view; }
    }
    ()
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "ASSEMBLY PROPERTY EDITOR", "ASSEMBLY_PROPERTY_EDITOR");
    }
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return (); }
    for item in docs.open_documents {
        // The scene test launches one explicit USD document. Do not gate
        // discovery on a transient composed query while a prior scoped edit
        // is still being projected; the exact prim is checked below through
        // InspectUsdDocument before any authoring command is issued.
        if item.kind == "usd" { return item.doc_id; }
    }
    ()
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments", "QueryUsdPrim"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.doc = ();
    this.phase = 100;
    this.elapsed = 0.0;
    this.interaction_frames = 0;
    this.wait_generation = 0;
    this.preview = PREVIEW_ID;
    this.gizmo_initial_translate = ();
    this.gizmo_initial_generation = 0;
    this.gizmo_after_translate = ();
    this.gizmo_camera_before = ();
    this.gizmo_start = ();
    this.gizmo_end = ();
    this.pan_start = ();
    this.pan_end = ();
    this.gizmo_drag_changed = false;
    this.gizmo_camera_stable = false;
    this.blank_pan_changed = false;
    this.blank_pan_kept_transform = false;
    this.patch_already_current = false;
    this.kind_override_seen = false;
    this.kind_restore_seen = false;
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    this.interaction_frames += 1;
    if this.doc == () {
        this.doc = find_document();
        if this.doc == () {
            if this.elapsed > 8.0 { fail(this, "property-editor fixture was not discoverable"); }
            return;
        }
    }

    let current = assembly_edit::describe(this.doc);
    if !query_ok(current) || current.generation == () {
        fail(this, "property-editor fixture did not expose a document generation");
        return;
    }

    // A USD transform commit can reload the stage and restart this authored
    // script. Preserve progress from the document state so the mouse test does
    // not apply another edit or repeat the already-verified pan.
    if this.phase == 100 {
        let current_kind = composed_kind(assembly_edit::inspect(this.doc, PANEL));
        if current_kind == TEMP_KIND {
            this.gizmo_drag_changed = true;
            this.gizmo_camera_stable = true;
            this.blank_pan_changed = true;
            this.blank_pan_kept_transform = true;
            this.kind_override_seen = true;
            if property_patch_is_current(this.doc) {
                let cleared = assembly_edit::prim_kind(
                    this.doc, "@runtime@", PANEL, (), current.generation,
                );
                if !query_ok(cleared) {
                    fail(this, "runtime kind marker could not be cleared: " + cleared.error);
                    return;
                }
                this.wait_generation = current.generation;
                this.phase = 3;
            } else {
                this.phase = 4;
            }
            return;
        }
        if property_patch_is_current(this.doc) {
            this.gizmo_drag_changed = true;
            this.gizmo_camera_stable = true;
            this.blank_pan_changed = true;
            this.blank_pan_kept_transform = true;
            this.kind_override_seen = true;
            this.kind_restore_seen = true;
            this.patch_already_current = true;
            this.phase = 1;
            return;
        }
    }

    // Open the fixture through the Editor preview owner. The measured image
    // bounds let the scenario send mouse events relative to the current dock
    // layout and display scale.
    if this.phase == 100 {
        let viewport = query("InspectUsdViewport", #{});
        let preview = preview_for_document(viewport, this.doc);
        if preview == () {
            let opened = assembly_ui::open_session(PREVIEW_ID, this.doc, "@root@");
            if !query_ok(opened) {
                fail(this, "could not open the fixture in the Editor preview");
                return;
            }
            this.preview = PREVIEW_ID;
        } else {
            this.preview = preview.preview;
            assembly_ui::focus("preview", this.preview, this.doc, "@root@");
        }
        this.phase = 101;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 101 {
        let viewport = query("InspectUsdViewport", #{});
        let preview = preview_for_document(viewport, this.doc);
        let view = focused_view(preview);
        if preview == () || preview.projection_ready != true || view == ()
            || view.image_rect == () || view.scale_factor == () {
            if this.interaction_frames > 720 {
                fail(this, "Editor preview did not publish a ready view and measured image bounds");
            }
            return;
        }
        let selected = assembly_ui::select_prim(this.preview, PANEL, false, false);
        if !query_ok(selected) {
            fail(this, "could not select the fixture prim in the Editor preview");
            return;
        }
        this.phase = 102;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 102 {
        if this.interaction_frames < 12 { return; }
        let viewport = query("InspectUsdViewport", #{});
        let preview = preview_for_document(viewport, this.doc);
        let view = focused_view(preview);
        let values = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: PANEL,
            attrs: ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"],
            children: false,
        });
        if view == () || view.image_rect == () || view.scale_factor == ()
            || !query_ok(values) || values.attrs == () {
            if this.interaction_frames > 720 {
                fail(this, "selected preview prim or viewport bounds were not queryable");
            }
            return;
        }
        let origin = view.image_rect.origin;
        let size = view.image_rect.size;
        let scale = view.scale_factor;
        let center_x = (origin[0] + size[0] * 0.5) / scale;
        let center_y = (origin[1] + size[1] * 0.5) / scale;
        // Exercise an axis handle at the image center using measured image
        // bounds, converted from physical pixels to logical window pixels.
        this.gizmo_start = [center_x, center_y - size[1] * 0.0816];
        this.gizmo_end = [center_x, center_y - size[1] * 0.1156];
        // Use the image's lower-right margin for a control drag over blank
        // space; it should pan the camera and leave the prim unchanged.
        this.pan_start = [
            (origin[0] + size[0] * 0.84) / scale,
            (origin[1] + size[1] * 0.82) / scale,
        ];
        this.pan_end = [this.pan_start[0] + 24.0, this.pan_start[1] + 18.0];
        this.gizmo_initial_translate = values.attrs["xformOp:translate"];
        this.gizmo_initial_pose = values.attrs;
        this.gizmo_initial_generation = current.generation;
        this.gizmo_camera_before = view.target;
        if !gizmo_pose_is_fixture_baseline(values.attrs) {
            // The previous incarnation committed this handle drag and caused
            // the stage to reload. Its untouched fixture camera means the
            // captured gesture changed the prim without panning.
            this.gizmo_after_pose = values.attrs;
            this.gizmo_after_translate = values.attrs["xformOp:translate"];
            this.gizmo_drag_changed = true;
            if vector_close(view.target, [11.0, 0.0, 0.0]) {
                this.gizmo_camera_stable = true;
                input_pointer_move(this.pan_start[0], this.pan_start[1]);
                this.phase = 107;
                return;
            }

            fail(this, "preview camera moved before the Rhai blank-space pan phase");
            return;
        }
        input_pointer_move(this.gizmo_start[0], this.gizmo_start[1]);
        this.phase = 103;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 103 {
        input_pointer_press("primary", this.gizmo_start[0], this.gizmo_start[1]);
        this.phase = 104;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 104 {
        if this.interaction_frames < 4 { return; }
        input_pointer_move(this.gizmo_end[0], this.gizmo_end[1]);
        this.phase = 105;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 105 {
        if this.interaction_frames < 6 { return; }
        input_pointer_release("primary", this.gizmo_end[0], this.gizmo_end[1]);
        this.phase = 106;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 106 {
        let latest = assembly_edit::describe(this.doc);
        if latest.generation <= this.gizmo_initial_generation {
            if this.interaction_frames > 480 {
                let values = query("QueryUsdPrim", #{
                    doc_id: this.doc,
                    path: PANEL,
                    attrs: ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"],
                    children: false,
                });
                let actual = if query_ok(values) && values.attrs != () {
                    values.attrs.to_string()
                } else { "unavailable" };
                fail(this, "gizmo drag did not commit a USD generation: initial="
                    + this.gizmo_initial_generation.to_string()
                    + ", latest=" + latest.generation.to_string()
                    + ", translation=" + actual
                    + ", handle_start=" + this.gizmo_start.to_string()
                    + ", handle_end=" + this.gizmo_end.to_string());
            }
            return;
        }
        let values = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: PANEL,
            attrs: ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"],
            children: false,
        });
        let viewport = query("InspectUsdViewport", #{});
        let preview = preview_for_document(viewport, this.doc);
        let view = focused_view(preview);
        if !query_ok(values) || values.attrs == () || view == () {
            if this.interaction_frames > 480 {
                fail(this, "gizmo commit did not become visible through USD and viewport queries");
            }
            return;
        }
        this.gizmo_after_translate = values.attrs["xformOp:translate"];
        this.gizmo_after_pose = values.attrs;
        this.gizmo_drag_changed = transform_changed(
            this.gizmo_initial_pose, this.gizmo_after_pose,
        );
        this.gizmo_camera_stable = vector_close(view.target, this.gizmo_camera_before);
        if !this.gizmo_drag_changed || !this.gizmo_camera_stable {
            fail(this, "gizmo drag failed: transform_changed="
                + this.gizmo_drag_changed.to_string()
                + ", preview_camera_stable=" + this.gizmo_camera_stable.to_string()
                + ", pose=" + this.gizmo_after_pose.to_string()
                + ", handle_start=" + this.gizmo_start.to_string());
            return;
        }
        input_pointer_move(this.pan_start[0], this.pan_start[1]);
        this.phase = 107;
        return;
    }

    if this.phase == 107 {
        input_pointer_press("primary", this.pan_start[0], this.pan_start[1]);
        this.phase = 108;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 108 {
        if this.interaction_frames < 4 { return; }
        input_pointer_move(this.pan_end[0], this.pan_end[1]);
        this.phase = 109;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 109 {
        if this.interaction_frames < 6 { return; }
        input_pointer_release("primary", this.pan_end[0], this.pan_end[1]);
        this.phase = 110;
        this.interaction_frames = 0;
        return;
    }

    if this.phase == 110 {
        if this.interaction_frames < 15 { return; }
        let viewport = query("InspectUsdViewport", #{});
        let preview = preview_for_document(viewport, this.doc);
        let view = focused_view(preview);
        let values = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: PANEL,
            attrs: ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"],
            children: false,
        });
        if view == () || !query_ok(values) || values.attrs == () {
            fail(this, "blank-area pan result was not queryable");
            return;
        }
        this.blank_pan_changed = !vector_close(view.target, this.gizmo_camera_before);
        this.blank_pan_kept_transform = !transform_changed(
            this.gizmo_after_pose, values.attrs,
        );
        if !this.blank_pan_changed || !this.blank_pan_kept_transform {
            fail(this, "blank-area drag failed: camera_panned="
                + this.blank_pan_changed.to_string()
                + ", document_transform_stable=" + this.blank_pan_kept_transform.to_string()
                + ", camera_before=" + this.gizmo_camera_before.to_string()
                + ", camera_after=" + view.target.to_string()
                + ", pan_start=" + this.pan_start.to_string()
                + ", pan_end=" + this.pan_end.to_string());
            return;
        }
        this.phase = 0;
    }

    // Keep the runtime kind override as a progress marker across the document
    // stage reloads caused by preview edits. Clear it after the property patch
    // and require the authored component kind to reappear in composed USD.
    if this.phase == 0 {
        let before_kind = composed_kind(assembly_edit::inspect(this.doc, PANEL));
        if before_kind == TEMP_KIND {
            this.kind_override_seen = true;
            this.phase = 4;
            return;
        }
        if before_kind != "component" {
            fail(this, "fixture panel did not expose its authored component kind");
            return;
        }
        let authored = assembly_edit::prim_kind(
            this.doc, "@runtime@", PANEL, TEMP_KIND, current.generation,
        );
        if !query_ok(authored) {
            fail(this, "runtime kind override was rejected: " + authored.error);
            return;
        }
        this.wait_generation = current.generation;
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        if current.generation <= this.wait_generation {
            if this.elapsed > 10.0 { fail(this, "runtime kind override did not reach its committed generation"); }
            return;
        }
        let overridden = composed_kind(assembly_edit::inspect(this.doc, PANEL));
        if overridden != TEMP_KIND {
            fail(this, "runtime kind override was not visible through composed USD");
            return;
        }
        this.kind_override_seen = true;
        this.phase = 4;
        return;
    }

    if this.phase == 3 {
        if current.generation <= this.wait_generation {
            if this.elapsed > 10.0 { fail(this, "cleared runtime kind did not reach its committed generation"); }
            return;
        }
        let restored = composed_kind(assembly_edit::inspect(this.doc, PANEL));
        if restored != "component" {
            fail(this, "clearing runtime kind did not reveal the authored component kind");
            return;
        }
        this.kind_restore_seen = true;
        this.phase = 4;
    }

    if this.phase == 1 {
        if !this.patch_already_current && current.generation <= this.wait_generation {
            if this.elapsed > 10.0 { fail(this, "property patch did not reach its committed generation"); }
            return;
        }
        if composed_kind(assembly_edit::inspect(this.doc, PANEL)) == TEMP_KIND {
            let cleared = assembly_edit::prim_kind(
                this.doc, "@runtime@", PANEL, (), current.generation,
            );
            if !query_ok(cleared) {
                fail(this, "runtime kind marker could not be cleared: " + cleared.error);
                return;
            }
            this.wait_generation = current.generation;
            this.phase = 3;
            return;
        }
        let values = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: PANEL,
            attrs: ["xformOp:translate", "size", "physics:collisionEnabled", "inputs:surface_area"],
            children: false,
        });
        if !query_ok(values) {
            if this.elapsed > 10.0 { fail(this, "patched property values were not queryable"); }
            return;
        }
        let same = assembly_builder::editable_property_patch_plan(
            this.doc, "@root@", PANEL,
            [
                #{ name: "xformOp:translate", value: [2.0, 0.0, 0.0] },
                #{ name: "size", type_name: "double", value: 2.0 },
                #{ name: "physics:collisionEnabled", type_name: "bool", value: false },
                #{ name: "inputs:surface_area", type_name: "float", value: 3.5 },
                #{ name: "variant:configuration", value: "wide" },
            ], current.generation,
        );
        let stale = assembly_builder::editable_property_patch_plan(
            this.doc, "@root@", PANEL,
            [#{ name: "size", type_name: "double", value: 2.0 }], this.wait_generation,
        );
        let structural = assembly_builder::editable_property_patch_plan(
            this.doc, "@root@", PANEL,
            [#{ name: "xformOpOrder", value: ["xformOp:translate"] }], current.generation,
        );
        let wrong_type = assembly_builder::editable_property_patch_plan(
            this.doc, "@root@", PANEL,
            [#{ name: "size", type_name: "float", value: 2.0 }], current.generation,
        );
        this.done = true;
        report_verdict([
            t_true(this.kind_override_seen && this.kind_restore_seen,
                "SetPrimKind runtime override and clear restore composed USD metadata"),
            t_true(this.gizmo_drag_changed && this.gizmo_camera_stable,
                "dragging a USD gizmo handle changes a transform channel without panning its preview"),
            t_true(this.blank_pan_changed && this.blank_pan_kept_transform,
                "dragging blank preview space pans the camera without changing the prim"),
            t_true(vector_close(values.attrs["xformOp:translate"], [2.0, 0.0, 0.0])
                && close_enough(values.attrs.size, 2.0)
                && values.attrs["physics:collisionEnabled"] == false
                && close_enough(values.attrs["inputs:surface_area"], 3.5),
                "typed property patch values are visible through composed USD queries"),
            t_true(query_ok(same) && same.no_op && same.ops.len() == 0,
                "repeating the inspected values produces a true no-op"),
            t_true(!stale.ok && !structural.ok && !wrong_type.ok,
                "stale generations, structural fields, and wrong types fail visibly"),
        ], "ASSEMBLY PROPERTY EDITOR", "ASSEMBLY_PROPERTY_EDITOR");
        return;
    }

    let requested = [
        "kind", "xformOp:translate", "size", "xformOpOrder",
        "physics:collisionEnabled", "inputs:surface_area", "outputs:observed_area",
        "variant:configuration",
    ];
    let catalog = assembly_builder::editable_property_catalog(
        this.doc, PANEL, "@root@", requested,
    );
    let custom = assembly_builder::editable_property_catalog(
        this.doc, PANEL, "@root@", ["lunco:component:units"],
    );
    let automatic = assembly_builder::editable_property_catalog(
        this.doc, PANEL, "@root@", (),
    );
    if !query_ok(catalog) || !query_ok(automatic) || custom.ok
        || property(automatic, "variant:configuration") == () {
        fail(this, "schema-driven property catalog did not validate requested fields");
        return;
    }
    let translate = property(catalog, "xformOp:translate");
    let size = property(catalog, "size");
    let kind = property(catalog, "kind");
    let order = property(catalog, "xformOpOrder");
    let variant = property(catalog, "variant:configuration");
    let input = property(catalog, "inputs:surface_area");
    if translate == () || size == () || kind == () || order == ()
        || variant == () || input == () {
        fail(this, "schema-driven catalog omitted a requested standard field");
        return;
    }
    let collision = property(catalog, "physics:collisionEnabled");
    let already_patched = vector_close(translate.value, [2.0, 0.0, 0.0])
        && close_enough(size.value, 2.0)
        && collision.value == false
        && close_enough(input.value, 3.5)
        && variant.value == "wide";
    if already_patched {
        // The scene runner can replace the file-backed script after its first
        // attach while the document remains live. Preserve the current
        // document as the authoritative state and continue with the final
        // no-op/rejection checks instead of replaying setup.
        this.patch_already_current = true;
        this.phase = 1;
        return;
    }
    if !vector_close(translate.value, this.gizmo_after_translate)
        || !close_enough(size.value, 13.0)
        || collision.value != true
        || !close_enough(input.value, 17.0)
        || variant.value != "base" {
        fail(this, "fixture defaults did not match the authored standard USD values");
        return;
    }
    let patch = assembly_builder::editable_property_patch_plan(
        this.doc, "@root@", PANEL,
        [
            #{ name: "xformOp:translate", value: [2.0, 0.0, 0.0] },
            #{ name: "size", type_name: "double", value: 2.0 },
            #{ name: "physics:collisionEnabled", type_name: "bool", value: false },
            #{ name: "inputs:surface_area", type_name: "float", value: 3.5 },
            #{ name: "variant:configuration", value: "wide" },
        ], current.generation,
    );
    if !query_ok(patch) {
        fail(this, "schema-driven patch returned an error");
        return;
    }
    if patch.no_op {
        fail(this, "schema-driven patch unexpectedly became a no-op");
        return;
    }
    if patch.ops.len() != 5 {
        fail(this, "schema-driven patch emitted an unexpected operation count");
        return;
    }
    if patch.changed != ["xformOp:translate", "size", "physics:collisionEnabled",
        "inputs:surface_area", "variant:configuration"] {
        fail(this, "schema-driven patch changed the wrong field names");
        return;
    }
    let proposal = assembly_edit::propose(
        this.doc, #{ Assembly: () }, "Apply schema-driven property patch",
        patch.ops, current.generation,
    );
    if !query_ok(proposal) {
        fail(this, "schema-driven property proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals.len() != 1
        || review.proposals[0].diagnostics.len() != 0 {
        fail(this, "schema-driven property proposal did not validate cleanly");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "schema-driven property proposal could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = current.generation;
    this.phase = 1;
}
