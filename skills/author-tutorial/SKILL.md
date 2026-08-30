---
name: author-tutorial
description: >
  Author an interactive tutorial, guided lesson, onboarding flow, coach-mark
  tour, or objectives checklist in LunCoSim. USE THIS SKILL for requests such as
  "teach X", "walk a user through Y", "add first-run onboarding", "spotlight
  this button", or "advance an objective after a real action". For the agent
  mid-code: `mission`, `objective`, `coach_step`, `hint`, `spotlight`,
  `requires_event:"cmd:*"`, `StartTutorial`, `TutorialProgress`, or a file under
  `assets/tutorials/`. A lesson is one curriculum USD prim with a Rhai script
  and optional payload; objectives use real events/state, never timers. Adding a
  lesson is data plus Rhai, not Rust. Reference `assets/tutorials/sandbox/` and
  `specs/011-interactive-tutorials/`.
---

# Authoring tutorials

**A lesson is one curriculum declaration.** Its USD prim supplies the `.rhai`
script and, when needed, a world `payload`. The shared launcher
(`lunco-tutorial`) mounts that payload through the scene lifecycle, waits for
completion, and then runs the script on a host entity via
`RunScenario`/`StartTutorial`. The coach card / spotlight / objectives come from
the shared HUD + the rhai prelude — **no Rust per lesson.**

This is [`author-scenario`](../author-scenario/SKILL.md) plus a teaching HUD —
read that first for the `me`/`this` callback contract and verbs. Reference lesson:
`assets/tutorials/sandbox/first_drive.rhai`. Overview: `assets/tutorials/README.md`.

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
    prepend payload = @lunco://tutorials/sandbox/first_drive.usda@
)
{
    uniform asset info:sourceAsset = @lunco://tutorials/sandbox/first_drive.rhai@
    string lunco:tutorial:title = "First Drive"
    string lunco:tutorial:blurb = "Take control of a rover and drive it to a flag."
    uniform token lunco:tutorial:difficulty = "beginner"
    uniform bool lunco:tutorial:firstStart = false      # true = onboarding entry
    rel lunco:tutorial:next = </Sandbox/LanderMission>  # omit to end the chain
}
```

The lesson's IDENTITY is its prim path, so `next` is a real relationship rather
than an id string that nothing checks.

Choose the payload's lighting/time contract before writing lesson policy. Use a
fixed authored `DistantLight` and omit `LunCoEpochAPI`/`SolarSystem` for UI,
onboarding, and basic-control lessons; use `LunCoEpochAPI` with an explicit
`lunco:time:epochJd` plus the `SolarSystem` reference for astronomy-dependent
lessons; reuse an existing scene when scenery is not the subject; and omit the
payload for a UI-only lesson. The selection and current corpus are maintained
in [`assets/tutorials/README.md`](../../assets/tutorials/README.md) and the
scene-building workflow in [`build-usd-scene`](../build-usd-scene/SKILL.md).
An applied epoch API without its authored epoch is rejected by the USD rule
`epoch-api-missing-time`.

**DECLARE THE WORLD, NEVER OPEN IT.** The launcher mounts the `payload` through
`LoadScene` before running the script. The launcher owns this boundary, which
means a lesson that HAS no world is intentionally UI-only, not a second scene
loader. When a UI-only lesson follows a world-owning lesson, the launcher clears
the outgoing scene before starting the UI-only host; the workbench keeps its
normal empty-viewport presentation visible while the tutorial card is shown.
Omitting the payload is a statement that the lesson itself has no visual world
to mount.

**A new TRACK** is a new `curriculum.usda` with a `LunCoTutorialTrackAPI` prim
(`string lunco:track:label = "…"`) — and it appears in an app only when that
app's `assets/tutorials/<app>.usda` sublayers it. Sublayer order is menu order.
Nothing in a track names an app: which app shows it is composition.

**Prerequisite (once per app):** the host app includes the scripting runtime
(`LunCoScriptingPlugin`) + `lunco_tutorial::TutorialCorePlugin { app: "<app>".into() }`.
Add `TutorialPlugin` for the launcher UI; it installs the Workbench owner and
its `HelpAnchors` dependency when the host has not already done so. Have the
host call `lunco_tutorial::consult_boot(world, has_scene_arg, automated)` at startup
for first-run onboarding. `luncosim` and `lunica` have this; a bare app does not.
Adding *lessons* after that never touches Rust — the curriculum layer + a `.rhai`.

That's it. `StartTutorial{id}` mounts the declared world, then loads the script —
**disk on native** (edit + replay, no rebuild) / **embedded on wasm** — and runs
it. The 🎓 Tutorials panel and the host's configured tutorial entry point launch
the same command.

## Two kinds of lesson

- **Coach-mark tour** (narrated slideshow) — `coach_step(steps, i)` (or
  `coach(...)`) in `on_start`, advanced by an `on_event` cursor. A tour may use
  the card's `cmd:TutorialNext` / `cmd:TutorialBack` / `cmd:TutorialSkip` events,
  or authored semantic action requirements such as `cmd:SpawnEntity`.
  **Guaranteed completable** means that every required action is documented and
  observable; it does not mean that every step must accept Next. End by
  `emit("MISSION_COMPLETE", 0)`. Reference:
  `assets/tutorials/sandbox/sandbox_intro.rhai`.
- **Objective mission** — `mission(me)` with objectives that advance on **real
  user actions** (a `cmd:*` event or a `done` predicate). Best for *doing*
  (drive, land). Only gate on events you've confirmed fire — `cmd:PossessVessel`
  and trigger-zone `enter:` events + `done` distance predicates are proven;
  don't assume an arbitrary UI click emits a `cmd:*`. Reference: `first_drive.rhai`.

## The shape of an objective lesson

```rhai
fn on_start(me) {
    hint("Welcome! Let's drive a rover on the Moon.");
    notify_kind("Tutorial: First Drive", "info");
}

