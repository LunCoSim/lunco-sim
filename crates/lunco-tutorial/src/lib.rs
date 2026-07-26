//! The unified tutorial launcher — for **every** workbench app (lunica, sandbox, …).
//!
//! ## One source: a tutorial is a `.rhai` scenario
//!
//! There is no scene-vs-script duality. A tutorial is a single rhai scenario
//! (`assets/tutorials/<script>`), and the launcher runs it by attaching it to a
//! persistent **host entity** via [`RunScenario`](lunco_scripting::commands::RunScenario).
//! Whatever environment the lesson needs, it sets up *itself* in `on_start`:
//! `load_scene("scenes/…")` for a 3D lesson, `cmd("OpenClass", …)` for a modeling
//! lesson, `set_subsystem(…)` for progressive fidelity, nothing for a pure UI
//! tour. The coach card, spotlight, and objectives all come from the shared HUD
//! (`lunco-workbench::tutorial_overlay`) + the rhai prelude.
//!
//! So this crate is the thin **shell** shared by all apps:
//! - [`TutorialRegistry`] — the catalog; apps register their own tutorials via
//!   [`TutorialAppExt::register_tutorial`] after adding [`TutorialCorePlugin`].
//! - a top-level **🎓 Tutorials** menu + a dockable [`TutorialsPanel`].
//! - [`StartTutorial`] — load `<script>` and run it on the host (the single
//!   launch path; menu, F1, HTTP API, MCP, and other scripts all funnel here).
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

/// One tutorial's catalog entry. The lesson itself is the `.rhai` at `script`;
/// this is what the menu/panel needs to list + launch it.
///
/// **Data, not code.** Entries live in a per-app JSON manifest
/// (`assets/tutorials/<app>/tutorials.json`) and are scanned by [`TutorialCorePlugin`]
/// at startup — adding a lesson never touches Rust.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TutorialMeta {
    /// Stable id (kebab-case). Progress, chaining, and [`StartTutorial`] key off this.
    pub id: String,
    /// Display title.
    pub title: String,
    /// One-line description shown under the title / on hover.
    #[serde(default)]
    pub blurb: String,
    /// Which app it targets — `"sandbox"`, `"lunica"`, or `"any"` (informational;
    /// the manifest it lives in already scopes it to one app).
    #[serde(default)]
    pub app: String,
    /// Difficulty tag (`"beginner"` / `"intermediate"` / …) shown as a chip.
    #[serde(default)]
    pub difficulty: String,
    /// The orchestrator's path **relative to `assets/tutorials/`** (e.g.
    /// `"lunica/overview.rhai"`, `"sandbox/first_drive.rhai"`). Loaded at
    /// launch by [`lunco_assets::tutorials::tutorial_source`] — disk on native
    /// (live-editable), embedded on wasm.
    pub script: String,
    /// Auto-launch this tutorial once on the user's first run (persisted via
    /// [`TutorialProgress::onboarded`]). At most one entry per app should set it —
    /// the onboarding entry point.
    #[serde(default)]
    pub first_start: bool,
    /// The id of the tutorial to chain to when this one completes
    /// (`MISSION_COMPLETE`). Data, not code. `None` = the chain ends here.
    #[serde(default)]
    pub next: Option<String>,
    /// Which root contributed this lesson. Provenance, stamped at registration —
    /// never authored, hence `skip`. It is what [`script`](Self::script) resolves
    /// against, so a twin's lesson and a bundled one load by the same rule.
    #[serde(skip)]
    pub source: CurriculumSource,
}

/// One TRACK's catalog entry — `assets/tutorials/<track>/track.json`.
///
/// Presentation and hosting for a whole track, as DATA. Both used to be Rust:
/// a `for track in ["basic"]` list in the sandbox app and a `[("basic", "2️⃣ Rover
/// Driving & Slopes"), …]` table in the menu, so adding a track meant editing two
/// crates and a track existed only in the build that happened to name it. Now the
/// asset tree answers "which tracks exist" ([`lunco_assets::tutorials::tutorial_tracks`])
/// and this file answers "who shows it, under what heading, in what order".
///
/// ONE form, every field required. A track without a readable `track.json` is
/// not loaded at all and says so — no defaulted label, no inferred host, no
/// "works anyway" path to keep a second shape alive. A curriculum that wants to
/// be shown declares how.
#[derive(Clone, Debug, Deserialize)]
pub struct TrackMeta {
    /// Heading shown for this track in the 🎓 menu.
    pub label: String,
    /// Sort key among tracks in the menu, ascending.
    pub order: i32,
    /// App ids that load this track. `basic` names `sandbox` here — that is the
    /// whole reason the field exists: a track is not owned by the app it happens
    /// to be named after.
    pub hosts: Vec<String>,
}

