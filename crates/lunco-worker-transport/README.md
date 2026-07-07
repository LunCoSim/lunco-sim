# lunco-worker-transport

The generic **Web Worker pool transport** — written once, reused by every
off-thread workload.

wasm-only (`#![cfg(target_arch = "wasm32")]`; compiles to nothing on native, where
callers use real `std::thread`). wasm32 has no OS threads, so multi-second
companion work would freeze the page; each pool member is a JS `Worker` running a
**second** wasm instance with its own linear memory, and work crosses the boundary
as copied bytes or Transferable `ArrayBuffer`s (no shared memory, no atomics /
`SharedArrayBuffer`).

## What it owns (and what it doesn't)

`WorkerPool` owns only the **payload-agnostic plumbing**:

- spawn / lazy-grow (`ensure(n)`; worker 0 fatal, later failures cap the pool),
- the boot **wire-id handshake** (a plain-string `"<prefix><id>"` the worker posts
  before any bincode — its framing survives the protocol drift it guards against),
- byte post (`post`) and Transferable-`ArrayBuffer` post (`post_transfer`),
- crash **respawn** (`respawn`; the `onmessage`/`onerror` closures are
  `.forget()`-leaked so a respawn from within a callback is safe).

Message framing, readiness gating, and result routing stay with the **caller**,
which wraps a `WorkerPool` in its own singleton and supplies `Callbacks`
(`on_message` / `on_ready` / `on_error` / `on_wire_mismatch`).

`Worker` and the `Rc` handlers are `!Send`, but the page is single-threaded, so the
pool is `unsafe impl Send + Sync` to live in a static `Mutex`.

## Used by

- **`lunco-modelica::worker_transport`** — the Modelica Fast-Run pool; composes
  `WorkerPool` and layers MSL-readiness + per-run demux on top.
- **`lunco-terrain-bake::worker_client`** — the DEM bake worker; composes the same
  pool for a one-shot "bytes in → heightfield out" job.

So the transport exists in exactly one place instead of being duplicated per
workload.
