//! Data-driven, Rhai-scripted tutorial system.
//!
//! A **tutorial is data**: a USD scene (`*.usda`) that sets up the environment
//! and, via `lunco:scriptPath`, attaches a `*.rhai` orchestrator. The orchestrator
//! uses the ordinary scripting substrate — the task sequencer, the declarative
//! `mission(me)` objectives, `wait_for`/`requires_event` on the TelemetryEvent
//! bus (including `cmd:*` UI actions), and the persistent HUD (`hint`,
//! `objectives_hud`, `spotlight`) — so tutorial *logic* needs no Rust.
//!
//! This crate adds only the surrounding shell:
//! - [`TutorialRegistry`] — the catalog of available tutorials.
//! - [`TutorialsPanel`] — a dockable launcher listing them with progress.
//! - [`StartTutorial`] / [`SkipTutorial`] — load a tutorial's scene / clear the HUD.
//! - [`TutorialProgress`] — persisted completed-set, updated on `MISSION_COMPLETE`.
//! - [`SetSubsystemEnabled`] — the progressive-fidelity toggle (flips
//!   [`SubsystemToggles`](lunco_core::subsystems::SubsystemToggles)).
//!
//! It is UI-gated and holds no simulation state — headless CI/CD engineering
//! runs pay nothing (spec 011, "Architectural Separation").

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_core::subsystems::{SubsystemToggles, SUBSYSTEMS};
use lunco_core::{on_command, register_commands, Command, NextScene, Severity, TelemetryEvent, TelemetryValue};
use lunco_workbench::tutorial_overlay::TutorialHud;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot, WorkbenchAppExt};
use serde::{Deserialize, Serialize};

/// One tutorial's catalog entry. The heavy content (scene + script) is on disk;
/// this is just what the launcher needs to list and start it.
#[derive(Clone, Debug)]
pub struct TutorialMeta {
    /// Stable id (kebab-case). Progress and `StartTutorial` key off this.
    pub id: &'static str,
    /// Display title.
    pub title: &'static str,
    /// One-line description shown under the title.
    pub blurb: &'static str,
    /// Which app it targets — `"sandbox"`, `"lunica"`, or `"any"`. The launcher
    /// may filter on this; today it lists all.
    pub app: &'static str,
    /// Difficulty tag (`"beginner"` / `"intermediate"` / …) shown as a chip.
    pub difficulty: &'static str,
    /// Asset-relative path to the tutorial's USD scene, loaded by [`StartTutorial`].
    /// The chain to the NEXT tutorial is NOT here — it lives in the scene's USD
    /// (`lunco:nextScene`), so each tutorial declares its own successor as data.
    pub scene: &'static str,
    /// Auto-launch this tutorial once on the user's first run (persisted via
    /// [`TutorialProgress::onboarded`]). At most one built-in should set it —
    /// the first-run onboarding entry point.
    pub first_start: bool,
}

/// The catalog of registered tutorials. Populated in [`TutorialPlugin::build`]
/// with the built-ins; extend at app-build time with [`register_tutorial`](Self::register_tutorial).
///
/// Tutorials are registered in code (not filesystem-scanned) so the catalog is
/// identical on native, packaged, and wasm builds — the *content* still lives in
/// data files; only the one-line catalog entry is code. A native `meta.toml`
/// scan can augment this later without changing the panel or commands.
#[derive(Resource, Default, Clone)]
pub struct TutorialRegistry {
    pub tutorials: Vec<TutorialMeta>,
}

impl TutorialRegistry {
    /// Add a tutorial to the catalog (idempotent on `id`).
    pub fn register_tutorial(&mut self, meta: TutorialMeta) {
        if !self.tutorials.iter().any(|t| t.id == meta.id) {
            self.tutorials.push(meta);
        }
    }

    fn get(&self, id: &str) -> Option<&TutorialMeta> {
        self.tutorials.iter().find(|t| t.id == id)
    }
}

