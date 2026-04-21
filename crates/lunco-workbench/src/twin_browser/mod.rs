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
//! - The legacy `package_browser.rs` panel keeps running side-by-side
//!   until the Modelica section reaches feature parity, at which point
//!   the old panel is removed in one cutover commit.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::panel::{Panel, PanelId, PanelSlot};

pub mod files_section;

pub use files_section::FilesSection;

/// Stable id of the Twin Browser singleton panel.
pub const TWIN_BROWSER_PANEL_ID: PanelId = PanelId("lunco.workbench.twin_browser");

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
    /// Path is relative to the open Twin's root.
    OpenFile {
        /// Twin-root-relative path of the file the user picked.
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
}

// ─────────────────────────────────────────────────────────────────────
// BrowserSection trait + render context
// ─────────────────────────────────────────────────────────────────────

/// Read-side context passed to a section's `render`.
///
/// Holds the open Twin (if any), the action outbox, and the full
/// `&mut World`. Sections that just need Twin + actions ignore the
/// world ref; sections that surface domain state (open documents,
/// selection, drill-in target) read it from their own resources via
/// `world`.
///
/// `world` is mutable for the duration of one section's render so
/// sections can run light queries without separate system plumbing.
/// They MUST NOT remove or replace `BrowserSectionRegistry`,
/// `BrowserActions`, or `WorkspaceResource` — those are extracted by
/// the panel for the duration of the render and inserting them here
/// would break the take-and-restore protocol.
pub struct BrowserCtx<'a> {
    /// The currently-active Twin, or `None` if no Twin is open. Today
    /// the browser is single-twin oriented; multi-twin rendering
    /// (per-Twin collapsing groups) is a follow-up once the Workspace
    /// habit of holding several is actually exercised by the UI.
    pub twin: Option<&'a lunco_twin::Twin>,
    /// Outbox the section pushes user actions into.
    pub actions: &'a mut BrowserActions,
    /// Full ECS world for sections that need domain resources.
    pub world: &'a mut bevy::prelude::World,
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

    /// Whether the section is open by default the first time it's
    /// rendered. egui persists the open/closed state per session
    /// after that.
    fn default_open(&self) -> bool {
        true
    }

    /// Paint the section body.
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx);
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
        "Twin Browser".to_string()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Take registry + actions + WorkspaceResource out of the world
        // so sections can borrow `world` mutably during render without
        // conflicting with the active-Twin read. Restored after.
        let Some(mut registry) = world.remove_resource::<BrowserSectionRegistry>() else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "BrowserSectionRegistry resource missing",
            );
            return;
        };
        let mut actions = world
            .remove_resource::<BrowserActions>()
            .unwrap_or_default();
        let workspace = world.remove_resource::<crate::WorkspaceResource>();

        if registry.sections.is_empty() {
            ui.label(
                egui::RichText::new("No browser sections registered")
                    .weak()
                    .italics(),
            );
        } else {
            for section in &mut registry.sections {
                let header = egui::CollapsingHeader::new(section.title())
                    .id_salt(("twin_browser_section", section.id()))
                    .default_open(section.default_open());
                header.show(ui, |ui| {
                    let twin_ref = workspace
                        .as_ref()
                        .and_then(|ws| ws.active_twin.and_then(|id| ws.twin(id)));
                    let mut ctx = BrowserCtx {
                        twin: twin_ref,
                        actions: &mut actions,
                        world,
                    };
                    section.render(ui, &mut ctx);
                });
            }
        }

        if let Some(w) = workspace {
            world.insert_resource(w);
        }
        world.insert_resource(actions);
        world.insert_resource(registry);
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
        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut BrowserCtx<'_>) {
            self.render_count
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
}
