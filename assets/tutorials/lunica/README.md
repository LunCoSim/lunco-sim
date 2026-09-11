# Lunica authored lessons

These files are Rhai scenarios for the Modelica workbench. They are launched
from an application menu through `RunScenarioAsset`; no Modelica crate owns
their catalog or lifecycle.

The scenarios use the generic guided-overlay helpers:

- `coach_step(steps, index)` presents a coach card;
- `focus_panel(id)` opens a workbench panel before `spotlight(...)` highlights it;
- `cmd("OpenClass", #{ qualified: "..." })` opens a read-only Modelica class;
- `emit("MISSION_COMPLETE", 0)` reports completion to the scenario's consumers.

The application may reread the `.rhai` source on native after an edit, so a
lesson can be replayed without rebuilding the Rust core. The catalog metadata
and launch paths live in [`../catalog.json`](../catalog.json).
