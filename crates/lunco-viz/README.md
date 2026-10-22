# lunco-viz

Domain-agnostic visualization framework for LunCoSim.

## Native signal grids

`ConfigureSignalGrid` composes 1–64 active, sampled scalar channels as small
multiples of the ordinary native line plot. Sources may mix Modelica and
physics. Every entry declares `target_gid`, exact `signal`, and a human
`caption`. The command validates the entire selection before changing a view;
missing owners, inactive/unsampled channels, duplicate channels, invalid column
counts and over-budget grids reject without replacing the previous grid.

Example: `{"grid":1,"title":"Stunt measurements","columns":2,"signals":[
{"target_gid":123,"signal":"specific_force","caption":"Accelerometer load"},
{"target_gid":123,"signal":"angular_acceleration","caption":"Angular acceleration"}]}`.
Use actual owner identities and `ListTelemetryChannels` to select available
channels. These example paths only exist when their physics publisher is active.

`InspectSignalGrid` returns each grid's ordered native plot ids. Apply ordinary
`ConfigureLinePlot` settings to those ids before revealing a view. Registered
instance kind `signal_grid`, instance equal to `grid`, works in docks or the
shared borderless floating hosts. The grid keeps each plot's native history,
source-reconnection, transform, follow-window and appearance owner. It creates
no measurements and does not derive acceleration in the presentation layer.
Keep a few readable channels for explanation; dense grids are overview views.

Modelica variables, Avian rigid-body state, simulation events, and any future
domain (USD, SysML, …) push into one **SignalRegistry**. Visualization kinds
(line plot, gauge, 3D arrow, trajectory, …) consume signals and render into
**views** (2D egui panels, the main Bevy 3D viewport, or render-to-texture
sub-panels). The three layers — signal / viz / view — are independent and
compose freely.

## Three layers

```
Signal          Viz kind                 View target
(what)          (how)                    (where)

scalar(f64) ─┐  ┌── LinePlot ───────────→ Panel2D (egui plot)
             ├──┼── Gauge ──────────────→ Panel2D
Vec3 ────────┤  ├── XYScatter ──────────→ Panel2D
             ├──┼── Table ──────────────→ Panel2D
Pose ────────┤  ├── Histogram ──────────→ Panel2D
             │  ├── Arrow3D ────────────→ Viewport3D (Bevy gizmos)
Event ───────┘  ├── Trajectory3D ───────→ Viewport3D (managed entities)
                ├── FrameGizmo3D ───────→ Viewport3D
                └── TensorGlyph3D ──────→ Panel3D (render-to-texture)
```

A `Visualization` impl declares which signal types it accepts (via roles) and
which views it can render into. Binding a viz to a signal is a user-level
operation: "plot `thrust` on the main time-series" = create a
`VisualizationConfig` with kind `LinePlot`, view `Panel2D`, and a
`SignalBinding { source: (rocket, "thrust"), role: "y" }`.

Compound telemetry remains addressable as scalar channels while producers
describe its operator presentation once through `SignalMeta::presentation`.
`SignalPresentation::Component` and `SignalPresentation::Summary` let the
telemetry browser and channel dictionary show vector components with a shared
group and a defined headline formula, without parsing channel names or
duplicating a derived-value registry.

The telemetry browser snapshots channel metadata and the relevant owner
hierarchy when `SignalRegistry::catalog_revision` or `TelemetryFocus` changes.
It sorts and derives the grouped tree on the async-compute pool; sample updates
do not rebuild the catalog, and panel rendering never waits for a pending tree.
Rows read their latest values from the shared registry when painted. Lowercased
filter fields are prepared with the catalog, and descendant visibility counts
are cached until the catalog, focus, filter, scope, or archived-row setting
changes; repainting an unchanged tree does not rescan its descendants.
The panel rebuilds a lightweight index for the expanded groups and paints only
the group/channel rows inside the scroll viewport; values and interaction
handlers are not created for offscreen rows.
Visible row labels, unit labels, and formatted numeric text are retained by
channel catalog revision; formatted values refresh only when the sample value
or display settings change. The panel copies only its three used theme tokens,
not the full theme, and empty-unit help is registered only on hover. Persistent
requested signal selections are resolved when the request or catalog revision
changes and reused across repaints; focus roots are inspected without copying
the selection list.
Each catalog row retains its drag payload with an `Arc<str>` path, so visible
row repaints clone only the shared handle; a `String` path is materialized when
a drop is committed into a visualization or canvas node.
The line-plot signal pickers enumerate scalar channels only while a picker is
open and reuse signal references and metadata until the signal catalog changes;
owner labels are resolved live while the menu is visible. Decimated plot points
are borrowed from the retained cache on steady frames. A changing history is
snapshotted at most 20 times per second, and pairing, log conversion, and
decimation run on the async-compute pool; the plot keeps drawing its latest
completed buffer while that work runs. This bounds presentation lag to roughly
50 ms without making the UI wait for history conversion.
The active line plot keeps its binding and frame-series lists inline for the
usual small curve count and moves each formatted label into its plot item once
per paint, without cloning retained point buffers.

