//! Read-only **source viewer** — the text view for asset files no domain
//! editor owns.
//!
//! The Scenarios menu lists every registered source file, but only `.usda`
//! (→ `LoadScene`) and `.mo` (→ Modelica's editor) have a destination. A
//! `.rhai` / `.btxml` / `.wgsl` clicked there used to dispatch `OpenFile` and
//! fall through silently — no observer claims those extensions. This panel is
//! that destination: a read-only text view that shows any such file's bytes.
//!
//! No syntax highlighting for v1 (an explicit non-goal); the point is that
//! every asset is at least *readable* in every workbench host.
//!
//! # Data flow
//!
//! ```text
//!   Scenarios menu click ──▶ OpenFile { path }  (rhai/btxml/wgsl)
//!      └─ on_open_file_for_text ─▶ PendingSourceReads (async Task)
//!                               └─▶ FocusPanel { "source_viewer" }
//!   drain_pending_source_reads (Update)
//!      └─▶ SourceViewerState.loaded { path, text }
//!   SourceViewerPanel::render ─▶ reads SourceViewerState, paints the text
//! ```
//!
//! Native reads run off-thread. Shipped web assets use the asynchronous
//! same-origin asset fetcher, because browser `localStorage` is not the deployed
//! asset tree. USD and Modelica keep their own `OpenFile` observers — this one
//! ignores every path it does not own, the documented `OpenFile` contract.

use std::{path::PathBuf, sync::Arc};

use crate::{FocusPanel, OpenSourceView, Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot};
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use bevy_egui::egui;
use lunco_core::on_command;
use lunco_doc_bevy::OpenFile;

/// Extensions this viewer claims — the text sources no domain `OpenFile`
/// observer already owns. USD has `lunco-usd`'s observer; Modelica has
/// `lunco-modelica`'s. A new text source type added here gets a read-only view
/// for free; give it a real editor when one exists.
const TEXT_VIEW_EXTS: &[&str] = &["rhai", "btxml", "wgsl"];

pub(crate) struct SourceViewerPanel;

impl Panel for SourceViewerPanel {
    fn id(&self) -> PanelId {
        PanelId("source_viewer")
    }
    fn title(&self) -> String {
        "📄 Source".into()
    }
    /// Show the open file's name in the tab, falling back to the static title
    /// when nothing is loaded — so an empty tab reads "Source", a loaded one
    /// reads "rover_autopilot.rhai".
    fn dynamic_title(&self, world: &World) -> String {
        world
            .get_resource::<SourceViewerState>()
            .and_then(|s| s.loaded.as_ref())
            .and_then(|l| {
                l.path
                    .file_name()
                    .map(|n| format!("📄 {}", n.to_string_lossy()))
            })
            .unwrap_or_else(|| self.title())
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }
    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Scene
    }
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        source_viewer_content(ui, ctx);
    }
}

fn source_viewer_content(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    // Clone the data we need out of the resource so the borrow ends before any
    // `ctx.defer` / further resource access below.
    let loaded = ctx
        .resource::<SourceViewerState>()
        .and_then(|s| s.loaded.as_ref())
        .map(|s| (s.path.clone(), Arc::clone(&s.text)));
    let pending = ctx
        .resource::<SourceViewerState>()
        .map(|s| s.request.is_some())
        .unwrap_or(false);
    let error = ctx
        .resource::<SourceViewerState>()
        .and_then(|state| state.error.clone());

    match (pending, loaded, error) {
        (true, _, _) => {
            ui.label(egui::RichText::new("Loading…").weak().italics());
        }
        (false, Some((path, text)), _) => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                    )
                    .strong(),
                );
                ui.label(egui::RichText::new("(read-only)").weak().small());
            });
            ui.separator();
            render_source_text(ui, &text);
        }
        (false, None, Some(error)) => {
            let color = ctx.resource::<lunco_theme::Theme>().map_or_else(
                || lunco_theme::Theme::default().tokens.error,
                |theme| theme.tokens.error,
            );
            ui.label(egui::RichText::new(error).color(color));
        }
        (false, None, None) => {
            ui.label(egui::RichText::new("No file open.").weak());
            ui.label(
                egui::RichText::new(
                    "Select a source file from the Library or Scenarios menu to view it.",
                )
                .weak()
                .small(),
            );
        }
    }
}

/// Monospace, selectable, non-editable source text.
///
/// A [`Label`](egui::Label) borrows the retained source, so the idle render path
/// does not clone the whole file every frame as a disabled [`TextEdit`](egui::TextEdit)
/// would.
fn render_source_text(ui: &mut egui::Ui, source: &str) {
    egui::ScrollArea::both()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(source).monospace())
                    .selectable(true)
                    .extend(),
            );
        });
}

// ─────────────────────────────────────────────────────────────────────
// State + async load
// ─────────────────────────────────────────────────────────────────────

