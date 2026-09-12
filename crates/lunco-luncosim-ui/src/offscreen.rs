//! GPU-backed, windowless recording support for LunCoSim.
//!
//! This belongs to the application UI package because it is a presentation
//! concern: it creates an image target, mirrors the authored presentation
//! camera, and reports render-pipeline readiness to the recorder. The shared
//! simulator core remains independent of this capture machinery.

use bevy::prelude::*;
use big_space::prelude::*;

/// The GPU-backed windowless recording mode (`--offscreen`) uses the same
/// simulator plugins as a windowed run, but renders into an image target. The
/// render stack is real (wgpu device, render world, and visual plugins), but no
/// window opens: the scene renders into an offscreen target image sized by
/// `--record-size WxH` (default 1280x720, the same resolution as the windowed
/// app) and the offline recorder captures that image. Combined with
/// `--record-offline out.mp4 --record-frames N`, the process exits once the
/// recording drains.
///
/// The shared simulator core owns the headless server plugin; this module owns
/// only the GPU recording path.
#[cfg(feature = "api-transport")]
pub struct LunCoSimOffscreenPlugin;

#[cfg(feature = "api-transport")]
impl Plugin for LunCoSimOffscreenPlugin {
    fn build(&self, app: &mut App) {
        // Same non-UI cores the headless server needs (see the twin comments in
        // `LunCoSimHeadlessPlugin`): the Modelica compile channels and the
        // spawn-command registry both normally arrive via UI plugins.
        app.add_plugins(lunco_modelica_core::ModelicaCorePlugin);
        app.add_plugins(lunco_scene_commands::commands::SpawnCommandPlugin);
        // The trail has no egui or picking dependency, but it is still part of
        // the rendered presentation and must be present in offscreen captures.
        app.add_plugins(lunco_luncosim_edit_ui::ui::VehicleTrailPlugin);

        // The workspace session (WorkspaceResource + journal persistence) —
        // the GUI gets this from `WorkbenchPlugin`, which this mode skips.
        // `setup_luncosim`'s twin-load path panics without it.
        app.add_plugins(lunco_workspace::WorkspacePlugin);

        // `CloseWindow` is a presentation intent in the shared recording
        // script. Offscreen has no OS window; the recorder's drained-state
        // exit owns process lifetime, so acknowledge this explicit intent
        // through the shared scenario-command policy instead of advertising a
        // window command whose semantics would terminate before the video
        // trailer is written.
        app.insert_resource(lunco_scripting::bridge_core::IgnoredScenarioCommands::new(
            ["CloseWindow"],
        ));

        // Recording owns its deterministic clock. Do not inherit a persisted
        // editor cadence (especially the scene-test EXACT setting), which
        // makes the expensive celestial cluster solve on every evaluation.
        app.insert_resource(lunco_celestial::cadence::CelestialCadenceSettings::default());

        // Presentation commands remain part of the scenario command surface
        // without requiring the egui workbench in an offscreen run.
        app.add_plugins(lunco_theme::ThemePlugin);
        app.add_plugins(lunco_workbench::theme_command::ThemeCommandPlugin);
        lunco_workbench::input_overlay::register_input_overlay_commands(app);

        // The recorder has no OS window and therefore no egui host. It still
        // renders Bevy UI into the same image as the authored scene camera;
        // install the shared HUI/Flair exposure layer so film HUDs are
        // captured as pixels rather than remaining editor-only overlays.
        crate::add_runtime_ui_layer(app);

        // The offline recorder itself — normally added by `WorkbenchPlugin`,
        // which this mode skips (egui needs a window).
        app.init_resource::<lunco_status_core::status_bus::StatusBus>();
        app.add_plugins(lunco_workbench::screenshot::ScreenshotPlugin);

        // No winit event loop, so tick the app ourselves — flat out, zero wait:
        // while recording, `drive_offline_clock` paces the sim (one 1/fps step
        // per frame, back-pressure holds the clock), so a faster tick rate means
        // faster-than-realtime capture, never a wrong-speed video.
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::ZERO,
        ));

        // One-shot contract: when the recording fully drains (frames delivered,
        // saves done, video trailer written), exit the process.
        app.insert_resource(lunco_workbench::screenshot::ExitAfterRecording);

        app.add_systems(Startup, setup_offscreen_target);
        app.add_systems(
            Update,
            (
                retarget_cameras_to_offscreen,
                activate_offscreen_camera,
                maintain_offscreen_render_camera,
            )
                .chain(),
        );

        // Keep the windowless render contract observable at the render boundary: the main world
        // cannot know whether visibility and phase binning actually admitted a mesh. The render
        // acknowledgement is consumed by the recorder before it starts virtual time.
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.init_resource::<lunco_workbench::screenshot::OfflineRenderReadiness>();
            render_app.add_systems(
                bevy::render::ExtractSchedule,
                copy_offscreen_render_readiness_to_main_world,
            );
            render_app.add_systems(
                bevy::render::Render,
                report_offscreen_render_view.in_set(bevy::render::RenderSystems::Prepare),
            );
        }

        info!(
            "[offscreen] GPU-full windowless recording mode: no window, scene renders to an offscreen target"
        );
    }
}

