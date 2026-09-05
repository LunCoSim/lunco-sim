// TEST-ONLY. Production scene-test acceptance for Rhai-native placement
// clearance. Every policy assertion is authored here; Rust only owns the
// generic proposal, review, commit, and composed-query boundaries.

const ROOT = "/AssemblyClearanceBuilderTest";
const ASSEMBLY = ROOT + "/Assembly";
const BLOCKER = ASSEMBLY + "/Blocker";
const BLOCKER_SHAPE = BLOCKER + "/Shape";
const MOVING = ASSEMBLY + "/Moving";
const MOVING_SHAPE = MOVING + "/Shape";
const NON_CUBE = ASSEMBLY + "/NotCube";
const NON_CUBE_SHAPE = NON_CUBE + "/Shape";
const ROTATED = ASSEMBLY + "/RotatedBlocker";
const ROTATED_SHAPE = ROTATED + "/Shape";

fn query_ok(record) {
    record != () && record.ok != false
}

fn finish(me, checks) {
    if !me.done {
        me.done = true;
        report_verdict(checks, "ASSEMBLY CLEARANCE BUILDER", "ASSEMBLY_CLEARANCE_BUILDER");
    }
}

fn fail(me, message) {
    finish(me, [message]);
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" {
            let root = assembly_edit::inspect(item.doc_id, ROOT);
            if query_ok(root) && root.prim != () && root.prim.exists == true {
                return item.doc_id;
            }
        }
    }
    -1
}

fn on_start(me) {
    this.doc = -1;
    this.elapsed = 0.0;
    this.done = false;
    this.phase = 0;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "clearance fixture document was not discoverable"); }
            return;
        }
    }

    let document = assembly_edit::describe(this.doc);
    if !query_ok(document) || document.generation == () {
        fail(this, "clearance fixture did not expose a document generation");
        return;
    }

    if this.phase == 1 {
        if document.generation <= this.wait_generation {
            if this.elapsed > 8.0 {
                fail(this, "clearance placement did not reach its committed generation");
            }
            return;
        }
        let moving = query("QueryUsdPrim", #{
            doc: this.doc,
            path: MOVING,
            attrs: ["xformOp:translate"],
        });
        let position = if query_ok(moving) { moving.world_position } else { () };
        finish(this, [
            t_true(query_ok(moving), "moving frame remains queryable after commit"),
            t_true(position != () && (position[0] - 2.0).abs() <= 0.001
                && position[1].abs() <= 0.001 && position[2].abs() <= 0.001,
                "committed moving frame uses the clearance-approved translation"),
            t_true(this.valid_plan == true && this.valid_plan_kind == "clearance_placement",
                "valid clearance input returns a typed placement plan"),
            t_true(this.overlap_rejected == true,
                "overlapping candidates are rejected before proposal"),
            t_true(this.negative_gap_rejected == true,
                "negative clearance requirements are rejected"),
            t_true(this.non_cube_rejected == true,
                "non-Cube blocker geometry is rejected"),
            t_true(this.rotated_rejected == true,
                "rotated blocker frames are rejected instead of guessed"),
            t_true(this.duplicate_rejected == true,
                "duplicate blocker paths are rejected"),
        ]);
        return;
    }

    let valid = assembly_builder::place_with_clearance_plan(
        this.doc, "@root@", MOVING, MOVING_SHAPE, [2.0, 0.0, 0.0],
        [#{ path: BLOCKER, shape: BLOCKER_SHAPE }], 0.25,
    );
    let overlap = assembly_builder::place_with_clearance_plan(
        this.doc, "@root@", MOVING, MOVING_SHAPE, [1.0, 0.0, 0.0],
        [#{ path: BLOCKER, shape: BLOCKER_SHAPE }], 0.25,
    );
    let negative_gap = assembly_builder::place_with_clearance_plan(
        this.doc, "@root@", MOVING, MOVING_SHAPE, [2.0, 0.0, 0.0],
        [#{ path: BLOCKER, shape: BLOCKER_SHAPE }], -0.1,
    );
    let non_cube = assembly_builder::place_with_clearance_plan(
        this.doc, "@root@", MOVING, MOVING_SHAPE, [2.0, 0.0, 0.0],
        [#{ path: NON_CUBE, shape: NON_CUBE_SHAPE }], 0.25,
    );
    let rotated = assembly_builder::place_with_clearance_plan(
        this.doc, "@root@", MOVING, MOVING_SHAPE, [2.0, 0.0, 0.0],
        [#{ path: ROTATED, shape: ROTATED_SHAPE }], 0.25,
    );
    let duplicate = assembly_builder::place_with_clearance_plan(
        this.doc, "@root@", MOVING, MOVING_SHAPE, [2.0, 0.0, 0.0],
        [
            #{ path: BLOCKER, shape: BLOCKER_SHAPE },
            #{ path: BLOCKER, shape: BLOCKER_SHAPE },
        ], 0.25,
    );
    if !valid.ok || overlap.ok || negative_gap.ok || non_cube.ok
        || rotated.ok || duplicate.ok {
        fail(this, "clearance builder did not enforce its explicit input contract");
        return;
    }

    this.valid_plan = valid.ok;
    this.valid_plan_kind = valid.kind;
    this.overlap_rejected = overlap.error.contains("overlaps");
    this.negative_gap_rejected = negative_gap.error.contains("non-negative");
    this.non_cube_rejected = non_cube.error.contains("axis-aligned Cube");
    this.rotated_rejected = rotated.error.contains("translation-only");
    this.duplicate_rejected = duplicate.error.contains("unique");

    let proposal = assembly_edit::propose(
        this.doc, #{ Assembly: () }, "Place moving cube with clearance",
        valid.ops, document.generation,
    );
    if !query_ok(proposal) {
        fail(this, "clearance placement proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals == () || review.proposals.len() != 1
        || review.proposals[0].diagnostics.len() != 0 {
        fail(this, "clearance placement proposal was not cleanly visible for review");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "clearance placement could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = document.generation;
    this.phase = 1;
}
