//! Source-library bundle loader and runtime admission.
//!
//! Inserts [`LibraryAssetSource`] and [`LibraryLoadState`] into the world.
//!
//! ## Native
//!
//! If the configured source-library root returns a path, we install it as the
//! runtime source. Native editor indexing/provisioning is composed separately
//! by `lunco-modelica-assets`; this plugin never opens a native network
//! connection.
//!
//! ## Web
//!
//! The Settings menu can start a `wasm_bindgen_futures::spawn_local` task that:
//!
//! 1. `fetch`es `library/manifest.json` (same-origin).
//! 2. Parses it into the generic source-library bundle manifest.
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

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use bevy::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::worker_bridge::ModelicaWorkerBridge;
#[cfg(target_arch = "wasm32")]
use lunco_assets_core::library::InMemoryLibrary as LibraryInMemory;
#[cfg(target_arch = "wasm32")]
use lunco_assets_core::library::LibraryLoadPhase;
use lunco_assets_core::library::{LibraryLoadState, LibrarySource as LibraryAssetSource};

/// Asset-library prefix for the Modelica source-library bundle.
pub const SOURCE_LIBRARY_ASSET_ROOT: &str = "library";
/// Name of the mutable manifest fetched before the source-library bundles.
pub const SOURCE_LIBRARY_MANIFEST_FILE_NAME: &str = "manifest.json";
/// Filename of the generated native source-library parse bundle.
pub const PARSED_LIBRARY_BUNDLE_FILE_NAME: &str = "parsed-library.bin";

/// Return the cache directory owned by the Modelica source library.
pub fn source_library_cache_dir() -> PathBuf {
    lunco_assets_core::source_library_dir(SOURCE_LIBRARY_ASSET_ROOT)
}

/// Resolve the materialised Modelica source-library root, if it exists.
pub fn source_library_root_path() -> Option<PathBuf> {
    lunco_assets_core::source_library_root_path(SOURCE_LIBRARY_ASSET_ROOT)
}

/// Return the web-relative path of one source-library bundle artifact.
///
/// The manifest supplies only a filename. Rejecting separators, absolute paths,
/// and parent traversal here keeps a malformed remote manifest from escaping
/// the source-library URL namespace.
pub fn source_library_asset_path(file_name: &str) -> Result<String, String> {
    let relative = lunco_assets_path::relative_path(file_name)
        .ok_or_else(|| format!("invalid source-library artifact filename {file_name}"))?;
    if relative.components().count() != 1 {
        return Err(format!(
            "source-library artifact must be a filename, got {file_name}"
        ));
    }
    Ok(format!("{SOURCE_LIBRARY_ASSET_ROOT}/{file_name}"))
}

/// Process-wide pre-parsed source library documents. Populated on wasm by the
/// chunked parse driver once the full bundle has been turned into
/// `StoredDefinition`s. `ModelicaCompiler::new` reads it (via
/// [`global_parsed_source_bundle`]) and installs into rumoca via
/// `Session::replace_parsed_source_set` — the entire parse cost is
/// already paid by then, so compile init is fast.
static GLOBAL_PARSED_SOURCE_BUNDLE: OnceLock<
    Arc<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>,
> = OnceLock::new();

/// Serializes the native lazy decode of `parsed-library.bin`. `GLOBAL_PARSED_SOURCE_BUNDLE`
/// (a `OnceLock`) dedupes the stored *value* but not the *work*: two callers
/// that both miss `get()` will each run the full ~1.2 s zstd+bincode decode,
/// and the loser's `set()` is silently dropped. In the sandbox that race is
/// real — the worker's `ModelicaCompiler` session and the main-thread
/// `ModelicaEngine` session both reach for source library on the first compile. This lock
/// makes the second caller block on the first decode and reuse it. Native-only;
/// wasm is single-threaded so no race exists there.
#[cfg(not(target_arch = "wasm32"))]
static SOURCE_BUNDLE_DECODE_LOCK: Mutex<()> = Mutex::new(());

/// Read the pre-parsed source library bundle if any has been installed.
pub fn global_parsed_source_bundle()
-> Option<&'static Arc<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>> {
    GLOBAL_PARSED_SOURCE_BUNDLE.get()
}

