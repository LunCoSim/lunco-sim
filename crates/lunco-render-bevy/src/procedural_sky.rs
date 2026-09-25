//! Camera-background binding for procedural shader looks.
//!
//! A [`ProceduralSkybox`] is a render-free marker on the same entity as a
//! [`lunco_materials::ShaderLook`]. The normal shader material binder still owns the material
//! asset and its reflected parameters; this module only adds the render path
//! that queues that material as one non-mesh item in Bevy's built-in opaque
//! phase, after opaque geometry. The normal phase owns the render pass and its
//! depth state, matching Bevy's built-in skybox lifecycle. That keeps sky
//! appearance in WGSL/USD while removing finite-sphere culling and far-plane
//! coupling from the scene.

use bevy::camera::Camera3d;
use bevy::core_pipeline::{
    FullscreenShader,
    core_3d::{CORE_3D_DEPTH_FORMAT, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
};
use bevy::ecs::{
    query::ROQueryItem,
    system::{SystemParamItem, lifetimeless::SRes},
};
use bevy::pbr::{MaterialBindGroupAllocators, PreparedMaterial, RenderMaterialBindings};
use bevy::prelude::*;
use bevy::render::{
    GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
    erased_render_asset::ErasedRenderAssets,
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    mesh::allocator::MeshSlabs,
    render_phase::{
        AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem,
        RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
        ViewBinnedRenderPhases,
    },
    render_resource::{
        AsBindGroup, BindGroup, BindGroupEntries, BindGroupLayoutDescriptor,
        BindGroupLayoutEntries, ColorTargetState, ColorWrites, CompareFunction, DepthBiasState,
        DepthStencilState, FragmentState, MultisampleState, PipelineCache, PrimitiveState,
        RenderPipelineDescriptor, ShaderStages, SpecializedRenderPipeline,
        SpecializedRenderPipelines, StencilFaceState, StencilState, TextureFormat, VertexState,
        binding_types::uniform_buffer,
    },
    renderer::RenderDevice,
    sync_world::MainEntity,
    view::{ExtractedView, Msaa, RetainedViewEntity, ViewUniform, ViewUniformOffset, ViewUniforms},
};
use bevy::shader::Shader;
use bevy::shader::ShaderDefVal;
use bevy::utils::default;
use lunco_environment::SunRenderState;
use lunco_materials::ParamValue;
use lunco_render::{ProceduralSkybox, SceneCamera};
use std::any::TypeId;
use std::collections::HashMap;

/// Render-side copy of a sky look's material handles.
///
/// It lives on the main entity too so Bevy's normal `ShaderMaterial` asset
/// extraction/allocator prepares the exact same bind group used by mesh looks.
#[derive(Component, Clone, Debug, ExtractComponent)]
pub(crate) struct ProceduralSkyboxMaterial {
    pub(crate) material: Handle<super::ShaderMaterial>,
    pub(crate) shader: Handle<Shader>,
}

impl ProceduralSkyboxMaterial {
    pub(crate) fn new(
        material: Handle<super::ShaderMaterial>,
        shader_path: &str,
        asset_server: &AssetServer,
    ) -> Self {
        Self {
            material,
            shader: asset_server.load(shader_path.to_owned()),
        }
    }
}

#[derive(Resource)]
struct ProceduralSkyboxPipeline {
    view_layout: BindGroupLayoutDescriptor,
    material_layout: BindGroupLayoutDescriptor,
    fullscreen_shader: FullscreenShader,
}

#[derive(Component)]
struct ProceduralSkyboxViewBindGroup(BindGroup);

#[derive(Clone, PartialEq, Eq, Hash)]
struct ProceduralSkyboxPipelineKey {
    shader: Handle<Shader>,
    target_format: TextureFormat,
    samples: u32,
}

fn same_skybox_render_item(
    previous: Option<&(MainEntity, ProceduralSkyboxPipelineKey)>,
    owner: MainEntity,
    pipeline_key: &ProceduralSkyboxPipelineKey,
) -> bool {
    previous.is_some_and(|(previous_owner, previous_key)| {
        previous_owner.id() == owner.id() && previous_key == pipeline_key
    })
}

impl SpecializedRenderPipeline for ProceduralSkyboxPipeline {
    type Key = ProceduralSkyboxPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let material_group_def = ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 1);
        RenderPipelineDescriptor {
            label: Some("procedural_skybox_pipeline".into()),
            layout: vec![self.view_layout.clone(), self.material_layout.clone()],
            vertex: VertexState {
                shader: self.fullscreen_shader.shader(),
                entry_point: Some("fullscreen_vertex_shader".into()),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: key.shader,
                shader_defs: vec![material_group_def],
                targets: vec![Some(ColorTargetState {
                    format: key.target_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                // Reverse-Z clear is 0. The fullscreen vertex has that depth,
                // so it passes only where opaque geometry left the clear value.
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState {
                    front: StencilFaceState::IGNORE,
                    back: StencilFaceState::IGNORE,
                    read_mask: 0,
                    write_mask: 0,
                },
                bias: DepthBiasState {
                    constant: 0,
                    slope_scale: 0.0,
                    clamp: 0.0,
                },
            }),
            multisample: MultisampleState {
                count: key.samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            primitive: PrimitiveState::default(),
            ..default()
        }
    }
}

pub(crate) fn build(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<ProceduralSkyboxMaterial>::default())
        .add_observer(remove_skybox_material)
        .add_systems(
            PostUpdate,
            wire_directional_sun_inputs
                .after(lunco_environment::finalize_sun_render_state)
                .run_if(directional_sun_inputs_changed),
        );

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };

    render_app
        .init_gpu_resource::<SpecializedRenderPipelines<ProceduralSkyboxPipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(
            Render,
            (prepare_view_bind_groups.in_set(RenderSystems::PrepareBindGroups),),
        )
        .add_systems(
            Render,
            queue_procedural_skybox.in_set(RenderSystems::QueueMeshes),
        );
    render_app.add_render_command::<Opaque3d, DrawProceduralSkyboxCommands>();
}

