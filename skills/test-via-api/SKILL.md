---
name: test-via-api
description: >
  How to verify luncosim changes end-to-end without asking the
  user to click. Trigger whenever a UI flow needs verification — a new
  diagram, a fix to drill-in, a screenshot to confirm a regression, a
  smoke test of any reflect-registered Event command. The workbench
  exposes a small HTTP API on `--api PORT`; this skill is the runbook
  for driving it from curl, capturing screenshots, diagnosing failures,
  and adding new commands when the existing surface isn't enough. Also
  trigger when you catch yourself about to `pkill lunica`,
  write a temp `.rs` test binary to inspect rumoca state, chain a
  `sleep 30 && tail` poll, or ask the user "can you check the
  screenshot?". The right move is always: send a command, take a
  screenshot, read it, decide.
---

# Test the workbench via API

The production `luncosim` exposes a reflect-registered Event API on
`--api PORT` (default 4101). Always pass this flag when launching the luncosim, including
visual checks; use another explicit free port if 4101 is occupied. UI verification — diagrams rendering,
drill-ins, simulations, file ops — should be driven from this API
rather than asking the user to click.

## Live shader iteration

Shader source edits are a live-test path. Keep the production luncosim running, edit the
WGSL under `assets/shaders/`, then dispatch `ReloadShader` through the same API (an empty
path reloads the standard shader set; pass `shaders/starfield.wgsl` to limit the reload).
Confirm the command result and inspect the unchanged window. Do not relaunch the app just
to pick up a starfield or material edit.

## Tutorial and Rhai iteration

Tutorial behavior is authored in `assets/tutorials/**/*.rhai` and should be
tested through its production scene gate in `assets/scenes/tests/` with the
observer in `assets/scenarios/tests/`. After editing either Rhai file, rerun
`./scripts/run_scene_tests.sh --no-build --exact <scene-name>` for a single
scene, or `./scripts/run_scene_tests.sh --no-build <scene-substring>` for a
group; the runner uses four independent headless processes by default, and
`-j/--jobs N` changes that bound (`-j 1` is the serial diagnostic mode). This
does not change each gate process's deterministic `--threads 1 --jitter 0`, and
graphics assertions remain a separate serial offscreen pass. Do not rebuild the
Rust core for a script-only change. The observer must verify public `cmd:*`
events plus the resulting live state and emit a real verdict. Parsing or
`--validate` is only preflight evidence.

For a one-shot assertion that needs the currently loaded USD stage, use
`./scripts/api/run_rhai_test.sh <port> <test.rhai> [probe-prim]`. It prepends
the test libraries and delegates to the native `luncosim rhai --stdout` client,
which calls `RunRhai` on the existing production session. Editing and rerunning
the test does not restart the app. Use `./scripts/api/run_scenario.sh`
when the assertion should remain attached as a persistent observer.

For an interactive tour, keep one production session and use `StartTutorial`
through `/api/commands`, then inspect the HUD and event stream. `RunScenario`
is the live hot-reload path for a script attached to an existing host. Restart
only when changing Rust or when a clean scene lifecycle is itself under test.

## Live runtime HTML/CSS iteration

The native `luncosim` UI watches the retained runtime surfaces under
`assets/ui/`. Edit a surface's `.html` or `.css` in place and inspect the same
window; HUI rebuilds the affected template and Flair reapplies the stylesheet
without a binary rebuild or relaunch. Editing `runtime_surfaces.json` rebuilds
the registered surface roots and action bindings. Rust exposure producers and
action observers still require a rebuilt production binary.

Use `ReadExposures` to verify the data side independently of the pixels:

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H 'content-type: application/json' \
  -d '{"type":"ExecuteCommand","command":"ReadExposures","params":{"surface":"driven-vessel"}}' | jq .
