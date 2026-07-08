//! Generic browser fetch + Cache-Storage primitives (`target_arch = "wasm32"`).
//!
//! Extracted from lunco-modelica's MSL fetcher so **every** bundle distributor
//! shares one implementation of "download a content-hashed blob over HTTP, cache
//! it in the browser's Cache Storage, and unpack it". Two consumers today:
//!
//! - **MSL** (`lunco-modelica`) — the Modelica Standard Library bundle.
//! - **Twin bundles** (`lunco-networking::http_bundle`) — the "drop a `.tar.zst`
//!   into the server's `twins/` folder and the browser picks it up" path, the
//!   static sibling of the live `scenario_sync` transport.
//!
//! Everything here is **content-agnostic**: the caller passes the Cache-Storage
//! *bucket name* (e.g. `"lunco-msl-v1"`, `"lunco-twins-v1"`) so each distributor
//! keeps its own namespace, and the keep-set for pruning. No MSL/twin schema
//! leaks in.
//!
//! ## Caching strategy (why two fetch entry points)
//! - Content-hashed blobs (`*-<sha>.tar.zst`) are **immutable** → cache-first
//!   forever via [`fetch_cached_with_progress`]. A new build changes the hash →
//!   the filename → a cache miss, so updates are picked up without busting.
//! - The **mutable** manifest (`manifest.json`) uses **stale-while-revalidate**
//!   ([`fetch_bytes_revalidated`]): serve the cached copy instantly, refresh in
//!   the background for next boot.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Response};

#[wasm_bindgen(inline_js = "
    export async function lunco_fetch_bytes_cached_with_progress(cacheName, path, expectedTotal, on_progress) {
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
            response = await fetch(path);
            if (!response.ok) {
                throw new Error('fetch ' + path + ': HTTP ' + response.status + ' ' + response.statusText);
            }
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
    ) -> Result<JsValue, JsValue>;
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
) -> Result<Vec<u8>, String> {
    let js =
        lunco_fetch_bytes_cached_with_progress(bucket, path, expected_total as f64, on_progress)
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
    let caches = window.caches().map_err(|e| format!("window.caches: {e:?}"))?;
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
pub async fn network_fetch_uncached(path: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let opts = RequestInit::new();
    opts.set_method("GET");
    opts.set_mode(RequestMode::SameOrigin);
    let request = Request::new_with_str_and_init(path, &opts)
        .map_err(|e| format!("Request::new {path}: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch {path}: {e:?}"))?;
    let response: Response = resp_value
        .dyn_into()
        .map_err(|_| "fetch result not a Response".to_string())?;
    if !response.ok() {
        return Err(format!(
            "fetch {path}: HTTP {} {}",
            response.status(),
            response.status_text()
        ));
    }
    let array_buffer = JsFuture::from(
        response
            .array_buffer()
            .map_err(|e| format!("array_buffer {path}: {e:?}"))?,
    )
    .await
    .map_err(|e| format!("array_buffer await {path}: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&array_buffer).to_vec())
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
    cache: &web_sys::Cache,
    path: &str,
) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let opts = RequestInit::new();
    opts.set_method("GET");
    opts.set_mode(RequestMode::SameOrigin);
    let request = Request::new_with_str_and_init(path, &opts)
        .map_err(|e| format!("Request::new {path}: {e:?}"))?;

    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch {path}: {e:?}"))?;
    let response: Response = resp_value
        .dyn_into()
        .map_err(|_| "fetch result not a Response".to_string())?;
    if !response.ok() {
        return Err(format!(
            "fetch {path}: HTTP {} {}",
            response.status(),
            response.status_text()
        ));
    }

    // Clone the response to cache it while we read the body.
    let response_to_cache = response
        .clone()
        .map_err(|e| format!("response.clone: {e:?}"))?;
    let _ = JsFuture::from(cache.put_with_str(path, &response_to_cache)).await;

    let array_buffer = JsFuture::from(
        response
            .array_buffer()
            .map_err(|e| format!("array_buffer {path}: {e:?}"))?,
    )
    .await
    .map_err(|e| format!("array_buffer await {path}: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&array_buffer).to_vec())
}

/// **Cache-first-forever** fetch of a same-origin asset: return the cached copy
/// if present, else fetch once over the network and cache it. No revalidation —
/// for content that is static per deploy (a Twin's DEM heightmap/metadata, big
/// and immutable), so once cached it never re-downloads. `path` is fetched
/// verbatim (same-origin), so the caller passes the full origin-relative URL
/// (e.g. `assets/twins/moonbase/terrain/…/heightmap.tif`).
pub async fn fetch_bytes_cached(bucket: &str, path: &str) -> Result<Vec<u8>, String> {
    // Insecure contexts (LAN IP / `file://`) have no Cache Storage — degrade to a
    // plain (uncached) fetch so the asset still loads instead of throwing.
    let cache = match open_cache(bucket).await {
        Ok(c) => c,
        Err(_) => return network_fetch_uncached(path).await,
    };
    if let Ok(Some(bytes)) = cache_lookup(&cache, path).await {
        return Ok(bytes);
    }
    network_fetch_and_cache(&cache, path).await
}

/// **Stale-while-revalidate** fetch for the one *mutable* artifact per bucket
/// (`manifest.json`). A cached copy is returned **immediately** and refreshed in
/// the background so the *next* load sees any new release; the content-hashed
/// blobs it names are themselves cache-first-forever, so serving last session's
/// manifest just serves last session's (already-cached) blobs. Cold (no cached
/// copy): fall back to the network, then to cache on a race.
pub async fn fetch_bytes_revalidated(bucket: &str, path: &str) -> Result<Vec<u8>, String> {
    // No Cache Storage in insecure contexts — just fetch fresh each time.
    let cache = match open_cache(bucket).await {
        Ok(c) => c,
        Err(_) => return network_fetch_uncached(path).await,
    };
    if let Ok(Some(bytes)) = cache_lookup(&cache, path).await {
        // Serve stale now; refresh for next time off the critical path.
        let bucket = bucket.to_string();
        let p = path.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(c) = open_cache(&bucket).await {
                if let Err(e) = network_fetch_and_cache(&c, &p).await {
                    bevy::log::debug!("[web_fetch] {p}: background revalidate failed: {e}");
                }
            }
        });
        return Ok(bytes);
    }
    // Cold cache — must hit the network. Fall back to a cached copy only if a
    // concurrent fetch landed one in the meantime.
    match network_fetch_and_cache(&cache, path).await {
        Ok(bytes) => Ok(bytes),
        Err(net_err) => match cache_lookup(&cache, path).await {
            Ok(Some(bytes)) => {
                bevy::log::warn!("[web_fetch] {path}: network fetch failed ({net_err}); using cached copy");
                Ok(bytes)
            }
            _ => Err(net_err),
        },
    }
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
    let decoder = ruzstd::StreamingDecoder::new(bundle)
        .map_err(|e| format!("zstd decoder: {e}"))?;
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
