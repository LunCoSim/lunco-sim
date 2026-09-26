// TEST_KIND = "editor": exercise authored USD edits through the production
// document and preview-projection lifecycle through the public editor APIs.
const TEST_KIND = "editor";
const ROOT = "/UsdMaterialEditProjection";
const SHADER = ROOT + "/World/MyMaterial/PbrShader";
const PREVIEW = 15991;
const SECOND_VIEW = 15992;

fn finish(me, passed, message) {
    if me.done { return; }
    me.done = true;
    report_verdict([t_true(passed, message)], "USD MATERIAL EDIT PROJECTION", "USD_MATERIAL_EDIT_PROJECTION");
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

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["InspectUsdDocument", "ListOpenDocuments", "QueryUsdPrim"], required_inputs: [] }
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
        finish(this, false, "USD material edit timed out in phase " + this.phase);
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
            finish(this, false, "Editor did not accept a material preview document fork");
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
            finish(this, false, "USD material fixture document was not readable");
            return;
        }
        this.generation = description.generation;
        this.original_source = source.layers.root.source;
        let opened = assembly_ui::open_session(PREVIEW, this.doc, "@root@");
        if opened == () || opened.ok == false {
            finish(this, false, "Editor preview session could not be opened");
            return;
        }
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        let description = assembly_edit::describe(this.doc);
        if description == () || description.ok == false {
            finish(this, false, "Editor preview document became unavailable");
            return;
        }
        this.generation = description.generation;
        if !preview_ready(PREVIEW, this.doc, this.generation) {
            return;
        }
        let viewport = assembly_edit::viewport();
        if viewport == () {
            finish(this, false, "InspectUsdViewport returned no viewport state");
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
            finish(this, false, "InspectUsdViewport did not report the opened preview session");
            return;
        }
        this.primary_view = preview.views[0].view;
        let opened = assembly_edit::preview_view_open(PREVIEW, SECOND_VIEW);
        if opened == () || opened.id == () || opened.ok == false {
            finish(this, false, "public USD command did not open a second preview view");
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
            finish(this, false, "second USD preview view failed: " + result.error);
            return;
        }
        let focused = assembly_edit::preview_view_focus(SECOND_VIEW);
        if focused == () || focused.id == () || focused.ok == false {
            finish(this, false, "public USD command did not focus the second preview view");
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
            finish(this, false, "second USD preview view could not be focused: " + result.error);
            return;
        }
        let viewport = assembly_edit::viewport();
        if viewport == () {
            finish(this, false, "InspectUsdViewport returned no viewport state");
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
            finish(this, false, "InspectUsdViewport did not report the exact focused preview/view handles");
            return;
        }
        if !shader_matches(this.doc, 1.0, 0.5, 0.25, 0.75) {
            finish(this, false, "fixture did not expose its authored USD material values");
            return;
        }
        let edit = assembly_edit::batch(this.doc, "Change material appearance", [
            #{ SetAttribute: #{ edit_target: "@root@", path: SHADER,
                name: "inputs:diffuseColor", type_name: "color3f", value: "(0.0, 0.0, 1.0)" } },
            #{ SetAttribute: #{ edit_target: "@root@", path: SHADER,
                name: "inputs:roughness", type_name: "float", value: "0.1" } },
        ], this.generation);
        if edit == () || edit.id == () || edit.ok == false {
            finish(this, false, "public USD edit command rejected the material change");
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
            finish(this, false, "USD material command failed: " + result.error);
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
            finish(this, false, "USD edit did not update the authored material query and preview generation");
            return;
        }
        let restore = cmd("ApplyUsdOp", #{
            doc_id: this.doc,
            parent_gen: this.generation,
            op: #{ ReplaceSource: #{ edit_target: "@root@", text: this.original_source } },
        });
        if restore == () || restore.id == () || restore.ok == false {
            finish(this, false, "public whole-source replacement command was rejected");
            return;
        }
        this.command = restore.id;
        this.phase = 4;
        return;
    }

    if this.phase == 5 {
        if !preview_ready(PREVIEW, this.doc, this.generation) { return; }
        if !shader_matches(this.doc, 1.0, 0.5, 0.25, 0.75) {
            finish(this, false, "whole-source replacement did not restore the original USD material");
            return;
        }
        assembly_edit::preview_close(PREVIEW);
        assembly_edit::close_document(this.doc);
        finish(this, true, "authored USD material edits and whole-source replacement advanced the production preview");
    }
}
