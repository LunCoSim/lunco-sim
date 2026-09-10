// TEST-ONLY. Whole observable acceptance stays in Rhai: the dynamic tool reads
// the composed mount contract, proposes its generic typed operations, and the
// scenario verifies both the authored result and topology preservation.

const ROOT = "/AssemblyMountFrameRealignTest";
const HOST = ROOT + "/Chassis";
const SOCKET = HOST + "/Interfaces/SocketBank/snap";
const BAD_SOCKET = HOST + "/Interfaces/SocketBank/bad_scale";
const PART = HOST + "/Payload";
const BAD_PART = HOST + "/BadPayload";
const JOINT = HOST + "/Constraints/PayloadMount";
const BAD_JOINT = HOST + "/Constraints/BadPayloadMount";

fn query_ok(record) {
    record != () && record.ok != false
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "ASSEMBLY MOUNT FRAME REALIGN", "ASSEMBLY_MOUNT_FRAME_REALIGN");
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
    actual != () && expected != () && (actual - expected).abs() <= epsilon
}

fn vector_close(actual, expected, epsilon) {
    if actual == () || expected == () || actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if !close_enough(actual[i], expected[i], epsilon) { return false; }
    }
    true
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
            if this.elapsed > 8.0 { fail(this, "mount-frame fixture document was not discoverable"); }
            return;
        }
    }

    let document = assembly_edit::describe(this.doc);
    if !query_ok(document) || document.generation == () {
        fail(this, "mount-frame fixture did not expose a document generation");
        return;
    }
    if this.phase == 1 && document.generation <= this.wait_generation {
        if this.elapsed > 8.0 { fail(this, "mount-frame realignment did not reach a new generation"); }
        return;
    }
    if this.phase == 1 {
        let part = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: PART,
            attrs: ["xformOp:translate", "xformOp:rotateXYZ"],
            rels: ["lunco:mount:frame", "lunco:mount:attachmentJoint"],
        });
        let joint = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: JOINT,
            attrs: ["physics:localPos0", "physics:localPos1"],
            rels: ["physics:body0", "physics:body1"],
        });
        let socket = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: SOCKET,
            rels: ["lunco:mount:part"],
        });
        this.done = true;
        report_verdict([
            t_true(this.plan_ops == 4, "realignment is one four-operation atomic plan"),
            t_true(vector_close(this.expected_translation,
                [0.4829629, 1.1026413, 0.0], 0.001),
                "nested frame math resolves the authored socket and plug positions"),
            t_true(vector_close(this.expected_rotation, [0.0, 0.0, 90.0], 0.001),
                "nested frame math resolves the authored 90 degree orientation"),
            t_true(vector_close(part.attrs["xformOp:translate"], this.expected_translation, 0.001),
                "part translation follows the nested socket-to-plug frame composition"),
            t_true(vector_close(part.attrs["xformOp:rotateXYZ"], this.expected_rotation, 0.001),
                "part rotation follows the composed rigid frame orientation"),
            t_true(vector_close(joint.attrs["physics:localPos0"], this.expected_translation, 0.001),
                "the exact linked joint anchor follows the part placement"),
            t_true(vector_close(joint.attrs["physics:localPos1"], [0.0, 0.0, 0.0], 0.001),
                "the part-side joint anchor remains the explicit origin"),
            t_true(query_ok(part) && query_ok(joint) && query_ok(socket),
                "realigned prims remain queryable"),
            t_true(part.relationships["lunco:mount:frame"] == this.frame_targets
                && part.relationships["lunco:mount:attachmentJoint"] == this.attachment_targets,
                "plug and exact attachment-joint topology is preserved"),
            t_true(joint.relationships["physics:body0"] == this.body0_targets
                && joint.relationships["physics:body1"] == this.body1_targets,
                "joint body topology is preserved"),
            t_true(socket.relationships["lunco:mount:part"] == this.socket_part_targets,
                "socket occupancy topology is preserved"),
            t_true(this.invalid_socket && this.invalid_joint && this.invalid_frame,
                "invalid socket identity, joint identity, and non-rigid frame all fail before commit"),
        ], "ASSEMBLY MOUNT FRAME REALIGN", "ASSEMBLY_MOUNT_FRAME_REALIGN");
        return;
    }

    let invalid_socket = assembly_builder::mount_frame_realignment_plan(
        this.doc, "@root@", HOST, HOST + "/Interfaces/SocketBank/not_advertised",
        PART, JOINT,
    );
    let invalid_joint = assembly_builder::mount_frame_realignment_plan(
        this.doc, "@root@", HOST, SOCKET, PART, BAD_JOINT,
    );
    let invalid_frame = assembly_builder::mount_frame_realignment_plan(
        this.doc, "@root@", HOST, BAD_SOCKET, BAD_PART, BAD_JOINT,
    );
    let plan = assembly_builder::mount_frame_realignment_plan(
        this.doc, "@root@", HOST, SOCKET, PART, JOINT,
    );
    if !plan.ok {
        fail(this, "valid mount-frame plan was rejected: " + plan.error);
        return;
    }
    let part_before = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: PART,
        rels: ["lunco:mount:frame", "lunco:mount:attachmentJoint"],
    });
    let joint_before = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: JOINT,
        rels: ["physics:body0", "physics:body1"],
    });
    let socket_before = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: SOCKET,
        rels: ["lunco:mount:part"],
    });
    if !query_ok(part_before) || !query_ok(joint_before) || !query_ok(socket_before)
        || invalid_socket.ok || invalid_joint.ok || invalid_frame.ok {
        fail(this, "mount-frame plan validation did not reject invalid identities and rigid frames");
        return;
    }
    this.plan_ops = plan.ops.len();
    this.expected_translation = plan.placement;
    this.expected_rotation = plan.rotation;
    this.invalid_socket = invalid_socket.error.contains("advertised");
    this.invalid_joint = invalid_joint.error.contains("attachment joint");
    this.invalid_frame = invalid_frame.error.contains("unit scale");
    this.frame_targets = part_before.relationships["lunco:mount:frame"];
    this.attachment_targets = part_before.relationships["lunco:mount:attachmentJoint"];
    this.body0_targets = joint_before.relationships["physics:body0"];
    this.body1_targets = joint_before.relationships["physics:body1"];
    this.socket_part_targets = socket_before.relationships["lunco:mount:part"];

    let proposal = assembly_edit::propose(
        this.doc, #{ Assembly: () }, "Realign existing mount frames",
        plan.ops, document.generation,
    );
    if !query_ok(proposal) {
        fail(this, "mount-frame proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals == () || review.proposals.len() != 1 {
        fail(this, "mount-frame proposal was not visible for review");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "mount-frame proposal could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = document.generation;
    this.phase = 1;
}
