//! The lunica tutorial curriculum — **rhai-driven** guided lessons.
//!
//! ## Tutorials are scripts now, not Rust data
//!
//! Lunica used to hardcode a single ten-step tour as a `Vec<TourStepDef>` in this
//! file. Now that the workbench hosts the rhai scripting runtime
//! ([`lunco_scripting::LunCoScriptingPlugin`], added in `build_modelica_core`),
//! each tutorial is an ordinary **scenario** authored in
//! `assets/tutorials/lunica/*.rhai` — the *same* substrate the sandbox tutorials
//! use. A lesson calls `coach_step()` (HUD prelude) to spotlight a widget and
//! draw a card, and advances its `this.i` cursor in `on_event` on the card's
//! Back/Next/Skip/dot bus events. No tour state machine in Rust.
//!
//! This module is just the thin **launcher shell**:
//! - [`TUTORIALS`] — the catalog (id/title/blurb). The `.rhai` source is loaded
//!   at launch from `lunco_assets` (disk on native, embedded on wasm).
//! - a top-level **🎓 Tutorials** menu + a Help-menu "Show Tour" entry.
//! - launch = load `assets/tutorials/lunica/<id>.rhai` via
//!   [`lunco_assets::tutorials::lunica_tutorial_source`] and attach it to a
//!   persistent host entity via [`RunScenario`](lunco_scripting::commands::RunScenario).
//!   On native the source is read fresh each launch, so editing a tutorial and
//!   replaying it shows the change with **no rebuild** — real live authoring.
//! - first-run onboarding + completion ticks. Completion ✓ persists in
//!   [`LunicaTutorialProgress`]; the onboarding *gate* is itself rhai
//!   (`onboarding.rhai`), persisting the old `tour_seen` flag via [`TutorialSeen`].
//!
//! Because `cmd()` dispatches through the transport-free command core that
//! scripting self-supplies, none of this needs the HTTP API — scripting is
//! independent of the API server.

use bevy::prelude::*;

/// The perspective whose help popup shows a "Show Tour" button. Publishing a
/// [`HelpTourRequest`](lunco_workbench::HelpTourRequest) for it (the button, or
/// our Help-menu entry) launches the Overview lesson.
#[cfg(feature = "scripting")]
const TOUR_PERSPECTIVE: lunco_workbench::PerspectiveId =
    lunco_workbench::PerspectiveId("modelica_analyze");

/// One lunica tutorial's catalog entry (id/title/blurb) — what the menu needs to
/// list + launch. The rhai orchestrator lives on disk and is loaded by id.
#[cfg(feature = "scripting")]
#[derive(Clone, Copy)]
struct LunicaTutorial {
    /// Stable id (kebab-case). Progress + launch key off this.
    id: &'static str,
    /// Menu label.
    title: &'static str,
    /// One-line "what this teaches", shown on hover.
    blurb: &'static str,
    // The rhai orchestrator is `assets/tutorials/lunica/<id>.rhai`, loaded at
    // launch by `lunco_assets::tutorials::lunica_tutorial_source(id)` — disk on
    // native (live-editable), embedded on wasm. The id IS the file stem, so the
    // catalog needs no source field.
}

/// The curriculum: one first-run Overview + focused, à-la-carte lessons, each on
/// one concept. Add a lesson by dropping a `.rhai` under `assets/tutorials/lunica/`
/// and adding a row here — no engine change.
#[cfg(feature = "scripting")]
const TUTORIALS: &[LunicaTutorial] = &[
    LunicaTutorial {
        id: "overview",
        title: "Lunica Overview",
        blurb: "A 90-second guided tour of the whole workbench — start here.",
    },
    LunicaTutorial {
        id: "workspace",
        title: "1 · Your Workspace",
        blurb: "Twins, the browser, read-only libraries, and learning paths.",
    },
    LunicaTutorial {
        id: "model",
        title: "2 · Open & View a Model",
        blurb: "The four views (Text/Diagram/Icon/Docs) + graphical composition.",
    },
    LunicaTutorial {
        id: "run",
        title: "3 · Compile & Run",
        blurb: "Interactive vs Fast Run, and driving inputs live.",
    },
    LunicaTutorial {
        id: "experiments",
        title: "4 · Experiments & Sweeps",
        blurb: "Override parameters and sweep them without editing source.",
    },
    LunicaTutorial {
        id: "plots",
        title: "5 · Plots & Results",
        blurb: "Reading a run: graphs, diagnostics, and the console.",
    },
    LunicaTutorial {
        id: "scripting",
        title: "6 · Automate (API + MCP)",
        blurb: "Drive the workbench from scripts, the HTTP API, and MCP.",
    },
];

