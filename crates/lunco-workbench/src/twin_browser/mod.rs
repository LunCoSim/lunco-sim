//! Twin Browser — the side panel that surfaces the open Twin's
//! contents through pluggable per-domain sections.
//!
//! ## What this is
//!
//! A single workbench panel ([`TwinBrowserPanel`]) that renders a
//! stack of collapsible *sections*. Each section is contributed by a
//! domain plugin via the [`BrowserSection`] trait and stored in the
//! [`BrowserSectionRegistry`] resource. The workbench itself ships
//! exactly one built-in section — [`files_section::FilesSection`] —
//! because file listing is domain-agnostic.
//!
//! ## Why a registry instead of a hard-coded list
//!
//! `lunco-workbench` cannot depend on the domain crates without
//! inverting the dependency graph (`lunco-modelica` already depends on
//! us). So domain crates push their section impls into the registry
//! at plugin-build time:
//!
//! ```ignore
//! // somewhere in ModelicaPlugin::build
//! app.world_mut()
//!    .resource_mut::<BrowserSectionRegistry>()
//!    .register(ModelicaSection::default());
//! ```
//!
//! The browser walks the registry each frame, asking each section to
//! render itself into a collapsing header. Section-emitted actions
//! (clicks, drags, context-menu choices) flow back through the
//! [`BrowserCtx::actions`] outbox so domain crates can install
//! observers that turn them into concrete app behaviour (open a tab,
//! drill into a class, reveal in editor, …).
//!
//! ## What's *not* here
//!
//! - The Modelica-specific class-tree section — that ships in
//!   `lunco-modelica` as `ModelicaSection`, registered by its plugin.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::panel::{Panel, PanelCtx, PanelId, PanelSlot};

pub mod files_section;
/// LunCo Library section — the engine's bundled `assets/` listed as a
/// sibling above the Twin's own files. Names only; click opens as text.
pub mod library_section;
mod path_tree;

pub use files_section::FilesSection;
pub use library_section::LuncoLibrarySection;

/// Stable id of the Twin Browser singleton panel.
pub const TWIN_BROWSER_PANEL_ID: PanelId = PanelId("lunco.workbench.twin_browser");

/// Shared transient filter for the Twin and Files browser panels.
///
/// The workbench owns the query because it is presentation state, while each
/// domain section decides which of its authoritative rows match. Keeping the
/// query here means a search survives switching between the Twin and Files
/// tabs without introducing a second per-domain browser model.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowserQuery {
    /// Text entered in the browser filter. Empty or whitespace-only means no
    /// filtering.
    pub text: String,
}

impl BrowserQuery {
    /// Whether this query hides non-matching rows.
    pub fn is_active(&self) -> bool {
        !self.text.trim().is_empty()
    }

    /// Case-insensitive substring match against a display label or path.
    pub fn matches(&self, value: &str) -> bool {
        let needle = self.text.trim();
        needle.is_empty() || value.to_lowercase().contains(&needle.to_lowercase())
    }
}

/// Paint the common browser search control and queue the changed query as a
/// panel intent. The panel never mutates [`BrowserQuery`] during egui paint.
pub fn render_search_bar(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    let mut query = ctx.resource::<BrowserQuery>().cloned().unwrap_or_default();
    let original = query.text.clone();

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Find").strong());
        ui.add(
            egui::TextEdit::singleline(&mut query.text)
                .hint_text("names, paths, or types")
                .desired_width(160.0),
        );
        if !query.text.trim().is_empty()
            && crate::icon_button(ui, crate::UiIcon::Close, "Clear browser filter").clicked()
        {
            query.text.clear();
        }
    });

    if query.text != original {
        ctx.set_resource(query);
    }
}

// ─────────────────────────────────────────────────────────────────────
// Browser scope (Models / Files tab toggle)
// ─────────────────────────────────────────────────────────────────────