const SUN_TAN_ANGULAR_RADIUS: f32 = 0.004_65;

fn sun_direction_in_view(world_direction: Vec3, camera_rotation: Quat) -> Option<Vec3> {
    if !world_direction.is_finite() || world_direction.length_squared() <= 0.0 {
        return None;
    }
    let view_direction = camera_rotation.inverse() * world_direction.normalize();
    (view_direction.is_finite() && view_direction.length_squared() > 0.0)
        .then(|| view_direction.normalize())
}

fn directional_sun_inputs_changed(
    sun: Option<Res<SunRenderState>>,
    cameras: Query<
        (),
        (
            With<SceneCamera>,
            With<Camera3d>,
            Or<(Added<Camera>, Changed<Camera>, Changed<GlobalTransform>)>,
        ),
    >,
    skyboxes: Query<(), Or<(Added<ProceduralSkyboxMaterial>, Changed<ProceduralSkyboxMaterial>)>>,
) -> bool {
    sun.is_some_and(|state| state.is_changed()) || !cameras.is_empty() || !skyboxes.is_empty()
}

/// Project Bevy's finalized scene-light direction into the active camera for
/// procedural sky disks. The disk uses a small fixed apparent radius; no solar
/// position or astronomical-distance value crosses the render boundary.
fn wire_directional_sun_inputs(
    sun: Option<Res<SunRenderState>>,
    cameras: Query<(&Camera, &GlobalTransform), (With<SceneCamera>, With<Camera3d>)>,
    skyboxes: Query<&ProceduralSkyboxMaterial>,
    materials: Option<ResMut<Assets<super::ShaderMaterial>>>,
) {
    let Some(mut materials) = materials else {
        return;
    };
    let state = sun.and_then(|state| state.direction_to_sun_world);
    let mut active_cameras = cameras.iter().filter(|(camera, _)| camera.is_active);
    let camera_rotation = active_cameras.next().map(|(_, transform)| transform.rotation());
    let direction = match (state, camera_rotation, active_cameras.next()) {
        (Some(world_direction), Some(rotation), None) => {
            sun_direction_in_view(world_direction, rotation).unwrap_or(Vec3::ZERO)
        }
        (None, _, _) => Vec3::ZERO,
        (Some(_), None, _) => Vec3::ZERO,
        (Some(_), Some(_), Some(_)) => {
            warn_once!("[render] multiple active scene cameras; procedural Sun disk is disabled");
            Vec3::ZERO
        }
    };
    let tan_radius = if direction.length_squared() > 0.0 {
        SUN_TAN_ANGULAR_RADIUS
    } else {
        0.0
    };

    for skybox in &skyboxes {
        let Some(mut material) = materials.get_mut(&skybox.material) else {
            continue;
        };
        let has_direction = material.schema.field("sun_dir_view").is_some();
        let has_radius = material.schema.field("sun_tan_radius").is_some();
        if !has_direction && !has_radius {
            continue;
        }
        if !(has_direction && has_radius) {
            warn_once!(
                "[render] procedural sky declares only one of `sun_dir_view` and `sun_tan_radius`; celestial Sun inputs require both"
            );
            continue;
        }
        let current_direction = material.get_vec3("sun_dir_view");
        let current_radius = material.get_scalar("sun_tan_radius");
        if current_direction
            .is_some_and(|current| (current - direction).length_squared() <= 1.0e-10)
            && current_radius.is_some_and(|current| (current - tan_radius).abs() <= 1.0e-7)
        {
            continue;
        }
        material.set_many([
            ("sun_dir_view", ParamValue::Vec3(direction.to_array())),
            ("sun_tan_radius", ParamValue::F32(tan_radius)),
        ]);
    }
}

