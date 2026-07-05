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
| Python (PyO3) | One-shot eval only (`RunPython`). A full scenario lifecycle (`PythonScenarioRuntime`) is planned — the language-neutral driver + world bridge are in place; the Python binding is not. |
| Lua | Reserved language id; not implemented. |

The language-neutral core means a backend supplies only the interpreter
mechanics; lifecycle, scheduling, hot-reload, pause, teardown, diagnostics, and
the world verbs are shared (see [`scenario.rs`](src/scenario.rs) and
[`bridge_core.rs`](src/bridge_core.rs)).

## Model

A scenario is a program attached to an entity via a `ScriptedModel` component +
a `ScriptDocument` (managed like a `lunco-doc` document: versioned, hot-reloadable).
It runs every `FixedUpdate` tick with lifecycle hooks:

```rhai
fn on_start(me)      { this.idx = 0; }                       // once, after (re)compile
fn on_tick(me)       { this.idx = run_plan(me, PLAN, this.idx, 1.0, 2.0); }  // every tick
fn on_event(me, evt) { /* a TelemetryEvent arrived */ }
fn on_stop(me)       { brake(me); }                          // teardown
```

The host exposes a minimal generic bridge — `cmd` / `query` / `get` /
`world_pos` / `world_forward` / `find` / `name` / `parent` / `children` /
`list_entities` / `emit` / `sim_tick` / `dt` / `elapsed_seconds`. Everything
ergonomic (navigation, sensing, sequencing, selection) is **policy** authored in
Everything ergonomic (navigation, sequencing, selection) is **policy** authored in
the hot-reloadable [`prelude/`](../../assets/scripting/prelude) — no Rust rebuild to
extend it.

Scenarios are **host-authoritative**: they run on the host and in single-player,
never on a networked client (which receives behaviour via replication).

## Key commands & queries

- **Run:** `RunScenario { target, source, params }` (attach/hot-reload), `RunRhai { code }` (one-shot), `RunTimeline` / `RunStoredTimeline` (declarative missions).
- **Control:** `SetScenarioPaused`, `StopScenario`.
- **Tools & timelines:** `RegisterToolLibrary`, `RegisterTimeline` (+ `List`/`Get` discovery queries; persisted under the Twin).
- **Introspection:** `ScriptStatus` (health), `ScriptInspect` (live state), `ScriptingCatalog` (the full callable surface).

## Layout

| Path | What |
|---|---|
| [`src/world_bridge.rs`](src/world_bridge.rs) | the rhai backend (verbs + `RhaiScenarioRuntime`) |
| [`src/bridge_core.rs`](src/bridge_core.rs) | language-neutral world bridge (`ValueBuilder`) |
| [`src/scenario.rs`](src/scenario.rs) | language-neutral lifecycle driver |
| [`src/commands.rs`](src/commands.rs) | the `#[Command]` entry points |
| [`src/catalog.rs`](src/catalog.rs) · [`src/diagnostics.rs`](src/diagnostics.rs) | discovery + introspection queries |
| [`src/tool_libs.rs`](src/tool_libs.rs) · [`src/timelines.rs`](src/timelines.rs) | tool / timeline registries + Twin persistence |
| [`prelude/`](../../assets/scripting/prelude) · [`examples/`](../../assets/scripting/examples) · [`tools/`](../../assets/scripting/tools) | the helper library, example scenarios, example tool libraries |

## Cargo features

- `rhai` (**default**) — the rhai backend; pure-Rust, wasm-clean.
- `python` — the PyO3 runtime (one-shot eval; requires a Python 3.12 shared library).

The crate builds with `rhai`, with `--no-default-features` (script-free), with
`python`, and for `wasm32-unknown-unknown`.

## Testing

```bash
# rhai backend (lib + live end-to-end scenario tests).
# Note the env var: rhai debug info can stress the linker — line-tables-only avoids it.
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p lunco-scripting --lib --test rhai_rover_live_test
```

## Docs

- **[Scripting Guide](../../docs/scripting-guide.md)** — how to write scenarios (start here).
- **[Rhai integration design](../../docs/rhai-integration-design.md)** — design rationale + as-built reference.
