//! USD `timeSamples` animation projection for Bevy.
//!
//! The plugin owns only time-domain binding, topology planning, and sampling
//! of render-free transform/appearance intent. Initial scene projection and
//! the `CanonicalStages` resource are owned by `lunco-usd-bevy`; install this
//! plugin after that visual adapter.

use bevy::prelude::*;
use openusd::sdf::Path as SdfPath;
use openusd::sdf::Value;

use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd_bevy_core::animation::animated_time_range;
use lunco_usd_bevy_core::canonical::CanonicalStages;
use lunco_usd_bevy_core::read::{
    attr_has_time_samples, read_primvar_vec3_at, read_token_at, read_vec3_f64_at,
    stage_time_codes_per_second,
};
use lunco_usd_bevy_core::{
    compose_xform_order_at, resolve_bound_shader, stage_convention, UsdReadObject, UsdStageAsset,
};
use lunco_usd_bevy_scene::{UsdAnimated, UsdPrimPath};

/// Install the USD animation planner and samplers.
///
/// The visual adapter must be installed first because it initializes
/// `UsdStageAsset`, `CanonicalStages`, and the scene entities carrying
/// `UsdAnimated`. This package intentionally does not install visual plugins.
pub struct UsdAnimationPlugin;

impl Plugin for UsdAnimationPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        app.add_systems(
            Update,
            (
                bind_animated_to_preview,
                clear_animation_plans_on_stage_reload.run_if(
                    bevy::ecs::schedule::common_conditions::on_message::<AssetEvent<UsdStageAsset>>,
                ),
                plan_usd_animation,
                (sample_usd_animation, sample_usd_material_animation)
                    .after(lunco_time::DomainResolveSet),
            )
                .chain(),
        );
    }
}

/// The transform channel that drives an [`AnimationPlan`] prim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XformDrive {
    /// The authored `xformOpOrder` stack is sampled as a whole.
    OpOrder,
    /// No valid transform stack was authored.
    None,
}

/// Resolved material-animation channels cached for one animated prim.
#[derive(Debug, Clone)]
pub struct MaterialPlan {
    /// Bound `UsdPreviewSurface`, if any.
    pub shader: Option<SdfPath>,
    /// The shader's diffuse-color input carries time samples.
    pub diffuse: bool,
    /// The geometry display-color primvar carries time samples.
    pub geom_color: bool,
    /// The shader's opacity input carries time samples.
    pub opacity: bool,
}

/// Tier-one memo of an animated prim's authored channel topology.
#[derive(Component, Debug, Clone)]
pub struct AnimationPlan {
    /// Parsed prim path retained for per-frame sampling.
    pub path: SdfPath,
    /// Stage time codes per second.
    pub time_codes_per_second: f64,
    /// Transform channel selected by authored USD topology.
    pub xform: XformDrive,
    /// Whether visibility is time-sampled.
    pub visibility: bool,
    /// Material channels selected by the bound shader/primvar topology.
    pub material: Option<MaterialPlan>,
}

/// Sample a scalar time-sampled numeric input.
fn read_f32_at(reader: &dyn UsdReadObject, path: &SdfPath, attr: &str, time: f64) -> Option<f32> {
    if !attr_has_time_samples(reader, path, attr) {
        return None;
    }
    reader
        .attr_value_at(path, attr, time)
        .and_then(|value| match value {
            Value::Float(value) => Some(value),
            Value::Double(value) => Some(value as f32),
            Value::Int(value) => Some(value as f32),
            Value::Int64(value) => Some(value as f32),
            _ => None,
        })
}

/// Derive one animated prim's channel plan after its stage is available.
pub fn plan_usd_animation(
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), (With<UsdAnimated>, Without<AnimationPlan>)>,
) {
    for (entity, prim) in &q {
        let Some(stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim.stage_handle.id(), stage_asset);
        let reader = &reader;
        let Ok(path) = SdfPath::new(prim.path.as_str()) else {
            continue;
        };

        let xform = if lunco_usd_bevy_core::read_xform_op_order(reader, &path).is_some() {
            XformDrive::OpOrder
        } else {
            XformDrive::None
        };
        let shader = resolve_bound_shader(reader, &path);
        let diffuse = shader
            .as_ref()
            .is_some_and(|shader| attr_has_time_samples(reader, shader, "inputs:diffuseColor"));
        let geom_color = !diffuse && attr_has_time_samples(reader, &path, "primvars:displayColor");
        let opacity = shader
            .as_ref()
            .is_some_and(|shader| attr_has_time_samples(reader, shader, "inputs:opacity"));
        let material = (diffuse || geom_color || opacity).then_some(MaterialPlan {
            shader,
            diffuse,
            geom_color,
            opacity,
        });

        commands.entity(entity).try_insert(AnimationPlan {
            time_codes_per_second: stage_time_codes_per_second(reader),
            xform,
            visibility: attr_has_time_samples(reader, &path, "visibility"),
            material,
            path,
        });
    }
}

