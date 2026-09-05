// TEST-ONLY: composed component edits through production proposal and journal owners.
const TEST_KIND = "graphics";
const PANEL = "/ReferencedPartEdit/Panel";
const RENAMED = "/ReferencedPartEdit/RenamedPanel";

fn on_start(me) {
    this.phase = 0; this.elapsed = 0.0; this.done = false;
    this.doc = -1; this.checks = [];
}

fn ready(doc, generation) {
    for p in assembly_edit::viewport().previews {
        if p.preview == 15401 && p.doc == doc && p.projection_ready
            && p.projected_generation == generation { return true; }
    }
    false
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 40.0 {
        this.checks.push(t_true(false, "referenced edit timeout phase " + this.phase));
        this.done = true;
        report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
        return;
    }
    if this.phase == 0 {
        for record in query("ListOpenDocuments").open_documents {
            if record.kind == "usd" {
                let prim = assembly_edit::inspect(record.doc_id, PANEL).prim;
                if prim != () && prim.exists { this.doc = record.doc_id; }
            }
        }
        if this.doc < 0 { return; }
        this.initial_generation = assembly_edit::describe(this.doc).generation;
        assembly_ui::open_session(15401, this.doc, "@root@");
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        if !ready(this.doc, this.initial_generation) { return; }
        let missing = assembly_edit::propose(this.doc, "Assembly", "Reject nonexistent referenced child",
            [#{ SetTranslate: #{ edit_target: "@root@", path: PANEL + "/Missing", value: [0.0, 1.0, 0.0] } }], this.initial_generation);
        this.checks.push(t_true(missing.ok == false && assembly_edit::describe(this.doc).generation == this.initial_generation,
            "missing child rejects without authoring an over"));
        let proposal = assembly_edit::propose(this.doc, "Assembly", "Rename and reshape referenced panel",
            [
                #{ MovePrim: #{ edit_target: "@root@", from_path: PANEL, to_path: RENAMED } },
                #{ SetTranslate: #{ edit_target: "@root@", path: RENAMED + "/PanelFrame", value: [0.0, 0.5, 0.0] } },
                #{ SetRotate: #{ edit_target: "@root@", path: RENAMED + "/PanelSurface", value: [0.0, 15.0, 0.0] } },
                #{ SetScale: #{ edit_target: "@root@", path: RENAMED + "/PanelSurface", value: [2.0, 0.02, 1.0] } }
            ], this.initial_generation);
        let session = assembly_edit::review_session(this.doc);
        this.checks.push(t_true(proposal.ok != false && session.proposals.len() == 1,
            "renamed referenced children accept transform proposal"));
        if proposal.ok == false || session.proposals.len() != 1 {
            this.done = true;
            report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
            return;
        }
        assembly_edit::commit_proposal(session.proposals[0].id);
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        let generation = assembly_edit::describe(this.doc).generation;
        if generation <= this.initial_generation || !ready(this.doc, generation) { return; }
        let frame = query("QueryUsdPrim", #{ doc: this.doc, path: RENAMED + "/PanelFrame" });
        let cells = query("QueryUsdPrim", #{ doc: this.doc, path: RENAMED + "/PanelSurface" });
        this.checks.push(t_true(frame.attrs["xformOp:translate"][1] == 0.5,
            "referenced frame translation projects"));
        this.checks.push(t_true(frame.attrs["xformOp:scale"][0] > 6.19,
            "translation preserves inherited scale and order"));
        this.checks.push(t_true(cells.attrs["xformOp:rotateXYZ"][1] == 15.0
            && cells.attrs["xformOp:scale"][0] == 2.0,
            "referenced rotation and scale project"));
        this.changed_generation = generation;
        cmd("UndoDocument", #{ doc: this.doc });
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        let generation = assembly_edit::describe(this.doc).generation;
        if generation <= this.changed_generation || !ready(this.doc, generation) { return; }
        let frame = query("QueryUsdPrim", #{ doc: this.doc, path: PANEL + "/PanelFrame" });
        this.checks.push(t_true(abs(frame.attrs["xformOp:translate"][1] + 0.005) < 0.000001,
            "group undo restores original namespace and inherited frame pose"));
        assembly_edit::preview_close(15401);
        this.done = true;
        report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
    }
}
