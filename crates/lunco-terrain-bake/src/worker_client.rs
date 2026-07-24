//! Main-thread client for the off-thread DEM bake worker (`dem_worker`).
//!
//! Thin wrapper over the shared [`lunco_worker_transport::WorkerPool`] (the SAME
//! generic transport the Modelica Fast-Run workers use): it owns one DEM worker,
//! encodes a [`DemBakeJob`] + transfers the ~40 MB GeoTIFF as a zero-copy
//! `ArrayBuffer`, and queues the worker's replies for the terrain systems to
//! drain. The worker emits TWO replies per job — a [`BakeStage::Coarse`] preview,
//! then [`BakeStage::Full`].
//!
//! Single-threaded page → `thread_local!` state (same pattern as the Modelica
//! transport). Only ONE DEM worker: bakes are rare (scene load / regenerate) and
//! a second instance would just double memory.

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use js_sys::{Array, Float64Array, Object, Reflect, Uint8Array};
use lunco_obstacle_field::field::HeightGrid;
use lunco_worker_transport::{Callbacks, WorkerPool};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::{BakeReplyHeader, BakeStage, DemBakeJob};

/// One stage's result routed back to the terrain systems. `grid` is the stamped
/// working grid (or an error); `id` correlates with the dispatched entity.
pub struct WorkerReply {
    pub id: u32,
    pub stage: BakeStage,
    pub site: String,
    pub native_res: usize,
    pub res: usize,
    pub grid: Result<HeightGrid, String>,
}

thread_local! {
    static POOL: RefCell<Option<WorkerPool>> = const { RefCell::new(None) };
    static WORKER_URL: RefCell<Option<String>> = const { RefCell::new(None) };
    static REPLIES: RefCell<VecDeque<WorkerReply>> = RefCell::new(VecDeque::new());
    /// Ids dispatched but not yet terminated (a `Full` reply, an error, or a
    /// [`cancel`] ends a job; `Coarse` keeps it). On a worker-level crash
    /// (`on_error`) these have no pending terminal reply, so the terrain systems
    /// would wait forever — we synthesise error replies for them so the job cleans
    /// up. A `HashSet` (not a `Vec`): retiring was an O(n) `retain` per reply, and a
    /// re-dispatched id must not be tracked twice.
    static INFLIGHT: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
    /// Worker 0 crashed and its slot still holds the DEAD `Worker`. `WorkerPool`
    /// delegates respawn to the caller (it must be DEFERRED — respawning from inside
    /// `on_error` would re-enter the pool mid-borrow), so we only flag it here and
    /// rebuild the worker on the next [`ensure_pool`]. Without this, `ensure(1)` sees
    /// an occupied slot, posts a 40 MB TIFF into the wedged worker, and the bake
    /// hangs forever — every bake for the rest of the session.
    static NEEDS_RESPAWN: Cell<bool> = const { Cell::new(false) };
}

/// Mark a job terminated (drop its id from the in-flight set).
fn retire_inflight(id: u32) {
    INFLIGHT.with(|f| f.borrow_mut().remove(&id));
}

/// Stop tracking `id` — call when the job's terrain entity is despawned before its
/// `Full` reply lands. Without it the id lives in `INFLIGHT` for the rest of the
/// session: the set grows unboundedly and the "failing N in-flight bake(s)" count on
/// a crash reports jobs nobody is waiting for.
pub fn cancel(id: u32) {
    retire_inflight(id);
}

/// Number of bakes currently in flight (dispatched, no terminal reply yet).
pub fn inflight_count() -> usize {
    INFLIGHT.with(|f| f.borrow().len())
}

/// Register the worker bootstrap URL (e.g. `./dem-worker/dem_worker_bootstrap.js`)
/// WITHOUT spawning — the worker is created lazily on the first [`dispatch`].
pub fn set_worker_url(url: &str) {
    WORKER_URL.with(|u| *u.borrow_mut() = Some(url.to_string()));
}

/// Whether a worker URL has been registered (i.e. the web build staged the DEM
/// worker). Callers fall back to the inline path when this is false.
pub fn is_available() -> bool {
    WORKER_URL.with(|u| u.borrow().is_some())
}

