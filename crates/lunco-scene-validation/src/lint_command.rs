//! `RunLint` — lint the LOADED scene, on demand.
//!
//! # Why this is a command and not a load-time pass
//!
//! Linting is something you RUN, not something that runs at you. A check that
//! fires on every scene load trains its reader to scroll past it, costs a stage
//! walk nobody asked for, and turns an opinion about authoring into a tax on
//! playing. So nothing lints automatically: `RunLint` is a verb, reachable
//! everywhere a verb is —
//!
//! ```text
//!   cmd("RunLint", #{})                       // explicit authoring/preflight check
//!   {"type":"ExecuteCommand","command":"RunLint","params":{}} // HTTP / MCP
//! ```
//!
//! There is no cadence, background watcher or per-tick lint monitor. An editor,
//! launcher or caller explicitly invokes the command again after an authored edit.
//!
//! # What it lints
//!
//! Every composed stage currently loaded, through the domain's authored rules
//! (`assets/scripting/policy/lint_usd.rhai`, hook `lint.usd`) over the complete
//! USD facts assembled from the standard-joint and USD-sim projection owners.
//! Rules are rhai: edit and bind the `lint.usd` policy, and the NEXT `RunLint` obeys
//! them, on a running sim, with no rebuild.
//!
//! `ValidateAsset` runs the same rules over the same facts for a FILE. This runs
//! them over what is actually loaded — which, after runtime spawns and edits, is
//! not the same stage any file describes. `RunLint { scope: "twin" }` instead
//! inspects the active Twin's resolver namespaces through the shared Twin
//! inspector; its file-only counterpart is `ValidateTwin`.

use bevy::prelude::*;
use lunco_api::ApiQueryResult;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api_core::{ApiValue, api_value};
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Diagnostic, DiagnosticSourceReport, DiagnosticSourceState, Document, DocumentId};
use lunco_doc_bevy::{DocumentChanged, DocumentRegistry};
use lunco_hooks::HookValue as H;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_bevy_stage::{StageView, UsdRead, UsdStageAsset, canonical::CanonicalStages};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Build the complete USD lint fact map from every owner of a USD simulation
/// projection. Standard `Physics*Joint` facts come from
/// `lunco-usd-avian-lint`;
/// `PhysxPhysicsGearJoint` facts come from the `lunco-usd-sim-authoring` reader
/// that owns that projection. The policy sees one map and one authoritative value for
/// each subject.
pub(crate) fn usd_physics_facts(view: &StageView<'_>) -> H {
    usd_physics_facts_with_control_info(view).0
}

fn lint_command_error(
    domain: &str,
    code: &str,
    subject: impl Into<String>,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic::error(message, None, None)
        .with_domain(domain)
        .with_source("RunLint")
        .with_code(code)
        .with_subject(subject)
}

fn fail_lint_scope(
    report: &mut lunco_lint::LintReport,
    scope: &str,
    revision: u64,
    finding: Diagnostic,
) {
    let message = finding.message.clone();
    report.extend_logged(vec![finding]);
    report.fail_scope(scope, revision, message);
}

fn on_usd_document_changed(
    trigger: On<DocumentChanged>,
    registry: Res<DocumentRegistry<lunco_usd_document::document::UsdDocument>>,
    mut diagnostics: ResMut<lunco_doc_bevy::DocumentDiagnostics>,
) {
    let doc_id = trigger.event().doc;
    let Some(host) = registry.host(doc_id) else {
        return;
    };
    let generation = host.document().generation();
    let Some(previous) = diagnostics
        .get(doc_id)
        .and_then(|entry| entry.sources.get("usd.document-lint"))
    else {
        return;
    };
    if previous.generation == generation {
        return;
    }
    let revision = previous.revision.unwrap_or(0).saturating_add(1);
    diagnostics.set_source_report(
        doc_id,
        DiagnosticSourceReport {
            id: "usd.document-lint".to_owned(),
            domain: "usd".to_owned(),
            generation,
            revision: Some(revision),
            state: DiagnosticSourceState::Pending,
            message: Some("The source changed since this report. Run `RunLint` to check the current generation".to_owned()),
            diagnostics: Vec::new(),
        },
    );
}

