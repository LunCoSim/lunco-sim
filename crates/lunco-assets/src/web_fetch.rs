//! Generic browser fetch + Cache-Storage primitives (`target_arch = "wasm32"`).
//!
//! Extracted from lunco-modelica's MSL fetcher so **every** bundle distributor
//! shares one implementation of "download a content-hashed blob over HTTP, cache
//! it in the browser's Cache Storage, and unpack it". Consumers today:
//!
//! - **MSL** (`lunco-modelica`) — the Modelica Standard Library bundle.
//! - **Twin terrain assets** (`lunco-terrain-surface`) — the server serves its
//!   `twins/` directory over HTTP (staged under `assets/twins/…` next to the
//!   wasm) and the browser client fetches the DEM heightmap/metadata from it —
//!   the static sibling of the live `scenario_sync` transport.
//! - **Bundled fonts** (`lunco-assets::font`) — the page-served DejaVu fallback
//!   uses the same retry and body-resume path as every other browser asset.
//!
//! Everything here is **content-agnostic**: the caller passes the Cache-Storage
//! *bucket name* (e.g. `"lunco-msl-v1"`, `"lunco-twin-v1"`) so each distributor
//! keeps its own namespace, and the keep-set for pruning. No MSL/twin schema
//! leaks in.
//!
//! ## Caching strategy (why three fetch entry points)
//! - Content-hashed blobs (`*-<sha>.tar.zst`) are **immutable** → cache-first
//!   forever via [`fetch_cached_with_progress`]. A new build changes the hash →
//!   the filename → a cache miss, so updates are picked up without busting.
//! - The **mutable** manifest (`manifest.json`) uses **stale-while-revalidate**
//!   ([`fetch_bytes_revalidated`]): serve the cached copy instantly, refresh in
//!   the background for next boot. Fine for a small file; it re-downloads the
//!   whole body every load.
//! - **Mutable large assets** (a Twin's DEM `heightmap.tif` — same URL, but a
//!   host-side twin update can replace the content in place) use **conditional
//!   revalidation** ([`fetch_bytes_cached_conditional`] /
//!   [`fetch_cached_with_progress_conditional`]): serve the cached copy
//!   instantly, then issue a background `If-None-Match`/`If-Modified-Since`
//!   request — a 304 costs headers only; a 200 refreshes the cache so the
//!   *next* load sees the new content.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, Response};

