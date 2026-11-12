//! Project output-owned geometry into the native shadow visibility inputs.
//!
//! Directional maps use their exact camera's cover. Native shared point/spot
//! maps use the explicit ShadowLodOrigin output. Ordinary scene casters keep
//! their native visibility lists and authored light masks.

use bevy::{
    camera::{
        ShadowLodOrigin,
        visibility::{NoFrustumCulling, RenderLayers},
    },
    ecs::query::QueryItem,
    extract::{
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        sync_component::SyncComponent,
    },
    pbr::{LightEntity, prepare_lights},
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
        sync_world::MainEntity,
        view::{
            ExtractedView,
            visibility::{
                RenderExtractedShadowMapVisibleEntities, RenderExtractedVisibleEntitiesClass,
                collect_visible_cpu_culled_entities,
            },
        },
    },
};
use lunco_render::camera::{
    SceneViewDemand, SceneViewDemandSet, SceneViewGeometry, SceneViewLayers,
};
use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
};

struct ShadowExtraction;

#[derive(Component, Clone)]
struct OutputLayers(SceneViewLayers);
impl SyncComponent<RenderApp, ShadowExtraction> for SceneViewLayers {
    type Target = OutputLayers;
}
impl ExtractComponent<RenderApp, ShadowExtraction> for SceneViewLayers {
    type QueryData = &'static SceneViewLayers;
    type QueryFilter = ();
    type Out = OutputLayers;
    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        Some(OutputLayers(item.clone()))
    }
}

#[derive(Component, Clone, Copy)]
struct OutputGeometry {
    camera: Entity,
}
impl SyncComponent<RenderApp, ShadowExtraction> for SceneViewGeometry {
    type Target = OutputGeometry;
}
impl ExtractComponent<RenderApp, ShadowExtraction> for SceneViewGeometry {
    type QueryData = (
        &'static SceneViewGeometry,
        &'static InheritedVisibility,
        Has<bevy::light::NotShadowCaster>,
    );
    type QueryFilter = With<Mesh3d>;
    type Out = OutputGeometry;
    fn extract_component(
        (geometry, visible, no_cast): QueryItem<'_, '_, Self::QueryData>,
    ) -> Option<Self::Out> {
        (visible.get() && !no_cast).then_some(OutputGeometry {
            camera: geometry.camera,
        })
    }
}

#[derive(Resource, Default, Clone, Copy)]
struct SharedShadowOutput(Option<Entity>);

pub(super) fn build(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    // Selected covers are bounded. Keeping them extractable also admits
    // shadow-only casters on the CPU rendering path; raster clipping remains
    // native. This trades extra CPU-path draws for complete shadow geometry.
    app.register_required_components::<SceneViewGeometry, NoFrustumCulling>();
    app.add_plugins((
        ExtractComponentPlugin::<SceneViewLayers, RenderApp, ShadowExtraction>::default(),
        ExtractComponentPlugin::<SceneViewGeometry, RenderApp, ShadowExtraction>::default(),
    ));
    app.init_resource::<SharedShadowOutput>()
        .init_resource::<lunco_core::RuntimeDiagnostics>()
        .add_systems(
            PostUpdate,
            resolve_shared_shadow_output.after(SceneViewDemandSet::Publish),
        );
    let render = app.sub_app_mut(RenderApp);
    render
        .init_resource::<SharedShadowOutput>()
        .add_systems(ExtractSchedule, extract_shared_shadow_output)
        .add_systems(
            Render,
            inject_output_shadow_candidates
                .in_set(RenderSystems::PrepareAssets)
                .before(collect_visible_cpu_culled_entities),
        )
        .add_systems(
            Render,
            restrict_output_shadow_masks
                .in_set(RenderSystems::CreateViews)
                .after(prepare_lights),
        );
}

fn resolve_shared_shadow_output(
    origins: Query<Entity, With<ShadowLodOrigin>>,
    outputs: Query<(Entity, &SceneViewDemand), With<SceneViewLayers>>,
    geometry: Query<
        (),
        (
            With<SceneViewGeometry>,
            Without<bevy::light::NotShadowCaster>,
        ),
    >,
    lights: Query<
        (Option<&PointLight>, Option<&SpotLight>),
        Or<(With<PointLight>, With<SpotLight>)>,
    >,
    mut output: ResMut<SharedShadowOutput>,
    mut diagnostics: ResMut<lunco_core::RuntimeDiagnostics>,
) {
    let origins = origins.iter().collect::<Vec<_>>();
    let exact = match origins.as_slice() {
        [origin] => {
            if outputs.get(*origin).is_ok() {
                Some(*origin)
            } else {
                let mut matches = outputs
                    .iter()
                    .filter(|(_, demand)| demand.source == *origin);
                let first = matches.next().map(|(entity, _)| entity);
                matches.next().is_none().then_some(first).flatten()
            }
        }
        _ => None,
    };
    output.0 = exact;
    let needed = !geometry.is_empty()
        && !outputs.is_empty()
        && lights.iter().any(|(point, spot)| {
            point.is_some_and(|light| light.shadow_maps_enabled)
                || spot.is_some_and(|light| light.shadow_maps_enabled)
        });
    let findings = if needed && exact.is_none() {
        vec![lunco_core::RuntimeDiagnostic {
            producer: "scene-view-shadows".into(), code: "shared-shadow-output-unresolved".into(),
            severity: lunco_core::DiagnosticSeverity::Error, subject: "ShadowLodOrigin".into(),
            message: "shared point/spot terrain shadows require one ShadowLodOrigin that resolves to a consumed camera output".into(),
        }]
    } else {
        Vec::new()
    };
    diagnostics.replace_producer("scene-view-shadows", findings);
}

