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
//                      subtree_collider, host_body, jointed }
//       every prim applying `PhysicsRigidBodyAPI`. `host_body` is the nearest
//       ANCESTOR body ("" if none) and `jointed` says whether any joint names it.
//
//   facts.joints[]  #{ path, type, bodies[], missing[] }
//       every `Physics*Joint`. `missing` = targets that are not bodies.
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

fn lint_usd(facts) {
    let out = [];
    for f in nested_body_no_joint(facts) { out.push(f); }
    for f in joint_target_not_a_body(facts) { out.push(f); }
    for f in dynamic_body_no_collider(facts) { out.push(f); }
    for f in mass_outside_any_body(facts) { out.push(f); }
    out
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
    let out = [];
    for b in facts.bodies {
        if b.host_body == "" { continue; }
        if b.jointed { continue; }
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
