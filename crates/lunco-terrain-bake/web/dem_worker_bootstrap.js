// Web Worker entry shim for the off-thread DEM bake bundle.
//
// `wasm-bindgen --target web` produces an ES module that EXPORTS an `init`
// function but does NOT auto-instantiate the wasm. When the main page does
// `new Worker(URL, { type: 'module' })` the browser loads this shim first; we
// pull in `dem_worker.js` and call `init()` so `#[wasm_bindgen(start)] fn run()`
// fires and installs `self.onmessage`.
//
// Layout produced by scripts/build_web.sh:
//   dist/<bin>/dem-worker/dem_worker.js
//   dist/<bin>/dem-worker/dem_worker_bootstrap.js  ← this file
import init from './dem_worker.js';

// Buffer bake jobs that arrive before WASM initializes to prevent races.
let bootQueue = [];
let isBooted = false;

self.onmessage = (e) => {
    if (!isBooted) {
        bootQueue.push(e);
    }
};

await init();

// `run()` has now executed and installed the real `self.onmessage`.
isBooted = true;

let wasmHandler = self.onmessage;
if (wasmHandler) {
    for (let e of bootQueue) {
        wasmHandler(e);
    }
} else {
    console.error("[dem_worker_bootstrap] WASM failed to install onmessage handler");
}
bootQueue = null;
