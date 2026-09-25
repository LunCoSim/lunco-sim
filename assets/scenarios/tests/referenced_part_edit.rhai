// TEST-ONLY: composed component edits through production proposal and journal owners.
const TEST_KIND = "editor";
const PANEL = "/ReferencedPartEdit/Panel";
const PREVIEW = 15401;

fn on_start(me, ctx) {
    this.phase = 0; this.elapsed = 0.0; this.done = false;
    this.doc = (); this.checks = [];
}

fn ready(preview_id, doc, generation) {
    for p in assembly_edit::viewport().previews {
        if p.preview.to_string() == preview_id.to_string()
            && p.doc_id == doc && p.projection_ready
            && p.projected_generation == generation { return true; }
    }
    false
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 40.0 {
        this.checks.push(t_true(false, "referenced edit timeout phase " + this.phase));
        this.done = true;
        report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
        return;
    }
    if this.phase == 0 {
        let live_panel = query("QueryUsdPrim", #{ path: PANEL });
        if live_panel == () || live_panel.ok == false || live_panel.doc_id == () { return; }
        let inspected = assembly_edit::inspect(live_panel.doc_id, PANEL);
        if inspected == () || inspected.ok == false
            || inspected.prim == () || !inspected.prim.exists { return; }
        this.doc = live_panel.doc_id;
        this.initial_generation = assembly_edit::describe(this.doc).generation;
        assembly_ui::open_session(PREVIEW, this.doc, "@root@");
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        if !ready(PREVIEW, this.doc, this.initial_generation) { return; }
        let missing = assembly_edit::propose(this.doc, "Assembly", "Reject nonexistent referenced child",
            [#{ SetTranslate: #{ edit_target: "@root@", path: PANEL + "/Missing", value: [0.0, 1.0, 0.0] } }], this.initial_generation);
        if missing == () || missing.id == () {
            this.checks.push(t_true(false, "invalid child proposal was not submitted"));
            this.done = true;
            report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
            return;
        }
        this.command = missing.id;
        this.phase = 10;
        return;
    }
    if this.phase == 10 {
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        let invalid = result.status == "failed" || result.status == "rejected";
        this.checks.push(t_true(invalid && result.error != ()
            && result.error.contains("composed transform target"),
            "missing child proposal reports composed-target validation: " + result.status));
        this.checks.push(t_true(assembly_edit::describe(this.doc).generation == this.initial_generation,
            "missing child rejection preserves document generation"));
        if !invalid {
            this.done = true;
            report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
            return;
        }
        let proposal = assembly_edit::propose(this.doc, "Assembly", "Reshape referenced panel",
            [
                #{ SetTranslate: #{ edit_target: "@root@", path: PANEL + "/PanelFrame", value: [0.0, 0.5, 0.0] } },
                #{ SetRotate: #{ edit_target: "@root@", path: PANEL + "/PanelSurface", value: [0.0, 15.0, 0.0] } },
                #{ SetScale: #{ edit_target: "@root@", path: PANEL + "/PanelSurface", value: [2.0, 0.02, 1.0] } }
            ], this.initial_generation);
        if proposal == () || proposal.id == () {
            this.checks.push(t_true(false, "valid transform proposal was not submitted"));
            this.done = true;
            report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
            return;
        }
        this.command = proposal.id;
        this.phase = 11;
        return;
    }
    if this.phase == 11 {
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        let session = assembly_edit::review_session(this.doc);
        this.checks.push(t_true(result.status == "applied"
            && session != () && session.proposals != ()
            && session.proposals.len() == 1,
            "referenced children accept transform proposal"));
        if result.status != "applied" || session == () || session.proposals == ()
            || session.proposals.len() != 1 {
            this.done = true;
            report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
            return;
        }
        let committed = assembly_edit::commit_proposal(session.proposals[0].id);
        if committed == () || committed.id == () {
            this.checks.push(t_true(false, "valid transform proposal was not queued for commit"));
            this.done = true;
            report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
            return;
        }
        this.command = committed.id;
        this.phase = 12;
        return;
    }
    if this.phase == 12 {
        let result = command_result(this.command);
        if result.status == "pending" { return; }
        this.checks.push(t_true(result.status == "applied",
            "accepted referenced-part proposal commits through the USD owner"));
        if result.status != "applied" {
            this.done = true;
            report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
            return;
        }
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        let generation = assembly_edit::describe(this.doc).generation;
        if generation <= this.initial_generation || !ready(PREVIEW, this.doc, generation) { return; }
        let frame = query("QueryUsdPrim", #{ doc_id: this.doc, path: PANEL + "/PanelFrame" });
        let cells = query("QueryUsdPrim", #{ doc_id: this.doc, path: PANEL + "/PanelSurface" });
        this.checks.push(t_true(frame.attrs["xformOp:translate"][1] == 0.5,
            "referenced frame translation projects"));
        this.checks.push(t_true(frame.attrs["xformOp:scale"][0] > 6.19,
            "translation preserves inherited scale and order"));
        this.checks.push(t_true(cells.attrs["xformOp:rotateXYZ"][1] == 15.0
            && cells.attrs["xformOp:scale"][0] == 2.0,
            "referenced rotation and scale project"));
        this.changed_generation = generation;
        cmd("UndoDocument", #{ doc_id: this.doc });
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        let generation = assembly_edit::describe(this.doc).generation;
        if generation <= this.changed_generation || !ready(PREVIEW, this.doc, generation) { return; }
        let frame = query("QueryUsdPrim", #{ doc_id: this.doc, path: PANEL + "/PanelFrame" });
        this.checks.push(t_true(abs(frame.attrs["xformOp:translate"][1] + 0.005) < 0.000001,
            "group undo restores the referenced frame pose"));
        assembly_edit::preview_close(PREVIEW);
        this.done = true;
        report_verdict(this.checks, "REFERENCED PART EDIT", "REFERENCED_PART_EDIT");
    }
}