Each line plot's toolbar has a `Line` thickness control (0.5–8 logical pixels).
The value belongs to `LinePlotStyle`, survives visualization save/load and
duplication, and affects only the curve stroke. It does not scale interface
text or modify samples, history decimation, axes, or physical state. Omitted
widths use 1.5 pixels. The toolbar emits `LinePlotEditRequested`; its observer
owns the registry update consumed by the native `egui_plot::Line` renderer.

Right-click a plot for appearance settings: axis titles (including units),
editing-control visibility, and a reference Y range. The range stays visible
during autoscaling but does not clip measurements outside it. This prevents
nearly constant signals magnifying numerical noise. Automation uses the same
persisted style through `ConfigureLinePlot`, for example:

```json
{"plot":1,"x_label":"Time [s]","y_label":"°C","line_width":4.0,"text_size":28,"show_controls":false,"show_legend":false,"simplified_grid":true,"caption":"Air temperature","compact_layout":true,"y_reference_range":[-140,30]}
```

`show_legend:false` removes redundant curve names when the axes explain a single
trace. `simplified_grid:true` keeps sparse major guides; `caption` adds a concise
title above the native chart. With `compact_layout:true`, the title moves inside
the upper center of the native plot frame, Y units sit horizontally at the upper-left,
and the X quantity/units sit at the lower-right. Numeric ticks also move inside, using the same native plot transform and major
grid marks. Labels that collide with corner titles or other values are omitted;
no outer axis rows are reserved. These text anchors do not enter data bounds or
autoscaling. Keep titles short and use a separate explanatory caption for provenance.
Empty caption removes it. Both renderers
use the same settings, also exposed in the context menu. Keep provenance in a separate contextual caption when a bench experiment sits
beside a different live scene.

Omitted fields preserve the style and signal bindings. An empty axis title
restores automatic labelling; an empty `y_reference_range` removes the reference
interval. Invalid widths or ranges reject the whole edit. Restore hidden
controls from the plot context menu.

`text_size` sets axis numbers, axis titles, legends and hover values together,
in logical pixels (8–64). The change is scoped to the plot; application chrome
and editing controls retain their theme sizes. Omission preserves the setting;
zero restores the theme. The context menu exposes the same customization.

`follow_latest: true` preserves the current horizontal time span and advances
it with the latest displayed sample. `time_window_s: 5` seeds a five-second
window; later user zoom changes are retained. Zero clears that requested span
and follows with the current window. Turning follow off restores fit-all
history. Both live and experiment plots use the same controls and renderer
policy. Following is inactive for a phase-space X signal. This only changes the
view; recording and telemetry retention are independent.

Persisted bindings are reconciled from stable `GlobalEntityId` identity only
when the visualization registry or an identity publication changes. Unchanged
scenes do not rebuild entity lookup maps or rewrite plot bindings; missing
identities remain explicitly unresolved until the authoritative scene
projection publishes them.

Scene-derived widget data uses `scene_state::scene_data`, a scoped egui
`IdTypeMap`: history buffers, pending plot tasks, canvas snapshots and interests,
input-write queues and plot-drop requests share one lifetime. `LuncoVizPlugin`
tracks all Bevy egui context lifetimes; final resource retirement
removes the entire store, telemetry drag payloads and native plot interaction
memory after shutdown callbacks, including bounds from the outgoing clock.
Hidden widgets need no repaint to release their data. New scene-derived entries
must use this store; layout preferences and visualization configurations
remain application-owned.

