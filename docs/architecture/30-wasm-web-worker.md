# 30 — Wasm Web Worker (off-thread Modelica)

How the browser build keeps the UI responsive while rumoca compiles a model.

## Why

`wasm32-unknown-unknown` is single-threaded. Until 2026-05, the wasm build
ran the Modelica worker logic *on the Bevy main thread* via
`worker::inline_worker_process`, so any compile that took seconds froze
the page. Native already had the right shape — `worker::modelica_worker`
on a `std::thread` exchanging crossbeam messages — and the goal was to
mirror that on the web without nightly Rust, atomics, or
`SharedArrayBuffer`.

The chosen approach: a **second wasm bundle running in a Web Worker**.
Same code, separate JS thread, separate wasm linear memory. Bevy systems
keep talking to the same `ModelicaChannels` resource; the only change is
a transport layer that bridges the channels to the worker over
`postMessage`.

## Lifecycle

```
┌─────────────────────────────────────────┐         ┌─────────────────────────────┐
│ Main page (lunica bundle)           │         │ Worker (lunica_worker)      │
│ ─────────────────────────────────────── │         │ ─────────────────────────── │
│ Bevy app, egui UI, MSL fetcher          │         │ no Bevy app                 │
│ ModelicaChannels (crossbeam)            │         │ InlineWorkerInner state     │
│   tx_cmd ──┐                            │         │ ModelicaCompiler (lazy)     │
│   rx_res ◄─┤                            │         │                             │
│            │                            │         │                             │
│  pump_commands_to_worker  ──postMessage─►  ──────►  onmessage:                  │
│       (Update system)        bincode    │         │   WireMessage::Command   →  │
│                                         │         │     process_inline_command  │
│                              postMessage             ◄── Vec<ModelicaResult>    │
│  worker.onmessage ◄──────────  bincode  │         │   WireMessage::Ping      → │
│      → tx_res.send(result)              │         │     pong via WireResult::Log│
└─────────────────────────────────────────┘         └─────────────────────────────┘
```

1. **Page boot.** Main wasm runs `lunica`'s `wasm_bindgen(start) run()`.
   `ModelicaPlugin::build` creates two crossbeam channels (cmd, res), stores
   them on `ModelicaChannels`, and registers the `tx_res` / `tx_cmd` handles
   with `worker_transport::register_result_sender` /
   `register_command_sender` so JS-side bridges can reach them.
2. **Worker spawn.** `lunica::run` calls
   `worker_transport::install_worker("./worker/worker_bootstrap.js")`. That
   constructs a `web_sys::Worker` of `type=module`, attaches an
   `onmessage` closure that decodes `WireResult` and pushes `Result` into
   `tx_res` and `Log` lines into `bevy::log::info!("[worker] …")`, and
   stashes the worker handle in a `OnceLock<WorkerHandle>`. The worker JS
   bundle is loaded via the **bootstrap shim** (see "Bootstrap" below).
3. **Worker init.** Inside the Worker, `bin/lunica_worker.rs::run()` runs
   under `wasm_bindgen(start)`. It installs `self.onmessage`, posts back
   `WireResult::Log("ready")`, and parks.
4. **MSL handoff.** When the main page's MSL fetcher decodes the
   pre-parsed bundle (`msl_remote::drain_msl_load_slot`), it calls
   `crate::worker_transport::install_msl_in_worker(&parsed_docs)` *before*
   `install_global_parsed_msl(parsed_docs)`. That bincode-encodes a
   `WireMessage::InstallParsedMsl(parsed.to_vec())` and posts it to the
   worker via `post_message_with_transfer`, **moving** the `ArrayBuffer`
   instead of cloning. The worker decodes and stores the bundle in *its*
   `GLOBAL_PARSED_MSL`. ~165 MB wire, single message.
5. **Compile / Step / etc.** Bevy systems send `ModelicaCommand` via
   `channels.tx` exactly as on native. Each `Update` tick,
   `worker_transport::pump_commands_to_worker` drains `channels.rx_cmd`,
   wraps each command in `WireMessage::Command(...)`, bincode-encodes,
   `worker.post_message(...)`. The inline-worker fallback bails out
   (`worker_transport::is_worker_active()`) so the two paths never race for
   the same queue.