/// One of the top-level lenses the Twin Browser presents.
///
/// A Twin is composed of multiple modeling modalities (Modelica
/// dynamics, USD scenes, SysML architecture, future Julia, …) plus
/// raw on-disk content. The user wants quick toggle between
/// *"what's modeled in this Twin"* and *"what's on disk"* — this enum
/// names the two lenses.
///
/// Each [`BrowserSection`] declares which scope it belongs to via
/// [`BrowserSection::scope`]. The panel renders only the sections
/// matching the active scope (see `ActiveBrowserScope`).
///
/// More scopes may join later (a Subsystems / Usages tab once the
/// cosim graph view exists, surfacing a Twin's `[scenarios.*]`
/// instance graph). The enum is `#[non_exhaustive]` so the matching
/// dispatch in the panel doesn't lock us out of additions.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BrowserScope {
    /// Sections rendered inside the Twin panel. Today: Modelica
    /// workspace, MSL standard library, bundled examples. Future:
    /// USD scenes, SysML packages, Julia modules, pinned externals.
    /// One panel hosts everything you'd browse "by name" — matches
    /// Dymola/OMEdit's single-Package-Browser pattern where MSL and
    /// user packages share one tree.
    Models,
    /// Sections rendered inside the Files panel. Raw on-disk content
    /// of the active Twin (or open Folder) — folder layout, file
    /// references, anything regardless of typed-Document status.
    Files,
}

impl BrowserScope {
    /// Stable kebab-case label used as the egui id salt for sections
    /// (so collapsed/expanded state survives across recompiles).
    pub const fn id(self) -> &'static str {
        match self {
            BrowserScope::Models => "models",
            BrowserScope::Files => "files",
        }
    }

    /// Human label — used in empty-state hints and developer logs.
    pub const fn label(self) -> &'static str {
        match self {
            BrowserScope::Models => "Twin",
            BrowserScope::Files => "Files",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Resources: section registry + action outbox
// ─────────────────────────────────────────────────────────────────────
//
// The currently-open Twins live in `WorkspaceResource`
// (`lunco-workspace`). This panel reads the active Twin from there
// each render; there is no panel-local "open twin" resource anymore.

/// One open workspace document — saved-to-disk OR untitled
/// in-memory. Surfaced in the Files section so the workspace
/// view stays stable across Save (a Save shouldn't make the
/// document vanish from the list). Populated by domain plugins
/// (e.g. lunco-modelica scans its document registry).
#[derive(Debug, Clone)]
pub struct UnsavedDocEntry {
    /// Document identity. Used by the Files section to dispatch
    /// per-doc actions (inline rename, close, save) keyed on the
    /// row the user interacted with.
    pub id: lunco_doc::DocumentId,
    /// Display name (file stem for saved docs, "Untitled-3.mo"
    /// for unsaved drafts).
    pub display_name: String,
    /// Domain hint shown as a small badge ("Modelica", "USD"…).
    pub kind: String,
    /// True when the doc has never been written to disk in this
    /// session — drives the dirty-dot prefix in the Files
    /// section. False once the doc has a writable file path
    /// bound (post-Save / opened from disk).
    pub is_unsaved: bool,
}

/// Cross-domain list of workspace documents (saved + unsaved).
/// Domain plugins **overwrite** this resource when their registry
/// changes; the Files section reads it to render the workspace
/// list with per-row dirty markers.
#[derive(Resource, Default, Debug, Clone)]
pub struct UnsavedDocs {
    /// One entry per dirty document the Files section should mark.
    pub entries: Vec<UnsavedDocEntry>,
}

/// Registry of [`BrowserSection`] impls contributed by domain plugins.
///
/// Sections render in registration order. The built-in
/// [`FilesSection`] is registered first by [`crate::WorkbenchPlugin`]
/// so it always appears at the bottom of the stack (typically
/// collapsed); domain sections appear above it.
#[derive(Resource, Default)]
pub struct BrowserSectionRegistry {
    sections: Vec<Box<dyn BrowserSection>>,
}

impl BrowserSectionRegistry {
    /// Append a section to the registry.
    pub fn register<S: BrowserSection + 'static>(&mut self, section: S) {
        self.sections.push(Box::new(section));
    }

    /// Number of registered sections (used by tests, not callers).
    #[doc(hidden)]
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// True iff no sections are registered yet.
    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Read-only iterator over registered sections. Used by the
    /// scope-filter step before render (panels build a Vec of indices,
    /// then dispatch via [`section_mut`](Self::section_mut)).
    pub fn iter(&self) -> impl Iterator<Item = &dyn BrowserSection> {
        self.sections.iter().map(|b| &**b)
    }

    /// Mutable access to the section at `index`. Panels use this
    /// during render to call `section.render(ui, &mut ctx)` on the
    /// scope-matching subset they collected via [`iter`](Self::iter).
    /// Indices change only when a section is registered (append-only
    /// during plugin build), so caching them within one render frame
    /// is safe.
    pub fn section_mut(&mut self, index: usize) -> &mut dyn BrowserSection {
        &mut *self.sections[index]
    }

    /// Forward Twin lifecycle cleanup to every registered section.
    pub fn on_twin_closed(&mut self, event: &lunco_workspace::TwinClosed) {
        for section in &mut self.sections {
            section.on_twin_closed(event);
        }
    }
}

