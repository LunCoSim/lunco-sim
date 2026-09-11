# Periodic Builder Perspective Render Stalls

> Status: Open diagnostic · Audience: maintainers of the USD projection,
> Workbench perspectives, and Bevy render startup path · 2026-09-11

## Finding

The stalls seen after opening the Sunfall Twin in the Builder/3D perspective
are primarily render-initialisation stalls, not telemetry-recording stalls.
The current Tracy capture shows lazy Bevy render setup occurring in bursts as
the perspective, its render target, and newly projected USD visuals become
available. Several of those setup paths synchronously wait for GPU-side or
driver-side state, so the whole `Render` schedule can miss multiple frame
deadlines.

The word *periodic* describes the user-visible pattern, but the capture does
not show a fixed-rate timer. It shows a finite sequence of event-driven bursts:
the USD visual queue continues admitting scene work, then the renderer pays a
one-time or transition cost when a new view/pipeline/cluster/surface state is
first used. Normal frames run between those events. Once the captured scene
settled, no further frame exceeded 16.67 ms in the trace tail.

## Scope and reproduction

The evidence was collected with Tracy on the current `main` source:

- Source commit: `4664fc44338281376ae2abe77cffff0ff9b588a1`
- Scene: `/home/rod/Documents/scenes/sunfall-run/my_survey.usda`
- Runtime: `target/debug/luncosim --api 4134 --no-vsync --no-throttle
  --log-diag --scene /home/rod/Documents/scenes/sunfall-run/my_survey.usda`
- Display backend: X11; the binary reported `Tracing with Tracy is active`
- Capture duration: 20.32 s; 1,697 frames; approximately 5.7 million zones
- Tracy capture: [sunfall-main-4664-telemetry-on-20260911.tracy](/home/rod/Documents/luncosim-workspace/main/scripts/perf/captures/sunfall-main-4664-telemetry-on-20260911.tracy)
- Report checkout: `/home/rod/Documents/luncosim-workspace/usd`, branch
  `usd` at `2c49d3fc7`; pre-existing dirty work was preserved

The absolute times below are profiling times. Tracy instrumentation adds
overhead, so they are attribution evidence rather than clean product frame
times. The ordering, ownership, and correlation are the useful parts.

## Measured cause

| Trace owner | Evidence | Interpretation |
|---|---:|---|
| `bevy_core_pipeline::upscaling::prepare_view_upscaling_pipelines` | 74.124 ms at 9.135 s | A new/changed view pipeline was specialised and synchronously waited on through `block_on_render_pipeline`. |
| `bevy_pbr::cluster::gpu::prepare_clusters_for_gpu_clustering` | 53.584 ms at 9.219 s | GPU-clustering buffers/readback state were prepared for a view; this is a render-preparation transition, not telemetry. |
| `bevy_render::view::window::prepare_windows` | 254.310 ms at 2.765 s; 29.702 ms at 9.295 s | Window surface acquisition/reconfiguration was expensive during startup and a later surface transition. |
| `bevy_render::view::window::create_surfaces` | 47.018 ms at 2.609 s; 28.164 ms at 3.056 s | Initial render surfaces were created/recreated while the window and view were coming up. |
| `schedule{name=Render}` | 103.025 ms at 2.570 s; 259.753 ms at 2.764 s; 83.877 ms at 9.133 s; 60.814 ms at 9.217 s; 40.610 ms at 9.294 s | The user-visible stalls are render-schedule misses containing the setup work above. |

The burst at approximately 9.13–9.30 s is especially explanatory:

1. `Render` takes 83.9 ms while upscaling pipeline preparation takes 74.1 ms.
2. The following transition takes 60.8 ms while GPU cluster preparation takes
   53.6 ms.
3. The next transition takes 40.6 ms while window preparation takes 29.7 ms.

These are different lazy prerequisites becoming ready on successive frames,
not one telemetry callback waking up at a regular interval.

## Why the pattern looks periodic

The runtime has two interacting streams:

