// TEST_KIND = "editor": exercise authored USD edits through the production
// document and preview-projection lifecycle through the public editor APIs.
const TEST_KIND = "editor";
const ROOT = "/UsdMaterialEditProjection";
const SHADER = ROOT + "/World/MyMaterial/PbrShader";
const GEOMETRY = ROOT + "/World/MeshWithMaterial";
const PREVIEW = 15991;
const SECOND_VIEW = 15992;
const SECOND_PREVIEW = 15993;
const PROBE = ROOT + "/World/LifecycleProbe";

fn finish(me, passed, message) {
    if me.done { return me; }
    me.done = true;
    report_verdict([t_true(passed, message)], "USD MATERIAL EDIT PROJECTION", "USD_MATERIAL_EDIT_PROJECTION");
    me
}

fn document_id() {
    let documents = query("ListOpenDocuments");
    if documents == () || documents.open_documents == () { return (); }
    for document in documents.open_documents {
        if document.kind != "usd" { continue; }
        let root = query("InspectUsdDocument", #{ doc_id: document.doc_id, path: ROOT });
        if root != () && root.ok != false && root.prim != () && root.prim.exists {
            return document.doc_id;
        }
    }
    ()
}

fn preview_ready(preview_id, doc, generation) {
    for preview in assembly_edit::viewport().previews {
        if preview.preview.to_string() == preview_id.to_string() && preview.doc_id == doc
            && preview.projection_ready
            && preview.projected_generation == generation { return true; }
    }
    false
}

fn preview_matches(viewport, preview_id, doc) {
    let matches = 0;
    for preview in viewport.previews {
        if preview.preview.to_string() == preview_id.to_string()
            && preview.doc_id == doc { matches += 1; }
    }
    matches
}

fn preview_record(id) {
    for preview in assembly_edit::viewport().previews {
        if preview.preview.to_string() == id.to_string() { return preview; }
    }
    ()
}

fn framed_at(preview_id, view_id, x) {
    let preview = preview_record(preview_id);
    if preview == () { return false; }
    for view in preview.views {
        if view.view.to_string() == view_id.to_string() {
            return (view.target[0] - x).abs() < 0.0001
                && view.target[1].abs() < 0.0001 && view.target[2].abs() < 0.0001;
        }
    }
    false
}

fn shader_matches(doc, red, green, blue, roughness) {
    let shader = query("QueryUsdPrim", #{
        doc_id: doc,
        path: SHADER,
        attrs: ["inputs:diffuseColor", "inputs:roughness"],
    });
    if shader == () || shader.ok == false || shader.attrs == () { return false; }
    let color = shader.attrs["inputs:diffuseColor"];
    let authored_roughness = shader.attrs["inputs:roughness"];
    let matches = color != () && color.len() == 3
        && abs(color[0] - red) < 0.0001
        && abs(color[1] - green) < 0.0001
        && abs(color[2] - blue) < 0.0001
        && authored_roughness != ()
        && abs(authored_roughness - roughness) < 0.0001;
    matches
}

