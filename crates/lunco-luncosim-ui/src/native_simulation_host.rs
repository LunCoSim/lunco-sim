//! Native presentation handoff and readiness-driven service of coupled ticks.

use bevy::prelude::*;
use bevy::render::pipelined_rendering::{RenderAppChannels, RenderExtractApp};
use std::future::{Future, poll_fn};
use std::task::{Context, Poll, Waker};

pub(crate) fn install(app: &mut App) {
    app.add_plugins(NativeSimulationHostPlugin);
}

struct NativeSimulationHostPlugin;

#[derive(Default)]
enum RenderFrameHandoff {
    #[default]
    Awaiting,
    Ready(bevy::app::SubApp),
    Failed,
}

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
        app.world_mut()
            .insert_non_send(RenderFrameHandoff::default());
        // The host samples its monotonic clock independently of render completion.
        // Bevy's optional render timestamp channel is retired at this boundary;
        // manual recording/test time strategies still use their selected source.
        app.world_mut()
            .remove_resource::<bevy::time::TimeReceiver>();
        app.add_systems(
            RunFixedMainLoop,
            service_simulation_during_render.in_set(lunco_core::RuntimeCycleSet::Presentation),
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

fn service_simulation_during_render(world: &mut World) {
    // Outstanding fixed demand can use ready completions while rendering.
    // Otherwise let all presentation systems overlap the outgoing render.
    if lunco_time::has_queued_fixed_tick(world) {
        let handoff = wait_for_render_frame(world, true)
            .map_or(RenderFrameHandoff::Failed, RenderFrameHandoff::Ready);
        *world.non_send_mut::<RenderFrameHandoff>() = handoff;
    }
}

fn wait_for_render_frame(world: &mut World, service_simulation: bool) -> Option<bevy::app::SubApp> {
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
                        if service_simulation {
                            match world
                                .resource::<lunco_core_runtime::SimulationCompletionSources>()
                                .poll_wake(cx)
                            {
                                Ok(Poll::Ready(())) => return Poll::Ready(HostReady::Completion),
                                Ok(Poll::Pending) => {}
                                Err(error) => return Poll::Ready(HostReady::Fault(error)),
                            }
                        }
                        Poll::Pending
                    }));
                })
                .pop();
            match ready {
                Some(HostReady::Frame(frame)) => break Ok(frame),
                Some(HostReady::Completion) => {
                    // Release the host task scope before entering ECS schedules;
                    // their own main-thread executor must remain available.
                    let mut received = None;
                    lunco_time::service_pending_simulation(world, || {
                        // Recheck at complete-cycle boundaries. The blocking
                        // scope installs its own waker if the frame is pending.
                        let mut context = Context::from_waker(Waker::noop());
                        match receipt.as_mut().poll(&mut context) {
                            Poll::Ready(frame) => {
                                received = Some(frame);
                                false
                            }
                            Poll::Pending => true,
                        }
                    });
                    if let Some(frame) = received {
                        break Ok(frame);
                    }
                }
                Some(HostReady::Fault(error)) => break Err(error),
                None => break Ok(None),
            }
        }
    });
    match ready {
        Ok(Some(frame)) => return Some(frame),
        Ok(None) => host_fault(
            world,
            "native-render-handoff-failed",
            "render thread closed before returning its frame".into(),
        ),
        Err(error) => host_fault(world, "simulation-completion-waker-missing", error.into()),
    }
    None
}

fn extract_ready_frame(world: &mut World, _extract: &mut World) {
    let handoff = std::mem::take(&mut *world.non_send_mut::<RenderFrameHandoff>());
    let mut frame = match handoff {
        RenderFrameHandoff::Awaiting => {
            // The main world has already sampled presentation for this update.
            // Receive the render world here without admitting further cycles.
            let Some(frame) = wait_for_render_frame(world, false) else {
                return;
            };
            frame
        }
        RenderFrameHandoff::Ready(frame) => frame,
        RenderFrameHandoff::Failed => return,
    };
    frame.extract(world);
    world
        .resource_mut::<RenderAppChannels>()
        .send_blocking(frame);
}
