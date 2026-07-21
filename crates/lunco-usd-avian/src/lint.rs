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
//!                host_body, jointed, collider_min, collider_max } ],
//!   joints: [ #{ path, type, bodies: [path, …], missing: [path, …] } ],
//! }
//! ```
//!
//! `host_body` is the nearest ANCESTOR body (empty when there is none) and
//! `jointed` says whether any joint names this body — together they are the
//! nested-body question. `missing` lists joint targets that are not bodies.
//!
//! # Topology is not enough
//!
//! Everything above except the last two fields is TOPOLOGY — schemas, ancestry,
//! joint targets — and topology cannot answer the question that actually breaks
//! mechanisms: which part reaches the ground FIRST. A landing leg can apply every
//! right schema, name every real body, validate clean, and still ground its strut
//! instead of its footpad, at which point the spring leaves the load path and
//! reads 0 N at 0 stroke while carrying the vehicle. Nothing logs it; the vehicle
//! sits level at a plausible height.
//!
//! `collider_min` / `collider_max` are the world-space bounds of everything a body
//! can touch the world with — the union over its collider subtree, taken through
//! the composed transform, so a raked strut measures where its corner actually
//! hangs. `[]` when the subtree states no bounds, which a rule must read as
//! UNKNOWN and never as zero.

use std::collections::HashSet;

use bevy::math::Vec3;
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

