//! MSL bundle loader.
//!
//! Inserts [`MslAssetSource`] and [`MslLoadState`] into the world.
//!
//! ## Native
//!
//! If [`lunco_assets::msl_source_root_path`] returns a path, we go straight
//! to `MslLoadState::Ready` with `MslAssetSource::Filesystem(...)`. No
//! fetch, no decompression.
//!
//! ## Web
//!
//! Spawns a `wasm_bindgen_futures::spawn_local` task that:
//!
//! 1. `fetch`es `msl/manifest.json` (same-origin).
//! 2. Parses it into [`lunco_assets::msl::MslManifest`].
//! 3. `fetch`es the bundle blob named in the manifest.
//! 4. Decompresses with `ruzstd`, untars into a `HashMap<PathBuf, Vec<u8>>`.
//! 5. Verifies the marker (`Modelica/package.mo`) is present.
//! 6. Inserts `MslAssetSource::InMemory(...)` and flips state to `Ready`.
//! 7. (Web only) Spawns the chunked parse driver that walks the
//!    in-memory source pairs over multiple frames — yielding to the
//!    browser between chunks so the page stays responsive — and
//!    installs the pre-parsed `Vec<(String, StoredDefinition)>` into a
//!    process-wide slot. `ModelicaCompiler::new` then short-circuits
//!    parsing via `Session::replace_parsed_source_set`.
//!
//! State transitions are mirrored to the bevy log so they show up in the
//! Console panel — that's our "status somewhere" until a dedicated status
//! bar lands.

use std::sync::{Arc, Mutex, OnceLock};

use bevy::prelude::*;

use lunco_assets::msl::{MslAssetSource, MslLoadPhase, MslLoadState};

/// Process-wide pre-parsed MSL documents. Populated on wasm by the
/// chunked parse driver once the full bundle has been turned into
/// `StoredDefinition`s. `ModelicaCompiler::new` reads it (via
/// [`global_parsed_msl`]) and installs into rumoca via
/// `Session::replace_parsed_source_set` — the entire parse cost is
/// already paid by then, so compile init is fast.
static GLOBAL_PARSED_MSL: OnceLock<Arc<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>> =
    OnceLock::new();

/// Serializes the native lazy decode of `parsed-msl.bin`. `GLOBAL_PARSED_MSL`
/// (a `OnceLock`) dedupes the stored *value* but not the *work*: two callers
/// that both miss `get()` will each run the full ~1.2 s zstd+bincode decode,
/// and the loser's `set()` is silently dropped. In the sandbox that race is
/// real — the worker's `ModelicaCompiler` session and the main-thread
/// `ModelicaEngine` session both reach for MSL on the first compile. This lock
/// makes the second caller block on the first decode and reuse it. Native-only;
/// wasm is single-threaded so no race exists there.
#[cfg(not(target_arch = "wasm32"))]
static MSL_DECODE_LOCK: Mutex<()> = Mutex::new(());

/// Read the pre-parsed MSL bundle if any has been installed.
pub fn global_parsed_msl() -> Option<&'static Arc<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>> {
    GLOBAL_PARSED_MSL.get()
}

/// Publish a freshly parsed MSL bundle to the process-wide slot. Only
/// the first install wins; subsequent calls are silently ignored
/// (the `OnceLock` guarantees a stable handle for the lifetime of
/// the page session).
fn install_global_parsed_msl(parsed: Vec<(String, rumoca_compile::parsing::StoredDefinition)>) {
    let _ = GLOBAL_PARSED_MSL.set(Arc::new(parsed));
}

/// The pre-parsed MSL bundle, loading it on demand if not yet present.
///
/// This is the **unified** accessor that drill-in / class-lookup paths
/// use on both targets:
/// - If [`global_parsed_msl`] is already populated (wasm chunked decode,
///   worker hand-off, or a prior native lazy-load), return it.
/// - On **native**, lazily deserialize `parsed-msl.bin` (the bundle the
///   `msl_indexer` writes) into the process-wide slot on first call —
///   one ~1–3 s bincode decode, then every subsequent lookup is an
///   in-memory hit. This replaces the old per-file `parse_files_parallel`
///   path that paid a full rumoca parse (tens of seconds for big
///   `package.mo` wrappers) on every drill-in.
/// - On **wasm** there is no synchronous disk path, so a miss just
///   returns `None` (the chunked decoder fills the slot a beat later).
pub fn parsed_msl_bundle(
) -> Option<&'static Arc<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>> {
    if let Some(bundle) = GLOBAL_PARSED_MSL.get() {
        return Some(bundle);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Hold the decode lock for the whole miss path, then re-check: a
        // peer may have filled the slot while we waited on the lock, in
        // which case we skip the redundant decode entirely.
        let _guard = MSL_DECODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(bundle) = GLOBAL_PARSED_MSL.get() {
            return Some(bundle);
        }
        let bundle_path = lunco_assets::msl_dir().join("parsed-msl.bin");
        // The bundle is zstd-compressed bincode (~10× smaller on disk than the
        // raw bincode it replaced). A stale/foreign bundle that fails to decode
        // returns `Err` below → the caller cold-parses and rewrites it. The
        // decode streams — it never holds the whole file as a `Vec<u8>`.
        match read_parsed_bundle_file(&bundle_path) {
            Ok(Some(docs)) => {
                info!(
                    "[MSL] lazy-loaded pre-parsed bundle ({} docs) from `{}` \
                     into process-wide slot",
                    docs.len(),
                    bundle_path.display()
                );
                install_global_parsed_msl(docs);
            }
            // No bundle on disk yet (indexer hasn't run) — caller parses source.
            Ok(None) => {}
            Err(e) => {
                // Stale/format-mismatched bundle (e.g. after a rumoca bump) —
                // caller falls back to a direct parse.
                warn!(
                    "[MSL] parsed bundle at `{}` failed to decode ({e}); \
                     drill-in will parse source directly",
                    bundle_path.display()
                );
            }
        }
    }
    GLOBAL_PARSED_MSL.get()
}

/// Kick the native `parsed-msl.bin` lazy decode onto a background thread so the
/// first palette drill-in / class lookup is an in-memory hit instead of paying
/// the ~1–3 s bincode decode inline. No-op if the slot is already populated or
/// the bundle isn't on disk (indexer hasn't run). Detached: nothing awaits it —
/// it just races to fill `GLOBAL_PARSED_MSL` before the user needs it.
#[cfg(not(target_arch = "wasm32"))]
pub fn warm_parsed_msl_in_background() {
    if GLOBAL_PARSED_MSL.get().is_some() {
        return;
    }
    bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async {
            let _ = parsed_msl_bundle();
        })
        .detach();
}

/// Startup system: warm the native parsed bundle unless the binary opted out of
/// MSL autoload (e.g. sandbox with [`SkipMslAutoLoad`]).
#[cfg(not(target_arch = "wasm32"))]
fn warm_parsed_msl_on_startup(skip: Option<Res<SkipMslAutoLoad>>) {
    if skip.is_some() {
        return;
    }
    warm_parsed_msl_in_background();
}

/// zstd level for the native `parsed-msl.bin` write. 9 is a good
/// ratio/speed balance for a one-time (cold-parse / indexer) write — the
/// disk win over raw bincode is ~10× either way; higher levels buy little.
#[cfg(not(target_arch = "wasm32"))]
const PARSED_BUNDLE_ZSTD_LEVEL: i32 = 9;

/// Read the native `parsed-msl.bin` fast-path bundle (zstd-compressed
/// bincode), streaming the decode so the whole file is never held as a
/// `Vec<u8>`.
///
/// `Ok(None)` = no/empty file (indexer hasn't run); `Err` = a present-but-
/// undecodable bundle (rumoca-version-stale, truncated, or a pre-zstd raw
/// bundle) so the caller cold-parses the source root and rewrites it.
#[cfg(not(target_arch = "wasm32"))]
fn read_parsed_bundle_file(
    path: &std::path::Path,
) -> Result<Option<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    if file.metadata().map(|m| m.len() == 0).unwrap_or(false) {
        return Ok(None); // empty / truncated
    }
    let mut decoder = zstd::stream::read::Decoder::new(std::io::BufReader::new(file))
        .map_err(|e| format!("zstd decoder: {e}"))?;
    bincode::serde::decode_from_std_read::<
        Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
        _,
        _,
    >(&mut decoder, bincode::config::standard())
    .map(Some)
    .map_err(|e| format!("bincode: {e}"))
}