/// Publish a freshly parsed source library bundle to the process-wide slot. Only
/// the first install wins; subsequent calls are silently ignored
/// (the `OnceLock` guarantees a stable handle for the lifetime of
/// the page session).
fn install_global_parsed_source_bundle(
    parsed: Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
) {
    let _ = GLOBAL_PARSED_SOURCE_BUNDLE.set(Arc::new(parsed));
}

/// The pre-parsed source library bundle, loading it on demand if not yet present.
///
/// This is the **unified** accessor that drill-in / class-lookup paths
/// use on both targets:
/// - If [`global_parsed_source_bundle`] is already populated (wasm chunked decode,
///   worker hand-off, or a prior native lazy-load), return it.
/// - On **native**, lazily deserialize `parsed-library.bin` (the bundle the
///   `modelica_library_indexer` writes) into the process-wide slot on first call —
///   one ~1–3 s bincode decode, then every subsequent lookup is an
///   in-memory hit. This replaces the old per-file `parse_files_parallel`
///   path that paid a full rumoca parse (tens of seconds for big
///   `package.mo` wrappers) on every drill-in.
/// - On **wasm** there is no synchronous disk path, so a miss just
///   returns `None` (the worker transfer fills the slot asynchronously).
pub fn parsed_source_bundle()
-> Option<&'static Arc<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>> {
    if let Some(bundle) = GLOBAL_PARSED_SOURCE_BUNDLE.get() {
        return Some(bundle);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Hold the decode lock for the whole miss path, then re-check: a
        // peer may have filled the slot while we waited on the lock, in
        // which case we skip the redundant decode entirely.
        let _guard = SOURCE_BUNDLE_DECODE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(bundle) = GLOBAL_PARSED_SOURCE_BUNDLE.get() {
            return Some(bundle);
        }
        let bundle_rel = std::path::Path::new(PARSED_LIBRARY_BUNDLE_FILE_NAME);
        // The bundle is zstd-compressed bincode (~10× smaller on disk than the
        // raw bincode it replaced). A stale/foreign bundle that fails to decode
        // returns `Err` below → the caller cold-parses and rewrites it. The
        // decode streams — it never holds the whole file as a `Vec<u8>`.
        match read_parsed_bundle_file() {
            Ok(Some(docs)) => {
                info!(
                    "[source library] lazy-loaded pre-parsed bundle ({} docs) from `{}` \
                     into process-wide slot",
                    docs.len(),
                    "installed source library"
                );
                install_global_parsed_source_bundle(docs);
            }
            // No bundle on disk yet (indexer hasn't run) — caller parses source.
            Ok(None) => {}
            Err(e) => {
                // Stale/format-mismatched bundle (e.g. after a rumoca bump) —
                // caller falls back to a direct parse.
                warn!(
                    "[source library] parsed bundle at `{}` failed to decode ({e}); \
                     drill-in will parse source directly",
                    bundle_rel.display()
                );
            }
        }
    }
    GLOBAL_PARSED_SOURCE_BUNDLE.get()
}

/// Read the native `parsed-library.bin` fast-path bundle (zstd-compressed
/// bincode), streaming the decode so the whole file is never held as a
/// `Vec<u8>`.
///
/// `Ok(None)` = no/empty file (indexer hasn't run); `Err` = a present-but-
/// undecodable bundle (rumoca-version-stale, truncated, or a pre-zstd raw
/// bundle) so the caller cold-parses the source root and rewrites it.
#[cfg(not(target_arch = "wasm32"))]
fn read_parsed_bundle_file()
-> Result<Option<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>, String> {
    let Some((_, file)) =
        lunco_assets_core::library::library_open(std::path::Path::new(
            PARSED_LIBRARY_BUNDLE_FILE_NAME,
        ))
    else {
        return Ok(None);
    };
    if file.metadata().map(|m| m.len() == 0).unwrap_or(false) {
        return Ok(None); // empty / truncated
    }
    let mut decoder = ruzstd::StreamingDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| format!("zstd decoder: {e}"))?;
    bincode::serde::decode_from_std_read::<
        Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
        _,
        _,
    >(&mut decoder, bincode::config::standard())
    .map(Some)
    .map_err(|e| format!("bincode: {e}"))
}

