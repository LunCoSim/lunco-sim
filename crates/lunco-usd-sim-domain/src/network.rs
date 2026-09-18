//! Composed USD network reading and validation.
//!
//! This module owns the structural network boundary: USD facts are decoded
//! into domain values and checked before synthesis or runtime projection.
//! Policy generation lives in [`super::synthesis`]; ECS lifecycle remains in
//! the parent module.

use super::*;

const COMMUNICATION_PERIOD_ATTR: &str = "lunco:program:communicationPeriod";

/// Aggregate member communication periods for one generated Modelica solver.
///
/// Periods are compared by their validated master-tick lattice index rather
/// than raw floating-point spelling, so `0.1` and an authored six-tick value
/// describe the same schedule. A generated wrapper has one scheduler; silently
/// selecting one member's period would make the other member's authored policy
/// false, so mixed periods are a terminal projection error.
pub(super) fn aggregate_communication_periods<I>(
    periods: I,
) -> Result<f64, Vec<DomainProjectionError>>
where
    I: IntoIterator<Item = Result<(String, f64), DomainProjectionError>>,
{
    let mut errors = Vec::new();
    let mut selected: Option<(String, u64, f64)> = None;
    for period in periods {
        let (path, period) = match period {
            Ok(period) => period,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let ticks = (period / lunco_core_runtime::SECS_PER_TICK).round() as u64;
        if let Some((selected_path, selected_ticks, selected_period)) = &selected {
            if *selected_ticks != ticks {
                errors.push(DomainProjectionError {
                    path: format!("{path}.{COMMUNICATION_PERIOD_ATTR}"),
                    message: format!(
                        "communication period {period:.9}s conflicts with {selected_period:.9}s authored at {selected_path}.{COMMUNICATION_PERIOD_ATTR}; one generated Modelica solver cannot honor mixed member schedules"
                    ),
                });
            }
        } else {
            selected = Some((path, ticks, period));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(selected
        .map(|(_, _, period)| period)
        .unwrap_or(lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS))
}

fn network_communication_period(
    view: &dyn ComposedReader,
    components: &[DomainComponent],
) -> Result<f64, Vec<DomainProjectionError>> {
    aggregate_communication_periods(components.iter().map(|component| {
        let path = SdfPath::new(&component.path).map_err(|error| DomainProjectionError {
            path: component.path.clone(),
            message: format!("invalid Modelica component path: {error}"),
        })?;
        let authored = view
            .attr_names(&path)
            .iter()
            .any(|name| name == COMMUNICATION_PERIOD_ATTR);
        let period = resolve_communication_period_secs(
            authored,
            view.real(&path, COMMUNICATION_PERIOD_ATTR),
        )
        .map_err(|reason| DomainProjectionError {
            path: format!("{}.{COMMUNICATION_PERIOD_ATTR}", component.path),
            message: format!("invalid Modelica communication period: {reason}"),
        })?;
        Ok((component.path.clone(), period))
    }))
}

/// Read one composed network root as a network, or say why it cannot be one.
///
/// `Ok(None)` = not a network root (or nothing solvable is left in it);
/// `Err` = authored opinions that would produce a model the compiler could only
/// reject, reported against the property that carries them.
///
/// Public because this is the layer worth testing against REAL composed USD:
/// every unit test below builds a `DomainNetwork` by hand, and the composition
/// arcs are exactly where this has broken in the field.
pub fn read_network(
    view: &dyn ComposedReader,
    root: &SdfPath,
    classes: &MemberClasses,
) -> Result<Option<DomainNetwork>, Vec<DomainProjectionError>> {
    let root_string = root.to_string();
    if !is_domain_network_root(view, root) {
        return Ok(None);
    }
    let member_paths = view
        .collection_members(root, "components")
        .map_err(|error| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message: format!("could not read component collection: {error}"),
            }]
        })?;
    let mut components = Vec::new();
    let mut extraction_errors = Vec::new();
    // Set when a member's class is not knowable yet — see `pending_sources`.
    let mut pending_sources = false;
    for path in member_paths {
        if path.is_property_path() || path.is_prim_variant_selection_path() {
            continue;
        }
        if !view.has_api_schema(&path, "LunCoProgramAPI") {
            // A member that lost `LunCoProgramAPI` through composition is not an
            // opinion about anything — it is almost always a reference arc that
            // failed to remap, and staying silent here is what makes the first
            // symptom a confusing boundary error about a prim the author can see
            // in their file. Name it where it happens.
            warn!(
                "[domain-projection] `{path}` is in collection `{root_string}/components` but \
                 applies no LunCoProgramAPI — it contributes nothing to the generated model. \
                 Check that the reference arc composing it survived."
            );
            continue;
        }
        // The source file's own `within` + class is the answer, once
        // `resolve_member_classes` has read it. A member with no verdict yet
        // leaves the whole network pending until the source declaration is
        // available.
        let source_ref = match modelica_source_ref(view, &path) {
            Ok(source_ref) => source_ref,
            Err(issue) => {
                extraction_errors.push(DomainProjectionError {
                    path: issue.property,
                    message: issue.message,
                });
                continue;
            }
        };
        let model_class = match classes.resolve(&source_ref.asset) {
            Ok(Some(class)) => class,
            Ok(None) => {
                pending_sources = true;
                continue;
            }
            Err(message) => {
                extraction_errors.push(DomainProjectionError {
                    path: format!("{path}.info:sourceAsset"),
                    message,
                });
                continue;
            }
        };
        let source_asset = source_ref.asset;
        let attrs = view.attr_names(&path);
        let mut constants = BTreeMap::new();
        let mut connectors = BTreeMap::new();
        let mut declared_connectors = BTreeSet::new();
        let mut inputs = BTreeMap::new();
        let mut declared_outputs = BTreeSet::new();
        let topology_role = if view.has_api_schema(&path, "LunCoModelicaTopologyAPI") {
            match view.text(&path, "lunco:modelica:topologyRole").as_deref() {
                Some(role @ ("source" | "storage" | "load" | "neutral")) => role.to_string(),
                Some(role) => {
                    extraction_errors.push(DomainProjectionError {
                        path: format!("{path}.lunco:modelica:topologyRole"),
                        message: format!(
                            "unsupported Modelica topology role `{role}`; expected source, storage, load, or neutral"
                        ),
                    });
                    "neutral".to_string()
                }
                None => {
                    extraction_errors.push(DomainProjectionError {
                        path: format!("{path}.lunco:modelica:topologyRole"),
                        message: "LunCoModelicaTopologyAPI is applied but its topology role is unauthored".into(),
                    });
                    "neutral".to_string()
                }
            }
        } else {
            "neutral".to_string()
        };
        for attr in attrs {
            if let Some(name) = attr.strip_prefix("connectors:") {
                let name = name.strip_suffix(".connect").unwrap_or(name);
                declared_connectors.insert(name.to_string());
                let targets: Vec<String> = view
                    .connections(&path, &attr)
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                if !targets.is_empty() {
                    connectors.insert(name.to_string(), targets);
                }
            } else if let Some(name) = attr.strip_prefix("inputs:") {
                let name = name.strip_suffix(".connect").unwrap_or(name);
                let targets = view.connections(&path, &attr);
                if targets.len() > 1 {
                    extraction_errors.push(DomainProjectionError {
                        path: format!("{path}.{attr}"),
                        message: "a scalar Modelica input must have at most one connection source"
                            .into(),
                    });
                } else if let Some(target) = targets.first() {
                    inputs.insert(name.to_string(), target.to_string());
                } else if let Some(value) = view.real(&path, &attr) {
                    // A non-finite opinion would be emitted verbatim (`NaN`,
                    // `inf`) and come back as a compiler error against generated
                    // source, blaming the model for the authoring.
                    if !value.is_finite() {
                        extraction_errors.push(DomainProjectionError {
                            path: format!("{path}.{attr}"),
                            message: format!(
                                "`{value}` is not a finite value; a generated Modelica \
                                 modification must be a real number"
                            ),
                        });
                        continue;
                    }
                    constants.insert(name.to_string(), value);
                } else {
                    extraction_errors.push(DomainProjectionError {
                        path: format!("{path}.{attr}"),
                        message:
                            "generated Modelica inputs must be scalar real values or connections"
                                .into(),
                    });
                }
            } else if let Some(name) = attr.strip_prefix("outputs:") {
                let name = name.strip_suffix(".connect").unwrap_or(name);
                declared_outputs.insert(name.to_string());
            }
        }
        components.push(DomainComponent {
            path: path.to_string(),
            source_asset,
            model_class,
            constants,
            connectors,
            declared_connectors,
            inputs,
            declared_outputs,
            topology_role,
        });
    }
    if !extraction_errors.is_empty() {
        return Err(extraction_errors);
    }
    if pending_sources {
        // The member set is INCOMPLETE while a class is unknown, so every
        // conclusion below — which parts are unwired, whether a boundary output
        // has a source — would be drawn from a partial network and reported as
        // an authoring error. Say "not yet" instead.
        return Ok(Some(DomainNetwork {
            root: root_string,
            components: Vec::new(),
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: true,
        }));
    }
    // A component with an acausal port but no authored edge is a legitimate
    // installed-but-unwired part may be valid in the authored assembly but has
    // no well-posed acausal network by itself. Omit it from this generated
    // island rather than rejecting unrelated connected equipment.
    // Causal-only components remain: they may be complete models without an
    // acausal connector at all.
    let omitted = retain_connected_acausal_components(&mut components);
    if components.is_empty() {
        return Ok(None);
    }
    let communication_period_secs = network_communication_period(view, &components)?;
    let attrs = view.attr_names(root);
    let authored_inputs: BTreeSet<_> = attrs
        .iter()
        .filter_map(|attr| {
            attr.strip_prefix("inputs:")
                .map(|name| name.strip_suffix(".connect").unwrap_or(name).to_string())
        })
        .collect();
    let mut authored_input_sources = BTreeMap::new();
    for attr in &attrs {
        let Some(name) = attr.strip_prefix("inputs:") else {
            continue;
        };
        let name = name.strip_suffix(".connect").unwrap_or(name);
        let targets = view.connections(root, attr);
        if targets.len() > 1 {
            extraction_errors.push(DomainProjectionError {
                path: format!("{root}.{attr}"),
                message: "a scalar network input must have at most one connection source".into(),
            });
        } else if let Some(target) = targets.first() {
            authored_input_sources.insert(name.to_string(), target.to_string());
        }
    }
    let internal_inputs: BTreeMap<String, String> = authored_inputs
        .iter()
        .filter_map(|name| {
            lunco_usd_bevy_core::program::internal_network_input_source(view, root, name)
                .map(|source| (name.clone(), source))
        })
        .collect();
    let internal_outputs: BTreeMap<String, String> = attrs
        .iter()
        .filter_map(|attr| {
            let name = attr
                .strip_prefix("outputs:")
                .map(|name| name.strip_suffix(".connect").unwrap_or(name))?;
            lunco_usd_bevy_core::program::network_member_output_source(view, root, name)
                .map(|source| (name.to_string(), source))
        })
        .collect();
    // Collapse a root input whose source is a member output into a direct
    // causal member edge. This keeps an authored drive-law forward internal to
    // the generated network instead of declaring the same identifier as both
    // a Modelica input and output.
    let root_input_prefix = format!("{root_string}.inputs:");
    for component in &mut components {
        for target in component.inputs.values_mut() {
            if let Some(name) = target.strip_prefix(&root_input_prefix) {
                if let Some(source) = internal_inputs.get(name) {
                    *target = source.clone();
                }
            } else if let Some(name) = target.strip_prefix(&format!("{root_string}.outputs:")) {
                if let Some(source) = internal_outputs.get(name) {
                    *target = source.clone();
                }
            }
        }
    }
    let inputs: BTreeSet<_> = authored_inputs
        .into_iter()
        .filter(|name| !internal_inputs.contains_key(name))
        .collect();
    let input_sources: BTreeMap<_, _> = authored_input_sources
        .into_iter()
        .filter(|(name, _)| !internal_inputs.contains_key(name))
        .collect();
    let mut outputs = BTreeMap::new();
    for attr in &attrs {
        let Some(name) = attr.strip_prefix("outputs:") else {
            continue;
        };
        // A USD prim may expose the same spelling in both namespaces. The
        // generated Modelica root cannot declare one identifier as input and
        // output, so a colliding output stays on the runtime actuator surface;
        // its promoted member alias is wired to that surface by the generic
        // USD connection pass.
        let name = name.strip_suffix(".connect").unwrap_or(name);
        if inputs.contains(name) {
            continue;
        }
        // A network root can also expose ordinary vehicle outputs such as
        // `drive_left`. Only an output sourced from a member in this root's
        // component collection is part of the generated Modelica interface.
        // The other outputs remain available to physics and control wiring.
        if !lunco_usd_bevy_core::program::is_network_boundary_output(view, root, attr) {
            continue;
        }
        let targets = view.connections(root, attr);
        if targets.len() != 1 {
            extraction_errors.push(DomainProjectionError {
                path: format!("{root}.{attr}"),
                message: "a network output must have exactly one component source".into(),
            });
            continue;
        }
        // A boundary output whose source was OMITTED above (an installed but
        // unwired part) drops with it. Rejecting the whole network would make
        // one incomplete reference arc hide every otherwise valid member. The
        // two policies have to agree — omitting a part means omitting what it
        // published.
        let source_prim = targets[0]
            .split_once(".outputs:")
            .map(|(prim, _)| prim.to_string())
            .unwrap_or_default();
        if omitted.contains(&source_prim) {
            warn!(
                "[domain-projection] `{root}.{attr}` publishes `{}`, which is installed but has \
                 no acausal connection, so it is not part of this generated network — the output \
                 is dropped. Wire its `connectors:*` to bring both back.",
                source_prim
            );
            continue;
        }
        outputs.insert(name.to_string(), targets[0].to_string());
    }
    if !extraction_errors.is_empty() {
        return Err(extraction_errors);
    }
    // A boundary input nothing consumes is authored intent that reaches no
    // equation: the wire into it lands, the value updates every tick, and the
    // DAE never reads it. Silent, and indistinguishable from a working feed.
    for input in &inputs {
        let boundary = format!("{root_string}.inputs:{input}");
        let consumed = components.iter().any(|component| {
            component
                .inputs
                .values()
                .any(|target| *target == boundary || input_sources.get(input) == Some(target))
        });
        if !consumed {
            warn!(
                "[domain-projection] `{boundary}` is declared on the network but no member \
                 consumes it — nothing in the generated model reads this input. Connect a \
                 member's `inputs:*` to it, or remove it."
            );
        }
    }
    let network = DomainNetwork {
        root: root_string,
        components,
        inputs,
        input_sources,
        outputs,
        communication_period_secs,
        pending_sources: false,
    };
    let mut errors = validate_network(&network);
    // Say WHY a causal source is missing when the answer is "it was installed
    // but never wired, so the island omitted it" — otherwise the only report is
    // `outside collection`, about a prim the author can see listed in their own
    // `collection:components:includes`.
    for error in &mut errors {
        if let Some(path) = omitted
            .iter()
            .find(|path| error.message.contains(path.as_str()))
        {
            error.message.push_str(&format!(
                " — `{path}` IS in the collection, but it declares an acausal connector that \
                 nothing connects to, so it is not part of the generated network. Wire its \
                 `connectors:*`."
            ));
        }
    }
    if errors.is_empty() {
        Ok(Some(network))
    } else {
        Err(errors)
    }
}

