//! The unified tutorial launcher — for **every** workbench app (lunica, sandbox, …).
//!
//! ## A lesson is an authored scenario plus an optional declared world
//!
//! A tutorial is a single Rhai scenario (`assets/tutorials/<script>`) and an
//! optional composed world declared by its curriculum metadata. The launcher
//! owns a short-lived host entity: it mounts the declared world first, waits for
//! the scene transaction's completion edge, and only then attaches the scenario
//! through [`RunScenario`](lunco_scripting::commands::RunScenario). UI-only
//! lessons omit the world and start immediately. Stopping or replacing a
//! lesson winds down that host and, when the lesson owns a world, clears it
//! through the normal scene lifecycle command. A lesson may still use
//! `cmd("OpenClass", …)` or `set_subsystem(…)` for its authored teaching steps,
//! but it does not own a second ad-hoc scene loader.
//!
//! So this crate is the thin **shell** shared by all apps:
//! - [`TutorialRegistry`] — the catalog, composed from curriculum LAYERS
//!   (`assets/tutorials/<app>.usda`) by [`curriculum::read`]; a twin contributes
//!   its own root, and apps may still add lessons via
//!   [`TutorialAppExt::register_tutorial`].
//! - a top-level **🎓 Tutorials** menu + a dockable [`TutorialsPanel`].
//! - [`StartTutorial`] — mount the declared world, then run `<script>` on the
//!   host (the single launch path; menu, F1, HTTP API, MCP, and other scripts
//!   all funnel here).
//! - first-run onboarding ([`TutorialProgress::onboarded`]), completion ticks
//!   (on `MISSION_COMPLETE`), a data-driven chain ([`TutorialMeta::next`]), F1
//!   (via [`EditorIntent::ShowTutorial`](lunco_doc_bevy::EditorIntent)), and the
//!   progressive-fidelity toggle ([`SetSubsystemEnabled`]).
//!
//! The execution core is headless-safe; the menu, launcher panel, HUD, and
//! confirmation popup are an optional UI projection.

use bevy::prelude::*;
#[cfg(feature = "ui")]
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use lunco_core::subsystems::{SubsystemToggles, SUBSYSTEMS};
use lunco_core::{
    on_command, register_commands, Command, Severity, TelemetryEvent, TelemetryValue,
};
use lunco_doc_bevy::EditorIntent;
use lunco_settings::AppSettingsExt;
#[cfg(feature = "ui")]
use lunco_workbench::tutorial_overlay::TutorialHud;
#[cfg(feature = "ui")]
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot, WorkbenchAppExt, WorkbenchLayout};
use serde::{Deserialize, Serialize};

/// One tutorial's catalog entry — a lesson prim, flattened for the menu/panel.
///
/// **Data, not code.** Lessons are authored in a curriculum LAYER
/// (`assets/tutorials/<track>/curriculum.usda`) and read by [`curriculum::read`]
/// at startup — adding a lesson never touches Rust.
#[derive(Clone, Debug)]
pub struct TutorialMeta {
    /// The lesson's prim path — its IDENTITY. Progress, chaining and
    /// [`StartTutorial`] key off this, so there is no separate id string to keep
    /// in step with the prim that defines the lesson.
    pub id: String,
    /// Display title.
    pub title: String,
    /// One-line description shown under the title / on hover.
    pub blurb: String,
    /// The TRACK prim path this lesson belongs to. The menu groups by it and
    /// looks the heading up in [`TutorialRegistry::tracks`] under the same key.
    pub app: String,
    /// Difficulty tag (`"beginner"` / `"intermediate"` / …) shown as a chip.
    pub difficulty: String,
    /// The orchestrator, as an authored asset path (`lunco://…`, `twin://…`) —
    /// resolved at launch by [`CurriculumRoot::read`].
    pub script: String,
    /// The world this lesson teaches in, from its `payload` arc, or `None` when
    /// the lesson DECLARES it has no world (a UI tour). The launcher mounts it
    /// before running the script; absent is a statement, not a missing value.
    pub world: Option<String>,
    /// Auto-launch this tutorial once on the user's first run (persisted via
    /// [`TutorialSeen::onboarded`]). At most one lesson per app should set it —
    /// the onboarding entry point.
    pub first_start: bool,
    /// The prim path of the lesson to chain to on completion
    /// (`MISSION_COMPLETE`). `None` = the chain ends here.
    pub next: Option<String>,
    /// Which root contributed this lesson. Provenance, stamped at registration —
    /// never authored. It is what [`script`](Self::script) resolves against, so a
    /// twin's lesson and a bundled one load by the same rule.
    pub source: CurriculumSource,
}

/// One TRACK's presentation, as composed.
///
/// Both fields are COMPOSITION facts, never authored. An app offers a track by
/// sublayering it (`assets/tutorials/<app>.usda`), so the layer stack answers
/// both "which tracks" and "in what order"; a track shown somewhere else is
/// composed somewhere else.
#[derive(Clone, Debug)]
pub struct TrackMeta {
    /// Heading shown for this track in the 🎓 menu (`lunco:track:label`).
    pub label: String,
    /// Position in the composed stack — derived from the order tracks were read,
    /// never authored. Ascending.
    pub order: usize,
}

/// Telemetry event name published when a curriculum or lesson failure would
/// otherwise live only in the log: an unopenable curriculum layer, a
/// composition error, a lesson with no script, an unknown tutorial id, a
/// missing lesson source, a lesson abandoned mid-run. The payload is a
/// human-readable string naming the tutorial/lesson and the cause.
///
/// Published at [`Severity::Error`] so the workbench status bar's error
/// telemetry observer surfaces it — the same arrangement
/// `lunco_usd_bevy::SCENE_LOAD_FAILED` uses. A student who clicks a lesson and
/// sees nothing happen must be told WHY in the UI, not only in a terminal they
/// are not watching.
pub const TUTORIAL_FAILED: &str = "TUTORIAL_FAILED";

/// The one shape every [`TUTORIAL_FAILED`] publication takes, so the payload
/// convention (a cause string naming the lesson) cannot drift between sites.
fn tutorial_failed(detail: impl Into<String>) -> TelemetryEvent {
    TelemetryEvent {
        name: TUTORIAL_FAILED.into(),
        source: 0,
        severity: Severity::Error,
        data: TelemetryValue::String(detail.into()),
        timestamp: 0.0,
    }
}

// ── Curriculum roots: the one extension point ───────────────────────────────

/// Who provided a curriculum. Carried on every registered lesson so its script
/// resolves against the root that declared it, and so dropping a provider drops
/// exactly its lessons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CurriculumSource {
    /// Shipped in `assets/tutorials/` (embedded on wasm).
    #[default]
    Bundled,
    /// Contributed by an open twin — arrives when it opens, leaves when it closes.
    Twin(lunco_workspace::TwinId),
}

