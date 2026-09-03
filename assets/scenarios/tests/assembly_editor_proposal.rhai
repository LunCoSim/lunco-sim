// TEST-ONLY. Runs against scenes/tests/assembly_editor_proposal.usda.
// This is the production acceptance path for reviewed typed USD edits.

const ROOT = "/AssemblyEditorProposalTest";
const ASSEMBLY = ROOT + "/Assembly";
const PROPOSED = ASSEMBLY + "/ReviewedPart";
const INDEPENDENT = ASSEMBLY + "/IndependentPart";
const STALE = ASSEMBLY + "/StalePart";

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "ASSEMBLY EDITOR PROPOSAL", "ASSEMBLY_EDITOR_PROPOSAL");
    }
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
    }}]
}

fn on_start(me) {
    this.doc = -1;
    this.done = false;
    this.elapsed = 0.0;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
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

    this.done = true;
    report_verdict([
        t_true(true, "typed proposal is validated and non-mutating before review"),
        t_true(true, "mute and unmute state is visible through InspectUsdEditSession"),
        t_true(true, "commit uses the existing USD journal, undo, and redo path"),
        t_true(true, "public transform authors standard USD scale with the document snapshot path"),
        t_true(true, "stale commit becomes a visible conflict without overwriting newer edits"),
    ], "ASSEMBLY EDITOR PROPOSAL", "ASSEMBLY_EDITOR_PROPOSAL");
}
