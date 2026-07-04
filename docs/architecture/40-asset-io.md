# 40 — Asset I/O policy

> Status: Active · Audience: contributors writing crates that read or write assets

**TL;DR.** Domain crates read assets through `bevy::asset::AssetServer` —
never `std::fs::read*`, `std::thread::spawn`, `std::time::Instant`, or
`tokio::fs`. Workspace clippy denies the offenders; a wasm build gate in
CI catches anything that slips through (transitive deps, std API drift).

---

## Why the policy exists

The codebase targets two platforms: native (server / desktop binary) and
wasm32 (browser sandbox). Bevy abstracts rendering and ECS scheduling
across both, but the **standard library does not** — several blocking
APIs return errors or panic on `wasm32-unknown-unknown`. Among them:

| Standard API | wasm behaviour | Domain-code symptom |
|---|---|---|
| `std::fs::read*`, `File::open` | `Err("operation not supported on this platform")` | Asset missing at runtime |
| `std::thread::spawn` | Panic | App boot crash |
| `std::time::Instant::now` | Panic | App boot crash |
| `tokio::fs::*` (default features) | Pulls `mio` → wasm link error | Compile failure |

Failure mode is **wasm-only**. Native `cargo test` passes; the browser
build silently no-ops or crashes. Without a policy, every wasm port
hunts the same class of bug — the day we wrote this doc the team spent
several hours tracing four separate instances (mio in `lunco-api`,
`std::fs` in cosim, `std::fs` in the USD composer's sublayer reads,
`crossbeam-channel` in `lunco-scripting::repl`).

The fix isn't a smarter function; it's a rule that any code reading a
shippable asset goes through one path that works on both targets.

## The one path: `AssetServer::load(...)`

Define an `Asset` + `AssetLoader` per source class once. Domain code
asks for a `Handle<T>`, observes `Assets<T>::get(&handle)` becoming
`Some`, and acts. The handle pattern is async-by-default; native and
wasm look identical at the call site.

```rust
// In a domain crate (e.g. lunco-usd-sim/src/cosim.rs):
let h: Handle<ModelicaSource> = asset_server.load("models/Balloon.mo");
commands.entity(e).insert(PendingModelicaSource(h));

// One drain system, runs every frame:
fn dispatch_when_loaded(
    mut commands: Commands,
    q: Query<(Entity, &PendingModelicaSource)>,
    sources: Res<Assets<ModelicaSource>>,
    channels: Res<ModelicaChannels>,
) {
    for (e, pending) in &q {
        let Some(src) = sources.get(&pending.0) else { continue };
        let _ = channels.tx.send(ModelicaCommand::Compile { /* ... */ });
        commands.entity(e).remove::<PendingModelicaSource>();
    }
}
```

Wins beyond wasm support:
- **Hot reload.** Edit the `.mo` file, AssetServer re-emits, drain re-fires.
- **Change detection.** `AssetEvent<ModelicaSource>` for free.
- **Single resolver.** Asset paths route through the registered
  `AssetSource` (default `assets://`, plus `embedded://`, future
  `https://`, `lunco-lib://`), so swapping out where the bytes come
  from doesn't ripple into domain code.

## Asset loaders we maintain

| Loader | Asset type | Where | Extensions |
|---|---|---|---|
| `UsdLoader` | `UsdStageAsset` | `lunco-usd-bevy` | `.usda` |
| `ModelicaSourceLoader` | `ModelicaSource` | `lunco-modelica` | `.mo` |
| `PythonSourceLoader` | `PythonSource` | `lunco-scripting` | `.py` |

New source classes get their own `Asset` + `AssetLoader` in the owning
domain crate. Loaders are dumb (parse to bytes / utf-8 / domain AST);
the dispatch logic lives in the consumer.

## Allow-list

Three classes of crate legitimately bypass `AssetServer`:

- **Filesystem-owning crates.** `lunco-assets` (download/extract/cache
  pipeline), `lunco-storage` (user-data persistence). Both are
  native-only by design.
- **Build scripts.** `*-build.rs` runs on the host at compile time.
- **Native-only binaries.** Worker subprocesses like `build_msl_assets`
  that never compile to wasm32.

To bypass the lint, the crate's `lib.rs` (or binary's `main.rs`) carries
a top-of-file:

```rust
#![allow(clippy::disallowed_methods)]
// Reason: this crate owns on-disk cache layout for the native build;
// wasm consumers go through AssetServer.
```

Adding new escapes requires PR review — the allow itself is the audit
trail.

## Enforcement layers

| Layer | Catches | Where |
|---|---|---|
| `clippy.toml` `disallowed-methods` | New direct `std::fs::*` / `std::thread::spawn` / `std::time::Instant` in workspace source | `clippy.toml` at repo root |
| `cargo clippy --workspace -- -D warnings` (CI) | Above, fails PR | (planned CI step) |
| `scripts/check_wasm.sh` (CI) | **Anything** that breaks the wasm link — including transitive deps that pull `mio`/`tokio-fs`/`std::thread`. Strongest gate. | `scripts/check_wasm.sh` |
| `docs/architecture/40-asset-io.md` (this file) | Human awareness | here |

The wasm-build step is the killer. Clippy only sees our source; the
linker sees the whole graph. Today's whole adventure (mio, crossbeam,
`std::time::Instant`, `std::fs`) would have failed CI at PR time with
no manual hunt.

## Migration status

| Site | Status |
|---|---|
| `lunco-usd-bevy/UsdLoader` | ✅ Bevy AssetLoader |
| `lunco-usd-bevy/compose.rs` compose (`flatten_stage`, injected fetcher) | ✅ injected fetcher, wasm path pre-fetches via `LoadContext::read_asset_bytes` |
| `lunco-usd-sim/cosim.rs` modelica/python source reads | ✅ migrated to AssetServer (see `ModelicaSource` / `PythonSource`) |
| `lunco-usd/src/ui/browser_dispatch.rs` twin browser open | ✅ routed to spawn_usd_load domain command |
| `lunco-usd/src/commands.rs` usd document load | ✅ reads through the storage abstraction |
| `lunco-modelica/msl_remote.rs` bundled MSL fetch | ⚠️ uses bespoke `web_sys::fetch`; folding into `EmbeddedAssetSource` / `HttpAssetSource` is a follow-up |
| `lunco-modelica::models::bundled_models()` `include_str!` | ⚠️ candidate for `EmbeddedAssetSource` registration so it looks like every other asset path |

## Related foot-guns (same rule applies)

- `std::thread::spawn` → on wasm use `AsyncComputeTaskPool` (main-thread,
  yield between items) for in-bundle work, or `lunica_worker`-style Web
  Worker bundle for genuinely concurrent work. See
  `docs/architecture/30-wasm-web-worker.md`.
- `std::time::Instant` → `web_time::Instant` (drop-in;
  `performance.now()` on wasm, real `Instant` on native).
- `tokio::fs::*`, `axum`, `mio` → drag in unsupported syscalls; gate
  behind a non-default feature on the crate exposing them
  (`transport-http` in `lunco-api`).