/// One thing a section asked the workbench to do.
///
/// Sections push these via [`BrowserCtx::actions`]; a host-side system
/// drains [`BrowserActions`] each frame and dispatches. Adding new
/// variants doesn't break section impls — match exhaustiveness lives
/// in the dispatcher only.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BrowserAction {
    /// User clicked a file row → open it in the appropriate editor.
    ///
    /// Relative to the open Twin's root, **or absolute** for a file outside it —
    /// a scene's reference closure reaches into the shipped asset library, so the
    /// Scene Files section emits already-resolved paths. Dispatchers anchor only
    /// the relative form.
    OpenFile {
        /// Twin-root-relative path of the file the user picked, or an absolute
        /// one when the emitter already resolved it.
        relative_path: std::path::PathBuf,
    },
    /// User clicked a class row → open / focus a diagram tab on it.
    /// `qualified_path` is the full Modelica path (e.g.
    /// `"AnnotatedRocketStage.RocketStage"`).
    OpenModelicaClass {
        /// Twin-root-relative path of the file the class lives in.
        relative_path: std::path::PathBuf,
        /// Fully-qualified Modelica path to the class.
        qualified_path: String,
    },
    /// User clicked a class belonging to a Modelica document that is
    /// *already loaded* in the host's document registry (in-memory
    /// drafts, files open from a previous session, anything not
    /// represented as a path in the open Twin). Carries the raw
    /// `DocumentId` so the dispatcher doesn't have to re-resolve a
    /// path.
    OpenLoadedClass {
        /// Raw [`lunco_doc::DocumentId`] of the document. The
        /// dispatcher knows how to focus a tab on it.
        doc_id: u64,
        /// Fully-qualified Modelica path to the class.
        qualified_path: String,
    },
    /// User clicked the close (✕) control on a workspace-document row
    /// in the Files section → close the document, all its tabs, and
    /// any backing state (on wasm this also clears the localStorage
    /// autosave entry, so an unwanted draft stops resurrecting on
    /// reload). Discards unsaved changes without a prompt — the row's
    /// dirty dot is the warning.
    CloseDoc {
        /// The document to close.
        doc: lunco_doc::DocumentId,
    },
    /// User selected a navigation entry that opens or foregrounds a
    /// registered singleton panel. The workbench owns the dock mutation so
    /// domain sections do not reach into layout state directly.
    OpenPanel {
        /// Stable panel id string of the panel to open.
        id: String,
    },
}

/// Frame-scoped outbox of actions emitted by sections during render.
/// A host-side system drains this after the render pass.
#[derive(Resource, Default)]
pub struct BrowserActions {
    queued: Vec<BrowserAction>,
}