#[cfg(feature = "api-transport")]
fn report_offscreen_render_view(
    cameras: Query<(
        Entity,
        &bevy::render::camera::ExtractedCamera,
        Option<&bevy::render::view::ExtractedView>,
        Option<&bevy::render::view::visibility::RenderVisibleEntities>,
    )>,
    opaque_phases: Option<
        Res<
            bevy::render::render_phase::ViewBinnedRenderPhases<
                bevy::core_pipeline::core_3d::Opaque3d,
            >,
        >,
    >,
    alpha_mask_phases: Option<
        Res<
            bevy::render::render_phase::ViewBinnedRenderPhases<
                bevy::core_pipeline::core_3d::AlphaMask3d,
            >,
        >,
    >,
    transparent_phases: Option<
        Res<
            bevy::render::render_phase::ViewSortedRenderPhases<
                bevy::core_pipeline::core_3d::Transparent3d,
            >,
        >,
    >,
    pipeline_cache: Res<bevy::render::render_resource::PipelineCache>,
    mut readiness: ResMut<lunco_workbench::screenshot::OfflineRenderReadiness>,
    mut ready_reported: Local<bool>,
) {
    *readiness = Default::default();
    for (_entity, camera, view, visible) in &cameras {
        let visible_entities = visible.map_or(0, |visible| {
            visible
                .classes
                .values()
                .map(|class| class.entities_cpu_culling.len() + class.entities_gpu_culling.len())
                .sum()
        });
        let opaque_bins = view
            .and_then(|view| {
                opaque_phases
                    .as_deref()
                    .and_then(|phases| phases.0.get(&view.retained_view_entity))
            })
            .map(|phase| {
                phase.multidrawable_meshes.len()
                    + phase.batchable_meshes.len()
                    + phase.unbatchable_meshes.len()
                    + phase.non_mesh_items.len()
            })
            .unwrap_or(0);
        let opaque_pipelines_ready = view.and_then(|view| {
            opaque_phases.as_deref().and_then(|phases| {
                phases.0.get(&view.retained_view_entity).map(|phase| {
                    let has_items = !phase.multidrawable_meshes.is_empty()
                        || !phase.batchable_meshes.is_empty()
                        || !phase.unbatchable_meshes.is_empty()
                        || !phase.non_mesh_items.is_empty();
                    has_items
                        && phase
                            .multidrawable_meshes
                            .keys()
                            .all(|key| pipeline_cache.get_render_pipeline(key.pipeline).is_some())
                        && phase.batchable_meshes.keys().all(|(key, _)| {
                            pipeline_cache.get_render_pipeline(key.pipeline).is_some()
                        })
                        && phase.unbatchable_meshes.keys().all(|(key, _)| {
                            pipeline_cache.get_render_pipeline(key.pipeline).is_some()
                        })
                })
            })
        });
        let alpha_mask_pipelines_ready = view.and_then(|view| {
            alpha_mask_phases.as_deref().and_then(|phases| {
                phases.0.get(&view.retained_view_entity).map(|phase| {
                    let has_items = !phase.multidrawable_meshes.is_empty()
                        || !phase.batchable_meshes.is_empty()
                        || !phase.unbatchable_meshes.is_empty()
                        || !phase.non_mesh_items.is_empty();
                    has_items
                        && phase
                            .multidrawable_meshes
                            .keys()
                            .all(|key| pipeline_cache.get_render_pipeline(key.pipeline).is_some())
                        && phase.batchable_meshes.keys().all(|(key, _)| {
                            pipeline_cache.get_render_pipeline(key.pipeline).is_some()
                        })
                        && phase.unbatchable_meshes.keys().all(|(key, _)| {
                            pipeline_cache.get_render_pipeline(key.pipeline).is_some()
                        })
                })
            })
        });
        let transparent_items = view
            .and_then(|view| {
                transparent_phases
                    .as_deref()
                    .and_then(|phases| phases.0.get(&view.retained_view_entity))
            })
            .map_or(0, |phase| phase.items.len());
        let transparent_pipelines_ready = view.and_then(|view| {
            transparent_phases.as_deref().and_then(|phases| {
                phases.0.get(&view.retained_view_entity).map(|phase| {
                    !phase.items.is_empty()
                        && phase
                            .items
                            .values()
                            .all(|item| pipeline_cache.get_render_pipeline(item.pipeline).is_some())
                })
            })
        });
        let is_capture_view = matches!(
            camera.output_mode,
            bevy::camera::CameraOutputMode::Write { .. }
        ) && camera
            .target
            .as_ref()
            .is_some_and(|target| matches!(target, bevy::camera::NormalizedRenderTarget::Image(_)));
        if is_capture_view {
            if let Some(view) = view {
                readiness.camera = Some(view.retained_view_entity.main_entity.id());
                readiness.visible_entities = visible_entities;
                readiness.opaque_items = opaque_bins;
                readiness.transparent_items = transparent_items;
                readiness.pipelines_ready = opaque_pipelines_ready.unwrap_or(false)
                    || alpha_mask_pipelines_ready.unwrap_or(false)
                    || transparent_pipelines_ready.unwrap_or(false);
                if !*ready_reported && opaque_bins + transparent_items > 0 {
                    info!(
                        "[offscreen] capture view submitted scene items: main={:?} world_translation={:?} visible_entities={visible_entities} opaque_bins={opaque_bins} transparent_items={transparent_items} pipelines_ready={}",
                        view.retained_view_entity.main_entity,
                        view.world_from_view.translation(),
                        readiness.pipelines_ready,
                    );
                    *ready_reported = true;
                }
            }
        }
    }
}

