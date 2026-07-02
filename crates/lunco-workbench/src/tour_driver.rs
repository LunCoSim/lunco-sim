//! Data-driven guided-tour driver — runs coach-mark tours from Rust **data**,
//! with no scripting runtime.
//!
//! The sandbox drives the coach card in [`tutorial_overlay`](crate::tutorial_overlay)
//! from `.rhai` scripts (`coach(...)` / `end_tour()`), but the lunica Modelica
//! workbench has no scripting engine. This module fills that gap: an app
//! registers a tour as a `Vec<`[`TourStepDef`]`>` in the [`TourCatalog`], and the
//! driver plays it through the *same* coach card, advancing on the same
//! `cmd:TutorialNext` / `cmd:TutorialBack` / `cmd:TutorialSkip` bus events the
//! card's buttons already emit. One renderer, two apps, tours as data.
//!
//! ```ignore
//! // at app Startup:
//! catalog.register(PerspectiveId("modelica_analyze"), TourDef {
//!     first_start: true,
//!     steps: vec![
//!         TourStepDef { title: "Welcome".into(), body: "…".into(), ..default() },
//!         TourStepDef { anchor: "panel.twin_browser".into(), title: "Twin Browser".into(),
//!                       body: "…".into(), focus_panel: Some("twin_browser".into()) },
//!     ],
//! });
//! ```
//!
//! The driver:
//! - consumes a [`HelpTourRequest`](crate::HelpTourRequest) (the "🎓 Show Tour"
//!   button) and starts the matching tour;
//! - auto-launches a `first_start` tour once per install (persisted in
//!   [`TourSeen`]) so first-run users get onboarded;
//! - fires [`FocusPanel`](crate::FocusPanel) when a step names a panel, so the
//!   spotlit widget is actually on screen;
//! - handles ←/→/Esc while a tour runs, and F1 to (re)open the first-start tour.
//!
//! It writes [`TutorialHud::tour`](crate::tutorial_overlay::TutorialHud); the
//! coach card renderer (`draw_tour`) does the drawing. Headless-safe: the
//! resources and systems exist, only the draw is ui-gated.

use std::collections::HashMap;

use bevy::prelude::*;
use lunco_core::{TelemetryEvent, TelemetryValue};
use lunco_settings::AppSettingsExt;
use serde::{Deserialize, Serialize};

use crate::tutorial_overlay::{TourStep, TutorialHud};
use crate::{FocusPanel, HelpTourRequest, PerspectiveId};

/// One step of a data-driven tour. Mirrors the shape of a coach card step plus
/// an optional panel to auto-focus on entry (so the anchor it spotlights is
/// actually painted). Owned `String`s so tours can be built at runtime.
#[derive(Clone, Debug, Default)]
pub struct TourStepDef {
    /// [`HelpAnchors`](crate::HelpAnchors) key to spotlight; empty = centred card.
    pub anchor: String,
    /// Coach-card banner title.
    pub title: String,
    /// Coach-card body text (may contain `\n`).
    pub body: String,
    /// Singleton [`PanelId`](crate::PanelId) string to open when this step is
    /// shown (via [`FocusPanel`](crate::FocusPanel)). `None` = don't touch the
    /// layout.
    pub focus_panel: Option<String>,
}

/// A registered tour: its ordered steps plus whether it should auto-open on the
/// user's first launch.
#[derive(Clone, Debug, Default)]
pub struct TourDef {
    /// The steps, played in order.
    pub steps: Vec<TourStepDef>,
    /// Auto-open once on first start (gated by [`TourSeen`]).
    pub first_start: bool,
}

/// Catalog of data-driven tours, keyed by the [`PerspectiveId`] they belong to
/// (so the perspective help popup's "Show Tour" button, which publishes a
/// [`HelpTourRequest`] for its perspective, maps straight onto a tour).
#[derive(Resource, Default)]
pub struct TourCatalog {
    tours: HashMap<PerspectiveId, TourDef>,
}

impl TourCatalog {
    /// Register (or replace) the tour for a perspective.
    pub fn register(&mut self, id: PerspectiveId, def: TourDef) {
        self.tours.insert(id, def);
    }

    /// The tour for a perspective, if any.
    pub fn get(&self, id: PerspectiveId) -> Option<&TourDef> {
        self.tours.get(&id)
    }

    /// The first registered tour flagged `first_start` and not yet in `seen`.
    fn pending_first_start(&self, seen: &TourSeen) -> Option<PerspectiveId> {
        self.tours
            .iter()
            .find(|(id, def)| def.first_start && !seen.contains(id.as_str()))
            .map(|(id, _)| *id)
    }

