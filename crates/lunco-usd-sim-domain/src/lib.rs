//! Runtime projection of composed USD component networks into Modelica wrappers.
//!
//! A reusable part applies `LunCoProgramAPI` for its model facet. Modelica remains the
//! authority for equations and member types; USD supplies instances, constant
//! input opinions, and ordinary property connections between public members.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use lunco_cosim_core::UsdSourcedCosim;
use lunco_modelica_ast::ast_extract::{
    parse_model_interface_from_ast, ModelInterface, ModelicaVariableMetadata,
};
use lunco_modelica_ast::{Causality, StoredDefinition};
use lunco_modelica_runtime::{resolve_communication_period_secs, ModelicaSource};
use lunco_modelica_runtime::{
    ModelicaChannels, ModelicaCommand, ModelicaModel, ModelicaNotice, ModelicaSignalLayout,
    ModelicaSignalProvenance, NoticeLevel,
};
use lunco_usd_bevy_core::program::{
    is_modelica_identifier, modelica_identifier, modelica_path_identifier, modelica_source_ref,
    select_synthesizer_name, ProgramGraph, ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER,
    DEFAULT_DOMAIN_SYNTHESIZER,
};
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_bevy_stage::read::UsdReadObject as ComposedReader;
use lunco_usd_bevy_stage::{canonical::CanonicalStages, UsdInstanceProjection, UsdStageAsset};
use lunco_usd_sim_core::PendingEntityWork;
use openusd::sdf::Path as SdfPath;

pub mod network;
pub mod synthesis;

use synthesis::{
    DomainProjectionError, DomainSynthesizer, SynthContext, SynthOutcome, SynthesisLayout,
    SynthesisUnit, SynthesizerRegistry,
};

// The USD side of a Modelica program facet — the class an asset names, the
// lexical rules for member/instance identifiers — is ONE reader, shared with the
// lint fact producer. See `lunco_usd_bevy_core::program`.
pub use lunco_usd_bevy_core::program::is_domain_network_root;

/// Whether a composed component collection is executable in the live runtime.
///
/// `is_domain_network_root` deliberately answers the structural USD question
/// for both runtime and authoring tools. A guide collection is still a real
/// composed graph and must remain visible to `RunLint`, but its members are
/// annotation/fixture data rather than solver participants. Keeping this
/// execution policy at the projection boundary prevents malformed authoring
/// fixtures from entering Modelica while preserving one reader for lint facts.
pub fn is_runtime_domain_network_root(
    view: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    prim: &SdfPath,
) -> bool {
    is_domain_network_root(view, prim)
        && view.text(prim, "purpose").as_deref() != Some("guide")
        && view.boolean(prim, "lunco:lintOnly") != Some(true)
}

/// The scalar interface authored on a USD Modelica program.
#[derive(Component, Clone, Debug)]
pub struct UsdModelicaPortContract {
    pub inputs: BTreeSet<String>,
    pub outputs: BTreeSet<String>,
}

impl UsdModelicaPortContract {
    /// The contract a USD-declared boundary makes, whatever declared it.
    pub fn new(
        inputs: impl IntoIterator<Item = String>,
        outputs: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            inputs: inputs.into_iter().collect(),
            outputs: outputs.into_iter().collect(),
        }
    }
}

/// The authored co-simulation schedule for a USD Modelica participant.
#[derive(Component, Clone, Copy, Debug)]
pub struct UsdModelicaSchedule {
    pub communication_period_secs: f64,
}

fn retire_sim_interface(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .remove::<(lunco_cosim_core::SimComponent, UsdModelicaSchedule)>();
}

/// Generated documents are runtime projections, unlike authored documents
/// whose source must outlive a scene entity. Retire only the generated origin;
/// this guard keeps ordinary document lifecycle semantics untouched.
fn retire_generated_document(
    document: lunco_doc::DocumentId,
    documents: &mut lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>,
) {
    if document.is_unassigned() {
        return;
    }
    let generated = documents.host(document).is_some_and(|host| {
        lunco_modelica_runtime::generated_source::is_generated_origin(host.document().origin())
    });
    if generated {
        documents.remove_document(document);
    }
}

fn queue_retire_generated_document(commands: &mut Commands, document: lunco_doc::DocumentId) {
    commands.queue(move |world: &mut World| {
        if let Some(mut documents) =
            world.get_resource_mut::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()
        {
            retire_generated_document(document, &mut documents);
        }
    });
}

/// Fingerprint of the generated wrapper currently installed on a network root.
#[derive(Component)]
pub struct DomainProjectionState {
    fingerprint: u64,
}

/// Inspectable runtime artifact for diagnostics and API/UI projection —
/// readable through the `GeneratedModelicaSource` query in
/// `lunco-usd-sim-domain-api`.
///
/// This is derived state, never persisted back into USD. Keeping the exact
/// compiler input beside the run entity makes a compiler line actionable: the
/// worker reports errors against `generated://<model>.mo`, a document that
/// exists nowhere on disk, so without a read path those line numbers name text
/// nobody can obtain. Projection metadata is replaced as one component when it
/// changes; consumers use that lifecycle edge for document and telemetry
/// invalidation.
#[derive(Component, Clone, Debug)]
pub struct GeneratedModelicaSource {
    /// Composed USD network root that owns this compilation unit.
    pub network_root: String,
    /// Stable transient document URI used by the Modelica compiler for this unit.
    pub doc_uri: String,
    /// Exact transient Modelica source sent to the compiler.
    pub source: String,
    /// Included composed USD component paths.
    pub component_paths: Vec<String>,
    /// `(prim path, source asset, instantiated class)` per member — the
    /// attribution a `generated://` compile error needs.
    pub members: Vec<(String, String, String)>,
    /// Bundled Modelica source roots required by the emitted classes. This is
    /// returned by the policy so the UI can load real dependencies without
    /// parsing generated source or hardcoding a library name in Rust.
    pub source_roots: Vec<String>,
    /// Causal outputs of generated members promoted to the wrapper boundary.
    /// Each tuple is `(member USD path, member output, wrapper output)`.
    ///
    /// A generated network is one solver participant, but its composed USD
    /// members remain addressable presentation/topology nodes. This map is the
    /// generic address translation that lets an external USD consumer (for
    /// example a light or a telemetry adapter) read a member output without
    /// creating a second solver for that member.
    pub member_output_aliases: Vec<(String, String, String)>,
    /// Deterministic composite units selected by the synthesizer.
    pub units: Vec<SynthesisUnit>,
    /// Public causal inputs on the generated root. Kept separate from
    /// promoted member telemetry so the workbench can explain the interface.
    pub boundary_inputs: Vec<String>,
    /// Public causal outputs authored on the generated root.
    pub boundary_outputs: Vec<String>,
    /// Unit and member positions selected by the synthesizer policy.
    pub layout: SynthesisLayout,
    /// Error produced while this USD network was being projected. Runtime
    /// solver errors belong to `ModelicaModel` and are not source-projection
    /// metadata.
    pub projection_error: Option<String>,
}

/// One public Modelica component facet authored in USD.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainComponent {
    /// Composed USD path of the `LunCoProgramAPI` facet.
    pub path: String,
    /// The `info:sourceAsset` this facet names — the file whose `within` + class
    /// decides what [`MemberClasses`] exposes to the selected synthesis policy.
    pub source_asset: String,
    /// Fully-qualified class declared by the loaded `info:sourceAsset` source.
    pub model_class: String,
    /// Constant public inputs, supplied to the selected synthesis policy as
    /// component modifications.
    pub constants: BTreeMap<String, f64>,
    /// Acausal member name to the connected `connectors:*` property path.
    pub connectors: BTreeMap<String, Vec<String>>,
    /// All declared acausal members, including currently unconnected pins.
    pub declared_connectors: BTreeSet<String>,
    /// Causal input name to its connected source property path.
    pub inputs: BTreeMap<String, String>,
    /// Public causal outputs declared by the reusable model facet.
    pub declared_outputs: BTreeSet<String>,
    /// Optional presentation role for a generated Modelica topology. This is
    /// USD-authored metadata, not a solver direction: acausal Modelica flow
    /// remains reversible and runtime sign still controls animated direction.
    pub topology_role: String,
}

/// One network root and its public causal boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainNetwork {
    /// Composed path of the USD prim carrying `CollectionAPI:components`.
    pub root: String,
    /// Modelica component facets in the root's explicit collection.
    pub components: Vec<DomainComponent>,
    /// Public wrapper inputs authored on the network root.
    pub inputs: BTreeSet<String>,
    /// Public wrapper input name to its composed external source.
    pub input_sources: BTreeMap<String, String>,
    /// Public wrapper output name to component output property.
    pub outputs: BTreeMap<String, String>,
    /// One master-clock communication period for the generated Modelica
    /// wrapper. Every composed member must resolve to the same lattice point;
    /// a single solver cannot honor conflicting member schedules.
    pub communication_period_secs: f64,
    /// At least one member's `.mo` has not loaded and therefore its declared
    /// class is not available. The projector waits instead of compiling a
    /// partial network. See [`MemberClasses`].
    pub pending_sources: bool,
}

/// A domain synthesis request that owns no OpenUSD handles.
///
/// The initial asset projection plan is prepared by the asset loader and is
/// shared by every network task. The task owns the policy call and source
/// validation; the main thread only commits the resulting ECS/Modelica state.
struct PendingDomainProjection {
    entity: Entity,
    stage_id: AssetId<UsdStageAsset>,
    stage_generation: u64,
    /// Prepared runtime instances are immutable read surfaces. Their scene
    /// stage generation may advance when the spawn layer is authored, so the
    /// canonical-generation fence only applies to ordinary scene plans.
    instance_plan: bool,
    root_path: String,
    model_name: String,
    requested: String,
    plan: Arc<lunco_usd_bevy_stage::UsdStageProjectionPlan>,
    task: Task<Result<SynthOutcome, Vec<DomainProjectionError>>>,
}

/// In-flight domain synthesis owned by the scene projection lifecycle.
///
/// One network has one synthesis owner. Completion is fenced by the USD entity
/// and either the ordinary canonical-stage generation or the exact immutable
/// prepared instance plan before it can publish a result.
#[derive(Resource, Default)]
pub struct PendingDomainProjections {
    tasks: Vec<PendingDomainProjection>,
}

/// Domain-root candidates discovered from USD entity and source lifecycles.
/// Discovery and projection are separate queues so settled class assets can
/// reproject only the networks that use them.
#[derive(Resource)]
pub struct PendingDomainProjectionCandidates {
    discovery: HashSet<Entity>,
    projection: HashSet<Entity>,
    initial_discovery: bool,
    observed_stage_generations: HashMap<AssetId<UsdStageAsset>, u64>,
}

impl Default for PendingDomainProjectionCandidates {
    fn default() -> Self {
        Self {
            discovery: HashSet::new(),
            projection: HashSet::new(),
            initial_discovery: true,
            observed_stage_generations: HashMap::new(),
        }
    }
}

impl PendingDomainProjectionCandidates {
    pub fn has_projection_work(&self) -> bool {
        !self.projection.is_empty()
    }

    fn observe_canonical_stage_generations(&mut self, stages: &CanonicalStages) -> bool {
        let mut changed = false;
        for (asset, stage) in stages.iter() {
            let generation = stage.generation();
            if self.observed_stage_generations.get(&asset) != Some(&generation) {
                self.observed_stage_generations.insert(asset, generation);
                changed = true;
            }
        }
        if self.observed_stage_generations.len() != stages.len() {
            self.observed_stage_generations
                .retain(|asset, _| stages.get(*asset).is_some());
            changed = true;
        }
        changed
    }

    fn reset_for_scene(&mut self) {
        *self = Self::default();
    }
}

/// Generated-source owners that still need their inspectable Modelica document
/// synchronized. Insert/remove observers feed this queue; one bootstrap pass
/// covers owners that predate observer installation.
#[derive(Resource)]
pub struct PendingGeneratedSourceDocuments(PendingEntityWork);

