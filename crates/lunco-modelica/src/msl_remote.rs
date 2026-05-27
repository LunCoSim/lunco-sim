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

/// Read the pre-parsed MSL bundle if any has been installed.
pub fn global_parsed_msl() -> Option<&'static Arc<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>> {
    GLOBAL_PARSED_MSL.get()
}

/// Publish a freshly parsed MSL bundle to the process-wide slot. Only
/// the first install wins; subsequent calls are silently ignored
/// (the `OnceLock` guarantees a stable handle for the lifetime of
/// the page session).
#[cfg(target_arch = "wasm32")]
fn install_global_parsed_msl(parsed: Vec<(String, rumoca_compile::parsing::StoredDefinition)>) {
    let _ = GLOBAL_PARSED_MSL.set(Arc::new(parsed));
}

/// `pub` re-export of `install_global_parsed_msl` so the off-thread
/// worker bin (`bin/lunica_worker.rs`) can install the MSL bundle it
/// receives over postMessage.
#[cfg(target_arch = "wasm32")]
pub fn install_global_parsed_msl_pub(parsed: Vec<(String, rumoca_compile::parsing::StoredDefinition)>) {
    install_global_parsed_msl(parsed);
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

        // Cross-platform: mirror MSL state changes into the workbench
        // status bus so renderers (status bar, console, diagnostics)
        // pick them up uniformly. Lives in this plugin (not the bus
        // crate) because it knows about `MslLoadState` shape.
        app.add_systems(Update, mirror_state_to_status_bus);

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
                let source = MslAssetSource::Filesystem(root);
                lunco_assets::msl::install_global_msl_source(source.clone());
                app.insert_resource(source);
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
        }

        // Web: kick off the async fetcher and have a system promote the
        // shared `Mutex` slot into a Bevy resource once the task completes.
        #[cfg(target_arch = "wasm32")]
        {
            let slot: SharedSlot = Arc::new(Mutex::new(SlotInner::default()));
            app.insert_resource(MslLoadState::Loading {
                phase: MslLoadPhase::FetchingManifest,
                bytes_done: 0,
                bytes_total: 0,
            });
            app.insert_resource(MslLoadSlot(slot.clone()));
            // Order:
            //  1. drain_msl_load_slot — bundle → parse handoff
            //  2. drive_msl_parse — chunked parse if needed
            //  3. mirror_state_to_status_bus (added cross-platform above)
            // The cross-platform mirror runs after the drain so it picks
            // up state changes within the same frame; egui's status bar
            // (rendered later by the workbench) reads StatusBus directly.
            app.add_systems(
                Update,
                (drain_msl_load_slot, drive_msl_parse).chain(),
            );
            wasm_bindgen_futures::spawn_local(web::run_fetcher(slot));
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
        let manifest = match AssetManifest::from_str(BUNDLED_ASSETS_TOML) {
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
            lunco_assets::download::download_asset_with_control(entry, "msl", control)
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
    world.remove_resource::<MslAssetSource>();
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
    mut commands: Commands,
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
        lunco_assets::msl::install_global_msl_source(source.clone());
        commands.insert_resource(source);
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

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SlotInner {
    /// Latest state the fetcher has reported. The drain system replaces
    /// the world's `MslLoadState` whenever this `take`s out a new value.
    pending_state: Option<MslLoadState>,
    /// The fetched + decompressed in-memory tree, handed off once.
    pending_source: Option<MslAssetSource>,
    /// Pre-parsed `Vec<(uri, StoredDefinition)>` if the manifest had a
    /// `parsed` entry. When `Some`, the drain skips the chunked-source
    /// parse path and installs directly into `GLOBAL_PARSED_MSL`.
    pending_parsed: Option<Vec<(String, rumoca_compile::parsing::StoredDefinition)>>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
struct MslLoadSlot(SharedSlot);

#[cfg(target_arch = "wasm32")]
fn drain_msl_load_slot(
    slot: Res<MslLoadSlot>,
    mut state: ResMut<MslLoadState>,
    mut worker: ResMut<crate::worker::InlineWorker>,
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
    if let Some(source) = inner.pending_source.take() {
        // Install the process-wide handle that `ModelicaCompiler::new`
        // consults; do this before inserting the resource so the next
        // compile attempt picks it up regardless of system ordering.
        lunco_assets::msl::install_global_msl_source(source.clone());

        // Fast path: pre-parsed bundle was shipped and decoded. Install
        // directly. The next time the user hits Compile, the inline
        // worker will lazy-build a fresh `ModelicaCompiler` that picks
        // up `GLOBAL_PARSED_MSL` and the model resolves cleanly.
        if let Some(parsed_docs) = inner.pending_parsed.take() {
            let count = parsed_docs.len();
            bevy::log::info!(
                "[MSL] using pre-parsed bundle — {count} docs ready (no on-page parse)"
            );
            // Forward to the off-thread worker (if installed) BEFORE
            // moving `parsed_docs` into the main address space's
            // `GLOBAL_PARSED_MSL`. The worker has its own wasm linear
            // memory and OnceLock; without this hand-off, every compile
            // dispatched to the worker would fail to resolve any
            // `Modelica.*` reference.
            crate::worker_transport::install_msl_in_worker(&parsed_docs);
            install_global_parsed_msl(parsed_docs);
            commands.insert_resource(source);
            // Drop any previously-cached empty compiler so the next
            // user-initiated compile rebuilds with MSL.
            worker.reset_compiler();
            return;
        }

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
        commands.insert_resource(source);
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

// ─── State → StatusBus mirror (cross-platform) ─────────────────────

/// Watch [`MslLoadState`] and translate transitions / progress ticks
/// into [`StatusBus`] events. Phase changes become discrete `Info`
/// entries (preserved in history); byte/file counts within a phase
/// become `Progress` ticks (updated in place).
///
/// This system uses the *legacy* `push_progress`/`clear_progress`
/// API rather than `begin` + `BusyHandle` because it is a state
/// mirror, not a task owner — there is no scope-bound future to
/// carry the handle. `MslLoadState` itself is the lifetime
/// authority. Legacy progress events implicitly target
/// [`BusyScope::Global`], which matches the actual semantics
/// (MSL preload affects the whole workspace). Don't refactor this
/// to `begin` without first introducing a place to store the
/// handle that drops in lockstep with state transitions.
fn mirror_state_to_status_bus(
    state: Res<MslLoadState>,
    bus: Option<ResMut<lunco_workbench::status_bus::StatusBus>>,
    mut last: bevy::prelude::Local<Option<MirrorMemo>>,
) {
    let Some(mut bus) = bus else {
        return;
    };
    let now_summary = MirrorMemo::from(&*state);
    let prior_phase_label = last.as_ref().and_then(|m| m.phase_label);

    match &*state {
        MslLoadState::NotStarted => {}
        MslLoadState::Loading { phase, bytes_done, bytes_total } => {
            let label = msl_phase_label(*phase);
            // Phase transition → discrete history entry.
            if prior_phase_label != Some(label) {
                bus.push(
                    MSL_SOURCE,
                    lunco_workbench::status_bus::StatusLevel::Info,
                    label,
                );
            }
            // Progress tick (in-place; doesn't accumulate in history).
            let detail = format_progress_detail(*phase, *bytes_done, *bytes_total);
            bus.push_progress(MSL_SOURCE, detail, *bytes_done, *bytes_total);
        }
        MslLoadState::Ready { file_count, .. } => {
            // Only fire once per Ready transition (re-renders shouldn't spam).
            if !matches!(last.as_ref(), Some(MirrorMemo { ready: true, .. })) {
                bus.push(
                    MSL_SOURCE,
                    lunco_workbench::status_bus::StatusLevel::Info,
                    format!("ready — {file_count} files"),
                );
                bus.clear_progress(MSL_SOURCE);
            }
        }
        MslLoadState::Failed(msg) => {
            if !matches!(last.as_ref(), Some(MirrorMemo { failed: true, .. })) {
                bus.push(
                    MSL_SOURCE,
                    lunco_workbench::status_bus::StatusLevel::Error,
                    msg.clone(),
                );
                bus.clear_progress(MSL_SOURCE);
            }
        }
    }

    *last = Some(now_summary);
}

const MSL_SOURCE: &str = "MSL";

fn msl_phase_label(p: MslLoadPhase) -> &'static str {
    match p {
        MslLoadPhase::FetchingManifest => "fetching manifest",
        MslLoadPhase::FetchingBundle   => "downloading",
        MslLoadPhase::Decompressing    => "decompressing",
        MslLoadPhase::Parsing          => "loading",
    }
}

fn format_progress_detail(phase: MslLoadPhase, done: u64, total: u64) -> String {
    let label = msl_phase_label(phase);
    match phase {
        MslLoadPhase::Parsing if total > 0 => format!("{label} {done} / {total}"),
        _ if total > 0 => format!(
            "{label} — {:.1} / {:.1} MB",
            done as f64 / 1_048_576.0,
            total as f64 / 1_048_576.0,
        ),
        _ => label.to_string(),
    }
}

#[derive(Default)]
struct MirrorMemo {
    phase_label: Option<&'static str>,
    ready: bool,
    failed: bool,
}

impl From<&MslLoadState> for MirrorMemo {
    fn from(s: &MslLoadState) -> Self {
        match s {
            MslLoadState::NotStarted => Self::default(),
            MslLoadState::Loading { phase, .. } => Self {
                phase_label: Some(msl_phase_label(*phase)),
                ..Self::default()
            },
            MslLoadState::Ready { .. } => Self { ready: true, ..Self::default() },
            MslLoadState::Failed(_) => Self { failed: true, ..Self::default() },
        }
    }
}

// (Status bar UI lives in lunco-workbench's egui status panel; this
// module just publishes events to the bus via mirror_state_to_status_bus.)

// ─── Web fetcher implementation ─────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use std::collections::HashMap;
    use std::io::Read;
    use std::path::PathBuf;

    use lunco_assets::msl::{MslBundleEntry, MslInMemory, MslManifest};
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, RequestInit, RequestMode, Response};

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

        let manifest_bytes = fetch_bytes("msl/manifest.json").await?;
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
        set_state(
            slot,
            MslLoadState::Loading {
                phase: MslLoadPhase::FetchingBundle,
                bytes_done: 0,
                bytes_total: manifest.sources.compressed_bytes
                    + manifest
                        .parsed
                        .as_ref()
                        .map(|p| p.compressed_bytes)
                        .unwrap_or(0),
            },
        );
        let sources_bytes = fetch_bytes(&bundle_path).await?;
        if sources_bytes.len() as u64 != manifest.sources.compressed_bytes {
            return Err(format!(
                "sources bundle size {} != manifest {}",
                sources_bytes.len(),
                manifest.sources.compressed_bytes
            ));
        }

        // ── Pre-parsed bundle (when the manifest advertises one). This
        // ── is the fast path: bincode-decode → install directly into
        // ── rumoca, no per-file parse.
        let parsed_bytes = if let Some(parsed_meta) = manifest.parsed.as_ref() {
            let parsed_path = format!("msl/{}", parsed_meta.filename);
            // Update progress to reflect that we've finished sources
            // and are now downloading the parsed bundle on top.
            set_state(
                slot,
                MslLoadState::Loading {
                    phase: MslLoadPhase::FetchingBundle,
                    bytes_done: manifest.sources.compressed_bytes,
                    bytes_total: manifest.sources.compressed_bytes
                        + parsed_meta.compressed_bytes,
                },
            );
            let bytes = fetch_bytes(&parsed_path).await?;
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

        // ── Decompress the source bundle (always needed — keep around
        // ── for editor / image loader / fallback parse path).
        set_state(
            slot,
            MslLoadState::Loading {
                phase: MslLoadPhase::Decompressing,
                bytes_done: 0,
                bytes_total: manifest.sources.uncompressed_bytes,
            },
        );
        let files = unpack(&sources_bytes, &manifest.sources)?;
        if !files.contains_key(std::path::Path::new(&manifest.msl_root_marker)) {
            return Err(format!(
                "bundle missing root marker `{}`",
                manifest.msl_root_marker
            ));
        }
        let in_memory = MslInMemory { files };
        let source_file_count = in_memory.file_count();
        let source_uncompressed = in_memory.total_bytes();

        // ── Decode the parsed bundle if we got one.
        let parsed_docs = if let Some(bytes) = parsed_bytes {
            match decode_parsed(&bytes) {
                Ok(docs) => Some(docs),
                Err(e) => {
                    bevy::log::warn!(
                        "[MSL] parsed bundle present but decode failed: {e}; \
                         falling back to chunked source parse"
                    );
                    None
                }
            }
        } else {
            None
        };

        if let Ok(mut s) = slot.lock() {
            s.pending_source = Some(MslAssetSource::InMemory(Arc::new(in_memory)));
            s.pending_parsed = parsed_docs;
            s.pending_state = Some(MslLoadState::Ready {
                file_count: source_file_count,
                compressed_bytes: manifest.sources.compressed_bytes,
                uncompressed_bytes: source_uncompressed,
            });
        }

        Ok(())
    }

    /// Decode the bincode-serialised `Vec<(uri, StoredDefinition)>`
    /// shipped as the `parsed` blob.
    fn decode_parsed(
        compressed: &[u8],
    ) -> Result<Vec<(String, rumoca_compile::parsing::StoredDefinition)>, String> {
        let decoder = ruzstd::StreamingDecoder::new(compressed)
            .map_err(|e| format!("zstd decoder: {e}"))?;
        let docs: Vec<(String, rumoca_compile::parsing::StoredDefinition)> =
            bincode::deserialize_from(decoder)
                .map_err(|e| format!("bincode deserialize: {e}"))?;
        Ok(docs)
    }

    /// Unpack a `tar.zst` byte slice into `(rel_path → contents)`.
    fn unpack(
        bundle: &[u8],
        entry_meta: &MslBundleEntry,
    ) -> Result<HashMap<PathBuf, Vec<u8>>, String> {
        let decoder = ruzstd::StreamingDecoder::new(bundle)
            .map_err(|e| format!("zstd decoder: {e}"))?;
        let mut archive = tar::Archive::new(decoder);
        let mut out: HashMap<PathBuf, Vec<u8>> = HashMap::with_capacity(entry_meta.file_count);
        for entry in archive
            .entries()
            .map_err(|e| format!("tar entries: {e}"))?
        {
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

    async fn fetch_bytes(path: &str) -> Result<Vec<u8>, String> {
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
        let bytes = js_sys::Uint8Array::new(&array_buffer).to_vec();
        Ok(bytes)
    }
}
