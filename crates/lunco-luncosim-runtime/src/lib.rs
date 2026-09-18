//! Production application integration for LunCoSim.
//!
//! [`lunco_luncosim_core`] owns the generic simulation substrate. This package
//! owns the application-level scripting boundary: Rhai plugin installation,
//! USD-authored policy projection, policy authoring, and scripting journal
//! consumers. Keeping that boundary here prevents changes to Rhai or policy
//! behavior from invalidating the generic simulation crate.

use bevy::asset::{AssetEvent, AssetServer};
use bevy::prelude::*;

use lunco_usd_bevy_core::UsdStageAsset;
use lunco_usd_bevy_core::program::{
    ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER, DEFAULT_DOMAIN_SYNTHESIZER,
};
use lunco_usd_bevy_core::read::UsdReadObject;

pub use lunco_luncosim_core::AppExit;

/// Install the application-level scripting and policy integration.
pub struct LunCoSimRuntimePlugin {
    /// Whether the host has no presentation surface and should acknowledge
    /// presentation-only scenario commands without executing them.
    pub headless: bool,
}

impl Default for LunCoSimRuntimePlugin {
    fn default() -> Self {
        Self { headless: false }
    }
}

impl Plugin for LunCoSimRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(lunco_scripting::LunCoScriptingPlugin)
            .add_plugins(lunco_scripting_rhai::LunCoScriptingRhaiPlugin);

        register_all_commands(app);

        if self.headless {
            app.insert_resource(lunco_scripting_bridge_core::IgnoredScenarioCommands::new([
                "SetHint",
                "SetObjectives",
                "Spotlight",
                "ClearSpotlight",
                "FocusPanel",
                "SetTourStep",
                "ClearTour",
            ]));
        }

        app.add_systems(
            Update,
            project_usd_policies.after(lunco_scripting::source_asset::RhaiSourceAssetSet),
        );

        #[cfg(feature = "networking")]
        app.add_systems(
            Update,
            (
                replay_scenario_journal_script,
                replay_scenario_journal_tools,
                replay_scenario_journal_timeline,
            ),
        );
    }
}

/// Read the one explicit startup-scene argument, if present.
pub fn startup_scene_arg(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == "--scene")
        .map(|pair| pair[1].clone())
}

/// Build the production headless simulation app with an optional fixed
/// compute-pool size and an explicit startup scene.
pub fn build_headless_app_with_scene(
    compute_threads: Option<usize>,
    startup_scene: Option<String>,
) -> App {
    let mut app = lunco_luncosim_core::build_core_app_with_scene(compute_threads, startup_scene);
    app.add_plugins(LunCoSimRuntimePlugin { headless: true });
    app
}

/// Build the production headless app with an optional fixed compute-pool size.
/// The returned app has no schedule runner, allowing deterministic scene tests
/// to install their own clock and loop.
pub fn build_headless_app_with_threads(compute_threads: Option<usize>) -> App {
    let args: Vec<String> = std::env::args().collect();
    build_headless_app_with_scene(compute_threads, startup_scene_arg(&args))
}

/// Build the normal headless app with the production schedule runner.
pub fn build_headless_app() -> App {
    let mut app = build_headless_app_with_threads(Some(1));
    app.add_plugins(lunco_luncosim_core::LunCoSimHeadlessPlugin::default());
    app
}

/// Run the production headless server.
pub fn run_headless() -> lunco_luncosim_core::AppExit {
    let args: Vec<String> = std::env::args().collect();
    let mode = if args.iter().any(|arg| arg == "--headless-max-speed") {
        "headless-max-speed"
    } else {
        "headless"
    };
    lunco_luncosim_core::log_build_identity(mode);
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "luncosim-server — headless LunCoSim runtime\n\nUsage: luncosim-server [--api PORT] [--scene PATH] [--headless-max-speed]"
        );
        return lunco_luncosim_core::AppExit::Success;
    }
    let execution_mode = if args.iter().any(|arg| arg == "--headless-max-speed") {
        lunco_core::SimulationExecutionMode::MaxSpeed
    } else {
        lunco_core::SimulationExecutionMode::Realtime
    };
    let mut app = build_headless_app_with_threads(Some(1));

    #[cfg(all(
        feature = "api-transport",
        feature = "transport-http",
        not(target_arch = "wasm32")
    ))]
    if let Some(error) = app
        .world_mut()
        .remove_resource::<lunco_api_transport::transports::HttpServerStartupError>()
    {
        eprintln!(
            "luncosim-server: cannot start HTTP API on {}:{}: {}",
            std::net::Ipv4Addr::LOCALHOST,
            error.port,
            error.message
        );
        return lunco_luncosim_core::AppExit::error();
    }

    app.add_plugins(lunco_luncosim_core::LunCoSimHeadlessPlugin { execution_mode });
    app.run()
}

