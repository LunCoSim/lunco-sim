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

await init();
// `run()` (annotated `#[wasm_bindgen(start)]`) has now executed and
// installed `self.onmessage`. The worker is ready to receive WireMessage
// payloads from the main page.
