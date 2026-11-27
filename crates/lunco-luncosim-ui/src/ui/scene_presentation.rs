//! Cover the main view while its initial simulation state is prepared.

use bevy::platform::time::Instant;
use bevy::prelude::*;
use lunco_core::{RuntimeFaults, SceneMountState, SceneTransitionCoordinator};
use lunco_core_runtime::SimulationProgress;
use lunco_readiness::{ReadinessRegistry, kinds};
use lunco_viewport_core::{SceneViewport, SceneViewportSet};

pub(super) fn install(app: &mut App) {
    app.add_observer(
        |_event: On<lunco_core::SceneTransitionStarted>, mut viewport: ResMut<SceneViewport>| {
            viewport.loading_since = Some(Instant::now());
        },
    );
    app.add_systems(
        PostUpdate,
        publish_scene_loading
            .before(SceneViewportSet::Reconcile)
            .before(bevy_egui::EguiPostUpdateSet::EndPass),
    );
}

fn publish_scene_loading(
    transition: Res<SceneTransitionCoordinator>,
    mounts: Res<SceneMountState>,
    progress: Res<SimulationProgress>,
    readiness: Res<ReadinessRegistry>,
    faults: Res<RuntimeFaults>,
    mut viewport: ResMut<SceneViewport>,
) {
    let transitioning = transition.active().is_some() || transition.has_admitted();
    // Keep the workbench backdrop through the preparation of the new mount. Once it
    // opens, later edits and per-participant waits leave it available.
    let waiting_for_initial_state = viewport.is_loading()
        && mounts.active_root().is_some()
        && (progress.is_held()
            || readiness
                .pending()
                .any(|item| item.kind != kinds::PROGRAM_FAILED));
    let loading = !faults.active() && (transitioning || waiting_for_initial_state);
    if viewport.is_loading() != loading {
        viewport.loading_since = loading.then(Instant::now);
        debug!(
            loading,
            "[scene-presentation] initial scene loading changed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::{SceneTransition, SceneTransitionStarted};
    use lunco_core_runtime::SimulationProgressKey;

    #[test]
    fn loading_start_survives_preparation_and_retires_with_readiness_or_fault() {
        let mut app = App::new();
        app.init_resource::<SceneTransitionCoordinator>()
            .init_resource::<SceneMountState>()
            .init_resource::<SimulationProgress>()
            .init_resource::<ReadinessRegistry>()
            .init_resource::<RuntimeFaults>()
            .init_resource::<SceneViewport>();
        install(&mut app);

        let transition = SceneTransition::load("scene.usda", "");
        let id = app
            .world_mut()
            .resource_mut::<SceneTransitionCoordinator>()
            .start(transition.clone());
        app.world_mut()
            .trigger(SceneTransitionStarted { id, transition });
        let started = app
            .world()
            .resource::<SceneViewport>()
            .loading_since
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<SceneViewport>().loading_since,
            Some(started)
        );

        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SceneMountState>()
            .register_root(root, true);
        let key = SimulationProgressKey::scene_datasets();
        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .acquire(key, "Preparing data");
        assert!(
            app.world_mut()
                .resource_mut::<SceneTransitionCoordinator>()
                .complete(id)
        );
        app.update();
        assert_eq!(
            app.world().resource::<SceneViewport>().loading_since,
            Some(started)
        );
        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .update_reason(key, "Preparing models");
        app.update();
        assert_eq!(
            app.world().resource::<SceneViewport>().loading_since,
            Some(started)
        );

        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .release(key);
        app.update();
        assert!(!app.world().resource::<SceneViewport>().is_loading());

        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .acquire(key, "Later edit");
        app.update();
        assert!(!app.world().resource::<SceneViewport>().is_loading());
        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .release(key);

        let transition = SceneTransition::Clear;
        let id = app
            .world_mut()
            .resource_mut::<SceneTransitionCoordinator>()
            .start(transition.clone());
        app.world_mut()
            .trigger(SceneTransitionStarted { id, transition });
        assert!(app.world().resource::<SceneViewport>().is_loading());
        app.world_mut()
            .resource_mut::<RuntimeFaults>()
            .raise("test-fault", None, "test", "failed");
        app.update();
        assert!(!app.world().resource::<SceneViewport>().is_loading());
    }
}
