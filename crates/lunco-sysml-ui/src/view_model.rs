use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_assets_core::twin_source::{TwinRoots, twin_uri};
use lunco_doc::{Document, FileBacked};
use lunco_doc_bevy::DocumentRegistry;
use lunco_scene_runner::SceneTestRunReport;
use lunco_sysml::{SysmlDocument, TwinSysmlAnalyses, TwinSysmlAnalysisState};
use lunco_sysml_ast::{
    SysmlAnalysis, SysmlDiagnosticKind, SysmlElement, SysmlElementHandle,
    SysmlRequirementConstraintKind, SysmlRequirementRecord,
};
use lunco_sysml_ir::VerificationVerdict;
use lunco_telemetry_core::{TelemetryEvent, TelemetryValue};
use lunco_twin::Twin;
use lunco_workspace::{TwinClosed, TwinId, WorkspaceResource};

use crate::panel::DocumentView;

/// Bounds retained check detail while aggregate verdict counts remain exact.
const MAX_EVIDENCE_DETAILS_PER_REQUIREMENT: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum AnalysisState {
    #[default]
    NoActiveTwin,
    Waiting,
    NoSources,
    Ready,
    Failed(Vec<String>),
}

#[derive(Clone, Debug)]
pub(crate) struct SourceFileView {
    pub logical_uri: String,
    pub relative_path: PathBuf,
    pub absolute_path: PathBuf,
    pub document_id: Option<lunco_doc::DocumentId>,
}

#[derive(Clone, Debug)]
pub(crate) struct RequirementView {
    pub handle: SysmlElementHandle,
    pub qualified_name: String,
    pub display_name: String,
    pub role: RequirementRole,
    pub definition_handle: Option<SysmlElementHandle>,
    pub documentation: Vec<String>,
    pub logical_uri: String,
    pub relative_path: Option<PathBuf>,
    pub line: Option<usize>,
    pub has_required_constraint: bool,
    pub subjects: Vec<RequirementSubjectView>,
    pub satisfied_by: Vec<ModelElementView>,
    pub verification_cases: Vec<String>,
    pub runtime_evidence: Vec<RuntimeRequirementEvidence>,
}

