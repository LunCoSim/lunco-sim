# lunco-twin

**Twin** is LunCoSim's top-level persistent artifact — a folder of
domain-standard files plus a `twin.toml` manifest. This crate defines
the container shape (folder + manifest + file index) and handles
file-system I/O.

UI-free, ECS-free, headless-capable. Depends only on `lunco-doc`,
`serde`, `toml`, `walkdir`, and `thiserror`.

See [`docs/architecture/13-twin-and-workflow.md`](../../docs/architecture/13-twin-and-workflow.md)
for the design narrative.

## Core types

| Type | Role |
|------|------|
| [`TwinMode`] | `Orphan(PathBuf)` / `Folder(Twin)` / `Twin(Twin)` — the three ways to open content |
| [`Twin`] | Loaded folder: `root` + optional `manifest` + file index |
| [`TwinManifest`] | Serde-backed `twin.toml` (name, version, optional description + default workspace) |
| [`FileEntry`] | One discovered file: `relative_path` + `kind` |
| [`FileKind`] | `Document(DocumentKind)` / `FileReference` / `Unknown` |
| [`DocumentKind`] | Which domain owns the file (`Modelica`, `Usd`, `Sysml`, `Mission`, `Data`, `Other`) |
| [`TwinError`] | `thiserror`-based error type |

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
        for doc in twin.documents() {
            println!("  doc: {:?}  {}", doc.kind, doc.relative_path.display());
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
3. **`twin.toml` is never indexed as content.** It is metadata
   *about* the Twin, not a Document inside it.
4. **Dotfile directories (`.lunco/`, `.git/`, `.vscode/`) are
   skipped** — but the Twin root itself may live under a dotfile
   parent (e.g. `/tmp/.tmpXXX`) without breaking indexing.

## What this crate does NOT do (yet)

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
- **Endpoint tracking** (FMI slaves, Nucleus connections) — named
  as a future concern in the design docs; not modeled yet.

## Tests

```bash
cargo test -p lunco-twin
```

24 unit tests + 1 doctest cover: three-mode open, manifest
round-trip, classification of every built-in extension (Modelica,
USD variants, SysML, mission compound extensions, data, textures,
meshes, audio, video, docs, unknown), dotfile-directory skipping,
promotion folder → Twin, manifest save/load, reload picking up new
files, error on missing path, error on save without manifest.

## Crate graph

```
lunco-doc           ← shape of Documents / ops / undo
   ▲
   │ used by
   └── lunco-twin   ← this crate (folder + manifest + file index)
          ▲
          │ used by
          └── apps, domain crates (they load Documents from the files Twin lists)
```
