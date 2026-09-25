//! Runtime support geometry shared by physics producers and terrain.
//!
//! Avian colliders already describe the support footprint of ordinary rigid
//! bodies. A physics model that deliberately has no collider (for example a
//! raycast suspension or a probe-based landing leg) still has real spatial
//! support geometry. It publishes that geometry here instead of making the
//! terrain know about the model that produced it.

use bevy::math::DVec3;
use bevy::prelude::*;

/// Ordering boundary for the shared support-footprint contract.
///
/// Physics producers publish their authored support geometry before terrain or
/// any other spatial consumer decides residency and initial-state validation.
/// Keeping this boundary in the physics contract avoids coupling a consumer to a
/// particular mobility implementation or relying on plugin insertion order.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicsSupportSet {
    /// Runtime physics models publish support geometry for the current world.
    Publish,
    /// Apply deferred publisher changes before support consumers inspect them.
    Apply,
    /// Terrain and other spatial systems consume the published support contract.
    Consume,
}

/// A live edge in the physics assembly graph.
///
/// Physics producers publish this before the native joint is admitted to the
/// solver. Spatial consumers therefore see the complete articulated assembly
/// during startup placement as well as during normal simulation. The link is
/// removed with its owning joint entity; it is not a second constraint.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsJointLink {
    /// The first body in the authored or synthesized joint.
    pub body0: Entity,
    /// The second body in the authored or synthesized joint.
    pub body1: Entity,
}

/// Requests the physics bridge to retire a live joint through its native
/// constraint lifecycle.
///
/// Scene/document commands must never despawn an Avian joint directly: a
/// despawn removes several components in an unspecified order and can make the
/// solver's joint graph and island counters disagree.  The command layer only
/// writes this marker; [`lunco_usd_avian`] consumes it at its owned lifecycle
/// boundary and performs one graph retirement transaction before despawning.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhysicsJointDetachRequested;

/// Records authored joint paths that were detached from a live physics graph.
///
/// The composed USD topology is immutable for a stage generation, while an
/// interactive `DetachJoint` is a live lifecycle transition. Keeping this
/// small, endpoint-local set lets the generic dynamic-admission gate
/// distinguish a deliberately released joint from one whose native constraint
/// has not arrived yet. Paths are qualified by the endpoint's own stage/entity
/// ownership; a replacement entity starts with an empty set.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct PhysicsJointDetachSet {
    /// Authored USD joint paths detached from this body during its lifetime.
    pub joint_paths: Vec<String>,
}

impl PhysicsJointDetachSet {
    /// Record one path idempotently. Empty paths represent synthesized joints
    /// and are intentionally ignored because they are not in authored USD
    /// topology and therefore do not need admission invalidation.
    pub fn record(&mut self, path: impl Into<String>) {
        let path = path.into();
        if path.is_empty() || self.joint_paths.iter().any(|existing| existing == &path) {
            return;
        }
        self.joint_paths.push(path);
    }

    /// Whether this body has observed a detach for the authored joint path.
    pub fn contains(&self, path: &str) -> bool {
        self.joint_paths.iter().any(|existing| existing == path)
    }
}

/// Marks an authored physics joint whose native constraint has not crossed the
/// admission boundary yet.
///
/// This remains present while USD is resolving the endpoints and while Avian is
/// parking the typed constraint. It is removed only when the native joint is
/// installed together with its collision policy. Terrain placement must wait
/// for that complete topology phase: moving only the root while an authored
/// child body is still waiting to resolve leaves a real joint violation for the
/// solver to repair on its first step.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhysicsJointPending;

/// Marks an authored joint whose USD endpoints have not yet been projected into
/// the runtime joint graph.
///
/// This closes authored topology before initial-pose validation. Once the
/// resolved body link and typed native constraint are published, this marker
/// clears even if Avian still has to admit the constraint to a solver island.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhysicsJointTopologyPending;

/// The default initialization policy for every USD-authored dynamic body.
pub const STRICT_AUTHORED_INITIALIZATION_POLICY: &str = "strict-authored";

/// Declared Twin policy seam for dynamic-body initialization decisions.
pub const PHYSICS_INITIALIZATION_POLICY_HOOK: &str = "physics.initialization";

