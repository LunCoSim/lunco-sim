//! Route local shadow roots through native preparation and shared shadow passes.

use bevy::{
    core_pipeline::{
        Core3d, Core3dSystems,
        mip_generation::experimental::depth::early_downsample_depth,
        prepass::{DeferredPrepass, DepthPrepass, MotionVectorPrepass, NormalPrepass},
        schedule::RootNonCameraView,
    },
    ecs::{
        schedule::{
            IntoSystemSet, Schedule, ScheduleBuildSettings, ScheduleCleanupPolicy, ScheduleLabel,
            SingleThreadedExecutor, SystemSet, common_conditions::any_match_filter,
        },
        system::ScheduleSystem,
        world::World,
    },
    pbr::{
        EARLY_SHADOW_PASS, GpuMeshPreprocessPlugin, LATE_SHADOW_PASS, LightEntity,
        PreprocessBindGroups, Shadow, ShadowView, SkipGpuPreprocess, ViewLightEntities,
        allocate_uniforms, early_gpu_preprocess, early_prepass_build_indirect_parameters,
        late_gpu_preprocess, late_prepass_build_indirect_parameters,
        main_build_indirect_parameters, per_view_shadow_pass, prepare_lights, shared_shadow_pass,
        unpack_bins,
    },
    prelude::{App, Entity, Has, IntoScheduleConfigs, IntoSystem, Or, Query, Res, With, Without},
    render::{
        Render, RenderApp, RenderSystems,
        batching::gpu_preprocessing::GpuPreprocessingSupport,
        diagnostic::{DiagnosticsRecorder, RecordDiagnostics, RenderDiagnosticsPlugin},
        occlusion_culling::OcclusionCulling,
        render_phase::ViewBinnedRenderPhases,
        renderer::{RenderContext, ViewQuery},
        view::{ExtractedView, NoIndirectDrawing},
    },
};

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct SharedShadowView;

// The native indirect-preparation admission is global across render views.
type IndirectPrepassView = (
    With<PreprocessBindGroups>,
    Without<SkipGpuPreprocess>,
    Without<NoIndirectDrawing>,
    Or<(
        With<DepthPrepass>,
        With<NormalPrepass>,
        With<MotionVectorPrepass>,
        With<DeferredPrepass>,
        With<ShadowView>,
    )>,
);

/// Use the native plugin's configuration and device admission for GPU passes.
pub(super) fn finish(app: &mut App) {
    let gpu_preprocessing_requested = app
        .get_added_plugins::<GpuMeshPreprocessPlugin>()
        .iter()
        .any(|plugin| plugin.use_gpu_instance_buffer_builder);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    let gpu_preprocessing = gpu_preprocessing_requested
        && render_app
            .world()
            .resource::<GpuPreprocessingSupport>()
            .is_available();
    render_app.add_schedule(shared_shadow_schedule(gpu_preprocessing));
    install_diagnostics(app);
}

fn shared_shadow_schedule(gpu_preprocessing: bool) -> Schedule {
    let mut schedule = Schedule::new(SharedShadowView);
    // Every native stage depends on the preceding stage; no work can overlap.
    schedule.set_executor(SingleThreadedExecutor::new());
    schedule.set_build_settings(ScheduleBuildSettings {
        auto_insert_apply_deferred: false,
        ..Default::default()
    });
    schedule.add_systems((
        shared_shadow_pass::<EARLY_SHADOW_PASS>
            .after(early_prepass_build_indirect_parameters)
            .before(late_gpu_preprocess)
            .before(shared_shadow_pass::<LATE_SHADOW_PASS>),
        shared_shadow_pass::<LATE_SHADOW_PASS>.after(late_prepass_build_indirect_parameters),
    ));
    if gpu_preprocessing {
        schedule.add_systems((
            (
                allocate_uniforms,
                unpack_bins,
                early_gpu_preprocess,
                early_prepass_build_indirect_parameters
                    .run_if(any_match_filter::<IndirectPrepassView>),
            )
                .chain()
                .before(shared_shadow_pass::<EARLY_SHADOW_PASS>),
            (
                late_gpu_preprocess,
                late_prepass_build_indirect_parameters
                    .run_if(any_match_filter::<(IndirectPrepassView, With<OcclusionCulling>)>),
            )
                .chain()
                .after(shared_shadow_pass::<EARLY_SHADOW_PASS>)
                .before(shared_shadow_pass::<LATE_SHADOW_PASS>),
        ));
    }
    schedule
}

