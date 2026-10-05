//! The USD side of a **Modelica program facet** — one reader, shared by every
//! consumer of that authoring contract.
//!
//! The runtime network projector resolves the Modelica class from the loaded
//! source file. This module owns only the composed-USD contract that must hold
//! before that asynchronous source resolution can begin; the lint fact producer
//! and the runtime projector call the same validator.
//!
//! Modelica lexical rules (identifiers, keywords, the mangling used to spell a
//! USD path as an identifier) live here for the same reason: the authoring
//! check and the code emitter must use ONE definition of "valid member name",
//! and this is the crate both sides already depend on.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

// The runtime projector and the per-prim binder share this same composed read
// contract. A prepared asset plan implements it without retaining OpenUSD
// handles, while live edits continue to use StageView.
use bevy::asset::{AssetId, AssetServer};
use bevy::prelude::{App, Entity, ResMut, Resource, World};
use lunco_usd_bevy_stage::{UsdStageAsset, read::UsdReadObject};
use openusd::sdf::Path as SdfPath;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct ModelicaNetworkMembershipKey {
    stage: AssetId<UsdStageAsset>,
    generation: u64,
    instance: Option<u64>,
}

/// Shared composed-stage membership facts used by program projection, cosim
/// participant admission, and connection wiring. Values are send-safe; no live
/// OpenUSD reader is retained.
#[derive(Resource, Default)]
pub struct ModelicaNetworkMembershipCache {
    entries: std::collections::HashMap<ModelicaNetworkMembershipKey, Arc<HashSet<String>>>,
}

impl ModelicaNetworkMembershipCache {
    /// Reuse the membership set for one composed source revision and instance.
    pub fn get(
        &self,
        stage: AssetId<UsdStageAsset>,
        generation: u64,
        instance: Option<u64>,
    ) -> Option<Arc<HashSet<String>>> {
        self.entries
            .get(&ModelicaNetworkMembershipKey {
                stage,
                generation,
                instance,
            })
            .cloned()
    }

    /// Publish one computed membership set, replacing stale generations for
    /// the same stage and instance.
    pub fn insert(
        &mut self,
        stage: AssetId<UsdStageAsset>,
        generation: u64,
        instance: Option<u64>,
        members: HashSet<String>,
    ) -> Arc<HashSet<String>> {
        let key = ModelicaNetworkMembershipKey {
            stage,
            generation,
            instance,
        };
        if let Some(existing) = self.entries.get(&key) {
            return Arc::clone(existing);
        }
        self.entries
            .retain(|cached, _| cached.stage != stage || cached.instance != instance);
        let members = Arc::new(members);
        self.entries.insert(key, Arc::clone(&members));
        members
    }

    /// Get the cached set or compute it once for this source revision.
    pub fn get_or_insert_with(
        &mut self,
        stage: AssetId<UsdStageAsset>,
        generation: u64,
        instance: Option<u64>,
        build: impl FnOnce() -> HashSet<String>,
    ) -> Arc<HashSet<String>> {
        if let Some(members) = self.get(stage, generation, instance) {
            return members;
        }
        self.insert(stage, generation, instance, build())
    }

    /// Drop all stage-derived facts at the scene replacement boundary.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Resource)]
struct ModelicaNetworkMembershipLifecycleInstalled;

/// Install the shared membership index and its one scene-teardown owner.
/// Multiple USD consumers may request installation; the teardown hook is
/// registered once for the application.
pub fn install_modelica_network_membership_cache(app: &mut App) {
    app.init_resource::<ModelicaNetworkMembershipCache>();
    if app
        .world()
        .get_resource::<ModelicaNetworkMembershipLifecycleInstalled>()
        .is_none()
    {
        app.world_mut()
            .insert_resource(ModelicaNetworkMembershipLifecycleInstalled);
        app.add_systems(
            lunco_core::SceneTeardown,
            clear_modelica_network_membership_cache,
        );
    }
}

fn clear_modelica_network_membership_cache(mut cache: ResMut<ModelicaNetworkMembershipCache>) {
    cache.clear();
}

/// Why a prim that claims to be a Modelica program facet cannot be used as one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramSourceIssue {
    /// The property carrying the unusable opinion (`<prim>.info:sourceAsset`).
    pub property: String,
    /// Actionable explanation, suitable for a console line or a lint message.
    pub message: String,
}

/// The source reference authored by a Modelica program facet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelicaSourceRef {
    /// The asset containing the Modelica source.
    pub asset: String,
    /// An optional fully-qualified definition selected inside the source file.
    pub sub_identifier: Option<String>,
}

