//! MSL bundle loader.
//!
//! Inserts [`MslAssetSource`] and [`MslLoadState`] into the world.
//!
//! ## Native
//!
//! If [`lunco_assets::msl_source_root_path`] returns a path, we use it and
//! build the editor index in the background. If it is absent, the generic
//! dataset registry owns the explicit download; this plugin never opens a
//! native network connection.
//!
//! ## Web
//!
//! The Settings menu can start a `wasm_bindgen_futures::spawn_local` task that:
//!
//! 1. `fetch`es `msl/manifest.json` (same-origin).
//! 2. Parses it into [`lunco_assets::msl::MslManifest`].
//! 3. `fetch`es the compressed bundles named in the manifest.
//! 4. Verifies bundle sizes and artifact tags.
//! 5. Transfers compressed Modelica work to the Web Worker; the worker owns
//!    decompression, source untar, and parsing.
//! 6. The main instance receives serialized AST bytes and time-slices only the
//!    bincode deserialize needed for resolution/autocomplete.
//!
//! State transitions are mirrored to the bevy log so they show up in the
//! Console panel — that's our "status somewhere" until a dedicated status
//! bar lands.

use std::sync::{Arc, Mutex, OnceLock};

use bevy::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
use lunco_assets::datasets::{DatasetRegistry, DatasetState};
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
pub fn global_parsed_msl(
) -> Option<&'static Arc<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>> {
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
///   returns `None` (the worker transfer fills the slot asynchronously).
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
/// `msl_indexer` build step and `ModelicaCompiler`'s cold-parse repair path.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_parsed_bundle(
    path: &std::path::Path,
    docs: &[(String, rumoca_compile::parsing::StoredDefinition)],
) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder =
        zstd::stream::write::Encoder::new(std::io::BufWriter::new(file), PARSED_BUNDLE_ZSTD_LEVEL)?;
    bincode::serde::encode_into_std_write(docs, &mut encoder, bincode::config::standard())
        .map_err(std::io::Error::other)?;
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
    let mut decoder =
        ruzstd::StreamingDecoder::new(compressed).map_err(|e| format!("zstd decoder: {e}"))?;
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
/// the worker's tag-mismatch source recovery (see
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
        let uri = lunco_assets::asset_path::slashed(&path);
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
        return Err(format!(
            "source bundle produced 0 parsed docs ({failed} failed)"
        ));
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

// ─── Chunked main-thread MSL deserialize ──────────────────────────
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
// resource so its large deserialize accumulator stays outside Bevy's resource
// graph. `drive_msl_main_decode` ticks it each `Update`.

#[cfg(target_arch = "wasm32")]
struct MainDecodeState {
    /// Decompressed bincode bytes transferred from the worker.
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
}

/// Seed the chunked main-thread deserializer, no-op if a deserialize is
/// already underway or the bundle is already installed.
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

/// Install the **already-decompressed** bincode bytes the off-thread worker
/// shipped back (transferred `ArrayBuffer`). The main thread then runs only the
/// chunked bincode deserialize into its own heap. No-op if a deserialize is
/// already underway or finished.
#[cfg(target_arch = "wasm32")]
pub fn ingest_worker_decoded_msl(decoded: Vec<u8>) {
    let seeded = seed_main_decode(MainDecodeState {
        out: decoded,
        pos: 0,
        remaining: 0,
        total: 0,
        header_read: false,
        acc: Vec::new(),
    });
    if seeded {
        info!(
            "[MSL] received decoded MSL bytes from worker — deserialize only (no main decompress)"
        );
    }
}