/// Write `docs` to `path` as zstd-compressed bincode (the native
/// `parsed-msl.bin` fast-path bundle). Streams straight into the encoder, so
/// the ~165 MB of uncompressed bincode is never held in memory, and the file
/// lands ~10× smaller than the raw bincode it replaces. Shared by the
/// `msl_indexer` build step and `ModelicaCompiler`'s cold-parse fallback.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_parsed_bundle(
    path: &std::path::Path,
    docs: &[(String, rumoca_compile::parsing::StoredDefinition)],
) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder =
        zstd::stream::write::Encoder::new(std::io::BufWriter::new(file), PARSED_BUNDLE_ZSTD_LEVEL)?;
    bincode::serde::encode_into_std_write(docs, &mut encoder, bincode::config::standard())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    encoder.finish()?;
    Ok(())
}

/// Inflate a `parsed-*.bin.zst` blob to the raw bincode bytes, *without*
/// deserializing. The worker uses this so it can both decode its own ASTs
/// (`bincode::deserialize` the returned bytes) **and** ship the same decoded
/// bytes to the main thread (transferred `ArrayBuffer`) — letting the main
/// thread skip the ruzstd decompress and only deserialize. See
/// [`ingest_worker_decoded_msl`].
#[cfg(target_arch = "wasm32")]
pub fn decompress_parsed_bundle(compressed: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let mut decoder = ruzstd::StreamingDecoder::new(compressed)
        .map_err(|e| format!("zstd decoder: {e}"))?;
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| format!("zstd inflate: {e}"))?;
    Ok(out)
}

/// bincode-deserialize the *decompressed* bundle bytes (output of
/// [`decompress_parsed_bundle`]) into the `Vec<(uri, StoredDefinition)>`.
#[cfg(target_arch = "wasm32")]
pub fn deserialize_parsed_bundle(
    decoded: &[u8],
) -> Result<Vec<(String, rumoca_compile::parsing::StoredDefinition)>, String> {
    bincode::serde::decode_from_slice::<Vec<(String, rumoca_compile::parsing::StoredDefinition)>, _>(
        decoded,
        bincode::config::standard(),
    )
    .map(|(v, _)| v)
    .map_err(|e| format!("bincode deserialize: {e}"))
}

/// Untar + parse the compressed **source** bundle (`sources-*.tar.zst`) into the
/// parsed AST bundle. Each doc is keyed by its root-relative tar path
/// (`Modelica/…`) — identical to the build-time bundle keys
/// (`build_msl_assets::rel_key`) — so runtime class resolution matches. This is
/// the worker's tag-mismatch fallback (see
/// `worker_transport::WireMessage::ParseSourceMslCompressed`): a rumoca-version
/// skew makes the shipped pre-parsed bundle undeserializable, but the `.mo`
/// sources are still valid and reparse into a fresh, matching bundle.
#[cfg(target_arch = "wasm32")]
pub fn parse_source_bundle_to_docs(
    compressed: &[u8],
) -> Result<Vec<(String, rumoca_compile::parsing::StoredDefinition)>, String> {
    let files = lunco_assets::web_fetch::unpack_tar_zst(compressed, 2700)?;
    let mut out = Vec::with_capacity(files.len());
    let mut failed = 0usize;
    for (path, content) in files {
        let uri = path.to_string_lossy().replace('\\', "/");
        if !uri.ends_with(".mo") {
            continue;
        }
        let Ok(src) = std::str::from_utf8(&content) else {
            failed += 1;
            continue;
        };
        match rumoca_phase_parse::parse_to_ast(src, &uri) {
            Ok(def) => out.push((uri, def)),
            Err(_) => failed += 1,
        }
    }
    if out.is_empty() {
        return Err(format!("source bundle produced 0 parsed docs ({failed} failed)"));
    }
    if failed > 0 {
        bevy::log::warn!(
            "[MSL] source reparse: {failed} file(s) failed to parse (kept {})",
            out.len()
        );
    }
    Ok(out)
}

/// bincode-encode a parsed bundle in the same wire format
/// [`deserialize_parsed_bundle`] reads — used by the worker to transfer a
/// freshly-reparsed bundle back to the main thread.
#[cfg(target_arch = "wasm32")]
pub fn encode_parsed_bundle(
    docs: &[(String, rumoca_compile::parsing::StoredDefinition)],
) -> Result<Vec<u8>, String> {
    bincode::serde::encode_to_vec(docs, bincode::config::standard())
        .map_err(|e| format!("bincode serialize: {e}"))
}

// ─── Chunked main-thread MSL decode ────────────────────────────────
//
// On wasm the main-thread rumoca session needs the MSL ASTs in *its own*
// linear memory for reference resolution / autocomplete — the worker's copy
// lives in a separate memory and can't be shared. So the main thread must
// spend the CPU to materialise ~173 MB of `StoredDefinition`s. Doing it in one
// `bincode::deserialize_from` call froze the page for seconds; instead we
// time-slice it across frames (chunked decompress, then chunked deserialize)
// so the UI stays responsive while MSL becomes ready a second or two in.
//
// State lives in a `thread_local` (wasm is single-threaded) rather than a Bevy
// resource so it can hold a `Box<dyn Read>` (not `Send`) without `NonSend`
// plumbing. `drive_msl_main_decode` ticks it each `Update`.

#[cfg(target_arch = "wasm32")]
struct MainDecodeState {
    /// `Some` during the decompress phase; `None` once the full bincode byte
    /// stream has been inflated into `out`.
    decoder: Option<Box<dyn std::io::Read>>,
    /// Decompressed bincode bytes (the `Vec<(uri, StoredDefinition)>` blob).
    out: Vec<u8>,
    /// Cursor position into `out` for the deserialize phase.
    pos: u64,
    /// Elements left to deserialize.
    remaining: u64,
    /// Total element count (read from the bincode seq header).
    total: u64,
    header_read: bool,
    acc: Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static MAIN_DECODE: std::cell::RefCell<Option<MainDecodeState>> =
        const { std::cell::RefCell::new(None) };

    /// Compressed bundle retained as a *fallback* when the off-thread worker is
    /// expected to deliver the decoded bytes (see [`ingest_worker_decoded_msl`])
    /// but hasn't by the deadline — at which point the main thread decodes the
    /// compressed bundle itself (`start_main_msl_decode`). Only ~19 MB, so cheap
    /// to hold. `None` when no worker is involved (main decodes immediately).
    static MAIN_DECODE_FALLBACK: std::cell::RefCell<Option<(Vec<u8>, web_time::Instant)>> =
        const { std::cell::RefCell::new(None) };
}

/// How long the main thread waits for the worker to ship back the decoded MSL
/// bytes before decoding the compressed bundle itself. Generous: the worker
/// decode is normally a second or two, and a respawn re-seed re-delivers — this
/// only fires if the worker is wedged.
#[cfg(target_arch = "wasm32")]
const WORKER_DECODE_DEADLINE_SECS: u64 = 10;

/// Seed the chunked main-thread decoder, no-op if a decode is already underway
/// or the bundle is already installed (dedupes against the fallback path and
/// duplicate worker deliveries).
#[cfg(target_arch = "wasm32")]
fn seed_main_decode(state: MainDecodeState) -> bool {
    if global_parsed_msl().is_some() {
        return false;
    }
    MAIN_DECODE.with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.is_some() {
            return false;
        }
        *guard = Some(state);
        true
    })
}

