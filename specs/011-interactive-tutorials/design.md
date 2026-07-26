# Design: Rhai-Driven Dynamic Tutorial System

**Status**: Implemented (all six tutorial tasks — see Reference Implementation section below)
**Supersedes the "Partial" status of** [spec.md](./spec.md) — this design fills the missing *objective/goal-evaluation framework* and generalizes tutorials beyond the lunica workbench to the sandbox.

---

## 1. Current State — three disjoint "tutorial" things, none dynamic

| System | Crate | Model | Advancement | Dynamic? |
|---|---|---|---|---|
| **Guided tour** (coachmarks) | `lunco-modelica` `ui/help_overlay.rs` | `const SCREENS: &[HelpScreen]` — 10 hardcoded steps `{title, body, anchor, focus_panel}` | Manual Next/Back/arrows | No — zero state detection |
| **Learning paths** | `lunco-modelica` `ui/welcome.rs` | `const PATHS: &[LearningPath]` — 3 arcs of MSL classes | Click-to-open | Only "opened once" ledger (`welcome_progress.rs`) |
| **Tutor mode** | `lunco-networking` | live screen mirroring tutor→students | n/a | Collaboration, not onboarding |

Findings:
- **No data-driven tutorial engine, no `TutorialStep` state machine, no goal evaluation.** Steps are `const` Rust arrays; advancement is entirely user-driven.
- Everything is **welded to the lunica Modelica workbench**. The sandbox explicitly disables the coachmark overlay (`ModelicaUiConfig { include_help_overlay: false }`, `lunco-sandbox/src/ui/mod.rs:134`).
- Spec 011 Story-1/3 (detect "Rover moved 10 m", evaluate "Reach Tycho with >20% battery") are **unimplemented**.

## 2. Key realization — the Rhai substrate is already a tutorial engine

Everything a *dynamic* tutorial needs already exists in `lunco-scripting`, built for missions. A tutorial **is** a mission that narrates itself:

| Tutorial need | Already provided by Rhai substrate |
|---|---|
| Sequential steps | Task sequencer: `seq([...])`, `once`, `wait`, `repeat`, `forever` (`prelude.rhai:148-237`) |
| **"Advance when the user does X"** | `wait_for("event")`, `wait_for_from("event", src)`, `wait_until(\|m\| cond)` — deterministic frame-delayed event delivery |
| Detect user actions | TelemetryEvent bus: `key:*` (keyboard), `COLLISION_START`, `enter:<zone>`/`exit:<zone>`, Modelica port edges, any script `emit()` |
| Detect ECS/world state | `get(id,"Comp.field")`, `world_pos`, `query("Raycast"/"Nearest"/…)`, `distance`, `arrived`, `nearest_where` |
| **Goal evaluation** ("Reach Tycho >20% battery") | `fn mission(me){ [objective(id,#{requires, dwell, fail, on_complete})] }` → emits `OBJECTIVE_COMPLETE`/`MISSION_COMPLETE` (`prelude.rhai:363-476`) |
| Toggle fidelity / load env (Story 2) | `cmd("LoadScene"/"SetSetting"/any command)` — every `#[Command]` is callable by name |
| Show instructions | `notify(msg)` / `notify_kind(msg, kind)` → `ShowNotification` toast |
| Multi-scenario | Any `.rhai` on any target; `RunScenario` command; a USD `LunCoProgramAPI` prim |

**So the tutorial "logic" moves to Rhai for free.** The gap is not logic — it is (a) a **persistent display surface** (toasts fade; a tutorial needs a sticky objectives panel + spotlight), (b) a **registry/launcher** for selecting among many tutorials with resumable progress, and (c) a few **input/state events** projected onto the bus so steps can react to UI actions.

## 3. Target architecture — "tutorials are Rhai scenarios + a thin HUD"