fn mission(me) {
    let rover = "/FirstDrive/Rover";                     // scene paths as LOCALS (see gotcha)
    let flag  = "/FirstDrive/Flag";
    [
        objective("possess", #{
            text: "Select the rover to take control",
            requires_event: "cmd:PossessVessel",         // advances on a REAL action
            on_complete: |m| hint("Now use " + input_hint("forward") + ", " + input_hint("left") + ", " + input_hint("backward") + ", and " + input_hint("right") + " to drive to the flag."),
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

**Dynamic controls:** `input_binding(name)` returns the resolved user-facing
label or `()` for an explicitly unbound intent. `input_hint(name)` is the copy
helper that renders that state as `unbound`; it never selects a key or drives
the simulation. Use these for labels, and gate progress on semantic commands
or authoritative state rather than raw key names. The settings owner is the
`input_bindings` section in `<OS config dir>/lunco/settings.json`.

**Advancing objectives — always on a real action, never a timer:**
- `requires_event: "cmd:<Name>"` — any command dispatch lands on the bus as
  `cmd:<Name>` (e.g. `cmd:PossessVessel`), so the step completes however the user
  triggers it (click or key). For waypoint missions consume the canonical
  `waypoint.reached` event; the lower-level Sensor/zone notification is an
  engine detail and is projected before tutorial policy sees it.
- `done: |m| <predicate>` — a rhai closure over live state (distance, a port
  read, SoC). Use for "reached / held / value crossed".

**Spotlight anchors:** a widget's `HelpAnchors` key; `focus_panel(id)` opens the
singleton panel on an interactive host, and `coach_step` uses it before the
spotlight. Unattended gates omit this presentation-only command. lunica ids include `modelica_experiments`, `modelica_inspector`,
`modelica_diagnostics`, `modelica_component_palette`,
`model_view.compile_buttons` (needs a model open); instance panel
`panel.modelica_plot` — spotlight but don't `focus`. Full list: `assets/tutorials/lunica/README.md`.

For luncosim, use `panel.center`, `panel.side_browser`,
`panel.right_inspector`, and `panel.bottom` for workbench docks. File → Network
and the other title-bar menus expose `menu.network`, `menu.time`, `menu.help`,
and `toolbar.run`; perspective tabs use
`menu.perspective.<registered-perspective-id>`, such as
`menu.perspective.rover_build`. An empty viewport keeps its normal presentation
visible while a tour card is shown, so UI-only lessons do not need an opaque
scene scrim. Do not invent panel-instance anchors.

## Test a tutorial in Rhai

Tutorial runtime tests belong beside the authored assets, not in a Rust test
per lesson:

```
assets/scenes/tests/<lesson>.usda       # the real lesson rig
assets/scenarios/tests/<lesson>.rhai    # an observing verdict scenario
```

The test scenario attaches to the same composed world, observes the lesson's
public `cmd:*`, `MISSION_COMPLETE`, `MISSION_FAILED`, and live state, then calls
the shared `report_verdict(...)` helper. It must not drive the lesson with a
second autopilot or accept `MISSION_COMPLETE` without checking the mechanism
that was supposed to teach something. A timeout is allowed only as test
liveness handling; it is not lesson policy. If the public Rhai read surface can
observe the regression, put the assertion in this authored observer and run the
production scene-test binary. Keep Rust coverage for generic engine mechanisms
or seams that Rhai cannot read.

For example, a First Drive observer should count `cmd:PossessVessel` and
`cmd:SetPorts`, then verify the rover reached the flag. Run it with the
production binary:

```
target/debug/luncosim test \
  --scene scenes/tests/tutorial_first_drive.usda \
  --max-ticks 6000
```

This is the normal edit loop for tutorial behavior: change the `.rhai`, rebuild
nothing, and run the scene gate again. `--validate` and a script parser only
prove syntax; the authored Rhai verdict is what proves the live behavior.

The generic Rust scripting contract may still check that every embedded script
compiles and that the shared hook seam does not panic. It must stay content
agnostic. Do not add a new Rust test just to encode a lesson's steps, action
requirements, or expected command sequence.

For a lesson that teaches model attachment, use the production Rhai gate
`assets/scenes/tests/program_attach_command.usda` plus
`assets/scenarios/tests/program_attach_command.rhai` as the pattern. It issues
`AttachProgram`, waits for the USD child, checks `ListPorts`, and requires the
participant in `CosimStatus`. Keep this in Rhai so lesson and source-contract
edits do not require recompiling Rust. Use semantic `input_binding(...)` and
`input_hint(...)` for interactive follow-up; never bake physical key names into
the lesson.

## Onboarding (first-run)

- `first_start: true` marks the once-only entry. The `boot.entry` rhai policy
  hook (`consult_boot`) decides first-run → show the tutorial instead of the
  default scene — onboarding is **policy, not Rust**.
- `TutorialProgress` (in `lunco-settings`) persists `onboarded` + per-tutorial
  completion + `autoproceed`; `SkipTutorial` opts out; `next` chains lessons.

## Gotchas

- **Task leaves use anonymous closures.** Write `once(|me| action(me))`,
  `step(|me| action(me), |me| done(me))`, and `wait_until(|me| done(me))`.
  `me` is the host entity id and `this` is persistent scenario state. Named
  `Fn("...")` pointers are not task leaves; named helpers may be called from
  an anonymous closure.

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
- **3D lesson needs a world** → ship an env-only `.usda` next to it and declare
  it as the lesson prim's `payload`; a model lesson can use its authored command
  surface from the script.

## Autopilot and test closure

An unattended tutorial is a policy variant of the human lesson, not a second
vehicle-control path. Drive through `PossessVessel`, the authored
`ControlBinding`/intent map, and the same `SetPorts` or live control stream a
person uses. Do not move entities by writing `Position`, `LinearVelocity`,
`ModelicaModel.inputs`, or private actuator state.

Test the path at its boundaries: observe `cmd:PossessVessel`, the port-write
event, a live movement/port predicate, and the final objective. A
`MISSION_COMPLETE` event without those checks can be a false green. The full
scene contract is documented in
[`tutorial-autopilot-and-port-contracts`](../../docs/architecture/tutorial-autopilot-and-port-contracts.md).

## Verify

Launch the app with `--api` (per [`test-via-api`](../test-via-api/SKILL.md)),
`StartTutorial {id}`, then drive the objective's real action (or set
`LUNCO_SCENARIO_UNATTENDED=1` and let the autopilot play it, even with a window
open) and confirm the HUD ticks + `MISSION_COMPLETE` fires. For acceptance,
build and run the production `target/debug/luncosim test` scene gate as well;
parsing or a queued API command is not runtime proof. Read live objective state
via [`inspect-simulation`](../inspect-simulation/SKILL.md).