/// A place lessons come from.
///
/// **This engine ships no lessons.** Every lesson arrives through a root that
/// somebody registered: the app registers [`Bundled`](CurriculumSource::Bundled)
/// at startup, and an open twin registers its own. A future provider — a
/// downloaded pack, a classroom server — is one more entry in
/// [`CurriculumRoots`] and needs no code here, which is what makes the
/// curriculum an extension rather than a subsystem.
///
/// ONE loader serves every root ([`CurriculumRoot::load_into`]), so there is one
/// answer to what a track is and no way for two providers to drift apart.
#[derive(Clone, Debug)]
pub struct CurriculumRoot {
    /// Provenance — what gets dropped when this provider goes away.
    pub source: CurriculumSource,
    /// This root's curriculum LAYER. Opening it composes whatever it sublayers,
    /// so one path is the whole contribution however many tracks it offers.
    pub layer: std::path::PathBuf,
    /// The twin directory `twin://` paths resolve against; `None` for bundled
    /// (which reads through [`lunco_assets`], the owner of the
    /// disk-vs-embedded policy).
    pub base: Option<std::path::PathBuf>,
}

/// Where a twin publishes its curriculum, relative to the twin root — one layer,
/// which may sublayer as many tracks as the twin likes.
const TWIN_CURRICULUM_LAYER: &str = "sim/tutorials/curriculum.usda";

impl CurriculumRoot {
    /// The bundled root — the app's own layer, `assets/tutorials/<app>.usda`.
    /// That layer is where the app declares which tracks it offers.
    pub fn bundled(app: &str) -> Self {
        Self {
            source: CurriculumSource::Bundled,
            // `assets_dir_abs`, never a bare `"assets"` join: a relative join
            // follows the CWD of whoever launched the process, and a packaged
            // binary carries `assets/` beside the executable instead.
            layer: lunco_assets::assets_dir_abs()
                .join("tutorials")
                .join(format!("{app}.usda")),
            base: None,
        }
    }

    /// A twin's root, based at the twin directory.
    pub fn twin(id: lunco_workspace::TwinId, root: std::path::PathBuf) -> Self {
        Self {
            layer: root.join(TWIN_CURRICULUM_LAYER),
            source: CurriculumSource::Twin(id),
            base: Some(root),
        }
    }

    /// Read an authored asset path through THIS root.
    ///
    /// `lunco://` goes through [`lunco_assets::tutorials::tutorial_source`] —
    /// on-disk first (so a lesson edit replays with no rebuild), embedded
    /// fallback (a packaged binary, wasm). `twin://<Name>/…` resolves against the
    /// root's own twin directory: the name is not looked up, because provenance
    /// already decided which twin is asking — that is what stopped a bundled
    /// lesson and a twin's from shadowing each other by relative path.
    pub fn read(&self, asset: &str) -> Option<String> {
        let (scheme, rest) = asset.split_once("://")?;
        match scheme {
            "lunco" => lunco_assets::tutorials::tutorial_source(rest.strip_prefix("tutorials/")?),
            "twin" => {
                let (_twin, rel) = rest.split_once('/')?;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    lunco_assets::read_asset_file_string(&self.base.as_ref()?.join(rel)).ok()
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = (self, _twin, rel);
                    // This synchronous curriculum reader has no browser byte
                    // source. Twin lessons on wasm must arrive through the
                    // async AssetServer/UsdLoader projection, just like their
                    // curriculum layer; do not pretend a filesystem read is a
                    // portable fallback.
                    None
                }
            }
            _ => None,
        }
    }

    /// Compose this root's layer and register everything it contributes.
    /// Returns the number of tracks added; user-actionable composition
    /// failures are appended to `failures` for the caller to surface.
    fn load_into(&self, registry: &mut TutorialRegistry, failures: &mut Vec<String>) -> usize {
        // A root with no curriculum layer is normal, not an error: most twins
        // ship none, and the app layer is optional for an app with no lessons.
        if !self.layer.is_file() {
            return 0;
        }
        let composed = match lunco_usd_compose::compose_file_to_stage_with_roots(
            &self.layer,
            Some(lunco_assets::assets_dir_abs().as_path()),
            self.base.as_deref(),
        ) {
            Ok(stage) => curriculum::project(&stage),
            Err(error) => {
                let layer = self.layer.display();
                warn!("[tutorial] curriculum layer '{layer}' did not compose: {error}");
                failures.push(format!(
                    "curriculum layer '{layer}' did not compose: {error}"
                ));
                return 0;
            }
        };
        failures.extend(composed.failures);
        let tracks = composed.tracks.len();
        for track in composed.tracks {
            // Keyed by the track's PRIM PATH — the same key each lesson carries
            // in `app`, and the only name a track has, so a heading always lands
            // on its own group.
            let order = registry.tracks.len();
            registry.tracks.insert(
                track.path,
                TrackMeta {
                    label: track.label,
                    order,
                },
            );
        }
        for lesson in composed.lessons {
            registry.register_tutorial(TutorialMeta {
                id: lesson.path,
                app: lesson.track,
                title: lesson.title,
                blurb: lesson.blurb,
                difficulty: lesson.difficulty,
                script: lesson.script,
                world: lesson.world,
                first_start: lesson.first_start,
                next: lesson.next,
                source: self.source,
            });
        }
        tracks
    }
}

/// Every curriculum provider, in registration order. Push one to contribute
/// lessons; drop one to withdraw them. [`rebuild_curriculum`] republishes the
/// catalog whenever this changes.
#[derive(Resource, Default)]
pub struct CurriculumRoots(pub Vec<CurriculumRoot>);

/// Republish the whole catalog from the registered roots.
///
/// A full rebuild rather than an incremental add/remove: the catalog is a few
/// small layers, and rebuilding makes "what is registered" a pure function of
/// "which roots exist" — so there is no separate unload rule to keep in step
/// with the load rule.
fn rebuild_curriculum(roots: &CurriculumRoots, registry: &mut TutorialRegistry) -> Vec<String> {
    *registry = TutorialRegistry::default();
    let mut failures = Vec::new();
    let mut tracks = 0;
    for root in &roots.0 {
        tracks += root.load_into(registry, &mut failures);
    }
    if tracks == 0 {
        warn!(
            "[tutorial] no tutorial track composed — nothing will appear in the 🎓 \
             menu. A track is a prim applying `LunCoTutorialTrackAPI` in a \
             curriculum layer; an app shows it by sublayering that layer from its \
             own `assets/tutorials/<app>.usda`."
        );
        // Surfaced only when a layer FILE is present and yielded nothing: a
        // host with no curriculum layer made a statement, not a mistake.
        if roots.0.iter().any(|r| r.layer.is_file()) {
            failures.push(
                "no tutorial track composed — a curriculum layer is present but \
                 contributed nothing (see log for the layer path)"
                    .into(),
            );
        }
    } else {
        info!(
            "[tutorial] curriculum: {tracks} track(s), {} lesson(s), from {} root(s)",
            registry.tutorials.len(),
            roots.0.len()
        );
    }
    failures
}

/// The catalog of registered tutorials. Filled by [`TutorialCorePlugin`] from the
/// tracks it discovers, plus anything an app or twin adds via
/// [`TutorialAppExt::register_tutorial`] — this crate ships no built-ins, so the
/// same engine serves every app with only the lessons that app hosts.
#[derive(Resource, Default, Clone)]
pub struct TutorialRegistry {
    pub tutorials: Vec<TutorialMeta>,
    /// Per-track presentation, keyed by track name — for the tracks actually
    /// registered. Menu headings and their order come from here.
    pub tracks: std::collections::HashMap<String, TrackMeta>,
}

