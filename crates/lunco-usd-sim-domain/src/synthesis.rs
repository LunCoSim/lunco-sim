//! Dynamic Modelica synthesis policies for composed USD domains.
//!
//! USD-derived topology and source facts are supplied by the parent domain
//! module. This module owns policy selection and validation of the generated
//! Modelica plan; runtime ECS projection remains in the parent module.

use super::network::read_network;
use super::*;

/// One deterministic Modelica composite unit inside a network root.
///
/// A unit is a connected component of the composed program graph. It is not a
/// second ECS participant: the selected synthesizer emits the units below one
/// generated root model, so the network root keeps one public boundary and one
/// runtime lifecycle while independent acausal subgraphs remain explicit in
/// the generated Modelica. This is the same composite-model shape used by
/// SSP/FMI toolchains.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SynthesisUnit {
    /// Stable generated Modelica class name for the composite unit.
    pub name: String,
    /// Modelica instance name chosen by the synthesis policy. Runtime signal
    /// mapping follows this exact name.
    pub instance: String,
    /// Composed USD members absorbed into the unit.
    pub component_paths: Vec<String>,
    /// Root boundary inputs consumed by this unit.
    pub inputs: BTreeSet<String>,
    /// Root boundary outputs produced by this unit.
    pub outputs: BTreeSet<String>,
}

/// Structural unit facts projected to the dynamic synthesis policy.
///
/// This is intentionally separate from [`SynthesisUnit`]: the latter is the
/// policy result and therefore owns generated Modelica identity, while these
/// facts contain only the USD-derived partition and public boundary. Keeping
/// the two shapes distinct prevents an empty or invented Rust instance name
/// from becoming an accidental policy default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct NetworkUnitFact {
    pub(super) name: String,
    pub(super) component_paths: Vec<String>,
    pub(super) inputs: BTreeSet<String>,
    pub(super) outputs: BTreeSet<String>,
}

/// Visual placement selected alongside a generated Modelica plan.
///
/// Positions are presentation facts, not simulation inputs. The selected
/// synthesizer returns them in its `layout` map. Keeping the positions in the
/// plan makes the policy result inspectable through the generated-source API
/// instead of leaving the visual decision implicit in a separate Rust pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SynthesisLayout {
    /// Generated child-unit class name to Modelica diagram position.
    pub unit_positions: BTreeMap<String, (i32, i32)>,
    /// Composed USD member path to Modelica diagram position, local to the
    /// generated unit that owns the member. Unit diagrams are independent
    /// coordinate systems; root diagrams use `unit_positions` instead.
    pub member_positions: BTreeMap<String, (i32, i32)>,
}

/// One authoring error that prevents a safe runtime projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainProjectionError {
    /// USD prim or property carrying the invalid opinion.
    pub path: String,
    /// Actionable explanation suitable for the simulator console.
    pub message: String,
}

// A network root is ONE runtime compilation unit: one generated root model on
// one entity carrying one `ModelicaModel`. The synthesizer may partition the
// composed graph into several Modelica composite units, but it owns that
// topology operation and emits the units under the root. The runtime therefore
// never invents entities or a second definition of graph connectivity.

/// What a synthesizer hands back: the generated root, its public contract, and
/// the explicit composite units inside it.
#[derive(Clone, Debug, Default)]
pub struct SynthesisPlan {
    /// The Modelica source to compile.
    pub source: String,
    /// The interface extracted from the validated source. Keeping it beside
    /// the source makes validation and installation share one AST parse.
    pub interface: ModelInterface,
    /// Public causal inputs of the generated model.
    pub inputs: BTreeSet<String>,
    /// Public causal outputs of the generated model.
    pub outputs: BTreeSet<String>,
    /// Composed USD paths absorbed into this unit.
    pub component_paths: Vec<String>,
    /// Bundled Modelica source roots required by the policy-emitted source.
    pub source_roots: BTreeSet<String>,
    /// `(prim, source asset, class)` per member — attribution + class audit.
    pub members: Vec<(String, String, String)>,
    /// Causal output aliases emitted for composed members. This is collected
    /// from the USD/Modelica facts once and carried with the policy result so
    /// the runtime projection does not reread the stage or infer aliases from
    /// generated strings.
    pub member_output_aliases: Vec<(String, String, String)>,
    /// Connected composite units emitted below the root model.
    pub units: Vec<SynthesisUnit>,
    /// Presentation placements selected by the synthesizer.
    pub layout: SynthesisLayout,
    /// Communication period inherited from the composed Modelica members.
    pub communication_period_secs: f64,
}

/// One way of turning a composed USD network root into ONE Modelica compilation unit.
///
/// The seam doc 37 §8 asks for. What ships is the acausal-network synthesizer
/// below; a `thermal`, `harness` or `comms-link` synthesizer is a registration,
/// not an edit to [`project_domain_islands`]. A generic network root may select one
/// through `LunCoDomainSynthesisAPI`; otherwise the projector derives the
/// built-in owner from the typed member role schemas.
///
/// Hook-backed entries use the existing `lunco_hooks` substrate: Rust supplies
/// the composed topology facts and Rhai returns the Modelica source, merge
/// units, instance names, and diagram placements. The registry keeps that
/// policy selection independent of
/// `project_domain_islands`, so changing dynamic building behaviour does not
/// require a Rust branch.
pub trait DomainSynthesizer: Send + Sync + 'static {
    /// Registry key, and the token a network root names.
    fn name(&self) -> &str;
    /// Turn one composed network root into a compilation unit.
    fn synthesize(
        &self,
        view: &dyn ComposedReader,
        root: &SdfPath,
        model_name: &str,
        ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>>;
}

/// What a synthesizer concluded about a network root.
#[derive(Debug)]
pub enum SynthOutcome {
    /// Not a scope this synthesizer compiles (or nothing solvable is in it).
    NotMine,
    /// Cannot be decided yet — a member's source has not loaded, so the class it
    /// declares is not knowable. The projection simply waits and is re-triggered
    /// when the source lands.
    Pending,
    Ready(Box<SynthesisPlan>),
}

/// Read-only facts a synthesizer may need beyond the stage itself.
pub struct SynthContext<'a> {
    /// Class-per-source-asset, as declared BY THE FILE. See [`MemberClasses`].
    pub classes: &'a MemberClasses,
}

/// Open registry of synthesizers, by name. No enum: a new domain is a
/// registration from any plugin.
#[derive(Resource)]
pub struct SynthesizerRegistry(
    std::collections::BTreeMap<String, std::sync::Arc<dyn DomainSynthesizer>>,
);

impl Default for SynthesizerRegistry {
    fn default() -> Self {
        let mut registry = Self(Default::default());
        // The shipped network schema is an authored policy. Rust owns the
        // composed-USD facts and validates the policy result, but it must not
        // silently become the owner of source/layout generation when the Rhai
        // policy is absent.
        registry.register(HookSynthesizer {
            hook_id: format!("synth.{DEFAULT_DOMAIN_SYNTHESIZER}"),
            name: DEFAULT_DOMAIN_SYNTHESIZER.to_string(),
        });
        registry.register(ActuatorWrenchSynthesizer);
        registry
    }
}

