// TEST-ONLY. Production scene-test acceptance for generic composed collision
// bounds and the dynamic Rhai placement tool. Policy assertions stay here;
// Rust owns the shared query, geometry, and proposal/journal boundaries.

const ROOT = "/AssemblyCollisionClearanceBuilderTest";
const ASSEMBLY = ROOT + "/Assembly";
const BLOCKER = ASSEMBLY + "/CompoundBlocker";
const MOVING = ASSEMBLY + "/MovingCylinder";
const VISUAL_ONLY = ASSEMBLY + "/VisualOnly";
const ROTATED = ASSEMBLY + "/RotatedBlocker";
const MALFORMED = ASSEMBLY + "/MalformedBlocker";

fn query_ok(record) {
    record != () && record.ok != false
}

fn finish(me, checks) {
    if !me.done {
        me.done = true;
        report_verdict(checks, "ASSEMBLY COLLISION CLEARANCE BUILDER",
            "ASSEMBLY_COLLISION_CLEARANCE_BUILDER");
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
            if this.elapsed > 8.0 {
                fail(this, "collision clearance fixture document was not discoverable");
            }
            return;
        }
    }

    let document = assembly_edit::describe(this.doc);
    if !query_ok(document) || document.generation == () {
        fail(this, "collision clearance fixture did not expose a document generation");
        return;
    }

    if this.phase == 1 {
        if document.generation <= this.wait_generation {
            if this.elapsed > 8.0 {
                fail(this, "collision clearance placement did not reach its committed generation");
            }
            return;
        }
        let moving = query("QueryUsdPrim", #{
            doc: this.doc,
            path: MOVING,
            attrs: ["xformOp:translate"],
            collision_bounds: true,
        });
        let position = if query_ok(moving) { moving.world_position } else { () };
        finish(this, [
            t_true(query_ok(moving), "moving cylinder remains queryable after commit"),
            t_true(position != () && (position[0] - 3.0).abs() <= 0.001
                && position[1].abs() <= 0.001 && position[2].abs() <= 0.001,
                "committed moving body uses the collision-approved translation"),
            t_true(this.moving_bounds_ok == true,
                "shared query exposes the composed Cylinder bounds"),
            t_true(this.compound_bounds_ok == true,
                "shared query aggregates the compound blocker bounds"),
            t_true(this.valid_plan == true
                && this.valid_plan_kind == "collision_clearance_placement",
                "valid collision input returns a typed placement plan"),
            t_true(this.overlap_rejected == true,
                "overlapping candidates are rejected before proposal"),
            t_true(this.negative_gap_rejected == true,
                "negative clearance requirements are rejected"),
            t_true(this.visual_only_rejected == true,
                "visual-only blockers are rejected without collision geometry"),
            t_true(this.rotated_rejected == true,
                "rotated blocker frames are rejected instead of guessed"),
            t_true(this.malformed_rejected == true,
                "malformed collision bounds fail visibly through the shared reader"),
            t_true(this.duplicate_rejected == true,
                "duplicate blocker paths are rejected"),
        ]);
        return;
    }

    let moving_record = query("QueryUsdPrim", #{
        doc: this.doc,
        path: MOVING,
        collision_bounds: true,
    });
    let blocker_record = query("QueryUsdPrim", #{
        doc: this.doc,
        path: BLOCKER,
        collision_bounds: true,
    });
    if !query_ok(moving_record) || !query_ok(blocker_record)
        || moving_record.collision_bounds == ()
        || blocker_record.collision_bounds == () {
        fail(this, "shared collision bounds query did not return composed envelopes");
        return;
    }
    this.moving_bounds_ok = moving_record.collision_bounds.frame == "canonical_stage"
        && moving_record.collision_bounds.center[0] == 6.0
        && (moving_record.collision_bounds.half_extents[0] - 0.5).abs() <= 0.001
        && (moving_record.collision_bounds.half_extents[1] - 1.0).abs() <= 0.001;
    this.compound_bounds_ok = blocker_record.collision_bounds.frame == "canonical_stage"
        && blocker_record.collision_bounds.center[0].abs() <= 0.001
        && (blocker_record.collision_bounds.half_extents[0] - 2.0).abs() <= 0.001;

    let valid = assembly_builder::place_with_collision_clearance_plan(
        this.doc, "@root@", MOVING, [3.0, 0.0, 0.0],
        [#{ path: BLOCKER }], 0.25,
    );
    let overlap = assembly_builder::place_with_collision_clearance_plan(
        this.doc, "@root@", MOVING, [2.4, 0.0, 0.0],
        [#{ path: BLOCKER }], 0.25,
    );
    let negative_gap = assembly_builder::place_with_collision_clearance_plan(
        this.doc, "@root@", MOVING, [3.0, 0.0, 0.0],
        [#{ path: BLOCKER }], -0.1,
    );
    let visual_only = assembly_builder::place_with_collision_clearance_plan(
        this.doc, "@root@", MOVING, [3.0, 0.0, 0.0],
        [#{ path: VISUAL_ONLY }], 0.25,
    );
    let rotated = assembly_builder::place_with_collision_clearance_plan(
        this.doc, "@root@", MOVING, [3.0, 0.0, 0.0],
        [#{ path: ROTATED }], 0.25,
    );
    let malformed = assembly_builder::place_with_collision_clearance_plan(
        this.doc, "@root@", MOVING, [3.0, 0.0, 0.0],
        [#{ path: MALFORMED }], 0.25,
    );
    let duplicate = assembly_builder::place_with_collision_clearance_plan(
        this.doc, "@root@", MOVING, [3.0, 0.0, 0.0],
        [
            #{ path: BLOCKER },
            #{ path: BLOCKER },
        ], 0.25,
    );
    if !valid.ok || overlap.ok || negative_gap.ok || visual_only.ok
        || rotated.ok || malformed.ok || duplicate.ok {
        fail(this, "collision clearance builder did not enforce its explicit input contract");
        return;
    }

    this.valid_plan = valid.ok;
    this.valid_plan_kind = valid.kind;
    this.overlap_rejected = overlap.error.contains("overlaps");
    this.negative_gap_rejected = negative_gap.error.contains("non-negative");
    this.visual_only_rejected = visual_only.error.contains("no composed collision geometry");
    this.rotated_rejected = rotated.error.contains("translation-only");
    this.malformed_rejected = malformed.error.contains("invalid collision bounds");
    this.duplicate_rejected = duplicate.error.contains("unique");

    let proposal = assembly_edit::propose(
        this.doc, #{ Assembly: () }, "Place moving cylinder with collision clearance",
        valid.ops, document.generation,
    );
    if !query_ok(proposal) {
        fail(this, "collision clearance proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals == () || review.proposals.len() != 1
        || review.proposals[0].diagnostics.len() != 0 {
        fail(this, "collision clearance proposal was not cleanly visible for review");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "collision clearance could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = document.generation;
    this.phase = 1;
}