impl TutorialRegistry {
    /// Add a tutorial to the catalog (idempotent on `id`).
    pub fn register_tutorial(&mut self, meta: TutorialMeta) {
        if !self.tutorials.iter().any(|t| t.id == meta.id) {
            self.tutorials.push(meta);
        }
    }

    fn get(&self, id: &str) -> Option<TutorialMeta> {
        self.tutorials.iter().find(|t| t.id == id).cloned()
    }

    /// The catalog in **curriculum order**: seed at the onboarding entry
    /// (`first_start`) and follow the `next` chain, then pick up any lesson not
    /// yet reached (a second chain / orphan) in registration order and follow
    /// its chain too. Display code iterates this so a lesson's position is its
    /// place in the chain — independent of the order its manifest lists it in.
    pub fn ordered(&self) -> Vec<&TutorialMeta> {
        let mut out: Vec<&TutorialMeta> = Vec::with_capacity(self.tutorials.len());
        let mut seen = std::collections::HashSet::new();
        // Seeds: the onboarding entry first, then every lesson in registration
        // order — so each not-yet-reached chain head starts its own run.
        let seeds = self
            .tutorials
            .iter()
            .filter(|t| t.first_start)
            .chain(self.tutorials.iter());
        for seed in seeds {
            let mut cur = Some(seed.id.as_str());
            while let Some(id) = cur {
                if !seen.insert(id.to_string()) {
                    break; // already placed (chain re-entry, or seed already run)
                }
                let Some(meta) = self.tutorials.iter().find(|t| t.id == id) else {
                    break; // `next` points at an id that isn't registered
                };
                out.push(meta);
                cur = meta.next.as_deref();
            }
        }
        out
    }
}

/// Register a tutorial at app-build time. Add [`TutorialCorePlugin`] first (it inits
/// the registry), then call this for each lesson.
pub trait TutorialAppExt {
    fn register_tutorial(&mut self, meta: TutorialMeta) -> &mut Self;
}

impl TutorialAppExt for App {
    fn register_tutorial(&mut self, meta: TutorialMeta) -> &mut Self {
        self.world_mut()
            .resource_mut::<TutorialRegistry>()
            .register_tutorial(meta);
        self
    }
}

pub mod curriculum;

/// Persisted tutorial progress, under the `"tutorial_progress"` key of `settings.json`.
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct TutorialProgress {
    /// Ids of tutorials whose mission reported `MISSION_COMPLETE`.
    pub completed: Vec<String>,
    /// The tutorial currently running (set by [`StartTutorial`], cleared on
    /// completion/skip) — so a `MISSION_COMPLETE` is attributed correctly.
    pub current: Option<String>,
    /// When `true`, a finished tutorial chains straight to its [`TutorialMeta::next`]
    /// with no prompt; when `false` (default), completion raises the [`PendingAdvance`]
    /// confirm popup. Toggled from the popup / panel; persisted.
    #[serde(default)]
    pub autoproceed: bool,
}

impl TutorialProgress {
    fn is_completed(&self, id: &str) -> bool {
        self.completed.iter().any(|c| c == id)
    }
}

impl lunco_settings::SettingsSection for TutorialProgress {
    const KEY: &'static str = "tutorial_progress";
}

/// Persisted "first-run onboarding done" flag — read by the boot policy
/// (`boot.rhai`) via the scripting settings verbs, and by [`consult_boot`].
/// Reflect-registered (key `tour_seen`, preserved from the pre-rhai tour) so the
/// rhai side can reach it. The *decision* to onboard lives in the hook; Rust only
/// stores the flag.
#[derive(Resource, Reflect, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
#[reflect(Resource)]
pub struct TutorialSeen {
    /// Whether first-run onboarding has already happened.
    pub onboarded: bool,
}

impl lunco_settings::SettingsSection for TutorialSeen {
    const KEY: &'static str = "tour_seen";
}

/// The current tutorial host. It is recreated for each lesson and destroyed
/// during wind-down, so a lesson's interpreter state cannot cross a scene or
/// tutorial boundary.
#[derive(Resource, Default)]
struct TutorialHost(Option<Entity>);

/// A lesson whose declared world is still being mounted. The script is held
/// until the scene transaction publishes its completion edge.
#[derive(Resource, Default)]
struct PendingTutorialStart(Option<PendingTutorial>);

struct PendingTutorial {
    id: String,
    source: String,
    world: String,
}

/// The active lesson's ownership claim. A declared world is cleared when the
/// lesson is explicitly stopped; a UI-only lesson leaves the current world
/// alone.
#[derive(Resource, Default)]
struct TutorialSession {
    world: Option<String>,
}

/// A completed tutorial is waiting on the user's confirmation before starting its
/// declared successor. `Some(id)` while the [`draw_advance_prompt`] popup shows;
/// cleared on Continue/Stay. Not persisted — transient per-completion.
#[derive(Resource, Default, Clone)]
pub struct PendingAdvance(pub Option<String>);

// ── Commands ────────────────────────────────────────────────────────────────

/// Start a tutorial by id: resolve its authored scenario, mount its declared
/// world if any, and run it on the host after the scene transaction completes.
/// The single launch path — menu, F1, HTTP API, MCP, and other scripts
/// (`cmd("StartTutorial", #{ id })`) all route here.
#[Command(default)]
pub struct StartTutorial {
    /// The [`TutorialMeta::id`] to start.
    pub id: String,
}

/// Stop the current tutorial: clear the HUD, synchronously stop its host, and
/// clear a world declared by that lesson through the normal scene lifecycle.
/// A UI-only lesson leaves an unrelated loaded world alone.
/// `cmd("SkipTutorial")`.
#[Command(default)]
pub struct SkipTutorial {}

/// Enable/disable a simulation subsystem at runtime (progressive fidelity).
/// `name` must be in [`SUBSYSTEMS`]. Rhai: `set_subsystem(name, on)`.
#[Command(default)]
pub struct SetSubsystemEnabled {
    /// Subsystem key from the [`SUBSYSTEMS`] allow-list.
    pub name: String,
    /// `true` enables, `false` disables.
    pub on: bool,
}

/// Drop everything the previous lesson put on screen.
///
/// The ONE place presentation is reset, shared by starting a lesson and stopping
/// one — two callers that must agree on what "the overlay" is.
///
/// `TutorialHud` is OPTIONAL for the same reason `on_skip_tutorial` takes it
/// optionally: it belongs to `lunco_workbench`'s overlay plugin, and a host can
/// run lessons without one (a headless sim driver, and every test of this
/// crate's execution core). Required, this would panic an app whose only sin was
/// not drawing a HUD.
#[cfg(feature = "ui")]
fn clear_tutorial_hud(world: &mut World) {
    reset_hud(world.get_resource_mut::<TutorialHud>());
}

/// WHAT "the overlay" is — the single field list, so the two callers cannot
/// drift. Generic over the smart pointer because one caller holds a `ResMut`
/// (an observer) and the other a `Mut` (an exclusive world closure); both deref
/// to the HUD.
#[cfg(feature = "ui")]
fn reset_hud<H: std::ops::DerefMut<Target = TutorialHud>>(hud: Option<H>) {
    let Some(mut hud) = hud else { return };
    hud.hint.clear();
    hud.objectives.clear();
    hud.spotlight = None;
    hud.tour = None;
}