/// Build the complete USD lint facts and the structured control-binding
/// projection used by `ValidateAsset`'s JSON report.
pub(crate) fn usd_physics_facts_with_control_info(
    view: &StageView<'_>,
) -> (H, Vec<serde_json::Value>) {
    let mut facts = lunco_usd_avian_lint::physics_facts(view);
    append_network_synthesizer_facts(view, &mut facts);
    lunco_usd_sim_authoring::append_gear_drive_facts(view, &mut facts);
    lunco_usd_sim_authoring::append_wheel_attachment_facts(view, &mut facts);
    let (bindings, info) = control_binding_facts(view);
    if let H::Map(entries) = &mut facts {
        entries.push(("runtime_connections".to_string(), H::Array(Vec::new())));
        entries.push(("runtime_joints".to_string(), H::Array(Vec::new())));
        entries.push(("runtime_port_collisions".to_string(), H::Array(Vec::new())));
        entries.push(("control_bindings".to_string(), H::Array(bindings)));
        entries.push(("shader_interfaces".to_string(), H::Array(Vec::new())));
    }
    (facts, info)
}

/// Add the domain owner selected by the same composed-USD classifier used by
/// runtime domain projection. This is an aggregation concern: it enriches the
/// complete validation fact table without coupling the vehicle authoring
/// readers to Modelica/network policy.
fn append_network_synthesizer_facts(view: &StageView<'_>, facts: &mut H) {
    let H::Map(entries) = facts else {
        return;
    };
    let Some((_, H::Array(scopes))) = entries.iter_mut().find(|(key, _)| key == "network_roots")
    else {
        return;
    };

    for scope in scopes {
        let Some(path) = scope.get("path").and_then(H::as_str) else {
            continue;
        };
        let Ok(root) = openusd::sdf::Path::new(path) else {
            set_scope_fact(scope, "synthesizer", H::str("invalid"));
            set_scope_fact(
                scope,
                "synthesizer_error",
                H::str("network root path is not a valid absolute USD path"),
            );
            continue;
        };

        match lunco_usd_bevy_core::program::select_synthesizer_name(view, &root) {
            Ok(name) => {
                set_scope_fact(scope, "synthesizer", H::str(name));
                set_scope_fact(scope, "synthesizer_error", H::str(""));
            }
            Err(error) => {
                set_scope_fact(scope, "synthesizer", H::str("invalid"));
                set_scope_fact(scope, "synthesizer_error", H::str(error));
            }
        }
    }
}

fn set_scope_fact(scope: &mut H, key: &str, value: H) {
    let H::Map(entries) = scope else {
        return;
    };
    if let Some((_, existing)) = entries.iter_mut().find(|(name, _)| name == key) {
        *existing = value;
    } else {
        entries.push((key.to_string(), value));
    }
}

/// Project the composed controls shape into policy facts.
///
/// Rust supplies only authored identity, port text, and the validity bit from
/// the parser used by the loader. The Rhai policy owns severity and wording.
fn control_binding_facts(view: &StageView<'_>) -> (Vec<H>, Vec<serde_json::Value>) {
    let mut facts = Vec::new();
    let mut info = Vec::new();
    for prim in view.prim_paths() {
        if !is_controls_scope(view, &prim) {
            continue;
        }
        for bind in view.children(&prim) {
            let Some(name) = bind.name() else { continue };
            let valid = lunco_control_core::parse_user_intent(name).is_some();
            let port = view.text(&bind, "lunco:port");
            facts.push(H::map([
                ("prim", H::str(bind.as_str())),
                ("intent", H::str(name)),
                ("port", port.clone().map(H::str).unwrap_or(H::Unit)),
                ("valid", H::Bool(valid)),
            ]));
            info.push(json!({
                "prim": bind.as_str(),
                "intent": name,
                "port": port,
                "ok": valid,
            }));
        }
    }
    (facts, info)
}

/// A prim whose children carry intent→port bindings. The shape decides, not a
/// prim name: shared control profiles are often composed under vessel-specific
/// scopes.
fn is_controls_scope(view: &impl UsdRead, prim: &openusd::sdf::Path) -> bool {
    view.children(prim)
        .iter()
        .any(|c| c.name().is_some() && view.attr_names(c).iter().any(|a| a == "lunco:port"))
}

/// Run the complete live USD lint pipeline over one composed stage.
///
/// This aggregation point owns the cross-domain fact table: standard physics
/// facts come from `lunco-usd-avian-lint`, while `lunco-usd-sim-authoring` owns
/// its gear and wheel facts and this module owns synthesizer aggregation. Callers must use this entry point rather than
/// linting a partial producer's facts.
pub fn lint_stage(view: &StageView<'_>) -> Vec<Diagnostic> {
    lunco_lint::run_lint(
        lunco_usd_avian_lint::USD_LINT_DOMAIN,
        usd_physics_facts(view),
    )
}

