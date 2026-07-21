//! USD physics **facts** for the linter — the rules live in authored policy.
//!
//! # The split
//!
//! Reading a composed stage is code: schemas, ancestry, joint relationships and
//! collider subtrees can only be answered by something that holds the stage. That
//! is this file, and it is tested as code.
//!
//! Deciding what is WRONG is policy: `assets/scripting/policy/lint_usd.rhai`,
//! entry `lint_usd(facts)`, reached through the `lint.usd` hook
//! ([`lunco_lint::run_lint`]). A rule can be added, tightened or silenced by
//! editing that script and re-registering the hook — against a running sim, with
//! no rebuild. That is deliberate: a rule you cannot try immediately is a rule
//! nobody writes.
//!
//! # Why this exists at all
//!
//! Every rover mounted four motors from `components/mobility/motor.usda`; the
//! asset applied `PhysicsRigidBodyAPI`; no joint attached them. Each became a
//! free body and fell out of the vehicle on the first physics step. The rovers
//! still drove, steered and made top speed — nothing failed, nothing logged. The
//! facts below are exactly what a rule needs to say so.
//!
//! # The fact table
//!
//! Only physics-relevant prims are described — bodies and joints — so a
//! 4000-prim scene hands policy a few dozen entries:
//!
//! ```text
//! #{
//!   stage: "<identifier>",
//!   bodies: [ #{ path, type, kinematic, simulated, collider, subtree_collider,
//!                host_body, jointed } ],
//!   joints: [ #{ path, type, bodies: [path, …], missing: [path, …] } ],
//! }
//! ```
//!
//! `host_body` is the nearest ANCESTOR body (empty when there is none) and
//! `jointed` says whether any joint names this body — together they are the
//! nested-body question. `missing` lists joint targets that are not bodies.

use std::collections::HashSet;

use lunco_hooks::HookValue as H;
use lunco_lint::LintFinding;
use lunco_usd_bevy::{StageView, UsdRead};
use openusd::schemas::physics::tokens as ptok;
use openusd::sdf::Path as SdfPath;

/// The lint domain these facts belong to: hook `lint.usd`, policy
/// `assets/scripting/policy/lint_usd.rhai`.
pub const USD_LINT_DOMAIN: &str = "usd";

/// The nearest ancestor of `path` that is a body, if any.
fn host_body(bodies: &HashSet<String>, path: &str) -> Option<String> {
    let mut cur = SdfPath::new(path).ok()?.parent();
    while let Some(p) = cur {
        if p.is_abs_root() {
            return None;
        }
        let s = p.to_string();
        if bodies.contains(&s) {
            return Some(s);
        }
        cur = p.parent();
    }
    None
}

/// Every applied API schema on `prim`, by name — `UsdRead` only answers the
/// yes/no question, and a rule that asks about a schema this crate has never
/// heard of needs the list.
fn applied_schemas(reader: &StageView<'_>, prim: &SdfPath) -> Vec<String> {
    reader
        .stage()
        .prim(prim.clone())
        .api_schemas()
        .map(|v| v.iter().map(|s| s.as_str().to_string()).collect())
        .unwrap_or_default()
}

/// Whether `path` or any descendant carries `PhysicsCollisionAPI`.
///
/// `sorted` is every prim path in lexical order, so a subtree is one contiguous
/// run starting at `path + '/'` — binary search rather than a stage walk per body.
fn subtree_has_collider(reader: &StageView<'_>, sorted: &[String], path: &str) -> bool {
    let prefix = format!("{path}/");
    let start = sorted.partition_point(|s| s.as_str() < prefix.as_str());
    for s in &sorted[start..] {
        if !s.starts_with(&prefix) {
            break;
        }
        if let Ok(p) = SdfPath::new(s) {
            if reader.has_api_schema(&p, ptok::API_COLLISION) {
                return true;
            }
        }
    }
    false
}