/// Drop plans whose stage asset was modified so topology is derived again.
pub fn clear_animation_plans_on_stage_reload(
    mut events: MessageReader<AssetEvent<UsdStageAsset>>,
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), With<AnimationPlan>>,
) {
    let reloaded: Vec<AssetId<UsdStageAsset>> = events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } => Some(*id),
            _ => None,
        })
        .collect();
    if reloaded.is_empty() {
        return;
    }
    for (entity, prim) in &q {
        if reloaded.contains(&prim.stage_handle.id()) {
            commands.entity(entity).remove::<AnimationPlan>();
        }
    }
}

/// Sample authored xform and visibility channels at each entity's resolved time.
pub fn sample_usd_animation(
    world: Res<lunco_time::WorldTime>,
    resolved: Res<lunco_time::ResolvedDomains>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut q: Query<
        (
            &UsdPrimPath,
            &AnimationPlan,
            &mut Transform,
            &mut Visibility,
            Option<&lunco_time::TimeBinding>,
        ),
        With<UsdAnimated>,
    >,
) {
    for (prim, plan, mut transform, mut visibility, binding) in &mut q {
        let Some(stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim.stage_handle.id(), stage_asset);
        let reader = &reader;
        let time = lunco_time::domain_time(&resolved, binding, &world) * plan.time_codes_per_second;
        let Ok(convention) = stage_convention(reader) else {
            error!(
                "[usd-animation] animated prim {} has invalid stage convention metadata; refusing sample",
                plan.path.as_str()
            );
            continue;
        };
        if matches!(plan.xform, XformDrive::OpOrder) {
            if let Ok(Some(local)) = compose_xform_order_at(reader, &plan.path, time) {
                let local = convention.local_transform(local);
                transform.translation = local.translation;
                transform.rotation = local.rotation;
                transform.scale = local.scale;
            }
        }
        if plan.visibility {
            if let Some(token) = read_token_at(reader, &plan.path, "visibility", time) {
                let wanted = if token == "invisible" {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                };
                if *visibility != wanted {
                    *visibility = wanted;
                }
            }
        }
    }
}

/// Sample authored shader and display-color animation into [`PbrLook`] intent.
pub fn sample_usd_material_animation(
    world: Res<lunco_time::WorldTime>,
    resolved: Res<lunco_time::ResolvedDomains>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut q: Query<
        (
            &UsdPrimPath,
            &AnimationPlan,
            &mut PbrLook,
            Option<&lunco_time::TimeBinding>,
        ),
        With<UsdAnimated>,
    >,
) {
    for (prim, plan, mut look, binding) in &mut q {
        let Some(material) = &plan.material else {
            continue;
        };
        let Some(stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim.stage_handle.id(), stage_asset);
        let reader = &reader;
        let time = lunco_time::domain_time(&resolved, binding, &world) * plan.time_codes_per_second;
        let color_source = if material.diffuse {
            material.shader.as_ref()
        } else if material.geom_color {
            Some(&plan.path)
        } else {
            None
        };
        if let Some(source) = color_source {
            let sampled = if material.diffuse {
                read_vec3_f64_at(reader, source, "inputs:diffuseColor", time)
            } else {
                read_primvar_vec3_at(reader, source, "primvars:displayColor", time)
            };
            if let Some(color) = sampled {
                let alpha = look.base_color.alpha;
                look.base_color =
                    LinearRgba::new(color[0] as f32, color[1] as f32, color[2] as f32, alpha);
            }
        }
        if material.opacity {
            if let Some(opacity) = read_f32_at(
                reader,
                material.shader.as_ref().unwrap_or(&plan.path),
                "inputs:opacity",
                time,
            ) {
                look.base_color.alpha = opacity;
                if opacity < 1.0 && look.alpha == SurfaceAlpha::Opaque {
                    look.alpha = SurfaceAlpha::Blend;
                }
            }
        }
    }
}

/// Bind newly projected animated prims to the shared animation preview domain.
pub fn bind_animated_to_preview(
    preview: Option<Res<lunco_time::AnimationPreview>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), (Added<UsdAnimated>, Without<lunco_time::TimeBinding>)>,
    mut playback: Query<&mut lunco_time::Playback>,
) {
    let Some(preview) = preview else {
        return;
    };
    let mut span: Option<(f64, f64)> = None;
    for (entity, prim) in &q {
        commands.entity(entity).try_insert(lunco_time::TimeBinding {
            domain: preview.domain,
        });
        if let Some(stage_asset) = stages.get(&prim.stage_handle) {
            let (reader, _generation) = canonical.reader_for(prim.stage_handle.id(), stage_asset);
            if let Ok(path) = SdfPath::new(prim.path.as_str()) {
                if let Some((start, end)) = animated_time_range(&reader, &path) {
                    span = Some(match span {
                        Some((lo, hi)) => (lo.min(start), hi.max(end)),
                        None => (start, end),
                    });
                }
            }
        }
    }
    if let (Some((start, end)), Ok(mut playback)) = (span, playback.get_mut(preview.domain)) {
        let (start, end) = if playback.bounded() {
            (playback.start.min(start), playback.end.max(end))
        } else {
            (start, end)
        };
        playback.start = start;
        playback.end = end;
    }
}
