# lunco-storage

**I/O abstraction for LunCoSim.**

A small crate that defines the [`Storage`] trait and ships the handles
used everywhere a document is read or written: the native filesystem
today, IndexedDB / OPFS / File-System-Access / HTTPS / IPFS tomorrow.
Higher-level crates (`lunco-doc`, `lunco-workspace`, `lunco-twin`) go
through this trait, so they compile unchanged when the app grows a
browser or remote-twin backend.

Headless, UI-free, ECS-free. Pulling `lunco-storage` into a crate does
not pull in bevy or egui.

## The shape

```text
┌────────────────────┐        ┌───────────────────────┐
│   Storage (trait)  │   ←    │   FileStorage         │  (native, this crate)
│                    │        └───────────────────────┘
│  read / write      │        ┌───────────────────────┐
│  exists            │   ←    │   OpfsStorage         │  (future, wasm)
│  is_writable       │        └───────────────────────┘
│  pick_open         │        ┌───────────────────────┐
│  pick_save         │   ←    │   IdbStorage          │  (future, wasm)
│  pick_folder       │        └───────────────────────┘
└────────────────────┘        ┌───────────────────────┐
          │                   │   HttpStorage         │  (future, remote)
          ▼                   └───────────────────────┘
    StorageHandle
      · File(PathBuf)            — active today
      · Memory(String)           — active today (tests)
      · Fsa(token)               — feature-stub
      · Idb { db, key }          — feature-stub
      · Opfs(String)             — feature-stub
      · Http(url)                — feature-stub
```

Only the `File` and `Memory` variants are live; the rest are declared
behind feature flags (`fsa_stub`, `idb_stub`, `opfs_stub`, `http_stub`)
so consumers can keep their match arms exhaustive without waiting for
the backend to ship.

## Usage

`Storage::entry_kind` follows native symbolic links for reads. Native
`entry_kind_no_follow_file_sync` identifies the entry itself and returns
`StorageEntryKind::Symlink` even for a broken link. Both use one native metadata
classifier; OPFS never emits this kind. The rename owner resolves and checks the
parent directory before moving an entry, and rejects existing distinct targets, including dangling links. A case-only
rename to the same canonical entry is admitted; the workbench canonicalizes
the command's Twin root before looking up its owner.

`FilePathIdentity::prepare` resolves the deepest existing ancestor for a native
output destination on its I/O worker. `overlaps` compares the carried snapshot
without I/O, including missing descendants and Windows directory case mode.
Ambiguous Windows names fail visibly when the case-mode query fails. Dataset
registration and merge use this mechanism to protect source/output ownership.

Native `FileStorage::write` publishes complete bytes through an atomic rename.
Each concurrent write reserves its own staging file; replacement and create-only
`write_new` share this staging owner. `write_new` preserves an existing destination.
Consumers must use these storage methods for concurrent cache persistence.

Native `FileStorage::lock_cache_directory` canonicalizes a File directory and
returns a `DirectoryCacheTransaction` holding a separately opened exclusive
standard-library file lock. Drop releases the lock on Unix and Windows. The
persistent `.cache-lock` file must never be removed or replaced. Encode heavy
artifacts before acquiring it; use existing Storage writes/deletes inside the
transaction. Its `files()` iterator streams direct regular-file handles and
sizes without following symlinks or collecting the whole directory. Consumers
choose their bounded retention set and close each iterator before deleting
entries. Generic `cache_directory_transaction_` tests exercise independent
writers, child-process contention, release, and regular-file enumeration.

`FileStorage::read_bounded(handle, max_bytes)` and the matching OPFS method
enforce caller-owned read budgets before materializing full contents. Native
reads take at most the limit plus one sentinel byte, including concurrent file
growth; memory reads check length before cloning. OPFS checks the immutable
`File` snapshot's size before allocating its array buffer or Rust bytes.
Oversized inputs return `StorageError::SizeLimitExceeded { max_bytes }`.
Optional cache and artifact readers use these methods before decoding records.

Native worker consumers use `FileStorage::read_chunks_bounded(handle, max_bytes,
consume)` to process regular-file bytes through a fixed 64 KiB buffer. It returns
the actual byte count and rejects growth past the caller's `u64` budget before
passing the rejected chunk to the consumer. Empty files work with a zero remaining
budget. It follows regular-file symlinks like ordinary Storage reads, but checks
regular-file metadata before and after opening; these checks do not claim safety
against concurrent hostile filesystem replacement. This method performs synchronous
I/O and belongs on an existing worker, never the UI thread.

`OpfsStorage` exposes asynchronous `read_directory` with the same File-handle
mapping and sorted direct-child results as native storage. Callers bound catalog
processing and metadata reads; directory enumeration returns the complete list.

Native file URI conversion belongs to `file_uri_to_path` and
`file_path_to_uri`. They delegate URL parsing and encoding to `url`, including
percent-encoded spaces and Unicode, local authorities, and Windows drive/UNC
paths. `file_uri_to_path` returns `None` only for non-file spellings; callers
may preserve a raw native path or route another scheme in that case. Invalid
file URIs return an error and must not be retried as literal paths. Browser
builds reject file URIs because they have no native filesystem. Use encoded
`%23` and `%3F` for filename characters; URI fragments and queries are rejected.

```rust
use lunco_storage::{FileStorage, Storage, StorageHandle};

let storage = FileStorage::new();
let handle = StorageHandle::File("/tmp/hello.mo".into());

storage.write(&handle, b"model Hello end Hello;")?;
let bytes = storage.read(&handle)?;
assert_eq!(bytes, b"model Hello end Hello;");
```

Pickers are synchronous on native (`rfd::FileDialog` blocks while the
OS dialog is up — standard behaviour). When the wasm backend lands it
will expose the same method signatures via a feature-gated async
variant; consumer code flips one line.

## Where this fits

- **`lunco-doc`**: Document trait doesn't touch the filesystem directly.
- **`lunco-twin`**: `Twin::root_handle()` returns a `StorageHandle`;
  `Twin::owns(&StorageHandle)` is how the Workspace decides which
  documents belong to which Twin.
- **`lunco-workspace`**: Session state references documents and twins
  by `StorageHandle`, so a session can mix native files, remote twins,
  and OPFS-backed scratch docs in one window.
- **`lunco-modelica-ui::ui::commands::on_save_as_document`**: Invokes
  `FileStorage::pick_save` + `FileStorage::write`; the only native-ish
  code is the backend choice.

## Design intent

- **One trait, many backends.** Adding a backend never touches the
  consumer. Feature-gated `StorageHandle` variants preserve exhaustive
  matches across the transition.
- **Sync where possible.** Reads and writes are synchronous because the
  common case (small text files) completes in microseconds; a backend
  that truly needs async (HTTP) can block on a short-lived executor
  behind the trait.
- **Pickers block the thread, not the app.** `rfd` blocks the calling
  thread for the duration of the OS dialog. That's acceptable — the
  user is looking at a modal — and saves us async-trait overhead.

## Not yet

- External-change watcher (`notify` on native, `storage` events on
  web). Planned as `Storage::watch(handle) -> Stream<Event>`.
- Transactions / multi-file atomic writes for future Twin-level saves.