/// Everything policy needs to judge a stage's physics authoring.
///
/// Pure — no ECS, no side effects — so it can be built from a live scene, from
/// `ValidateAsset`'s pre-flight compose, or from a test fixture, and all three
/// then get identical findings from identical rules.
pub fn physics_facts(reader: &StageView<'_>) -> H {
    let paths: Vec<SdfPath> = reader.prim_paths();

    let mut bodies: HashSet<String> = HashSet::new();
    let mut joint_paths: Vec<SdfPath> = Vec::new();
    for p in &paths {
        if reader.has_api_schema(p, ptok::API_RIGID_BODY) {
            bodies.insert(p.to_string());
        }
        let is_joint = reader
            .prim_type_name(p)
            .map(|t| t.starts_with("Physics") && t.ends_with("Joint"))
            .unwrap_or(false);
        if is_joint {
            joint_paths.push(p.clone());
        }
    }

    // Joint facts first: which bodies are attached, and which targets do not
    // name a body at all.
    let mut attached: HashSet<String> = HashSet::new();
    let mut joints: Vec<H> = Vec::new();
    for jp in &joint_paths {
        let mut targets: Vec<String> = Vec::new();
        for rel in ["physics:body0", "physics:body1"] {
            for t in reader.rel_targets(jp, rel) {
                let s = t.to_string();
                if !s.is_empty() {
                    targets.push(s);
                }
            }
        }
        let missing: Vec<String> =
            targets.iter().filter(|t| !bodies.contains(*t)).cloned().collect();
        attached.extend(targets.iter().cloned());
        joints.push(H::map([
            ("path", H::str(jp.to_string())),
            ("type", H::str(reader.prim_type_name(jp).unwrap_or_default())),
            ("bodies", H::Array(targets.into_iter().map(H::str).collect())),
            ("missing", H::Array(missing.into_iter().map(H::str).collect())),
        ]));
    }

    let mut sorted: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    sorted.sort();

    let mut body_facts: Vec<H> = Vec::new();
    for p in &paths {
        let path = p.to_string();
        if !bodies.contains(&path) {
            continue;
        }
        let own_collider = reader.has_api_schema(p, ptok::API_COLLISION);
        body_facts.push(H::map([
            ("path", H::str(path.clone())),
            ("type", H::str(reader.prim_type_name(p).unwrap_or_default())),
            (
                "kinematic",
                H::Bool(reader.scalar::<bool>(p, ptok::A_KINEMATIC_ENABLED).unwrap_or(false)),
            ),
            (
                "simulated",
                H::Bool(reader.scalar::<bool>(p, ptok::A_RIGID_BODY_ENABLED).unwrap_or(true)),
            ),
            ("collider", H::Bool(own_collider)),
            (
                "subtree_collider",
                H::Bool(own_collider || subtree_has_collider(reader, &sorted, &path)),
            ),
            ("host_body", H::str(host_body(&bodies, &path).unwrap_or_default())),
            ("jointed", H::Bool(attached.contains(&path))),
        ]));
    }

    // The GENERIC projection: every prim that applies any schema at all, with its
    // type, its parent and its applied-schema list. `bodies`/`joints` above are
    // pre-chewed answers to the questions we already know we ask; this is what
    // lets a NEW rule ask a NEW question — "PhysicsMassAPI on a prim inside no
    // body", "LunCoMotorAPI with no drivenWheel", "a collider outside every
    // body" — without a Rust change, which is the whole point of putting rules in
    // rhai. Bounded by schema'd prims (hundreds), not by prim count (thousands).
    let mut prims: Vec<H> = Vec::new();
    for p in &paths {
        let schemas = applied_schemas(reader, p);
        if schemas.is_empty() {
            continue;
        }
        let parent = p.parent().map(|x| x.to_string()).unwrap_or_default();
        prims.push(H::map([
            ("path", H::str(p.to_string())),
            ("type", H::str(reader.prim_type_name(p).unwrap_or_default())),
            ("parent", H::str(parent)),
            ("schemas", H::Array(schemas.into_iter().map(H::str).collect())),
        ]));
    }

    H::map([
        ("bodies", H::Array(body_facts)),
        ("joints", H::Array(joints)),
        ("prims", H::Array(prims)),
    ])
}

