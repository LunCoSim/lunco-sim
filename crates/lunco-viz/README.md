# lunco-viz

Domain-agnostic visualization framework for LunCoSim.

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

## Crate layout

| Module              | Role                                                        |
|---------------------|-------------------------------------------------------------|
| `signal`            | `SignalRef`, `SignalType`, `SignalRegistry` (Bevy resource) |
| `viz`               | `Visualization` trait, `VisualizationConfig`, `VizKindId`   |
| `view`              | `ViewTarget`, `ViewKind`, compatibility matrix              |
| `registry`          | `VisualizationRegistry` (Bevy resource)                     |
| `panel`             | `VizPanel` — generic `InstancePanel` keyed by `VizId`       |
| `kinds::line_plot`  | First concrete viz kind (time-series line on `Panel2D`)     |
| `render::panel_2d`  | egui_plot adaptor                                           |

All of these are wired by `LuncoVizPlugin`.

## Current status (v0.1)

Implemented:

- [x] `SignalRegistry` with scalar time-series support
- [x] `Visualization` trait + registration via `App` extension
- [x] `LinePlot` viz kind (2D time-series, feature-parity with the
      Modelica Graphs panel)
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
lunco-modelica     lunco-cosim (Avian bridge)      future: lunco-usd, ...
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

## Usage sketch (once it matures)

```rust,ignore
// Domain side (Modelica worker, Avian bridge, etc.):
app.add_plugins(LuncoVizPlugin);

// Publish samples each frame:
fn publish_modelica_samples(
    q_models: Query<&ModelicaModel>,
    mut registry: ResMut<SignalRegistry>,
) {
    for model in &q_models {
        for (name, value) in &model.variables {
            registry.push_scalar(
                SignalRef::new(model.entity, name),
                model.current_time,
                *value,
            );
        }
    }
}

// UI side: a user opens a new graph from the tab strip:
fn on_new_plot(world: &mut World) {
    let id = VizId::next();
    world.resource_mut::<VisualizationRegistry>().insert(
        id,
        VisualizationConfig {
            kind: VizKindId::line_plot(),
            view: ViewTarget::Panel2D,
            inputs: vec![/* bound later via drag-drop or inspector */],
            style: VizStyle::default(),
            ..default()
        },
    );
    world.commands().trigger(OpenTab {
        kind: VIZ_PANEL_KIND,
        instance: id.raw(),
    });
}
```