fn init_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    fullscreen_shader: Res<FullscreenShader>,
) {
    let view_layout = BindGroupLayoutDescriptor::new(
        "procedural_skybox_view_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (uniform_buffer::<ViewUniform>(true),),
        ),
    );
    let material_layout = super::ShaderMaterial::bind_group_layout_descriptor(&render_device);
    commands.insert_resource(ProceduralSkyboxPipeline {
        view_layout,
        material_layout,
        fullscreen_shader: fullscreen_shader.clone(),
    });
}

fn selected_skybox(
    skyboxes: &Query<&ProceduralSkyboxMaterial>,
) -> Option<ProceduralSkyboxMaterial> {
    let mut iter = skyboxes.iter();
    let selected = iter.next()?.clone();
    if iter.next().is_some() {
        warn_once!(
            "multiple procedural skybox owners are active; the background is disabled until the scene has one owner"
        );
        return None;
    }
    Some(selected)
}

fn prepare_view_bind_groups(
    mut commands: Commands,
    pipeline: Res<ProceduralSkyboxPipeline>,
    pipeline_cache: Res<PipelineCache>,
    view_uniforms: Res<ViewUniforms>,
    render_device: Res<RenderDevice>,
    skyboxes: Query<&ProceduralSkyboxMaterial>,
    cameras: Query<(Entity, &ViewUniformOffset), With<Camera3d>>,
) {
    if selected_skybox(&skyboxes).is_none() {
        for (entity, _) in &cameras {
            commands
                .entity(entity)
                .remove::<ProceduralSkyboxViewBindGroup>();
        }
        return;
    }
    let Some(view_binding) = view_uniforms.uniforms.binding() else {
        return;
    };
    let layout = pipeline_cache.get_bind_group_layout(&pipeline.view_layout);
    for (entity, _) in &cameras {
        let bind_group = render_device.create_bind_group(
            "procedural_skybox_view_bind_group",
            &layout,
            &BindGroupEntries::sequential((view_binding.clone(),)),
        );
        commands
            .entity(entity)
            .try_insert(ProceduralSkyboxViewBindGroup(bind_group));
    }
}

type DrawProceduralSkyboxCommands = (SetItemPipeline, DrawProceduralSkybox);

struct DrawProceduralSkybox;

impl<P: PhaseItem> RenderCommand<P> for DrawProceduralSkybox {
    type Param = (
        SRes<ErasedRenderAssets<PreparedMaterial>>,
        SRes<RenderMaterialBindings>,
        SRes<MaterialBindGroupAllocators>,
    );
    type ViewQuery = (
        &'static ProceduralSkyboxViewBindGroup,
        &'static ViewUniformOffset,
    );
    type ItemQuery = &'static ProceduralSkyboxMaterial;

    fn render<'w>(
        _: &P,
        (view_bind_group, view_uniform_offset): ROQueryItem<'w, '_, Self::ViewQuery>,
        maybe_skybox: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (prepared_materials, material_bindings, material_allocators): SystemParamItem<
            'w,
            '_,
            Self::Param,
        >,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(skybox) = maybe_skybox else {
            return RenderCommandResult::Skip;
        };
        let prepared_materials = prepared_materials.into_inner();
        let material_bindings = material_bindings.into_inner();
        let material_allocators = material_allocators.into_inner();
        let Some(material_binding) = material_bindings.get(&skybox.material.id().untyped()) else {
            return RenderCommandResult::Skip;
        };
        let Some(prepared_material) = prepared_materials.get(skybox.material.id().untyped()) else {
            return RenderCommandResult::Skip;
        };
        let Some(material_allocator) =
            material_allocators.get(&TypeId::of::<super::ShaderMaterial>())
        else {
            return RenderCommandResult::Skip;
        };
        let Some(material_bind_group) = material_allocator
            .get(material_binding.group)
            .and_then(|slab| slab.bind_group())
        else {
            return RenderCommandResult::Skip;
        };
        if prepared_material.binding.group != material_binding.group
            || prepared_material.binding.slot != material_binding.slot
        {
            return RenderCommandResult::Skip;
        }