#[derive(Clone, Debug)]
pub(crate) struct RequirementSubjectView {
    pub name: String,
    pub type_name: Option<String>,
    pub target: Option<ModelElementView>,
    pub ambiguous_target: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelElementView {
    pub handle: SysmlElementHandle,
    pub owner_handle: Option<SysmlElementHandle>,
    pub display_name: String,
    pub qualified_name: String,
    pub kind: String,
    pub logical_uri: String,
    pub relative_path: Option<PathBuf>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelStructureNodeView {
    pub element: ModelElementView,
    pub children: Vec<ModelStructureNodeView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequirementRole {
    Definition,
    Usage,
}

#[derive(Clone, Debug)]
pub(crate) struct VerificationCaseView {
    pub name: String,
    pub scene: PathBuf,
    pub verdict_channel: Option<String>,
    pub source_element: Option<ModelElementView>,
}

#[derive(Clone, Debug)]
pub(crate) struct ParserDiagnosticView {
    pub kind: SysmlDiagnosticKind,
    pub logical_uri: String,
    pub line: usize,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Per-channel check totals and provenance for one analyzed requirement.
pub(crate) struct RuntimeRequirementEvidence {
    pub requirement: String,
    pub channel: String,
    pub verification: Option<String>,
    pub source_revision: u64,
    pub sim_tick: u64,
    pub checks: u64,
    pub failures: u64,
    pub inconclusive: u64,
    pub errors: u64,
    pub unverified: u64,
    pub details: Vec<RuntimeEvidenceCheck>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Displayable fields from one typed verification check result.
pub(crate) struct RuntimeEvidenceCheck {
    pub id: Option<String>,
    pub component: Option<String>,
    pub kind: Option<String>,
    pub path: Option<String>,
    pub verdict: VerificationVerdict,
    pub error: Option<String>,
    pub actual: Option<String>,
    pub expected: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct TwinVerificationSnapshot {
    source_revision: u64,
    latest_channel: String,
    latest_sim_tick: u64,
    channels: BTreeMap<String, VerificationChannelSnapshot>,
}

#[derive(Clone, Debug, Default)]
/// Latest structured evidence for one verification telemetry channel.
struct VerificationChannelSnapshot {
    requirements: BTreeMap<String, RuntimeRequirementEvidence>,
}

#[derive(Resource, Default)]
pub(crate) struct SysmlVerificationEvidence {
    by_twin: HashMap<u64, TwinVerificationSnapshot>,
}

impl SysmlVerificationEvidence {
    fn for_twin(&self, twin: TwinId) -> Option<&TwinVerificationSnapshot> {
        self.by_twin.get(&twin.raw())
    }
}

/// Presentation model built from the active Twin's existing SysML analysis,
/// open-document registry, and structured verification telemetry.
#[derive(Resource, Default)]
pub struct SysmlRequirementsViewModel {
    pub(crate) state: AnalysisState,
    pub(crate) twin_id: Option<TwinId>,
    pub(crate) twin_name: Option<String>,
    pub(crate) twin_root: Option<PathBuf>,
    pub(crate) source_revision: Option<u64>,
    pub(crate) source_files: Vec<SourceFileView>,
    pub(crate) requirements: Vec<RequirementView>,
    pub(crate) model_structure: Vec<ModelStructureNodeView>,
    pub(crate) model_elements: Vec<ModelElementView>,
    pub(crate) verification_cases: Vec<VerificationCaseView>,
    pub(crate) verification_setup_errors: Vec<String>,
    pub(crate) parser_diagnostics: Vec<ParserDiagnosticView>,
    pub(crate) analysis_has_errors: bool,
    pub(crate) verification_revision: Option<u64>,
    pub(crate) verification_channel: Option<String>,
    pub(crate) verification_sim_tick: Option<u64>,
    pub(crate) stale_verification: bool,
}

pub(crate) fn produce_sysml_requirements_view_model(
    workspace: Option<Res<WorkspaceResource>>,
    roots: Option<Res<TwinRoots>>,
    analyses: Option<Res<TwinSysmlAnalyses>>,
    documents: Option<Res<DocumentRegistry<SysmlDocument>>>,
    evidence: Option<Res<SysmlVerificationEvidence>>,
    mut view_model: ResMut<SysmlRequirementsViewModel>,
    mut initialized: Local<bool>,
) {
    let changed = !*initialized
        || workspace.as_ref().is_some_and(Res::is_changed)
        || roots.as_ref().is_some_and(Res::is_changed)
        || analyses.as_ref().is_some_and(Res::is_changed)
        || documents.as_ref().is_some_and(Res::is_changed)
        || evidence.as_ref().is_some_and(Res::is_changed);
    if !changed {
        return;
    }
    *initialized = true;

    let next = build_view_model(
        workspace.as_deref(),
        roots.as_deref(),
        analyses.as_deref(),
        documents.as_deref(),
        evidence.as_deref(),
    );
    *view_model = next;
}

fn build_view_model(
    workspace: Option<&WorkspaceResource>,
    roots: Option<&TwinRoots>,
    analyses: Option<&TwinSysmlAnalyses>,
    documents: Option<&DocumentRegistry<SysmlDocument>>,
    evidence: Option<&SysmlVerificationEvidence>,
) -> SysmlRequirementsViewModel {
    let Some(workspace) = workspace else {
        return SysmlRequirementsViewModel::default();
    };
    let Some(twin_id) = workspace.active_twin else {
        return SysmlRequirementsViewModel::default();
    };
    let Some(twin) = workspace.twin(twin_id) else {
        return SysmlRequirementsViewModel {
            state: AnalysisState::Failed(vec![
                "The active Twin is missing from the workspace registry.".to_owned(),
            ]),
            twin_id: Some(twin_id),
            ..Default::default()
        };
    };
    let Some(roots) = roots else {
        return SysmlRequirementsViewModel {
            state: AnalysisState::Failed(vec![
                "Twin asset identities are not available yet.".to_owned(),
            ]),
            twin_id: Some(twin_id),
            twin_name: Some(twin_display_name(twin)),
            ..Default::default()
        };
    };
    let name = match roots.name_for_root(&twin.root) {
        Ok(Some(name)) => name,
        Ok(None) => {
            return SysmlRequirementsViewModel {
                state: AnalysisState::Failed(vec![
                    "The active Twin has no registered twin:// source identity.".to_owned(),
                ]),
                twin_id: Some(twin_id),
                twin_name: Some(twin_display_name(twin)),
                ..Default::default()
            };
        }
        Err(error) => {
            return SysmlRequirementsViewModel {
                state: AnalysisState::Failed(vec![format!(
                    "Cannot resolve the active Twin source identity: {error}"
                )]),
                twin_id: Some(twin_id),
                twin_name: Some(twin_display_name(twin)),
                ..Default::default()
            };
        }
    };
    let has_sources = twin
        .files()
        .iter()
        .any(|file| is_sysml_path(&file.relative_path));
    if !has_sources {
        return SysmlRequirementsViewModel {
            state: AnalysisState::NoSources,
            twin_id: Some(twin_id),
            twin_name: Some(twin_display_name(twin)),
            ..Default::default()
        };
    }
    let Some(analyses) = analyses else {
        return SysmlRequirementsViewModel {
            state: AnalysisState::Waiting,
            twin_id: Some(twin_id),
            twin_name: Some(twin_display_name(twin)),
            ..Default::default()
        };
    };
    let Some(state) = analyses.state_for(&name, twin_id, &twin.root) else {
        return SysmlRequirementsViewModel {
            state: AnalysisState::Waiting,
            twin_id: Some(twin_id),
            twin_name: Some(twin_display_name(twin)),
            ..Default::default()
        };
    };
    let analysis = match state {
        TwinSysmlAnalysisState::Pending => {
            return SysmlRequirementsViewModel {
                state: AnalysisState::Waiting,
                twin_id: Some(twin_id),
                twin_name: Some(twin_display_name(twin)),
                ..Default::default()
            };
        }
        TwinSysmlAnalysisState::Failed(errors) => {
            return SysmlRequirementsViewModel {
                state: AnalysisState::Failed(errors),
                twin_id: Some(twin_id),
                twin_name: Some(twin_display_name(twin)),
                ..Default::default()
            };
        }
        TwinSysmlAnalysisState::Ready(analysis) => analysis,
    };
    if analysis.files().is_empty() {
        return SysmlRequirementsViewModel {
            state: AnalysisState::NoSources,
            twin_id: Some(twin_id),
            twin_name: Some(twin_display_name(twin)),
            source_revision: Some(analysis.source_revision()),
            ..Default::default()
        };
    }

    let source_files = build_source_file_views(twin, &name, &analysis, documents);
    let model_elements = build_model_element_views(&analysis, &source_files);
    let model_structure = build_model_structure(&model_elements);
    let requirements = build_requirement_views(&analysis, &source_files, &model_elements);
    let verification_cases = twin
        .verification_cases()
        .iter()
        .map(|case| VerificationCaseView {
            name: case.name.clone(),
            scene: case.scene.clone(),
            verdict_channel: case.verdict_channel.clone(),
            source_element: model_elements
                .iter()
                .find(|element| element.qualified_name == case.name)
                .cloned(),
        })
        .collect();
    let mut verification_setup_errors = twin.verification_registry_errors();
    verification_setup_errors.extend(twin.component_registry_errors());
    let snapshot = evidence.and_then(|evidence| evidence.for_twin(twin_id));
    let verification_revision = snapshot.map(|snapshot| snapshot.source_revision);
    let stale_verification =
        verification_revision.is_some_and(|revision| revision != analysis.source_revision());
    let mut requirements = requirements;
    for requirement in &mut requirements {
        requirement.runtime_evidence = snapshot.map_or_else(Vec::new, |snapshot| {
            requirement_evidence_for(snapshot, &requirement.qualified_name)
        });
    }
    SysmlRequirementsViewModel {
        state: AnalysisState::Ready,
        twin_id: Some(twin_id),
        twin_name: Some(twin_display_name(twin)),
        twin_root: twin.manifest.as_ref().map(|_| twin.root.clone()),
        source_revision: Some(analysis.source_revision()),
        source_files,
        requirements,
        model_structure,
        model_elements,
        verification_cases,
        verification_setup_errors,
        parser_diagnostics: analysis
            .diagnostics()
            .iter()
            .map(|diagnostic| ParserDiagnosticView {
                kind: diagnostic.kind,
                logical_uri: diagnostic.file.clone(),
                line: line_for_offset(
                    analysis
                        .files()
                        .iter()
                        .find(|file| file.name == diagnostic.file)
                        .map_or("", |file| file.text.as_str()),
                    diagnostic.start,
                ),
                message: diagnostic.message.clone(),
            })
            .collect(),
        analysis_has_errors: analysis.has_errors(),
        verification_revision,
        verification_channel: snapshot.map(|snapshot| snapshot.latest_channel.clone()),
        verification_sim_tick: snapshot.map(|snapshot| snapshot.latest_sim_tick),
        stale_verification,
    }
}

/// Collects channel records and caps details across the requirement as a whole.
fn requirement_evidence_for(
    snapshot: &TwinVerificationSnapshot,
    requirement_name: &str,
) -> Vec<RuntimeRequirementEvidence> {
    let mut evidence: Vec<_> = snapshot
        .channels
        .values()
        .filter_map(|channel| channel.requirements.get(requirement_name))
        .cloned()
        .collect();
    let mut selected: Vec<Vec<bool>> = evidence
        .iter()
        .map(|record| vec![false; record.details.len()])
        .collect();
    let mut retained = 0;
    for passed in [false, true] {
        for (record_index, record) in evidence.iter().enumerate() {
            for (detail_index, detail) in record.details.iter().enumerate() {
                let is_pass = detail.verdict == VerificationVerdict::Pass;
                if is_pass != passed || retained == MAX_EVIDENCE_DETAILS_PER_REQUIREMENT {
                    continue;
                }
                selected[record_index][detail_index] = true;
                retained += 1;
            }
        }
    }
    for (record, keep) in evidence.iter_mut().zip(selected) {
        record.details = std::mem::take(&mut record.details)
            .into_iter()
            .zip(keep)
            .filter_map(|(detail, selected)| selected.then_some(detail))
            .collect();
    }
    evidence
}

fn twin_display_name(twin: &Twin) -> String {
    twin.manifest
        .as_ref()
        .map(|manifest| manifest.name.clone())
        .or_else(|| {
            twin.root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Twin".to_owned())
}

fn is_sysml_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("sysml" | "kerml")
    )
}

fn build_source_file_views(
    twin: &Twin,
    twin_name: &str,
    analysis: &SysmlAnalysis,
    documents: Option<&DocumentRegistry<SysmlDocument>>,
) -> Vec<SourceFileView> {
    let mut files = analysis
        .files()
        .iter()
        .filter_map(|file| {
            let indexed = twin.files().iter().find(|entry| {
                is_sysml_path(&entry.relative_path)
                    && twin_uri(twin_name, &entry.relative_path) == file.name
            })?;
            let absolute_path = twin.root.join(&indexed.relative_path);
            let document_id = documents.and_then(|registry| {
                registry.ids().find(|doc_id| {
                    let Some(host) = registry.host(*doc_id) else {
                        return false;
                    };
                    host.document()
                        .origin()
                        .canonical_path()
                        .is_some_and(|path| lunco_doc::same_file(path, &absolute_path))
                })
            });
            Some(SourceFileView {
                logical_uri: file.name.clone(),
                relative_path: indexed.relative_path.clone(),
                absolute_path,
                document_id,
            })
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    files
}

fn build_requirement_views(
    analysis: &SysmlAnalysis,
    sources: &[SourceFileView],
    model_elements: &[ModelElementView],
) -> Vec<RequirementView> {
    let source_paths: HashMap<_, _> = sources
        .iter()
        .map(|source| (source.logical_uri.as_str(), source.relative_path.as_path()))
        .collect();
    let source_texts: HashMap<_, _> = analysis
        .files()
        .iter()
        .map(|file| (file.name.as_str(), file.text.as_str()))
        .collect();
    let mut verified_by: HashMap<SysmlElementHandle, Vec<String>> = HashMap::new();
    for verification in analysis.verifications() {
        for requirement in &verification.verified_requirements {
            verified_by
                .entry(*requirement)
                .or_default()
                .push(verification.element.qualified_name.clone());
        }
    }
    let satisfied_by = build_satisfaction_index(analysis, model_elements);
    let subject_types = build_subject_type_index(analysis, model_elements);
    let requirement_definitions: HashSet<_> = analysis
        .requirements()
        .iter()
        .filter(|requirement| requirement.element.kind == "RequirementDefinition")
        .map(|requirement| requirement.element.handle)
        .collect();
    let requirement_usages: HashSet<_> = analysis
        .requirements()
        .iter()
        .filter(|requirement| requirement.element.kind == "RequirementUsage")
        .map(|requirement| requirement.element.handle)
        .collect();
    let mut definitions_by_usage =
        HashMap::<SysmlElementHandle, HashSet<SysmlElementHandle>>::new();
    for reference in analysis.references() {
        if !requirement_definitions.contains(&reference.target) {
            continue;
        }
        for usage in [Some(reference.from), reference.from_owner]
            .into_iter()
            .flatten()
            .filter(|usage| requirement_usages.contains(usage))
        {
            definitions_by_usage
                .entry(usage)
                .or_default()
                .insert(reference.target);
        }
    }

    let mut requirements = analysis
        .requirements()
        .iter()
        .map(|requirement| {
            requirement_view(
                requirement,
                &source_paths,
                &source_texts,
                &verified_by,
                &satisfied_by,
                &subject_types,
                &definitions_by_usage,
                model_elements,
            )
        })
        .collect::<Vec<_>>();
    requirements.sort_by(|left, right| {
        left.logical_uri
            .cmp(&right.logical_uri)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
    });
    requirements
}

fn requirement_view(
    requirement: &SysmlRequirementRecord,
    source_paths: &HashMap<&str, &Path>,
    source_texts: &HashMap<&str, &str>,
    verified_by: &HashMap<SysmlElementHandle, Vec<String>>,
    satisfied_by: &HashMap<SysmlElementHandle, Vec<ModelElementView>>,
    subject_types: &HashMap<SysmlElementHandle, Vec<ModelElementView>>,
    definitions_by_usage: &HashMap<SysmlElementHandle, HashSet<SysmlElementHandle>>,
    model_elements: &[ModelElementView],
) -> RequirementView {
    let element = &requirement.element;
    let source = source_texts
        .get(element.file.as_str())
        .copied()
        .unwrap_or("");
    let mut verification_cases = verified_by
        .get(&element.handle)
        .cloned()
        .unwrap_or_default();
    verification_cases.sort();
    verification_cases.dedup();
    let subjects = requirement
        .subjects
        .iter()
        .map(|subject| {
            let (target, ambiguous_target) = resolve_subject_type(
                element.handle,
                &subject.name,
                subject.type_name.is_some(),
                model_elements,
                subject_types,
            );
            RequirementSubjectView {
                name: subject.name.clone(),
                type_name: subject.type_name.clone(),
                target,
                ambiguous_target,
            }
        })
        .collect();
    let definition_handle = if element.kind == "RequirementUsage" {
        definitions_by_usage
            .get(&element.handle)
            .filter(|definitions| definitions.len() == 1)
            .and_then(|definitions| definitions.iter().next().copied())
    } else {
        None
    };
    RequirementView {
        handle: element.handle,
        qualified_name: element.qualified_name.clone(),
        display_name: element.short_name.clone().unwrap_or_else(|| {
            element
                .qualified_name
                .rsplit("::")
                .next()
                .unwrap_or(&element.qualified_name)
                .to_owned()
        }),
        role: match element.kind.as_str() {
            "RequirementDefinition" => RequirementRole::Definition,
            _ => RequirementRole::Usage,
        },
        definition_handle,
        documentation: requirement.documentation.clone(),
        logical_uri: element.file.clone(),
        relative_path: source_paths
            .get(element.file.as_str())
            .map(|path| path.to_path_buf()),
        line: Some(line_for_offset(source, element.start)),
        has_required_constraint: requirement
            .constraints
            .iter()
            .any(|constraint| constraint.kind == SysmlRequirementConstraintKind::Require),
        subjects,
        satisfied_by: satisfied_by
            .get(&element.handle)
            .cloned()
            .unwrap_or_default(),
        verification_cases,
        runtime_evidence: Vec::new(),
    }
}

fn build_model_element_views(
    analysis: &SysmlAnalysis,
    sources: &[SourceFileView],
) -> Vec<ModelElementView> {
    let source_paths: HashMap<_, _> = sources
        .iter()
        .map(|source| (source.logical_uri.as_str(), source.relative_path.as_path()))
        .collect();
    let source_texts: HashMap<_, _> = analysis
        .files()
        .iter()
        .map(|file| (file.name.as_str(), file.text.as_str()))
        .collect();
    let mut elements = analysis
        .elements()
        .iter()
        .map(|element| model_element_view(element, &source_paths, &source_texts))
        .collect::<Vec<_>>();
    elements.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
    elements
}

fn model_element_view(
    element: &SysmlElement,
    source_paths: &HashMap<&str, &Path>,
    source_texts: &HashMap<&str, &str>,
) -> ModelElementView {
    let source = source_texts
        .get(element.file.as_str())
        .copied()
        .unwrap_or("");
    ModelElementView {
        handle: element.handle,
        owner_handle: element.owner_handle,
        display_name: element.short_name.clone().unwrap_or_else(|| {
            element
                .qualified_name
                .rsplit("::")
                .next()
                .unwrap_or_default()
                .to_owned()
        }),
        qualified_name: element.qualified_name.clone(),
        kind: element.kind.clone(),
        logical_uri: element.file.clone(),
        relative_path: source_paths
            .get(element.file.as_str())
            .map(|path| path.to_path_buf()),
        line: line_for_offset(source, element.start),
    }
}

fn build_model_structure(model_elements: &[ModelElementView]) -> Vec<ModelStructureNodeView> {
    let structural_handles: std::collections::HashSet<_> = model_elements
        .iter()
        .filter(|element| is_structure_kind(&element.kind))
        .map(|element| element.handle)
        .collect();
    let elements_by_handle: HashMap<_, _> = model_elements
        .iter()
        .map(|element| (element.handle, element))
        .collect();
    let mut children_by_parent = HashMap::<SysmlElementHandle, Vec<SysmlElementHandle>>::new();
    let mut roots = Vec::new();

    for element in model_elements
        .iter()
        .filter(|element| structural_handles.contains(&element.handle))
    {
        let parent = nearest_structure_owner(
            element.owner_handle,
            &elements_by_handle,
            &structural_handles,
        );
        if let Some(parent) = parent {
            children_by_parent
                .entry(parent)
                .or_default()
                .push(element.handle);
        } else {
            roots.push(element.handle);
        }
    }

    // `model_elements` is already sorted by qualified identity, so roots and
    // each sibling list remain stable across entity and hash-map allocation.
    let mut nodes_by_handle: HashMap<_, _> = model_elements
        .iter()
        .filter(|element| structural_handles.contains(&element.handle))
        .map(|element| (element.handle, element.clone()))
        .collect();
    roots
        .into_iter()
        .filter_map(|handle| {
            take_structure_node(handle, &mut nodes_by_handle, &mut children_by_parent)
        })
        .collect()
}

fn is_structure_kind(kind: &str) -> bool {
    matches!(
        kind,
        "Package"
            | "PartDefinition"
            | "PartUsage"
            | "PortDefinition"
            | "PortUsage"
            | "ItemDefinition"
            | "ItemUsage"
            | "InterfaceDefinition"
            | "InterfaceUsage"
            | "ConnectionDefinition"
            | "ConnectionUsage"
    )
}

fn nearest_structure_owner(
    mut owner: Option<SysmlElementHandle>,
    elements_by_handle: &HashMap<SysmlElementHandle, &ModelElementView>,
    structural_handles: &std::collections::HashSet<SysmlElementHandle>,
) -> Option<SysmlElementHandle> {
    let mut visited = std::collections::HashSet::new();
    while let Some(handle) = owner {
        if !visited.insert(handle) {
            return None;
        }
        if structural_handles.contains(&handle) {
            return Some(handle);
        }
        owner = elements_by_handle
            .get(&handle)
            .and_then(|element| element.owner_handle);
    }
    None
}

fn take_structure_node(
    handle: SysmlElementHandle,
    nodes_by_handle: &mut HashMap<SysmlElementHandle, ModelElementView>,
    children_by_parent: &mut HashMap<SysmlElementHandle, Vec<SysmlElementHandle>>,
) -> Option<ModelStructureNodeView> {
    let element = nodes_by_handle.remove(&handle)?;
    let children = children_by_parent
        .remove(&handle)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|child| take_structure_node(child, nodes_by_handle, children_by_parent))
        .collect();
    Some(ModelStructureNodeView { element, children })
}

fn build_satisfaction_index(
    analysis: &SysmlAnalysis,
    model_elements: &[ModelElementView],
) -> HashMap<SysmlElementHandle, Vec<ModelElementView>> {
    let elements_by_handle: HashMap<_, _> = model_elements
        .iter()
        .map(|element| (element.handle, element))
        .collect();
    let structural_handles: std::collections::HashSet<_> = model_elements
        .iter()
        .filter(|element| is_structure_kind(&element.kind))
        .map(|element| element.handle)
        .collect();
    let mut satisfying_by_requirement =
        HashMap::<SysmlElementHandle, std::collections::BTreeMap<String, ModelElementView>>::new();

    for relationship in analysis
        .relationships()
        .iter()
        .filter(|relationship| relationship.element.kind == "SatisfyRequirementUsage")
    {
        let requirements = relationship
            .properties
            .iter()
            .find(|property| property.name == "satisfiedRequirement")
            .map(|property| property.targets.as_slice())
            .unwrap_or_default();
        let satisfying_features = relationship
            .properties
            .iter()
            .find(|property| property.name == "satisfyingFeature")
            .map(|property| property.targets.as_slice())
            .unwrap_or_default();

        for requirement in requirements {
            for satisfying_feature in satisfying_features {
                let Some(structure_handle) = nearest_structure_element(
                    *satisfying_feature,
                    &elements_by_handle,
                    &structural_handles,
                ) else {
                    continue;
                };
                let Some(element) = elements_by_handle.get(&structure_handle) else {
                    continue;
                };
                satisfying_by_requirement
                    .entry(*requirement)
                    .or_default()
                    .insert(element.qualified_name.clone(), (*element).clone());
            }
        }
    }

    satisfying_by_requirement
        .into_iter()
        .map(|(requirement, elements)| (requirement, elements.into_values().collect()))
        .collect()
}

fn resolve_subject_type(
    requirement: SysmlElementHandle,
    subject_name: &str,
    has_type: bool,
    model_elements: &[ModelElementView],
    subject_types: &HashMap<SysmlElementHandle, Vec<ModelElementView>>,
) -> (Option<ModelElementView>, bool) {
    if !has_type {
        return (None, false);
    }
    let subject_features: Vec<_> = model_elements
        .iter()
        .filter(|element| {
            element.owner_handle == Some(requirement)
                && element.kind == "ReferenceUsage"
                && element.display_name == subject_name
        })
        .collect();
    let feature = match subject_features.as_slice() {
        [feature] => *feature,
        [] => return (None, false),
        _ => return (None, true),
    };
    match subject_types.get(&feature.handle).map(Vec::as_slice) {
        Some([target]) => (Some(target.clone()), false),
        Some([]) | None => (None, false),
        _ => (None, true),
    }
}

fn build_subject_type_index(
    analysis: &SysmlAnalysis,
    model_elements: &[ModelElementView],
) -> HashMap<SysmlElementHandle, Vec<ModelElementView>> {
    let elements_by_handle: HashMap<_, _> = model_elements
        .iter()
        .map(|element| (element.handle, element))
        .collect();
    let mut targets_by_feature =
        HashMap::<SysmlElementHandle, std::collections::BTreeMap<String, ModelElementView>>::new();

    for reference in analysis.references() {
        let Some(feature) = elements_by_handle.get(&reference.from) else {
            continue;
        };
        if feature.kind != "ReferenceUsage" {
            continue;
        }
        let Some(target) = elements_by_handle.get(&reference.target) else {
            continue;
        };
        targets_by_feature
            .entry(feature.handle)
            .or_default()
            .insert(target.qualified_name.clone(), (*target).clone());
    }

    targets_by_feature
        .into_iter()
        .map(|(feature, targets)| (feature, targets.into_values().collect()))
        .collect()
}

fn nearest_structure_element(
    mut handle: SysmlElementHandle,
    elements_by_handle: &HashMap<SysmlElementHandle, &ModelElementView>,
    structural_handles: &std::collections::HashSet<SysmlElementHandle>,
) -> Option<SysmlElementHandle> {
    let mut visited = std::collections::HashSet::new();
    loop {
        if !visited.insert(handle) {
            return None;
        }
        if structural_handles.contains(&handle) {
            return Some(handle);
        }
        handle = elements_by_handle.get(&handle)?.owner_handle?;
    }
}

fn line_for_offset(source: &str, offset: u32) -> usize {
    source
        .as_bytes()
        .get(..(offset as usize).min(source.len()))
        .map(|prefix| prefix.iter().filter(|byte| **byte == b'\n').count() + 1)
        .unwrap_or(1)
}

/// Captures inline and streamed structured checks for the active Twin.
pub(crate) fn capture_verification_evidence(
    trigger: On<TelemetryEvent>,
    workspace: Option<Res<WorkspaceResource>>,
    mut evidence: ResMut<SysmlVerificationEvidence>,
) {
    let event = trigger.event();
    if !event.name.ends_with("_EVIDENCE") && !event.name.ends_with("_EVIDENCE_RESULT") {
        return;
    }
    let Some(twin_id) = workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
    else {
        return;
    };
    let TelemetryValue::Map(payload) = &event.data else {
        return;
    };
    if telemetry_unsigned(payload.get("schema_version")) != Some(3) {
        return;
    }
    let Some(source_revision) = telemetry_unsigned(payload.get("source_revision")) else {
        return;
    };
    let channel = payload
        .get("channel")
        .and_then(telemetry_string)
        .unwrap_or_else(|| {
            event
                .name
                .trim_end_matches("_EVIDENCE_RESULT")
                .trim_end_matches("_EVIDENCE")
                .to_owned()
        });

    let snapshot =
        evidence
            .by_twin
            .entry(twin_id.raw())
            .or_insert_with(|| TwinVerificationSnapshot {
                source_revision,
                ..Default::default()
            });
    if snapshot.source_revision != source_revision {
        *snapshot = TwinVerificationSnapshot {
            source_revision,
            ..Default::default()
        };
    }
    snapshot.latest_channel.clone_from(&channel);
    snapshot.latest_sim_tick = event.sim_tick;

    let verification = payload.get("verification").and_then(telemetry_string);
    let channel_snapshot = snapshot.channels.entry(channel.clone()).or_default();

    if event.name.ends_with("_EVIDENCE_RESULT") {
        if telemetry_unsigned(payload.get("result_index")) == Some(0) {
            channel_snapshot.requirements.clear();
        }
        if let Some(TelemetryValue::Map(result)) = payload.get("result") {
            append_evidence_check(
                channel_snapshot,
                &channel,
                verification,
                source_revision,
                event.sim_tick,
                result,
            );
        }
        return;
    }

    let Some(TelemetryValue::Map(requirement_summary)) = payload.get("requirement_summary") else {
        return;
    };
    let inline_results = payload
        .get("results")
        .and_then(telemetry_array)
        .filter(|results| !results.is_empty());
    if let Some(results) = inline_results {
        channel_snapshot.requirements.clear();
        for result in results {
            let TelemetryValue::Map(result) = result else {
                continue;
            };
            append_evidence_check(
                channel_snapshot,
                &channel,
                verification.clone(),
                source_revision,
                event.sim_tick,
                result,
            );
        }
    } else if telemetry_unsigned(payload.get("check_count")) == Some(0) {
        channel_snapshot.requirements.clear();
    }

    for (name, summary) in requirement_summary {
        let TelemetryValue::Map(summary) = summary else {
            continue;
        };
        let (Some(checks), Some(failures), Some(inconclusive), Some(errors), Some(unverified)) = (
            telemetry_unsigned(summary.get("checks")),
            telemetry_unsigned(summary.get("failures")),
            telemetry_unsigned(summary.get("inconclusive")),
            telemetry_unsigned(summary.get("errors")),
            telemetry_unsigned(summary.get("unverified")),
        ) else {
            continue;
        };
        let requirement = channel_snapshot
            .requirements
            .entry(name.clone())
            .or_insert_with(|| RuntimeRequirementEvidence {
                requirement: name.clone(),
                channel: channel.clone(),
                verification: verification.clone(),
                source_revision,
                sim_tick: event.sim_tick,
                checks: 0,
                failures: 0,
                inconclusive: 0,
                errors: 0,
                unverified: 0,
                details: Vec::new(),
            });
        requirement.channel.clone_from(&channel);
        requirement.verification.clone_from(&verification);
        requirement.source_revision = source_revision;
        requirement.sim_tick = event.sim_tick;
        requirement.checks = checks;
        requirement.failures = failures;
        requirement.inconclusive = inconclusive;
        requirement.errors = errors;
        requirement.unverified = unverified;
    }
}

/// Projects the versioned child-process report into the same requirement
/// evidence shape used for live Twin telemetry.
pub(crate) fn scene_test_requirement_evidence(
    report: &SceneTestRunReport,
) -> Vec<RuntimeRequirementEvidence> {
    let mut evidence = BTreeMap::<(String, String), RuntimeRequirementEvidence>::new();
    let mut summaries = Vec::new();

    for event in &report.evidence {
        let TelemetryValue::Map(payload) = &event.data else {
            continue;
        };
        if telemetry_unsigned(payload.get("schema_version")) != Some(3) {
            continue;
        }
        let Some(source_revision) = telemetry_unsigned(payload.get("source_revision")) else {
            continue;
        };
        let channel = payload
            .get("channel")
            .and_then(telemetry_string)
            .unwrap_or_else(|| event.name.trim_end_matches("_EVIDENCE").to_owned());
        let verification = payload.get("verification").and_then(telemetry_string);

        let inline_results = payload
            .get("results")
            .and_then(telemetry_array)
            .filter(|results| !results.is_empty());
        let result_values = inline_results.or_else(|| {
            payload
                .get("non_pass_results")
                .and_then(telemetry_array)
                .filter(|results| !results.is_empty())
        });
        if let Some(results) = result_values {
            for result in results {
                let TelemetryValue::Map(result) = result else {
                    continue;
                };
                append_report_evidence_detail(
                    &mut evidence,
                    &channel,
                    verification.clone(),
                    source_revision,
                    event.sim_tick,
                    result,
                );
            }
        }

        if let Some(TelemetryValue::Map(requirements)) = payload.get("requirement_summary") {
            for (name, summary) in requirements {
                let TelemetryValue::Map(summary) = summary else {
                    continue;
                };
                let (
                    Some(checks),
                    Some(failures),
                    Some(inconclusive),
                    Some(errors),
                    Some(unverified),
                ) = (
                    telemetry_unsigned(summary.get("checks")),
                    telemetry_unsigned(summary.get("failures")),
                    telemetry_unsigned(summary.get("inconclusive")),
                    telemetry_unsigned(summary.get("errors")),
                    telemetry_unsigned(summary.get("unverified")),
                )
                else {
                    continue;
                };
                summaries.push((
                    channel.clone(),
                    verification.clone(),
                    source_revision,
                    event.sim_tick,
                    name.clone(),
                    checks,
                    failures,
                    inconclusive,
                    errors,
                    unverified,
                ));
            }
        }
    }

    for event in &report.non_pass_checks {
        let TelemetryValue::Map(payload) = &event.data else {
            continue;
        };
        let Some(source_revision) = telemetry_unsigned(payload.get("source_revision")) else {
            continue;
        };
        let Some(TelemetryValue::Map(result)) = payload.get("result") else {
            continue;
        };
        let channel = event.name.trim_end_matches("_EVIDENCE_RESULT").to_owned();
        append_report_evidence_detail(
            &mut evidence,
            &channel,
            payload.get("verification").and_then(telemetry_string),
            source_revision,
            event.sim_tick,
            result,
        );
    }

    for (
        channel,
        verification,
        revision,
        sim_tick,
        name,
        checks,
        failures,
        inconclusive,
        errors,
        unverified,
    ) in summaries
    {
        let requirement = evidence
            .entry((channel.clone(), name.clone()))
            .or_insert_with(|| RuntimeRequirementEvidence {
                requirement: name.clone(),
                channel: channel.clone(),
                verification: verification.clone(),
                source_revision: revision,
                sim_tick,
                checks: 0,
                failures: 0,
                inconclusive: 0,
                errors: 0,
                unverified: 0,
                details: Vec::new(),
            });
        requirement.channel = channel;
        requirement.verification = verification;
        requirement.source_revision = revision;
        requirement.sim_tick = sim_tick;
        requirement.checks = checks;
        requirement.failures = failures;
        requirement.inconclusive = inconclusive;
        requirement.errors = errors;
        requirement.unverified = unverified;
    }
    evidence.into_values().collect()
}

/// Formats structured failed observations that are not represented by a
/// requirement summary so runner errors remain visible in the test detail.
pub(crate) fn scene_test_report_diagnostics(report: &SceneTestRunReport) -> Vec<String> {
    const MAX_DIAGNOSTICS: usize = 32;
    let mut diagnostics = Vec::new();
    for event in &report.evidence {
        let TelemetryValue::Map(payload) = &event.data else {
            continue;
        };
        let results = payload
            .get("results")
            .and_then(telemetry_array)
            .filter(|results| !results.is_empty());
        let non_pass_results = payload
            .get("non_pass_results")
            .and_then(telemetry_array)
            .filter(|results| !results.is_empty());
        if let Some(results) = results.or(non_pass_results) {
            for result in results {
                if diagnostics.len() == MAX_DIAGNOSTICS {
                    return diagnostics;
                }
                if let TelemetryValue::Map(result) = result {
                    append_scene_test_failure_diagnostic(&mut diagnostics, result);
                } else if let TelemetryValue::String(message) = result {
                    diagnostics.push(message.clone());
                }
            }
        }
    }
    for event in &report.non_pass_checks {
        if diagnostics.len() == MAX_DIAGNOSTICS {
            break;
        }
        let TelemetryValue::Map(payload) = &event.data else {
            continue;
        };
        if let Some(TelemetryValue::Map(result)) = payload.get("result") {
            append_scene_test_failure_diagnostic(&mut diagnostics, result);
        }
    }
    diagnostics
}

fn append_scene_test_failure_diagnostic(
    diagnostics: &mut Vec<String>,
    result: &BTreeMap<String, TelemetryValue>,
) {
    if evidence_verdict(result) == VerificationVerdict::Pass {
        return;
    }
    let mut parts = Vec::new();
    for (field, label) in [
        ("requirement", "requirement"),
        ("id", "check"),
        ("component", "component"),
        ("path", "path"),
    ] {
        if let Some(value) = result.get(field).and_then(telemetry_string) {
            parts.push(format!("{label}={value}"));
        }
    }
    if let Some(error) = result
        .get("error")
        .or_else(|| result.get("message"))
        .and_then(telemetry_string)
    {
        parts.push(error);
    }
    if let Some(actual) = result.get("actual") {
        parts.push(format!("actual={}", telemetry_value_label(actual)));
    }
    if let Some(expected) = result.get("expected") {
        parts.push(format!("expected={}", telemetry_value_label(expected)));
    }
    if parts.is_empty() {
        return;
    }
    let diagnostic = parts.join(" · ");
    if !diagnostics.contains(&diagnostic) {
        diagnostics.push(diagnostic);
    }
}

fn append_report_evidence_detail(
    evidence: &mut BTreeMap<(String, String), RuntimeRequirementEvidence>,
    channel: &str,
    verification: Option<String>,
    source_revision: u64,
    sim_tick: u64,
    result: &BTreeMap<String, TelemetryValue>,
) {
    let Some(requirement_name) = result.get("requirement").and_then(telemetry_string) else {
        return;
    };
    let verdict = evidence_verdict(result);
    let requirement = evidence
        .entry((channel.to_owned(), requirement_name.clone()))
        .or_insert_with(|| RuntimeRequirementEvidence {
            requirement: requirement_name,
            channel: channel.to_owned(),
            verification: verification.clone(),
            source_revision,
            sim_tick,
            checks: 0,
            failures: 0,
            inconclusive: 0,
            errors: 0,
            unverified: 0,
            details: Vec::new(),
        });
    requirement.verification.clone_from(&verification);
    requirement.source_revision = source_revision;
    requirement.sim_tick = sim_tick;
    let detail = RuntimeEvidenceCheck {
        id: result.get("id").and_then(telemetry_string),
        component: result.get("component").and_then(telemetry_string),
        kind: result.get("kind").and_then(telemetry_string),
        path: result.get("path").and_then(telemetry_string),
        verdict,
        error: result
            .get("error")
            .or_else(|| result.get("message"))
            .and_then(telemetry_string),
        actual: result.get("actual").map(telemetry_value_label),
        expected: result.get("expected").map(telemetry_value_label),
    };
    if requirement.details.contains(&detail) {
        return;
    }
    requirement.checks = requirement.checks.saturating_add(1);
    match verdict {
        VerificationVerdict::Pass => {}
        VerificationVerdict::Fail => requirement.failures = requirement.failures.saturating_add(1),
        VerificationVerdict::Inconclusive => {
            requirement.inconclusive = requirement.inconclusive.saturating_add(1)
        }
        VerificationVerdict::Error => requirement.errors = requirement.errors.saturating_add(1),
        VerificationVerdict::Unverified => {
            requirement.unverified = requirement.unverified.saturating_add(1)
        }
    }
    if requirement.details.len() < MAX_EVIDENCE_DETAILS_PER_REQUIREMENT {
        requirement.details.push(detail);
    } else if detail.verdict != VerificationVerdict::Pass
        && let Some(index) = requirement
            .details
            .iter()
            .rposition(|item| item.verdict == VerificationVerdict::Pass)
    {
        requirement.details.remove(index);
        requirement.details.push(detail);
    }
}

/// Adds one bounded check detail while preserving exact summary counters.
fn append_evidence_check(
    channel: &mut VerificationChannelSnapshot,
    channel_name: &str,
    verification: Option<String>,
    source_revision: u64,
    sim_tick: u64,
    result: &BTreeMap<String, TelemetryValue>,
) {
    let Some(requirement_name) = result.get("requirement").and_then(telemetry_string) else {
        return;
    };
    let verdict = evidence_verdict(result);
    let requirement = channel
        .requirements
        .entry(requirement_name.clone())
        .or_insert_with(|| RuntimeRequirementEvidence {
            requirement: requirement_name,
            channel: channel_name.to_owned(),
            verification: verification.clone(),
            source_revision,
            sim_tick,
            checks: 0,
            failures: 0,
            inconclusive: 0,
            errors: 0,
            unverified: 0,
            details: Vec::new(),
        });
    requirement.channel = channel_name.to_owned();
    requirement.verification = verification;
    requirement.source_revision = source_revision;
    requirement.sim_tick = sim_tick;
    requirement.checks = requirement.checks.saturating_add(1);
    match verdict {
        VerificationVerdict::Pass => {}
        VerificationVerdict::Fail => requirement.failures = requirement.failures.saturating_add(1),
        VerificationVerdict::Inconclusive => {
            requirement.inconclusive = requirement.inconclusive.saturating_add(1)
        }
        VerificationVerdict::Error => requirement.errors = requirement.errors.saturating_add(1),
        VerificationVerdict::Unverified => {
            requirement.unverified = requirement.unverified.saturating_add(1)
        }
    }

    let detail = RuntimeEvidenceCheck {
        id: result.get("id").and_then(telemetry_string),
        component: result.get("component").and_then(telemetry_string),
        kind: result.get("kind").and_then(telemetry_string),
        path: result.get("path").and_then(telemetry_string),
        verdict,
        error: result
            .get("error")
            .or_else(|| result.get("message"))
            .and_then(telemetry_string),
        actual: result.get("actual").map(telemetry_value_label),
        expected: result.get("expected").map(telemetry_value_label),
    };
    if requirement.details.len() < MAX_EVIDENCE_DETAILS_PER_REQUIREMENT {
        requirement.details.push(detail);
    } else if detail.verdict != VerificationVerdict::Pass
        && let Some(index) = requirement
            .details
            .iter()
            .rposition(|item| item.verdict == VerificationVerdict::Pass)
    {
        requirement.details.remove(index);
        requirement.details.push(detail);
    }
}

fn evidence_verdict(result: &BTreeMap<String, TelemetryValue>) -> VerificationVerdict {
    match result.get("verdict").and_then(telemetry_string).as_deref() {
        Some("pass") => VerificationVerdict::Pass,
        Some("fail") => VerificationVerdict::Fail,
        Some("inconclusive") => VerificationVerdict::Inconclusive,
        Some("error") => VerificationVerdict::Error,
        Some("unverified") => VerificationVerdict::Unverified,
        Some(_) => VerificationVerdict::Error,
        None if matches!(result.get("ok"), Some(TelemetryValue::Bool(true))) => {
            VerificationVerdict::Pass
        }
        None => VerificationVerdict::Fail,
    }
}

pub(crate) fn clear_verification_evidence_on_twin_closed(
    trigger: On<TwinClosed>,
    mut evidence: ResMut<SysmlVerificationEvidence>,
) {
    evidence.by_twin.remove(&trigger.event().twin.raw());
}

fn telemetry_unsigned(value: Option<&TelemetryValue>) -> Option<u64> {
    match value? {
        TelemetryValue::U64(value) => Some(*value),
        TelemetryValue::I64(value) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn telemetry_string(value: &TelemetryValue) -> Option<String> {
    match value {
        TelemetryValue::String(value) => Some(value.clone()),
        _ => None,
    }
}

/// Reads an optional telemetry array without converting its typed contents.
fn telemetry_array(value: &TelemetryValue) -> Option<&[TelemetryValue]> {
    match value {
        TelemetryValue::Array(values) => Some(values),
        _ => None,
    }
}

/// Formats an observed value for the UI and bounds its retained text size.
fn telemetry_value_label(value: &TelemetryValue) -> String {
    let formatted = match value {
        TelemetryValue::F64(value) => value.to_string(),
        TelemetryValue::I64(value) => value.to_string(),
        TelemetryValue::U64(value) => value.to_string(),
        TelemetryValue::Bool(value) => value.to_string(),
        TelemetryValue::String(value) => value.clone(),
        TelemetryValue::Array(_) | TelemetryValue::Map(_) => format!("{value:?}"),
    };
    let mut chars = formatted.chars();
    let label: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() {
        format!("{label}…")
    } else {
        label
    }
}

impl SourceFileView {
    pub(crate) fn document_view(
        &self,
        registry: Option<&DocumentRegistry<SysmlDocument>>,
    ) -> Option<DocumentView> {
        let doc_id = self.document_id?;
        let host = registry?.host(doc_id)?;
        let document = host.document();
        Some(DocumentView {
            id: doc_id,
            generation: document.generation(),
            source: document.source().to_owned(),
            dirty: document.is_dirty(),
            writable: document.origin().is_writable(),
        })
    }
}

#[cfg(test)]
mod model_view_tests {
    use super::*;

    #[test]
    fn model_views_preserve_requirement_links_and_nested_structure() {
        let analysis = SysmlAnalysis::from_files([(
            "model.sysml",
            "requirement def PayloadRequirement;\n\
             part def Rover;\n\
             package Mission {\n\
                 requirement payload : PayloadRequirement { subject rover : Rover; }\n\
                 part rover : Rover;\n\
                 satisfy payload by rover;\n\
                 verification def VerifyPayload { verify payload; }\n\
             }\n",
        )]);
        assert!(
            analysis.diagnostics().is_empty(),
            "analysis diagnostics: {:?}",
            analysis.diagnostics()
        );

        let model_elements = build_model_element_views(&analysis, &[]);
        let structure = build_model_structure(&model_elements);
        let requirements = build_requirement_views(&analysis, &[], &model_elements);
        let requirement = requirements
            .iter()
            .find(|requirement| requirement.display_name == "payload")
            .expect("requirement usage is projected");

        assert_eq!(requirement.subjects.len(), 1);
        let rover = model_elements
            .iter()
            .find(|element| element.qualified_name == "Rover")
            .expect("subject type is in the typed analysis snapshot");
        assert_eq!(
            requirement.subjects[0]
                .target
                .as_ref()
                .map(|target| target.handle),
            Some(rover.handle)
        );
        assert!(!requirement.subjects[0].ambiguous_target);
        assert_eq!(
            requirement
                .satisfied_by
                .iter()
                .map(|element| element.display_name.as_str())
                .collect::<Vec<_>>(),
            ["rover"]
        );
        assert!(
            requirement
                .verification_cases
                .iter()
                .any(|name| name.ends_with("::VerifyPayload"))
        );

        let mission = structure
            .iter()
            .find(|node| node.element.display_name == "Mission")
            .expect("package is a structure root");
        assert!(
            mission.children.iter().any(
                |node| node.element.display_name == "rover" && node.element.kind == "PartUsage"
            )
        );
    }
}