/// Per-frame driver for the chunked main-thread MSL deserialize. No-op once the
/// `MAIN_DECODE` slot is empty (the common case after boot). On completion it
/// installs `GLOBAL_PARSED_MSL` and flips `MslLoadState` to `Ready`, after
/// which `drive_msl_bootstrap` seeds the workspace engine session exactly as
/// before — so resolution/autocomplete are unaffected, just non-blocking.
#[cfg(target_arch = "wasm32")]
fn drive_msl_main_decode(mut state: ResMut<MslLoadState>) {
    // Tuned so each frame's slice stays a few ms. Deserialization allocates
    // deep ASTs, so its chunk is in documents.
    const DESER_CHUNK: usize = 96;

    MAIN_DECODE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(d) = guard.as_mut() else {
            return;
        };

        // Bincode-deserialize, bounded docs per frame. The blob is a
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
            info!(
                "[MSL] main-thread deserialize complete: {count} docs — resolution/autocomplete ready"
            );
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
            for (subdir, _pkg) in crate::package_tree::scanner::discover_third_party_libs() {
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
pub fn install_global_parsed_msl_pub(
    parsed: Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
) {
    install_global_parsed_msl(parsed);
}

/// Web: a Settings action requests one explicit async MSL fetch.
#[cfg(target_arch = "wasm32")]
fn kick_web_msl_fetcher(
    slot: Res<MslLoadSlot>,
    mut request: ResMut<WebMslInstallRequest>,
    mut state: ResMut<MslLoadState>,
    settings: Res<lunco_settings::DownloadSettings>,
) {
    if !request.0 {
        return;
    }
    request.0 = false;
    *state = MslLoadState::Loading {
        phase: MslLoadPhase::FetchingManifest,
        bytes_done: 0,
        bytes_total: 0,
    };
    wasm_bindgen_futures::spawn_local(web::run_fetcher(slot.0.clone(), settings.clone()));
}

/// Plugin that owns MSL asset loading. Add once during app build.
pub struct MslRemotePlugin;

/// Web-only user intent for the MSL bundle fetcher. Native downloads are
/// owned by [`DatasetRegistry`] and requested by the generic data panel.
#[cfg(target_arch = "wasm32")]
#[derive(Event, Clone, Copy, Debug)]
pub enum MslInstallAction {
    Install,
    Reinstall,
}

#[cfg(target_arch = "wasm32")]
#[derive(Resource, Default)]
struct WebMslInstallRequest(bool);

impl Plugin for MslRemotePlugin {
    fn build(&self, app: &mut App) {
        lunco_settings::ensure_download_settings(app);
        app.init_resource::<MslLoadState>();
        // Persisted user settings (the local-root override). Lives in
        // settings.json so the Settings menu and source resolver share one
        // source of truth.
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<crate::msl_settings::MslSettings>();

        #[cfg(target_arch = "wasm32")]
        app.add_observer(on_msl_install_action);
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.add_observer(on_native_msl_index_action);
            app.add_systems(
                Update,
                (drain_native_msl_install, drive_native_msl_dataset).chain(),
            );
        }

        // (The MSL-state → status-bus mirror is a UI reactive observer; it
        // lives in `ui::core_observers` and is registered by the UI plugin.
        // Core just owns `MslLoadState`.)

        // Native: use an already-materialised tree (workspace dev cache or a
        // user-supplied override). The generic dataset registry owns any
        // explicit download; this plugin only turns the installed tree into
        // the Modelica source/index used by the domain.
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
                info!(
                    "[MSL] using on-disk root {} ({count} .mo files)",
                    root.display()
                );
                lunco_assets::msl::install_global_msl_sources(sources_with_extras(
                    MslAssetSource::Filesystem(root.clone()),
                ));
                if crate::visual_diagram::msl_index_available() {
                    app.insert_resource(MslLoadState::Ready {
                        file_count: count,
                        compressed_bytes: 0,
                        uncompressed_bytes: 0,
                    });
                } else {
                    info!(
                        "[MSL] source root is present but its generated editor index is missing; indexing in the background"
                    );
                    app.insert_resource(MslLoadState::Loading {
                        phase: MslLoadPhase::Parsing,
                        bytes_done: 0,
                        bytes_total: 0,
                    });
                    app.insert_resource(native_index_resources(root));
                }
            } else {
                info!("[MSL] no on-disk root — waiting for the dataset registry");
                app.insert_resource(MslLoadState::NotStarted);
            }

            // NO startup warm. Filling the parsed slot here would flip
            // `drive_msl_bootstrap` onto its eager branch on every native launch,
            // costing a measured 1645 ms main-thread stall on a scene with no
            // Modelica in it. `parsed_msl_bundle` loads lazily at first lookup.
        }

        // Web: create the dormant fetch slot. The Settings menu inserts an
        // explicit request; until then no network task is started.
        #[cfg(target_arch = "wasm32")]
        {
            let slot: SharedSlot = Arc::new(Mutex::new(SlotInner::default()));
            app.insert_resource(MslLoadState::NotStarted);
            app.insert_resource(MslLoadSlot(slot));
            app.init_resource::<WebMslInstallRequest>();
            app.add_systems(
                Update,
                (
                    kick_web_msl_fetcher,
                    drain_msl_load_slot,
                    drive_msl_main_decode,
                )
                    .chain(),
            );
        }
    }
}

