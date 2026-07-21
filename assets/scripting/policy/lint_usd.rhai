// ── USD lint policy — the rules, in rhai ────────────────────────────────────
//
// Hook `lint.usd`, entry `lint_usd(facts)`. Registered at startup from this file
// (`lunco_scripting::register_builtin_policies`) and REPLACEABLE AT RUNTIME:
//
//     register_hook("lint.usd", "lint_usd", my_source)   // from any scenario/REPL
//     unregister_hook("lint.usd")                        // back to no USD rules
//
// so a rule can be written, tried and tightened against a running sim with no
// rebuild. That is the point of authoring rules here instead of in Rust: a rule
// you cannot try immediately is a rule nobody writes.
//
// ── WHAT A RULE MAY SEE ─────────────────────────────────────────────────────
//
// `facts` is a projection of the COMPOSED stage, built by
// `lunco-usd-avian/src/lint.rs`:
//
//   facts.bodies[]  #{ path, type, kinematic, simulated, collider,
//                      subtree_collider, host_body, jointed,
//                      collider_min, collider_max }
//       every prim applying `PhysicsRigidBodyAPI`. `host_body` is the nearest
//       ANCESTOR body ("" if none) and `jointed` says whether any joint names it.
//       `collider_min`/`collider_max` are WORLD-space bounds of everything the
//       body can touch the world with — `[x,y,z]`, or `[]` when unknown, which is
//       NOT the same as zero. They STOP AT A NESTED BODY: a leg's bounds do not
//       include the foot mounted on it, because that foot is its own body. Count
//       it and a leg looks like it reaches exactly as low as its own foot, which
//       is the question `sprung_foot_not_lowest` asks. They are the only geometric facts here, and they
//       exist because a mechanism can be topologically perfect and still wrong:
//       see `sprung_foot_not_lowest`.
//
//   facts.joints[]  #{ path, type, bodies[], missing[] }
//       every `Physics*Joint`. `missing` = targets that are not bodies.
//
//   facts.filtered_pairs[]  #{ path, owner, targets[], target_owners[], missing[] }
//       every prim applying `PhysicsFilteredPairsAPI`. `owner` is the BODY the
//       prim's collider belongs to (itself, or its nearest ancestor body) and
//       `target_owners` the same for each target — because the pair avian filters
//       is between bodies, not between shapes. `missing` = targets that are not
//       prims on this stage.
//
//   facts.prims[]   #{ path, type, parent, schemas[] }
//       EVERY prim applying any schema at all — the generic projection, so a new
//       rule about a schema nobody has thought about yet needs no Rust change.
//
// ── WHAT A RULE RETURNS ─────────────────────────────────────────────────────
//
//   #{ rule, severity, subject, message }
//
// `severity` is "error" | "warn" | "info". `subject` is the prim the author must
// open. `message` says what is wrong and then what to do — in that order, because
// the reader is deciding whether to act, not admiring the diagnosis.
//
// ── WHAT BELONGS HERE ───────────────────────────────────────────────────────
//
// A rule earns its place when the mistake is (a) always wrong, (b) invisible at
// runtime, and (c) decidable from the authored stage alone. Anything that already
// fails loudly on its own does not: a linter that repeats the log is noise, and
// noise is how a real finding gets scrolled past.

/// One finding, built FIELD BY FIELD.
///
/// Not a map literal: rhai caps EXPRESSION complexity, and a four-field literal
/// nested inside two `for` loops trips "Expression exceeds maximum complexity" at
/// COMPILE time — which does not break one rule, it takes the whole policy down
/// and leaves the linter silently registering nothing. Every rule goes through
/// here so that failure mode cannot come back.
fn finding(rule, severity, subject, message) {
    let f = #{};
    f.rule = rule;
    f.severity = severity;
    f.subject = subject;
    f.message = message;
    f
}

