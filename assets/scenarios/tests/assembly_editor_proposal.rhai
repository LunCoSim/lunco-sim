// TEST-ONLY. Runs against scenes/tests/assembly_editor_proposal.usda.
// This is the production acceptance path for reviewed typed USD edits, including
// one generated nonzero SysML -> Modelica -> Editor proposal round trip.

import "modelica_editor" as modelica_editor;
import "sysml_modelica_constraints" as geometry_constraints;

const ROOT = "/AssemblyEditorProposalTest";
const ASSEMBLY = ROOT + "/Assembly";
const PROPOSED = ASSEMBLY + "/ReviewedPart";
const INDEPENDENT = ASSEMBLY + "/IndependentPart";
const STALE = ASSEMBLY + "/StalePart";

fn fail(me, message) {
    if !me.done {
        if me.solve_ticket != () && me.solve_ticket.experiment_id != () {
            modelica_editor::cancel_experiment_ticket(me.solve_ticket);
        }
        if me.modelica_doc > 0 {
            modelica_editor::close(me.modelica_doc);
        }
        me.done = true;
        report_verdict([message], "ASSEMBLY EDITOR PROPOSAL", "ASSEMBLY_EDITOR_PROPOSAL");
    }
}

fn assertion_failures(checks) {
    let failures = [];
    for check in checks { if check != "" { failures.push(check); } }
    failures
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn find_usd_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" { return item.doc_id; }
    }
    -1
}

fn prim_exists(doc, path) {
    let inspection = assembly_edit::inspect(doc, path);
    query_ok(inspection) && inspection.prim != () && inspection.prim.exists == true
}

fn proposal_for(review, id) {
    if review == () || review.proposals == () { return (); }
    for proposal in review.proposals {
        if proposal.id == id { return proposal; }
    }
    ()
}

fn add_prim_plan(parent, name) {
    [#{ AddPrim: #{
        edit_target: "@root@",
        parent_path: parent,
        name: name,
        type_name: "Xform",
        reference: (),
        reference_prim_path: "",
    }}]
}

