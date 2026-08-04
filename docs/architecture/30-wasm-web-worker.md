# 30 — Wasm Web Worker (off-thread Modelica)

> Status: Active · Audience: contributors on the web/wasm build, worker runtime, and deploy

How the browser build keeps the UI responsive while rumoca compiles a model.

## Why

`wasm32-unknown-unknown` has no native thread API available to the runtime.
The Modelica worker therefore runs in a dedicated Web Worker. Native already
has the same ownership boundary — `worker::modelica_worker` on a
`std::thread` exchanging crossbeam messages — and the web transport mirrors
that boundary without nightly Rust, atomics, or `SharedArrayBuffer`.

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
│ ModelicaChannels (crossbeam)            │         │ WorkerState                  │
│   tx_cmd ──┐                            │         │ ModelicaCompiler (lazy)     │
│   rx_res ◄─┤                            │         │                             │
│            │                            │         │                             │
│  pump_commands_to_worker  ──postMessage─►  ──────►  onmessage:                  │
│       (Update system)        bincode    │         │   WireMessage::Command   →  │
│                                         │         │     process_command          │
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
2. **Worker spawn.** The first Modelica compile, document parse, or fast run
   calls `worker_transport::ensure_pool_spawned`, which constructs a
   `web_sys::Worker` of `type=module`, attaches an
   `onmessage` closure that decodes `WireResult` and pushes `Result` into
   `tx_res` and `Log` lines into `bevy::log::info!("[worker] …")`, and
   stashes the worker handle in a `OnceLock<WorkerHandle>`. The worker JS
   bundle is loaded via the **bootstrap adapter** (see "Bootstrap" below).
3. **Worker init.** Inside the Worker, `bin/lunica_worker.rs::run()` runs
   under `wasm_bindgen(start)`. It installs `self.onmessage`, posts back
   `WireResult::Log("ready")`, and parks.
4. **MSL handoff.** The main page's MSL fetcher keeps the downloaded
   `parsed-*.bin.zst` or `sources-*.tar.zst` bytes compressed and gives them to
   `msl_remote::drain_msl_load_slot`. The transport posts the compressed parsed
   bundle to the worker as `InstallParsedMslCompressed`; the worker owns
   decompression, bincode deserialization, and installation into its own
   `GLOBAL_PARSED_MSL`. For source recovery it uses
   `ParseSourceMslCompressed`, which also keeps untar and parsing off the page
   thread. The primary worker transfers raw decoded bincode bytes back only so
   the page can build its separate resolution/autocomplete index with chunked
   deserialization; the page never decompresses, untars, or parses MSL.
5. **Compile / Step / etc.** Bevy systems send `ModelicaCommand` via
   `channels.tx` exactly as on native. Each `Update` tick,
   `worker_transport::pump_commands_to_worker` drains `channels.rx_cmd`,
   wraps each command in `WireMessage::Command(...)`, bincode-encodes,
   `worker.post_message(...)`. If the worker is unavailable, each command
   receives an explicit lifecycle failure; simulation is never run on the page
   thread and commands never remain queued indefinitely.
6. **Worker dispatch.** Worker `onmessage` decodes the envelope:
   - `Command(cmd)` → `worker::process_command(state, cmd, |r| post_result(r))`.
     This is the single wasm command-dispatch path.
     `catch_unwind` wraps the call so a panic surfaces as
     `WireResult::Log("PANIC during {label}: {msg}")` instead of silent death.
   - `InstallParsedMslCompressed(bytes)` → worker-owned decompress, deserialize,
     and `msl_remote::install_global_parsed_msl_pub(parsed)`.
   - `ParseSourceMslCompressed(bytes)` → worker-owned untar, parse, install,
     and (for the primary) decoded-bundle transfer back to the page.
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
    InstallParsedMslCompressed { bytes: Vec<u8>, provide_to_main: bool },
    ParseSourceMslCompressed { bytes: Vec<u8>, provide_to_main: bool },
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

