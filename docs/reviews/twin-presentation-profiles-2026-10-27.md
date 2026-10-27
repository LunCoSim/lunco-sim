# Twin-owned input and HUD loading

## Contract and owners

Ordinary `OpenTwin` discovers the active Twin's typed input and runtime UI
JSON documents through the existing `JsonAssetScopeChanged` publication.
`lunco-luncosim-simulation` owns input-source admission; `lunco-input-core`
composes scene overrides into `ActiveInputBindings` before native input
processing. All controllers, key injection, pointer tools, HUD help and Rhai
labels consume that effective snapshot. `InputBindingsSettings` remains the
persisted application/user map. Empty lists unbind; omitted entries inherit.

`lunco-workbench-runtime-ui` admits one UI manifest per scope, composes matching
surface IDs, qualifies asset references to their exact source authority, and
rebuilds retained roots. Shared application surfaces stay available alongside
Twin additions. Explicit `lunco://` imports reuse shared assets. JSON is parsed
only at the authored asset boundary; the internal profiles and manifests are
owned Rust types.

Source caches have explicit open-Twin lifetimes and reject retired Twin IDs.
Scene teardown resets effective state and wakes publication without requiring a
physics tick. `TwinClosed` retires the exact source contribution. Invalid or
ambiguous typed data raises a structured owner fault; simulation admission stops
and the application remains controllable for replacement.

This extends existing asset, input and retained UI mechanisms. Source selection
and composition are generic invariants; they do not need a new policy hook.
Rhai continues to own the actual input-to-port meaning, HUD contents, visibility
and button actions. No third-party source or USD schema changes are involved.

## Tesla consumer

The existing `roadster_cinematic` scene is already the correct manifest default.
Its Twin now contains a typed input document and its own runtime UI manifest,
using the existing Roadster HTML/CSS and telemetry policy. Direct loading acquires
manual control and resolves K lift, the corner/pair jets, Ctrl+P and all other
Tesla bindings. The launcher opens that same scene using shared application
assets; it does not copy a keymap into user settings or generate an asset tree.
The scene and vehicle USD sources are unchanged.

## Evidence

A clean configuration and shared application asset root reproduced the defect:
`OpenTwin` acquired the Roadster, but `input_binding("speed_boost")` returned
Shift and the generic vessel HUD rendered. The fixed direct load reports K and
Ctrl+P and renders the Roadster air, fan, refill and course controls.

- Production build: `cargo build -p lunco-luncosim -j 4`.
- `scripts/check_controls.py --port 4519 --output-dir ...` in the Tesla Twin:
  31 checks passed through native input events and real participant ports,
  including held/released jets, drive, steering, braking, fans, refill, pause,
  camera cycling and all tested HUD actions. Injection begins at Bevy events;
  it does not prove OS/compositor keyboard delivery.
- `scripts/api/test_twin_presentation_profiles.py --port 4520`: 21 authored
  Rhai checks passed across direct load, repeated reopen, paused replacement,
  unchanged user preferences, invalid input, duplicate input and invalid UI.
  Negative cases assert the owner fault and stopped master clock. Recovery,
  API Exit and port release passed.
- Consumer test targets compile with
  `cargo check -p lunco-controller -p lunco-workbench -p lunco-workbench-runtime-ui --tests -j 4`.
- Skill catalogue, shell syntax and diff checks pass.

Local captures, reports and runtime logs are under
`.runtime-check/tesla-default-loading/`. The before/after screenshots were
visually inspected. Runtime controls were tested with the original native
solver revision; integration with newer local main is qualified separately.