// Read geometry through the same public provider as the authoring tools. A
// ready standalone preview must use its edited stage rather than its recipe.
fn geometry_matches(doc, expected_x, expected_half_x) {
    let prim = query("QueryUsdPrim", #{doc_id: doc, path: GEOMETRY,
        attrs: [], geometry_bounds: true});
    if prim == () || prim.ok == false || prim.geometry_bounds == () { return false; }
    let bounds = prim.geometry_bounds;
    abs(bounds.center[0] - expected_x) < 0.000001
        && abs(bounds.half_extents[0] - expected_half_x) < 0.000001
        && abs(bounds.center[1]) < 0.000001
        && abs(bounds.center[2]) < 0.000001
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["InspectUsdDocument", "InspectUsdViewport", "ListOpenDocuments", "QueryUsdPrim", "SyncUsdDocument"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.phase = 0;
    this.elapsed = 0.0;
    this.doc = ();
    this.source_doc = ();
    this.initial_docs = [];
    this.command = ();
    this.generation = ();
    this.primary_view = ();
    this.original_source = "";
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 45.0 {
        this = finish(this, false, "USD material edit timed out in phase " + this.phase);
        return;
    }

    if this.phase == 0 {
        this.source_doc = document_id();
        if this.source_doc == () { return; }
        let documents = query("ListOpenDocuments");
        for document in documents.open_documents {
            this.initial_docs.push(document.doc_id);
        }
        let forked = assembly_edit::fork_document(this.source_doc, "PBR material preview");
        if forked == () || forked.ok == false {
            this = finish(this, false, "Editor did not accept a material preview document fork");
            return;
        }
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        let documents = query("ListOpenDocuments");
        for document in documents.open_documents {
            if document.kind == "usd" && !this.initial_docs.contains(document.doc_id) {
                this.doc = document.doc_id;
                break;
            }
        }
        if this.doc == () { return; }
        let description = assembly_edit::describe(this.doc);
        let source = assembly_edit::sync_document(this.doc, ());
        if description == () || description.ok == false || source == ()
            || source.layers == () || source.layers.root == () {
            this = finish(this, false, "USD material fixture document was not readable");
            return;
        }
        this.generation = description.generation;
        this.original_source = source.layers.root.source;
        let opened = assembly_ui::open_session(PREVIEW, this.doc, "@root@");
        if opened == () || opened.ok == false {
            this = finish(this, false, "Editor preview session could not be opened");
            return;
        }
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        let prepared = preview_record(PREVIEW);
        if prepared == () || !prepared.projection_ready { return; }
        let description = assembly_edit::describe(this.doc);
        if description == () || description.ok == false {
            this = finish(this, false, "Editor preview document became unavailable");
            return;
        }
        this.generation = description.generation;
        if !preview_ready(PREVIEW, this.doc, this.generation) {
            return;
        }
        let viewport = assembly_edit::viewport();
        if viewport == () {
            this = finish(this, false, "InspectUsdViewport returned no viewport state");
            return;
        }
        let preview_count = preview_matches(viewport, PREVIEW, this.doc);
        let preview = ();
        for candidate in viewport.previews {
            if candidate.preview.to_string() == PREVIEW.to_string()
                && candidate.doc_id == this.doc {
                preview = candidate;
            }
        }
        if preview_count != 1 || viewport.focused_preview == ()
            || viewport.focused_preview.to_string() != PREVIEW.to_string()
            || preview == () || !preview.focused || preview.views.len() != 1
            || viewport.focused_view == () || viewport.focused_view != preview.views[0].view {
            this = finish(this, false, "InspectUsdViewport did not report the opened preview session");
            return;
        }
        this.primary_view = preview.views[0].view;
        let opened = assembly_edit::preview_view_open(PREVIEW, SECOND_VIEW);
        if opened == () || opened.id == () || opened.ok == false {
            this = finish(this, false, "public USD command did not open a second preview view");
            return;
        }
        this.command = opened.id;
        this.phase = 6;
        return;
    }

    if this.phase == 6 {
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        if result.status != "applied" || result.ok != true {
            this = finish(this, false, "second USD preview view failed: " + result.error);
            return;
        }
        let focused = assembly_edit::preview_view_focus(SECOND_VIEW);
        if focused == () || focused.id == () || focused.ok == false {
            this = finish(this, false, "public USD command did not focus the second preview view");
            return;
        }
        this.command = focused.id;
        this.phase = 7;
        return;
    }

    if this.phase == 7 {
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        if result.status != "applied" || result.ok != true {
            this = finish(this, false, "second USD preview view could not be focused: " + result.error);
            return;
        }
        let viewport = assembly_edit::viewport();
        if viewport == () {
            this = finish(this, false, "InspectUsdViewport returned no viewport state");
            return;
        }
        let preview_count = preview_matches(viewport, PREVIEW, this.doc);
        let preview = ();
        for candidate in viewport.previews {
            if candidate.preview.to_string() == PREVIEW.to_string()
                && candidate.doc_id == this.doc {
                preview = candidate;
            }
        }
        if viewport.focused_preview == () || viewport.focused_view == ()
            || viewport.focused_preview.to_string() != PREVIEW.to_string()
            || viewport.focused_view.to_string() != SECOND_VIEW.to_string()
            || preview_count != 1 || preview == () || !preview.focused || preview.views.len() != 2
            || preview.views[0].view != this.primary_view || preview.views[0].focused
            || preview.views[1].view.to_string() != SECOND_VIEW.to_string()
            || !preview.views[1].focused {
            this = finish(this, false, "InspectUsdViewport did not report the exact focused preview/view handles");
            return;
        }
        if !shader_matches(this.doc, 1.0, 0.5, 0.25, 0.75) {
            this = finish(this, false, "fixture did not expose its authored USD material values");
            return;
        }
        if !geometry_matches(this.doc, 0.0, 1.0) {
            this = finish(this, false, "fixture geometry did not match its initial preview");
            return;
        }
        let edit = assembly_edit::batch(this.doc, "Change material appearance", [
            #{ SetAttribute: #{ edit_target: "@root@", path: SHADER,
                name: "inputs:diffuseColor", type_name: "color3f", value: "(0.0, 0.0, 1.0)" } },
            #{ SetAttribute: #{ edit_target: "@root@", path: SHADER,
                name: "inputs:roughness", type_name: "float", value: "0.1" } },
            #{ SetTranslate: #{ edit_target: "@root@", path: GEOMETRY,
                value: [3.0, 0.0, 0.0] } },
            #{ SetScale: #{ edit_target: "@root@", path: GEOMETRY,
                value: [2.0, 1.0, 1.0] } },
        ], this.generation);
        if edit == () || edit.id == () || edit.ok == false {
            this = finish(this, false, "public USD edit command rejected the material change");
            return;
        }
        this.command = edit.id;
        this.phase = 8;
        return;
    }

    if this.phase == 8 || this.phase == 4 {
        let phase = this.phase;
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        if result.status != "applied" || result.ok != true {
            this = finish(this, false, "USD material command failed: " + result.error);
            return;
        }
        this.generation = assembly_edit::describe(this.doc).generation;
        if phase == 8 {
            this.phase = 3;
        } else {
            this.phase = 5;
        }
        return;
    }

    if this.phase == 3 {
        if !preview_ready(PREVIEW, this.doc, this.generation) { return; }
        if !shader_matches(this.doc, 0.0, 0.0, 1.0, 0.1) {
            this = finish(this, false, "USD edit did not update the authored material query and preview generation");
            return;
        }
        if !geometry_matches(this.doc, 3.0, 2.0)
            || geometry_matches(this.doc, 0.0, 1.0) {
            this = finish(this, false, "ready preview query returned geometry from the immutable recipe");
            return;
        }
        let restore = cmd("ApplyUsdOp", #{
            doc_id: this.doc,
            parent_gen: this.generation,
            op: #{ ReplaceSource: #{ edit_target: "@root@", text: this.original_source } },
        });
        if restore == () || restore.id == () || restore.ok == false {
            this = finish(this, false, "public whole-source replacement command was rejected");
            return;
        }
        this.command = restore.id;
        this.phase = 4;
        return;
    }

    if this.phase == 5 {
        if !preview_ready(PREVIEW, this.doc, this.generation) { return; }
        if !shader_matches(this.doc, 1.0, 0.5, 0.25, 0.75) {
            this = finish(this, false, "whole-source replacement did not restore the original USD material");
            return;
        }
        if !geometry_matches(this.doc, 0.0, 1.0) {
            this = finish(this, false, "whole-source replacement did not restore preview geometry");
            return;
        }
        this.stage = preview_record(PREVIEW).stage_asset_id;
        let opened = assembly_ui::open_session(SECOND_PREVIEW, this.doc, "@root@");
        if opened == () || opened.ok == false {
            this = finish(this, false, "second document lease could not be opened"); return;
        }
        this.phase = 10; return;
    }

    if this.phase == 10 {
        if !preview_ready(SECOND_PREVIEW, this.doc, this.generation) { return; }
        let preview = preview_record(SECOND_PREVIEW);
        if preview.stage_asset_id != this.stage || preview.views.len() != 1 {
            this = finish(this, false, "same-document leases do not share their source stage"); return;
        }
        this.other_view = preview.views[0].view;
        let edit = assembly_edit::batch(this.doc, "Create structural projection probe", [
            #{AddPrim: #{edit_target: "@root@", parent_path: ROOT + "/World", name: "LifecycleProbe",
                type_name: "Cube", reference: (), reference_prim_path: ()}},
            #{SetTranslate: #{edit_target: "@root@", path: PROBE, value: [17.0, 0.0, 0.0]}},
        ], this.generation);
        if edit == () || edit.ok == false || edit.id == () {
            this = finish(this, false, "structural projection probe was rejected"); return;
        }
        this.command = edit.id; this.phase = 11; return;
    }

    if this.phase == 11 || this.phase == 14 {
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        if result.ok != true || result.status != "applied" {
            this = finish(this, false, "structural preview edit failed: " + result.error); return;
        }
        this.generation = assembly_edit::describe(this.doc).generation;
        this.phase = if this.phase == 11 { 12 } else { 15 }; return;
    }

    if this.phase == 12 || this.phase == 15 {
        if !preview_ready(PREVIEW, this.doc, this.generation)
            || !preview_ready(SECOND_PREVIEW, this.doc, this.generation) { return; }
        if preview_record(PREVIEW).stage_asset_id != this.stage
            || preview_record(SECOND_PREVIEW).stage_asset_id != this.stage {
            this = finish(this, false, "structural edit replaced a leased source stage"); return;
        }
        if this.phase == 12 {
            cmd("FrameUsdPreviewSelection", #{preview: PREVIEW, view: this.primary_view, path: PROBE});
            cmd("FrameUsdPreviewSelection", #{preview: SECOND_PREVIEW, view: this.other_view, path: PROBE});
            this.phase = 13;
        } else {
            cmd("FrameUsdPreviewView", #{view: this.primary_view});
            cmd("FrameUsdPreviewView", #{view: this.other_view});
            this.phase = 16;
        }
        return;
    }

    if this.phase == 13 {
        if !framed_at(PREVIEW, this.primary_view, 17.0)
            || !framed_at(SECOND_PREVIEW, this.other_view, 17.0) { return; }
        let edit = assembly_edit::batch(this.doc, "Retire structural projection probe", [
            #{RemovePrim: #{edit_target: "@root@", path: PROBE}},
        ], this.generation);
        if edit == () || edit.ok == false || edit.id == () {
            this = finish(this, false, "structural probe removal was rejected"); return;
        }
        this.command = edit.id; this.phase = 14; return;
    }

    if this.phase == 16 {
        // Framing reads projected mesh bounds. A leaked probe would retain
        // the old x=17 extent even though the document no longer contains it.
        if !framed_at(PREVIEW, this.primary_view, 0.0)
            || !framed_at(SECOND_PREVIEW, this.other_view, 0.0) { return; }
        let removed = query("QueryUsdPrim", #{doc_id: this.doc, path: PROBE});
        if removed.ok != false || removed.code != 404 {
            this = finish(this, false, "retired probe remains in the document"); return;
        }
        let edit = assembly_edit::batch(this.doc, "Admit a changed reference target", [
            #{AddPrim: #{edit_target: "@root@", parent_path: ROOT + "/World", name: "LifecycleProbe",
                type_name: "Xform", reference: (), reference_prim_path: ()}},
            #{SetReferenceArcs: #{edit_target: "@root@", path: PROBE, list_op: "Explicit",
                references: [#{asset_path: "lunco://vessels/markers/landing_location.usda", prim_path: ()}]}},
        ], this.generation);
        if edit == () || edit.ok == false || edit.id == () {
            this = finish(this, false, "changed reference target was rejected"); return;
        }
        this.command = edit.id; this.phase = 17; return;
    }

    if this.phase == 17 {
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        if result.ok != true || result.status != "applied" {
            this = finish(this, false, "reference closure admission failed: " + result.error); return;
        }
        this.generation = assembly_edit::describe(this.doc).generation;
        this.phase = 18; return;
    }

    if this.phase == 18 {
        if !preview_ready(PREVIEW, this.doc, this.generation)
            || !preview_ready(SECOND_PREVIEW, this.doc, this.generation) { return; }
        let marker = query("QueryUsdPrim", #{doc_id: this.doc, path: PROBE + "/Dome",
            attrs: [], geometry_bounds: true});
        if marker == () || marker.ok == false || marker.type_name != "Cylinder"
            || marker.geometry_bounds == () || marker.geometry_bounds.half_extents[0] < 2.49 {
            this = finish(this, false, "ready previews omitted the newly referenced geometry"); return;
        }
        let invalid = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: this.generation,
            op: #{SetReferenceArcs: #{edit_target: "@root@", path: PROBE, list_op: "Append",
                references: [#{asset_path: "lunco://vessels/markers/landing_location.usda", prim_path: "NotAbsolute"}]}}});
        if invalid == () || invalid.id == () {
            this = finish(this, false, "invalid reference command had no terminal result"); return;
        }
        this.command = invalid.id; this.phase = 19; return;
    }

    if this.phase == 19 {
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        if result.ok != false || assembly_edit::describe(this.doc).generation != this.generation {
            this = finish(this, false, "invalid reference target changed the document"); return;
        }
        assembly_edit::preview_close(SECOND_PREVIEW);
        assembly_edit::preview_close(PREVIEW);
        assembly_edit::close_document(this.doc);
        this = finish(this, true, "material, source, structure and reference closures reconcile every preview lease; invalid targets reject");
    }
}