/// Headless hosts have no presentation to reset.
#[cfg(not(feature = "ui"))]
fn clear_tutorial_hud(_world: &mut World) {}

/// Spawn (once) and return the host entity that tutorial scenarios attach to.
fn ensure_host(world: &mut World) -> Entity {
    if let Some(e) = world.resource::<TutorialHost>().0 {
        return e;
    }
    let e = world.spawn(Name::new("TutorialHost")).id();
    world.resource_mut::<TutorialHost>().0 = Some(e);
    e
}

/// Stop and destroy the current tutorial host synchronously.
fn stop_tutorial_host(world: &mut World) {
    let Some(host) = world.resource_mut::<TutorialHost>().0.take() else {
        return;
    };
    lunco_scripting::scenario::ScenarioDriver::<
        lunco_scripting::world_bridge::RhaiScenarioRuntime,
    >::stop_entity(world, host);
    if let Ok(entity) = world.get_entity_mut(host) {
        entity.despawn();
    }
}

fn start_tutorial_scenario(
    world: &mut World,
    id: String,
    source: String,
    world_path: Option<String>,
) {
    let host = ensure_host(world);
    info!("[tutorial] starting '{}'", id);
    world.trigger(lunco_scripting::commands::RunScenario {
        target: host,
        source,
        params: String::new(),
    });
    world.resource_mut::<TutorialProgress>().current = Some(id);
    world.resource_mut::<TutorialSession>().world = world_path;
    if let Some(mut seen) = world.get_resource_mut::<TutorialSeen>() {
        seen.onboarded = true;
    }
}

#[on_command(StartTutorial)]
fn on_start_tutorial(trigger: On<StartTutorial>, mut commands: Commands) {
    let id = trigger.event().id.clone();
    // Metadata and source resolution happen before wind-down. An invalid
    // request must not destroy a currently running lesson.
    commands.queue(move |world: &mut World| {
        let Some(meta) = world.resource::<TutorialRegistry>().get(&id) else {
            warn!("[tutorial] StartTutorial: unknown id '{}'", id);
            world.trigger(tutorial_failed(format!(
                "StartTutorial: unknown id '{}' — not in the composed curriculum",
                id
            )));
            return;
        };
        let Some(root) = world
            .resource::<CurriculumRoots>()
            .0
            .iter()
            .find(|root| root.source == meta.source)
            .cloned()
        else {
            warn!(
                "[tutorial] '{}' came from a root that is no longer mounted",
                id
            );
            world.trigger(tutorial_failed(format!(
                "'{}' came from a curriculum root that is no longer mounted",
                id
            )));
            return;
        };
        let Some(source) = root.read(&meta.script) else {
            warn!("[tutorial] no source for '{}' ({})", id, meta.script);
            world.trigger(tutorial_failed(format!(
                "no lesson source for '{}' ({})",
                id, meta.script
            )));
            return;
        };

        // The outgoing lesson owns its declared world until this boundary. A
        // replacement lesson that has no world must explicitly release that
        // ownership; otherwise a UI-only lesson would leave the previous
        // lesson's simulation alive under a new tutorial host.
        let outgoing_world = world
            .resource::<TutorialSession>()
            .world
            .clone()
            .or_else(|| {
                world
                    .resource::<PendingTutorialStart>()
                    .0
                    .as_ref()
                    .map(|pending| pending.world.clone())
            });

        clear_tutorial_hud(world);
        world.resource_mut::<PendingAdvance>().0 = None;
        stop_tutorial_host(world);
        world.resource_mut::<TutorialProgress>().current = None;
        world.resource_mut::<TutorialSession>().world = None;
        world.resource_mut::<PendingTutorialStart>().0 = None;

        if let Some(scene) = meta.world.clone() {
            info!("[tutorial] '{}' declares world {}", meta.title, scene);
            world.resource_mut::<PendingTutorialStart>().0 = Some(PendingTutorial {
                id: id.clone(),
                source,
                world: scene.clone(),
            });
            world.trigger(lunco_api::ApiCommandEvent {
                command: "LoadScene".to_string(),
                params: serde_json::json!({ "path": scene }),
                id: 0,
            });
        } else {
            if outgoing_world.is_some() {
                world.trigger(lunco_api::ApiCommandEvent {
                    command: "ClearScene".to_string(),
                    params: serde_json::Value::Null,
                    id: 0,
                });
            }
            start_tutorial_scenario(world, id, source, None);
        }
    });
}

/// Headless runs have no presentation state to clear, but stopping a lesson
/// must retain the same execution semantics as the UI command.
#[on_command(SkipTutorial)]
fn on_skip_tutorial(_t: On<SkipTutorial>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        let clear_world = world.resource::<TutorialSession>().world.is_some()
            || world.resource::<PendingTutorialStart>().0.is_some();
        clear_tutorial_hud(world);
        stop_tutorial_host(world);
        world.resource_mut::<TutorialProgress>().current = None;
        world.resource_mut::<PendingAdvance>().0 = None;
        world.resource_mut::<PendingTutorialStart>().0 = None;
        world.resource_mut::<TutorialSession>().world = None;
        if clear_world {
            // A tutorial that declared a world owns that world. Stopping it
            // therefore winds the viewport down through the normal scene
            // command, while a UI-only tour deliberately leaves the world.
            world.trigger(lunco_api::ApiCommandEvent {
                command: "ClearScene".to_string(),
                params: serde_json::Value::Null,
                id: 0,
            });
        }
    });
}