/// Inflate a `parsed-*.bin.zst` blob to the raw bincode bytes, *without*
/// deserializing. The worker uses this so it can both decode its own ASTs
/// (`bincode::deserialize` the returned bytes) **and** ship the same decoded
/// bytes to the main thread (transferred `ArrayBuffer`) — letting the main
/// thread skip the ruzstd decompress and only deserialize. See
/// [`ingest_worker_decoded_library`].
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

/// Extract the generated editor index from a web source bundle without
/// materialising Modelica ASTs. The worker uses this on the pre-parsed fast
/// path, where the source archive is otherwise retained only for lazy drill-in.
#[cfg(target_arch = "wasm32")]
pub fn load_library_index_from_source_bundle(
    compressed: &[u8],
) -> Result<lunco_modelica_index::visual_diagram::LibraryIndex, String> {
    let files = lunco_assets_core::web_fetch::unpack_tar_zst(compressed, 1)?;
    let bytes = files
        .get(std::path::Path::new(
            lunco_modelica_index::visual_diagram::LIBRARY_INDEX_FILE_NAME,
        ))
        .ok_or_else(|| {
            format!(
                "source bundle has no generated {}",
                lunco_modelica_index::visual_diagram::LIBRARY_INDEX_FILE_NAME
            )
        })?;
    lunco_modelica_index::visual_diagram::decode_library_index(bytes)
}

// ─── Chunked main-thread source library deserialize ──────────────────────────
//
// On wasm the main-thread rumoca session needs the source library ASTs in *its own*
// linear memory for reference resolution / autocomplete — the worker's copy
// lives in a separate memory and can't be shared. So the main thread must
// spend the CPU to materialise ~173 MB of `StoredDefinition`s. Doing it in one
// `bincode::deserialize_from` call froze the page for seconds; instead we
// time-slice it across frames (chunked decompress, then chunked deserialize)
// so the UI stays responsive while source library becomes ready a second or two in.
//
// State lives in a `thread_local` (wasm is single-threaded) rather than a Bevy
// resource so its large deserialize accumulator stays outside Bevy's resource
// graph. `drive_library_main_decode` ticks it each `Update`.

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
    if global_parsed_source_bundle().is_some() {
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
pub fn ingest_worker_decoded_library(decoded: Vec<u8>) {
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
            "[source library] received decoded source library bytes from worker — deserialize only (no main decompress)"
        );
    }
}

/// Per-frame driver for the chunked main-thread source library deserialize. No-op once the
/// `MAIN_DECODE` slot is empty (the common case after boot). On completion it
/// installs `GLOBAL_PARSED_SOURCE_BUNDLE` and flips `LibraryLoadState` to `Ready`, after
/// which `drive_source_bundle_bootstrap` seeds the workspace engine session exactly as
/// before — so resolution/autocomplete are unaffected, just non-blocking.
#[cfg(target_arch = "wasm32")]
fn drive_library_main_decode(mut state: ResMut<LibraryLoadState>) {
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
                    warn!("[source library] main decode: bad bundle header: {e}");
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
                    warn!("[source library] main decode deserialize error: {e}");
                    d.remaining = 0;
                    break;
                }
            }
        }

        if d.remaining == 0 {
            let docs = std::mem::take(&mut d.acc);
            let count = docs.len();
            let uncompressed = d.out.len() as u64;
            install_global_parsed_source_bundle(docs);
            *guard = None; // frees `out`
            *state = LibraryLoadState::Ready {
                file_count: count,
                compressed_bytes: 0,
                uncompressed_bytes: uncompressed,
            };
            info!(
                "[source library] main-thread deserialize complete: {count} docs — resolution/autocomplete ready"
            );
        } else {
            *state = LibraryLoadState::Loading {
                phase: LibraryLoadPhase::Parsing,
                bytes_done: d.total - d.remaining,
                bytes_total: d.total,
            };
        }
    });
}

// ─── Lazy source-bundle unpack ─────────────────────────────────────
//
// The 37 MB source tree is only needed when the user drills into a source-library file
// in the editor — so we keep it compressed and untar it on first demand
// (`ensure_library_source_unpacked`) instead of on the boot future, where it was a
// second freeze. Image/icon loading is disabled on wasm, so nothing else needs
// it at boot.
#[cfg(target_arch = "wasm32")]
static LIBRARY_SOURCE_COMPRESSED: OnceLock<(
    Vec<u8>,
    lunco_assets_core::library::LibraryBundleEntry,
)> = OnceLock::new();

