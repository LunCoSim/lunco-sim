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
use lunco_core::subsystems::SubsystemToggles;
use lunco_core::{
    on_command, register_commands, Command, Severity, TelemetryEvent, TelemetryValue,
};
use lunco_doc_bevy::EditorIntent;
use lunco_settings::AppSettingsExt;
#[cfg(feature = "ui")]
use lunco_workbench::tutorial_overlay::{TutorialHud, TutorialStopRequested};
#[cfg(feature = "ui")]
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot, WorkbenchAppExt, WorkbenchLayout};
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

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
    /// Whether this is guided reference content or a simulator-verified exercise.
    pub format: curriculum::LessonFormat,
    /// The orchestrator, as an authored asset path (`lunco://…`, `twin://…`) —
    /// resolved at launch by [`CurriculumRoot::read`].
    pub script: String,
    /// The world this lesson teaches in, from its `payload` arc, or `None` when
    /// the lesson DECLARES it has no world (a UI tour). The launcher mounts it
    /// before running the script; absent is a statement, not a missing value.
    pub world: Option<String>,
    /// Auto-launch this tutorial once on the user's first run (persisted via
    /// [`TutorialProgress::onboarded`]). At most one lesson per app should set it —
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
/// telemetry observer surfaces it. A student who clicks a lesson and
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
        // URI identity is slash-based on every platform. Twin-authored USD
        // can arrive from a Windows editor with backslashes in both the
        // scheme remainder and the twin/relative separator; normalize before
        // splitting so the same curriculum composes on native Windows and
        // Linux.
        let asset = lunco_assets::asset_path::slashed(asset);
        let (scheme, rest) = asset.split_once("://")?;
        match scheme {
            "lunco" => lunco_assets::tutorials::tutorial_source(rest.strip_prefix("tutorials/")?),
            "twin" => {
                let (_twin, rel) = rest.split_once('/')?;
                if !lunco_assets::asset_path::is_safe_relative_path(rel) {
                    warn!("[tutorial] rejecting unsafe twin lesson source path: {asset:?}");
                    return None;
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let root = self.base.as_ref()?;
                    let path = lunco_assets::existing_path_within_root(root, Path::new(rel))?;
                    lunco_assets::read_asset_file_string(&path).ok()
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
        // USD exposes composed top-level children in strongest-to-weakest
        // composition order. The authored subLayer list is the learner-facing
        // order, so reverse that projection once here; menu code then only
        // sorts by this derived order and never parses hand-written ordinals.
        for track in composed.tracks.into_iter().rev() {
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
                format: lesson.format,
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
    /// First-run onboarding has been successfully dispatched through the
    /// canonical tutorial launcher. This belongs with tutorial progress; a
    /// second persisted onboarding resource would drift from it.
    #[serde(default)]
    pub onboarded: bool,
    /// The tutorial currently running (set by [`StartTutorial`], cleared on
    /// completion/skip) — so a `MISSION_COMPLETE` is attributed correctly.
    #[serde(skip)]
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
    /// A lesson replacement clears the outgoing owned world before issuing the
    /// new load. This matters when both lessons declare the same stage:
    /// ordinary `LoadScene` intentionally no-ops for an already-mounted
    /// identity.
    clear_before_load: bool,
    elapsed_secs: f32,
}

/// The visible lesson world's ownership claim. A declared world remains owned
/// after mission completion while it stays mounted, so replay and replacement
/// can clear it through the normal scene lifecycle. A UI-only lesson leaves
/// the current world alone.
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

/// Clear persisted completion and first-run state without changing the loaded
/// scene. This is the explicit recovery path for a shared settings file whose
/// tutorial history no longer matches the user's current installation.
#[Command(default)]
pub struct ResetTutorialProgress {}

#[on_command(ResetTutorialProgress)]
fn on_reset_tutorial_progress(
    _trigger: On<ResetTutorialProgress>,
    mut progress: ResMut<TutorialProgress>,
) {
    progress.completed.clear();
    progress.onboarded = false;
    info!("[tutorial] cleared persisted tutorial progress");
}

/// Enable/disable a simulation subsystem at runtime (progressive fidelity).
/// `name` must be registered by the owning subsystem plugin. Rhai:
/// `set_subsystem(name, on)`.
#[Command(default)]
pub struct SetSubsystemEnabled {
    /// Registered subsystem key.
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
    hud.title.clear();
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
    let is_first_start = world
        .resource::<TutorialRegistry>()
        .get(&id)
        .is_some_and(|meta| meta.first_start);
    #[cfg(feature = "ui")]
    if let Some(meta) = world.resource::<TutorialRegistry>().get(&id) {
        if let Some(mut hud) = world.get_resource_mut::<TutorialHud>() {
            hud.title = meta.title;
        }
    }
    info!("[tutorial] starting '{}'", id);
    world.trigger(lunco_scripting::commands::RunScenario {
        target: host,
        source,
        params: String::new(),
    });
    world.resource_mut::<TutorialProgress>().current = Some(id);
    world.resource_mut::<TutorialSession>().world = world_path;
    if is_first_start {
        // Mark first-run onboarding only after the scene transaction (if any)
        // reached this point and the canonical launcher actually attached the
        // scenario. A failed scene never reaches this function.
        world.resource_mut::<TutorialProgress>().onboarded = true;
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

        // A tutorial is a guided presentation, not a continuation of the
        // user's previous editor session. Reset the shared workbench owner at
        // the canonical launch boundary so menu, panel, F1, API, and chained
        // launches all get the same perspective and layout semantics.
        #[cfg(feature = "ui")]
        if let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() {
            layout.reset_to_default_perspective();
        }
        clear_tutorial_hud(world);
        world.resource_mut::<PendingAdvance>().0 = None;
        stop_tutorial_host(world);
        world.resource_mut::<TutorialProgress>().current = None;
        world.resource_mut::<TutorialSession>().world = None;
        world.resource_mut::<PendingTutorialStart>().0 = None;

        if let Some(scene) = meta.world.clone() {
            info!("[tutorial] '{}' declares world {}", meta.title, scene);
            let clear_before_load = outgoing_world.is_some();
            world.resource_mut::<PendingTutorialStart>().0 = Some(PendingTutorial {
                id,
                source,
                world: scene.clone(),
                clear_before_load,
                elapsed_secs: 0.0,
            });
            if clear_before_load {
                // Keep LoadScene's same-stage no-op correct for ordinary scene
                // selection. Tutorial replacement has an explicit lifecycle:
                // clear the owned world, then load from its completion edge.
                world.trigger(lunco_core::SceneTransitionIntent::clear());
            } else {
                world.trigger(lunco_core::SceneTransitionIntent::load(scene, ""));
            }
        } else {
            if outgoing_world.is_some() {
                world.trigger(lunco_core::SceneTransitionIntent::clear());
            }
            start_tutorial_scenario(world, id, source, None);
        }
        #[cfg(feature = "ui")]
        if let Some(mut hud) = world.get_resource_mut::<TutorialHud>() {
            hud.title = meta.title;
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
            world.trigger(lunco_core::SceneTransitionIntent::clear());
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
    if !toggles.is_registered(&ev.name) {
        warn!(
            "[subsystem] unknown subsystem '{}' (registered: {:?}) — ignored",
            ev.name,
            toggles.registered_names()
        );
        return;
    }
    if !toggles.set(ev.name.clone(), ev.on) {
        warn!(
            "[subsystem] '{}' was unregistered before its state could be set",
            ev.name
        );
        return;
    }
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
    on_reset_tutorial_progress,
    on_set_subsystem_enabled,
);

#[cfg(feature = "ui")]
fn on_tutorial_stop_requested(_trigger: On<TutorialStopRequested>, mut commands: Commands) {
    commands.trigger(SkipTutorial {});
}

/// On `MISSION_COMPLETE`, record the completion and advance the chain by starting
/// the current tutorial's [`TutorialMeta::next`] — the chain lives entirely in
/// DATA (each tutorial names its successor's id), so there is no per-tutorial Rust.
fn on_mission_complete(
    trigger: On<TelemetryEvent>,
    registry: Res<TutorialRegistry>,
    api_entities: Option<Res<lunco_api::ApiEntityRegistry>>,
    host: Res<TutorialHost>,
    mut progress: ResMut<TutorialProgress>,
    mut pending: ResMut<PendingAdvance>,
    mut commands: Commands,
) {
    let event = trigger.event();
    if event.name != "MISSION_COMPLETE" {
        return;
    }
    let Some(host) = host.0 else {
        warn!("[tutorial] ignored MISSION_COMPLETE without an active tutorial host");
        return;
    };
    let Some(api_entities) = api_entities else {
        warn!("[tutorial] ignored MISSION_COMPLETE without entity identity registry");
        return;
    };
    let Some(host_source) = api_entities.api_id_for(host).map(|id| id.get()) else {
        warn!("[tutorial] ignored MISSION_COMPLETE from an unidentified tutorial host");
        return;
    };
    if event.source != host_source {
        return;
    }
    // Attribute the completion only to the scenario host that emitted it. The
    // telemetry bus is intentionally broadcast, so a matching name from any
    // other scenario is not a tutorial verdict.
    let Some(id) = progress.current.take() else {
        return;
    };
    // Completion ends execution even when the authored world remains visible
    // for review. The host is not allowed to keep ticking against a completed
    // lesson or leak its interpreter state into the next one.
    commands.queue(|world: &mut World| {
        stop_tutorial_host(world);
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
        commands.trigger(StartTutorial { id: next });
    } else {
        info!("[tutorial] complete — awaiting confirm to advance → '{next}'");
        pending.0 = Some(next);
    }
}

/// Wind down an active lesson before any scene transition is applied. The
/// lifecycle owner emits this edge for Load, Clear, and Restart, so every
/// scene entry path has identical tutorial cleanup semantics.
fn on_scene_transition_started(
    trigger: On<lunco_core::SceneTransitionStarted>,
    mut progress: ResMut<TutorialProgress>,
    mut pending: ResMut<PendingTutorialStart>,
    mut session: ResMut<TutorialSession>,
    mut commands: Commands,
) {
    let is_pending_clear = matches!(
        &trigger.event().transition,
        lunco_core::SceneTransition::Clear
    ) && pending
        .0
        .as_ref()
        .is_some_and(|request| request.clear_before_load);
    let belongs_to_pending = match &trigger.event().transition {
        lunco_core::SceneTransition::Load { path, .. } => pending
            .0
            .as_ref()
            .is_some_and(|request| request.world.as_str() == path.as_str()),
        lunco_core::SceneTransition::Clear | lunco_core::SceneTransition::Restart { .. } => false,
    };
    if !belongs_to_pending && !is_pending_clear {
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
fn on_scene_transition_completed(
    trigger: On<lunco_core::SceneTransitionCompleted>,
    pending: Res<PendingTutorialStart>,
    mut commands: Commands,
) {
    match &trigger.event().transition {
        lunco_core::SceneTransition::Clear => {
            let Some(request) = pending
                .0
                .as_ref()
                .filter(|request| request.clear_before_load)
            else {
                return;
            };
            let scene = request.world.clone();
            commands.queue(move |world: &mut World| {
                let should_load = {
                    let mut pending = world.resource_mut::<PendingTutorialStart>();
                    let Some(request) = pending.0.as_mut() else {
                        return;
                    };
                    if !request.clear_before_load || request.world != scene {
                        return;
                    }
                    request.clear_before_load = false;
                    request.elapsed_secs = 0.0;
                    true
                };
                if !should_load {
                    return;
                }
                world.trigger(lunco_core::SceneTransitionIntent::load(scene, ""));
            });
        }
        lunco_core::SceneTransition::Load { path, .. } => {
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
        lunco_core::SceneTransition::Restart { .. } => {}
    }
}

/// Fail a tutorial mount that never publishes a scene-completed/scene-failed
/// lifecycle edge. This protects the progress state from a lost event or a
/// loader that wedges after accepting a transition intent.
fn pending_tutorial_watchdog(
    time: Res<Time>,
    mut pending: ResMut<PendingTutorialStart>,
    mut progress: ResMut<TutorialProgress>,
    mut pending_advance: ResMut<PendingAdvance>,
    mut session: ResMut<TutorialSession>,
    mut commands: Commands,
) {
    const MOUNT_TIMEOUT_SECS: f32 = 30.0;
    let Some(request) = pending.0.as_mut() else {
        return;
    };
    request.elapsed_secs += time.delta_secs();
    if request.elapsed_secs < MOUNT_TIMEOUT_SECS {
        return;
    }
    let request = pending.0.take().expect("pending tutorial request exists");
    progress.current = None;
    pending_advance.0 = None;
    session.world = None;
    commands.queue(|world: &mut World| {
        clear_tutorial_hud(world);
        stop_tutorial_host(world);
    });
    commands.trigger(tutorial_failed(format!(
        "abandoning '{}' — scene '{}' did not complete within {MOUNT_TIMEOUT_SECS:.0}s",
        request.id, request.world
    )));
}

/// Abandon the running lesson when a scene load fails, so it cannot go on to
/// report success it did not earn.
///
/// A lesson may have a declared world, and nothing after its mount should run
/// unless that world arrived. A coach-mark tour in particular
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
/// published by the typed scene owner, so the tutorial does not parse a
/// telemetry name or JSON payload to decide whether its world failed.
fn on_scene_transition_failed(
    trigger: On<lunco_core::SceneTransitionFailed>,
    mut progress: ResMut<TutorialProgress>,
    mut pending_advance: ResMut<PendingAdvance>,
    mut pending_start: ResMut<PendingTutorialStart>,
    mut session: ResMut<TutorialSession>,
    mut commands: Commands,
) {
    let path = match &trigger.event().transition {
        lunco_core::SceneTransition::Load { path, .. }
        | lunco_core::SceneTransition::Restart { path, .. } => Some(path.as_str()),
        lunco_core::SceneTransition::Clear => None,
    };
    let matches_pending = path.is_some_and(|path| {
        pending_start
            .0
            .as_ref()
            .is_some_and(|request| request.world == path)
    });
    if path.is_none()
        && pending_start
            .0
            .as_ref()
            .is_some_and(|request| request.clear_before_load)
    {
        let request = pending_start
            .0
            .take()
            .expect("pending clear request exists");
        pending_advance.0 = None;
        session.world = None;
        commands.queue(|world: &mut World| {
            clear_tutorial_hud(world);
            stop_tutorial_host(world);
        });
        commands.trigger(tutorial_failed(format!(
            "abandoning '{}' — its previous scene could not be cleared ({:?})",
            request.id,
            trigger.event().error
        )));
        return;
    }
    if matches_pending {
        let request = pending_start.0.take().expect("pending request matched");
        pending_advance.0 = None;
        error!(
            "[tutorial] abandoning '{}' — its scene failed to load ({:?})",
            request.id,
            trigger.event().error
        );
        commands.trigger(tutorial_failed(format!(
            "abandoning '{}' — its scene failed to load ({:?})",
            request.id,
            trigger.event().error
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
        trigger.event().error
    );
    commands.trigger(tutorial_failed(format!(
        "abandoning '{}' — its scene failed to load ({:?})",
        id,
        trigger.event().error
    )));
}

/// A tidy display name for a tutorial id: prefer its registered title, else the id.
#[cfg(feature = "ui")]
fn pretty_tutorial(registry: &TutorialRegistry, id: &str) -> String {
    registry
        .get(id)
        .map(|m| m.title)
        .unwrap_or_else(|| id.to_string())
}

/// Modal confirm popup shown when a tutorial finishes and a successor is queued
/// (unless [`TutorialProgress::autoproceed`]). Continue starts the next tutorial;
/// Stay dismisses. Auto-continue is an app setting in the Settings menu, not a
/// completion-prompt action.
#[cfg(feature = "ui")]
fn draw_advance_prompt(
    mut egui_ctx: EguiContexts,
    mut pending: ResMut<PendingAdvance>,
    registry: Res<TutorialRegistry>,
    theme: Option<Res<lunco_theme::Theme>>,
    mut commands: Commands,
) {
    let Some(next) = pending.0.clone() else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    let next_title = pretty_tutorial(&registry, &next);
    let theme = theme
        .map(|theme| theme.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);

    let mut proceed = false;
    let mut dismiss = false;
    let screen = ctx.content_rect();
    // Render at `Order::Tooltip` so the prompt paints above every overlay.
    egui::Area::new(egui::Id::new("tutorial_advance_scrim"))
        .order(egui::Order::Tooltip)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, theme.tokens.scrim);
            ui.allocate_rect(screen, egui::Sense::click());
        });
    egui::Area::new(egui::Id::new("tutorial_advance_prompt"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(360.0);
                ui.heading("Tutorial complete");
                ui.separator();
                ui.label(format!("Continue to “{next_title}”?"));
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
        commands.trigger(StartTutorial { id });
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
        commands.trigger(StartTutorial { id });
    }
}

/// Read argv for the boot ctx (rhai can't). Returns `(has_scene_arg, automated)`.
fn boot_env() -> (bool, bool) {
    let (mut has_scene, mut automated) = (false, false);
    for a in std::env::args() {
        match a.as_str() {
            "--scene" => has_scene = true,
            "--no-ui" => automated = true,
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
    let onboarded = world.resource::<TutorialProgress>().onboarded;
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
    const MENU_MIN_WIDTH: f32 = 320.0;
    const MENU_MAX_WIDTH: f32 = 420.0;
    const MENU_HEIGHT: f32 = 360.0;

    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, ctx| {
        let Some(mut progress) = ctx.resource::<TutorialProgress>().cloned() else {
            return;
        };
        ui.label(egui::RichText::new("Tutorials").weak().small());
        if ui
            .checkbox(&mut progress.autoproceed, "Auto-continue to next tutorial")
            .on_hover_text("When off, a completion popup asks before starting the next tutorial.")
            .changed()
        {
            ctx.set_resource(progress);
        }
    });
    layout.register_custom_menu("Tutorials", |ui, ctx| {
        ui.set_min_width(MENU_MIN_WIDTH);
        ui.set_max_width(MENU_MAX_WIDTH);
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
            egui::RichText::new("Tours · simulator exercises · completed")
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
                ui.set_min_width(MENU_MIN_WIDTH);
                ui.set_max_width(MENU_MAX_WIDTH);
                egui::ScrollArea::vertical()
                    .max_height(MENU_HEIGHT)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for meta in metas {
                            let done = progress.is_completed(&meta.id);
                            let glyph = if done {
                                "Done"
                            } else if meta.format == curriculum::LessonFormat::Tour {
                                "Tour"
                            } else {
                                "Exercise"
                            };
                            if ui
                                .add_sized(
                                    [ui.available_width(), 0.0],
                                    egui::Button::new(format!(
                                        "{glyph}  {}  · {}",
                                        meta.title,
                                        meta.format.label()
                                    ))
                                    .wrap(),
                                )
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
            });
        }

        ui.separator();
        if !progress.completed.is_empty() && ui.button("Reset completion history").clicked() {
            ctx.trigger(ResetTutorialProgress {});
            ui.close();
        }
        let running = progress.current.is_some()
            || ctx
                .resource::<TutorialHud>()
                .is_some_and(|hud| !hud.title.is_empty());
        ui.add_enabled_ui(running, |ui| {
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
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);

        ui.add_space(4.0);
        ui.heading("Tutorials");
        ui.label(
            egui::RichText::new(
                "Tours explain the UI · exercises require simulator evidence · ✓ completed.",
            )
            .weak()
            .small(),
        );
        if !progress.completed.is_empty() && ui.button("Reset completion history").clicked() {
            ctx.trigger(ResetTutorialProgress {});
        }

        if registry.tutorials.is_empty() {
            ui.label(egui::RichText::new("No tutorials registered.").weak());
            return;
        }

        if let Some(cur) = &progress.current {
            let title = registry
                .get(cur)
                .map(|m| m.title)
                .unwrap_or_else(|| cur.clone());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("▶ Running: {title}")).color(theme.tokens.accent),
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
                            ui.label(
                                egui::RichText::new("✓")
                                    .color(theme.tokens.success)
                                    .strong(),
                            );
                        }
                        ui.label(egui::RichText::new(meta.title.as_str()).strong());
                        ui.label(egui::RichText::new(meta.format.label()).weak().small());
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
        register_all_commands(app);
        app.add_observer(on_mission_complete);
        app.add_observer(on_scene_transition_started);
        app.add_observer(on_scene_transition_completed);
        app.add_observer(on_scene_transition_failed);
        #[cfg(feature = "ui")]
        app.add_observer(on_tutorial_stop_requested);
        app.add_observer(resolve_show_tutorial_intent);
        app.add_systems(Startup, surface_boot_curriculum_failures);
        app.add_systems(Update, sync_twin_curriculum_root);
        app.add_systems(Update, boot_seam);
        app.add_systems(Update, pending_tutorial_watchdog);
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
            format: curriculum::LessonFormat::Exercise,
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

        // External scene controls use the same lifecycle edge as LoadScene.
        // The active lesson must wind down before the outgoing world changes.
        app.world_mut().trigger(lunco_core::SceneTransitionStarted {
            transition: lunco_core::SceneTransition::clear(),
        });
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());

        app.world_mut().trigger(StartTutorial {
            id: "/Test/Lesson".into(),
        });
        app.update();

        app.world_mut().trigger(SkipTutorial {});
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
    }

    /// The public recovery command clears only persisted progress; it must not
    /// tear down a running lesson or mutate the loaded scene.
    #[test]
    fn reset_progress_clears_history_without_stopping_the_lesson() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TutorialCorePlugin {
                app: "sandbox".into(),
            });
        app.world_mut().resource_mut::<TutorialProgress>().completed =
            vec!["/Test/Finished".into()];
        app.world_mut().resource_mut::<TutorialProgress>().onboarded = true;
        app.world_mut().resource_mut::<TutorialProgress>().current = Some("/Test/Running".into());

        app.world_mut().trigger(ResetTutorialProgress {});
        app.update();

        let progress = app.world().resource::<TutorialProgress>();
        assert!(progress.completed.is_empty());
        assert!(!progress.onboarded);
        assert_eq!(progress.current.as_deref(), Some("/Test/Running"));
    }

    /// A world-backed lesson is a transaction: it requests the world first,
    /// starts only on the completion edge, and cancelling while it is pending
    /// clears the transaction through the normal scene command.
    #[test]
    fn world_lesson_waits_for_mount_and_cancels_cleanly() {
        #[derive(Resource, Default)]
        struct TransitionsSeen(Vec<lunco_core::SceneTransitionRequest>);

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
            format: curriculum::LessonFormat::Exercise,
            script: "lunco://tutorials/sandbox/first_drive.rhai".into(),
            world: Some("lunco://tutorials/sandbox/first_drive.usda".into()),
            first_start: false,
            next: None,
            source: CurriculumSource::Bundled,
        });
        app.insert_resource(TransitionsSeen::default());
        app.add_observer(
            |trigger: On<lunco_core::SceneTransitionIntent>, mut seen: ResMut<TransitionsSeen>| {
                seen.0.push(trigger.event().request.clone());
            },
        );

        app.world_mut().trigger(StartTutorial {
            id: "/Test/WorldLesson".into(),
        });
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
        assert_eq!(
            app.world().resource::<TransitionsSeen>().0,
            vec![lunco_core::SceneTransitionRequest::load(
                "lunco://tutorials/sandbox/first_drive.usda",
                "",
            )]
        );

        app.world_mut()
            .trigger(lunco_core::SceneTransitionCompleted {
                transition: lunco_core::SceneTransition::load(
                    "lunco://tutorials/sandbox/first_drive.usda",
                    "",
                ),
            });
        app.update();
        assert_eq!(
            app.world()
                .resource::<TutorialProgress>()
                .current
                .as_deref(),
            Some("/Test/WorldLesson")
        );

        app.world_mut().trigger(SkipTutorial {});
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
        assert!(app.world().resource::<TransitionsSeen>().0.ends_with(&[
            lunco_core::SceneTransitionRequest::load(
                "lunco://tutorials/sandbox/first_drive.usda",
                "",
            ),
            lunco_core::SceneTransitionRequest::Clear,
        ]));
    }

    /// Replacing a world-backed lesson must clear the outgoing world before
    /// loading the new one, even when both lessons name the same stage. A
    /// direct LoadScene would correctly no-op on that identity and leave the
    /// old lesson's entities in place.
    #[test]
    fn replacing_world_lesson_clears_before_loading_same_stage() {
        #[derive(Resource, Default)]
        struct TransitionsSeen(Vec<lunco_core::SceneTransitionRequest>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TutorialCorePlugin {
                app: "sandbox".into(),
            });
        for (id, title) in [("/Test/First", "First"), ("/Test/Second", "Second")] {
            app.register_tutorial(TutorialMeta {
                id: id.into(),
                title: title.into(),
                blurb: String::new(),
                app: "/Test".into(),
                difficulty: String::new(),
                format: curriculum::LessonFormat::Exercise,
                script: "lunco://tutorials/sandbox/first_drive.rhai".into(),
                world: Some("lunco://tutorials/sandbox/first_drive.usda".into()),
                first_start: false,
                next: None,
                source: CurriculumSource::Bundled,
            });
        }
        app.insert_resource(TransitionsSeen::default());
        app.add_observer(
            |trigger: On<lunco_core::SceneTransitionIntent>, mut seen: ResMut<TransitionsSeen>| {
                seen.0.push(trigger.event().request.clone());
            },
        );

        app.world_mut().trigger(StartTutorial {
            id: "/Test/First".into(),
        });
        app.update();
        app.world_mut()
            .trigger(lunco_core::SceneTransitionCompleted {
                transition: lunco_core::SceneTransition::load(
                    "lunco://tutorials/sandbox/first_drive.usda",
                    "",
                ),
            });
        app.update();

        app.world_mut().trigger(StartTutorial {
            id: "/Test/Second".into(),
        });
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
        assert!(matches!(
            app.world().resource::<TransitionsSeen>().0.as_slice(),
            [
                lunco_core::SceneTransitionRequest::Load { .. },
                lunco_core::SceneTransitionRequest::Clear,
            ]
        ));

        app.world_mut()
            .trigger(lunco_core::SceneTransitionCompleted {
                transition: lunco_core::SceneTransition::Clear,
            });
        app.update();
        assert!(matches!(
            app.world().resource::<TransitionsSeen>().0.as_slice(),
            [
                lunco_core::SceneTransitionRequest::Load { .. },
                lunco_core::SceneTransitionRequest::Clear,
                lunco_core::SceneTransitionRequest::Load { .. },
            ]
        ));

        app.world_mut()
            .trigger(lunco_core::SceneTransitionCompleted {
                transition: lunco_core::SceneTransition::load(
                    "lunco://tutorials/sandbox/first_drive.usda",
                    "",
                ),
            });
        app.update();
        assert_eq!(
            app.world()
                .resource::<TutorialProgress>()
                .current
                .as_deref(),
            Some("/Test/Second")
        );
    }

    #[test]
    fn completed_world_lesson_replay_clears_before_loading_same_stage() {
        #[derive(Resource, Default)]
        struct TransitionsSeen(Vec<lunco_core::SceneTransitionRequest>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TutorialCorePlugin {
                app: "sandbox".into(),
            });
        app.insert_resource(lunco_api::ApiEntityRegistry::default());
        app.register_tutorial(TutorialMeta {
            id: "/Test/Replay".into(),
            title: "Replay".into(),
            blurb: String::new(),
            app: "/Test".into(),
            difficulty: String::new(),
            format: curriculum::LessonFormat::Exercise,
            script: "lunco://tutorials/sandbox/first_drive.rhai".into(),
            world: Some("lunco://tutorials/sandbox/first_drive.usda".into()),
            first_start: false,
            next: None,
            source: CurriculumSource::Bundled,
        });
        app.insert_resource(TransitionsSeen::default());
        app.add_observer(
            |trigger: On<lunco_core::SceneTransitionIntent>, mut seen: ResMut<TransitionsSeen>| {
                seen.0.push(trigger.event().request.clone());
            },
        );

        app.world_mut().trigger(StartTutorial {
            id: "/Test/Replay".into(),
        });
        app.update();
        app.world_mut()
            .trigger(lunco_core::SceneTransitionCompleted {
                transition: lunco_core::SceneTransition::load(
                    "lunco://tutorials/sandbox/first_drive.usda",
                    "",
                ),
            });
        app.update();

        let host = app
            .world()
            .resource::<TutorialHost>()
            .0
            .expect("mounted lesson has a tutorial host");
        app.world_mut()
            .resource_mut::<lunco_api::ApiEntityRegistry>()
            .assign(host, lunco_core::GlobalEntityId::from_raw(700));
        app.world_mut().trigger(TelemetryEvent {
            name: "MISSION_COMPLETE".into(),
            source: 700,
            severity: Severity::Info,
            data: TelemetryValue::F64(0.0),
            timestamp: 0.0,
        });
        app.update();

        assert!(app.world().resource::<TutorialProgress>().current.is_none());
        assert_eq!(
            app.world().resource::<TutorialSession>().world.as_deref(),
            Some("lunco://tutorials/sandbox/first_drive.usda")
        );

        app.world_mut().trigger(StartTutorial {
            id: "/Test/Replay".into(),
        });
        app.update();
        assert!(matches!(
            app.world().resource::<TransitionsSeen>().0.as_slice(),
            [
                lunco_core::SceneTransitionRequest::Load { .. },
                lunco_core::SceneTransitionRequest::Clear,
            ]
        ));

        app.world_mut()
            .trigger(lunco_core::SceneTransitionCompleted {
                transition: lunco_core::SceneTransition::Clear,
            });
        app.update();
        assert!(matches!(
            app.world().resource::<TransitionsSeen>().0.as_slice(),
            [
                lunco_core::SceneTransitionRequest::Load { .. },
                lunco_core::SceneTransitionRequest::Clear,
                lunco_core::SceneTransitionRequest::Load { .. },
            ]
        ));
    }

    #[test]
    fn mission_complete_requires_the_active_tutorial_host_source() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TutorialCorePlugin {
                app: "sandbox".into(),
            });
        app.insert_resource(lunco_api::ApiEntityRegistry::default());
        app.register_tutorial(TutorialMeta {
            id: "/Test/Source".into(),
            title: "Source".into(),
            blurb: String::new(),
            app: "/Test".into(),
            difficulty: String::new(),
            format: curriculum::LessonFormat::Exercise,
            script: "lunco://tutorials/sandbox/first_drive.rhai".into(),
            world: None,
            first_start: false,
            next: None,
            source: CurriculumSource::Bundled,
        });
        app.world_mut().trigger(StartTutorial {
            id: "/Test/Source".into(),
        });
        app.update();

        let host = app
            .world()
            .resource::<TutorialHost>()
            .0
            .expect("started lesson has a tutorial host");
        app.world_mut()
            .resource_mut::<lunco_api::ApiEntityRegistry>()
            .assign(host, lunco_core::GlobalEntityId::from_raw(701));

        app.world_mut().trigger(TelemetryEvent {
            name: "MISSION_COMPLETE".into(),
            source: 702,
            severity: Severity::Info,
            data: TelemetryValue::F64(0.0),
            timestamp: 0.0,
        });
        app.update();
        assert_eq!(
            app.world()
                .resource::<TutorialProgress>()
                .current
                .as_deref(),
            Some("/Test/Source")
        );

        app.world_mut().trigger(TelemetryEvent {
            name: "MISSION_COMPLETE".into(),
            source: 701,
            severity: Severity::Info,
            data: TelemetryValue::F64(0.0),
            timestamp: 0.0,
        });
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
        assert_eq!(
            app.world().resource::<TutorialProgress>().completed,
            vec!["/Test/Source"]
        );
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

    #[test]
    fn twin_lesson_source_cannot_escape_its_root() {
        let root = CurriculumRoot::twin(
            lunco_workspace::TwinId::new(1),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        );
        // Without the shared asset-boundary guard this would read the
        // workspace manifest through the tutorial root.
        assert!(root.read("twin://untrusted/../../Cargo.toml").is_none());
        assert!(root.read(r"twin://untrusted\..\Cargo.toml").is_none());
    }
}