/// The applied schemas of `path`, from the generic prim projection — the escape
/// hatch that lets a rule ask about a schema no fact was pre-chewed for.
fn schemas_of(facts, path) {
    for p in facts.prims {
        if p.path == path { return p.schemas; }
    }
    []
}

fn lint_usd(facts) {
    let out = [];
    for f in nested_body_no_joint(facts) { out.push(f); }
    for f in joint_target_not_a_body(facts) { out.push(f); }
    for f in dynamic_body_no_collider(facts) { out.push(f); }
    for f in mass_outside_any_body(facts) { out.push(f); }
    for f in sprung_foot_not_lowest(facts) { out.push(f); }
    for f in body_is_its_own_scaled_mesh(facts) { out.push(f); }
    for f in filtered_pair_is_inert(facts) { out.push(f); }
    out
}

/// The lowest world Y of everything `path` can touch the world with, or `()` when
/// the body states no collider bounds. `()` means UNKNOWN and must not be compared
/// — a body with no collider is not a body whose foot is too high.
fn lowest_y(facts, path) {
    for b in facts.bodies {
        if b.path != path { continue; }
        if b.collider_min.len() != 3 { return (); }
        return b.collider_min[1];
    }
    ()
}

/// How far `path` reaches — its longest collider dimension, or `()` if unknown.
///
/// A proxy for the lever arm between a leg's pivot and its foot, which is what
/// turns an attitude change into vertical travel at the contact. Using it keeps
/// clearance rules SCALE-RELATIVE: the same rule has to serve a 7 m landing strut
/// and a 0.2 m suspension link, and any absolute margin is wrong for one of them.
fn reach_of(facts, path) {
    for b in facts.bodies {
        if b.path != path { continue; }
        if b.collider_min.len() != 3 { return (); }
        let r = b.collider_max[0] - b.collider_min[0];
        let dy = b.collider_max[1] - b.collider_min[1];
        let dz = b.collider_max[2] - b.collider_min[2];
        if dy > r { r = dy; }
        if dz > r { r = dz; }
        return r;
    }
    ()
}

/// A part with its own body, mounted inside another body, held by no joint.
///
/// THE ONE THAT SHIPPED. `components/mobility/motor.usda` applied
/// `PhysicsRigidBodyAPI`; four motors per rover were children of the chassis
/// body; no joint named them. Each was therefore a SEPARATE free body: they fell
/// through the hull on the first physics step and were left on the ground. Every
/// rover still drove, steered and made top speed, so no test and no log said a
/// word — the bug was found in a screenshot.
///
/// Why nesting alone is not the error: nesting-plus-joint is how this engine
/// mounts a wheel (`Wheel_FL` under the chassis + a `PhysicsRevoluteJoint`), and
/// the loader deliberately does not consult ancestry for that reason. The joint
/// is what attaches; the hierarchy is only namespace. Robotics says the same in
/// every dialect — URDF lumps a fixed-jointed link into its parent's inertia,
/// MJCF treats a jointless nested body as welded, and neither has any notion of a
/// link inside a link attached to nothing.
fn nested_body_no_joint(facts) {
    // Prims the ENGINE realizes rather than the joint graph. A wheel carrying
    // `PhysxVehicleWheelAPI` is one: in the `physical` drivetrain it gets a
    // revolute joint, and in `raycast` the spawner deliberately gives it neither
    // body nor collider and drives it from the suspension cast
    // (`lunco-mobility`, `sync_raycast_wheel_physics_pose`). Both are correct and
    // neither is "a part held by nothing", so the rule would otherwise fire on
    // every raycast rover we ship — and a rule that fires on everything is read
    // by no one. That wheels STAY ON is checked where it belongs, by simulating
    // it: `scenes/tests/parts_attached.usda`.
    let engine_realized = "PhysxVehicleWheelAPI";

    let out = [];
    for b in facts.bodies {
        if b.host_body == "" { continue; }
        if b.jointed { continue; }
        // A disabled body is unmoving collision geometry, not a free body: it
        // cannot fall out of anything, so it is not this rule's business.
        if !b.simulated { continue; }
        if schemas_of(facts, b.path).contains(engine_realized) { continue; }
        // Built by successive `+=`: rhai caps EXPRESSION complexity, and one long
        // concatenation chain fails to COMPILE — which would take the whole
        // policy down, not just this rule.
        let m = "applies PhysicsRigidBodyAPI inside the body <" + b.host_body + ">";
        m += " but no joint names it — it is a SEPARATE body held by nothing";
        m += " and will fall out of the vehicle.";
        m += " Either drop PhysicsRigidBodyAPI (an internal part is mass + geometry,";
        m += " and its colliders fold into the host body)";
        m += " or author a joint fixing it to <" + b.host_body + ">.";
        out.push(finding("nested-body-no-joint", "error", b.path, m));
    }
    out
}

