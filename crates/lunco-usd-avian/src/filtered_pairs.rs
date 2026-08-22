//! `PhysicsFilteredPairsAPI` — authored "these two never collide" pairs.
//!
//! A joint filters the pair it names, and no further: `JointCollisionDisabled`
//! covers parent and child. Parts **two joints apart** still collide — a hull and
//! the footpad on the end of its leg, a wheel and the rocker its bogie hangs
//! from. Author them close enough and the solver spends every step pushing a
//! vehicle apart from itself.
//!
//! UsdPhysics already has the schema for saying so: `PhysicsFilteredPairsAPI`
//! carries a `physics:filteredPairs` relationship naming the prims this one must
//! not collide with. This module is the whole of its support — read the rel,
//! resolve it to the entities that actually own the colliders, and hand the pair
//! to avian's [`CollisionHooks`] filter.
//!
//! # Why not infer it
//!
//! The tempting rule is "a vehicle never collides with itself", and it does not
//! survive contact with a scene: *vehicle* is not a thing the physics knows. A
//! rover parked on a lander's deck is one vehicle or two depending on the minute;
//! a robot arm **should** collide with its own base, or it folds through it;
//! filtering a whole joint-graph component silently kills contacts an articulated
//! mechanism depends on. Every engine that solved this made it explicit — MuJoCo
//! takes authored `<exclude>` pairs, PhysX takes filtered pairs, URDF/MoveIt
//! *precomputes* a pair list into authoring. Parent-child is automatic, the rest
//! is authored, and finding the pairs that need authoring is the linter's job.
//!
//! # Which entity is filtered
//!
//! The rel may name a body prim or a collider prim underneath one, and the two
//! are not the same entity: a collider under a body folds into that body's
//! compound shape, so the entity avian sees is the **body**. Both ends are
//! therefore resolved to the nearest self-or-ancestor prim that carries a
//! collider — the same ownership rule the loader applies when it builds the
//! compound, and the same one that stops at a nested body.
//!
//! Filtering is symmetric (the USD docs say so, and a one-sided contact filter is
//! meaningless), so authoring it on either prim filters the pair.

use avian3d::prelude::*;
use avian3d::{
    collider_tree::{ColliderTreeProxyFlags, ColliderTreeProxyKey, ColliderTrees},
    math::{Scalar, Vector},
};
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lunco_usd_bevy::{instance_key, StageView, UsdInstanceRoot, UsdPrimPath, UsdRead};
use openusd::schemas::physics::tokens as ptok;
use openusd::sdf::Path as SdfPath;
/// Authored `physics:filteredPairs` targets, waiting for their prims to spawn.
///
/// The rel names prims; the pair is only expressible once both ends exist as
/// entities, which for a streamed or deferred scene is not the frame the prim was
/// read on. Same deferral as `PendingUsdJoint`, and for the same reason.
#[derive(Component, Debug, Clone)]
pub struct PendingFilteredPairs {
    /// Prim paths this prim must not collide with, verbatim from the rel.
    pub targets: Vec<String>,
}

/// The resolved collider entities this entity must never collide with.
///
/// Written on BOTH ends of every authored pair, so the hook below needs only one
/// lookup per side.
#[derive(Component, Debug, Clone, Default)]
pub struct FilteredPairs(pub EntityHashSet);

/// Marks a rigid body whose wheel-ground tangential force is supplied by the
/// shared LunCo tire model rather than Avian's generic Coulomb tangent solver.
///
/// Normal contact, penetration, and joint constraints remain Avian-owned.  The
/// collision hook below only removes Avian's tangential impulse for this body;
/// `lunco-mobility` then applies the authored tire law at the saved contact
/// points.  Keeping this marker in the Avian bridge avoids a dependency cycle
/// while making the contact ownership explicit at the physics boundary.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct SharedTireContact;