fn modelica_document_is_open(doc_id) {
    let documents = query("ListOpenDocuments", #{});
    if documents == () || documents.open_documents == () { return (); }
    for document in documents.open_documents {
        if document.kind == "modelica" && document.doc_id == doc_id { return true; }
    }
    false
}

fn vector_matches(value, expected) {
    if value == () { return false; }
    if type_of(value) == "Vec3" {
        return vlen(vsub(value, expected)) <= 1.0e-9;
    }
    type_of(value) == "array" && value.len() == 3
        && (value[0] - expected.x).abs() <= 1.0e-9
        && (value[1] - expected.y).abs() <= 1.0e-9
        && (value[2] - expected.z).abs() <= 1.0e-9
}

fn on_start(me, ctx) {
    this.doc = -1;
    this.modelica_doc = -1;
    this.solve_ticket = ();
    this.solve_checks = [];
    this.done = false;
    this.elapsed = 0.0;
    this.phase = "proposal";
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    if this.phase == "proposal" {
        if this.doc < 0 {
            this.doc = find_usd_document();
            if this.doc < 0 {
                if this.elapsed > 8.0 { fail(this, "no open USD document was discoverable"); }
                return;
            }
        }

        let before = assembly_edit::describe(this.doc);
        if !query_ok(before) || before.generation == () {
            fail(this, "InspectUsdDocument did not expose the proposal base generation");
            return;
        }
        if prim_exists(this.doc, PROPOSED) || prim_exists(this.doc, INDEPENDENT)
            || prim_exists(this.doc, STALE) {
            fail(this, "proposal fixture started with unexpected authored prims");
            return;
        }

        let plan = add_prim_plan(ASSEMBLY, "ReviewedPart");
        let created = assembly_edit::propose(
            this.doc,
            #{ Assembly: () },
            "Review a USD assembly part",
            plan,
            before.generation,
        );
        if !query_ok(created) {
            fail(this, "CreateUsdProposal was rejected: " + created.error);
            return;
        }

        let pending = assembly_edit::review_session(this.doc);
        if !query_ok(pending) || pending.proposals == () || pending.proposals.len() != 1 {
            fail(this, "proposal was not visible in the document review session");
            return;
        }
        let proposal = pending.proposals[0];
        if proposal.state != "pending" || proposal.ops == () || proposal.ops.len() != 1
            || proposal.affected_paths[0] != PROPOSED || prim_exists(this.doc, PROPOSED) {
            fail(this, "pending review did not preserve a typed, non-mutating plan");
            return;
        }

        let muted = assembly_edit::review_proposal(proposal.id, #{ Mute: () });
        let muted_review = assembly_edit::review_session(this.doc);
        let muted_proposal = proposal_for(muted_review, proposal.id);
        if !query_ok(muted) || muted_proposal == () || muted_proposal.state != "muted" {
            fail(this, "proposal mute was not reflected by the review query");
            return;
        }

        let unmuted = assembly_edit::review_proposal(proposal.id, #{ Unmute: () });
        let unmuted_review = assembly_edit::review_session(this.doc);
        let unmuted_proposal = proposal_for(unmuted_review, proposal.id);
        if !query_ok(unmuted) || unmuted_proposal == () || unmuted_proposal.state != "pending" {
            fail(this, "proposal unmute was not reflected by the review query");
            return;
        }

        let committed = assembly_edit::commit_proposal(proposal.id);
        let after_commit = assembly_edit::describe(this.doc);
        let committed_review = assembly_edit::review_session(this.doc);
        if !query_ok(committed) || !prim_exists(this.doc, PROPOSED)
            || after_commit.generation <= before.generation
            || committed_review.proposals.len() != 0 {
            fail(this, "proposal commit did not enter the document journal atomically");
            return;
        }

        let undone = assembly_edit::undo(this.doc);
        if !query_ok(undone) || prim_exists(this.doc, PROPOSED) {
            fail(this, "committed proposal did not use the existing USD undo path");
            return;
        }
        let redone = assembly_edit::redo(this.doc);
        if !query_ok(redone) || !prim_exists(this.doc, PROPOSED) {
            fail(this, "committed proposal did not use the existing USD redo path");
            return;
        }

        let sysml_path = "assets/scripting/tests/fixtures/sysml_constraint_projection.sysml";
        this.constraint_report = geometry_constraints::analyze_selected(
            sysml_path, ["movingAnchor", "fixedAnchor"]);
        if this.constraint_report == () || this.constraint_report.ok != true {
            fail(this, "SysML geometry fixture did not produce typed source facts");
            return;
        }
        let generated = geometry_constraints::assemble_coincident_translation(
            this.constraint_report,
            "ConstraintProjection::StrutAssembly::alignMount");
        if generated == () || generated.ok != true {
            fail(this, "Rhai did not assemble the bound CoincidentPointTranslation model");
            return;
        }
        this.generated = generated;
        let new_modelica = modelica_editor::new_scratch(
            "model ConstraintSolveScratch\nend ConstraintSolveScratch;\n",
            "SysMLConstraintSolveScratch");
        if new_modelica == () || new_modelica.ok != true {
            fail(this, "Modelica Editor did not create a scratch document for the generated solve");
            return;
        }
        this.modelica_doc = new_modelica.doc_id;
        this.solve_elapsed = 0.0;
        this.phase = "await_modelica_document";
        return;
    }

    if this.phase == "await_modelica_document" {
        this.solve_elapsed += dt();
        let source = query("GetDocumentSource", #{ doc_id: this.modelica_doc });
        if source == () || source.ok == false {
            if this.solve_elapsed > 8.0 {
                fail(this, "Modelica Editor did not publish source for the acknowledged scratch document");
            }
            return;
        }
        if source.generation == () {
            fail(this, "Modelica Editor did not expose the scratch source generation");
            return;
        }
        this.solve_checks.push(t_true(
            source.doc_id == this.modelica_doc,
            "CreateNewScratchModel returned the exact document identity used for source inspection"));
        let started = geometry_constraints::begin_coincident_translation(
            this.modelica_doc, source.generation, this.constraint_report,
            "ConstraintProjection::StrutAssembly::alignMount");
        if started == () || started.ok != true {
            fail(this, "generated Modelica wrapper was not admitted by the document owner");
            return;
        }
        this.generated = started.generated;
        this.solve_ticket = started.ticket;
        this.phase = "await_modelica_solve";
        return;
    }

    if this.phase == "await_modelica_solve" {
        this.solve_elapsed += dt();
        if this.solve_elapsed > 30.0 {
            fail(this, "generated Modelica experiment did not reach a terminal readback");
            return;
        }
        let advanced = geometry_constraints::poll_coincident_translation(
            this.solve_ticket, this.generated);
        if advanced == () || advanced.ok != true {
            let message = if advanced == () || advanced.error == () {
                "missing diagnostic"
            } else {
                advanced.error
            };
            fail(this, "Rhai Modelica solve/readback failed: " + message);
            return;
        }
        this.solve_ticket = advanced.ticket;
        if advanced.pending == true { return; }

        let solution = advanced.solution;
        let expected = vsub(this.generated.fixed_point, this.generated.moving_point);
        let numerical_checks = [
            t_true(advanced.complete == true && advanced.run.state == "done"
                && advanced.result.state == "done",
                "the generated Modelica experiment completed and returned its typed trajectory"),
            t_true(type_of(solution.translation) == "Vec3"
                && solution.units == "m"
                && vlen(vsub(solution.translation, expected)) <= 1.0e-9,
                "the nonzero Modelica translation matches the source-bound SysML endpoints in metres"),
            t_true(vlen(solution.translation) > 0.1
                && solution.experiment_id == advanced.ticket.experiment_id
                && type_of(solution.experiment_id) == "string"
                && solution.source_revision_hex == this.generated.source_revision_hex,
                "the readback preserves a nonzero solved value, command-returned run identity, and SysML source revision"),
            t_true(solution.sample_time >= 0.01 - 1.0e-9
                && solution.result.times.len() >= 2,
                "the numerical readback contains the requested bounded solve samples"),
        ];
        let numerical_failures = assertion_failures(numerical_checks);
        if !numerical_failures.is_empty() {
            fail(this, "generated Modelica solve returned values outside its typed SysML contract: "
                + numerical_failures.join("; "));
            return;
        }
        this.solve_checks += numerical_checks;

        let doc_state = assembly_edit::describe(this.doc);
        if !query_ok(doc_state) || doc_state.generation == () {
            fail(this, "USD proposal fixture did not expose its current placement generation");
            return;
        }
        let placement = geometry_constraints::coincident_translation_placement_plan(
            this.doc, "@root@", PROPOSED, doc_state.generation, solution);
        if placement == () || placement.ok != true {
            fail(this, "solved result did not produce a generation-bound typed Editor placement plan");
            return;
        }
        let reviewed = geometry_constraints::propose_coincident_translation_plan(
            placement, "Review Modelica-derived coincidence placement", false);
        if reviewed == () || reviewed.ok != true || reviewed.committed != false {
            fail(this, "Editor did not expose the solved placement as an uncommitted reviewed proposal");
            return;
        }
        let translate_value = ();
        for op in reviewed.proposal.ops {
            if op.SetTranslate != () { translate_value = op.SetTranslate.value; }
        }
        let proposal_checks = [
            t_true(reviewed.proposal.state == "pending"
                && reviewed.proposal.affected_paths.len() == 1
                && reviewed.proposal.affected_paths[0] == PROPOSED,
                "the Editor review session contains the exact pending placement target"),
            t_true(vector_matches(translate_value, solution.translation)
                && placement.parent_generation == doc_state.generation
                && placement.experiment_id == solution.experiment_id,
                "the reviewed typed SetTranslate carries the solved f64 vector and source/run provenance"),
        ];
        let proposal_failures = assertion_failures(proposal_checks);
        if !proposal_failures.is_empty() {
            fail(this, "Editor proposal readback did not preserve solved translation provenance: "
                + proposal_failures.join("; "));
            return;
        }
        this.solve_checks += proposal_checks;
        let rejected = assembly_edit::review_proposal(
            reviewed.proposal.id, #{ Reject: () });
        let cleared = assembly_edit::review_session(this.doc);
        if !query_ok(rejected) || cleared.proposals.len() != 0 {
            fail(this, "test placement proposal did not leave the fixture geometry unmodified");
            return;
        }
        this.phase = "placement_finish";
    }

    if this.phase == "await_modelica_close" {
        this.solve_elapsed += dt();
        let still_open = modelica_document_is_open(this.modelica_doc);
        if still_open == () {
            if this.solve_elapsed > 10.0 {
                fail(this, "Modelica Editor could not confirm the scratch document close");
            }
            return;
        }
        if still_open {
            if this.solve_elapsed > 10.0 {
                fail(this, "the test-owned Modelica scratch document did not close cleanly");
            }
            return;
        }
        this.done = true;
        let checks = [
            t_true(true, "typed placement proposal was validated without committing fixture geometry"),
            t_true(true, "mute and unmute state is visible through InspectUsdEditSession"),
            t_true(true, "commit uses the existing USD journal, undo, and redo path"),
            t_true(true, "public transform authors standard USD scale with the document snapshot path"),
            t_true(true, "stale commit becomes a visible conflict without overwriting newer edits"),
            t_true(true, "the Modelica scratch document was closed after the solve readback"),
        ];
        checks += this.solve_checks;
        report_verdict(checks, "ASSEMBLY EDITOR PROPOSAL", "ASSEMBLY_EDITOR_PROPOSAL");
        return;
    }

    if this.phase != "placement_finish" { return; }

    let after_redo = assembly_edit::describe(this.doc);
    let scaled = assembly_edit::transform(
        this.doc,
        "@root@",
        PROPOSED,
        (),
        (),
        [2.0, 3.0, 4.0],
        after_redo.generation,
    );
    if !query_ok(scaled) {
        fail(this, "the public transform helper did not author a scale edit");
        return;
    }
    let snapshot = assembly_edit::sync_document(this.doc, ());
    let source = if snapshot != () && snapshot.layers != ()
        && snapshot.layers.root != () {
        snapshot.layers.root.source
    } else {
        ()
    };
    if source == () || !source.contains("custom double3 xformOp:scale")
        || !source.contains("(2.0, 3.0, 4.0)")
        || !source.contains("xformOpOrder = [\"xformOp:scale\"]") {
        fail(this, "scale edit did not serialize as a standard USD xformOp:scale");
        return;
    }

    let current = assembly_edit::describe(this.doc);
    let stale_created = assembly_edit::propose(
        this.doc,
        #{ Assembly: () },
        "Reject a stale USD assembly part",
        add_prim_plan(ASSEMBLY, "StalePart"),
        current.generation,
    );
    let stale_review = assembly_edit::review_session(this.doc);
    if !query_ok(stale_created) || stale_review.proposals.len() != 1 {
        fail(this, "stale-conflict proposal was not created for the current generation");
        return;
    }
    let stale_proposal = stale_review.proposals[0];
    let independent = assembly_edit::add_prim(
        this.doc,
        "@root@",
        ASSEMBLY,
        "IndependentPart",
        "Xform",
        (),
        current.generation,
    );
    if !query_ok(independent) || !prim_exists(this.doc, INDEPENDENT) {
        fail(this, "independent USD edit did not advance the proposal base");
        return;
    }

    let stale_commit = assembly_edit::commit_proposal(stale_proposal.id);
    let conflicted_review = assembly_edit::review_session(this.doc);
    let conflicted = proposal_for(conflicted_review, stale_proposal.id);
    if stale_commit.ok != false || conflicted == () || conflicted.state != "conflict"
        || prim_exists(this.doc, STALE) || !prim_exists(this.doc, INDEPENDENT) {
        fail(this, "stale proposal was not rejected without overwriting newer USD edits");
        return;
    }

    let rejected = assembly_edit::review_proposal(stale_proposal.id, #{ Reject: () });
    let final_review = assembly_edit::review_session(this.doc);
    if !query_ok(rejected) || final_review.proposals.len() != 0 {
        fail(this, "conflicted proposal was not removable from the review session");
        return;
    }

    let closed = modelica_editor::close(this.modelica_doc);
    if closed == () || closed.ok != true {
        fail(this, "the test-owned Modelica scratch document did not accept its close request");
        return;
    }
    this.solve_elapsed = 0.0;
    this.phase = "await_modelica_close";
}