/// Kick off the chunked main-thread decode of the **compressed** parsed bundle
/// (decompress → deserialize). Used when no off-thread worker is available, or
/// as the fallback when the worker fails to deliver decoded bytes in time.
#[cfg(target_arch = "wasm32")]
fn start_main_msl_decode(compressed: Vec<u8>) {
    match ruzstd::StreamingDecoder::new(std::io::Cursor::new(compressed)) {
        Ok(decoder) => {
            let seeded = seed_main_decode(MainDecodeState {
                decoder: Some(Box::new(decoder)),
                out: Vec::new(),
                pos: 0,
                remaining: 0,
                total: 0,
                header_read: false,
                acc: Vec::new(),
            });
            if seeded {
                info!("[MSL] started chunked main-thread decode (decompress + deserialize)");
            }
        }
        Err(e) => error!("[MSL] could not start main decode: {e}"),
    }
}

/// Stash the compressed bundle as a deadline fallback while the off-thread
/// worker decodes and ships back the decoded bytes. If the worker delivers
/// first ([`ingest_worker_decoded_msl`]) this is dropped unused; otherwise
/// `drive_msl_main_decode` picks it up once the deadline passes.
#[cfg(target_arch = "wasm32")]
fn stash_main_decode_fallback(compressed: Vec<u8>) {
    let deadline =
        web_time::Instant::now() + std::time::Duration::from_secs(WORKER_DECODE_DEADLINE_SECS);
    MAIN_DECODE_FALLBACK.with(|f| *f.borrow_mut() = Some((compressed, deadline)));
}

/// Install the **already-decompressed** bincode bytes the off-thread worker
/// shipped back (transferred `ArrayBuffer`). The main thread then only runs the
/// chunked bincode *deserialize* into its own heap — it never pays the ruzstd
/// decompress. No-op if a decode is already underway or finished.
#[cfg(target_arch = "wasm32")]
pub fn ingest_worker_decoded_msl(decoded: Vec<u8>) {
    // Clear the fallback regardless — the worker delivered, so the deadline
    // path must not also fire.
    MAIN_DECODE_FALLBACK.with(|f| *f.borrow_mut() = None);
    let seeded = seed_main_decode(MainDecodeState {
        decoder: None, // already decompressed → skip Phase 1
        out: decoded,
        pos: 0,
        remaining: 0,
        total: 0,
        header_read: false,
        acc: Vec::new(),
    });
    if seeded {
        info!("[MSL] received decoded MSL bytes from worker — deserialize only (no main decompress)");
    }
}

/// Per-frame driver for the chunked main-thread MSL decode. No-op once the
/// `MAIN_DECODE` slot is empty (the common case after boot). On completion it
/// installs `GLOBAL_PARSED_MSL` and flips `MslLoadState` to `Ready`, after
/// which `drive_msl_bootstrap` seeds the workspace engine session exactly as
/// before — so resolution/autocomplete are unaffected, just non-blocking.
#[cfg(target_arch = "wasm32")]
fn drive_msl_main_decode(mut state: ResMut<MslLoadState>) {
    // Tuned so each frame's slice stays a few ms. Decompress is cheap per byte;
    // deserialize allocates deep ASTs, so its chunk is in documents.
    const DECOMPRESS_CHUNK: usize = 8 * 1024 * 1024;
    const DESER_CHUNK: usize = 96;

    // ── Fallback: the worker was expected to ship back decoded bytes but
    // hasn't by the deadline → decode the compressed bundle here. Cheap check;
    // skipped once a decode is underway (`MAIN_DECODE` set) or MSL is installed.
    if global_parsed_msl().is_none() && MAIN_DECODE.with(|c| c.borrow().is_none()) {
        let overdue = MAIN_DECODE_FALLBACK.with(|f| {
            let mut g = f.borrow_mut();
            match g.as_ref() {
                Some((_, deadline)) if web_time::Instant::now() >= *deadline => {
                    g.take().map(|(bytes, _)| bytes)
                }
                _ => None,
            }
        });
        if let Some(bytes) = overdue {
            warn!("[MSL] worker decode overdue — decoding bundle on the main thread instead");
            start_main_msl_decode(bytes);
        }
    }

    MAIN_DECODE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(d) = guard.as_mut() else {
            return;
        };

        // ── Phase 1: inflate the zstd stream, bounded bytes per frame.
        if let Some(reader) = d.decoder.as_mut() {
            let mut buf = vec![0u8; 256 * 1024];
            let mut got = 0usize;
            while got < DECOMPRESS_CHUNK {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        d.decoder = None;
                        break;
                    }
                    Ok(n) => {
                        d.out.extend_from_slice(&buf[..n]);
                        got += n;
                    }
                    Err(e) => {
                        warn!("[MSL] main decode decompress error: {e}");
                        *guard = None;
                        return;
                    }
                }
            }
            *state = MslLoadState::Loading {
                phase: MslLoadPhase::Decompressing,
                bytes_done: d.out.len() as u64,
                bytes_total: 0,
            };
            return;
        }

        // ── Phase 2: bincode-deserialize, bounded docs per frame. The blob is a
        // bincode `standard()` encoding of `Vec<(uri, StoredDefinition)>`: a
        // variable-int element count followed by the elements back-to-back.
        // Decode the count once, then walk elements one at a time, advancing
        // `pos` by the bytes each consumes so the work splits across frames.
        let cfg = bincode::config::standard();
        if !d.header_read {
            match bincode::serde::decode_from_slice::<u64, _>(&d.out[d.pos as usize..], cfg) {
                Ok((count, n)) => {
                    d.total = count;
                    d.remaining = count;
                    d.pos += n as u64;
                    d.header_read = true;
                    d.acc.reserve(count as usize);
                }
                Err(e) => {
                    warn!("[MSL] main decode: bad bundle header: {e}");
                    *guard = None;
                    return;
                }
            }
        }

        for _ in 0..DESER_CHUNK {
            if d.remaining == 0 {
                break;
            }
            match bincode::serde::decode_from_slice::<
                (String, rumoca_compile::parsing::StoredDefinition),
                _,
            >(&d.out[d.pos as usize..], cfg)
            {
                Ok((item, n)) => {
                    d.acc.push(item);
                    d.pos += n as u64;
                    d.remaining -= 1;
                }
                Err(e) => {
                    warn!("[MSL] main decode deserialize error: {e}");
                    d.remaining = 0;
                    break;
                }
            }
        }

        if d.remaining == 0 {
            let docs = std::mem::take(&mut d.acc);
            let count = docs.len();
            let uncompressed = d.out.len() as u64;
            install_global_parsed_msl(docs);
            *guard = None; // frees `out`
            *state = MslLoadState::Ready {
                file_count: count,
                compressed_bytes: 0,
                uncompressed_bytes: uncompressed,
            };
            info!("[MSL] main-thread decode complete: {count} docs — resolution/autocomplete ready");
        } else {
            *state = MslLoadState::Loading {
                phase: MslLoadPhase::Parsing,
                bytes_done: d.total - d.remaining,
                bytes_total: d.total,
            };
        }
    });
}

// ─── Lazy source-bundle unpack ─────────────────────────────────────
//
// The 37 MB source tree is only needed when the user drills into an MSL file
// in the editor — so we keep it compressed and untar it on first demand
// (`ensure_msl_source_unpacked`) instead of on the boot future, where it was a
// second freeze. Image/icon loading is disabled on wasm, so nothing else needs
// it at boot.
#[cfg(target_arch = "wasm32")]
static MSL_SOURCE_COMPRESSED: OnceLock<(Vec<u8>, lunco_assets::msl::MslBundleEntry)> =
    OnceLock::new();

#[cfg(target_arch = "wasm32")]
fn stash_compressed_source(bytes: Vec<u8>, meta: lunco_assets::msl::MslBundleEntry) {
    let _ = MSL_SOURCE_COMPRESSED.set((bytes, meta));
}