// ─── Native dataset bridge and background index ────────────────────
//
// `lunco-assets` owns the manifest, download, processing, cancellation and
// operation lifetime. This module observes that one authoritative state and owns
// only the Modelica-specific post-download index.

#[cfg(not(target_arch = "wasm32"))]
type NativeInstallSlot = Arc<Mutex<NativeInstallSlotInner>>;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct NativeInstallSlotInner {
    /// Latest load-state the worker has reported; drained each frame.
    pending_state: Option<MslLoadState>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct NativeMslInstallSlot {
    state: NativeInstallSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
fn native_index_resources(root: std::path::PathBuf) -> NativeMslInstallSlot {
    let slot: NativeInstallSlot = Arc::new(Mutex::new(NativeInstallSlotInner::default()));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    lunco_assets::msl::install_global_msl_sources(sources_with_extras(MslAssetSource::Filesystem(
        root.clone(),
    )));
    spawn_native_index(slot.clone(), root, cancel.clone());
    NativeMslInstallSlot {
        state: slot,
        cancel,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_native_index(
    slot: NativeInstallSlot,
    root: std::path::PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) {
    bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async move {
            bevy::log::info!("[MSL] indexing editor metadata for {}…", root.display());
            let completed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::indexer::run_with_cancel(
                    crate::indexer::Options::for_source_root(root.clone()),
                    Some(cancel.clone()),
                );
            }))
            .is_ok();

            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            if !completed || !crate::visual_diagram::msl_index_available() {
                set_install_state(
                    &slot,
                    MslLoadState::Failed(format!(
                        "MSL editor index was not generated at {}",
                        root.join("msl_index.json").display()
                    )),
                );
                return;
            }

            set_install_state(
                &slot,
                MslLoadState::Ready {
                    file_count: count_mo_files(&root),
                    compressed_bytes: 0,
                    uncompressed_bytes: 0,
                },
            );
        })
        .detach();
}

