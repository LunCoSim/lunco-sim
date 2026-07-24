//! Off-thread DEM bake worker — wasm32-unknown-unknown only.
//!
//! Runs inside a Web Worker with its own wasm linear memory. Receives a bake job
//! ({ id, job: bincode `DemBakeJob`, site: id bytes, tif: transferred
//! GeoTIFF `ArrayBuffer` }) from the main page, decodes the GeoTIFF ONCE, then
//! emits two `postMessage` replies — a coarse preview grid, then the full-res
//! grid — each as { header: bincode `BakeReplyHeader`, heights: transferred f64
//! `ArrayBuffer` }. This moves the ~15-30 s decode + crater stamp off the page's
//! main thread; it calls the SAME `lunco_terrain_bake::finish_bake` the native
//! async task uses.
//!
//! Mirrors `lunica_worker`: a separate wasm-bindgen bin (own linear memory), no
//! atomics / SharedArrayBuffer. The native build gets an inert stub `main`.

// Wasm32-only binary; the desktop stub keeps host-target `cargo build` passing.
fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    panic!("dem_worker is wasm32-only — built into a Web Worker bundle by scripts/build_web.sh.");
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use js_sys::{Array, Float64Array, Object, Reflect, Uint8Array};
    use lunco_terrain_bake::{
        decode_raw, finish_bake, BakeReplyHeader, BakeStage, BakedGrid, DemBakeJob,
    };
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

    fn scope() -> DedicatedWorkerGlobalScope {
        js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>()
    }

    fn get_u8(data: &JsValue, key: &str) -> Vec<u8> {
        Reflect::get(data, &JsValue::from_str(key))
            .ok()
            .map(|v| v.unchecked_into::<Uint8Array>().to_vec())
            .unwrap_or_default()
    }

    /// Post one baked stage back to the main thread, transferring its heights.
    fn post_baked(id: u32, baked: &BakedGrid) {
        let header = BakeReplyHeader {
            id,
            stage: baked.stage,
            err: None,
            site: baked.site.clone(),
            res: baked.res,
            half_extent: baked.grid.half_extent,
            native_res: baked.native_res,
        };
        let Ok(header_bytes) = bincode::serde::encode_to_vec(&header, bincode::config::standard())
        else {
            return;
        };
        let header_arr = Uint8Array::new_with_length(header_bytes.len() as u32);
        header_arr.copy_from(&header_bytes);

        let heights = &baked.grid.heights;
        let h_arr = Float64Array::new_with_length(heights.len() as u32);
        h_arr.copy_from(heights);
        let h_buf = h_arr.buffer();

        let obj = Object::new();
        let _ = Reflect::set(&obj, &"header".into(), &header_arr);
        let _ = Reflect::set(&obj, &"heights".into(), &h_buf);
        let transfer = Array::of1(&h_buf);
        if let Err(e) = scope().post_message_with_transfer(&obj, &transfer) {
            web_sys::console::error_2(&"[dem_worker] post failed".into(), &e);
        }
    }

    /// Post a failure header (no heights) for the given stage.
    fn post_error(id: u32, stage: BakeStage, err: String) {
        let header = BakeReplyHeader {
            id,
            stage,
            err: Some(err),
            site: String::new(),
            res: 0,
            half_extent: 0.0,
            native_res: 0,
        };
        let Ok(bytes) = bincode::serde::encode_to_vec(&header, bincode::config::standard()) else {
            return;
        };
        let arr = Uint8Array::new_with_length(bytes.len() as u32);
        arr.copy_from(&bytes);
        let obj = Object::new();
        let _ = Reflect::set(&obj, &"header".into(), &arr);
        let _ = scope().post_message(&obj);
    }

    #[wasm_bindgen]
    extern "C" {
        fn setTimeout(handler: &js_sys::Function, timeout: i32);
    }

    async fn yield_to_event_loop() {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            setTimeout(&resolve, 0);
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    fn handle(ev: MessageEvent) {
        let data = ev.data();
        let id = Reflect::get(&data, &JsValue::from_str("id"))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as u32;

        let job: DemBakeJob = match bincode::serde::decode_from_slice::<DemBakeJob, _>(
            &get_u8(&data, "job"),
            bincode::config::standard(),
        ) {
            Ok((j, _)) => j,
            Err(e) => return post_error(id, BakeStage::Coarse, format!("job decode: {e}")),
        };
        // The site id — the cache/identity key, and the one fact the raster does
        // not carry. The raster is self-describing, so only the name travels.
        let site_bytes = get_u8(&data, "site");
        let site_id = String::from_utf8_lossy(&site_bytes).to_string();
        // The tif rode a transferred ArrayBuffer → view it, copy into wasm memory.
        let tif = match Reflect::get(&data, &JsValue::from_str("tif")) {
            Ok(buf) if !buf.is_undefined() => Uint8Array::new(&buf).to_vec(),
            _ => return post_error(id, BakeStage::Coarse, "message missing tif".into()),
        };

        // ONE decode shared by both stages (the expensive GeoTIFF parse).
        let raw = match decode_raw(&tif) {
            Ok(v) => v,
            Err(e) => return post_error(id, BakeStage::Coarse, e),
        };
        drop(tif);

        // Coarse preview first (fast → terrain + collider appear)
        let baked_coarse = finish_bake(&raw, &site_id, &job, BakeStage::Coarse);
        post_baked(id, &baked_coarse);

        // Yield to the event loop so the coarse preview message is dispatched immediately,
        // then refine the full grid in the background.
        wasm_bindgen_futures::spawn_local(async move {
            yield_to_event_loop().await;
            let baked_full = finish_bake(&raw, &site_id, &job, BakeStage::Full);
            post_baked(id, &baked_full);
        });
    }

    #[wasm_bindgen(start)]
    #[allow(unreachable_pub)]
    pub fn run() -> Result<(), JsValue> {
        console_error_panic_hook::set_once();
        let cb = Closure::wrap(Box::new(handle) as Box<dyn FnMut(MessageEvent)>);
        scope().set_onmessage(Some(cb.as_ref().unchecked_ref()));
        cb.forget(); // handler lives for the worker's lifetime
        web_sys::console::log_1(&"[dem_worker] ready".into());
        Ok(())
    }
}