/// Validate that projection will preserve every authored network edge.
pub fn validate_network(network: &DomainNetwork) -> Vec<DomainProjectionError> {
    let mut errors = Vec::new();
    let components: BTreeMap<_, _> = network
        .components
        .iter()
        .map(|component| (component.path.as_str(), component))
        .collect();
    let boundary_sources: BTreeSet<_> =
        network.input_sources.values().map(String::as_str).collect();

    let mut boundaries_by_source = BTreeMap::<&str, Vec<&str>>::new();
    for (boundary, source) in &network.input_sources {
        boundaries_by_source
            .entry(source)
            .or_default()
            .push(boundary);
    }
    for (source, boundaries) in boundaries_by_source {
        if boundaries.len() > 1 {
            errors.push(DomainProjectionError {
                path: network.root.clone(),
                message: format!(
                    "network inputs {} resolve to the same composed source `{source}`; their authored boundary identity is ambiguous",
                    boundaries.join(", ")
                ),
            });
        }
    }

    let mut generated_names = BTreeMap::<String, String>::new();
    for component in &network.components {
        let generated = match instance_identifier(&network.root, &component.path) {
            Ok(generated) => generated,
            Err(message) => {
                errors.push(DomainProjectionError {
                    path: component.path.clone(),
                    message,
                });
                continue;
            }
        };
        if let Some(previous) = generated_names.insert(generated.clone(), component.path.clone()) {
            errors.push(DomainProjectionError {
                path: component.path.clone(),
                message: format!(
                    "component paths `{previous}` and `{}` produce the same Modelica identifier `{generated}`",
                    component.path
                ),
            });
        }
        for member in component
            .constants
            .keys()
            .chain(component.declared_connectors.iter())
            .chain(component.inputs.keys())
            .chain(component.declared_outputs.iter())
        {
            if !is_modelica_identifier(member) {
                errors.push(DomainProjectionError {
                    path: component.path.clone(),
                    message: format!("public member `{member}` is not a valid Modelica identifier"),
                });
            }
        }
    }

    for component in &network.components {
        for (connector, targets) in &component.connectors {
            for target in targets {
                let Some((target_prim, target_connector)) = target.split_once(".connectors:")
                else {
                    errors.push(DomainProjectionError {
                        path: format!("{}.connectors:{connector}", component.path),
                        message: format!("target `{target}` is not a connectors: property"),
                    });
                    continue;
                };
                let Some(target_component) = components.get(target_prim) else {
                    errors.push(DomainProjectionError {
                        path: format!("{}.connectors:{connector}", component.path),
                        message: format!(
                            "target component `{target_prim}` is outside collection `{}`",
                            network.root
                        ),
                    });
                    continue;
                };
                if !target_component
                    .declared_connectors
                    .contains(target_connector)
                {
                    errors.push(DomainProjectionError {
                        path: format!("{}.connectors:{connector}", component.path),
                        message: format!("target connector `{target}` does not exist"),
                    });
                }
            }
        }
        for (input, target) in &component.inputs {
            let boundary_prefix = format!("{}.inputs:", network.root);
            if let Some(boundary) = target.strip_prefix(&boundary_prefix) {
                if !network.inputs.contains(boundary) {
                    errors.push(DomainProjectionError {
                        path: format!("{}.inputs:{input}", component.path),
                        message: format!("network boundary input `{target}` does not exist"),
                    });
                }
                continue;
            }
            if boundary_sources.contains(target.as_str()) {
                continue;
            }
            let Some((target_prim, output)) = target.split_once(".outputs:") else {
                errors.push(DomainProjectionError {
                    path: format!("{}.inputs:{input}", component.path),
                    message: format!(
                        "target `{target}` must be a network inputs: property or component outputs: property"
                    ),
                });
                continue;
            };
            let Some(target_component) = components.get(target_prim) else {
                errors.push(DomainProjectionError {
                    path: format!("{}.inputs:{input}", component.path),
                    message: format!(
                        "causal source component `{target_prim}` is outside collection `{}`",
                        network.root
                    ),
                });
                continue;
            };
            if !target_component.declared_outputs.contains(output) {
                errors.push(DomainProjectionError {
                    path: format!("{}.inputs:{input}", component.path),
                    message: format!("causal source output `{target}` does not exist"),
                });
            }
        }
    }
    for (output, target) in &network.outputs {
        let Some((target_prim, member)) = target.split_once(".outputs:") else {
            errors.push(DomainProjectionError {
                path: format!("{}.outputs:{output}", network.root),
                message: format!("target `{target}` is not a component outputs: property"),
            });
            continue;
        };
        let Some(component) = components.get(target_prim) else {
            errors.push(DomainProjectionError {
                path: format!("{}.outputs:{output}", network.root),
                message: format!(
                    "output source component `{target_prim}` is outside collection `{}`",
                    network.root
                ),
            });
            continue;
        };
        if !component.declared_outputs.contains(member) {
            errors.push(DomainProjectionError {
                path: format!("{}.outputs:{output}", network.root),
                message: format!("output source `{target}` does not exist"),
            });
        }
    }

    errors
}

