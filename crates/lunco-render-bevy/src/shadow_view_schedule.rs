//! Admit shadow-root stages and instrument native shadow passes when profiling.

use bevy::{
    core_pipeline::{
        Core3d, Core3dSystems, mip_generation::experimental::depth::early_downsample_depth,
    },
    ecs::{
        schedule::{IntoSystemSet, Schedule, ScheduleCleanupPolicy, SystemSet},
        system::ScheduleSystem,
        world::World,
    },
    pbr::{
        EARLY_SHADOW_PASS, LATE_SHADOW_PASS, LightEntity, Shadow, ShadowView, ViewLightEntities,
        early_prepass_build_indirect_parameters, late_prepass_build_indirect_parameters,
        main_build_indirect_parameters, per_view_shadow_pass, shared_shadow_pass,
    },
    prelude::{App, Entity, Has, IntoScheduleConfigs, IntoSystem, Query, Res, With},
    render::{
        RenderApp,
        diagnostic::{DiagnosticsRecorder, RecordDiagnostics, RenderDiagnosticsPlugin},
        occlusion_culling::OcclusionCulling,
        render_phase::ViewBinnedRenderPhases,
        renderer::{CurrentView, RenderContext, ViewQuery},
        view::ExtractedView,
    },
};

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
pub(super) fn install_diagnostics(app: &mut App) {
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
                shared_shadow_pass::<EARLY_SHADOW_PASS>,
                begin_shared_shadow_span::<EARLY_SHADOW_PASS>
                    .pipe(shared_shadow_pass::<EARLY_SHADOW_PASS>)
                    .pipe(end_shadow_span::<true, EARLY_SHADOW_PASS>)
                    .after(early_prepass_build_indirect_parameters)
                    .before(early_downsample_depth)
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
}

/// Validate every native parameter before opening a shared shadow span.
fn begin_shared_shadow_span<const IS_LATE: bool>(
    _world: &World,
    _view: ViewQuery<(Entity, &ShadowView, &ExtractedView, Has<OcclusionCulling>)>,
    _phases: Res<ViewBinnedRenderPhases<Shadow>>,
    mut ctx: RenderContext,
    recorder: Res<DiagnosticsRecorder>,
) {
    let name = if IS_LATE {
        "lunco_shadow_shared_late"
    } else {
        "lunco_shadow_shared_early"
    };
    recorder.begin_time_span(ctx.command_encoder(), name.into());
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
    recorder.end_time_span(ctx.command_encoder());
}

pub(super) fn build(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };

    render_app.configure_sets(
        Core3d,
        (
            Core3dSystems::Prepass,
            Core3dSystems::MainPass,
            Core3dSystems::EarlyPostProcess,
            Core3dSystems::PostProcess,
        )
            .run_if(current_root_uses_camera_pipeline),
    );
}

fn current_root_uses_camera_pipeline(
    current_view: Res<CurrentView>,
    shadow_views: Query<(), With<LightEntity>>,
) -> bool {
    shadow_views.get(current_view.0).is_err()
}

#[cfg(test)]
mod tests {
    use std::{sync::Mutex, thread::ThreadId};

    use bevy::{
        pbr::LightEntity,
        prelude::{
            App, Commands, Component, Entity, IntoScheduleConfigs, IntoSystem, Res, ResMut,
            Resource, Update, World,
        },
        render::renderer::{CurrentView, ViewQuery},
    };

    use super::{current_root_uses_camera_pipeline, replace_shadow_pass};

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
        assert_shadow_schedule(false);
        assert_shadow_schedule(true);
    }

    fn assert_shadow_schedule(native_owned_edges: bool) {
        let mut app = App::new();
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
        assert_eq!(
            records.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            [
                "before",
                "begin",
                "native",
                "end",
                "begin_buffer",
                "native_buffer",
                "end_buffer",
                "after"
            ]
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

    #[derive(Resource, Default)]
    struct CameraPipelineRuns(usize);

    fn count_camera_pipeline(mut runs: ResMut<CameraPipelineRuns>) {
        runs.0 += 1;
    }

    #[test]
    fn camera_stages_are_skipped_only_for_light_shadow_roots() {
        let mut app = App::new();
        app.init_resource::<CameraPipelineRuns>();

        let camera = app.world_mut().spawn_empty().id();
        let other_root = app.world_mut().spawn_empty().id();
        let shadow_root = app
            .world_mut()
            .spawn(LightEntity::Spot {
                light_entity: Entity::PLACEHOLDER,
            })
            .id();
        app.insert_resource(CurrentView(camera));
        app.add_systems(
            Update,
            count_camera_pipeline.run_if(current_root_uses_camera_pipeline),
        );

        app.update();
        assert_eq!(app.world().resource::<CameraPipelineRuns>().0, 1);

        app.insert_resource(CurrentView(shadow_root));
        app.update();
        assert_eq!(app.world().resource::<CameraPipelineRuns>().0, 1);

        app.insert_resource(CurrentView(other_root));
        app.update();
        assert_eq!(app.world().resource::<CameraPipelineRuns>().0, 2);
    }
}