/// Copy the previous render-frame acknowledgement into the main world during
/// extraction. `MainWorld` is available at this boundary, while render-phase
/// bins are only available later in `RenderSystems::Prepare`; the two systems
/// therefore form one explicit one-frame handoff rather than observing a main
/// world approximation of render participation.
#[cfg(feature = "api-transport")]
fn copy_offscreen_render_readiness_to_main_world(
    mut main_world: ResMut<bevy::render::MainWorld>,
    readiness: Res<lunco_workbench::screenshot::OfflineRenderReadiness>,
) {
    if let Some(mut main_readiness) =
        main_world.get_resource_mut::<lunco_workbench::screenshot::OfflineRenderReadiness>()
    {
        *main_readiness = *readiness;
    }
}

/// Create the offscreen render-target image and expose it to the recorder as
/// [`lunco_workbench::screenshot::OfflineCaptureTarget`].
#[cfg(feature = "api-transport")]
fn setup_offscreen_target(mut images: ResMut<Assets<bevy::image::Image>>, mut commands: Commands) {
    let (width, height) = parse_record_size();
    let mut image = bevy::image::Image::new_target_texture(
        width,
        height,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    );
    // `new_target_texture` sets RENDER_ATTACHMENT|TEXTURE_BINDING|COPY_DST;
    // the screenshot readback additionally copies OUT of the texture.
    image.texture_descriptor.usage |= bevy::render::render_resource::TextureUsages::COPY_SRC;
    let handle = images.add(image);
    info!("[offscreen] render target {width}x{height} (override with --record-size WxH)");
    commands.insert_resource(lunco_workbench::screenshot::OfflineCaptureTarget(handle));
}