impl Default for PendingGeneratedSourceDocuments {
    fn default() -> Self {
        Self(PendingEntityWork::with_initial_discovery())
    }
}

pub fn queue_generated_source_document_sync(
    trigger: On<Insert, GeneratedModelicaSource>,
    mut pending: ResMut<PendingGeneratedSourceDocuments>,
) {
    pending.0.queue(trigger.entity);
}

pub fn queue_model_document_sync_for_generated_source(
    trigger: On<Insert, ModelicaModel>,
    generated_sources: Query<(), With<GeneratedModelicaSource>>,
    mut pending: ResMut<PendingGeneratedSourceDocuments>,
) {
    if generated_sources.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

pub fn forget_generated_source_document_sync(
    trigger: On<Remove, GeneratedModelicaSource>,
    mut pending: ResMut<PendingGeneratedSourceDocuments>,
) {
    pending.0.forget(trigger.entity);
}

pub fn generated_source_document_sync_due(pending: Res<PendingGeneratedSourceDocuments>) -> bool {
    pending.0.has_work()
}

pub fn mark_generated_sources_dirty_on_insert(
    _: On<Insert, GeneratedModelicaSource>,
    mut generated: ResMut<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>,
) {
    generated.dirty = true;
}

pub fn domain_projection_due(candidates: Res<PendingDomainProjectionCandidates>) -> bool {
    candidates.has_projection_work()
}

/// Reverse index from a Modelica source asset to the domain roots that depend
/// on its declared class. Source completion then invalidates those roots only.
#[derive(Resource, Default)]
pub struct DomainClassUsers {
    roots_by_asset: HashMap<String, HashSet<Entity>>,
    assets_by_root: HashMap<Entity, HashSet<String>>,
}

impl DomainClassUsers {
    fn root_sources_settled(&self, root: Entity, classes: &MemberClasses) -> bool {
        self.assets_by_root
            .get(&root)
            .is_none_or(|assets| assets.iter().all(|asset| classes.known.contains_key(asset)))
    }

    fn replace_root_assets(&mut self, root: Entity, assets: HashSet<String>) {
        self.remove_root(root);
        for asset in &assets {
            self.roots_by_asset
                .entry(asset.clone())
                .or_default()
                .insert(root);
        }
        if !assets.is_empty() {
            self.assets_by_root.insert(root, assets);
        }
    }

    fn remove_root(&mut self, root: Entity) {
        let Some(assets) = self.assets_by_root.remove(&root) else {
            return;
        };
        for asset in assets {
            if let Some(roots) = self.roots_by_asset.get_mut(&asset) {
                roots.remove(&root);
                if roots.is_empty() {
                    self.roots_by_asset.remove(&asset);
                }
            }
        }
    }

    fn clear(&mut self) {
        self.roots_by_asset.clear();
        self.assets_by_root.clear();
    }
}

/// Retire scene-owned discovery state before a replacement scene is admitted.
/// Resolved Modelica class facts remain cached because they are asset-owned.
pub fn reset_scene_projection_work(
    mut users: ResMut<DomainClassUsers>,
    mut candidates: ResMut<PendingDomainProjectionCandidates>,
    mut generated_documents: ResMut<PendingGeneratedSourceDocuments>,
) {
    users.clear();
    candidates.reset_for_scene();
    generated_documents.0 = PendingEntityWork::with_initial_discovery();
}

pub fn queue_added_domain_prim(
    trigger: On<Add, UsdPrimPath>,
    mut pending: ResMut<PendingDomainProjectionCandidates>,
) {
    pending.discovery.insert(trigger.entity);
}

pub fn queue_added_domain_identity(
    trigger: On<Add, lunco_core::GlobalEntityId>,
    prims: Query<(), With<UsdPrimPath>>,
    mut pending: ResMut<PendingDomainProjectionCandidates>,
) {
    if prims.contains(trigger.entity) {
        pending.discovery.insert(trigger.entity);
    }
}

pub fn queue_added_domain_instance_projection(
    trigger: On<Add, UsdInstanceProjection>,
    prims: Query<(), With<UsdPrimPath>>,
    mut pending: ResMut<PendingDomainProjectionCandidates>,
) {
    if prims.contains(trigger.entity) {
        pending.discovery.insert(trigger.entity);
    }
}

pub fn queue_removed_domain_identity(
    trigger: On<Remove, lunco_core::GlobalEntityId>,
    prims: Query<(), With<UsdPrimPath>>,
    mut pending: ResMut<PendingDomainProjectionCandidates>,
) {
    if prims.contains(trigger.entity) {
        pending.discovery.insert(trigger.entity);
    }
}

pub fn queue_removed_domain_instance_projection(
    trigger: On<Remove, UsdInstanceProjection>,
    prims: Query<(), With<UsdPrimPath>>,
    mut pending: ResMut<PendingDomainProjectionCandidates>,
) {
    if prims.contains(trigger.entity) {
        pending.discovery.insert(trigger.entity);
    }
}

pub fn forget_domain_projection_entity(
    trigger: On<Remove, UsdPrimPath>,
    mut users: ResMut<DomainClassUsers>,
    mut pending: ResMut<PendingDomainProjectionCandidates>,
) {
    users.remove_root(trigger.entity);
    pending.discovery.remove(&trigger.entity);
    pending.projection.remove(&trigger.entity);
}

fn queue_domain_projection(
    pending: &mut PendingDomainProjections,
    entity: Entity,
    stage_id: AssetId<UsdStageAsset>,
    stage_generation: u64,
    root_path: &SdfPath,
    model_name: String,
    requested: String,
    synthesizer: Arc<dyn DomainSynthesizer>,
    plan: Arc<lunco_usd_bevy_stage::UsdStageProjectionPlan>,
    instance_plan: bool,
    classes: MemberClasses,
) {
    let root_path_string = root_path.to_string();
    let task_root = root_path.clone();
    let task_model_name = model_name.clone();
    let task_plan = plan.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let view: &dyn ComposedReader = task_plan.as_ref();
        let context = SynthContext { classes: &classes };
        synthesizer.synthesize(view, &task_root, &task_model_name, &context)
    });
    pending.tasks.push(PendingDomainProjection {
        entity,
        stage_id,
        stage_generation,
        instance_plan,
        root_path: root_path_string,
        model_name,
        requested,
        plan,
        task,
    });
}

fn resolve_domain_synthesizer(
    view: &dyn ComposedReader,
    root_path: &SdfPath,
    prim_path: &str,
    registry: &SynthesizerRegistry,
) -> Option<(String, Arc<dyn DomainSynthesizer>)> {
    let requested = match select_synthesizer_name(view, root_path) {
        Ok(name) => name,
        Err(message) => {
            error!("[domain-projection] `{prim_path}` rejected: {message}");
            return None;
        }
    };
    let Some(synthesizer) = registry.get(&requested).cloned() else {
        let known = registry.names().join(", ");
        error!(
            "[domain-projection] `{prim_path}` names synthesizer `{requested}`, which is not \
             registered (known: {known}) — the scope is not projected."
        );
        return None;
    };
    Some((requested, synthesizer))
}

/// Commit one completed synthesis result on the main thread.
///
/// Rhai execution, graph extraction, and generated-source validation happen in
/// the task above. This function is the single publication path for both the
/// prepared startup plan and the canonical live-edit reader, so the Modelica
/// lifecycle, signal layout, and generated-source diagnostics cannot diverge.
fn commit_domain_projection(
    commands: &mut Commands,
    entity: Entity,
    prim: &UsdPrimPath,
    previous: Option<&DomainProjectionState>,
    installed_model: Option<&ModelicaModel>,
    root_path: &SdfPath,
    view: &dyn ComposedReader,
    classes: &MemberClasses,
    channels: &ModelicaChannels,
    requested: &str,
    model_name: &str,
    synthesized: Result<SynthOutcome, Vec<DomainProjectionError>>,
    notices: &mut MessageWriter<ModelicaNotice>,
) -> bool {
    let synthesized = match synthesized {
        Ok(synthesized) => synthesized,
        Err(errors) => {
            let message = errors
                .iter()
                .map(|error| format!("{}: {}", error.path, error.message))
                .collect::<Vec<_>>()
                .join("; ");
            let fingerprint = source_fingerprint(&format!("projection-error:{message}"));
            if previous.is_some_and(|state| state.fingerprint == fingerprint) {
                return false;
            }
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: format!("[{model_name}] Projection error: {message}"),
            });
            error!("[domain-projection] `{}` rejected: {message}", prim.path);
            retire_sim_interface(commands, entity);
            if let Some(model) = installed_model {
                queue_retire_generated_document(commands, model.document);
            }
            commands
                .entity(entity)
                .remove::<(UsdModelicaPortContract, ModelicaSignalLayout)>();
            commands.entity(entity).try_insert((
                ModelicaModel {
                    model_name: model_name.to_string(),
                    source_uri: format!("generated://{model_name}.mo"),
                    session_id: installed_model.map_or(1, |model| model.session_id + 1),
                    is_stepping: false,
                    is_compiling: false,
                    last_error: Some(message.clone()),
                    ..default()
                },
                UsdSourcedCosim,
                DomainProjectionState { fingerprint },
                GeneratedModelicaSource {
                    network_root: prim.path.clone(),
                    doc_uri: format!("generated://{model_name}.mo"),
                    source: String::new(),
                    component_paths: Vec::new(),
                    members: Vec::new(),
                    source_roots: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    layout: SynthesisLayout::default(),
                    projection_error: Some(message),
                },
            ));
            return false;
        }
    };

    if matches!(synthesized, SynthOutcome::Pending) {
        return false;
    }
    let SynthOutcome::Ready(synthesized) = synthesized else {
        if previous.is_some() {
            retire_sim_interface(commands, entity);
            if let Some(model) = installed_model {
                queue_retire_generated_document(commands, model.document);
            }
            commands.entity(entity).remove::<(
                ModelicaModel,
                ModelicaSignalLayout,
                UsdSourcedCosim,
                UsdModelicaPortContract,
                DomainProjectionState,
                GeneratedModelicaSource,
            )>();
        }
        return false;
    };

    let component_count = synthesized.component_paths.len();
    let interface = synthesized.interface;
    let source = synthesized.source;
    let source_for_diagnostics = source.clone();
    let fingerprint = source_fingerprint(&source);
    if previous.is_some_and(|state| state.fingerprint == fingerprint) {
        return false;
    }

    // The synthesizer already parsed and validated this source once. Carry its
    // interface through installation so the runtime does not recover it again.
    let compiled_name = interface
        .model_name
        .unwrap_or_else(|| model_name.to_string());
    let declared_output_ports = interface.outputs.clone();
    let session_id = installed_model.map_or(1, |model| model.session_id + 1);
    let doc_uri = format!("generated://{model_name}.mo");
    let mut model = ModelicaModel {
        model_name: compiled_name.clone(),
        source_uri: doc_uri.clone(),
        parameters: interface.parameters,
        inputs: interface.inputs,
        communication_period_secs: synthesized.communication_period_secs,
        session_id,
        is_stepping: true,
        is_compiling: true,
        resume_after_compile: true,
        ..default()
    };
    let member_output_aliases = synthesized
        .member_output_aliases
        .iter()
        .filter(|(_, _, alias)| interface.outputs.contains(alias))
        .cloned()
        .collect::<Vec<_>>();
    let signal_layout = match generated_signal_layout(
        view,
        root_path,
        &prim.path,
        &synthesized.outputs,
        &synthesized.members,
        &member_output_aliases,
        &synthesized.units,
        classes,
    ) {
        Ok(layout) => layout,
        Err(message) => {
            let message = format!("generated signal layout failed: {message}");
            model.is_stepping = false;
            model.is_compiling = false;
            model.last_error = Some(message.clone());
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: format!("[{}] Projection error: {message}", model.model_name),
            });
            error!("[domain-projection] {} rejected: {message}", prim.path);
            retire_sim_interface(commands, entity);
            commands.entity(entity).try_insert(model);
            return false;
        }
    };
    let projection_error = match channels.tx.send(ModelicaCommand::Compile {
        entity,
        session_id,
        model_name: compiled_name,
        source,
        doc_uri: doc_uri.clone(),
        extra_sources: Vec::new(),
        parameter_overrides: Vec::new(),
        stream: None,
        // The worker, not this projector, owns backend selection and DAE
        // lowering for generated domain networks.
        realtime_safe: false,
    }) {
        Ok(()) => {
            info!(
                "[domain-projection] compiling `{}` from {} component(s) via `{requested}` as \
                 generated://{}.mo",
                prim.path, component_count, model_name
            );
            None
        }
        Err(error) => {
            let message = format!("could not dispatch generated model compile: {error}");
            model.is_stepping = false;
            model.is_compiling = false;
            model.last_error = Some(message.clone());
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: format!("[{}] Compile error: {message}", model.model_name),
            });
            Some(message)
        }
    };
    let generated_source = GeneratedModelicaSource {
        network_root: prim.path.clone(),
        doc_uri,
        source: source_for_diagnostics,
        component_paths: synthesized.component_paths,
        members: synthesized.members,
        source_roots: synthesized.source_roots.into_iter().collect(),
        member_output_aliases,
        units: synthesized.units,
        boundary_inputs: synthesized.inputs.iter().cloned().collect(),
        boundary_outputs: synthesized.outputs.iter().cloned().collect(),
        layout: synthesized.layout,
        projection_error,
    };
    retire_sim_interface(commands, entity);
    commands.entity(entity).try_insert((
        model,
        signal_layout,
        UsdSourcedCosim,
        lunco_port_core::PortSurfacePending,
        UsdModelicaPortContract::new(synthesized.inputs.iter().cloned(), declared_output_ports),
        UsdModelicaSchedule {
            communication_period_secs: synthesized.communication_period_secs,
        },
        DomainProjectionState { fingerprint },
        generated_source,
    ));
    true
}