/// A joint naming a prim that is not a body.
///
/// The joint is dropped at load and the mechanism silently becomes rigid (or
/// free). The symptom appears in the vehicle's behaviour, far from the typo.
fn joint_target_not_a_body(facts) {
    let out = [];
    for j in facts.joints {
        for m in j.missing {
            let msg = "names <" + m + "> as one of its bodies, but that prim applies";
            msg += " no PhysicsRigidBodyAPI (or does not exist) — the joint is dropped";
            msg += " at load and the mechanism is silently rigid.";
            out.push(finding("joint-target-not-a-body", "error", j.path, msg));
        }
    }
    out
}

/// A simulated, non-kinematic body with no collider anywhere in its subtree.
///
/// It cannot touch the world: it accelerates under gravity and leaves the scene.
/// A WARN, not an ERROR — a body held entirely by joints, or driven by a program,
/// is a legitimate if rare construction, and a linter that cries wolf is one
/// nobody reads.
fn dynamic_body_no_collider(facts) {
    let out = [];
    for b in facts.bodies {
        if !b.simulated || b.kinematic { continue; }
        if b.subtree_collider { continue; }
        let m = "is a dynamic body with no PhysicsCollisionAPI in its subtree —";
        m += " it cannot touch anything and will accelerate out of the scene";
        m += " unless a joint or a program holds it.";
        out.push(finding("dynamic-body-no-collider", "warn", b.path, m));
    }
    out
}

/// `PhysicsMassAPI` on a prim that is not a body and sits inside no body.
///
/// Authored mass that reaches nothing. Written against `facts.prims` — the
/// generic projection — rather than a bespoke fact, which is the pattern for
/// every rule after the first three: ask the stage, do not ask for new Rust.
fn mass_outside_any_body(facts) {
    // Path prefixes of every body, so "inside a body" is a string test rather
    // than a second projection.
    let body_paths = [];
    for b in facts.bodies { body_paths.push(b.path); }

    let out = [];
    for p in facts.prims {
        if !p.schemas.contains("PhysicsMassAPI") { continue; }
        if body_paths.contains(p.path) { continue; }
        let inside = false;
        for bp in body_paths {
            if p.path.starts_with(bp + "/") { inside = true; }
        }
        if inside { continue; }
        let m = "applies PhysicsMassAPI but is not a rigid body and sits inside none —";
        m += " the authored mass reaches no solver.";
        m += " Mount it under the body it belongs to, or give it a body and a joint.";
        out.push(finding("mass-outside-any-body", "warn", p.path, m));
    }
    out
}

