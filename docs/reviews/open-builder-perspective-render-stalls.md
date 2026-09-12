# Builder perspective render-stall handoff

Status: implementation complete in `main`; the original render-transition
findings remain valid, and the Builder-only port path plus the independent
physics stall have separate owners and fixes. The latest follow-up keeps
invalidation at the provider boundary: structural keys now cover Avian backing
components, value-to-membership transitions, and link-state identity without
sampling live values.

## Report evidence

The source report is Trello card #211, “Document periodic Builder perspective
render stalls”. Its Tracy capture used the Sunfall Twin scene and recorded the
visible stalls during render initialization rather than in telemetry or the
steady-state UI:

- `bevy_core_pipeline::upscaling::prepare_view_upscaling_pipelines`: 74.124 ms
  at 9.135 s, including `block_on_render_pipeline`.
- `bevy_pbr::cluster::gpu::prepare_clusters_for_gpu_clustering`: 53.584 ms at
  9.219 s.
- `bevy_render::view::window::prepare_windows`: 254.310 ms at 2.765 s and
  29.702 ms at 9.295 s.
- `create_surfaces`: 47.018 ms at 2.609 s and 28.164 ms at 3.056 s.

The largest render-schedule bursts were 103.025 ms at 2.57 s, 259.753 ms at
2.764 s, 83.877 ms at 9.133 s, 60.814 ms at 9.217 s, and 40.610 ms at 9.294 s.
The 9.13–9.30 s sequence is an event-driven chain of lazy render prerequisites
(upscaling, GPU clusters, then window preparation), not evidence of a fixed
timer.

`process_queued_usd_visuals` in `crates/lunco-usd-bevy/src/lib.rs` is a bounded
secondary feeder: the default budget is 8 ms; the report measured 182 passes,
1.494 s total, 8.210 ms mean, and 26.848 ms maximum. Telemetry and ordinary UI
paths were not dominant: `retain_physics_telemetry` totalled 8.222 ms across
893 calls, while `populate_inspector_view` averaged 138.317 us and peaked at
704.617 us. After about 9.4 s, the capture tail had Render max 10.940 ms,
Update 4.313 ms, and PostUpdate 5.470 ms, with no continued >16.67 ms burst.

## Current owner map

- `crates/lunco-workbench/src/viewport.rs` intentionally keeps the scene
  `Camera3d` full-window in the docked Builder layout. `apply_workbench_viewport`
  publishes `visible=true, rect=None`; the measured dock leaf is for
  occlusion/picking and must not become a camera crop.
- The same file’s `sync_egui_host_msaa` is change-driven and mirrors the active
  scene camera’s MSAA/HDR into the persistent egui host because both cameras
  share the window main texture.
- `crates/lunco-usd-bevy/src/camera_switch.rs::reconcile_scene_viewport` is the
  sole writer of window-camera `is_active` and `viewport`. It gates activation
  on the render target, projection, positive physical size, and cluster
  readiness.

Do not change these ownership contracts until the explicit View → Builder
transition has been measured for camera, viewport, render-target, egui-host,
surface, and pipeline changes. The likely fix must remove avoidable transition
churn at its owner, without suppressing invalid state or reducing render
quality.

## Root-cause investigation

The first production capture of the Builder-only panel path showed that the
largest repeated work was `lunco_luncosim_edit::ui::ports::populate_port_view`,
not a render-pipeline transition. Its first implementation called
`PortRegistry::entity_port_infos` for every candidate on every 10 Hz sample.
The link backend made that worse by scanning `World::iter_entities()` for every
candidate while discovering authored peer classes. View does not open this panel,
which explains the perspective-specific symptom.

The first committed fix (`b63bf0370`) moved candidate discovery behind the
backend-owned `PortRegistry::port_entities` boundary. A follow-up change indexes
authored link classes on `LinkNode` lifecycle changes and adds owner-provided
`PortBackend::topology_key` callbacks. That still left the invalidation gate
wrong: `UsdStageRevision` describes the whole projected scene, not the port
surface, and the registry-only fallback polled global entity count. Any unrelated
scene projection or entity churn could therefore turn the panel's 10 Hz live
sample into a full candidate/metadata rebuild. View does not mount this panel.

