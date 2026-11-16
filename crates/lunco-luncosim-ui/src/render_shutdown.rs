//! Keep the native window and render resources alive until admitted pipeline
//! compilation finishes. Closing is a one-way admission boundary: no new scene
//! extraction or render work starts after an exit request.

use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Resource, Default, Clone)]
struct Shutdown(Arc<ShutdownState>);

#[derive(Default)]
struct ShutdownState {
    requested: AtomicBool,
    idle: AtomicBool,
}

#[derive(Resource, Default)]
struct PendingExit(Option<AppExit>);

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct FinishFrame;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct RenderOrDrain;

pub(super) fn install(app: &mut App) {
    let shutdown = Shutdown::default();
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    let original_schedule = render
        .update_schedule
        .expect("render update schedule is installed");
    let mut extract = render.take_extract();
    let extraction_shutdown = shutdown.clone();
    render.set_extract(move |main, render| {
        if !extraction_shutdown.0.requested.load(Ordering::Acquire) {
            if let Some(extract) = extract.as_mut() {
                extract(main, render);
            }
        }
    });
    render.insert_resource(shutdown.clone());
    render.add_systems(RenderOrDrain, move |world: &mut World| {
        let shutdown = world.resource::<Shutdown>().clone();
        if !shutdown.0.requested.load(Ordering::Acquire) {
            world.run_schedule(original_schedule);
        } else {
            let idle = world.get_resource::<PipelineCache>().is_none_or(|cache| {
                cache.pipelines().all(|pipeline| match &pipeline.state {
                    CachedPipelineState::Creating(task) => task.is_finished(),
                    _ => true,
                })
            });
            shutdown.0.idle.store(idle, Ordering::Release);
        }
    });
    render.update_schedule = Some(RenderOrDrain.intern());
    app.insert_resource(shutdown)
        .init_resource::<PendingExit>()
        .add_systems(FinishFrame, hold_exit);
    app.world_mut()
        .resource_mut::<MainScheduleOrder>()
        .insert_after(Last, FinishFrame);
}

fn hold_exit(world: &mut World) {
    let exits: Vec<_> = world.resource_mut::<Messages<AppExit>>().drain().collect();
    if !exits.is_empty() {
        let mut pending = world.resource_mut::<PendingExit>();
        for exit in exits {
            if pending.0.as_ref().is_none_or(|current| !current.is_error()) {
                pending.0 = Some(exit);
            }
        }
        let shutdown = world.resource::<Shutdown>();
        if !shutdown.0.requested.swap(true, Ordering::AcqRel) {
            // Native runners can wait for a presented frame before admitting
            // another visible-window update. Closing hides the windows while
            // retaining their handles, so draining needs no further presentation.
            for mut window in world.query::<&mut Window>().iter_mut(world) {
                window.visible = false;
            }
            world.trigger(lunco_time::SetTimeTransport {
                playing: Some(false),
                rate: None,
            });
        }
    }
    if world.resource::<Shutdown>().0.idle.load(Ordering::Acquire) {
        if let Some(exit) = world.resource_mut::<PendingExit>().0.take() {
            world.write_message(exit);
        }
    }
    if world.resource::<PendingExit>().0.is_some() {
        // Pausing can leave the native runner asleep without another input or
        // presented frame. Keep the shutdown boundary scheduled until idle.
        if let Some(proxy) = world.get_resource::<bevy::winit::EventLoopProxyWrapper>() {
            let _ = proxy.send_event(bevy::winit::WinitUserEvent::WakeUp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_waits_for_render_idle_and_preserves_failure() {
        let mut world = World::new();
        world.init_resource::<Messages<AppExit>>();
        world.init_resource::<Shutdown>();
        world.init_resource::<PendingExit>();
        let window = world.spawn(Window::default()).id();
        world.write_message(AppExit::error());
        hold_exit(&mut world);
        assert!(world.resource::<Messages<AppExit>>().is_empty());
        assert!(!world.get::<Window>(window).unwrap().visible);
        assert!(
            world
                .resource::<Shutdown>()
                .0
                .requested
                .load(Ordering::Acquire)
        );
        world.write_message(AppExit::Success);
        hold_exit(&mut world);
        assert!(world.resource::<Messages<AppExit>>().is_empty());
        world
            .resource::<Shutdown>()
            .0
            .idle
            .store(true, Ordering::Release);
        hold_exit(&mut world);
        let exits: Vec<_> = world.resource_mut::<Messages<AppExit>>().drain().collect();
        assert_eq!(exits.len(), 1);
        assert!(exits[0].is_error());
    }
}