#[on_command(SetSubsystemEnabled)]
fn on_set_subsystem_enabled(
    trigger: On<SetSubsystemEnabled>,
    mut toggles: ResMut<SubsystemToggles>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    if !SubsystemToggles::is_known(&ev.name) {
        warn!(
            "[subsystem] unknown subsystem '{}' (allow-list: {:?}) — ignored",
            ev.name, SUBSYSTEMS
        );
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

register_commands!(
    on_start_tutorial,
    on_skip_tutorial,
    on_set_subsystem_enabled,
);

/// On `MISSION_COMPLETE`, record the completion and advance the chain by starting
/// the current tutorial's [`TutorialMeta::next`] — the chain lives entirely in
/// DATA (each tutorial names its successor's id), so there is no per-tutorial Rust.
fn on_mission_complete(
    trigger: On<TelemetryEvent>,
    registry: Res<TutorialRegistry>,
    mut progress: ResMut<TutorialProgress>,
    mut pending: ResMut<PendingAdvance>,
    mut commands: Commands,
) {
    if trigger.event().name != "MISSION_COMPLETE" {
        return;
    }
    // Attribute the completion to whatever tutorial is running.
    let Some(id) = progress.current.take() else {
        return;
    };
    // Completion ends execution even when the authored world remains visible
    // for review. The host is not allowed to keep ticking against a completed
    // lesson or leak its interpreter state into the next one.
    commands.queue(|world: &mut World| {
        stop_tutorial_host(world);
        world.resource_mut::<TutorialSession>().world = None;
    });
    if !progress.is_completed(&id) {
        info!("[tutorial] completed '{id}'");
        progress.completed.push(id.clone());
    }
    // Successor by id (data chain). None → the chain ends here.
    let Some(next) = registry.get(&id).and_then(|m| m.next) else {
        return;
    };
    if progress.autoproceed {
        info!("[tutorial] auto-advancing → '{next}'");
        commands.trigger(StartTutorial {
            id: next.to_string(),
        });
    } else {
        info!("[tutorial] complete — awaiting confirm to advance → '{next}'");
        pending.0 = Some(next.to_string());
    }
}

/// Wind down an active lesson before any other scene is mounted. The scene
/// lifecycle emits this edge from the authoritative LoadScene command, so raw
/// scene selection and API/script scene loads have the same cleanup semantics.
fn on_scene_load_started(
    trigger: On<TelemetryEvent>,
    mut progress: ResMut<TutorialProgress>,
    mut pending: ResMut<PendingTutorialStart>,
    mut session: ResMut<TutorialSession>,
    mut commands: Commands,
) {
    if trigger.event().name != "SCENE_LOAD_STARTED" {
        return;
    }
    let path = match &trigger.event().data {
        TelemetryValue::String(path) => path,
        _ => return,
    };
    let belongs_to_pending = pending
        .0
        .as_ref()
        .is_some_and(|request| request.world.as_str() == path.as_str());
    if !belongs_to_pending {
        pending.0 = None;
    }
    if progress.current.is_none() && session.world.is_none() {
        return;
    }
    progress.current = None;
    session.world = None;
    commands.queue(|world: &mut World| {
        clear_tutorial_hud(world);
        stop_tutorial_host(world);
    });
}

/// Attach a declared tutorial scenario only after its scene transaction has
/// completed. This closes the old race where the script started while the
/// viewport was still empty or still belonged to the outgoing scene.
fn on_scene_load_completed(trigger: On<TelemetryEvent>, mut commands: Commands) {
    if trigger.event().name != "SCENE_LOAD_COMPLETED" {
        return;
    }
    let TelemetryValue::String(path) = &trigger.event().data else {
        return;
    };
    let path = path.clone();
    commands.queue(move |world: &mut World| {
        let Some(request) = world.resource_mut::<PendingTutorialStart>().0.take() else {
            return;
        };
        if request.world != path {
            world.resource_mut::<PendingTutorialStart>().0 = Some(request);
            return;
        }
        start_tutorial_scenario(world, request.id, request.source, Some(request.world));
    });
}

/// Abandon the running lesson when a scene load fails, so it cannot go on to
/// report success it did not earn.
///
/// A lesson's first act is almost always `load_scene(...)`, and nothing after
/// that checks whether the scene arrived. A coach-mark tour in particular
/// advances on the user pressing Next, so against an empty viewport it walks
/// its whole step list, emits `MISSION_COMPLETE`, records a completion and
/// starts its successor — the observed failure, where a tutorial wrote
/// `MISSION_COMPLETE` to the black box with no scene loaded at all. An
/// automated suite reads that as green while it has tested nothing, which is
/// worse than a red run: a red run gets investigated.
///
/// Clearing `current` is what makes the false pass impossible rather than
/// merely unlikely: [`on_mission_complete`] attributes a completion to whatever
/// lesson is running, so with no lesson running a late `MISSION_COMPLETE` from
/// the abandoned script records nothing and advances nothing.
///
/// Deliberately not filtered to "the scene THIS lesson asked for": a lesson has
/// no way to declare that, and any scene failing to mount while a lesson is
/// running means the lesson is not showing what it claims to. The event is
/// published by `lunco_usd_bevy::SCENE_LOAD_FAILED`, matched by name here
/// because the tutorial crate sits above USD and does not depend on it — the
/// same arrangement `MISSION_COMPLETE` already uses in the other direction.
fn on_scene_load_failed(
    trigger: On<TelemetryEvent>,
    mut progress: ResMut<TutorialProgress>,
    mut pending_advance: ResMut<PendingAdvance>,
    mut pending_start: ResMut<PendingTutorialStart>,
    mut session: ResMut<TutorialSession>,
    mut commands: Commands,
) {
    if trigger.event().name != "SCENE_LOAD_FAILED" {
        return;
    }
    let path = match &trigger.event().data {
        TelemetryValue::String(path) => Some(path.as_str()),
        _ => None,
    };
    let matches_pending = path.is_some_and(|path| {
        pending_start
            .0
            .as_ref()
            .is_some_and(|request| request.world == path)
    });
    if matches_pending {
        let request = pending_start.0.take().expect("pending request matched");
        pending_advance.0 = None;
        error!(
            "[tutorial] abandoning '{}' — its scene failed to load ({:?})",
            request.id,
            trigger.event().data
        );
        commands.trigger(tutorial_failed(format!(
            "abandoning '{}' — its scene failed to load ({:?})",
            request.id,
            trigger.event().data
        )));
        return;
    }
    let Some(id) = progress.current.take() else {
        return;
    };
    pending_advance.0 = None;
    session.world = None;
    commands.queue(|world: &mut World| {
        clear_tutorial_hud(world);
        stop_tutorial_host(world);
    });
    error!(
        "[tutorial] abandoning '{}' — its scene failed to load ({:?})",
        id,
        trigger.event().data
    );
    commands.trigger(tutorial_failed(format!(
        "abandoning '{}' — its scene failed to load ({:?})",
        id,
        trigger.event().data
    )));
}

/// A tidy display name for a tutorial id: prefer its registered title, else the id.
#[cfg(feature = "ui")]
fn pretty_tutorial(registry: &TutorialRegistry, id: &str) -> String {
    registry
        .get(id)
        .map(|m| m.title.to_string())
        .unwrap_or_else(|| id.to_string())
}

/// Modal confirm popup shown when a tutorial finishes and a successor is queued
/// (unless [`TutorialProgress::autoproceed`]). Continue starts the next tutorial;
/// Stay dismisses. The checkbox flips `autoproceed`.
#[cfg(feature = "ui")]
fn draw_advance_prompt(
    mut egui_ctx: EguiContexts,
    mut pending: ResMut<PendingAdvance>,
    mut progress: ResMut<TutorialProgress>,
    registry: Res<TutorialRegistry>,
    mut commands: Commands,
) {
    let Some(next) = pending.0.clone() else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    let next_title = pretty_tutorial(&registry, &next);

    let mut proceed = false;
    let mut dismiss = false;
    let screen = ctx.content_rect();
    // Render at `Order::Tooltip` so the prompt paints above every overlay.
    egui::Area::new(egui::Id::new("tutorial_advance_scrim"))
        .order(egui::Order::Tooltip)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            // TODO(theme): migrate to lunco-theme once the token set covers this.
            // Full-screen dim behind the "advance" prompt -> `tokens.scrim`.
            // BLOCKED: `lunco-tutorial` has no `[features]` section, so there is
            // nowhere safe to hang an optional `lunco-theme` dep (it pulls
            // bevy_egui -> bevy_render -> wgpu). See lunco-theme's crate docs.
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
            ui.allocate_rect(screen, egui::Sense::click());
        });
    egui::Area::new(egui::Id::new("tutorial_advance_prompt"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(360.0);
                ui.heading("🎓 Tutorial complete");
                ui.separator();
                ui.label(format!("Continue to “{next_title}”?"));
                ui.add_space(6.0);
                let mut auto = progress.autoproceed;
                if ui
                    .checkbox(&mut auto, "Continue automatically from now on")
                    .on_hover_text("Skip this prompt and chain straight to the next tutorial.")
                    .changed()
                {
                    progress.autoproceed = auto;
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Continue →").clicked() {
                        proceed = true;
                    }
                    if ui.button("Stay here").clicked() {
                        dismiss = true;
                    }
                });
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        dismiss = true;
    }

    if proceed {
        commands.trigger(StartTutorial { id: next });
        pending.0 = None;
    } else if dismiss {
        pending.0 = None;
    }
}

