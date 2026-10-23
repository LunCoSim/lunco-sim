//! Bind the selected renderer through upstream Bevy plugins. The reference path
//! tracer is an offline image producer; it never advances simulation time.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraMainTextureUsages, MainPassResolutionOverride};
use bevy::core_pipeline::{
    core_3d::{Transparent3d, main_transparent_pass_3d},
    oit::{OrderIndependentTransparencySettings, resolve::OitResolvePipelineId},
    schedule::{Core3d, Core3dSystems},
    tonemapping::tonemapping,
};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use bevy::render::{
    ExtractSchedule, MainWorld, RenderApp,
    camera::ExtractedCamera,
    render_phase::ViewSortedRenderPhases,
    render_resource::{CachedPipelineState, PipelineCache},
    renderer::{RenderContext, RenderDevice, ViewQuery},
    view::{ExtractedView, ViewDepthStencilTexture, ViewTarget},
};
use bevy::solari::{
    SolariPlugins,
    pathtracer::{Pathtracer, PathtracingPlugin, PathtracingSystems},
    scene::{RaytracingMesh3d, RaytracingSceneBindings, RaytracingScenePlugin},
};
use lunco_core::{AppSceneExt, SceneResource};
use lunco_render::{
    OfflineImageSampling, RendererSettings, RendererStatus, RenderingBackend, TwinRendererOverride,
};
use lunco_settings::AppSettingsExt;
use std::collections::HashMap;

/// Install only in the rendering host; no ray tracing dependency reaches physics.
pub(crate) fn build(app: &mut App) {
    app.register_settings_section::<RendererSettings>()
        .register_type::<Pathtracer>()
        .register_type::<RaytracingMesh3d>()
        .register_type::<RendererSettings>()
        .register_type::<RendererStatus>()
        .init_resource::<TwinRendererOverride>()
        .init_scene_resource::<OfflineImageSampling>()
        .init_resource::<RendererStatus>()
        .init_scene_resource::<RayMeshCache>();
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    app.add_plugins((RaytracingScenePlugin, PathtracingPlugin))
        .add_systems(
            PostUpdate,
            bind_backend
                .in_set(lunco_render::RenderCameraBindingSet)
                .after(TransformSystems::Propagate),
        );
    app.sub_app_mut(RenderApp)
        .init_resource::<SampleAck>()
        .add_systems(ExtractSchedule, extract_sample_request)
        .add_systems(
            Core3d,
            composite_path_traced_effects
                .after(PathtracingSystems::Trace)
                .before(Core3dSystems::EarlyPostProcess),
        )
        .add_systems(Core3d, acknowledge_path_sample.after(tonemapping));
}

/// Composite native sorted transparency onto the traced opaque image. Reuse
/// Bevy's pass and opaque depth; effects never enter the accumulation texture.
fn composite_path_traced_effects(
    world: &World,
    view: ViewQuery<(
        &ExtractedCamera,
        &ExtractedView,
        &ViewTarget,
        &ViewDepthStencilTexture,
        Option<&MainPassResolutionOverride>,
        Has<OrderIndependentTransparencySettings>,
        Option<&OitResolvePipelineId>,
    )>,
    transparent_phases: Res<ViewSortedRenderPhases<Transparent3d>>,
    context: RenderContext,
) {
    if world.get::<Pathtracer>(view.entity()).is_none() {
        return;
    }
    main_transparent_pass_3d(world, view, transparent_phases, context);
}

fn forward_transparent(mode: AlphaMode) -> bool {
    matches!(
        mode,
        AlphaMode::Blend | AlphaMode::Premultiplied | AlphaMode::Add | AlphaMode::Multiply
    )
}

/// The authored pose owner may stay active for LOD/transport while its output
/// is skipped. Bind tracing only to actual color writers, including the
/// offscreen mirror. Preview cameras are excluded by the owning query.
fn writes_scene_image(camera: &Camera) -> bool {
    camera.is_active
        && matches!(
            camera.output_mode,
            bevy::camera::CameraOutputMode::Write { .. }
        )
}

/// Derived GPU geometry is private to the ray tracer; authored/shared raster
/// mesh buffers are never modified to meet another backend's vertex layout.
#[derive(Resource, Default)]
struct RayMeshCache(HashMap<AssetId<Mesh>, Handle<Mesh>>);
impl SceneResource for RayMeshCache {}

#[derive(Component)]
struct RasterCameraState {
    msaa: Msaa,
    usages: CameraMainTextureUsages,
    hdr: bool,
}