/// Replace a pass with a pipe containing its native delegate and dependency set.
fn replace_shadow_pass<M, N>(
    world: &mut World,
    schedule: &mut Schedule,
    native: impl IntoSystemSet<M>,
    instrumented: impl IntoScheduleConfigs<ScheduleSystem, N>,
) {
    let native = native.into_system_set().intern();
    let removed = schedule
        .remove_systems_in_set(native, world, ScheduleCleanupPolicy::RemoveSystemsOnly)
        .expect("PBR shadow schedule must be installed before shadow diagnostics");
    assert_eq!(removed, 1, "PBR shadow pass must be installed exactly once");
    schedule.add_systems(instrumented);
}

/// Install after native plugins have built their complete render schedule.
fn install_diagnostics(app: &mut App) {
    if !app.is_plugin_added::<RenderDiagnosticsPlugin>() {
        return;
    }
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };

    render_app
        .world_mut()
        .schedule_scope(Core3d, |world, schedule| {
            // Native system-local edges are removed with the system. Reapply PBR's
            // shadow-pass contract as well as retaining its implicit delegate sets.
            replace_shadow_pass(
                world,
                schedule,
                per_view_shadow_pass::<EARLY_SHADOW_PASS>,
                begin_camera_shadow_span::<EARLY_SHADOW_PASS>
                    .pipe(per_view_shadow_pass::<EARLY_SHADOW_PASS>)
                    .pipe(end_shadow_span::<false, EARLY_SHADOW_PASS>)
                    .after(early_prepass_build_indirect_parameters)
                    .before(early_downsample_depth)
                    .before(per_view_shadow_pass::<LATE_SHADOW_PASS>),
            );
            replace_shadow_pass(
                world,
                schedule,
                per_view_shadow_pass::<LATE_SHADOW_PASS>,
                begin_camera_shadow_span::<LATE_SHADOW_PASS>
                    .pipe(per_view_shadow_pass::<LATE_SHADOW_PASS>)
                    .pipe(end_shadow_span::<false, LATE_SHADOW_PASS>)
                    .after(late_prepass_build_indirect_parameters)
                    .before(main_build_indirect_parameters)
                    .before(Core3dSystems::MainPass),
            );
        });
    for label in [Core3d.intern(), SharedShadowView.intern()] {
        render_app
            .world_mut()
            .schedule_scope(label, |world, schedule| {
                replace_shadow_pass(
                    world,
                    schedule,
                    shared_shadow_pass::<EARLY_SHADOW_PASS>,
                    begin_shared_shadow_span::<EARLY_SHADOW_PASS>
                        .pipe(shared_shadow_pass::<EARLY_SHADOW_PASS>)
                        .pipe(end_shadow_span::<true, EARLY_SHADOW_PASS>)
                        .after(early_prepass_build_indirect_parameters)
                        .before(early_downsample_depth)
                        .before(late_gpu_preprocess)
                        .before(shared_shadow_pass::<LATE_SHADOW_PASS>),
                );
                replace_shadow_pass(
                    world,
                    schedule,
                    shared_shadow_pass::<LATE_SHADOW_PASS>,
                    begin_shared_shadow_span::<LATE_SHADOW_PASS>
                        .pipe(shared_shadow_pass::<LATE_SHADOW_PASS>)
                        .pipe(end_shadow_span::<true, LATE_SHADOW_PASS>)
                        .after(late_prepass_build_indirect_parameters)
                        .before(main_build_indirect_parameters)
                        .before(Core3dSystems::MainPass),
                );
            });
    }
}