/// Ensure the single DEM worker is spawned and HEALTHY, creating the pool on first
/// use and rebuilding a crashed worker (the deferred respawn the transport contract
/// requires — see [`NEEDS_RESPAWN`]).
fn ensure_pool() -> Result<(), JsValue> {
    POOL.with(|p| {
        if p.borrow().is_none() {
            let url = WORKER_URL
                .with(|u| u.borrow().clone())
                .ok_or_else(|| JsValue::from_str("dem worker url not set"))?;
            // The DEM worker posts only structured-object replies (never the
            // handshake string), so no wire-id enforcement is needed here.
            let cbs = Callbacks {
                on_message: Rc::new(|_idx, data| handle_reply(data)),
                on_ready: Callbacks::noop(),
                on_error: Rc::new(|idx| {
                    // A worker-level crash posts no terminal reply, so every job it
                    // was baking would leave its entity pending forever. Synthesise a
                    // terminal error reply per in-flight id (a `Coarse` error makes the
                    // consumer drop both the request and the job) — and flag the dead
                    // worker for a DEFERRED respawn, or the next dispatch posts into
                    // its corpse and hangs forever.
                    NEEDS_RESPAWN.with(|f| f.set(true));
                    let stuck: Vec<u32> = INFLIGHT.with(|f| f.borrow_mut().drain().collect());
                    web_sys::console::warn_1(
                        &format!(
                            "[dem-worker] worker {idx} errored — failing {} in-flight bake(s)",
                            stuck.len()
                        )
                        .into(),
                    );
                    for id in stuck {
                        REPLIES.with(|r| {
                            r.borrow_mut().push_back(WorkerReply {
                                id,
                                stage: BakeStage::Coarse,
                                site: String::new(),
                                native_res: 0,
                                res: 0,
                                grid: Err("dem worker crashed".to_string()),
                            })
                        });
                    }
                }),
                on_wire_mismatch: Rc::new(|_idx, _got| {}),
            };
            *p.borrow_mut() = Some(WorkerPool::new(url, None, "DEM_WIRE:", cbs));
            NEEDS_RESPAWN.with(|f| f.set(false));
        }
        let mut pool = p.borrow_mut();
        let pool = pool.as_mut().unwrap();
        // Deferred crash recovery: the slot still holds the dead worker, so `ensure`
        // would consider it live. Terminate + rebuild it BEFORE anything is posted.
        if NEEDS_RESPAWN.with(|f| f.replace(false)) {
            web_sys::console::warn_1(&"[dem-worker] respawning crashed worker 0".into());
            if let Err(e) = pool.respawn(0) {
                // Re-arm: the next dispatch tries again (the caller falls back to the
                // inline bake path meanwhile).
                NEEDS_RESPAWN.with(|f| f.set(true));
                return Err(e);
            }
        }
        pool.ensure(1)
    })
}