/// Project already resolved runtime port-owner collisions into USD lint facts.
/// The registry supplies ownership; Rhai policy decides finding severity and wording.
fn live_runtime_port_collision_facts(
    world: &World,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
) -> Vec<H> {
    let Some(registry) = world.get_resource::<lunco_port_core::ports::PortRegistry>() else {
        return Vec::new();
    };

    // A composed prim has one projected entity per stage/path. Keep the first
    // path deterministically if a transient projection view duplicates it.
    let mut entities = BTreeMap::new();
    for entity in world.iter_entities() {
        let Some(prim) = entity.get::<UsdPrimPath>() else {
            continue;
        };
        if prim.stage_handle.id() == stage_id {
            entities
                .entry(prim.path.clone())
                .or_insert_with(|| entity.id());
        }
    }

    let mut facts = Vec::new();
    for (entity_path, entity) in entities {
        for collision in registry.entity_port_collisions(world, entity) {
            let direction = match collision.direction {
                lunco_port_core::ports::PortCollisionDirection::Input => "input",
                lunco_port_core::ports::PortCollisionDirection::Output => "output",
                lunco_port_core::ports::PortCollisionDirection::InOut => "inout",
            };
            let owners = collision
                .owners
                .iter()
                .map(|owner| {
                    let direction = match owner.direction {
                        lunco_port_core::ports::PortDirection::In => "in",
                        lunco_port_core::ports::PortDirection::Out => "out",
                        lunco_port_core::ports::PortDirection::InOut => "inout",
                    };
                    H::map([
                        ("source", H::str(owner.metadata.source.clone())),
                        ("direction", H::str(direction)),
                        ("precedence", H::Int(owner.precedence as i64)),
                    ])
                })
                .collect();
            facts.push(H::map([
                ("subject", H::str(entity_path.clone())),
                ("name", H::str(collision.name)),
                ("direction", H::str(direction)),
                ("owners", H::Array(owners)),
            ]));
        }
    }
    facts
}

/// Validate source endpoints that are intentionally absent from authored USD
/// because their provider publishes them at projection time. The authored lint
/// can prove that the source prim exists and carries a known provider schema;
/// only the live registry can prove that the requested dynamic port name and
/// direction actually exist on the projected entity.
fn live_runtime_connection_facts(
    world: &World,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    view: &StageView<'_>,
) -> Vec<H> {
    let Some(registry) = world.get_resource::<lunco_port_core::ports::PortRegistry>() else {
        return Vec::new();
    };
    let mut entities = BTreeMap::new();
    for entity in world.iter_entities() {
        let Some(prim) = entity.get::<UsdPrimPath>() else {
            continue;
        };
        if prim.stage_handle.id() == stage_id {
            entities
                .entry(prim.path.clone())
                .or_insert_with(|| entity.id());
        }
    }

    let mut facts = Vec::new();
    for sink in view.prim_paths() {
        for attribute in view.attr_names(&sink) {
            for source in view.connections(&sink, &attribute) {
                let Ok(source_path) = openusd::sdf::Path::new(&source) else {
                    continue;
                };
                let Some((source_prim, source_property)) = source_path.split_property() else {
                    continue;
                };
                if view.attr_type_name(&source_prim, source_property).is_some()
                    || !lunco_usd_bevy_stage::read::has_runtime_port_surface(view, &source_prim)
                {
                    continue;
                }
                let (direction, source_name) = source_property
                    .strip_prefix("outputs:")
                    .map(|name| ("output", name))
                    .or_else(|| {
                        source_property
                            .strip_prefix("inputs:")
                            .map(|name| ("input", name))
                    })
                    .unwrap_or(("", source_property));
                let source_entity = entities.get(source_prim.as_str()).copied();
                let has_port = source_entity.is_some_and(|entity| {
                    if direction == "output" {
                        registry.has_output_port(world, entity, source_name)
                    } else {
                        registry.has_input_port(world, entity, source_name)
                    }
                });
                let pending = source_entity
                    .and_then(|entity| world.get::<lunco_port_core::PortSurfacePending>(entity))
                    .is_some();
                let provider =
                    lunco_usd_bevy_stage::read::runtime_port_provider(view, &source_prim)
                        .unwrap_or("runtime provider");
                facts.push(H::map([
                    ("subject", H::str(format!("{}.{}", sink, attribute))),
                    ("source", H::str(source)),
                    ("source_prim", H::str(source_prim.to_string())),
                    ("source_property", H::str(source_property.to_string())),
                    ("direction", H::str(direction)),
                    ("port_name", H::str(source_name)),
                    ("provider", H::str(provider)),
                    ("projected", H::Bool(source_entity.is_some())),
                    ("port_exists", H::Bool(has_port)),
                    ("pending", H::Bool(pending)),
                ]));
            }
        }
    }
    facts
}

