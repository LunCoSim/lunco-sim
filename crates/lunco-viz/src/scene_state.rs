//! Scene lifetime for visualization data accessed without an ECS World.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use bevy::prelude::*;
use bevy_egui::EguiContext;
use lunco_core::{AppSceneExt, SceneResource};

#[derive(Clone, Default)]
struct SceneUiData(Arc<Mutex<egui::util::IdTypeMap>>);

fn storage_id() -> egui::Id {
    egui::Id::new("lunco_viz_scene_data")
}

/// All scene-derived widget snapshots, tasks and queued actions use this store.
/// Layout preferences keep their normal application lifetime.
pub(crate) fn scene_data<R>(
    ctx: &egui::Context,
    access: impl FnOnce(&mut egui::util::IdTypeMap) -> R,
) -> R {
    let storage = ctx.data_mut(|data| {
        data.get_temp::<SceneUiData>(storage_id())
            .unwrap_or_else(|| {
                let storage = SceneUiData::default();
                data.insert_temp(storage_id(), storage.clone());
                storage
            })
    });
    let mut data = storage
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    access(&mut data)
}

/// Track context lifetimes independently of shutdown-system ordering. Removed
/// contexts stay available until final reset drains terminal callback writes.
#[derive(Resource, Default)]
struct SceneUiRetirement {
    contexts: HashMap<Entity, egui::Context>,
    retiring: Vec<egui::Context>,
}

fn clear_context(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.remove::<SceneUiData>(storage_id());
        data.remove_by_type::<egui_plot::PlotMemory>();
    });
    if egui::DragAndDrop::has_payload_of_type::<crate::ChannelDragPayload>(ctx) {
        egui::DragAndDrop::clear_payload(ctx);
    }
}

impl SceneResource for SceneUiRetirement {
    fn clear_scene(&mut self) {
        let retired = std::mem::take(self);
        for ctx in retired.contexts.values().chain(&retired.retiring) {
            clear_context(ctx);
        }
        // Active UI contexts are application-owned; their data store is not.
        self.contexts = retired.contexts;
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_scene_resource::<SceneUiRetirement>()
        .add_observer(
            |event: On<Insert<EguiContext>>,
             mut contexts: Query<&mut EguiContext>,
             mut retirement: ResMut<SceneUiRetirement>| {
                if let Ok(mut ctx) = contexts.get_mut(event.entity) {
                    if let Some(previous) = retirement
                        .contexts
                        .insert(event.entity, ctx.get_mut().clone())
                    {
                        clear_context(&previous);
                        retirement.retiring.push(previous);
                    }
                }
            },
        )
        .add_observer(
            |event: On<Remove<EguiContext>>, mut retirement: ResMut<SceneUiRetirement>| {
                if let Some(ctx) = retirement.contexts.remove(&event.entity) {
                    clear_context(&ctx);
                    retirement.retiring.push(ctx);
                }
            },
        );
    let world = app.world_mut();
    let contexts: HashMap<_, _> = world
        .query::<(Entity, &mut EguiContext)>()
        .iter_mut(world)
        .map(|(entity, mut ctx)| (entity, ctx.get_mut().clone()))
        .collect();
    world
        .resource_mut::<SceneUiRetirement>()
        .contexts
        .extend(contexts);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::canvas_plot_node::{drain_input_writes, queue_input_write};
    use crate::telemetry_browser::{
        ChannelDragPayload, PlotDropRequest, drain_plot_drops, queue_plot_drop,
    };
    use lunco_core::SceneTeardown;

    #[test]
    fn scene_lifecycle_retires_hidden_widget_data_and_pending_actions() {
        let mut app = App::new();
        let context_entity = app.world_mut().spawn(EguiContext::default()).id();
        install(&mut app);
        let ctx = app
            .world_mut()
            .get_mut::<EguiContext>(context_entity)
            .unwrap()
            .get_mut()
            .clone();
        let app_key = egui::Id::new("application preference");
        ctx.data_mut(|data| data.insert_temp(app_key, 37_u32));
        // Terminal callbacks can still publish while shutdown is running.
        let terminal_ctx = ctx.clone();
        app.add_systems(SceneTeardown, move || {
            queue_input_write(&terminal_ctx, "old.input", 9.0)
        });

        for _ in 0..3 {
            let retiring_entity = app
                .world_mut()
                .spawn((EguiContext::default(), lunco_core::SceneOwned))
                .id();
            let retiring_ctx = app
                .world_mut()
                .get_mut::<EguiContext>(retiring_entity)
                .unwrap()
                .get_mut()
                .clone();
            queue_input_write(&retiring_ctx, "retired.context.input", 5.0);
            app.world_mut()
                .entity_mut(retiring_entity)
                .insert(EguiContext::default());
            let replacement_ctx = app
                .world_mut()
                .get_mut::<EguiContext>(retiring_entity)
                .unwrap()
                .get_mut()
                .clone();
            queue_input_write(&replacement_ctx, "replacement.context.input", 6.0);
            let buffer = Arc::new(vec![37.0_f64]);
            let weak = Arc::downgrade(&buffer);
            scene_data(&ctx, |data| {
                data.insert_temp(egui::Id::new("future cache type"), buffer)
            });
            queue_plot_drop(
                &ctx,
                PlotDropRequest {
                    payload: ChannelDragPayload {
                        entity_bits: 1,
                        path: Arc::from("old.signal"),
                    },
                    world_pos: None,
                },
            );
            egui::DragAndDrop::set_payload(
                &ctx,
                ChannelDragPayload {
                    entity_bits: 1,
                    path: Arc::from("old.signal"),
                },
            );
            let plot_id = egui::Id::new("scene plot interaction");
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui_plot::Plot::new("scene plot")
                    .id(plot_id)
                    .show(ui, |_| {});
            });
            output.textures_delta.clear();
            assert!(egui_plot::PlotMemory::load(&ctx, plot_id).is_some());
            lunco_core::run_scene_teardown(app.world_mut());
            assert!(app.world().get_entity(retiring_entity).is_err());
            assert!(
                drain_input_writes(&retiring_ctx).is_empty(),
                "removed contexts must retire too"
            );
            assert!(
                drain_input_writes(&replacement_ctx).is_empty(),
                "replaced contexts must retire too"
            );
            assert!(
                weak.upgrade().is_none(),
                "hidden buffers must be dropped without repainting"
            );
            assert!(drain_plot_drops(&ctx).is_empty());
            assert!(
                drain_input_writes(&ctx).is_empty(),
                "terminal writes must not reach the next scene"
            );
            assert!(!egui::DragAndDrop::has_payload_of_type::<ChannelDragPayload>(&ctx));
            assert!(
                egui_plot::PlotMemory::load(&ctx, plot_id).is_none(),
                "old plot bounds must not hide a restarted clock"
            );
            assert_eq!(ctx.data(|data| data.get_temp::<u32>(app_key)), Some(37));
        }
    }
}
