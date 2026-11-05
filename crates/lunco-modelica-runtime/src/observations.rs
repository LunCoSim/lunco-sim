//! Checked names are bound at admission; numerical publication uses retained slots.

use std::collections::BTreeSet;
use std::sync::Arc;

use bevy::prelude::*;
use lunco_core::ReflectReadOnly;
use rumoca_core::VarName;

type Values = indexmap::IndexMap<String, f64, bevy::platform::hash::FixedHasher>;

/// Compiler-owned input/parameter classification, independent of live values.
#[derive(Clone, Debug, Default)]
pub struct ModelicaVariableDeclarations {
    inputs: BTreeSet<String>,
    parameters: BTreeSet<String>,
    non_observables: bevy::platform::collections::HashSet<VarName>,
}

impl ModelicaVariableDeclarations {
    /// Bind checked declaration identities at source/interface admission.
    pub fn new(inputs: BTreeSet<String>, parameters: BTreeSet<String>) -> Self {
        let non_observables = inputs
            .iter()
            .chain(parameters.iter())
            .map(|name| VarName::intern(name))
            .collect();
        Self {
            inputs,
            parameters,
            non_observables,
        }
    }

    /// Text inventory for authoring and named control admission.
    pub fn inputs(&self) -> &BTreeSet<String> {
        &self.inputs
    }

    /// Text inventory for authoring and named parameter admission.
    pub fn parameters(&self) -> &BTreeSet<String> {
        &self.parameters
    }

    fn is_observable(&self, name: &VarName) -> bool {
        !self.non_observables.contains(name)
    }
}

/// Opaque process-local identity of one numerical inventory layout.
/// It is not a persisted variable identity or a portable wire reference.
#[derive(Clone, Debug)]
pub struct ModelicaValueLayout(Arc<()>);

impl Default for ModelicaValueLayout {
    fn default() -> Self {
        Self(Arc::new(()))
    }
}

impl PartialEq for ModelicaValueLayout {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ModelicaValueLayout {}

/// A variable resolved in one exact inventory layout.
#[derive(Clone, Debug)]
pub struct ModelicaVariableSlot {
    layout: ModelicaValueLayout,
    index: usize,
}

struct ObservationBinding {
    declarations: Arc<ModelicaVariableDeclarations>,
    rows: Vec<(VarName, Option<usize>)>,
}

struct OutputProjection {
    source: ModelicaValueLayout,
    destination: u64,
    rows: Vec<(ModelicaVariableSlot, u64)>,
}

/// Full accepted variable inventory, separate from declared co-simulation ports.
/// Only this owner mutates layout; reflection retains the named read surface.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, ReadOnly)]
pub struct ModelicaVariables {
    values: Values,
    #[reflect(ignore)]
    layout: ModelicaValueLayout,
    #[reflect(ignore)]
    binding: Option<ObservationBinding>,
    #[reflect(ignore)]
    projection: Option<OutputProjection>,
}

impl ModelicaVariables {
    /// Construct an explicitly supplied inventory at a named admission boundary.
    pub fn from_values(values: Values) -> Result<Self, String> {
        if let Some((name, value)) = values.iter().find(|(_, value)| !value.is_finite()) {
            return Err(format!("non-finite Modelica observation '{name}': {value}"));
        }
        Ok(Self {
            values,
            ..Default::default()
        })
    }

    /// Named inspection view; mutable numerical access remains with the owner.
    pub fn values(&self) -> &Values {
        &self.values
    }

    /// Retire all values and every resolved handle at lifecycle teardown/reset.
    pub fn clear(&mut self) {
        self.values.clear();
        self.layout = ModelicaValueLayout::default();
        self.binding = None;
        self.projection = None;
    }

    /// Admit an individual named value outside the numerical exchange path.
    pub fn insert_named(&mut self, name: String, value: f64) -> Result<(), String> {
        if !value.is_finite() {
            return Err(format!("non-finite Modelica observation '{name}': {value}"));
        }
        self.values.insert(name, value);
        self.layout = ModelicaValueLayout::default();
        self.binding = None;
        self.projection = None;
        Ok(())
    }

    /// Resolve a textual variable once when preparing a consumer projection.
    pub fn resolve(&self, name: &str) -> Option<ModelicaVariableSlot> {
        Some(ModelicaVariableSlot {
            layout: self.layout.clone(),
            index: self.values.get_index_of(name)?,
        })
    }