Native unchanged. The serde derives are no-ops at runtime.
`worker_transport.rs` and `bin/lunica_worker.rs` are
`#![cfg(target_arch = "wasm32")]` end-to-end. The wasm worker owns the
`WorkerState` and dispatches through `worker::process_command`; there is no
main-thread simulation fallback. The native `worker::modelica_worker` loop
keeps its native dispatch and ownership of `SimulationSession` values.

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

The required entry point is a tiny adapter:

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
| MSL compressed bundle post                 | ≈0           | compressed bytes are transferred to the worker    |
| MSL decompress + bincode deserialize worker| ~0.5 s       | off-thread, doesn't block UI                      |
| MSL chunked deserialize on main            | bounded      | resolution index only; no decompress/untar/parse  |
| Compile `Osc` (no MSL)                     | 0.07 s       | round-trip including pump + post + decode         |
| Compile `AnnotatedRocketStage` (full MSL)  | ~3.4 s       | round-trip; native equivalent ~2 s                 |
| Step                                       | ~50 µs RT    | post + structuredClone of small payload           |

Per-Step roundtrip is dominated by JS event-loop scheduling, not
serde. At 60 Hz that's ~0.3 % main-thread overhead.

## Memory

Two wasm linear memories share the page. The worker bundle is ~13 MB
compressed (28 MB wasm, slimmed by `wasm-opt -O2 --strip-debug`). The worker
owns one installed parsed MSL bundle. The page also receives the worker's
decoded bincode bytes and incrementally deserializes a separate copy for
resolution/autocomplete, because wasm memories cannot be shared. Compressed
source and parsed bundles remain compressed until the worker takes ownership;
there is no page-thread untar/parse/decompression fallback.

## Failure modes & diagnostics

| Symptom                                     | Cause                                         | Where to look                                                       |
|---------------------------------------------|-----------------------------------------------|---------------------------------------------------------------------|
| Compile spinner forever, no `[worker]` logs | Worker bundle didn't init                     | Missing/broken `worker_bootstrap.js`                                |
| `[worker] PANIC during Compile X: ...`      | rumoca panic inside worker                    | Surfaced via `catch_unwind` + `WireResult::Log`                    |
| `Simulation worker crashed and restarted`   | Result with `Entity::PLACEHOLDER` (test path) | Cosmetic; only fires from `__lc_test_dispatch_compile`              |
| `[worker_transport] post_message failed`    | Worker died, browser refused message          | Browser DevTools → Application → Service Workers / Workers panel    |
| UI stutters during MSL install              | Main-side chunked AST deserialization        | `msl_remote::drive_msl_main_decode`; no page-thread parse/decompress |

The worker's own `web_sys::console::log_1` lines (e.g. `[lunica_worker]
starting`) DO appear in the page console in Chrome — Chrome merges
worker stdout/stderr into the main page Console panel. Other browsers
may not; `WireResult::Log` is the portable channel.

## Worker-unavailable behavior

If `install_worker` fails, or a post to the worker is rejected, the transport
drains the affected command tickets into explicit lifecycle failures. The UI
can report that the Modelica worker is unavailable and the user can reload the
web application. No command is silently retained, and no Modelica compile or
step runs on the page thread.

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

## Prerequisites

```bash
# Required: wasm32 target
rustup target add wasm32-unknown-unknown

# Required: wasm-bindgen CLI (the build script also looks at
# .cargo-bin/bin/wasm-bindgen if you keep a project-local copy).
cargo install wasm-bindgen-cli

# Strongly recommended: wasm-opt (binaryen). Shrinks the release wasm
# ~30–40% and cuts in-browser compile time proportionally. The build
# script auto-detects it on PATH and runs it after wasm-bindgen.
sudo apt install binaryen          # Debian/Ubuntu (preferred)
# or: cargo install --locked wasm-opt