    /// Any registered `first_start` tour (for the F1 re-open shortcut).
    fn any_first_start(&self) -> Option<PerspectiveId> {
        self.tours
            .iter()
            .find(|(_, def)| def.first_start)
            .map(|(id, _)| *id)
    }
}

/// The tour currently playing, if any. Public so apps can react to the tour's
/// lifecycle (e.g. lunica opens a demo document while its tour runs).
#[derive(Resource, Default)]
pub struct ActiveTour {
    /// The perspective whose tour is playing; `None` = no tour.
    pub id: Option<PerspectiveId>,
    /// 0-based index of the current step.
    pub cursor: usize,
}

impl ActiveTour {
    /// Whether a tour is currently playing.
    pub fn is_active(&self) -> bool {
        self.id.is_some()
    }
}

/// Persisted "already auto-shown" set, stored under the `"tour_seen"` key of
/// `settings.json`. Holds the `PerspectiveId` string of every first-start tour
/// that has auto-opened, so onboarding fires exactly once per install.
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct TourSeen {
    /// Perspective ids whose first-start tour has already auto-opened.
    pub seen: Vec<String>,
}

impl TourSeen {
    fn contains(&self, id: &str) -> bool {
        self.seen.iter().any(|s| s == id)
    }
}

impl lunco_settings::SettingsSection for TourSeen {
    const KEY: &'static str = "tour_seen";
}

// ── Playback helpers ────────────────────────────────────────────────────────

/// Push the current step onto the coach card and focus its panel, if any.
fn show_current_step(
    active: &ActiveTour,
    catalog: &TourCatalog,
    hud: &mut TutorialHud,
    commands: &mut Commands,
) {
    let Some(id) = active.id else { return };
    let Some(def) = catalog.get(id) else { return };
    let Some(step) = def.steps.get(active.cursor) else { return };
    hud.tour = Some(TourStep {
        index: active.cursor,
        total: def.steps.len(),
        anchor: step.anchor.clone(),
        title: step.title.clone(),
        body: step.body.clone(),
    });
    if let Some(panel) = &step.focus_panel {
        commands.trigger(FocusPanel { id: panel.clone() });
    }
}

/// Begin `id`'s tour at step 0 (no-op if it has no steps).
fn start_tour(
    id: PerspectiveId,
    catalog: &TourCatalog,
    active: &mut ActiveTour,
    hud: &mut TutorialHud,
    commands: &mut Commands,
) {
    if catalog.get(id).is_none_or(|d| d.steps.is_empty()) {
        return;
    }
    active.id = Some(id);
    active.cursor = 0;
    show_current_step(active, catalog, hud, commands);
}

/// Stop the tour and hide the coach card.
fn end_tour(active: &mut ActiveTour, hud: &mut TutorialHud) {
    active.id = None;
    active.cursor = 0;
    hud.tour = None;
}

/// Advance one step, or end the tour when past the last step.
fn next_step(
    catalog: &TourCatalog,
    active: &mut ActiveTour,
    hud: &mut TutorialHud,
    commands: &mut Commands,
) {
    let total = active
        .id
        .and_then(|id| catalog.get(id))
        .map_or(0, |d| d.steps.len());
    if active.cursor + 1 >= total {
        end_tour(active, hud);
    } else {
        active.cursor += 1;
        show_current_step(active, catalog, hud, commands);
    }
}

/// Jump directly to `target` (clamped into range) — the clickable progress dots.
fn goto_step(
    target: usize,
    catalog: &TourCatalog,
    active: &mut ActiveTour,
    hud: &mut TutorialHud,
    commands: &mut Commands,
) {
    let total = active
        .id
        .and_then(|id| catalog.get(id))
        .map_or(0, |d| d.steps.len());
    if total == 0 {
        return;
    }
    active.cursor = target.min(total - 1);
    show_current_step(active, catalog, hud, commands);
}

/// Step back one (no-op at step 0).
fn prev_step(
    catalog: &TourCatalog,
    active: &mut ActiveTour,
    hud: &mut TutorialHud,
    commands: &mut Commands,
) {
    if active.cursor > 0 {
        active.cursor -= 1;
        show_current_step(active, catalog, hud, commands);
    }
}

// ── Systems ─────────────────────────────────────────────────────────────────

