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
- **`lunco-modelica::ui::commands::on_save_as_document`**: Invokes
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