/// Numerical stiction band for Avian's discrete Coulomb solve.
///
/// Coulomb friction is discontinuous at zero slip, while a discrete solver
/// never measures an exact mathematical zero. One centimetre per second is a
/// solver tolerance, not a material classifier: both coefficients and their
/// combine rule still come entirely from the authored [`Friction`] properties.
const STATIC_FRICTION_MAX_SLIP_SPEED_MPS: Scalar = 0.01;

/// Declare that `a` and `b` must never collide, from this command flush onward.
///
/// The programmatic counterpart of an authored `physics:filteredPairs`, writing
/// the same component on both ends so the hook below needs no second code path.
///
/// Used by [`attach_joint`](crate::attach_joint) for every jointed pair, and
/// that is not an optimisation — it is what makes deferring a joint SAFE. A
/// jointed pair does not collide (`JointCollisionDisabled`), but that marker only
/// prevents a contact that does not exist yet: avian's
/// `on_disable_joint_collision` deletes ALREADY-formed contacts without telling
/// the island manager, leaving a freed `ContactId` in an island's list that a
/// later op unwraps and dies on ("Contact has no island", `islands/mod.rs:861`).
/// A joint waiting for its bodies to be admitted is exactly a window in which
/// that contact can form — wheel and fender overlap on the frame they spawn — so
/// the pair is filtered at ATTACH time, before any narrow phase can see it,
/// rather than at insert time.
pub fn filter_pair(commands: &mut Commands, a: Entity, b: Entity) {
    for (from, to) in [(a, b), (b, a)] {
        commands
            .entity(from)
            .entry::<FilteredPairs>()
            .or_default()
            .and_modify(move |mut pairs| {
                pairs.0.insert(to);
            });
        enable_collision_hook(commands, from, ActiveCollisionHooks::FILTER_PAIRS);
    }
}

/// Add one Avian collision hook without erasing another hook already authored
/// or installed on the same collider.
///
/// `ActiveCollisionHooks` is an immutable bitset component, so a plain insert
/// is a replacement, not a union.  Filtering and contact modification are
/// independent contracts and frequently coexist on one body; queue the union
/// at command-application time so deferred USD extraction and joint admission
/// see the current component value.
pub fn enable_collision_hook(commands: &mut Commands, entity: Entity, hook: ActiveCollisionHooks) {
    commands.queue(move |world: &mut World| {
        let current = world
            .get::<ActiveCollisionHooks>(entity)
            .copied()
            .unwrap_or_default();
        world.entity_mut(entity).insert(current | hook);
    });
}

/// Physics ticks a pending pair may scan at full rate before it is reported.
///
/// A typo'd rel target never spawns, and a silent forever-scan is exactly the
/// failure mode this project pays most for.
const PAIR_RESOLVE_WARN_TICKS: u32 = 600;

/// Retry cadence past the budget: the prim may still spawn late (streamed
/// content), so resolution never stops — it just stops being hot.
const PAIR_RESOLVE_RETRY_INTERVAL: u32 = 60;

/// Reads `physics:filteredPairs` off `sdf_path`, if the prim applies the API.
///
/// Returns `None` when the schema is absent, so the caller can skip the insert
/// entirely rather than stamping an empty carrier on every prim in the scene.
pub(crate) fn read_filtered_pairs(
    reader: &StageView<'_>,
    sdf_path: &SdfPath,
) -> Option<PendingFilteredPairs> {
    if !reader.has_api_schema(sdf_path, ptok::API_FILTERED_PAIRS) {
        return None;
    }
    let targets: Vec<String> = reader
        .rel_targets(sdf_path, ptok::A_FILTERED_PAIRS)
        .into_iter()
        .map(|p| p.to_string())
        .collect();
    if targets.is_empty() {
        warn!(
            "[usd-avian] {sdf_path}: applies {} but names no targets in {} — \
             the pair it was meant to filter still collides.",
            ptok::API_FILTERED_PAIRS,
            ptok::A_FILTERED_PAIRS,
        );
        return None;
    }
    Some(PendingFilteredPairs { targets })
}

