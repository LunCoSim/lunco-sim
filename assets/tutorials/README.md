# Tutorials (data-driven, Rhai-scripted)

A tutorial is **data**, not Rust. Each tutorial is a folder here with:

```
first_drive/
  first_drive.usda   # the scene: environment, entities, and (via
                     #   lunco:scriptPath) the orchestrator attachment
  first_drive.rhai   # the orchestrator: a mission(me) that drives the HUD
                     #   and advances on real user actions
```

The orchestrator uses the ordinary scripting substrate (`crates/lunco-scripting`)
— nothing tutorial-specific in the language:

- **Persistent HUD** (rendered by `lunco-workbench`'s `tutorial_overlay`, on in
  both sandbox and lunica):
  - `hint("…")` / `clear_hint()` — one-line instruction that stays until changed.
  - `objectives_hud([#{text, state}])` — a checklist; or just declare a
    `mission(me)` and the engine auto-publishes its objectives.
  - `spotlight("anchor_key", "caption")` / `clear_spotlight()` — dim the screen
    and ring a workbench widget (keys come from `HelpAnchors`).
  - `notify_kind("…", "success")` — transient toast (from before).
- **Dynamic advancement** — a step completes when the student *does the thing*:
  - `requires_event: "cmd:PossessVessel"` — any command dispatch lands on the bus
    as `cmd:<Name>`, so *any* UI/API action can advance a step.
  - `requires_event: "enter:<zone>"` — trigger-zone entry (USD `lunco:triggerZone`).
  - `done: |m| distance(find("/Path/A"), find("/Path/B")) < 6.0` — any world-state
    predicate; `dwell`/`fail`/`requires` supported.
- **Progressive fidelity** — `set_subsystem("thermal", true)` ramps subsystems
  one at a time (allow-list in `lunco-core::subsystems`).

## Registering a tutorial in the launcher

The Tutorials panel (side browser) lists tutorials from `TutorialRegistry`.
Built-ins are registered in `crates/lunco-tutorial/src/lib.rs`
(`builtin_tutorials()`); add an entry pointing at your scene:

```rust
TutorialMeta {
    id: "first-drive",
    title: "First Drive",
    blurb: "…",
    app: "sandbox",          // "sandbox" | "lunica" | "any"
    difficulty: "beginner",
    scene: "tutorials/first_drive/first_drive.usda",
}
```

Or at app build time: `app.world_mut().resource_mut::<TutorialRegistry>()
.register_tutorial(meta)`.

`StartTutorial { id }` loads the scene (which auto-attaches the orchestrator via
`lunco:scriptPath`); `MISSION_COMPLETE` marks the tutorial done in the persisted
`TutorialProgress`.

## Example

See `first_drive/` for the minimal, fully-working reference: possess a rover
(advances on `cmd:PossessVessel`) → drive to a flag (advances on
`enter:waypoint`) → `MISSION_COMPLETE`.
