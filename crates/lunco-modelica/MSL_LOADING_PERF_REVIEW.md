# MSL Loading Workflow — Performance Review

Date: 2026-06-14. Scope: how the Modelica Standard Library (~2670 `.mo` files) is
loaded/parsed on **native** and **web**, and how web caching behaves across reloads.
All findings verified against source in-session (`git` tree clean; nothing modified).

## TL;DR

The architecture is already good: MSL is **pre-parsed once** by a bundler into a
bincode `Vec<(String, StoredDefinition)>` (native `parsed-msl.bin` ~316 MB raw; web
`parsed-<sha>.bin.zst`), so the runtime installs it via
`Session::replace_parsed_source_set` instead of parsing 2670 files (~27 min on wasm).

The inefficiencies are in the **install/redundancy/caching** layer, not the design:

- **Native re-reads + re-deserializes the 316 MB bundle up to 3× per compiler**, via
  three independent code paths that don't share the memoized slot that already exists.
- **`std::fs::read` + `bincode::deserialize` doubles peak memory** (~632 MB transient).
- **Native bundle is stored uncompressed** (316 MB cold disk read) while web uses zstd.
- **Web re-pays the full decompress + deserialize on every page reload** — CacheStorage
  caches the *bytes*, but the deserialized AST lives only in the wasm heap and is rebuilt
  each load, on the **main thread**.
- **`manifest.json` is cached-forever** (cache-first), so new MSL bundles are never picked
  up until the user manually clears browser storage (in-code TODO, real correctness bug).

---

## Findings, ranked by impact

### 1. [HIGH, native] In-process triple-load of the 316 MB bundle; no shared memoization
`crates/lunco-modelica/src/lib.rs` + `src/msl_remote.rs`

Three separate implementations each `std::fs::read` + `bincode::deserialize` the *same*
`parsed-msl.bin`, none sharing the result:

1. `ModelicaCompiler::new()` → `preload_from_global()` (`lib.rs:~306-314`, the
   `MslAssetSource::Filesystem` fast path) reads + deserializes the bundle.
2. `ModelicaCompiler::load_source_root("Modelica", …)` (`lib.rs:~591-598`) reads +
   deserializes the **same file again** when MSL is lazily loaded via the `LoadSourceRoot`
   worker command after an empty-session `new()`.
3. `msl_remote::parsed_msl_bundle()` (`msl_remote.rs:~74-100`) is a **memoizing** path
   (caches into the `GLOBAL_PARSED_MSL` `OnceLock`) — but the two `lib.rs` paths above
   bypass it and do their own raw read+deserialize.

