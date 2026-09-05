// TEST-ONLY. Production scene-test acceptance for semantic placement and
// alignment tools supplied by the dynamically reloadable assembly_builder
// library.

const ROOT = "/AssemblyLayoutBuilderTest";
const ASSEMBLY = ROOT + "/Assembly";
const TARGET = ASSEMBLY + "/Target";
const TARGET_SHAPE = TARGET + "/Shape";
const MOVING = ASSEMBLY + "/Moving";
const MOVING_SHAPE = MOVING + "/Shape";

fn query_ok(record) {
    record != () && record.ok != false
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "ASSEMBLY LAYOUT BUILDER", "ASSEMBLY_LAYOUT_BUILDER");
    }
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

fn close_enough(actual, expected, epsilon) {
    actual != () && (actual - expected).abs() <= epsilon
}

fn on_start(me) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "assembly layout fixture document was not discoverable"); }
            return;
        }
    }

    let current = assembly_edit::describe(this.doc);
    if !query_ok(current) || current.generation == () {
        fail(this, "assembly layout fixture did not expose a document generation");
        return;
    }

    if this.phase == 1 || this.phase == 2 {
        if current.generation <= this.wait_generation {
            if this.elapsed > 8.0 {
                fail(this, "semantic layout edit did not reach its inspected generation");
            }
            return;
        }
    }

    if this.phase == 2 {
        let moving = query("QueryUsdPrim", #{
            doc: this.doc,
            path: MOVING,
            attrs: ["xformOp:translate"],
        });
        let target = query("QueryUsdPrim", #{
            doc: this.doc,
            path: TARGET,
            attrs: ["xformOp:translate"],
        });
        let moving_position = if query_ok(moving) {
            moving.world_position
        } else { () };
        let target_position = if query_ok(target) {
            target.world_position
        } else { () };
        this.done = true;
        report_verdict([
            t_true(this.place_ops == 1, "place_plan emits one typed local transform"),
            t_true(this.alignment_ops == 1, "edge alignment emits one typed local transform"),
            t_true(this.invalid_place == true, "invalid placement dimensions are rejected"),
            t_true(this.invalid_axis == true, "invalid alignment axes are rejected"),
            t_true(this.invalid_parent == true, "cross-parent alignment is rejected"),
            t_true(query_ok(moving) && query_ok(target), "aligned prims remain queryable"),
            t_true(moving_position != () && target_position != ()
                && close_enough(moving_position[0], -1.75, 0.001),
                "moving cube edge is aligned to target edge with the authored gap"),
        ], "ASSEMBLY LAYOUT BUILDER", "ASSEMBLY_LAYOUT_BUILDER");
        return;
    }

    if this.phase == 1 {
        let alignment = assembly_builder::align_cube_edges_plan(
            this.doc, "@root@", MOVING, MOVING_SHAPE,
            TARGET, TARGET_SHAPE, "X", 1.0, -1.0, 0.25,
        );
        if !alignment.ok {
            fail(this, "edge alignment plan was rejected: " + alignment.error);
            return;
        }
        this.alignment_ops = alignment.ops.len();
        let proposal = assembly_edit::propose(
            this.doc, #{ Assembly: () }, "Align moving cube edge to target",
            alignment.ops, current.generation,
        );
        if !query_ok(proposal) {
            fail(this, "edge alignment proposal was rejected: " + proposal.error);
            return;
        }
        let review = assembly_edit::review_session(this.doc);
        if !query_ok(review) || review.proposals == () || review.proposals.len() != 1 {
            fail(this, "edge alignment proposal was not visible for review");
            return;
        }
        let committed = assembly_edit::commit_proposal(review.proposals[0].id);
        if !query_ok(committed) {
            fail(this, "edge alignment proposal could not be committed: " + committed.error);
            return;
        }
        this.wait_generation = current.generation;
        this.phase = 2;
        return;
    }

    let placement = assembly_builder::place_plan(
        "@root@", MOVING, [3.0, 0.0, 0.0], (), (),
    );
    let invalid_place = assembly_builder::place_plan(
        "@root@", MOVING, [3.0, 0.0], (), (),
    );
    let invalid_axis = assembly_builder::align_centers_plan(
        this.doc, "@root@", MOVING, TARGET, "Q", 0.0,
    );
    let invalid_parent = assembly_builder::align_centers_plan(
        this.doc, "@root@", MOVING, ROOT, "X", 0.0,
    );
    if !placement.ok || invalid_place.ok || invalid_axis.ok || invalid_parent.ok {
        fail(this, "semantic placement/alignment tools did not reject invalid input");
        return;
    }
    this.place_ops = placement.ops.len();
    this.invalid_place = invalid_place.error.contains("three components");
    this.invalid_axis = invalid_axis.error.contains("axis");
    this.invalid_parent = invalid_parent.error.contains("same authored parent");

    let proposal = assembly_edit::propose(
        this.doc, #{ Assembly: () }, "Place moving cube",
        placement.ops, current.generation,
    );
    if !query_ok(proposal) {
        fail(this, "placement proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals == () || review.proposals.len() != 1 {
        fail(this, "placement proposal was not visible for review");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "placement proposal could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = current.generation;
    this.phase = 1;
}
