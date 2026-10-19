//! Backend-neutral engineering contracts for map-backed scalar participants.

use bevy::prelude::*;
use lunco_engineering_values::{CoordinateFrameId, UnitReference};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
};

use crate::ports::{PortDirection, PortMetadata, PortTopologyRevision, PortTopologyState};

/// Engineering facts supplied by the participant's admitted source revision.
/// An omitted unit is explicitly unspecified, not a dimensionless assumption.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScalarPortContract {
    pub unit: Option<UnitReference>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub frame: Option<CoordinateFrameId>,
}

impl ScalarPortContract {
    pub fn metadata(
        &self,
        direction: PortDirection,
        source: &'static str,
        authority: &'static str,
    ) -> PortMetadata {
        PortMetadata::scalar(
            direction,
            self.unit.clone(),
            self.min,
            self.max,
            source,
            authority,
            true,
            self.frame.clone(),
        )
    }
}

/// Live scalar metadata, separate from changing numerical samples and source-
/// language provenance. Producers publish it with their admitted port surface;
/// the registry reads this same component for topology and write validation.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct ScalarPortContracts {
    entries: BTreeMap<String, ScalarPortContract>,
    topology_key: u64,
}

impl ScalarPortContracts {
    pub fn new(
        entries: impl IntoIterator<Item = (String, ScalarPortContract)>,
    ) -> Result<Self, String> {
        let mut contracts = Self::default();
        for (name, contract) in entries {
            if contracts.entries.contains_key(&name) {
                return Err(format!("duplicate scalar port contract '{name}'"));
            }
            contracts.set(name, contract)?;
        }
        Ok(contracts)
    }

    pub fn set(&mut self, name: String, contract: ScalarPortContract) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("scalar port contract requires a non-empty name".into());
        }
        contract
            .metadata(PortDirection::InOut, "scalar participant", "port owner")
            .validate_contract()
            .map_err(|error| format!("invalid contract for '{name}': {error}"))?;
        self.topology_key ^= contract_key(&name, &contract);
        if let Some(previous) = self.entries.insert(name.clone(), contract) {
            self.topology_key ^= contract_key(&name, &previous);
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ScalarPortContract> {
        self.entries.get(name)
    }

    /// Include conversion definitions and bounds, not just a unit's spelling.
    /// A source can revise its scale without changing the declared symbol.
    /// The owner updates this fingerprint on mutation, keeping live reads O(1).
    pub fn topology_key(&self) -> u64 {
        self.topology_key
    }
}

fn contract_key(name: &str, contract: &ScalarPortContract) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hash);
    contract.min.map(f64::to_bits).hash(&mut hash);
    contract.max.map(f64::to_bits).hash(&mut hash);
    contract.frame.hash(&mut hash);
    contract.unit.is_some().hash(&mut hash);
    if let Some(unit) = &contract.unit {
        unit.id().hash(&mut hash);
        unit.definition().is_some().hash(&mut hash);
        if let Some(definition) = unit.definition() {
            definition.dimension().hash(&mut hash);
            definition.angle_power().hash(&mut hash);
            definition.relative_quantity().hash(&mut hash);
            definition.scale_to_si().to_bits().hash(&mut hash);
            definition.offset_to_si().to_bits().hash(&mut hash);
            std::mem::discriminant(&definition.scale_exactness()).hash(&mut hash);
        }
    }
    hash.finish()
}

/// Install shared contract membership observers once across all backends.
pub struct ScalarPortContractsPlugin;
impl Plugin for ScalarPortContractsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PortTopologyRevision>()
            .init_resource::<PortTopologyState>()
            .add_observer(crate::ports::bump_port_topology_on_add::<ScalarPortContracts>)
            .add_observer(crate::ports::bump_port_topology_on_remove::<ScalarPortContracts>);
    }
}