## Crate layout

The render-free `VizId` is owned by [`lunco-viz-core`](../lunco-viz-core/).
This crate consumes that identifier and owns the UI/rendering side of the
visualization framework.

| Module              | Role                                                        |
|---------------------|-------------------------------------------------------------|
| `signal`            | `SignalRef`, `SignalType`, `SignalMeta`, `SignalPresentation`, `SignalRegistry` |
| `viz`               | `Visualization` trait, `VisualizationConfig`, `VizKindId`   |
| `view`              | `ViewTarget`, `ViewKind`, compatibility matrix              |
| `registry`          | `VisualizationRegistry` (Bevy resource)                     |
| `scene_state`       | Scene-scoped widget data and final egui retirement         |
| `panel`             | `VizPanel` — generic `InstancePanel` keyed by `VizId`       |
| `kinds::line_plot`  | First concrete viz kind (time-series line on `Panel2D`)     |
| `multi_series_plot` | Reusable completed-run/live trajectory comparison widget    |

All of these are wired by `LuncoVizPlugin`.

## Current status (v0.1)

Implemented:

- [x] `SignalRegistry` with scalar time-series support
- [x] Producer-owned compound-value presentation metadata and telemetry grouping
- [x] `Visualization` trait + registration via `App` extension
- [x] `LinePlot` viz kind (2D time-series, feature-parity with the
      Modelica Graphs panel)
- [x] Reusable multi-series trajectory plot with overlays, log-Y, fit, and
      scrub interaction
- [x] `VizPanel` — multi-instance workbench panel keyed by `VizId`
- [x] `LuncoVizPlugin`

Not yet implemented (all accommodated by the data model):

