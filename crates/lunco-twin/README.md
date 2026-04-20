# lunco-twin

**Twin** is LunCoSim's top-level persistent artifact — a folder of
domain-standard files plus an optional `twin.toml` manifest. This
crate defines the container shape (folder + manifest + file index +
recursive sub-twins) and handles file-system I/O.

UI-free, ECS-free, headless-capable. Depends on `lunco-doc`,
`lunco-storage`, `serde`, `toml`, `walkdir`, and `thiserror`.

See [`docs/architecture/13-twin-and-workflow.md`](../../docs/architecture/13-twin-and-workflow.md)
for the design narrative.

## Recursion — Cargo-style

A Twin may nest child Twins via `[[children]]` in `twin.toml`:

```toml
# rover-twin/twin.toml
name = "Rover"
version = "0.1.0"

[[children]]
name = "engine"
path = "engine"         # → rover-twin/engine/twin.toml

[[children]]
name = "chassis"
path = "chassis"

[[children]]
name = "shared-sensors"
url = "https://twins.lunco.space/sensors"   # remote ref (not followed today)
```

Opening a Twin eagerly loads every local child (cycles guarded,
missing folders tolerated with warnings). Remote URL children are
listed on the manifest but not followed yet — reserved for the
remote-twin milestone.

## Core types

| Type | Role |
|------|------|
| [`TwinMode`] | `Orphan(PathBuf)` / `Folder(Twin)` / `Twin(Twin)` — the three ways to open content |
| [`Twin`] | Loaded folder: `root` + optional `manifest` + file index + sub-twins |
| [`TwinManifest`] | Serde-backed `twin.toml` (name, version, optional description + default perspective + `children`) |
| [`TwinChildRef`] | One `[[children]]` entry — `name` + (`path` or `url`) |
| [`FileEntry`] | One discovered file: `relative_path` + `kind` |
| [`FileKind`] | `Document(DocumentKind)` / `FileReference` / `Unknown` |
| [`DocumentKind`] | Which domain owns the file (`Modelica`, `Usd`, `Sysml`, `Mission`, `Data`, `Other`) |
| [`TwinError`] | `thiserror`-based error type |

## Key methods on `Twin`

| Method | What it does |
|--------|-------------|
| `files()` | Flat list of every discovered file (non-recursive). |
| `documents()` / `file_references()` | Filters over `files()`. |
| `children()` | Sub-twins loaded from `[[children]]` with local `path`. |
| `walk()` | Depth-first iterator over self + all descendants. |
| `root_handle()` | `StorageHandle::File(root)` — bridge to `lunco-storage`. |
| `owns(handle)` | Returns `true` if `handle`'s path is under this Twin's folder or any sub-Twin's folder. **Core predicate for the Workspace's document-routing rule.** |
| `find_owning(handle)` | Returns the **deepest** Twin in the subtree whose folder contains `handle` (sub-Twins win over their parent — matches Cargo's "nearest Cargo.toml" rule). |
| `promote_to_twin(manifest)` | Writes `twin.toml` into a plain folder, registering it as a Twin. |
| `save_manifest()` / `reload()` | Manifest persistence + folder re-scan. |

## Minimal usage

```rust,no_run
use lunco_twin::{TwinMode, TwinManifest};
use std::path::Path;

match TwinMode::open(Path::new("./my_base"))? {
    TwinMode::Orphan(path) => println!("single file: {}", path.display()),
    TwinMode::Folder(twin) => {
        println!("{} files in folder (no twin.toml)", twin.files().len());
    }
    TwinMode::Twin(twin) => {
        let m = twin.manifest.as_ref().unwrap();
        println!("Opened Twin: {} (v{})", m.name, m.version);
        for sub in twin.walk() {
            if let Some(mf) = &sub.manifest {
                println!("  sub-twin: {} @ {}", mf.name, sub.root.display());
            }
        }
    }
}
# Ok::<(), lunco_twin::TwinError>(())
```

## Design rules

1. **Classification by extension only.** `lunco-twin` never opens or
   parses file contents. A `.mo` is a Modelica Document because its
   extension says so; validating it is the domain crate's job.
2. **All indexed paths are relative to the Twin root.** The index
   survives moving the folder.
3. **`twin.toml` is never indexed as content.** It is metadata *about*
   the Twin, not a Document inside it.
4. **Dotfile directories (`.lunco/`, `.git/`, `.vscode/`) are
   skipped** — but the Twin root itself may live under a dotfile
   parent (e.g. `/tmp/.tmpXXX`) without breaking indexing.
5. **Twin is a *view* over Documents.** A Twin's `owns(handle)` says
   whether a document *belongs* to it; it does NOT hold a
   `Vec<Document>`. Documents live in
   [`lunco-workspace`](../lunco-workspace/README.md); Twin just
   answers membership questions.

## What this crate does NOT do (yet)

- **Simulation control.** Clock, pause, compile scope, PauseTwin /
  ResumeTwin / StepTwin commands — planned as a feature-gated `ecs`
  module (`TwinPlugin` + Bevy components) in a follow-up milestone.
  Today Twin is purely a file-system descriptor.
- **Load / parse Documents.** Each domain crate owns its parser.
  Twin tracks paths and kinds only.
- **Runtime `DocumentRegistry`** holding live `DocumentHost<D>`
  instances — that belongs to apps/domain crates, not here.
- **`TwinTransaction`** for atomic multi-Document ops — deferred
  until we have a real multi-Document op site.
- **Cache registry** (AST, DAE, compiled artifacts) — deferred
  until we have a cache consumer.
- **File watching** (inotify / FSEvents) — manual `Twin::reload()`
  for now.

## Tests

```bash
cargo test -p lunco-twin
```

28 unit tests + 1 doctest cover: three-mode open, manifest round-trip
(including `children` array), classification of every built-in
extension, dotfile-directory skipping, promotion folder → Twin,
manifest save/load, reload picking up new files, recursive child
load, missing-child tolerance, `owns` predicate (hierarchy with
deepest-wins), `walk` visiting every twin in tree, error paths.

## Crate graph

```
lunco-doc           ← shape of Documents / ops / undo
lunco-storage       ← Storage trait + StorageHandle
   ▲
   │ both used by
   └── lunco-twin   ← this crate (folder + manifest + file index + recursion)
          ▲
          │ used by
          ├── lunco-workspace    ← "editor session — which Twins are open"
          └── apps, domain crates
```