/// Reactively compile every prim containing a standard component collection of
/// Modelica program facets. The generated source is runtime projection only.
pub fn project_domain_islands(
    mut commands: Commands,
    preview: (
        Query<&ChildOf>,
        Query<(), With<lunco_usd_bevy_scene::UsdPreviewOnly>>,
    ),
    prims: Query<(
        Entity,
        &UsdPrimPath,
        Option<&DomainProjectionState>,
        Option<&ModelicaModel>,
        Option<&UsdInstanceProjection>,
    )>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_instance_root: Query<(), With<lunco_usd_bevy_stage::UsdInstanceRoot>>,
    // A runtime-instanced descendant stays out of Modelica synthesis while its
    // root identity is pending. Once the root GID is available, the durable
    // instance projection scopes the generated session even after the transient
    // membership marker is consumed.
    q_instance_member: Query<(), With<lunco_usd_bevy_stage::UsdInstanceMember>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut pending: ResMut<PendingDomainProjections>,
    mut candidates: ResMut<PendingDomainProjectionCandidates>,
    class_users: Res<DomainClassUsers>,
    classes: Res<MemberClasses>,
    registry: Res<SynthesizerRegistry>,
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<ModelicaNotice>,
) {
    let Some(channels) = channels else { return };
    if candidates.projection.is_empty() {
        return;
    }
    let started = web_time::Instant::now();
    let mut projected = 0usize;
    let mut candidate_entities: Vec<_> = candidates.projection.drain().collect();
    candidate_entities.sort_unstable();
    let candidate_set: HashSet<_> = candidate_entities.iter().copied().collect();
    // Invalidate only in-flight synthesis for roots whose source view changed.
    // Unrelated network tasks remain valid and continue without restarting.
    pending
        .tasks
        .retain(|task| !candidate_set.contains(&task.entity));
    for entity in candidate_entities {
        let Ok((entity, prim, previous, installed_model, instance_projection)) = prims.get(entity)
        else {
            continue;
        };
        if lunco_usd_bevy_scene::is_preview_only(entity, &preview.0, &preview.1) {
            continue;
        }
        // Source asset arrivals invalidate their dependent roots individually.
        // Do not run graph extraction for a partially loaded network: the
        // synthesizer would return Pending after traversing the same USD graph,
        // then repeat that work for each remaining class arrival.
        if !class_users.root_sources_settled(entity, &classes) {
            continue;
        }
        // Scope every authored path to the same USD instance as the generated
        // network. Runtime-spawned copies intentionally share stage-relative
        // paths; the instance root identity is the structural disambiguator.
        let instance_id = lunco_usd_bevy_scene::instance_key_from_projection(
            entity,
            &q_provenance,
            &q_gid,
            &q_instance_root,
            instance_projection,
        );
        // Identity still pending — wait for it rather than compile under a name
        // that is neither stable nor unique. The upgrade lands a
        // `GlobalEntityId`, which re-triggers this system through
        // `identity_added`.
        if q_instance_member.contains(entity) {
            continue;
        }
        // Runtime-spawned copies may have byte-identical stage-relative paths.
        // Use the same stable instance-root identity as the USD wiring resolver;
        // scene-owned prims need no suffix because their composed paths are unique.
        let id = prim.stage_handle.id();
        let Some(stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        let (reader, stage_generation) =
            canonical.reader_for_entity(id, stage_asset, instance_projection);
        let Ok(root_path) = SdfPath::new(&prim.path) else {
            continue;
        };
        if stage_generation == 0 {
            if pending.tasks.iter().any(|task| task.entity == entity) {
                continue;
            }
            // A runtime reference owns a remapped prepared plan.  The scene's
            // base plan contains only the stage as it was loaded and therefore
            // cannot see the referenced component collection added by a later
            // spawn.  Keep the task on the same immutable read surface that
            // admitted the root so discovery and synthesis cannot disagree
            // about the composed network.
            let plan = instance_projection
                .map(|projection| projection.plan.clone())
                .unwrap_or_else(|| {
                    stages
                        .get(&prim.stage_handle)
                        .expect("loaded USD asset always carries a prepared projection plan")
                        .projection_plan
                        .clone()
                });
            let plan_view: &dyn ComposedReader = &reader;
            if !is_runtime_domain_network_root(plan_view, &root_path) {
                continue;
            }
            let Some((requested, synthesizer)) =
                resolve_domain_synthesizer(plan_view, &root_path, &prim.path, &registry)
            else {
                continue;
            };
            let model_name = network_model_name(&prim.path, instance_id);
            queue_domain_projection(
                &mut pending,
                entity,
                id,
                stage_generation,
                &root_path,
                model_name,
                requested,
                synthesizer,
                plan,
                instance_projection.is_some(),
                classes.clone(),
            );
            continue;
        }
        // Domain projection owns only prims with the standard component
        // collection.  Keep this structural gate ahead of synthesizer
        // selection: deriving ownership for an ordinary prim would walk its
        // collection metadata even though it cannot be a network root.
        if !is_runtime_domain_network_root(&reader, &root_path) {
            continue;
        }
        // Domain ownership is derived from the typed member role schemas. A
        // domain API may still explicitly select a registered non-default
        // policy for a generic Modelica collection; physical actuator
        // collections have no exposed selector and are classified from their
        // `LunCoForceActuatorAPI` members.
        let Some((requested, synthesizer)) =
            resolve_domain_synthesizer(&reader, &root_path, &prim.path, &registry)
        else {
            continue;
        };
        let model_name = network_model_name(&prim.path, instance_id);
        let synthesized = synthesizer.synthesize(
            &reader,
            &root_path,
            &model_name,
            &SynthContext { classes: &classes },
        );
        if commit_domain_projection(
            &mut commands,
            entity,
            prim,
            previous,
            installed_model,
            &root_path,
            &reader,
            &classes,
            &channels,
            &requested,
            &model_name,
            synthesized,
            &mut notices,
        ) {
            projected += 1;
        }
        continue;
    }
    if projected > 0 {
        bevy::log::debug!(
            "[domain-projection] prepared {projected} network(s) in {:.2} ms",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
}

/// Publish completed startup synthesis tasks without making the UI schedule
/// wait for Rhai, network extraction, or generated-source validation.
pub fn poll_domain_projection_tasks(
    mut commands: Commands,
    preview: (
        Query<&ChildOf>,
        Query<(), With<lunco_usd_bevy_scene::UsdPreviewOnly>>,
    ),
    mut pending: ResMut<PendingDomainProjections>,
    prims: Query<(
        &UsdPrimPath,
        Option<&DomainProjectionState>,
        Option<&ModelicaModel>,
        Option<&UsdInstanceProjection>,
    )>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    classes: Res<MemberClasses>,
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<ModelicaNotice>,
) {
    let Some(channels) = channels else { return };
    let mut index = 0;
    while index < pending.tasks.len() {
        // A task may finish after its entity enters a presentation-only lease.
        // Cancel before polling so it cannot publish a runtime participant.
        if lunco_usd_bevy_scene::is_preview_only(
            pending.tasks[index].entity,
            &preview.0,
            &preview.1,
        ) {
            pending.tasks.swap_remove(index);
            continue;
        }
        let ready = block_on(future::poll_once(&mut pending.tasks[index].task));
        let Some(synthesized) = ready else {
            index += 1;
            continue;
        };
        let task = pending.tasks.swap_remove(index);
        let Ok((prim, previous, installed_model, instance_projection)) = prims.get(task.entity)
        else {
            continue;
        };
        if prim.stage_handle.id() != task.stage_id {
            continue;
        }
        let Some(_stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        if task.instance_plan {
            let Some(instance_projection) = instance_projection else {
                continue;
            };
            if !Arc::ptr_eq(&task.plan, &instance_projection.plan) {
                continue;
            }
        } else if canonical.generation_for(task.stage_id) != task.stage_generation {
            continue;
        }
        let Ok(root_path) = SdfPath::new(&task.root_path) else {
            continue;
        };
        let view: &dyn ComposedReader = task.plan.as_ref();
        commit_domain_projection(
            &mut commands,
            task.entity,
            prim,
            previous,
            installed_model,
            &root_path,
            view,
            &classes,
            &channels,
            &task.requested,
            &task.model_name,
            synthesized,
            &mut notices,
        );
    }
}

/// Give every successful generated network a normal, read-only Modelica
/// document. This is intentionally a separate system: the compiler projection
/// owns synthesis, while the document registry owns inspectable source and the
/// scene-to-document link used by the standard Modelica UI and API.
pub fn sync_generated_network_documents(
    mut generated: Query<(Entity, &GeneratedModelicaSource, &mut ModelicaModel)>,
    source_entities: Query<Entity, With<GeneratedModelicaSource>>,
    mut pending: ResMut<PendingGeneratedSourceDocuments>,
    mut documents: ResMut<
        lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>,
    >,
    mut generated_metadata: ResMut<
        lunco_modelica_runtime::generated_source::GeneratedModelicaSources,
    >,
) {
    let mut entities = pending.0.take_queued();
    if pending.0.take_initial_discovery() {
        entities.extend(source_entities.iter());
    }
    let mut entities: Vec<_> = entities.into_iter().collect();
    entities.sort_unstable();
    for entity in entities {
        let Ok((entity, source, mut model)) = generated.get_mut(entity) else {
            continue;
        };
        // Projection errors are represented by an empty diagnostic source and
        // must not create a misleading editable-looking blank document.
        if source.source.is_empty() {
            continue;
        }
        // Generated documents use the same source-aware class resolver as
        // authored Modelica documents. Request every referenced bundled root
        // asynchronously; the canvas shows an explicit loading state until
        // the shared engine publishes the generic completion notification.
        if let Some(handle) = lunco_modelica_core::engine_resource::global_engine_handle() {
            let mut roots: BTreeSet<String> = source.source_roots.iter().cloned().collect();
            roots.extend(
                source
                    .members
                    .iter()
                    .filter_map(|(_, _, class)| class.split('.').next())
                    .map(str::to_string),
            );
            for root in roots {
                let _ = handle.ensure_source_root_async(&root);
            }
        }
        let document = if !model.document.is_unassigned()
            && documents.host(model.document).is_some()
        {
            model.document
        } else {
            documents.allocate(
                source.source.clone(),
                lunco_doc::PathlessOrigin::bundled(format!("generated/{}.mo", model.model_name)),
            )
        };
        documents.reload_external_source(document, &source.source);
        if let Err(error) = documents.link(entity, document) {
            bevy::log::warn!(
                "[ModelicaProjection] failed to link entity {entity} to document {document}: {error}"
            );
            continue;
        }
        model.document = document;
        generated_metadata.dirty = true;
    }
}

/// Remove the ephemeral source/document metadata when a generated component is
/// removed for any reason, including scene despawn. The normal Modelica
/// cleanup intentionally keeps authored documents, so generated lifecycle has
/// its own narrowly classified observer.
pub fn on_remove_generated_source(
    trigger: On<Remove, GeneratedModelicaSource>,
    source_query: Query<(&GeneratedModelicaSource, Option<&ModelicaModel>)>,
    mut documents: Option<
        ResMut<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>,
    >,
    mut generated: Option<
        ResMut<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>,
    >,
) {
    let (network_root, doc_uri, model_document) = source_query
        .get(trigger.entity)
        .map(|(source, model)| {
            (
                Some(source.network_root.clone()),
                Some(source.doc_uri.clone()),
                model.map(|m| m.document),
            )
        })
        .unwrap_or((None, None, None));
    let document = documents.as_deref_mut().and_then(|registry| {
        let document = model_document
            .filter(|document| !document.is_unassigned())
            .or_else(|| {
                let model_name = doc_uri
                    .as_deref()?
                    .strip_prefix("generated://")?
                    .strip_suffix(".mo")?;
                registry.find_bundled(&format!("generated/{model_name}.mo"))
            })?;
        let is_generated = registry.host(document).is_some_and(|host| {
            lunco_modelica_runtime::generated_source::is_generated_origin(host.document().origin())
        });
        if is_generated {
            registry.remove_document(document);
            Some(document)
        } else {
            None
        }
    });
    if let Some(metadata) = generated.as_deref_mut() {
        metadata.entries.retain(|entry| {
            network_root
                .as_deref()
                .is_none_or(|root| entry.network_root != root)
                && document.is_none_or(|doc| entry.document != doc)
        });
        metadata.dirty = true;
    }
}

/// Publish the current generated sources to the UI-facing derived registry.
pub fn publish_generated_sources(
    q_generated: Query<(&GeneratedModelicaSource, Option<&ModelicaModel>)>,
    mut generated: ResMut<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>,
) {
    generated.entries = q_generated
        .iter()
        .map(|(source, model)| {
            lunco_modelica_runtime::generated_source::GeneratedModelicaSourceEntry {
                document: model.map(|m| m.document).unwrap_or_default(),
                uri: model
                    .map(|m| format!("generated://{}.mo", m.model_name))
                    .unwrap_or_else(|| {
                        format!(
                            "generated://{}.mo",
                            source.network_root.trim_matches('/').replace('/', "_")
                        )
                    }),
                network_root: source.network_root.clone(),
                model_name: model
                    .map(|m| m.model_name.clone())
                    .unwrap_or_else(|| source.network_root.trim_matches('/').replace('/', "_")),
                source: source.source.clone(),
                component_paths: source.component_paths.clone(),
                units: source
                    .units
                    .iter()
                    .map(
                        |unit| lunco_modelica_runtime::generated_source::GeneratedModelicaUnit {
                            name: unit.name.clone(),
                            instance: unit.instance.clone(),
                            members: unit.component_paths.clone(),
                            inputs: unit.inputs.iter().cloned().collect(),
                            outputs: unit.outputs.iter().cloned().collect(),
                        },
                    )
                    .collect(),
                members: source.members.clone(),
                source_roots: source.source_roots.clone(),
                boundary_inputs: source.boundary_inputs.clone(),
                boundary_outputs: source.boundary_outputs.clone(),
                member_output_aliases: source.member_output_aliases.clone(),
                projection_error: source.projection_error.clone(),
            }
        })
        .collect();
    generated.dirty = false;
}

/// Change gate for the generated metadata publisher. Generated-source
/// lifecycle observers and document-link/removal owners set the shared dirty
/// flag; runtime solver output is deliberately outside this metadata contract.
pub fn generated_sources_need_publish(
    generated: Res<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>,
) -> bool {
    generated.dirty
}

/// Stable, path-qualified identity for a generated network model.
///
/// The leaf name alone is not unique: a stage may contain several independent
/// scopes with the same leaf name. Including the composed prim path also keeps
/// worker sessions and diagnostics attributable to the authored network.
fn network_model_name(root: &str, global_id: Option<u64>) -> String {
    let path = modelica_path_identifier(root.trim_matches('/'));
    match global_id {
        Some(global_id) => format!("{path}_G{global_id}_System"),
        None => format!("{path}_System"),
    }
}

/// The workspace hashing substrate, not `DefaultHasher`: this value decides
/// whether a live edit recompiles, and one definition of "same source" is worth
/// more than a std default whose stability is unspecified.
fn source_fingerprint(source: &str) -> u64 {
    lunco_hash::fnv1a64(source.as_bytes())
}

/// Stable, readable Modelica instance name for a composed USD member.
///
/// The full prim path remains the authoritative identity in members, signal
/// provenance, and USD. It is a poor display name, though: emitting the
/// assembly name made a six-member diagram read SolarRover__Motor__FL.
/// Prefer the member leaf (Motor_FL) and add its immediate parent only for
/// nested members (YawHead__SolarPanel). The network root's parent is used
/// as the common assembly scope because composed members may sit beside the
/// component collection rather than below it. validate_network still
/// rejects a same-name collision; it must be fixed in USD rather than hidden
/// by a numeric fallback.
fn instance_identifier(root: &str, path: &str) -> Result<String, String> {
    let root_scope = root
        .trim_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| format!("/{}", parent))
        .unwrap_or_else(|| root.trim_matches('/').to_string());
    let relative = path
        .strip_prefix(root)
        .or_else(|| path.strip_prefix(root_scope.as_str()))
        .unwrap_or(path)
        .trim_matches('/');
    let mut segments = relative.split('/').filter(|segment| !segment.is_empty());
    let Some(last) = segments.next_back() else {
        return Err(format!(
            "generated Modelica member path `{path}` has no name relative to network root `{root}`"
        ));
    };
    let Some(parent) = segments.next_back() else {
        return Ok(modelica_identifier(last));
    };
    Ok(format!(
        "{}__{}",
        modelica_identifier(parent),
        modelica_identifier(last)
    ))
}

/// Whether a generated member output already has an authored operator-facing
/// telemetry declaration.  Such a declaration owns the public channel name;
/// the generated wrapper alias remains available as implementation state but
/// must not be classified as a second public channel for the same value.
fn has_authored_telemetry_for_output(
    view: &dyn ComposedReader,
    member: &str,
    output: &str,
) -> bool {
    let Ok(path) = SdfPath::new(member) else {
        return false;
    };
    if view.boolean(&path, "lunco:telemetry") == Some(true)
        && view
            .text(&path, "lunco:telemetry:port")
            .is_some_and(|port| port == output)
    {
        return true;
    }

    // One prim can carry one LunCoTelemetryAPI declaration. Additional
    // operator channels are authored as declaration prims that target the
    // measured member through the same API's relationship, so they still
    // suppress a duplicate generated public alias.
    view.prim_paths().into_iter().any(|candidate| {
        view.boolean(&candidate, "lunco:telemetry") == Some(true)
            && view
                .text(&candidate, "lunco:telemetry:port")
                .is_some_and(|port| port == output)
            && view
                .rel_target(&candidate, "lunco:telemetry:target")
                .is_some_and(|target| target == member)
    })
}

/// Build the runtime address map that reconnects one generated solver to the
/// composed USD ownership tree.
///
/// A generated network intentionally has one `ModelicaModel` entity.  Its
/// solver variables therefore cannot be grouped by ECS parentage: that parent
/// is the network root, not the battery, motor, or panel that owns a value.
/// The composed network already contains the authoritative mapping in two
/// forms: boundary output connections and generated unit/member instance
/// names.  Materialize those facts once at projection time so every telemetry
/// consumer sees the same USD structure without parsing generated names in a
/// UI or retaining a second solver.
fn generated_signal_layout(
    view: &dyn ComposedReader,
    root_path: &SdfPath,
    root: &str,
    outputs: &BTreeSet<String>,
    members: &[(String, String, String)],
    member_output_aliases: &[(String, String, String)],
    units: &[SynthesisUnit],
    classes: &MemberClasses,
) -> Result<ModelicaSignalLayout, String> {
    let mut layout = ModelicaSignalLayout {
        root_path: root.to_string(),
        ..default()
    };
    let mut public_member_outputs = BTreeMap::new();

    // Public network outputs retain the authored connection's physical owner.
    for output in outputs {
        let attr = format!("outputs:{output}");
        let connections = view.connections(root_path, &attr);
        let Some(target) = connections.first() else {
            continue;
        };
        let Some((target_prim, target_output)) = target.rsplit_once(".outputs:") else {
            continue;
        };
        public_member_outputs.insert(
            format!("{target_prim}.outputs:{target_output}"),
            output.clone(),
        );
        layout
            .exact_paths
            .insert(output.clone(), target_prim.to_string());
        // If the member already authored the operator-facing channel, that
        // declaration owns the public identity and this wrapper boundary is
        // only its generated implementation address.  Keep it retained, but
        // classify it internal so the same physical value is not presented
        // twice in the canonical catalog.
        if !has_authored_telemetry_for_output(view, target_prim, target_output) {
            layout.public_exact_paths.insert(output.clone());
        }
        if let Some((_, asset, class)) = members.iter().find(|(path, _, _)| path == target_prim) {
            layout.exact_provenance.insert(
                output.clone(),
                ModelicaSignalProvenance {
                    source_asset: Some(asset.clone()),
                    model_class: Some(class.clone()),
                    model_variable: Some(target_output.to_string()),
                    canonical_name: Some(output.clone()),
                },
            );
            if let Some(metadata) = classes.output_metadata(asset, target_output) {
                layout.metadata.insert(output.clone(), metadata.clone());
            }
        }
    }

    // Boundary inputs are also solver variables.  Their authored source is
    // the ownership fact for the command/environment value, so use it instead
    // of leaving every generated input under the implementation scope.
    for attr in view.attr_names(root_path) {
        let Some(name) = attr
            .strip_prefix("inputs:")
            .map(|name| name.strip_suffix(".connect").unwrap_or(name))
        else {
            continue;
        };
        let connections = view.connections(root_path, &attr);
        let Some(target) = connections.first() else {
            continue;
        };
        let Some((target_prim, _)) = target.rsplit_once(".outputs:") else {
            continue;
        };
        layout
            .exact_paths
            .entry(name.to_string())
            .or_insert_with(|| target_prim.to_string());
    }

    for (member, output, alias) in member_output_aliases {
        layout.exact_paths.insert(alias.clone(), member.clone());
        if let Some((_, asset, class)) = members.iter().find(|(path, _, _)| path == member) {
            let canonical_name = public_member_outputs
                .get(&format!("{member}.outputs:{output}"))
                .cloned()
                .unwrap_or_else(|| alias.clone());
            layout.exact_provenance.insert(
                alias.clone(),
                ModelicaSignalProvenance {
                    source_asset: Some(asset.clone()),
                    model_class: Some(class.clone()),
                    model_variable: Some(output.clone()),
                    canonical_name: Some(canonical_name),
                },
            );
            if let Some(metadata) = classes.output_metadata(asset, output) {
                layout.metadata.insert(alias.clone(), metadata.clone());
            }
        }
        // A generated member alias is the canonical public projection for an
        // authored component output unless the network already exposes that
        // same USD port under a public boundary name. This is topology-derived
        // and therefore applies equally to motors, batteries, panels, and
        // future Modelica facets without a component-name classifier.
        if !public_member_outputs.contains_key(&format!("{member}.outputs:{output}"))
            && !has_authored_telemetry_for_output(view, member, output)
        {
            layout.public_exact_paths.insert(alias.clone());
        }
    }

    // The synthesizer emits every component under its policy-selected unit
    // instance. A longest-prefix lookup assigns all public and internal
    // variables of that member—including variables introduced by a later
    // Modelica revision—to the authored member without an output annotation.
    for unit in units {
        let unit_prefix = unit.instance.clone();
        for (output, owner) in layout.exact_paths.clone() {
            let qualified = format!("{unit_prefix}.{output}");
            layout.exact_paths.insert(qualified.clone(), owner);
            // The unit instance is a generated implementation boundary, not a
            // new physical value. Preserve the public classification of the
            // authored boundary/member alias when copying it into the unit;
            // otherwise the runtime retains the value but the operator tree
            // hides the only representation of an unpromoted member output.
            if layout.public_exact_paths.contains(&output) {
                layout.public_exact_paths.insert(qualified);
            }
        }
        for (variable, identity) in layout.exact_provenance.clone() {
            layout
                .exact_provenance
                .insert(format!("{unit_prefix}.{variable}"), identity);
        }
        for (member, _, alias) in member_output_aliases
            .iter()
            .filter(|(member, _, _)| unit.component_paths.iter().any(|path| path == member))
        {
            layout
                .exact_paths
                .insert(format!("{unit_prefix}.{alias}"), member.clone());
        }
        for member in &unit.component_paths {
            let member_prefix = instance_identifier(root, member)?;
            let prefix = format!("{unit_prefix}.{member_prefix}.");
            layout.prefixes.push((prefix.clone(), member.clone()));
            if let Some((_, asset, class)) = members.iter().find(|(path, _, _)| path == member) {
                if let Some(metadata) = classes.variable_metadata(asset) {
                    for (variable, metadata) in metadata {
                        layout
                            .metadata
                            .entry(format!("{prefix}{variable}"))
                            .or_insert_with(|| metadata.clone());
                    }
                }
                layout.provenance_prefixes.push((
                    prefix,
                    ModelicaSignalProvenance {
                        source_asset: Some(asset.clone()),
                        model_class: Some(class.clone()),
                        ..default()
                    },
                ));
            }
        }
    }
    // Deterministic ordering keeps the component inspectable and makes the
    // metadata stable for tests and API snapshots. Resolution itself chooses
    // the longest prefix, so nested instance names remain unambiguous.
    layout
        .prefixes
        .sort_by(|(left, _), (right, _)| right.len().cmp(&left.len()).then(left.cmp(right)));
    layout
        .provenance_prefixes
        .sort_by(|(left, _), (right, _)| right.len().cmp(&left.len()).then(left.cmp(right)));

    // A generated member alias may be emitted in more than one synthesized
    // unit only if the authored topology is invalid. The projection validator
    // owns that error; this map remains a direct data projection and never
    // invents another owner.
    debug_assert!(members.iter().all(|(member, _, _)| {
        units
            .iter()
            .any(|unit| unit.component_paths.contains(member))
    }));
    Ok(layout)
}

/// What class a member's source asset actually declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberClass {
    /// Read from the file: `within` + the class it declares.
    Declared(String),
    /// The source settled without a usable Modelica class. This is a terminal
    /// authoring error; the projector does not substitute another class.
    Invalid(String),
}

/// The class each member source declares — the ONE authority on what a generated
/// model may instantiate.
///
/// The emitter has to name a class while it is reading the stage, where the
/// `.mo` may be an unfetched HTTP resource, so the name used to be DERIVED from
/// the asset path (`models/LunCo/Electrical/Battery.mo` → `LunCo.Electrical.Battery`).
/// That assumes the directory layout mirrors the package, which is true of the
/// shipped library and silently false the moment a directory is renamed or a
/// file's `within` says otherwise — and the symptom is "class not found" from
/// the compiler, against generated source, naming neither the prim nor the file.
///
/// So the file is loaded and read, and a network whose members are not all
/// resolved yet simply does not project until they are. Keyed by asset path, so
/// one file is fetched and parsed once per session however many networks
/// instantiate it.
#[derive(Resource, Clone, Default)]
pub struct MemberClasses {
    known: HashMap<String, MemberClass>,
    outputs: HashMap<String, BTreeSet<String>>,
    metadata: HashMap<String, HashMap<String, ModelicaVariableMetadata>>,
    /// Resident handles let source modification events invalidate the exact
    /// declaration they changed without rescanning every pending source.
    handles: HashMap<String, Handle<ModelicaSource>>,
    pending: HashMap<String, Handle<ModelicaSource>>,
}

impl MemberClasses {
    /// State a verdict directly, bypassing the loader.
    pub fn declare(&mut self, asset: impl Into<String>, class: impl Into<String>) {
        self.known
            .insert(asset.into(), MemberClass::Declared(class.into()));
    }

    /// State a terminal source-resolution error. Runtime code uses the same
    /// state after an asset load fails or its declaration cannot be parsed;
    /// tests and offline tools can seed that authoritative verdict directly.
    pub fn reject(&mut self, asset: impl Into<String>, message: impl Into<String>) {
        self.known
            .insert(asset.into(), MemberClass::Invalid(message.into()));
    }

    /// Causal outputs declared by the resolved Modelica class. `None` means
    /// the class was seeded by an offline test/tool without source interface
    /// data; callers then retain their authored contract and let compilation
    /// be the authority.
    pub fn output_names(&self, asset: &str) -> Option<&BTreeSet<String>> {
        self.outputs.get(asset)
    }

    /// Units and descriptions for the output declarations that the generated
    /// wrapper promotes from this member.  The metadata is read from the same
    /// Modelica source as the class and output names; it is not reconstructed
    /// from generated solver identifiers or component names.
    pub fn output_metadata(&self, asset: &str, output: &str) -> Option<&ModelicaVariableMetadata> {
        self.metadata
            .get(asset)
            .and_then(|metadata| metadata.get(output))
    }

    /// Units and descriptions for every declared variable in a member source.
    /// Generated solver members use this map for internal inspection rows as
    /// well as promoted outputs, so the browser and API do not lose authored
    /// metadata at the generated-document boundary.
    pub fn variable_metadata(
        &self,
        asset: &str,
    ) -> Option<&HashMap<String, ModelicaVariableMetadata>> {
        self.metadata.get(asset)
    }

    /// Resolve the class to instantiate for `asset`. `Ok(None)` means the source
    /// is still loading; `Err` is a terminal source error.
    pub fn resolve(&self, asset: &str) -> Result<Option<String>, String> {
        match self.known.get(asset) {
            Some(MemberClass::Declared(class)) => Ok(Some(class.clone())),
            Some(MemberClass::Invalid(message)) => Err(message.clone()),
            None => Ok(None),
        }
    }
}

/// Resolve every member source's DECLARED class before synthesis.
///
/// Scans the stage for component collections, loads each member's
/// `info:sourceAsset` once, and reads `within` + the class the file declares.
/// Until a member has a verdict its network does not project at all
/// ([`SynthOutcome::Pending`]) — synthesizing before the source settles would
/// produce a generated model with an unknown member class and an unattributed
/// compiler failure.
///
/// A source that fails to load or does not expose a class settles as
/// [`MemberClass::Invalid`]. The projection reports that terminal source error
/// and does not compile an incomplete model. Completion and failure are driven
/// by the Modelica asset events; there is no time-based give-up path.
pub fn resolve_member_classes(
    prims: Query<(Entity, &UsdPrimPath, Option<&UsdInstanceProjection>)>,
    preview: (
        Query<&ChildOf>,
        Query<(), With<lunco_usd_bevy_scene::UsdPreviewOnly>>,
    ),
    mut classes: ResMut<MemberClasses>,
    mut class_users: ResMut<DomainClassUsers>,
    mut candidates: ResMut<PendingDomainProjectionCandidates>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    asset_server: Res<AssetServer>,
    sources: Res<Assets<ModelicaSource>>,
    mut source_events: MessageReader<AssetEvent<ModelicaSource>>,
    mut source_failures: MessageReader<bevy::asset::AssetLoadFailedEvent<ModelicaSource>>,
) {
    let mut loaded = HashSet::new();
    let mut modified = HashSet::new();
    for event in source_events.read() {
        match event {
            AssetEvent::Added { id } | AssetEvent::LoadedWithDependencies { id } => {
                loaded.insert(*id);
            }
            AssetEvent::Modified { id } => {
                modified.insert(*id);
            }
            _ => {}
        }
    }
    let failed: HashMap<AssetId<ModelicaSource>, String> = source_failures
        .read()
        .map(|event| (event.id, event.error.to_string()))
        .collect();
    // `UsdWiringDirty` also covers endpoint additions/removals, which already
    // arrive through `candidates.discovery`. Only authored canonical-stage
    // generations and USD asset changes can invalidate composed member
    // membership for every network root.
    let canonical_stage_changed = candidates.observe_canonical_stage_generations(&canonical);
    let full_discovery =
        candidates.initial_discovery || canonical_stage_changed || stages.is_changed();
    let discover = full_discovery || !candidates.discovery.is_empty();
    if !discover && loaded.is_empty() && modified.is_empty() && failed.is_empty() {
        return;
    }

    // Discovery runs on the same triggers as the projector — plus never at all
    // once every member is known, which is the steady state.
    let mut discovered = HashSet::new();
    let modified_assets: Vec<_> = classes
        .handles
        .iter()
        .filter(|(_, handle)| modified.contains(&handle.id()))
        .map(|(asset, handle)| (asset.clone(), handle.clone()))
        .collect();
    for (asset, handle) in modified_assets {
        classes.known.remove(&asset);
        classes.outputs.remove(&asset);
        classes.metadata.remove(&asset);
        classes.pending.insert(asset, handle);
    }
    if discover {
        let discovery_entities: Vec<_> = if full_discovery {
            class_users.clear();
            candidates.discovery.clear();
            candidates.initial_discovery = false;
            prims.iter().collect()
        } else {
            let mut entities: Vec<_> = candidates.discovery.drain().collect();
            entities.sort_unstable();
            entities
                .into_iter()
                .filter_map(|entity| prims.get(entity).ok())
                .collect()
        };
        for (entity, prim, instance_projection) in discovery_entities {
            if lunco_usd_bevy_scene::is_preview_only(entity, &preview.0, &preview.1) {
                class_users.remove_root(entity);
                continue;
            }
            let id = prim.stage_handle.id();
            let Some(stage_asset) = stages.get(&prim.stage_handle) else {
                continue;
            };
            let (reader, _generation) =
                canonical.reader_for_entity(id, stage_asset, instance_projection);
            let view: &dyn ComposedReader = &reader;
            let Ok(root) = SdfPath::new(&prim.path) else {
                continue;
            };
            if !is_runtime_domain_network_root(view, &root) {
                class_users.remove_root(entity);
                continue;
            }
            let Ok(members) = view.collection_members(&root, "components") else {
                class_users.remove_root(entity);
                continue;
            };
            let mut source_assets = HashSet::new();
            for member in members {
                if !view.has_api_schema(&member, "LunCoProgramAPI") {
                    continue;
                }
                let source_ref = match modelica_source_ref(view, &member) {
                    Ok(source_ref) => source_ref,
                    Err(issue) => {
                        warn!(
                            "[domain-projection] member {} has unresolved Modelica source at {}: {}",
                            member, issue.property, issue.message
                        );
                        continue;
                    }
                };
                let asset = source_ref.asset;
                source_assets.insert(asset.clone());
                if classes.known.contains_key(&asset) || classes.pending.contains_key(&asset) {
                    continue;
                }
                let handle: Handle<ModelicaSource> = asset_server.load(asset.clone());
                discovered.insert(handle.id());
                classes.handles.insert(asset.clone(), handle.clone());
                classes.pending.insert(asset, handle);
            }
            class_users.replace_root_assets(entity, source_assets);
            candidates.projection.insert(entity);
        }
    }

    if classes.pending.is_empty() {
        return;
    }
    let settled: Vec<(
        String,
        Result<
            (
                String,
                BTreeSet<String>,
                HashMap<String, ModelicaVariableMetadata>,
            ),
            String,
        >,
    )> = classes
        .pending
        .iter()
        .filter_map(|(asset, handle)| {
            let id = handle.id();
            if !discovered.contains(&id)
                && !loaded.contains(&id)
                && !modified.contains(&id)
                && !failed.contains_key(&id)
            {
                return None;
            }
            if let Some(source) = sources.get(handle) {
                let interface = &source.interface;
                let Some(declared) = interface.model_name.as_ref().cloned() else {
                    return Some((
                        asset.clone(),
                        Err("the Modelica source did not expose a declared class".into()),
                    ));
                };
                let class = match interface.within.as_deref() {
                    Some(within) => format!("{within}.{declared}"),
                    None => declared,
                };
                return Some((
                    asset.clone(),
                    Ok((
                        class,
                        interface.outputs.clone(),
                        interface.variable_metadata.clone(),
                    )),
                ));
            }
            failed.get(&id).map(|error| {
                (
                    asset.clone(),
                    Err(format!("failed to load Modelica source asset: {error}")),
                )
            })
        })
        .collect();
    for (asset, result) in settled {
        classes.pending.remove(&asset);
        match result {
            Ok((class, outputs, metadata)) => {
                classes.outputs.insert(asset.clone(), outputs);
                classes.metadata.insert(asset.clone(), metadata);
                classes
                    .known
                    .insert(asset.clone(), MemberClass::Declared(class));
            }
            Err(error) => {
                warn!("[domain-projection] {asset}: {error}");
                classes
                    .known
                    .insert(asset.clone(), MemberClass::Invalid(error));
            }
        }
        if let Some(roots) = class_users.roots_by_asset.get(&asset) {
            candidates.projection.extend(roots.iter().copied());
        }
    }
    // A network's declared member classes are a single synthesis input. Keep
    // its root queued until every referenced source has a terminal verdict so
    // the projector runs once with a complete class set, not once per asset
    // arrival while the rest of the network is still pending.
    candidates
        .projection
        .retain(|root| class_users.root_sources_settled(*root, &classes));
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy_stage::canonical::CanonicalStage;

    #[test]
    fn source_class_users_invalidate_only_dependent_roots() {
        let mut users = DomainClassUsers::default();
        let motor = Entity::from_bits(1);
        let battery = Entity::from_bits(2);
        users.replace_root_assets(
            motor,
            HashSet::from(["motor.mo".to_string(), "battery.mo".to_string()]),
        );
        users.replace_root_assets(battery, HashSet::from(["battery.mo".to_string()]));

        assert_eq!(users.roots_by_asset["motor.mo"], HashSet::from([motor]));
        assert_eq!(
            users.roots_by_asset["battery.mo"],
            HashSet::from([motor, battery])
        );

        users.replace_root_assets(motor, HashSet::from(["motor.mo".to_string()]));
        assert_eq!(users.roots_by_asset["battery.mo"], HashSet::from([battery]));

        users.remove_root(motor);
        assert!(!users.roots_by_asset.contains_key("motor.mo"));
        assert_eq!(users.roots_by_asset["battery.mo"], HashSet::from([battery]));
    }

    #[test]
    fn domain_projection_waits_for_every_member_class_verdict() {
        let root = Entity::from_bits(3);
        let mut users = DomainClassUsers::default();
        users.replace_root_assets(
            root,
            HashSet::from(["motor.mo".to_string(), "battery.mo".to_string()]),
        );
        let mut classes = MemberClasses::default();

        assert!(!users.root_sources_settled(root, &classes));
        classes.declare("motor.mo", "LunCo.Electrical.Motor");
        assert!(!users.root_sources_settled(root, &classes));
        classes.reject("battery.mo", "source asset did not declare a class");
        assert!(users.root_sources_settled(root, &classes));
    }

    #[test]
    fn full_domain_discovery_tracks_canonical_stage_generations() {
        let asset = AssetId::<UsdStageAsset>::default();
        let recipe = lunco_usd_compose::recipe::StageRecipe::from_source(
            "domain.usda",
            "#usda 1.0\ndef Xform \"Root\" {}\n",
        );
        let mut stages = CanonicalStages::default();
        stages.insert(
            asset,
            CanonicalStage::from_recipe(&recipe).expect("canonical stage builds"),
        );
        let mut candidates = PendingDomainProjectionCandidates::default();

        assert!(candidates.observe_canonical_stage_generations(&stages));
        assert!(!candidates.observe_canonical_stage_generations(&stages));

        assert!(stages.rebuild(asset, &recipe));
        assert!(candidates.observe_canonical_stage_generations(&stages));
        assert!(!candidates.observe_canonical_stage_generations(&stages));
    }

    #[test]
    fn scene_teardown_resets_domain_projection_work_but_not_asset_class_facts() {
        let mut app = App::new();
        let root = Entity::from_bits(4);
        let mut users = DomainClassUsers::default();
        users.replace_root_assets(root, HashSet::from(["motor.mo".to_string()]));
        let mut candidates = PendingDomainProjectionCandidates::default();
        candidates.initial_discovery = false;
        candidates.discovery.insert(root);
        candidates.projection.insert(root);
        let mut classes = MemberClasses::default();
        classes.declare("motor.mo", "LunCo.Electrical.Motor");

        app.insert_resource(users)
            .insert_resource(candidates)
            .insert_resource(PendingGeneratedSourceDocuments::default())
            .insert_resource(classes)
            .add_systems(Update, reset_scene_projection_work);
        app.update();

        let users = app.world().resource::<DomainClassUsers>();
        assert!(users.roots_by_asset.is_empty());
        assert!(users.assets_by_root.is_empty());
        let candidates = app.world().resource::<PendingDomainProjectionCandidates>();
        assert!(candidates.discovery.is_empty());
        assert!(candidates.projection.is_empty());
        assert!(candidates.initial_discovery);
        assert!(app
            .world()
            .resource::<PendingGeneratedSourceDocuments>()
            .0
            .has_work());
        assert_eq!(
            app.world().resource::<MemberClasses>().resolve("motor.mo"),
            Ok(Some("LunCo.Electrical.Motor".into()))
        );
    }

    #[test]
    fn domain_discovery_observers_coalesce_path_and_identity_arrivals() {
        let mut app = App::new();
        app.insert_resource(PendingDomainProjectionCandidates {
            discovery: HashSet::new(),
            projection: HashSet::new(),
            initial_discovery: false,
            observed_stage_generations: HashMap::new(),
        })
        .add_observer(queue_added_domain_prim)
        .add_observer(queue_added_domain_identity)
        .add_observer(queue_removed_domain_identity);

        let entity = app
            .world_mut()
            .spawn(UsdPrimPath {
                stage_handle: Handle::default(),
                path: "/Scene/Body".into(),
            })
            .id();
        app.world_mut()
            .entity_mut(entity)
            .insert(lunco_core::GlobalEntityId::from_raw(12));
        app.world_mut()
            .spawn(lunco_core::GlobalEntityId::from_raw(13));

        let pending = app.world().resource::<PendingDomainProjectionCandidates>();
        assert_eq!(pending.discovery.len(), 1);
        assert!(pending.discovery.contains(&entity));
        assert!(pending.projection.is_empty());

        app.world_mut()
            .resource_mut::<PendingDomainProjectionCandidates>()
            .discovery
            .clear();
        app.world_mut()
            .entity_mut(entity)
            .remove::<lunco_core::GlobalEntityId>();
        assert!(app
            .world()
            .resource::<PendingDomainProjectionCandidates>()
            .discovery
            .contains(&entity));
    }

    #[test]
    fn generated_document_sync_queues_source_and_model_lifecycle() {
        fn source() -> GeneratedModelicaSource {
            GeneratedModelicaSource {
                network_root: "/Rig".into(),
                doc_uri: "generated://Rig.mo".into(),
                source: "model Rig end Rig;".into(),
                component_paths: Vec::new(),
                members: Vec::new(),
                source_roots: Vec::new(),
                member_output_aliases: Vec::new(),
                units: Vec::new(),
                boundary_inputs: Vec::new(),
                boundary_outputs: Vec::new(),
                layout: SynthesisLayout::default(),
                projection_error: None,
            }
        }

        let mut app = App::new();
        app.init_resource::<PendingGeneratedSourceDocuments>()
            .add_observer(queue_generated_source_document_sync)
            .add_observer(queue_model_document_sync_for_generated_source)
            .add_observer(forget_generated_source_document_sync);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(entity)
            .insert(ModelicaModel::default());
        assert!(!app
            .world()
            .resource::<PendingGeneratedSourceDocuments>()
            .0
            .contains(entity));

        app.world_mut().entity_mut(entity).insert(source());
        assert!(app
            .world()
            .resource::<PendingGeneratedSourceDocuments>()
            .0
            .contains(entity));

        app.world_mut()
            .entity_mut(entity)
            .remove::<GeneratedModelicaSource>();
        assert!(!app
            .world()
            .resource::<PendingGeneratedSourceDocuments>()
            .0
            .contains(entity));

        app.world_mut().entity_mut(entity).insert(source());
        app.world_mut().entity_mut(entity).remove::<ModelicaModel>();
        app.world_mut()
            .entity_mut(entity)
            .insert(ModelicaModel::default());
        assert!(app
            .world()
            .resource::<PendingGeneratedSourceDocuments>()
            .0
            .contains(entity));
    }

    fn component(path: &str, target: Option<&str>) -> DomainComponent {
        DomainComponent {
            path: path.into(),
            source_asset: "lunco://models/LunCo/Electrical/DCMotor.mo".into(),
            model_class: "LunCo.Electrical.DCMotor".into(),
            constants: BTreeMap::from([("rated_power".into(), 2000.0)]),
            connectors: target
                .map(|target| BTreeMap::from([("p".into(), vec![target.into()])]))
                .unwrap_or_default(),
            declared_connectors: BTreeSet::from(["p".into()]),
            inputs: BTreeMap::new(),
            declared_outputs: BTreeSet::new(),
            topology_role: "neutral".into(),
        }
    }

    #[test]
    fn synthesizer_partitions_disconnected_graph_into_composite_units() {
        let mut left = component("/Thermal/Left/Mass", None);
        left.inputs
            .insert("heat_w".into(), "/Thermal.inputs:left_heat".into());
        let mut right = component("/Thermal/Right/Mass", None);
        right
            .inputs
            .insert("heat_w".into(), "/Thermal.inputs:right_heat".into());
        let network = DomainNetwork {
            root: "/Thermal".into(),
            components: vec![right, left],
            inputs: BTreeSet::from(["left_heat".into(), "right_heat".into()]),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::from([
                (
                    "left_temp".into(),
                    "/Thermal/Left/Mass.outputs:temp_k".into(),
                ),
                (
                    "right_temp".into(),
                    "/Thermal/Right/Mass.outputs:temp_k".into(),
                ),
            ]),
            communication_period_secs: lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: false,
        };

        let units = synthesis::partition_network(&network);
        assert_eq!(units.len(), 2);
        assert_eq!(
            units
                .iter()
                .map(|unit| unit.component_paths.clone())
                .collect::<Vec<_>>(),
            vec![
                vec!["/Thermal/Left/Mass".to_string()],
                vec!["/Thermal/Right/Mass".to_string()],
            ]
        );
        assert_eq!(units[0].inputs, BTreeSet::from(["left_heat".into()]));
        assert_eq!(units[1].outputs, BTreeSet::from(["right_temp".into()]));
    }

    #[test]
    fn member_layout_coordinates_are_scoped_to_their_owning_unit() {
        let network = DomainNetwork {
            root: "/Rig".into(),
            components: vec![
                component("/Rig/Source_A", Some("/Rig/Load_A")),
                component("/Rig/Load_A", Some("/Rig/Source_A")),
                component("/Rig/Source_B", Some("/Rig/Load_B")),
                component("/Rig/Load_B", Some("/Rig/Source_B")),
            ],
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: false,
        };
        let units = vec![
            SynthesisUnit {
                name: "NetworkUnit_1".into(),
                instance: "network_unit_1".into(),
                component_paths: vec!["/Rig/Source_A".into(), "/Rig/Load_A".into()],
                ..Default::default()
            },
            SynthesisUnit {
                name: "NetworkUnit_2".into(),
                instance: "network_unit_2".into(),
                component_paths: vec!["/Rig/Source_B".into(), "/Rig/Load_B".into()],
                ..Default::default()
            },
        ];
        let layout = lunco_hooks::HookValue::map([
            (
                "units",
                lunco_hooks::HookValue::Array(vec![
                    lunco_hooks::HookValue::map([
                        ("name", lunco_hooks::HookValue::str("NetworkUnit_1")),
                        ("x", lunco_hooks::HookValue::Int(-200)),
                        ("y", lunco_hooks::HookValue::Int(0)),
                    ]),
                    lunco_hooks::HookValue::map([
                        ("name", lunco_hooks::HookValue::str("NetworkUnit_2")),
                        ("x", lunco_hooks::HookValue::Int(200)),
                        ("y", lunco_hooks::HookValue::Int(0)),
                    ]),
                ]),
            ),
            (
                "members",
                lunco_hooks::HookValue::Array(vec![
                    lunco_hooks::HookValue::map([
                        ("path", lunco_hooks::HookValue::str("/Rig/Source_A")),
                        ("x", lunco_hooks::HookValue::Int(-170)),
                        ("y", lunco_hooks::HookValue::Int(0)),
                    ]),
                    lunco_hooks::HookValue::map([
                        ("path", lunco_hooks::HookValue::str("/Rig/Load_A")),
                        ("x", lunco_hooks::HookValue::Int(0)),
                        ("y", lunco_hooks::HookValue::Int(80)),
                    ]),
                    lunco_hooks::HookValue::map([
                        ("path", lunco_hooks::HookValue::str("/Rig/Source_B")),
                        ("x", lunco_hooks::HookValue::Int(-170)),
                        ("y", lunco_hooks::HookValue::Int(0)),
                    ]),
                    lunco_hooks::HookValue::map([
                        ("path", lunco_hooks::HookValue::str("/Rig/Load_B")),
                        ("x", lunco_hooks::HookValue::Int(0)),
                        ("y", lunco_hooks::HookValue::Int(80)),
                    ]),
                ]),
            ),
        ]);

        let parsed =
            synthesis::parse_policy_layout(Some(&layout), &network, &units, "/Rig", "local-layout")
                .expect("independent unit diagrams may reuse local coordinates");
        assert_eq!(parsed.member_positions["/Rig/Source_A"], (-170, 0));
        assert_eq!(parsed.member_positions["/Rig/Source_B"], (-170, 0));
    }

    #[test]
    fn generated_member_instance_names_are_readable_without_a_collision_fallback() {
        assert_eq!(
            instance_identifier(
                "/SolarRoverTest/SolarRover",
                "/SolarRoverTest/SolarRover/Motor_FL"
            )
            .unwrap(),
            "Motor_FL"
        );
        assert_eq!(
            instance_identifier("/Rig", "/Rig/Battery").unwrap(),
            "Battery"
        );
        assert_ne!(
            instance_identifier("/Rig", "/Rig/Motor-A").unwrap(),
            instance_identifier("/Rig", "/Rig/Motor_A").unwrap()
        );
    }

    #[test]
    fn member_path_without_a_name_is_reported_instead_of_panicking() {
        let error = instance_identifier("/Rig", "/Rig")
            .expect_err("the network root is not a member instance");
        assert!(error.contains("has no name"), "{error}");
    }

    #[test]
    fn generated_signal_layout_keeps_member_class_and_canonical_output_identity() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/electrical_network.usda");
        let composed =
            lunco_usd_bevy_stage::compose::compose_file_to_stage(&path).expect("compose fixture");
        let stage = CanonicalStage::from_stage(composed, path.to_string_lossy().to_string());
        let view = stage.view();
        let root_path = SdfPath::new("/Rig").unwrap();
        let mut classes = MemberClasses::default();
        classes.declare(
            "lunco://models/LunCo/Electrical/Battery.mo",
            "LunCo.Electrical.Battery",
        );
        classes.metadata.insert(
            "lunco://models/LunCo/Electrical/Battery.mo".into(),
            HashMap::from([(
                "terminal_voltage_v".into(),
                ModelicaVariableMetadata {
                    description: Some("Battery terminal voltage on the electrical bus".into()),
                    unit: Some("V".into()),
                },
            )]),
        );
        classes.declare(
            "lunco://models/LunCo/Electrical/DCMotor.mo",
            "LunCo.Electrical.DCMotor",
        );
        classes.declare(
            "lunco://models/LunCo/Electrical/SolarPanel.mo",
            "LunCo.Electrical.SolarPanel",
        );
        lunco_hooks_rhai::register_rhai_hook(
            "synth.acausal-network",
            "synthesize",
            lunco_assets_runtime::scripting::active_policy_set()
                .expect("shipped synthesis policies")
                .into_iter()
                .find(|policy| policy.spec.source == "synth_acausal_network.rhai")
                .expect("synthesis policy source exists")
                .source
                .as_str(),
            true,
        )
        .expect("shipped synthesis policy compiles");
        let synthesizer = SynthesizerRegistry::default()
            .get(DEFAULT_DOMAIN_SYNTHESIZER)
            .expect("default synthesizer is policy-backed")
            .clone();
        let SynthOutcome::Ready(plan) = synthesizer
            .synthesize(
                &view,
                &root_path,
                "Rig_System",
                &SynthContext { classes: &classes },
            )
            .expect("fixture synthesis")
        else {
            panic!("fixture must be a ready acausal network");
        };
        let aliases = plan.member_output_aliases.clone();
        let layout = generated_signal_layout(
            &view,
            &root_path,
            "/Rig",
            &plan.outputs,
            &plan.members,
            &aliases,
            &plan.units,
            &classes,
        )
        .expect("validated generated member paths");

        let soc = layout.provenance("soc").expect("boundary output identity");
        assert_eq!(soc.model_class.as_deref(), Some("LunCo.Electrical.Battery"));
        assert_eq!(soc.model_variable.as_deref(), Some("soc_out"));
        assert_eq!(soc.canonical_name.as_deref(), Some("soc"));
        assert_eq!(
            soc.source_asset.as_deref(),
            Some("lunco://models/LunCo/Electrical/Battery.mo")
        );

        let battery_unit = plan
            .units
            .iter()
            .find(|unit| {
                unit.component_paths
                    .iter()
                    .any(|path| path == "/Rig/Battery")
            })
            .expect("battery synthesis unit");
        let solver_name = format!(
            "{}.{}.soc_out",
            battery_unit.instance,
            instance_identifier("/Rig", "/Rig/Battery").unwrap(),
        );
        let internal = layout
            .provenance(&solver_name)
            .expect("member solver identity");
        assert_eq!(
            internal.model_class.as_deref(),
            Some("LunCo.Electrical.Battery")
        );
        assert_eq!(internal.model_variable.as_deref(), Some("soc_out"));

        let internal_terminal = format!(
            "{}.{}.terminal_voltage_v",
            battery_unit.instance,
            instance_identifier("/Rig", "/Rig/Battery").unwrap(),
        );
        let terminal_metadata = layout
            .metadata
            .get(&internal_terminal)
            .expect("member metadata follows the generated solver prefix");
        assert_eq!(terminal_metadata.unit.as_deref(), Some("V"));
        assert_eq!(
            terminal_metadata.description.as_deref(),
            Some("Battery terminal voltage on the electrical bus")
        );

        let motor_alias = aliases
            .iter()
            .find(|(member, output, _)| member == "/Rig/Motor" && output == "electrical_power")
            .map(|(_, _, alias)| alias)
            .expect("the unpromoted motor output is in the generated interface");
        assert_eq!(
            layout.exposure(motor_alias),
            lunco_signal::SignalExposure::Public
        );
        let motor_unit = plan
            .units
            .iter()
            .find(|unit| unit.component_paths.iter().any(|path| path == "/Rig/Motor"))
            .expect("motor synthesis unit");
        let motor_solver_name = format!("{}.{}", motor_unit.instance, motor_alias);
        assert_eq!(
            layout.exposure(&motor_solver_name),
            lunco_signal::SignalExposure::Public,
            "unit-qualified member aliases retain their authored public exposure"
        );
    }

    #[test]
    fn authored_member_telemetry_owns_the_public_output_identity() {
        let stage =
            CanonicalStage::from_recipe(&lunco_usd_compose::recipe::StageRecipe::from_source(
                "telemetry-owner.usda",
                r#"#usda 1.0
def Scope "Rig"
{
    def Xform "Battery"
    {
        bool lunco:telemetry = true
        token lunco:telemetry:port = "soc_out"
        token outputs:soc_out
    }
}
"#,
            ))
            .expect("telemetry owner stage");
        let view = stage.view();
        assert!(has_authored_telemetry_for_output(
            &view,
            "/Rig/Battery",
            "soc_out"
        ));
        assert!(!has_authored_telemetry_for_output(
            &view,
            "/Rig/Battery",
            "terminal_voltage_v"
        ));
    }

    #[test]
    fn rejects_external_connector_targets_and_keeps_unit_partition_deterministic() {
        let mut external = component("/Rig/Load/Model", None);
        external
            .connectors
            .insert("p".into(), vec!["/Other/Battery/Model.connectors:p".into()]);
        let network = DomainNetwork {
            root: "/Rig".into(),
            components: vec![component("/Rig/Battery/Model", None), external],
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: false,
        };
        let errors = network::validate_network(&network);
        assert!(errors
            .iter()
            .any(|error| error.message.contains("outside collection")));
        assert_eq!(synthesis::partition_network(&network).len(), 2);
    }

    #[test]
    fn unconnected_acausal_component_is_omitted_from_generated_network() {
        let panel = component("/Rig/SolarPanel", None);
        let mut battery = component("/Rig/Battery", None);
        battery
            .connectors
            .insert("p".into(), vec!["/Rig/Motor.connectors:p".into()]);
        let motor = component("/Rig/Motor", None);
        let mut components = vec![panel, battery, motor];
        let omitted = network::retain_connected_acausal_components(&mut components);
        assert_eq!(
            components
                .iter()
                .map(|component| component.path.as_str())
                .collect::<Vec<_>>(),
            ["/Rig/Battery", "/Rig/Motor"],
            "only explicitly wired program facets enter a generated acausal island"
        );
        assert!(
            omitted.contains("/Rig/SolarPanel"),
            "what the island omits has to be nameable — a boundary output published \
             through an omitted part drops with it instead of rejecting the network"
        );
    }

    #[test]
    fn generated_model_identity_is_qualified_by_network_path() {
        assert_ne!(
            network_model_name("/Rover", Some(10)),
            network_model_name("/Payload", Some(20))
        );
        assert_eq!(network_model_name("/Rover", Some(42)), "Rover_G42_System");
        assert_ne!(
            network_model_name("/Rover", Some(10)),
            network_model_name("/Rover", Some(20))
        );
    }

    #[test]
    fn projection_fingerprint_changes_only_with_generated_source() {
        let source = "model A\n  Real x;\nend A;\n";
        assert_eq!(source_fingerprint(source), source_fingerprint(source));
        assert_ne!(
            source_fingerprint(source),
            source_fingerprint("model A\n  Real y;\nend A;\n")
        );
    }

    #[test]
    fn aggregates_one_schedule_and_rejects_conflicting_member_periods() {
        assert_eq!(
            network::aggregate_communication_periods(std::iter::empty()).unwrap(),
            lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS
        );
        let six_ticks = 6.0 * lunco_core_runtime::SECS_PER_TICK;
        assert_eq!(
            network::aggregate_communication_periods([
                Ok(("/Rig/A".into(), six_ticks)),
                Ok(("/Rig/B".into(), 0.1)),
            ])
            .unwrap(),
            six_ticks
        );
        let errors = network::aggregate_communication_periods([
            Ok(("/Rig/A".into(), six_ticks)),
            Ok(("/Rig/B".into(), 12.0 * lunco_core_runtime::SECS_PER_TICK)),
        ])
        .unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("mixed member schedules"));
    }

    #[test]
    fn rejects_ambiguous_forwarded_boundary_sources() {
        let network = DomainNetwork {
            root: "/Rig".into(),
            components: vec![component("/Rig/Battery", None)],
            inputs: BTreeSet::from(["left".into(), "right".into()]),
            input_sources: BTreeMap::from([
                ("left".into(), "/Controls.outputs:throttle".into()),
                ("right".into(), "/Controls.outputs:throttle".into()),
            ]),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: false,
        };
        assert!(network::validate_network(&network)
            .iter()
            .any(|error| error.message.contains("boundary identity is ambiguous")));
    }

    #[test]
    fn rejects_modelica_keywords_as_public_members() {
        let mut bad = component("/Rig/Load", None);
        bad.inputs
            .insert("equation".into(), "/Rig.inputs:demand".into());
        let network = DomainNetwork {
            root: "/Rig".into(),
            components: vec![bad],
            inputs: BTreeSet::from(["demand".into()]),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: false,
        };
        assert!(network::validate_network(&network)
            .iter()
            .any(|error| error.message.contains("not a valid Modelica identifier")));
    }

    #[test]
    fn retiring_a_changed_projection_removes_stale_outputs() {
        fn retire_once(mut commands: Commands, q: Query<Entity, With<GeneratedModelicaSource>>) {
            for entity in &q {
                retire_sim_interface(&mut commands, entity);
            }
        }

        let mut app = App::new();
        app.add_systems(Update, retire_once);
        let entity = app
            .world_mut()
            .spawn((
                GeneratedModelicaSource {
                    network_root: "/Rig".into(),
                    doc_uri: "generated://Rig.mo".into(),
                    source: "model Rig end Rig;".into(),
                    component_paths: vec!["/Battery".into()],
                    members: Vec::new(),
                    source_roots: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    layout: SynthesisLayout::default(),
                    projection_error: None,
                },
                lunco_cosim_core::SimComponent {
                    outputs: std::collections::HashMap::from([("soc".into(), 0.75)]),
                    ..default()
                },
            ))
            .id();

        app.update();

        assert!(
            app.world()
                .get::<lunco_cosim_core::SimComponent>(entity)
                .is_none(),
            "a changed or rejected projection must not retain solved values from its previous topology"
        );
    }

    #[test]
    fn removing_generated_source_retires_only_its_ephemeral_document() {
        let mut app = App::new();
        app.init_resource::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()
            .init_resource::<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>()
            .add_observer(on_remove_generated_source);
        let document = app
            .world_mut()
            .resource_mut::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()
            .allocate(
                "model Generated end Generated;".into(),
                lunco_doc::PathlessOrigin::bundled("generated/Generated.mo"),
            );
        let entity = app
            .world_mut()
            .spawn((
                ModelicaModel {
                    document,
                    ..default()
                },
                GeneratedModelicaSource {
                    network_root: "/Rig".into(),
                    doc_uri: "generated://Generated.mo".into(),
                    source: "model Generated end Generated;".into(),
                    component_paths: Vec::new(),
                    members: Vec::new(),
                    source_roots: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    layout: SynthesisLayout::default(),
                    projection_error: None,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()
            .link(entity, document)
            .expect("generated test document link");
        app.world_mut()
            .resource_mut::<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>()
            .entries
            .push(
                lunco_modelica_runtime::generated_source::GeneratedModelicaSourceEntry {
                    document,
                    uri: "generated://Generated.mo".into(),
                    network_root: "/Rig".into(),
                    model_name: "Generated".into(),
                    source: "model Generated end Generated;".into(),
                    component_paths: Vec::new(),
                    units: Vec::new(),
                    members: Vec::new(),
                    source_roots: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    member_output_aliases: Vec::new(),
                    projection_error: None,
                },
            );

        app.world_mut()
            .entity_mut(entity)
            .remove::<GeneratedModelicaSource>();
        app.update();

        assert!(app
            .world()
            .resource::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()
            .host(document)
            .is_none());
        assert!(app
            .world()
            .resource::<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>()
            .entries
            .is_empty());
    }

    #[test]
    fn generated_source_publication_ignores_runtime_model_output_changes() {
        #[derive(Resource, Default)]
        struct PublicationCount(usize);

        fn count_publications(
            mut count: ResMut<PublicationCount>,
            mut generated: ResMut<
                lunco_modelica_runtime::generated_source::GeneratedModelicaSources,
            >,
        ) {
            count.0 += 1;
            generated.dirty = false;
        }

        let mut app = App::new();
        app.init_resource::<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>()
            .init_resource::<PublicationCount>()
            .add_observer(mark_generated_sources_dirty_on_insert)
            .add_systems(
                Update,
                count_publications.run_if(generated_sources_need_publish),
            );
        let entity = app
            .world_mut()
            .spawn((
                GeneratedModelicaSource {
                    network_root: "/Rig".into(),
                    doc_uri: "generated://Rig.mo".into(),
                    source: "model Rig end Rig;".into(),
                    component_paths: Vec::new(),
                    members: Vec::new(),
                    source_roots: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    layout: SynthesisLayout::default(),
                    projection_error: None,
                },
                ModelicaModel::default(),
            ))
            .id();

        app.update();
        assert_eq!(app.world().resource::<PublicationCount>().0, 1);

        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 1.0;
        app.update();
        assert_eq!(
            app.world().resource::<PublicationCount>().0,
            1,
            "solver output changes are not generated-source metadata changes"
        );

        let mut updated_source = app
            .world()
            .get::<GeneratedModelicaSource>(entity)
            .unwrap()
            .clone();
        updated_source.source.push(' ');
        app.world_mut().entity_mut(entity).insert(updated_source);
        app.update();
        assert_eq!(app.world().resource::<PublicationCount>().0, 2);

        app.world_mut()
            .resource_mut::<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>()
            .dirty = true;
        app.update();
        assert_eq!(app.world().resource::<PublicationCount>().0, 3);
    }

    #[test]
    fn default_registry_contains_each_shipped_synthesizer() {
        let registry = SynthesizerRegistry::default();
        assert!(registry.get(DEFAULT_DOMAIN_SYNTHESIZER).is_some());
        assert!(registry.get(ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER).is_some());
    }

    #[test]
    fn actuator_wrench_allocation_uses_authored_body_torque_axes() {
        let actuators = [
            lunco_cosim_core::ForceActuator {
                local_position: Vec3::Y,
                direction_local: Vec3::Z,
                max_force_n: 1.0,
            },
            lunco_cosim_core::ForceActuator {
                local_position: Vec3::Z,
                direction_local: Vec3::X,
                max_force_n: 1.0,
            },
            lunco_cosim_core::ForceActuator {
                local_position: Vec3::X,
                direction_local: Vec3::Y,
                max_force_n: 1.0,
            },
        ];

        let (matrix, step) = synthesis::actuator_wrench_matrix(&actuators).unwrap();
        assert_eq!(matrix.len(), 3);
        assert!(step > 0.0 && step.is_finite());
        assert_eq!(matrix[0], [0.0, 0.0, 1.0, 1.0, 0.0, 0.0]);
        assert_eq!(matrix[1], [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(matrix[2], [0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn actuator_wrench_rejects_zero_authority_geometry() {
        let actuators = [lunco_cosim_core::ForceActuator {
            local_position: Vec3::ZERO,
            direction_local: Vec3::ZERO,
            max_force_n: 1.0,
        }];

        let error = synthesis::actuator_wrench_matrix(&actuators).unwrap_err();
        assert!(error.contains("no finite physical wrench authority"));
    }

    #[test]
    fn actuator_wrench_policy_emits_source_and_visual_schema() {
        lunco_hooks_rhai::register_rhai_hook(
            "synth.actuator-wrench",
            "synthesize",
            lunco_assets_runtime::scripting::active_policy_set()
                .expect("shipped synthesis policies")
                .into_iter()
                .find(|policy| policy.spec.source == "synth_actuator_wrench.rhai")
                .expect("actuator policy source exists")
                .source
                .as_str(),
            true,
        )
        .expect("actuator policy compiles");
        let facts = lunco_hooks::HookValue::Map(vec![
            (
                "model_name".into(),
                lunco_hooks::HookValue::str("AttitudeActuation"),
            ),
            (
                "root".into(),
                lunco_hooks::HookValue::str("/Lander/Actuation"),
            ),
            (
                "inputs".into(),
                lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str(
                    "desired_torque_z",
                )]),
            ),
            (
                "outputs".into(),
                lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str("valve")]),
            ),
            (
                "actuator_paths".into(),
                lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str(
                    "/Lander/Thruster",
                )]),
            ),
            (
                "wrench_matrix".into(),
                lunco_hooks::HookValue::Array(
                    (0..6)
                        .map(|row| {
                            lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::Float(
                                if row == 5 { 1.0 } else { 0.0 },
                            )])
                        })
                        .collect(),
                ),
            ),
            ("allocation_step".into(), lunco_hooks::HookValue::Float(0.1)),
            ("actuator_count".into(), lunco_hooks::HookValue::Int(1)),
        ]);
        let value = lunco_hooks::invoke("synth.actuator-wrench", &[facts])
            .expect("actuator hook registered")
            .expect("actuator policy succeeds");
        let lunco_hooks::HookValue::Map(map) = value else {
            panic!("actuator policy must return a map");
        };
        let source = map
            .into_iter()
            .find_map(|(key, value)| (key == "source").then(|| value.as_str().map(str::to_owned)))
            .flatten()
            .expect("actuator policy source");
        assert!(source.contains("LunCo.Actuation.WrenchAllocator"));
        assert!(source.contains("wrench_matrix = ["));
        assert!(source.contains("allocation_step = 0.1"));
        assert!(source.contains("allocator.desired_torque_z = desired_torque_z;"));
        assert!(source.contains("allocator.desired_force_x = 0.0;"));
        assert!(source.contains("valve = allocator.command[1];"));
        assert!(source.contains("Force allocation | 1 actuator(s)"));
        let ast = lunco_modelica_ast::parse_to_ast(&source, "wrench.mo")
            .expect("generated actuator visual schema must remain valid Modelica");
        let class = lunco_modelica_index::class_lookup::find_class_by_qualified_name(
            &ast,
            "AttitudeActuation",
        )
        .expect("generated actuator model");
        assert!(lunco_modelica_ast::annotations::extract_icon(&class.annotation).is_some());
        assert!(lunco_modelica_ast::annotations::extract_diagram(&class.annotation).is_some());
    }
}
