//! Per-entity gravity — plugin replacement for avian3d::prelude::Gravity.
//!
//! ## Usage
//!
//! ```rust
//! # use bevy::prelude::*;
//! # use bevy::math::DVec3;
//! # use lunco_environment::Gravity;
//! # let mut app = App::new();
//! // Sandbox / flat ground:
//! app.insert_resource(Gravity::flat(9.81, DVec3::NEG_Y));
//!
//! // Full client (surface gravity):
//! app.insert_resource(Gravity::surface());
//! ```
//!
//! ## Architecture
//!
//! The gravity system runs in `FixedUpdate` and automatically applies forces
//! to all `RigidBody` entities. This is a drop-in replacement for Avian3D's
//! built-in gravity.
//!
//! ### Gravity modes
//!
//! - **`Gravity::Flat`** — constant gravity, same for all entities.
//!   Used for sandbox, tests, and flat-ground simulations.
//!   Equivalent to `avian3d::prelude::Gravity`.
//!
//! - **`Gravity::Surface`** — surface gravity for spherical bodies.
//!   Direction = `-normalize(entity_body_local_position)`.
//!   Entities must have `GravityBody` to identify which body they're on.
//!
//! ### Body-local positions
//!
//! In the merged Body+Grid design, the Body entity IS the Grid. Surface
//! entities (rovers, tiles) are children of Body/Grid. Their `Transform.translation`
//! is in the body-fixed frame (origin = body center). For these entities,
//! `Transform.translation` IS the body-local position — no Grid lookup needed.
//!
//! For orbit cameras and entities NOT on the Body/Grid, we compute the
//! absolute position and subtract the body's absolute position to get
//! the body-relative offset.

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};

// Gravity configuration *types* now live in `lunco-environment` (environmental
// state, sibling to lighting). This crate owns only the `PointMassGravity`
// model impl, the cached `LocalGravityField`, and the system that fills it.
use lunco_environment::{Gravity, GravityBody, GravityModel, GravityProvider};

// ─────────────────────────────────────────────────────────────────────────────
// Gravity models (orbital / multi-body)
// ─────────────────────────────────────────────────────────────────────────────

/// Point-mass gravity: a = GM/r² toward center. A [`GravityModel`] impl used as
/// the `model` inside a [`GravityProvider`].
pub struct PointMassGravity {
    pub gm: f64,
}