/// The entity that owns the collider for `path`: itself if it carries one, else
/// the nearest ancestor that does.
///
/// A collider authored under a body is folded into that body's compound shape and
/// has no collider of its own, so naming it in a rel must filter the **body**.
/// The walk stops at the first collider it finds, which is exactly where the
/// loader's own ownership walk stops — including at a nested body, whose collider
/// is its own and never its parent's.
fn collider_owner(
    path: &str,
    stage_handle: &Handle<lunco_usd_bevy::UsdStageAsset>,
    root: Option<u64>,
    q_colliders: &Query<(Entity, &UsdPrimPath), With<Collider>>,
    q_provenance: &Query<&lunco_core::Provenance>,
    q_gid: &Query<&lunco_core::GlobalEntityId>,
    q_instance_root: &Query<(), With<UsdInstanceRoot>>,
) -> Option<Entity> {
    let mut candidate = Some(path.to_string());
    while let Some(p) = candidate {
        if let Some((e, _)) = q_colliders.iter().find(|(e, up)| {
            up.path == p
                && up.stage_handle == *stage_handle
                && instance_key(*e, q_provenance, q_gid, q_instance_root) == root
        }) {
            return Some(e);
        }
        candidate = p
            .rsplit_once('/')
            .map(|(head, _)| head.to_string())
            .filter(|h| !h.is_empty());
    }
    None
}

/// Resolves authored pairs to entities and arms avian's pair filter on both ends.
///
/// Runs in the same window as `build_usd_physics_joints` and for the same reason:
/// avian's broad phase skips any pair already in the contact graph
/// (`bvh_broad_phase.rs`, "Avoid duplicate pairs"), so a filter that arrives after
/// the first narrow phase does not apply to a contact that already exists. It has
/// to be armed before the bodies can touch, not merely eventually.
pub(crate) fn resolve_filtered_pairs(
    mut commands: Commands,
    q_pending: Query<(Entity, &PendingFilteredPairs, &UsdPrimPath)>,
    q_colliders: Query<(Entity, &UsdPrimPath), With<Collider>>,
    q_filtered: Query<&FilteredPairs>,
    q_hooks: Query<&ActiveCollisionHooks>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    mut resolve_ticks: Local<EntityHashMap<u32>>,
) {
    resolve_ticks.retain(|e, _| q_pending.contains(*e));

    // Accumulated in-system as well as read from the world: several pairs may name
    // the same entity, and `Commands` do not apply until the system ends, so a
    // second insert would otherwise overwrite the first.
    let mut additions: EntityHashMap<EntityHashSet> = EntityHashMap::default();

    for (entity, pending, prim) in q_pending.iter() {
        let ticks = resolve_ticks.get(&entity).copied().unwrap_or(0);
        if ticks >= PAIR_RESOLVE_WARN_TICKS && ticks % PAIR_RESOLVE_RETRY_INTERVAL != 0 {
            resolve_ticks.insert(entity, ticks.saturating_add(1));
            continue;
        }
        let root = instance_key(entity, &q_provenance, &q_gid, &q_instance_root);

        let Some(self_owner) = collider_owner(
            &prim.path,
            &prim.stage_handle,
            root,
            &q_colliders,
            &q_provenance,
            &q_gid,
            &q_instance_root,
        ) else {
            let ticks = ticks.saturating_add(1);
            if ticks == PAIR_RESOLVE_WARN_TICKS {
                warn!(
                    "[usd-avian] {}: applies {} but neither it nor any ancestor has a \
                     collider, so there is no pair to filter.",
                    prim.path,
                    ptok::API_FILTERED_PAIRS,
                );
            }
            resolve_ticks.insert(entity, ticks);
            continue;
        };

        let mut unresolved = Vec::new();
        let mut pairs = Vec::new();
        for target in &pending.targets {
            match collider_owner(
                target,
                &prim.stage_handle,
                root,
                &q_colliders,
                &q_provenance,
                &q_gid,
                &q_instance_root,
            ) {
                Some(other) if other == self_owner => {
                    // Two prims of ONE compound body: avian never pairs a body with
                    // itself, so this is authoring that reads as protection and is
                    // not. Say so rather than resolving it silently.
                    warn!(
                        "[usd-avian] {}: filtered pair '{target}' resolves to the same \
                         body — both are colliders of one compound shape, which never \
                         collides with itself. The pair you meant is elsewhere.",
                        prim.path,
                    );
                }
                Some(other) => pairs.push(other),
                None => unresolved.push(target.clone()),
            }
        }

        if !unresolved.is_empty() {
            let ticks = ticks.saturating_add(1);
            if ticks == PAIR_RESOLVE_WARN_TICKS {
                warn!(
                    "[usd-avian] {}: filtered-pair target(s) {} still unresolved after {} \
                     physics ticks — check the rel paths; retrying every {} ticks.",
                    prim.path,
                    unresolved.join(", "),
                    PAIR_RESOLVE_WARN_TICKS,
                    PAIR_RESOLVE_RETRY_INTERVAL,
                );
            }
            resolve_ticks.insert(entity, ticks);
            continue;
        }

        resolve_ticks.remove(&entity);
        commands.entity(entity).remove::<PendingFilteredPairs>();

        for other in pairs {
            // Symmetric: authoring on either prim filters the pair, so both ends
            // carry it and the hook needs one lookup per side.
            additions.entry(self_owner).or_default().insert(other);
            additions.entry(other).or_default().insert(self_owner);
            info!(
                "[usd-avian] filtered pair: {} <-x-> {}",
                prim.path,
                q_colliders
                    .get(other)
                    .map(|(_, p)| p.path.clone())
                    .unwrap_or_default(),
            );
        }
    }

    for (entity, added) in additions {
        let mut set = q_filtered
            .get(entity)
            .map(|f| f.0.clone())
            .unwrap_or_default();
        set.extend(added);
        let hooks =
            q_hooks.get(entity).copied().unwrap_or_default() | ActiveCollisionHooks::FILTER_PAIRS;
        commands
            .entity(entity)
            .try_insert(FilteredPairs(set))
            .try_insert(hooks);
    }
}