/// Validate every native parameter before opening a shared shadow span.
fn begin_shared_shadow_span<const IS_LATE: bool>(
    _world: &World,
    view: ViewQuery<(Entity, &ShadowView, &ExtractedView, Has<OcclusionCulling>)>,
    phases: Res<ViewBinnedRenderPhases<Shadow>>,
    mut ctx: RenderContext,
    recorder: Res<DiagnosticsRecorder>,
) {
    let name = if IS_LATE {
        "lunco_shadow_shared_late"
    } else {
        "lunco_shadow_shared_early"
    };
    recorder.begin_time_span(ctx.command_encoder(), name.into());
    if !IS_LATE {
        let (entity, shadow, view, _) = view.into_inner();
        let phase = phases.get(&view.retained_view_entity);
        let _span = bevy::log::info_span!(
            "lunco_shadow_view_work",
            pass = %shadow.pass_name,
            view = entity.to_bits(),
            width = view.viewport.z,
            height = view.viewport.w,
            phase_empty = ?phase.map(|phase| phase.is_empty()),
            multidraw_sets = ?phase.map(|phase| phase.multidrawable_meshes.len()),
            batchable_bins = ?phase.map(|phase| phase.batchable_meshes.len()),
            unbatchable_bins = ?phase.map(|phase| phase.unbatchable_meshes.len()),
            non_mesh_bins = ?phase.map(|phase| phase.non_mesh_items.len()),
        )
        .entered();
        recorder.begin_time_span(
            ctx.command_encoder(),
            format!("lunco_light_depth/{}", shadow.pass_name).into(),
        );
    }
}

/// Validate every native parameter before opening a camera shadow span.
fn begin_camera_shadow_span<const IS_LATE: bool>(
    _world: &World,
    _view: ViewQuery<&ViewLightEntities>,
    _light_views: Query<(&ShadowView, &ExtractedView, Has<OcclusionCulling>)>,
    _phases: Res<ViewBinnedRenderPhases<Shadow>>,
    mut ctx: RenderContext,
    recorder: Res<DiagnosticsRecorder>,
) {
    let name = if IS_LATE {
        "lunco_shadow_camera_late"
    } else {
        "lunco_shadow_camera_early"
    };
    recorder.begin_time_span(ctx.command_encoder(), name.into());
}

/// A pipe keeps begin/native/end on one thread and queues their buffers in order.
/// Distinct const identities keep each pass's implicit end-system set singular.
fn end_shadow_span<const SHARED: bool, const IS_LATE: bool>(
    mut ctx: RenderContext,
    recorder: Res<DiagnosticsRecorder>,
) {
    if SHARED && !IS_LATE {
        recorder.end_time_span(ctx.command_encoder());
    }
    recorder.end_time_span(ctx.command_encoder());
}

pub(super) fn build(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };

    render_app.add_systems(
        Render,
        route_shared_shadow_views
            .in_set(RenderSystems::PrepareViews)
            .after(prepare_lights),
    );
}