impl SynthesizerRegistry {
    pub fn register(&mut self, synthesizer: impl DomainSynthesizer) {
        self.0.insert(
            synthesizer.name().to_string(),
            std::sync::Arc::new(synthesizer),
        );
    }
    pub fn get(&self, name: &str) -> Option<&std::sync::Arc<dyn DomainSynthesizer>> {
        self.0.get(name)
    }
    pub fn names(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

/// A synthesizer whose EMIT policy is authored, not compiled in.
///
/// The house split, applied to synthesis: **facts in Rust, rules in rhai.** The
/// composed graph is read here — membership, connectors, causal edges, the
/// boundary, the class each member's file declares — and handed to a hook as a
/// map. What that graph becomes in Modelica is the hook's business: which source library
/// class stands in for a part, whether a fuse is inserted, whether a low-fidelity
/// variant omits parasitic resistance. None of that is a Rust concern, and none
/// of it should require a rebuild to change.
///
/// The hook receives one argument — [`network_facts`] — and returns a map with
/// required `source`, `units`, `layout`, `source_roots`, and
/// `member_output_aliases` keys. `layout` must contain both `units` and
/// `members`, even when a policy has no entries in one section. Unit positions
/// are root-diagram coordinates; member positions are local to their owning
/// unit diagram. Rust validates the policy-owned result but never fills an
/// omitted synthesis decision from a second emitter.
///
/// Registered through [`register_hook_synthesizer`]; the hook id is by convention
/// `synth.<name>`, reached exactly like `lint.usd`.
pub struct HookSynthesizer {
    name: String,
    hook_id: String,
}

impl DomainSynthesizer for HookSynthesizer {
    fn name(&self) -> &str {
        &self.name
    }
    fn synthesize(
        &self,
        view: &dyn ComposedReader,
        root: &SdfPath,
        model_name: &str,
        ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>> {
        // The READER is not the policy's business — a rhai body that had to
        // re-walk USD would be a second, divergent definition of what a network
        // is, which is the exact failure the one-reader rule exists to prevent.
        let Some(network) = read_network(view, root, ctx.classes)? else {
            return Ok(SynthOutcome::NotMine);
        };
        if network.pending_sources {
            return Ok(SynthOutcome::Pending);
        }
        let network_root = network.root.clone();
        let facts = network_facts(&network, model_name, Some(ctx.classes)).map_err(|message| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message: format!(
                    "synthesizer `{}` could not build policy facts: {message}",
                    self.name
                ),
            }]
        })?;
        let result = lunco_hooks::invoke(&self.hook_id, &[facts]).ok_or_else(|| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message: format!(
                    "synthesizer `{}` is selected but its hook `{}` is not registered",
                    self.name, self.hook_id
                ),
            }]
        })?;
        let value = result.map_err(|error| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message: format!("synthesizer `{}` failed: {}", self.name, error.0),
            }]
        })?;
        let lunco_hooks::HookValue::Map(map) = &value else {
            return Err(vec![DomainProjectionError {
                path: network_root,
                message: format!(
                    "synthesizer `{}` must return a map with a Modelica `source` key",
                    self.name
                ),
            }]);
        };
        let Some(source) = map
            .iter()
            .find_map(|(key, value)| (key == "source").then(|| value.as_str()))
            .flatten()
        else {
            return Err(vec![DomainProjectionError {
                path: network_root,
                message: format!(
                    "synthesizer `{}` returned a map with no string `source` key",
                    self.name
                ),
            }]);
        };
        let units = parse_policy_units(
            hook_map_value(map, "units"),
            &network,
            &network_root,
            &self.name,
        )
        .map_err(|message| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message,
            }]
        })?;
        let layout = parse_policy_layout(
            hook_map_value(map, "layout"),
            &network,
            &units,
            &network_root,
            &self.name,
        )
        .map_err(|message| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message,
            }]
        })?;
        let source_roots =
            parse_policy_source_roots(hook_map_value(map, "source_roots"), &self.name).map_err(
                |message| {
                    vec![DomainProjectionError {
                        path: network_root.clone(),
                        message,
                    }]
                },
            )?;
        let member_output_aliases = parse_policy_member_output_aliases(
            hook_map_value(map, "member_output_aliases"),
            &network,
            Some(ctx.classes),
            &network_root,
            &self.name,
        )
        .map_err(|message| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message,
            }]
        })?;
        let interface =
            validate_generated_source(source, model_name, &network, &units, &member_output_aliases)
                .map_err(|message| {
                    vec![DomainProjectionError {
                        path: network_root.clone(),
                        message: format!(
                            "synthesizer `{}` returned invalid Modelica: {message}",
                            self.name
                        ),
                    }]
                })?;
        Ok(SynthOutcome::Ready(Box::new(SynthesisPlan {
            source: source.to_string(),
            interface,
            // The BOUNDARY remains Rust's composed-USD answer. The policy owns
            // the emitted source, merge partition, and visual placement, but
            // cannot invent a runtime port surface or a member outside the
            // composed network.
            inputs: network.inputs.clone(),
            outputs: network.outputs.keys().cloned().collect(),
            component_paths: network
                .components
                .iter()
                .map(|component| component.path.clone())
                .collect(),
            source_roots,
            members: network
                .components
                .iter()
                .map(|component| {
                    (
                        component.path.clone(),
                        component.source_asset.clone(),
                        component.model_class.clone(),
                    )
                })
                .collect(),
            member_output_aliases,
            units,
            layout,
            communication_period_secs: network.communication_period_secs,
        })))
    }
}

/// Register an authored synthesizer under `name`, backed by hook `synth.<name>`.
///
/// The hook itself is registered by whatever compiled it — `lunco_hooks_rhai::register_rhai_hook`
/// for a rhai policy — so this crate needs no scripting dependency and any
/// language that implements [`lunco_hooks::ScriptHook`] can author one.
pub fn register_hook_synthesizer(registry: &mut SynthesizerRegistry, name: impl Into<String>) {
    let name = name.into();
    registry.register(HookSynthesizer {
        hook_id: format!("synth.{name}"),
        name,
    });
}

/// Remove a policy-owned hook synthesizer. A removed selector has no runtime
/// owner; selecting it therefore reports the missing registration instead of
/// silently restoring a compiled policy.
pub fn unregister_hook_synthesizer(registry: &mut SynthesizerRegistry, name: &str) {
    registry.0.remove(name);
}

fn hook_map_value<'a>(
    map: &'a [(String, lunco_hooks::HookValue)],
    key: &str,
) -> Option<&'a lunco_hooks::HookValue> {
    map.iter()
        .find_map(|(candidate, value)| (candidate == key).then_some(value))
}