#[cfg(target_arch = "wasm32")]
fn stash_compressed_source(bytes: Vec<u8>, meta: lunco_assets_core::library::LibraryBundleEntry) {
    let _ = LIBRARY_SOURCE_COMPRESSED.set((bytes, meta));
}

/// Untar the source bundle into the process-wide `LibraryAssetSource` on first
/// use (idempotent). Called by the drill-in paths (`Document::load_library_class` /
/// compiler-owned library document loader before it reads source text. No-op if already
/// unpacked or if no compressed source was stashed.
#[cfg(target_arch = "wasm32")]
pub fn ensure_library_source_unpacked() {
    if lunco_assets_core::library::has_library_source() {
        return;
    }
    let Some((bytes, meta)) = LIBRARY_SOURCE_COMPRESSED.get() else {
        return;
    };
    match lunco_assets_core::web_fetch::unpack_tar_zst(bytes, meta.file_count) {
        Ok(files) => {
            let n = files.len();
            lunco_assets_core::library::install_global_library_sources(vec![
                LibraryAssetSource::InMemory(Arc::new(LibraryInMemory { files })),
            ]);
            info!("[source library] source bundle unpacked lazily ({n} files) for drill-in");
        }
        Err(e) => warn!("[source library] lazy source unpack failed: {e}"),
    }
}

/// `pub` re-export of `install_global_parsed_source_bundle` so the off-thread
/// worker bin (`bin/lunica_worker.rs`) can install the source library bundle it
/// receives over postMessage.
#[cfg(target_arch = "wasm32")]
pub fn install_global_parsed_source_bundle_pub(
    parsed: Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
) {
    install_global_parsed_source_bundle(parsed);
}

#[cfg(target_arch = "wasm32")]
struct WebLibraryIndexAssembly {
    components: Vec<lunco_modelica_index::index::ClassEntry>,
    bundled: Vec<lunco_modelica_index::package_tree::types::PackageNode>,
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static WEB_LIBRARY_INDEX: std::cell::RefCell<Option<WebLibraryIndexAssembly>> =
        const { std::cell::RefCell::new(None) };
    static WEB_LIBRARY_INDEX_READY: std::cell::RefCell<Option<lunco_modelica_index::visual_diagram::LibraryIndex>> =
        const { std::cell::RefCell::new(None) };
}

/// Receive one bounded editor-index chunk from the Modelica worker. Keeping
/// chunks on the main side avoids decoding the entire generated index inside
/// one browser event-loop turn.
#[cfg(target_arch = "wasm32")]
pub fn ingest_worker_library_index_chunk(
    components: Vec<lunco_modelica_index::index::ClassEntry>,
    bundled: Vec<lunco_modelica_index::package_tree::types::PackageNode>,
    done: bool,
) {
    WEB_LIBRARY_INDEX.with(|slot| {
        let mut slot = slot.borrow_mut();
        let assembly = slot.get_or_insert_with(|| WebLibraryIndexAssembly {
            components: Vec::new(),
            bundled: Vec::new(),
        });
        assembly.components.extend(components);
        assembly.bundled.extend(bundled);
        if done {
            let assembly = slot
                .take()
                .expect("source library index assembly disappeared while completing");
            WEB_LIBRARY_INDEX_READY.with(|ready| {
                *ready.borrow_mut() = Some(lunco_modelica_index::visual_diagram::LibraryIndex {
                    components: assembly.components,
                    bundled: assembly.bundled,
                });
            });
        }
    });
}

#[cfg(target_arch = "wasm32")]
pub fn fail_worker_library_index(error: String) {
    WEB_LIBRARY_INDEX.with(|slot| *slot.borrow_mut() = None);
    bevy::log::error!("[source library] editor index load failed: {error}");
}

