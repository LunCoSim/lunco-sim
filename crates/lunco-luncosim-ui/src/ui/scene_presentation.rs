//! Cover the main view while its initial simulation state is prepared.

use bevy::prelude::*;
use lunco_core::{RuntimeFaults, SceneMountState, SceneTransitionCoordinator};
use lunco_core_runtime::SimulationProgress;
use lunco_readiness::{ReadinessRegistry, kinds};
use lunco_viewport_core::{SceneViewport, SceneViewportSet};

pub(super) fn install(app: &mut App) {
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
    let waiting_for_initial_state = viewport.loading
        && mounts.active_root().is_some()
        && (progress.is_held()
            || readiness
                .pending()
                .any(|item| item.kind != kinds::PROGRAM_FAILED));
    let loading = !faults.active() && (transitioning || waiting_for_initial_state);
    if viewport.loading != loading {
        viewport.loading = loading;
        debug!(
            loading,
            "[scene-presentation] initial scene loading changed"
        );
    }
}
