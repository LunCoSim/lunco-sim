# 40 — Asset I/O policy

> Status: Active · Audience: contributors writing crates that read or write assets

**TL;DR.** Domain crates read assets through `bevy::asset::AssetServer` —
never `std::fs::read*`, `std::thread::spawn`, `std::time::Instant`, or
`tokio::fs`. These are denied by clippy **on the wasm target**, which is the only
place they are actually true (see *Enforcement layers*); a wasm build gate in CI
catches anything that slips through (transitive deps, std API drift).

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
  `https://`, `lunco://`), so swapping out where the bytes come
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

**There are two clippy configs, and the split is the whole point.**

| Layer | Catches | Where |
|---|---|---|
| `ci/wasm-lint/clippy.toml` — **config only; no workflow invokes it** | The wasm-portability bans — `std::fs::*`, `std::thread::spawn`, `std::time::Instant::now` — intended to run with `--target wasm32-unknown-unknown` and `CLIPPY_CONF_DIR=ci/wasm-lint` on the crates that ship to the browser | `ci/wasm-lint/clippy.toml` |
| `clippy.toml` at the repo root — **config only; no workflow invokes it** | Everything that is genuinely wrong *on native* too: the `big_space` re-parenting atomicity contract (`add_child` / `set_parent_in_place` → `migrate_to_grid`) and the USD stage deep-clone (`TextReader::clone`) | `clippy.toml` |
| `scripts/check_wasm.sh` — **run by hand; no workflow invokes it** | **Anything** that breaks the wasm *link*, including transitive deps that pull `mio`/`tokio-fs`/`std::thread`. Strongest check, but it is not a gate until something calls it. | `scripts/check_wasm.sh` |
| This file | Human awareness | here |

> ### Why the portability bans do NOT run on native
>
> **Native is the one target where they cannot be true**, and enforcing them there
> is what made clippy unusable in this repo:
>
> - **`std::fs` / `std::thread`** — the overwhelming majority of call sites are
>   already inside `#[cfg(not(target_arch = "wasm32"))]`, where using them is
>   *correct*. Native clippy sees that code anyway and flags it.
> - **`std::time::Instant::now`** — worse. `web_time` supplies its native impl via
>   `pub use std::time::*`, so on native `web_time::Instant` **is**
>   `std::time::Instant` — the same DefId. Clippy resolves straight through the
>   re-export and flags every *correct* caller: **73 hits in `lunco-modelica`
>   alone, all false, not one true.**
>
> **A lint that is wrong every single time it fires is a lint people silence** —
> and a silenced lint is how the hole stayed open. On `wasm32`, `cfg` strips the
> native-only code before clippy sees it and the two `Instant` types are genuinely
> distinct, so the bans fire **only** on code that will actually break in a browser.
>
> **Do not move these entries back into the root `clippy.toml`.**

**Known debt, counted rather than hidden:** `lunco-modelica` has ~16 `std::fs`
calls that *are* reachable on wasm (the MSL indexer, the package browser, the icon
loader) — the long-standing "MSL missing on the web" symptom. They are **not**
`#[allow]`ed: a non-fatal CI step prints the count and the sites every run, and it
must trend to zero. An `#[allow]` would hide the debt *and* blind the gate to new
wasm bugs in those same files.

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