/// Persisted tutorial progress, under the `"lunica_tutorial_progress"` key of
/// `settings.json`.
#[cfg(feature = "scripting")]
#[derive(Resource, serde::Serialize, serde::Deserialize, Default, Clone, PartialEq, Debug)]
pub struct LunicaTutorialProgress {
    /// Ids whose lesson reported `MISSION_COMPLETE`.
    completed: Vec<String>,
    /// The lesson currently running (set on launch), so a `MISSION_COMPLETE` is
    /// attributed correctly.
    current: Option<String>,
}

#[cfg(feature = "scripting")]
impl lunco_settings::SettingsSection for LunicaTutorialProgress {
    const KEY: &'static str = "lunica_tutorial_progress";
}

/// Persisted "already onboarded" flag — the reimplementation of the old
/// `tour_seen` state, now **owned by rhai**. The onboarding *decision* (is this a
/// first run? show the overview?) lives in `assets/tutorials/lunica/onboarding.rhai`,
/// which reads and writes this flag with `get_setting`/`set_setting(
/// "TutorialSeen.onboarded", ..)`. It is therefore [`Reflect`]-registered (so the
/// rhai settings verbs can reach it) and persisted under the **`tour_seen`** key —
/// preserving the pre-rhai tour's setting. Rust only stores it; the policy is rhai.
#[cfg(feature = "scripting")]
#[derive(Resource, Reflect, serde::Serialize, serde::Deserialize, Default, Clone, PartialEq, Debug)]
#[reflect(Resource)]
pub struct TutorialSeen {
    /// Whether first-run onboarding has already happened (persisted).
    pub onboarded: bool,
}

#[cfg(feature = "scripting")]
impl lunco_settings::SettingsSection for TutorialSeen {
    const KEY: &'static str = "tour_seen";
}

/// The persistent entity every tutorial scenario attaches to. Spawned lazily on
/// the first launch; re-launching a lesson hot-reloads the scenario on it.
#[cfg(feature = "scripting")]
#[derive(Resource, Default)]
struct LunicaTutorialHost(Option<Entity>);

pub struct HelpOverlayPlugin;

impl Plugin for HelpOverlayPlugin {
    #[cfg(feature = "scripting")]
    fn build(&self, app: &mut App) {
        use lunco_settings::AppSettingsExt;
        app.init_resource::<LunicaTutorialHost>();
        app.register_settings_section::<LunicaTutorialProgress>();
        // `tour_seen` flag — reflect-registered so `onboarding.rhai` reads/writes it.
        app.register_type::<TutorialSeen>();
        app.register_settings_section::<TutorialSeen>();
        launcher::register(app); // LaunchTutorial command + F1 intent resolver
        app.add_systems(Startup, register_menus);
        app.add_systems(Update, (attach_onboarding_once, consume_show_tour_request));
        app.add_observer(mark_completion);
    }

    // Scripting off (a rare `ui`-without-`scripting` build): no rhai runtime to
    // run tutorials, so the launcher is a no-op. The workbench still compiles.
    #[cfg(not(feature = "scripting"))]
    fn build(&self, _app: &mut App) {}
}

// Everything below needs the scripting runtime (RunScenario) — gated so the
// module still compiles without it.
#[cfg(feature = "scripting")]
mod launcher {
    use super::*;
    use bevy_egui::egui;
    use lunco_core::{on_command, register_commands, Command};
    use lunco_doc_bevy::EditorIntent;
    use lunco_workbench::{tutorial_overlay::TutorialHud, HelpTourRequest, WorkbenchLayout};