```

The response's `revision` changes only when an exposed value or visibility flag
changes. If it is stable, an unchanged runtime surface should not rebuild its
view-model. Use `CaptureScreenshot` for the visual check. `ReloadShader` and
`RunScenario` reload WGSL and Rhai respectively; neither reloads HTML/CSS.

Runtime UI is a small native HUI/Flair language, not a browser DOM. Do not
expect JavaScript, forms, text inputs, full CSS, or host-font fallback. Read the
[runtime UI skill](../runtime-ui/SKILL.md) for the supported surface contract,
placement/dock ownership, font rules, and performance gates.

## Session lifecycle

Before launching another luncosim, send `Exit` to the existing API session and verify that
its process and port are gone. Never overlap GUI/API sessions or reuse a port while the
session is alive. Keep the current process for live shader/Rhai edits; restart only when a
rebuilt binary or an explicit clean session is required.

## Lifecycle (start → drive → stop)

```bash
# 1. After the existing session is confirmed stopped, start the production
#    binary built in this worktree. Keep it alive in the runner's background
#    session.
target/debug/luncosim --api 4101

# 2. Wait for the readiness contract, not just an open socket:
until curl -s http://127.0.0.1:4101/api/ready 2>/dev/null \
  | jq -e '.data.ready == true and .data.world_hold == false and .data.pending_count == 0' >/dev/null; do
  sleep 1
done

# 3. Send commands (see catalog below).

# 4. Stop with Exit, NEVER pkill / kill (user has to confirm those):
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"type":"ExecuteCommand","command":"Exit","params":{}}'
```

After `Exit`, verify that the process and `:4101` listener are gone before
starting another session. A queued command or a reachable socket is not proof
that a scene is ready; `/api/ready` is the gate for scene load, Modelica compile
and participant initialization.

## Curl shape

Every typed command uses the tagged envelope
`{"type":"ExecuteCommand","command":"<Name>","params":{...}}`.
Include `params` even for parameterless commands. Built-in discovery and entity
listing use their own explicit `type` values.

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"type":"ExecuteCommand","command":"OpenClass","params":{"qualified":"Modelica.Blocks.Continuous.PID"}}'
```

Successful fire-and-forget response: `{"data":{"accepted":true}}`. A result-returning typed command puts its command-specific payload in the same `data` envelope. Malformed envelopes are rejected at the transport boundary, and invalid typed parameters return HTTP 422. Deferred commands return their completed payload or error on the same request; there is no command-id polling endpoint.

### Loading a scene or model

Two commands, two different argument types, and mixing them up is a silent no-op:

| command | takes | notes |
|---|---|---|
| `OpenTwin` | a **folder** containing `twin.toml` | auto-loads `[usd] default_scene` |
| `LoadScene` | a root-qualified `twin://` or `lunco://` address | mounts a scene address; it is not a filesystem opener |
| `OpenFile` | a filesystem path or supported URI | extension-routes the document to its owning domain; USD paths resolve their Twin root |

Passing the `.usda` *file* to `OpenTwin` fails the `twin.toml` check and is
refused with a `warn!`.

`LoadScene` is not a general file opener. Bare and absolute filesystem paths
are refused with

```
[scene] `…` is not a root-qualified scene address — LoadScene takes `lunco://…` or `twin://…`
```

and the load is a no-op — the currently mounted scene remains active. Read the
status bar or query the active scene before trusting a screenshot. Use `OpenFile`
for a filesystem path; it resolves the workspace layer and preserves the
document-first mounting contract.

`CaptureScreenshot` returns the PNG as the **response body**; write those bytes
yourself rather than relying on `save_to_file`.

### Validate an asset without loading it

`ValidateAsset` is the parse-only pre-flight ("does this file compile?"):
no cosim, no scene load, no GPU — safe against any running luncosim, even
mid-simulation. Unlike the commands below it is a **query provider**, so the
report comes back in the response body; no secondary result request is needed.

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"type":"ExecuteCommand","command":"ValidateAsset","params":{"path":"lunco://models/LunCo/Electrical/Battery.mo"}}'
```