```text
async USD/stage work becomes ready
    -> bounded visual projection admits a batch
    -> render-world view/resource state changes
    -> lazy pipeline, cluster, or surface setup runs
    -> Render blocks for that prerequisite
    -> ordinary frames resume until the next state change
```

The USD side deliberately uses a bounded main-thread queue. In
[`lunco-usd-bevy/src/lib.rs`](../../crates/lunco-usd-bevy/src/lib.rs),
`UsdVisualProjectionSettings` defaults to an 8 ms projection budget, and
`process_queued_usd_visuals` is the structural projection boundary. The
capture recorded 182 `process_queued_usd_visuals` passes, with 1.494 s total,
8.210 ms mean, and 26.848 ms maximum. This queue is a secondary contributor
and the feeder for render-state changes; it is not the large blocking leaf in
the observed stalls.

Because stage/asset readiness and GPU resource creation do not arrive at a
constant cadence, the resulting gaps can look periodic in the UI while their
actual intervals vary. The observed large events are clustered during startup
and settling, rather than repeating at a stable 5 Hz, 60 Hz, or telemetry
sampling interval.

## Telemetry and UI exclusion

Physics telemetry was measured directly. The
[`retain_physics_telemetry`](../../crates/lunco-usd-sim/src/physics_telemetry.rs)
system consumed 8.222 ms total across 893 calls, with a 9.206 µs mean and a
38.462 µs maximum. Its command wrapper had a 1.917 ms total and an 8.967 µs
maximum. That is orders of magnitude below the 29–260 ms render stalls and
does not explain their cadence.

Other inspected paths were also not dominant:

- `populate_inspector_view`: 217.296 ms total over 1,571 calls, 138.317 µs
  mean, 704.617 µs maximum.
- `run_egui_context_pass_loop_system`: 1.160 s total over 1,696 calls,
  684 µs mean, 9.156 ms maximum.
- `process_usd_sim_prims`: 242.99 ms total over 182 calls, 1.335 ms mean,
  4.494 ms maximum.

The telemetry recorder may make the workload look active, but it is not the
owner of the periodic stalls in this capture.

## Startup versus settled behaviour

Startup contained the largest isolated event: `Render` reached 259.753 ms at
2.764 s, coincident with a 254.310 ms `prepare_windows` event. Surface creation,
shader extraction, and initial view setup were also present around 2.6–3.1 s.

The later 9.13–9.30 s sequence is a settling/transition burst associated with
the incoming visual state. After approximately 9.4 s:

- `Render` maximum: 10.940 ms; no frame over 16.67 ms
- `Update` maximum: 4.313 ms
- `PostUpdate` maximum: 5.470 ms

Therefore this run does not demonstrate an ongoing steady-state periodic
stall after all visual work has settled. If stalls continue after that point
in the user session, a capture must include that post-settle interaction to
determine whether a separate invalidation source is involved.

## Builder-specific interpretation

If “Builder” means the Build/Assembly/Editor perspective shown in the UI, the
perspective is a plausible trigger because entering or switching it can create
or change a camera, preview/render target, viewport size, cluster configuration,
and surface lifecycle. Those changes are exactly the kind of first-use state
that the measured Bevy preparation systems handle lazily. The perspective does
not need to record telemetry for this to happen.

This run launched into the saved scene/perspective state; it did not contain a
synthetic click on one named Builder button. The Builder attribution is thus a
source-and-timeline inference from the render resources created during the
perspective startup, not a claim that one specific UI callback was isolated.

If “builder” means `cargo build`, that is unrelated to the runtime stalls. The
profiled process was already a built executable; the stalls occurred during
runtime render preparation.

## Ownership and limits

The immediate blocking owners are the pinned Bevy render systems in the
upscaling, PBR cluster, and window/surface paths. The USD-side owner is the
bounded visual projection queue, which supplies render-world changes but was
not the largest blocking leaf. Physics telemetry is a fixed-step producer and
is conclusively secondary in this trace.

This report is a diagnosis only. No source, scene, telemetry, render-quality,
or external Twin changes were made. The next measurement, if needed, should
record an explicit Builder enter/switch and a fully settled post-load window so
that a persistent invalidation can be distinguished from the finite startup
burst documented here.
