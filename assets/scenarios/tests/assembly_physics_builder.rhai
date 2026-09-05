// TEST-ONLY. Production scene-test acceptance for the dynamic semantic
// assembly_builder tool library. This scenario checks the complete typed plan
// contract; the live physics projection is covered by assembly_joint_audit and
// the layout builder covers committed transform edits.

const ROOT = "/AssemblyPhysicsBuilderTest";
const HOST = ROOT + "/Host";
const BODY = ROOT + "/Panel";
const EXISTING = ROOT + "/ExistingPanel";
const EXISTING_COLLIDER = EXISTING + "/PanelSurface";

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
    let fixed_plan = assembly_edit::fixed_joint_plan(
        "@root@", ROOT, "PanelFixed", HOST, BODY,
        [0.0, 0.0, 0.0], [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], false,
    );
    let invalid_fixed = assembly_edit::fixed_joint_plan(
        "@root@", ROOT, "PanelFixed", HOST, HOST,
        [0.0, 0.0, 0.0], [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], false,
    );
    let existing_plan = assembly_builder::existing_rigid_body_plan(
        this.doc, "@root@", EXISTING, 3.0, [0.0, 0.0, 0.0],
        [0.75, 0.5, 0.25], [EXISTING_COLLIDER],
    );
    let invalid_existing = assembly_builder::existing_rigid_body_plan(
        this.doc, "@root@", EXISTING, 3.0, [0.0, 0.0],
        [0.75, 0.5, 0.25], [EXISTING_COLLIDER],
    );
    let invalid_collider = assembly_builder::existing_rigid_body_plan(
        this.doc, "@root@", EXISTING, 3.0, [0.0, 0.0, 0.0],
        [0.75, 0.5, 0.25], [ROOT + "/Host"],
    );
    let existing_hinge = assembly_builder::existing_hinged_body_plan(
        this.doc, "@root@", EXISTING, 3.0, [0.0, 0.0, 0.0],
        [0.75, 0.5, 0.25], [EXISTING_COLLIDER], ROOT, "ExistingPanelHinge",
        HOST, "Y", [0.0, 0.0, 0.0], [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], 0.0, 90.0, false,
    );
    let invalid_hinge_parent = assembly_builder::existing_hinged_body_plan(
        this.doc, "@root@", EXISTING, 3.0, [0.0, 0.0, 0.0],
        [0.75, 0.5, 0.25], [EXISTING_COLLIDER], ROOT + "/Host", "BadHinge",
        HOST, "Y", [0.0, 0.0, 0.0], [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], 0.0, 90.0, false,
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
    if !body_plan.ok || !joint_plan.ok || !fixed_plan.ok
        || !existing_plan.ok || !existing_hinge.ok
        || invalid_joint.ok || invalid_placement.ok
        || invalid_existing.ok || invalid_collider.ok || invalid_hinge_parent.ok
        || invalid_fixed.ok {
        fail(this, "semantic assembly tools did not enforce their explicit input contract");
        return;
    }

    let existing_proposal = assembly_edit::propose(
        this.doc, "Assembly", "Validate referenced body and hinge plan",
        existing_hinge.ops, document.generation,
    );
    let review = assembly_edit::review_session(this.doc);
    if existing_proposal.ok == false || review.proposals.len() != 1
        || review.proposals[0].diagnostics.len() != 0 {
        fail(this, "referenced body and hinge plan failed proposal validation");
        return;
    }
    assembly_edit::review_proposal(review.proposals[0].id, #{ Reject: () });

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
        t_true(fixed_plan.ops.len() == 8
            && fixed_plan.ops[0].AddPrim.type_name == "PhysicsFixedJoint"
            && fixed_plan.ops[1].SetRelationship.targets[0] == HOST
            && fixed_plan.ops[2].SetRelationship.targets[0] == BODY,
            "fixed_joint_plan returns a standard two-body fixed-joint plan"),
        t_true(fixed_plan.ops[3].SetAttribute.name == "physics:localPos0"
            && fixed_plan.ops[4].SetAttribute.name == "physics:localPos1"
            && fixed_plan.ops[5].SetAttribute.name == "physics:localRot0"
            && fixed_plan.ops[6].SetAttribute.name == "physics:localRot1"
            && fixed_plan.ops[7].SetAttribute.name == "physics:collisionEnabled",
            "fixed joint carries both explicit frame quaternions and collision policy"),
        t_true(invalid_joint.error.contains("axis") && invalid_placement.error.contains("three components"),
            "invalid semantic inputs fail before document mutation"),
        t_true(invalid_fixed.error.contains("distinct"),
            "fixed-joint plans reject identical body identities before mutation"),
        t_true(existing_plan.kind == "existing_rigid_body"
            && existing_plan.path == EXISTING
            && existing_plan.colliders == [EXISTING_COLLIDER]
            && existing_plan.ops.len() == 5,
            "existing referenced-style part gets one explicit body/mass plan"),
        t_true(existing_plan.ops[0].AddPrim != ()
            && existing_plan.ops[0].AddPrim.type_name == "Xform"
            && existing_plan.ops[1].SetApiSchemas.schemas.contains("PhysicsRigidBodyAPI")
            && existing_plan.ops[2].SetAttribute.name == "physics:mass"
            && existing_plan.ops[4].SetAttribute.name == "physics:diagonalInertia",
            "existing body promotion defines one local over without copying geometry"),
        t_true(invalid_existing.error.contains("three-component")
            && invalid_collider.error.contains("descendant"),
            "existing-body plans reject malformed mass data and non-descendant colliders"),
        t_true(existing_hinge.kind == "existing_hinged_body"
            && existing_hinge.path == EXISTING
            && existing_hinge.joint == ROOT + "/ExistingPanelHinge"
            && existing_hinge.ops.len() == 16
            && existing_hinge.ops[5].AddPrim != ()
            && existing_hinge.ops[5].AddPrim.type_name == "PhysicsRevoluteJoint",
            "existing hinged parts combine body and joint plans without duplicate geometry"),
        t_true(invalid_hinge_parent.error.contains("sibling"),
            "existing hinged plans reject a joint outside the body parent"),
    ]);
}