/// The viewer's state. `request` is set the instant a read is kicked off (so
/// the panel can show "Loading…" before the bytes land); `loaded` is the
/// result once the off-thread read completes.
#[derive(Resource, Default)]
pub(crate) struct SourceViewerState {
    /// Monotonic id assigned to the next read request.
    next_request: u64,
    /// Path currently being read, if a read is in flight.
    pub request: Option<(u64, PathBuf)>,
    /// Last successfully read source, if any.
    pub loaded: Option<LoadedSource>,
    /// Last read error, if any.
    error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct LoadedSource {
    pub path: PathBuf,
    pub text: Arc<str>,
}

/// A read kicked off by [`on_open_file_for_text`], polled to completion by
/// [`drain_pending_source_reads`]. Mirrors `PendingUsdLoads` in `lunco-usd`.
#[derive(Resource, Default)]
pub(crate) struct PendingSourceReads {
    tasks: Vec<PendingSourceRead>,
}

struct PendingSourceRead {
    request: u64,
    path: PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    task: Task<Result<String, String>>,
    #[cfg(target_arch = "wasm32")]
    result: crossbeam_channel::Receiver<Result<String, String>>,
}

/// Observer for the workbench's typed [`OpenFile`] command. Claims the text
/// extensions no domain editor owns (`rhai`/`btxml`/`wgsl`) and routes them
/// into the source viewer; ignores everything else so USD/Modelica keep their
/// own observers. See `lunco-doc-bevy::OpenFile` for the coexistence contract.
///
/// The LunCo Library browser uses [`crate::OpenSourceView`] instead
/// (see [`on_open_source_view`]) because the library opens *every* file as
/// text, including `.usda`/`.mo` that this observer deliberately leaves to
/// their domain editors.
#[on_command(OpenFile)]
pub(crate) fn on_open_file_for_text(trigger: On<OpenFile>, mut commands: Commands) {
    let path = trigger.event().path.clone();
    commands.queue(move |world: &mut World| {
        let Some(ext) = std::path::Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
        else {
            return;
        };
        if !TEXT_VIEW_EXTS.contains(&ext) {
            return;
        }
        view_as_text(world, &path);
    });
}

/// Observer for [`OpenSourceView`] — fired by the LunCo Library browser
/// section. Unlike [`on_open_file_for_text`] this claims NO extension: the
/// library is a browse-and-read surface, so every file opens as text.
/// `OpenSourceView` exists separately from `OpenFile` precisely so this "open
/// as text for all" intent does not collide with USD's/Modelica's `OpenFile`
/// observers (which would otherwise also fire and double-open `.usda`/`.mo`
/// into their native editors).
#[on_command(OpenSourceView)]
pub(crate) fn on_open_source_view(trigger: On<OpenSourceView>, mut commands: Commands) {
    let asset_path = trigger.event().asset_path.clone();
    commands.queue(move |world: &mut World| {
        let asset = {
            let Some(manifest) = world.get_resource::<lunco_assets::discovery::AssetManifest>()
            else {
                return;
            };
            let Some(roots) = world.get_resource::<lunco_assets::twin_source::TwinRoots>() else {
                return;
            };
            lunco_assets::discovery::list_all_assets(manifest, roots)
                .into_iter()
                .find(|asset| asset.asset_path == asset_path)
        };
        let Some(asset) = asset else {
            bevy::log::warn!("[SourceViewer] rejected unregistered asset path: {asset_path}");
            return;
        };
        kick_off_asset_read(world, asset);
        world.trigger(FocusPanel {
            id: "source_viewer".into(),
        });
    });
}

/// Shared body of both observers: kick off the off-thread read and bring the
/// source viewer panel to the front. Rejects scheme-prefixed paths that have no
/// filesystem backing (`mem://`).
fn view_as_text(world: &mut World, path: &str) {
    if path.starts_with("mem://") {
        return;
    }
    kick_off_read(world, PathBuf::from(path));
    // Bring the panel to the front so the click is answered visibly — without
    // this the file loads into a panel the user may not have open.
    world.trigger(FocusPanel {
        id: "source_viewer".into(),
    });
}

/// Spawn the off-thread read for `path` and queue it for polling. Reading
/// through `lunco-storage` (not `std::fs`) keeps the clippy ban honest and
/// reaches the wasm storage backend; `read_file_sync` is the synchronous
/// convenience wrapper that delegates to `FileStorage` on native.
fn kick_off_read(world: &mut World, path: PathBuf) {
    let request = begin_request(world, path.clone());
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path_for_task = path.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            match lunco_storage::read_file_sync(&path_for_task) {
                Ok(bytes) => String::from_utf8(bytes)
                    .map_err(|e| format!("{}: not valid UTF-8 ({e})", path_for_task.display())),
                Err(e) => Err(format!("failed to read {}: {e:?}", path_for_task.display())),
            }
        });
        world.resource_mut::<PendingSourceReads>().tasks = vec![PendingSourceRead {
            request,
            path,
            task,
        }];
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (tx, result) = crossbeam_channel::bounded(1);
        let path_for_task = path.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let read = lunco_storage::read_file_sync(&path_for_task)
                .map_err(|e| format!("failed to read {}: {e:?}", path_for_task.display()))
                .and_then(|bytes| {
                    String::from_utf8(bytes)
                        .map_err(|e| format!("{}: not valid UTF-8 ({e})", path_for_task.display()))
                });
            let _ = tx.send(read);
        });
        world.resource_mut::<PendingSourceReads>().tasks = vec![PendingSourceRead {
            request,
            path,
            result,
        }];
    }
}

