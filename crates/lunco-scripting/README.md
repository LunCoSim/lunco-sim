# lunco-scripting

The scripting subsystem for the LunCoSim Digital Twin: **scenarios** — persistent
per-entity programs that sense and drive the simulation through the same
command/query API the HTTP API, MCP, and UI use.

> **Writing scenarios? Start with the [Scripting Guide](../../docs/scripting-guide.md).**
> This README is the crate/architecture overview.

## Backends

| Language | Status |
|---|---|
| **rhai** | **Default & primary.** Pure-Rust, sandboxed, wasm-clean — runs natively and in the browser. The full scenario lifecycle + world bridge. |
| Python (PyO3) | Optional one-shot eval only (`RunPython`); compiled and registered only with the `python` feature. A full scenario lifecycle (`PythonScenarioRuntime`) is planned. |
| Lua | Reserved language id; not implemented. |

The language-neutral core means a backend supplies only the interpreter
mechanics; lifecycle, scheduling, hot-reload, pause, teardown, diagnostics, and
the generic world mechanism is shared. Domain-specific world verbs are provided
by the spatial, time, and USD bridge adapters (see the package layout below).

## Model

A scenario is a program attached to an entity via a `ScriptedModel` component +
a `ScriptDocument` (managed like a `lunco-doc` document: versioned, hot-reloadable).
Production mission progression is returned from `task(me, ctx)` and advanced by
the native behavior kernel. `ctx` is the immutable, instance-specific launch
parameter map. Event and lifecycle hooks remain available for setup, telemetry,
and teardown:

```rhai
fn task(me, ctx)          { seq([wait_until(|m| arrived(m, GOAL, 2.0))]); }
fn on_start(me, ctx)      { /* setup */ }                         // once, after (re)compile
fn on_event(me, evt, ctx) { /* a TelemetryEvent arrived */ }
fn on_stop(me, ctx)       { brake(me); }                          // teardown
```

New mission scripts must use the task tree and events. `on_tick` is reserved for
authored test scenarios to sample state and publish a bounded verdict; it is
not a production mission or controller hook. The task tree supplies
deterministic fixed-tick progression without putting the cursor, event delivery,
or dwell timing in user policy.

Every task node has an explicit `kind` discriminator; missing/unknown kinds and
fields from another node kind are rejected. See the [task-tree schema](../../docs/architecture/rhai-task-tree.md).

Task action and predicate fields accept an anonymous closure with one
positional argument, `|me| ...`, or a named script function pointer
`Fn("name")` declared as `fn name(me)`. Both forms access persistent task state
through the driver-bound `this`; the native task driver owns cursor, dwell, and
event progression.

The host exposes a minimal generic bridge — `cmd` / `query` / `get` / `set` /
`get_setting` / `set_setting` / `world_pos` / `world_forward` / `find` / `name` /
`parent` / `children` / `list_entities` / `emit` / `sim_tick` / `dt` /
`elapsed_seconds` / `usd_document_generation`. Reflection and the canonical co-simulation port registry
provide the generic state surface; `ScriptingCatalog` reports the live command,
query, reflection, prelude, hook, and tool contracts. Everything ergonomic
(navigation, sensing, sequencing, selection) is **policy** authored in
the hot-reloadable [`prelude/`](../../assets/scripting/prelude) — no Rust rebuild to
extend it.

For topology-sensitive policy, use `usd_document_generation(doc_id)` as the
structural invalidation clock and cache the detailed USD snapshot until that
generation changes. This keeps fixed-tick policy from repeatedly serializing
`InspectUsdDocument`; live poses and commands remain per-tick reads/writes.

The prelude also provides `usd_path(id)`, which resolves
`QueryEntity.usd_prim_path` for canonical USD topology addressing. `name(id)`
remains a human-readable presentation label and must not be used to construct
scene paths.

`name(id)` and the `name` field from `list_entities()` are presentation labels
resolved from authored `ui:displayName`, catalog identity, or the `Name` leaf.
Use the API id for machine identity and `QueryEntity` when a full USD path is
needed; display labels are not topology addresses.

Scenarios are **host-authoritative**: they run on the host and in single-player,
never on a networked client (which receives behaviour via replication).

## Scenario parameters