    /// Read a bound variable without hashing text or accepting another layout.
    pub fn read_slot(&self, slot: &ModelicaVariableSlot) -> Option<f64> {
        (slot.layout == self.layout)
            .then(|| self.values.get_index(slot.index).map(|(_, value)| *value))
            .flatten()
    }

    /// Current layout lease for consumers that prepare several bound reads.
    pub fn layout(&self) -> ModelicaValueLayout {
        self.layout.clone()
    }

    /// Whether the consumer must prepare its declared destination namespace.
    pub fn needs_output_binding(&self, target: &lunco_port_core::ports::ScalarPortMap) -> bool {
        !self.projection.as_ref().is_some_and(|projection| {
            projection.source == self.layout && projection.destination == target.topology_key()
        })
    }

    /// Publish into the consumer's declared scalar slots. USD/interface owners
    /// prepare the destination namespace; this mechanism only binds matching
    /// values. The sole live inventory and destination retain their own storage.
    pub fn publish_outputs(
        &mut self,
        target: &mut lunco_port_core::ports::ScalarPortMap,
    ) -> Result<bool, String> {
        let destination = target.topology_key();
        if self.needs_output_binding(target) {
            let rows = self
                .values
                .keys()
                .filter_map(|name| self.resolve(name).zip(target.resolve_slot(name)))
                .collect();
            self.projection = Some(OutputProjection {
                source: self.layout(),
                destination,
                rows,
            });
        }
        let projection = self
            .projection
            .as_ref()
            .ok_or("Modelica output projection was not prepared")?;
        // Validate the full batch before its first write. Borrowing both owners
        // exclusively keeps these slots and source values fixed through commit.
        for (source, slot) in &projection.rows {
            let value = self
                .read_slot(source)
                .ok_or("bound Modelica source slot is stale or absent")?;
            if !value.is_finite() || !target.contains_slot(*slot) {
                let (name, _) = self
                    .values
                    .get_index(source.index)
                    .expect("bound source slot was validated");
                return Err(format!("invalid bound Modelica output '{name}'"));
            }
        }
        let mut changed = false;
        for (source, slot) in &projection.rows {
            let value = self
                .read_slot(source)
                .expect("complete source batch was validated");
            changed |= target
                .set_slot_existing(*slot, value)
                .expect("complete destination batch was validated");
        }
        Ok(changed)
    }