/// A sprung leg whose own structure reaches the ground before its foot does.
///
/// THE ONE THAT COST A DAY. The descent lander's footpad cleared its strut's
/// bottom corner by 6.6 mm on an 11 m vehicle. Touchdown tipped the leg a fraction
/// of a degree, the strut's corner took the ground, and that rigid contact LATCHED:
/// the vehicle stood on its shins. Every spring read 0 N at 0 stroke while carrying
/// a lander, and the gear settled by bending the prismatic's angular lock ~2°
/// instead of sliding along it. Nothing failed and nothing logged — the vehicle sat
/// level, at a plausible height, at rest. It reads as a sign error in the joint and
/// is nothing of the kind.
///
/// A suspension has exactly ONE intended load path: foot -> spring -> chassis. Any
/// second way for the leg to reach the ground wins, because it is rigid and the
/// spring is not, and it latches on the first frame it grazes.
///
/// Why this is decidable here and nowhere else: it is a fact about composed
/// TRANSFORMS and collider EXTENTS, not about schemas — a leg can be perfectly
/// well-formed, validate clean, and still land on its shins. That is why
/// `collider_min` exists.
///
/// Scope is deliberately narrow: a prismatic carrying a linear drive IS a spring,
/// and the body that spring's member carries through a further joint is its FOOT.
/// Raycast wheels have no such joint and revolute-driven wheels are not prismatic,
/// so this cannot fire on a rover.
fn sprung_foot_not_lowest(facts) {
    let out = [];
    for j in facts.joints {
        if j.type != "PhysicsPrismaticJoint" { continue; }
        if !schemas_of(facts, j.path).contains("PhysicsDriveAPI:linear") { continue; }
        if j.bodies.len() < 2 { continue; }
        let strut = j.bodies[1];
        let sy = lowest_y(facts, strut);
        if sy == () { continue; }
        let reach = reach_of(facts, strut);
        if reach == () { continue; }
        for f in feet_of(facts, j.path, strut, sy, reach) { out.push(f); }
    }
    out
}

/// Every finding for the feet `strut` carries. Split out because rhai caps
/// expression complexity CUMULATIVELY over a function body: the same code inlined
/// as a second nested loop fails to COMPILE, which takes the whole policy down and
/// leaves the linter silently registering nothing.
fn feet_of(facts, jpath, strut, sy, reach) {
    let out = [];
    for k in facts.joints {
        if k.bodies.len() < 2 { continue; }
        if k.bodies[0] != strut { continue; }
        let foot = k.bodies[1];
        let fy = lowest_y(facts, foot);
        if fy == () { continue; }
        let r = clearance(jpath, strut, foot, sy, fy, reach);
        if r != () { out.push(r); }
    }
    out
}

/// Judge one strut/foot pair. `()` when the foot is properly proud of the strut.
fn clearance(jpath, strut, foot, sy, fy, reach) {
    if fy >= sy {
        let m = "carries <";
        m += foot;
        m += "> as its foot, but the sprung member <";
        m += strut;
        m += "> reaches LOWER (";
        m += sy;
        m += " vs ";
        m += fy;
        m += "). The strut grounds first and carries the vehicle rigidly:";
        m += " the spring never sees the landing, reads 0 N at 0 stroke, and the";
        m += " gear settles by bending this joint's angular lock instead of stroking.";
        m += " Extend the foot BELOW the strut's tip rather than centring it on the tip.";
        return finding("sprung-foot-not-lowest", "error", jpath, m);
    }
    // ONE DEGREE of attitude, in metres at this leg's own scale: sin(1°)*reach.
    // A clearance thinner than that is closed by a tip too small to see on a
    // screenshot, so it is not a margin — it is a coincidence. This is an ERROR
    // rather than a warning because it is the form the bug actually SHIPPED in:
    // the foot was lower, by 6.6 mm, and the gear still stood on its shins.
    let gap = sy - fy;
    let need = reach * 0.01745;
    if gap < need {
        let m = "clears its foot <";
        m += foot;
        m += "> by only ";
        m += gap;
        m += " m across a reach of ";
        m += reach;
        m += " m — under one degree of attitude (";
        m += need;
        m += " m). A tip that small puts the strut on the ground, the contact";
        m += " latches, and the spring leaves the load path: 0 N at 0 stroke while";
        m += " the vehicle stands on it, level and at a plausible height.";
        m += " Deepening the foot in place does NOT help — it demands a larger";
        m += " rotation to reach the ground, which brings the strut down faster.";
        m += " Move the foot BELOW the strut's tip.";
        return finding("sprung-foot-thin-clearance", "error", jpath, m);
    }
    ()
}