**Answered by luncosim binaries only** — it lives in `lunco-scene-commands`,
which lunica does not link, so lunica returns `CommandNotFound`. With no
instance (or only lunica) up, the same checks run as a one-shot CLI that builds
no app at all:

```bash
target/debug/luncosim --validate assets/models/LunCo/Electrical/Battery.mo
```

Full runbook — per-extension checks, exit codes, and the CWD path-resolution
trap: [`validate-assets`](../validate-assets/SKILL.md).

## Command ownership

This skill owns the generic API envelope, runtime lifecycle, screenshots, and
end-to-end evidence. For Modelica-specific loading, compile/run, experiment,
and plot commands, use [`run-modelica`](../run-modelica/SKILL.md); keep that
catalog in one place.

## Verification workflow

```
1. Start workbench (run_in_background:true).
2. Monitor until READY.
3. OpenFile or OpenClass to load model.
4. Wait ~3-5s for rumoca parse + projection (background tasks).
5. OpenClass or drill action if scoping to a sub-class.
6. Wait ~3-5s for the post-drill projection to land.
7. FitCanvas + sleep 1.
8. CaptureScreenshot → /tmp/foo.png.
9. Read the PNG to inspect.
10. Check the process log for lines like `[Projection] import done in Xms: N nodes M edges`.
11. Exit when done.

## Rover modeling loop: reload the live scene, then measure it

For a world-direction tracker, use the API to verify one complete coordinate
chain after reload: target vector in the mount frame, controller setpoint,
measured joint angle, and rendered boresight. Do not accept a controller's
internal `locked` state alone; it can be self-consistent with an incorrect axis
or boresight convention.

Keep one luncosim process running while iterating on a rover. Edit the USD, then
use `OpenFile` for a file-backed asset, `RestartScene` for the mounted scene, or
`ApplyUsdOp` for an in-place authored opinion. Reattach a diagnostic script with
`RunScenario`; this hot-reloads only that script. Read `ScriptInspect`,
`QueryEntity`, `rover_status`, and relevant ports while the simulation is live.

A rover test must report measured telemetry and movement, not merely compile or
compare two values at rest. Use `luncosim test` for deterministic CI verdicts,
but keep the live API check because it exercises the production reload and
command paths. Do not add a second reload command or a standalone rover test
binary.

For presentation work, establish the acceptance chain in order: builtin
raycast drive first (`DRIVETRAIN PARITY: PASS`), then the Modelica drive-law
overlay (`MODELICA DRIVE LAW: PASS`), then optional power/thermal/autonomy.
Never use a visual screenshot as a substitute for either verdict: a rover that
does not move, or one still driven by the builtin kernel after a failed Modelica
overlay, can look plausible in a parked frame.

Partial USD object/reference reload is intentionally not exposed yet. Until its
composition, connection, and Modelica-worker lifecycle are implemented as one
operation, use the full `RestartScene` reload for rover tests. A successful full
reload must re-run USD prim projection, cosim model creation/compilation, and
connection rewiring before the test verdict is trusted.

```

For a placed rover, also query the composed USD transform after reload. Check
that every authored rotation op appears in `xformOpOrder` and that the effective
heading comes from one placement layer. For a fixed solar panel, list the
composed `SolarPanel`, `Battery` and rover-root network entities, then read the
rover-root boundary ports. Presence of a panel mesh is not a power verdict: require
positive `solar_power`/panel `power_out`, a valid incidence, and battery current
or changing `soc`.

## Production tutorial tests

Tutorial acceptance belongs to the production `target/debug/luncosim` binary.
Build that binary in the worktree, run the scene-test command directly, and
capture its exit code and authored verdict. `--validate` proves only USD
parsing; a successful acknowledgement proves validation and dispatch, not that
the simulation has finished its work. A live API check must also wait for `/api/ready` to report `ready:true`,
`world_hold:false`, and `pending_count:0`.

