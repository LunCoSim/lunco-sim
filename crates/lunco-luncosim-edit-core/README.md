# lunco-luncosim-edit-core

Headless-safe ECS mechanisms for LunCoSim scene editing.

This crate owns spawn and terrain-tool state, scene picking, typed editor
command registration, and the editor-side ECS state used by those mechanisms.
It depends on the shared scene-command layer and
does not contain egui panels, workbench layout, transform-gizmo integration,
or immediate-mode debug visualization.

The rendered presentation is provided by the sibling
[`lunco-luncosim-edit-ui`](../lunco-luncosim-edit-ui) package. Hosts that need
the editor add `SceneEditPlugin` and `SceneEditUiPlugin` separately; headless
hosts can use this package without compiling the UI package.