# Optional: Node.js http-server (recommended, fallback to python3)
npm install -g http-server
```

If wasm-opt isn't on PATH the build still succeeds — the script logs a
hint and skips the optimisation pass.

## The wasm32 time problem (rumoca fork)

`std::time::Instant` **panics** on `wasm32-unknown-unknown` (browsers
restrict high-resolution monotonic clocks — Spectre mitigation). A fork
at `LunCoSim/rumoca` replaces those imports with conditional compilation:

```rust
#[cfg(target_arch = "wasm32")]
use instant::Instant;      // → performance.now() via wasm-bindgen
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
```

The `Instant` / `thread::spawn` wasm fixes live on **`main`**; the web
build consumes the **`wasm-asset-loader`** branch (which adds
`Session::load_source_root_in_memory` on top of `main`).

## Building & running

```bash
# Build wasm + bindings (writes dist/<bin>/)
./scripts/build_web.sh build lunica

# Serve locally
./scripts/build_web.sh serve            # or: cd dist/lunica && http-server -p 8080 -c-1 --cors
# Fallback: python3 -m http.server 8080  (won't serve pre-compressed siblings)
# Open http://localhost:8080/index.html
```

Manual equivalent of the build:

```bash
cargo build --release --target wasm32-unknown-unknown --bin lunica
wasm-bindgen target/wasm32-unknown-unknown/release/lunica.wasm \
    --out-dir dist/lunica --target web
```

`./scripts/build_web.sh` is the supported path. There is no committed
`crates/lunco-modelica/web/pkg/`.

**Browser requirements:** Chrome/Edge 113+ or Safari 16.4+ with WebGPU
(`chrome://gpu`); falls back to WebGL2. Must be served over HTTP —
`file://` won't load wasm.

### Output layout

```
dist/<binary>/
  lunica.js          # wasm-bindgen JS glue
  lunica_bg.wasm     # post-wasm-opt binary
  lunica.d.ts        # TypeScript declarations
  index.html         # copy of crates/lunco-web/web/index.html (shared template)
  msl/
    manifest.json    # bundle metadata + content hashes
    sources-<sha>.tar.zst   # ~2 MB MSL source bundle
    parsed-<sha>.bin.zst    # ~14 MB pre-parsed StoredDefinitions
  worker/
    lunica_worker.js, lunica_worker_bg.wasm   # separate worker bundle
    worker_bootstrap.js                       # REQUIRED adapter (see Bootstrap)
target/wasm32-unknown-unknown/web-release/<binary>.wasm   # cargo's raw output
target/web/<binary>/                          # wasm-bindgen intermediate
.cargo-bin/                                    # optional local wasm-bindgen install
```

`dist/` and `.cargo-bin/` are git-ignored.

## Performance — time-to-interactive

Three levers, wired into the build + page:

