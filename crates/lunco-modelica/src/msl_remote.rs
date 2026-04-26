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
static GLOBAL_PARSED_MSL: OnceLock<Arc<Vec<(String, rumoca_session::parsing::StoredDefinition)>>> =
    OnceLock::new();

/// Read the pre-parsed MSL bundle if any has been installed.
pub fn global_parsed_msl() -> Option<&'static Arc<Vec<(String, rumoca_session::parsing::StoredDefinition)>>> {
    GLOBAL_PARSED_MSL.get()
}

fn install_global_parsed_msl(docs: Vec<(String, rumoca_session::parsing::StoredDefinition)>) {
    let _ = GLOBAL_PARSED_MSL.set(Arc::new(docs));
}

/// Plugin that owns MSL asset loading. Add once during app build.
pub struct MslRemotePlugin;

impl Plugin for MslRemotePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MslLoadState>();

        // Native: synchronous decision based on whether the disk tree is
        // there. We never need an async task on this target.
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(root) = lunco_assets::msl_source_root_path() {
                let count = count_mo_files(&root);
                info!("[MSL] using on-disk root {} ({count} .mo files)", root.display());
                app.insert_resource(MslAssetSource::Filesystem(root));
                app.insert_resource(MslLoadState::Ready {
                    file_count: count,
                    compressed_bytes: 0,
                    uncompressed_bytes: 0,
                });
            } else {
                warn!("[MSL] no on-disk root found — workbench will run without MSL");
                app.insert_resource(MslLoadState::Failed(
                    "MSL not present on disk; run `lunco-assets -- download` first".into(),
                ));
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
            // The drain runs first to hand bundle → parse, then the
            // parse driver runs in the same frame so chunks start
            // immediately without an extra tick of latency. The DOM
            // mirror runs after both so it always reflects the latest
            // state in the same frame the user can see.
            app.add_systems(
                Update,
                (drain_msl_load_slot, drive_msl_parse, mirror_state_to_dom).chain(),
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
    parsed: Vec<(String, rumoca_session::parsing::StoredDefinition)>,
    total: usize,
    started: web_time::Instant,
    last_log: web_time::Instant,
}

#[cfg(target_arch = "wasm32")]
const PARSE_CHUNK_SIZE: usize = 1;

#[cfg(target_arch = "wasm32")]
const PARSE_LOG_INTERVAL_SECS: u64 = 10;

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
    pending_parsed: Option<Vec<(String, rumoca_session::parsing::StoredDefinition)>>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
struct MslLoadSlot(SharedSlot);

#[cfg(target_arch = "wasm32")]
fn drain_msl_load_slot(
    slot: Res<MslLoadSlot>,
    mut state: ResMut<MslLoadState>,
    mut worker: ResMut<crate::InlineWorker>,
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
    mut worker: ResMut<crate::InlineWorker>,
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
        install_global_parsed_msl(parsed);
        // Drop the compiler that was lazily built before MSL was ready
        // (or before parse finished); next compile reinstates with the
        // pre-parsed bundle.
        worker.reset_compiler();
        commands.remove_resource::<MslParseInProgress>();
    }
}

#[cfg(target_arch = "wasm32")]
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

// ─── DOM status mirror ─────────────────────────────────────────────