/// Point cameras that target a window at the offscreen image. The authored
/// camera remains the explicit non-writing pose owner and the maintained image
/// camera below is its render consumer. Runs every frame because cameras spawn
/// throughout a session (scene loads, camera paths, possession).
#[cfg(feature = "api-transport")]
fn retarget_cameras_to_offscreen(
    target: Option<Res<lunco_workbench::screenshot::OfflineCaptureTarget>>,
    mut cameras: Query<(
        &mut bevy::camera::RenderTarget,
        Option<&mut bevy::camera::Projection>,
    )>,
) {
    let Some(target) = target else { return };
    for (mut rt, projection) in &mut cameras {
        if matches!(*rt, bevy::camera::RenderTarget::Window(_)) {
            *rt = bevy::camera::RenderTarget::Image(target.0.clone().into());
            // BEVY QUIRK (0.19): `camera_system` recomputes a camera's target
            // info on window/image EVENTS, `is_added`, or PROJECTION changes —
            // NOT on `RenderTarget` component changes. A camera whose
            // projection bound while its target was still the nonexistent
            // primary window resolves to nothing, and pointing it at the image
            // afterwards leaves `computed_size = None` FOREVER — the render
            // world silently skips it (black take, no log). Touching the
            // projection's change tick forces the recompute.
            if let Some(mut projection) = projection {
                projection.set_changed();
            }
        }
    }
}

/// Bevy's camera target is an immutable render-graph choice in practice: changing
/// a live window camera to an image updates its projection metadata, but leaves
/// the original camera's output path bound to the windowless swapchain setup.
/// Keep the authored camera as the pose owner and render that pose through a
/// camera created with the image target from birth.
#[cfg(feature = "api-transport")]
#[derive(Component)]
struct OffscreenRenderCamera(Entity);

#[cfg(feature = "api-transport")]
fn skybox_matches(left: &bevy::light::Skybox, right: &bevy::light::Skybox) -> bool {
    left.image == right.image
        && left.brightness == right.brightness
        && left.rotation == right.rotation
}

#[cfg(feature = "api-transport")]
fn generated_environment_map_matches(
    left: &bevy::light::GeneratedEnvironmentMapLight,
    right: &bevy::light::GeneratedEnvironmentMapLight,
) -> bool {
    left.environment_map == right.environment_map
        && left.intensity == right.intensity
        && left.rotation == right.rotation
        && left.affects_lightmapped_mesh_diffuse == right.affects_lightmapped_mesh_diffuse
}