/// Validate the projected joint lifecycle against the one generic detach
/// marker. The Avian bridge owns the evidence; this layer only converts it to
/// the shared Rhai fact shape so the authored policy decides severity/wording.
fn live_runtime_joint_facts(
    world: &World,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
) -> Vec<H> {
    lunco_usd_avian::runtime_joint_facts(world, stage_id)
        .into_iter()
        .map(|joint| {
            H::map([
                ("entity_bits", H::Int(joint.entity_bits as i64)),
                (
                    "subject",
                    H::str(
                        joint
                            .path
                            .unwrap_or_else(|| format!("entity:{}", joint.entity_bits)),
                    ),
                ),
                ("linked", H::Bool(joint.linked)),
                ("pending", H::Bool(joint.pending)),
                ("native", H::Bool(joint.native)),
                ("graph", H::Bool(joint.graph)),
                ("graph_available", H::Bool(joint.graph_available)),
                ("detach_requested", H::Bool(joint.detach_requested)),
            ])
        })
        .collect()
}

/// Project loaded shader ABI facts and identify the body looks consumed by a
/// generated DEM continuation. The linter policy owns whether a mismatch is an
/// error and how the authored USD should be repaired.
fn live_shader_interface_facts(
    world: &World,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
) -> Vec<H> {
    let continuation_sources: BTreeSet<_> = world
        .iter_entities()
        .filter_map(|entity| {
            entity
                .get::<lunco_terrain_surface::TerrainVisualContinuation>()
                .map(|continuation| continuation.surface_source)
        })
        .collect();

    world
        .iter_entities()
        .filter_map(|entity| {
            let prim = entity.get::<UsdPrimPath>()?;
            if prim.stage_handle.id() != stage_id {
                return None;
            }
            let look = entity.get::<lunco_materials::ShaderLook>()?;
            let reflection = entity.get::<lunco_materials::ShaderLookSourceInterface>();
            let reflected = reflection.is_some_and(|source| source.shader == look.shader);
            let continuation_required = continuation_sources.contains(&entity.id());
            Some(H::map([
                ("subject", H::str(prim.path.clone())),
                ("shader_path", H::str(look.shader.clone())),
                (
                    "declared_interface",
                    H::str(look.interface.as_deref().unwrap_or("")),
                ),
                (
                    "actual_interface",
                    H::str(
                        reflection
                            .filter(|source| source.shader == look.shader)
                            .and_then(|source| source.identifier.as_deref())
                            .unwrap_or(""),
                    ),
                ),
                ("source_reflected", H::Bool(reflected)),
                (
                    "source_valid",
                    H::Bool(reflected && reflection.is_some_and(|source| source.source_valid)),
                ),
                ("continuation_required", H::Bool(continuation_required)),
                (
                    "expected_interface",
                    H::str(if continuation_required {
                        lunco_terrain_surface::stream_viz::LUNAR_SURFACE_CONTINUATION_INTERFACE
                    } else {
                        ""
                    }),
                ),
            ]))
        })
        .collect()
}

/// Run the USD policy over authored facts plus the live runtime facts.
///
/// Rust supplies evidence; Rhai owns finding policy and user-facing wording.
fn lint_stage_with_runtime(
    world: &World,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    view: &StageView<'_>,
) -> Vec<Diagnostic> {
    let mut facts = usd_physics_facts(view);
    if let H::Map(entries) = &mut facts {
        let runtime = H::Array(live_runtime_connection_facts(world, stage_id, view));
        if let Some((_, existing)) = entries
            .iter_mut()
            .find(|(key, _)| key == "runtime_connections")
        {
            *existing = runtime;
        } else {
            entries.push(("runtime_connections".to_string(), runtime));
        }
        entries.push((
            "runtime_joints".to_string(),
            H::Array(live_runtime_joint_facts(world, stage_id)),
        ));
        entries.push((
            "runtime_port_collisions".to_string(),
            H::Array(live_runtime_port_collision_facts(world, stage_id)),
        ));
        let shader_interfaces = H::Array(live_shader_interface_facts(world, stage_id));
        if let Some((_, existing)) = entries
            .iter_mut()
            .find(|(key, _)| key == "shader_interfaces")
        {
            *existing = shader_interfaces;
        } else {
            entries.push(("shader_interfaces".to_string(), shader_interfaces));
        }
    }
    lunco_lint::run_lint(lunco_usd_avian_lint::USD_LINT_DOMAIN, facts)
}