/// Mirror the current `MslLoadState` into the `#status-bar` strip at
/// the bottom of `index.html`. MSL is non-blocking — the workbench is
/// already usable by the time this runs — so the indicator is small
/// and peripheral. Hidden once MSL transitions to `Ready` (after a
/// brief "ready" pulse so the user sees completion).
///
/// We poke the DOM directly via web-sys here. No JS bridge: the
/// elements are stable, the strings are short, and avoiding the
/// bridge means one less thing that can drift between Rust and JS.
#[cfg(target_arch = "wasm32")]
fn mirror_state_to_dom(
    state: Res<MslLoadState>,
    mut last_summary: bevy::prelude::Local<Option<String>>,
) {
    use web_sys::window;

    // Cheap change-detection: a short summary string captures the
    // visible-state delta. Avoids touching the DOM when nothing moved.
    let summary = summarize_for_dom(&state);
    if last_summary.as_deref() == Some(summary.as_str()) {
        return;
    }
    *last_summary = Some(summary);

    let Some(doc) = window().and_then(|w| w.document()) else { return };
    let Some(bar) = doc.get_element_by_id("status-bar") else { return };

    match &*state {
        MslLoadState::NotStarted => {
            let _ = bar.set_attribute("class", "hidden");
        }
        MslLoadState::Loading { phase, bytes_done, bytes_total } => {
            let phase_label = match phase {
                MslLoadPhase::FetchingManifest => "fetching manifest",
                MslLoadPhase::FetchingBundle   => "downloading",
                MslLoadPhase::Decompressing    => "decompressing",
                MslLoadPhase::Parsing          => "loading",
            };
            let text = match phase {
                MslLoadPhase::Parsing => format!("{phase_label} {} / {}", bytes_done, bytes_total),
                _ if *bytes_total > 0 => format!(
                    "{phase_label} — {:.1} / {:.1} MB",
                    *bytes_done as f64 / 1_048_576.0,
                    *bytes_total as f64 / 1_048_576.0,
                ),
                _ => phase_label.to_string(),
            };
            let cls = if *bytes_total > 0 { "" } else { "indeterminate" };
            let _ = bar.set_attribute("class", cls);
            set_text(&doc, "#status-bar .text", &text);
            if *bytes_total > 0 {
                let pct = (*bytes_done as f64 / *bytes_total as f64 * 100.0).clamp(0.0, 100.0);
                set_fill(&doc, pct);
            }
        }
        MslLoadState::Ready { file_count, .. } => {
            let _ = bar.set_attribute("class", "ready");
            set_text(&doc, "#status-bar .text", &format!("ready — {file_count} files"));
            // Hide after a beat so the user sees the green ready dot.
            schedule_status_bar_hide(1500);
        }
        MslLoadState::Failed(msg) => {
            let _ = bar.set_attribute("class", "error");
            set_text(&doc, "#status-bar .text", &format!("failed: {msg}"));
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn summarize_for_dom(state: &MslLoadState) -> String {
    match state {
        MslLoadState::NotStarted => "n".into(),
        MslLoadState::Loading { phase, bytes_done, bytes_total } => {
            // Bucket into 1% steps so the bar doesn't redraw on every
            // single byte and we avoid DOM thrash.
            let pct_bucket = if *bytes_total > 0 {
                ((*bytes_done as f64 / *bytes_total as f64) * 100.0) as u32
            } else {
                0
            };
            format!("L:{phase:?}:{pct_bucket}")
        }
        MslLoadState::Ready { file_count, .. } => format!("R:{file_count}"),
        MslLoadState::Failed(m) => format!("F:{}", m.len()),
    }
}

#[cfg(target_arch = "wasm32")]
fn set_text(doc: &web_sys::Document, selector: &str, text: &str) {
    let Ok(Some(node)) = doc.query_selector(selector) else { return };
    node.set_text_content(Some(text));
}

#[cfg(target_arch = "wasm32")]
fn set_fill(doc: &web_sys::Document, pct: f64) {
    use wasm_bindgen::JsCast;
    let Ok(Some(node)) = doc.query_selector("#status-bar .bar > .fill") else { return };
    if let Ok(el) = node.dyn_into::<web_sys::HtmlElement>() {
        let _ = el.style().set_property("width", &format!("{pct:.1}%"));
    }
}

#[cfg(target_arch = "wasm32")]
fn schedule_status_bar_hide(delay_ms: i32) {
    use wasm_bindgen::JsCast;
    let Some(win) = web_sys::window() else { return };
    let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Some(el) = doc.get_element_by_id("status-bar") {
                let cur = el.get_attribute("class").unwrap_or_default();
                let new_cls = if cur.contains("hidden") {
                    cur
                } else {
                    format!("{cur} hidden").trim().to_string()
                };
                let _ = el.set_attribute("class", &new_cls);
            }
        }
    });
    let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
        cb.as_ref().unchecked_ref(),
        delay_ms,
    );
}

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
    ) -> Result<Vec<(String, rumoca_session::parsing::StoredDefinition)>, String> {
        let decoder = ruzstd::StreamingDecoder::new(compressed)
            .map_err(|e| format!("zstd decoder: {e}"))?;
        let docs: Vec<(String, rumoca_session::parsing::StoredDefinition)> =
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