/// Avian's pair filter, backed by authored [`FilteredPairs`].
///
/// Installed with `PhysicsPlugins::default().with_collision_hooks::<UsdCollisionFilter>()`.
/// Only one set of hooks exists per app, so this is THE filter — anything else
/// that needs to veto a pair belongs here rather than in a second hook.
#[derive(SystemParam)]
pub struct UsdCollisionFilter<'w, 's> {
    filtered: Query<'w, 's, &'static FilteredPairs>,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    shared_tire: Query<'w, 's, (), With<SharedTireContact>>,
    friction: Query<'w, 's, &'static Friction>,
    linear_velocity: Query<'w, 's, &'static LinearVelocity>,
    angular_velocity: Query<'w, 's, &'static AngularVelocity>,
    default_friction: Res<'w, DefaultFriction>,
}

/// Add contact-modification capability without overwriting an authored pair
/// filter that may already be present on the same collider.
pub fn enable_shared_tire_contact_hooks(
    mut commands: Commands,
    q_added: Query<Entity, Added<SharedTireContact>>,
) {
    for entity in &q_added {
        enable_collision_hook(&mut commands, entity, ActiveCollisionHooks::MODIFY_CONTACTS);
    }
}

/// Enable contact modification for materials with distinct static and dynamic
/// coefficients.
///
/// Avian 0.7 stores both on [`Friction`], but its narrow phase currently copies
/// only `dynamic_coefficient` onto contact manifolds. The shared hook restores
/// the authored static branch without classifying bodies by path or vehicle.
pub fn enable_static_friction_contact_hooks(
    mut commands: Commands,
    q_changed: Query<(Entity, &Friction), Changed<Friction>>,
) {
    for (entity, friction) in &q_changed {
        if (friction.static_coefficient - friction.dynamic_coefficient).abs() <= Scalar::EPSILON {
            continue;
        }
        enable_collision_hook(&mut commands, entity, ActiveCollisionHooks::MODIFY_CONTACTS);
    }
}