/// Keybinding → intent → command: `lunco-doc-bevy` maps `F1` to
/// [`EditorIntent::ShowTutorial`]; this turns that intent into a [`StartTutorial`]
/// for the app's onboarding (`first_start`) tutorial — or the first registered
/// one if none is flagged.
fn resolve_show_tutorial_intent(
    trigger: On<EditorIntent>,
    registry: Res<TutorialRegistry>,
    mut commands: Commands,
) {
    if !matches!(*trigger.event(), EditorIntent::ShowTutorial) {
        return;
    }
    let id = registry
        .tutorials
        .iter()
        .find(|t| t.first_start)
        .or_else(|| registry.tutorials.first())
        .map(|t| t.id.clone());
    if let Some(id) = id {
        commands.trigger(StartTutorial { id: id.to_string() });
    }
}

/// A perspective help popup's "🎓 Show Tour" button publishes a
/// [`HelpTourRequest`](lunco_workbench::HelpTourRequest). Consume it → start the
/// app's onboarding (`first_start`) tutorial. Works for any app/perspective.
#[cfg(feature = "ui")]
fn consume_tour_request(
    mut req: ResMut<lunco_workbench::HelpTourRequest>,
    registry: Res<TutorialRegistry>,
    mut commands: Commands,
) {
    if req.0.is_none() {
        return;
    }
    let id = registry
        .tutorials
        .iter()
        .find(|t| t.first_start)
        .or_else(|| registry.tutorials.first())
        .map(|t| t.id.clone());
    if let Some(id) = id {
        req.0 = None;
        commands.trigger(StartTutorial { id: id.to_string() });
    }
}

/// Read argv for the boot ctx (rhai can't). Returns `(has_scene_arg, automated)`.
fn boot_env() -> (bool, bool) {
    let (mut has_scene, mut automated) = (false, false);
    for a in std::env::args() {
        match a.as_str() {
            "--scene" => has_scene = true,
            "--api" | "--no-ui" => automated = true,
            _ => {}
        }
    }
    (has_scene, automated)
}

/// [`HookValue`](lunco_hooks::HookValue) → `serde_json::Value`, for a boot
/// directive's `params` (the command dispatcher expects JSON).
fn hookvalue_to_json(v: &lunco_hooks::HookValue) -> serde_json::Value {
    use lunco_hooks::HookValue as H;
    use serde_json::Value as J;
    match v {
        H::Unit => J::Null,
        H::Int(i) => J::from(*i),
        H::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        H::Bool(b) => J::Bool(*b),
        H::Str(s) => J::String(s.clone()),
        H::Array(a) => J::Array(a.iter().map(hookvalue_to_json).collect()),
        H::Map(m) => J::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), hookvalue_to_json(v)))
                .collect(),
        ),
    }
}

/// Consult the **boot-entry policy** ([`BOOT_HOOK`](lunco_core::session::BOOT_HOOK),
/// authored in `boot.rhai`) and dispatch its `#{ command, params }` directive, if
/// any. Returns `true` when the policy TOOK OVER (a command was dispatched) — the
/// caller then skips its own default load; `false` = "load your default."
///
/// This is the single startup-decision seam: onboarding, "load the tutorial not
/// the default scene," resume, deep-links — all live in the rhai policy, not here.
/// Rust only marshals the context Rust alone can see (argv, first-run flag) and
/// dispatches generically. Marks onboarding done on take-over, so a repeat consult
/// (e.g. the shared [`boot_seam`] after a sandbox's own `consult_boot`) no-ops.
pub fn consult_boot(world: &mut World, has_scene_arg: bool, automated: bool) -> bool {
    use lunco_hooks::HookValue as H;
    let onboarded = world
        .get_resource::<TutorialSeen>()
        .map(|s| s.onboarded)
        .unwrap_or(false);
    let first_start_id = world.get_resource::<TutorialRegistry>().and_then(|r| {
        r.tutorials
            .iter()
            .find(|t| t.first_start)
            .map(|t| t.id.to_string())
    });
    let mut ctx = vec![
        ("onboarded".to_string(), H::Bool(onboarded)),
        ("has_scene_arg".to_string(), H::Bool(has_scene_arg)),
        ("automated".to_string(), H::Bool(automated)),
    ];
    if let Some(id) = &first_start_id {
        ctx.push(("first_start_id".to_string(), H::Str(id.clone())));
    }
    let out = match lunco_hooks::invoke(lunco_core::session::BOOT_HOOK, &[H::Map(ctx)]) {
        Some(Ok(v)) => v,
        _ => return false, // no hook / policy fault → the app loads its default
    };
    let Some(command) = out.get("command").and_then(|c| c.as_str()) else {
        return false; // policy returned () → "do nothing", app loads its default
    };
    let params = out
        .get("params")
        .map(hookvalue_to_json)
        .unwrap_or(serde_json::Value::Object(Default::default()));
    info!("[tutorial] boot policy → {command}");
    world.trigger(lunco_api::ApiCommandEvent {
        command: command.to_string(),
        params,
        id: 0,
    });
    if let Some(mut s) = world.get_resource_mut::<TutorialSeen>() {
        s.onboarded = true;
    }
    true
}

/// Startup boot seam for apps with **no** Startup scene load of their own (e.g.
/// lunica): once, on the first frame, consult the boot policy. Apps that DO load a
/// scene at Startup (the sandbox) call [`consult_boot`] there instead and skip
/// their default load on take-over; this then no-ops (onboarding already marked).
fn boot_seam(world: &mut World, mut done: Local<bool>) {
    if *done {
        return;
    }
    *done = true;
    let (has_scene, automated) = boot_env();
    consult_boot(world, has_scene, automated);
}

// ── Twin-provided curriculum ────────────────────────────────────────────────