fn route_shared_shadow_views(mut roots: Query<(&mut RootNonCameraView, &LightEntity)>) {
    for (mut root, light) in &mut roots {
        if root.0 == Core3d.intern()
            && matches!(light, LightEntity::Point { .. } | LightEntity::Spot { .. })
        {
            root.0 = SharedShadowView.intern();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Mutex, thread::ThreadId};

    use bevy::{
        ecs::system::System,
        pbr::LightEntity,
        prelude::{
            App, Commands, Component, Entity, IntoScheduleConfigs, IntoSystem, Res, Resource,
            Update, World,
        },
        render::renderer::{CurrentView, ViewQuery},
    };

    use super::{SharedShadowView, replace_shadow_pass, shared_shadow_schedule};

    #[test]
    fn shared_schedule_retains_native_gpu_admission_and_final_buffer_flush() {
        for gpu_preprocessing in [false, true] {
            let mut schedule = shared_shadow_schedule(gpu_preprocessing);
            assert!(!schedule.get_build_settings().auto_insert_apply_deferred);
            schedule.initialize(&mut World::new()).unwrap();
            let systems: Vec<_> = schedule
                .systems()
                .unwrap()
                .map(|(_, system)| system.system_type())
                .collect();
            let allocation = systems.iter().position(|kind| {
                *kind == IntoSystem::into_system(bevy::pbr::allocate_uniforms).system_type()
            });
            let unpacking = systems.iter().position(|kind| {
                *kind == IntoSystem::into_system(bevy::pbr::unpack_bins).system_type()
            });
            let preprocessing = systems.iter().position(|kind| {
                *kind == IntoSystem::into_system(bevy::pbr::early_gpu_preprocess).system_type()
            });
            if gpu_preprocessing {
                let allocation = allocation
                    .expect("native uniform allocation precedes shared-shadow bin unpacking");
                assert!(allocation < unpacking.unwrap());
                assert!(unpacking.unwrap() < preprocessing.unwrap());
            } else {
                assert!(allocation.is_none() && unpacking.is_none() && preprocessing.is_none());
            }
        }
    }

    #[test]
    fn native_local_shadow_roots_use_shared_schedule() {
        use bevy::{
            app::SubApp,
            core_pipeline::{Core3d, schedule::RootNonCameraView},
            ecs::schedule::{Schedule, ScheduleLabel},
            render::{Render, RenderApp},
        };

        let mut app = App::new();
        let mut render_app = SubApp::new();
        render_app.add_schedule(Schedule::new(Render));
        let light = Entity::PLACEHOLDER;
        let point = render_app
            .world_mut()
            .spawn((
                RootNonCameraView(Core3d.intern()),
                LightEntity::Point {
                    light_entity: light,
                    face_index: 0,
                },
            ))
            .id();
        let spot = render_app
            .world_mut()
            .spawn((
                RootNonCameraView(Core3d.intern()),
                LightEntity::Spot {
                    light_entity: light,
                },
            ))
            .id();
        let custom = render_app
            .world_mut()
            .spawn((
                RootNonCameraView(Update.intern()),
                LightEntity::Spot {
                    light_entity: light,
                },
            ))
            .id();
        let other = render_app
            .world_mut()
            .spawn(RootNonCameraView(Core3d.intern()))
            .id();
        let directional = render_app
            .world_mut()
            .spawn((
                RootNonCameraView(Core3d.intern()),
                LightEntity::Directional {
                    light_entity: light,
                    cascade_index: 0,
                },
            ))
            .id();
        app.insert_sub_app(RenderApp, render_app);
        super::build(&mut app);
        let world = app.sub_app_mut(RenderApp).world_mut();
        world.run_schedule(Render);
        for entity in [point, spot] {
            assert_eq!(
                world.get::<RootNonCameraView>(entity).unwrap().0,
                SharedShadowView.intern()
            );
        }
        assert_eq!(
            world.get::<RootNonCameraView>(custom).unwrap().0,
            Update.intern()
        );
        assert_eq!(
            world.get::<RootNonCameraView>(other).unwrap().0,
            Core3d.intern()
        );
        assert_eq!(
            world.get::<RootNonCameraView>(directional).unwrap().0,
            Core3d.intern()
        );

        world
            .entity_mut(point)
            .insert(RootNonCameraView(Core3d.intern()));
        world.run_schedule(Render);
        assert_eq!(
            world.get::<RootNonCameraView>(point).unwrap().0,
            SharedShadowView.intern()
        );
    }

    #[derive(Component)]
    struct TestShadowView;

    #[derive(Resource, Default)]
    struct ShadowRecords(Mutex<Vec<(&'static str, ThreadId)>>);

    fn record(records: &ShadowRecords, name: &'static str) {
        records
            .0
            .lock()
            .unwrap()
            .push((name, std::thread::current().id()));
    }

    fn queue_record(commands: &mut Commands, name: &'static str) {
        commands.queue(move |world: &mut World| record(world.resource::<ShadowRecords>(), name));
    }

    fn native_shadow(
        mut commands: Commands,
        records: Res<ShadowRecords>,
        _view: ViewQuery<&TestShadowView>,
    ) {
        record(&records, "native");
        queue_record(&mut commands, "native_buffer");
    }

    fn begin_shadow(
        mut commands: Commands,
        records: Res<ShadowRecords>,
        _view: ViewQuery<&TestShadowView>,
    ) {
        record(&records, "begin");
        queue_record(&mut commands, "begin_buffer");
    }

    fn end_shadow(mut commands: Commands, records: Res<ShadowRecords>) {
        record(&records, "end");
        queue_record(&mut commands, "end_buffer");
    }

    fn before_shadow(records: Res<ShadowRecords>) {
        record(&records, "before");
    }

    fn after_shadow(records: Res<ShadowRecords>) {
        record(&records, "after");
    }

    #[test]
    fn shadow_instrumentation_preserves_native_dependencies_and_buffer_order() {
        for native_owned_edges in [false, true] {
            for auto_insert_apply_deferred in [false, true] {
                for single_threaded in [false, true] {
                    assert_shadow_schedule(
                        native_owned_edges,
                        auto_insert_apply_deferred,
                        single_threaded,
                    );
                }
            }
        }
    }

    fn assert_shadow_schedule(
        native_owned_edges: bool,
        auto_insert_apply_deferred: bool,
        single_threaded: bool,
    ) {
        use bevy::ecs::schedule::{ScheduleBuildSettings, SingleThreadedExecutor};
        let mut app = App::new();
        app.edit_schedule(Update, |schedule| {
            schedule.set_build_settings(ScheduleBuildSettings {
                auto_insert_apply_deferred,
                ..Default::default()
            });
            if single_threaded {
                schedule.set_executor(SingleThreadedExecutor::new());
            }
        });
        app.init_resource::<ShadowRecords>();
        let shadow = app.world_mut().spawn(TestShadowView).id();
        let other = app.world_mut().spawn_empty().id();
        app.insert_resource(CurrentView(shadow));
        if native_owned_edges {
            app.add_systems(
                Update,
                (
                    native_shadow.after(before_shadow).before(after_shadow),
                    before_shadow,
                    after_shadow,
                ),
            );
        } else {
            app.add_systems(
                Update,
                (
                    native_shadow,
                    before_shadow.before(native_shadow),
                    after_shadow.after(native_shadow),
                ),
            );
        }
        app.world_mut().schedule_scope(Update, |world, schedule| {
            let pipe = begin_shadow.pipe(native_shadow).pipe(end_shadow);
            if native_owned_edges {
                replace_shadow_pass(
                    world,
                    schedule,
                    native_shadow,
                    pipe.after(before_shadow).before(after_shadow),
                );
            } else {
                replace_shadow_pass(world, schedule, native_shadow, pipe);
            }
        });
        app.update();

        let records = app.world().resource::<ShadowRecords>().0.lock().unwrap();
        let expected = if auto_insert_apply_deferred {
            [
                "before",
                "begin",
                "native",
                "end",
                "begin_buffer",
                "native_buffer",
                "end_buffer",
                "after",
            ]
        } else {
            [
                "before",
                "begin",
                "native",
                "end",
                "after",
                "begin_buffer",
                "native_buffer",
                "end_buffer",
            ]
        };
        assert_eq!(
            records.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(records[1].1, records[2].1);
        assert_eq!(records[2].1, records[3].1);
        drop(records);

        app.world()
            .resource::<ShadowRecords>()
            .0
            .lock()
            .unwrap()
            .clear();
        app.insert_resource(CurrentView(other));
        app.update();
        let records = app.world().resource::<ShadowRecords>().0.lock().unwrap();
        assert_eq!(
            records.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            ["before", "after"]
        );
    }
}