/// Persisted tutorial progress. Stored under the `"tutorial_progress"` key of
/// `settings.json`.
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct TutorialProgress {
    /// Ids of tutorials whose mission reported `MISSION_COMPLETE`.
    pub completed: Vec<String>,
    /// The tutorial currently running (set by [`StartTutorial`], cleared on
    /// completion/skip) — so a `MISSION_COMPLETE` event is attributed correctly.
    pub current: Option<String>,
    /// First-run onboarding has been offered. Set once the `first_start`
    /// tutorial auto-launches, so it never re-fires on later launches.
    /// `#[serde(default)]` keeps older `settings.json` (no field) loading.
    #[serde(default)]
    pub onboarded: bool,
}

impl TutorialProgress {
    fn is_completed(&self, id: &str) -> bool {
        self.completed.iter().any(|c| c == id)
    }
}

impl lunco_settings::SettingsSection for TutorialProgress {
    const KEY: &'static str = "tutorial_progress";
}

// ── Commands ────────────────────────────────────────────────────────────────

/// Start a tutorial by id: load its scene (which auto-attaches the orchestrator
/// script) and record it as current. Rhai/API: `cmd("StartTutorial", #{ id })`.
#[Command(default)]
pub struct StartTutorial {
    /// The [`TutorialMeta::id`] to start.
    pub id: String,
}

/// Stop the current tutorial: clear the HUD (hint, objectives, spotlight) and
/// forget the current id. Does not unload the scene. Rhai/API: `cmd("SkipTutorial")`.
#[Command(default)]
pub struct SkipTutorial {}

/// Enable/disable a simulation subsystem at runtime (progressive fidelity).
/// `name` must be in [`SUBSYSTEMS`]; unknown names are rejected. Flips
/// [`SubsystemToggles`] and lands `subsystem:<name>` on the event bus. Rhai:
/// `set_subsystem(name, on)`.
#[Command(default)]
pub struct SetSubsystemEnabled {
    /// Subsystem key from the [`SUBSYSTEMS`] allow-list.
    pub name: String,
    /// `true` enables, `false` disables.
    pub on: bool,
}

#[on_command(StartTutorial)]
fn on_start_tutorial(
    trigger: On<StartTutorial>,
    registry: Res<TutorialRegistry>,
    mut progress: ResMut<TutorialProgress>,
    mut commands: Commands,
) {
    let id = trigger.event().id.clone();
    let Some(meta) = registry.get(&id) else {
        warn!("[tutorial] StartTutorial: unknown id '{id}'");
        return;
    };
    info!("[tutorial] starting '{}' → {}", meta.title, meta.scene);
    // Dispatch LoadScene generically through the API bus so this crate needs no
    // dependency on whichever crate owns the scene-load command. The scene's
    // `lunco:scriptPath` orchestrator drives the HUD from there.
    commands.trigger(lunco_api::ApiCommandEvent {
        command: "LoadScene".to_string(),
        params: serde_json::json!({ "path": meta.scene }),
        id: 0,
    });
    progress.current = Some(id);
}

#[on_command(SkipTutorial)]
fn on_skip_tutorial(_t: On<SkipTutorial>, mut hud: ResMut<TutorialHud>, mut progress: ResMut<TutorialProgress>) {
    hud.hint.clear();
    hud.objectives.clear();
    hud.spotlight = None;
    progress.current = None;
}

#[on_command(SetSubsystemEnabled)]
fn on_set_subsystem_enabled(
    trigger: On<SetSubsystemEnabled>,
    mut toggles: ResMut<SubsystemToggles>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    if !SubsystemToggles::is_known(&ev.name) {
        warn!("[subsystem] unknown subsystem '{}' (allow-list: {:?}) — ignored", ev.name, SUBSYSTEMS);
        return;
    }
    toggles.set(ev.name.clone(), ev.on);
    info!("[subsystem] {} = {}", ev.name, ev.on);
    commands.trigger(TelemetryEvent {
        name: format!("subsystem:{}", ev.name),
        source: 0,
        severity: Severity::Info,
        data: TelemetryValue::Bool(ev.on),
        timestamp: 0.0,
    });
}

