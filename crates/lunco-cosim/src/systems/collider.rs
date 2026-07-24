//! Collider synchronization from simulation outputs.
//!
//! Watches [`crate::SimComponent`] outputs for `volume` and updates
//! the entity's [`Collider`] to a sphere with the corresponding radius.

use avian3d::prelude::{Collider, RigidBody};
use bevy::prelude::*;

use crate::SimComponent;

/// Last `volume` value applied to an entity's collider by [`sync_collider`].
///
/// The sphere radius is a pure function of `volume`, so rebuilding the
/// [`Collider`] when the value is unchanged just reallocates the same shape
/// every frame. This memo gates the rebuild on an actual change.
#[derive(Component)]
pub struct LastColliderVolume(pub f64);

/// Updates [`Collider`] from simulation output values.
///
/// Currently supports:
/// - `volume` → `Collider::sphere(cbrt(3*V/(4π)))`
///
/// ## Execution
///
/// Runs in [`Update`] (not FixedUpdate) because collider changes are
/// expensive and don't need per-physics-step precision. Rebuilds only when
/// `volume` actually changes (see [`LastColliderVolume`]) — a constant-volume
/// body would otherwise reallocate its collider every frame.
pub fn sync_collider(
    mut commands: Commands,
    q_components: Query<(Entity, &SimComponent, Option<&LastColliderVolume>), With<RigidBody>>,
    mut q_colliders: Query<&mut Collider>,
) {
    for (entity, comp, last) in &q_components {
        // Check for volume output → update sphere collider radius
        if let Some(&volume) = comp.outputs.get("volume") {
            if volume > 0.0 {
                // Skip the rebuild when the volume is bit-identical to the last
                // one we applied — exact compare is correct here (we only skip
                // when nothing changed; any change, however small, rebuilds).
                if matches!(last, Some(l) if l.0 == volume) {
                    continue;
                }
                let radius = ((3.0 * volume) / (4.0 * std::f64::consts::PI)).cbrt();
                if let Ok(mut collider) = q_colliders.get_mut(entity) {
                    *collider = Collider::sphere(radius);
                    // `try_insert`, not `insert`: a `LoadScene` scene-reload or
                    // obstacle-field churn can despawn this body between the query
                    // and this deferred command applying. Plain `.insert` panics on
                    // the dead entity under Bevy 0.18's command error handler
                    // (observed crashing the sandbox); `try_insert` is a no-op — a
                    // despawned body has no collider to gate, so skipping is correct.
                    commands
                        .entity(entity)
                        .try_insert(LastColliderVolume(volume));
                }
            }
        }
    }
}