The proper boundary is a durable `PortTopologyRevision` published by the port
providers. Lifecycle observers cover component membership; change-filtered
structural checks compare identity-only fingerprints for in-place control
surfaces, solver interfaces, shader projections, projected link shapes, Avian
backing-component availability, value-to-membership transitions, and connection
endpoints. Live values and connection affine transforms do not advance it.
Mobility is structural only for the Kinematic position group: Dynamic↔Kinematic
changes its candidate surface, while Dynamic↔Static does not. Avian groups keep
their topology key and invalidation hook next to the group predicate, preventing
the candidate key from remaining unchanged after a backing port disappears. The
Builder panel rebuilds candidates only when that generation changes; stable
samples read only live values, wire state, and held values. This preserves
dynamic physics values and does not suppress or fake missing ports.

The post-discovery capture still measured 34–52 ms in the metadata path. The
later capture isolated the physics outlier to
`avian3d::collider_tree::optimization::block_on_optimize_trees`: Avian started
an async collider-tree optimizer and then joined it inside `PhysicsSchedule`.
That join reached 48.926 ms even when the Builder port producer was gated out,
so the physics issue is intermittent scheduler contention, not a Builder-specific
physics configuration.

The final physics owner configuration disables Avian's async optimizer mode. The
supported optimizer still runs with Avian's normal tree-quality algorithm, but
its work is performed in the owning physics schedule instead of being joined
from a worker at the end of the same schedule. The standard
`lunco_physics::DEFAULT_SUBSTEP_COUNT` remains eight.

The final Builder-specific owner was the panel's presentation path, not Avian.
The panel was painting thousands of collapsed headers and matching all 72,131
rows every frame, while its producer rediscovered 4,028 candidates every 100 ms.
The replacement keeps the complete registry projection and command contract,
but virtualizes fixed-height entity headers, paints only explicitly expanded
port grids, requests live values only for those expanded entities, and uses the
owner-published `PortTopologyRevision` to invalidate candidate discovery. This
is why View is unaffected: it does not open the Ports panel or execute its
producer/paint path.

The final Tracy capture (`scripts/perf/captures/builder-perspective-physics-owner-sync-final-20260912.tracy`)
measured `PhysicsSchedule` at 1.528 ms mean and 3.639 ms maximum under
profiler overhead; the optimizer itself peaked at 8.856 us and the former
blocking join peaked at 3.427 us. The non-Tracy production transition run
(`target/luncosim-view-builder-physics-final-20260912.log`) reported Avian total
step samples from 0.247–0.703 ms while switching View → Builder → View, with
settled rolling averages around 0.35–0.55 ms and no runtime errors.

## Validation record

The production `target/debug/luncosim` was built with the opt-in Tracy feature
and profiled with `../tracy/capture/build/tracy-capture` on an explicit free API
port using the Sunfall scene from report #211. The run issued
`ActivatePerspective` for `rover_build`, repeated the View / Builder switch
after settling, and inspected each transition. The clean FPS run was separate
from the Tracy run: its physics result meets the requested sub-1 ms budget,
while the profiled result is diagnostic only and is not the product timing
number.

The post-fix Tracy capture
(`scripts/perf/captures/builder-stage-gated-main-20260912.tracy`) recorded 114
port-producer calls: the initial topology projection took 34.346 ms under
profiler overhead, then stable calls were 37–66 us. `render_workbench` averaged
0.851 ms (8.813 ms maximum) and the egui pass averaged 1.046 ms (9.122 ms
maximum). The clean production run
(`target/builder-stage-gated-main.log`) reported Avian total-step samples of
0.224–0.912 ms during the Builder interval; the standard eight substeps were
unchanged. The isolated transition rebuild remains a one-time render/scene
startup cost and is not the recurring Builder port stall.

The structural-invalidation follow-up was checked with the affected-crate
`cargo check`, the complete `lunco-cosim` library suite (48 passed), the
Builder port-view regression, and the link-class catalog regression. A fresh
production build of `target/debug/luncosim` loaded
`assets/scenes/luncosim/solar_rover_demo.usda` headlessly on API port 4108;
`GET /api/health` returned `{"status":"ok"}`, and the session was closed with
the typed `Exit(force=true)` command. The process and port were absent after
shutdown. The run showed no new runtime failure from the invalidation path.

## Handoff constraints

- The `usd` worktree contains unrelated dirty waypoint-refactor work for
  Trello #182; preserve it and do not use it as a scratch checkout.
- The first fix is committed as `b63bf0370`; the topology/index/cache and
  physics-owner changes are included in the follow-up commit.
- Keep the change scoped to the Builder stall, use the smallest owner test, and
  update this review plus Trello before moving the card to Review.