/// The USD type name of a policy prim.
const LUNCO_POLICY_TYPE: &str = "LunCoPolicy";

/// One authored `LunCoPolicy` prim before its Rhai source is resolved.
struct AuthoredPolicy {
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    seam: String,
    entry: String,
    deterministic: bool,
    inline_source: Option<String>,
    source_path: Option<String>,
}

fn append_usd_policies(
    reader: &dyn UsdReadObject,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    out: &mut Vec<AuthoredPolicy>,
) {
    for prim in reader.prim_paths() {
        if reader.type_name(&prim).as_deref() != Some(LUNCO_POLICY_TYPE) {
            continue;
        }
        let seam = reader.text(&prim, "lunco:policy:seam").unwrap_or_default();
        let inline_source = reader
            .text(&prim, "info:sourceCode")
            .filter(|source| !source.is_empty());
        let source_path = reader
            .asset(&prim, "info:sourceAsset")
            .filter(|source| !source.is_empty());
        if seam.is_empty() || (inline_source.is_none() && source_path.is_none()) {
            continue;
        }
        out.push(AuthoredPolicy {
            stage_id,
            seam,
            entry: reader.text(&prim, "lunco:policy:entry").unwrap_or_default(),
            deterministic: reader
                .boolean(&prim, "lunco:policy:deterministic")
                .unwrap_or(true),
            inline_source,
            source_path,
        });
    }
}

fn extract_active_usd_policies(
    stages: &Assets<UsdStageAsset>,
    canonical: &lunco_usd_bevy_core::canonical::CanonicalStages,
    roots: impl IntoIterator<Item = AssetId<UsdStageAsset>>,
) -> Vec<AuthoredPolicy> {
    let mut out = Vec::new();
    for stage_id in roots {
        let Some(stage_asset) = stages.get(stage_id) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(stage_id, stage_asset);
        append_usd_policies(&reader, stage_id, &mut out);
    }
    out
}

enum PolicySource {
    Ready(String),
    Loading,
    Failed,
}

fn resolve_policy_source_file(
    path: &str,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset_server: &AssetServer,
    sources: Option<&Assets<lunco_scripting::source_asset::RhaiSource>>,
    pending: &mut std::collections::HashMap<
        String,
        Handle<lunco_scripting::source_asset::RhaiSource>,
    >,
) -> PolicySource {
    let Some(sources) = sources else {
        warn!("[policy] sourcePath '{path}' authored but the RhaiSource asset loader is absent");
        return PolicySource::Failed;
    };
    let asset_id =
        lunco_usd_bevy_core::asset::resolve_stage_asset_path(asset_server, stage_id, path);
    let handle = pending.entry(asset_id.clone()).or_insert_with(|| {
        asset_server.load(bevy::asset::AssetPath::parse(&asset_id).into_owned())
    });
    let root_failed = asset_server.load_state(&*handle).is_failed();
    let dependencies_failed = asset_server
        .recursive_dependency_load_state(&*handle)
        .is_failed();
    if root_failed || dependencies_failed {
        warn!(
            "[policy] failed to load sourcePath '{path}' as '{asset_id}' via AssetServer \
             (root_failed={root_failed}, dependencies_failed={dependencies_failed})"
        );
        return PolicySource::Failed;
    }
    if !asset_server.is_loaded_with_dependencies(&*handle) {
        return PolicySource::Loading;
    }
    match sources.get(&*handle) {
        Some(src) => PolicySource::Ready(src.text.clone()),
        None => PolicySource::Loading,
    }
}

