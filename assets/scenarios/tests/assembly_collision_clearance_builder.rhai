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
const MISSING = ASSEMBLY + "/MissingBody";

fn query_ok(record) {
    record != () && record.ok != false
}

fn shape_bounds_match(doc, parent) {
    let path = parent + "/Shape";
    let single = query("QueryUsdPrim", #{doc_id: doc, path: path,
        attrs: [], collision_bounds: true, geometry_bounds: true});
    let batch = query("QueryUsdPrims", #{doc_id: doc, paths: [parent, path],
        attrs: [], collision_bounds: true, geometry_bounds: true});
    if !query_ok(single) || !query_ok(batch) || batch.records.len() != 2
        || single.collision_bounds == () || single.geometry_bounds == () {
        return false;
    }
    let collision = single.collision_bounds;
    let geometry = single.geometry_bounds;
    let aggregate = batch.records[0].collision_bounds;
    let child = batch.records[1].collision_bounds;
    if aggregate == () || child == () || collision.frame != "canonical_stage"
        || geometry.frame != "canonical_stage" || child.frame != "canonical_stage" {
        return false;
    }
    for axis in 0..3 {
        if (collision.min[axis] - geometry.min[axis]).abs() > 1.0e-12
            || (collision.max[axis] - geometry.max[axis]).abs() > 1.0e-12
            || (collision.min[axis] - aggregate.min[axis]).abs() > 1.0e-12
            || (collision.max[axis] - aggregate.max[axis]).abs() > 1.0e-12
            || (collision.min[axis] - child.min[axis]).abs() > 1.0e-12
            || (collision.max[axis] - child.max[axis]).abs() > 1.0e-12 {
            return false;
        }
    }
    true
}

fn cooked_cube_frame_matches(doc,path,translation,angle_deg) {
    let record=query("QueryUsdPrim",#{doc_id:doc,path:path,collision_geometry:true});
    if !query_ok(record) || record.collision_geometry==() {return false;}
    let shape=record.collision_geometry;
    if shape.shape!="convex_hull" || shape.frame!="canonical_stage"
        || shape.vertices==() || shape.vertices.len()!=8 || shape.local_to_stage==() {return false;}
    let pose=shape.local_to_stage;
    if pose.translation_m==() || pose.rotation_wxyz==() {return false;}
    let origin=vec3_from(pose.translation_m);let r=pose.rotation_wxyz;
    let q=quat_from([r[1],r[2],r[3],r[0]]);let inverse=quat_inverse(q);
    if vlen(origin-vec3_from(translation))>1.0e-12
        || vlen(qrot(q,vec3(1.0,0.0,0.0))-qrot(quat_from_euler_xyz_deg(vec3(0.0,0.0,angle_deg)),vec3(1.0,0.0,0.0)))>1.0e-12 {return false;}
    let lo=[1.0e30,1.0e30,1.0e30];let hi=[-1.0e30,-1.0e30,-1.0e30];
    for vertex in shape.vertices {
        let p=vec3_array(qrot(inverse,vec3_from(vertex)-origin));
        for axis in 0..3 {lo[axis]=min(lo[axis],p[axis]);hi[axis]=max(hi[axis],p[axis]);}
    }
    for axis in 0..3 {if abs(lo[axis]+1.0)>1.0e-12 || abs(hi[axis]-1.0)>1.0e-12 {return false;}}
    true
}

fn finish(me, checks) {
    if !me.done {
        me.done = true;
        for check in checks {
            if check != "" {
                report_verdict(checks, "ASSEMBLY COLLISION CLEARANCE BUILDER",
                    "ASSEMBLY_COLLISION_CLEARANCE_BUILDER");
                return;
            }
        }
        // A malformed annotation is inert until explicitly made physical.
        // Require the collider owner to reject it after the positive checks.
        print("Collision clearance checks completed: " + checks.len());
        expect_runtime_fault("usd-avian-collider-invalid");
        cmd("ApplyUsdOp", #{doc_id: me.doc, op: #{SetAttribute: #{
            edit_target: "@runtime@", path: MALFORMED + "/Shape",
            name: "purpose", type_name: "token", value: "\"default\"",
        }}});
    }
}