- [ ] Additional 2D viz kinds: `Gauge`, `Table`, `XYScatter`, `Histogram`, `BarChart`
- [ ] 3D view target (`Viewport3D`) with Bevy gizmos + managed entities
- [ ] 3D viz kinds: `Arrow3D`, `Trajectory3D`, `FrameGizmo3D`, `TensorGlyph3D`
- [ ] `Panel3D` (render-to-texture sub-scenes in egui)
- [ ] Per-viz inspector UI (config editor)
- [ ] Derived signals (transforms at the binding: derivative, magnitude, unit convert)
- [ ] Workspace JSON persistence (save / load plot presets)
- [ ] Shared X-axis / shared cursor across multiple `VizPanel` instances
- [ ] Event markers (compile / reset / user-defined) as first-class annotations
- [ ] Drag-from-Telemetry signal binding
- [ ] **[Rerun](https://rerun.io) sidecar logging** — see "Rerun integration" below

## Future: Rerun integration (sidecar logger, not embedded)

[Rerun](https://rerun.io) is the Rust-native multi-modal visualization
toolkit used heavily in robotics / autonomy / simulation
post-processing. It handles exactly the data types we care about
(scalar time-series, `Points3D`, `Arrows3D`, `Transform3D`, meshes,
images, annotations, coordinate frames) with a polished time-scrubber
viewer.

We do **not** plan to embed Rerun inside the workbench — their viewer
is architected as a standalone egui + wgpu app, and mixing render
pipelines with our Bevy scene is an integration rabbit hole. Instead:
Rerun becomes an **optional sidecar logger** that the user can turn on
to get replay / deep-dive / side-by-side tooling for free.

### Integration plan

1. **Behind a Cargo feature.** `lunco-viz` adds
   `rerun = { version = "...", optional = true }` and a feature
   `rerun`. Off by default — workbench builds without the dep when
   unused.
2. **Single subscription point.** `SignalRegistry::push_scalar` (and
   future `push_vec3`, `push_pose`, `push_event`, …) are the only
   producers that need to know about Rerun. When the `rerun` feature
   is on and a `RerunLogger` resource is installed, each push is
   mirrored into Rerun's recording stream as a scalar / points3d /
   transform entity on the right timeline.
3. **Entity path mapping.** `SignalRef { entity, path }` ↔ Rerun's
   `EntityPath` — straightforward: `/entity_{id}/{path}`. Provenance
   metadata (`modelica`, `avian`, `script`) becomes a namespace
   prefix.
4. **Time is shared.** The simulation time from the Modelica worker
   becomes Rerun's primary timeline (`sim_time`). Real wall-clock is
   a secondary timeline.
5. **Toolbar toggle.** A workbench-level command opens a Rerun
   recording: either starts the standalone viewer (`rerun::spawn()`)
   and streams live, or writes a `.rrd` file for later replay.
6. **No bidirectional control.** Rerun is a downstream sink — we
   never read from Rerun back into our own plots. Simplifies
   lifecycle: users can close the viewer without breaking anything.

### Why a sidecar, not embedded

| Aspect | Embedded Rerun | Sidecar Rerun |
|---|---|---|
| Integration cost | Months (two viewers competing for egui / wgpu / window) | Hours (subscribe to `push_*` hooks) |
| Disruption to current UX | Large | None (opt-in, same window) |
| What we gain | Rerun's full UI in our workbench | Rerun's full UI next to our workbench |
| What we lose | — | A unified window |
| Maintenance coupling | Tight (Rerun releases) | Loose (feature-gated) |

The sidecar path is strictly additive: users who never enable it see
no difference; users who enable it get replay, blueprint presets, and
all the power-user tooling Rerun ships. Over time, if embedded
integration becomes practical (Rerun's `re_viewer::embed()` matures),
we can revisit — the `SignalRegistry` → Rerun subscription stays the
same in either case.

### Prior art to steal from

- Rerun's SDK examples under `examples/rust/` show the per-domain
  logging idioms (SLAM, IMU, manipulation). Apply the same shape for
  Modelica scalars + Avian rigid bodies.
- The `rerun-rs` crate has a `RecordingStream` that's cheap to hold
  as a Bevy resource; mirror our `SignalRegistry` write paths into
  it.
- For the file-output mode, Rerun's `.rrd` files are self-contained
  and good artifacts for issue reproduction — attach one to a bug
  report and reviewers load it in the standalone viewer.

## Architectural invariants

These are the boundaries we do **not** cross:

1. **Signal producers never depend on `lunco-viz`-side viz kinds.** The
   Modelica worker publishes `SignalSample` into the registry; it does not
   know or care what viz kind renders those samples.
2. **Visualization kinds never depend on domain crates.** A `LinePlot`
   doesn't know about Modelica; it operates on `SignalRef` + `SignalType`.
3. **Views are rendering targets, not data stores.** A `Panel2D` doesn't
   hold the data it renders — it reads from `SignalRegistry` every frame.
4. **`VizId` is stable across sessions.** A saved workspace can reload a
   plot's configuration and re-bind to its signals by `SignalRef`. Missing
   signals render as "pending" rather than errors.

## Dependency direction

```
lunco-modelica-core     lunco-cosim (Avian bridge)      USD projection packages
       │                   │                              │
       └───────────────────┴──────────────────────────────┘
                           │
                           ▼
                      lunco-viz
                           │
                           ▼
                    lunco-workbench
```

`lunco-viz` depends only on `lunco-workbench` (for the `Panel` /
`InstancePanel` traits) and the rendering backends (`bevy`, `bevy_egui`,
`egui_plot`). Nothing viz-side knows about Modelica, USD, Avian, or any
specific domain.

## Modelica samples

`ModelicaExecutionPlugin` installs `ModelicaTelemetryPlugin`. That owner reads
`ModelicaModel` clocks and the required `ModelicaVariables.values` snapshot,
then retains samples through the shared `SignalRegistry` rate, deadband and
channel admission. Visualization panels consume that registry. Do not install
another per-frame Modelica publisher beside the telemetry owner.

Experiment and mixed live/experiment graphs consume the same persisted
`LinePlotStyle`: axis titles, line width, editing-control visibility and
reference Y bounds remain effective when a completed experiment is present.
`NewPlotPanel.signals` initializes both live bindings and experiment-variable
selection in the current document scope.

`show_latest_value:true` adds the last measured value for a single displayed
trace, with the Y unit, in the upper-right of the compact chart. The title stays
centered on the same row. It uses the actual series endpoint; multiple traces
omit that readout to avoid assigning one number to several quantities. For live
presentation, combine it with `follow_latest:true,time_window_s:5`.