```
┌────────────────────────────────────────────────────────────┐
│ tutorials/<name>/                (data, no Rust)            │
│   tutorial.usda   ── scene: env, entities, LunCoProgramAPI prim│
│   tutorial.rhai   ── fn mission(me){ steps as objectives }  │
│   meta.toml       ── title, blurb, difficulty, app: sandbox │
│                       |lunica, prerequisites                │
└───────────────┬────────────────────────────────────────────┘
                │ discovered by
┌───────────────▼────────────────────────────────────────────┐
│ lunco-tutorial  (NEW small crate, UI-gated, not in core)   │
│  • TutorialRegistry   scan dirs → [TutorialMeta]           │
│  • TutorialProgress   per-tutorial/per-step, persisted     │
│  • Commands: StartTutorial{id} / NextStep / SkipTutorial   │
│  • TutorialHudPanel    persistent objectives + hint + ▶     │
│  • Spotlight overlay   (lifted from help_overlay, shared)  │
└───────────────┬────────────────────────────────────────────┘
                │ drives via existing bus + commands
┌───────────────▼────────────────────────────────────────────┐
│ lunco-scripting  (EXISTING) — runs the tutorial.rhai:      │
│   objectives/tasks, wait_for(events), cmd(), emit()        │
│   + a few NEW core commands it can call (below)            │
└────────────────────────────────────────────────────────────┘
```

Principle (per spec §"Architectural Separation"): **`lunco-tutorial` is UI-gated and sits on top; the core only gains generic, reusable primitives** (a HUD command, a spotlight command, command-events on the bus). Nothing tutorial-specific enters the sim core, so headless CI pays nothing.

## 4. Gap analysis → concrete core features to add

### P0 — display surface (the real missing piece)

1. **Persistent objectives HUD** — new command in `lunco-avatar` or the new crate, modeled on the `ShowNotification` handler/queue/renderer trio (`lunco-avatar/src/lib.rs:2368-2405`):
   - `SetObjectives { items: [ {id, text, state: pending|active|done|failed} ] }` — sticky panel, not a fading toast.
   - `SetHint { text, secs? }` — persistent instruction line (0 secs = until replaced).
   - Rhai wrappers in prelude: `objectives_hud(list)`, `hint(msg)`, `clear_hint()`.
   - **Wire the existing `mission()`/`objective()` machinery to this HUD.** Today `OBJECTIVE_COMPLETE`/`MISSION_COMPLETE` are headless telemetry only — have the HUD subscribe to those events so declarative missions render for free.

2. **Rhai-driven spotlight** — the `HelpAnchors` rect substrate is *already in the shared `lunco-workbench` crate and live in the sandbox* (`lunco-workbench/src/lib.rs:165-189`). Two moves:
   - Lift the scrim+callout renderer out of `lunco-modelica/ui/help_overlay.rs` into `lunco-tutorial` (or `lunco-workbench`) so both apps share it.
   - New command `Spotlight { anchor: String, text?: String }` / `ClearSpotlight` → sets a target the shared renderer draws. Rhai: `spotlight("twin_browser", "Click here")`.
   - Sandbox panels must `HelpAnchors::set(key, rect)` (workbench panels already have the hook; add keys to the panels a tutorial targets).

### P1 — selection, progress, launch (multi-scenario)

3. **TutorialRegistry + launcher** — scan `assets/tutorials/**/meta.toml` → `[{id,title,blurb,app,difficulty,prereqs}]`. Reuse the `WorkbenchAppExt::register_panel` mechanism for a "Tutorials" panel (a real, dockable, data-driven replacement for the hardcoded `WelcomePanel` PATHS). `StartTutorial{id}` = `LoadScene(tutorial.usda)` + `RunScenario(tutorial.rhai on the orchestrator prim)`.

4. **TutorialProgress persistence** — generalize `welcome_progress.rs` from "class opened count" to `{tutorial_id → {step_id → done, last_step}}` via `lunco-settings`. Enables resume + progress dots. Script reads/writes it through `get_setting`/`set_setting` (already exposed to Rhai).

### P2 — richer reactivity (make "detect user did X" cover UI actions)

5. **Project command dispatch onto the bus** — use the existing `ScriptEventAppExt::project_events` registrar to emit a `cmd:<CommandName>` TelemetryEvent whenever a command runs. Then a step can `wait_for("cmd:SpawnEntity")` — i.e. "advance when the user spawns something." This is the single highest-leverage addition: it turns *every UI action* into a tutorial trigger with no per-action code.
   - Same pattern already used for `KeyboardInput` (`lunco-avatar/src/lib.rs:486`). One registrar over `ApiCommandEvent`.

6. **Toggle-plugin / fidelity ramp (Story 2)** — expose a `SetSubsystemEnabled { name, on }` command over a small allow-list (thermal, comms-degradation, obstacle-field). Rhai then ramps fidelity per step: `cmd("SetSubsystemEnabled", #{name:"thermal", on:true})`.