fn ray_mesh(source: &Mesh) -> Result<Mesh, String> {
    if source.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err("path tracing requires triangle-list geometry".into());
    }
    if !source.contains_attribute(Mesh::ATTRIBUTE_POSITION)
        || !source.contains_attribute(Mesh::ATTRIBUTE_NORMAL)
    {
        return Err("path tracing requires mesh positions and normals".into());
    }
    let mut mesh = source.clone();
    let kept = [
        Mesh::ATTRIBUTE_POSITION.id,
        Mesh::ATTRIBUTE_NORMAL.id,
        Mesh::ATTRIBUTE_UV_0.id,
        Mesh::ATTRIBUTE_TANGENT.id,
    ];
    let removed: Vec<_> = mesh
        .attributes()
        .map(|(attribute, _)| attribute.id)
        .filter(|id| !kept.contains(id))
        .collect();
    for id in removed {
        mesh.remove_attribute(id);
    }
    let vertices = mesh.count_vertices();
    if !mesh.contains_attribute(Mesh::ATTRIBUTE_UV_0) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; vertices]);
    }
    if !mesh.contains_attribute(Mesh::ATTRIBUTE_TANGENT) {
        // An untextured primitive has no meaningful UV gradient. Its orthonormal
        // tangent frame still has to be finite for the upstream BRDF boundary.
        if let Some(VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        {
            let tangents: Vec<[f32; 4]> = normals
                .iter()
                .map(|normal| {
                    let n = Vec3::from_array(*normal)
                        .try_normalize()
                        .ok_or("zero mesh normal")?;
                    let t = n.any_orthonormal_vector();
                    Ok([t.x, t.y, t.z, 1.0])
                })
                .collect::<Result<_, &str>>()
                .map_err(str::to_owned)?;
            mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
        } else {
            return Err("path tracing requires float3 normals".into());
        }
    }
    let indices = mesh
        .indices()
        .map(|indices| indices.iter().map(|i| i as u32).collect())
        .unwrap_or_else(|| (0..vertices as u32).collect());
    mesh.insert_indices(Indices::U32(indices));
    mesh.enable_raytracing = true;
    Ok(mesh)
}