/// Advance the active tour on the coach card's `cmd:Tutorial*` bus events.
/// Idle (returns immediately) when no tour is playing, so it never interferes
/// with a rhai-driven tour in the sandbox.
fn advance_on_bus(
    trigger: On<TelemetryEvent>,
    catalog: Res<TourCatalog>,
    mut active: ResMut<ActiveTour>,
    mut hud: ResMut<TutorialHud>,
    mut commands: Commands,
) {
    if !active.is_active() {
        return;
    }
    match trigger.event().name.as_str() {
        "cmd:TutorialNext" => next_step(&catalog, &mut active, &mut hud, &mut commands),
        "cmd:TutorialBack" => prev_step(&catalog, &mut active, &mut hud, &mut commands),
        "cmd:TutorialSkip" => end_tour(&mut active, &mut hud),
        "cmd:TutorialGoto" => {
            if let TelemetryValue::I64(i) = &trigger.event().data {
                let target = (*i).max(0) as usize;
                goto_step(target, &catalog, &mut active, &mut hud, &mut commands);
            }
        }
        _ => {}
    }
}

/// Toggle whether the active tour auto-opens on the next start — the card's
/// "Show on next start" checkbox. Payload `true` re-arms (removes the tour from
/// [`TourSeen`]); `false` marks it seen. Only meaningful while a data tour is
/// active (rhai tours don't use [`TourSeen`]).
fn on_tour_pin(trigger: On<TelemetryEvent>, active: Res<ActiveTour>, mut seen: ResMut<TourSeen>) {
    if trigger.event().name != "cmd:TutorialPin" {
        return;
    }
    let Some(id) = active.id else { return };
    let key = id.as_str();
    let show_next_start = matches!(trigger.event().data, TelemetryValue::Bool(true));
    if show_next_start {
        seen.seen.retain(|s| s != key);
    } else if !seen.contains(key) {
        seen.seen.push(key.to_string());
    }
}

/// Start a tour when the perspective help popup (or a Help-menu entry) publishes
/// a [`HelpTourRequest`] for a perspective we have a tour for. Leaves requests
/// for other perspectives untouched.
fn consume_tour_request(
    mut req: ResMut<HelpTourRequest>,
    catalog: Res<TourCatalog>,
    mut active: ResMut<ActiveTour>,
    mut hud: ResMut<TutorialHud>,
    mut commands: Commands,
) {
    let Some(id) = req.0 else { return };
    if catalog.get(id).is_some() {
        req.0 = None;
        start_tour(id, &catalog, &mut active, &mut hud, &mut commands);
    }
}

/// Auto-open a `first_start` tour once per install. Runs a single time (guarded
/// by a `Local`); marks the tour seen *before* opening so closing it normally
/// won't re-arm it — matching the old lunica behaviour (critical on wasm, where
/// settings persist but the flag must flip on first show).
fn first_start_autolaunch(
    mut done: Local<bool>,
    catalog: Res<TourCatalog>,
    mut seen: ResMut<TourSeen>,
    mut active: ResMut<ActiveTour>,
    mut hud: ResMut<TutorialHud>,
    mut commands: Commands,
) {
    if *done {
        return;
    }
    *done = true;
    let Some(id) = catalog.pending_first_start(&seen) else {
        return;
    };
    seen.seen.push(id.as_str().to_string());
    start_tour(id, &catalog, &mut active, &mut hud, &mut commands);
}

/// Keyboard: ←/→ navigate and Esc closes while a tour plays; F1 (re)opens the
/// first-start tour when idle.
fn tour_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    catalog: Res<TourCatalog>,
    mut active: ResMut<ActiveTour>,
    mut hud: ResMut<TutorialHud>,
    mut commands: Commands,
) {
    if active.is_active() {
        if keys.just_pressed(KeyCode::Escape) {
            end_tour(&mut active, &mut hud);
        } else if keys.just_pressed(KeyCode::ArrowRight) {
            next_step(&catalog, &mut active, &mut hud, &mut commands);
        } else if keys.just_pressed(KeyCode::ArrowLeft) {
            prev_step(&catalog, &mut active, &mut hud, &mut commands);
        }
    } else if keys.just_pressed(KeyCode::F1) {
        if let Some(id) = catalog.any_first_start() {
            start_tour(id, &catalog, &mut active, &mut hud, &mut commands);
        }
    }
}

/// Adds the tour catalog, active-tour state, persisted seen-set, the coach-card
/// advancement observer, and the request/first-start/keyboard systems. Added by
/// [`WorkbenchPlugin`](crate::WorkbenchPlugin), so every workbench app can play
/// data tours. Idempotent.
pub struct TourDriverPlugin;

impl Plugin for TourDriverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TourCatalog>();
        app.init_resource::<ActiveTour>();
        app.register_settings_section::<TourSeen>();
        app.add_observer(advance_on_bus);
        app.add_observer(on_tour_pin);
        app.add_systems(
            Update,
            (consume_tour_request, first_start_autolaunch, tour_keyboard),
        );
    }
}
