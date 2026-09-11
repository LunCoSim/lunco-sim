---
name: author-tutorial
description: >
  Author an interactive tutorial, guided lesson, onboarding flow, coach-mark
  tour, or objectives checklist in LunCoSim. Use for work under
  `assets/tutorials/` and for requests involving `mission`, `objective`,
  `coach_step`, `hint`, or `spotlight`.
---

# Author an authored lesson

A lesson is a file-backed Rhai scenario with optional standard USD scene
content. The application menu is the Rust-owned presentation entry point. The
lesson itself does not need a Rust type, registry, lifecycle owner, or custom
USD schema.

Read [`author-scenario`](../author-scenario/SKILL.md) first, then use
[`assets/tutorials/README.md`](../../assets/tutorials/README.md) and the
examples under `assets/tutorials/`.

## Add a lesson

1. Add `assets/tutorials/<track>/<name>.rhai`.
2. If it needs a world, reuse or add an authored scene under `assets/`.
3. Add an entry to `assets/tutorials/catalog.json`:

```json
{
  "track": "Sandbox",
  "title": "First Drive",
  "blurb": "Take control of a rover and drive it to a lunar flag.",
  "difficulty": "beginner",
  "source_asset": "lunco://tutorials/sandbox/first_drive.rhai",
  "scene_asset": "lunco://tutorials/sandbox/first_drive.usda"
}
```

The menu submits the generic `RunScenarioAsset` command. It uses
`ScenarioReloadPolicy::Restart` for a predictable fresh start. Other apps can
reuse the command without importing tutorial code.

## Rhai contract

Use the shared prelude:

- `hint(...)`, `spotlight(anchor, caption)`, and `notify_kind(...)` for
  presentation;
- `coach_step(steps, index)` with an `on_event` cursor for a guided tour;
- `mission(me)` and `objective(...)` for an objective-driven exercise;
- `input_binding(...)`/`input_hint(...)` for the controller-owned semantic
  labels.

Progression must observe semantic commands or authoritative state. Never gate a
lesson on a physical key name or a timer. A lesson must not open a USD layer
directly; if it needs a world, the catalog's `scene_asset` is the request.

Example objective:

```rhai
fn mission(me) {
    [
        objective("possess", #{
            text: "Select the rover to take control",
            requires_event: "cmd:PossessVessel",
        }),
        objective("reach_flag", #{
            text: "Drive to the glowing flag",
            requires: ["possess"],
            done: |m| distance(find("/World/Rover"), find("/World/Flag")) < 6.0,
            dwell: 0.4,
        }),
    ]
}
```

## USD scene contract

Scene ownership stays in the USD scene command layer. `RunScenarioAsset`
submits a `SceneTransitionIntent`; the USD owner resolves and composes it, and
the generic scenario driver waits for the completion/readiness edge.

Use standard USD composition and schemas: `subLayers`, `references`,
`payloads`, `UsdPhysics`, and `UsdLux`. For a celestial lesson, author an
explicit epoch together with the celestial payload. For a basic or UI lesson,
author a fixed `DistantLight` or omit the world. Do not add tutorial-specific
API schemas or repair missing authored facts with runtime defaults.

## Test without a Rust rebuild

Put behavior assertions in `assets/scenarios/tests/<name>.rhai` and use the
production scene-test binary. Keep Rust tests limited to generic scripting,
asset, USD, and lifecycle seams.

```bash
target/debug/luncosim test \
  --scene scenes/tests/tutorial_first_drive.usda --max-ticks 6000
```

`--validate` proves preflight only. Inspect the authored verdict and process
exit code for runtime evidence. Rhai, catalog, and authored USD edits should
be replayed through the already-built production binary whenever possible.