fn fail(me, message) {
    finish(me, [message]);
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return (); }
    for item in docs.open_documents {
        if item.kind == "usd" {
            let root = assembly_edit::inspect(item.doc_id, ROOT);
            if query_ok(root) && root.prim != () && root.prim.exists == true {
                return item.doc_id;
            }
        }
    }
    ()
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments", "QueryUsdPrim", "InspectUsdDocument", "InspectUsdEditSession", "ResolveUsdTarget", "QueryUsdPrims"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.doc = ();
    this.elapsed = 0.0;
    this.done = false;
    this.phase = 0;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc == () {
        this.doc = find_document();
        if this.doc == () {
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
            doc_id: this.doc,
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
            t_true(this.child_bounds_ok == true,
                "single and batch child collision bounds retain ancestor translation"),
            t_true(this.rotated_child_bounds_ok == true,
                "child and parent collision bounds agree through ancestor rotation"),
            t_true(this.cooked_cube_frame_ok == true,
                "world hull vertices expose their native translated cooking frame"),
            t_true(this.rotated_cooked_cube_frame_ok == true,
                "native cooking-frame inversion recovers a rotated hull's local dimensions"),
            t_true(this.center_alignment_ok == true,
                "collision-envelope centre alignment returns the expected local placement"),
            t_true(this.edge_alignment_ok == true,
                "collision-envelope edge alignment preserves the requested gap"),
            t_true(this.invalid_axis_rejected == true,
                "collision-envelope alignment rejects invalid axes"),
            t_true(this.invalid_edge_rejected == true,
                "collision-envelope alignment rejects invalid edge signs"),
            t_true(this.missing_alignment_rejected == true,
                "collision-envelope alignment rejects missing bodies"),
            t_true(this.alignment_rotated_rejected == true,
                "collision-envelope alignment rejects rotated body frames"),
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
                "guide annotation must not supply physical collision bounds"),
            t_true(this.duplicate_rejected == true,
                "duplicate blocker paths are rejected"),
        ]);
        this.done = true;
        return;
    }

    let moving_record = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: MOVING,
        collision_bounds: true,
    });
    let blocker_record = query("QueryUsdPrim", #{
        doc_id: this.doc,
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
    this.child_bounds_ok = shape_bounds_match(this.doc, MOVING);
    this.rotated_child_bounds_ok = shape_bounds_match(this.doc, ROTATED);
    this.cooked_cube_frame_ok=cooked_cube_frame_matches(this.doc,BLOCKER+"/Left",[-1.0,0.0,0.0],0.0);
    this.rotated_cooked_cube_frame_ok=cooked_cube_frame_matches(this.doc,ROTATED+"/Shape",[0.0,-4.0,0.0],15.0);

    let center_alignment = assembly_builder::align_collision_centers_plan(
        this.doc, "@root@", MOVING, BLOCKER, "X", 0.0,
    );
    let edge_alignment = assembly_builder::align_collision_edges_plan(
        this.doc, "@root@", MOVING, BLOCKER, "X", -1, 1, 0.25,
    );
    let invalid_axis = assembly_builder::align_collision_centers_plan(
        this.doc, "@root@", MOVING, BLOCKER, "W", 0.0,
    );
    let invalid_edge = assembly_builder::align_collision_edges_plan(
        this.doc, "@root@", MOVING, BLOCKER, "X", 0, 1, 0.25,
    );
    let missing_alignment = assembly_builder::align_collision_centers_plan(
        this.doc, "@root@", MOVING, MISSING, "X", 0.0,
    );
    let rotated_alignment = assembly_builder::align_collision_centers_plan(
        this.doc, "@root@", MOVING, ROTATED, "X", 0.0,
    );
    if !center_alignment.ok || !edge_alignment.ok || invalid_axis.ok
        || invalid_edge.ok || missing_alignment.ok || rotated_alignment.ok {
        fail(this, "collision-envelope alignment did not enforce its explicit input contract");
        return;
    }
    this.center_alignment_ok = center_alignment.kind == "collision_center_alignment"
        && center_alignment.ops.len() == 2
        && center_alignment.ops[0].SetTranslate.value[0].abs() <= 0.001;
    this.edge_alignment_ok = edge_alignment.kind == "collision_edge_alignment"
        && edge_alignment.ops.len() == 2
        && (edge_alignment.ops[0].SetTranslate.value[0] - 2.75).abs() <= 0.001;
    this.invalid_axis_rejected = invalid_axis.error.contains("axis");
    this.invalid_edge_rejected = invalid_edge.error.contains("moving_edge");
    this.missing_alignment_rejected = missing_alignment.error.contains("composed prim is unavailable");
    this.alignment_rotated_rejected = rotated_alignment.error.contains("translation-only");

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
    this.malformed_rejected = malformed.error.contains("no composed collision geometry");
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