/// Keep Avian's broad-phase proxy flags in sync with the public hook component.
///
/// Avian 0.7 updates only its custom-filter bit from the `ActiveCollisionHooks`
/// insert observer.  A USD material can legitimately arm contact modification
/// after the collider proxy exists (for example when a streamed material is
/// resolved), so relying on that observer leaves `MODIFY_CONTACTS` absent from
/// the proxy and silently skips [`CollisionHooks::modify_contacts`].  The USD
/// bridge owns the hook component it adds; synchronize the corresponding proxy
/// flags here, without changing any contact or body state.
pub fn synchronize_collision_hook_flags(
    mut trees: ResMut<ColliderTrees>,
    query: Query<(
        &ColliderTreeProxyKey,
        &ActiveCollisionHooks,
        Option<&UsdPrimPath>,
    )>,
) {
    for (key, hooks, prim) in &query {
        if *key == ColliderTreeProxyKey::PLACEHOLDER {
            continue;
        }
        let Some(proxy) = trees.get_proxy_mut(*key) else {
            // The component may arrive before Avian creates the proxy.  In that
            // case the proxy constructor reads the complete component later.
            continue;
        };
        let desired_modify = hooks.contains(ActiveCollisionHooks::MODIFY_CONTACTS);
        let was_modify = proxy.has_contact_modification();
        proxy.flags.set(
            ColliderTreeProxyFlags::CUSTOM_FILTER,
            hooks.contains(ActiveCollisionHooks::FILTER_PAIRS),
        );
        proxy
            .flags
            .set(ColliderTreeProxyFlags::MODIFY_CONTACTS, desired_modify);
        if was_modify != desired_modify {
            info!(
                "[usd-avian] synchronized contact hooks for proxy key={key:?} prim={}: hooks={hooks:?} modify={desired_modify}",
                prim.map_or("<non-usd>", |p| p.path.as_str()),
            );
        }
    }
}

fn use_static_friction(max_tangent_speed: Scalar) -> bool {
    max_tangent_speed <= STATIC_FRICTION_MAX_SLIP_SPEED_MPS
}

impl CollisionHooks for UsdCollisionFilter<'_, '_> {
    fn filter_pairs(&self, collider1: Entity, collider2: Entity, _commands: &mut Commands) -> bool {
        // The hook is handed COLLIDER entities. A collider that is a child entity
        // of a body is filtered by what its BODY names, so both are checked.
        let body1 = self
            .collider_of
            .get(collider1)
            .map(|c| c.body)
            .unwrap_or(collider1);
        let body2 = self
            .collider_of
            .get(collider2)
            .map(|c| c.body)
            .unwrap_or(collider2);

        let names = |a: Entity, b: Entity, b_body: Entity| {
            self.filtered
                .get(a)
                .is_ok_and(|f| f.0.contains(&b) || f.0.contains(&b_body))
        };

        !(names(collider1, collider2, body2)
            || names(body1, collider2, body2)
            || names(collider2, collider1, body1)
            || names(body2, collider1, body1))
    }

