---
name: author-tutorial
description: >
  How to author an interactive tutorial / guided lesson / onboarding flow in
  LunCoSim. USE THIS SKILL whenever the user asks, in plain words, things like:
  "make a tutorial that teaches X", "add a guided lesson for the rover / the
  Modelica workbench", "walk a new user through Y step by step", "add an
  onboarding flow / first-run experience", "spotlight this button and explain
  it", or "add an objectives checklist that advances as the user does things".
  Any request to teach a user how to do something in-app, guided, belongs here.
  (For the agent mid-code: a `mission(me)` / `objective(...)`, `coach_step`,
  `hint` / `spotlight`, `requires_event:"cmd:*"`, `register_tutorial`,
  `StartTutorial`, `TutorialProgress`, or a file under `assets/tutorials/`.)
  Project-specific and non-obvious: a tutorial IS a single `.rhai` scenario (no
  scene-vs-script split), objectives advance on REAL user actions (a `cmd:*` bus
  event or a `done` predicate — never a timer), the HUD auto-publishes from
  `mission(me)`, and adding one is two steps (drop a `.rhai`, register a row) —
  no Rust per lesson. Builds on author-scenario (a tutorial is a scenario with a
  teaching HUD). Reference impls: assets/tutorials/sandbox/first_drive.rhai,
  assets/tutorials/lunica/*.rhai. Design: specs/011-interactive-tutorials/.
---

# Authoring tutorials

**A tutorial is one thing: a `.rhai` scenario.** There is no scene-vs-script
split. The shared launcher (`lunco-tutorial`) runs it on a host entity via
`RunScenario`/`StartTutorial`; the scenario sets up its own environment in
`on_start`. The coach card / spotlight / objectives come from the shared HUD +
the rhai prelude — **no Rust per lesson.**

This is [`author-scenario`](../author-scenario/SKILL.md) plus a teaching HUD —
read that first for hooks, `this`-state, and verbs. Reference lesson:
`assets/tutorials/sandbox/first_drive.rhai`. Overview: `assets/tutorials/README.md`.

## Layout & the two-step add

```
assets/tutorials/<app>/<name>.rhai        # the lesson (app = "sandbox" | "lunica" | …)
assets/tutorials/<app>/<name>.usda        # optional env-only scene, load_scene'd (3D lessons)
```

**1. Drop the `.rhai`** (author with the prelude verbs below).
**2. Declare the catalog entry** — **data, not Rust**. Two homes, pick by whether
the lesson has a scene:

**(a) JSON manifest** `assets/tutorials/<app>/tutorials.json` — the default,
required for **scene-less** lessons (coach tours, model lessons).
`TutorialPlugin { app }` scans it at startup. Note: strict JSON, **no comments**.

```json
{
  "id": "first-drive", "title": "First Drive",
  "blurb": "Take control of a rover and drive it to a flag.",
  "app": "sandbox", "difficulty": "beginner",
  "script": "sandbox/first_drive.rhai",
  "first_start": false,
  "next": "lander-mission"
}
```

**(b) On the scene (hybrid)** — a **scene-backed** 3D lesson may instead declare
its catalog entry on its own `.usda`, as `lunco:tutorial*` on the default prim,
so the file that IS the environment doubles as the catalog row (single source of
truth). `lunco_usd_bevy::tutorial_scene_metas(app)` scans `<app>/*.usda`, and the
launcher merges these with the JSON manifest (idempotent on `id`, ordered by the
`next` chain). **Presence of `lunco:tutorialId` = this scene is a tutorial**; omit
it and the `.usda` is just an environment. Don't also add a JSON row for the same
`id`. (Wiring is per-app + USD-only, so lunica/luncosim — which have no scene
lessons — never pull in `openusd`; sandbox does the scan.)

```usda
def Xform "FirstDrive"
{
    custom string lunco:tutorialId = "first-drive"
    custom string lunco:tutorialTitle = "First Drive"
    custom string lunco:tutorialBlurb = "Take control of a rover and drive it to a flag."
    custom string lunco:tutorialDifficulty = "beginner"
    custom string lunco:tutorialScript = "sandbox/first_drive.rhai"
    custom string lunco:tutorialNext = "lander-mission"   # omit for end-of-chain
    # … the lesson environment (ground, rover, flag, lights) …
}
```

**Prerequisite (once per app):** the host app includes the scripting runtime
(`LunCoScriptingPlugin`) + `lunco_tutorial::TutorialPlugin { app: "<app>".into() }`,
and calls `lunco_tutorial::consult_boot(world, has_scene_arg, automated)` at startup
for first-run onboarding. `sandbox` and `lunica` have this; a bare app does not.
Adding *lessons* after that never touches Rust — just the manifest + a `.rhai`.

That's it. `StartTutorial{id}` loads the source via `tutorial_source(script)` —
**disk on native** (edit + replay, no rebuild) / **embedded on wasm** — and runs
it. F1 (`EditorIntent::ShowTutorial`) and the 🎓 Tutorials panel also launch it.

## Two kinds of lesson

- **Coach-mark tour** (narrated slideshow) — `coach(i, len, anchor, title, body)`
  in `on_start`, advanced by an `on_event` cursor on `cmd:TutorialNext` /
  `cmd:TutorialBack` / `cmd:TutorialSkip` (the card's own buttons). **Guaranteed
  completable** — it depends on nothing in the scene, so it's the safe default for
  teaching *concepts* and UI. End by `emit("MISSION_COMPLETE", 0)`. Reference:
  `assets/tutorials/sandbox/sandbox_intro.rhai`.
- **Objective mission** — `mission(me)` with objectives that advance on **real
  user actions** (a `cmd:*` event or a `done` predicate). Best for *doing*
  (drive, land). Only gate on events you've confirmed fire — `cmd:PossessVessel`
  and trigger-zone `enter:` events + `done` distance predicates are proven;
  don't assume an arbitrary UI click emits a `cmd:*`. Reference: `first_drive.rhai`.

## The shape of an objective lesson

```rhai
fn on_start(me) {
    load_scene("tutorials/sandbox/first_drive.usda");   // or cmd("OpenClass", #{qualified}) for a model lesson
    hint("Welcome! Let's drive a rover on the Moon.");
    notify_kind("Tutorial: First Drive", "info");
}

fn mission(me) {
    let rover = "/FirstDrive/Rover";                     // scene paths as LOCALS (see gotcha)
    let flag  = "/FirstDrive/Flag";
    [
        objective("possess", #{
            text: "Click the rover (or press F) to take control",
            requires_event: "cmd:PossessVessel",         // advances on a REAL action
            on_complete: |m| hint("Now use W/A/S/D to drive to the flag."),
        }),
        objective("reach_flag", #{
            text: "Drive to the glowing flag",
            requires: ["possess"],                       // gated on step 1
            done: |m| { let d = distance(find(rover), find(flag)); d >= 0.0 && d < 6.0 },
            dwell: 0.4,                                   // must hold 0.4s (no fly-through blip)
            on_complete: |m| notify_kind("Nice driving!", "success"),
        }),
    ]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" {                  // engine emits when all objectives done
        hint("Tutorial complete! Pick another lesson from the Tutorials panel.");
    }
}
```

`mission(me)` is **auto-published** to the objectives HUD — you don't render it.
The engine tracks `requires`/`requires_event`/`done`/`dwell`, fires `on_complete`,
and emits `MISSION_COMPLETE`.

## Teaching HUD verbs (prelude `hud.rhai`)

| Verb | Effect |
|---|---|
| `hint(msg)` / `clear_hint()` | sticky instruction line |
| `spotlight(anchor, caption)` / `clear_spotlight()` | dim the screen + ring a workbench widget by its `HelpAnchors` key |
| `coach_step(steps, i)` | a guided coach-mark tour step — advance the cursor `i` in `on_event` |
| `objectives_hud(list)` | manual checklist (or just declare `mission(me)` and let it auto-publish) |
| `notify_kind(msg, "info"\|"warn"\|"error"\|"success")` | toast |

**Advancing objectives — always on a real action, never a timer:**
- `requires_event: "cmd:<Name>"` — any command dispatch lands on the bus as
  `cmd:<Name>` (e.g. `cmd:PossessVessel`), so the step completes however the user
  triggers it (click or key). Physics/zone events work too (`enter:waypoint`).
- `done: |m| <predicate>` — a rhai closure over live state (distance, a port
  read, SoC). Use for "reached / held / value crossed".

**Spotlight anchors:** a widget's `HelpAnchors` key; `focus` opens the panel
first. lunica ids include `modelica_experiments`, `modelica_inspector`,
`modelica_diagnostics`, `modelica_component_palette`,
`model_view.compile_buttons` (needs a model open); instance panel
`panel.modelica_plot` — spotlight but don't `focus`. Full list: `assets/tutorials/lunica/README.md`.

## Onboarding (first-run)

- `first_start: true` marks the once-only entry. The `boot.entry` rhai policy
  hook (`consult_boot`) decides first-run → show the tutorial instead of the
  default scene — onboarding is **policy, not Rust**.
- `TutorialProgress` (in `lunco-settings`) persists `onboarded` + per-tutorial
  completion + `autoproceed`; `SkipTutorial` opts out; `next` chains lessons.

## Gotchas

- **Scene paths as LOCALS inside `mission(me)`, not top-level `const`.** rhai
  closures (`done`/`on_complete`) capture enclosing locals by value, but named
  `fn`s can't see module consts — a `const` path is invisible to the closure. Bind
  `let rover = "…"` in `mission` and `find()` it each tick.
- **Objectives never advance on a timer** — use `requires_event`/`done`. A
  timed step teaches nothing and desyncs from the user.
- **A tutorial can have BOTH a `mission` tracker and a `task` behaviour** — e.g. a
  DEBUG autopilot (`if is_debug()`) that auto-plays the lesson for CI while a human
  plays it in release. Keep the conditional in the scenario (`is_debug()`), not Rust.
- **Native edits are live** — `tutorial_source` reads from disk, so edit the
  `.rhai` and re-`StartTutorial` to see changes; no rebuild.
- **3D lesson needs a world** → ship an env-only `.usda` next to it and
  `load_scene` it in `on_start`; a model lesson just `cmd("OpenClass", …)`.

## Verify

Launch the app with `--api` (per [`test-via-api`](../test-via-api/SKILL.md)),
`StartTutorial {id}`, then drive the objective's real action (or rely on the
`is_debug()` autopilot) and confirm the HUD ticks + `MISSION_COMPLETE` fires. Read
live objective state via [`inspect-simulation`](../inspect-simulation/SKILL.md).
