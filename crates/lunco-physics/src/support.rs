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

/// The default initialization policy for every USD-authored dynamic body.
pub const STRICT_AUTHORED_INITIALIZATION_POLICY: &str = "strict-authored";

/// Hook prefix for Twin-authored initialization policies.
pub const PHYSICS_INITIALIZATION_HOOK_PREFIX: &str = "physics.initialization.";

/// Authored policy selected before a dynamic body crosses the physics admission
/// boundary.
///
/// This is deliberately an initialization policy, not a grounding mode. The
/// default policy validates the composed USD pose and accepts it unchanged. A
/// Twin may name a custom policy and provide the corresponding deterministic
/// `LunCoPolicy` hook. The engine never silently changes a policy name or falls
/// back to a terrain placement algorithm.
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

    /// Hook id used for an explicit custom policy.
    pub fn hook_id(&self) -> String {
        format!("{PHYSICS_INITIALIZATION_HOOK_PREFIX}{}", self.0)
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

/// Evaluate a named initialization policy without giving it a pose mutation
/// capability. Built-in strict-authored is handled by the engine; custom
/// policies are deterministic hook decisions and must return exactly
/// `"accept"` or `"reject"`.
pub fn evaluate_initialization_policy(
    policy: &PhysicsInitializationPolicy,
    facts: lunco_hooks::HookValue,
) -> Result<(), String> {
    if policy.is_strict_authored() {
        return Ok(());
    }
    let hook_id = policy.hook_id();
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
    let decision = match hook.hook.invoke(&[facts]) {
        Err(error) => {
            return Err(format!(
                "initialization policy `{}` failed: {error}",
                policy.0
            ));
        }
        Ok(value) => value,
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
    fn custom_initialization_policy_is_an_explicit_hook_name() {
        let policy = PhysicsInitializationPolicy::new("lander-contact").unwrap();
        assert!(!policy.is_strict_authored());
        assert_eq!(policy.hook_id(), "physics.initialization.lander-contact");
    }

    #[test]
    fn malformed_initialization_policy_names_are_rejected_without_fallback() {
        assert!(PhysicsInitializationPolicy::new("").is_err());
        assert!(PhysicsInitializationPolicy::new("terrain contact").is_err());
    }
}