/// Read a catalogue-resolved asset through the platform's asset byte path:
/// native storage on desktop and same-origin fetch on wasm.
fn kick_off_asset_read(world: &mut World, asset: lunco_assets::discovery::AssetFile) {
    let path = asset.abs_path.clone();
    let request = begin_request(world, path.clone());
    #[cfg(not(target_arch = "wasm32"))]
    {
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { lunco_assets::asset_read::read_asset_text(&asset).await });
        world.resource_mut::<PendingSourceReads>().tasks = vec![PendingSourceRead {
            request,
            path,
            task,
        }];
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (tx, result) = crossbeam_channel::bounded(1);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(lunco_assets::asset_read::read_asset_text(&asset).await);
        });
        world.resource_mut::<PendingSourceReads>().tasks = vec![PendingSourceRead {
            request,
            path,
            result,
        }];
    }
}

fn begin_request(world: &mut World, path: PathBuf) -> u64 {
    let mut state = world.resource_mut::<SourceViewerState>();
    state.next_request = state.next_request.wrapping_add(1);
    let request = state.next_request;
    state.request = Some((request, path));
    state.loaded = None;
    state.error = None;
    request
}

/// Publish a completed read only when it is still the most recent request.
///
/// Superseded tasks are dropped when a new request starts; the request check is
/// an additional race guard so an already-completing read cannot replace newer
/// content.
fn publish_completed_read(
    state: &mut SourceViewerState,
    request: u64,
    path: PathBuf,
    text: String,
) -> bool {
    if !state
        .request
        .as_ref()
        .is_some_and(|(active, _)| *active == request)
    {
        return false;
    }
    state.loaded = Some(LoadedSource {
        path,
        text: Arc::from(text),
    });
    state.request = None;
    state.error = None;
    true
}

/// Poll outstanding [`PendingSourceReads`] each frame and land each completed
/// read into [`SourceViewerState::loaded`]. A completed or errored read clears
/// the matching `request` so the panel stops saying "Loading…". Mirrors
/// `drain_pending_usd_file_loads`.
pub(crate) fn drain_pending_source_reads(world: &mut World) {
    if world.resource::<PendingSourceReads>().tasks.is_empty() {
        return;
    }
    let taken = std::mem::take(&mut world.resource_mut::<PendingSourceReads>().tasks);
    let mut still_pending: Vec<PendingSourceRead> = Vec::new();
    for mut read in taken {
        #[cfg(not(target_arch = "wasm32"))]
        let result = block_on(future::poll_once(&mut read.task));
        #[cfg(target_arch = "wasm32")]
        let result = read.result.try_recv().ok();
        match result {
            None => still_pending.push(read),
            Some(Err(err)) => {
                bevy::log::warn!("[SourceViewer] {err}");
                let mut state = world.resource_mut::<SourceViewerState>();
                if state
                    .request
                    .as_ref()
                    .is_some_and(|(active, _)| *active == read.request)
                {
                    state.request = None;
                    state.loaded = None;
                    state.error = Some(err);
                }
            }
            Some(Ok(text)) => {
                publish_completed_read(
                    &mut world.resource_mut::<SourceViewerState>(),
                    read.request,
                    read.path,
                    text,
                );
            }
        }
    }
    world.resource_mut::<PendingSourceReads>().tasks = still_pending;
}

// Registered directly by `WorkbenchPlugin`: this UI-owned observer must not be
// installed by a headless composition root.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_read_cannot_replace_the_latest_source() {
        let mut state = SourceViewerState {
            next_request: 2,
            request: Some((2, PathBuf::from("new.rhai"))),
            loaded: None,
            error: None,
        };
        assert!(!publish_completed_read(
            &mut state,
            1,
            PathBuf::from("old.rhai"),
            "old".into(),
        ));
        assert!(state.loaded.is_none());
        assert!(publish_completed_read(
            &mut state,
            2,
            PathBuf::from("new.rhai"),
            "new".into(),
        ));
        assert_eq!(state.loaded.as_ref().map(|s| s.text.as_ref()), Some("new"));
    }
}