6. **Worker dispatch.** Worker `onmessage` decodes the envelope:
   - `Command(cmd)` → `worker::process_inline_command(state, cmd, |r| post_result(r))`.
     Same dispatch the inline path uses — single source of truth.
     `catch_unwind` wraps the call so a panic surfaces as
     `WireResult::Log("PANIC during {label}: {msg}")` instead of silent death.
   - `InstallParsedMsl(parsed)` → `msl_remote::install_global_parsed_msl_pub(parsed)`.
   - `Ping(tag)` → `WireResult::Log("pong: {tag} (msl={})")`.
7. **Result fan-in.** Worker posts each `WireResult` back. Main's
   `onmessage` decodes:
   - `Result(r)` → `tx_res.send(r)` — picked up by the existing
     `worker::handle_modelica_responses` system.
   - `Log(line)` → `bevy::log::info!("[worker] {line}")` — surfaces in
     the page Console panel. Web Workers have a separate console context
     that page DevTools can't see, so without this any worker activity
     would be invisible.

## Wire types (`worker_transport`)

```rust
pub enum WireMessage {
    Command(ModelicaCommand),
    InstallParsedMsl(Vec<(String, StoredDefinition)>),
    Ping(String),
}
pub enum WireResult {
    Result(ModelicaResult),
    Log(String),
}
```

`ModelicaCommand` and `ModelicaResult` derive `Serialize`/`Deserialize`.
`ModelicaCommand::Compile.stream` is `#[serde(skip)]` because
`Arc<ArcSwap<SimSnapshot>>` only makes sense in one address space; on
wasm we always use the per-Step result-message path instead of the
shared-snapshot fast-path.

## Cross-platform footprint

Native unchanged. The serde derives are no-ops at runtime; the inline
fallback (`worker::inline_worker_process` + `InlineWorker`) is wasm-only.
`worker_transport.rs` and `bin/lunica_worker.rs` are
`#![cfg(target_arch = "wasm32")]` end-to-end. The single source of truth
for command dispatch is `worker::process_inline_command` (also wasm-only,
extracted from `inline_worker_process` for reuse). The native
`worker::modelica_worker` loop kept its own dispatch.

## Build (`scripts/build_web.sh build lunica`)

Two cargo builds, two `wasm-bindgen` passes:

```
target/wasm32-unknown-unknown/web-release/lunica.wasm
                                        /lunica_worker.wasm

target/web/lunica/{lunica.js, lunica_bg.wasm, …}
target/web/lunica_worker/{lunica_worker.js, lunica_worker_bg.wasm, …}

dist/lunica/
├── index.html             ← imports & calls init('lunica.js')
├── lunica.js, …       ← main bundle
├── msl/                   ← parsed MSL artefacts
└── worker/
    ├── lunica_worker.js, …  ← worker bundle (wasm-bindgen output)
    └── worker_bootstrap.js  ← `import init; await init();`  ← REQUIRED
```

`RUSTFLAGS=--cfg=web_sys_unstable_apis` is mandatory for both bins
(wgpu's WebGPU bindings and `web_sys::DedicatedWorkerGlobalScope` are
gated on it).

### Bootstrap

`wasm-bindgen --target web` produces an ES module that *exports* `init`
without auto-running. When the main page does
`new Worker('./worker/lunica_worker.js', { type: 'module' })`, the browser
loads the JS but module-level code only declares imports/exports — `init`
is never called, `wasm_bindgen(start)` never fires, the worker silently
stays without an `onmessage` handler. Every command sent to it queues
forever.

The fix is a tiny shim:

```js
// dist/lunica/worker/worker_bootstrap.js
import init from './lunica_worker.js';
await init();
```

`worker_transport::install_worker` points at `worker_bootstrap.js`, not
`lunica_worker.js`. This is the single most important file in the whole
pipeline; without it nothing else works.

## Dev bridges (JS-callable)

`web/index.html` re-exports two `#[wasm_bindgen]` functions on `window`
so DevTools can drive the pipeline without going through canvas clicks
(synthetic mouse events don't reach egui reliably on web — winit listens
for trusted events only):

```js
// In Console:
__lc_test_worker_ping('hello')          // → [worker] pong: hello (msl=2670)
__lc_test_dispatch_compile('Osc', src)  // fires ModelicaCommand::Compile
                                        //  with Entity::PLACEHOLDER
```