#[wasm_bindgen(inline_js = "
    function retryableStatus(status) {
        return status === 408 || status === 425 || status === 429 || (status >= 500 && status <= 599);
    }

    function retryDelayMs(initialDelayMs, multiplier, maxDelayMs, failedAttempt) {
        return Math.min(maxDelayMs, initialDelayMs * Math.pow(multiplier, Math.max(0, failedAttempt - 1)));
    }

    function sleep(ms) {
        return new Promise(resolve => setTimeout(resolve, ms));
    }

    async function fetchWithRetry(path, headerName, headerValue, allowNotModified, maxAttempts, initialDelayMs, multiplier, maxDelayMs) {
        const attempts = Math.max(1, maxAttempts);
        let lastError = null;
        for (let attempt = 1; attempt <= attempts; attempt++) {
            try {
                const init = { method: 'GET', mode: 'same-origin' };
                if (headerName && headerValue) {
                    init.headers = { [headerName]: headerValue };
                }
                const response = await fetch(path, init);
                if (response.ok || (allowNotModified && response.status === 304)) {
                    return response;
                }
                const error = new Error('fetch ' + path + ': HTTP ' + response.status + ' ' + response.statusText);
                if (!retryableStatus(response.status)) {
                    error.nonRetryable = true;
                }
                throw error;
            } catch (error) {
                if (error && error.nonRetryable) {
                    throw error;
                }
                lastError = error;
                if (attempt < attempts) {
                    await sleep(retryDelayMs(initialDelayMs, multiplier, maxDelayMs, attempt));
                }
            }
        }
        throw lastError || new Error('fetch ' + path + ': retry budget exhausted');
    }

    async function fetchBytesWithProgressAndResume(cache, path, expectedTotal, on_progress, maxAttempts, initialDelayMs, multiplier, maxDelayMs) {
        const attempts = Math.max(1, maxAttempts);
        let chunks = [];
        let receivedLength = 0;
        let total = expectedTotal || 0;
        let responseForCache = null;
        for (let attempt = 1; attempt <= attempts; attempt++) {
            try {
                const init = { method: 'GET', mode: 'same-origin' };
                if (receivedLength > 0) {
                    init.headers = { Range: 'bytes=' + receivedLength + '-' };
                }
                const response = await fetch(path, init);
                if (!response.ok) {
                    const error = new Error('fetch ' + path + ': HTTP ' + response.status + ' ' + response.statusText);
                    if (!retryableStatus(response.status)) {
                        error.nonRetryable = true;
                    }
                    throw error;
                }

                const status = response.status;
                if (receivedLength > 0 && status === 206) {
                    const contentRange = response.headers.get('content-range') || '';
                    const match = /^bytes (\\d+)-\\d+\\/(\\d+|\\*)$/.exec(contentRange);
                    if (!match || Number(match[1]) !== receivedLength) {
                        const error = new Error('fetch ' + path + ': invalid resume Content-Range ' + contentRange);
                        error.nonRetryable = true;
                        throw error;
                    }
                    if (match[2] !== '*') {
                        total = Number(match[2]) || total;
                    }
                } else if (receivedLength > 0 && status === 200) {
                    // The origin ignored Range. It is safe to restart only because
                    // this response is explicitly a complete representation.
                    chunks = [];
                    receivedLength = 0;
                    total = Number(response.headers.get('content-length')) || expectedTotal || 0;
                } else if (receivedLength === 0) {
                    total = Number(response.headers.get('content-length')) || expectedTotal || 0;
                }
                responseForCache = response;
                if (on_progress && receivedLength === 0) {
                    try { on_progress(0, total); } catch (e) { console.warn('on_progress error:', e); }
                }

                if (!response.body) {
                    const arrayBuffer = await response.array_buffer();
                    const bytes = new Uint8Array(arrayBuffer);
                    chunks.push(bytes);
                    receivedLength += bytes.byteLength;
                } else {
                    const reader = response.body.getReader();
                    while (true) {
                        const {done, value} = await reader.read();
                        if (done) break;
                        chunks.push(value);
                        receivedLength += value.length;
                        if (on_progress) {
                            try { on_progress(receivedLength, total || receivedLength); } catch (e) { console.warn('on_progress error:', e); }
                        }
                    }
                }
                if (total && receivedLength < total) {
                    throw new Error('fetch ' + path + ': received ' + receivedLength + ' of ' + total + ' bytes');
                }
                const allChunks = new Uint8Array(receivedLength);
                let position = 0;
                for (const chunk of chunks) {
                    allChunks.set(chunk, position);
                    position += chunk.length;
                }
                if (on_progress) {
                    try { on_progress(receivedLength, total || receivedLength); } catch (e) {}
                }
                if (cache) {
                    try {
                        const headers = new Headers(responseForCache.headers);
                        headers.delete('content-range');
                        headers.set('content-length', String(allChunks.byteLength));
                        await cache.put(path, new Response(allChunks, {
                            status: 200,
                            headers: headers
                        }));
                    } catch (e) {
                        console.warn('Failed to write to cache:', e);
                    }
                }
                return allChunks;
            } catch (error) {
                if (error && error.nonRetryable) throw error;
                if (attempt < attempts) {
                    await sleep(retryDelayMs(initialDelayMs, multiplier, maxDelayMs, attempt));
                } else {
                    throw error;
                }
            }
        }
        throw new Error('fetch ' + path + ': retry budget exhausted');
    }

    export async function lunco_fetch_response_with_retries(path, headerName, headerValue, allowNotModified, maxAttempts, initialDelayMs, multiplier, maxDelayMs) {
        return await fetchWithRetry(path, headerName, headerValue, allowNotModified, maxAttempts, initialDelayMs, multiplier, maxDelayMs);
    }

    export async function lunco_fetch_bytes_with_resume(path, maxAttempts, initialDelayMs, multiplier, maxDelayMs) {
        return await fetchBytesWithProgressAndResume(null, path, 0, null, maxAttempts, initialDelayMs, multiplier, maxDelayMs);
    }

    export async function lunco_fetch_bytes_cached_with_resume(cacheName, path, maxAttempts, initialDelayMs, multiplier, maxDelayMs) {
        let cache = null;
        try {
            if (typeof caches !== 'undefined' && caches) {
                cache = await caches.open(cacheName);
            }
        } catch (e) {
            console.warn('Cache Storage open failed, continuing without cache:', e);
        }
        return await fetchBytesWithProgressAndResume(cache, path, 0, null, maxAttempts, initialDelayMs, multiplier, maxDelayMs);
    }

    export async function lunco_fetch_bytes_cached_with_progress(cacheName, path, expectedTotal, on_progress, maxAttempts, initialDelayMs, multiplier, maxDelayMs) {
        let cache = null;
        try {
            if (typeof caches !== 'undefined' && caches) {
                cache = await caches.open(cacheName);
            }
        } catch (e) {
            console.warn('Cache Storage open failed, degrading to network fetch:', e);
        }

        let matchResponse = null;
        if (cache) {
            try {
                matchResponse = await cache.match(path);
            } catch (e) {
                console.warn('Cache lookup failed:', e);
            }
        }

        let response;
        let fromCache = false;
        if (matchResponse) {
            response = matchResponse;
            fromCache = true;
        } else {
            return await fetchBytesWithProgressAndResume(cache, path, expectedTotal, on_progress, maxAttempts, initialDelayMs, multiplier, maxDelayMs);
        }

        // Prefer the advertised Content-Length; fall back to the caller's known
        // size (from a manifest) so the bar always has a denominator — a blob
        // served from Cache Storage often reports no Content-Length.
        const contentLength = response.headers.get('content-length');
        const total = (contentLength ? parseInt(contentLength, 10) : 0) || expectedTotal || 0;

        if (!response.body) {
            const arrayBuffer = await response.array_buffer();
            if (on_progress) {
                try { on_progress(arrayBuffer.byteLength, arrayBuffer.byteLength); } catch (e) {}
            }
            const allChunks = new Uint8Array(arrayBuffer);
            if (!fromCache && cache) {
                try {
                    const cachedResponse = new Response(allChunks, {
                        status: response.status,
                        statusText: response.statusText,
                        headers: response.headers
                    });
                    await cache.put(path, cachedResponse);
                } catch (e) {
                    console.warn('Failed to write to cache:', e);
                }
            }
            return allChunks;
        }

        const reader = response.body.getReader();
        const chunks = [];
        let receivedLength = 0;

        // Emit an initial 0-tick so the bar shows this blob's phase/denominator
        // immediately, even if the body then arrives in a single chunk.
        if (on_progress) {
            try { on_progress(0, total); } catch (e) { console.warn('on_progress error:', e); }
        }

        while (true) {
            const {done, value} = await reader.read();
            if (done) {
                break;
            }
            chunks.push(value);
            receivedLength += value.length;
            if (on_progress) {
                try {
                    on_progress(receivedLength, total || receivedLength);
                } catch (e) {
                    console.warn('on_progress error:', e);
                }
            }
        }

        const allChunks = new Uint8Array(receivedLength);
        let position = 0;
        for (let chunk of chunks) {
            allChunks.set(chunk, position);
            position += chunk.length;
        }

        if (!fromCache && cache) {
            try {
                const cachedResponse = new Response(allChunks, {
                    status: response.status,
                    statusText: response.statusText,
                    headers: response.headers
                });
                await cache.put(path, cachedResponse);
            } catch (e) {
                console.warn('Failed to write to cache:', e);
            }
        }

        return allChunks;
    }
