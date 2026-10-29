//! Native per-camera opaque depth and sensor image post-processing.
use bevy::camera::CameraUpdateSystems;
use bevy::core_pipeline::{
    FullscreenShader,
    prepass::ViewPrepassTextures,
    schedule::{Core3d, Core3dSystems},
    tonemapping::tonemapping,
};
use bevy::prelude::*;
use bevy::render::{
    Render, RenderApp, RenderStartup, RenderSystems,
    extract_component::{
        ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
        UniformComponentPlugin,
    },
    render_resource::{
        binding_types::{sampler, texture_2d, uniform_buffer},
        *,
    },
    renderer::{RenderContext, RenderDevice, ViewQuery},
    view::{PostProcessBindGroupCache, PostProcessBindGroupCacheBuilder, ViewTarget},
};
use bevy::shader::ShaderDefVal;
use lunco_render::camera_image::{CameraImageEffects, CameraImageMode, CameraImagePresentation};

#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
#[extract_app(RenderApp)]
struct ImageUniform {
    view_from_clip: Mat4,
    viewport: Vec4,
    range: Vec4,
    noise: Vec4,
    timing: UVec4,
}
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(RenderApp)]
struct EffectShader(Handle<Shader>);
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(RenderApp)]
struct ImagePresentation(CameraImagePresentation);
#[derive(Component)]
struct ImageSourceKey(String);

pub(crate) fn build(app: &mut App) {
    app.register_type::<CameraImageEffects>()
        .add_plugins((
            ExtractComponentPlugin::<ImageUniform>::default(),
            UniformComponentPlugin::<ImageUniform>::default(),
            ExtractComponentPlugin::<EffectShader>::default(),
            ExtractComponentPlugin::<ImagePresentation>::default(),
        ))
        .add_systems(PostUpdate, sync_image_intent.after(CameraUpdateSystems));
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<SpecializedRenderPipelines<ImagePipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(
                Core3d,
                draw.in_set(Core3dSystems::PostProcess).after(tonemapping),
            )
            .add_systems(Render, mark_clean_presented.after(RenderSystems::Render));
    }
}
fn sync_image_intent(
    sources: Query<(
        Entity,
        Ref<CameraImageEffects>,
        Ref<Projection>,
        &Camera,
        Option<&ImageSourceKey>,
        Option<&CameraImagePresentation>,
    )>,
    retired: Query<Entity, (With<CameraImagePresentation>, Without<CameraImageEffects>)>,
    frames: Res<bevy::diagnostic::FrameCount>,
    server: Res<AssetServer>,
    mut commands: Commands,
) {
    for entity in &retired {
        commands.entity(entity).try_remove::<(
            ImageUniform,
            EffectShader,
            ImageSourceKey,
            CameraImagePresentation,
            ImagePresentation,
        )>();
    }
    for (entity, effect, projection, camera, shader, presentation) in &sources {
        let dimensions = camera.physical_viewport_size();
        if effect.is_changed()
            || projection.is_changed()
            || presentation.is_none_or(|current| {
                current.dimensions != dimensions || current.settings != *effect
            })
        {
            let receipt = CameraImagePresentation::new((*effect).clone(), dimensions);
            commands
                .entity(entity)
                .try_insert((receipt.clone(), ImagePresentation(receipt)));
        }
        if !effect.enabled() {
            if shader.is_some() {
                commands
                    .entity(entity)
                    .try_remove::<(ImageUniform, EffectShader, ImageSourceKey)>();
            }
            continue;
        }
        if let Err(error) = effect.validate() {
            if effect.is_changed() {
                lunco_core::trigger_runtime_error(&mut commands, "camera-image-effects", error);
            }
            commands
                .entity(entity)
                .try_remove::<(ImageUniform, EffectShader, ImageSourceKey)>();
            continue;
        }
        let Some(viewport) = camera.physical_viewport_rect() else {
            continue;
        };
        // Explicit GPU boundary: validated, bounded image parameters become f32.
        let uniform = ImageUniform {
            view_from_clip: projection.get_clip_from_view().inverse(),
            viewport: Vec4::new(
                viewport.min.x as f32,
                viewport.min.y as f32,
                viewport.width() as f32,
                viewport.height() as f32,
            ),
            range: Vec4::new(
                effect.near_m as f32,
                effect.far_m as f32,
                if effect.mode == CameraImageMode::Depth {
                    1.0
                } else {
                    0.0
                },
                0.0,
            ),
            noise: Vec4::new(
                effect.read_noise as f32,
                effect.shot_noise as f32,
                effect.row_noise as f32,
                effect.dropout as f32,
            ),
            timing: UVec4::new(
                effect.seed,
                if effect.animated { frames.0 } else { 0 },
                effect.quantization_levels,
                0,
            ),
        };
        if shader.is_none_or(|current| current.0 != effect.shader) {
            let handle = server.load(effect.shader.clone());
            commands
                .entity(entity)
                .try_insert((EffectShader(handle), ImageSourceKey(effect.shader.clone())));
        }
        commands.entity(entity).try_insert(uniform);
    }
}