/// Native feature checks and compatibility admission are visible. Unsupported
/// authored content must never disappear silently from a recorded image.
fn bind_backend(
    settings: Res<RendererSettings>,
    project: Res<TwinRendererOverride>,
    device: Res<RenderDevice>,
    mut status: ResMut<RendererStatus>,
    mut cache: ResMut<RayMeshCache>,
    mut mesh_events: MessageReader<AssetEvent<Mesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    materials: Res<Assets<StandardMaterial>>,
    custom_materials: Res<Assets<crate::ShaderMaterial>>,
    instances: Query<(
        Entity,
        &Mesh3d,
        Option<&MeshMaterial3d<StandardMaterial>>,
        Option<&MeshMaterial3d<crate::ShaderMaterial>>,
        Option<&InheritedVisibility>,
        Option<&RaytracingMesh3d>,
        Option<&RenderLayers>,
    )>,
    local_lights: Query<
        (
            Option<&InheritedVisibility>,
            Option<&RenderLayers>,
            Option<&PointLight>,
            Option<&SpotLight>,
            Option<&bevy::light::RectLight>,
        ),
        Or<(
            With<PointLight>,
            With<SpotLight>,
            With<bevy::light::RectLight>,
        )>,
    >,
    oit_cameras: Query<
        &Camera,
        (
            With<Camera3d>,
            Without<lunco_render::EditorPreviewCamera>,
            With<OrderIndependentTransparencySettings>,
        ),
    >,
    mut cameras: Query<
        (
            Entity,
            &Camera,
            Option<&mut Pathtracer>,
            Option<&Msaa>,
            Option<&CameraMainTextureUsages>,
            Has<bevy::camera::Hdr>,
            Option<&RasterCameraState>,
            Option<&RenderLayers>,
        ),
        (With<Camera3d>, Without<lunco_render::EditorPreviewCamera>),
    >,
    sampling: Res<OfflineImageSampling>,
    mut last_request: Local<Option<u64>>,
    mut commands: Commands,
) {
    let mut next = RendererStatus {
        hardware_ray_tracing: device
            .features()
            .contains(SolariPlugins::required_wgpu_features()),
        ..default()
    };
    let resolved = project.resolve(*settings);
    let effective = match resolved {
        Ok(value) => value,
        Err(error) => {
            next.error = Some(error);
            status.set_if_neq(next);
            return;
        }
    };
    next.requested = effective.backend;
    next.samples_per_pixel = effective.samples_per_pixel;
    if sampling
        .capture_settings
        .is_some_and(|locked| locked != effective)
    {
        next.error = Some("Renderer settings changed during a take; restore them or stop recording before selecting another backend".into());
        status.set_if_neq(next);
        return;
    }
    for event in mesh_events.read() {
        if let AssetEvent::Modified { id } | AssetEvent::Removed { id } = event {
            cache.0.remove(id);
        }
    }
    let layers = cameras
        .iter()
        .find(|(_, camera, ..)| writes_scene_image(camera))
        .map(|(_, _, _, _, _, _, _, layers)| layers.cloned().unwrap_or_default());
    let visible = |visibility: Option<&InheritedVisibility>, mesh_layers: Option<&RenderLayers>| {
        visibility.is_none_or(|v| v.get())
            && layers.as_ref().is_some_and(|camera_layers| {
                camera_layers.intersects(&mesh_layers.cloned().unwrap_or_default())
            })
    };
    if effective.backend == RenderingBackend::PathTracer {
        if !next.hardware_ray_tracing {
            next.error = Some("The selected GPU lacks Bevy's required ray-query features".into());
        } else if local_lights
            .iter()
            .any(|(visibility, layers, point, spot, rect)| {
                visible(visibility, layers)
                    && (point.is_some_and(|light| light.intensity > 0.0)
                        || spot.is_some_and(|light| light.intensity > 0.0)
                        || rect.is_some_and(|light| light.intensity > 0.0))
            })
        {
            next.error = Some("Bevy reference path tracing does not support authored point, spot or rectangle lights; use explicit emissive geometry or a supported lighting backend".into());
        } else if oit_cameras.iter().any(writes_scene_image) {
            next.error = Some("Offline path tracing supports sorted transparency, not order-independent transparency".into());
        } else {
            for (_, _, material, custom, visibility, _, mesh_layers) in &instances {
                if !visible(visibility, mesh_layers) {
                    continue;
                }
                let alpha = if let Some(custom) = custom {
                    let Some(surface) = custom_materials.get(&custom.0) else {
                        next.pending_reason =
                            Some("The custom shader material is not ready".into());
                        break;
                    };
                    if !forward_transparent(surface.alpha_mode) {
                        next.error = Some("The scene contains an opaque or alpha-tested custom shader surface that the reference path tracer cannot render".into());
                        break;
                    }
                    surface.alpha_mode
                } else if let Some(material) = material {
                    let Some(surface) = materials.get(&material.0) else {
                        next.pending_reason = Some("The standard material is not ready".into());
                        break;
                    };
                    surface.alpha_mode
                } else {
                    next.error = Some(
                        "The scene contains a surface without a ready supported material".into(),
                    );
                    break;
                };
                if alpha != AlphaMode::Opaque && !forward_transparent(alpha) {
                    next.error = Some("The scene contains an alpha-tested surface that the reference path tracer cannot render".into());
                    break;
                }
            }
        }
    }
    let trace = effective.backend == RenderingBackend::PathTracer
        && next.error.is_none()
        && next.pending_reason.is_none();
    if trace
        && sampling.requested
        && cameras
            .iter()
            .filter(|(_, camera, ..)| writes_scene_image(camera))
            .count()
            != 1
    {
        next.error = Some("Offline path tracing requires exactly one active scene camera".into());
    }
    for (entity, mesh, material, _, visibility, existing, mesh_layers) in &instances {
        let opaque = material
            .and_then(|h| materials.get(&h.0))
            .is_some_and(|surface| surface.alpha_mode == AlphaMode::Opaque);
        if !trace || !opaque || !visible(visibility, mesh_layers) {
            if existing.is_some() {
                commands.entity(entity).remove::<RaytracingMesh3d>();
            }
            continue;
        }
        let handle = if let Some(handle) = cache.0.get(&mesh.0.id()) {
            handle.clone()
        } else {
            let Some(source) = meshes.get(&mesh.0) else {
                continue;
            };
            match ray_mesh(source) {
                Ok(mesh_copy) => {
                    let handle = meshes.add(mesh_copy);
                    cache.0.insert(mesh.0.id(), handle.clone());
                    handle
                }
                Err(error) => {
                    next.error = Some(error);
                    break;
                }
            }
        };
        if existing.is_none_or(|ray| ray.0 != handle) {
            commands.entity(entity).insert(RaytracingMesh3d(handle));
        }
    }
    let reset = sampling.requested && *last_request != Some(sampling.revision());
    if sampling.requested {
        *last_request = Some(sampling.revision());
    }
    for (entity, camera, pathtracer, msaa, usages, hdr, raster, _) in &mut cameras {
        if trace && camera.is_active && next.error.is_none() {
            // OutputMode::Skip still runs Bevy's prepasses. Active pose owners
            // and their output mirrors share a render-target texture key, so
            // they must agree on HDR, samples and usages even though only the
            // color writer owns a path integrator and sampling receipts.
            if raster.is_none() {
                commands.entity(entity).insert(RasterCameraState {
                    msaa: msaa.copied().unwrap_or(Msaa::Off),
                    usages: usages.copied().unwrap_or_default(),
                    hdr,
                });
            }
            if msaa.is_none_or(|msaa| *msaa != Msaa::Off) {
                commands.entity(entity).insert(Msaa::Off);
            }
            if !hdr {
                commands.entity(entity).insert(bevy::camera::Hdr);
            }
            let target_usages = usages
                .copied()
                .unwrap_or_default()
                .with(bevy::render::render_resource::TextureUsages::STORAGE_BINDING);
            if usages.is_none_or(|current| current.0 != target_usages.0) {
                commands.entity(entity).insert(target_usages);
            }
            if writes_scene_image(camera) {
                if let Some(mut pathtracer) = pathtracer {
                    pathtracer.reset = reset;
                } else {
                    commands.entity(entity).insert(Pathtracer { reset: true });
                }
            } else if pathtracer.is_some() {
                commands.entity(entity).remove::<Pathtracer>();
            }
        } else if raster.is_some() || pathtracer.is_some() {
            let mut entity = commands.entity(entity);
            entity.remove::<Pathtracer>();
            if let Some(raster) = raster {
                entity.insert((raster.msaa, raster.usages));
                if !raster.hdr {
                    entity.remove::<bevy::camera::Hdr>();
                }
                entity.remove::<RasterCameraState>();
            }
        }
    }
    if next.error != status.error
        && let Some(error) = &next.error
    {
        error!("[renderer] {error}");
    }
    status.set_if_neq(next);
}