")]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn lunco_fetch_bytes_cached_with_progress(
        cache_name: &str,
        path: &str,
        expected_total: f64,
        on_progress: &js_sys::Function,
        max_attempts: f64,
        initial_delay_ms: f64,
        multiplier: f64,
        max_delay_ms: f64,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn lunco_fetch_response_with_retries(
        path: &str,
        header_name: Option<&str>,
        header_value: Option<&str>,
        allow_not_modified: bool,
        max_attempts: f64,
        initial_delay_ms: f64,
        multiplier: f64,
        max_delay_ms: f64,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn lunco_fetch_bytes_with_resume(
        path: &str,
        max_attempts: f64,
        initial_delay_ms: f64,
        multiplier: f64,
        max_delay_ms: f64,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn lunco_fetch_bytes_cached_with_resume(
        cache_name: &str,
        path: &str,
        max_attempts: f64,
        initial_delay_ms: f64,
        multiplier: f64,
        max_delay_ms: f64,
    ) -> Result<JsValue, JsValue>;
}

fn retry_arguments(settings: &lunco_settings::DownloadSettings) -> (f64, f64, f64, f64) {
    (
        settings.max_attempts as f64,
        settings.retry_initial_delay_secs as f64 * 1000.0,
        settings.retry_backoff_multiplier as f64,
        settings.retry_max_delay_secs as f64 * 1000.0,
    )
}

async fn fetch_response_with_retries(
    path: &str,
    conditional_header: Option<(&str, &str)>,
    allow_not_modified: bool,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Response, String> {
    let (max_attempts, initial_delay_ms, multiplier, max_delay_ms) = retry_arguments(settings);
    let (header_name, header_value) = conditional_header
        .map(|(name, value)| (Some(name), Some(value)))
        .unwrap_or((None, None));
    lunco_fetch_response_with_retries(
        path,
        header_name,
        header_value,
        allow_not_modified,
        max_attempts,
        initial_delay_ms,
        multiplier,
        max_delay_ms,
    )
    .await
    .map_err(|e| format!("fetch {path}: {e:?}"))?
    .dyn_into()
    .map_err(|_| "fetch result not a Response".to_string())
}

async fn fetch_bytes_with_resume(
    bucket: Option<&str>,
    path: &str,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, String> {
    let (max_attempts, initial_delay_ms, multiplier, max_delay_ms) = retry_arguments(settings);
    let js = match bucket {
        Some(bucket) => {
            lunco_fetch_bytes_cached_with_resume(
                bucket,
                path,
                max_attempts,
                initial_delay_ms,
                multiplier,
                max_delay_ms,
            )
            .await
        }
        None => {
            lunco_fetch_bytes_with_resume(
                path,
                max_attempts,
                initial_delay_ms,
                multiplier,
                max_delay_ms,
            )
            .await
        }
    }
    .map_err(|e| format!("fetch {path}: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&js).to_vec())
}

/// Cache-first streamed fetch with per-chunk progress. Returns the full body.
///
/// If `path` is already in the `bucket` Cache-Storage it is served locally (no
/// network); otherwise it is fetched and written into the cache.
///
/// `expected_total` is the caller's known byte size for `path` (e.g. from a
/// manifest), used as the progress denominator when the response carries no
/// `content-length` — pass `0` if unknown. `on_progress` is a JS callback
/// `(received_bytes, total_bytes)`; it fires once with `(0, total)` up front so
/// a single-chunk body still shows this blob's bar, and `total_bytes` is never
/// `0` once any bytes arrive. Callers that want a Rust closure should build a
/// [`wasm_bindgen::closure::Closure`] and pass `cb.as_ref().unchecked_ref()`.
pub async fn fetch_cached_with_progress(
    bucket: &str,
    path: &str,
    expected_total: u64,
    on_progress: &js_sys::Function,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, String> {
    let (max_attempts, initial_delay_ms, multiplier, max_delay_ms) = retry_arguments(settings);
    let js = lunco_fetch_bytes_cached_with_progress(
        bucket,
        path,
        expected_total as f64,
        on_progress,
        max_attempts,
        initial_delay_ms,
        multiplier,
        max_delay_ms,
    )
    .await
    .map_err(|e| format!("fetch {path}: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&js).to_vec())
}

/// Open the named bucket in the browser's Cache Storage.
///
/// The Cache Storage API only exists in **secure contexts** (HTTPS, or
/// `http://localhost`). Served over plain HTTP from a LAN IP or `file://`,
/// `window.caches` is `undefined` — and `web_sys`'s getter casts that undefined
/// to a `CacheStorage` without validating, so a later `.open()` would throw
/// "Cannot read properties of undefined". We detect that here and return an
/// `Err` so callers can degrade to an uncached network fetch.
pub async fn open_cache(bucket: &str) -> Result<web_sys::Cache, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let caches = window
        .caches()
        .map_err(|e| format!("window.caches: {e:?}"))?;
    // Guard the insecure-context case where `caches` is really `undefined`.
    if caches.is_undefined() || caches.is_null() {
        return Err("Cache Storage unavailable (insecure context)".to_string());
    }
    JsFuture::from(caches.open(bucket))
        .await
        .map_err(|e| format!("caches.open: {e:?}"))?
        .dyn_into()
        .map_err(|_| "caches.open result not a Cache".to_string())
}

/// Fetch `path` over the network **without** touching Cache Storage. The
/// uncached fallback for insecure contexts (LAN IP / `file://`) where
/// [`open_cache`] fails because `window.caches` is undefined.
pub async fn network_fetch_uncached(
    path: &str,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, String> {
    fetch_bytes_with_resume(None, path, settings).await
}

/// Cheap existence check — is `path` in `bucket`? Does **not** read the (up to
/// tens of MB) body, so it's safe to call just to pick a progress label
/// (download vs. cache).
pub async fn cache_has(bucket: &str, path: &str) -> bool {
    let Ok(cache) = open_cache(bucket).await else {
        return false;
    };
    match JsFuture::from(cache.match_with_str(path)).await {
        Ok(v) => !v.is_null() && !v.is_undefined(),
        Err(_) => false,
    }
}

/// Read `path` from an already-open `cache`, returning `None` on a miss.
pub async fn cache_lookup(cache: &web_sys::Cache, path: &str) -> Result<Option<Vec<u8>>, String> {
    let match_value = JsFuture::from(cache.match_with_str(path))
        .await
        .map_err(|e| format!("cache.match {path}: {e:?}"))?;
    if match_value.is_null() || match_value.is_undefined() {
        return Ok(None);
    }
    let response: Response = match_value
        .dyn_into()
        .map_err(|_| "cache match not a Response".to_string())?;
    let array_buffer = JsFuture::from(
        response
            .array_buffer()
            .map_err(|e| format!("array_buffer cached {path}: {e:?}"))?,
    )
    .await
    .map_err(|e| format!("array_buffer await cached {path}: {e:?}"))?;
    Ok(Some(js_sys::Uint8Array::new(&array_buffer).to_vec()))
}

/// Fetch `path` from the network and write the response into `cache`. Whole-body
/// (no progress) — use [`fetch_cached_with_progress`] when you need a bar.
pub async fn network_fetch_and_cache(
    bucket: &str,
    path: &str,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, String> {
    fetch_bytes_with_resume(Some(bucket), path, settings).await
}

/// **Cache-first-forever** fetch of a same-origin asset: return the cached copy
/// if present, else fetch once over the network and cache it. No revalidation —
/// for content that never changes under its URL, so once cached it never
/// re-downloads. Mutable-URL assets (the Twin's DEM files) use
/// [`fetch_bytes_cached_conditional`] instead. `path` is fetched verbatim
/// (same-origin), so the caller passes the full origin-relative URL
/// (e.g. `assets/twins/moonbase/terrain/…/heightmap.tif`).
pub async fn fetch_bytes_cached(
    bucket: &str,
    path: &str,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, String> {
    // Insecure contexts (LAN IP / `file://`) have no Cache Storage — degrade to a
    // plain (uncached) fetch so the asset still loads instead of throwing.
    let cache = match open_cache(bucket).await {
        Ok(c) => c,
        Err(_) => return network_fetch_uncached(path, settings).await,
    };
    if let Ok(Some(bytes)) = cache_lookup(&cache, path).await {
        return Ok(bytes);
    }
    network_fetch_and_cache(bucket, path, settings).await
}

/// **Stale-while-revalidate** fetch for the one *mutable* artifact per bucket
/// (`manifest.json`). A cached copy is returned **immediately** and refreshed in
/// the background so the *next* load sees any new release; the content-hashed
/// blobs it names are themselves cache-first-forever, so serving last session's
/// manifest just serves last session's (already-cached) blobs. Cold (no cached
/// copy): fall back to the network, then to cache on a race.
pub async fn fetch_bytes_revalidated(
    bucket: &str,
    path: &str,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, String> {
    // No Cache Storage in insecure contexts — just fetch fresh each time.
    let cache = match open_cache(bucket).await {
        Ok(c) => c,
        Err(_) => return network_fetch_uncached(path, settings).await,
    };
    if let Ok(Some(bytes)) = cache_lookup(&cache, path).await {
        // Serve stale now; refresh for next time off the critical path.
        let bucket = bucket.to_string();
        let p = path.to_string();
        let settings = settings.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = network_fetch_and_cache(&bucket, &p, &settings).await {
                bevy::log::debug!("[web_fetch] {p}: background revalidate failed: {e}");
            }
        });
        return Ok(bytes);
    }
    // Cold cache — must hit the network. Fall back to a cached copy only if a
    // concurrent fetch landed one in the meantime.
    match network_fetch_and_cache(bucket, path, settings).await {
        Ok(bytes) => Ok(bytes),
        Err(net_err) => match cache_lookup(&cache, path).await {
            Ok(Some(bytes)) => {
                bevy::log::warn!(
                    "[web_fetch] {path}: network fetch failed ({net_err}); using cached copy"
                );
                Ok(bytes)
            }
            _ => Err(net_err),
        },
    }
}

/// **Conditional stale-while-revalidate** fetch for a *mutable* asset at a
/// stable URL (a Twin's DEM `metadata.yaml` / `heightmap.tif` — a host-side
/// twin update can replace the file in place). The cached copy is returned
/// **immediately**; a background *conditional* request
/// (`If-None-Match`/`If-Modified-Since`, see [`conditional_revalidate`]) then
/// checks the server — a 304 costs headers only, a 200 refreshes the cache so
/// the *next* load sees the new content. Unlike [`fetch_bytes_revalidated`]
/// this never re-downloads an unchanged multi-MB body. Cold cache (or insecure
/// context): same behavior as [`fetch_bytes_cached`].
pub async fn fetch_bytes_cached_conditional(
    bucket: &str,
    path: &str,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, String> {
    // No Cache Storage in insecure contexts — degrade to a plain (uncached)
    // fetch so the asset still loads instead of throwing.
    let cache = match open_cache(bucket).await {
        Ok(c) => c,
        Err(_) => return network_fetch_uncached(path, settings).await,
    };
    if let Ok(Some(bytes)) = cache_lookup(&cache, path).await {
        // Serve stale now; validate against the server off the critical path.
        spawn_conditional_revalidate(bucket, path, settings);
        return Ok(bytes);
    }
    network_fetch_and_cache(bucket, path, settings).await
}

/// [`fetch_cached_with_progress`] + the background *conditional* revalidation
/// of [`fetch_bytes_cached_conditional`] — for mutable **large** assets that
/// need a download bar (the DEM heightmap). The serve path is exactly
/// [`fetch_cached_with_progress`] (including its insecure-context degradation
/// to a plain network fetch); when the bytes came from the cache, a background
/// `If-None-Match`/`If-Modified-Since` probe refreshes the entry for the next
/// load. A fresh download needs no revalidation — it *is* the current content.
pub async fn fetch_cached_with_progress_conditional(
    bucket: &str,
    path: &str,
    expected_total: u64,
    on_progress: &js_sys::Function,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, String> {
    // Probe before the fetch: revalidation only applies to a pre-existing
    // cache entry. In insecure contexts this is `false` (no Cache Storage)
    // and the inline-JS fetch below degrades to a plain network fetch.
    let was_cached = cache_has(bucket, path).await;
    let bytes =
        fetch_cached_with_progress(bucket, path, expected_total, on_progress, settings).await?;
    if was_cached {
        spawn_conditional_revalidate(bucket, path, settings);
    }
    Ok(bytes)
}

/// Detach [`conditional_revalidate`] onto the browser event loop (best-effort;
/// failures are logged at debug — the cached copy already served the caller).
fn spawn_conditional_revalidate(
    bucket: &str,
    path: &str,
    settings: &lunco_settings::DownloadSettings,
) {
    let bucket = bucket.to_string();
    let path = path.to_string();
    let settings = settings.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = conditional_revalidate(&bucket, &path, &settings).await {
            bevy::log::debug!("[web_fetch] {path}: background conditional revalidate failed: {e}");
        }
    });
}

/// Ask the server whether the cached copy of `path` is still current, using
/// the validators the cached response carried (`ETag` → `If-None-Match`,
/// falling back to `Last-Modified` → `If-Modified-Since`). On 304 the cache is
/// untouched; on 200 the fresh response replaces the entry so the next load
/// picks it up — the running session keeps the bytes it already served (no
/// hot-swap).
async fn conditional_revalidate(
    bucket: &str,
    path: &str,
    settings: &lunco_settings::DownloadSettings,
) -> Result<(), String> {
    let cache = open_cache(bucket).await?;
    let match_value = JsFuture::from(cache.match_with_str(path))
        .await
        .map_err(|e| format!("cache.match {path}: {e:?}"))?;
    if match_value.is_null() || match_value.is_undefined() {
        // Entry evicted between serve and revalidate — nothing to validate.
        return Ok(());
    }
    let cached: Response = match_value
        .dyn_into()
        .map_err(|_| "cache match not a Response".to_string())?;

    let headers = cached.headers();
    let etag = headers.get("etag").ok().flatten();
    let last_modified = headers.get("last-modified").ok().flatten();
    // ETag is the stronger validator; Last-Modified is the fallback.
    let (cond_header, validator) = match (etag, last_modified) {
        (Some(v), _) => ("If-None-Match", v),
        (None, Some(v)) => ("If-Modified-Since", v),
        (None, None) => {
            // The cached response carries no validator, so a conditional
            // request is impossible — and an unconditional refetch would
            // re-download the full (tens-of-MB) body on every load. Keep the
            // cache-first behavior instead and accept staleness until the
            // user clears the cache.
            bevy::log::debug!(
                "[web_fetch] {path}: cached response has no ETag/Last-Modified; \
                 skipping revalidation (cache-first)"
            );
            return Ok(());
        }
    };

    let response =
        fetch_response_with_retries(path, Some((cond_header, &validator)), true, settings).await?;
    match response.status() {
        304 => {
            bevy::log::debug!("[web_fetch] {path}: 304 Not Modified — cached copy is current");
        }
        200 => {
            // `cache.put` consumes the response body; we never read it here —
            // the fresh bytes are for the *next* load, not this session.
            JsFuture::from(cache.put_with_str(path, &response))
                .await
                .map_err(|e| format!("cache.put {path}: {e:?}"))?;
            bevy::log::info!(
                "[web_fetch] {path}: changed on the server — cache updated; \
                 a reload will pick up the new version"
            );
        }
        s => {
            return Err(format!("HTTP {s} {}", response.status_text()));
        }
    }
    Ok(())
}

/// Evict every cached entry in `bucket` whose filename is not in `keep`. The
/// content-hashed blobs are immutable and cached-first-forever; when a new
/// release ships, the manifest points at fresh hashes and the old blobs would
/// otherwise linger indefinitely (unbounded growth across releases). Call after
/// a successful manifest load — once the *current* blobs are (re)cached — with
/// `keep` = the filenames the current manifest references (plus `manifest.json`).
/// Best-effort; returns the number evicted, logs and returns `0` on any error.
pub async fn prune_cache(bucket: &str, keep: &HashSet<String>) -> u32 {
    let cache = match open_cache(bucket).await {
        Ok(c) => c,
        Err(e) => {
            bevy::log::warn!("[web_fetch] cache prune skipped (open failed): {e}");
            return 0;
        }
    };

    let keys_val = match JsFuture::from(cache.keys()).await {
        Ok(v) => v,
        Err(e) => {
            bevy::log::warn!("[web_fetch] cache prune skipped (keys() failed): {e:?}");
            return 0;
        }
    };

    let mut removed = 0u32;
    for entry in js_sys::Array::from(&keys_val).iter() {
        let req: Request = match entry.dyn_into() {
            Ok(r) => r,
            Err(_) => continue,
        };
        let url = req.url();
        // Last path segment (sans any query) is the blob filename.
        let filename = url
            .rsplit('/')
            .next()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("");
        if filename.is_empty() || keep.contains(filename) {
            continue;
        }
        if JsFuture::from(cache.delete_with_str(&url)).await.is_ok() {
            removed += 1;
            bevy::log::info!("[web_fetch] cache prune: evicted superseded blob `{filename}`");
        }
    }
    if removed > 0 {
        bevy::log::info!("[web_fetch] cache prune: evicted {removed} superseded blob(s)");
    }
    removed
}

/// Unpack a `tar.zst` byte slice into `(rel_path → contents)`. Pure Rust
/// (`ruzstd` + `tar`), so it runs in the browser with no filesystem. `capacity_hint`
/// pre-sizes the map (pass the manifest's known file count, or `0`).
pub fn unpack_tar_zst(
    bundle: &[u8],
    capacity_hint: usize,
) -> Result<HashMap<PathBuf, Vec<u8>>, String> {
    let decoder =
        ruzstd::StreamingDecoder::new(bundle).map_err(|e| format!("zstd decoder: {e}"))?;
    let mut archive = tar::Archive::new(decoder);
    let mut out: HashMap<PathBuf, Vec<u8>> = HashMap::with_capacity(capacity_hint);
    for entry in archive.entries().map_err(|e| format!("tar entries: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("tar path: {e}"))?
            .into_owned();
        let mut buf = Vec::with_capacity(entry.header().size().unwrap_or(0) as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("tar read: {e}"))?;
        out.insert(path, buf);
    }
    Ok(out)
}
