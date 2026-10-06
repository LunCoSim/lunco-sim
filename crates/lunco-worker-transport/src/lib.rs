//! Generic Web Worker pool transport — the payload-agnostic plumbing shared by
//! the Modelica Fast-Run workers (`lunco-modelica-execution::worker_transport`) and the
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
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{ErrorEvent, MessageEvent, Worker, WorkerOptions, WorkerType};

/// Caller-supplied event handlers. All are `Rc<dyn Fn>` so the pool can keep them
/// alive across respawns and share them into every worker's `onmessage` closure.
/// They run on the main thread. The caller must release its pool borrow before
/// dispatching an event that can call one of these handlers.
#[derive(Clone)]
pub struct Callbacks {
    /// A non-handshake message arrived from worker `idx`. `data` is the raw
    /// `MessageEvent.data` (a `Uint8Array` of bincode, or a bare/objected
    /// `ArrayBuffer` for transferred bulk) — the caller decodes it.
    pub on_message: Rc<dyn Fn(usize, JsValue)>,
    /// Worker `idx` announced a wire id matching ours → it booted and its
    /// protocol is compatible. Optional readiness hook (e.g. flush a queue).
    pub on_ready: Rc<dyn Fn(usize)>,
    /// Worker `idx` fired `onerror` (panic / OOM). The caller owns replacement
    /// admission and failure handling; replacement can occur inside this callback
    /// when the caller is not already borrowing the pool.
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
/// Each slot owns its worker and callbacks. Retirement detaches handlers,
/// terminates the worker and drops its owned closures. wasm-bindgen retains an
/// executing owned closure until its invocation returns, so replacement from a
/// callback does not require leaking that callback.
///
/// `Worker` (a `JsValue`) and the `Rc` handlers are `!Send`, but
/// wasm32-unknown-unknown is single-threaded and the pool is only ever touched
/// from the main thread — so it's `unsafe impl Send + Sync` to live in a caller's
/// `OnceLock<Mutex<_>>`.
pub struct WorkerPool {
    url: String,
    /// Expected wire-build id the worker announces on boot; `None` = no handshake
    /// (the worker need not send one). Guards against a stale companion wasm.
    wire_id: Option<String>,
    handshake_prefix: String,
    slots: Vec<Option<WorkerSlot>>,
    cbs: Callbacks,
}

// SAFETY: wasm32-unknown-unknown has no threads; the pool never leaves the main
// thread. The Send/Sync bounds only exist to satisfy a static `Mutex`.
unsafe impl Send for WorkerPool {}
unsafe impl Sync for WorkerPool {}

struct WorkerSlot {
    worker: Worker,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_error: Closure<dyn FnMut(ErrorEvent)>,
}

impl Drop for WorkerSlot {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
    }
}

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
        self.slots
            .get(idx)
            .and_then(|slot| slot.as_ref())
            .map(|slot| &slot.worker)
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
                // Worker 0 failing is fatal; a later failure caps the pool.
                Err(e) if idx == 0 => return Err(e),
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// Discard and rebuild worker `idx` (crash recovery / grow-only-memory
    /// recycle). The new instance re-announces its wire id and re-runs the boot
    /// handshake; the caller re-seeds any per-worker state. Retire the old slot
    /// before spawning, so a failed replacement leaves no terminated live slot.
    /// An executing owned callback remains valid until its invocation returns.
    pub fn respawn(&mut self, idx: usize) -> Result<(), JsValue> {
        if let Some(slot) = self.slots.get_mut(idx) {
            slot.take();
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
        let worker = self
            .worker(idx)
            .ok_or_else(|| JsValue::from_str("worker not spawned"))?;
        let array = Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(bytes);
        worker.post_message(&array)
    }

    /// Post `msg` to worker `idx`, TRANSFERRING the buffers in `transfer`
    /// (zero-copy; the source buffers detach). `msg` is any JS value — typically
    /// an object bundling small headers with the transferred `ArrayBuffer`s.
    pub fn post_transfer(
        &self,
        idx: usize,
        msg: &JsValue,
        transfer: &Array,
    ) -> Result<(), JsValue> {
        let worker = self
            .worker(idx)
            .ok_or_else(|| JsValue::from_str("worker not spawned"))?;
        worker.post_message_with_transfer(msg, transfer)
    }

    /// Build one worker + wire its `onmessage` (handshake demux → `on_message`)
    /// and `onerror` (→ `on_error`) closures, owned by the resulting slot.
    fn make_worker(&self, idx: usize) -> Result<WorkerSlot, JsValue> {
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

        let on_err_cb = self.cbs.on_error.clone();
        let on_error = Closure::wrap(Box::new(move |e: ErrorEvent| {
            web_sys::console::error_2(
                &format!("[worker-transport] worker {idx} error").into(),
                &e.message().into(),
            );
            (on_err_cb)(idx);
        }) as Box<dyn FnMut(ErrorEvent)>);
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        Ok(WorkerSlot {
            worker,
            _on_message: on_message,
            _on_error: on_error,
        })
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen(
        inline_js = "export function worker_test_url() { return URL.createObjectURL(new Blob(['self.onmessage = () => {};'], {type: 'text/javascript'})); } export function revoke_worker_test_url(url) { URL.revokeObjectURL(url); }"
    )]
    extern "C" {
        fn worker_test_url() -> String;
        fn revoke_worker_test_url(url: &str);
    }

    struct WorkerUrl(String);

    impl Drop for WorkerUrl {
        fn drop(&mut self) {
            revoke_worker_test_url(&self.0);
        }
    }

    fn callbacks(message: Rc<dyn Fn(usize, JsValue)>, error: Rc<dyn Fn(usize)>) -> Callbacks {
        Callbacks {
            on_message: message,
            on_ready: Callbacks::noop(),
            on_error: error,
            on_wire_mismatch: Rc::new(|_, _| {}),
        }
    }

    #[wasm_bindgen_test]
    fn worker_lifecycle_replacement_and_drop_release_callbacks_and_retire_delivery() {
        let url = WorkerUrl(worker_test_url());
        let delivered = Rc::new(Cell::new(0));
        let message: Rc<dyn Fn(usize, JsValue)> = {
            let delivered = Rc::clone(&delivered);
            Rc::new(move |_, _| delivered.set(delivered.get() + 1))
        };
        let error = Callbacks::noop();
        let mut pool = WorkerPool::new(
            &url.0,
            None,
            "",
            callbacks(Rc::clone(&message), Rc::clone(&error)),
        );
        pool.ensure(1).unwrap();
        let message_refs = Rc::strong_count(&message);
        let error_refs = Rc::strong_count(&error);
        for _ in 0..5 {
            let retired = pool.worker(0).unwrap().clone();
            pool.respawn(0).unwrap();
            assert!(retired.onmessage().is_none());
            assert!(retired.onerror().is_none());
            let before = delivered.get();
            retired
                .dispatch_event(&MessageEvent::new("message").unwrap())
                .unwrap();
            assert_eq!(
                delivered.get(),
                before,
                "retired worker must not deliver into its replacement"
            );
            pool.worker(0)
                .unwrap()
                .dispatch_event(&MessageEvent::new("message").unwrap())
                .unwrap();
            assert_eq!(delivered.get(), before + 1);
            assert_eq!(
                Rc::strong_count(&message),
                message_refs,
                "replacement must release the old callback capture"
            );
            assert_eq!(Rc::strong_count(&error), error_refs);
        }
        let last = pool.worker(0).unwrap().clone();
        drop(pool);
        assert!(last.onmessage().is_none());
        assert!(last.onerror().is_none());
        assert_eq!(Rc::strong_count(&message), 1);
        assert_eq!(Rc::strong_count(&error), 1);
    }

    #[wasm_bindgen_test]
    fn worker_lifecycle_can_replace_from_an_executing_owned_callback() {
        let url = WorkerUrl(worker_test_url());
        let pool: Rc<RefCell<Option<WorkerPool>>> = Rc::new(RefCell::new(None));
        let weak = Rc::downgrade(&pool);
        let error: Rc<dyn Fn(usize)> = Rc::new(move |idx| {
            weak.upgrade()
                .unwrap()
                .borrow_mut()
                .as_mut()
                .unwrap()
                .respawn(idx)
                .unwrap();
        });
        let message: Rc<dyn Fn(usize, JsValue)> = Rc::new(|_, _| {});
        let mut initial = WorkerPool::new(
            &url.0,
            None,
            "",
            callbacks(Rc::clone(&message), Rc::clone(&error)),
        );
        initial.ensure(1).unwrap();
        let before = Rc::strong_count(&error);
        *pool.borrow_mut() = Some(initial);
        let retired = pool.borrow().as_ref().unwrap().worker(0).unwrap().clone();
        retired
            .dispatch_event(&ErrorEvent::new("error").unwrap())
            .unwrap();
        assert!(retired.onerror().is_none());
        assert_eq!(
            Rc::strong_count(&error),
            before,
            "active invocation must release its callback after returning"
        );
        pool.borrow_mut().take();
        assert_eq!(Rc::strong_count(&message), 1);
        assert_eq!(Rc::strong_count(&error), 1);
    }

    #[wasm_bindgen_test]
    fn worker_lifecycle_failed_replacement_retires_the_slot_before_recovery() {
        let url = WorkerUrl(worker_test_url());
        let message: Rc<dyn Fn(usize, JsValue)> = Rc::new(|_, _| {});
        let error = Callbacks::noop();
        let mut pool = WorkerPool::new(
            &url.0,
            None,
            "",
            callbacks(Rc::clone(&message), Rc::clone(&error)),
        );
        pool.ensure(1).unwrap();
        let retired = pool.worker(0).unwrap().clone();
        pool.url = "http://[".into();
        assert!(pool.respawn(0).is_err());
        assert!(pool.worker(0).is_none());
        assert!(retired.onmessage().is_none());
        assert!(retired.onerror().is_none());
        assert_eq!(
            Rc::strong_count(&message),
            2,
            "only the caller and empty pool retain the handler"
        );
        pool.url = url.0.clone();
        pool.ensure(1).unwrap();
        drop(pool);
        assert_eq!(Rc::strong_count(&message), 1);
        assert_eq!(Rc::strong_count(&error), 1);
    }
}