    fn modify_contacts(&self, contacts: &mut ContactPair, _commands: &mut Commands) -> bool {
        let surface_friction = |collider: Entity, body: Option<Entity>| {
            self.friction
                .get(collider)
                .ok()
                .copied()
                .or_else(|| body.and_then(|body| self.friction.get(body).ok().copied()))
                .unwrap_or(self.default_friction.0)
        };
        let combined = surface_friction(contacts.collider1, contacts.body1)
            .combine(surface_friction(contacts.collider2, contacts.body2));
        let point_velocity = |body: Option<Entity>, anchor: Vector| {
            let Some(body) = body else {
                return Vector::ZERO;
            };
            let linear = self
                .linear_velocity
                .get(body)
                .map_or(Vector::ZERO, |velocity| velocity.0);
            let angular = self
                .angular_velocity
                .get(body)
                .map_or(Vector::ZERO, |velocity| velocity.0);
            linear + angular.cross(anchor)
        };

        for manifold in &mut contacts.manifolds {
            let max_tangent_speed = manifold
                .points
                .iter()
                .fold(0.0 as Scalar, |maximum, point| {
                    let relative = point_velocity(contacts.body2, point.anchor2)
                        - point_velocity(contacts.body1, point.anchor1);
                    let tangent = relative - manifold.normal * relative.dot(manifold.normal);
                    maximum.max(tangent.length())
                });
            manifold.friction = if use_static_friction(max_tangent_speed) {
                combined.static_coefficient
            } else {
                combined.dynamic_coefficient
            };
        }

        // A wheel body may be represented by a collider child in a future
        // composition, so use the resolved body endpoints rather than assuming
        // the collider entity itself carries the marker.
        let shared = contacts
            .body1
            .is_some_and(|body| self.shared_tire.get(body).is_ok())
            || contacts
                .body2
                .is_some_and(|body| self.shared_tire.get(body).is_ok());
        if shared {
            for manifold in &mut contacts.manifolds {
                // The normal impulse is still generated by Avian.  Only the
                // generic tangent impulse is disabled; mobility supplies the
                // same authored `tire_patch_force` for both realizations.
                manifold.friction = 0.0;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    //! The read and the ownership walk. The filter itself is proven by
    //! `scenes/tests/filtered_pairs.usda`, which needs a stepping solver.

    use super::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};

    /// Two overlapping bodies, one filtering the other by naming its COLLIDER
    /// child — the form that must resolve to the body.
    const PAIRS: &str = r#"#usda 1.0
def Xform "Rig"
{
    def Xform "A" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsFilteredPairsAPI"] )
    {
        rel physics:filteredPairs = </Rig/B/Shell>
        def Cube "Shell" ( prepend apiSchemas = ["PhysicsCollisionAPI"] ) { double size = 1 }
    }
    def Xform "B" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] )
    {
        def Cube "Shell" ( prepend apiSchemas = ["PhysicsCollisionAPI"] ) { double size = 1 }
    }
}
"#;

    #[test]
    fn an_authored_pair_is_read_off_the_prim_that_applies_the_api() {
        let recipe = StageRecipe::from_source("t.usda", PAIRS);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let reader = cs.view();

        let a = SdfPath::new("/Rig/A").unwrap();
        let read = read_filtered_pairs(&reader, &a).expect("A applies the API");
        assert_eq!(read.targets, vec!["/Rig/B/Shell".to_string()]);

        // The other end authors nothing: filtering is symmetric, so one opinion
        // is the whole pair and the loader must not require two.
        let b = SdfPath::new("/Rig/B").unwrap();
        assert!(
            read_filtered_pairs(&reader, &b).is_none(),
            "B applies no {} — reading one would mean the schema is being guessed at",
            ptok::API_FILTERED_PAIRS,
        );
    }

    #[test]
    fn a_prim_without_the_api_is_not_a_filtered_pair() {
        let recipe = StageRecipe::from_source("t.usda", PAIRS);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let reader = cs.view();
        let shell = SdfPath::new("/Rig/B/Shell").unwrap();
        assert!(read_filtered_pairs(&reader, &shell).is_none());
    }

    #[test]
    fn stiction_band_selects_static_only_near_zero_slip() {
        assert!(use_static_friction(0.0));
        assert!(use_static_friction(STATIC_FRICTION_MAX_SLIP_SPEED_MPS));
        assert!(!use_static_friction(
            STATIC_FRICTION_MAX_SLIP_SPEED_MPS + 0.001
        ));
    }
}