fn hook_map_string(
    map: &[(String, lunco_hooks::HookValue)],
    key: &str,
    context: &str,
) -> Result<String, String> {
    hook_map_value(map, key)
        .and_then(lunco_hooks::HookValue::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{context} must contain a non-empty string `{key}`"))
}

fn hook_map_string_array(
    map: &[(String, lunco_hooks::HookValue)],
    key: &str,
    context: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = hook_map_value(map, key) else {
        return Ok(Vec::new());
    };
    let lunco_hooks::HookValue::Array(values) = value else {
        return Err(format!("{context}.{key} must be an array of strings"));
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{context}.{key}[{index}] must be a string"))
        })
        .collect()
}

fn parse_policy_source_roots(
    value: Option<&lunco_hooks::HookValue>,
    policy_name: &str,
) -> Result<BTreeSet<String>, String> {
    let Some(value) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` must return `source_roots`"
        ));
    };
    let lunco_hooks::HookValue::Array(values) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` returned `source_roots`, which must be an array of strings"
        ));
    };
    let roots: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                format!("synthesizer `{policy_name}` source_roots[{index}] must be a string")
            })
        })
        .collect::<Result<_, _>>()?;
    roots
        .into_iter()
        .map(|root| {
            if is_modelica_identifier(&root) {
                Ok(root)
            } else {
                Err(format!(
                    "synthesizer `{policy_name}` returned invalid source root `{root}`"
                ))
            }
        })
        .collect()
}

/// Parse the policy-owned telemetry promotion table. The policy must return the
/// table explicitly, even when it is empty; Rust only validates its references.
fn parse_policy_member_output_aliases(
    value: Option<&lunco_hooks::HookValue>,
    network: &DomainNetwork,
    classes: Option<&MemberClasses>,
    root: &str,
    policy_name: &str,
) -> Result<Vec<(String, String, String)>, String> {
    let Some(value) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` must return `member_output_aliases`"
        ));
    };
    let lunco_hooks::HookValue::Array(entries) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` returned `member_output_aliases`, which must be an array"
        ));
    };
    let known: BTreeSet<(String, String)> = generated_member_outputs(network, classes)?
        .into_iter()
        .map(|(member, output, _)| (member, output))
        .collect();
    let mut aliases = Vec::with_capacity(entries.len());
    let mut seen = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let context = format!("synthesizer `{policy_name}` member_output_aliases[{index}]");
        let lunco_hooks::HookValue::Map(map) = entry else {
            return Err(format!("{context} must be a map"));
        };
        let member = hook_map_string(map, "member_path", &context)?;
        let output = hook_map_string(map, "output", &context)?;
        let alias = hook_map_string(map, "alias", &context)?;
        if !known.contains(&(member.clone(), output.clone())) {
            return Err(format!(
                "{context} refers to `{member}.outputs:{output}`, which is not a declared member output in `{root}`"
            ));
        }
        if !is_modelica_identifier(&alias) {
            return Err(format!(
                "{context}.alias `{alias}` is not a valid Modelica identifier"
            ));
        }
        if network.inputs.contains(&alias) || network.outputs.contains_key(&alias) {
            return Err(format!(
                "{context}.alias `{alias}` collides with a root boundary port"
            ));
        }
        if !seen.insert(alias.clone()) {
            return Err(format!("{context}.alias `{alias}` is duplicated"));
        }
        aliases.push((member, output, alias));
    }
    Ok(aliases)
}

/// Parse a policy result and validate the root interface shared by every
/// generated Modelica synthesizer. Returning the AST keeps callers from
/// parsing the same generated source twice before they inspect policy-specific
/// structure.
fn parse_validated_root_interface(
    source: &str,
    model_name: &str,
    inputs: &BTreeSet<String>,
    outputs: &BTreeSet<String>,
    aliases: &[(String, String, String)],
) -> Result<StoredDefinition, String> {
    let ast = lunco_modelica_ast::parse_to_ast(source, "generated-policy.mo")
        .map_err(|error| format!("strict Modelica parse failed: {error:?}"))?;
    let root = lunco_modelica_index::class_lookup::find_class_by_qualified_name(&ast, model_name)
        .ok_or_else(|| format!("root class `{model_name}` is missing"))?;

    let mut expected_root_outputs = outputs.clone();
    expected_root_outputs.extend(aliases.iter().map(|(_, _, alias)| alias.clone()));
    for component in root.components.values() {
        match &component.causality {
            Causality::Input(_) if !inputs.contains(component.name.as_str()) => {
                return Err(format!(
                    "root declares undeclared boundary input `{}`",
                    component.name
                ));
            }
            Causality::Output(_) if !expected_root_outputs.contains(component.name.as_str()) => {
                return Err(format!(
                    "root declares undeclared boundary output `{}`",
                    component.name
                ));
            }
            _ => {}
        }
    }

    for input in inputs {
        let Some(component) = root.components.get(input) else {
            return Err(format!("root boundary input `{input}` is missing"));
        };
        if !matches!(component.causality, Causality::Input(_)) {
            return Err(format!("root boundary `{input}` is not declared as input"));
        }
    }
    for output in outputs {
        let Some(component) = root.components.get(output) else {
            return Err(format!("root boundary output `{output}` is missing"));
        };
        if !matches!(component.causality, Causality::Output(_)) {
            return Err(format!(
                "root boundary `{output}` is not declared as output"
            ));
        }
    }
    for (_, _, alias) in aliases {
        let Some(component) = root.components.get(alias) else {
            return Err(format!(
                "promoted output `{alias}` is missing from the root"
            ));
        };
        if !matches!(component.causality, Causality::Output(_)) {
            return Err(format!(
                "promoted output `{alias}` is not declared as output"
            ));
        }
    }
    Ok(ast)
}

