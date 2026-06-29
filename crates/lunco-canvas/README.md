# lunco-canvas

A stateful 2D scene editor substrate for LunCoSim.

## What This Crate Does

`lunco-canvas` provides a generic, extensible foundation for 2D diagramming and node-based editing. It drives Modelica diagrams, future node-graph editors, and annotation overlays.

- **Stateful Viewport** — Smooth pan and zoom math with coordinate round-trips.
- **Pluggable Tools** — One active tool handles input (Pan, Drag, Connect, etc.).
- **Layer Pipeline** — Ordered render passes for grid, nodes, edges, and selection halos.
- **Visual Registry** — Maps data kinds to specific `NodeVisual` or `EdgeVisual` implementations.
- **Domain-Agnostic** — The data model (`Scene`, `Node`, `Edge`) is pure data and serializable.

## Architecture

The canvas is designed around three main extension seams:

| Slot | Purpose |
|---|---|
| **Tool** | Handles user input and produces `ToolOutcome` |
| **Layer** | Renders a specific part of the scene to the `egui::Ui` |
| **Overlay** | Floating screen-space UI elements (nav bars, toolbars) |

```
lunco-canvas/src/
  ├── canvas.rs     — Main Canvas widget and UI loop
  ├── scene.rs      — Data model: Scene, Node, Edge, Port
  ├── viewport.rs   — Viewport math (zoom, pan, smoothing)
  ├── event.rs      — Canvas input/interaction events
  ├── tool.rs       — Input handling traits and DefaultTool
  ├── layer.rs      — Render pass implementations (Grid, Nodes, Edges)
  ├── overlay.rs    — Floating screen-space UI (nav/toolbars)
  ├── visual.rs     — Visual representation traits and registries
  └── selection.rs  — Selection state management
```

## Usage

```rust
let mut canvas = Canvas::new(scene, visual_registry);
canvas.ui(ui, &SnapSettings::default());
```

## See Also

- `lunco-ui` — Consumes this for the Modelica diagram editor.
- `lunco-doc` — Plans to integrate this as a first-class `SceneDocument`.