#[derive(Resource)]
struct ImagePipeline {
    layouts: [BindGroupLayoutDescriptor; 2],
    sampler: Sampler,
    fullscreen: FullscreenShader,
}
#[derive(Clone, Hash, PartialEq, Eq)]
struct ImageKey {
    format: TextureFormat,
    multisampled: bool,
    shader: Handle<Shader>,
}
fn image_layout(multisampled: bool) -> BindGroupLayoutDescriptor {
    let mut entries = BindGroupLayoutEntries::sequential(
        ShaderStages::FRAGMENT,
        (
            texture_2d(TextureSampleType::Float { filterable: true }),
            sampler(SamplerBindingType::Filtering),
            uniform_buffer::<ImageUniform>(true),
        ),
    )
    .to_vec();
    entries.push(BindGroupLayoutEntry {
        binding: 3,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture {
            sample_type: TextureSampleType::Depth,
            view_dimension: TextureViewDimension::D2,
            multisampled,
        },
        count: None,
    });
    BindGroupLayoutDescriptor::new("camera_image_layout", &entries)
}
fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
) {
    commands.insert_resource(ImagePipeline {
        layouts: [image_layout(false), image_layout(true)],
        sampler: device.create_sampler(&SamplerDescriptor::default()),
        fullscreen: fullscreen.clone(),
    });
}
impl SpecializedRenderPipeline for ImagePipeline {
    type Key = ImageKey;
    fn specialize(&self, key: ImageKey) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("camera_image".into()),
            layout: vec![self.layouts[usize::from(key.multisampled)].clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: key.shader,
                shader_defs: vec![ShaderDefVal::Bool(
                    "CAMERA_MULTISAMPLED".into(),
                    key.multisampled,
                )],
                targets: vec![Some(ColorTargetState {
                    format: key.format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}
#[derive(Component)]
struct PreparedImage {
    cache: PostProcessBindGroupCache,
    depth: TextureViewId,
    buffer: BufferId,
    key: ImageKey,
    pipeline: CachedRenderPipelineId,
}
fn prepare(
    mut commands: Commands,
    mut views: Query<
        (
            Entity,
            &ViewTarget,
            &ViewPrepassTextures,
            &Msaa,
            &EffectShader,
            Option<&mut PreparedImage>,
        ),
        With<ImageUniform>,
    >,
    pipeline: Option<Res<ImagePipeline>>,
    mut pipelines: ResMut<SpecializedRenderPipelines<ImagePipeline>>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<ImageUniform>>,
    device: Res<RenderDevice>,
    retired: Query<Entity, (With<PreparedImage>, Without<ImageUniform>)>,
) {
    for entity in &retired {
        commands.entity(entity).remove::<PreparedImage>();
    }
    let Some(pipeline) = pipeline else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let Some(buffer) = uniforms.uniforms().buffer() else {
        return;
    };
    let buffer_id = buffer.id();
    for (entity, target, depth, msaa, shader, prepared) in &mut views {
        let Some(depth) = depth.depth_only_view() else {
            continue;
        };
        let key = ImageKey {
            format: target.main_texture_format(),
            multisampled: msaa.samples() > 1,
            shader: shader.0.clone(),
        };
        let pipeline_id = pipelines.specialize(&cache, &pipeline, key.clone());
        let builder = PostProcessBindGroupCacheBuilder::new(|texture: &TextureView| {
            (
                texture.id(),
                device.create_bind_group(
                    "camera_image_bindings",
                    &cache.get_bind_group_layout(&pipeline.layouts[usize::from(key.multisampled)]),
                    &BindGroupEntries::sequential((
                        texture,
                        &pipeline.sampler,
                        binding.clone(),
                        depth,
                    )),
                ),
            )
        });
        if let Some(mut prepared) = prepared {
            if prepared.depth != depth.id()
                || prepared.buffer != buffer_id
                || prepared.key != key
                || prepared.cache.should_update(target)
            {
                prepared.cache.update(target, builder);
                prepared.depth = depth.id();
                prepared.buffer = buffer_id;
                prepared.key = key;
            }
            prepared.pipeline = pipeline_id;
        } else {
            commands.entity(entity).insert(PreparedImage {
                cache: builder.generate_bind_groups(target),
                depth: depth.id(),
                buffer: buffer_id,
                key,
                pipeline: pipeline_id,
            });
        }
    }
}
fn mark_clean_presented(
    views: Query<&ImagePresentation, (With<ViewTarget>, Without<ImageUniform>)>,
) {
    for receipt in &views {
        receipt.0.mark_presented();
    }
}
fn draw(
    view: ViewQuery<(
        &ViewTarget,
        &DynamicUniformIndex<ImageUniform>,
        &PreparedImage,
        &ImageUniform,
        &ImagePresentation,
    )>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, index, prepared, _, presentation) = view.into_inner();
    let Some(pipeline) = cache.get_render_pipeline(prepared.pipeline) else {
        return;
    };
    let post = target.post_process_write();
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("camera_image"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(
        0,
        prepared.cache.get_current_bind_group(post.source),
        &[index.index()],
    );
    pass.draw(0..3, 0..1);
    presentation.0.mark_presented();
}