lunco_hooks::declare_hook! {
    id: PHYSICS_INITIALIZATION_POLICY_HOOK,
    owner: "lunco-physics",
    description: "Decide whether an authored rigid-body pose is admissible.",
    signature: [facts: Map],
    output: String,
    deterministic: true,
    required: false,
    installable: true,
}

/// Authored policy selected before a dynamic body crosses the physics admission
/// boundary.
///
/// This is deliberately an initialization policy, not a grounding mode. The
/// default policy validates the composed USD pose and accepts it unchanged. A
/// Twin may select a named policy and provide the declared deterministic
/// `physics.initialization` hook. Rhai interprets the selected name; Rust never
/// turns it into a dynamic hook id or falls back to terrain placement.
#[derive(Component, Debug, Clone, Reflect, PartialEq, Eq)]
#[reflect(Component)]
pub struct PhysicsInitializationPolicy(pub String);

impl Default for PhysicsInitializationPolicy {
    fn default() -> Self {
        Self(STRICT_AUTHORED_INITIALIZATION_POLICY.to_string())
    }
}

impl PhysicsInitializationPolicy {
    /// Construct a policy after checking the authored name's basic contract.
    pub fn new(name: impl Into<String>) -> Result<Self, &'static str> {
        let name = name.into();
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(
                "physics initialization policy name must be non-empty and contain no whitespace",
            );
        }
        Ok(Self(name))
    }

    /// Whether this is the built-in strict authored-pose policy.
    pub fn is_strict_authored(&self) -> bool {
        self.0 == STRICT_AUTHORED_INITIALIZATION_POLICY
    }
}

/// A dynamic body is waiting for its initial pose to be validated by the
/// selected policy. It remains kinematic while this marker is present.
#[derive(Component, Debug, Default, Clone, Copy, Reflect, PartialEq, Eq)]
#[reflect(Component)]
pub struct PhysicsInitializationPending;

/// Marks an authored initialization-policy error. This is separate from the
/// pending marker so diagnostics can distinguish malformed authoring from a
/// policy that is still waiting on terrain or a custom hook.
#[derive(Component, Debug, Default, Clone, Copy, Reflect, PartialEq, Eq)]
#[reflect(Component)]
pub struct PhysicsInitializationInvalid;

/// Stable authored subject attached to a runtime body for diagnostics and hook
/// facts. Keeping this on the body avoids reconstructing a USD path in a
/// terrain or policy consumer.
#[derive(Component, Debug, Clone, Reflect, PartialEq, Eq)]
#[reflect(Component)]
pub struct PhysicsInitializationSubject(pub String);

/// Set by a domain plugin that owns additional initial-pose facts, such as the
/// terrain surface. When absent, the generic physics gate can still validate
/// finite authored poses and admit bodies in a terrain-free scene.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsInitializationExternalValidator(pub bool);

/// Construct the lifecycle context for a physics initialization decision.
///
/// An active transition owns projected bodies until its completion edge; once
/// no transition is active, the latest committed scene owns new admission.
/// Missing generation is an error so a custom policy cannot run with an
/// invented Twin identity.
pub fn physics_initialization_context(
    coordinator: Option<&lunco_core::SceneTransitionCoordinator>,
) -> Result<lunco_core::RuntimeExecutionContext, String> {
    let generation = coordinator
        .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation)
        .ok_or_else(|| {
            "physics initialization policy requires an active or committed scene generation"
                .to_string()
        })?;
    Ok(lunco_core::RuntimeExecutionContext {
        route: Some(lunco_core::RuntimeRoute::twin(
            lunco_core::RuntimeCycle::Lifecycle,
            generation,
        )),
        phase: lunco_core::RuntimePhase::Preparation,
        clock: lunco_core::RuntimeClock::None,
        time_seconds: None,
        delta_seconds: None,
        sequence: None,
        producer: None,
    })
}

/// Stable facts shared by both physics initialization paths. ECS entity ids
/// are process-local handles, not replay-stable identity, so they do not cross
/// the hook boundary.
pub fn physics_initialization_facts(
    policy: &PhysicsInitializationPolicy,
    subject: &str,
    position: DVec3,
    assembly_member_count: usize,
) -> lunco_hooks::HookValue {
    lunco_hooks::HookValue::map([
        ("subject", lunco_hooks::HookValue::str(subject)),
        ("policy", lunco_hooks::HookValue::str(policy.0.clone())),
        (
            "position",
            lunco_hooks::HookValue::Array(
                [position.x, position.y, position.z]
                    .into_iter()
                    .map(lunco_hooks::HookValue::Float)
                    .collect(),
            ),
        ),
        (
            "assembly_member_count",
            lunco_hooks::HookValue::UInt(assembly_member_count as u64),
        ),
    ])
}