/// Build the ordered library-root list to install from a primary
/// source. On native, also registers any third-party Modelica libraries
/// already unpacked in the cache (so palette / drill-in resolve them
/// too); on web the bundle already carries every shipped library in the
/// one in-memory root, so the primary stands alone.
fn sources_with_extras(primary: MslAssetSource) -> Vec<MslAssetSource> {
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut sources = vec![primary];
    #[cfg(not(target_arch = "wasm32"))]
    {
        if matches!(sources[0], MslAssetSource::Filesystem(_)) {
            for (subdir, _pkg) in
                crate::package_tree::scanner::discover_third_party_libs()
            {
                sources.push(MslAssetSource::Filesystem(
                    lunco_assets::cache_dir().join(subdir),
                ));
            }
        }
    }
    sources
}

/// Untar the MSL source bundle into the process-wide `MslAssetSource` on first
/// use (idempotent). Called by the drill-in paths (`Document::load_msl_class` /
/// `load_msl_file`) before they read MSL source text. No-op if already
/// unpacked or if no compressed source was stashed.
#[cfg(target_arch = "wasm32")]
pub fn ensure_msl_source_unpacked() {
    if lunco_assets::msl::has_msl_source() {
        return;
    }
    let Some((bytes, meta)) = MSL_SOURCE_COMPRESSED.get() else {
        return;
    };
    match lunco_assets::web_fetch::unpack_tar_zst(bytes, meta.file_count) {
        Ok(files) => {
            let n = files.len();
            lunco_assets::msl::install_global_msl_sources(sources_with_extras(
                MslAssetSource::InMemory(Arc::new(lunco_assets::msl::MslInMemory { files })),
            ));
            info!("[MSL] source bundle unpacked lazily ({n} files) for drill-in");
        }
        Err(e) => warn!("[MSL] lazy source unpack failed: {e}"),
    }
}

/// `pub` re-export of `install_global_parsed_msl` so the off-thread
/// worker bin (`bin/lunica_worker.rs`) can install the MSL bundle it
/// receives over postMessage.
#[cfg(target_arch = "wasm32")]
pub fn install_global_parsed_msl_pub(parsed: Vec<(String, rumoca_compile::parsing::StoredDefinition)>) {
    install_global_parsed_msl(parsed);
}


/// Marker resource. Insert **before** adding [`MslRemotePlugin`] (or
/// [`ModelicaCorePlugin`], which adds it transitively) to suppress the
/// auto-fetch of the MSL bundle on app start. The sandbox uses this on
/// wasm — sandbox cosim doesn't load any Modelica.Library classes, so
/// fetching `msl/manifest.json` produces a noisy 404 and a wasted
/// parse pipeline.
#[derive(Resource, Default, Clone, Copy)]
pub struct SkipMslAutoLoad;

/// Gate resource: while present, the **web** MSL bootstrap (network fetch AND the
/// main-thread decode/parse) is held off. Unlike [`SkipMslAutoLoad`] (which
/// suppresses MSL entirely), this only *defers* it — remove the resource and the
/// fetch kicks off, then the decode/parse chain runs.
///
/// The sandbox uses this on wasm to load **sequentially**: the single browser
/// thread bakes the moonbase terrain first, THEN the ~2 MB MSL bundle downloads
/// and its chunked decompress/deserialize runs — instead of the two contending
/// for the main thread and stalling terrain generation. Insert before the app
/// runs; remove once the terrain is baked (the sandbox watches its
/// `TerrainGenStatus`). No-op on native (real threads; never inserted there).
#[derive(Resource, Default, Clone, Copy)]
pub struct DeferMslLoad;

/// Run condition: the web MSL bootstrap may proceed (not gated by [`DeferMslLoad`]).
#[cfg(target_arch = "wasm32")]
fn msl_not_deferred(defer: Option<Res<DeferMslLoad>>) -> bool {
    defer.is_none()
}

/// Web: kick the async MSL fetcher exactly once, the first frame the deferral
/// gate is clear. Paired with [`msl_not_deferred`] as a `run_if`, so while
/// [`DeferMslLoad`] is present this never runs and no download starts.
#[cfg(target_arch = "wasm32")]
fn kick_web_msl_fetcher(slot: Res<MslLoadSlot>, mut kicked: Local<bool>) {
    if *kicked {
        return;
    }
    *kicked = true;
    wasm_bindgen_futures::spawn_local(web::run_fetcher(slot.0.clone()));
}

/// Plugin that owns MSL asset loading. Add once during app build.
pub struct MslRemotePlugin;

impl Plugin for MslRemotePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MslLoadState>();
        // Persisted user settings (bundle URL, local-root override,
        // last-fetched bookkeeping). Lives in settings.json so the
        // Assets panel and the auto-download path see the same source
        // of truth.
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<crate::msl_settings::MslSettings>();

        // (The MSL-state → status-bus mirror is a UI reactive observer; it
        // lives in `ui::core_observers` and is registered by the UI plugin.
        // Core just owns `MslLoadState`.)

        // Native: prefer an already-materialised tree (workspace dev
        // cache, user-supplied override, or a previously-completed
        // auto-download). If nothing is present, fall back to fetching
        // the configured bundle URL into the cache dir. The fetch runs
        // on `AsyncComputeTaskPool`; `drain_native_msl_fetch` promotes
        // its result back into ECS state.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let settings = app
                .world()
                .resource::<crate::msl_settings::MslSettings>()
                .clone();

            // 1. Settings-level override wins — user explicitly pointed
            //    us at a tree on disk (e.g. a system install, a local
            //    Modelica checkout).
            let override_root = settings.local_root_override.as_ref().and_then(|p| {
                if p.join("Modelica").exists() {
                    Some(p.clone())
                } else {
                    warn!(
                        "[MSL] settings.msl.local_root_override = {} has no Modelica/ subdir; ignoring",
                        p.display()
                    );
                    None
                }
            });

            let resolved_root = override_root.or_else(lunco_assets::msl_source_root_path);

            if let Some(root) = resolved_root {
                let count = count_mo_files(&root);
                info!("[MSL] using on-disk root {} ({count} .mo files)", root.display());
                lunco_assets::msl::install_global_msl_sources(sources_with_extras(
                    MslAssetSource::Filesystem(root),
                ));
                app.insert_resource(MslLoadState::Ready {
                    file_count: count,
                    compressed_bytes: 0,
                    uncompressed_bytes: 0,
                });
            } else {
                // No tree on disk → kick off the existing
                // downloader+indexer pipeline in the background. The
                // `[msl]` entry in Assets.toml has the URL, version,
                // and (once filled in) sha256. The indexer follows
                // automatically after extract completes.
                info!("[MSL] no on-disk root — starting background install");
                let slot: NativeInstallSlot =
                    Arc::new(Mutex::new(NativeInstallSlotInner::default()));
                let cancel = MslInstallCancel::default();
                app.insert_resource(MslLoadState::Loading {
                    phase: MslLoadPhase::FetchingBundle,
                    bytes_done: 0,
                    bytes_total: 0,
                });
                app.insert_resource(NativeMslInstallSlot(slot.clone()));
                app.insert_resource(cancel.clone());
                app.add_systems(Update, drain_native_msl_install);
                spawn_native_install(slot, cancel.0);
            }

            // Warm the pre-parsed bundle off-thread so the first drill-in /
            // class lookup is instant instead of paying the bincode decode
            // inline. No-op when the bundle isn't on disk yet (download path:
            // the indexer writes it later) or autoload is suppressed.
            app.add_systems(Startup, warm_parsed_msl_on_startup);
        }

        // Web: kick off the async fetcher and have a system promote the
        // shared `Mutex` slot into a Bevy resource once the task completes.
        // Apps that don't ship an MSL bundle (sandbox) can pre-insert
        // `SkipMslAutoLoad` to suppress the fetch entirely.
        #[cfg(target_arch = "wasm32")]
        {
            if app.world().contains_resource::<SkipMslAutoLoad>() {
                app.insert_resource(MslLoadState::NotStarted);
            } else {
                let slot: SharedSlot = Arc::new(Mutex::new(SlotInner::default()));
                app.insert_resource(MslLoadState::Loading {
                    phase: MslLoadPhase::FetchingManifest,
                    bytes_done: 0,
                    bytes_total: 0,
                });
                app.insert_resource(MslLoadSlot(slot.clone()));
                // The fetch AND the main-thread decode/parse are all gated on the
                // deferral: an app can insert `DeferMslLoad` to load MSL only after
                // higher-priority startup work (the sandbox: after the terrain
                // bakes). `kick_web_msl_fetcher` starts the download the first
                // ungated frame; the decode/parse chain drains it. With no
                // `DeferMslLoad` present (the default), the gate is open from frame
                // one — identical timing to the previous unconditional fetch.
                app.add_systems(
                    Update,
                    (
                        kick_web_msl_fetcher,
                        drain_msl_load_slot,
                        drive_msl_parse,
                        drive_msl_main_decode,
                    )
                        .chain()
                        .run_if(msl_not_deferred),
                );
            }
        }
    }
}