#[allow(clippy::type_complexity)]
fn project_usd_policies(
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<lunco_usd_bevy_core::canonical::CanonicalStages>,
    roots: Query<&lunco_usd_bevy_scene::UsdPrimPath, With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    mut registry: ResMut<lunco_scripting::policy::ScriptedPolicyRegistry>,
    mut synthesizers: ResMut<lunco_usd_sim_domain::synthesis::SynthesizerRegistry>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    asset_server: Res<AssetServer>,
    sources: Option<Res<Assets<lunco_scripting::source_asset::RhaiSource>>>,
    mut pending: Local<
        std::collections::HashMap<String, Handle<lunco_scripting::source_asset::RhaiSource>>,
    >,
    mut source_events: MessageReader<AssetEvent<lunco_scripting::source_asset::RhaiSource>>,
    mut last: Local<Option<(usize, usize, u64)>>,
    mut awaiting: Local<bool>,
) {
    let source_changed = source_events.read().any(|event| {
        matches!(
            event,
            AssetEvent::Added { .. }
                | AssetEvent::Modified { .. }
                | AssetEvent::Removed { .. }
                | AssetEvent::Unused { .. }
                | AssetEvent::LoadedWithDependencies { .. }
        )
    });
    let root_ids: Vec<_> = roots.iter().map(|prim| prim.stage_handle.id()).collect();
    let signal = (
        root_ids.len(),
        root_ids.iter().filter_map(|id| stages.get(*id)).count(),
        root_ids
            .iter()
            .filter_map(|id| stages.get(*id).map(|_| canonical.generation_for(*id)))
            .sum::<u64>(),
    );
    if *last == Some(signal) && !*awaiting && !source_changed {
        return;
    }
    *last = Some(signal);

    let authored = extract_active_usd_policies(&stages, &canonical, root_ids);
    let live: std::collections::HashSet<String> = authored
        .iter()
        .filter_map(|a| {
            a.source_path.as_deref().map(|path| {
                lunco_usd_bevy_core::asset::resolve_stage_asset_path(
                    &asset_server,
                    a.stage_id,
                    path,
                )
            })
        })
        .collect();
    pending.retain(|p, _| live.contains(p.as_str()));

    let mut desired = Vec::with_capacity(authored.len());
    let mut unresolved = false;
    for a in &authored {
        let source = if let Some(src) = &a.inline_source {
            src.clone()
        } else if let Some(path) = &a.source_path {
            match resolve_policy_source_file(
                path,
                a.stage_id,
                &asset_server,
                sources.as_deref(),
                &mut pending,
            ) {
                PolicySource::Ready(text) => text,
                PolicySource::Loading => {
                    unresolved = true;
                    continue;
                }
                PolicySource::Failed => continue,
            }
        } else {
            continue;
        };
        desired.push(lunco_scripting::policy::PolicyDef {
            seam: a.seam.clone(),
            entry: a.entry.clone(),
            source,
            deterministic: a.deterministic,
        });
    }
    *awaiting = unresolved;
    let previous_synthesizers: std::collections::HashSet<String> = registry
        .policies
        .iter()
        .filter_map(|policy| policy.seam.strip_prefix("synth.").map(str::to_string))
        .collect();
    lunco_scripting::policy::project_policies(desired, &mut registry, journal.as_deref());
    let active_synthesizers: std::collections::HashSet<String> = registry
        .policies
        .iter()
        .filter_map(|policy| policy.seam.strip_prefix("synth.").map(str::to_string))
        .collect();
    for name in previous_synthesizers.difference(&active_synthesizers) {
        if name == DEFAULT_DOMAIN_SYNTHESIZER || name == ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER {
            continue;
        }
        lunco_usd_sim_domain::synthesis::unregister_hook_synthesizer(&mut synthesizers, name);
    }
    for name in active_synthesizers {
        lunco_usd_sim_domain::synthesis::register_hook_synthesizer(&mut synthesizers, name);
    }
}

/// Author or hot-replace a Rhai policy as a `LunCoPolicy` USD prim.
#[lunco_core::Command(default)]
pub struct SetRhaiPolicy {
    /// Prim name under the mounted scene's `Policies` scope.
    pub name: String,
    /// The hook seam (id).
    pub seam: String,
    /// The Rhai entry function name.
    pub entry: String,
    /// The Rhai source defining `entry` and its helpers.
    pub source: String,
    /// Whether the policy is deterministic.
    pub deterministic: bool,
}

