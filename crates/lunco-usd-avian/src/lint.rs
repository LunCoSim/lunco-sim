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
//! editing that script and re-registering the hook, then invoking the explicit
//! lint command against the selected composed stage — with no rebuild. That is
//! deliberate: a rule you cannot try immediately is a rule nobody writes.
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
//! Vehicle geometry is also described — only renderable gprims below an
//! assembly body — so policy can distinguish a real collider, an owner-level
//! wheel projector, and explicitly visual-only decoration without scanning
//! materials or controllers:
//!
//! ```text
//! #{
//!   stage: "<identifier>",
//!   bodies: [ #{ path, type, kinematic, simulated, collider, subtree_collider,
//!                host_body, jointed, collider_min, collider_max } ],
//!   joints: [ #{ path, type, bodies: [path, …], missing: [path, …] } ],
//!   vehicle_parts: [ #{ path, type, vehicle, body, purpose, collision_api,
//!                        collision_state, wheel_projector, visual_only,
//!                        shape_valid, contract } ],
//!   telemetry_declarations: [ #{ path, targets[], target_exists,
//!                                direct_surface, source_valid } ],
//!   prims: [ #{ path, type, parent, schemas[], attributes[],
//!                connected_attributes[], epoch_jd } ],
//! }
//! ```
//!
//! `host_body` is the nearest ANCESTOR body (empty when there is none) and
//! `jointed` says whether any joint holds this body — together they are the
//! nested-body question. `missing` lists joint targets that resolve to no body at
//! all; a target that is not itself a body but sits under one is NOT missing,
//! because that is how a mounted mechanism names its host (see the joint loop).
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
//! Drive facts use `realization = "derived"` when a force spring has no explicit
//! generalized inertia. That is not an error: the runtime asks Avian for the
//! computed mass/inertia after the body's collider tree and density are live.
//!
//! `collider_min` / `collider_max` are the world-space bounds of everything a body
//! can touch the world with — the union over its collider subtree, taken through
//! the composed transform, so a raked strut measures where its corner actually
//! hangs. `[]` when the subtree states no bounds, which a rule must read as
//! UNKNOWN and never as zero.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::MotorModel;
use bevy::math::Vec3;
use lunco_hooks::HookValue as H;
use lunco_usd_bevy::{StageView, UsdRead, program::ProgramGraph};
use openusd::schemas::physics::tokens as ptok;
use openusd::sdf::Path as SdfPath;
use openusd::usd::{Collection, PrimPredicate, compute_included_paths};

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
    let t = crate::world_transform(reader, p).ok()?;
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
    let subtree = std::iter::once(path.to_string()).chain(
        sorted[start..]
            .iter()
            .take_while(|s| s.starts_with(&prefix))
            .cloned(),
    );
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
        let Some((lo, hi)) = world_aabb(reader, &p) else {
            continue;
        };
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

/// Geometry types projected by the shared USD visual reader. Physics coverage
/// is about renderable vehicle parts, not every schema-bearing scope, material,
/// light, or controller prim.
fn is_vehicle_geometry_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "Cube"
            | "Sphere"
            | "Cylinder"
            | "Cone"
            | "Capsule"
            | "Plane"
            | "Mesh"
            | "BasisCurves"
            | "NurbsCurves"
            | "NurbsPatch"
    )
}

/// Find the nearest path in candidates, including path itself.
fn nearest_path_in_set(candidates: &HashSet<String>, path: &SdfPath) -> Option<String> {
    let mut cur = Some(path.clone());
    while let Some(p) = cur {
        if p.is_abs_root() {
            return None;
        }
        let value = p.to_string();
        if candidates.contains(&value) {
            return Some(value);
        }
        cur = p.parent();
    }
    None
}

/// Whether the runtime compound reader sees a proxy description for body.
/// The walk stops at nested bodies, exactly as collider ownership does, so a
/// child wheel or articulated link cannot make its host body's render mesh
/// look intentionally excluded from collision.
fn body_has_proxy(
    reader: &StageView<'_>,
    sorted: &[String],
    bodies: &HashSet<String>,
    body: &str,
) -> bool {
    let prefix = body.to_string() + "/";
    let start = sorted.partition_point(|s| s.as_str() < prefix.as_str());
    for s in &sorted[start..] {
        if !s.starts_with(&prefix) {
            break;
        }
        if inside_nested_body(bodies, body, s) {
            continue;
        }
        let Ok(p) = SdfPath::new(s) else { continue };
        if is_vehicle_geometry_type(&reader.prim_type_name(&p).unwrap_or_default())
            && lunco_usd_bevy::effective_purpose(reader, &p) == lunco_usd_bevy::Purpose::Proxy
        {
            return true;
        }
    }
    false
}