/// **A twin brings its own lessons.** Keep the twin's [`CurriculumRoot`] in step
/// with the active twin, and republish the catalog when it changes.
///
/// All this system does is add and remove a root — it holds no opinion about
/// what a track is, where a layer lives, or which lessons belong to whom. A
/// twin's curriculum is not a second kind of curriculum and gets no second set
/// of rules.
///
/// A system, not a draw callback: mounting a curriculum is a load-time concern,
/// so a twin's lessons are there whether or not anyone opens the 🎓 menu.
fn sync_twin_curriculum_root(
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut roots: ResMut<CurriculumRoots>,
    mut registry: ResMut<TutorialRegistry>,
    mut commands: Commands,
) {
    let active = workspace.as_ref().and_then(|ws| ws.0.active_twin);
    let mounted = roots.0.iter().find_map(|r| match r.source {
        CurriculumSource::Twin(id) => Some(id),
        CurriculumSource::Bundled => None,
    });
    if mounted == active {
        return;
    }
    roots
        .0
        .retain(|r| !matches!(r.source, CurriculumSource::Twin(_)));
    if let Some((ws, id)) = workspace.as_ref().zip(active) {
        if let Some(twin) = ws.0.twin(id) {
            roots.0.push(CurriculumRoot::twin(id, twin.root.clone()));
        }
    }
    for failure in rebuild_curriculum(&roots, &mut registry) {
        commands.trigger(tutorial_failed(failure));
    }
}

// ── Menu + launcher panel ───────────────────────────────────────────────────

/// Register the top-level **🎓 Tutorials** menu, listing the app's tutorials with
/// a completion tick; clicking starts one. Shared by every workbench app.
#[cfg(feature = "ui")]
fn register_tutorials_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_custom_menu("🎓 Tutorials", |ui, ctx| {
        let registry = ctx
            .resource::<TutorialRegistry>()
            .cloned()
            .unwrap_or_default();
        let progress = ctx
            .resource::<TutorialProgress>()
            .cloned()
            .unwrap_or_default();
        if registry.tutorials.is_empty() {
            ui.label(
                egui::RichText::new("(no tutorials registered)")
                    .weak()
                    .italics(),
            );
            return;
        }
        ui.label(
            egui::RichText::new("Interactive, scripted lessons")
                .weak()
                .small(),
        );
        ui.separator();

        let mut grouped: std::collections::HashMap<String, Vec<&TutorialMeta>> =
            std::collections::HashMap::new();
        for meta in registry.ordered() {
            grouped.entry(meta.app.clone()).or_default().push(meta);
        }

        // Heading + order per track come from the composed curriculum
        // (`registry.tracks`), so a new track brings its own presentation and this
        // menu never learns any track's name. A group with no metadata renders
        // under its own prim path and sorts after everything composed.
        let mut groups: Vec<(&String, &Vec<&TutorialMeta>)> = grouped.iter().collect();
        groups.sort_by_key(|(app_key, _)| {
            (
                registry
                    .tracks
                    .get(app_key.as_str())
                    .map(|t| t.order)
                    .unwrap_or(usize::MAX),
                (*app_key).clone(),
            )
        });

        for (app_key, metas) in groups {
            let label = registry
                .tracks
                .get(app_key.as_str())
                .map(|t| t.label.clone())
                .unwrap_or_else(|| app_key.clone());
            ui.menu_button(label, |ui| {
                for meta in metas {
                    let done = progress.is_completed(&meta.id);
                    let glyph = if done { "✓" } else { "🎓" };
                    if ui
                        .button(format!("{glyph}  {}", meta.title))
                        .on_hover_text(meta.blurb.as_str())
                        .clicked()
                    {
                        ctx.trigger(StartTutorial {
                            id: meta.id.to_string(),
                        });
                        ui.close();
                    }
                }
            });
        }

        ui.separator();
        ui.add_enabled_ui(progress.current.is_some(), |ui| {
            if ui.button("⏹ Stop tutorial").clicked() {
                ctx.trigger(SkipTutorial {});
                ui.close();
            }
        });
    });
}

/// Panel id for the tutorials launcher.
#[cfg(feature = "ui")]
pub const TUTORIALS_PANEL_ID: PanelId = PanelId("tutorials");

/// Dockable launcher: lists registered tutorials with a completion tick and a
/// Start button; offers Stop while one is running.
#[cfg(feature = "ui")]
pub struct TutorialsPanel;

#[cfg(feature = "ui")]
impl Panel for TutorialsPanel {
    fn id(&self) -> PanelId {
        TUTORIALS_PANEL_ID
    }
    fn title(&self) -> String {
        "Tutorials".to_string()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Tools
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let registry = ctx
            .resource::<TutorialRegistry>()
            .cloned()
            .unwrap_or_default();
        let progress = ctx
            .resource::<TutorialProgress>()
            .cloned()
            .unwrap_or_default();

        ui.add_space(4.0);
        ui.heading("🎓 Tutorials");
        ui.label(
            egui::RichText::new("Interactive, scripted lessons.")
                .weak()
                .small(),
        );

        let mut auto = progress.autoproceed;
        if ui
            .checkbox(&mut auto, "Auto-continue to next tutorial")
            .on_hover_text("When off, a popup asks before starting each next tutorial.")
            .changed()
        {
            ctx.resource_scope::<TutorialProgress, ()>(|_ctx, p| p.autoproceed = auto);
        }
        ui.separator();

        if registry.tutorials.is_empty() {
            ui.label(egui::RichText::new("No tutorials registered.").weak());
            return;
        }

        if let Some(cur) = &progress.current {
            let title = registry
                .get(cur)
                .map(|m| m.title.to_string())
                .unwrap_or_else(|| cur.clone());
            ui.horizontal(|ui| {
                // TODO(theme): migrate to lunco-theme once the token set covers this.
                // "Currently running" accent for the launcher row. Blocked on the
                // dep, as above.
                ui.label(
                    egui::RichText::new(format!("▶ Running: {title}"))
                        .color(egui::Color32::from_rgb(120, 200, 255)),
                );
                if ui.small_button("Stop").clicked() {
                    ctx.trigger(SkipTutorial {});
                }
            });
            ui.separator();
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for meta in registry.ordered() {
                let done = progress.is_completed(&meta.id);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if done {
                            // TODO(theme): migrate to lunco-theme once the token set covers this.
                            // Completed-tutorial tick -> `tokens.success`. Blocked on the dep.
                            ui.label(
                                egui::RichText::new("✓")
                                    .color(egui::Color32::from_rgb(120, 210, 140))
                                    .strong(),
                            );
                        }
                        ui.label(egui::RichText::new(meta.title.as_str()).strong());
                        ui.label(egui::RichText::new(meta.difficulty.as_str()).weak().small());
                    });
                    ui.label(egui::RichText::new(meta.blurb.as_str()).small());
                    ui.horizontal(|ui| {
                        let label = if done { "Replay" } else { "Start" };
                        if ui.button(label).clicked() {
                            ctx.trigger(StartTutorial {
                                id: meta.id.to_string(),
                            });
                        }
                        ui.label(
                            egui::RichText::new(format!("· {}", meta.app))
                                .weak()
                                .small(),
                        );
                    });
                });
                ui.add_space(4.0);
            }
        });
    }
}

/// Headless-safe tutorial execution: registry, source loading, typed commands,
/// completion chaining, boot policy, and twin curriculum discovery. Tutorials
/// are composed from `assets/tutorials/<app>.usda`.
pub struct TutorialCorePlugin {
    /// App name — selects the curriculum layer `assets/tutorials/<app>.usda`.
    pub app: String,
}