/// The runtime backend selected by a program's composed source.
///
/// This is deliberately a small classification, not a registry of running
/// programs. The USD program prim remains the identity; consumers use this
/// result only to decide which executor owns the prim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramBackend {
    /// A registered implementation named by `info:id`.
    Builtin,
    /// A Rhai source, inline or file-backed.
    Rhai,
    /// A Modelica source file.
    Modelica,
    /// A Python source file.
    Python,
}

/// The one selected implementation arm of a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramSource {
    /// A registered implementation named by `info:id`.
    Id(String),
    /// Text authored directly on the program prim.
    Code(String),
    /// A resolver-visible external asset.
    Asset(String),
}

/// A source resolved far enough for a backend to claim it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProgram {
    /// The backend that owns execution of this source.
    pub backend: ProgramBackend,
    /// The selected source arm and its value.
    pub source: ProgramSource,
}

/// Apply one resolved generic program to its owning ECS entity.
///
/// The USD program contract is shared by the initial visual projection and
/// the live document bridge. The owner-specific refresh logic remains in the
/// USD runtime crate, but this marker projection belongs beside the shared
/// program resolution types so both paths use one implementation.
pub fn apply_program_resolution(
    world: &mut World,
    entity: Entity,
    stage_id: AssetId<lunco_usd_bevy_stage::UsdStageAsset>,
    resolved: Option<ResolvedProgram>,
) {
    let rhai_asset = resolved.as_ref().and_then(|resolved| {
        let ProgramSource::Asset(asset) = &resolved.source else {
            return None;
        };
        (resolved.backend == ProgramBackend::Rhai).then(|| {
            lunco_usd_bevy_stage::asset::resolve_stage_asset_path(
                world.resource::<AssetServer>(),
                stage_id,
                asset,
                world.get_resource::<lunco_assets_core::TwinRoots>(),
            )
        })
    });
    let mut entity = world.entity_mut(entity);
    entity
        .remove::<lunco_core::programs::ProgramDriverId>()
        .remove::<lunco_core::EmbeddedScenarioSource>()
        .remove::<lunco_core::EmbeddedScenarioPath>();

    match resolved {
        Some(ResolvedProgram {
            backend: ProgramBackend::Builtin,
            source: ProgramSource::Id(id),
        }) => {
            entity.insert(lunco_core::programs::ProgramDriverId(id));
        }
        Some(ResolvedProgram {
            backend: ProgramBackend::Rhai,
            source: ProgramSource::Code(source),
        }) => {
            entity.insert(lunco_core::EmbeddedScenarioSource(source));
        }
        Some(ResolvedProgram {
            backend: ProgramBackend::Rhai,
            source: ProgramSource::Asset(_),
        }) => {
            let Some(asset) = rhai_asset else {
                bevy::log::warn!(
                    "[usd] Rhai program asset could not be resolved for {:?}",
                    entity.id()
                );
                return;
            };
            match asset {
                Ok(asset) => {
                    entity.insert(lunco_core::EmbeddedScenarioPath(asset));
                }
                Err(error) => {
                    let id = entity.id();
                    drop(entity);
                    lunco_core::trigger_runtime_error(
                        &mut world.commands(),
                        "usd-program-asset-resolution-failed",
                        format!("Rhai program asset for {id:?}: {error}"),
                    );
                }
            }
        }
        Some(resolved) => {
            bevy::log::warn!(
                "[usd] non-generic program {:?} reached generic projection: {:?}",
                entity.id(),
                resolved.backend
            );
        }
        None => {}
    }
}