/// Lint what is loaded now.
///
/// Findings land in [`lunco_lint::LintReport`] (readable via `GetDiagnostics`)
/// and are logged — errors at `error!`, warnings at `warn!`.
#[Command(default)]
pub struct RunLint {
    /// Restrict to one lint domain (`"usd"`). Empty = every domain this scene
    /// can produce facts for. Named rather than enumerated so a domain added
    /// later needs no change to this verb.
    pub domain: String,
    /// Inspection scope. Empty or `"loaded_stages"` keeps the existing live
    /// stage behavior; `"twin"` inspects the active Twin's resolver namespaces.
    #[serde(default)]
    pub scope: String,
    /// Twin namespace severity policy: `"warn"` (default) or `"error"`.
    /// The policy is passed to authored Rhai; facts and collision ownership stay
    /// in the generic Rust inspection path.
    #[serde(default)]
    pub policy: String,
    /// When present, lint exactly this open Editor document after its projected
    /// stage reaches the document generation. Omitted keeps the loaded-scene
    /// behavior for live simulation callers.
    #[serde(default)]
    pub doc_id: Option<u64>,
}

/// Observer for [`RunLint`].
#[on_command(RunLint)]
pub fn on_run_lint(
    trigger: On<RunLint>,
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
    // NonSend: the canonical stages hold non-Send USD data. Same treatment as
    // `on_spawn_entity_command`.
    mut canonical: NonSendMut<CanonicalStages>,
    mut report: ResMut<lunco_lint::LintReport>,
    mut document_diagnostics: ResMut<lunco_doc_bevy::DocumentDiagnostics>,
    asset_server: Option<Res<AssetServer>>,
    documents: Option<Res<DocumentRegistry<lunco_usd_document::document::UsdDocument>>>,
    backed: Option<Res<lunco_usd_bevy_twin::DocBackedTwinScenes>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) {
    report.clear_domain("lint");
    let scope = trigger.event().scope.trim();
    if scope == "twin" {
        let revision = report.begin_scope("twin");
        report.clear_domain("twin");
        let domain = trigger.event().domain.trim();
        if !domain.is_empty() && domain != "twin" {
            fail_lint_scope(
                &mut report,
                "twin",
                revision,
                lint_command_error(
                    "twin",
                    "invalid-twin-lint-domain",
                    "RunLint",
                    format!(
                        "scope `twin` cannot be combined with domain `{domain}`; omit domain or use `twin`"
                    ),
                ),
            );
            return;
        }
        if trigger.event().doc_id.is_some() {
            fail_lint_scope(
                &mut report,
                "twin",
                revision,
                lint_command_error(
                    "twin",
                    "invalid-twin-lint-document",
                    "RunLint",
                    "scope `twin` inspects the active Twin and cannot take doc_id",
                ),
            );
            return;
        }
        let policy = match crate::twin_lint::policy_name(&trigger.event().policy) {
            Ok(policy) => policy,
            Err(message) => {
                fail_lint_scope(
                    &mut report,
                    "twin",
                    revision,
                    lint_command_error("twin", "invalid-twin-lint-policy", "RunLint", message),
                );
                return;
            }
        };
        let Some(workspace) = workspace.as_deref() else {
            fail_lint_scope(
                &mut report,
                "twin",
                revision,
                lint_command_error(
                    "twin",
                    "twin-lint-no-workspace",
                    "RunLint",
                    "Twin namespace lint requires the Workspace resource",
                ),
            );
            return;
        };
        let Some(twin_id) = workspace.active_twin else {
            fail_lint_scope(
                &mut report,
                "twin",
                revision,
                lint_command_error(
                    "twin",
                    "twin-lint-no-active-twin",
                    "Workspace",
                    "Twin namespace lint requires an active Twin",
                ),
            );
            return;
        };
        let Some(twin) = workspace.twin(twin_id) else {
            fail_lint_scope(
                &mut report,
                "twin",
                revision,
                lint_command_error(
                    "twin",
                    "twin-lint-missing-active-twin",
                    format!("TwinId({})", twin_id.raw()),
                    "Workspace active_twin does not resolve to an open Twin",
                ),
            );
            return;
        };
        let snapshot = crate::twin_lint::inspect_twin(twin);
        let findings = lunco_lint::run_lint("twin", crate::twin_lint::facts(&snapshot, policy));
        report.extend_logged(findings);
        report.complete_scope("twin", revision);
        info!(
            "[lint] RunLint: Twin `{}` — {} namespace collision(s), {} source read error(s), policy={policy}",
            snapshot.twin,
            snapshot.collisions.len(),
            snapshot.read_errors.len(),
        );
        return;
    }
    if !scope.is_empty() && scope != "loaded_stages" {
        let revision = report.begin_scope("loaded_stages");
        report.clear_domain(lunco_usd_avian_lint::USD_LINT_DOMAIN);
        fail_lint_scope(
            &mut report,
            "loaded_stages",
            revision,
            lint_command_error(
                "lint",
                "invalid-lint-scope",
                "RunLint",
                format!("unknown lint scope `{scope}`; use `loaded_stages` or `twin`"),
            ),
        );
        return;
    }
    let domain = trigger.event().domain.trim().to_string();
    if !domain.is_empty() && domain != lunco_usd_avian_lint::USD_LINT_DOMAIN {
        let revision = report.begin_scope("loaded_stages");
        report.clear_domain(lunco_usd_avian_lint::USD_LINT_DOMAIN);
        fail_lint_scope(
            &mut report,
            "loaded_stages",
            revision,
            lint_command_error(
                "lint",
                "live-lint-domain-unavailable",
                "RunLint",
                format!(
                    "RunLint has no live producer for `{domain}`; use GetDiagnostics for open-document diagnostics or ValidateAsset for file preflight"
                ),
            ),
        );
        return;
    }

    if let Some(raw_doc) = trigger.event().doc_id {
        let doc = DocumentId::new(raw_doc);
        let Some(host) = documents.as_deref().and_then(|registry| registry.host(doc)) else {
            warn!("[lint] RunLint: document {doc} is not open");
            return;
        };
        let generation = host.document().generation();
        let revision = document_diagnostics
            .get(doc)
            .and_then(|report| report.sources.get("usd.document-lint"))
            .and_then(|report| report.revision)
            .unwrap_or(0)
            .saturating_add(1);
        let ready = backed
            .as_deref()
            .and_then(|scenes| scenes.synced_generation(doc))
            == Some(generation);
        if !ready {
            document_diagnostics.set_source_report(
                doc,
                lunco_doc::DiagnosticSourceReport {
                    id: "usd.document-lint".to_owned(),
                    domain: "usd".to_owned(),
                    generation,
                    revision: Some(revision),
                    state: lunco_doc::DiagnosticSourceState::Pending,
                    message: Some(
                        "Waiting for the USD document projection to reach this source generation"
                            .to_owned(),
                    ),
                    diagnostics: Vec::new(),
                },
            );
            warn!(
                "[lint] RunLint: document {doc} projection is not current (generation {generation})"
            );
            return;
        }

        let stage_handle = asset_server.as_deref().and_then(|server| {
            backed
                .as_deref()
                .and_then(|scenes| scenes.coords_of(doc))
                .map(|(name, rel)| lunco_assets_core::twin_uri(&name, &rel))
                .and_then(|path| server.get_handle::<UsdStageAsset>(path))
        });
        let stage_id = stage_handle.as_ref().map(|handle| handle.id());
        let stage = stage_handle.and_then(|handle| canonical.get(handle.id()));
        let Some(_stage) = stage else {
            document_diagnostics.set_source_report(
                doc,
                lunco_doc::DiagnosticSourceReport {
                    id: "usd.document-lint".to_owned(),
                    domain: "usd".to_owned(),
                    generation,
                    revision: Some(revision),
                    state: lunco_doc::DiagnosticSourceState::Failed(
                        "Projected USD document has no canonical composed stage".to_owned(),
                    ),
                    message: None,
                    diagnostics: Vec::new(),
                },
            );
            warn!("[lint] RunLint: document {doc} has no projected canonical stage");
            return;
        };
        document_diagnostics.set_source_report(
            doc,
            lunco_doc::DiagnosticSourceReport {
                id: "usd.document-lint".to_owned(),
                domain: "usd".to_owned(),
                generation,
                revision: Some(revision),
                state: lunco_doc::DiagnosticSourceState::Pending,
                message: Some("Running lint on the current USD document projection".to_owned()),
                diagnostics: Vec::new(),
            },
        );
        if let Some(stage_id) = stage_id {
            commands.queue(move |world: &mut World| {
                let findings = world
                    .get_non_send::<CanonicalStages>()
                    .and_then(|canonical| canonical.get(stage_id))
                    .map(|stage| lint_stage_with_runtime(world, stage_id, &stage.view()))
                    .unwrap_or_default();
                if let Some(mut diagnostics) =
                    world.get_resource_mut::<lunco_doc_bevy::DocumentDiagnostics>()
                {
                    diagnostics.set_source_report(
                        doc,
                        lunco_doc::DiagnosticSourceReport {
                            id: "usd.document-lint".to_owned(),
                            domain: "usd".to_owned(),
                            generation,
                            revision: Some(revision),
                            state: lunco_doc::DiagnosticSourceState::Ready,
                            message: None,
                            diagnostics: findings,
                        },
                    );
                }
            });
        }
        info!("[lint] RunLint: document {doc} — report queued after live projection check");
        return;
    }

    // Re-linting REPLACES this domain's findings: a rule that was fixed between
    // two runs must disappear, not accumulate a second copy.
    let revision = report.begin_scope("loaded_stages");
    report.clear_domain(lunco_usd_avian_lint::USD_LINT_DOMAIN);

    // Every loaded stage, composed. `get_or_build` is what the loader itself
    // calls, so this lints exactly what physics reads.
    let ids: Vec<_> = stages.ids().collect();
    let mut linted_ids = Vec::new();
    for id in ids {
        if canonical.get(id).is_none() {
            let Some(recipe) = stages.get(id).and_then(|a| a.recipe.clone()) else {
                continue;
            };
            canonical.get_or_build(id, &recipe);
        }
        let Some(_cs) = canonical.get(id) else {
            continue;
        };
        linted_ids.push(id);
    }
    let linted = linted_ids.len();
    commands.queue(move |world: &mut World| {
        let mut findings = Vec::new();
        for id in linted_ids {
            if let Some(stage) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|canonical| canonical.get(id))
            {
                let view = stage.view();
                findings.extend(lint_stage_with_runtime(world, id, &view));
            }
        }
        if let Some(mut report) = world.get_resource_mut::<lunco_lint::LintReport>() {
            if report
                .scopes
                .get("loaded_stages")
                .is_none_or(|scope| scope.revision != revision)
            {
                return;
            }
            report.extend_logged(findings);
            report.complete_scope("loaded_stages", revision);
        }
    });

    info!("[lint] RunLint: queued {linted} stage(s) for composed and live checks");
}