/// Drop only unconnected acausal component facets before emitting a DAE.
///
/// USD connections are directional authoring opinions whereas Modelica
/// `connect()` is symmetric, so retaining only the source side would be wrong:
/// every endpoint of an authored edge belongs to the generated island. A part
/// with an acausal connector and no edge at all has no solvable network context;
/// it remains a perfectly valid physical component, just not a member of this
/// runtime Modelica model.
///
/// Returns the paths it dropped, because everything the network published
/// THROUGH those parts has to drop with them (see the boundary-output handling
/// in [`read_network`]).
pub(super) fn retain_connected_acausal_components(
    components: &mut Vec<DomainComponent>,
) -> BTreeSet<String> {
    let connected: BTreeSet<String> = components
        .iter()
        .flat_map(|component| {
            component.connectors.values().flat_map(move |targets| {
                targets.iter().filter_map(move |target| {
                    target
                        .split_once(".connectors:")
                        .map(|(path, _)| [component.path.clone(), path.to_string()])
                })
            })
        })
        .flatten()
        .collect();
    let mut omitted = BTreeSet::new();
    components.retain(|component| {
        let keep = component.declared_connectors.is_empty() || connected.contains(&component.path);
        if !keep {
            omitted.insert(component.path.clone());
        }
        keep
    });
    omitted
}