fn asset_path_without_fragment(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

/// Classify a source asset by its canonical extension.
fn program_asset_backend(path: &str) -> Option<ProgramBackend> {
    let path = asset_path_without_fragment(path);
    if path.ends_with(".rhai") {
        Some(ProgramBackend::Rhai)
    } else if path.ends_with(".mo") {
        Some(ProgramBackend::Modelica)
    } else if path.ends_with(".py") {
        Some(ProgramBackend::Python)
    } else {
        None
    }
}

fn source_issue(prim: &SdfPath, property: &str, message: impl Into<String>) -> ProgramSourceIssue {
    ProgramSourceIssue {
        property: format!("{prim}.{property}"),
        message: message.into(),
    }
}

/// Resolve the selected implementation arm for one composed
/// `LunCoProgramAPI` prim.
///
/// The selector is authoritative. A populated non-selected source arm is an
/// authoring conflict rather than an alternative to try. No host or collection
/// traversal happens here; execution ownership is resolved separately.
pub fn resolve_program(
    view: &dyn UsdReadObject,
    prim: &SdfPath,
) -> Result<ResolvedProgram, ProgramSourceIssue> {
    let selector = view
        .text(prim, "info:implementationSource")
        .unwrap_or_default();
    let id = view
        .text(prim, "info:id")
        .filter(|id| !id.trim().is_empty());
    let code = view
        .text(prim, "info:sourceCode")
        .filter(|code| !code.trim().is_empty());
    let asset = view
        .asset(prim, "info:sourceAsset")
        .filter(|asset| !asset.trim().is_empty());

    match selector.as_str() {
        "id" => {
            let Some(id) = id else {
                return Err(source_issue(
                    prim,
                    "info:id",
                    "info:implementationSource selects id but info:id is empty",
                ));
            };
            if code.is_some() || asset.is_some() {
                return Err(source_issue(
                    prim,
                    "info:implementationSource",
                    "id is selected but sourceCode or sourceAsset is also populated",
                ));
            }
            Ok(ResolvedProgram {
                backend: ProgramBackend::Builtin,
                source: ProgramSource::Id(id),
            })
        }
        "sourceCode" => {
            let Some(code) = code else {
                return Err(source_issue(
                    prim,
                    "info:sourceCode",
                    "info:implementationSource selects sourceCode but info:sourceCode is empty",
                ));
            };
            if id.is_some() || asset.is_some() {
                return Err(source_issue(
                    prim,
                    "info:implementationSource",
                    "sourceCode is selected but info:id or sourceAsset is also populated",
                ));
            }
            Ok(ResolvedProgram {
                backend: ProgramBackend::Rhai,
                source: ProgramSource::Code(code),
            })
        }
        "sourceAsset" => {
            let Some(asset) = asset else {
                return Err(source_issue(
                    prim,
                    "info:sourceAsset",
                    "info:implementationSource selects sourceAsset but info:sourceAsset is empty",
                ));
            };
            if id.is_some() || code.is_some() {
                return Err(source_issue(
                    prim,
                    "info:implementationSource",
                    "sourceAsset is selected but info:id or sourceCode is also populated",
                ));
            }
            let Some(backend) = program_asset_backend(&asset) else {
                return Err(source_issue(
                    prim,
                    "info:sourceAsset",
                    format!("unsupported program source asset `{asset}`"),
                ));
            };
            Ok(ResolvedProgram {
                backend,
                source: ProgramSource::Asset(asset),
            })
        }
        "" => Err(source_issue(
            prim,
            "info:implementationSource",
            "info:implementationSource is empty",
        )),
        other => Err(source_issue(
            prim,
            "info:implementationSource",
            format!("unsupported info:implementationSource `{other}`"),
        )),
    }
}

/// Whether the source is owned by the generic script/driver projection in
/// `lunco-usd-bevy`. Modelica and Python sources have their own projections.
pub fn is_generic_program_backend(backend: ProgramBackend) -> bool {
    matches!(backend, ProgramBackend::Builtin | ProgramBackend::Rhai)
}

/// Why a prim that claims to be a Modelica program facet cannot enter source
/// resolution.
///
/// The `.mo` itself is deliberately NOT parsed here: this runs inside stage
/// reads on the web too, where the file may still be unfetched. The loaded
/// source resolver is the only authority for the class name.
pub fn modelica_source_ref(
    view: &dyn UsdReadObject,
    prim: &SdfPath,
) -> Result<ModelicaSourceRef, ProgramSourceIssue> {
    let resolved = resolve_program(view, prim)?;
    if resolved.backend != ProgramBackend::Modelica {
        return Err(source_issue(
            prim,
            "info:sourceAsset",
            "a Modelica program facet must select a .mo sourceAsset",
        ));
    }
    let ProgramSource::Asset(asset) = resolved.source else {
        return Err(source_issue(
            prim,
            "info:sourceAsset",
            "a Modelica program facet must select a .mo sourceAsset",
        ));
    };
    let sub_identifier = view
        .text(prim, "info:sourceAsset:subIdentifier")
        .filter(|value| !value.is_empty());
    if let Some(class) = sub_identifier.as_deref() {
        if class.is_empty() || !class.split('.').all(is_modelica_identifier) {
            return Err(ProgramSourceIssue {
                property: format!("{prim}.info:sourceAsset:subIdentifier"),
                message: format!("`{class}` is not a fully-qualified Modelica class name"),
            });
        }
    }
    Ok(ModelicaSourceRef {
        asset,
        sub_identifier,
    })
}

/// Is `prim` the root of a projected domain network — i.e. does it carry the
/// component collection the runtime compiles into one generated model?
///
/// Codeless multiple-apply schemas are not consistently surfaced by every
/// OpenUSD binding through `HasAPI`; their standard authored properties are
/// authoritative and round-trip in all runtimes.
pub fn is_domain_network_root(view: &dyn UsdReadObject, prim: &SdfPath) -> bool {
    view.any_attr_with_prefix(prim, "collection:components:")
}

/// Reusable root attributes and component paths for classifying a network's
/// Modelica boundary without re-reading its USD collection for every port.
/// Build this once per network read from the same composed reader and revision.
pub struct ModelicaNetworkBoundaryIndex<'a> {
    root: &'a SdfPath,
    root_path: String,
    root_attributes: &'a [String],
    component_paths: HashSet<String>,
}