impl TrackMeta {
    /// Does `app` load this track?
    fn hosted_by(&self, app: &str) -> bool {
        self.hosts.iter().any(|h| h == app)
    }
}

/// Parse one `track.json`. `None` (with a reason) if it is unreadable — the
/// single parse used by both the bundled tracks and a twin's, so the two cannot
/// drift into different rules about what a track declaration is.
fn parse_track_meta(source: &str, src: &str) -> Option<TrackMeta> {
    match serde_json::from_str::<TrackMeta>(src) {
        Ok(m) => Some(m),
        Err(e) => {
            warn!("[tutorial] {source} is invalid: {e} (needs label, order, hosts)");
            None
        }
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
/// There used to be two loaders: the bundled walk in the plugin's `build`, and a
/// separate twin path with its own manifest constant, its own parse, its own
/// provenance field and its own unload rule. Two loaders meant two sets of rules
/// for what a track is, and they had already drifted — the bundled one keyed the
/// track table by DIRECTORY NAME while the twin one keyed it by the lessons'
/// declared `app`, so a track whose directory did not happen to match its `app`
/// lost its menu heading. One loader over N roots removes the class of bug, not
/// just that instance.
#[derive(Clone, Debug)]
pub struct CurriculumRoot {
    /// Provenance — what gets dropped when this provider goes away.
    pub source: CurriculumSource,
    /// Base every path in this root resolves against: `assets/tutorials/` for
    /// bundled (implicit — that root reads through [`lunco_assets`], which owns
    /// the disk-vs-embedded policy), the twin's directory for a twin. A lesson's
    /// `script` is base-relative, which is already true of both today.
    pub base: Option<std::path::PathBuf>,
}

/// Where a twin publishes its curriculum, relative to the twin root. A twin may
/// put one track here (a `track.json` beside its lessons) or several (a
/// subdirectory per track) — the same rule the bundled root already follows.
const TWIN_CURRICULUM_DIR: &str = "sim/tutorials";

impl CurriculumRoot {
    /// The bundled root — `assets/tutorials/`.
    pub fn bundled() -> Self {
        Self {
            source: CurriculumSource::Bundled,
            base: None,
        }
    }

    /// A twin's root, based at the twin directory.
    pub fn twin(id: lunco_workspace::TwinId, root: std::path::PathBuf) -> Self {
        Self {
            source: CurriculumSource::Twin(id),
            base: Some(root),
        }
    }

    /// Read a file by its path relative to this root's base.
    ///
    /// The bundled root goes through [`lunco_assets::tutorials::tutorial_source`]
    /// — on-disk first (so a lesson edit replays with no rebuild), embedded
    /// fallback (a packaged binary, wasm). A twin is on disk by definition.
    pub fn read(&self, rel: &str) -> Option<String> {
        match &self.base {
            None => lunco_assets::tutorials::tutorial_source(rel),
            Some(base) => std::fs::read_to_string(base.join(rel)).ok(),
        }
    }

    /// Directories in this root that declare a track, base-relative. A track IS
    /// a directory holding a `track.json` — discovery, not a list, so adding one
    /// is dropping in a directory.
    fn track_dirs(&self) -> Vec<String> {
        let Some(base) = &self.base else {
            return lunco_assets::tutorials::tutorial_tracks();
        };
        let dir = base.join(TWIN_CURRICULUM_DIR);
        if dir.join("track.json").is_file() {
            return vec![TWIN_CURRICULUM_DIR.to_string()];
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new(); // a twin with no curriculum is normal, not an error
        };
        let mut out: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().join("track.json").is_file())
            .filter_map(|e| e.file_name().to_str().map(|n| format!("{TWIN_CURRICULUM_DIR}/{n}")))
            .collect();
        out.sort();
        out
    }

    /// Load every track this root declares that `host` shows, into `registry`.
    fn load_into(&self, host: &str, registry: &mut TutorialRegistry) -> usize {
        let mut loaded = 0;
        for dir in self.track_dirs() {
            let track_path = format!("{dir}/track.json");
            let Some(track_src) = self.read(&track_path) else {
                warn!(
                    "[tutorial] '{dir}' has no readable track.json — its lessons are \
                     not loaded. Declare `label`, `order` and `hosts`."
                );
                continue;
            };
            let Some(track) = parse_track_meta(&track_path, &track_src) else {
                continue;
            };
            if !track.hosted_by(host) {
                continue; // this track is for a different app
            }
            let manifest_path = format!("{dir}/tutorials.json");
            let Some(src) = self.read(&manifest_path) else {
                warn!("[tutorial] no lesson manifest at '{manifest_path}'");
                continue;
            };
            let metas = match serde_json::from_str::<Vec<TutorialMeta>>(&src) {
                Ok(m) => m,
                // Say so. A malformed manifest used to fail silently, and its
                // lessons simply never appeared with nothing anywhere saying why.
                Err(e) => {
                    warn!("[tutorial] '{manifest_path}' is invalid: {e}");
                    continue;
                }
            };
            // Keyed by the `app` its lessons declare — the same key the menu
            // groups by, so the heading always lands on its own group.
            if let Some(key) = metas.first().map(|m| m.app.clone()) {
                registry.tracks.insert(key, track);
            }
            for mut meta in metas {
                meta.source = self.source;
                registry.register_tutorial(meta);
            }
            loaded += 1;
        }
        loaded
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
/// small manifests, and rebuilding makes "what is registered" a pure function of
/// "which roots exist". That is what deleted the old `retain_bundled` — an
/// unload rule that had to be kept in step with the load rule by hand, and once
/// wasn't (it matched on the literal app name `"school"`, making a twin's chosen
/// label a reserved word in the engine).
fn rebuild_curriculum(roots: &CurriculumRoots, host: &str, registry: &mut TutorialRegistry) {
    *registry = TutorialRegistry::default();
    let mut tracks = 0;
    for root in &roots.0 {
        tracks += root.load_into(host, registry);
    }
    if tracks == 0 {
        warn!(
            "[tutorial] app '{host}' shows no tutorial track — nothing will appear in \
             the 🎓 menu. A track is a directory with a `tutorials.json` and a \
             `track.json`; it is shown here when that `track.json` lists this app in \
             `hosts`."
        );
    } else {
        info!(
            "[tutorial] curriculum: {tracks} track(s), {} lesson(s), from {} root(s)",
            registry.tutorials.len(),
            roots.0.len()
        );
    }
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

/// The app id this host runs as (`"sandbox"`, `"lunica"`, …) — the value
/// [`TutorialCorePlugin::app`] was built with.
///
/// A resource because the rule "which tracks does this app load" is enforced in
/// two places that must agree: plugin build (bundled tracks) and
/// [`sync_twin_tutorials`] (a twin's). One value, read by both.
#[derive(Resource, Clone, Debug)]
pub struct TutorialHostApp(pub String);

/// The persistent entity every tutorial scenario attaches to. Spawned lazily on
/// the first launch; re-launching hot-reloads the scenario on it.
#[derive(Resource, Default)]
struct TutorialHost(Option<Entity>);

/// A completed tutorial is waiting on the user's confirmation before starting its
/// declared successor. `Some(id)` while the [`draw_advance_prompt`] popup shows;
/// cleared on Continue/Stay. Not persisted — transient per-completion.
#[derive(Resource, Default, Clone)]
pub struct PendingAdvance(pub Option<String>);

// ── Commands ────────────────────────────────────────────────────────────────

/// Start a tutorial by id: load its `.rhai` and run it on the host entity, and
/// record it as current. The single launch path — menu, F1, HTTP API, MCP, and
/// other scripts (`cmd("StartTutorial", #{ id })`) all route here.
#[Command(default)]
pub struct StartTutorial {
    /// The [`TutorialMeta::id`] to start.
    pub id: String,
}

/// Stop the current tutorial: clear the HUD (hint, objectives, spotlight, coach
/// card) and forget the current id. Leaves any loaded scene. `cmd("SkipTutorial")`.
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
/// one — two callers that must agree on what "the overlay" is, and previously
/// didn't.
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

#[on_command(StartTutorial)]
fn on_start_tutorial(trigger: On<StartTutorial>, mut commands: Commands) {
    let id = trigger.event().id.clone();
    // `ensure_host` + `RunScenario` need `&mut World`; an observer only has
    // `Commands`, so defer to an exclusive closure.
    commands.queue(move |world: &mut World| {
        // Starting a lesson OWNS the presentation reset: the outgoing lesson's
        // hint, objectives, spotlight, coach card and "continue to next?" prompt
        // all go, before this one publishes anything.
        //
        // It must happen HERE and not be left to the incoming lesson, because a
        // lesson only overwrites the parts it happens to set. One that shows a
        // coach card but no objectives inherited the previous lesson's
        // checklist, and a UI tour with no `load_scene` at all (every lunica
        // lesson) inherited the whole overlay — the scene-change observer in the
        // app was the only thing that ever cleared it, which made the bug look
        // like it was about scenes rather than about switching lessons.
        //
        // Stopping a lesson already did exactly this (`on_skip_tutorial`);
        // starting one did not. That asymmetry WAS the bug.
        clear_tutorial_hud(world);
        world.resource_mut::<PendingAdvance>().0 = None;
        let Some(meta) = world.resource::<TutorialRegistry>().get(&id) else {
            warn!("[tutorial] StartTutorial: unknown id '{id}'");
            return;
        };
        // A lesson's script resolves against the root that CONTRIBUTED it —
        // provenance, not a search. The previous version tried the active twin's
        // directory and fell back to `assets/`, so a twin lesson and a bundled
        // one with the same relative path shadowed each other depending on which
        // twin happened to be open.
        let Some(root) = world
            .resource::<CurriculumRoots>()
            .0
            .iter()
            .find(|r| r.source == meta.source)
            .cloned()
        else {
            warn!("[tutorial] '{id}' came from a root that is no longer mounted");
            return;
        };
        let Some(source) = root.read(&meta.script) else {
            warn!("[tutorial] no source for '{id}' ({})", meta.script);
            return;
        };
        let host = ensure_host(world);
        info!("[tutorial] starting '{}' → {}", meta.title, meta.script);
        world.trigger(lunco_scripting::commands::RunScenario {
            target: host,
            source,
            params: String::new(),
        });
        world.resource_mut::<TutorialProgress>().current = Some(id);
        if let Some(mut s) = world.get_resource_mut::<TutorialSeen>() {
            s.onboarded = true;
        }
    });
}

#[on_command(SkipTutorial)]
#[cfg(feature = "ui")]
fn on_skip_tutorial(
    _t: On<SkipTutorial>,
    // OPTIONAL, because `ui` compiled in does not mean the workbench HUD is
    // installed: `TutorialHud` belongs to `lunco_workbench`'s overlay plugin, and
    // a host can run lessons without it (a `ui`-featured build driving the sim
    // headlessly, and every test of this crate's own execution core). Required,
    // this observer fails parameter validation and stopping a lesson PANICS —
    // taking down an app whose only sin was not drawing a HUD.
    hud: Option<ResMut<TutorialHud>>,
    mut progress: ResMut<TutorialProgress>,
    mut pending: ResMut<PendingAdvance>,
) {
    // The same reset starting a lesson performs.
    reset_hud(hud);
    progress.current = None;
    pending.0 = None;
}

/// Headless runs have no presentation state to clear, but stopping a lesson
/// must retain the same execution semantics as the UI command.
#[on_command(SkipTutorial)]
#[cfg(not(feature = "ui"))]
fn on_skip_tutorial(
    _t: On<SkipTutorial>,
    mut progress: ResMut<TutorialProgress>,
    mut pending: ResMut<PendingAdvance>,
) {
    progress.current = None;
    pending.0 = None;
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
    mut pending: ResMut<PendingAdvance>,
) {
    if trigger.event().name != "SCENE_LOAD_FAILED" {
        return;
    }
    let Some(id) = progress.current.take() else {
        return;
    };
    pending.0 = None;
    error!(
        "[tutorial] abandoning '{id}' — its scene failed to load ({:?}). The \
         lesson cannot demonstrate anything against an empty viewport, so it is \
         NOT recorded as complete and its successor will not start.",
        trigger.event().data
    );
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
/// what a track is, where a manifest lives, or which lessons belong to whom.
/// That is the point: a twin's curriculum is not a second kind of curriculum, so
/// it does not get a second set of rules (it previously had its own manifest
/// constant, parse, provenance stamp and unload rule, and they had drifted from
/// the bundled ones).
///
/// It runs as a system because it is a load-time concern. It used to live inside
/// the 🎓 menu's DRAW closure, so a twin's lessons appeared only once somebody
/// opened the menu, and the manifest was re-read on a draw callback.
fn sync_twin_curriculum_root(
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut roots: ResMut<CurriculumRoots>,
    mut registry: ResMut<TutorialRegistry>,
    host: Res<TutorialHostApp>,
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
    rebuild_curriculum(&roots, &host.0, &mut registry);
}

// ── Menu + launcher panel ───────────────────────────────────────────────────

/// Register the top-level **🎓 Tutorials** menu, listing the app's tutorials with
/// a completion tick; clicking starts one. Shared by every workbench app.
#[cfg(feature = "ui")]
fn register_tutorials_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_custom_menu("🎓 Tutorials", |ui, world| {
        let registry = world
            .get_resource::<TutorialRegistry>()
            .cloned()
            .unwrap_or_default();
        let progress = world
            .get_resource::<TutorialProgress>()
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

        // Heading + order per track come from that track's `track.json`
        // (`registry.tracks`), so a new track brings its own presentation and this
        // menu never learns any track's name. A group with no metadata — a twin's
        // curriculum, a user's own manifest — renders under its authored `app` key
        // and sorts after the tracks that declared an order.
        let mut groups: Vec<(&String, &Vec<&TutorialMeta>)> = grouped.iter().collect();
        groups.sort_by_key(|(app_key, _)| {
            (
                registry
                    .tracks
                    .get(app_key.as_str())
                    .map(|t| t.order)
                    .unwrap_or(i32::MAX),
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
                        world.trigger(StartTutorial {
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
                world.trigger(SkipTutorial {});
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
/// are loaded from `assets/tutorials/<app>/tutorials.json`.
pub struct TutorialCorePlugin {
    /// App name — selects `assets/tutorials/<app>/tutorials.json`.
    pub app: String,
}

impl Plugin for TutorialCorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TutorialRegistry>();
        app.insert_resource(TutorialHostApp(self.app.clone()));
        // The app contributes the bundled root; a twin adds its own when it opens
        // (`sync_twin_curriculum_root`). Both go through the SAME loader, so the
        // engine names no track and knows no lesson.
        let mut roots = CurriculumRoots(vec![CurriculumRoot::bundled()]);
        let mut registry = TutorialRegistry::default();
        rebuild_curriculum(&roots, &self.app, &mut registry);
        // `roots` is moved in whole so a provider added later (a pack, a
        // classroom server) is a push here and needs no code in this crate.
        roots.0.shrink_to_fit();
        app.insert_resource(roots);
        app.insert_resource(registry);
        app.init_resource::<TutorialHost>();
        app.init_resource::<PendingAdvance>();
        app.register_settings_section::<TutorialProgress>();
        app.register_type::<TutorialSeen>();
        app.register_settings_section::<TutorialSeen>();
        register_all_commands(app);
        app.add_observer(on_mission_complete);
        app.add_observer(on_scene_load_failed);
        app.add_observer(resolve_show_tutorial_intent);
        app.add_systems(Update, sync_twin_curriculum_root);
        app.add_systems(Update, boot_seam);
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
    /// App name — selects `assets/tutorials/<app>/tutorials.json`.
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

    #[test]
    fn core_executes_and_stops_a_lesson_without_the_ui_plugin() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TutorialCorePlugin {
                app: "sandbox".into(),
            });

        app.world_mut().trigger(StartTutorial {
            id: "first-drive".into(),
        });
        app.update();
        assert_eq!(
            app.world()
                .resource::<TutorialProgress>()
                .current
                .as_deref(),
            Some("first-drive")
        );

        app.world_mut().trigger(SkipTutorial {});
        app.update();
        assert!(app.world().resource::<TutorialProgress>().current.is_none());
    }
}