`__lc_test_dispatch_compile` posts via `COMMAND_TX.send(...)` directly,
so the result still flows through `pump_commands_to_worker → worker →
handle_modelica_responses` like a real UI command. Useful for autonomous
test loops.

## Performance notes

| Phase                                      | Cost (cold)  | Notes                                              |
|--------------------------------------------|--------------|----------------------------------------------------|
| Worker wasm download + instantiate         | ~1–2 s       | parallel with main wasm                            |
| MSL `bincode::serialize` on main           | ~1.0 s       | 165 MB output, main-thread blocking                |
| MSL `post_message_with_transfer`           | ≈0           | `ArrayBuffer` ownership transferred, no clone     |
| MSL `bincode::deserialize` in worker       | ~0.5 s       | off-thread, doesn't block UI                      |
| Compile `Osc` (no MSL)                     | 0.07 s       | round-trip including pump + post + decode         |
| Compile `AnnotatedRocketStage` (full MSL)  | ~3.4 s       | round-trip; native equivalent ~2 s, was 18 s inline |
| Step                                       | ~50 µs RT    | post + structuredClone of small payload           |

Per-Step roundtrip is dominated by JS event-loop scheduling, not
serde. At 60 Hz that's ~0.3 % main-thread overhead.

## Memory

Two wasm linear memories share the page. The worker bundle is ~13 MB
compressed (28 MB wasm, slimmed by `wasm-opt -O2 --strip-debug`). The
MSL bundle exists in *both* memories after install — main has the parsed
`Vec` for `ModelicaCompiler::new` on its side, worker has its own copy.
~165 MB extra heap. Future optimisation: ship the original
`parsed-*.bin.zst` bytes (16 MB) to the worker and have it decompress +
deserialize itself, removing the main-side cost entirely. Tracked but
not blocking.

## Failure modes & diagnostics

| Symptom                                     | Cause                                         | Where to look                                                       |
|---------------------------------------------|-----------------------------------------------|---------------------------------------------------------------------|
| Compile spinner forever, no `[worker]` logs | Worker bundle didn't init                     | Missing/broken `worker_bootstrap.js`                                |
| `[worker] PANIC during Compile X: ...`      | rumoca panic inside worker                    | Surfaced via `catch_unwind` + `WireResult::Log`                    |
| `Simulation worker crashed and restarted`   | Result with `Entity::PLACEHOLDER` (test path) | Cosmetic; only fires from `__lc_test_dispatch_compile`              |
| `[worker_transport] post_message failed`    | Worker died, browser refused message          | Browser DevTools → Application → Service Workers / Workers panel    |
| UI stutters once at MSL install             | Main-side bincode + transfer of 165 MB        | Optimisation TODO above                                              |

The worker's own `web_sys::console::log_1` lines (e.g. `[lunica_worker]
starting`) DO appear in the page console in Chrome — Chrome merges
worker stdout/stderr into the main page Console panel. Other browsers
may not; `WireResult::Log` is the portable channel.

## Inline fallback

If `install_worker` fails (worker bundle missing, browser sandbox
refuses, etc.) the inline path stays alive. `pump_commands_to_worker`
early-returns on `WORKER.get().is_none()`, so commands stay in
`rx_cmd`; `inline_worker_process` then drains them on the main thread
just like the pre-2026-05 build. UI blocks on compile in that mode, but
the page still works.

## What's NOT solved

- **Single-page MSL clone.** Worker fetches its own MSL would eliminate
  the main-side serialise + transfer entirely, at the cost of two
  network requests. Worth doing later.
- **Worker lifecycle on page reload.** Browser disposes the worker on
  navigate; we re-init from scratch every load. Could persist via
  `SharedWorker` but YAGNI.
- **Cancel mid-compile.** No way to interrupt a compile in flight. Same
  as native today.
- **Worker bundle size.** 28 MB wasm is unnecessarily large because the
  worker pulls all of `lunco-modelica` (incl. UI code it never uses).
  Splitting the worker logic into its own crate would cut this in half;
  not done because the bundle is loaded in parallel with the main wasm
  and doesn't show up as a startup-time bottleneck.