### Non-goals / already-covered
- Goal-eval framework — **already exists** as `mission()`/`objective()`; P0.1 just renders it.
- Step advancement logic — **already exists** as the task sequencer; no new engine.

## 5. Authoring model — a tutorial is one `.rhai` file

```rhai
// assets/tutorials/first_drive/first_drive.rhai  — attached to /Tutorial/Orchestrator
fn mission(me) {
  hint("Welcome. Let's drive a rover on the Moon.");
  [
    objective("possess", #{
      text: "Press F to take control of the rover",
      requires: || false,                       // gated by event instead:
      on_start: || spotlight("rover", "This is your rover"),
      requires_event: "cmd:PossessVessel",      // advance when user possesses
      on_complete: || { clear_spotlight(); hint("Use WASD to drive to the flag."); }
    }),
    objective("reach_flag", #{
      text: "Drive to the flagged waypoint",
      requires: || distance(me, find("/Tutorial/Flag")) < 5.0,   // ECS-state goal
      dwell: 1.0,
      on_complete: || notify_kind("Nice driving!", "success")
    }),
    objective("battery", #{                       // Story-3 style goal-eval
      text: "Return to base with >20% battery",
      requires: || arrived(me, "/Tutorial/Base") && get(me,"battery.soc") > 0.2,
      fail: || get(me,"battery.soc") <= 0.05,
      on_complete: || notify_kind("Tutorial complete!", "success")
    }),
  ]
}
```

The engine already drives `mission(me)`, tracks dwell/fail/complete, and emits the events; P0.1 makes the objectives + hints show on screen; P2.5 makes `requires_event: "cmd:*"` possible. **No Rust per tutorial.** New tutorials = new folder.

## 6. Migration path for the existing lunica tour

- Rewrite the 10 `SCREENS` as a data tutorial `assets/tutorials/lunica_tour/` whose steps `spotlight(anchor)` + `hint(body)` + optional `cmd("FocusPanel",…)`, advancing on `wait_for("cmd:OpenClass")` etc. instead of Next-button-only (or keep a manual "Next" step via a `▶` button that emits `cmd:NextStep`).
- Delete `const SCREENS`/`const PATHS`; `WelcomePanel` learning-paths become registry entries. `help_overlay.rs` shrinks to the shared spotlight renderer.
- Net: one engine, two apps, N tutorials, all data.

## 7. Task breakdown (suggested order)

1. **P0.1** `SetObjectives`/`SetHint` commands + persistent HUD panel (copy `ShowNotification` trio) + prelude wrappers; subscribe HUD to existing `OBJECTIVE_COMPLETE`/`MISSION_COMPLETE`. *(unblocks visible dynamic tutorials immediately)*
2. **P2.5** project `ApiCommandEvent` → `cmd:<Name>` bus events (one registrar) + `requires_event` support in `objective()`. *(turns UI actions into triggers)*
3. **P0.2** lift spotlight renderer into shared crate + `Spotlight`/`ClearSpotlight` commands + publish `HelpAnchors` keys on sandbox panels.
4. **P1.3/P1.4** `lunco-tutorial` crate: registry scan, `Tutorials` panel, `StartTutorial`/`SkipTutorial`, generalized progress persistence.
5. **P2.6** `SetSubsystemEnabled` allow-list for fidelity ramps.
6. **Migration**: convert lunica tour + learning paths to data tutorials; delete hardcoded arrays.

## Reference Implementation & Outcomes

All six tasks are implemented. Deviations from the plan are noted inline.

- **P0.1 Objectives/Hint HUD** — `crates/lunco-workbench/src/tutorial_overlay.rs`.
  `TutorialHud` resource + commands `SetHint` / `SetObjectives` (single-string
  payloads — objectives arrive pre-formatted from the prelude, avoiding nested
  reflection) + a persistent top-left egui card. Prelude wrappers `hint`,
  `clear_hint`, `objectives_hud`, `clear_objectives`. **The declarative
  `mission(me)` now auto-publishes its checklist**: `__run_mission` formats the
  objectives and calls `SetObjectives` on change, so any mission renders on screen
  for free. *Deviation:* HUD lives in `lunco-workbench` (not `lunco-avatar`) — it's
  the crate both apps load and already hosts a command+overlay (`perf_hud`).