impl<'a> ModelicaNetworkBoundaryIndex<'a> {
    /// Index one network root from its already-read attributes and collection.
    pub fn new(root: &'a SdfPath, root_attributes: &'a [String], members: &[SdfPath]) -> Self {
        Self {
            root,
            root_path: root.to_string(),
            root_attributes,
            component_paths: members
                .iter()
                .filter(|path| !path.is_property_path())
                .map(ToString::to_string)
                .collect(),
        }
    }

    /// Whether a root output is sourced by a component and does not collide
    /// with a same-named generated network input.
    pub fn is_network_boundary_output(&self, view: &dyn UsdReadObject, attr: &str) -> bool {
        let Some(name) = attr
            .strip_prefix("outputs:")
            .map(|name| name.strip_suffix(".connect").unwrap_or(name))
        else {
            return false;
        };
        if self.root_attributes.iter().any(|candidate| {
            candidate
                .strip_prefix("inputs:")
                .map(|name| name.strip_suffix(".connect").unwrap_or(name))
                == Some(name)
        }) {
            return false;
        }
        view.connections(self.root, attr).iter().any(|target| {
            target
                .rsplit_once(".outputs:")
                .is_some_and(|(source, _)| self.component_paths.contains(source))
        })
    }

    /// Resolve a root input forwarded through a root output to a component
    /// output in this collection.
    pub fn internal_network_input_source(
        &self,
        view: &dyn UsdReadObject,
        input: &str,
    ) -> Option<String> {
        let input_name = input.strip_prefix("inputs:").unwrap_or(input);
        let input_name = input_name.strip_suffix(".connect").unwrap_or(input_name);
        let input_attr = self.root_attributes.iter().find(|attr| {
            attr.strip_prefix("inputs:")
                .map(|name| name.strip_suffix(".connect").unwrap_or(name))
                == Some(input_name)
        })?;
        let input_connections = view.connections(self.root, input_attr);
        let input_source = input_connections.first()?.as_str();
        let (source_root, source_output) = input_source.split_once(".outputs:")?;
        if source_root != self.root_path {
            return None;
        }
        self.network_member_output_source(view, source_output)
    }

    /// Resolve a root output to a component output in this collection.
    pub fn network_member_output_source(
        &self,
        view: &dyn UsdReadObject,
        output: &str,
    ) -> Option<String> {
        let output_name = output.strip_prefix("outputs:").unwrap_or(output);
        let output_name = output_name.strip_suffix(".connect").unwrap_or(output_name);
        let output_attr = self.root_attributes.iter().find(|attr| {
            attr.strip_prefix("outputs:")
                .map(|name| name.strip_suffix(".connect").unwrap_or(name))
                == Some(output_name)
        })?;
        let output_connections = view.connections(self.root, output_attr);
        let member_output = output_connections.first()?.as_str();
        self.member_output_target(member_output)
    }

    fn member_output_target(&self, member_output: &str) -> Option<String> {
        let (member, _) = member_output.split_once(".outputs:")?;
        self.component_paths
            .contains(member)
            .then(|| member_output.to_string())
    }
}

/// Whether an authored root output is a Modelica network boundary.
///
/// A vehicle root may carry ordinary output ports such as `drive_left` and
/// `steering` while also owning a `CollectionAPI:components` network. Those
/// ports are not generated-model outputs unless their authored connection
/// names a member of the collection. A USD root may also have an input and an
/// output with the same name: the input is the generated network's internal
/// boundary, while the output is the external actuator surface. Modelica has
/// one identifier namespace, so the colliding output remains a runtime port
/// fed by the promoted member output rather than becoming a second generated
/// boundary. Keeping this distinction in the shared USD contract prevents the
/// linter and projector from treating an actuator command surface as an
/// unsourced or duplicate Modelica boundary.
pub fn is_network_boundary_output(view: &dyn UsdReadObject, root: &SdfPath, attr: &str) -> bool {
    let root_attributes = view.attr_names(root);
    let Some(name) = attr
        .strip_prefix("outputs:")
        .map(|name| name.strip_suffix(".connect").unwrap_or(name))
    else {
        return false;
    };
    if root_attributes.iter().any(|candidate| {
        candidate
            .strip_prefix("inputs:")
            .map(|name| name.strip_suffix(".connect").unwrap_or(name))
            == Some(name)
    }) {
        return false;
    }
    let Ok(members) = view.collection_members(root, "components") else {
        return false;
    };
    ModelicaNetworkBoundaryIndex::new(root, &root_attributes, &members)
        .is_network_boundary_output(view, attr)
}