register_commands!(on_start_tutorial, on_skip_tutorial, on_set_subsystem_enabled,);

/// On `MISSION_COMPLETE`, record the completion and auto-advance the chain by
/// loading the scene named in USD (`lunco:nextScene` → [`NextScene`]) — the
/// scenario manager. The chain lives entirely in DATA: each tutorial's scene
/// declares its own successor, so there is NO per-tutorial Rust and no central
/// campaign object. Works regardless of how the current scene was launched
/// (`--scene`, the panel, or a prior chain step), since it reads the loaded
/// world, not `TutorialProgress`.
fn on_mission_complete(
    trigger: On<TelemetryEvent>,
    q_next: Query<&NextScene>,
    mut progress: ResMut<TutorialProgress>,
    mut commands: Commands,
) {
    if trigger.event().name != "MISSION_COMPLETE" {
        return;
    }
    // Mark the current tutorial complete (for the launcher's ✓), if one is tracked.
    if let Some(id) = progress.current.take() {
        if !progress.is_completed(&id) {
            info!("[tutorial] completed '{id}'");
            progress.completed.push(id);
        }
    }
    // Auto-advance: load the successor scene declared in USD, if any.
    let Some(next) = q_next.iter().map(|n| n.0.clone()).find(|s| !s.is_empty()) else {
        return;
    };
    info!("[tutorial] auto-advancing → scene '{next}'");
    commands.trigger(lunco_api::ApiCommandEvent {
        command: "ShowNotification".to_string(),
        params: serde_json::json!({ "text": "Loading next scene…", "kind": "success" }),
        id: 0,
    });
    commands.trigger(lunco_api::ApiCommandEvent {
        command: "LoadScene".to_string(),
        params: serde_json::json!({ "path": next }),
        id: 0,
    });
}

/// Auto-launch the first-run onboarding tutorial once. Fires ~1 s after start
/// (so the default scene has settled before we swap to the tutorial scene) and
/// only for a genuine interactive first run: skipped when `--scene` or `--api`
/// is on the command line (automated / explicit-scene sessions), and naturally
/// absent headless (the plugin is UI-gated). Gated by the persisted
/// [`TutorialProgress::onboarded`] flag so it never re-fires.
fn first_start_autolaunch(
    mut ticks: Local<u32>,
    registry: Res<TutorialRegistry>,
    mut progress: ResMut<TutorialProgress>,
    mut commands: Commands,
) {
    const DONE: u32 = u32::MAX;
    const SETTLE_TICKS: u32 = 60; // ~1 s at 60 fps

    if *ticks == DONE {
        return;
    }
    if progress.onboarded {
        *ticks = DONE;
        return;
    }
    *ticks += 1;
    if *ticks < SETTLE_TICKS {
        return;
    }
    *ticks = DONE; // act at most once per process

    // Don't hijack automated or explicit-scene launches — those aren't
    // first-run onboarding. Leaving `onboarded` false means a later plain
    // launch still onboards.
    if std::env::args().any(|a| a == "--scene" || a == "--api") {
        return;
    }

    let Some(meta) = registry
        .tutorials
        .iter()
        .find(|t| t.first_start && !progress.is_completed(t.id))
    else {
        // Nothing to onboard (already completed, or none flagged) — mark done
        // so we stop checking on every future launch.
        progress.onboarded = true;
        return;
    };
    info!("[tutorial] first-run onboarding → {}", meta.title);
    progress.onboarded = true;
    commands.trigger(StartTutorial { id: meta.id.to_string() });
}

// ── Launcher panel ────────────────────────────────────────────────────────

/// Panel id for the tutorials launcher.
pub const TUTORIALS_PANEL_ID: PanelId = PanelId("tutorials");

/// Dockable launcher: lists registered tutorials with a completion tick and a
/// Start button; offers Stop while a tutorial is running.
pub struct TutorialsPanel;