    /// Admit a complete checked observation. Stable layouts compare canonical
    /// IDs, then publish by index; classification and text lookup occur at binding.
    /// Non-finite batches reject before changing any value or layout.
    pub fn admit_observation(
        &mut self,
        declarations: &Arc<ModelicaVariableDeclarations>,
        rows: &[(VarName, f64)],
    ) -> Result<bool, String> {
        if rows.is_empty() {
            return Ok(false);
        }
        if let Some((name, value)) = rows.iter().find(|(_, value)| !value.is_finite()) {
            return Err(format!("non-finite Modelica observation '{name}': {value}"));
        }
        let current = self.binding.as_ref().is_some_and(|binding| {
            Arc::ptr_eq(&binding.declarations, declarations)
                && binding.rows.len() == rows.len()
                && binding
                    .rows
                    .iter()
                    .zip(rows)
                    .all(|((name, _), (source, _))| name == source)
        });
        let mut changed = false;
        if !current {
            let mut position = 0;
            for (name, value) in rows {
                if !declarations.is_observable(name) {
                    continue;
                }
                let index = match self.values.get_index_of(name.as_str()) {
                    Some(index) => index,
                    None => {
                        self.values.insert(name.as_str().to_owned(), *value);
                        changed = true;
                        self.values.len() - 1
                    }
                };
                if position < self.values.len() {
                    self.values.swap_indices(index, position);
                }
                position += 1;
            }
            let mut bindings = Vec::with_capacity(rows.len());
            for (name, _) in rows {
                let slot =
                    if declarations.is_observable(name) {
                        Some(self.values.get_index_of(name.as_str()).ok_or_else(|| {
                            format!("bound Modelica observable '{name}' is absent")
                        })?)
                    } else {
                        None
                    };
                bindings.push((name.clone(), slot));
            }
            self.binding = Some(ObservationBinding {
                declarations: Arc::clone(declarations),
                rows: bindings,
            });
            self.layout = ModelicaValueLayout::default();
        }
        let binding = self
            .binding
            .as_ref()
            .ok_or("Modelica observation binding was not prepared")?;
        for ((_, slot), (_, value)) in binding.rows.iter().zip(rows) {
            if let Some(index) = slot {
                let (_, current) = self
                    .values
                    .get_index_mut(*index)
                    .expect("owner-bound variable index remains present");
                if current.to_bits() != value.to_bits() {
                    *current = *value;
                    changed = true;
                }
            }
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_bindings_preserve_values_and_reject_stale_or_invalid_rows() {
        assert!(
            ModelicaVariables::from_values([("invalid".into(), f64::NAN)].into_iter().collect())
                .is_err()
        );
        let declarations = Arc::new(ModelicaVariableDeclarations::new(
            ["command".into()].into(),
            ["gain".into()].into(),
        ));
        let mut values = ModelicaVariables::default();
        let row = |name: &str, value| (VarName::intern(name), value);
        let rows = [
            row("long.component.variable", 1.25),
            row("command", 2.0),
            row("gain", 3.0),
        ];
        assert!(values.admit_observation(&declarations, &rows).unwrap());
        let slot = values.resolve("long.component.variable").unwrap();
        let layout = values.layout();
        assert_eq!(values.values().len(), 1);
        assert!(!values.admit_observation(&declarations, &rows).unwrap());
        assert_eq!(values.layout(), layout);
        assert!(
            values
                .admit_observation(
                    &declarations,
                    &[
                        row("long.component.variable", 99.0),
                        row("invalid", f64::NAN)
                    ]
                )
                .is_err()
        );
        assert_eq!(values.read_slot(&slot), Some(1.25));
        assert_eq!(values.layout(), layout);
        assert!(
            values
                .admit_observation(
                    &declarations,
                    &[row("other", 4.0), row("command", 2.0), row("gain", 3.0)]
                )
                .unwrap()
        );
        assert_eq!(values.read_slot(&slot), None);
        assert_eq!(values.values()["long.component.variable"], 1.25);
        values
            .admit_observation(&declarations, &[row("other", 2.0), row("other", -0.0)])
            .unwrap();
        assert_eq!(values.values()["other"].to_bits(), (-0.0_f64).to_bits());
        assert_eq!(
            values
                .values()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["long.component.variable", "other"]
        );
        let slot = values.resolve("other").unwrap();
        values.clear();
        values
            .insert_named("other".into(), 7.0)
            .expect("finite admitted observation");
        assert_eq!(values.read_slot(&slot), None);
    }

    #[test]
    fn output_projection_rebinds_changed_sources_and_destination_slots() {
        let mut source = ModelicaVariables::from_values(
            [("x".into(), -0.0), ("internal".into(), 9.0)]
                .into_iter()
                .collect(),
        )
        .expect("finite admitted observation");
        let mut ports = lunco_port_core::ports::ScalarPortMap::default();
        ports.declare("x".into());
        assert!(source.publish_outputs(&mut ports).unwrap());
        assert_eq!(ports.get("x").unwrap().to_bits(), (-0.0_f64).to_bits());
        assert!(!ports.contains_key("internal"));
        assert!(!source.publish_outputs(&mut ports).unwrap());
        let layout = ports.topology_key();
        assert!(ports.invalidate_samples());
        assert_eq!(ports.topology_key(), layout);
        assert_eq!(ports.get("x"), None);
        assert!(!source.needs_output_binding(&ports));
        assert!(source.publish_outputs(&mut ports).unwrap());
        ports.remove("x");
        ports.declare("internal".into());
        assert!(source.publish_outputs(&mut ports).unwrap());
        assert_eq!(ports.get("internal"), Some(&9.0));
        source
            .insert_named("internal".into(), 2.0)
            .expect("finite admitted observation");
        assert!(source.publish_outputs(&mut ports).unwrap());
        assert_eq!(ports.get("internal"), Some(&2.0));
        assert!(source.insert_named("internal".into(), f64::NAN).is_err());
        assert!(!source.publish_outputs(&mut ports).unwrap());
        assert_eq!(ports.get("internal"), Some(&2.0));
    }
}