/// Resolve a root input that is internally fed by a member output.
///
/// A composed assembly can expose an actuator name in both namespaces: the
/// power network consumes `inputs:drive_left`, while a composed drive law
/// produces `outputs:drive_left`.  When the latter is authored as the source
/// of the former, the path is internal to one generated Modelica network.  It
/// must not become an external wrapper input or a second runtime wire.
///
/// The returned value is the member output target after following the one
/// authored root-output forward.  `None` means the root input remains an
/// ordinary external boundary, including the common case where its source is
/// a runtime actuator port rather than a generated member.
pub fn internal_network_input_source(
    view: &dyn UsdReadObject,
    root: &SdfPath,
    input: &str,
) -> Option<String> {
    let root_string = root.to_string();
    let input_name = input.strip_prefix("inputs:").unwrap_or(input);
    let input_name = input_name.strip_suffix(".connect").unwrap_or(input_name);
    let root_attributes = view.attr_names(root);
    let input_attr = root_attributes.iter().find(|attr| {
        attr.strip_prefix("inputs:")
            .map(|name| name.strip_suffix(".connect").unwrap_or(name))
            == Some(input_name)
    })?;
    let input_source = view.connections(root, input_attr).first()?.to_string();
    let (source_root, source_output) = input_source.split_once(".outputs:")?;
    if source_root != root_string {
        return None;
    }
    let members = view.collection_members(root, "components").ok()?;
    ModelicaNetworkBoundaryIndex::new(root, &root_attributes, &members)
        .network_member_output_source(view, source_output)
}

/// Resolve an authored root output to a generated member output.
///
/// This is the output half of [`internal_network_input_source`].  It is useful
/// when USD composition has already normalized a member's consumer connection
/// to `root.outputs:name`: the reader still needs the authored member address so
/// it can turn that edge into a direct Modelica causal equation.
pub fn network_member_output_source(
    view: &dyn UsdReadObject,
    root: &SdfPath,
    output: &str,
) -> Option<String> {
    let root_attributes = view.attr_names(root);
    let output_name = output.strip_prefix("outputs:").unwrap_or(output);
    let output_name = output_name.strip_suffix(".connect").unwrap_or(output_name);
    let output_attr = root_attributes.iter().find(|attr| {
        attr.strip_prefix("outputs:")
            .map(|name| name.strip_suffix(".connect").unwrap_or(name))
            == Some(output_name)
    })?;
    let member_output = view.connections(root, output_attr).first()?.to_string();
    let (member, _) = member_output.split_once(".outputs:")?;
    let members = view.collection_members(root, "components").ok()?;
    let index = ModelicaNetworkBoundaryIndex::new(root, &root_attributes, &members);
    index
        .component_paths
        .contains(member)
        .then_some(member_output)
}

/// The default synthesizer for a collection of Modelica program facets.
pub const DEFAULT_DOMAIN_SYNTHESIZER: &str = "acausal-network";

/// The geometry-derived synthesizer for a collection of force actuators.
pub const ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER: &str = "actuator-wrench";

/// Derive the owner of a component collection from its composed member roles.
///
/// A collection of LunCoProgramAPI members is a Modelica network. A
/// collection of LunCoForceActuatorAPI members is a geometry-derived wrench
/// allocator. Mixed or unclassified collections are invalid and must be
/// reported by the caller; no owner is guessed.
pub fn derive_synthesizer_name(view: &dyn UsdReadObject, root: &SdfPath) -> Result<String, String> {
    let members = view
        .collection_members(root, "components")
        .map_err(|error| format!("could not read component collection: {error}"))?;
    let roles = members
        .iter()
        .filter(|path| !path.is_property_path())
        .map(|member| {
            (
                member.to_string(),
                view.has_api_schema(member, "LunCoForceActuatorAPI"),
                view.has_api_schema(member, "LunCoProgramAPI"),
            )
        });
    derive_synthesizer_name_from_member_roles(roles)
}