/// Per-frame parse-progress state on wasm. Created by
/// `drain_msl_load_slot` once the bundle has been decompressed; ticked
/// by `drive_msl_parse` until empty, then removed. While present, each
/// frame parses `PARSE_CHUNK_SIZE` `(uri, source)` pairs and emits a
/// `[MSL] parsing… N / total` log line every `PARSE_LOG_INTERVAL_SECS`.
#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
struct MslParseInProgress {
    pending: Vec<(String, String)>,
    parsed: Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
    total: usize,
    started: web_time::Instant,
    last_log: web_time::Instant,
}

#[cfg(target_arch = "wasm32")]
const PARSE_CHUNK_SIZE: usize = 1;

#[cfg(target_arch = "wasm32")]
const PARSE_LOG_INTERVAL_SECS: u64 = 10;

// ─── Native background install (downloader + indexer) ──────────────
//
// Reuses the existing infrastructure rather than reinventing it:
//
//   1. `lunco_assets::download::download_asset` handles HTTP fetch,
//      sha256 verify, gzip/bzip2 untar, and version-file caching.
//      The `[msl]` entry it reads lives in
//      `crates/lunco-modelica/Assets.toml` (compiled in via
//      `include_str!` so a packaged binary works without the source
//      tree on disk).
//
//   2. The `msl_indexer` binary builds the rumoca bincode cache
//      (`parsed-msl.bin`) used by `ModelicaCompiler::new` for the
//      fast preload path. Spawned as a subprocess from the same
//      background task so the user sees one continuous "MSL loading"
//      indicator instead of two separate stages.
//
// Both steps run on `AsyncComputeTaskPool` so the Bevy main thread
// stays responsive throughout. `drain_native_msl_install` promotes the
// shared `NativeInstallSlot` into ECS state each frame.

/// Bundled copy of `crates/lunco-modelica/Assets.toml`. Packaged
/// binaries don't have the source tree, so we compile the manifest in.
#[cfg(not(target_arch = "wasm32"))]
pub const BUNDLED_ASSETS_TOML: &str = include_str!("../Assets.toml");

/// Cooperative cancel flag for the in-flight MSL install task. The
/// download polls it between chunks; the indexer polls it between
/// phases. Settings → Assets exposes a "Cancel" button that flips it.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Clone, Default)]
pub struct MslInstallCancel(pub Arc<std::sync::atomic::AtomicBool>);

#[cfg(not(target_arch = "wasm32"))]
type NativeInstallSlot = Arc<Mutex<NativeInstallSlotInner>>;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct NativeInstallSlotInner {
    /// Latest load-state the worker has reported; drained each frame.
    pending_state: Option<MslLoadState>,
    /// Final `MslAssetSource::Filesystem(root)` produced on success.
    pending_source: Option<MslAssetSource>,
    /// MSL release tag that just landed on disk (mirrors `Assets.toml`
    /// `version`). Written into `MslSettings.last_fetched_version`.
    pending_version: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct NativeMslInstallSlot(NativeInstallSlot);

#[cfg(not(target_arch = "wasm32"))]
fn spawn_native_install(slot: NativeInstallSlot, cancel: Arc<std::sync::atomic::AtomicBool>) {
    use lunco_assets::download::AssetManifest;

    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    let cancel_for_task = cancel.clone();
    pool.spawn(async move {
        // Parse the bundled Assets.toml to recover the `[msl]` entry.
        let manifest = match BUNDLED_ASSETS_TOML.parse::<AssetManifest>() {
            Ok(m) => m,
            Err(e) => {
                set_install_state(
                    &slot,
                    MslLoadState::Failed(format!("Assets.toml parse: {e}")),
                );
                return;
            }
        };
        let Some(entry) = manifest.assets.get("msl") else {
            set_install_state(
                &slot,
                MslLoadState::Failed("no [msl] entry in Assets.toml".into()),
            );
            return;
        };
        let version = entry.version.clone();

        // ── Download + extract ────────────────────────────────────
        // `download_asset` is synchronous (ureq) and prints to stdout.
        // It already handles cache-hit (version file, sha256) so a
        // repeat launch is effectively a no-op.
        set_install_state(
            &slot,
            MslLoadState::Loading {
                phase: MslLoadPhase::FetchingBundle,
                bytes_done: 0,
                bytes_total: 0,
            },
        );
        // Per-chunk download progress and per-entry extract progress
        // both feed the shared slot so `drain_native_msl_install`
        // picks them up next frame.
        let progress_slot = slot.clone();
        let extract_slot = slot.clone();
        let control = lunco_assets::download::DownloadControl {
            progress: Some(Box::new(move |done, total| {
                if let Ok(mut inner) = progress_slot.lock() {
                    inner.pending_state = Some(MslLoadState::Loading {
                        phase: MslLoadPhase::FetchingBundle,
                        bytes_done: done,
                        bytes_total: total,
                    });
                }
            })),
            extracting: Some(Box::new(move |entries_done| {
                if let Ok(mut inner) = extract_slot.lock() {
                    inner.pending_state = Some(MslLoadState::Loading {
                        phase: MslLoadPhase::Decompressing,
                        bytes_done: entries_done,
                        bytes_total: 0,
                    });
                }
            })),
            cancel: Some(cancel_for_task.clone()),
        };
        if let Err(e) =
            // `None` = cache-relative: the MSL bundle lands under the asset cache
            // root, not a Twin. Matches the pre-`dest_root` behaviour.
            lunco_assets::download::download_asset_with_control(entry, "msl", control, None)
        {
            let msg = match e {
                lunco_assets::download::DownloadError::Cancelled => "cancelled".to_string(),
                other => format!("MSL download: {other}"),
            };
            set_install_state(&slot, MslLoadState::Failed(msg));
            return;
        }

        // ── Resolve resulting on-disk root ────────────────────────
        let Some(root) = lunco_assets::msl_source_root_path() else {
            set_install_state(
                &slot,
                MslLoadState::Failed(
                    "downloader succeeded but no Modelica/ tree was found in cache"
                        .into(),
                ),
            );
            return;
        };

        // MSL is usable for compilation as soon as the tree is on
        // disk, but the workbench's bincode cache (`parsed-msl.bin`)
        // is built by the indexer below. Publish the source now so
        // any compile dispatched in parallel resolves correctly, but
        // *don't* flip to `Ready` yet — the chip stays at
        // `Loading { phase: Parsing }` while the indexer runs so the
        // user sees one continuous indicator instead of "ready"
        // followed by 30 s of silence.
        let source = MslAssetSource::Filesystem(root.clone());
        if let Ok(mut inner) = slot.lock() {
            inner.pending_source = Some(source);
            inner.pending_state = Some(MslLoadState::Loading {
                phase: MslLoadPhase::Parsing,
                bytes_done: 0,
                bytes_total: 0,
            });
            inner.pending_version = version;
        }

        // ── Indexer (best-effort, in-process) ─────────────────────
        // Same workflow the `msl_indexer` binary uses. The runtime
        // parse path still works without the bincode cache, just
        // slower; any panic inside `run` is logged and swallowed.
        bevy::log::info!("[MSL] running indexer to warm parsed-msl.bin cache…");
        let cancel_for_indexer = cancel_for_task.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::indexer::run_with_cancel(
                crate::indexer::Options::default(),
                Some(cancel_for_indexer),
            );
        }));

        // Indexer done — flip the chip to Ready.
        let file_count = count_mo_files(&root);
        set_install_state(
            &slot,
            MslLoadState::Ready {
                file_count,
                compressed_bytes: 0,
                uncompressed_bytes: 0,
            },
        );
    })
    .detach();
}