/// Validate the policy's actual Modelica source against the Rust-owned graph
/// facts. Parsing only is insufficient: a policy can return a syntactically
/// valid empty model while the runtime later falls back to an invented class
/// name or silently loses every member. This validator is intentionally an AST
/// mechanism, not a knowledge of the shipped emitter, so future Rhai policies
/// can change layout, equations, and partition without Rust changes.
fn validate_generated_source(
    source: &str,
    model_name: &str,
    network: &DomainNetwork,
    units: &[SynthesisUnit],
    aliases: &[(String, String, String)],
) -> Result<ModelInterface, String> {
    let outputs: BTreeSet<String> = network.outputs.keys().cloned().collect();
    let ast =
        parse_validated_root_interface(source, model_name, &network.inputs, &outputs, aliases)?;
    let root = lunco_modelica_index::class_lookup::find_class_by_qualified_name(&ast, model_name)
        .ok_or_else(|| format!("root class `{model_name}` is missing"))?;

    let expected_members: BTreeMap<String, String> = network
        .components
        .iter()
        .map(|component| {
            instance_identifier(&network.root, &component.path)
                .map(|instance| (instance, component.model_class.clone()))
        })
        .collect::<Result<_, _>>()?;
    let expected_unit_instances: BTreeSet<String> =
        units.iter().map(|unit| unit.instance.clone()).collect();
    if expected_unit_instances.len() != units.len() {
        return Err("generated unit instances must be unique".into());
    }
    for unit in units {
        let instance = &unit.instance;
        if network.inputs.contains(instance)
            || outputs.contains(instance)
            || aliases.iter().any(|(_, _, alias)| alias == instance)
        {
            return Err(format!(
                "generated unit instance `{instance}` collides with a root interface name"
            ));
        }
        let Some(component) = root.components.get(instance) else {
            return Err(format!("root unit instance `{instance}` is missing"));
        };
        if component.type_name.to_string() != unit.name {
            return Err(format!(
                "root unit `{instance}` has type `{}`, expected `{}`",
                component.type_name, unit.name
            ));
        }
    }
    for component in root.components.values() {
        let name = component.name.as_str();
        if expected_members.contains_key(name)
            || expected_members
                .values()
                .any(|class| class == &component.type_name.to_string())
        {
            return Err(format!(
                "root directly declares native member `{name}`; members must live inside generated units"
            ));
        }
        if component.type_name.to_string().starts_with("Unit_")
            && !expected_unit_instances.contains(name)
        {
            return Err(format!("root contains undeclared generated unit `{name}`"));
        }
    }

    for unit in units {
        let class =
            lunco_modelica_index::class_lookup::find_class_by_qualified_name(&ast, &unit.name)
                .ok_or_else(|| format!("generated unit class `{}` is missing", unit.name))?;
        let mut expected_unit_outputs = unit.outputs.clone();
        expected_unit_outputs.extend(
            aliases
                .iter()
                .filter(|(member, _, _)| unit.component_paths.iter().any(|path| path == member))
                .map(|(_, _, alias)| alias.clone()),
        );
        for component in class.components.values() {
            match &component.causality {
                Causality::Input(_) if !unit.inputs.contains(component.name.as_str()) => {
                    return Err(format!(
                        "unit `{}` declares undeclared boundary input `{}`",
                        unit.name, component.name
                    ));
                }
                Causality::Output(_)
                    if !expected_unit_outputs.contains(component.name.as_str()) =>
                {
                    return Err(format!(
                        "unit `{}` declares undeclared boundary output `{}`",
                        unit.name, component.name
                    ));
                }
                _ => {}
            }
        }
        for input in &unit.inputs {
            let Some(component) = class.components.get(input) else {
                return Err(format!(
                    "unit `{}` is missing boundary input `{input}`",
                    unit.name
                ));
            };
            if !matches!(component.causality, Causality::Input(_)) {
                return Err(format!(
                    "unit `{}` boundary `{input}` is not declared as input",
                    unit.name
                ));
            }
        }
        for output in &unit.outputs {
            let Some(component) = class.components.get(output) else {
                return Err(format!(
                    "unit `{}` is missing boundary output `{output}`",
                    unit.name
                ));
            };
            if !matches!(component.causality, Causality::Output(_)) {
                return Err(format!(
                    "unit `{}` boundary `{output}` is not declared as output",
                    unit.name
                ));
            }
        }
        for member_path in &unit.component_paths {
            let instance = instance_identifier(&network.root, member_path)?;
            let expected_type = network
                .components
                .iter()
                .find(|component| component.path == *member_path)
                .map(|component| component.model_class.as_str())
                .ok_or_else(|| {
                    format!(
                        "unit `{}` references unknown member `{member_path}`",
                        unit.name
                    )
                })?;
            let Some(component) = class.components.get(&instance) else {
                return Err(format!(
                    "unit `{}` is missing member instance `{instance}`",
                    unit.name
                ));
            };
            if component.type_name.to_string() != expected_type {
                return Err(format!(
                    "unit `{}` member `{instance}` has type `{}`, expected `{expected_type}`",
                    unit.name, component.type_name
                ));
            }
        }
        let expected_instances: BTreeSet<String> = unit
            .component_paths
            .iter()
            .map(|path| instance_identifier(&network.root, path))
            .collect::<Result<_, _>>()?;
        let native_types: BTreeSet<String> = network
            .components
            .iter()
            .map(|component| component.model_class.clone())
            .collect();
        for component in class.components.values() {
            let is_native = native_types.contains(&component.type_name.to_string());
            if is_native && !expected_instances.contains(&component.name) {
                return Err(format!(
                    "unit `{}` contains unassigned native member `{}`",
                    unit.name, component.name
                ));
            }
        }
        for (member, _, alias) in aliases {
            if !unit.component_paths.iter().any(|path| path == member) {
                continue;
            }
            let Some(component) = class.components.get(alias) else {
                return Err(format!(
                    "unit `{}` is missing promoted output `{alias}` for member `{member}`",
                    unit.name
                ));
            };
            if !matches!(component.causality, Causality::Output(_)) {
                return Err(format!(
                    "unit `{}` promoted output `{alias}` is not declared as output",
                    unit.name
                ));
            }
        }
    }
    Ok(parse_model_interface_from_ast(&ast))
}