/// Derive network ownership from role facts already read from the composed
/// members. Runtime discovery uses this when it has read those schemas while
/// resolving Modelica sources, avoiding a second collection traversal.
pub fn derive_synthesizer_name_from_member_roles(
    member_roles: impl IntoIterator<Item = (String, bool, bool)>,
) -> Result<String, String> {
    let mut force_actuators = 0usize;
    let mut modelica_programs = 0usize;
    let mut unclassified = Vec::new();
    for (member, is_force, is_program) in member_roles {
        match (is_force, is_program) {
            (true, false) => force_actuators += 1,
            (false, true) => modelica_programs += 1,
            _ => unclassified.push(member),
        }
    }
    if force_actuators > 0 && modelica_programs == 0 && unclassified.is_empty() {
        return Ok(ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER.to_string());
    }
    if modelica_programs > 0 && force_actuators == 0 && unclassified.is_empty() {
        return Ok(DEFAULT_DOMAIN_SYNTHESIZER.to_string());
    }
    if force_actuators > 0 || modelica_programs > 0 || !unclassified.is_empty() {
        return Err(format!(
            "component collection has incompatible member roles: force_actuators={force_actuators}, modelica_programs={modelica_programs}, unclassified={unclassified:?}"
        ));
    }
    Ok(DEFAULT_DOMAIN_SYNTHESIZER.to_string())
}

/// Select the owner of a composed component collection.
///
/// An authored `LunCoDomainSynthesisAPI` is an explicit contract. Without
/// that API, ownership is derived from the composed member role schemas. This
/// USD-facing selector belongs beside the role classifier so runtime
/// projection and lint facts cannot drift apart.
pub fn select_synthesizer_name(view: &dyn UsdReadObject, root: &SdfPath) -> Result<String, String> {
    if let Some(name) = authored_synthesizer_name(view, root) {
        return Ok(name);
    }
    derive_synthesizer_name(view, root)
}

/// Select the authored synthesizer when present, otherwise derive it from
/// member role facts captured by the owning discovery pass.
pub fn select_synthesizer_name_from_member_roles(
    view: &dyn UsdReadObject,
    root: &SdfPath,
    member_roles: impl IntoIterator<Item = (String, bool, bool)>,
) -> Result<String, String> {
    if let Some(name) = authored_synthesizer_name(view, root) {
        return Ok(name);
    }
    derive_synthesizer_name_from_member_roles(member_roles)
}

fn authored_synthesizer_name(view: &dyn UsdReadObject, root: &SdfPath) -> Option<String> {
    view.has_api_schema(root, "LunCoDomainSynthesisAPI")
        .then(|| {
            view.text(root, "lunco:synthesizer")
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| DEFAULT_DOMAIN_SYNTHESIZER.to_string())
        })
}

/// Every Modelica program prim on the stage that belongs to SOME component
/// collection.
///
/// A Modelica program member is compiled as part of its network's generated
/// model, so no other pass may give it a solver of its own. Membership — not
/// "does it declare an acausal connector" — is what makes that true: a
/// causal-only member (a controller, a PDU) is just as much part of the
/// generated DAE, and gating on connectors handed it a second, independent
/// solver whose outputs then fed the wire fabric.
///
/// A collection may also contain physical prims consumed by a different
/// synthesizer. For example, the actuator-wrench network owns USD force
/// actuators as geometry, while those actuators still need their authored
/// scalar input wires materialised by the cosim projection. `LunCoProgramAPI`
/// is the authoritative boundary between a Modelica member and such a
/// physical participant; collection membership alone is not.
pub fn modelica_network_member_paths(view: &dyn UsdReadObject) -> HashSet<String> {
    let mut members = HashSet::new();
    for prim in view.prim_paths() {
        if !is_domain_network_root(view, &prim) {
            continue;
        }
        let Ok(paths) = view.collection_members(&prim, "components") else {
            continue;
        };
        members.extend(
            paths
                .into_iter()
                .filter(|path| view.has_api_schema(path, "LunCoProgramAPI"))
                .map(|path| path.to_string()),
        );
    }
    members
}

/// The undirected topology used to derive synthesis units from a composed
/// program graph.
///
/// Both acausal `connectors:*` edges and internal causal output-to-input edges
/// are represented here. The graph deliberately carries no domain vocabulary:
/// electrical, thermal, harness, and future synthesizers all need the same
/// deterministic connected-component operation, while the rules for emitting
/// a component remain owned by the selected synthesizer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgramGraph {
    nodes: BTreeSet<String>,
    edges: BTreeMap<String, BTreeSet<String>>,
}

