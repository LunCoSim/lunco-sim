//! Camera-background binding for procedural shader looks.
//!
//! A [`ProceduralSkybox`] is a render-free marker on the same entity as a
//! [`ShaderLook`]. The normal shader material binder still owns the material
//! asset and its reflected parameters; this module only adds the render path
//! that draws that material with a fullscreen triangle after opaque geometry.
//! Depth testing leaves the already-rendered scene in front while the clear
//! value admits the background, matching Bevy's built-in skybox lifecycle. That
//! keeps sky appearance in WGSL/USD while removing the finite-sphere culling
//! and far-plane coupling from the scene.

use bevy::camera::{Camera3d, MainPassResolutionOverride, Viewport};
use bevy::core_pipeline::{
    core_3d::{main_opaque_pass_3d, CORE_3D_DEPTH_FORMAT},
    Core3d, Core3dSystems, FullscreenShader,
};
use bevy::pbr::{MaterialBindGroupAllocators, PreparedMaterial, RenderMaterialBindings};
use bevy::prelude::*;
use bevy::render::{
    camera::ExtractedCamera,
    erased_render_asset::ErasedRenderAssets,
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    render_resource::{
        binding_types::uniform_buffer, AsBindGroup, BindGroup, BindGroupEntries,
        BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
        ColorTargetState, ColorWrites, CompareFunction, DepthBiasState, DepthStencilState,
        FragmentState, MultisampleState, PipelineCache, PrimitiveState, RenderPassDescriptor,
        RenderPipelineDescriptor, ShaderStages, SpecializedRenderPipeline,
        SpecializedRenderPipelines, StencilFaceState, StencilState, StoreOp, TextureFormat,
        VertexState,
    },
    renderer::{RenderContext, RenderDevice, ViewQuery},
    view::{
        ExtractedView, Msaa, ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset,
        ViewUniforms,
    },
    GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy::shader::Shader;
use bevy::shader::ShaderDefVal;
use bevy::utils::default;
use lunco_materials::ProceduralSkybox;
use std::any::TypeId;

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
struct ProceduralSkyboxPipelineId(CachedRenderPipelineId);

#[derive(Component)]
struct ProceduralSkyboxViewBindGroup(BindGroup);

#[derive(Clone, PartialEq, Eq, Hash)]
struct ProceduralSkyboxPipelineKey {
    shader: Handle<Shader>,
    target_format: TextureFormat,
    samples: u32,
}

impl SpecializedRenderPipeline for ProceduralSkyboxPipeline {
    type Key = ProceduralSkyboxPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let background_def = ShaderDefVal::from("LUNCO_PROCEDURAL_SKY_BACKGROUND");
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
                shader_defs: vec![background_def.clone(), material_group_def.clone()],
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
                // Reverse-Z clear is 0. The background passes at that clear
                // value after opaque geometry has populated the depth buffer;
                // rover and terrain fragments therefore reject it in front.
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
        .add_observer(remove_skybox_material);

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };

    render_app
        .init_gpu_resource::<SpecializedRenderPipelines<ProceduralSkyboxPipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(
            Render,
            (
                prepare_pipelines.in_set(RenderSystems::Prepare),
                prepare_view_bind_groups.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        .add_systems(
            Core3d,
            draw_procedural_skybox
                .in_set(Core3dSystems::MainPass)
                .after(main_opaque_pass_3d),
        );
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

fn prepare_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<ProceduralSkyboxPipeline>>,
    pipeline: Res<ProceduralSkyboxPipeline>,
    skyboxes: Query<&ProceduralSkyboxMaterial>,
    cameras: Query<(Entity, &ExtractedView, &Msaa), With<Camera3d>>,
) {
    let Some(skybox) = selected_skybox(&skyboxes) else {
        for (entity, _, _) in &cameras {
            commands
                .entity(entity)
                .remove::<ProceduralSkyboxPipelineId>();
        }
        return;
    };
    for (entity, view, msaa) in &cameras {
        let pipeline_id = pipelines.specialize(
            &pipeline_cache,
            &pipeline,
            ProceduralSkyboxPipelineKey {
                shader: skybox.shader.clone(),
                target_format: view.target_format,
                samples: msaa.samples(),
            },
        );
        commands
            .entity(entity)
            .insert(ProceduralSkyboxPipelineId(pipeline_id));
    }
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
            .insert(ProceduralSkyboxViewBindGroup(bind_group));
    }
}

fn draw_procedural_skybox(
    _world: &World,
    view: ViewQuery<(
        &ExtractedCamera,
        &ExtractedView,
        &ViewTarget,
        &ViewDepthTexture,
        &ProceduralSkyboxPipelineId,
        &ProceduralSkyboxViewBindGroup,
        &ViewUniformOffset,
        Option<&MainPassResolutionOverride>,
    )>,
    skyboxes: Query<&ProceduralSkyboxMaterial>,
    pipeline_cache: Res<PipelineCache>,
    prepared_materials: Res<ErasedRenderAssets<PreparedMaterial>>,
    material_bindings: Res<RenderMaterialBindings>,
    material_allocators: Res<MaterialBindGroupAllocators>,
    mut ctx: RenderContext,
) {
    let Some(skybox) = selected_skybox(&skyboxes) else {
        return;
    };
    let Some(material_binding) = material_bindings.get(&skybox.material.id().untyped()) else {
        return;
    };
    let Some(prepared_material) = prepared_materials.get(skybox.material.id().untyped()) else {
        return;
    };
    let Some(material_allocator) = material_allocators.get(&TypeId::of::<super::ShaderMaterial>())
    else {
        return;
    };
    let Some(material_bind_group) = material_allocator
        .get(material_binding.group)
        .and_then(|slab| slab.bind_group())
    else {
        return;
    };
    // Keep the prepared asset lookup in the same path as Bevy's material draw
    // command. The binding map and prepared asset must agree before a frame is
    // allowed to submit the background.
    if prepared_material.binding.group != material_binding.group
        || prepared_material.binding.slot != material_binding.slot
    {
        return;
    }

    let (
        camera,
        _view,
        target,
        depth,
        pipeline_id,
        view_bind_group,
        view_uniform_offset,
        resolution_override,
    ) = view.into_inner();
    let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id.0) else {
        return;
    };

    let color_attachments = [Some(target.get_color_attachment())];
    let depth_stencil_attachment = Some(depth.get_attachment(StoreOp::Store));
    let mut render_pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("procedural_skybox_pass"),
        color_attachments: &color_attachments,
        depth_stencil_attachment,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if let Some(viewport) =
        Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution_override)
    {
        render_pass.set_camera_viewport(&viewport);
    }
    render_pass.set_render_pipeline(pipeline);
    render_pass.set_bind_group(0, &view_bind_group.0, &[view_uniform_offset.offset]);
    render_pass.set_bind_group(1, material_bind_group, &[]);
    render_pass.draw(0..3, 0..1);
}

fn remove_skybox_material(remove: On<Remove, ProceduralSkybox>, mut commands: Commands) {
    commands
        .entity(remove.entity)
        .remove::<ProceduralSkyboxMaterial>();
}