- **P0.2 Spotlight** — same module. Commands `Spotlight` / `ClearSpotlight` drive a
  scrim-cutout + pulsing ring + caption over a `HelpAnchors` rect. Prelude
  `spotlight` / `clear_spotlight`. *Deviation:* reimplemented compactly (~120 lines)
  rather than lifting the 600-line modelica tour renderer (too coupled to its
  Next/Back state machine).
- **P2.5 `cmd:*` bus events + `requires_event`** — `crates/lunco-api/src/executor.rs`
  `project_command_events` observer fires `cmd:<Name>` on the TelemetryEvent bus for
  every dispatched command. Prelude `objective()` gained `requires_event` (latched)
  and `text`; an independent `this.__mevents` buffer feeds missions so event-gated
  objectives work with or without a running task. *Deviation:* an observer (not
  `project_events`, which needs a buffered Message) since `ApiCommandEvent` is
  observer-triggered.
- **P1 `lunco-tutorial` crate** — registry + `TutorialsPanel` (side-browser, via
  `register_panel`) + `StartTutorial`/`SkipTutorial` + `TutorialProgress`
  (persisted via `lunco-settings`, marked on `MISSION_COMPLETE`). `StartTutorial`
  dispatches `LoadScene` through `ApiCommandEvent` (no dep on the scene-load crate).
  Wired into the sandbox behind the `ui` feature. *Deviation:* the catalog is
  code-registered (`builtin_tutorials()` + `register_tutorial`) rather than
  filesystem-scanned — identical on native/packaged/wasm; a native `meta.toml` scan
  can augment it later. The tutorial *content* is still data (`.usda` + `.rhai`).
