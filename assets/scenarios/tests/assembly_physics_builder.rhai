// TEST-ONLY. Production scene-test acceptance for the dynamic semantic
// assembly_builder tool library. This scenario checks the complete typed plan
// contract; the live physics projection is covered by assembly_joint_audit and
// the layout builder covers committed transform edits.

const ROOT = "/AssemblyPhysicsBuilderTest";
const HOST = ROOT + "/Host";
const BODY = ROOT + "/Panel";

fn query_ok(record) {
    record != () && record.ok != false
}

fn finish(me, checks) {
    if !me.done {
        me.done = true;
        report_verdict(checks, "ASSEMBLY PHYSICS BUILDER", "ASSEMBLY_PHYSICS_BUILDER");
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
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "physics builder fixture document was not discoverable"); }
            return;
        }
    }

    let document = assembly_edit::describe(this.doc);
    if !query_ok(document) || document.generation == () {
        fail(this, "physics builder fixture did not expose a document generation");
        return;
    }

    let body_plan = assembly_builder::movable_cube_plan(
        "@root@", ROOT, "Panel", 2.0, [0.0, 0.0, 0.0], [0.5, 0.75, 0.5],
        1.0, [1.0, 1.0, 0.0], (), [1.0, 0.2, 0.8], true,
    );
    let joint_plan = assembly_builder::hinge_plan(
        "@root@", ROOT, "PanelHinge", HOST, BODY, "Y",
        [0.0, 0.0, 0.0], [-1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0],
        0.0, 90.0, false,
    );
    let invalid_joint = assembly_builder::hinge_plan(
        "@root@", ROOT, "InvalidHinge", HOST, BODY, "Diagonal",
        [0.0, 0.0, 0.0], [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0],
        1.0, 0.0, false,
    );
    let invalid_placement = assembly_builder::place_plan(
        "@root@", BODY, [1.0, 2.0], (), (),
    );
    if !body_plan.ok || !joint_plan.ok || invalid_joint.ok || invalid_placement.ok {
        fail(this, "semantic assembly tools did not enforce their explicit input contract");
        return;
    }

    finish(this, [
        t_true(body_plan.kind == "movable_cube" && body_plan.path == BODY
            && body_plan.shape == BODY + "/Shape",
            "movable_cube_plan returns exact body and shape identities"),
        t_true(body_plan.ops.len() == 11,
            "movable_cube_plan composes body, placement, shape, mass, and collider facts"),
        t_true(body_plan.ops[1].SetApiSchemas.schemas.contains("PhysicsRigidBodyAPI")
            && body_plan.ops[1].SetApiSchemas.schemas.contains("PhysicsMassAPI"),
            "movable cube uses the standard body and mass schemas"),
        t_true(body_plan.ops[5].SetTranslate.path == BODY
            && body_plan.ops[5].SetTranslate.value == [1.0, 1.0, 0.0],
            "movable cube carries an explicit local placement"),
        t_true(body_plan.ops[9].SetApiSchemas.schemas.contains("PhysicsCollisionAPI")
            && body_plan.ops[10].SetAttribute.name == "physics:collisionEnabled",
            "movable cube carries an explicit collider contract"),
        t_true(joint_plan.ops.len() == 11
            && joint_plan.ops[0].AddPrim.type_name == "PhysicsRevoluteJoint",
            "hinge_plan returns a standard revolute joint plan"),
        t_true(joint_plan.ops[1].SetRelationship.targets[0] == HOST
            && joint_plan.ops[2].SetRelationship.targets[0] == BODY,
            "hinge plan carries both explicit body relationships"),
        t_true(joint_plan.ops[3].SetAttribute.value == "\"Y\""
            && joint_plan.ops[8].SetAttribute.value == "0.0"
            && joint_plan.ops[9].SetAttribute.value == "90.0",
            "hinge plan carries the axis and ordered limits"),
        t_true(invalid_joint.error.contains("axis") && invalid_placement.error.contains("three components"),
            "invalid semantic inputs fail before document mutation"),
    ]);
}