#[cfg(target_arch = "wasm32")]
fn on_msl_install_action(trigger: On<MslInstallAction>, mut request: ResMut<WebMslInstallRequest>) {
    if matches!(
        *trigger.event(),
        MslInstallAction::Install | MslInstallAction::Reinstall
    ) {
        request.0 = true;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn set_install_state(slot: &NativeInstallSlot, state: MslLoadState) {
    if let Ok(mut inner) = slot.lock() {
        inner.pending_state = Some(state);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn drain_native_msl_install(
    slot: Option<Res<NativeMslInstallSlot>>,
    mut state: ResMut<MslLoadState>,
) {
    let Some(slot) = slot else { return };
    let Ok(mut inner) = slot.state.lock() else {
        return;
    };
    if let Some(new_state) = inner.pending_state.take() {
        match (&*state, &new_state) {
            (MslLoadState::Loading { phase: a, .. }, MslLoadState::Loading { phase: b, .. })
                if a == b => {}
            _ => log_state_transition(&new_state),
        }
        *state = new_state;
    }
}

#[cfg(not(target_arch = "wasm32"))]
const NATIVE_MSL_DATASET_ID: &str = "engine/modelica/msl";

/// User intent to rebuild the native editor index after a failed post-download
/// indexing attempt. Downloading remains the generic dataset registry's job;
/// this action only restarts Modelica's domain projection.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Event, Clone, Copy, Debug)]
pub enum NativeMslIndexAction {
    Rebuild,
}

#[cfg(not(target_arch = "wasm32"))]
fn on_native_msl_index_action(
    trigger: On<NativeMslIndexAction>,
    state: Res<MslLoadState>,
    existing: Option<Res<NativeMslInstallSlot>>,
    mut commands: Commands,
) {
    if !matches!(*trigger.event(), NativeMslIndexAction::Rebuild)
        || !matches!(*state, MslLoadState::Failed(_))
    {
        return;
    }
    let Some(root) = lunco_assets::msl_source_root_path() else {
        return;
    };
    if let Some(existing) = existing {
        existing
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    commands.insert_resource(MslLoadState::Loading {
        phase: MslLoadPhase::Parsing,
        bytes_done: 0,
        bytes_total: 0,
    });
    commands.insert_resource(native_index_resources(root));
}

#[cfg(not(target_arch = "wasm32"))]
fn drive_native_msl_dataset(
    registry: Option<Res<DatasetRegistry>>,
    slot: Option<Res<NativeMslInstallSlot>>,
    mut state: ResMut<MslLoadState>,
    mut commands: Commands,
) {
    let Some(registry) = registry else { return };
    let Some(dataset) = registry
        .entries()
        .iter()
        .find(|entry| entry.id == NATIVE_MSL_DATASET_ID)
    else {
        return;
    };

    match &dataset.state {
        DatasetState::Missing => *state = MslLoadState::NotStarted,
        DatasetState::Downloading {
            bytes_done,
            bytes_total,
        } => {
            *state = MslLoadState::Loading {
                phase: MslLoadPhase::FetchingBundle,
                bytes_done: *bytes_done,
                bytes_total: *bytes_total,
            };
        }
        DatasetState::Processing { .. } => {
            *state = MslLoadState::Loading {
                phase: MslLoadPhase::Parsing,
                bytes_done: 0,
                bytes_total: 0,
            };
        }
        DatasetState::Cancelling => {
            *state = MslLoadState::Loading {
                phase: MslLoadPhase::FetchingBundle,
                bytes_done: 0,
                bytes_total: 0,
            };
        }
        DatasetState::Cancelled => *state = MslLoadState::NotStarted,
        DatasetState::Failed(error) => *state = MslLoadState::Failed(error.clone()),
        DatasetState::Installed => {
            // A slot remains as the lifecycle marker for the one index attempt.
            // This prevents a failed indexer from being relaunched every frame.
            if slot.is_some() {
                return;
            }
            let Some(root) = lunco_assets::msl_source_root_path() else {
                *state = MslLoadState::Failed(
                    "dataset is installed but no Modelica/ tree exists in the cache".into(),
                );
                return;
            };
            if crate::visual_diagram::msl_index_available() {
                lunco_assets::msl::install_global_msl_sources(sources_with_extras(
                    MslAssetSource::Filesystem(root.clone()),
                ));
                *state = MslLoadState::Ready {
                    file_count: count_mo_files(&root),
                    compressed_bytes: 0,
                    uncompressed_bytes: 0,
                };
                return;
            }
            *state = MslLoadState::Loading {
                phase: MslLoadPhase::Parsing,
                bytes_done: 0,
                bytes_total: 0,
            };
            commands.insert_resource(native_index_resources(root));
        }
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

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SlotInner {
    /// Latest state the fetcher has reported. The drain system replaces
    /// the world's `MslLoadState` whenever this `take`s out a new value.
    pending_state: Option<MslLoadState>,
    /// Raw **compressed** `parsed-*.bin.zst` bytes. Decompressed/decoded off
    /// the boot future: shipped to the worker + chunk-decoded on main. This is
    /// the fast path when the manifest advertises a pre-parsed bundle.
    pending_parsed_compressed: Option<Vec<u8>>,
    /// Raw **compressed** `sources-*.tar.zst` bytes + their manifest entry,
    /// stashed for lazy unpack on first editor drill-in.
    pending_source_compressed: Option<(Vec<u8>, lunco_assets::msl::MslBundleEntry)>,
    /// Raw **compressed** `sources-*.tar.zst` bytes to untar + reparse **in the
    /// worker** when the pre-parsed bundle tag does not match. Drained by
    /// [`drain_msl_load_slot`].
    pending_source_parse_compressed: Option<Vec<u8>>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
struct MslLoadSlot(SharedSlot);

#[cfg(target_arch = "wasm32")]
fn drain_msl_load_slot(slot: Res<MslLoadSlot>, mut state: ResMut<MslLoadState>) {
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
    // worker for decompression/deserialization and transfer the decoded bytes
    // back for chunked main-thread deserialization (resolution/autocomplete).
    // Stash the compressed source for lazy drill-in unpack. `MslLoadState`
    // stays `Loading{Parsing}` until `drive_msl_main_decode` finishes.
    if let Some(pbytes) = inner.pending_parsed_compressed.take() {
        // Ship the compressed bundle to the off-thread worker(s). The worker
        // decompresses + deserializes for its own compiles, then transfers the
        // decoded bincode bytes back so the main thread skips the ruzstd
        // decompress and only deserializes into its own heap (resolution /
        // autocomplete) — see `ingest_worker_decoded_msl`.
        let shipped = crate::worker_transport::install_msl_compressed_in_worker(&pbytes);
        if shipped == 0 {
            inner.pending_state = Some(MslLoadState::Failed(
                "Modelica Web Worker is unavailable; rebuild the browser worker bundle and retry"
                    .into(),
            ));
        }
        if let Some((sbytes, smeta)) = inner.pending_source_compressed.take() {
            stash_compressed_source(sbytes, smeta);
        }
        return;
    }
    // Tag-mismatch source recovery: a compressed SOURCE bundle is waiting to
    // be reparsed. The worker untars and parses it, then the main thread
    // ingests only the transferred serialized AST bytes.
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
        inner.pending_source_parse_compressed = None;
        inner.pending_state = Some(MslLoadState::Failed(
            "Modelica Web Worker is unavailable for the MSL source bundle; retry after rebuilding the browser worker bundle".into(),
        ));
        if let Some((sbytes, smeta)) = inner.pending_source_compressed.take() {
            stash_compressed_source(sbytes, smeta);
        }
        return;
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

    pub(super) async fn run_fetcher(slot: SharedSlot, settings: lunco_settings::DownloadSettings) {
        match try_fetch(&slot, &settings).await {
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

    async fn try_fetch(
        slot: &SharedSlot,
        settings: &lunco_settings::DownloadSettings,
    ) -> Result<(), String> {
        set_state(
            slot,
            MslLoadState::Loading {
                phase: MslLoadPhase::FetchingManifest,
                bytes_done: 0,
                bytes_total: 0,
            },
        );

        let manifest_bytes =
            web_fetch::fetch_bytes_revalidated(CACHE_NAME, "msl/manifest.json", settings).await?;
        let manifest: MslManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| format!("manifest.json parse: {e}"))?;
        if manifest.schema_version != 1 {
            return Err(format!(
                "unsupported manifest schema_version {}",
                manifest.schema_version
            ));
        }

        // ── Sources blob (small, always shipped). Used by the editor
        // ── for opening MSL files and for worker-owned source recovery when
        // ── the parsed bundle is unavailable.
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
            settings,
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
        // ── source recovery (below) parse from `.mo` instead.
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
        let parsed_bytes = if let Some(parsed_meta) = manifest.parsed.as_ref().filter(|_| tag_ok) {
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
                settings,
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
        //     (off-thread decode) and starts the chunked main-thread deserialize;
        //     the source bundle is untarred lazily on first drill-in.
        //   • source-bundle path (no pre-parsed bundle): no worker decode
        //     possible, so untar the source here and let the per-frame
        //     chunked parser build the AST.
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

        // ── Source recovery (tag mismatch / no pre-parsed bundle): do NOT untar on this
        // ── async task — a synchronous untar on the single wasm thread blocks the
        // ── event loop (the "stall then finish" freeze). Stash the compressed
        // ── source; `drain_msl_load_slot` ships it to the worker for off-thread
        // ── untar + reparse (keys match the build-time bundle). Also stash it for lazy
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
