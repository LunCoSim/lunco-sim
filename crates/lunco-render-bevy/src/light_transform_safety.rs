//! Stop malformed spotlight transforms before Bevy constructs a normalized
//! direction or shadow frustum from them.

use bevy::prelude::*;
use lunco_usd_bevy_scene::UsdPrimPath;

/// Prior presentation settings retained while an invalid spotlight is hidden.
#[derive(Component, Debug, Clone, Copy)]
struct InvalidSpotlightTransform {
    visibility: Visibility,
    shadow_maps_enabled: bool,
}

/// Installs the spotlight transform admission check before Bevy visibility and
/// light-frustum systems consume the propagated transform.
pub(super) fn build(app: &mut App) {
    app.add_systems(
        PostUpdate,
        contain_invalid_spotlight_transforms
            .after(TransformSystems::Propagate)
            .before(bevy::light::SimulationLightSystems::UpdateBounds)
            .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility)
            .before(bevy::light::SimulationLightSystems::UpdateLightFrusta),
    );
}

fn contain_invalid_spotlight_transforms(
    mut commands: Commands,
    mut lights: Query<(
        Entity,
        &GlobalTransform,
        &mut bevy::light::SpotLight,
        &mut Visibility,
        Option<&UsdPrimPath>,
        Option<&Name>,
        Option<&InvalidSpotlightTransform>,
    )>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let mut findings = Vec::new();

    for (entity, transform, mut light, mut visibility, prim_path, name, invalid) in &mut lights {
        if !spotlight_transform_is_safe(transform) {
            let subject = prim_path
                .map(|path| path.path.clone())
                .or_else(|| name.map(ToString::to_string))
                .unwrap_or_else(|| format!("spotlight {entity:?}"));
            let detail = "the propagated spotlight transform cannot produce a finite normalized direction; the light was hidden and its shadow map disabled";

            if invalid.is_none() {
                commands.entity(entity).insert(InvalidSpotlightTransform {
                    visibility: *visibility,
                    shadow_maps_enabled: light.shadow_maps_enabled,
                });
            }
            light.shadow_maps_enabled = false;
            *visibility = Visibility::Hidden;

            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "spotlight-transform-invalid",
                    Some(entity),
                    subject.clone(),
                    detail,
                );
            }
            findings.push(lunco_core::RuntimeDiagnostic {
                code: "spotlight-transform-invalid".to_owned(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "render-light-transform-safety".to_owned(),
                subject,
                message: detail.to_owned(),
            });
        } else if let Some(invalid) = invalid {
            *visibility = invalid.visibility;
            light.shadow_maps_enabled = invalid.shadow_maps_enabled;
            commands
                .entity(entity)
                .remove::<InvalidSpotlightTransform>();
        }
    }

    if let Some(mut diagnostics) = diagnostics {
        diagnostics.replace_producer("render-light-transform-safety", findings);
    }
}

fn spotlight_transform_is_safe(transform: &GlobalTransform) -> bool {
    if !transform.to_matrix().is_finite() {
        return false;
    }

    let (_, rotation, translation) = transform.to_scale_rotation_translation();
    if !translation.is_finite() || !rotation.to_array().iter().all(|value| value.is_finite()) {
        return false;
    }

    // Bevy's spotlight systems wrap this axis in `Dir3::new_unchecked`, which
    // panics in debug builds when its squared length is not finite and unit.
    let backward = rotation * Vec3::Z;
    let length_squared = backward.length_squared();
    length_squared.is_finite() && (length_squared - 1.0).abs() <= 2.0e-2
}
