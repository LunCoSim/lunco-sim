# Transform gizmo Bevy integration

Source: LunCoSim/mitchty-transform-gizmo revision 8e5d26f6d8bcae547ed0d269f9e25cd62ceacb53, package transform-gizmo-bevy 0.9.0. Original licenses are retained. The geometry engine remains pinned to that revision.

The frontend reads Bevy primary mouse PointerLocation, the same input owner used by native picking. Window cursor state is not an input source for transform evaluation. This makes operating-system and injected pointer events follow one production gesture path. Preview viewport conversion remains owned by this integration.

Consumer: SceneEditGizmoPlugin. Production verification: assembly_property_editor, including handle drag, blank-space camera pan, and document journal validation.