`RunScenario` and `RunScenarioAsset` accept one natural typed object for an
instance's launch context. The source document remains reusable; parameters are
stored on the attached `ScriptedModel` and passed explicitly as `ctx` to every
lifecycle, task, and mission hook. Omitting the field means `{}`. Defaults for
individual keys are authored in Rhai, at the point where the policy uses them.

```json
{"target": 4869542932533563, "source": "fn on_start(me, ctx) {}", "params": {"speed": 1.5}}
```

```rhai
fn task(me, ctx) {
    let speed = if ctx.speed == () { 0.6 } else { ctx.speed };
    forever(once(|m| drive(m, speed, 0.0)))
}
```

The command boundary rejects a scalar, array, `null`, non-finite number, or
value outside the shared telemetry range. Rust performs that wire validation
and one typed conversion; it does not choose domain defaults or inject a global
`params` variable.

## Key commands & queries

- **Run:** `RunScenario { target, source, params }` (attach/hot-reload), `RunRhai { code }` (one-shot), `RunRhaiTool { tool, args }` (typed tool invocation), `RunRhaiToolHook { tool, hook, args }` (typed authored UI/pointer hook), `RunTimeline` / `RunStoredTimeline` (declarative missions).
- **Control:** `SetScenarioPaused`, `StopScenario`.
- **Tools & timelines:** `RegisterToolLibrary`, `RegisterTimeline` (+ `List`/`Get` discovery queries; persisted under the Twin).
- **Introspection:** `ScriptStatus` (health), `ScriptInspect` (live state), `ScriptingCatalog` (the full callable surface).

## Layout

| Path | What |
|---|---|
| [`src/world_bridge.rs`](src/world_bridge.rs) | the rhai backend (verbs + `RhaiScenarioRuntime`) |
| [`lunco-scripting-bridge-core`](../lunco-scripting-bridge-core) | language-neutral world mechanism (`ValueBuilder`) |
| [`lunco-scripting-bridge-spatial`](../lunco-scripting-bridge-spatial) | pose, navigation, geolocation, and entity projections |
| [`lunco-scripting-bridge-time`](../lunco-scripting-bridge-time) | deterministic simulation-clock projections |
| [`lunco-scripting-bridge-usd`](../lunco-scripting-bridge-usd) | USD document and prim-path projections |
| [`src/scenario.rs`](src/scenario.rs) | language-neutral lifecycle driver |
| [`src/commands.rs`](src/commands.rs) | the `#[Command]` entry points |
| [`src/catalog.rs`](src/catalog.rs) · [`src/diagnostics.rs`](src/diagnostics.rs) | discovery + introspection queries |
| [`src/tool_libs.rs`](src/tool_libs.rs) · [`src/timelines.rs`](src/timelines.rs) | tool / timeline registries + Twin persistence |
| [`prelude/`](../../assets/scripting/prelude) · [`examples/`](../../assets/scripting/examples) · [`tools/`](../../assets/scripting/tools) | the helper library, example scenarios, example tool libraries |

## Cargo features

- `rhai` (**default**) — the rhai backend; pure-Rust, wasm-clean.
- `python` — the optional PyO3 runtime (one-shot eval; requires a Python 3.12
  shared library, probed when Python is first used). Without this feature, the
  `.py` loader, Python status resource, and execution systems are not registered.

The crate builds with `rhai`, with `--no-default-features` (script-free), with
`python`, and for `wasm32-unknown-unknown`.

## Testing

```bash
# Authored prelude, tool, and policy assets through the production runtime.
LUNCOSIM_BIN=target/debug/luncosim ./scripts/run_scene_tests.sh --no-build --exact scripting_asset_contracts
```

Shipped prelude, tool, policy, and scenario behavior is tested beside the
authored assets through production scene gates. This crate does not maintain a
second Rust integration harness for scripting product behavior. Low-level Rust
tests, when needed for mechanisms that the authored runtime cannot observe,
use inline Rhai source and do not enumerate or read repository assets through
`lunco-assets-core` or `lunco-storage`.

## Docs

- **[Scripting Guide](../../docs/scripting-guide.md)** — how to write scenarios (start here).
- **[Rhai integration design](../../docs/architecture/rhai-integration.md)** — design rationale + as-built reference.
