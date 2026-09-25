//! Skip camera-only Core3d stages for auxiliary light-shadow roots.

use bevy::{
    core_pipeline::{Core3d, Core3dSystems},
    pbr::LightEntity,
    prelude::{App, IntoScheduleConfigs, Query, Res, With},
    render::{RenderApp, renderer::CurrentView},
};

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
    use bevy::{
        pbr::LightEntity,
        prelude::{App, Entity, IntoScheduleConfigs, ResMut, Resource, Update},
        render::renderer::CurrentView,
    };

    use super::current_root_uses_camera_pipeline;

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