#[lunco_core::on_command(SetRhaiPolicy)]
fn on_set_rhai_policy(
    trigger: On<SetRhaiPolicy>,
    backed: Res<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    roots: Query<&lunco_usd_bevy_scene::UsdPrimPath, With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    use lunco_usd_core::commands::ApplyUsdOp;
    use lunco_usd_document::document::{LayerId, UsdOp};

    let cmd = trigger.event();
    let roots: Vec<_> = roots.iter().collect();
    let [root] = roots.as_slice() else {
        warn!(
            "[policy] SetRhaiPolicy needs exactly one mounted USD scene (found {})",
            roots.len()
        );
        return;
    };
    let Some(doc) =
        lunco_usd_bevy_twin::scene_document_for(&backed, &asset_server, root.stage_handle.id())
    else {
        warn!(
            "[policy] the mounted scene is not Twin document-backed; open it through a Twin to author a policy"
        );
        return;
    };
    let mounted_root = root.path.trim_end_matches('/');
    let mounted_root = if mounted_root.is_empty() {
        "/"
    } else {
        mounted_root
    };
    let base = if cmd.name.is_empty() {
        &cmd.seam
    } else {
        &cmd.name
    };
    let mut name: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if name.is_empty() {
        name = "policy".to_string();
    }
    let policies_path = if mounted_root == "/" {
        "/Policies".to_string()
    } else {
        format!("{mounted_root}/Policies")
    };
    let prim = format!("{policies_path}/{name}");
    let root = LayerId::root();
    let ops = vec![
        UsdOp::AddPrim {
            edit_target: root.clone(),
            parent_path: mounted_root.into(),
            name: "Policies".into(),
            type_name: Some("Scope".into()),
            reference: None,
            reference_prim_path: None,
        },
        UsdOp::AddPrim {
            edit_target: root.clone(),
            parent_path: policies_path,
            name,
            type_name: Some("LunCoPolicy".into()),
            reference: None,
            reference_prim_path: None,
        },
        UsdOp::SetAttribute {
            edit_target: root.clone(),
            path: prim.clone(),
            name: "lunco:policy:seam".into(),
            type_name: "string".into(),
            value: cmd.seam.clone(),
        },
        UsdOp::SetAttribute {
            edit_target: root.clone(),
            path: prim.clone(),
            name: "lunco:policy:entry".into(),
            type_name: "string".into(),
            value: cmd.entry.clone(),
        },
        UsdOp::SetAttribute {
            edit_target: root.clone(),
            path: prim.clone(),
            name: "info:sourceCode".into(),
            type_name: "string".into(),
            value: cmd.source.clone(),
        },
        UsdOp::SetAttribute {
            edit_target: root,
            path: prim.clone(),
            name: "lunco:policy:deterministic".into(),
            type_name: "bool".into(),
            value: cmd.deterministic.to_string(),
        },
    ];
    for op in ops {
        commands.trigger(ApplyUsdOp {
            doc_id: doc,
            parent_gen: None,
            op,
        });
    }
    info!(
        "[policy] SetRhaiPolicy authored `{prim}` (seam '{}') — journals + projects",
        cmd.seam
    );
}

lunco_core::register_commands!(on_set_rhai_policy);

#[cfg(feature = "networking")]
fn replay_scenario_journal_script(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    registry: Option<ResMut<lunco_scripting::ScriptRegistry>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut registry)) = (journal, registry) else {
        return;
    };
    let base: Option<lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        lunco_networking_sync::scenario_sync::manifest_journal_head(Some(manifest))
    };
    let docs: Vec<_> = registry.documents.keys().copied().collect();
    let [doc] = docs.as_slice() else {
        return;
    };
    let me = journal.local_author();
    let pending = lunco_networking_sync::journal_plane::domain_ops_after(
        &journal,
        base.as_ref(),
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Script,
    );
    for (id, op) in pending {
        registry.replay_op(*doc, &op);
        applied.insert(id);
    }
}

#[cfg(feature = "networking")]
fn replay_scenario_journal_tools(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scoped: Option<ResMut<lunco_scripting::tool_libs::TwinToolLibraries>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let Some(journal) = journal else {
        return;
    };
    let Some(workspace) = workspace.as_deref() else {
        return;
    };
    let Some(active) = workspace.active_twin else {
        return;
    };
    let Some(mut scoped) = scoped else {
        return;
    };
    scoped.ensure_active(active);
    let base: Option<lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        lunco_networking_sync::scenario_sync::manifest_journal_head(Some(manifest))
    };
    let me = journal.local_author();
    let pending = lunco_networking_sync::journal_plane::domain_ops_after(
        &journal,
        base.as_ref(),
        &me,
        &applied,
        lunco_twin_journal::DomainKind::ToolLibrary,
    );
    for (id, op) in pending {
        if let Some((name, source)) =
            lunco_scripting::registration_journal::replay_tool_library(&op)
        {
            if let Err(error) = scoped.register(active, &name, &source) {
                warn!("[tool_libs] ignored journal replay outside its active scope: {error}");
            }
        }
        applied.insert(id);
    }
}

#[cfg(feature = "networking")]
fn replay_scenario_journal_timeline(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    store: Option<ResMut<lunco_scripting::timelines::TimelineStore>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut store)) = (journal, store) else {
        return;
    };
    let Ok(owner) = lunco_scripting::timelines::active_owner(workspace.as_deref()) else {
        return;
    };
    if !matches!(owner, lunco_scripting::timelines::TimelineOwner::Twin(_)) {
        return;
    }
    store.ensure_scope(owner);
    let base: Option<lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        lunco_networking_sync::scenario_sync::manifest_journal_head(Some(manifest))
    };
    let me = journal.local_author();
    let pending = lunco_networking_sync::journal_plane::domain_ops_after(
        &journal,
        base.as_ref(),
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Timeline,
    );
    for (id, op) in pending {
        if let Some((name, timeline)) = lunco_scripting::registration_journal::replay_timeline(&op)
        {
            if let Err(error) = store.insert_for(owner, name, timeline) {
                warn!("[timeline] ignored journal replay outside its active scope: {error:?}");
            }
        }
        applied.insert(id);
    }
}
