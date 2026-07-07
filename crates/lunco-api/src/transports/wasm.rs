//! In-browser JS bridge for the API.
//!
//! The native transport binds a `TcpListener` and serves `/api/commands` over
//! axum. A browser can't bind a socket, so this module exposes the *same*
//! bridge core through a `#[wasm_bindgen]` async export instead:
//!
//! ```js
//! const res = await window.lunco_api('{"command":"FastRunActiveModel", ...}');
//! ```
//!
//! The JSON envelope is byte-identical to the HTTP API. `execute()` awaits a
//! `tokio::sync::oneshot`, which `wasm-bindgen-futures` surfaces to JS as a
//! real `Promise`; the response resolves once the ECS produces it (a frame or
//! two later). Screenshot responses come back as a base64 PNG string under
//! `data.png_base64` rather than the HTTP `image/png` body.

use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use base64::{engine::general_purpose::STANDARD, Engine};

use crate::schema::{ApiRequest, ApiResponse};
use crate::transports::envelope::{ApiRequestUnified, ApiResponseEnvelope};
use crate::transports::HttpBridge;

thread_local! {
    /// The bridge tx, installed by `LunCoApiPlugin` during `build()`. `None`
    /// until the app is constructed — calls before then return "not ready".
    static WASM_BRIDGE: RefCell<Option<HttpBridge>> = const { RefCell::new(None) };
}

/// Register the bridge so `lunco_api` can reach it. Called once from the
/// plugin on the wasm build.
pub fn set_wasm_bridge(bridge: HttpBridge) {
    WASM_BRIDGE.with(|b| *b.borrow_mut() = Some(bridge));
}

/// JS-callable command bridge. Accepts the same `{"command"|"type", ...}`
/// envelope as the HTTP API and returns the JSON response envelope as a
/// string. Resolves a Promise on the JS side.
#[wasm_bindgen]
pub async fn lunco_api(json: String) -> Result<String, JsValue> {
    // Clone the bridge out before any await — never hold the RefCell borrow
    // across an await point.
    let bridge = WASM_BRIDGE
        .with(|b| b.borrow().clone())
        .ok_or_else(|| JsValue::from_str("lunco_api: app not ready (bridge unset)"))?;

    let req: ApiRequest = serde_json::from_str::<ApiRequestUnified>(&json)
        .map_err(|e| JsValue::from_str(&format!("lunco_api: bad request JSON: {e}")))?
        .into();

    let resp = bridge
        .execute(req)
        .await
        .map_err(|_| JsValue::from_str("lunco_api: request dropped (app shutting down?)"))?;

    // Screenshots can't ride the standard envelope (raw bytes) — hand JS a
    // base64 PNG under a stable key instead.
    if let ApiResponse::Screenshot { png_bytes } = resp {
        let envelope = ApiResponseEnvelope {
            command_id: None,
            data: Some(serde_json::json!({ "png_base64": STANDARD.encode(&png_bytes) })),
            error: None,
        };
        return serde_json::to_string(&envelope)
            .map_err(|e| JsValue::from_str(&format!("lunco_api: encode failed: {e}")));
    }

    let envelope = ApiResponseEnvelope::from(resp);
    serde_json::to_string(&envelope)
        .map_err(|e| JsValue::from_str(&format!("lunco_api: encode failed: {e}")))
}

/// JS-callable rhai one-shot for the browser console and in-app tools:
///
/// ```js
/// await window.lunco_rhai('restart_scene(); pause()');
/// ```
///
/// Thin convenience over [`lunco_api`] that wraps `code` in the `RunRhai`
/// envelope, so the web build runs rhai through the **exact same** ECS dispatch
/// as the native `sandbox rhai` client and MCP — no sockets. Returns the JSON
/// response envelope string; its `data` carries the script's captured stdout.
#[wasm_bindgen]
pub async fn lunco_rhai(code: String) -> Result<String, JsValue> {
    let bridge = WASM_BRIDGE
        .with(|b| b.borrow().clone())
        .ok_or_else(|| JsValue::from_str("lunco_rhai: app not ready (bridge unset)"))?;

    let req = crate::rhai_request(&code)
        .map_err(|e| JsValue::from_str(&format!("lunco_rhai: {e}")))?;

    let resp = bridge
        .execute(req)
        .await
        .map_err(|_| JsValue::from_str("lunco_rhai: request dropped (app shutting down?)"))?;

    let envelope = ApiResponseEnvelope::from(resp);
    serde_json::to_string(&envelope)
        .map_err(|e| JsValue::from_str(&format!("lunco_rhai: encode failed: {e}")))
}