    /// Launch a lunica tutorial by id — the single command **every** entry point
    /// funnels through: the 🎓 Tutorials menu, F1 (via the intent resolver below),
    /// the HTTP API, MCP, and other scripts. Reflect-dispatched, so `cmd(
    /// "LaunchTutorial", #{ id: "run" })` works from rhai too.
    #[Command(default)]
    pub struct LaunchTutorial {
        /// The [`TUTORIALS`] id to launch (e.g. `"overview"`, `"run"`).
        pub id: String,
    }

    #[on_command(LaunchTutorial)]
    fn on_launch_tutorial(trigger: On<LaunchTutorial>, mut commands: Commands) {
        let id = trigger.event().id.clone();
        // `launch` needs `&mut World` (spawns the host, triggers RunScenario); an
        // observer only has `Commands`, so defer to an exclusive closure.
        commands.queue(move |world: &mut World| launch(world, &id));
    }

    /// Keybinding → intent → command: `lunco-doc-bevy` maps `F1` to
    /// [`EditorIntent::ShowTutorial`]; this resolver turns that intent into the
    /// [`LaunchTutorial`] command (Overview). One shared path with the menu / API.
    pub(super) fn resolve_show_tutorial_intent(trigger: On<EditorIntent>, mut commands: Commands) {
        if matches!(*trigger.event(), EditorIntent::ShowTutorial) {
            commands.trigger(LaunchTutorial { id: "overview".to_string() });
        }
    }

    register_commands!(on_launch_tutorial,);

    /// Register the launch command + the F1/`ShowTutorial` intent resolver.
    pub(super) fn register(app: &mut App) {
        register_all_commands(app);
        app.add_observer(resolve_show_tutorial_intent);
    }

    /// Spawn (once) and return the host entity that scenarios attach to.
    fn ensure_host(world: &mut World) -> Entity {
        if let Some(e) = world.resource::<LunicaTutorialHost>().0 {
            return e;
        }
        let e = world.spawn(Name::new("LunicaTutorialHost")).id();
        world.resource_mut::<LunicaTutorialHost>().0 = Some(e);
        e
    }

    /// Launch lesson `id`: attach its rhai source to the host (hot-reloads on
    /// re-run), and record it as current. The scenario's `on_start` takes over
    /// from there (spotlight + card).
    pub(super) fn launch(world: &mut World, id: &str) {
        let Some(tut) = TUTORIALS.iter().find(|t| t.id == id) else {
            warn!("[lunica-tutorial] unknown id '{id}'");
            return;
        };
        // The asset crate owns the disk-vs-embed policy: on native this reads the
        // `.rhai` fresh from `assets/tutorials/lunica/<id>.rhai` each launch, so a
        // user can edit a lesson and replay it live; on wasm it's the embedded copy.
        let Some(source) = lunco_assets::tutorials::lunica_tutorial_source(id) else {
            warn!("[lunica-tutorial] no source for '{id}'");
            return;
        };
        let host = ensure_host(world);
        info!("[lunica-tutorial] launching '{}'", tut.title);
        world.trigger(lunco_scripting::commands::RunScenario {
            target: host,
            source,
            params: String::new(),
        });
        if let Some(mut p) = world.get_resource_mut::<LunicaTutorialProgress>() {
            p.current = Some(id.to_string());
        }
    }

    /// Stop the running lesson: clear the coach card + hint + spotlight. The
    /// idle scenario stays attached (harmless); the next launch replaces it.
    fn stop(world: &mut World) {
        if let Some(mut hud) = world.get_resource_mut::<TutorialHud>() {
            hud.tour = None;
            hud.hint.clear();
            hud.spotlight = None;
        }
        if let Some(mut p) = world.get_resource_mut::<LunicaTutorialProgress>() {
            p.current = None;
        }
    }

    /// Register the top-level **🎓 Tutorials** menu and the Help-menu entry.
    pub(super) fn register_menus(world: &mut World) {
        let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
            return;
        };