- **P2.6 `SetSubsystemEnabled`** — `SubsystemToggles` resource + allow-list live in
  `lunco-core::subsystems`; the command lives in `lunco-tutorial` (the `#[Command]`
  derive can't expand inside `lunco-core` itself). Prelude `set_subsystem(name, on)`.
  Opt-in gating: subsystems read `SubsystemToggles::enabled(name)` (defaults true).
- **Data tutorial** — `assets/tutorials/first_drive/` (`.usda` + `.rhai` + this dir's
  `README.md`): possess (advances on `cmd:PossessVessel`) → drive to flag (advances
  on `enter:waypoint`) → `MISSION_COMPLETE`. Second built-in reuses the existing
  `scenes/sandbox/lander_ops.usda`.

**Not done (noted follow-up):** the hardcoded lunica `SCREENS`/`PATHS` in
`lunco-modelica` are left intact — ripping out a working shipped tour without
runtime testing is riskier than the value here. The new data-driven layer runs
alongside; converting the lunica tour to a `lunica_tour` data tutorial (spotlighting
existing `HelpAnchors` keys) is a clean, isolated follow-up.

### Why this is cheap
- Step logic, event waiting, goal evaluation, command dispatch, world reads, scene loading, USD script attachment — **all already exist**.
- New Rust ≈ 2 HUD commands + 2 spotlight commands + 1 event registrar + 1 small UI-gated crate. Everything else is Rhai + USD + TOML data.

---

## 8. A lesson's ENVIRONMENT must be declared, not called (2026-07-26)

### The defect that forced this

A lesson set up its own world by calling `load_scene(...)` as the first statement
of `on_start`. Nothing checked that it did. `sandbox/rhai_repl.rhai` did not, so
opening it drew coach marks over whatever scene happened to be loaded — the
symptom being "the tutorial doesn't load its scenario, it just overlays the
current one". A sweep of every shipped lesson found exactly one other shape:
`lunica/*` lessons legitimately have **no** world at all, so "every lesson calls
`load_scene`" is not even a valid rule to enforce.

The problem is not the file format. It is that the environment is an imperative
side effect of running the lesson, so its absence is unrepresentable and
therefore unverifiable. Two further consequences, both observed live:

- The lesson's own `load_scene` RACES its own HUD publication — the scene mount
  and the first `hint()` land in one `on_start`. Clearing stale overlay state on
  a scene change has to happen synchronously on the `LoadScene` trigger, or it
  wipes the incoming lesson's hint instead of the outgoing lesson's.
- Chaining a lesson whose successor uses the SAME `.usda` takes the
  `already loaded — no-op` path (correct), so the world visibly does not change
  while the avatar/camera is re-derived — which reads to a user as a bug.

### Decision

**The lesson prim declares its world as a `payload`; the launcher mounts it
before running the scenario.** A lesson with no payload is *declaring* itself a
UI tour. A lesson that meant to have a world and lacks one is a lint finding, not
a student's discovery.

```usda
def Scope "Sandbox" ( prepend apiSchemas = ["LunCoTutorialTrackAPI"] ) {
    string lunco:track:label = "1️⃣ Sandbox Onboarding"

    def Scope "FirstDrive" (
        prepend apiSchemas = ["LunCoProgramAPI", "LunCoTutorialAPI"]
        prepend payload = @./first_drive.usda@      # deferred by construction
    ) {
        asset  info:sourceAsset     = @./first_drive.rhai@
        string lunco:tutorial:title = "First Drive"
        rel    lunco:tutorial:next  = </Sandbox/SandboxIntro>
    }
}
```

Why each part is the canonical spelling, not a preference:

- **`payload`, not a string field or a `rel`.** USD already means by payload what
  we mean: heavy content, referenced but *unloaded until asked*. It is in the
  dependency closure, so the world ships with the lesson. A `rel` targets a prim
  inside the composed stage — it is not an asset dependency at all, and a string
  path is invisible to every USD tool. `openusd`'s stage carries real load rules
  (`InitialLoadSet`, `load`/`unload`), so deferral is a mechanism we have.
- **The payload is a DECLARATION; mounting stays `LoadScene`.** The curriculum
  stage must not become the world. The launcher reads the arc and mounts it
  through the one scene path that already exists.
- **`LunCoProgramAPI` + `info:sourceAsset`** is already how this engine binds a
  rhai program to a prim. A lesson is that, plus display attributes.
- **`next` as a `rel`** makes a dangling chain a lint error instead of an
  unchecked string. Order IS the chain (`TutorialRegistry::ordered()` already
  walks it), so there is no `order` field.
- **NO `hosts` attribute.** Which app offers which track is application
  configuration, not scene description — the same category error as
  `kind = "tutorialTrack"` (which is also invalid: `kind` is the model hierarchy,
  a registered token set). The canonical expression is **which layers the app
  composes**: bundled tracks are sublayers of the app's curriculum stage; a twin
  contributes by adding its layer on the SESSION layer (runtime, non-persistent,
  removed when the twin closes). That deletes the hand-rolled twin path —
  `TWIN_TUTORIALS_MANIFEST`, `from_twin`, `retain_bundled`,
  `LoadedTwinCurriculum`, a second parse — because composition already answers
  "these layers, in this order".

### What stays OUT of USD

Lesson **logic**. Objectives, gating, coach steps stay rhai. Encoding control
flow as prim attributes means writing an interpreter for a language expressed in
attributes — strictly worse than the rhai already sanctioned for behaviour.

### Prerequisite: the dependency closure must be the library's answer

"Does this lesson's world/script ship" is `UsdUtils.ComputeAllDependencies`.
Ours re-parsed layers and matched raw spec fields, and stopped at composition
arcs — so anything bound by an `asset` attribute (`info:sourceAsset`, textures)
was outside the closure.

- **Landed** (openusd `a198b40`): `Data::composition_asset_dependencies()`
  (`SdfLayer::GetCompositionAssetDependencies`) and `Data::asset_dependencies()`.
  `lunco-usd-bevy::closure` now asks the library; `discover_arcs`/`ArcFilter` are
  deleted.
- **Pending**: a recursive, resolver-anchored `compute_all_dependencies` in the
  fork, so `reference_closure` can be deleted and the `TODO(multiplayer)`
  unconfined-walker note answered (a resolver context has a root; `base.join`
  does not).

### Current state vs. target

As-built today is the interim: the catalog is `assets/tutorials/<track>/tutorials.json`
plus a `track.json` (`label`, `order`, `hosts` — all required, no defaults),
discovered by walking `assets/tutorials/`, and the environment is still
`load_scene` inside `on_start`. That fixed the live defect and made tracks
data-driven rather than a `["basic"]` list in Rust, but it is **superseded** by
the layer above: `tutorials.json` + `track.json` become the curriculum layer,
and `hosts` disappears into composition.

Migration order: closure prerequisite → curriculum layer + codeless schema
(registered in `plugInfo.json`, or it does not exist outside this engine) →
launcher mounts the payload before running the scenario → `load_scene` leaves
the lesson scripts → twin path deleted in favour of session-layer composition.