impl Plugin for TutorialCorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TutorialRegistry>();
        // The app contributes the bundled root; a twin adds its own when it opens
        // (`sync_twin_curriculum_root`). Both go through the SAME loader, so the
        // engine names no track and knows no lesson.
        let mut roots = CurriculumRoots(vec![CurriculumRoot::bundled(&self.app)]);
        let mut registry = TutorialRegistry::default();
        // Failures found while BUILDING cannot be triggered here: other
        // plugins' observers (the status bar's error-telemetry one among them)
        // may not be installed yet. Parked and surfaced at Startup instead.
        let failures = rebuild_curriculum(&roots, &mut registry);
        app.insert_resource(BootCurriculumFailures(failures));
        // `roots` is moved in whole so a provider added later (a pack, a
        // classroom server) is a push here and needs no code in this crate.
        roots.0.shrink_to_fit();
        app.insert_resource(roots);
        app.insert_resource(registry);
        app.init_resource::<TutorialHost>();
        app.init_resource::<PendingTutorialStart>();
        app.init_resource::<TutorialSession>();
        app.init_resource::<PendingAdvance>();
        app.register_settings_section::<TutorialProgress>();
        app.register_type::<TutorialSeen>();
        app.register_settings_section::<TutorialSeen>();
        register_all_commands(app);
        app.add_observer(on_mission_complete);
        app.add_observer(on_scene_load_started);
        app.add_observer(on_scene_load_completed);
        app.add_observer(on_scene_load_failed);
        app.add_observer(resolve_show_tutorial_intent);
        app.add_systems(Startup, surface_boot_curriculum_failures);
        app.add_systems(Update, sync_twin_curriculum_root);
        app.add_systems(Update, boot_seam);
    }
}

/// Curriculum failures met while [`TutorialCorePlugin`] was building, parked
/// until Startup so every observer that wants them (the status bar's) exists.
#[derive(Resource, Default)]
struct BootCurriculumFailures(Vec<String>);

/// Publish the parked boot-time curriculum failures as [`TUTORIAL_FAILED`].
fn surface_boot_curriculum_failures(
    mut failures: ResMut<BootCurriculumFailures>,
    mut commands: Commands,
) {
    for failure in failures.0.drain(..) {
        commands.trigger(tutorial_failed(failure));
    }
}

/// Optional UI projection for [`TutorialCorePlugin`]: menu, launcher panel,
/// HUD cleanup, and the completion confirmation popup.
///
/// ```ignore
/// app.add_plugins(lunco_tutorial::TutorialPlugin { app: "sandbox".into() });
/// ```
#[cfg(feature = "ui")]
pub struct TutorialPlugin {
    /// App name — selects the curriculum layer `assets/tutorials/<app>.usda`.
    pub app: String,
}

#[cfg(feature = "ui")]
impl Plugin for TutorialPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TutorialCorePlugin>() {
            app.add_plugins(TutorialCorePlugin {
                app: self.app.clone(),
            });
        }
        app.add_systems(Startup, register_tutorials_menu);
        app.add_systems(Update, consume_tour_request);
        app.add_systems(EguiPrimaryContextPass, draw_advance_prompt);
        app.register_panel(TutorialsPanel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The execution core runs and stops a lesson with no UI plugin present.
    ///
    /// The lesson is registered HERE rather than taken from the shipped
    /// curriculum: a curriculum layer is composed from disk (`assets_dir_abs`),
    /// and a unit test's working directory is its own crate, so depending on the
    /// bundled catalog would make this a test of where cargo happens to put the
    /// CWD. What it means to test is start → `current`, stop → cleared.
    #[test]
    fn core_executes_and_stops_a_lesson_without_the_ui_plugin() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TutorialCorePlugin {
                app: "sandbox".into(),
            });
        app.register_tutorial(TutorialMeta {
            id: "/Test/Lesson".into(),
            title: "Test".into(),
            blurb: String::new(),
            app: "/Test".into(),
            difficulty: String::new(),
            // A real shipped script, so it resolves through the EMBEDDED copy
            // wherever this runs from. No world: a lesson may decline one.
            script: "lunco://tutorials/sandbox/first_drive.rhai".into(),
            world: None,
            first_start: false,
            next: None,
            source: CurriculumSource::Bundled,
        });

        app.world_mut().trigger(StartTutorial {
            id: "/Test/Lesson".into(),
        });
        app.update();
        assert_eq!(
            app.world()
                .resource::<TutorialProgress>()
                .current
                .as_deref(),
            Some("/Test/Lesson")
        );

        app.world_mut().trigger(SkipTutorial {});
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
    }

    /// A world-backed lesson is a transaction: it requests the world first,
    /// starts only on the completion edge, and cancelling while it is pending
    /// clears the transaction through the normal scene command.
    #[test]
    fn world_lesson_waits_for_mount_and_cancels_cleanly() {
        #[derive(Resource, Default)]
        struct CommandsSeen(Vec<String>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TutorialCorePlugin {
                app: "sandbox".into(),
            });
        app.register_tutorial(TutorialMeta {
            id: "/Test/WorldLesson".into(),
            title: "World test".into(),
            blurb: String::new(),
            app: "/Test".into(),
            difficulty: String::new(),
            script: "lunco://tutorials/sandbox/first_drive.rhai".into(),
            world: Some("lunco://tutorials/sandbox/first_drive.usda".into()),
            first_start: false,
            next: None,
            source: CurriculumSource::Bundled,
        });
        app.insert_resource(CommandsSeen::default());
        app.add_observer(
            |trigger: On<lunco_api::ApiCommandEvent>, mut seen: ResMut<CommandsSeen>| {
                seen.0.push(trigger.event().command.clone());
            },
        );

        app.world_mut().trigger(StartTutorial {
            id: "/Test/WorldLesson".into(),
        });
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
        assert_eq!(app.world().resource::<CommandsSeen>().0, vec!["LoadScene"]);

        app.world_mut().trigger(SkipTutorial {});
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
        assert!(app
            .world()
            .resource::<CommandsSeen>()
            .0
            .ends_with(&["LoadScene".into(), "ClearScene".into()]));
    }

    /// Starting an unknown tutorial id must publish [`TUTORIAL_FAILED`] — the
    /// status bar surfaces every `Severity::Error` telemetry event, so this is
    /// exactly the contract that puts the failure in front of the user instead
    /// of leaving it as a `warn!` in a terminal nobody is watching.
    #[test]
    fn unknown_tutorial_id_publishes_tutorial_failed() {
        #[derive(Resource, Default)]
        struct Seen(Vec<String>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TutorialCorePlugin {
                app: "sandbox".into(),
            });
        app.init_resource::<Seen>();
        app.add_observer(|trigger: On<TelemetryEvent>, mut seen: ResMut<Seen>| {
            let ev = trigger.event();
            if ev.name == TUTORIAL_FAILED {
                assert_eq!(ev.severity, Severity::Error);
                if let TelemetryValue::String(s) = &ev.data {
                    seen.0.push(s.clone());
                }
            }
        });

        app.world_mut().trigger(StartTutorial {
            id: "/No/Such/Lesson".into(),
        });
        app.update();

        let seen = &app.world().resource::<Seen>().0;
        assert!(
            seen.iter().any(|s| s.contains("/No/Such/Lesson")),
            "TUTORIAL_FAILED naming the unknown id was not published; saw {seen:?}"
        );
    }
}