        // Dedicated top-level menu, listing every lesson with a completion tick.
        layout.register_custom_menu("🎓 Tutorials", |ui, world| {
            let progress = world
                .get_resource::<LunicaTutorialProgress>()
                .cloned()
                .unwrap_or_default();
            let running = progress.current.is_some();

            ui.label(egui::RichText::new("Interactive, scripted lessons").weak().small());
            ui.separator();

            for (idx, tut) in TUTORIALS.iter().enumerate() {
                let done = progress.completed.iter().any(|c| c == tut.id);
                let glyph = if done { "✓" } else { "🎓" };
                let resp = ui
                    .button(format!("{glyph}  {}", tut.title))
                    .on_hover_text(tut.blurb);
                if resp.clicked() {
                    world.trigger(LaunchTutorial { id: tut.id.to_string() });
                    ui.close();
                }
                // A rule after the Overview separates it from the focused set.
                if idx == 0 {
                    ui.separator();
                }
            }

            ui.separator();
            ui.add_enabled_ui(running, |ui| {
                if ui.button("⏹ Stop tutorial").clicked() {
                    stop(world);
                    ui.close();
                }
            });
        });

        // Keep the familiar Help ▸ Show Tour entry pointed at the Overview.
        layout.register_help_menu(|ui, world, _layout| {
            if ui
                .button("🎓 Show Tour")
                .on_hover_text("Replay the guided overview of the workbench")
                .clicked()
            {
                world.trigger(LaunchTutorial { id: "overview".to_string() });
                ui.close();
            }
        });
    }

    /// The perspective help popup's "Show Tour" button publishes a
    /// [`HelpTourRequest`] for our perspective. Consume it → launch the Overview.
    pub(super) fn consume_show_tour_request(world: &mut World) {
        let hit = {
            let mut req = world.resource_mut::<HelpTourRequest>();
            if req.0 == Some(TOUR_PERSPECTIVE) {
                req.0 = None;
                true
            } else {
                false
            }
        };
        if hit {
            world.trigger(LaunchTutorial { id: "overview".to_string() });
        }
    }

    /// Attach the rhai onboarding gate once per process — ~1s after start (so the
    /// layout settles), skipping automated `--api` / `--no-ui` sessions. This only
    /// TRIGGERS; the *decision* (first run? show the overview? remember it) lives
    /// in `onboarding.rhai`, which self-gates on the persisted `tour_seen` flag.
    /// So an already-onboarded user just gets an inert scenario attached (a no-op).
    pub(super) fn attach_onboarding_once(world: &mut World, mut ticks: Local<u32>) {
        const DONE: u32 = u32::MAX;
        const SETTLE_TICKS: u32 = 60; // ~1s at 60fps

        if *ticks == DONE {
            return;
        }
        *ticks += 1;
        if *ticks < SETTLE_TICKS {
            return;
        }
        *ticks = DONE;

        // Don't hijack automation / explicit API sessions.
        if std::env::args().any(|a| a == "--api" || a == "--no-ui") {
            return;
        }

        // If the user already launched a lesson during the settle window, don't
        // clobber it with the onboarding gate.
        if world.resource::<LunicaTutorialProgress>().current.is_some() {
            return;
        }

        let Some(source) = lunco_assets::tutorials::lunica_tutorial_source("onboarding") else {
            return;
        };
        let host = ensure_host(world);
        world.trigger(lunco_scripting::commands::RunScenario {
            target: host,
            source,
            params: String::new(),
        });
    }

    /// On `MISSION_COMPLETE`, mark the current lesson complete (for the menu ✓).
    pub(super) fn mark_completion(
        trigger: On<lunco_core::TelemetryEvent>,
        mut progress: ResMut<LunicaTutorialProgress>,
    ) {
        if trigger.event().name != "MISSION_COMPLETE" {
            return;
        }
        if let Some(id) = progress.current.take() {
            if !progress.completed.iter().any(|c| c == &id) {
                info!("[lunica-tutorial] completed '{id}'");
                progress.completed.push(id);
            }
        }
    }
}

#[cfg(feature = "scripting")]
use launcher::{
    attach_onboarding_once, consume_show_tour_request, mark_completion, register_menus,
};
