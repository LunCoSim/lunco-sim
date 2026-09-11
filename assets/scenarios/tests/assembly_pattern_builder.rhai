// TEST-ONLY. Verifies generic ordered pattern and local-axis mirror plans
// through the normal proposal, review, journal, and composed-query boundary.

const ROOT = "/AssemblyPatternBuilderTest";
const ASSEMBLY = ROOT + "/Assembly";
const ASSET = "lunco://props/ramp.usda";

fn query_ok(record) {
    record != () && record.ok != false
}

fn array_has(values, expected) {
    if values == () { return false; }
    for value in values {
        if value == expected { return true; }
    }
    false
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

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "ASSEMBLY PATTERN BUILDER", "ASSEMBLY_PATTERN_BUILDER");
    }
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" {
            let root = query("QueryUsdPrim", #{
                doc_id: item.doc_id,
                path: ROOT,
                children: false,
            });
            if query_ok(root) { return item.doc_id; }
        }
    }
    -1
}

fn on_start(me) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.wait_generation = -1;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "pattern-builder fixture was not discoverable"); }
            return;
        }
    }

    let current = assembly_edit::describe(this.doc);
    if !query_ok(current) || current.generation == () {
        fail(this, "pattern-builder fixture did not expose a document generation");
        return;
    }
    if this.phase == 1 {
        if current.generation <= this.wait_generation {
            if this.elapsed > 8.0 { fail(this, "pattern/mirror edit did not reach its inspected generation"); }
            return;
        }
        let assembly = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ASSEMBLY,
            children: true,
        });
        if !query_ok(assembly) || assembly.children == ()
            || !array_has(assembly.children, ASSEMBLY + "/Left")
            || !array_has(assembly.children, ASSEMBLY + "/Right")
            || !array_has(assembly.children, ASSEMBLY + "/Rear") {
            if this.elapsed > 8.0 { fail(this, "pattern/mirror instances did not materialize in the composed assembly"); }
            return;
        }
        let left = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ASSEMBLY + "/Left",
            attrs: ["xformOp:translate"],
        });
        let right = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ASSEMBLY + "/Right",
            attrs: ["xformOp:translate"],
        });
        let rear = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ASSEMBLY + "/Rear",
            attrs: ["xformOp:translate"],
        });
        this.done = true;
        report_verdict([
            t_true(this.pattern_paths == [ASSEMBLY + "/Left", ASSEMBLY + "/Right"],
                "pattern preserves explicit placement order and paths"),
            t_true(this.pattern_ops > 2 && this.mirror_ops > 1,
                "pattern and mirror compose typed reference and transform operations"),
            t_true(this.deterministic,
                "repeating a dry pattern request returns the same ordered plan shape"),
            t_true(this.invalid_duplicate && this.invalid_pose && this.invalid_axis
                && this.invalid_rotation,
                "duplicate, incomplete, invalid-axis, and reflected-rotation inputs fail visibly"),
            t_true(query_ok(left) && query_ok(right) && query_ok(rear),
                "pattern and mirror instances remain queryable after composition"),
            t_true(vector_close(left.world_position, [-2.0, 0.0, 0.0])
                && vector_close(right.world_position, [2.0, 0.0, 0.0])
                && vector_close(rear.world_position, [-1.0, 0.0, 1.0]),
                "instances preserve their explicit local poses and mirrored axis"),
        ], "ASSEMBLY PATTERN BUILDER", "ASSEMBLY_PATTERN_BUILDER");
        return;
    }

    let template = #{
        type_name: "Xform",
        asset: ASSET,
        api_schemas: [],
        attributes: [],
    };
    let placements = [
        #{ name: "Left", translation: [-2.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0] },
        #{ name: "Right", translation: [2.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0] },
    ];
    let pattern = assembly_builder::referenced_instance_pattern_plan(
        this.doc, "@root@", ASSEMBLY, template, placements,
    );
    let repeated = assembly_builder::referenced_instance_pattern_plan(
        this.doc, "@root@", ASSEMBLY, template, placements,
    );
    let duplicate = assembly_builder::referenced_instance_pattern_plan(
        this.doc, "@root@", ASSEMBLY, template,
        [
            #{ name: "Left", translation: [-2.0, 0.0, 0.0] },
            #{ name: "Left", translation: [2.0, 0.0, 0.0] },
        ],
    );
    let invalid_pose = assembly_builder::referenced_instance_pattern_plan(
        this.doc, "@root@", ASSEMBLY, template, [#{ name: "NoPose" }],
    );
    let mirror = assembly_builder::referenced_instance_mirror_plan(
        this.doc, "@root@", ASSEMBLY, template, "Rear", "X",
        [1.0, 0.0, 1.0], [0.0, 0.0, 0.0], (),
    );
    let invalid_axis = assembly_builder::referenced_instance_mirror_plan(
        this.doc, "@root@", ASSEMBLY, template, "BadAxis", "Q",
        [1.0, 0.0, 0.0], (), (),
    );
    let invalid_rotation = assembly_builder::referenced_instance_mirror_plan(
        this.doc, "@root@", ASSEMBLY, template, "BadRotation", "X",
        [1.0, 0.0, 0.0], [0.0, 15.0, 0.0], (),
    );
    if !pattern.ok || !repeated.ok || !mirror.ok || duplicate.ok || invalid_pose.ok
        || invalid_axis.ok || invalid_rotation.ok {
        fail(this, "pattern/mirror plans did not produce the expected valid and invalid results");
        return;
    }
    this.pattern_paths = pattern.paths;
    this.pattern_ops = pattern.ops.len();
    this.mirror_ops = mirror.ops.len();
    this.deterministic = pattern.paths == repeated.paths
        && pattern.placement_names == repeated.placement_names
        && pattern.ops.len() == repeated.ops.len();
    this.invalid_duplicate = duplicate.error.contains("duplicated");
    this.invalid_pose = invalid_pose.error.contains("explicit local pose");
    this.invalid_axis = invalid_axis.error.contains("mirror axis");
    this.invalid_rotation = invalid_rotation.error.contains("zero source rotation");

    let ops = [];
    for op in pattern.ops { ops.push(op); }
    for op in mirror.ops { ops.push(op); }
    let proposal = assembly_edit::propose(
        this.doc, #{ Assembly: () }, "Place repeated and mirrored references",
        ops, current.generation,
    );
    if !query_ok(proposal) {
        fail(this, "pattern/mirror proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals == () || review.proposals.len() != 1
        || review.proposals[0].diagnostics.len() != 0 {
        fail(this, "pattern/mirror proposal was not visible for clean review");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "pattern/mirror proposal could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = current.generation;
    this.phase = 1;
}
