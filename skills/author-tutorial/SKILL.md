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
  teaching HUD). Reference impls: assets/tutorials/luncosim/first_drive.rhai,
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
`assets/tutorials/luncosim/first_drive.rhai`. Overview: `assets/tutorials/README.md`.

## Layout & the two-step add

```
assets/tutorials/<track>/curriculum.usda   # the track: its lessons, as prims
assets/tutorials/<track>/<name>.rhai       # one lesson's script
assets/tutorials/<track>/<name>.usda       # optional env-only scene (3D lessons)
assets/tutorials/<app>.usda                # the app: which tracks it offers
```

**1. Drop the `.rhai`** (author with the prelude verbs below). It does NOT open
its own world — see below.

**2. Declare the lesson in the track's `curriculum.usda`** — **data, not Rust**.
A lesson is a prim applying `LunCoTutorialAPI` (presentation) and
`LunCoProgramAPI` (behaviour), whose world is a `payload` arc:

```usda
def Scope "FirstDrive" (
    prepend apiSchemas = ["LunCoProgramAPI", "LunCoTutorialAPI"]
    prepend payload = @lunco://tutorials/luncosim/first_drive.usda@
)
{
    uniform asset info:sourceAsset = @lunco://tutorials/luncosim/first_drive.rhai@
    string lunco:tutorial:title = "First Drive"
    string lunco:tutorial:blurb = "Take control of a rover and drive it to a flag."
    uniform token lunco:tutorial:difficulty = "beginner"
    uniform bool lunco:tutorial:firstStart = false      # true = onboarding entry
    rel lunco:tutorial:next = </Sandbox/LanderMission>  # omit to end the chain
}
```

The lesson's IDENTITY is its prim path, so `next` is a real relationship rather
than an id string that nothing checks.

**DECLARE THE WORLD, NEVER OPEN IT.** The launcher mounts the `payload` through
`LoadScene` before running the script. A lesson used to call `load_scene(...)` as
the first statement of `on_start`, which made a lesson that HAS no world (a UI
tour) indistinguishable from one that forgot — and lessons that share a world
would reload it on every switch. Omitting the payload is a statement: this lesson
leaves the viewport alone.

**A new TRACK** is a new `curriculum.usda` with a `LunCoTutorialTrackAPI` prim
(`string lunco:track:label = "…"`) — and it appears in an app only when that
app's `assets/tutorials/<app>.usda` sublayers it. Sublayer order is menu order.
Nothing in a track names an app: which app shows it is composition.

**Prerequisite (once per app):** the host app includes the scripting runtime
(`LunCoScriptingPlugin`) + `lunco_tutorial::TutorialCorePlugin { app: "<app>".into() }`.
Add `TutorialPlugin` as well when the host provides the optional workbench UI,
and have the host call `lunco_tutorial::consult_boot(world, has_scene_arg, automated)` at startup
for first-run onboarding. `luncosim` and `lunica` have this; a bare app does not.
Adding *lessons* after that never touches Rust — the curriculum layer + a `.rhai`.

That's it. `StartTutorial{id}` mounts the declared world, then loads the script —
**disk on native** (edit + replay, no rebuild) / **embedded on wasm** — and runs
it. F1 (`EditorIntent::ShowTutorial`) and the 🎓 Tutorials panel also launch it.

## Two kinds of lesson

- **Coach-mark tour** (narrated slideshow) — `coach(i, len, anchor, title, body)`
  in `on_start`, advanced by an `on_event` cursor on `cmd:TutorialNext` /
  `cmd:TutorialBack` / `cmd:TutorialSkip` (the card's own buttons). **Guaranteed
  completable** — it depends on nothing in the scene, so it's the safe default for
  teaching *concepts* and UI. End by `emit("MISSION_COMPLETE", 0)`. Reference:
  `assets/tutorials/luncosim/sandbox_intro.rhai`.
- **Objective mission** — `mission(me)` with objectives that advance on **real
  user actions** (a `cmd:*` event or a `done` predicate). Best for *doing*
  (drive, land). Only gate on events you've confirmed fire — `cmd:PossessVessel`
  and trigger-zone `enter:` events + `done` distance predicates are proven;
  don't assume an arbitrary UI click emits a `cmd:*`. Reference: `first_drive.rhai`.

## The shape of an objective lesson

```rhai
fn on_start(me) {
    load_scene("tutorials/luncosim/first_drive.usda");   // or cmd("OpenClass", #{qualified}) for a model lesson
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
- **A tutorial can have BOTH a `mission` tracker and a `task` behaviour** — e.g. an
  autopilot (`if !is_unattended() { return; }`) that auto-plays the lesson for CI
  while a student plays it by hand. Keep the conditional in the scenario, not Rust.
  Gate it on `is_unattended()` (no window ⇒ nobody can click) and never on the
  build profile: every `cargo run` is a debug build, so a `cfg!(debug_assertions)`
  gate makes the lesson play itself in front of the student.
- **Native edits are live** — `tutorial_source` reads from disk, so edit the
  `.rhai` and re-`StartTutorial` to see changes; no rebuild.
- **3D lesson needs a world** → ship an env-only `.usda` next to it and
  `load_scene` it in `on_start`; a model lesson just `cmd("OpenClass", …)`.

## Verify

Launch the app with `--api` (per [`test-via-api`](../test-via-api/SKILL.md)),
`StartTutorial {id}`, then drive the objective's real action (or set
`LUNCO_SCENARIO_UNATTENDED=1` and let the autopilot play it, even with a window
open) and confirm the HUD ticks + `MISSION_COMPLETE` fires. Read
live objective state via [`inspect-simulation`](../inspect-simulation/SKILL.md).
