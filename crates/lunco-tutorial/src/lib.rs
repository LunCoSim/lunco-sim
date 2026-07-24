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
    /// Which twin contributed this lesson, if any. `None` = bundled with the app.
    ///
    /// Provenance, set at registration — never authored, hence `skip`. It is what
    /// lets [`TutorialRegistry::retain_bundled`] unload the previous twin's
    /// curriculum without knowing (or caring) what track it called itself.
    #[serde(skip)]
    pub from_twin: Option<lunco_workspace::TwinId>,
}

/// The catalog of registered tutorials. Empty until an app registers its own via
/// [`TutorialAppExt::register_tutorial`] — this crate ships no built-ins so the
/// same engine serves every app with only that app's lessons in its registry.
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

    /// Drop every lesson a twin contributed, leaving the bundled catalog intact.
    ///
    /// Keyed on PROVENANCE, not on an app name. The previous version did
    /// `retain(|t| t.app != "school")` — a magic string that made "school" a
    /// reserved word in Rust: it deleted a bundled track that happened to share the
    /// name, and left behind any twin whose lessons declared a different app. What
    /// actually needs clearing is "whatever the last twin added", which is a fact
    /// the registry can hold rather than a name it has to recognise.
    pub fn retain_bundled(&mut self) {
        self.tutorials.retain(|t| t.from_twin.is_none());
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
        // Starting a lesson dismisses any leftover "continue to next?" prompt from a
        // previously completed one (it would otherwise overlay this lesson's HUD).
        world.resource_mut::<PendingAdvance>().0 = None;
        let Some(meta) = world.resource::<TutorialRegistry>().get(&id) else {
            warn!("[tutorial] StartTutorial: unknown id '{id}'");
            return;
        };
        // Try loading from the active twin first (dynamic twin tutorials)
        let mut source = None;
        if let Some(ws) = world.get_resource::<lunco_workspace::WorkspaceResource>() {
            if let Some(active_id) = ws.0.active_twin {
                if let Some(twin) = ws.0.twin(active_id) {
                    let twin_script_path = twin.root.join(&meta.script);
                    if let Ok(src) = std::fs::read_to_string(&twin_script_path) {
                        source = Some(src);
                    }
                }
            }
        }

        // Fall back to general assets
        let source = source.or_else(|| lunco_assets::tutorials::tutorial_source(&meta.script));

        let Some(source) = source else {
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
    mut hud: ResMut<TutorialHud>,
    mut progress: ResMut<TutorialProgress>,
    mut pending: ResMut<PendingAdvance>,
) {
    hud.hint.clear();
    hud.objectives.clear();
    hud.spotlight = None;
    hud.tour = None;
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

/// The twin whose curriculum is currently loaded, so a change can unload it.
#[derive(Resource, Default)]
struct LoadedTwinCurriculum(Option<lunco_workspace::TwinId>);

/// The manifest a twin publishes its lessons in, relative to the twin root.
const TWIN_TUTORIALS_MANIFEST: &str = "sim/tutorials/tutorials.json";

/// **A twin brings its own lessons.** On the active twin changing, drop the previous
/// twin's curriculum and load the new one's `sim/tutorials/tutorials.json`.
///
/// This is a load-time concern, so it runs as a system. It used to live inside the
/// 🎓 menu's DRAW closure — meaning a twin's lessons only appeared once someone
/// opened the menu, and the file was re-read on a draw callback.
///
/// **No track is named here.** The previous version force-wrote `m.app = "school"`
/// over every twin's manifest and cleared with `retain(|t| t.app != "school")`,
/// which made "school" a reserved word in the engine: the Summer Space School twin
/// already declares `"app": "school"` itself, so Rust was overwriting data with the
/// same data — while any OTHER twin's lessons were silently relabelled school. The
/// manifest says which track it belongs to; the engine's job is to load it, not to
/// have an opinion about who wrote it.
fn sync_twin_tutorials(
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut loaded: ResMut<LoadedTwinCurriculum>,
    mut registry: ResMut<TutorialRegistry>,
) {
    let active = workspace.as_ref().and_then(|ws| ws.0.active_twin);
    if loaded.0 == active {
        return;
    }
    // The previous twin's lessons go with it — identified by provenance, not name.
    registry.retain_bundled();
    loaded.0 = active;

    let Some((ws, id)) = workspace.as_ref().zip(active) else {
        return;
    };
    let Some(twin) = ws.0.twin(id) else { return };
    let manifest = twin.root.join(TWIN_TUTORIALS_MANIFEST);
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return; // a twin with no curriculum is the normal case, not an error
    };
    match serde_json::from_str::<Vec<TutorialMeta>>(&text) {
        Ok(metas) => {
            let n = metas.len();
            for mut m in metas {
                m.from_twin = Some(id);
                registry.register_tutorial(m);
            }
            info!(
                "[tutorial] loaded {n} lesson(s) from twin at {}",
                twin.root.display()
            );
        }
        // Say so. A malformed manifest previously failed silently, and the lessons
        // simply never appeared — with nothing anywhere to say why.
        Err(e) => warn!("[tutorial] {} is invalid: {e}", manifest.display()),
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

        // Display labels for the tracks THIS APP SHIPS — presentation and ordering,
        // nothing more. A track is not required to be here: anything else (a twin's
        // curriculum, a user's own manifest) renders below under its authored `app`.
        //
        // `"school"` was in this table, which made the Summer Space School a special
        // case in the engine. It is not: it is a twin like any other, and it names
        // its own track in its manifest. Bundled tracks are listed here because the
        // app genuinely owns them — not because the engine knows who they are.
        let tracks = [
            ("sandbox", "1️⃣ Sandbox Onboarding"),
            ("basic", "2️⃣ Rover Driving & Slopes"),
            ("lunica", "3️⃣ Modelica Workbench"),
        ];

        for &(app_key, label) in &tracks {
            if let Some(metas) = grouped.get(app_key) {
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
        }

        // Any other apps/tracks not in our hardcoded list
        for (app_key, metas) in &grouped {
            if !tracks.iter().any(|(k, _)| k == app_key) {
                ui.menu_button(app_key.as_str(), |ui| {
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
        let manifest_path = format!("{}/tutorials.json", self.app);
        match lunco_assets::tutorials::tutorial_source(&manifest_path) {
            Some(src) => match serde_json::from_str::<Vec<TutorialMeta>>(&src) {
                Ok(metas) => {
                    let mut reg = app.world_mut().resource_mut::<TutorialRegistry>();
                    for meta in metas {
                        reg.register_tutorial(meta);
                    }
                }
                Err(e) => warn!("tutorials manifest '{manifest_path}' failed to parse: {e}"),
            },
            None => warn!("no tutorials manifest found at 'assets/tutorials/{manifest_path}'"),
        }
        app.init_resource::<TutorialHost>();
        app.init_resource::<PendingAdvance>();
        app.register_settings_section::<TutorialProgress>();
        app.register_type::<TutorialSeen>();
        app.register_settings_section::<TutorialSeen>();
        register_all_commands(app);
        app.add_observer(on_mission_complete);
        app.add_observer(resolve_show_tutorial_intent);
        app.init_resource::<LoadedTwinCurriculum>();
        app.add_systems(Update, sync_twin_tutorials);
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
