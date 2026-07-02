//! The lunica product tour, as **data**.
//!
//! Lunica has no scripting runtime, so its guided tour is registered as a
//! `Vec<TourStepDef>` in the shared [`lunco_workbench::tour_driver::TourCatalog`]
//! and played by the shared coach-card driver — the *same* renderer the sandbox
//! drives from rhai. One tour engine, two apps (spec 011 §6 migration).
//!
//! This module now only contributes the lunica-specific pieces the generic
//! driver can't know about:
//! - the ten tour steps themselves (titles, bodies, `HelpAnchors` keys, and the
//!   panel each step auto-focuses);
//! - a Help-menu "🎓 Show Tour" entry (publishes a [`HelpTourRequest`]);
//! - opening the `CascadedRCFilter` demo model while the tour runs, so the
//!   model-view steps (Text / Diagram / Icon, Compilation modes, Graphs) have
//!   real content to spotlight. Closed again when the tour ends.
//!
//! First-start auto-open and step navigation live in the shared driver
//! (persisted under `settings.json` → `tour_seen`), so this file no longer owns
//! a "seen" flag, a renderer, or an advance state machine.
//!
//! (`CascadedRCFilter` has no MSL dependency, so it loads fast — unlike
//! `AnnotatedRocketStage`, which pulls in the Modelica Standard Library and
//! stalls the tour on first launch.)

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_workbench::tour_driver::{ActiveTour, TourCatalog, TourDef, TourStepDef};
use lunco_workbench::{HelpTourRequest, PerspectiveId, WorkbenchLayout};

use crate::state::ModelicaDocumentRegistry;

/// The perspective the lunica tour belongs to. Its help popup declares
/// `has_tour`, so its "Show Tour" button publishes a [`HelpTourRequest`] for
/// this id, which the shared driver maps to the tour registered below.
const TOUR_PERSPECTIVE: PerspectiveId = PerspectiveId("modelica_analyze");

/// Demo-doc lifecycle state for the tour. The generic driver spotlights widgets
/// but can't know lunica needs a model open to *have* those widgets — so we open
/// `CascadedRCFilter` when the tour starts and close it when it ends. These are
/// tick-staged flags so the exclusive doc-manage system runs independently of
/// the tour driver.
#[derive(Resource, Default)]
pub struct HelpOverlayState {
    /// "Open the demo on the next frame."
    wants_open_doc: bool,
    /// "Capture the resulting active doc id the frame after."
    wants_capture_doc: bool,
    /// "Close the demo we opened."
    wants_close_doc: bool,
    /// The bundled demo we opened for the tour's duration (`None` if the user
    /// already had it open — then the tour must not take ownership of it).
    tutorial_doc: Option<DocumentId>,
}

pub struct HelpOverlayPlugin;

impl Plugin for HelpOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HelpOverlayState>();
        app.add_systems(Startup, (register_help_entry, register_lunica_tour));
        app.add_systems(Update, (sync_demo_doc_with_tour, manage_tutorial_doc));
    }
}

/// One tour step. `focus` is a singleton `PanelId` the driver opens on entry.
fn step(anchor: &str, title: &str, body: &str, focus: Option<&str>) -> TourStepDef {
    TourStepDef {
        anchor: anchor.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        focus_panel: focus.map(str::to_string),
    }
}

/// Register the ten-step lunica tour as data. `first_start: true` so it
/// auto-opens once on a fresh install (the shared driver tracks "seen").
fn register_lunica_tour(mut catalog: ResMut<TourCatalog>) {
    catalog.register(
        TOUR_PERSPECTIVE,
        TourDef {
            first_start: true,
            steps: vec![
                step(
                    "",
                    "Welcome to Lunica",
                    "Lunica (LunCoSim Modelica Workbench) — a quick interactive tour. \
                     Use ◀ ▶ or the arrow keys. Esc to skip.",
                    None,
                ),
                step(
                    "panel.lunco_twin_browser",
                    "Twin Browser",
                    "Your workspace lives here. A Twin is a folder of .mo files + a \
                     manifest. Bundled demos and the Modelica Standard Library are \
                     read-only — duplicate to edit.",
                    Some("lunco_twin_browser"),
                ),
                step(
                    "model_view.view_toggles",
                    "Text · Diagram · Icon",
                    "Four live views of the same model: 📝 Text source, 🔗 Diagram \
                     (wired components), 🎨 Icon (the class's own annotation), and \
                     📖 Docs.",
                    None,
                ),
                step(
                    "model_view.compile_buttons",
                    "Compilation modes",
                    "Two ways to compile and run a model:\n\
                     • 🚀 Interactive — compile then step in real time; live \
                       sliders drive inputs, signals stream into Graphs.\n\
                     • ⏩ Fast Run — batch, headless; integrate 0 → t_end \
                       then dump results to plot. Best for sweeps and \
                       reproducibility.",
                    None,
                ),
                step(
                    "panel.modelica_plot",
                    "Graphs",
                    "Multi-axis plots, per document. Pick signals, snapshot a run \
                     to compare against future ones. Lives in the bottom dock by \
                     default — next to Experiments, Diagnostics, Console, Journal.",
                    None,
                ),
                step(
                    "panel.modelica_plot",
                    "Rearrange tabs",
                    "Every tab is draggable. Drop one next to another to split \
                     the area; drop it on a tab strip to add it as a sibling. \
                     Great for watching signals next to the model while it runs.",
                    None,
                ),
                step(
                    "panel.modelica_experiments",
                    "Experiments",
                    "Override any parameter without editing source. Fast Run does a \
                     one-shot batch; scheduled runs sweep override lists.",
                    Some("modelica_experiments"),
                ),
                step(
                    "panel.modelica_inspector",
                    "Telemetry",
                    "Live parameters, inputs, and variable plot toggles. In \
                     Interactive mode, sliders here drive inputs at simulation \
                     rate.",
                    Some("modelica_inspector"),
                ),
                step(
                    "menu.help",
                    "Scripting — HTTP API & MCP",
                    "Every UI action has a typed Command. POST to /api/commands, or \
                     drive the workbench from an LLM agent over MCP.",
                    None,
                ),
                step(
                    "",
                    "You're set",
                    "Reopen this tour any time from Help → Show Tour, or press F1.",
                    None,
                ),
            ],
        },
    );
}