/// Read a policy-owned unit partition and prove that it is only rearranging
/// the composed graph. USD facts stay authoritative for membership and public
/// boundaries; Rhai chooses how those members are merged into Modelica units.
fn parse_policy_units(
    value: Option<&lunco_hooks::HookValue>,
    network: &DomainNetwork,
    root: &str,
    policy_name: &str,
) -> Result<Vec<SynthesisUnit>, String> {
    let Some(value) = value else {
        return Err(format!("synthesizer `{policy_name}` must return `units`"));
    };
    let lunco_hooks::HookValue::Array(raw_units) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` returned `units`, which must be an array"
        ));
    };
    let known_components: BTreeSet<String> = network
        .components
        .iter()
        .map(|component| component.path.clone())
        .collect();
    let known_inputs = &network.inputs;
    let known_outputs: BTreeSet<_> = network.outputs.keys().map(String::as_str).collect();
    let mut seen_components = BTreeSet::new();
    let mut seen_names = BTreeSet::new();
    let mut seen_instances = BTreeSet::new();
    let mut units = Vec::with_capacity(raw_units.len());

    for (index, raw_unit) in raw_units.iter().enumerate() {
        let context = format!("synthesizer `{policy_name}` units[{index}]");
        let lunco_hooks::HookValue::Map(map) = raw_unit else {
            return Err(format!("{context} must be a map"));
        };
        let name = hook_map_string(map, "name", &context)?;
        if !is_modelica_identifier(&name) {
            return Err(format!(
                "{context}.name `{name}` is not a valid Modelica identifier"
            ));
        }
        if !seen_names.insert(name.clone()) {
            return Err(format!("{context}.name `{name}` is duplicated"));
        }
        let instance = hook_map_string(map, "instance", &context)?;
        if !is_modelica_identifier(&instance) {
            return Err(format!(
                "{context}.instance `{instance}` is not a valid Modelica identifier"
            ));
        }
        if !seen_instances.insert(instance.clone()) {
            return Err(format!("{context}.instance `{instance}` is duplicated"));
        }
        if known_inputs.contains(&instance) || known_outputs.contains(instance.as_str()) {
            return Err(format!(
                "{context}.instance `{instance}` collides with a network boundary name"
            ));
        }
        let component_paths = hook_map_string_array(map, "components", &context)?;
        if component_paths.is_empty() {
            return Err(format!("{context}.components must not be empty"));
        }
        for path in &component_paths {
            if !known_components.contains(path) {
                return Err(format!(
                    "{context}.components contains `{path}`, which is not in `{root}`"
                ));
            }
            if !seen_components.insert(path.clone()) {
                return Err(format!(
                    "component `{path}` occurs in more than one policy unit"
                ));
            }
        }
        let inputs = hook_map_string_array(map, "inputs", &context)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for input in &inputs {
            if !known_inputs.contains(input) {
                return Err(format!(
                    "{context}.inputs contains `{input}`, which is not a network boundary input"
                ));
            }
        }
        let outputs = hook_map_string_array(map, "outputs", &context)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for output in &outputs {
            if !known_outputs.contains(output.as_str()) {
                return Err(format!(
                    "{context}.outputs contains `{output}`, which is not a network boundary output"
                ));
            }
        }
        units.push(SynthesisUnit {
            name,
            instance,
            component_paths,
            inputs,
            outputs,
        });
    }

    if seen_components != known_components {
        let missing = known_components
            .difference(&seen_components)
            .map(String::as_str)
            .collect::<Vec<_>>();
        return Err(format!(
            "synthesizer `{policy_name}` units do not cover every composed component; missing: {}",
            missing.join(", ")
        ));
    }
    if units.is_empty() {
        return Err(format!(
            "synthesizer `{policy_name}` returned an empty unit partition for `{root}`"
        ));
    }
    Ok(units)
}

fn parse_policy_coordinate(
    map: &[(String, lunco_hooks::HookValue)],
    key: &str,
    context: &str,
) -> Result<i32, String> {
    let value = hook_map_value(map, key)
        .and_then(lunco_hooks::HookValue::as_i64)
        .ok_or_else(|| format!("{context} must contain integer `{key}`"))?;
    i32::try_from(value)
        .map_err(|_| format!("{context}.{key} is outside Modelica coordinate range"))
}

/// Read the policy-owned unit/member diagram placements. Both sections and
/// every placement are required; Rust validates the result but never fills in
/// omitted coordinates from a second presentation policy.
pub(super) fn parse_policy_layout(
    value: Option<&lunco_hooks::HookValue>,
    network: &DomainNetwork,
    units: &[SynthesisUnit],
    root: &str,
    policy_name: &str,
) -> Result<SynthesisLayout, String> {
    let Some(value) = value else {
        return Err(format!("synthesizer `{policy_name}` must return `layout`"));
    };
    let lunco_hooks::HookValue::Map(map) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` returned `layout`, which must be a map"
        ));
    };
    let known_units: BTreeSet<_> = units.iter().map(|unit| unit.name.as_str()).collect();
    let known_members: BTreeSet<_> = network
        .components
        .iter()
        .map(|component| component.path.as_str())
        .collect();

    let mut layout = SynthesisLayout::default();
    for (section, key, known, target) in [
        ("unit", "units", known_units, &mut layout.unit_positions),
        (
            "member",
            "members",
            known_members,
            &mut layout.member_positions,
        ),
    ] {
        let Some(value) = hook_map_value(map, key) else {
            return Err(format!(
                "synthesizer `{policy_name}` layout must contain `{key}`"
            ));
        };
        let lunco_hooks::HookValue::Array(placements) = value else {
            return Err(format!(
                "synthesizer `{policy_name}` layout.{key} must be an array"
            ));
        };
        let mut provided = BTreeSet::new();
        for (index, placement) in placements.iter().enumerate() {
            let context = format!("synthesizer `{policy_name}` layout.{key}[{index}]");
            let lunco_hooks::HookValue::Map(placement) = placement else {
                return Err(format!("{context} must be a map"));
            };
            let identity_key = if section == "unit" { "name" } else { "path" };
            let identity = hook_map_string(placement, identity_key, &context)?;
            if !known.contains(identity.as_str()) {
                return Err(format!(
                    "{context}.{identity_key} `{identity}` is not part of `{root}`"
                ));
            }
            if !provided.insert(identity.clone()) {
                return Err(format!(
                    "{context}.{identity_key} `{identity}` is duplicated"
                ));
            }
            let x = parse_policy_coordinate(placement, "x", &context)?;
            let y = parse_policy_coordinate(placement, "y", &context)?;
            target.insert(identity, (x, y));
        }
        let provided: BTreeSet<_> = target.keys().map(String::as_str).collect();
        if provided != known {
            let missing = known.difference(&provided).copied().collect::<Vec<_>>();
            return Err(format!(
                "synthesizer `{policy_name}` layout.{key} is missing: {}",
                missing.join(", ")
            ));
        }
    }
    let mut occupied_units = BTreeMap::<(i32, i32), &str>::new();
    for (identity, position) in &layout.unit_positions {
        if let Some(previous) = occupied_units.insert(*position, identity.as_str()) {
            return Err(format!(
                "synthesizer `{policy_name}` layout.units places `{identity}` on top of `{previous}` at ({}, {})",
                position.0, position.1
            ));
        }
    }
    for unit in units {
        let mut occupied_members = BTreeMap::<(i32, i32), &str>::new();
        for identity in &unit.component_paths {
            let Some(position) = layout.member_positions.get(identity) else {
                return Err(format!(
                    "synthesizer `{policy_name}` layout.members is missing `{identity}` in unit `{}`",
                    unit.name
                ));
            };
            if let Some(previous) = occupied_members.insert(*position, identity.as_str()) {
                return Err(format!(
                    "synthesizer `{policy_name}` layout.members places `{identity}` on top of `{previous}` in unit `{}` at ({}, {})",
                    unit.name, position.0, position.1
                ));
            }
        }
    }
    Ok(layout)
}

