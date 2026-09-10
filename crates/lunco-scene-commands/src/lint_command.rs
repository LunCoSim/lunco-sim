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
//! Rules are rhai: edit and
//! `register_hook("lint.usd", "lint_usd", src)` and the NEXT `RunLint` obeys
//! them, on a running sim, with no rebuild.
//!
//! `ValidateAsset` runs the same rules over the same facts for a FILE. This runs
//! them over what is actually loaded — which, after runtime spawns and edits, is
//! not the same stage any file describes. `RunLint { scope: "twin" }` instead
//! inspects the active Twin's resolver namespaces through the shared Twin
//! inspector; its file-only counterpart is `ValidateTwin`.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::ApiResponse;
use lunco_core::{on_command, Command};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::DocumentRegistry;
use lunco_hooks::HookValue as H;
use lunco_usd_bevy::{CanonicalStages, UsdStageAsset};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

/// Build the complete USD lint fact map from every owner of a USD simulation
/// projection. Standard `Physics*Joint` facts come from `lunco-usd-avian`;
/// `PhysxPhysicsGearJoint` facts come from the `lunco-usd-sim` reader that owns
/// that projection. The policy sees one map and one authoritative value for
/// each subject.
pub(crate) fn usd_physics_facts(view: &lunco_usd_bevy::StageView<'_>) -> H {
    let mut facts = lunco_usd_avian::physics_facts(view);
    lunco_usd_sim::lint::append_network_synthesizer_facts(view, &mut facts);
    lunco_usd_sim::lint::append_gear_drive_facts(view, &mut facts);
    lunco_usd_sim::lint::append_wheel_attachment_facts(view, &mut facts);
    facts
}

/// Run the complete live USD lint pipeline over one composed stage.
///
/// This aggregation point owns the cross-domain fact table: standard physics
/// facts come from `lunco-usd-avian`, while USD-sim owns its gear, wheel, and
/// synthesizer projections. Callers must use this entry point rather than
/// linting a partial producer's facts.
pub fn lint_stage(view: &lunco_usd_bevy::StageView<'_>) -> Vec<lunco_lint::LintFinding> {
    lunco_lint::run_lint(lunco_usd_avian::USD_LINT_DOMAIN, usd_physics_facts(view))
}

/// Inspect the already projected port surface for one composed USD stage.
///
/// `PortRegistry` remains the authority for ownership and precedence; this
/// bridge only gives its diagnostic result the composed USD identity carried by
/// `UsdPrimPath`. Runtime routes are not changed and no name-based fallback is
/// introduced.
fn live_port_collision_findings(
    world: &World,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
) -> Vec<lunco_lint::LintFinding> {
    let Some(registry) = world.get_resource::<lunco_core::ports::PortRegistry>() else {
        return Vec::new();
    };

    // A composed prim has one projected entity per stage/path. Keep the first
    // path deterministically if a transient projection view duplicates it.
    let mut entities = BTreeMap::new();
    for entity in world.iter_entities() {
        let Some(prim) = entity.get::<lunco_usd_bevy::UsdPrimPath>() else {
            continue;
        };
        if prim.stage_handle.id() == stage_id {
            entities.entry(prim.path.clone()).or_insert(entity.id());
        }
    }

    let mut findings = Vec::new();
    for (entity_path, entity) in entities {
        for collision in registry.entity_port_collisions(world, entity) {
            let direction = match collision.direction {
                lunco_core::ports::PortCollisionDirection::Input => "input",
                lunco_core::ports::PortCollisionDirection::Output => "output",
                lunco_core::ports::PortCollisionDirection::InOut => "inout",
            };
            let property_path = |owner: &lunco_core::ports::PortOwnerInfo| {
                let namespace = match owner.direction {
                    lunco_core::ports::PortDirection::In => "inputs",
                    lunco_core::ports::PortDirection::Out => "outputs",
                    lunco_core::ports::PortDirection::InOut => "inputs/outputs",
                };
                format!("{entity_path}.{namespace}:{}", collision.name)
            };
            let owner_text = |owner: &lunco_core::ports::PortOwnerInfo| {
                let source = if owner.metadata.source.is_empty() {
                    "unknown backend"
                } else {
                    owner.metadata.source.as_str()
                };
                format!(
                    "{source} at {} (registry precedence {})",
                    property_path(owner),
                    owner.precedence
                )
            };
            let winner = owner_text(&collision.owners[0]);
            let shadowed = collision.owners[1..]
                .iter()
                .map(|owner| format!("  shadowed: {}", owner_text(owner)))
                .collect::<Vec<_>>()
                .join("\n");
            let access = match collision.direction {
                lunco_core::ports::PortCollisionDirection::Input => {
                    "writes may be routed to the winner"
                }
                lunco_core::ports::PortCollisionDirection::Output => {
                    "reads may be routed to the winner"
                }
                lunco_core::ports::PortCollisionDirection::InOut => {
                    "reads and writes may be routed to the winner"
                }
            };
            findings.push(lunco_lint::LintFinding {
                domain: lunco_usd_avian::USD_LINT_DOMAIN.to_owned(),
                rule: "port-owner-collision".to_owned(),
                severity: lunco_lint::LintSeverity::Warn,
                subject: entity_path.clone(),
                message: format!(
                    "PORT_OWNER_COLLISION: `{}` has {} {} owners on {}\n  winner: {}\n{}\n  {}; give the owners distinct public port names",
                    collision.name,
                    collision.owners.len(),
                    direction,
                    entity_path,
                    winner,
                    shadowed,
                    access,
                ),
            });
        }
    }
    findings
}