Autopilot checks should observe the same `PossessVessel` and port-write events
as a human control sequence, plus a real movement/port predicate and the final
goal. Keep declared cosim topology separate from current samples: a connection
may resolve before the first sample, but an absent value is not a valid zero.
The complete boundary is in
[`tutorial-autopilot-and-port-contracts`](../../docs/architecture/tutorial-autopilot-and-port-contracts.md).

For source-backed program authoring, query `ListOpenDocuments` for the USD
document, dispatch `AttachProgram`, then verify `ListPorts`, `CosimStatus`, and
`GetBrokenConnections`. The production Rhai gate is:

```bash
target/debug/luncosim test \
  --scene scenes/tests/program_attach_command.usda --max-ticks 3000
```

It proves both a declared Modelica participant and the visible error status for
an attached source with no port contract. Do not treat a prim appearing in the
scene tree or a fire-and-forget command acknowledgement as a running model.

## Diagnosing common failures

- **"0 nodes 0 edges" after drill-in**: the target class resolved but
  conversion dropped nodes. Check:
  1. `InspectActiveDoc` → are the components really there in the AST?
     If not, parse failed.
  2. If components exist: their TYPES probably aren't in
     `local_classes_by_short` or the MSL palette. The diagram-builder
     registers the target's nested + sibling classes (sibling-pass in
     `panels/canvas_projection.rs`, the `local_classes_by_short`
     registration); connector types need to be in
     `msl_index.json` (regenerate via
     `cargo run -p lunco-modelica --bin msl_indexer`).
- **"Command 'X' not found or not API-accessible"**: the Event isn't
  reflect-registered. Give the struct the `#[Command]` attribute, mark
  its observer with `#[on_command(X)]`, and list that observer in the
  `register_commands!(...)` block in
  `crates/lunco-modelica/src/ui/commands/mod.rs` (see [§ Add a command](#add-a-command)).
- **API returns 500 / silent no-op**: check `params` includes the
  empty object `{}` even for parameterless commands.
- **Projection deadline exceeded (60s)**: rumoca parse stall, usually
  from a synchronous library load inside the worker pool. Move heavy loads to
  a separate `std::thread::spawn` and use the cache-only source-aware resolver
  in the projection (`peek_class_cached`).
- **A rebuilt binary is not visible**: replace the session through `Exit`, verify
  port 4101 is free, then start the rebuilt production binary.

## Add a command

When testing reveals a missing API surface, add the command immediately
rather than asking the user:

1. In the matching file under `crates/lunco-modelica/src/ui/commands/`,
   define the struct with the `#[Command]` attribute and the observer
   with `#[on_command(...)]` (both from `lunco_core`):
   ```rust
   use lunco_core::{Command, on_command};

   #[Command(default)]              // or `#[Command]` if you impl Default
   pub struct MyCommand { pub foo: String }

   #[on_command(MyCommand)]
   pub fn on_my_command(trigger: On<MyCommand>, mut commands: Commands) {
       let foo = trigger.event().foo.clone();
       commands.queue(move |world: &mut World| { /* ... */ });
   }
   ```
   `#[Command]` emits the `Event`/`Reflect`/`reflect(Event)` derives and
   `#[on_command]` generates the `register_type` + `add_observer` wiring —
   you don't write them by hand.
2. Add the observer fn to the `register_commands!(...)` list in
   `crates/lunco-modelica/src/ui/commands/mod.rs` (use the
   `module::fn` path form, e.g. `inspect::on_my_command`).
3. Build, restart workbench, curl it.

## What NOT to do

- Don't `pkill -f lunica`. The user has to confirm; use
  `Exit` command.
- Don't write standalone test binaries / temp `.rs` files to verify
  rumoca behaviour. Add an `Inspect*` command if the workbench can't
  already surface what you need.
- Don't chain `sleep 30 && tail ...`. Use Monitor with an `until` loop.
- Don't ask the user to take a screenshot or check anything visually
  unless API verification is genuinely impossible.
