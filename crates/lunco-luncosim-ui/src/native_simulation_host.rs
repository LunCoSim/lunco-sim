//! Native presentation handoff and readiness-driven service of coupled ticks.

use bevy::prelude::*;
use bevy::render::pipelined_rendering::{RenderAppChannels, RenderExtractApp};
use std::future::{Future, poll_fn};
use std::task::Poll;

pub(crate) fn install(app: &mut App) {
    app.add_plugins(NativeSimulationHostPlugin);
}

struct NativeSimulationHostPlugin;

// The render world stays on the main thread between receipt and extraction.
// Keeping the handoff before clock sampling leaves every presentation producer
// between the final physical tick and the next native extraction.
struct ReadyRenderFrame(Option<bevy::app::SubApp>);

enum HostReady {
    Frame(Option<bevy::app::SubApp>),
    Completion,
    Fault(&'static str),
}

impl Plugin for NativeSimulationHostPlugin {
    fn build(&self, _app: &mut App) {}

    fn cleanup(&self, app: &mut App) {
        let proxy = (**app.world().resource::<bevy::winit::EventLoopProxyWrapper>()).clone();
        if let Err(error) = app
            .world_mut()
            .resource_mut::<lunco_core_runtime::SimulationCompletionSources>()
            .install_waker(std::sync::Arc::new(move || {
                let _ = proxy.send_event(bevy::winit::WinitUserEvent::WakeUp);
            }))
        {
            host_fault(
                app.world_mut(),
                "simulation-completion-waker-failed",
                error.to_string(),
            );
            return;
        }
        if app.get_sub_app(RenderExtractApp).is_none() {
            return;
        }
        app.world_mut().insert_non_send(ReadyRenderFrame(None));
        app.add_systems(
            First,
            receive_render_frame.before(lunco_core_runtime::SimulationCompletionSet),
        );
        app.sub_app_mut(RenderExtractApp)
            .set_extract(extract_ready_frame);
    }
}

fn host_fault(world: &mut World, code: &'static str, detail: String) {
    world.resource_mut::<lunco_core::RuntimeFaults>().raise(
        code,
        None,
        "native simulation host",
        detail,
    );
    world.write_message(AppExit::error());
}

fn receive_render_frame(world: &mut World) {
    let executor = world
        .resource::<bevy::ecs::schedule::MainThreadExecutor>()
        .clone();
    let ready = world.resource_scope(|world, mut channels: Mut<RenderAppChannels>| {
        let mut receipt = std::pin::pin!(channels.recv());
        loop {
            let ready = bevy::tasks::ComputeTaskPool::get()
                // Keep this latency-sensitive wait on its scope and render
                // callbacks; unrelated compute jobs stay on the pool workers.
                .scope_with_executor(false, Some(&*executor.0), |scope| {
                    scope.spawn_on_scope(poll_fn(|cx| {
                        // Rendering readiness always wins. A solver can never extend
                        // this existing render handoff by making the host wait for it.
                        if let Poll::Ready(frame) = receipt.as_mut().poll(cx) {
                            return Poll::Ready(HostReady::Frame(frame));
                        }
                        match world
                            .resource::<lunco_core_runtime::SimulationCompletionSources>()
                            .poll_wake(cx)
                        {
                            Ok(Poll::Ready(())) => return Poll::Ready(HostReady::Completion),
                            Ok(Poll::Pending) => {}
                            Err(error) => return Poll::Ready(HostReady::Fault(error)),
                        }
                        Poll::Pending
                    }));
                })
                .pop();
            match ready {
                Some(HostReady::Frame(frame)) => break frame,
                Some(HostReady::Completion) => {
                    // Release the host task scope before entering ECS schedules;
                    // their own main-thread executor must remain available.
                    lunco_time::service_pending_simulation(world);
                }
                Some(HostReady::Fault(error)) => {
                    host_fault(world, "simulation-completion-waker-missing", error.into());
                    break None;
                }
                None => break None,
            }
        }
    });
    match ready {
        Some(frame) => world.non_send_mut::<ReadyRenderFrame>().0 = Some(frame),
        None => host_fault(
            world,
            "native-render-handoff-failed",
            "render thread closed before returning its frame".into(),
        ),
    }
}

fn extract_ready_frame(world: &mut World, _extract: &mut World) {
    let Some(mut frame) = world.non_send_mut::<ReadyRenderFrame>().0.take() else {
        host_fault(
            world,
            "native-render-frame-missing",
            "presentation extraction has no received render frame".into(),
        );
        return;
    };
    frame.extract(world);
    world
        .resource_mut::<RenderAppChannels>()
        .send_blocking(frame);
}