impl ProgramGraph {
    /// Add a program facet, including an isolated facet as a one-node unit.
    pub fn add_node(&mut self, node: impl Into<String>) {
        let node = node.into();
        self.nodes.insert(node.clone());
        self.edges.entry(node).or_default();
    }

    /// Add an undirected relation between two program facets.
    pub fn connect(&mut self, left: impl Into<String>, right: impl Into<String>) {
        let left = left.into();
        let right = right.into();
        self.add_node(left.clone());
        self.add_node(right.clone());
        self.edges
            .entry(left.clone())
            .or_default()
            .insert(right.clone());
        self.edges.entry(right).or_default().insert(left);
    }

    /// Return stable connected units, sorted by their first composed path.
    pub fn connected_components(&self) -> Vec<Vec<String>> {
        let mut unseen = self.nodes.clone();
        let mut units = Vec::new();
        while let Some(seed) = unseen.iter().next().cloned() {
            let mut pending = vec![seed];
            let mut unit = Vec::new();
            while let Some(current) = pending.pop() {
                if !unseen.remove(&current) {
                    continue;
                }
                unit.push(current.clone());
                if let Some(neighbors) = self.edges.get(&current) {
                    pending.extend(neighbors.iter().cloned());
                }
            }
            unit.sort();
            units.push(unit);
        }
        units.sort_by(|left, right| left.first().cmp(&right.first()));
        units
    }
}

/// The Modelica keywords a generated or authored member name may not be.
const KEYWORDS: &[&str] = &[
    "algorithm",
    "and",
    "annotation",
    "block",
    "break",
    "class",
    "connect",
    "connector",
    "constant",
    "constrainedby",
    "der",
    "discrete",
    "each",
    "else",
    "elseif",
    "elsewhen",
    "encapsulated",
    "end",
    "enumeration",
    "equation",
    "expandable",
    "extends",
    "external",
    "false",
    "final",
    "flow",
    "for",
    "function",
    "if",
    "import",
    "impure",
    "in",
    "initial",
    "inner",
    "input",
    "loop",
    "model",
    "not",
    "operator",
    "or",
    "outer",
    "output",
    "package",
    "parameter",
    "partial",
    "protected",
    "public",
    "pure",
    "record",
    "redeclare",
    "replaceable",
    "return",
    "stream",
    "then",
    "true",
    "type",
    "when",
    "while",
    "within",
];

pub fn is_modelica_identifier(raw: &str) -> bool {
    let mut chars = raw.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
        && !KEYWORDS.contains(&raw)
}

/// Injective ASCII spelling for arbitrary USD path/name text.
///
/// `_` is escaped too, so punctuation replacement cannot collapse `Motor-A`
/// and `Motor_A` onto one Modelica instance.
pub fn modelica_identifier(raw: &str) -> String {
    if is_modelica_identifier(raw) {
        return raw.to_string();
    }
    let mut result = modelica_path_identifier(raw);
    if !result.starts_with("usd_") {
        result.insert_str(0, "usd_");
    }
    result
}