/// Whether `s` lies inside a body nested under `root` — i.e. belongs to someone
/// else's body rather than to `root`'s.
///
/// OWNERSHIP STOPS AT A BODY BOUNDARY, and it is the same rule the loader applies
/// when it folds child colliders into a compound shape. A foot mounted on a leg is
/// the leg's neighbour, not the leg's geometry: counting it as the leg's would
/// make a leg look like it reaches as low as its own foot, which is exactly the
/// question `sprung-foot-not-lowest` asks.
fn inside_nested_body(bodies: &HashSet<String>, root: &str, s: &str) -> bool {
    for b in bodies {
        if b == root || !b.starts_with(&format!("{root}/")) {
            continue;
        }
        if s == b || s.starts_with(&format!("{b}/")) {
            return true;
        }
    }
    false
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

/// A gprim's own bounds in its LOCAL frame, before any transform.
///
/// The authored `extent` wins when present — it is what USD itself treats as a
/// boundable's bounds. Otherwise the size is derived from the gprim's defining
/// attributes, using USD's schema defaults for anything unauthored, so a
/// hand-written `def Cube "X" {}` measures the same here as it renders.
///
/// `None` for a prim with no bounds we can state honestly — a `Mesh` with no
/// `extent`, or a type this does not know. A rule must treat that as "unknown",
/// never as "zero-sized", which is why it is an Option rather than a default.
fn local_bounds(reader: &StageView<'_>, p: &SdfPath) -> Option<(Vec3, Vec3)> {
    if let Some(e) = reader.value::<Vec<[f32; 3]>>(p, "extent") {
        if e.len() == 2 {
            return Some((Vec3::from(e[0]), Vec3::from(e[1])));
        }
    }
    let f = |name: &str, default: f64| reader.value::<f64>(p, name).unwrap_or(default) as f32;
    // `uniform token axis` names the axis a Cylinder/Cone/Capsule is built along.
    let along = |half_axis: f32, half_radial: f32| -> Vec3 {
        match reader.value_str(p, "axis").as_deref().unwrap_or("Z") {
            "X" => Vec3::new(half_axis, half_radial, half_radial),
            "Y" => Vec3::new(half_radial, half_axis, half_radial),
            _ => Vec3::new(half_radial, half_radial, half_axis),
        }
    };
    let half = match reader.prim_type_name(p)?.as_str() {
        "Cube" => Vec3::splat(f("size", 2.0) / 2.0),
        "Sphere" => Vec3::splat(f("radius", 1.0)),
        "Cylinder" => along(f("height", 2.0) / 2.0, f("radius", 1.0)),
        "Cone" => along(f("height", 2.0) / 2.0, f("radius", 1.0)),
        // A capsule's hemispherical caps stand proud of its cylinder by `radius`.
        "Capsule" => {
            let r = f("radius", 0.5);
            along(f("height", 1.0) / 2.0 + r, r)
        }
        _ => return None,
    };
    Some((-half, half))
}

/// A gprim's axis-aligned bounds in WORLD space.
///
/// The eight local corners are carried through the composed transform and
/// re-bounded, so a rotated or non-uniformly scaled part measures where it
/// actually sits — a landing strut raked 25° is exactly the case that matters,
/// and taking its local box as world would understate how low its corner hangs.
fn world_aabb(reader: &StageView<'_>, p: &SdfPath) -> Option<(Vec3, Vec3)> {
    let (lo, hi) = local_bounds(reader, p)?;
    let t = crate::world_transform(reader, p);
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for i in 0..8 {
        let c = Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        let w = t.transform_point(c);
        min = min.min(w);
        max = max.max(w);
    }
    Some((min, max))
}

/// The union of every collider's world bounds in `path`'s subtree (itself
/// included) — what the body can actually touch the world with.
fn collider_world_aabb(
    reader: &StageView<'_>,
    sorted: &[String],
    bodies: &HashSet<String>,
    path: &str,
) -> Option<(Vec3, Vec3)> {
    let prefix = format!("{path}/");
    let start = sorted.partition_point(|s| s.as_str() < prefix.as_str());
    let subtree = std::iter::once(path.to_string())
        .chain(sorted[start..].iter().take_while(|s| s.starts_with(&prefix)).cloned());
    let mut acc: Option<(Vec3, Vec3)> = None;
    for s in subtree {
        let Ok(p) = SdfPath::new(&s) else { continue };
        if !reader.has_api_schema(&p, ptok::API_COLLISION) {
            continue;
        }
        if inside_nested_body(bodies, path, &s) {
            continue;
        }
        // Collision is opt-OUT: the API applied with `physics:collisionEnabled = 0`
        // is geometry the solver never sees, so it cannot be what grounds a leg.
        if reader.value::<bool>(&p, "physics:collisionEnabled") == Some(false) {
            continue;
        }
        let Some((lo, hi)) = world_aabb(reader, &p) else { continue };
        acc = Some(match acc {
            None => (lo, hi),
            Some((a, b)) => (a.min(lo), b.max(hi)),
        });
    }
    acc
}

/// A world point as `[x, y, z]` for policy, or `[]` when there is none to state.
fn vec3_h(v: Option<Vec3>) -> H {
    match v {
        Some(v) => H::Array(vec![
            H::Float(v.x as f64),
            H::Float(v.y as f64),
            H::Float(v.z as f64),
        ]),
        None => H::Array(Vec::new()),
    }
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

    // Authored never-collide pairs. The rel is a promise the loader keeps at
    // RUNTIME, where a target that never spawns is a warning 10 seconds into a
    // run; here it is decidable from the stage alone. `owner` is the body each
    // end resolves to, because a collider under a body folds into that body's
    // compound and the pair is between BODIES — which is also how a pair that
    // names two shapes of one body comes to be inert.
    let known: HashSet<String> = paths.iter().map(|p| p.to_string()).collect();
    let mut filtered_pairs: Vec<H> = Vec::new();
    for p in &paths {
        if !reader.has_api_schema(p, ptok::API_FILTERED_PAIRS) {
            continue;
        }
        let path = p.to_string();
        let owner_of = |s: &str| {
            if bodies.contains(s) {
                s.to_string()
            } else {
                host_body(&bodies, s).unwrap_or_default()
            }
        };
        let targets: Vec<String> = reader
            .rel_targets(p, ptok::A_FILTERED_PAIRS)
            .into_iter()
            .map(|t| t.to_string())
            .filter(|t| !t.is_empty())
            .collect();
        let missing: Vec<String> =
            targets.iter().filter(|t| !known.contains(t)).cloned().collect();
        let owners: Vec<String> = targets.iter().map(|t| owner_of(t)).collect();
        filtered_pairs.push(H::map([
            ("path", H::str(path.clone())),
            ("owner", H::str(owner_of(&path))),
            ("targets", H::Array(targets.into_iter().map(H::str).collect())),
            ("target_owners", H::Array(owners.into_iter().map(H::str).collect())),
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
        let aabb = collider_world_aabb(reader, &sorted, &bodies, &path);
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
            // WHERE the body can touch the world, not just whether it can. Every
            // other fact here is topological — schemas, ancestry, joint targets —
            // and topology cannot answer the question that actually breaks
            // mechanisms: which part reaches the ground FIRST. `[]` when nothing
            // in the subtree has statable bounds, so a rule can tell "no collider"
            // from "collider of unknown size" instead of reading both as zero.
            // A shaping transform on the BODY prim itself. Harmless for a lone
            // test rig; a design fault on anything that hosts a part, because a
            // child cannot be placed in a frame that stretches it.
            (
                "scale_nonuniform",
                H::Bool(
                    reader
                        .value_vec3(p, "xformOp:scale")
                        .is_some_and(|v| v[0] != v[1] || v[1] != v[2]),
                ),
            ),
            ("collider_min", vec3_h(aabb.map(|b| b.0))),
            ("collider_max", vec3_h(aabb.map(|b| b.1))),
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
        ("filtered_pairs", H::Array(filtered_pairs)),
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

    /// A RAKED LEG, measured. This is the geometry the descent lander flies and
    /// the arithmetic a clearance rule stands on, so it is pinned here rather than
    /// trusted: a 0.15 x 7.05 x 0.15 strut raked 25° about Z, and the footpad that
    /// has to be the thing which reaches the ground.
    ///
    /// The strut's LOCAL box is only 0.075 deep, but rotated its bottom corner
    /// hangs 0.075*sin25 = 0.032 m below its tip — which is exactly why bounds are
    /// taken in world space over the eight transformed corners. Take the local box
    /// as world and the corner disappears, along with the bug.
    const RAKED_LEG: &str = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Lander" (prepend apiSchemas = ["PhysicsRigidBodyAPI"])
{
    def Cube "Leg" (prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"])
    {
        double size = 1.0
        double3 xformOp:translate = (4.009, -1.807, 0)
        double3 xformOp:rotateXYZ = (0, 0, 25.0)
        double3 xformOp:scale = (0.15, 7.05, 0.15)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]
    }
    def Cylinder "Pad" (prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"])
    {
        uniform token axis = "Y"
        double radius = 0.4
        double height = 0.3
        double3 xformOp:translate = (5.5634, -5.1359, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
"#;

    fn low_y(item: &H) -> f32 {
        match field(item, "collider_min") {
            H::Array(v) if v.len() == 3 => match v[1] {
                H::Float(y) => y as f32,
                _ => panic!("collider_min.y is not a float"),
            },
            other => panic!("no collider bounds: {other:?}"),
        }
    }

    #[test]
    fn a_raked_struts_bounds_include_the_corner_its_rotation_swings_down() {
        let f = facts(RAKED_LEG);
        let bodies = entries(&f, "bodies");
        let leg = low_y(body(&bodies, "/Lander/Leg"));
        let pad = low_y(body(&bodies, "/Lander/Pad"));

        // centre_y - (half_thickness*sin25 + half_length*cos25)
        let expected = -1.807 - (0.075 * 25f32.to_radians().sin() + 3.525 * 25f32.to_radians().cos());
        assert!((leg - expected).abs() < 1e-3, "strut low point {leg}, expected {expected}");

        // The pad's own bottom face — a cylinder is centred on its origin.
        assert!((pad - (-5.1359 - 0.15)).abs() < 1e-3, "pad low point {pad}");

        // And the fact these two numbers exist to state: the FOOT reaches lower.
        assert!(pad < leg, "pad {pad} must reach below the strut corner {leg}");
    }

    /// A body whose subtree has no collider states no bounds — `[]`, so a rule can
    /// tell "nothing to touch the world with" from "a collider of unknown size".
    /// Reading either as zero would put a phantom part at the origin.
    #[test]
    fn a_body_with_no_collider_states_no_bounds() {
        let f = facts(ROVER_WITH_LOOSE_MOTOR);
        let bodies = entries(&f, "bodies");
        let motor = body(&bodies, "/Rover/Motor_FL");
        assert_eq!(field(motor, "collider_min"), &H::Array(Vec::new()));
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
