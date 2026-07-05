---
name: test-via-api
description: >
  How to verify lunica changes end-to-end without asking the
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

The lunica exposes a reflect-registered Event API on
`--api PORT` (default 4101). UI verification — diagrams rendering,
drill-ins, simulations, file ops — should be driven from this API
rather than asking the user to click.

## ⚠️ NEVER kill the user's running workbench

**Default rule: if a workbench is already running on port 4101,
DO NOT send `Exit` and DO NOT start a new one.** Take the screenshot
/ run the API command against the existing instance.

Why: the user's session holds their state — open tabs, the menu
they have open right now, an in-progress drag, the canvas zoom they
set up. Killing it destroys that state and renders the screenshot
useless. Many things (open context menus, hover tooltips, drag
previews) **cannot be reproduced via API** because they only exist
during user interaction.

When you need to restart:
- The user explicitly says "restart" / "start fresh" / "kill it".
- The running binary is verifiably stale (you just rebuilt and the
  user wants to see the new behaviour). Even then: **ask first**.
- The port is bound by a zombie that's not responding to API calls.
  Try a quick `FitCanvas` ping; if it answers, that's the user's
  session — leave it alone.

If you need state inside the workbench that isn't there (a drilled-
in tab, a loaded file, a plot), drive the API to add it. NEVER
restart to "start clean."

## Lifecycle (start → drive → stop)

```bash
# 1. Start. MUST use run_in_background:true of the Bash tool, otherwise
#    the bash wrapper exits and the workbench dies with it.
cargo run --bin lunica -- --api 4101   # run_in_background:true

# 2. Wait for API. Use a Monitor with an until-loop, NOT chained sleeps:
until curl -s -o /dev/null -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"Ping","params":{}}' 2>/dev/null; do sleep 1; done

# 3. Send commands (see catalog below).

# 4. Stop with Exit, NEVER pkill / kill (user has to confirm those):
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"Exit","params":{}}'
```

## Curl shape

Always include `"params":{}` even for parameterless commands. Without
it the API logs `Deserialization error: invalid type: null, expected
reflected struct value` and the command silently no-ops.

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"OpenClass","params":{"qualified":"Modelica.Blocks.Continuous.PID"}}'
```

Successful response: `{"command_id": N}`. Error: `{"error":"..."}`.

## Command catalog

All commands live under `crates/lunco-modelica/src/ui/commands/` as
reflect-registered `Event` structs, grouped by area (`inspect.rs`,
`compile.rs`, `lifecycle.rs`, `diagram.rs`, `nav.rs`, `sim.rs`,
`plot.rs`, `doc.rs`, …). Add new ones in the matching file if a flow
needs them.

| Command | Params | Purpose |
|---|---|---|
| `OpenFile` | `{path}` | Open any `.mo` file from disk into a new tab. Use this for non-MSL examples (`assets/models/*.mo`) — `OpenClass` only works on MSL paths. |
| `OpenClass` | `{qualified, action?}` | MSL drill-in by qualified name OR (with the open-doc fallback) drill into a class within an already-loaded doc. `action: {Duplicate: {name: ""}}` = open as an editable workspace copy (empty name → derives `<short>Copy`); default `View` = read-only drill-in. |
| `FormatDocument` | `{doc}` (`0`=active) | Run `rumoca-tool-fmt` on active doc; replaces source via `ReplaceSource`. |
| `GetFile` | `{path}` | Read file from disk and log contents at INFO. |
| `InspectActiveDoc` | `{}` | Log parsed AST class tree of active doc — use to diagnose "0 nodes" projections. |
| `Exit` | `{}` | AppExit. Always use instead of pkill. |
| `FitCanvas` | `{doc}` | Fit-all in active canvas. Defers to next render so widget rect is correct. |
| `CaptureScreenshot` | `{}` | Returns raw PNG bytes. Save with `curl ... -o /tmp/foo.png` then read with the Read tool. |
| `PanCanvas` | `{doc, x, y}` | Pan to (x,y) in canvas world coords. |
| `SetZoom` | `{doc, zoom}` | Set zoom factor. |
| `SetViewMode` | `{doc, mode}` | mode = `"Diagram"` / `"Icon"` / `"Text"`. |
| `MoveComponent` | `{class, name, x, y, width, height}` | Modelica-coord drag. `class` empty = active. `width=height=0` = preserve size. |
| `Undo` / `Redo` | `{doc}` | Document op stack. |
| `AutoArrangeDiagram` | `{doc}` | Re-layout. |
| `FocusDocumentByName` | `{pattern}` | Switch active tab (field is `pattern`, NOT `name`). |
| `ConfirmClassPicker` | `{qualified?, cancel?}` | Confirm/dismiss the "Which class should Compile/Fast Run …?" picker that opens when a package has >1 model. `qualified` = pick that class (omit → dialog's pre-selected); `cancel:true` = dismiss without running. Headless equivalent of clicking the dialog. **Gotcha:** the picker only opens once the doc's AST has parsed AND there are >1 candidates — `FastRunActiveModel` on a just-opened package logs `no compilable top-level class` and opens NO picker if you fire it before parse completes. Wait for `async parse complete doc=N` in the log, THEN `FastRunActiveModel`, THEN `ConfirmClassPicker`. |

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
```

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
     `msl_index.json` (regenerate via `cargo run --bin msl_indexer`).
- **"Command 'X' not found or not API-accessible"**: the Event isn't
  reflect-registered. Give the struct the `#[Command]` attribute, mark
  its observer with `#[on_command(X)]`, and list that observer in the
  `register_commands!(...)` block in
  `crates/lunco-modelica/src/ui/commands/mod.rs` (see [§ Add a command](#add-a-command)).
- **API returns 500 / silent no-op**: check `params` includes the
  empty object `{}` even for parameterless commands.
- **Projection deadline exceeded (60s)**: rumoca parse stall, usually
  from a sync MSL load inside the worker pool. Move heavy loads to a
  separate `std::thread::spawn` and use the cache-only resolver in the
  projection (`peek_msl_class_cached`).
- **Workbench seems stale after rebuild**: it didn't restart. Send
  Exit, verify port 4101 freed, then start.

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