impl BrowserActions {
    /// Append an action to the outbox.
    pub fn push(&mut self, action: BrowserAction) {
        self.queued.push(action);
    }

    /// Drain everything queued this frame. Domain dispatch systems
    /// call this once per frame and react to each variant.
    pub fn drain(&mut self) -> Vec<BrowserAction> {
        std::mem::take(&mut self.queued)
    }

    /// Take only the actions matching `predicate`, leaving the rest in
    /// the outbox for sibling dispatch systems to process the same
    /// frame. Used to partition `OpenFile` by file extension between
    /// domain crates (Modelica takes `.mo`, USD takes `.usda` / `.usd` / `.usdc`,
    /// …) without coupling the workbench to any domain's filetype list.
    pub fn take_where<F: Fn(&BrowserAction) -> bool>(
        &mut self,
        predicate: F,
    ) -> Vec<BrowserAction> {
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(self.queued.len());
        for action in std::mem::take(&mut self.queued) {
            if predicate(&action) {
                taken.push(action);
            } else {
                kept.push(action);
            }
        }
        self.queued = kept;
        taken
    }
}

// ─────────────────────────────────────────────────────────────────────
// BrowserSection trait + render context
// ─────────────────────────────────────────────────────────────────────

/// Read-side context passed to a section's `render`.
///
/// Holds the open Twin (if any), the action outbox, and the
/// capability-narrowed panel context. Sections read view-models and emit
/// typed intents through it.
/// They MUST NOT remove or replace `BrowserSectionRegistry`,
/// `BrowserActions`, or `WorkspaceResource` — those are extracted by
/// the panel for the duration of the render and inserting them here
/// would break the take-and-restore protocol.
pub struct BrowserCtx<'a, 'w> {
    /// Outbox the section pushes user actions into.
    pub actions: &'a mut BrowserActions,
    /// Capability-narrowed panel access (reads + typed intents).
    /// Sections read domain state via [`resource`](Self::resource) /
    /// [`get`](Self::get) (e.g. the active Twin from `WorkspaceResource`)
    /// and emit changes via [`trigger`](Self::trigger).
    panel: &'a mut PanelCtx<'w>,
}

impl<'a, 'w> BrowserCtx<'a, 'w> {
    /// Build a section render context from the action outbox and the
    /// narrowed [`PanelCtx`]. Used by sibling panels (e.g.
    /// [`crate::FilesPanel`]) that drive the section registry from
    /// outside this module, where the `panel` field isn't visible.
    pub fn new(actions: &'a mut BrowserActions, panel: &'a mut PanelCtx<'w>) -> Self {
        Self { actions, panel }
    }

    /// O(1) read of a resource (e.g. `WorkspaceResource` for the active
    /// Twin / twin list, or any domain view-model).
    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.panel.resource::<T>()
    }

    /// O(1) read of one entity's component.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.panel.get::<T>(entity)
    }

    /// Replace an existing browser view-model resource after the egui pass.
    pub fn set_resource<T: Resource<Mutability = bevy::ecs::component::Mutable>>(
        &mut self,
        value: T,
    ) {
        self.panel.set_resource(value);
    }

    /// Temporarily scope one resource without exposing the world itself.
    pub fn resource_scope<R: Resource, T>(
        &mut self,
        f: impl FnOnce(&mut BrowserCtx<'_, 'w>, &mut R) -> T,
    ) -> Option<T> {
        // The nested context shares the section action outbox and retains the
        // same capability boundary while the selected resource is removed.
        let actions = &mut *self.actions;
        self.panel.resource_scope(move |panel, resource| {
            let mut scoped = BrowserCtx { actions, panel };
            f(&mut scoped, resource)
        })
    }

    /// Emit an event after the egui pass.
    pub fn trigger<E: bevy::ecs::event::Event>(&mut self, event: E)
    where
        for<'t> <E as bevy::ecs::event::Event>::Trigger<'t>: Default,
    {
        self.panel.trigger(event);
    }
}

