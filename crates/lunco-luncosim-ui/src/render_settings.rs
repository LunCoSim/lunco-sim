//! Active Twin projection for renderer preferences. Manifest settings own the
//! override; the rendering owner resolves it over persisted user preferences.

use bevy::prelude::*;
use lunco_render::{RenderingBackend, TwinRendererOverride};
use lunco_workspace::{TwinSettingValue, WorkspaceResource};

pub(crate) fn register(app: &mut App) {
    app.init_resource::<TwinRendererOverride>()
        .add_systems(PreUpdate, project_active_twin_renderer);
}

fn project_active_twin_renderer(
    workspace: Res<WorkspaceResource>,
    mut projected: ResMut<TwinRendererOverride>,
) {
    if !workspace.is_changed() {
        return;
    }
    let mut next = TwinRendererOverride::default();
    let manifest = workspace
        .active_twin
        .and_then(|id| workspace.twin(id))
        .and_then(|twin| twin.manifest.as_ref());
    if let Some(manifest) = manifest {
        match manifest.setting("render.backend") {
            None => {}
            Some(TwinSettingValue::Text(value)) => match RenderingBackend::parse(value) {
                Ok(value) => next.backend = Some(value),
                Err(error) => next.error = Some(error),
            },
            Some(_) => next.error = Some("Twin render.backend must be text".into()),
        }
        match manifest.setting("render.samples_per_pixel") {
            None => {}
            Some(TwinSettingValue::Integer(value)) => match u32::try_from(*value) {
                Ok(value) if value > 0 => next.samples_per_pixel = Some(value),
                _ => {
                    next.error =
                        Some("Twin render.samples_per_pixel must be a positive u32 integer".into())
                }
            },
            Some(_) => next.error = Some("Twin render.samples_per_pixel must be an integer".into()),
        }
    }
    projected.set_if_neq(next);
}