#[cfg(feature = "api-transport")]
fn sync_offscreen_environment(
    commands: &mut Commands,
    mirror_entity: Entity,
    source_skybox: Option<&bevy::light::Skybox>,
    source_environment: Option<&bevy::light::GeneratedEnvironmentMapLight>,
    mut mirror_skybox: Option<&mut bevy::light::Skybox>,
    mut mirror_environment: Option<&mut bevy::light::GeneratedEnvironmentMapLight>,
    mirror_derived_environment: Option<&bevy::light::EnvironmentMapLight>,
) {
    let skybox_changed = match (source_skybox, mirror_skybox.as_deref()) {
        (Some(source), Some(mirror)) => !skybox_matches(source, mirror),
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => false,
    };
    if skybox_changed {
        match source_skybox {
            Some(source) => {
                if let Some(mirror) = mirror_skybox.as_deref_mut() {
                    *mirror = source.clone();
                } else {
                    commands.entity(mirror_entity).insert(source.clone());
                }
            }
            None => {
                commands
                    .entity(mirror_entity)
                    .remove::<bevy::light::Skybox>();
            }
        }
    }

    let environment_changed = match (source_environment, mirror_environment.as_deref()) {
        (Some(source), Some(mirror)) => !generated_environment_map_matches(source, mirror),
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => mirror_derived_environment.is_some(),
    };
    if environment_changed {
        match source_environment {
            Some(source) => {
                if let Some(mirror) = mirror_environment.as_deref_mut() {
                    *mirror = source.clone();
                } else {
                    commands.entity(mirror_entity).insert(source.clone());
                }
            }
            None => {
                commands.entity(mirror_entity).remove::<(
                    bevy::light::GeneratedEnvironmentMapLight,
                    bevy::light::EnvironmentMapLight,
                )>();
            }
        }
        if mirror_derived_environment.is_some() {
            commands
                .entity(mirror_entity)
                .remove::<bevy::light::EnvironmentMapLight>();
        }
    }
}

