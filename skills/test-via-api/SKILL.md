---
name: test-via-api
description: >
  How to verify modelica_workbench changes end-to-end without asking the
  user to click. Trigger whenever a UI flow needs verification — a new
  diagram, a fix to drill-in, a screenshot to confirm a regression, a
  smoke test of any reflect-registered Event command. The workbench
  exposes a small HTTP API on `--api PORT`; this skill is the runbook
  for driving it from curl, capturing screenshots, diagnosing failures,
  and adding new commands when the existing surface isn't enough. Also
  trigger when you catch yourself about to `pkill modelica_workbench`,
  write a temp `.rs` test binary to inspect rumoca state, chain a
  `sleep 30 && tail` poll, or ask the user "can you check the
  screenshot?". The right move is always: send a command, take a
  screenshot, read it, decide.
---

# Test the workbench via API

The modelica_workbench exposes a reflect-registered Event API on
`--api PORT` (default 3000). UI verification — diagrams rendering,
drill-ins, simulations, file ops — should be driven from this API
rather than asking the user to click.

## Lifecycle (start → drive → stop)

```bash
# 1. Start. MUST use run_in_background:true of the Bash tool, otherwise
#    the bash wrapper exits and the workbench dies with it.
cargo run --bin modelica_workbench -- --api 3000   # run_in_background:true

# 2. Wait for API. Use a Monitor with an until-loop, NOT chained sleeps:
until curl -s -o /dev/null -X POST http://127.0.0.1:3000/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"FitCanvas","params":{}}' 2>/dev/null; do sleep 1; done

# 3. Send commands (see catalog below).

# 4. Stop with Exit, NEVER pkill / kill (user has to confirm those):
curl -s -X POST http://127.0.0.1:3000/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"Exit","params":{}}'
```

## Curl shape

Always include `"params":{}` even for parameterless commands. Without
it the API logs `Deserialization error: invalid type: null, expected
reflected struct value` and the command silently no-ops.

```bash
curl -s -X POST http://127.0.0.1:3000/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"OpenClass","params":{"qualified":"Modelica.Blocks.Continuous.PID"}}'
```

Successful response: `{"command_id": N}`. Error: `{"error":"..."}`.

## Command catalog

All commands live in `crates/lunco-modelica/src/ui/commands.rs` as
reflect-registered `Event` structs. Add new ones there if a flow needs
them.

| Command | Params | Purpose |
|---|---|---|
| `OpenFile` | `{path}` | Open any `.mo` file from disk into a new tab. Use this for non-MSL examples (`assets/models/*.mo`) — `OpenClass` only works on MSL paths. |
| `OpenClass` | `{qualified}` | MSL drill-in by qualified name OR (with the open-doc fallback) drill into a class within an already-loaded doc. |
| `OpenExample` | `{qualified}` | MSL example duplicate-to-workspace. Returns "Could not locate" for non-MSL paths. |
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
| `FocusDocumentByName` | `{name}` | Switch active tab. |

## Verification workflow

```
1. Start workbench (run_in_background:true).
2. Monitor until READY.
3. OpenFile or OpenExample to load model.
4. Wait ~3-5s for rumoca parse + projection (background tasks).
5. OpenClass or drill action if scoping to a sub-class.
6. Wait ~3-5s for the post-drill projection to land.
7. FitCanvas + sleep 1.
8. CaptureScreenshot → /tmp/foo.png.
9. Read the PNG to inspect.
10. Tail /tmp/claude-1000/.../tasks/<task-id>.output for log lines
    (`[Projection] import done in Xms: N nodes M edges`, etc.).
11. Exit when done.
```

## Diagnosing common failures

- **"0 nodes 0 edges" after drill-in**: the target class resolved but
  conversion dropped nodes. Check:
  1. `InspectActiveDoc` → are the components really there in the AST?
     If not, parse failed.
  2. If components exist: their TYPES probably aren't in
     `local_classes_by_short` or the MSL palette. The diagram-builder
     registers the target's nested + sibling classes (sibling-pass at
     `panels/diagram.rs:1936`); connector types need to be in
     `msl_index.json` (regenerate via `cargo run --bin msl_indexer`).
- **"Command 'X' not found or not API-accessible"**: the Event isn't
  reflect-registered. Add `.register_type::<X>()` +
  `.add_observer(on_x)` in `ModelicaCommandsPlugin::build`, and make
  sure the struct has
  `#[derive(Event, Reflect, ..., Default)] #[reflect(Event, Default)]`.
- **API returns 500 / silent no-op**: check `params` includes the
  empty object `{}` even for parameterless commands.
- **Projection deadline exceeded (60s)**: rumoca parse stall, usually
  from a sync MSL load inside the worker pool. Move heavy loads to a
  separate `std::thread::spawn` and use the cache-only resolver in the
  projection (`peek_msl_class_cached`).
- **Workbench seems stale after rebuild**: it didn't restart. Send
  Exit, verify port 3000 freed, then start.

## Add a command

When testing reveals a missing API surface, add the command immediately
rather than asking the user:

1. In `crates/lunco-modelica/src/ui/commands.rs`:
   ```rust
   #[derive(Event, Reflect, Clone, Debug, Default)]
   #[reflect(Event, Default)]
   pub struct MyCommand { pub foo: String }

   fn on_my_command(trigger: On<MyCommand>, mut commands: Commands) {
       let foo = trigger.event().foo.clone();
       commands.queue(move |world: &mut World| { /* ... */ });
   }
   ```
2. Register in `ModelicaCommandsPlugin::build`:
   ```rust
   .register_type::<MyCommand>()
   .add_observer(on_my_command)
   ```
3. Build, restart workbench, curl it.

## What NOT to do

- Don't `pkill -f modelica_workbench`. The user has to confirm; use
  `Exit` command.
- Don't write standalone test binaries / temp `.rs` files to verify
  rumoca behaviour. Add an `Inspect*` command if the workbench can't
  already surface what you need.
- Don't chain `sleep 30 && tail ...`. Use Monitor with an `until` loop.
- Don't ask the user to take a screenshot or check anything visually
  unless API verification is genuinely impossible.
