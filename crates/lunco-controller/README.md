# lunco-controller

Input mapping and controller translation for LunCoSim vessels.

## What This Crate Does

This crate resolves user input (Keyboard, Gamepad, Mouse) from the persisted
`InputBindingsSettings` section and translates it into typed command events that
Flight Software (FSW) can consume.

- **Input Mapping** — `assets/config/keybindings.json` supplies the bundled
  semantic defaults; user overrides live under `input_bindings` in
  `<OS config dir>/lunco/settings.json`. The resolved resource is projected into every live
  avatar input map when it changes.
- **Intent Translation** — Maps semantic `UserIntent` actions into the shared
  `SetPorts` command surface and authored per-vessel `ControlBinding`s.
- **Context Awareness** — UI focus and session authority gates are applied at
  the shared controller boundary, so input cannot fight a text field,
  autopilot, or another owning session.

## Architecture

The controller acts as the **Human-Machine Interface (HMI)** layer, decoupling raw HID events from simulation logic.

```
lunco-controller/
  ├── InputBindingsSettings — persisted semantic key/pointer map
  ├── UserIntent            — shared abstract action vocabulary
  ├── ControllerLink    — Component linking a controller entity to a vessel
  └── lib.rs             — translation, authority, and input projection
```

## Usage

```rust
app.add_plugins(LunCoControllerPlugin);

// Assign a controller to a rover
commands.spawn((
    ActionState::<UserIntent>::default(),
    InputBindingsSettings::default().input_map().expect("bundled keymap"),
    ControllerLink { vessel_entity: rover_id },
));
```

Tutorials use `input_binding("forward")` / `input_hint("forward")` through the
Rhai bridge, so their labels follow the same resolved settings resource rather
than copying physical key names.

The bundled `speed_boost` intent (`ShiftLeft`/`ShiftRight`) is authored on the
free-flight avatar as a normalized `speed_boost` command port. It is emitted in
the same `SetPorts` batch as `forward`/`side`/`up`, so physical Shift and
`SimulateIntent` use one controller path and cannot split a movement frame.

The settings section is an override layer over the bundled map: omitted semantic
bindings inherit the current defaults, while an explicit empty array means
unbound. This keeps a saved keymap from silently losing a newly added intent.

### Lander control frame

The authored `LanderControls` profile maps semantic intents to the lander's
body-frame command ports: forward/back to `pitch` (`-1`/`+1`), left/right to
`roll` (`+1`/`-1`), yaw-left/yaw-right to `yaw` (`+1`/`-1`), thrust to
`external_throttle`, and release to `release`. The profile selects an `orbit`
camera, so the camera remains in a stable external frame while the vehicle
rotates inside it; changing the camera orientation does not remap the physical
body axes. Opposite intents sum and cancel through `ControlBinding::resolve`,
and releasing an intent writes zero to every authored command port.

The default W/S/A/D/Q/E labels are only the current `input_bindings` projection.
UI help resolves them from `InputBindingsSettings`, so remapping changes the
display without introducing a second control scheme.

## See Also

- `lunco-mobility` — Consumes the bound `SetPorts` inputs and projects them onto the authored actuator ports.
- `lunco-core` — Defines standard `UserIntent` used for avatar navigation.