        pass.set_bind_group(0, &view_bind_group.0, &[view_uniform_offset.offset]);
        pass.set_bind_group(1, material_bind_group, &[]);
        pass.draw(0..3, 0..1);
        RenderCommandResult::Success
    }
}

fn queue_procedural_skybox(
    mut opaque_render_phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
    opaque_draw_functions: Res<DrawFunctions<Opaque3d>>,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<ProceduralSkyboxPipeline>>,
    pipeline: Res<ProceduralSkyboxPipeline>,
    skyboxes: Query<(Entity, &MainEntity, &ProceduralSkyboxMaterial)>,
    views: Query<(&ExtractedView, &Msaa), With<Camera3d>>,
    mut previous_skyboxes: Local<
        HashMap<RetainedViewEntity, (MainEntity, ProceduralSkyboxPipelineKey)>,
    >,
) {
    let draw_procedural_skybox = opaque_draw_functions
        .read()
        .id::<DrawProceduralSkyboxCommands>();
    let mut skyboxes = skyboxes.iter();
    let mut skybox = skyboxes
        .next()
        .map(|(entity, main_entity, skybox)| (entity, *main_entity, skybox.shader.clone()));
    if skyboxes.next().is_some() {
        warn_once!(
            "multiple procedural skybox owners are active; the background is disabled until the scene has one owner"
        );
        skybox = None;
    }
    for (view, msaa) in &views {
        let view_entity = view.retained_view_entity;
        let Some(opaque_phase) = opaque_render_phases.get_mut(&view_entity) else {
            continue;
        };
        let Some((render_entity, main_entity, shader)) = skybox.as_ref() else {
            if let Some((previous, _)) = previous_skyboxes.remove(&view_entity) {
                opaque_phase.remove(previous);
            }
            continue;
        };
        let pipeline_key = ProceduralSkyboxPipelineKey {
            shader: shader.clone(),
            target_format: view.target_format,
            samples: msaa.samples(),
        };
        let previous = previous_skyboxes.remove(&view_entity);
        if !same_skybox_render_item(previous.as_ref(), *main_entity, &pipeline_key) {
            if let Some((previous, _)) = previous {
                opaque_phase.remove(previous);
            }
        }
        let pipeline_id = pipelines.specialize(&pipeline_cache, &pipeline, pipeline_key.clone());
        opaque_phase.add(
            Opaque3dBatchSetKey {
                draw_function: draw_procedural_skybox,
                pipeline: pipeline_id,
                material_bind_group_index: None,
                slabs: MeshSlabs::default(),
                lightmap_slab: None,
            },
            Opaque3dBinKey {
                asset_id: AssetId::<Mesh>::invalid().untyped(),
            },
            (*render_entity, *main_entity),
            InputUniformIndex::default(),
            BinnedRenderPhaseType::NonMesh,
        );
        previous_skyboxes.insert(view_entity, (*main_entity, pipeline_key));
    }
}

fn remove_skybox_material(remove: On<Remove, ProceduralSkybox>, mut commands: Commands) {
    commands
        .entity(remove.entity)
        .remove::<ProceduralSkyboxMaterial>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_disk_uses_the_directional_light_in_camera_space() {
        let world_direction = Vec3::NEG_Z;
        let camera_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);

        let view_direction = sun_direction_in_view(world_direction, camera_rotation)
            .expect("finite scene sun direction");

        assert!(view_direction.abs_diff_eq(Vec3::X, 1.0e-6));
    }

    #[test]
    fn sky_disk_rejects_an_invalid_direction() {
        assert!(sun_direction_in_view(Vec3::ZERO, Quat::IDENTITY).is_none());
        assert!(sun_direction_in_view(Vec3::splat(f32::NAN), Quat::IDENTITY).is_none());
    }

    fn key(target_format: TextureFormat) -> ProceduralSkyboxPipelineKey {
        ProceduralSkyboxPipelineKey {
            shader: Handle::default(),
            target_format,
            samples: 1,
        }
    }

    #[test]
    fn render_item_changes_when_target_attachment_changes() {
        let owner = MainEntity::from(Entity::from_raw_u32(7).unwrap());
        let previous = (owner, key(TextureFormat::Rgba16Float));
        assert!(same_skybox_render_item(
            Some(&previous),
            owner,
            &key(TextureFormat::Rgba16Float)
        ));
        assert!(!same_skybox_render_item(
            Some(&previous),
            owner,
            &key(TextureFormat::Rgba8UnormSrgb)
        ));
    }
}