#[cfg(feature = "api-transport")]
fn maintain_offscreen_render_camera(
    target: Option<Res<lunco_workbench::screenshot::OfflineCaptureTarget>>,
    sources: Query<
        (
            Entity,
            &Transform,
            &Projection,
            &bevy::camera::Exposure,
            &bevy::core_pipeline::tonemapping::Tonemapping,
            &bevy::render::view::Msaa,
            Option<&ChildOf>,
            Option<&CellCoord>,
            Option<&bevy::light::Skybox>,
            Option<&bevy::light::GeneratedEnvironmentMapLight>,
        ),
        (
            With<lunco_render::SceneCamera>,
            With<lunco_core::LocalAvatar>,
            Without<OffscreenRenderCamera>,
        ),
    >,
    mut source_cameras: Query<
        &mut Camera,
        (
            With<lunco_render::SceneCamera>,
            Without<OffscreenRenderCamera>,
        ),
    >,
    mut mirrors: Query<(
        Entity,
        &OffscreenRenderCamera,
        &mut Transform,
        &mut Projection,
        &mut Camera,
        Option<&mut bevy::camera::Exposure>,
        Option<&mut bevy::core_pipeline::tonemapping::Tonemapping>,
        Option<&mut bevy::render::view::Msaa>,
        Option<&mut CellCoord>,
        Option<&mut bevy::light::Skybox>,
        Option<&mut bevy::light::GeneratedEnvironmentMapLight>,
        Option<&bevy::light::EnvironmentMapLight>,
    )>,
    mut commands: Commands,
) {
    let Some(target) = target else { return };
    let mut source_iter = sources.iter();
    let Some((
        source,
        source_transform,
        projection,
        source_exposure,
        source_tonemapping,
        source_msaa,
        parent,
        cell,
        source_skybox,
        source_environment,
    )) = source_iter.next()
    else {
        return;
    };
    if source_iter.next().is_some() {
        warn!("[offscreen] LocalAvatar camera is ambiguous; no image render camera was created");
        return;
    }

    let source_camera_settings = source_cameras.get_mut(source).ok().map(|mut camera| {
        let settings = (
            camera.viewport.clone(),
            camera.msaa_writeback,
            camera.clear_color.clone(),
            camera.invert_culling,
            camera.sub_camera_view.clone(),
        );
        camera.output_mode = bevy::camera::CameraOutputMode::Skip;
        // Keep the authored camera active for the scene's camera-driven LOD and
        // pose systems, but give the non-writing source a distinct priority so
        // Bevy does not report two active cameras for the image target.
        camera.order = -1;
        settings
    });

    let mut found = false;
    for (
        mirror_entity,
        mirror,
        mut mirror_transform,
        mut mirror_projection,
        mut camera,
        mirror_exposure,
        mirror_tonemapping,
        mirror_msaa,
        mut mirror_cell,
        mut mirror_skybox,
        mut mirror_environment,
        mirror_derived_environment,
    ) in &mut mirrors
    {
        if mirror.0 != source {
            camera.is_active = false;
            continue;
        }
        found = true;
        *mirror_transform = source_transform.clone();
        *mirror_projection = projection.clone();
        if let Some((viewport, msaa_writeback, clear_color, invert_culling, sub_camera_view)) =
            &source_camera_settings
        {
            camera.viewport = viewport.clone();
            camera.msaa_writeback = *msaa_writeback;
            camera.clear_color = clear_color.clone();
            camera.invert_culling = *invert_culling;
            camera.sub_camera_view = sub_camera_view.clone();
        }
        if let Some(mut exposure) = mirror_exposure {
            *exposure = *source_exposure;
        } else {
            commands.entity(mirror_entity).try_insert(*source_exposure);
        }
        if let Some(mut tonemapping) = mirror_tonemapping {
            *tonemapping = *source_tonemapping;
        } else {
            commands
                .entity(mirror_entity)
                .try_insert(*source_tonemapping);
        }
        if let Some(mut msaa) = mirror_msaa {
            *msaa = *source_msaa;
        } else {
            commands.entity(mirror_entity).try_insert(*source_msaa);
        }
        if let (Some(source_cell), Some(mirror_cell)) = (cell, mirror_cell.as_deref_mut()) {
            *mirror_cell = *source_cell;
        }
        sync_offscreen_environment(
            &mut commands,
            mirror_entity,
            source_skybox,
            source_environment,
            mirror_skybox.as_deref_mut(),
            mirror_environment.as_deref_mut(),
            mirror_derived_environment,
        );
        camera.is_active = true;
    }
    if !found {
        let mut entity = commands.spawn((
            Camera3d::default(),
            OffscreenRenderCamera(source),
            bevy::camera::RenderTarget::Image(target.0.clone().into()),
            source_transform.clone(),
            projection.clone(),
            *source_exposure,
            *source_tonemapping,
            *source_msaa,
        ));
        if let Some(source_skybox) = source_skybox {
            entity.insert(source_skybox.clone());
        }
        if let Some(source_environment) = source_environment {
            entity.insert(source_environment.clone());
        }
        if let Some((viewport, msaa_writeback, clear_color, invert_culling, sub_camera_view)) =
            &source_camera_settings
        {
            entity.insert(Camera {
                viewport: viewport.clone(),
                msaa_writeback: *msaa_writeback,
                clear_color: clear_color.clone(),
                invert_culling: *invert_culling,
                sub_camera_view: sub_camera_view.clone(),
                ..default()
            });
        }
        if let Some(parent) = parent {
            entity.insert(parent.clone());
        }
        if let Some(cell) = cell {
            entity.insert(*cell);
        }
        info!("[offscreen] created image render camera from authored LocalAvatar {source}");
    }
}

/// Windowed mode always has an active camera — the workbench VIEWPORT camera,
/// which this mode skips along with the rest of the workbench. Every camera a
/// scene brings spawns `is_active: false` by design (see the camera-ambiguity
/// fix), so without this nothing renders and the recording is black frames.
/// Offscreen recording has the same explicit camera contract as the windowed
/// viewport. A path-driven camera, or an already active authored camera, may
/// own the take. If neither is authored/active, all image cameras stay off and
/// the once-per-run diagnostic explains the black recording; the recorder does
/// not invent a primary camera by entity order.
#[cfg(feature = "api-transport")]
fn unique_offscreen_camera(
    candidates: Vec<Entity>,
    role: &str,
    warned: &mut bool,
    ambiguous: &mut bool,
) -> Option<Entity> {
    match candidates.as_slice() {
        [] => None,
        [only] => Some(*only),
        _ => {
            *ambiguous = true;
            if !*warned {
                *warned = true;
                warn!(
                    "[offscreen] {role} is ambiguous ({} authored image cameras); recording stays black until exactly one is selected",
                    candidates.len()
                );
            }
            None
        }
    }
}

