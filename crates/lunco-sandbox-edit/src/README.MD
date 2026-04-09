   Selection Behavior Now


    ┌───────────────┬─────────────────────────────────┬─────────────────────────┬───────────┐
    │ You Click On  │ Raycast Hits                    │ find_selectable Returns │ Selected  │
    ├───────────────┼─────────────────────────────────┼─────────────────────────┼───────────┤
    │ Wall          │ Wall collider → Wall entity     │ Wall (has Name)         │ Wall ✓    │
    │ Ground        │ Ground collider → Ground entity │ Ground (has Name)       │ Ground ✓  │
    │ Rover chassis │ Rover collider → Rover entity   │ Rover (has Name)        │ Rover ✓   │
    │ Rover wheel   │ Wheel collider → Wheel entity   │ Walks up → Rover        │ Rover ✓   │
    │ Empty space   │ Nothing                         │ None                    │ Nothing ✓ │
    └───────────────┴─────────────────────────────────┴─────────────────────────┴───────────┘


    Controls

    ┌──────────────────┬────────────────────────────┐
    │ Input            │ Action                     │
    ├──────────────────┼────────────────────────────┤
    │ Shift+Left-click │ Select entity under cursor │
    │ Left-click       │ Camera possession          │
    │ G                │ Translate gizmo            │
    │ R                │ Rotate gizmo               │
    │ Q                │ Select mode (no gizmo)     │
    │ Delete           │ Delete selected            │
    │ Ctrl+Z           │ Undo                       │
    └──────────────────┴────────────────────────────┘