/// One pluggable section in the Twin Browser.
///
/// Implementors live in domain crates (Modelica, USD, …) and are
/// registered into [`BrowserSectionRegistry`] by their plugin's
/// `build`. The browser handles the collapsing-header chrome itself —
/// `render` only paints the inner contents.
pub trait BrowserSection: Send + Sync + 'static {
    /// Stable id (used as the egui collapsing-header id source).
    fn id(&self) -> &str;

    /// Title shown on the section's header bar.
    fn title(&self) -> &str;

    /// Which top-level scope this section belongs to. Determines
    /// whether the section appears in the Models tab or the Files
    /// tab (or both — sections may belong to one only by default).
    ///
    /// Default is [`BrowserScope::Models`] because new domain
    /// sections are virtually always type-catalog views; built-in
    /// `FilesSection` overrides this to [`BrowserScope::Files`].
    fn scope(&self) -> BrowserScope {
        BrowserScope::Models
    }

    /// Whether the section is open by default the first time it's
    /// rendered. egui persists the open/closed state per session
    /// after that.
    fn default_open(&self) -> bool {
        true
    }

    /// Render order within the panel — lower numbers render first.
    /// Conventional values: Modelica/domain catalogs ~100, Files ~200,
    /// Library/system ~300. Sections with equal `order` fall back to
    /// registration order. Avoids relying on plugin add order.
    fn order(&self) -> u32 {
        0
    }

    /// Clear transient row state when a Twin is closed.
    ///
    /// Sections normally keep only presentation caches, but an inline rename
    /// or selection can carry a path/document identity. The workbench calls
    /// this lifecycle hook so that state cannot attach itself to a replacement
    /// Twin with the same name or path.
    fn on_twin_closed(&mut self, _event: &lunco_workspace::TwinClosed) {}

    /// Paint the section body.
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>);
}

// ─────────────────────────────────────────────────────────────────────
// Panel impl
// ─────────────────────────────────────────────────────────────────────

/// The Twin Browser singleton panel. Renders every section in the
/// registry inside its own `egui::CollapsingHeader`.
#[derive(Default)]
pub struct TwinBrowserPanel;

impl Panel for TwinBrowserPanel {
    fn id(&self) -> PanelId {
        TWIN_BROWSER_PANEL_ID
    }

    fn title(&self) -> String {
        "Twin".to_string()
    }

    fn menu_group(&self) -> crate::PanelMenuGroup {
        crate::PanelMenuGroup::Design
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        render_search_bar(ui, ctx);
        ui.separator();
        // Scope the section registry + action outbox out of the world for
        // the duration of the render (the narrow, structural way to get
        // `&mut` to the registry's trait objects — no raw `&mut World`).
        // Sections read domain state (the active Twin via
        // `WorkspaceResource`, …) through `BrowserCtx::resource` and emit
        // changes via typed events/actions.
        let present = ctx.resource_scope::<BrowserSectionRegistry, _>(|ctx, registry| {
            // Precompute each section's `order()` in a single walk; sorting
            // by an `nth(i)` re-walk inside the comparator was O(n²)/frame.
            let orders: Vec<_> = registry.iter().map(|s| s.order()).collect();
            let mut visible: Vec<usize> = (0..orders.len()).collect();
            visible.sort_by_key(|&i| (orders[i], i));

            if visible.is_empty() {
                ui.label(
                    egui::RichText::new("No Twin sections registered.")
                        .weak()
                        .italics(),
                );
                return;
            }

            ctx.resource_scope::<BrowserActions, _>(|ctx, actions| {
                // ScrollArea wraps every section so a fully-expanded MSL
                // tree (~2500 entries) doesn't push later sections off
                // screen. `auto_shrink=[false; 2]` fills the panel rect.
                egui::ScrollArea::vertical()
                    .id_salt("twin_panel_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for &i in &visible {
                            let section = registry.section_mut(i);
                            let header = egui::CollapsingHeader::new(section.title())
                                .id_salt(("twin_panel_section", section.id()))
                                .default_open(section.default_open());
                            let resp = header.show(ui, |ui| {
                                let mut bctx = BrowserCtx {
                                    actions: &mut *actions,
                                    panel: &mut *ctx,
                                };
                                section.render(ui, &mut bctx);
                            });
                            resp.header_response
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                        }
                    });
            });
        });

        if present.is_none() {
            let error_color = ctx
                .resource::<lunco_theme::Theme>()
                .map(|t| t.tokens.error)
                .unwrap_or(egui::Color32::LIGHT_RED);
            ui.colored_label(error_color, "BrowserSectionRegistry resource missing");
        }
    }
}