/// Add a Help-menu "Show Tour" entry. Publishes a [`HelpTourRequest`] for the
/// tour's perspective; the shared driver starts the registered tour.
fn register_help_entry(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_help_menu(|ui, world, _layout| {
        if ui
            .button("🎓 Show Tour")
            .on_hover_text("Replay the guided interactive tour of the workbench")
            .clicked()
        {
            world.resource_mut::<HelpTourRequest>().0 = Some(TOUR_PERSPECTIVE);
            ui.close();
        }
    });
}

/// Open the demo model when the lunica tour starts and close it when it ends,
/// by watching the shared [`ActiveTour`] for our perspective. Drives the flags
/// that [`manage_tutorial_doc`] acts on.
fn sync_demo_doc_with_tour(
    active: Res<ActiveTour>,
    mut state: ResMut<HelpOverlayState>,
    mut was_active: Local<bool>,
) {
    let now = active.id == Some(TOUR_PERSPECTIVE);
    if now && !*was_active {
        if state.tutorial_doc.is_none() {
            state.wants_open_doc = true;
        }
    } else if !now && *was_active {
        state.wants_close_doc = true;
    }
    *was_active = now;
}

/// Exclusive system: opens `CascadedRCFilter` when the tour starts, captures the
/// resulting active doc id, and closes that doc when the tour ends. Runs as
/// `&mut World` because `open_class` and the close intent both need it.
fn manage_tutorial_doc(world: &mut World) {
    // Step 1 — fire the open. Next-frame capture flag is set so we pick up the
    // new active doc once the open-observer has run.
    let wants_open = world.resource::<HelpOverlayState>().wants_open_doc;
    if wants_open {
        // Only the tour-opened doc may be torn down on finish. If the user
        // already has the demo model open (a restored session, or a manual
        // open), the tour must NOT take ownership of it — otherwise closing the
        // tour would close the user's document. In that case leave
        // `tutorial_doc` None and capture disarmed, so the teardown in Step 3 is
        // a no-op and the user's doc stays put.
        let already_open = world
            .get_resource::<ModelicaDocumentRegistry>()
            .and_then(|reg| reg.find_bundled("CascadedRCFilter.mo"))
            .is_some();
        if already_open {
            let mut state = world.resource_mut::<HelpOverlayState>();
            state.wants_open_doc = false;
            state.wants_capture_doc = false;
            state.tutorial_doc = None;
            return;
        }
        crate::ui::panels::package_browser::open_class(
            world,
            crate::class_ref::ClassRef::bundled(["CascadedRCFilter"]),
            false,
        );
        let mut state = world.resource_mut::<HelpOverlayState>();
        state.wants_open_doc = false;
        state.wants_capture_doc = true;
        return;
    }

    // Step 2 — capture. Reads whatever active_document the open observer landed
    // on. The open may resolve a frame or more late (the bundled source still
    // has to parse), so keep the capture flag armed until a doc actually shows
    // up — otherwise a fast skip leaves `tutorial_doc` None and the demo leaks.
    let wants_capture = world.resource::<HelpOverlayState>().wants_capture_doc;
    if wants_capture {
        let active = world
            .resource::<lunco_workspace::WorkspaceResource>()
            .active_document;
        let mut state = world.resource_mut::<HelpOverlayState>();
        if active.is_some() {
            state.tutorial_doc = active;
            state.wants_capture_doc = false;
        }
    }

    // Step 3 — close. Triggers `EditorIntent::Close` while the tour doc is
    // active; the doc's own observer handles tab teardown.
    let wants_close = world.resource::<HelpOverlayState>().wants_close_doc;
    if wants_close {
        // Prefer the captured id; if capture never landed (fast skip before the
        // open resolved), fall back to whatever the tour left active so the demo
        // is still torn down.
        let (captured, pending) = {
            let s = world.resource::<HelpOverlayState>();
            (s.tutorial_doc, s.wants_capture_doc)
        };
        let doc = captured.or_else(|| {
            if pending {
                world
                    .resource::<lunco_workspace::WorkspaceResource>()
                    .active_document
            } else {
                None
            }
        });
        if let Some(doc) = doc {
            // Make the tour doc active so `EditorIntent::Close` (which closes the
            // active document) targets the right one.
            world
                .resource_mut::<lunco_workspace::WorkspaceResource>()
                .active_document = Some(doc);
            world.trigger(lunco_doc_bevy::EditorIntent::Close);
        }
        let mut state = world.resource_mut::<HelpOverlayState>();
        state.tutorial_doc = None;
        state.wants_close_doc = false;
        // Disarm capture too — a late open resolving after teardown must not
        // re-adopt a doc the tour no longer owns.
        state.wants_capture_doc = false;
    }
}