impl Panel for TutorialsPanel {
    fn id(&self) -> PanelId {
        TUTORIALS_PANEL_ID
    }
    fn title(&self) -> String {
        "Tutorials".to_string()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let registry = ctx.resource::<TutorialRegistry>().cloned().unwrap_or_default();
        let progress = ctx.resource::<TutorialProgress>().cloned().unwrap_or_default();

        ui.add_space(4.0);
        ui.heading("🎓 Tutorials");
        ui.label(egui::RichText::new("Interactive, scripted lessons.").weak().small());
        ui.separator();

        if registry.tutorials.is_empty() {
            ui.label(egui::RichText::new("No tutorials registered.").weak());
            return;
        }

        if let Some(cur) = &progress.current {
            let title = registry.get(cur).map(|m| m.title).unwrap_or(cur.as_str());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("▶ Running: {title}")).color(egui::Color32::from_rgb(120, 200, 255)));
                if ui.small_button("Stop").clicked() {
                    ctx.trigger(SkipTutorial {});
                }
            });
            ui.separator();
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for meta in &registry.tutorials {
                let done = progress.is_completed(meta.id);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if done {
                            ui.label(egui::RichText::new("✓").color(egui::Color32::from_rgb(120, 210, 140)).strong());
                        }
                        ui.label(egui::RichText::new(meta.title).strong());
                        ui.label(egui::RichText::new(meta.difficulty).weak().small());
                    });
                    ui.label(egui::RichText::new(meta.blurb).small());
                    ui.horizontal(|ui| {
                        let label = if done { "Replay" } else { "Start" };
                        if ui.button(label).clicked() {
                            ctx.trigger(StartTutorial { id: meta.id.to_string() });
                        }
                        ui.label(egui::RichText::new(format!("· {}", meta.app)).weak().small());
                    });
                });
                ui.add_space(4.0);
            }
        });
    }
}

/// The built-in tutorial catalog. Both point at real, shipped scenes; the
/// scene's `lunco:scriptPath` supplies the lesson logic.
fn builtin_tutorials() -> Vec<TutorialMeta> {
    vec![
        TutorialMeta {
            id: "sandbox-intro",
            title: "Sandbox Intro",
            blurb: "A guided coach-mark tour of the workspace — viewport, browser, inspector, console. Advances with Back / Next / Skip. Chains into First Drive.",
            app: "sandbox",
            difficulty: "beginner",
            scene: "tutorials/sandbox_intro/sandbox_intro.usda",
            first_start: true,
        },
        TutorialMeta {
            id: "first-drive",
            title: "First Drive",
            blurb: "Take control of a rover and drive it to a flag on the lunar surface. Teaches possession and driving — every step advances when you actually do it.",
            app: "sandbox",
            difficulty: "beginner",
            scene: "tutorials/first_drive/first_drive.usda",
            first_start: false,
        },
        TutorialMeta {
            id: "lander-rover-mission",
            title: "Lander & Rover Mission",
            blurb: "Watch a powered descent land a rover, then drive the deployed rover through a waypoint course. A full multi-vehicle mission scene.",
            app: "sandbox",
            difficulty: "intermediate",
            scene: "scenes/sandbox/lander_test.usda",
            first_start: false,
        },
    ]
}

/// Adds the registry (seeded with built-ins), persisted progress, the three
/// commands, the mission-complete observer, and the launcher panel. UI-gated:
/// panel + commands are harmless headless (the panel just isn't drawn).
pub struct TutorialPlugin;

impl Plugin for TutorialPlugin {
    fn build(&self, app: &mut App) {
        use lunco_settings::AppSettingsExt;

        let mut registry = TutorialRegistry::default();
        for t in builtin_tutorials() {
            registry.register_tutorial(t);
        }
        app.insert_resource(registry);
        app.register_settings_section::<TutorialProgress>();
        register_all_commands(app);
        app.add_observer(on_mission_complete);
        app.add_systems(Update, first_start_autolaunch);
        app.register_panel(TutorialsPanel);
    }
}