/// Notify browser sections and clear the shared filter when the active Twin is
/// replaced. Non-active Twin closure must not disturb navigation in the Twin
/// the user is still working in.
pub(crate) fn clear_browser_state_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    query: Option<ResMut<BrowserQuery>>,
    registry: Option<ResMut<BrowserSectionRegistry>>,
) {
    let event = trigger.event();
    if event.was_active {
        if let Some(mut query) = query {
            query.text.clear();
        }
    }
    if let Some(mut registry) = registry {
        registry.on_twin_closed(event);
    }
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter {
        pub render_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl BrowserSection for Counter {
        fn id(&self) -> &str {
            "test.counter"
        }
        fn title(&self) -> &str {
            "Counter"
        }
        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut BrowserCtx<'_, '_>) {
            self.render_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct LifecycleProbe {
        closed_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl BrowserSection for LifecycleProbe {
        fn id(&self) -> &str {
            "test.lifecycle"
        }

        fn title(&self) -> &str {
            "Lifecycle"
        }

        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut BrowserCtx<'_, '_>) {}

        fn on_twin_closed(&mut self, _event: &lunco_workspace::TwinClosed) {
            self.closed_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn registry_appends_in_order() {
        let mut reg = BrowserSectionRegistry::default();
        assert!(reg.is_empty());
        reg.register(Counter {
            render_count: Default::default(),
        });
        reg.register(Counter {
            render_count: Default::default(),
        });
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn actions_drain_clears_outbox() {
        let mut a = BrowserActions::default();
        a.push(BrowserAction::OpenFile {
            relative_path: "x.mo".into(),
        });
        a.push(BrowserAction::OpenFile {
            relative_path: "y.mo".into(),
        });
        let drained = a.drain();
        assert_eq!(drained.len(), 2);
        assert!(a.drain().is_empty());
    }

    #[test]
    fn browser_query_is_case_insensitive_and_ignores_outer_whitespace() {
        let query = BrowserQuery {
            text: "  rocker  ".into(),
        };
        assert!(query.is_active());
        assert!(query.matches("Rocker-Bogie"));
        assert!(!query.matches("Lander"));
    }

    #[test]
    fn empty_browser_query_matches_everything() {
        let query = BrowserQuery::default();
        assert!(!query.is_active());
        assert!(query.matches("any authored path"));
    }

    #[test]
    fn active_twin_close_clears_query_and_notifies_sections() {
        let closed_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut app = App::new();
        app.insert_resource(BrowserQuery {
            text: "stale navigation".into(),
        });
        let mut registry = BrowserSectionRegistry::default();
        registry.register(LifecycleProbe {
            closed_count: closed_count.clone(),
        });
        app.insert_resource(registry);
        app.add_observer(clear_browser_state_on_twin_closed);

        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: lunco_workspace::TwinId::new(1),
            root: std::path::PathBuf::from("/outgoing"),
            was_active: true,
        });
        assert!(app.world().resource::<BrowserQuery>().text.is_empty());
        assert_eq!(closed_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        app.world_mut().resource_mut::<BrowserQuery>().text = "keep".into();
        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: lunco_workspace::TwinId::new(2),
            root: std::path::PathBuf::from("/other"),
            was_active: false,
        });
        assert_eq!(app.world().resource::<BrowserQuery>().text, "keep");
        assert_eq!(closed_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