impl GravityModel for PointMassGravity {
    fn acceleration(&self, relative_pos: DVec3) -> DVec3 {
        let r2 = relative_pos.length_squared();
        if r2 < 1.0 {
            return DVec3::ZERO;
        }
        let r = r2.sqrt();
        -relative_pos * (self.gm / (r * r2))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Local gravity field (cached for camera/UI)
// ─────────────────────────────────────────────────────────────────────────────

/// Cached gravity state at the avatar's position.
///
/// Camera and UI systems read this resource to determine "up" direction
/// and surface gravity magnitude. Updated each frame in `PreUpdate`.
#[derive(Resource)]
pub struct LocalGravityField {
    /// The body we're gravitationally bound to.
    pub body_entity: Option<Entity>,
    /// Avatar position relative to the bound body's centre, expressed in the
    /// body's rotating frame.  This is the canonical position for surface
    /// decisions: it is composed through the shared BigSpace grid branch and
    /// never obtained by subtracting two root-frame `GlobalTransform`s.
    pub body_relative_position: DVec3,
    /// "Up" direction in world space.
    pub up: DVec3,
    /// "Up" direction in body-local space.
    pub local_up: DVec3,
    /// Surface gravity magnitude (m/s²).
    pub surface_g: f64,
}

impl Default for LocalGravityField {
    fn default() -> Self {
        Self {
            body_entity: None,
            body_relative_position: DVec3::ZERO,
            up: DVec3::Y,
            local_up: DVec3::Y,
            surface_g: 0.0,
        }
    }
}

// Note: Gravity force application moved to `lunco-environment`.
// See `lunco_environment::apply_gravity_to_rigid_bodies` — it consumes the
// per-entity `LocalGravity` component instead of recomputing per tick.

// ─────────────────────────────────────────────────────────────────────────────
// Local gravity field update (camera/UI)
// ─────────────────────────────────────────────────────────────────────────────

/// Updates `LocalGravityField` based on avatar position.
///
/// Uses the avatar's body-relative position composed through the shared local
/// BigSpace branch. This correctly handles nested grids, reparenting, and a
/// rotating celestial frame without subtracting astronomical root positions.
///
/// Runs in `PreUpdate` so camera systems see fresh data.
pub fn update_local_gravity_field(
    q_avatar: Query<
        (
            Entity,
            &Transform,
            &CellCoord,
            &ChildOf,
            Option<&GravityBody>,
        ),
        With<lunco_core::Avatar>,
    >,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_bodies: Query<&GravityProvider>,
    gravity: Res<Gravity>,
    mut field: ResMut<LocalGravityField>,
    orbital_pin: Res<crate::placement::OrbitalViewPin>,
) {
    // Orbital VIEW active: the camera has flown to the focused body, but the
    // scene/physics stayed at the site. A field computed at the camera's
    // position would be garbage for the site content (Earth gravity at the
    // Moon site), so HOLD the last surface field until the view returns.
    if orbital_pin.active {
        return;
    }
    let Some((avatar_ent, _tf, _cell, _, gravity_body)) = q_avatar.iter().next() else {
        *field = LocalGravityField::default();
        return;
    };

    let derived = if let Some(gb) = gravity_body {
        let Some(derived) = gravity_at_body(
            gb.body_entity,
            avatar_ent,
            &q_parents,
            &q_grids,
            &q_spatial,
            &q_bodies,
        ) else {
            error_once!(
                    "cannot publish gravity for avatar {:?}: body {:?} is disconnected or has no gravity provider",
                    avatar_ent,
                    gb.body_entity
                );
            return;
        };
        derived
    } else {
        (DVec3::ZERO, DVec3::Y, DVec3::Y, 0.0)
    };
    let (body_relative_position, body_up_local, body_up_world, surface_g) = derived;

    // Publish the association and all values together only after the complete
    // body-frame evaluation succeeded. A structural failure therefore cannot
    // pair a new body id with stale vectors from the preceding frame.
    field.body_entity = gravity_body.map(|gb| gb.body_entity);

    field.body_relative_position = body_relative_position;
    field.surface_g = surface_g;

    field.local_up = body_up_local;
    field.up = body_up_world;

    // For flat gravity, use the configured g.
    if let Gravity::Flat { g, direction } = gravity.as_ref() {
        field.surface_g = *g;
        field.local_up = -*direction / direction.length();
        field.up = field.local_up;
    }
}

/// Evaluate the active body's gravity in its own rotating frame and publish
/// both sides of the frame boundary. `GravityModel::acceleration` receives a
/// body-fixed relative position. When camera and body share a local grid
/// branch, that relative position is composed in the branch directly instead
/// of subtracting two astronomical root-frame positions.
fn gravity_at_body(
    body_entity: Entity,
    camera_entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    q_bodies: &Query<&GravityProvider>,
) -> Option<(DVec3, DVec3, DVec3, f64)> {
    let (_, body_rotation) =
        lunco_core::coords::world_pose(body_entity, q_parents, q_grids, q_spatial)?;
    let relative_body =
        local_body_relative_position(body_entity, camera_entity, q_parents, q_grids, q_spatial)?;
    let provider = q_bodies.get(body_entity).ok()?;
    let acceleration = provider.model.acceleration(relative_body);
    let magnitude = acceleration.length();
    let up_body = if magnitude > 1e-12 {
        -acceleration / magnitude
    } else {
        // At the exact centre a radial gravity direction is physically
        // undefined.  +Y is the canonical frame basis, not an approximation.
        DVec3::Y
    };
    Some((relative_body, up_body, body_rotation.0 * up_body, magnitude))
}

/// Express a camera position in the active body's body-fixed frame without
/// traversing their shared astronomical ancestors. The two entities are
/// siblings beneath the body frame: the camera is under a surface grid and
/// the body is at the body-frame origin.
fn local_body_relative_position(
    body_entity: Entity,
    camera_entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<DVec3> {
    let body_frame = q_parents.get(body_entity).ok()?.parent();
    let (camera_position, _) =
        lunco_core::coords::pose_in_grid(camera_entity, body_frame, q_parents, q_grids, q_spatial)?;
    let (body_position, body_rotation) = lunco_core::coords::grid_relative_pose(
        body_entity,
        body_frame,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    Some(body_rotation.inverse() * (camera_position - body_position))
}

#[cfg(test)]
mod tests {
    use super::*;
    use big_space::plugin::BigSpaceMinimalPlugins;
    use big_space::prelude::BigSpaceRootBundle;

    #[test]
    fn local_gravity_is_owned_by_the_avatar_gravity_body() {
        let mut app = App::new();
        app.add_plugins(BigSpaceMinimalPlugins);
        app.insert_resource(Gravity::surface());
        app.init_resource::<LocalGravityField>();
        app.init_resource::<crate::placement::OrbitalViewPin>();
        app.add_systems(Update, update_local_gravity_field);

        let grid = app
            .world_mut()
            .spawn((
                Grid::new(10_000.0, 1_000.0),
                CellCoord::default(),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut()
            .spawn(BigSpaceRootBundle::default())
            .add_child(grid);

        let body = app
            .world_mut()
            .spawn((
                CellCoord::default(),
                Transform::default(),
                GlobalTransform::default(),
                GravityProvider {
                    model: Box::new(PointMassGravity { gm: 100.0 }),
                },
            ))
            .id();

        // This entity deliberately satisfies the former broad spatial query.
        // It must never be allowed to become the camera/UI gravity source.
        let distractor = app
            .world_mut()
            .spawn((
                CellCoord::default(),
                Transform::from_xyz(0.0, 500.0, 0.0),
                GlobalTransform::default(),
            ))
            .id();

        let avatar = app
            .world_mut()
            .spawn((
                lunco_core::Avatar,
                CellCoord::default(),
                Transform::from_xyz(10.0, 0.0, 0.0),
                GlobalTransform::default(),
                GravityBody { body_entity: body },
            ))
            .id();
        app.world_mut()
            .entity_mut(grid)
            .add_children(&[body, distractor, avatar]);

        app.update();

        let field = app.world().resource::<LocalGravityField>();
        assert_eq!(field.body_entity, Some(body));
        assert!(field
            .body_relative_position
            .abs_diff_eq(DVec3::X * 10.0, 1e-9));
        assert!(field.local_up.abs_diff_eq(DVec3::X, 1e-9));

        // Removing the authoritative association must clear it immediately;
        // consumers must not silently keep using a former body's frame.
        app.world_mut().entity_mut(avatar).remove::<GravityBody>();
        app.update();
        let field = app.world().resource::<LocalGravityField>();
        assert_eq!(field.body_entity, None);
        assert_eq!(field.body_relative_position, DVec3::ZERO);
    }
}