#[cfg(feature = "api-transport")]
fn activate_offscreen_camera(
    mut cameras: Query<
        (
            Entity,
            &mut Camera,
            &bevy::camera::RenderTarget,
            bevy::ecs::query::Has<Camera3d>,
            bevy::ecs::query::Has<lunco_render::SceneCamera>,
            Option<&lunco_usd_bevy_scene::UsdPrimPath>,
            bevy::ecs::query::Has<lunco_usd_bevy_camera::camera_path::CameraPathDriven>,
            bevy::ecs::query::Has<lunco_core::LocalAvatar>,
            bevy::ecs::query::Has<bevy::camera::ShadowLodOrigin>,
        ),
        Without<OffscreenRenderCamera>,
    >,
    mirror_sources: Query<&OffscreenRenderCamera>,
    selection: Res<lunco_usd_bevy_camera::camera_switch::ViewportCameraSelection>,
    mut commands: Commands,
    mut warned: Local<bool>,
) {
    // The capture target has one owner. Do not preserve an arbitrary active
    // Camera3d: the authored presentation camera must be the camera the
    // recording path drives.
    // That is the source of the sky-only first frame and the apparent sky/ground
    // flicker in the marketing take.
    //
    // A path-driven camera is the authored presentation owner. It must win over
    // an interactive/avatar camera even when that camera was active while the
    // USD scene was loading: the path writer and capture owner must be the same
    // entity or the take contains alternating views. This uses the existing
    // camera-role component, not an episode-specific camera name.
    let mut ambiguous = false;
    let active_path = unique_offscreen_camera(
        cameras
            .iter()
            .filter(
                |(_, c, target, has_pipeline, has_scene, _, has_path, _, _)| {
                    c.is_active
                        && *has_pipeline
                        && *has_scene
                        && *has_path
                        && matches!(target, bevy::camera::RenderTarget::Image(_))
                },
            )
            .map(|(entity, ..)| entity)
            .collect(),
        "active cinematic camera",
        &mut warned,
        &mut ambiguous,
    );
    let path_driven = unique_offscreen_camera(
        cameras
            .iter()
            .filter(
                |(_, _, target, has_pipeline, has_scene, _, has_path, _, _)| {
                    *has_pipeline
                        && *has_scene
                        && *has_path
                        && matches!(target, bevy::camera::RenderTarget::Image(_))
                },
            )
            .map(|(entity, ..)| entity)
            .collect(),
        "cinematic camera path",
        &mut warned,
        &mut ambiguous,
    );
    // If no cinematic path owns the presentation, preserve an explicit active
    // authored camera. There is no entity-order fallback.
    let active_authored = unique_offscreen_camera(
        cameras
            .iter()
            .filter(
                |(_, c, target, has_pipeline, has_scene, _, has_path, _, _)| {
                    c.is_active
                        && *has_pipeline
                        && *has_scene
                        && !*has_path
                        && matches!(target, bevy::camera::RenderTarget::Image(_))
                },
            )
            .map(|(entity, ..)| entity)
            .collect(),
        "active authored camera",
        &mut warned,
        &mut ambiguous,
    );
    // A camera-track cut is an explicit director selection, but it is not a
    // `CameraPathDriven` curve. Preserve that selected authored camera after
    // the window target is retargeted to the capture image; otherwise the
    // offscreen owner would silently drop a valid mounted or authored track
    // camera between the director and recorder boundaries.
    let requested = unique_offscreen_camera(
        cameras
            .iter()
            .filter(
                |(entity, _, target, has_pipeline, has_scene, path, _, _, _)| {
                    *has_pipeline
                        && *has_scene
                        && selection.matches_requested(*entity, *path)
                        && matches!(target, bevy::camera::RenderTarget::Image(_))
                },
            )
            .map(|(entity, ..)| entity)
            .collect(),
        "explicitly selected authored camera",
        &mut warned,
        &mut ambiguous,
    );
    // A scene without a cinematic track can still author one LocalAvatar camera
    // as its initial presentation. It is an explicit identity marker, not an
    // entity-order fallback, and is shared with the windowed camera contract.
    let local_avatar = unique_offscreen_camera(
        cameras
            .iter()
            .filter(
                |(entity, _, target, has_pipeline, has_scene, _, _, has_avatar, _)| {
                    *has_pipeline
                        && *has_scene
                        && *has_avatar
                        && !mirror_sources.iter().any(|mirror| mirror.0 == *entity)
                        && matches!(target, bevy::camera::RenderTarget::Image(_))
                },
            )
            .map(|(entity, ..)| entity)
            .collect(),
        "authored LocalAvatar camera",
        &mut warned,
        &mut ambiguous,
    );
    let selected = (!ambiguous)
        .then(|| {
            active_path
                .or(requested)
                .or(path_driven)
                .or(active_authored)
                .or(local_avatar)
        })
        .flatten();

    let mut has_image_camera = false;
    for (
        entity,
        mut camera,
        target,
        has_pipeline,
        has_scene,
        _path,
        _has_path,
        _has_avatar,
        has_lod_origin,
    ) in &mut cameras
    {
        let is_image_camera =
            has_pipeline && matches!(target, bevy::camera::RenderTarget::Image(_));
        if !is_image_camera {
            continue;
        }
        has_image_camera = true;

        // A non-SceneCamera image target is never a capture owner. It is
        // explicitly deactivated even when no authored camera exists yet, so
        // an unrelated render camera cannot take over on the next frame. The
        // target-born OffscreenRenderCamera is maintained separately above.
        let keep = has_scene && Some(entity) == selected;
        if camera.is_active != keep {
            camera.is_active = keep;
            if keep {
                info!("[offscreen] selected authored scene camera {entity}");
            } else {
                info!("[offscreen] disabled competing image camera {entity}");
            }
        }
        if keep {
            // The recorder's PNG/readback target is SDR.  An authored HDR camera
            // otherwise keeps a floating-point intermediate while the offscreen
            // target remains Rgba8UnormSrgb; the window compositor performs that
            // conversion, but the image target path does not.
            commands.entity(entity).try_remove::<bevy::camera::Hdr>();
            commands
                .entity(entity)
                .try_remove::<bevy::post_process::bloom::Bloom>();
            if !has_lod_origin {
                commands
                    .entity(entity)
                    .try_insert(bevy::camera::ShadowLodOrigin);
            }
        } else if has_lod_origin {
            commands
                .entity(entity)
                .try_remove::<bevy::camera::ShadowLodOrigin>();
        }
    }

    if selected.is_none() && !*warned && has_image_camera {
        *warned = true;
        warn!(
            "[offscreen] no authored SceneCamera is ready; all competing image cameras are disabled and the recording remains black until a scene camera is bound"
        );
    } else if selected.is_none() && !*warned && !cameras.is_empty() {
        *warned = true;
        warn!(
            "[offscreen] no renderable SceneCamera yet (binding pending or the scene authors none) — the recording stays black until one exists"
        );
    }
}

/// Parse `--record-size WxH`; default 1280x720 — the resolution the windowed
/// luncosim authors for its window (see `default_plugins`), so offscreen
/// recordings match windowed ones by default.
#[cfg(feature = "api-transport")]
fn parse_record_size() -> (u32, u32) {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--record-size" {
            if let Some(spec) = args.get(i + 1) {
                if let Some((w, h)) = spec.split_once('x') {
                    if let (Ok(w), Ok(h)) = (w.trim().parse(), h.trim().parse()) {
                        return (w, h);
                    }
                }
                warn!("--record-size expects WxH (e.g. 1920x1080), got {spec:?} — using 1280x720");
            }
        }
    }
    (1280, 720)
}