/// Publish a terminal worker failure to the Bevy-owned source library state. The worker
/// callback has no `World` access, so it crosses the same shared slot used by
/// the fetch task and lets `drain_library_load_slot` update the resource.
#[cfg(target_arch = "wasm32")]
pub fn fail_worker_library(error: String) {
    if let Some(slot) = WEB_LIBRARY_SLOT.get() {
        if let Ok(mut slot) = slot.lock() {
            slot.pending_parsed_compressed = None;
            slot.pending_source_compressed = None;
            slot.pending_state = Some(LibraryLoadState::Failed(error));
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn drive_web_library_index(mut commands: Commands) {
    let index = WEB_LIBRARY_INDEX_READY.with(|ready| ready.borrow_mut().take());
    let Some(index) = index else { return };
    if lunco_modelica_index::visual_diagram::install_library_index(index) {
        bevy::log::info!("[source library] editor index loaded in bounded worker chunks");
        commands.trigger(lunco_modelica_index::visual_diagram::LibraryEditorIndexBecameReady);
    }
}

/// Web: a Settings action requests one explicit async source library fetch.
#[cfg(target_arch = "wasm32")]
fn kick_web_library_fetcher(
    slot: Res<LibraryLoadSlot>,
    mut request: ResMut<WebLibraryInstallRequest>,
    mut state: ResMut<LibraryLoadState>,
    settings: Res<lunco_settings::DownloadSettings>,
    worker_bridge: Res<ModelicaWorkerBridge>,
) {
    if !request.0 {
        return;
    }
    request.0 = false;
    *state = LibraryLoadState::Loading {
        phase: LibraryLoadPhase::FetchingManifest,
        bytes_done: 0,
        bytes_total: 0,
    };
    wasm_bindgen_futures::spawn_local(web::run_fetcher(
        slot.0.clone(),
        settings.clone(),
        worker_bridge.clone(),
    ));
}

/// Plugin that owns source-library asset loading. Add once during app build.
pub struct SourceLibraryPlugin;

/// Web-only user intent for the source library bundle fetcher. Native download
/// requests remain owned by the generic dataset registry and data panel.
#[cfg(target_arch = "wasm32")]
#[derive(Event, Clone, Copy, Debug)]
pub enum LibraryInstallAction {
    Install,
    Reinstall,
}

#[cfg(target_arch = "wasm32")]
#[derive(Resource, Default)]
struct WebLibraryInstallRequest(bool);

#[cfg(target_arch = "wasm32")]
fn on_library_install_action(
    trigger: On<LibraryInstallAction>,
    mut request: ResMut<WebLibraryInstallRequest>,
    worker_bridge: Res<ModelicaWorkerBridge>,
) {
    if matches!(trigger.event(), LibraryInstallAction::Reinstall) {
        worker_bridge.reset_pipeline();
    }
    request.0 = true;
}

impl Plugin for SourceLibraryPlugin {
    fn build(&self, app: &mut App) {
        lunco_settings::ensure_download_settings(app);
        app.init_resource::<LibraryLoadState>();
        // Persisted user settings (the local-root override). Lives in
        // settings.json so the Settings menu and source resolver share one
        // source of truth.
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<crate::settings::LibrarySettings>();

        #[cfg(target_arch = "wasm32")]
        app.add_observer(on_library_install_action);
        // (The source library-state → status-bus mirror is a UI reactive observer; it
        // lives in `ui::core_observers` and is registered by the UI plugin.
        // Core just owns `LibraryLoadState`.)

        // Native: use an already-materialised tree (workspace dev cache or a
        // user-supplied override). Provisioning and editor-index generation
        // are optional host capabilities composed by `lunco-modelica-assets`.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let settings = app.world().resource::<crate::settings::LibrarySettings>();
            let resolved_root = configured_native_library_root(settings);

            if let Some(root) = resolved_root {
                info!("[source library] using on-disk root {}", root.display());
                lunco_assets_core::library::install_global_library_sources(vec![
                    LibraryAssetSource::Filesystem(root.clone()),
                ]);
                app.insert_resource(LibraryLoadState::Ready {
                    file_count: lunco_assets_core::library::filesystem_library_file_count(),
                    compressed_bytes: 0,
                    uncompressed_bytes: 0,
                });
            } else {
                info!(
                    "[source library] no on-disk root — source-library provisioning is not composed"
                );
                app.insert_resource(LibraryLoadState::NotStarted);
            }

            // NO startup warm. Filling the parsed slot here would flip
            // `drive_source_bundle_bootstrap` onto its eager branch on every native launch,
            // costing a measured 1645 ms main-thread stall on a scene with no
            // Modelica in it. `parsed_source_bundle` loads lazily at first lookup.
        }

        // Web: create the dormant fetch slot. The Settings menu inserts an
        // explicit request; until then no network task is started.
        #[cfg(target_arch = "wasm32")]
        {
            let slot: SharedSlot = Arc::new(Mutex::new(SlotInner::default()));
            let _ = WEB_LIBRARY_SLOT.set(slot.clone());
            app.insert_resource(LibraryLoadState::NotStarted);
            app.insert_resource(LibraryLoadSlot(slot));
            app.init_resource::<WebLibraryInstallRequest>();
            app.add_systems(
                Update,
                (
                    kick_web_library_fetcher,
                    drain_library_load_slot,
                    drive_library_main_decode,
                    drive_web_library_index,
                )
                    .chain(),
            );
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn configured_native_library_root(
    settings: &crate::settings::LibrarySettings,
) -> Option<std::path::PathBuf> {
    // A settings-level override wins: the user explicitly selected a local
    // Modelica checkout or system installation.
    let override_root = settings.local_root_override.as_ref().and_then(|path| {
        if path.is_dir() {
            Some(path.clone())
        } else {
            warn!(
                "[source-library] settings.library.local_root_override = {} is not a directory; ignoring",
                path.display()
            );
            None
        }
    });

    override_root.or_else(source_library_root_path)
}

// ─── Shared slot the wasm fetcher writes into ───────────────────────

#[cfg(target_arch = "wasm32")]
type SharedSlot = Arc<Mutex<SlotInner>>;

#[cfg(target_arch = "wasm32")]
static WEB_LIBRARY_SLOT: OnceLock<SharedSlot> = OnceLock::new();

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SlotInner {
    /// Latest state the fetcher has reported. The drain system replaces
    /// the world's `LibraryLoadState` whenever this `take`s out a new value.
    pending_state: Option<LibraryLoadState>,
    /// Raw **compressed** `parsed-*.bin.zst` bytes. Decompressed/decoded off
    /// the boot future: shipped to the worker + chunk-decoded on main. This is
    /// the fast path when the manifest advertises a pre-parsed bundle.
    pending_parsed_compressed: Option<Vec<u8>>,
    /// Raw **compressed** `sources-*.tar.zst` bytes + their manifest entry,
    /// stashed for lazy unpack on first editor drill-in.
    pending_source_compressed: Option<(Vec<u8>, lunco_assets_core::library::LibraryBundleEntry)>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
struct LibraryLoadSlot(SharedSlot);

#[cfg(target_arch = "wasm32")]
fn drain_library_load_slot(
    slot: Res<LibraryLoadSlot>,
    mut state: ResMut<LibraryLoadState>,
    worker_bridge: Res<ModelicaWorkerBridge>,
) {
    let mut inner = match slot.0.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some(new_state) = inner.pending_state.take() {
        // Log only on phase transitions / terminal states; progress
        // updates within the same phase would spam the console.
        match (&*state, &new_state) {
            (
                LibraryLoadState::Loading { phase: a, .. },
                LibraryLoadState::Loading { phase: b, .. },
            ) if a == b => {}
            _ => log_library_state_transition(&new_state),
        }
        *state = new_state;
    }
    // Fast boot path: a compressed parsed bundle is waiting. Ship it to the
    // worker for decompression/deserialization and transfer the decoded bytes
    // back for chunked main-thread deserialization (resolution/autocomplete).
    // Stash the compressed source for lazy drill-in unpack. `LibraryLoadState`
    // stays `Loading{Parsing}` until `drive_library_main_decode` finishes.
    if let Some(pbytes) = inner.pending_parsed_compressed.take() {
        // Ship the compressed bundle to the off-thread worker(s). The worker
        // decompresses + deserializes for its own compiles, then transfers the
        // decoded bincode bytes back so the main thread skips the ruzstd
        // decompress and only deserializes into its own heap (resolution /
        // autocomplete) — see `ingest_worker_decoded_library`.
        let shipped = worker_bridge.install_library_compressed(&pbytes);
        if shipped == 0 {
            let error =
                "Modelica Web Worker is unavailable; rebuild the browser worker bundle".to_string();
            worker_bridge.fail_pipeline(error.clone());
            inner.pending_state = Some(LibraryLoadState::Failed(error));
            inner.pending_source_compressed = None;
            return;
        }
        if let Some((sbytes, smeta)) = inner.pending_source_compressed.take() {
            worker_bridge.load_library_index(&sbytes);
            stash_compressed_source(sbytes, smeta);
        }
        return;
    }
}

/// Log a source-library state transition without duplicating status formatting
/// in the native and web lifecycle adapters.
pub fn log_library_state_transition(s: &LibraryLoadState) {
    match s {
        LibraryLoadState::NotStarted => {}
        LibraryLoadState::Loading {
            phase,
            bytes_done,
            bytes_total,
        } => {
            if *bytes_total > 0 {
                bevy::log::info!(
                    "[source library] {} ({:.1}/{:.1} MB)",
                    phase.as_str(),
                    *bytes_done as f64 / 1_048_576.0,
                    *bytes_total as f64 / 1_048_576.0,
                );
            } else {
                bevy::log::info!("[source library] {}", phase.as_str());
            }
        }
        LibraryLoadState::Ready {
            file_count,
            compressed_bytes,
            uncompressed_bytes,
        } => {
            bevy::log::info!(
                "[source library] ready — {file_count} files ({:.1} MB compressed → {:.1} MB)",
                *compressed_bytes as f64 / 1_048_576.0,
                *uncompressed_bytes as f64 / 1_048_576.0,
            );
        }
        LibraryLoadState::Failed(msg) => {
            bevy::log::error!("[source library] failed: {msg}");
        }
    }
}

// The source library-state → status-bus mirror belongs to the UI adapter.
// This package owns `LibraryLoadState` + `LibraryLoadPhase`.

// ─── Web fetcher implementation ─────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use std::collections::HashSet;

    use lunco_assets_core::library::LibraryManifest;
    use lunco_assets_core::web_fetch;
    use wasm_bindgen::prelude::*;

    pub(super) async fn run_fetcher(
        slot: SharedSlot,
        settings: lunco_settings::DownloadSettings,
        worker_bridge: ModelicaWorkerBridge,
    ) {
        match try_fetch(&slot, &settings).await {
            Ok(()) => {}
            Err(e) => {
                worker_bridge.fail_pipeline(e.clone());
                if let Ok(mut s) = slot.lock() {
                    s.pending_state = Some(LibraryLoadState::Failed(e));
                }
            }
        }
    }

    fn set_state(slot: &SharedSlot, state: LibraryLoadState) {
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
            LibraryLoadState::Loading {
                phase: LibraryLoadPhase::FetchingManifest,
                bytes_done: 0,
                bytes_total: 0,
            },
        );

        let manifest_path = source_library_asset_path(SOURCE_LIBRARY_MANIFEST_FILE_NAME)?;
        let manifest_bytes =
            web_fetch::fetch_bytes_revalidated(CACHE_NAME, &manifest_path, settings).await?;
        let manifest: LibraryManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| format!("{SOURCE_LIBRARY_MANIFEST_FILE_NAME} parse: {e}"))?;
        if manifest.schema_version != 1 {
            return Err(format!(
                "unsupported manifest schema_version {}",
                manifest.schema_version
            ));
        }

        // ── Sources blob (small, always shipped). Used by the editor for
        // ── opening source library files after the runtime artifact is installed.
        let bundle_path = source_library_asset_path(&manifest.sources.filename)?;
        let phase1 = bundle_fetch_phase(&bundle_path).await;
        // Per-blob progress: this download sweeps 0..its own size, so the bar
        // Each blob reports progress over its own byte range, so the bar
        // advances continuously through both downloads.
        // `total` comes from the fetcher — Content-Length, else the expected
        // size passed below (a Cache-Storage hit often omits Content-Length).
        let sources_total = manifest.sources.compressed_bytes;
        let progress_slot1 = slot.clone();
        let progress_cb1 = Closure::<dyn FnMut(f64, f64)>::new(move |done: f64, total: f64| {
            if let Ok(mut s) = progress_slot1.lock() {
                s.pending_state = Some(LibraryLoadState::Loading {
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

        if manifest.rumoca_artifact_tag != lunco_assets_core::library::EXPECTED_RUMOCA_ARTIFACT_TAG
        {
            return Err(format!(
                "source library parsed artifact tag `{}` does not match runtime `{}`; rebuild the source library bundle",
                manifest.rumoca_artifact_tag,
                lunco_assets_core::library::EXPECTED_RUMOCA_ARTIFACT_TAG,
            ));
        }
        let parsed_meta = &manifest.parsed;
        let parsed_path = source_library_asset_path(&parsed_meta.filename)?;
        let phase2 = bundle_fetch_phase(&parsed_path).await;
        // Per-blob again: the (larger) parsed bundle sweeps 0..its own size.
        let parsed_total = parsed_meta.compressed_bytes;
        let progress_slot2 = slot.clone();
        let progress_cb2 = Closure::<dyn FnMut(f64, f64)>::new(move |done: f64, total: f64| {
            if let Ok(mut s) = progress_slot2.lock() {
                s.pending_state = Some(LibraryLoadState::Loading {
                    phase: phase2,
                    bytes_done: done as u64,
                    bytes_total: total as u64,
                });
            }
        });

        let parsed_bytes = web_fetch::fetch_cached_with_progress(
            CACHE_NAME,
            &parsed_path,
            parsed_total,
            progress_cb2.as_ref().unchecked_ref(),
            settings,
        )
        .await
        .map_err(|e| format!("parsed bundle fetch: {e}"))?;
        if parsed_bytes.len() as u64 != parsed_meta.compressed_bytes {
            return Err(format!(
                "parsed bundle size {} != manifest {}",
                parsed_bytes.len(),
                parsed_meta.compressed_bytes
            ));
        }

        // The current blobs are now cached. Evict superseded content-hashed
        // bundles so the
        // browser cache doesn't grow without bound. Best-effort — never fails
        // the load.
        {
            // Filenames the current manifest references; everything else in the
            // source library bucket is a superseded release and gets evicted.
            let mut keep = HashSet::new();
            // The cache pruner compares final URL components, while fetch
            // paths include the source-library prefix.
            keep.insert(SOURCE_LIBRARY_MANIFEST_FILE_NAME.to_string());
            keep.insert(manifest.sources.filename.clone());
            keep.insert(manifest.parsed.filename.clone());
            web_fetch::prune_cache(CACHE_NAME, &keep).await;
        }

        // Hand the COMPRESSED blobs off WITHOUT decoding them here — this runs
        // on the main-thread event loop, so decompress/untar/decode would
        // freeze the page. The parsed artifact is decoded by the worker and
        // the source bundle remains compressed until the editor opens a file.
        set_state(
            slot,
            LibraryLoadState::Loading {
                phase: LibraryLoadPhase::Parsing,
                bytes_done: 0,
                bytes_total: manifest.parsed.file_count as u64,
            },
        );
        if let Ok(mut s) = slot.lock() {
            s.pending_parsed_compressed = Some(parsed_bytes);
            s.pending_source_compressed = Some((sources_bytes, manifest.sources.clone()));
        }

        Ok(())
    }

    const CACHE_NAME: &str = "lunco-library-v1";

    /// The progress phase to show while fetching `path`: a cache hit loads
    /// locally (no network), so report [`LoadingCache`](LibraryLoadPhase::LoadingCache)
    /// instead of [`FetchingBundle`](LibraryLoadPhase::FetchingBundle) ("downloading").
    async fn bundle_fetch_phase(path: &str) -> LibraryLoadPhase {
        if web_fetch::cache_has(CACHE_NAME, path).await {
            LibraryLoadPhase::LoadingCache
        } else {
            LibraryLoadPhase::FetchingBundle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_library_artifact_paths_are_single_filenames() {
        assert_eq!(
            source_library_asset_path("sources-a.tar.zst").as_deref(),
            Ok("library/sources-a.tar.zst")
        );
        assert!(source_library_asset_path("../outside.tar.zst").is_err());
        assert!(source_library_asset_path("nested/bundle.tar.zst").is_err());
        assert!(source_library_asset_path("C:\\bundle.tar.zst").is_err());
    }
}