/// Read structural runtime diagnostics from owning subsystems. These findings
/// complement `LintReport`: lint is an explicit authoring pass, while these
/// are live admission/ownership errors that must remain highlighted as the
/// loaded scene changes.
pub struct RuntimeDiagnosticsQuery;

impl ApiQueryProvider for RuntimeDiagnosticsQuery {
    fn name(&self) -> &'static str {
        "RuntimeDiagnostics"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let findings = world
            .get_resource::<lunco_core::RuntimeDiagnostics>()
            .map(|diagnostics| {
                diagnostics
                    .findings
                    .iter()
                    .map(|finding| {
                        api_value!({
                            "code": finding.code.clone(),
                            "severity": finding.severity.as_str(),
                            "producer": finding.producer.clone(),
                            "subject": finding.subject.clone(),
                            "message": finding.message.clone(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let errors = findings
            .iter()
            .filter(|finding| finding.get("severity").and_then(ApiValue::as_str) == Some("error"))
            .count();
        let warnings = findings
            .iter()
            .filter(|finding| finding.get("severity").and_then(ApiValue::as_str) == Some("warning"))
            .count();
        Ok(Some(api_value!({
            "errors": errors,
            "warnings": warnings,
            "findings": findings,
        })))
    }
}

/// Register the query alongside the command (the command registers itself with
/// the rest of this crate's verbs).
pub fn register(app: &mut App) {
    app.init_resource::<lunco_lint::LintReport>();
    app.init_resource::<lunco_doc_bevy::DocumentDiagnostics>();
    // Findings belong to the loaded scene. A replacement must not leave the
    // previous scene's errors highlighted as if they were current.
    app.add_systems(lunco_core::SceneTeardown, lunco_lint::clear_report);
    app.add_observer(on_usd_document_changed);
    lunco_api::add_plugin_once::<lunco_api::ApiQueryRegistryPlugin>(
        app,
        lunco_api::ApiQueryRegistryPlugin,
    );
    let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
    registry.register(RuntimeDiagnosticsQuery);
}

register_commands!(on_run_lint);
#[cfg(test)]
mod tests {
    use super::live_port_collision_findings;
    use bevy::asset::Handle;
    use bevy::prelude::*;
    use lunco_port_core::ports::{
        PortBackend, PortDeclaration, PortDirection, PortMetadata, PortRegistry,
    };
    use lunco_usd_bevy_scene::UsdPrimPath;
    use lunco_usd_bevy_stage::{UsdRead, UsdStageAsset, canonical::CanonicalStage};
    use lunco_usd_compose::recipe::StageRecipe;

    #[derive(Component)]
    struct ModelicaInput;

    #[derive(Component)]
    struct RuntimeActuator {
        name: String,
    }

    fn modelica_list(world: &World, entity: Entity, out: &mut Vec<PortDeclaration>) {
        if world.get::<ModelicaInput>(entity).is_some() {
            out.push(PortDeclaration {
                name: "release".into(),
                direction: PortDirection::In,
            });
        }
    }

    fn runtime_actuator_list(world: &World, entity: Entity, out: &mut Vec<PortDeclaration>) {
        if let Some(actuator) = world.get::<RuntimeActuator>(entity) {
            out.push(PortDeclaration {
                name: actuator.name.clone(),
                direction: PortDirection::InOut,
            });
        }
    }

    fn modelica_input_slot(world: &World, entity: Entity, name: &str) -> Option<u64> {
        (name == "release" && world.get::<ModelicaInput>(entity).is_some()).then_some(0)
    }

    fn runtime_actuator_input_slot(world: &World, entity: Entity, name: &str) -> Option<u64> {
        world
            .get::<RuntimeActuator>(entity)
            .is_some_and(|actuator| actuator.name == name)
            .then_some(0)
    }

    fn modelica_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "Modelica/OBC",
            "solver",
            false,
            None,
        )
    }

    fn runtime_actuator_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "hardware port",
            "actuator",
            false,
            None,
        )
    }

    const MODELICA_BACKEND: PortBackend = PortBackend {
        list: modelica_list,
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        metadata: modelica_metadata,
        read_output: |_world, _entity, _name| None,
        read_input: |_world, _entity, _name| Some(0.0),
        resolve_input: Some(modelica_input_slot),
        resolve_output: None,
        read_slot: None,
        read_input_slot: Some(|_, _, slot| (slot == 0).then_some(0.0)),
        write_slot: None,
    };

    const RUNTIME_ACTUATOR_BACKEND: PortBackend = PortBackend {
        list: runtime_actuator_list,
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        metadata: runtime_actuator_metadata,
        read_output: |_world, _entity, _name| Some(0.0),
        read_input: |_world, _entity, _name| Some(0.0),
        resolve_input: Some(runtime_actuator_input_slot),
        resolve_output: None,
        read_slot: None,
        read_input_slot: Some(|_, _, slot| (slot == 0).then_some(0.0)),
        write_slot: None,
    };

    fn composed_fixture() -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source(
            "port_owner_collision.usda",
            "#usda 1.0\n\
             def Xform \"Lander1\" {\n\
                 float inputs:release\n\
             }\n",
        ))
        .expect("duplicate-port fixture composes")
    }