/// The composed network, as a map an authored policy can read.
///
/// Deliberately the WHOLE graph, flat and self-describing: member identity,
/// class, constants, acausal edges, causal edges, and the wrapper boundary.
/// Unit facts contain only the USD-derived partition and boundary; generated
/// Modelica instance names and presentation layout are policy outputs. A
/// policy that needs another authored fact is a reason to extend this function
/// — not a reason for the policy to go read USD itself.
pub fn network_facts(
    network: &DomainNetwork,
    model_name: &str,
    classes: Option<&MemberClasses>,
) -> Result<lunco_hooks::HookValue, String> {
    use lunco_hooks::HookValue as H;
    let units = partition_network(network);
    let member_outputs = generated_member_outputs(network, classes)?;
    let source_roots: BTreeSet<String> = network
        .components
        .iter()
        .filter_map(|component| component.model_class.split('.').next())
        .map(str::to_string)
        .collect();
    let component_paths: BTreeSet<_> = network
        .components
        .iter()
        .map(|component| component.path.as_str())
        .collect();
    let boundary_prefix = format!("{}.inputs:", network.root);
    let mut connections = BTreeSet::new();
    let mut causal_links = BTreeSet::new();
    let mut boundary_links = BTreeSet::new();
    for component in &network.components {
        let target_instance = instance_identifier(&network.root, &component.path)?;
        for (connector, targets) in &component.connectors {
            for target in targets {
                let Some((target_path, target_connector)) = target.split_once(".connectors:")
                else {
                    continue;
                };
                if !component_paths.contains(target_path) {
                    continue;
                }
                let left = (
                    component.path.clone(),
                    connector.clone(),
                    target_path.to_string(),
                    target_connector.to_string(),
                );
                let right = (
                    target_path.to_string(),
                    target_connector.to_string(),
                    component.path.clone(),
                    connector.clone(),
                );
                connections.insert(left.min(right));
            }
        }
        for (input, target) in &component.inputs {
            if let Some((source_path, output)) = target.split_once(".outputs:") {
                if component_paths.contains(source_path) {
                    causal_links.insert((
                        source_path.to_string(),
                        output.to_string(),
                        component.path.clone(),
                        target_instance.clone(),
                        input.clone(),
                    ));
                    continue;
                }
            }
            if let Some(boundary) = target.strip_prefix(&boundary_prefix).or_else(|| {
                network
                    .input_sources
                    .iter()
                    .find_map(|(name, source)| (source == target).then_some(name.as_str()))
            }) {
                boundary_links.insert((
                    boundary.to_string(),
                    component.path.clone(),
                    target_instance.clone(),
                    input.clone(),
                ));
            }
        }
    }
    let connection_facts = connections
        .into_iter()
        .map(|(left_path, left_connector, right_path, right_connector)| {
            Ok(H::map([
                ("left_path", H::str(left_path.clone())),
                (
                    "left_instance",
                    H::str(instance_identifier(&network.root, &left_path)?),
                ),
                ("left_connector", H::str(left_connector)),
                ("right_path", H::str(right_path.clone())),
                (
                    "right_instance",
                    H::str(instance_identifier(&network.root, &right_path)?),
                ),
                ("right_connector", H::str(right_connector)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let causal_link_facts = causal_links
        .into_iter()
        .map(
            |(source_path, source_output, target_path, target_instance, target_input)| {
                Ok(H::map([
                    ("source_path", H::str(source_path.clone())),
                    (
                        "source_instance",
                        H::str(instance_identifier(&network.root, &source_path)?),
                    ),
                    ("source_output", H::str(source_output)),
                    ("target_path", H::str(target_path)),
                    ("target_instance", H::str(target_instance)),
                    ("target_input", H::str(target_input)),
                ]))
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    let boundary_link_facts = boundary_links
        .into_iter()
        .map(|(input, target_path, target_instance, target_input)| {
            H::map([
                ("input", H::str(input)),
                ("target_path", H::str(target_path)),
                ("target_instance", H::str(target_instance)),
                ("target_input", H::str(target_input)),
            ])
        })
        .collect::<Vec<_>>();
    let boundary_output_facts = network
        .outputs
        .iter()
        .map(|(name, target)| {
            let (source_path, source_output) = target.split_once(".outputs:").ok_or_else(|| {
                format!(
                    "network output `{name}` points to malformed target `{target}`; expected `.outputs:`"
                )
            })?;
            Ok(H::map([
                ("name", H::str(name.clone())),
                ("source_path", H::str(source_path)),
                (
                    "source_instance",
                    H::str(instance_identifier(&network.root, source_path)?),
                ),
                ("source_output", H::str(source_output)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let member_output_facts = member_outputs
        .iter()
        .map(|(member_path, output, alias)| {
            Ok(H::map([
                ("member_path", H::str(member_path.clone())),
                (
                    "member_instance",
                    H::str(instance_identifier(&network.root, member_path)?),
                ),
                ("output", H::str(output.clone())),
                ("alias", H::str(alias.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let components: Vec<H> = network
        .components
        .iter()
        .map(|component| {
            Ok(H::map([
                ("path", H::str(component.path.clone())),
                (
                    "instance",
                    H::str(instance_identifier(&network.root, &component.path)?),
                ),
                ("class", H::str(component.model_class.clone())),
                ("source_asset", H::str(component.source_asset.clone())),
                (
                    "constants",
                    H::Map(
                        component
                            .constants
                            .iter()
                            .map(|(name, value)| (name.clone(), H::Float(*value)))
                            .collect(),
                    ),
                ),
                (
                    "constant_modifications",
                    H::Array(
                        component
                            .constants
                            .iter()
                            .map(|(name, value)| {
                                H::map([
                                    ("name", H::str(name.clone())),
                                    ("value", H::Float(*value)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "connectors",
                    H::Map(
                        component
                            .connectors
                            .iter()
                            .map(|(name, targets)| {
                                (
                                    name.clone(),
                                    H::Array(targets.iter().cloned().map(H::str).collect()),
                                )
                            })
                            .collect(),
                    ),
                ),
                (
                    "declared_connectors",
                    H::Array(
                        component
                            .declared_connectors
                            .iter()
                            .cloned()
                            .map(H::str)
                            .collect(),
                    ),
                ),
                (
                    "inputs",
                    H::Map(
                        component
                            .inputs
                            .iter()
                            .map(|(name, target)| (name.clone(), H::str(target.clone())))
                            .collect(),
                    ),
                ),
                (
                    "declared_outputs",
                    H::Array(
                        component
                            .declared_outputs
                            .iter()
                            .cloned()
                            .map(H::str)
                            .collect(),
                    ),
                ),
                ("topology_role", H::str(component.topology_role.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(H::map([
        ("model_name", H::str(model_name.to_string())),
        ("root", H::str(network.root.clone())),
        (
            "source_roots",
            H::Array(source_roots.into_iter().map(H::str).collect()),
        ),
        ("components", H::Array(components)),
        (
            "inputs",
            H::Array(network.inputs.iter().cloned().map(H::str).collect()),
        ),
        (
            "input_sources",
            H::Map(
                network
                    .input_sources
                    .iter()
                    .map(|(name, source)| (name.clone(), H::str(source.clone())))
                    .collect(),
            ),
        ),
        (
            "outputs",
            H::Map(
                network
                    .outputs
                    .iter()
                    .map(|(name, target)| (name.clone(), H::str(target.clone())))
                    .collect(),
            ),
        ),
        ("connections", H::Array(connection_facts)),
        ("causal_links", H::Array(causal_link_facts)),
        ("boundary_links", H::Array(boundary_link_facts)),
        ("boundary_outputs", H::Array(boundary_output_facts)),
        ("member_outputs", H::Array(member_output_facts)),
        (
            "units",
            H::Array(
                units
                    .into_iter()
                    .map(|unit| {
                        H::map([
                            ("name", H::str(unit.name)),
                            (
                                "components",
                                H::Array(unit.component_paths.into_iter().map(H::str).collect()),
                            ),
                            (
                                "inputs",
                                H::Array(unit.inputs.into_iter().map(H::str).collect()),
                            ),
                            (
                                "outputs",
                                H::Array(unit.outputs.into_iter().map(H::str).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ]))
}

/// Partition a composed network once, at the synthesizer boundary.
///
/// Acausal connector edges and internal causal output-to-input edges both keep
/// components in one composite unit. Boundary connections do not: they are
/// the public FMI/SSP-style interface of the containing network root. The
/// returned order and generated names are stable across runs and independent
/// of USD collection ordering.
pub(super) fn partition_network(network: &DomainNetwork) -> Vec<NetworkUnitFact> {
    let paths: BTreeSet<String> = network
        .components
        .iter()
        .map(|component| component.path.clone())
        .collect();
    let mut graph = ProgramGraph::default();
    for path in &paths {
        graph.add_node(path.clone());
    }
    for component in &network.components {
        for target in component.connectors.values().flatten() {
            if let Some((target_prim, _)) = target.split_once(".connectors:") {
                if paths.contains(target_prim) {
                    graph.connect(component.path.clone(), target_prim.to_string());
                }
            }
        }
        for target in component.inputs.values() {
            if let Some((target_prim, _)) = target.split_once(".outputs:") {
                if paths.contains(target_prim) {
                    graph.connect(component.path.clone(), target_prim.to_string());
                }
            }
        }
    }

    let component_by_path: BTreeMap<_, _> = network
        .components
        .iter()
        .map(|component| (component.path.as_str(), component))
        .collect();
    graph
        .connected_components()
        .into_iter()
        .map(|component_paths| {
            let members: BTreeSet<_> = component_paths.iter().map(String::as_str).collect();
            let inputs = network
                .components
                .iter()
                .filter(|component| members.contains(component.path.as_str()))
                .flat_map(|component| component.inputs.values())
                .filter_map(|target| network_boundary_for_target(network, target))
                .collect();
            let outputs = network
                .outputs
                .iter()
                .filter_map(|(name, target)| {
                    let (target_prim, _) = target.split_once(".outputs:")?;
                    members.contains(target_prim).then(|| name.clone())
                })
                .collect();
            let first = component_paths
                .first()
                .expect("connected component always has a seed");
            let relative = first
                .strip_prefix(&network.root)
                .unwrap_or(first)
                .trim_matches('/');
            let name = format!("Unit_{}", modelica_path_identifier(relative));
            debug_assert!(component_paths
                .iter()
                .all(|path| component_by_path.contains_key(path.as_str())));
            NetworkUnitFact {
                name,
                component_paths,
                inputs,
                outputs,
            }
        })
        .collect()
}

fn network_boundary_for_target(network: &DomainNetwork, target: &str) -> Option<String> {
    let prefix = format!("{}.inputs:", network.root);
    target
        .strip_prefix(&prefix)
        .map(str::to_string)
        .or_else(|| {
            network
                .input_sources
                .iter()
                .find_map(|(boundary, source)| (source == target).then(|| boundary.clone()))
        })
}

/// Synthesize a normalized actuator command map from composed USD geometry.
///
/// This is deliberately a separate synthesizer from `acausal-network`: force
/// actuator prims are physical USD members, not Modelica component facets. The
/// authored geometry supplies each actuator's moment contribution and an
/// explicit relationship to the generated network's command output. Modelica
/// owns the runtime clamp and matrix operation. A rank-deficient actuator
/// arrangement is an authoring error, not a reason to silently select a
/// different allocation policy.
pub struct ActuatorWrenchSynthesizer;

impl DomainSynthesizer for ActuatorWrenchSynthesizer {
    fn name(&self) -> &str {
        ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER
    }

    fn synthesize(
        &self,
        view: &dyn ComposedReader,
        root: &SdfPath,
        model_name: &str,
        _ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>> {
        if !is_runtime_domain_network_root(view, root) {
            return Ok(SynthOutcome::NotMine);
        }

        let root_string = root.to_string();
        let members = view
            .collection_members(root, "components")
            .map_err(|error| {
                vec![DomainProjectionError {
                    path: root_string.clone(),
                    message: format!("could not read actuator collection: {error}"),
                }]
            })?;

        let mut actuators = BTreeMap::new();
        for path in members {
            if path.is_property_path() || path.is_prim_variant_selection_path() {
                continue;
            }
            let command_targets = view.rel_targets(&path, "lunco:forceActuator:commandSource");
            if command_targets.len() != 1 {
                return Err(vec![DomainProjectionError {
                    path: path.to_string(),
                    message: "actuator-wrench collection members must target exactly one \
                              scalar output with lunco:forceActuator:commandSource"
                        .into(),
                }]);
            }
            let command_target = &command_targets[0];
            let Some((command_root, command_property)) = command_target.split_property() else {
                return Err(vec![DomainProjectionError {
                    path: format!("{path}.lunco:forceActuator:commandSource"),
                    message: format!(
                        "command source `{command_target}` must target a scalar output property"
                    ),
                }]);
            };
            let Some(command) = command_property.strip_prefix("outputs:") else {
                return Err(vec![DomainProjectionError {
                    path: format!("{path}.lunco:forceActuator:commandSource"),
                    message: format!(
                        "command source `{command_target}` must target an `outputs:` property"
                    ),
                }]);
            };
            if command_root != *root || command.is_empty() || !is_modelica_identifier(command) {
                return Err(vec![DomainProjectionError {
                    path: format!("{path}.lunco:forceActuator:commandSource"),
                    message: format!(
                        "command source `{command_target}` must target a valid Modelica output \
                         on network root `{root}`"
                    ),
                }]);
            }
            let command = command.to_string();
            let Some(actuator) = lunco_usd_actuation::force_actuator_from_usd(view, &path) else {
                return Err(vec![DomainProjectionError {
                    path: path.to_string(),
                    message: "actuator-wrench member is not a valid force actuator with a \
                              rigid-body owner, finite direction, and positive maxForce"
                        .into(),
                }]);
            };
            if actuators
                .insert(command.clone(), (path.clone(), actuator))
                .is_some()
            {
                return Err(vec![DomainProjectionError {
                    path: format!("{root}.lunco:forceActuator:commandSource"),
                    message: format!("actuator output `{command}` is authored more than once"),
                }]);
            }
        }
        if actuators.is_empty() {
            return Err(vec![DomainProjectionError {
                path: root.to_string(),
                message: "actuator-wrench collection contains no force actuators".into(),
            }]);
        }
        let inputs: BTreeSet<String> = view
            .attr_names(root)
            .into_iter()
            .filter_map(|attr| attr.strip_prefix("inputs:").map(strip_connection_suffix))
            .filter(|name| !name.is_empty())
            .collect();
        let outputs: BTreeSet<String> = view
            .attr_names(root)
            .into_iter()
            .filter_map(|attr| attr.strip_prefix("outputs:").map(strip_connection_suffix))
            .filter(|name| !name.is_empty())
            .collect();
        for name in inputs.iter().chain(outputs.iter()) {
            if !is_modelica_identifier(name) {
                return Err(vec![DomainProjectionError {
                    path: root.to_string(),
                    message: format!("public port `{name}` is not a valid Modelica identifier"),
                }]);
            }
        }
        let actuator_outputs: BTreeSet<_> = actuators.keys().cloned().collect();
        if actuator_outputs != outputs {
            return Err(vec![DomainProjectionError {
                path: root.to_string(),
                message: format!(
                    "actuator command outputs {:?} do not match the authored network outputs {:?}",
                    actuator_outputs, outputs
                ),
            }]);
        }

        let columns: Vec<_> = actuators.values().map(|(_, actuator)| *actuator).collect();
        let (wrench_matrix, allocation_step) =
            actuator_wrench_matrix(&columns).map_err(|message| {
                vec![DomainProjectionError {
                    path: root_string.clone(),
                    message,
                }]
            })?;
        let component_paths = actuators
            .values()
            .map(|(path, _)| path.to_string())
            .collect::<Vec<_>>();
        let facts = lunco_hooks::HookValue::Map(vec![
            (
                "model_name".to_string(),
                lunco_hooks::HookValue::str(model_name),
            ),
            (
                "root".to_string(),
                lunco_hooks::HookValue::str(root_string.clone()),
            ),
            (
                "inputs".to_string(),
                lunco_hooks::HookValue::Array(
                    inputs
                        .iter()
                        .cloned()
                        .map(lunco_hooks::HookValue::str)
                        .collect(),
                ),
            ),
            (
                "outputs".to_string(),
                lunco_hooks::HookValue::Array(
                    outputs
                        .iter()
                        .cloned()
                        .map(lunco_hooks::HookValue::str)
                        .collect(),
                ),
            ),
            (
                "actuator_paths".to_string(),
                lunco_hooks::HookValue::Array(
                    component_paths
                        .iter()
                        .cloned()
                        .map(lunco_hooks::HookValue::str)
                        .collect(),
                ),
            ),
            (
                "wrench_matrix".to_string(),
                lunco_hooks::HookValue::Array(
                    (0..6)
                        .map(|row| {
                            lunco_hooks::HookValue::Array(
                                wrench_matrix
                                    .iter()
                                    .map(|column| lunco_hooks::HookValue::Float(column[row]))
                                    .collect(),
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "allocation_step".to_string(),
                lunco_hooks::HookValue::Float(allocation_step),
            ),
            (
                "actuator_count".to_string(),
                lunco_hooks::HookValue::Int(wrench_matrix.len() as i64),
            ),
        ]);
        let value = lunco_hooks::invoke("synth.actuator-wrench", &[facts]).ok_or_else(|| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message: "actuator-wrench is selected but its Rhai synthesis policy is not registered".into(),
            }]
        })?.map_err(|error| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message: format!("actuator-wrench synthesis policy failed: {}", error.0),
            }]
        })?;
        let lunco_hooks::HookValue::Map(map) = value else {
            return Err(vec![DomainProjectionError {
                path: root_string,
                message: "actuator-wrench synthesis policy must return a map with a Modelica `source` key".into(),
            }]);
        };
        let Some(source) = map
            .iter()
            .find_map(|(key, value)| (key == "source").then(|| value.as_str()))
            .flatten()
        else {
            return Err(vec![DomainProjectionError {
                path: root_string,
                message: "actuator-wrench synthesis policy returned no string `source` key".into(),
            }]);
        };
        let interface = parse_validated_root_interface(source, model_name, &inputs, &outputs, &[])
            .map_err(|message| {
                vec![DomainProjectionError {
                    path: root_string.clone(),
                    message: format!(
                        "actuator-wrench synthesis policy returned invalid Modelica: {message}"
                    ),
                }]
            })?;
        let source_roots = parse_policy_source_roots(
            hook_map_value(&map, "source_roots"),
            ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER,
        )
        .map_err(|message| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message,
            }]
        })?;
        // Force actuators are Avian/USD members, not Modelica component
        // members. Do not invent a generated unit class for them: the policy
        // emits one ordinary root model whose real Modelica child is the
        // allocator, and the source UI must not promise a drill-down class
        // that does not exist.
        Ok(SynthOutcome::Ready(Box::new(SynthesisPlan {
            source: source.to_string(),
            interface: parse_model_interface_from_ast(&interface),
            inputs,
            outputs,
            component_paths,
            source_roots,
            members: Vec::new(),
            member_output_aliases: Vec::new(),
            units: Vec::new(),
            layout: SynthesisLayout::default(),
            communication_period_secs: lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS,
        })))
    }
}

fn strip_connection_suffix(name: &str) -> String {
    name.strip_suffix(".connect").unwrap_or(name).to_string()
}

/// Return the authored six-component wrench matrix and a stable projected-
/// gradient step for the bounded actuator solve. Each column is one actuator's
/// maximum body force and torque, so the Modelica allocator solves the actual
/// one-sided least-squares problem instead of clamping a signed pseudo-inverse.
pub(super) fn actuator_wrench_matrix(
    actuators: &[lunco_cosim_core::ForceActuator],
) -> Result<(Vec<[f64; 6]>, f64), String> {
    let columns: Vec<[f64; 6]> = actuators
        .iter()
        .map(|actuator| {
            let direction = actuator.direction_local.normalize_or_zero().as_dvec3();
            let force = direction * actuator.max_force_n;
            let torque = actuator.local_position.as_dvec3().cross(direction) * actuator.max_force_n;
            [force.x, force.y, force.z, torque.x, torque.y, torque.z]
        })
        .collect();
    if columns.iter().flatten().any(|value| !value.is_finite()) {
        return Err("actuator-wrench geometry produced a non-finite torque column".into());
    }

    let gram_trace = columns
        .iter()
        .flatten()
        .map(|value| value * value)
        .sum::<f64>();
    if !gram_trace.is_finite() || gram_trace <= f64::EPSILON {
        return Err("actuator-wrench geometry has no finite physical wrench authority".into());
    }
    // `||B||_F²` bounds the largest eigenvalue of BᵀB. A 0.9 margin keeps the
    // fixed projected-gradient solve stable for every authored arrangement.
    Ok((columns, 0.9 / gram_trace))
}

/// Stable Modelica name for a causal output promoted from a generated member.
///
/// The wrapper is the only runtime solver participant, so member outputs that
/// remain visible in USD need a first-class boundary name. The prefix keeps
/// these derived names separate from authored network outputs; the escaped
/// instance identifier keeps the mapping injective for arbitrary USD paths.
pub(crate) fn generated_member_output_name(
    root: &str,
    member: &str,
    output: &str,
) -> Result<String, String> {
    Ok(format!(
        "__member_{}_{}",
        super::instance_identifier(root, member)?,
        modelica_identifier(output)
    ))
}

pub(super) fn generated_member_outputs(
    network: &DomainNetwork,
    classes: Option<&MemberClasses>,
) -> Result<Vec<(String, String, String)>, String> {
    let mut member_outputs = Vec::new();
    for component in &network.components {
        let modelica_outputs =
            classes.and_then(|classes| classes.output_names(&component.source_asset));
        for output in component
            .declared_outputs
            .iter()
            .filter(|output| modelica_outputs.is_none_or(|outputs| outputs.contains(*output)))
        {
            member_outputs.push((
                component.path.clone(),
                output.clone(),
                generated_member_output_name(&network.root, &component.path, output)?,
            ));
        }
    }
    Ok(member_outputs)
}