/// Gather the facts and ask `lint.usd` policy what is wrong with them.
///
/// Returns nothing when no policy is registered — an app without scripting lints
/// nothing rather than falling back to a second, compiled copy of the rules.
/// There is exactly ONE place a USD physics rule is written.
pub fn lint_stage(reader: &StageView<'_>) -> Vec<LintFinding> {
    lunco_lint::run_lint(USD_LINT_DOMAIN, physics_facts(reader))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy::compose_file_to_stage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Compose a fixture through the real composer, so facts are read off
    /// composed opinions and not off a parse of the text.
    fn facts(usda: &str) -> H {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join("lunco_usd_lint_facts");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!("fixture_{}.usda", N.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&f, usda).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");
        let view = StageView::new(&stage);
        physics_facts(&view)
    }

    fn entries(facts: &H, key: &str) -> Vec<H> {
        let H::Map(m) = facts else { panic!("facts is not a map: {facts:?}") };
        match m.iter().find(|(k, _)| k == key).map(|(_, v)| v) {
            Some(H::Array(a)) => a.clone(),
            other => panic!("facts.{key} is {other:?}"),
        }
    }

    fn field<'a>(item: &'a H, key: &str) -> &'a H {
        let H::Map(m) = item else { panic!("not a map: {item:?}") };
        m.iter().find(|(k, _)| k == key).map(|(_, v)| v).expect("field present")
    }

    fn body<'a>(facts: &'a [H], path: &str) -> &'a H {
        facts
            .iter()
            .find(|b| field(b, "path") == &H::str(path))
            .unwrap_or_else(|| panic!("no body fact for {path}"))
    }

    const ROVER_WITH_LOOSE_MOTOR: &str = "#usda 1.0\n\
        def Xform \"Rover\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n\
        {\n\
            def Cube \"Chassis\" ( prepend apiSchemas = [\"PhysicsCollisionAPI\"] ) {}\n\
            def Xform \"Motor_FL\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] ) {}\n\
            def Cylinder \"Wheel_FL\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsCollisionAPI\"] ) {}\n\
            def PhysicsRevoluteJoint \"Wheel_FL_Joint\"\n\
            {\n\
                rel physics:body0 = </Rover>\n\
                rel physics:body1 = </Rover/Wheel_FL>\n\
            }\n\
        }\n";

    /// The motor bug, as facts: inside a body, named by no joint. This pair of
    /// fields is what the `nested-body-no-joint` rule reads.
    #[test]
    fn a_mounted_body_reports_its_host_and_that_nothing_joints_it() {
        let f = facts(ROVER_WITH_LOOSE_MOTOR);
        let bodies = entries(&f, "bodies");
        let motor = body(&bodies, "/Rover/Motor_FL");
        assert_eq!(field(motor, "host_body"), &H::str("/Rover"));
        assert_eq!(field(motor, "jointed"), &H::Bool(false));
        assert_eq!(field(motor, "subtree_collider"), &H::Bool(false));
    }

    /// The SAME nesting with a joint is the normal wheel mount and must be
    /// distinguishable, or a rule would fire on every rover in the repository.
    #[test]
    fn a_jointed_nested_body_is_marked_jointed() {
        let f = facts(ROVER_WITH_LOOSE_MOTOR);
        let bodies = entries(&f, "bodies");
        let wheel = body(&bodies, "/Rover/Wheel_FL");
        assert_eq!(field(wheel, "host_body"), &H::str("/Rover"));
        assert_eq!(field(wheel, "jointed"), &H::Bool(true));
    }

    /// A body's collider may live on a CHILD — the compound case every vehicle
    /// uses — and the fact must follow the subtree, not the prim.
    #[test]
    fn subtree_collider_sees_a_child_collider() {
        let f = facts(ROVER_WITH_LOOSE_MOTOR);
        let bodies = entries(&f, "bodies");
        let rover = body(&bodies, "/Rover");
        assert_eq!(field(rover, "collider"), &H::Bool(false));
        assert_eq!(field(rover, "subtree_collider"), &H::Bool(true));
        assert_eq!(field(rover, "host_body"), &H::str(""));
    }

    /// A part with no body of its own produces no body fact at all — the shape
    /// every internal part should have.
    #[test]
    fn a_massy_part_without_a_body_is_not_in_the_table() {
        let f = facts(
            "#usda 1.0\n\
             def Xform \"Rover\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n\
             {\n\
                 def Cube \"Chassis\" ( prepend apiSchemas = [\"PhysicsCollisionAPI\"] ) {}\n\
                 def Xform \"Motor_FL\" ( prepend apiSchemas = [\"PhysicsMassAPI\"] )\n\
                 {\n\
                     float physics:mass = 8.0\n\
                 }\n\
             }\n",
        );
        let bodies = entries(&f, "bodies");
        assert_eq!(bodies.len(), 1, "only the rover is a body: {bodies:?}");
    }

    /// A joint target that names a non-body is reported as `missing`, which is
    /// what the `joint-target-not-a-body` rule reads.
    #[test]
    fn joint_targets_that_are_not_bodies_are_listed_as_missing() {
        let f = facts(
            "#usda 1.0\n\
             def Xform \"Rig\"\n\
             {\n\
                 def Cube \"A\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsCollisionAPI\"] ) {}\n\
                 def Cube \"B\" ( prepend apiSchemas = [\"PhysicsCollisionAPI\"] ) {}\n\
                 def PhysicsFixedJoint \"Weld\"\n\
                 {\n\
                     rel physics:body0 = </Rig/A>\n\
                     rel physics:body1 = </Rig/B>\n\
                 }\n\
             }\n",
        );
        let joints = entries(&f, "joints");
        assert_eq!(joints.len(), 1);
        assert_eq!(
            field(&joints[0], "missing"),
            &H::Array(vec![H::str("/Rig/B")]),
            "B applies no PhysicsRigidBodyAPI"
        );
    }

    /// Kinematic and disabled bodies are flagged as such: a rule about "will
    /// fall out of the world" must be able to exclude the things that cannot.
    #[test]
    fn kinematic_and_disabled_bodies_are_flagged() {
        let f = facts(
            "#usda 1.0\n\
             def Xform \"Rig\"\n\
             {\n\
                 def Cube \"Anchor\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n\
                 {\n\
                     bool physics:kinematicEnabled = true\n\
                 }\n\
                 def Cube \"Prop\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n\
                 {\n\
                     bool physics:rigidBodyEnabled = false\n\
                 }\n\
             }\n",
        );
        let bodies = entries(&f, "bodies");
        assert_eq!(field(body(&bodies, "/Rig/Anchor"), "kinematic"), &H::Bool(true));
        assert_eq!(field(body(&bodies, "/Rig/Prop"), "simulated"), &H::Bool(false));
    }
}