/// A rigid body prim that is also its own shaped geometry.
///
/// `UsdGeomCube` carries only a uniform `size`, so a rectangular part gets faked
/// with a non-uniform `xformOp:scale`. Put that on the BODY prim and the body's
/// frame is no longer usable: every child inherits the stretch, so nothing can be
/// mounted on the part without being deformed by it. Authors then place the
/// neighbouring part in VEHICLE coordinates instead — writing the same geometry a
/// second time, in a second frame, free to drift from the first. That is how a
/// landing strut and the footpad meant to carry it ended up 6.6 mm apart with
/// nothing relating the two numbers.
///
/// The fix is not to delete the scale but to MOVE it: make the body an `Xform`
/// frame and put the geometry in a child, where a shaping transform belongs.
/// Better still, use a primitive that carries real dimensions — `Cylinder`,
/// `Capsule`, `Sphere` have `radius`/`height` in metres and need no scale at all.
///
/// A WARN, not an error: a lone test rig with no children and nothing to clear is
/// harmed by none of this, and a rule that fails a build over a fixture is a rule
/// people switch off. The consequence that IS always wrong has its own rule —
/// `sprung-foot-not-lowest`.
fn body_is_its_own_scaled_mesh(facts) {
    let out = [];
    for b in facts.bodies {
        if !b.scale_nonuniform { continue; }
        let m = "is a rigid body carrying a non-uniform xformOp:scale, so its frame";
        m += " stretches anything placed in it and no part can be mounted on it.";
        m += " Make this prim an Xform frame and move the geometry (and the scale)";
        m += " into a child — or use a Cylinder/Capsule/Sphere, which carry real";
        m += " radius/height and need no scale.";
        out.push(finding("body-is-its-own-scaled-mesh", "warn", b.path, m));
    }
    out
}

/// An authored `physics:filteredPairs` that filters nothing.
///
/// Three ways for the rel to be inert, all of them silent at runtime for as long
/// as anyone watches:
///
///   names a prim that is not on the stage  a typo. The loader reports it after
///       600 physics ticks — ten seconds into a run nobody is reading — and the
///       pair collides the whole time.
///   names nothing at all                   the API applied with an empty rel.
///   names a shape of the SAME body         two colliders of one compound never
///       collide with each other anyway, so the pair reads as protection and is
///       not. The pair that needed authoring is still unauthored.
///
/// All decidable from the stage alone, and all invisible in a screenshot: the
/// symptom is a vehicle that pushes itself apart, which reads as bad tuning.
fn filtered_pair_is_inert(facts) {
    let out = [];
    for fp in facts.filtered_pairs {
        if fp.targets.is_empty() {
            let m = "applies PhysicsFilteredPairsAPI but names nothing in";
            m += " physics:filteredPairs — the pair it was meant to protect still";
            m += " collides. Name the prim it must not touch, or drop the schema.";
            out.push(finding("filtered-pair-empty", "warn", fp.path, m));
        }
        for t in fp.missing {
            let m = "filters against <" + t + ">, which is not a prim on this stage";
            m += " — the pair is never filtered and the parts collide. Check the";
            m += " path.";
            out.push(finding("filtered-pair-target-missing", "error", fp.path, m));
        }
        for i in 0..fp.targets.len() {
            let own = fp.target_owners[i];
            if own == "" || own != fp.owner { continue; }
            let m = "filters against <" + fp.targets[i] + ">, which belongs to the";
            m += " same body <" + own + "> — both are colliders of one compound";
            m += " shape, and a body never collides with itself. This filters";
            m += " nothing; the pair you meant is between two DIFFERENT bodies.";
            out.push(finding("filtered-pair-same-body", "warn", fp.path, m));
        }
    }
    out
}