pub fn modelica_path_identifier(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len() + 1);
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character);
        } else if character == '_' {
            result.push_str("__");
        } else {
            result.push_str(&format!("_x{:x}_", character as u32));
        }
    }
    if result.is_empty() {
        result.push_str("ModelicaNetwork");
    }
    if result.as_bytes()[0].is_ascii_digit() || !is_modelica_identifier(&result) {
        result.insert_str(0, "usd_");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestStage(openusd::usd::Stage);

    impl TestStage {
        fn view(&self) -> lunco_usd_bevy_stage::view::StageView<'_> {
            lunco_usd_bevy_stage::view::StageView::new(&self.0)
        }
    }

    fn program_stage(source: &str) -> TestStage {
        let recipe = lunco_usd_compose::recipe::StageRecipe::from_source("programs.usda", source);
        let stage = lunco_usd_bevy_stage::compose::build_stage_with_resolver(&recipe)
            .expect("build program stage")
            .0;
        TestStage(stage)
    }

    #[test]
    fn generated_identifiers_are_injective_and_avoid_keywords() {
        assert_ne!(
            modelica_path_identifier("Motor-A"),
            modelica_path_identifier("Motor_A")
        );
        assert_eq!(modelica_identifier("model"), "usd_model");
        assert_eq!(modelica_identifier("3phase"), "usd_3phase");
        assert!(is_modelica_identifier(&modelica_identifier("left/right")));
    }

    #[test]
    fn selected_program_arm_is_the_single_backend_resolution() {
        let stage = program_stage(
            "#usda 1.0\n\
             def Scope \"InlineRhai\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform token info:implementationSource = \"sourceCode\"\n\
                 uniform string info:sourceCode = \"fn task(me, ctx) { seq([]) }\"\n\
             }\n\
             def Scope \"Rhai\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform token info:implementationSource = \"sourceAsset\"\n\
                 uniform asset info:sourceAsset = @lunco://scenarios/test.rhai@\n\
             }\n\
             def Scope \"Modelica\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform token info:implementationSource = \"sourceAsset\"\n\
                 uniform asset info:sourceAsset = @lunco://models/Test.mo@\n\
             }\n\
             def Scope \"Conflict\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform token info:implementationSource = \"sourceAsset\"\n\
                 uniform asset info:sourceAsset = @lunco://scenarios/test.rhai@\n\
                 uniform string info:sourceCode = \"fn drive(ctx) { 1 }\"\n\
             }\n\
             def Scope \"MissingSelector\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform asset info:sourceAsset = @lunco://scenarios/test.rhai@\n\
             }\n",
        );
        let view = stage.view();

        let rhai = SdfPath::new("/Rhai").unwrap();
        assert_eq!(
            resolve_program(&view, &rhai),
            Ok(ResolvedProgram {
                backend: ProgramBackend::Rhai,
                source: ProgramSource::Asset("lunco://scenarios/test.rhai".into()),
            })
        );
        let modelica = SdfPath::new("/Modelica").unwrap();
        assert_eq!(
            modelica_source_ref(&view, &modelica).unwrap().asset,
            "lunco://models/Test.mo"
        );

        let conflict = SdfPath::new("/Conflict").unwrap();
        let issue = resolve_program(&view, &conflict).expect_err("conflicting arms are invalid");
        assert!(issue.property.ends_with(".info:implementationSource"));

        let missing = SdfPath::new("/MissingSelector").unwrap();
        let issue = resolve_program(&view, &missing).expect_err("selection is mandatory");
        assert!(issue.message.contains("info:implementationSource is empty"));
    }

    #[test]
    fn program_graph_returns_stable_units_for_acausal_and_causal_edges() {
        let mut graph = ProgramGraph::default();
        graph.add_node("/Rover/Thermal/LeftMass");
        graph.connect("/Rover/Thermal/LeftMass", "/Rover/Thermal/LeftRadiator");
        graph.connect("/Rover/Thermal/LeftLoad", "/Rover/Thermal/LeftMass");
        graph.add_node("/Rover/Thermal/RightMass");

        assert_eq!(
            graph.connected_components(),
            vec![
                vec![
                    "/Rover/Thermal/LeftLoad".to_string(),
                    "/Rover/Thermal/LeftMass".to_string(),
                    "/Rover/Thermal/LeftRadiator".to_string(),
                ],
                vec!["/Rover/Thermal/RightMass".to_string()],
            ]
        );
    }

    #[test]
    fn modelica_membership_cache_reuses_revision_and_replaces_stale_generation() {
        let stage = AssetId::<UsdStageAsset>::default();
        let mut cache = ModelicaNetworkMembershipCache::default();
        let prepared = cache.get_or_insert_with(stage, 0, None, || {
            HashSet::from(["/Prepared/Member".to_string()])
        });
        let reused = cache.get_or_insert_with(stage, 0, None, || {
            panic!("a cached generation must not rescan the composed stage")
        });
        assert!(Arc::ptr_eq(&prepared, &reused));
        assert!(reused.contains("/Prepared/Member"));

        let edited = cache.get_or_insert_with(stage, 1, None, || {
            HashSet::from(["/Live/Member".to_string()])
        });
        assert!(edited.contains("/Live/Member"));
        assert_eq!(cache.entries.len(), 1);

        let instance = cache.get_or_insert_with(stage, 0, Some(17), || {
            HashSet::from(["/Instance/Member".to_string()])
        });
        assert!(instance.contains("/Instance/Member"));
        assert_eq!(cache.entries.len(), 2);

        cache.clear();
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn modelica_membership_cache_is_cleared_at_scene_teardown() {
        let mut app = App::new();
        install_modelica_network_membership_cache(&mut app);
        install_modelica_network_membership_cache(&mut app);
        app.world_mut()
            .resource_mut::<ModelicaNetworkMembershipCache>()
            .insert(
                AssetId::<UsdStageAsset>::default(),
                0,
                None,
                HashSet::from(["/Prepared/Member".to_string()]),
            );

        app.world_mut().run_schedule(lunco_core::SceneTeardown);

        assert!(
            app.world()
                .resource::<ModelicaNetworkMembershipCache>()
                .entries
                .is_empty()
        );
    }
}