fn extract_shared_shadow_output(
    source: Extract<Res<SharedShadowOutput>>,
    mut target: ResMut<SharedShadowOutput>,
) {
    target.0 = source.0;
}

fn update_candidates(
    class: &mut RenderExtractedVisibleEntitiesClass,
    previous: &HashSet<MainEntity>,
    current: impl IntoIterator<Item = (Entity, MainEntity)>,
) {
    class.entities.retain(|(_, main)| !previous.contains(main));
    class.entities.extend(current);
    class.entities.sort_unstable_by_key(|(_, main)| *main);
    class.entities.dedup_by_key(|(_, main)| *main);
}

fn inject_output_shadow_candidates(
    geometry: Query<(Entity, &MainEntity, &OutputGeometry)>,
    outputs: Query<(&MainEntity, &OutputLayers)>,
    mut lights: Query<(&RenderLayers, &mut RenderExtractedShadowMapVisibleEntities)>,
    shared: Res<SharedShadowOutput>,
    mut previous: Local<HashSet<MainEntity>>,
) {
    let output_layers = outputs
        .iter()
        .map(|(entity, layers)| (entity.id(), &layers.0))
        .collect::<HashMap<_, _>>();
    let mut by_output: HashMap<Entity, Vec<(Entity, MainEntity)>> = HashMap::new();
    for (render, main, geometry) in &geometry {
        by_output
            .entry(geometry.camera)
            .or_default()
            .push((render, *main));
    }
    for (light_layers, mut table) in &mut lights {
        for (subview, visible) in &mut table.subviews {
            let camera = subview
                .auxiliary_entity
                .map(|entity| entity.id())
                .or(shared.0);
            let current = camera
                .and_then(|camera| {
                    output_layers
                        .get(&camera)
                        .filter(|layers| light_layers.intersects(&layers.base))
                        .and_then(|_| by_output.get(&camera))
                })
                .into_iter()
                .flatten()
                .copied();
            let class = visible.classes.entry(TypeId::of::<Mesh3d>()).or_default();
            update_candidates(class, &previous, current);
        }
    }
    previous.clear();
    previous.extend(geometry.iter().map(|(_, main, _)| *main));
}

fn restrict_output_shadow_masks(
    outputs: Query<(&MainEntity, &OutputLayers)>,
    light_layers: Query<&RenderLayers, Without<LightEntity>>,
    mut shadows: Query<(&LightEntity, &ExtractedView, &mut RenderLayers)>,
    shared: Res<SharedShadowOutput>,
) {
    let outputs = outputs
        .iter()
        .map(|(entity, layers)| (entity.id(), &layers.0))
        .collect::<HashMap<_, _>>();
    for (light, view, mut mask) in &mut shadows {
        let source = match light {
            LightEntity::Directional { light_entity, .. }
            | LightEntity::Point { light_entity, .. }
            | LightEntity::Spot { light_entity } => *light_entity,
        };
        let Ok(base) = light_layers.get(source) else {
            continue;
        };
        let camera = match light {
            LightEntity::Directional { .. } => view
                .retained_view_entity
                .auxiliary_entity
                .map(|entity| entity.id()),
            LightEntity::Point { .. } | LightEntity::Spot { .. } => shared.0,
        };
        let next = output_shadow_mask(
            base,
            camera.and_then(|camera| outputs.get(&camera).copied()),
        );
        mask.set_if_neq(next);
    }
}

fn output_shadow_mask(base: &RenderLayers, output: Option<&SceneViewLayers>) -> RenderLayers {
    output
        .filter(|output| base.intersects(&output.base))
        .map_or_else(|| base.clone(), |output| base.clone().with(output.layer))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shadow_mask_preserves_shared_props_and_exactly_one_compatible_output_cover() {
        let base = RenderLayers::default();
        let first = SceneViewLayers {
            layer: 64,
            base: base.clone(),
        };
        let second = SceneViewLayers {
            layer: 65,
            base: base.clone(),
        };
        let mask = output_shadow_mask(&base, Some(&first));
        assert!(mask.intersects(&base));
        assert!(mask.intersects(&RenderLayers::none().with(first.layer)));
        assert!(!mask.intersects(&RenderLayers::none().with(second.layer)));
        let preview = SceneViewLayers {
            layer: 66,
            base: RenderLayers::layer(1),
        };
        assert_eq!(output_shadow_mask(&base, Some(&preview)), base);
        assert_eq!(output_shadow_mask(&base, None), base);
    }
    #[test]
    fn shadow_candidate_projection_preserves_props_and_replaces_only_owned_cover() {
        let prop = MainEntity::from(Entity::from_bits(1));
        let old = MainEntity::from(Entity::from_bits(2));
        let new = MainEntity::from(Entity::from_bits(3));
        let mut class = RenderExtractedVisibleEntitiesClass::default();
        class.entities = vec![(Entity::PLACEHOLDER, prop), (Entity::PLACEHOLDER, old)];
        let previous = HashSet::from([old]);
        update_candidates(
            &mut class,
            &previous,
            [(Entity::PLACEHOLDER, new), (Entity::PLACEHOLDER, new)],
        );
        assert_eq!(
            class.entities.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
            vec![prop, new]
        );
        update_candidates(&mut class, &HashSet::from([new]), []);
        assert_eq!(class.entities, vec![(Entity::PLACEHOLDER, prop)]);
    }
}
