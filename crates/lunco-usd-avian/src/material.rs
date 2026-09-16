//! USD physics material authoring and Avian surface mapping.

use avian3d::prelude::CoefficientCombine;
use openusd::sdf::Path as SdfPath;

/// How two contacting surfaces' coefficients are combined — `PhysxMaterialAPI`'s
/// `physxMaterial:frictionCombineMode`. Also not core UsdPhysics: the spec says
/// what a surface IS, and leaves the pairwise combination to the solver. This is
/// Omniverse's (and PhysX's) name for it, and Avian implements the same rules.
const PHYSX_FRICTION_COMBINE_MODE: &str = "physxMaterial:frictionCombineMode";
const PHYSX_RESTITUTION_COMBINE_MODE: &str = "physxMaterial:restitutionCombineMode";

/// PhysX/Omniverse combine-mode token → Avian's [`CoefficientCombine`].
///
/// `average` is the default in both, so an unauthored mode behaves identically.
/// (Avian additionally offers `GeometricMean`, which PhysX has no token for; it
/// is reachable only from Rust.)
fn combine_mode(token: &str) -> Option<CoefficientCombine> {
    match token {
        "average" => Some(CoefficientCombine::Average),
        "min" => Some(CoefficientCombine::Min),
        "multiply" => Some(CoefficientCombine::Multiply),
        "max" => Some(CoefficientCombine::Max),
        _ => None,
    }
}

/// The surface properties of a bound `UsdPhysicsMaterialAPI` material.
///
/// Dynamic and static friction are kept **separate**, because both USD and Avian
/// model them separately (`physics:dynamicFriction` / `physics:staticFriction`;
/// `Friction::dynamic_coefficient` / `static_coefficient`). Collapsing them to
/// one number — as the old `physics:friction` did — throws away the distinction
/// between "how hard is it to start sliding" and "how hard is it to keep
/// sliding", which for a rover on regolith is exactly the interesting part.
pub(super) struct PhysicsMaterial {
    /// `physics:dynamicFriction` — kinetic, while surfaces slide.
    pub(super) dynamic_friction: Option<f32>,
    /// `physics:staticFriction` — resists the onset of sliding.
    pub(super) static_friction: Option<f32>,
    /// `physics:restitution` — bounciness.
    pub(super) restitution: Option<f32>,
    /// `physics:density` — for bodies that author no mass of their own (stage
    /// units: mass per unit³).
    pub(super) density: Option<f32>,
    /// `physxMaterial:frictionCombineMode` — how THIS surface's friction combines
    /// with whatever it touches.
    pub(super) friction_combine: Option<CoefficientCombine>,
    /// `physxMaterial:restitutionCombineMode`.
    pub(super) restitution_combine: Option<CoefficientCombine>,
}

/// Resolve the physics material bound to `prim` and read its surface properties.
///
/// # Why this is not just an attribute read
///
/// There is no `physics:friction` in UsdPhysics. Friction is
/// `UsdPhysicsMaterialAPI` — `physics:dynamicFriction` / `physics:staticFriction`
/// / `physics:restitution` / `physics:density` — applied to a **`Material`** prim
/// and bound to geometry through the purpose-specific relationship
/// `material:binding:physics`:
///
/// ```usda
/// def Scope "PhysicsMaterials" {
///     def Material "Regolith" (prepend apiSchemas = ["PhysicsMaterialAPI"]) {
///         float physics:dynamicFriction = 1.0
///         float physics:staticFriction  = 1.0
///     }
/// }
/// def Cube "Ground" (prepend apiSchemas = ["PhysicsCollisionAPI"]) {
///     rel material:binding:physics = </World/PhysicsMaterials/Regolith>
/// }
/// ```
///
/// Friction comes off the bound `Material`, never off a bare `physics:friction`
/// on the body prim: that name is not defined by UsdPhysics, so no other
/// physics-aware consumer reads it, and USD is free to give it another meaning.
///
/// Binding resolution — namespace inheritance, and the purpose→all-purpose
/// fallback that lets ONE `Material` drive both look and friction — is SHARED
/// with the renderer ([`lunco_usd_bevy_core::resolve_bound_material`]). A physical and
/// a visual material are the same USD concept bound for different purposes, so
/// they must resolve through the same code or they will drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PhysicsMaterialReadError {
    /// Authored material attribute that failed strict decoding.
    pub(super) attribute: String,
}

impl PhysicsMaterialReadError {
    fn new(attribute: &str) -> Self {
        Self {
            attribute: attribute.to_owned(),
        }
    }
}

impl std::fmt::Display for PhysicsMaterialReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid authored physics material `{}`", self.attribute)
    }
}

impl std::error::Error for PhysicsMaterialReadError {}

pub(super) fn read_physics_material(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    prim: &SdfPath,
) -> Result<Option<PhysicsMaterial>, PhysicsMaterialReadError> {
    use openusd::schemas::physics::tokens as ptok;

    let Some(mat_path) = reader.bound_material(prim, lunco_usd_bevy_core::MaterialPurpose::Physics)
    else {
        return Ok(None);
    };
    let mat = SdfPath::new(&mat_path)
        .map_err(|_| PhysicsMaterialReadError::new(ptok::REL_MATERIAL_BINDING_PHYSICS))?;
    if !reader.has_api_schema(&mat, "PhysicsMaterialAPI") {
        return Ok(None);
    }
    let read_coefficient =
        |attr: &str, upper: Option<f64>| -> Result<Option<f32>, PhysicsMaterialReadError> {
            match super::read_authored_real(reader, &mat, attr)
                .map_err(|_| PhysicsMaterialReadError::new(attr))?
            {
                None => Ok(None),
                Some(value)
                    if value.is_finite()
                        && value >= 0.0
                        && upper.is_none_or(|maximum| value <= maximum)
                        && value <= f32::MAX as f64 =>
                {
                    Ok(Some(value as f32))
                }
                Some(_) => Err(PhysicsMaterialReadError::new(attr)),
            }
        };
    let dynamic_friction = read_coefficient(ptok::A_DYNAMIC_FRICTION, None)?;
    let static_friction = read_coefficient(ptok::A_STATIC_FRICTION, None)?;
    let restitution = read_coefficient(ptok::A_RESTITUTION, Some(1.0))?;
    let density = read_coefficient(ptok::A_DENSITY, None)?;
    let read_combine =
        |attr: &str| -> Result<Option<CoefficientCombine>, PhysicsMaterialReadError> {
            if !reader.has_authored_attribute(&mat, attr) {
                return Ok(None);
            }
            let token = match reader.attr_value(&mat, attr) {
                Some(openusd::sdf::Value::Token(token)) => token.to_string(),
                _ => return Err(PhysicsMaterialReadError::new(attr)),
            };
            Ok(Some(
                combine_mode(&token).ok_or_else(|| PhysicsMaterialReadError::new(attr))?,
            ))
        };
    let friction_combine = read_combine(PHYSX_FRICTION_COMBINE_MODE)?;
    let restitution_combine = read_combine(PHYSX_RESTITUTION_COMBINE_MODE)?;

    // A Material bound only for LOOKS resolves here via the purpose→all-purpose
    // fallback but carries no `PhysicsMaterialAPI` properties. That is not a
    // physics material — don't fabricate a zero-friction one out of it.
    Ok((dynamic_friction.is_some()
        || static_friction.is_some()
        || restitution.is_some()
        || density.is_some())
    .then_some(PhysicsMaterial {
        dynamic_friction,
        static_friction,
        restitution,
        density,
        friction_combine,
        restitution_combine,
    }))
}
