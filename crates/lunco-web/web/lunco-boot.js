// Shared LunCoSim wasm boot loader — see crates/lunco-web/src/lib.rs.
//
// A per-app index.html is reduced to a config call:
//
//   import init, * as wasm from './lunica.js';
//   import { boot } from './lunco-boot.js';
//   const __LC_WASM_SIZE__ = 0;            // injected by build_web.sh
//   boot({ init, wasmUrl: './lunica_bg.wasm', wasmSize: __LC_WASM_SIZE__, title: 'Lunica' });
//
// `boot` injects the loader card, streams the wasm with a progress bar,
// runs `init()`, and leaves the loader up until Bevy's WebReadyPlugin
// calls `window.__lc_app_ready()` after the first frame paints.

/**
 * @param {object} cfg
 * @param {(opts:{module_or_path:Response})=>Promise<any>} cfg.init  wasm-bindgen init
 * @param {string} cfg.wasmUrl     origin-relative URL of the `*_bg.wasm`
 * @param {number} [cfg.wasmSize]  uncompressed wasm bytes (accurate progress under gzip_static)
 * @param {string} [cfg.title]     loader card title
 * @param {string} [cfg.accent]    CSS colour override for --lc-accent
 */
export async function boot({ init, wasmUrl, wasmSize = 0, title = 'LunCoSim', accent } = {}) {
    if (accent) document.documentElement.style.setProperty('--lc-accent', accent);

    // Build the loader card once — its markup lives here, not in each
    // app's html, so adding an app is a config call with no copy-paste.
    const root = document.createElement('div');
    root.id = 'lc-loading';
    const card = document.createElement('div');
    card.className = 'card';
    card.innerHTML =
        '<div class="title"></div>' +
        '<div class="phase">Loading…</div>' +
        '<div class="detail">&nbsp;</div>' +
        '<div class="bar"><div class="fill"></div></div>';
    card.querySelector('.title').textContent = title;
    root.appendChild(card);
    document.body.appendChild(root);

    // Bevy's WindowPlugin targets canvas:"#bevy"; make sure it exists.
    if (!document.getElementById('bevy')) {
        const canvas = document.createElement('canvas');
        canvas.id = 'bevy';
        document.body.appendChild(canvas);
    }

    const phaseE = card.querySelector('.phase');
    const detailE = card.querySelector('.detail');
    const fillE = card.querySelector('.bar > .fill');
    const setPhase = (t) => { phaseE.textContent = t; };
    const setDetail = (t) => { detailE.innerHTML = t || '&nbsp;'; };
    const setProgress = (pct) => {
        root.classList.remove('indeterminate');
        fillE.style.width = `${Math.max(0, Math.min(100, pct)).toFixed(1)}%`;
    };
    const setIndeterminate = () => root.classList.add('indeterminate');
    const setError = (msg) => {
        root.classList.add('error');
        root.classList.remove('indeterminate');
        setPhase('Failed');
        setDetail(msg);
        fillE.style.width = '100%';
    };

    // NOTE: no JS canvas-resize handler. Bevy's `fit_canvas_to_parent`
    // owns the canvas drawing-buffer size and writes it every frame; a JS
    // handler setting `canvas.width/height` fights the engine on every
    // resize (regressed the sandbox once — see git history). The loader
    // card is CSS-centred and needs no sizing.

    // Called by WebReadyPlugin (Rust) after the first egui frame paints.
    window.__lc_app_ready = function () {
        root.classList.add('hidden');
    };

    const fmtBytes = (n) => (n / 1048576).toFixed(1) + ' MB';
    const fmtRate = (bps) => {
        if (bps <= 0) return '';
        if (bps > 1048576) return (bps / 1048576).toFixed(1) + ' MB/s';
        if (bps > 1024) return (bps / 1024).toFixed(0) + ' KB/s';
        return bps.toFixed(0) + ' B/s';
    };
    const fmtEta = (s) => {
        if (!isFinite(s) || s <= 0) return '';
        if (s < 60) return `${Math.ceil(s)}s left`;
        const m = Math.floor(s / 60);
        return `${m}m ${Math.ceil(s - m * 60)}s left`;
    };

    // Streaming wasm load: wrap the fetch body in a TransformStream that
    // counts bytes for the progress UI, then hand the still-streaming
    // Response to init(). The browser compiles chunks as they arrive
    // (compileStreaming), overlapping download and compile.
    async function streamWasmResponse(url) {
        setPhase('Loading runtime');
        setDetail('connecting…');
        setIndeterminate();
        const resp = await fetch(url);
        if (!resp.ok) throw new Error(`HTTP ${resp.status} ${resp.statusText}`);
        // Prefer the build-time uncompressed size: under gzip_static the
        // Content-Length is the compressed size but the browser auto-
        // decompresses, so received bytes would overshoot it.
        const totalHdr = resp.headers.get('Content-Length');
        const total = wasmSize > 0 ? wasmSize : (totalHdr ? parseInt(totalHdr, 10) : 0);

        const start = performance.now();
        let lastT = start, lastReceived = 0, received = 0, speed = 0;
        const SMOOTH = 0.2;

        const counter = new TransformStream({
            transform(chunk, controller) {
                received += chunk.byteLength;
                const now = performance.now();
                const dt = (now - lastT) / 1000;
                if (dt > 0.05) {
                    const inst = (received - lastReceived) / dt;
                    speed = speed === 0 ? inst : speed * (1 - SMOOTH) + inst * SMOOTH;
                    lastT = now;
                    lastReceived = received;
                }
                if (total > 0) {
                    setProgress((received / total) * 100);
                    const eta = speed > 0 ? (total - received) / speed : 0;
                    setDetail([
                        `${fmtBytes(received)} / ${fmtBytes(total)}`,
                        fmtRate(speed),
                        fmtEta(eta),
                    ].filter(Boolean).join(' · '));
                } else {
                    setDetail([fmtBytes(received), fmtRate(speed)].filter(Boolean).join(' · '));
                }
                controller.enqueue(chunk);
            },
        });

        // Some servers omit `Content-Type: application/wasm`; force it so
        // wasm-bindgen's compileStreaming doesn't fall back to buffered.
        const headers = new Headers(resp.headers);
        headers.set('Content-Type', 'application/wasm');
        return new Response(resp.body.pipeThrough(counter), {
            status: resp.status,
            statusText: resp.statusText,
            headers,
        });
    }

    try {
        const wasmResp = await streamWasmResponse(wasmUrl);
        setPhase('Starting');
        setDetail('initialising…');
        setIndeterminate();
        await init({ module_or_path: wasmResp });
        // Loader stays up until the wasm side calls __lc_app_ready().
    } catch (e) {
        console.error(e);
        setError(String(e && e.message ? e.message : e));
    }
}