/// Acknowledgements cross the same extraction boundary as the rendered ticket.
#[derive(Resource, Default)]
struct SampleAck {
    revision: Option<u64>,
    samples: u32,
    required: u32,
    submitted: Option<(u64, u32, u32)>,
    error: Option<String>,
}

fn extract_sample_request(mut main: ResMut<MainWorld>, mut ack: ResMut<SampleAck>) {
    let settings = main.resource::<RendererStatus>().clone();
    let mut sampling = main.resource_mut::<OfflineImageSampling>();
    if let Some((revision, samples, required)) = ack.submitted.take() {
        sampling.acknowledge(revision, samples, required);
    }
    if !sampling.requested {
        ack.revision = None;
        ack.samples = 0;
        ack.error = None;
        return;
    }
    if ack.revision != Some(sampling.revision()) {
        ack.revision = Some(sampling.revision());
        ack.samples = 0;
        ack.error = None;
    }
    if let Some(error) = settings.error {
        sampling.error = Some(error);
        return;
    }
    if settings.pending_reason.is_some() {
        return;
    }
    if let Some(error) = &ack.error {
        sampling.error = Some(error.clone());
        return;
    }
    if settings.requested == RenderingBackend::Raster {
        sampling.acknowledge(ack.revision.unwrap(), 1, 1);
    } else {
        ack.required = settings.samples_per_pixel;
    }
}

/// The upstream pass has run before tonemapping. Waiting pipelines or missing
/// ray-scene bindings are not samples and must not advance an image ticket.
fn acknowledge_path_sample(
    view: ViewQuery<&Pathtracer>,
    pipelines: Res<PipelineCache>,
    bindings: Option<Res<RaytracingSceneBindings>>,
    mut ack: ResMut<SampleAck>,
) {
    let pathtracer = view.into_inner();
    let Some(revision) = ack.revision else {
        return;
    };
    if let Some(error) = pipelines
        .pipelines()
        .find_map(|pipeline| match &pipeline.state {
            CachedPipelineState::Err(error) => {
                Some(format!("GPU pipeline compilation failed: {error}"))
            }
            _ => None,
        })
    {
        ack.error = Some(error);
        return;
    }
    if pipelines.waiting_pipelines().next().is_some()
        || bindings.is_none_or(|b| b.bind_group.is_none())
    {
        return;
    }
    if pathtracer.reset {
        ack.samples = 0;
    }
    ack.samples = ack.samples.saturating_add(1);
    ack.submitted = Some((revision, ack.samples, ack.required));
}