/// Evaluate a named initialization policy without giving it a pose mutation
/// capability. Built-in strict-authored is handled by the engine; custom
/// policies are deterministic hook decisions and must return exactly
/// `"accept"` or `"reject"`.
pub fn evaluate_initialization_policy(
    policy: &PhysicsInitializationPolicy,
    facts: lunco_hooks::HookValue,
    context: Result<lunco_core::RuntimeExecutionContext, String>,
) -> Result<(), String> {
    if policy.is_strict_authored() {
        return Ok(());
    }
    let context = context?;
    let hook_id = PHYSICS_INITIALIZATION_POLICY_HOOK;
    let Some(route) = context.route else {
        return Err(
            "physics initialization policy requires a classified Twin lifecycle preparation context"
                .to_string(),
        );
    };
    if route.scope != lunco_core::RuntimeScope::Twin
        || route.cycle != lunco_core::RuntimeCycle::Lifecycle
        || route.generation == 0
        || context.phase != lunco_core::RuntimePhase::Preparation
        || context.clock != lunco_core::RuntimeClock::None
        || context.time_seconds.is_some()
        || context.delta_seconds.is_some()
        || context.sequence.is_some()
        || context.producer.is_some()
    {
        return Err(
            "physics initialization policy requires a classified Twin lifecycle preparation context"
                .to_string(),
        );
    }
    let Some(hook) = lunco_hooks::get(&hook_id) else {
        return Err(format!(
            "initialization policy `{}` is selected but hook `{hook_id}` is not registered",
            policy.0
        ));
    };
    if !hook.deterministic {
        return Err(format!(
            "initialization policy `{}` must be registered as deterministic",
            policy.0
        ));
    }
    let decision = match lunco_hooks::invoke_with_context(hook_id, &[facts], context) {
        None => {
            return Err(format!(
                "initialization policy `{}` became unavailable before invocation",
                policy.0
            ));
        }
        Some(Err(error)) => {
            return Err(format!(
                "initialization policy `{}` failed: {error}",
                policy.0
            ));
        }
        Some(Ok(value)) => value,
    };
    match decision.as_str() {
        Some("accept") => Ok(()),
        Some("reject") => Err(format!(
            "initialization policy `{}` rejected the authored pose",
            policy.0
        )),
        _ => Err(format!(
            "initialization policy `{}` must return the string `accept` or `reject`",
            policy.0
        )),
    }
}

/// Contact geometry that contributes to a body's terrain-support footprint.
///
/// Offsets are expressed in the owning body's local physics frame. The
/// publisher owns the meaning of the probe; consumers only need a conservative
/// radius around its transformed centre. A normal rigid body does not need this
/// component because its Avian collider AABBs are aggregated automatically.
#[derive(Component, Debug, Clone, Reflect, PartialEq)]
#[reflect(Component)]
pub struct PhysicsSupportFootprint(pub Vec<PhysicsSupportContact>);

/// Runtime result of evaluating a published support footprint.
///
/// [`PhysicsSupportFootprint`] describes where a model may contact the
/// environment. This component is the per-tick observation of that geometry;
/// it is kept separate so a query cannot mistake an authored probe count for a
/// live contact count. Producers update it after their native contact/raycast
/// solve, and consumers treat an absent component as unavailable evidence.
#[derive(Component, Debug, Clone, Copy, Reflect, PartialEq, Eq)]
#[reflect(Component)]
pub struct PhysicsSupportState {
    /// Number of footprint probes with a valid contact this sample.
    pub active_contact_count: u32,
    /// Fixed-step sample at which the contact count was evaluated.
    pub sample_tick: u64,
}