/// Decode one worker reply (a `{ header: Uint8Array, heights?: ArrayBuffer }`
/// object) and queue it for the terrain systems to drain.
fn handle_reply(data: JsValue) {
    let header_bytes = match Reflect::get(&data, &JsValue::from_str("header")) {
        Ok(v) if !v.is_undefined() => v.unchecked_into::<Uint8Array>().to_vec(),
        _ => {
            web_sys::console::error_1(&"[dem-worker] reply missing header".into());
            return;
        }
    };
    let header: BakeReplyHeader = match bincode::serde::decode_from_slice::<BakeReplyHeader, _>(
        &header_bytes,
        bincode::config::standard(),
    ) {
        Ok((h, _)) => h,
        Err(e) => {
            web_sys::console::error_2(&"[dem-worker] bad header".into(), &e.to_string().into());
            return;
        }
    };

    let grid = if let Some(err) = header.err {
        Err(err)
    } else {
        match Reflect::get(&data, &JsValue::from_str("heights")) {
            Ok(buf) if !buf.is_undefined() => {
                // ArrayBuffer (transferred) → view → copy into wasm memory.
                let heights = Float64Array::new(&buf).to_vec();
                // `res * res` OVERFLOWS usize on wasm32 (32-bit) for `res ≥ 65536` —
                // a truncated/foreign header could wrap the product back into a
                // plausible length and defeat this very guard. `checked_mul` → reject.
                let Some(expected) = header.res.checked_mul(header.res) else {
                    retire_inflight(header.id);
                    REPLIES.with(|r| {
                        r.borrow_mut().push_back(WorkerReply {
                            id: header.id,
                            stage: header.stage,
                            site: header.site.clone(),
                            native_res: header.native_res,
                            res: header.res,
                            grid: Err(format!("worker grid res {} is impossible", header.res)),
                        })
                    });
                    return;
                };
                if heights.len() != expected {
                    // A truncated/foreign buffer would later index out of bounds far
                    // from here (`heights[z*res+x]`); fail the reply at the source.
                    Err(format!(
                        "worker grid size mismatch: got {} heights, expected {expected} ({}²)",
                        heights.len(),
                        header.res
                    ))
                } else {
                    Ok(HeightGrid {
                        res: header.res,
                        half_extent: header.half_extent,
                        heights,
                    })
                }
            }
            _ => Err("worker reply missing heights buffer".to_string()),
        }
    };

    // A `Full` reply or any error terminates the job; `Coarse` keeps it in flight.
    if header.stage == BakeStage::Full || grid.is_err() {
        retire_inflight(header.id);
    }

    REPLIES.with(|r| {
        r.borrow_mut().push_back(WorkerReply {
            id: header.id,
            stage: header.stage,
            site: header.site,
            native_res: header.native_res,
            res: header.res,
            grid,
        })
    });
}

/// Dispatch a bake into the worker. `tif` (the ~40 MB GeoTIFF, already fetched on
/// the main thread) is TRANSFERRED to the worker (zero-copy, detaching this
/// copy). The worker replies asynchronously via [`drain_replies`].
pub fn dispatch(id: u32, job: &DemBakeJob, site_id: &str, tif: &[u8]) -> Result<(), JsValue> {
    ensure_pool()?;
    let job_bytes = bincode::serde::encode_to_vec(job, bincode::config::standard())
        .map_err(|e| JsValue::from_str(&format!("job encode: {e}")))?;

    let job_arr = Uint8Array::new_with_length(job_bytes.len() as u32);
    job_arr.copy_from(&job_bytes);
    let site_arr = Uint8Array::new_with_length(site_id.len() as u32);
    site_arr.copy_from(site_id.as_bytes());
    // Fresh JS-heap copy of the tif so its backing buffer can be transferred.
    let tif_arr = Uint8Array::new_with_length(tif.len() as u32);
    tif_arr.copy_from(tif);
    let tif_buf = tif_arr.buffer();

    let obj = Object::new();
    Reflect::set(&obj, &"id".into(), &JsValue::from_f64(id as f64))?;
    Reflect::set(&obj, &"job".into(), &job_arr)?;
    Reflect::set(&obj, &"site".into(), &site_arr)?;
    Reflect::set(&obj, &"tif".into(), &tif_buf)?;

    let transfer = Array::of1(&tif_buf);
    POOL.with(|p| {
        p.borrow()
            .as_ref()
            .unwrap()
            .post_transfer(0, &obj, &transfer)
    })?;
    INFLIGHT.with(|f| f.borrow_mut().insert(id));
    Ok(())
}

/// Inject a locally-produced reply (e.g. an OPFS grid-cache hit) into the SAME
/// queue the worker's replies land in, so cache hits drive the identical
/// reply-consumption path as a [`BakeStage::Full`] worker reply — no worker
/// spawned, no duplicated oracle-composition logic downstream.
pub fn push_local_reply(reply: WorkerReply) {
    REPLIES.with(|r| r.borrow_mut().push_back(reply));
}

/// Take all worker replies received since the last call (drains the queue).
pub fn drain_replies() -> Vec<WorkerReply> {
    REPLIES.with(|r| r.borrow_mut().drain(..).collect())
}