    fn registry() -> PortRegistry {
        let mut registry = PortRegistry::default();
        registry.register(MODELICA_BACKEND);
        registry.register(RUNTIME_ACTUATOR_BACKEND);
        registry
    }

    #[test]
    fn collision_report_contains_structured_winner_and_shadowed_owner_fields() {
        let stage = composed_fixture();
        assert!(
            stage
                .view()
                .prim_paths()
                .iter()
                .any(|path| path.to_string() == "/Lander1")
        );

        let mut world = World::new();
        world.spawn((
            UsdPrimPath {
                stage_handle: Handle::default(),
                path: "/Lander1".into(),
            },
            ModelicaInput,
            RuntimeActuator {
                name: "release".into(),
            },
        ));
        world.insert_resource(registry());

        let findings =
            live_port_collision_findings(&world, Handle::<UsdStageAsset>::default().id());
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.code.as_deref(), Some("port-owner-collision"));
        assert_eq!(finding.severity, DiagnosticSeverity::Warning);
        assert_eq!(finding.subject.as_deref(), Some("/Lander1"));
        assert!(
            finding
                .message
                .contains("PORT_OWNER_COLLISION: `release` has 2 input owners")
        );
        assert!(finding.message.contains("Modelica/OBC"));
        assert!(finding.message.contains("hardware port"));
        assert!(finding.message.contains("/Lander1.inputs:release"));
        assert!(finding.message.contains("/Lander1.inputs/outputs:release"));
        assert!(finding.message.contains("registry precedence 1"));
        assert!(finding.message.contains("registry precedence 2"));
        assert!(
            finding
                .message
                .contains("writes may be routed to the winner")
        );
    }

    #[test]
    fn renamed_actuator_has_no_collision_finding() {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source(
            "port_owner_collision_clean.usda",
            "#usda 1.0\n\
             def Xform \"Lander1\" {\n\
                 float inputs:release\n\
                 float outputs:dock_release\n\
             }\n",
        ))
        .expect("clean port fixture composes");
        assert!(
            stage
                .view()
                .prim_paths()
                .iter()
                .any(|path| path.to_string() == "/Lander1")
        );

        let mut world = World::new();
        world.spawn((
            UsdPrimPath {
                stage_handle: Handle::default(),
                path: "/Lander1".into(),
            },
            ModelicaInput,
            RuntimeActuator {
                name: "dock_release".into(),
            },
        ));
        world.insert_resource(registry());

        assert!(
            live_port_collision_findings(&world, Handle::<UsdStageAsset>::default().id(),)
                .is_empty()
        );
    }
}
