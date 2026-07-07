//! Generic Web Worker pool transport — the payload-agnostic plumbing shared by
//! the Modelica Fast-Run workers (`lunico-modelica::worker_transport`) and the
//! DEM bake worker (`lunco-terrain-bake::worker_client`).
//!
//! wasm32 has no OS threads, so multi-second companion work (a Modelica compile,
//! a DEM decode + crater stamp) would freeze the page. Each pool member is a JS
//! `Worker` running a *second* wasm instance with its own linear memory; work is
//! posted as bytes (bincode) or Transferable `ArrayBuffer`s (zero-copy) and
//! results come back through a caller-registered [`Callbacks::on_message`].
//!
//! This crate owns ONLY the generic concerns: spawn / lazy-grow, the boot
//! wire-id handshake (stale-worker guard), byte + transfer posting, and crash
//! respawn. Message framing, readiness gating, and result routing stay with the
//! caller, which wraps a [`WorkerPool`] in its own singleton and supplies the
//! [`Callbacks`]. Native builds compile this to nothing.
#![cfg(target_arch = "wasm32")]

use std::rc::Rc;

use js_sys::{Array, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{ErrorEvent, MessageEvent, Worker, WorkerOptions, WorkerType};

/// Caller-supplied event handlers. All are `Rc<dyn Fn>` so the pool can keep them
/// alive across respawns and share them into every worker's `onmessage` closure.
/// They run on the main thread; none may re-enter the pool's `&mut` methods
/// (respawn on error must be *deferred* by the caller — see [`Callbacks::on_error`]).
#[derive(Clone)]
pub struct Callbacks {
    /// A non-handshake message arrived from worker `idx`. `data` is the raw
    /// `MessageEvent.data` (a `Uint8Array` of bincode, or a bare/objected
    /// `ArrayBuffer` for transferred bulk) — the caller decodes it.
    pub on_message: Rc<dyn Fn(usize, JsValue)>,
    /// Worker `idx` announced a wire id matching ours → it booted and its
    /// protocol is compatible. Optional readiness hook (e.g. flush a queue).
    pub on_ready: Rc<dyn Fn(usize)>,
    /// Worker `idx` fired `onerror` (panic / OOM). The caller should schedule a
    /// DEFERRED [`WorkerPool::respawn`] (calling it from here would re-enter the
    /// pool mid-borrow); right after an OOM the heap is also too starved to
    /// re-seed immediately.
    pub on_error: Rc<dyn Fn(usize)>,
    /// Worker `idx` announced a wire id that DISAGREES with ours — the shipped
    /// worker wasm is stale; every bincode message will mis-decode. Surface loudly.
    pub on_wire_mismatch: Rc<dyn Fn(usize, String)>,
}

impl Callbacks {
    /// A no-op default for hooks a caller doesn't need.
    pub fn noop() -> Rc<dyn Fn(usize)> {
        Rc::new(|_| {})
    }
}

/// A pool of identical Web Workers loading `url`. Payload-agnostic; the caller
/// wraps it in its own singleton and drives it via [`Callbacks`].
///
/// The `onmessage`/`onerror` closures are `.forget()`-leaked into the JS runtime
/// rather than owned here — deliberately, and exactly as the Modelica pool always
/// has: a worker's `onerror`/`onmessage` can trigger a synchronous [`respawn`],
/// and dropping a `Closure` while it is executing is undefined behaviour. Leaking
/// makes the callbacks permanent; respawns are crash/recycle-only and rare, so the
/// few-KB-per-respawn leak is negligible.
///
/// `Worker` (a `JsValue`) and the `Rc` handlers are `!Send`, but
/// wasm32-unknown-unknown is single-threaded and the pool is only ever touched
/// from the main thread — so it's `unsafe impl Send + Sync` to live in a caller's
/// `OnceLock<Mutex<_>>`, exactly as the Modelica pool always has.
pub struct WorkerPool {
    url: String,
    /// Expected wire-build id the worker announces on boot; `None` = no handshake
    /// (the worker need not send one). Guards against a stale companion wasm.
    wire_id: Option<String>,
    handshake_prefix: String,
    slots: Vec<Option<Worker>>,
    cbs: Callbacks,
}

// SAFETY: wasm32-unknown-unknown has no threads; the pool never leaves the main
// thread. The Send/Sync bounds only exist to satisfy a static `Mutex`.
unsafe impl Send for WorkerPool {}
unsafe impl Sync for WorkerPool {}

impl WorkerPool {
    /// Create an empty pool. Nothing spawns until [`WorkerPool::ensure`].
    pub fn new(
        url: impl Into<String>,
        wire_id: Option<String>,
        handshake_prefix: impl Into<String>,
        cbs: Callbacks,
    ) -> Self {
        Self {
            url: url.into(),
            wire_id,
            handshake_prefix: handshake_prefix.into(),
            slots: Vec::new(),
            cbs,
        }
    }

    /// Number of worker slots (spawned or reserved).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The live `Worker` at `idx`, if spawned.
    pub fn worker(&self, idx: usize) -> Option<&Worker> {
        self.slots.get(idx).and_then(|s| s.as_ref())
    }

    /// Grow the pool to at least `n` live workers (idempotent — already-spawned
    /// slots are left untouched). Worker 0 spawning is fatal (returns `Err`);
    /// later workers that fail just leave a smaller pool.
    pub fn ensure(&mut self, n: usize) -> Result<(), JsValue> {
        for idx in 0..n {
            if self.slots.get(idx).and_then(|s| s.as_ref()).is_some() {
                continue;
            }
            match self.make_worker(idx) {
                Ok(worker) => {
                    if idx < self.slots.len() {
                        self.slots[idx] = Some(worker);
                    } else {
                        self.slots.push(Some(worker));
                    }
                }
                // Worker 0 failing is fatal (the caller falls back to inline work);
                // a later one failing just caps the pool smaller.
                Err(e) if idx == 0 => return Err(e),
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// Discard and rebuild worker `idx` (crash recovery / grow-only-memory
    /// recycle). The new instance re-announces its wire id and re-runs the boot
    /// handshake; the caller re-seeds any per-worker state in `on_ready`. Safe to
    /// call synchronously from within a worker callback (the old closures were
    /// leaked, not owned, so nothing executing is dropped).
    pub fn respawn(&mut self, idx: usize) -> Result<(), JsValue> {
        if let Some(Some(old)) = self.slots.get(idx) {
            old.terminate();
        }
        let worker = self.make_worker(idx)?;
        if idx >= self.slots.len() {
            self.slots.resize_with(idx + 1, || None);
        }
        self.slots[idx] = Some(worker);
        Ok(())
    }

    /// Post raw `bytes` (a fresh `Uint8Array` copy) to worker `idx`.
    pub fn post(&self, idx: usize, bytes: &[u8]) -> Result<(), JsValue> {
        let worker = self.worker(idx).ok_or_else(|| JsValue::from_str("worker not spawned"))?;
        let array = Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(bytes);
        worker.post_message(&array)
    }

    /// Post `msg` to worker `idx`, TRANSFERRING the buffers in `transfer`
    /// (zero-copy; the source buffers detach). `msg` is any JS value — typically
    /// an object bundling small headers with the transferred `ArrayBuffer`s.
    pub fn post_transfer(&self, idx: usize, msg: &JsValue, transfer: &Array) -> Result<(), JsValue> {
        let worker = self.worker(idx).ok_or_else(|| JsValue::from_str("worker not spawned"))?;
        worker.post_message_with_transfer(msg, transfer)
    }

    /// Build one worker + wire its `onmessage` (handshake demux → `on_message`)
    /// and `onerror` (→ `on_error`) closures. The closures are `.forget()`-leaked
    /// (see the struct doc) so a synchronous respawn from within them is safe.
    fn make_worker(&self, idx: usize) -> Result<Worker, JsValue> {
        let opts = WorkerOptions::new();
        opts.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(&self.url, &opts)?;

        let cbs = self.cbs.clone();
        let wire_id = self.wire_id.clone();
        let prefix = self.handshake_prefix.clone();
        let on_message = Closure::wrap(Box::new(move |ev: MessageEvent| {
            let data = ev.data();
            // The boot handshake is a PLAIN STRING ("<prefix><id>") posted before
            // any bincode, so its framing survives the very protocol drift it
            // detects. Demux it out here; everything else is the caller's payload.
            if let Some(s) = data.as_string() {
                if let Some(got) = s.strip_prefix(prefix.as_str()) {
                    match &wire_id {
                        Some(expect) if got != expect => {
                            (cbs.on_wire_mismatch)(idx, got.to_string());
                        }
                        _ => (cbs.on_ready)(idx),
                    }
                    return;
                }
            }
            (cbs.on_message)(idx, data);
        }) as Box<dyn FnMut(MessageEvent)>);
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();

        let on_err_cb = self.cbs.on_error.clone();
        let on_error = Closure::wrap(Box::new(move |e: ErrorEvent| {
            web_sys::console::error_2(
                &format!("[worker-transport] worker {idx} error").into(),
                &e.message().into(),
            );
            (on_err_cb)(idx);
        }) as Box<dyn FnMut(ErrorEvent)>);
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        Ok(worker)
    }
}