/// Restart the MSL install task. Used by Reinstall / Retry buttons.
/// Cancels any in-flight task, wipes the cache, clears the published
/// source, reseeds the cancel flag and slot, and spawns a fresh task.
#[cfg(not(target_arch = "wasm32"))]
pub fn reinstall_msl(world: &mut World) {
    if let Some(old) = world.get_resource::<MslInstallCancel>() {
        old.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let dir = lunco_assets::cache_subdir("msl");
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        if e.kind() != std::io::ErrorKind::NotFound {
            bevy::log::warn!("[MSL] could not clear cache at {}: {e}", dir.display());
        }
    }
    world.insert_resource(MslLoadState::Loading {
        phase: MslLoadPhase::FetchingBundle,
        bytes_done: 0,
        bytes_total: 0,
    });
    let slot: NativeInstallSlot = Arc::new(Mutex::new(NativeInstallSlotInner::default()));
    let cancel = MslInstallCancel::default();
    world.insert_resource(NativeMslInstallSlot(slot.clone()));
    world.insert_resource(cancel.clone());
    spawn_native_install(slot, cancel.0);
    bevy::log::info!("[MSL] reinstall requested by user");
}

#[cfg(not(target_arch = "wasm32"))]
fn set_install_state(slot: &NativeInstallSlot, state: MslLoadState) {
    if let Ok(mut inner) = slot.lock() {
        inner.pending_state = Some(state);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn drain_native_msl_install(
    slot: Res<NativeMslInstallSlot>,
    mut state: ResMut<MslLoadState>,
    mut settings: ResMut<crate::msl_settings::MslSettings>,
) {
    let Ok(mut inner) = slot.0.lock() else { return };
    if let Some(new_state) = inner.pending_state.take() {
        match (&*state, &new_state) {
            (
                MslLoadState::Loading { phase: a, .. },
                MslLoadState::Loading { phase: b, .. },
            ) if a == b => {}
            _ => log_state_transition(&new_state),
        }
        *state = new_state;
    }
    if let Some(source) = inner.pending_source.take() {
        lunco_assets::msl::install_global_msl_sources(sources_with_extras(source));
    }
    if let Some(v) = inner.pending_version.take() {
        settings.last_fetched_version = Some(v);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn count_mo_files(root: &std::path::Path) -> usize {
    fn walk(p: &std::path::Path, n: &mut usize) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(&path, n);
            } else if path.extension().and_then(|s| s.to_str()) == Some("mo") {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(root, &mut n);
    n
}

// ─── Shared slot the wasm fetcher writes into ───────────────────────

#[cfg(target_arch = "wasm32")]
type SharedSlot = Arc<Mutex<SlotInner>>;

/// Frames `drain_msl_load_slot` waits for a worker to come up before untarring
/// the tag-mismatch source bundle on the main thread instead. ~3 s at 60 fps —
/// the worker normally installs within the first few boot frames.
#[cfg(target_arch = "wasm32")]
const MSL_SOURCE_PARSE_MAX_WAIT: u32 = 180;

#[cfg(target_arch = "wasm32")]
thread_local! {
    static MSL_SOURCE_PARSE_WAIT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SlotInner {
    /// Latest state the fetcher has reported. The drain system replaces
    /// the world's `MslLoadState` whenever this `take`s out a new value.
    pending_state: Option<MslLoadState>,
    /// The fetched + decompressed in-memory tree, handed off once.
    pending_source: Option<MslAssetSource>,
    /// Raw **compressed** `parsed-*.bin.zst` bytes. Decompressed/decoded off
    /// the boot future: shipped to the worker + chunk-decoded on main. This is
    /// the fast path when the manifest advertises a pre-parsed bundle.
    pending_parsed_compressed: Option<Vec<u8>>,
    /// Raw **compressed** `sources-*.tar.zst` bytes + their manifest entry,
    /// stashed for lazy unpack on first editor drill-in.
    pending_source_compressed: Option<(Vec<u8>, lunco_assets::msl::MslBundleEntry)>,
    /// Raw **compressed** `sources-*.tar.zst` bytes to untar + reparse **in the
    /// worker** (the tag-mismatch fallback), instead of a synchronous
    /// main-thread untar that would freeze the page. Drained by
    /// [`drain_msl_load_slot`], which ships it to the worker and only untars on
    /// the main thread if no worker is available.
    pending_source_parse_compressed: Option<Vec<u8>>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
struct MslLoadSlot(SharedSlot);

#[cfg(target_arch = "wasm32")]
fn drain_msl_load_slot(
    slot: Res<MslLoadSlot>,
    mut state: ResMut<MslLoadState>,
    mut commands: Commands,
) {
    let mut inner = match slot.0.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some(new_state) = inner.pending_state.take() {
        // Log only on phase transitions / terminal states; progress
        // updates within the same phase would spam the console.
        match (&*state, &new_state) {
            (MslLoadState::Loading { phase: a, .. }, MslLoadState::Loading { phase: b, .. })
                if a == b => {}
            _ => log_state_transition(&new_state),
        }
        *state = new_state;
    }
    // Fast boot path: a compressed parsed bundle is waiting. Ship it to the
    // worker (off-thread decode for compiles) and start the chunked
    // main-thread decode (for resolution/autocomplete). Stash the compressed
    // source for lazy drill-in unpack. `MslLoadState` stays `Loading{Parsing}`
    // until `drive_msl_main_decode` finishes — neither thread blocks the UI.
    if let Some(pbytes) = inner.pending_parsed_compressed.take() {
        // Ship the compressed bundle to the off-thread worker(s). The worker
        // decompresses + deserializes for its own compiles, then transfers the
        // decoded bincode bytes back so the main thread skips the ruzstd
        // decompress and only deserializes into its own heap (resolution /
        // autocomplete) — see `ingest_worker_decoded_msl`.
        let shipped = crate::worker_transport::install_msl_compressed_in_worker(&pbytes);
        if shipped == 0 {
            // No worker (inline path) — the main thread must decompress +
            // deserialize the bundle itself.
            start_main_msl_decode(pbytes);
        } else {
            // Worker will deliver the decoded bytes; keep the compressed blob as
            // a deadline fallback in case it never does (crash before delivery).
            stash_main_decode_fallback(pbytes);
        }
        if let Some((sbytes, smeta)) = inner.pending_source_compressed.take() {
            stash_compressed_source(sbytes, smeta);
        }
        return;
    }
    // Tag-mismatch fallback: a compressed SOURCE bundle is waiting to be
    // reparsed. Prefer the worker (off-thread untar + parse → the main thread
    // ingests deserialize-only via `ingest_worker_decoded_msl`). If no worker is
    // ready yet, wait a bounded number of frames (it comes up during boot); only
    // then untar on the main thread so inline / no-worker builds still load.
    if inner.pending_source_parse_compressed.is_some() {
        let shipped = crate::worker_transport::parse_msl_source_in_worker(
            inner.pending_source_parse_compressed.as_ref().unwrap(),
        );
        if shipped > 0 {
            inner.pending_source_parse_compressed = None;
            // Keep the compressed source for lazy drill-in / msl_index unpack.
            if let Some((sbytes, smeta)) = inner.pending_source_compressed.take() {
                stash_compressed_source(sbytes, smeta);
            }
            return;
        }
        let waited = MSL_SOURCE_PARSE_WAIT.with(|c| {
            let n = c.get() + 1;
            c.set(n);
            n
        });
        if waited < MSL_SOURCE_PARSE_MAX_WAIT {
            return; // worker not up yet — keep pending, retry next frame
        }
        // Give up waiting: untar on the main thread and route to the existing
        // chunked parser via the `pending_source` path (installed next frame).
        let sbytes = inner.pending_source_parse_compressed.take().unwrap();
        match lunco_assets::web_fetch::unpack_tar_zst(&sbytes, 2700) {
            Ok(files) => {
                inner.pending_source = Some(MslAssetSource::InMemory(Arc::new(
                    lunco_assets::msl::MslInMemory { files },
                )));
                bevy::log::warn!(
                    "[MSL] no worker after waiting — untarred + parsing source on the main \
                     thread (chunked)"
                );
            }
            Err(e) => {
                inner.pending_state = Some(MslLoadState::Failed(e));
            }
        }
        if let Some((sbytes, smeta)) = inner.pending_source_compressed.take() {
            stash_compressed_source(sbytes, smeta);
        }
        return;
    }
    if let Some(source) = inner.pending_source.take() {
        // Install the process-wide handle that `ModelicaCompiler::new`
        // consults so the next compile attempt picks it up regardless of
        // system ordering.
        lunco_assets::msl::install_global_msl_sources(sources_with_extras(source.clone()));

        // Slow path: only sources available. Hand the source pairs to
        // the chunked parse driver. Retrigger compile happens after
        // parse completes, otherwise compile would call into the sync
        // `load_source_root_in_memory` path and freeze the page.
        if let MslAssetSource::InMemory(in_memory) = &source {
            let pending = in_memory.as_source_pairs();
            let total = pending.len();
            let now = web_time::Instant::now();
            bevy::log::info!(
                "[MSL] parsing {total} files (chunked, {PARSE_CHUNK_SIZE}/frame)…"
            );
            commands.insert_resource(MslParseInProgress {
                pending,
                parsed: Vec::with_capacity(total),
                total,
                started: now,
                last_log: now,
            });
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn drive_msl_parse(
    state: Option<ResMut<MslParseInProgress>>,
    mut load_state: ResMut<MslLoadState>,
    mut worker: ResMut<crate::worker::InlineWorker>,
    mut commands: Commands,
) {
    let Some(mut state) = state else { return };

    // Parse one chunk this frame. Pop from the back so the Vec walks
    // its tail in O(1); insertion order into `parsed` doesn't matter
    // because rumoca rebuilds its own indices on insert.
    for _ in 0..PARSE_CHUNK_SIZE {
        let Some((uri, source)) = state.pending.pop() else { break };
        match rumoca_phase_parse::parse_to_ast(&source, &uri) {
            Ok(definition) => state.parsed.push((uri, definition)),
            Err(e) => bevy::log::warn!("[MSL] parse '{uri}': {e}"),
        }
    }

    // Mirror current parse progress into the shared `MslLoadState`
    // each frame so the DOM bar (and any other observer) gets a smooth
    // tick. `bytes_done` carries the file count for the Parsing phase
    // — it's not bytes, but the same proportion the bar wants.
    let done = state.total - state.pending.len();
    *load_state = MslLoadState::Loading {
        phase: MslLoadPhase::Parsing,
        bytes_done: done as u64,
        bytes_total: state.total as u64,
    };

    // Log progress on a wall-clock cadence so log output isn't spammy
    // on fast machines or silent on slow ones. Always log the first
    // chunk and the final completion separately below.
    let now = web_time::Instant::now();
    if now.duration_since(state.last_log).as_secs() >= PARSE_LOG_INTERVAL_SECS
        && !state.pending.is_empty()
    {
        let pct = if state.total == 0 {
            100.0
        } else {
            done as f64 / state.total as f64 * 100.0
        };
        bevy::log::info!(
            "[MSL] parsing… {done} / {} files ({pct:.0}%, {:.1}s elapsed)",
            state.total,
            state.started.elapsed().as_secs_f64(),
        );
        state.last_log = now;
    }

    if state.pending.is_empty() {
        let parsed = std::mem::take(&mut state.parsed);
        let total = parsed.len();
        let elapsed = state.started.elapsed();
        bevy::log::info!(
            "[MSL] parse complete — {total} docs in {:.1}s",
            elapsed.as_secs_f64()
        );
        crate::worker_transport::install_msl_in_worker(&parsed);
        install_global_parsed_msl(parsed);
        // Drop the compiler that was lazily built before MSL was ready
        // (or before parse finished); next compile reinstates with the
        // pre-parsed bundle.
        worker.reset_compiler();
        commands.remove_resource::<MslParseInProgress>();
    }
}

fn log_state_transition(s: &MslLoadState) {
    match s {
        MslLoadState::NotStarted => {}
        MslLoadState::Loading {
            phase,
            bytes_done,
            bytes_total,
        } => {
            if *bytes_total > 0 {
                bevy::log::info!(
                    "[MSL] {} ({:.1}/{:.1} MB)",
                    phase.as_str(),
                    *bytes_done as f64 / 1_048_576.0,
                    *bytes_total as f64 / 1_048_576.0,
                );
            } else {
                bevy::log::info!("[MSL] {}", phase.as_str());
            }
        }
        MslLoadState::Ready {
            file_count,
            compressed_bytes,
            uncompressed_bytes,
        } => {
            bevy::log::info!(
                "[MSL] ready — {file_count} files ({:.1} MB compressed → {:.1} MB)",
                *compressed_bytes as f64 / 1_048_576.0,
                *uncompressed_bytes as f64 / 1_048_576.0,
            );
        }
        MslLoadState::Failed(msg) => {
            bevy::log::error!("[MSL] failed: {msg}");
        }
    }
}

// The MSL-state → status-bus mirror moved to `crate::ui::core_observers`
// (reactive UI layer). Core here only owns `MslLoadState` + `MslLoadPhase`.

// ─── Web fetcher implementation ─────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use std::collections::HashSet;

    use lunco_assets::msl::MslManifest;
    use lunco_assets::web_fetch;
    use wasm_bindgen::prelude::*;

    pub(super) async fn run_fetcher(slot: SharedSlot) {
        match try_fetch(&slot).await {
            Ok(()) => {}
            Err(e) => {
                if let Ok(mut s) = slot.lock() {
                    s.pending_state = Some(MslLoadState::Failed(e));
                }
            }
        }
    }

    fn set_state(slot: &SharedSlot, state: MslLoadState) {
        if let Ok(mut s) = slot.lock() {
            s.pending_state = Some(state);
        }
    }

    async fn try_fetch(slot: &SharedSlot) -> Result<(), String> {
        set_state(
            slot,
            MslLoadState::Loading {
                phase: MslLoadPhase::FetchingManifest,
                bytes_done: 0,
                bytes_total: 0,
            },
        );

        let manifest_bytes = web_fetch::fetch_bytes_revalidated(CACHE_NAME, "msl/manifest.json").await?;
        let manifest: MslManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| format!("manifest.json parse: {e}"))?;
        if manifest.schema_version != 1 {
            return Err(format!(
                "unsupported manifest schema_version {}",
                manifest.schema_version
            ));
        }

        // ── Sources blob (small, always shipped). Used by the editor
        // ── for opening MSL files and as a fallback if the parsed
        // ── bundle is unavailable.
        let bundle_path = format!("msl/{}", manifest.sources.filename);
        let phase1 = bundle_fetch_phase(&bundle_path).await;
        // Per-blob progress: this download sweeps 0..its own size, so the bar
        // can't stall at a summed fraction (the old bug: phase 1 only reached
        // the sources/total slice, then relied on phase 2 ticking to move).
        // `total` comes from the fetcher — Content-Length, else the expected
        // size passed below (a Cache-Storage hit often omits Content-Length).
        let sources_total = manifest.sources.compressed_bytes;
        let progress_slot1 = slot.clone();
        let progress_cb1 = Closure::<dyn FnMut(f64, f64)>::new(move |done: f64, total: f64| {
            if let Ok(mut s) = progress_slot1.lock() {
                s.pending_state = Some(MslLoadState::Loading {
                    phase: phase1,
                    bytes_done: done as u64,
                    bytes_total: total as u64,
                });
            }
        });

        let sources_bytes = web_fetch::fetch_cached_with_progress(
            CACHE_NAME,
            &bundle_path,
            sources_total,
            progress_cb1.as_ref().unchecked_ref(),
        )
        .await
        .map_err(|e| format!("sources bundle fetch: {e}"))?;

        if sources_bytes.len() as u64 != manifest.sources.compressed_bytes {
            return Err(format!(
                "sources bundle size {} != manifest {}",
                sources_bytes.len(),
                manifest.sources.compressed_bytes
            ));
        }

        // ── Pre-parsed bundle (when the manifest advertises one AND it was
        // ── produced by a rumoca whose `StoredDefinition` layout matches
        // ── ours). This is the fast path: bincode-decode → install directly
        // ── into rumoca, no per-file parse. A tag mismatch means the bundle
        // ── would deserialize into garbage/error, so we skip it and let the
        // ── source fallback (below) parse from `.mo` instead.
        let tag_ok = manifest.rumoca_artifact_tag.as_deref()
            == Some(lunco_assets::msl::EXPECTED_RUMOCA_ARTIFACT_TAG);
        if manifest.parsed.is_some() && !tag_ok {
            bevy::log::warn!(
                "[MSL] parsed bundle tag {:?} != expected `{}`; ignoring fast path, \
                 parsing source instead (rebuild the bundle with the current rumoca)",
                manifest.rumoca_artifact_tag,
                lunco_assets::msl::EXPECTED_RUMOCA_ARTIFACT_TAG,
            );
        }
        let parsed_bytes = if let Some(parsed_meta) =
            manifest.parsed.as_ref().filter(|_| tag_ok)
        {
            let parsed_path = format!("msl/{}", parsed_meta.filename);
            let phase2 = bundle_fetch_phase(&parsed_path).await;
            // Per-blob again: the (larger) parsed bundle sweeps 0..its own size.
            let parsed_total = parsed_meta.compressed_bytes;
            let progress_slot2 = slot.clone();
            let progress_cb2 = Closure::<dyn FnMut(f64, f64)>::new(move |done: f64, total: f64| {
                if let Ok(mut s) = progress_slot2.lock() {
                    s.pending_state = Some(MslLoadState::Loading {
                        phase: phase2,
                        bytes_done: done as u64,
                        bytes_total: total as u64,
                    });
                }
            });

            let bytes = web_fetch::fetch_cached_with_progress(
                CACHE_NAME,
                &parsed_path,
                parsed_total,
                progress_cb2.as_ref().unchecked_ref(),
            )
            .await
            .map_err(|e| format!("parsed bundle fetch: {e}"))?;
            if bytes.len() as u64 != parsed_meta.compressed_bytes {
                return Err(format!(
                    "parsed bundle size {} != manifest {}",
                    bytes.len(),
                    parsed_meta.compressed_bytes
                ));
            }
            Some(bytes)
        } else {
            None
        };

        // The current blobs are now (re)cached. Evict any superseded
        // content-hashed bundles a previous MSL release left behind so the
        // browser cache doesn't grow without bound. Best-effort — never fails
        // the load.
        {
            // Filenames the current manifest references; everything else in the
            // MSL bucket is a superseded release and gets evicted.
            let mut keep = HashSet::new();
            keep.insert("manifest.json".to_string());
            keep.insert(manifest.sources.filename.clone());
            if let Some(p) = manifest.parsed.as_ref() {
                keep.insert(p.filename.clone());
            }
            web_fetch::prune_cache(CACHE_NAME, &keep).await;
        }

        // Hand the COMPRESSED blobs off WITHOUT decoding them here — this runs
        // on the main-thread event loop, so decompress/untar/decode would
        // freeze the page (the original bug). Two paths:
        //   • fast path (manifest advertises a pre-parsed bundle): stash both
        //     compressed blobs. The drain ships the parsed one to the worker
        //     (off-thread decode) and starts the chunked main-thread decode;
        //     the source bundle is untarred lazily on first drill-in.
        //   • fallback (no pre-parsed bundle): no worker decode possible, so
        //     untar the source here and let the per-frame chunked *parser*
        //     build the AST (slow legacy path; our bundles always ship parsed).
        if parsed_bytes.is_some() {
            set_state(
                slot,
                MslLoadState::Loading {
                    phase: MslLoadPhase::Parsing,
                    bytes_done: 0,
                    bytes_total: manifest
                        .parsed
                        .as_ref()
                        .map(|p| p.file_count as u64)
                        .unwrap_or(0),
                },
            );
            if let Ok(mut s) = slot.lock() {
                s.pending_parsed_compressed = parsed_bytes;
                s.pending_source_compressed = Some((sources_bytes, manifest.sources.clone()));
            }
            return Ok(());
        }

        // ── Fallback (tag mismatch / no pre-parsed bundle): do NOT untar on this
        // ── async task — a synchronous untar on the single wasm thread blocks the
        // ── event loop (the "stall then finish" freeze). Stash the compressed
        // ── source; `drain_msl_load_slot` ships it to the worker for off-thread
        // ── untar + reparse (keys match the build-time bundle), and only untars
        // ── on the main thread if no worker ever comes up. Also stash it for lazy
        // ── drill-in unpack.
        set_state(
            slot,
            MslLoadState::Loading {
                phase: MslLoadPhase::Decompressing,
                bytes_done: 0,
                bytes_total: manifest.sources.uncompressed_bytes,
            },
        );
        if let Ok(mut s) = slot.lock() {
            s.pending_source_parse_compressed = Some(sources_bytes.clone());
            s.pending_source_compressed = Some((sources_bytes, manifest.sources.clone()));
        }

        Ok(())
    }

    const CACHE_NAME: &str = "lunco-msl-v1";

    /// The progress phase to show while fetching `path`: a cache hit loads
    /// locally (no network), so report [`LoadingCache`](MslLoadPhase::LoadingCache)
    /// instead of [`FetchingBundle`](MslLoadPhase::FetchingBundle) ("downloading").
    async fn bundle_fetch_phase(path: &str) -> MslLoadPhase {
        if web_fetch::cache_has(CACHE_NAME, path).await {
            MslLoadPhase::LoadingCache
        } else {
            MslLoadPhase::FetchingBundle
        }
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod parsed_bundle_tests {
    use super::{read_parsed_bundle_file, write_parsed_bundle};

    fn sample_docs() -> Vec<(String, rumoca_compile::parsing::StoredDefinition)> {
        let src = "model M Real x; equation der(x) = -x; end M;";
        let def = rumoca_phase_parse::parse_to_ast(src, "M.mo").expect("parse sample model");
        vec![("M.mo".to_string(), def)]
    }

    /// `write_parsed_bundle` emits a zstd frame, and the reader decodes it
    /// back to the same docs.
    #[test]
    fn zstd_bundle_roundtrips() {
        let dir = std::env::temp_dir().join("lunco_parsed_bundle_zstd");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("parsed-msl.bin");
        let docs = sample_docs();

        write_parsed_bundle(&path, &docs).expect("write compressed bundle");

        // On disk it must be a real zstd frame (magic 0x28 0xB5 0x2F 0xFD).
        let head = std::fs::read(&path).expect("read bundle bytes");
        assert_eq!(
            &head[0..4],
            &[0x28, 0xB5, 0x2F, 0xFD],
            "bundle must be zstd-compressed"
        );

        let back = read_parsed_bundle_file(&path)
            .expect("decode ok")
            .expect("bundle present");
        assert_eq!(back.len(), docs.len());
        assert_eq!(back[0].0, docs[0].0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