1. **`wasm-opt` (build step, ~40% smaller).** `build_web.sh` runs
   `wasm-opt -Oz --converge --strip-debug` if binaryen is on PATH (`-Oz`
   size-first, `--converge` re-runs to fixpoint). Typical: 103.9 MB →
   ~60 MB. The Rust side also contributes — `[profile.web-release]` sets
   `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, `strip = true`,
   `panic = "abort"`.
2. **Streaming compile (page-side, free).** `crates/lunco-web/web/index.html`
   fetches the wasm via a `TransformStream` and hands the live `Response`
   to `init()`, so the browser pipes it into
   `WebAssembly.instantiateStreaming` — compiling chunks as they download.
3. **Brotli + gzip pre-compression at deploy (~3–4× on the wire).**
   `scripts/deploy_web.sh` emits `.br` (`-q 11 --large_window=24`) and
   `.gz` (`-9`) siblings for `wasm/js/html/json/css/svg/ts/xml/txt/map`;
   already-compressed formats (zstd/png/woff2) are skipped. `python -m
   http.server` won't serve these — production needs `brotli_static on;
   gzip_static on;`.

| stage                       | size      |
|-----------------------------|-----------|
| Rust release                | ~104 MB   |
| `-Oz` + `panic=abort` + LTO | ~70 MB    |
| `wasm-opt -Oz --converge`   | ~60 MB    |
| gzip -9 on the wire         | ~14–16 MB |
| brotli -q 11 on the wire    | ~11–13 MB |

Still costs time: Bevy plugin construction at boot (auditing `bevy`
features to drop unused renderers would help but is shared with the
rover/viz bins), and the ~16 MB MSL fetch (non-blocking; status in the
bottom egui bar).

## Maintaining the rumoca fork

The fork lives at `LunCoSim/rumoca`; the web build pulls branch
`wasm-asset-loader` (adds `Session::load_source_root_in_memory` on top of
`main`). Local dev typically uses a sibling worktree at `../rumoca/` with
`path = …` deps in `lunco-modelica/Cargo.toml` / `lunco-assets/Cargo.toml`.
To update:

```bash
cd ../rumoca
git fetch origin
git checkout wasm-asset-loader
git rebase origin/main        # replay our diff on top of upstream
git push --force-with-lease
```

Verify it's wired in:

```bash
cargo metadata --format-version 1 | jq '.packages[] | select(.name == "rumoca-sim") | .source'
```

## Deployment

`scripts/deploy_web.sh` pre-compresses the bundle (brotli + gzip) and
rsyncs it to a remote host.

**Local setup:** `sudo apt install brotli binaryen` (deploy still runs
gzip-only with a warning if brotli is missing).

**Remote (nginx + brotli module):** `sudo apt install nginx
libnginx-mod-http-brotli`. Site config:

```nginx
server {
    listen 443 ssl http2;
    server_name lunco.example;
    root /var/www/lunco;

    # application/wasm is required for streaming compile —
    # most nginx installs don't ship it.
    types {
        application/wasm        wasm;
        application/javascript  js mjs;
    }

    brotli_static on;       # drop if libnginx-mod-http-brotli absent
    gzip_static   on;

    location ~* \.(?:wasm|js|css|tar\.zst|bin\.zst)$ {
        add_header Cache-Control "public, max-age=31536000, immutable";
    }
    location = /index.html { add_header Cache-Control "no-cache"; }
    index index.html;
}
```

```bash
./scripts/build_web.sh build lunica
./scripts/deploy_web.sh deploy@host:/var/www/lunco
```

`deploy_web.sh` env vars: `BIN` (default `lunica`), `DEPLOY_TARGET`
(rsync dest, overrides positional), `SSH_PORT`, `EXTRA_RSYNC` (e.g. `-n`
for dry-run). Verify on the wire:

```bash
curl -I -H "Accept-Encoding: br"   https://lunco.example/lunica_bg.wasm   # → Content-Encoding: br
curl -I -H "Accept-Encoding: gzip" https://lunco.example/lunica_bg.wasm   # → Content-Encoding: gzip
```

If `br` is missing, `libnginx-mod-http-brotli` isn't loaded — install it
or remove the `brotli_static on;` line and rely on gzip.

## Troubleshooting (build / web)

| Symptom                                     | Cause                                       | Fix                                                       |
|---------------------------------------------|---------------------------------------------|-----------------------------------------------------------|
| `time not implemented on this platform`     | direct `std::time::Instant` usage           | use `web_time::Instant` (or rely on the rumoca fork)      |
| `thread::spawn` / `failed to spawn thread`  | raw `std::thread::spawn` on wasm            | `AsyncComputeTaskPool::get().spawn(async {…}).detach()`   |
| Blank/dark canvas, no UI                    | wasm loaded, Bevy not painted yet           | check console for plugin-build panics; loader hides on first egui frame |
| 404 on `lunica.js`                          | stale `dist/` after a layout change         | re-run `./scripts/build_web.sh build …`                   |
| `[MSL] failed: …` in status bar             | `dist/<bin>/msl/manifest.json` missing/corrupt | re-run build (`build_msl_assets` regenerates)          |
| Model errors `unresolved type reference: Modelica.*` | compile fired before MSL ready     | wait for "MSL · ready" then Compile again                 |
| `wasm-opt` step says `not installed`        | binaryen not on PATH                        | see Prerequisites; install or skip                        |
| Compile spinner forever, no `[worker]` logs | worker bundle didn't init                   | missing/broken `worker_bootstrap.js` (see Bootstrap)      |
| `br` missing on the wire                    | `libnginx-mod-http-brotli` not loaded       | install it or drop `brotli_static on;`                    |
