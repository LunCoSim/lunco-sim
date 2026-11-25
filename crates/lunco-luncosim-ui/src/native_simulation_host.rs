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
    if lunco_time::can_service_pending_simulation(world) {
        let handoff = wait_for_render_frame(world, true);
        // A returned render world closes the overlap opportunity, not the
        // remaining ordinary application service slice. Consume any ready
        // causal work before sampling presentation, but never wait on a
        // participant once the renderer has returned.
        if matches!(handoff, RenderFrameHandoff::Ready(_)) {
            if let Some(deadline) = lunco_time::fixed_service_deadline(world) {
                lunco_time::service_pending_simulation(world, || {
                    std::time::Instant::now() < deadline
                });
            }
        }
        *world.non_send_mut::<RenderFrameHandoff>() = handoff;
    }
}

fn wait_for_render_frame(world: &mut World, service_fixed_ticks: bool) -> RenderFrameHandoff {
    let executor = world
        .resource::<bevy::ecs::schedule::MainThreadExecutor>()
        .clone();
    let Some(ticker) = executor.0.ticker() else {
        host_fault(
            world,
            "native-render-executor-missing",
            "render handoff is not on the main-thread executor".into(),
        );
        return RenderFrameHandoff::Failed;
    };
    let ready = world.resource_scope(|world, mut channels: Mut<RenderAppChannels>| {
        let mut receipt = std::pin::pin!(channels.recv());
        loop {
            if service_fixed_ticks {
                let mut received = None;
                // Spend otherwise blocked render time on already admitted
                // simulation, even if the ordinary host slice is exhausted.
                // This also works with no asynchronous participants. Check
                // frame readiness before every indivisible fixed cycle.
                lunco_time::service_pending_simulation(world, || {
                    let mut context = Context::from_waker(Waker::noop());
                    let mut ready = receipt.as_mut().poll(&mut context);
                    if ready.is_pending() {
                        // A render task may need the main-thread executor to
                        // produce the receipt. Give it service at each cycle
                        // boundary instead of starving it behind a fixed burst.
                        ticker.try_tick();
                        ready = receipt.as_mut().poll(&mut context);
                    }
                    match ready {
                        Poll::Ready(frame) => {
                            received = Some(frame);
                            false
                        }
                        Poll::Pending => true,
                    }
                });
                if let Some(frame) = received {
                    break Ok(frame.map_or(RenderFrameHandoff::Failed, RenderFrameHandoff::Ready));
                }
                if !lunco_time::can_service_pending_simulation(world) {
                    // Presentation may now overlap the remaining render time.
                    break Ok(RenderFrameHandoff::Awaiting);
                }
            }
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
                            Ok(Poll::Ready(())) => Poll::Ready(HostReady::Completion),
                            Ok(Poll::Pending) => Poll::Pending,
                            Err(error) => Poll::Ready(HostReady::Fault(error)),
                        }
                    }));
                })
                .pop();
            match ready {
                Some(HostReady::Frame(frame)) => {
                    break Ok(frame.map_or(RenderFrameHandoff::Failed, RenderFrameHandoff::Ready));
                }
                Some(HostReady::Completion) => {
                    // Release the host task scope before entering ECS schedules;
                    // their own main-thread executor must remain available.
                    // Extraction admits receipts even while the fixed clock is held.
                    if !service_fixed_ticks {
                        lunco_core_runtime::admit_simulation_completions(world);
                    }
                }
                Some(HostReady::Fault(error)) => break Err(error),
                None => break Ok(RenderFrameHandoff::Failed),
            }
        }
    });
    match ready {
        Ok(RenderFrameHandoff::Failed) => host_fault(
            world,
            "native-render-handoff-failed",
            "render thread closed before returning its frame".into(),
        ),
        Ok(handoff) => return handoff,
        Err(error) => host_fault(world, "simulation-completion-waker-missing", error.into()),
    }
    RenderFrameHandoff::Failed
}

fn extract_ready_frame(world: &mut World, _extract: &mut World) {
    let handoff = std::mem::take(&mut *world.non_send_mut::<RenderFrameHandoff>());
    let handoff = match handoff {
        RenderFrameHandoff::Awaiting => {
            // Presentation endpoints are already sampled. Only the shared
            // clock-free completion schedule can run before extraction.
            wait_for_render_frame(world, false)
        }
        handoff => handoff,
    };
    let RenderFrameHandoff::Ready(mut frame) = handoff else {
        return;
    };
    frame.extract(world);
    world
        .resource_mut::<RenderAppChannels>()
        .send_blocking(frame);
}
