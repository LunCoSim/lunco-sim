// Web Worker entry shim for the off-thread Modelica worker bundle.
//
// `wasm-bindgen --target web` produces an ES module that EXPORTS an `init`
// function but does NOT auto-instantiate the wasm — module-level code only
// declares imports/exports. When the main page does `new Worker(URL, { type:
// 'module' })` the browser loads this shim first; we pull in `lunica_worker.js`
// and call `init()` so `#[wasm_bindgen(start)] fn run()` actually fires.
//
// The relative path matches the layout produced by `scripts/build_web.sh`:
//   dist/lunica/worker/lunica_worker.js
//   dist/lunica/worker/worker_bootstrap.js  ← this file
import init from './lunica_worker.js';

// Buffer messages that arrive before WASM initializes to prevent races.
let bootQueue = [];
let isBooted = false;

self.onmessage = (e) => {
    if (!isBooted) {
        console.log("[worker_bootstrap] Queued early message", e);
        bootQueue.push(e);
    }
};

await init();

// `run()` (annotated `#[wasm_bindgen(start)]`) has now executed and
// installed `self.onmessage`. The worker is ready to receive WireMessage
// payloads from the main page.
isBooted = true;

let wasmHandler = self.onmessage;
if (wasmHandler) {
    console.log(`[worker_bootstrap] Replaying ${bootQueue.length} messages`);
    for (let e of bootQueue) {
        wasmHandler(e);
    }
} else {
    console.error("[worker_bootstrap] WASM failed to install onmessage handler");
}
bootQueue = null;