`grep` confirms **no `OnceLock`/`static`/`Lazy` memoization in `lib.rs`** for the
deserialized `Vec<(String, StoredDefinition)>`. So every fresh `ModelicaCompiler`
(`indexer.rs:1424`, `bin/modelica_run.rs:237`, `experiments_runner.rs:668`, 6 test sites,
and every web `reset_compiler()` re-init) re-pays a full 316 MB read + bincode decode
(~1–3 s each, the comment's own number).

**Fix:** route *all* native loads through `msl_remote::parsed_msl_bundle()` so the file
is read + deserialized **once per process** into `GLOBAL_PARSED_MSL: Arc<Vec<…>>`;
`new()` and `load_source_root` then install from that Arc. Kills paths (1)+(2) duplication
and shares across every compiler in the process.

### 2. [HIGH, web] Decompress + deserialize repeats on every reload, on the main thread
`crates/lunco-modelica/src/msl_remote.rs` — `fetch_bytes_cached` (~1296), `drive_msl_main_decode` (~199-285)

CacheStorage (`lunco-msl-v1`) caches the **compressed blob bytes**, so a reload skips the
network. But the deserialized `Vec<(String, StoredDefinition)>` lives only in the wasm
heap (`GLOBAL_PARSED_MSL`) and **cannot** be persisted to CacheStorage (it stores `Response`
bytes, not Rust graphs). So **every page load re-runs**:

- Phase 1: ruzstd streaming decompress of the blob (`DECOMPRESS_CHUNK = 8 MB`/frame).
- Phase 2: bincode-deserialize ~2670 deep ASTs (`DESER_CHUNK = 96` docs/frame) — the
  step whose un-chunked version "froze the page for seconds" (module comment).

Both run on the **main thread** (system `drive_msl_main_decode`, "main-thread decode
complete" log), time-sliced so it doesn't hard-hang but still delays MSL-ready by seconds
each reload and steals frame budget.

**Fixes (in increasing order of effort/payoff):**
- Cache the **decompressed** bincode bytes in CacheStorage as a synthetic `Response`
  keyed by `parsed-<sha>` — reload then skips ruzstd entirely (decompress of tens of MB is
  cheaper than re-fetch but the deserialize remains).
- Move decode to the **Web Worker** (the off-thread worker bin already exists and can
  `install_global_parsed_msl_pub`), so the main thread never pays it.
- Real elimination requires a zero-copy format (see #6) — blocked upstream.

### 3. [HIGH, web — correctness] `manifest.json` cached-forever → stale MSL never updates
`crates/lunco-modelica/src/msl_remote.rs:~1283-1296` (in-code `TODO`)

`fetch_bytes_cached` is **cache-first for everything, including `manifest.json`**. The
content-hashed blobs (`parsed-<sha>.bin.zst`) are safe to cache forever, but `manifest.json`
is the indirection that points at the current hash. Caching it forever means a client that
loaded once **never sees a new MSL release** until it manually clears browser storage.

**Fix:** stale-while-revalidate for `manifest.json` only — serve cached immediately for
startup speed, refetch in the background, update the cache for next reload. Keep cache-first
for the immutable `*-<sha>.*` blobs.

### 4. [MEDIUM, native] `std::fs::read` + `deserialize` doubles peak memory (~632 MB)
`crates/lunco-modelica/src/lib.rs:~306` and `:~591`

`std::fs::read(&bundle_path)` materializes the full **316 MB** `Vec<u8>`, which stays alive
while `bincode::deserialize::<Vec<…>>(&bytes)` allocates the entire AST on top → ~2× peak.

**Fix:** `bincode::deserialize_from(BufReader::new(File::open(bundle_path)?))` streams the
file and never holds the whole byte buffer — roughly halves transient peak and avoids a
316 MB allocation+copy. (Pair with #1 so it happens once.)

### 5. [MEDIUM, native] Bundle stored uncompressed (316 MB cold read)
`crates/lunco-assets/src/bin/build_msl_assets.rs:~230` (`serialise_parsed`) vs the web `.zst`

The native artifact is raw bincode (`parsed-msl.bin`, ~316 MB); the web artifact is the
same data zstd-compressed (tens of MB). The native runtime pays a 316 MB **cold disk read**
every load. Reading ~tens of MB + zstd-decompress is typically faster than a 316 MB cold
read, and far smaller on disk / in the page cache. `ruzstd` is already a dependency.

**Fix:** write the native bundle as `parsed-msl.bin.zst` too and decode via the existing
`decode_parsed_bundle` path (already used on web). Unifies native+web on one artifact shape.

### 6. [MEDIUM, both — design lever] Monolithic bundle forces all-or-nothing deserialize
`replace_parsed_source_set` consumers (`lib.rs`, `msl_remote.rs`); format in `build_msl_assets.rs`

A model that imports only `Modelica.Blocks` still deserializes **all ~2670 docs**. The
bundle is one flat `Vec<(String, StoredDefinition)>`.

**Fix (larger):** split the bundle by top-level package (`Modelica.Blocks`, `.Mechanics`, …)
with a small index, and deserialize on demand from the `LoadSourceRoot` resolver. Biggest
lever for cutting both native and web load time and peak memory, but needs the resolver to
drive partial loads.

**Note — zero-copy is blocked upstream:** the ideal "no deserialize at all" path (rkyv
archived access, or mmap + `zerocopy`) is **not currently possible** because
`StoredDefinition` is a `rumoca` type and can't derive `rkyv::Archive` without rumoca
changes. File as an upstream ask; it's the only thing that removes the deserialize cost on
reload (#2) outright.

### 7. [LOW-MEDIUM, web] `(**parsed).clone()` deep-clones the whole AST per compiler init
`crates/lunco-modelica/src/lib.rs:~268` (preload web path)

`let docs = (**parsed).clone();` deep-clones the entire `Arc<Vec<(String, StoredDefinition)>>`
into an owned `Vec` to hand to `replace_parsed_source_set` — hundreds of MB of heap churn on
a 4 GB-capped wasm heap, repeated on each `reset_compiler()` re-init.

**Fix:** check whether `replace_parsed_source_set` can accept `Arc<[…]>` or borrow; if not,
have the first install `Arc::try_unwrap`/move rather than clone, or feed it an iterator that
moves elements out.

### 8. [LOW, native] `locate_library_file` stat-storm when building the resolver index
`crates/lunco-modelica/src/library_fs.rs` (`build_class_to_file_index` → `locate_library_file`)

For each qualified name, `locate_library_file` walks every root × every prefix length doing
`Path::exists()` on `package.mo` and the flat `.mo` sibling — many `stat()` syscalls per
class. `build_class_to_file_index` calls it for **every** palette class → thousands of stats
at startup. It's one-shot (memoized into the `INDEX` `OnceLock`), so impact is a startup
blip, native-only.

**Fix:** walk the MSL tree once into a `qualified → PathBuf` map and look up, instead of
probing the filesystem per candidate.

### 9. [LOW, web] `class_to_file_index` retries the build un-memoized until MSL lands
`crates/lunco-modelica/src/library_fs.rs:~35-50`

While MSL isn't loaded yet (web), `class_to_file_index()` returns an **empty placeholder
without memoizing** so it retries once MSL arrives. Each pre-MSL call re-checks
`msl_class_library()` (which *is* memoized) and returns empty — cheap, but if called
per-frame by resolve/autocomplete it's avoidable churn until MSL is ready.

**Fix:** gate callers on `MslLoadState::Ready` so the resolver isn't poked every frame
before MSL exists.

---

## Not problems (checked, already mitigated)

- `msl_class_library()` is fully memoized — not rebuilt per call.
- Web blob **network** fetch *is* cached (CacheStorage cache-first) — only the
  decompress/deserialize repeats (#2), and `manifest.json` freshness (#3).
- Worker compiler uses `get_or_insert_with` — constructs once per worker lifetime; the
  re-pay happens only on explicit `reset_compiler()` (web, on MSL-ready) and on fresh
  `ModelicaCompiler::new()` sites (#1).
- The lazy source-tree unpack (`ensure_msl_source_unpacked`, 37 MB) is correctly deferred
  to first drill-in, not done at boot.

## Couldn't fully verify (review agents hit the session limit before the verify pass)

- Whether `build_msl_assets::pre_parse` / `indexer` parse the 2670 files **sequentially**.
  `parse_to_ast` is called in a loop (`build_msl_assets.rs:217`); a Cargo.toml comment
  references rumoca's `parse_files_parallel`. **Action:** confirm; if sequential, parallelize
  with rayon — embarrassingly parallel, build-time only but it gates web deploys.

## Suggested order of work

1. #1 + #4 together — memoize native load through the existing `Arc` slot + stream the read
   (one change, removes the duplicate reads and halves peak). Highest native win, low risk.
2. #3 — stale-while-revalidate for `manifest.json` (small, fixes a real staleness bug).
3. #2 — cache decompressed bytes and/or move web decode to the worker (biggest web reload win).
4. #5 — compress the native bundle (unifies the artifact shape).
5. #6 / zero-copy upstream ask — the structural lever, scheduled separately.