/// Composed collision coverage facts for every renderable part below a
/// vehicle assembly. The runtime has two legitimate owners beyond an ordinary
/// PhysicsCollisionAPI: a wheel projector and standard purpose metadata. All
/// other geometry must either have a usable collider or explicitly author its
/// visual-only status with physics:collisionEnabled = false or purpose.
fn vehicle_part_facts(
    reader: &StageView<'_>,
    paths: &[SdfPath],
    bodies: &HashSet<String>,
    vehicle_roots: &HashSet<String>,
    proxy_bodies: &HashSet<String>,
) -> Vec<H> {
    paths
        .iter()
        .filter_map(|path| {
            let type_name = reader.prim_type_name(path)?;
            if !is_vehicle_geometry_type(&type_name) {
                return None;
            }
            let vehicle = nearest_path_in_set(vehicle_roots, path)?;
            let body = nearest_path_in_set(bodies, path).unwrap_or_default();
            let collision_api = reader.has_api_schema(path, ptok::API_COLLISION);
            let wheel_projector = reader.has_api_schema(path, "PhysxVehicleWheelAPI");
            let collision_state = if collision_api {
                match super::read_authored_bool_or_default(
                    reader,
                    path,
                    ptok::A_COLLISION_ENABLED,
                    true,
                ) {
                    Ok(true) => "enabled",
                    Ok(false) => "disabled",
                    Err(()) => "malformed",
                }
            } else {
                match reader.boolean(path, ptok::A_COLLISION_ENABLED) {
                    Some(true) => "enabled_without_api",
                    Some(false) => "disabled",
                    None if reader.has_authored_attribute(path, ptok::A_COLLISION_ENABLED) => {
                        "malformed"
                    }
                    None => "missing",
                }
            };
            let purpose = lunco_usd_bevy::effective_purpose(reader, path);
            let render_excluded_by_proxy =
                purpose == lunco_usd_bevy::Purpose::Render && proxy_bodies.contains(&body);
            let visual_only = purpose == lunco_usd_bevy::Purpose::Guide
                || collision_state == "disabled"
                || render_excluded_by_proxy;
            let covered =
                wheel_projector || (collision_api && collision_state == "enabled" && !visual_only);
            let shape_valid = if covered && collision_api {
                matches!(super::build_collider_from_usd(reader, path), Ok(Some(_)))
            } else {
                true
            };
            let contract = if wheel_projector {
                "projector"
            } else if visual_only {
                "visual-only"
            } else if !collision_api {
                "missing-collider-api"
            } else if collision_state != "enabled" {
                "malformed-collision-enabled"
            } else if !shape_valid {
                "unsupported-collider"
            } else {
                "collider"
            };
            Some(H::map([
                ("path", H::str(path.to_string())),
                ("type", H::str(type_name)),
                ("vehicle", H::str(vehicle)),
                ("body", H::str(body)),
                (
                    "purpose",
                    H::str(match purpose {
                        lunco_usd_bevy::Purpose::Default => "default",
                        lunco_usd_bevy::Purpose::Render => "render",
                        lunco_usd_bevy::Purpose::Proxy => "proxy",
                        lunco_usd_bevy::Purpose::Guide => "guide",
                    }),
                ),
                ("collision_api", H::Bool(collision_api)),
                ("collision_state", H::str(collision_state)),
                ("wheel_projector", H::Bool(wheel_projector)),
                (
                    "render_excluded_by_proxy",
                    H::Bool(render_excluded_by_proxy),
                ),
                ("visual_only", H::Bool(visual_only)),
                ("shape_valid", H::Bool(shape_valid)),
                ("contract", H::str(contract)),
            ]))
        })
        .collect()
}

/// Generic USD facts for the authored telemetry contract. The runtime accepts
/// an omitted target only when the declaration prim itself publishes the
/// requested port; a metadata Scope must name its measured prim explicitly.
fn telemetry_declaration_facts(reader: &StageView<'_>, paths: &[SdfPath]) -> Vec<H> {
    let known: HashSet<String> = paths.iter().map(ToString::to_string).collect();
    paths
        .iter()
        .filter(|path| reader.boolean(path, "lunco:telemetry") == Some(true))
        .map(|path| {
            let targets: Vec<String> = reader
                .rel_targets(path, "lunco:telemetry:target")
                .into_iter()
                .map(|target| target.to_string())
                .collect();
            let port = reader.text(path, "lunco:telemetry:port");
            let reflect = reader.text(path, "lunco:telemetry:reflect");
            let direct_surface = port.as_deref().is_some_and(|port| {
                !port.is_empty()
                    && reader.attr_names(path).iter().any(|name| {
                        name == &format!("outputs:{port}") || name == &format!("inputs:{port}")
                    })
            });
            let source_valid = port.as_ref().is_some_and(|value| !value.is_empty())
                || reflect.as_ref().is_some_and(|value| !value.is_empty());
            let target_exists = targets.iter().all(|target| known.contains(target));
            H::map([
                ("path", H::str(path.to_string())),
                (
                    "targets",
                    H::Array(targets.into_iter().map(H::str).collect()),
                ),
                ("target_exists", H::Bool(target_exists)),
                ("direct_surface", H::Bool(direct_surface)),
                ("source_valid", H::Bool(source_valid)),
            ])
        })
        .collect()
}

/// The semantic drive facts consumed by `lint_usd`.
///
/// This deliberately calls the same composed USD joint reader that the runtime
/// projection uses. The linter must report the motor model the loader will
/// actually install, not re-interpret raw `drive:*` attributes through a
/// second unit-conversion or defaulting path.
fn drive_facts(reader: &StageView<'_>, joint_paths: &[SdfPath]) -> Vec<H> {
    let mut drives = Vec::new();
    for path in joint_paths {
        let Some(spec) = crate::read_joint_spec_for_lint(reader, path) else {
            continue;
        };
        let Some(drive) = spec.drive else {
            continue;
        };

        let (realization, frequency, damping_ratio) = match drive.motor_model() {
            Ok(MotorModel::SpringDamper {
                frequency,
                damping_ratio,
            }) => ("spring_damper", frequency, damping_ratio),
            Ok(MotorModel::ForceBased { .. }) => ("force_based", 0.0, 0.0),
            Ok(MotorModel::AccelerationBased { .. }) => ("acceleration_based", 0.0, 0.0),
            // Missing authored mass/inertia is valid USD authoring. The runtime
            // resolves this drive from Avian's computed properties after the
            // body's collider tree has been admitted; policy must not turn a
            // schema-valid omission into an error.
            Err(lunco_physics::ForceDriveMotorError::MissingGeneralizedInertia) => {
                ("derived", 0.0, 0.0)
            }
            Err(lunco_physics::ForceDriveMotorError::InvalidCoefficients) => ("invalid", 0.0, 0.0),
        };
        let stiffness = drive.stiffness.unwrap_or(0.0);
        let damping = drive.damping.unwrap_or(0.0);
        drives.push(H::map([
            ("path", H::str(path.to_string())),
            ("joint_type", H::str(spec.joint_type)),
            ("body0", H::str(spec.body0_path)),
            ("body1", H::str(spec.body1_path)),
            ("realization", H::str(realization)),
            ("stiffness", H::Float(stiffness)),
            ("damping", H::Float(damping)),
            (
                "max_force",
                drive.max_force.map(H::Float).unwrap_or(H::Unit),
            ),
            (
                "generalized_inertia",
                drive.generalized_inertia.map(H::Float).unwrap_or(H::Unit),
            ),
            ("frequency", H::Float(frequency)),
            ("damping_ratio", H::Float(damping_ratio)),
        ]));
    }
    drives
}