/// Native contact evidence published by a raycast-wheel realization.
///
/// The producer (currently `lunco-mobility`) owns the raycast and suspension
/// semantics; consumers only see the resulting sample. Keeping this contract
/// in the physics substrate lets scene queries expose effective contacts
/// without depending on the mobility crate or guessing from authored support
/// footprints. `hit_entity` is the actual Avian collider selected by the
/// wheel's ray, not an inferred terrain/body name.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct PhysicsWheelContact {
    /// Rigid body that receives this wheel's suspension and tire forces.
    pub owner: Entity,
    /// Whether the selected hit has a finite, non-degenerate normal and lies
    /// within the authored suspension travel.
    pub contact_valid: bool,
    /// Actual collider selected by the native raycast, when one exists.
    pub hit_entity: Option<Entity>,
    /// Native ray distance to the selected hit, if any.
    pub distance_m: Option<f64>,
    /// Native contact normal in world physics coordinates; zero means no hit.
    pub normal: DVec3,
    /// Normal spring/damper force actually published by suspension.
    pub normal_force_n: f64,
    /// Authored suspension rest length used for the contact decision.
    pub suspension_rest_length_m: f64,
    /// Effective spring compression for this sample.
    pub suspension_compression_m: f64,
    /// Resultant tire force applied to the owner in this sample.
    pub tire_force: DVec3,
    /// Number of raw Avian ray hits returned before validity filtering.
    pub ray_hit_count: u32,
    /// Number of raw hits with a finite, non-degenerate normal.
    pub valid_ray_hit_count: u32,
    /// Number of collider entities currently excluded by the wheel raycast
    /// filter (assembly ownership diagnostics).
    pub raycast_filter_excluded_entity_count: u32,
    /// Effective ray origin in world physics coordinates.
    pub ray_origin: DVec3,
    /// Effective ray direction in world physics coordinates.
    pub ray_direction: DVec3,
    /// Effective native ray length for this sample.
    pub ray_max_distance_m: f64,
    /// Fixed-step sample at which this snapshot was published.
    pub sample_tick: u64,
}

/// The effective collider exclusion set used by one raycast wheel.
///
/// This is a topology diagnostic, not an authored vehicle property. It is
/// refreshed by the mobility producer only when the connected-body graph or
/// wheel caster changes, so exposing exact members does not allocate in the
/// fixed-step contact publication path. Consumers can resolve the entities to
/// API IDs and USD paths without knowing how the wheel was realized.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct PhysicsWheelRaycastFilter {
    /// Bodies and colliders excluded from this wheel's native raycast.
    pub excluded_entities: Vec<Entity>,
    /// Deterministic signature of the current exclusion set. The producer
    /// compares this without allocating in its fixed-step publication path.
    pub signature: u64,
}

impl Default for PhysicsSupportState {
    fn default() -> Self {
        Self {
            active_contact_count: 0,
            sample_tick: 0,
        }
    }
}

/// One support contact in a body's local physics frame.
///
/// The probe description is part of the contract because initial-state validation
/// must inspect the same collider surface that the live support query uses. A
/// terrain oracle and a sampled heightfield can legitimately differ between
/// lattice points; validation therefore consumes the authoritative spatial
/// query rather than inventing a second surface approximation.
#[derive(Debug, Clone, Copy, Reflect, PartialEq)]
pub struct PhysicsSupportContact {
    /// Contact centre relative to the owning body's origin.
    pub local_offset: DVec3,
    /// Conservative horizontal support radius in metres.
    pub radius: f64,
    /// Probe origin relative to the owning body's origin.
    pub probe_origin: DVec3,
    /// Probe direction in the owning body's local physics frame.
    pub probe_direction: DVec3,
    /// Probe distance at the authored/rest support pose.
    pub probe_length: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_defaults_to_strict_authored_pose() {
        let policy = PhysicsInitializationPolicy::default();
        assert!(policy.is_strict_authored());
        assert_eq!(policy.0, STRICT_AUTHORED_INITIALIZATION_POLICY);
    }

    #[test]
    fn custom_initialization_policy_is_a_named_selector() {
        let policy = PhysicsInitializationPolicy::new("lander-contact").unwrap();
        assert!(!policy.is_strict_authored());
        assert_eq!(policy.0, "lander-contact");
    }

    #[test]
    fn malformed_initialization_policy_names_are_rejected_without_fallback() {
        assert!(PhysicsInitializationPolicy::new("").is_err());
        assert!(PhysicsInitializationPolicy::new("terrain contact").is_err());
    }
}