/// Lint what is loaded now.
///
/// Findings land in [`lunco_lint::LintReport`] (readable via the `LintReport`
/// query) and are logged — errors at `error!`, warnings at `warn!`.
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

/// The latest live lint result for each open Editor document.
///
/// This is separate from the loaded-scene report because a preview document is
/// not the mounted live stage and must never replace or pollute its findings.
#[derive(Resource, Default)]
pub struct DocumentLintReports {
    reports: HashMap<DocumentId, DocumentLintReport>,
}

#[derive(Default)]
struct DocumentLintReport {
    generation: Option<u64>,
    projection_ready: bool,
    findings: Vec<lunco_lint::LintFinding>,
}

/// Clear document-scoped lint state with the scene lifecycle.
pub fn clear_document_reports(mut reports: ResMut<DocumentLintReports>) {
    reports.reports.clear();
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
    mut document_reports: ResMut<DocumentLintReports>,
    asset_server: Option<Res<AssetServer>>,
    documents: Option<Res<DocumentRegistry<lunco_usd::document::UsdDocument>>>,
    backed: Option<Res<lunco_usd::twin_projection::DocBackedTwinScenes>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) {
    let scope = trigger.event().scope.trim();
    if scope == "twin" {
        report.clear_domain("twin");
        let domain = trigger.event().domain.trim();
        if !domain.is_empty() && domain != "twin" {
            report.extend_logged(vec![lunco_lint::LintFinding {
                domain: "twin".to_string(),
                rule: "invalid-twin-lint-domain".to_string(),
                severity: lunco_lint::LintSeverity::Error,
                subject: "RunLint".to_string(),
                message: format!(
                    "scope `twin` cannot be combined with domain `{domain}`; omit domain or use `twin`"
                ),
            }]);
            return;
        }
        if trigger.event().doc_id.is_some() {
            report.extend_logged(vec![lunco_lint::LintFinding {
                domain: "twin".to_string(),
                rule: "invalid-twin-lint-document".to_string(),
                severity: lunco_lint::LintSeverity::Error,
                subject: "RunLint".to_string(),
                message: "scope `twin` inspects the active Twin and cannot take doc_id".to_string(),
            }]);
            return;
        }
        let policy = match crate::twin_lint::policy_name(&trigger.event().policy) {
            Ok(policy) => policy,
            Err(message) => {
                report.extend_logged(vec![lunco_lint::LintFinding {
                    domain: "twin".to_string(),
                    rule: "invalid-twin-lint-policy".to_string(),
                    severity: lunco_lint::LintSeverity::Error,
                    subject: "RunLint".to_string(),
                    message,
                }]);
                return;
            }
        };
        let Some(workspace) = workspace.as_deref() else {
            report.extend_logged(vec![lunco_lint::LintFinding {
                domain: "twin".to_string(),
                rule: "twin-lint-no-workspace".to_string(),
                severity: lunco_lint::LintSeverity::Error,
                subject: "RunLint".to_string(),
                message: "Twin namespace lint requires the Workspace resource".to_string(),
            }]);
            return;
        };
        let Some(twin_id) = workspace.active_twin else {
            report.extend_logged(vec![lunco_lint::LintFinding {
                domain: "twin".to_string(),
                rule: "twin-lint-no-active-twin".to_string(),
                severity: lunco_lint::LintSeverity::Error,
                subject: "Workspace".to_string(),
                message: "Twin namespace lint requires an active Twin".to_string(),
            }]);
            return;
        };
        let Some(twin) = workspace.twin(twin_id) else {
            report.extend_logged(vec![lunco_lint::LintFinding {
                domain: "twin".to_string(),
                rule: "twin-lint-missing-active-twin".to_string(),
                severity: lunco_lint::LintSeverity::Error,
                subject: format!("TwinId({})", twin_id.raw()),
                message: "Workspace active_twin does not resolve to an open Twin".to_string(),
            }]);
            return;
        };
        let snapshot = crate::twin_lint::inspect_twin(twin);
        let findings = lunco_lint::run_lint("twin", crate::twin_lint::facts(&snapshot, policy));
        report.extend_logged(findings);
        info!(
            "[lint] RunLint: Twin `{}` — {} namespace collision(s), {} source read error(s), policy={policy}",
            snapshot.twin,
            snapshot.collisions.len(),
            snapshot.read_errors.len(),
        );
        return;
    }
    if !scope.is_empty() && scope != "loaded_stages" {
        report.clear_domain("twin");
        report.extend_logged(vec![lunco_lint::LintFinding {
            domain: "twin".to_string(),
            rule: "invalid-lint-scope".to_string(),
            severity: lunco_lint::LintSeverity::Error,
            subject: "RunLint".to_string(),
            message: format!("unknown lint scope `{scope}`; use `loaded_stages` or `twin`"),
        }]);
        return;
    }
    let domain = trigger.event().domain.trim().to_string();
    if !domain.is_empty() && domain != lunco_usd_avian::USD_LINT_DOMAIN {
        warn!(
            "[lint] RunLint: no producer for domain '{domain}' in a loaded scene — \
             the USD domain is the one a live stage can supply facts for; \
             ValidateAsset covers .mo/.rhai/.wgsl files"
        );
        return;
    }

    if let Some(raw_doc) = trigger.event().doc_id {
        let doc = DocumentId::new(raw_doc);
        let Some(host) = documents.as_deref().and_then(|registry| registry.host(doc)) else {
            document_reports
                .reports
                .insert(doc, DocumentLintReport::default());
            warn!("[lint] RunLint: document {doc} is not open");
            return;
        };
        let generation = host.document().generation();
        let ready = backed
            .as_deref()
            .and_then(|scenes| scenes.synced_generation(doc))
            == Some(generation);
        if !ready {
            document_reports.reports.insert(
                doc,
                DocumentLintReport {
                    generation: Some(generation),
                    projection_ready: false,
                    findings: Vec::new(),
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
                .map(|(name, rel)| lunco_assets::twin_uri(&name, &rel))
                .and_then(|path| server.get_handle::<UsdStageAsset>(path))
        });
        let stage_id = stage_handle.as_ref().map(|handle| handle.id());
        let stage = stage_handle.and_then(|handle| canonical.get(handle.id()));
        let Some(stage) = stage else {
            document_reports.reports.insert(
                doc,
                DocumentLintReport {
                    generation: Some(generation),
                    projection_ready: false,
                    findings: Vec::new(),
                },
            );
            warn!("[lint] RunLint: document {doc} has no projected canonical stage");
            return;
        };
        let findings = lint_stage(&stage.view());
        let errors = findings
            .iter()
            .filter(|finding| finding.severity == lunco_lint::LintSeverity::Error)
            .count();
        let warnings = findings
            .iter()
            .filter(|finding| finding.severity == lunco_lint::LintSeverity::Warn)
            .count();
        document_reports.reports.insert(
            doc,
            DocumentLintReport {
                generation: Some(generation),
                projection_ready: true,
                findings,
            },
        );
        if let Some(stage_id) = stage_id {
            commands.queue(move |world: &mut World| {
                let findings = live_port_collision_findings(world, stage_id);
                if findings.is_empty() {
                    return;
                }
                if let Some(mut reports) = world.get_resource_mut::<DocumentLintReports>() {
                    if let Some(report) = reports.reports.get_mut(&doc) {
                        report.findings.extend(findings);
                    }
                }
            });
        }
        info!("[lint] RunLint: document {doc} — {errors} error(s), {warnings} warning(s)");
        return;
    }

    // Re-linting REPLACES this domain's findings: a rule that was fixed between
    // two runs must disappear, not accumulate a second copy.
    report.clear_domain(lunco_usd_avian::USD_LINT_DOMAIN);

    // Every loaded stage, composed. `get_or_build` is what the loader itself
    // calls, so this lints exactly what physics reads.
    let ids: Vec<_> = stages.ids().collect();
    let mut linted = 0usize;
    for id in ids {
        if canonical.get(id).is_none() {
            let Some(recipe) = stages.get(id).and_then(|a| a.recipe.clone()) else {
                continue;
            };
            canonical.get_or_build(id, &recipe);
        }
        let Some(cs) = canonical.get(id) else {
            continue;
        };
        let found = lint_stage(&cs.view());
        report.extend_logged(found);
        commands.queue(move |world: &mut World| {
            let findings = live_port_collision_findings(world, id);
            if let Some(mut report) = world.get_resource_mut::<lunco_lint::LintReport>() {
                report.extend_logged(findings);
            }
        });
        linted += 1;
    }

    info!(
        "[lint] RunLint: {linted} stage(s) — {} error(s), {} warning(s)",
        report.errors(),
        report.warnings()
    );
}

/// `LintReport` — read the findings back.
///
/// A QUERY, not a command response: the report is state ("what is wrong with what
/// is loaded"), and a UI panel, a scenario and an HTTP caller all want to read it
/// without re-running the rules.
pub struct LintReportQuery;

impl ApiQueryProvider for LintReportQuery {
    fn name(&self) -> &'static str {
        "LintReport"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        let requested_doc = match _params.get("doc_id") {
            None => None,
            Some(value) => match value.as_u64() {
                Some(raw) => Some(DocumentId::new(raw)),
                None => {
                    return lunco_api::schema::ApiResponse::error(
                        lunco_api::schema::ApiErrorCode::DeserializationError,
                        "LintReport: doc_id must be an explicit numeric document id",
                    )
                }
            },
        };
        if let Some(doc) = requested_doc {
            let scoped = world
                .get_resource::<DocumentLintReports>()
                .and_then(|reports| reports.reports.get(&doc));
            let findings = scoped
                .map(|report| {
                    report
                        .findings
                        .iter()
                        .map(|f| {
                            json!({
                                "domain": f.domain,
                                "rule": f.rule,
                                "severity": f.severity.as_str(),
                                "subject": f.subject,
                                "message": f.message,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let errors = scoped
                .map(|report| {
                    report
                        .findings
                        .iter()
                        .filter(|finding| finding.severity == lunco_lint::LintSeverity::Error)
                        .count()
                })
                .unwrap_or(0);
            let warnings = scoped
                .map(|report| {
                    report
                        .findings
                        .iter()
                        .filter(|finding| finding.severity == lunco_lint::LintSeverity::Warn)
                        .count()
                })
                .unwrap_or(0);
            return ApiResponse::ok(json!({
                "scope": "document",
                "doc_id": doc.raw(),
                "generation": scoped.and_then(|report| report.generation),
                "projection_ready": scoped.is_some_and(|report| report.projection_ready),
                "errors": errors,
                "warnings": warnings,
                "findings": findings,
            }));
        }
        let report = world.get_resource::<lunco_lint::LintReport>();
        let findings: Vec<serde_json::Value> = report
            .map(|r| {
                r.findings
                    .iter()
                    .map(|f| {
                        json!({
                            "domain": f.domain,
                            "rule": f.rule,
                            "severity": f.severity.as_str(),
                            "subject": f.subject,
                            "message": f.message,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let errors = report.map(|r| r.errors()).unwrap_or(0);
        let warnings = report.map(|r| r.warnings()).unwrap_or(0);
        ApiResponse::ok(json!({
            "scope": "loaded_stages",
            "errors": errors,
            "warnings": warnings,
            "findings": findings,
        }))
    }
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

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        let findings = world
            .get_resource::<lunco_core::RuntimeDiagnostics>()
            .map(|diagnostics| {
                diagnostics
                    .findings
                    .iter()
                    .map(|finding| {
                        json!({
                            "code": finding.code,
                            "severity": finding.severity.as_str(),
                            "producer": finding.producer,
                            "subject": finding.subject,
                            "message": finding.message,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ApiResponse::ok(json!({
            "errors": findings.iter().filter(|f| f["severity"] == "error").count(),
            "warnings": findings.iter().filter(|f| f["severity"] == "warning").count(),
            "findings": findings,
        }))
    }
}

/// Register the query alongside the command (the command registers itself with
/// the rest of this crate's verbs).
pub fn register(app: &mut App) {
    app.init_resource::<lunco_lint::LintReport>();
    app.init_resource::<DocumentLintReports>();
    // Findings belong to the loaded scene. A replacement must not leave the
    // previous scene's errors highlighted as if they were current.
    app.add_systems(lunco_core::SceneTeardown, lunco_lint::clear_report);
    app.add_systems(lunco_core::SceneTeardown, clear_document_reports);
    app.init_resource::<ApiQueryRegistry>();
    let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
    registry.register(LintReportQuery);
    registry.register(RuntimeDiagnosticsQuery);
}

#[cfg(test)]
mod tests {
    use super::live_port_collision_findings;
    use bevy::asset::Handle;
    use bevy::prelude::*;
    use lunco_core::ports::{PortBackend, PortDirection, PortMetadata, PortRef, PortRegistry};
    use lunco_usd_bevy::{CanonicalStage, StageRecipe, UsdPrimPath, UsdRead, UsdStageAsset};

    #[derive(Component)]
    struct ModelicaInput;

    #[derive(Component)]
    struct RuntimeActuator {
        name: String,
    }

    fn modelica_list(world: &World, entity: Entity, out: &mut Vec<PortRef>) {
        if world.get::<ModelicaInput>(entity).is_some() {
            out.push(PortRef {
                name: "release".into(),
                direction: PortDirection::In,
                value: 0.0,
            });
        }
    }

    fn runtime_actuator_list(world: &World, entity: Entity, out: &mut Vec<PortRef>) {
        if let Some(actuator) = world.get::<RuntimeActuator>(entity) {
            out.push(PortRef {
                name: actuator.name.clone(),
                direction: PortDirection::InOut,
                value: 0.0,
            });
        }
    }

    fn modelica_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(direction, None, None, None, "Modelica/OBC", "solver", true)
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
            true,
        )
    }

    const MODELICA_BACKEND: PortBackend = PortBackend {
        list: modelica_list,
        metadata: Some(modelica_metadata),
        read_output: |_world, _entity, _name| None,
        read_input: |_world, _entity, _name| Some(0.0),
        write_input: |_world, _entity, _name, _value| true,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        write_slot: None,
    };

    const RUNTIME_ACTUATOR_BACKEND: PortBackend = PortBackend {
        list: runtime_actuator_list,
        metadata: Some(runtime_actuator_metadata),
        read_output: |_world, _entity, _name| Some(0.0),
        read_input: |_world, _entity, _name| Some(0.0),
        write_input: |_world, _entity, _name, _value| true,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        write_slot: None,
    };

    fn composed_fixture() -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source(
            "port_owner_collision.usda",
            "#usda 1.0\n\
             def Xform \"Griffin1\" {\n\
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
        assert!(stage
            .view()
            .prim_paths()
            .iter()
            .any(|path| path.to_string() == "/Griffin1"));

        let mut world = World::new();
        world.spawn((
            UsdPrimPath {
                stage_handle: Handle::default(),
                path: "/Griffin1".into(),
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
        assert_eq!(finding.rule, "port-owner-collision");
        assert_eq!(finding.severity, lunco_lint::LintSeverity::Warn);
        assert_eq!(finding.subject, "/Griffin1");
        assert!(finding
            .message
            .contains("PORT_OWNER_COLLISION: `release` has 2 input owners"));
        assert!(finding.message.contains("Modelica/OBC"));
        assert!(finding.message.contains("hardware port"));
        assert!(finding.message.contains("/Griffin1.inputs:release"));
        assert!(finding.message.contains("/Griffin1.inputs/outputs:release"));
        assert!(finding.message.contains("registry precedence 1"));
        assert!(finding.message.contains("registry precedence 2"));
        assert!(finding
            .message
            .contains("writes may be routed to the winner"));
    }

    #[test]
    fn renamed_actuator_has_no_collision_finding() {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source(
            "port_owner_collision_clean.usda",
            "#usda 1.0\n\
             def Xform \"Griffin1\" {\n\
                 float inputs:release\n\
                 float outputs:dock_release\n\
             }\n",
        ))
        .expect("clean port fixture composes");
        assert!(stage
            .view()
            .prim_paths()
            .iter()
            .any(|path| path.to_string() == "/Griffin1"));

        let mut world = World::new();
        world.spawn((
            UsdPrimPath {
                stage_handle: Handle::default(),
                path: "/Griffin1".into(),
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