/// Everything policy needs to judge a stage's physics authoring.
///
/// Pure — no ECS, no side effects — so it can be built from a live scene, from
/// `ValidateAsset`'s pre-flight compose, or from a test fixture, and all three
/// then get identical findings from identical rules.
pub fn physics_facts(reader: &StageView<'_>) -> H {
    let paths: Vec<SdfPath> = reader.prim_paths();
    let telemetry_declarations = telemetry_declaration_facts(reader, &paths);

    // `physics:collisionEnabled = true` is only meaningful on a prim that
    // applies PhysicsCollisionAPI. Terrain and PhysX wheels are the two
    // deliberate exceptions: their owning projectors admit geometry through
    // LunCoTerrainAPI and PhysxVehicleWheelAPI respectively. Catch the
    // ordinary-geometry case here because the Avian compound reader otherwise
    // ignores the prim without any indication that the authored intent was
    // dropped.
    let collision_enabled_without_api: Vec<H> = paths
        .iter()
        .filter(|p| {
            reader.boolean(p, ptok::A_COLLISION_ENABLED) == Some(true)
                && !reader.has_api_schema(p, ptok::API_COLLISION)
                && !reader.has_api_schema(p, "LunCoTerrainAPI")
                && !reader.has_api_schema(p, "PhysxVehicleWheelAPI")
        })
        .map(|p| H::str(p.to_string()))
        .collect();

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
        // A joint endpoint that is not itself a body RESOLVES TO ITS NEAREST
        // ANCESTOR BODY — the UsdPhysics rule, and the one
        // `joint_endpoint_that_is_not_a_body_resolves_to_its_nearest_ancestor_body`
        // pins in the loader. It is how every mounted mechanism works:
        // `components/comms/antenna.usda` names its own root Xform, because a
        // component cannot know the host it will be parented under. So `missing`
        // is "resolves to NO body", not "is not a body" — the latter reported all
        // 73 shipped antennas, landers and masts as broken mechanisms while the
        // asset's own doc comment described the construction as intended.
        let resolved: Vec<String> = targets
            .iter()
            .map(|t| {
                if bodies.contains(t) {
                    t.clone()
                } else {
                    host_body(&bodies, t).unwrap_or_else(|| t.clone())
                }
            })
            .collect();
        let missing = targets
            .iter()
            .filter(|t| !bodies.contains(*t) && host_body(&bodies, t).is_none())
            .cloned();
        // The RESOLVED endpoints, so `jointed` answers "is this body held" rather
        // than "was this body's path typed into a joint".
        attached.extend(resolved);
        joints.push(H::map([
            ("path", H::str(jp.to_string())),
            (
                "type",
                H::str(reader.prim_type_name(jp).unwrap_or_default()),
            ),
            (
                "bodies",
                H::Array(targets.iter().cloned().map(H::str).collect()),
            ),
            ("missing", H::Array(missing.map(H::str).collect())),
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
        let missing = targets.iter().filter(|t| !known.contains(*t)).cloned();
        let owners = targets.iter().map(|t| owner_of(t));
        filtered_pairs.push(H::map([
            ("path", H::str(path.clone())),
            ("owner", H::str(owner_of(&path))),
            (
                "targets",
                H::Array(targets.iter().cloned().map(H::str).collect()),
            ),
            ("target_owners", H::Array(owners.map(H::str).collect())),
            ("missing", H::Array(missing.map(H::str).collect())),
        ]));
    }

    // Collision groups. A group whose collection includes nothing that exists, or
    // that filters against a prim which is not a group, is authoring that reads as
    // protection and is not — decidable here, and at runtime only a warning in a
    // log nobody is watching.
    let mut collision_groups: Vec<H> = Vec::new();
    let group_paths: HashSet<String> = paths
        .iter()
        .filter(|p| reader.prim_type_name(p).as_deref() == Some(ptok::T_PHYSICS_COLLISION_GROUP))
        .map(|p| p.to_string())
        .collect();
    for p in &paths {
        let path = p.to_string();
        if !group_paths.contains(&path) {
            continue;
        }
        let includes: Vec<String> = reader
            .rel_targets(p, "collection:colliders:includes")
            .into_iter()
            .map(|t| t.to_string())
            .collect();
        let filtered: Vec<String> = reader
            .rel_targets(p, ptok::A_FILTERED_GROUPS)
            .into_iter()
            .map(|t| t.to_string())
            .collect();
        // An include is a SUBTREE root, so "exists" means some prim is it or under
        // it — an include naming a prim that was renamed matches nothing at all.
        let missing_includes = includes
            .iter()
            .filter(|t| {
                !known
                    .iter()
                    .any(|k| k == *t || k.starts_with(&format!("{t}/")))
            })
            .cloned();
        let missing_filtered = filtered
            .iter()
            .filter(|t| !group_paths.contains(*t))
            .cloned();
        collision_groups.push(H::map([
            ("path", H::str(path)),
            (
                "merge",
                H::str(reader.text(p, ptok::A_MERGE_GROUP).unwrap_or_default()),
            ),
            (
                "invert",
                H::Bool(
                    reader
                        .boolean(p, ptok::A_INVERT_FILTERED_GROUPS)
                        .unwrap_or(false),
                ),
            ),
            (
                "includes",
                H::Array(includes.iter().cloned().map(H::str).collect()),
            ),
            (
                "filtered",
                H::Array(filtered.iter().cloned().map(H::str).collect()),
            ),
            (
                "missing_includes",
                H::Array(missing_includes.map(H::str).collect()),
            ),
            (
                "missing_filtered",
                H::Array(missing_filtered.map(H::str).collect()),
            ),
        ]));
    }

    let mut sorted: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    sorted.sort();

    let vehicle_roots: HashSet<String> = paths
        .iter()
        .filter(|p| {
            reader.kind(p).as_deref() == Some("assembly")
                && reader.has_api_schema(p, ptok::API_RIGID_BODY)
        })
        .map(ToString::to_string)
        .collect();
    let proxy_bodies: HashSet<String> = bodies
        .iter()
        .filter(|body| body_has_proxy(reader, &sorted, &bodies, body))
        .cloned()
        .collect();
    let vehicle_parts = vehicle_part_facts(reader, &paths, &bodies, &vehicle_roots, &proxy_bodies);

    let mut body_facts: Vec<H> = Vec::new();
    let mut unsupported_program_prims: Vec<H> = Vec::new();
    let mut connector_programs: Vec<H> = Vec::new();
    for p in &paths {
        if reader.prim_type_name(p).as_deref() == Some("LunCoProgram") {
            unsupported_program_prims.push(H::str(p.to_string()));
        }
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
                H::Bool(
                    reader
                        .boolean(p, ptok::A_KINEMATIC_ENABLED)
                        .unwrap_or(false),
                ),
            ),
            (
                "simulated",
                H::Bool(
                    reader
                        .boolean(p, ptok::A_RIGID_BODY_ENABLED)
                        .unwrap_or(true),
                ),
            ),
            ("collider", H::Bool(own_collider)),
            (
                "subtree_collider",
                H::Bool(own_collider || subtree_has_collider(reader, &sorted, &path)),
            ),
            (
                "host_body",
                H::str(host_body(&bodies, &path).unwrap_or_default()),
            ),
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
    // type, its parent, applied schemas, and property names. `bodies`/`joints` above are
    // pre-chewed answers to the questions we already know we ask; this is what
    // lets a NEW rule ask a NEW question — "PhysicsMassAPI on a prim inside no
    // body", "a motor with no shaft binding", "a collider outside every
    // body" — without a Rust change, which is the whole point of putting rules in
    // rhai. Bounded by schema'd prims (hundreds), not by prim count (thousands).
    let mut prims: Vec<H> = Vec::new();
    let mut collections: Vec<H> = Vec::new();
    let mut network_roots: Vec<H> = Vec::new();
    for p in &paths {
        if lunco_usd_bevy::program::is_domain_network_root(reader, p) {
            let (members, collection_error) = match reader.collection_members(p, "components") {
                Ok(members) => (members, String::new()),
                Err(error) => (Vec::new(), error),
            };
            let member_names: HashSet<String> = members.iter().map(ToString::to_string).collect();
            let (synthesizer, synthesizer_error) =
                if reader.has_api_schema(p, "LunCoDomainSynthesisAPI") {
                    (
                        reader
                            .text(p, "lunco:synthesizer")
                            .filter(|name| !name.is_empty())
                            .unwrap_or_default(),
                        String::new(),
                    )
                } else {
                    match lunco_usd_bevy::program::derive_synthesizer_name(reader, p) {
                        Ok(name) => (name, String::new()),
                        Err(error) => (String::new(), error),
                    }
                };
            let mut modelica_members = HashSet::new();
            let mut graph_edges = Vec::<(String, String)>::new();
            let mut dangling_connectors = Vec::new();
            let mut modelica_member_count = 0_i64;
            let mut invalid_program_sources = Vec::new();
            let mut invalid_causal_properties = Vec::new();
            let mut boundary_sources: HashMap<String, Vec<String>> = HashMap::new();
            for attr in reader.attr_names(p) {
                let connections = reader.connections(p, &attr);
                if (lunco_usd_bevy::program::is_network_boundary_output(reader, p, &attr)
                    && connections.len() != 1)
                    || (attr.starts_with("inputs:") && connections.len() > 1)
                {
                    invalid_causal_properties.push(format!("{p}.{attr}"));
                }
                if let Some(name) = attr.strip_prefix("inputs:") {
                    if let [source] = connections.as_slice() {
                        boundary_sources
                            .entry(source.to_string())
                            .or_default()
                            .push(name.to_string());
                    }
                }
            }
            let ambiguous_boundary_sources =
                boundary_sources
                    .into_iter()
                    .filter_map(|(source, boundaries)| {
                        (boundaries.len() > 1).then(|| {
                            format!("{} inputs {} resolve to {source}", p, boundaries.join(", "))
                        })
                    });
            for member in &members {
                if !reader.has_api_schema(member, "LunCoProgramAPI") {
                    continue;
                }
                let member_name = member.to_string();
                // ONE definition of the authored program contract, shared with
                // the runtime projector. The class itself is resolved from the
                // loaded source by lunco-usd-sim; lint must not invent one from
                // the asset path.
                let Ok(_) = lunco_usd_bevy::program::modelica_source_ref(reader, member) else {
                    invalid_program_sources.push(member_name.clone());
                    continue;
                };
                modelica_member_count += 1;
                modelica_members.insert(member_name.clone());
                for attr in reader.attr_names(member) {
                    if (attr.starts_with("inputs:") || attr.starts_with("outputs:"))
                        && reader.connections(member, &attr).len() > 1
                    {
                        invalid_causal_properties.push(format!("{member_name}.{attr}"));
                    }
                    let Some(connector) = attr.strip_prefix("connectors:") else {
                        continue;
                    };
                    for target in reader.connections(member, &attr) {
                        let target_string = target.to_string();
                        let Some((target_prim, target_connector)) =
                            target_string.split_once(".connectors:")
                        else {
                            dangling_connectors
                                .push(format!("{member_name}.connectors:{connector}"));
                            continue;
                        };
                        if !member_names.contains(target_prim) {
                            dangling_connectors
                                .push(format!("{member_name}.connectors:{connector}"));
                            continue;
                        }
                        let target_exists = SdfPath::new(target_prim).ok().is_some_and(|path| {
                            reader
                                .attr_names(&path)
                                .iter()
                                .any(|name| name == &format!("connectors:{target_connector}"))
                        });
                        if !target_exists {
                            dangling_connectors
                                .push(format!("{member_name}.connectors:{connector}"));
                            continue;
                        }
                        graph_edges.push((member_name.clone(), target_prim.to_string()));
                    }
                }
            }
            let mut graph = ProgramGraph::default();
            for member in &modelica_members {
                graph.add_node(member.clone());
            }
            for (left, right) in graph_edges {
                if modelica_members.contains(&right) {
                    graph.connect(left, right);
                }
            }
            // A causal edge also couples two members in one generated Modelica
            // composite unit. Read it through the same graph helper as the
            // runtime synthesizer; lint must not maintain a second connectivity
            // algorithm.
            for member in &modelica_members {
                let Ok(member_path) = SdfPath::new(member) else {
                    continue;
                };
                for attr in reader.attr_names(&member_path) {
                    if !attr.starts_with("inputs:") {
                        continue;
                    }
                    let connections = reader.connections(&member_path, &attr);
                    let [source] = connections.as_slice() else {
                        continue;
                    };
                    let source = source.to_string();
                    let Some((source_prim, _)) = source.split_once(".outputs:") else {
                        continue;
                    };
                    if modelica_members.contains(source_prim) {
                        graph.connect(member.clone(), source_prim.to_string());
                    }
                }
            }
            let units = graph.connected_components();
            network_roots.push(H::map([
                ("path", H::str(p.to_string())),
                (
                    "parent",
                    H::str(
                        p.parent()
                            .map(|parent| parent.to_string())
                            .unwrap_or_default(),
                    ),
                ),
                (
                    "members",
                    H::Array(
                        members
                            .into_iter()
                            .map(|member| H::str(member.to_string()))
                            .collect(),
                    ),
                ),
                // The selected domain synthesizer owns the interpretation of
                // this CollectionAPI:components scope.  The default empty
                // value is the documented acausal-network synthesizer; an
                // authored name is a different domain contract and must not
                // be judged by the generic Modelica cardinality rules below.
                ("synthesizer", H::str(synthesizer)),
                ("synthesizer_error", H::str(synthesizer_error)),
                (
                    "units",
                    H::Array(
                        units
                            .into_iter()
                            .map(|members| {
                                H::map([(
                                    "members",
                                    H::Array(members.into_iter().map(H::str).collect()),
                                )])
                            })
                            .collect(),
                    ),
                ),
                ("modelica_member_count", H::Int(modelica_member_count)),
                ("collection_error", H::str(collection_error)),
                (
                    "dangling_connectors",
                    H::Array(dangling_connectors.into_iter().map(H::str).collect()),
                ),
                (
                    "invalid_program_sources",
                    H::Array(invalid_program_sources.into_iter().map(H::str).collect()),
                ),
                (
                    "invalid_causal_properties",
                    H::Array(invalid_causal_properties.into_iter().map(H::str).collect()),
                ),
                (
                    "ambiguous_boundary_sources",
                    H::Array(ambiguous_boundary_sources.map(H::str).collect()),
                ),
            ]));
        }
        let schemas = applied_schemas(reader, p);
        if schemas.is_empty() {
            continue;
        }
        if schemas.iter().any(|schema| schema == "LunCoProgramAPI") {
            // DECLARED vs WIRED, kept as two fields. A bare `connectors:p` is an
            // INTERFACE — the USD spelling of Modelica's `Pin p` — and a catalogue
            // part cannot know whether the vehicle composing it will wire that pin:
            // `power = "infinite"` on every rover wires none of them. Only an
            // authored `.connect` is a claim about topology, and only that claim
            // can be wrong.
            let mut connectors: Vec<H> = Vec::new();
            let mut connected: Vec<H> = Vec::new();
            for name in reader.attr_names(p) {
                let Some(short) = name.strip_prefix("connectors:") else {
                    continue;
                };
                connectors.push(H::str(short));
                if !reader.connections(p, &name).is_empty() {
                    connected.push(H::str(short));
                }
            }
            if !connectors.is_empty() {
                let source_asset = lunco_usd_bevy::program::modelica_source_ref(reader, p)
                    .map(|source_ref| source_ref.asset)
                    .unwrap_or_default();
                connector_programs.push(H::map([
                    ("path", H::str(p.to_string())),
                    ("source_asset", H::str(source_asset)),
                    ("connectors", H::Array(connectors)),
                    ("connected", H::Array(connected)),
                ]));
            }
        }
        for schema in &schemas {
            let Some(name) = schema.strip_prefix("CollectionAPI:") else {
                continue;
            };
            let relationship = format!("collection:{name}:includes");
            let explicit = reader
                .value_str(p, &format!("collection:{name}:expansionRule"))
                .as_deref()
                == Some("explicitOnly");
            let members = if explicit {
                reader.rel_targets(p, &relationship)
            } else {
                let collection = Collection::new(p.clone(), name);
                collection
                    .compute_membership_query(reader.stage())
                    .ok()
                    .and_then(|query| {
                        compute_included_paths(reader.stage(), &query, PrimPredicate::DEFAULT).ok()
                    })
                    .unwrap_or_default()
            };
            collections.push(H::map([
                ("path", H::str(p.to_string())),
                (
                    "parent",
                    H::str(
                        p.parent()
                            .map(|parent| parent.to_string())
                            .unwrap_or_default(),
                    ),
                ),
                ("name", H::str(name)),
                (
                    "members",
                    H::Array(
                        members
                            .into_iter()
                            .map(|member| H::str(member.to_string()))
                            .collect(),
                    ),
                ),
            ]));
        }
        let parent = p.parent().map(|x| x.to_string()).unwrap_or_default();
        let attributes = reader.attr_names(p);
        let connected_attributes = attributes
            .iter()
            .filter(|name| !reader.connections(p, name).is_empty())
            .cloned();
        let epoch_jd = reader
            .value::<f64>(p, "lunco:time:epochJd")
            .map(H::Float)
            .unwrap_or(H::Unit);
        prims.push(H::map([
            ("path", H::str(p.to_string())),
            ("type", H::str(reader.prim_type_name(p).unwrap_or_default())),
            ("parent", H::str(parent)),
            (
                "schemas",
                H::Array(schemas.into_iter().map(H::str).collect()),
            ),
            (
                "attributes",
                H::Array(attributes.iter().cloned().map(H::str).collect()),
            ),
            (
                "connected_attributes",
                H::Array(connected_attributes.map(H::str).collect()),
            ),
            ("epoch_jd", epoch_jd),
        ]));
    }

    H::map([
        (
            "stage",
            H::map([
                (
                    "meters_per_unit_authored",
                    H::Bool(
                        reader
                            .stage()
                            .stage_metadata("metersPerUnit")
                            .ok()
                            .flatten()
                            .is_some(),
                    ),
                ),
                ("fixed_hz", H::Float(lunco_core::FIXED_HZ)),
                (
                    "physics_substeps",
                    H::Int(lunco_physics::DEFAULT_SUBSTEP_COUNT as i64),
                ),
                (
                    "substep_dt",
                    H::Float(
                        1.0 / (lunco_core::FIXED_HZ * lunco_physics::DEFAULT_SUBSTEP_COUNT as f64),
                    ),
                ),
            ]),
        ),
        ("bodies", H::Array(body_facts)),
        ("joints", H::Array(joints)),
        ("drives", H::Array(drive_facts(reader, &joint_paths))),
        ("filtered_pairs", H::Array(filtered_pairs)),
        ("collision_groups", H::Array(collision_groups)),
        ("collections", H::Array(collections)),
        ("network_roots", H::Array(network_roots)),
        ("prims", H::Array(prims)),
        ("vehicle_parts", H::Array(vehicle_parts)),
        (
            "collision_enabled_without_api",
            H::Array(collision_enabled_without_api),
        ),
        (
            "unsupported_program_prims",
            H::Array(unsupported_program_prims),
        ),
        ("connector_programs", H::Array(connector_programs)),
        ("telemetry_declarations", H::Array(telemetry_declarations)),
    ])
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
        let f = dir.join(format!(
            "fixture_{}.usda",
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&f, usda).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");
        let view = StageView::new(&stage);
        physics_facts(&view)
    }

    fn entries(facts: &H, key: &str) -> Vec<H> {
        let H::Map(m) = facts else {
            panic!("facts is not a map: {facts:?}")
        };
        match m.iter().find(|(k, _)| k == key).map(|(_, v)| v) {
            Some(H::Array(a)) => a.clone(),
            other => panic!("facts.{key} is {other:?}"),
        }
    }

    #[test]
    fn collision_enabled_without_api_is_reported_except_for_owned_projectors() {
        let f = facts(
            r#"#usda 1.0
def Scope "Root"
{
    def Cube "Missing" { bool physics:collisionEnabled = true }
    def Cube "Terrain" (prepend apiSchemas = ["LunCoTerrainAPI"])
    { bool physics:collisionEnabled = true }
    def Cylinder "Wheel" (prepend apiSchemas = ["PhysxVehicleWheelAPI"])
    { bool physics:collisionEnabled = true }
    def Cube "Collider" (prepend apiSchemas = ["PhysicsCollisionAPI"])
    { bool physics:collisionEnabled = true }
}
"#,
        );
        assert_eq!(
            entries(&f, "collision_enabled_without_api"),
            vec![H::str("/Root/Missing")]
        );
    }

    #[test]
    fn vehicle_part_facts_require_a_collision_owner_or_explicit_visual_only_intent() {
        let f = facts(
            r#"#usda 1.0
def Xform "Vehicle" (
    kind = "assembly"
    prepend apiSchemas = ["PhysicsRigidBodyAPI"]
)
{
    def Cube "Hull" (prepend apiSchemas = ["PhysicsCollisionAPI"]) {}
    def Cylinder "Decoration"
    {
        bool physics:collisionEnabled = false
    }
    def Cylinder "Missing" {}
    def Cube "Proxy" (prepend apiSchemas = ["PhysicsCollisionAPI"])
    {
        uniform token purpose = "proxy"
    }
    def Cube "Render"
    {
        uniform token purpose = "render"
    }
    def Cylinder "Wheel" (prepend apiSchemas = ["PhysxVehicleWheelAPI"])
    {
        bool physics:collisionEnabled = true
    }
}
"#,
        );
        let parts = entries(&f, "vehicle_parts");
        let contract = |path| field(vehicle_part(&parts, path), "contract");
        assert_eq!(contract("/Vehicle/Hull"), &H::str("collider"));
        assert_eq!(contract("/Vehicle/Decoration"), &H::str("visual-only"));
        assert_eq!(
            contract("/Vehicle/Missing"),
            &H::str("missing-collider-api")
        );
        assert_eq!(contract("/Vehicle/Proxy"), &H::str("collider"));
        assert_eq!(contract("/Vehicle/Render"), &H::str("visual-only"));
        assert_eq!(
            field(
                vehicle_part(&parts, "/Vehicle/Render"),
                "render_excluded_by_proxy"
            ),
            &H::Bool(true)
        );
        assert_eq!(contract("/Vehicle/Wheel"), &H::str("projector"));
    }

    #[test]
    fn vehicle_part_facts_reject_an_unsupported_enabled_collider() {
        let f = facts(
            r#"#usda 1.0
def Xform "Vehicle" (
    kind = "assembly"
    prepend apiSchemas = ["PhysicsRigidBodyAPI"]
)
{
    def NurbsPatch "Unsupported" (prepend apiSchemas = ["PhysicsCollisionAPI"])
    {
        bool physics:collisionEnabled = true
    }
}
"#,
        );
        let parts = entries(&f, "vehicle_parts");
        let part = vehicle_part(&parts, "/Vehicle/Unsupported");
        assert_eq!(field(part, "shape_valid"), &H::Bool(false));
        assert_eq!(field(part, "contract"), &H::str("unsupported-collider"));
    }

    fn field<'a>(item: &'a H, key: &str) -> &'a H {
        let H::Map(m) = item else {
            panic!("not a map: {item:?}")
        };
        m.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .expect("field present")
    }

    fn body<'a>(facts: &'a [H], path: &str) -> &'a H {
        facts
            .iter()
            .find(|b| field(b, "path") == &H::str(path))
            .unwrap_or_else(|| panic!("no body fact for {path}"))
    }

    fn prim<'a>(facts: &'a [H], path: &str) -> &'a H {
        facts
            .iter()
            .find(|p| field(p, "path") == &H::str(path))
            .unwrap_or_else(|| panic!("no prim fact for {path}"))
    }

    fn vehicle_part<'a>(facts: &'a [H], path: &str) -> &'a H {
        facts
            .iter()
            .find(|part| field(part, "path") == &H::str(path))
            .unwrap_or_else(|| panic!("no vehicle part fact for {path}"))
    }

    const ROVER_WITH_LOOSE_MOTOR: &str = "#usda 1.0\n\
        def Xform \"Rover\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n\
        {\n\
            def Cube \"Chassis\" ( prepend apiSchemas = [\"PhysicsCollisionAPI\"] ) {}\n\
            def Xform \"Motor_FL\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] ) {}\n\
            def Cylinder \"Wheel_FL\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsCollisionAPI\"] ) {}\n\
            def PhysicsRevoluteJoint \"constraint_wheel_fl\"\n\
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

    #[test]
    fn generic_prim_facts_preserve_authored_epoch_values() {
        let f = facts(
            "#usda 1.0\n\
             def Scope \"AuthoredEpoch\" ( prepend apiSchemas = [\"LunCoEpochAPI\"] )\n\
             { double lunco:time:epochJd = 2461395.5 }\n\
             def Scope \"ZeroEpoch\" ( prepend apiSchemas = [\"LunCoEpochAPI\"] )\n\
             { double lunco:time:epochJd = 0.0 }\n\
             def Scope \"MissingEpoch\" ( prepend apiSchemas = [\"LunCoEpochAPI\"] ) {}\n",
        );
        let prims = entries(&f, "prims");
        assert_eq!(
            field(prim(&prims, "/AuthoredEpoch"), "epoch_jd"),
            &H::Float(2461395.5)
        );
        assert_eq!(
            field(prim(&prims, "/ZeroEpoch"), "epoch_jd"),
            &H::Float(0.0)
        );
        assert_eq!(field(prim(&prims, "/MissingEpoch"), "epoch_jd"), &H::Unit);
    }

    #[test]
    fn connector_program_fact_keeps_acausal_capability_out_of_runtime_ports() {
        let f = facts(
            "#usda 1.0\n\
             def Scope \"Battery\" ( prepend apiSchemas = [\"LunCoProgramAPI\"] )\n\
             {\n\
                 uniform token info:implementationSource = \"sourceAsset\"\n\
                 uniform asset info:sourceAsset = @models/Battery.mo@\n\
                 token connectors:p\n\
             }\n",
        );
        let programs = entries(&f, "connector_programs");
        assert_eq!(programs.len(), 1);
        assert_eq!(field(&programs[0], "path"), &H::str("/Battery"));
        assert_eq!(
            field(&programs[0], "source_asset"),
            &H::str("models/Battery.mo")
        );
        assert_eq!(
            field(&programs[0], "connectors"),
            &H::Array(vec![H::str("p")])
        );
        assert_eq!(
            field(&programs[0], "connected"),
            &H::Array(Vec::new()),
            "a bare `connectors:p` DECLARES an interface; nothing is wired yet"
        );
    }

    #[test]
    fn targetless_metadata_telemetry_has_no_direct_surface() {
        let f = facts(
            "#usda 1.0\n\
             def Scope \"Telemetry\" ( prepend apiSchemas = [\"LunCoTelemetryAPI\"] )\n\
             {\n\
                 bool lunco:telemetry = true\n\
                 token lunco:telemetry:port = \"ghost\"\n\
             }\n",
        );
        let declarations = entries(&f, "telemetry_declarations");
        assert_eq!(declarations.len(), 1);
        assert_eq!(field(&declarations[0], "targets"), &H::Array(Vec::new()));
        assert_eq!(field(&declarations[0], "target_exists"), &H::Bool(true));
        assert_eq!(field(&declarations[0], "direct_surface"), &H::Bool(false));
        assert_eq!(field(&declarations[0], "source_valid"), &H::Bool(true));
    }

    /// DECLARED and WIRED must be separable, or the rule that reads them cannot
    /// tell a catalogue part advertising a pin from a phantom wire. Every motor we
    /// ship declares `connectors:p` unconditionally and is wired only by the
    /// vehicle's `power = "battery"` variant.
    #[test]
    fn a_connected_connector_is_reported_separately_from_a_declared_one() {
        let f = facts(
            "#usda 1.0\n\
             def Scope \"Battery\" ( prepend apiSchemas = [\"LunCoProgramAPI\"] )\n\
             {\n\
                 uniform asset info:sourceAsset = @models/Battery.mo@\n\
                 token connectors:p\n\
             }\n\
             def Scope \"Motor\" ( prepend apiSchemas = [\"LunCoProgramAPI\"] )\n\
             {\n\
                 uniform asset info:sourceAsset = @models/DCMotor.mo@\n\
                 token connectors:p.connect = </Battery.connectors:p>\n\
             }\n",
        );
        let programs = entries(&f, "connector_programs");
        let motor = programs
            .iter()
            .find(|p| field(p, "path") == &H::str("/Motor"))
            .expect("motor fact");
        assert_eq!(field(motor, "connected"), &H::Array(vec![H::str("p")]));
    }

    /// Drive facts must describe the motor model the loader will install. A
    /// massed linear force drive is converted to the implicit SpringDamper path;
    /// the same authored law without explicit generalized inertia is marked for
    /// runtime derivation from the body's computed mass properties.
    #[test]
    fn drive_facts_preserve_the_runtime_motor_realization() {
        let f = facts(
            "#usda 1.0\n\
             ( metersPerUnit = 1 )\n\
             def Xform \"Rig\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsMassAPI\"] )\n\
             {\n\
                 float physics:mass = 100.0\n\
                 def Xform \"Massed\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsMassAPI\"] )\n\
                 {\n\
                     float physics:mass = 10.0\n\
                 }\n\
                 def Xform \"Unmassed\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] ) {}\n\
                 def PhysicsPrismaticJoint \"Implicit\" ( prepend apiSchemas = [\"PhysicsDriveAPI:linear\"] )\n\
                 {\n\
                     rel physics:body0 = </Rig>\n\
                     rel physics:body1 = </Rig/Massed>\n\
                     uniform token drive:linear:physics:type = \"force\"\n\
                     float drive:linear:physics:stiffness = 4000.0\n\
                     float drive:linear:physics:damping = 2200.0\n\
                 }\n\
                 def PhysicsPrismaticJoint \"Conditional\" ( prepend apiSchemas = [\"PhysicsDriveAPI:linear\"] )\n\
                 {\n\
                     rel physics:body0 = </Rig>\n\
                     rel physics:body1 = </Rig/Unmassed>\n\
                     uniform token drive:linear:physics:type = \"force\"\n\
                     float drive:linear:physics:stiffness = 4000.0\n\
                     float drive:linear:physics:damping = 2200.0\n\
                 }\n\
             }\n",
        );
        let drives = entries(&f, "drives");
        assert_eq!(
            drives.len(),
            2,
            "both authored drives must be projected: {drives:?}"
        );
        let implicit = drives
            .iter()
            .find(|drive| field(drive, "path") == &H::str("/Rig/Implicit"))
            .expect("massed drive fact");
        assert_eq!(field(implicit, "realization"), &H::str("spring_damper"));
        let conditional = drives
            .iter()
            .find(|drive| field(drive, "path") == &H::str("/Rig/Conditional"))
            .expect("unmassed drive fact");
        assert_eq!(field(conditional, "realization"), &H::str("derived"));
        let stage = match &f {
            H::Map(entries) => entries
                .iter()
                .find(|(key, _)| key == "stage")
                .map(|(_, value)| value)
                .expect("stage facts"),
            _ => panic!("facts are not a map"),
        };
        assert_eq!(
            field(stage, "physics_substeps"),
            &H::Int(lunco_physics::DEFAULT_SUBSTEP_COUNT as i64)
        );
    }

    /// A MOUNTED MECHANISM names a plain Xform, because a component referenced
    /// onto a rover, lander or mast cannot know any of their paths. UsdPhysics
    /// resolves that endpoint to the nearest ancestor body, so it is not
    /// `missing`, and the body it resolves to IS held.
    #[test]
    fn a_joint_endpoint_under_a_body_is_not_missing_and_joints_that_body() {
        let f = facts(
            "#usda 1.0\n\
             def Xform \"Host\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsCollisionAPI\"] )\n\
             {\n\
                 def Xform \"Mount\"\n\
                 {\n\
                     def Xform \"Head\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsCollisionAPI\"] ) {}\n\
                     def PhysicsRevoluteJoint \"YawJoint\"\n\
                     {\n\
                         rel physics:body0 = </Host/Mount>\n\
                         rel physics:body1 = </Host/Mount/Head>\n\
                     }\n\
                 }\n\
             }\n",
        );
        let joints = entries(&f, "joints");
        assert_eq!(joints.len(), 1);
        assert_eq!(
            field(&joints[0], "missing"),
            &H::Array(Vec::new()),
            "</Host/Mount> is not a body but hangs under one, so the endpoint resolves"
        );
        let bodies = entries(&f, "bodies");
        assert_eq!(
            field(body(&bodies, "/Host"), "jointed"),
            &H::Bool(true),
            "the joint names the Xform and therefore holds the body it resolves to"
        );
    }

    /// The real form of the mistake: an endpoint under NO body at all. Nothing
    /// resolves, the joint is dropped, and this must stay a finding.
    #[test]
    fn a_joint_endpoint_under_no_body_is_still_missing() {
        let f = facts(
            "#usda 1.0\n\
             def Xform \"Anchor\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsCollisionAPI\"] ) {}\n\
             def Cube \"NotABody\" ( prepend apiSchemas = [\"PhysicsCollisionAPI\"] ) {}\n\
             def PhysicsFixedJoint \"BadJoint\"\n\
             {\n\
                 rel physics:body0 = </Anchor>\n\
                 rel physics:body1 = </NotABody>\n\
             }\n",
        );
        let joints = entries(&f, "joints");
        assert_eq!(
            field(&joints[0], "missing"),
            &H::Array(vec![H::str("/NotABody")])
        );
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
        let expected =
            -1.807 - (0.075 * 25f32.to_radians().sin() + 3.525 * 25f32.to_radians().cos());
        assert!(
            (leg - expected).abs() < 1e-3,
            "strut low point {leg}, expected {expected}"
        );

        // The pad's own bottom face — a cylinder is centred on its origin.
        assert!((pad - (-5.1359 - 0.15)).abs() < 1e-3, "pad low point {pad}");

        // And the fact these two numbers exist to state: the FOOT reaches lower.
        assert!(
            pad < leg,
            "pad {pad} must reach below the strut corner {leg}"
        );
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
        assert_eq!(
            field(body(&bodies, "/Rig/Anchor"), "kinematic"),
            &H::Bool(true)
        );
        assert_eq!(
            field(body(&bodies, "/Rig/Prop"), "simulated"),
            &H::Bool(false)
        );
    }

    #[test]
    fn empty_collection_uses_the_default_synthesizer_without_an_error() {
        let f = facts(
            r#"#usda 1.0
               def Xform "Empty" ( prepend apiSchemas = ["CollectionAPI:components"] )
               {
                   uniform token collection:components:expansionRule = "explicitOnly"
               }
            "#,
        );
        let scopes = entries(&f, "network_roots");
        let scope = scopes
            .iter()
            .find(|scope| field(scope, "path") == &H::str("/Empty"))
            .expect("empty collection fact");
        assert_eq!(field(scope, "synthesizer"), &H::str("acausal-network"));
        assert_eq!(field(scope, "synthesizer_error"), &H::str(""));
    }

    #[test]
    fn force_actuator_collection_uses_the_wrench_synthesizer() {
        let f = facts(
            r#"#usda 1.0
               def Scope "Actuators" ( prepend apiSchemas = ["CollectionAPI:components"] )
               {
                   uniform token collection:components:expansionRule = "explicitOnly"
                   prepend rel collection:components:includes = [</Actuators/Nozzle>]
                   def Cube "Nozzle" ( prepend apiSchemas = ["LunCoForceActuatorAPI"] ) {}
               }
            "#,
        );
        let scopes = entries(&f, "network_roots");
        let scope = scopes
            .iter()
            .find(|scope| field(scope, "path") == &H::str("/Actuators"))
            .expect("actuator collection fact");
        assert_eq!(field(scope, "synthesizer"), &H::str("actuator-wrench"));
        assert_eq!(field(scope, "synthesizer_error"), &H::str(""));
    }
}
