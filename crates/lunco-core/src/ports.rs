//! Co-simulation **port substrate** — the FMI/SSP scalar-exchange surface shared
//! by every participant so they all read/write exposed values through ONE path.
//!
//! A *port* is a named scalar (`f64`) on a participant entity. Modelica variables,
//! avian rigid-body state, joint angles, the SysML/hardware "nervous system"
//! ports, and (in future) an imported FMU all present as ports, so that wires,
//! the API (`ListPorts` / `GetPort` / `SetPorts`), the UI inspector, and every
//! scripting runtime (rhai/python) treat them uniformly — the FMI/SSP contract.
//!
//! ## Why this lives in `lunco-core`
//!
//! Ports are co-sim *substrate*, not an engine or API concern: the wire engine
//! (`lunco-cosim`) runs ON them, the API and scripts merely consume them. Putting
//! the registry here — below every participant — lets each crate **register** its
//! backends downward and **consume** the registry, with nobody depending "up".
//! This is what lets `lunco-scripting` reach ports even though `lunco-cosim`
//! (which owns the avian/joint/Modelica backends) depends ON scripting: both
//! depend down on this module. A future FMU-import or script-defined component is
//! just one more registered backend the wire engine then honours.
//!
//! ## Value model
//!
//! The wire currency is `f64` (continuous Real — what FMI-CS exchanges almost
//! everywhere), and it is the currency end to end: a [`Port`] holds one `f64`,
//! whatever the signal means. A backend whose own storage is narrower converts at
//! its boundary. We deliberately do **not** model `Bool`/`Enum`/`String` ports
//! until a concrete need appears.
//!
//! [`Port`]: crate::architecture::Port
//!
//! ## One registry, one discovery path and four thin access operations
//!
//! Every port-bearing backend is one [`PortBackend`] entry with an entity
//! enumerator and the access operations (list / read-output / read-input /
//! write-input), registered into the [`PortRegistry`] resource. Discovery and
//! access fold over the registered backends in order, so a new backend is added
//! by **registering** it — no consumer changes. Registration order *is*
//! resolution precedence (first match wins).

use bevy::prelude::*;
use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

use crate::InputPorts;

/// Durable invalidation generation for the shared runtime port surface.
///
/// Port identity is not a sampled value. A consumer may be hidden when an
/// owner changes its declared ports, so a transient event would be lossy. The
/// owner-side structural checks advance this monotonic generation after a
/// component's identity key changes; consumers retain the last generation they
/// projected and rebuild only when it differs. Live port values must not advance
/// it.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortTopologyRevision(pub u64);

impl PortTopologyRevision {
    /// Advance the invalidation generation after a declared port surface change.
    #[inline]
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

/// Last structural identity observed for each port-owner component and entity.
///
/// A port owner may update one ECS component for both live samples and declared
/// names. The owner-side change check records only the identity key, so a live
/// update still marks the component changed but does not invalidate the port
/// projection. Entity membership is handled by the lifecycle observers below.
#[derive(Resource, Default)]
pub struct PortTopologyState {
    keys: HashMap<(TypeId, Entity), u64>,
}

impl PortTopologyState {
    /// Record one structural key and report whether an already-observed key
    /// differs. The first observation seeds the cache; the lifecycle observer
    /// has already published the add as the structural invalidation.
    pub fn changed<T: 'static>(&mut self, entity: Entity, key: u64) -> bool {
        self.keys
            .insert((TypeId::of::<T>(), entity), key)
            .is_some_and(|previous| previous != key)
    }

    /// Forget a removed owner component so an entity id cannot retain a stale
    /// identity if that id is later reused by Bevy.
    pub fn forget<T: 'static>(&mut self, entity: Entity) {
        self.keys.remove(&(TypeId::of::<T>(), entity));
    }
}

/// Advance the port-surface generation when a component-owned backend candidate
/// is added. The owning plugin supplies the observer for the component types it
/// registers in its backend.
pub fn bump_port_topology_on_add<T: Component>(
    _trigger: On<Add, T>,
    mut revision: ResMut<PortTopologyRevision>,
) {
    revision.bump();
}

/// Advance the port-surface generation when a component-owned backend candidate
/// is removed. `On<Remove, T>` also covers entity teardown, so a hidden panel
/// cannot retain a despawned candidate until its next full sample.
pub fn bump_port_topology_on_remove<T: Component>(
    trigger: On<Remove, T>,
    mut revision: ResMut<PortTopologyRevision>,
    mut state: ResMut<PortTopologyState>,
) {
    state.forget::<T>(trigger.entity);
    revision.bump();
}

/// Direction (causality) of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Reflect)]
pub enum PortDirection {
    /// Port receives values from connections.
    In,
    /// Port provides values to connections.
    Out,
    /// Port can both receive and provide values.
    InOut,
}

/// Metadata describing the value and control contract of a discovered port.
///
/// The runtime currently exposes scalar `f64` values end to end. Keeping that
/// fact here, beside the registry that owns the port surface, gives native UI
/// and API consumers one authoritative place for units, validation, and
/// control ownership instead of making them infer policy from port names.
#[derive(Debug, Clone, PartialEq)]
pub struct PortMetadata {
    /// Stable value kind shown to generic consumers.
    pub value_type: &'static str,
    /// Authored/physical unit, when the owner knows one.
    pub unit: Option<String>,
    /// Inclusive lower validation bound, if one exists.
    pub min: Option<f64>,
    /// Inclusive upper validation bound, if one exists.
    pub max: Option<f64>,
    /// The subsystem that owns the value.
    pub source: String,
    /// The authority currently responsible for changing the value.
    pub authority: String,
    /// Whether the port owner accepts manual writes through `SetPorts`.
    pub writable: bool,
}

impl PortMetadata {
    /// Build metadata for the scalar port contract.
    pub fn scalar(
        direction: PortDirection,
        unit: Option<&str>,
        min: Option<f64>,
        max: Option<f64>,
        source: impl Into<String>,
        authority: impl Into<String>,
        writable: bool,
    ) -> Self {
        Self {
            value_type: "scalar",
            unit: unit.map(str::to_owned),
            min,
            max,
            source: source.into(),
            authority: authority.into(),
            writable: writable && matches!(direction, PortDirection::In | PortDirection::InOut),
        }
    }

    /// Metadata for a backend that has not supplied a richer description.
    pub fn unknown(direction: PortDirection) -> Self {
        Self::scalar(
            direction,
            None,
            None,
            None,
            "unknown backend",
            "backend owner",
            false,
        )
    }

    /// Validate a value before dispatching it to a writable port.
    pub fn validate(&self, value: f64) -> Result<(), String> {
        if !value.is_finite() {
            return Err("value must be finite".into());
        }
        if let Some(min) = self.min {
            if value < min {
                return Err(format!("value must be ≥ {min}"));
            }
        }
        if let Some(max) = self.max {
            if value > max {
                return Err(format!("value must be ≤ {max}"));
            }
        }
        Ok(())
    }
}

/// A discovered port with its live value and owner-provided metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct PortInfo {
    /// Port name — the key in the owning backend, or the canonical name for a
    /// single-value backend.
    pub name: String,
    /// Causality.
    pub direction: PortDirection,
    /// Snapshot of the current value.
    pub value: f64,
    /// Owner-provided type, unit, validation, source, and authority.
    pub metadata: PortMetadata,
}

/// A port owner as seen by the registry, including the precedence used when
/// more than one backend exposes the same public name.
#[derive(Debug, Clone, PartialEq)]
pub struct PortOwnerInfo {
    /// Public port name.
    pub name: String,
    /// Causality declared by the owner.
    pub direction: PortDirection,
    /// Registration order used by [`PortRegistry::write_port`] and the read
    /// methods. Lower values win.
    pub precedence: usize,
    /// Owner-provided domain/backend description.
    pub metadata: PortMetadata,
}

/// A process-local handle to one registered port backend.
///
/// It is captured by inspection projections so a later live-value read can
/// address the original owner directly instead of folding over every backend
/// again. Like [`ResolvedPort`], it is valid only for the current registry
/// ordering and must not be serialized.
#[derive(Clone, Copy, Debug, Default)]
pub struct PortHandle {
    backend: usize,
    slot: Option<u64>,
    reader: Option<fn(&World, Entity, u64) -> Option<f64>>,
}

impl PartialEq for PortHandle {
    fn eq(&self, other: &Self) -> bool {
        self.backend == other.backend && self.slot == other.slot
    }
}

impl Eq for PortHandle {}

/// The access side on which a duplicate public name collides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortCollisionDirection {
    /// More than one owner can receive a write.
    Input,
    /// More than one owner can provide a read value.
    Output,
    /// More than one bidirectional owner exposes the same name.
    InOut,
}

/// Multiple runtime owners of one public port name on one entity.
#[derive(Debug, Clone, PartialEq)]
pub struct PortCollision {
    /// Public port name.
    pub name: String,
    /// Access side that is ambiguous.
    pub direction: PortCollisionDirection,
    /// Owners in registry precedence order. The first entry is the owner that
    /// receives the corresponding registry operation.
    pub owners: Vec<PortOwnerInfo>,
}

/// A discovered port: identity, causality, current value.
///
/// Returned by [`PortRegistry::entity_ports`] for listing/introspection. The
/// `value` is a snapshot read at call time; live consumers read through the
/// registry directly.
#[derive(Debug, Clone)]
pub struct PortRef {
    /// Port name — the key in the owning backend, or the canonical name for a
    /// single-value backend.
    pub name: String,
    /// Causality.
    pub direction: PortDirection,
    /// Snapshot of the current value.
    pub value: f64,
}

/// Append every `(name, value)` in `map` as a [`PortRef`] of direction `dir`.
/// Helper for map-backed backends (e.g. Modelica `inputs`/`outputs`).
#[inline]
pub fn push_map(out: &mut Vec<PortRef>, map: &HashMap<String, f64>, dir: PortDirection) {
    for (name, value) in map {
        out.push(PortRef {
            name: name.clone(),
            direction: dir,
            value: *value,
        });
    }
}

/// Return an order-independent cache key for a backend's port-name set.
///
/// Values are deliberately excluded: a topology key changes only when port
/// identity changes, so live samples can be refreshed without rebuilding the
/// metadata rows.
pub fn port_name_set_key<'a, I>(names: I) -> u64
where
    I: IntoIterator<Item = &'a String>,
{
    let mut key = 0xcbf29ce484222325u64;
    let mut count = 0u64;
    for name in names {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut hasher);
        key = key.wrapping_add(hasher.finish().rotate_left(17));
        count = count.wrapping_add(1);
    }
    key ^ count.rotate_left(41)
}

/// Return an order-independent cache key for a named port-to-entity map.
///
/// Entity identity is part of a projected surface's structure: replacing the
/// endpoint behind an unchanged name must invalidate consumers just as adding
/// or removing the name does. Values are not part of this key.
pub fn port_entity_map_key<'a, I>(entries: I) -> u64
where
    I: IntoIterator<Item = (&'a String, &'a Entity)>,
{
    let mut key = 0xcbf29ce484222325u64;
    let mut count = 0u64;
    for (name, entity) in entries {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut hasher);
        entity.hash(&mut hasher);
        key = key.wrapping_add(hasher.finish().rotate_left(17));
        count = count.wrapping_add(1);
    }
    key ^ count.rotate_left(41)
}

/// One port-bearing backend, expressed as an entity enumerator plus operations
/// over `(World, Entity)`.
///
/// Ops are plain `fn` pointers (non-capturing closures), so a backend is `Copy`
/// and the registry is cheap to clone out of the world for `&mut World` access.
/// Each op is causality-correct: `read_output`/`read_input` see only the matching
/// direction; `write_input` accepts only an existing input slot (the strictness
/// that lets `propagate` report dangling wires). A single-value backend is
/// bidirectional — its one scalar *is* both its output and input.
#[derive(Clone, Copy)]
pub struct PortBackend {
    /// Append entities owned by this backend to `out`.
    ///
    /// This is the backend's authoritative discovery boundary. Consumers that
    /// need to inspect all ports must use [`PortRegistry::port_entities`]
    /// instead of scanning every ECS entity and probing every backend.
    pub list_entities: fn(&mut World, &mut Vec<Entity>),
    /// Return a key for this backend's port identity on `entity`.
    ///
    /// The key must ignore live values and change when the backend's port names
    /// or directions change. The owning plugin's change-filtered structural
    /// check publishes [`PortTopologyRevision`] when that key changes; the key
    /// is evaluated only on the changed-owner path. It lets consumers cache
    /// metadata while still observing dynamic authored surfaces.
    pub topology_key: fn(&World, Entity) -> u64,
    /// Append this backend's ports on `entity` (outputs then inputs) to `out`.
    pub list: fn(&World, Entity, &mut Vec<PortRef>),
    /// Describe one port returned by `list`, or `None` for the generic scalar
    /// fallback. The callback belongs to the backend owner so consumers never
    /// need a second type/name switch to reconstruct its contract.
    pub metadata: Option<fn(&World, Entity, &str, PortDirection) -> PortMetadata>,
    /// Read the **output** named `name`, or `None`.
    pub read_output: fn(&World, Entity, &str) -> Option<f64>,
    /// Read the **input** named `name`, or `None`.
    pub read_input: fn(&World, Entity, &str) -> Option<f64>,
    /// Write `value` to **input** `name`; `true` iff the port existed here.
    pub write_input: fn(&mut World, Entity, &str, f64) -> bool,

    // ── Optional resolve→slot fast path (the FMI valueReference model) ──────────
    //
    // A backend behind a multi-group presence scan (avian: up to 6 `world.get`
    // gating checks + a name scan per read) can expose these so a hot consumer
    // (the propagation master) resolves an endpoint to a process-local `slot`
    // ONCE (when wiring changes) and then exchanges by slot every tick — one
    // component read, no cross-backend fold, no group scan. `None` ⇒ no fast
    // path; the resolver falls back to the name-based ops above (correct for a
    // map-backed backend registered first, whose name read already costs one
    // `get`). See [`PortRegistry::resolve_output`].
    /// Resolve an **output** name to a backend-private `slot` (opaque `u64`),
    /// or `None` if this backend doesn't own it. Encodes causality: only an
    /// `Out`/`InOut` port resolves here.
    pub resolve_output: Option<fn(&World, Entity, &str) -> Option<u64>>,
    /// Resolve an **input** name to a backend-private `slot`, or `None`. Only an
    /// `In`/`InOut` port resolves here.
    pub resolve_input: Option<fn(&World, Entity, &str) -> Option<u64>>,
    /// Read the value at a previously-resolved `slot`. `None` if the slot no
    /// longer backs a live value (component removed) → the caller re-resolves or
    /// skips, exactly as an absent name read would.
    pub read_slot: Option<fn(&World, Entity, u64) -> Option<f64>>,
    /// Write `value` to a previously-resolved input `slot`; `false` if it no
    /// longer backs a live input.
    pub write_slot: Option<fn(&mut World, Entity, u64, f64) -> bool>,
}

/// A process-local resolved locator for one port on one backend — the FMI
/// *valueReference* analogue.
///
/// `slot` is an opaque `u64` the **owning backend** encodes and decodes; it is
/// meaningful only within this process/run and MUST NEVER be serialized or sent
/// on the wire (resolve fresh on every peer — slots are process-local, like FMI
/// value references). Produced by [`PortRegistry::resolve_output`] /
/// [`resolve_input`](PortRegistry::resolve_input), consumed by
/// [`read_resolved`](PortRegistry::read_resolved) /
/// [`write_resolved`](PortRegistry::write_resolved): the resolver folds over
/// backends ONCE, then the hot loop exchanges by slot with no re-scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedPort {
    /// Index of the owning backend in the registry (its registration order).
    backend: usize,
    /// Backend-private opaque locator.
    slot: u64,
}

/// The single registry of port-bearing backends — **the** read/write/list surface
/// for every exposed simulation value, whichever backend owns it.
///
/// Backends are registered (in dependency-correct order) by their owning crate's
/// plugin; the registry folds their discovery and access operations. Registration order is
/// resolution precedence (first match wins). `Clone` is cheap (a `Vec` of `Copy`
/// `fn` pointers) so a `&mut World` caller clones it out before writing.
#[derive(Resource, Clone)]
pub struct PortRegistry {
    backends: Vec<PortBackend>,
}

impl Default for PortRegistry {
    fn default() -> Self {
        // Every entity's declared `inputs:*` values use this substrate backend.
        // It is installed with the registry rather than by mobility, Modelica, or
        // a UI plugin, so physical, scripted and future participants share one
        // command/value spelling.
        Self {
            backends: vec![INPUT_PORTS_BACKEND],
        }
    }
}

/// The runtime storage behind a participant's declared `inputs:*` ports.
///
/// This is deliberately generic: it contains neither vehicle vocabulary nor
/// control/authority policy. A controller, wire, script, or network peer writes
/// these inputs through [`PortRegistry`] exactly as it writes a Modelica input.
const INPUT_PORTS_BACKEND: PortBackend = PortBackend {
    list_entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<InputPorts>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        world
            .get::<InputPorts>(entity)
            .map(|inputs| port_name_set_key(inputs.values.keys()))
            .unwrap_or(0)
    },
    list: |world, entity, out| {
        if let Some(inputs) = world.get::<InputPorts>(entity) {
            push_map(out, &inputs.values, PortDirection::In);
        }
    },
    metadata: Some(|_world, _entity, name, direction| {
        let (min, max) = match name {
            "throttle" | "steer" | "brake" => (Some(-1.0), Some(1.0)),
            "speed_boost" => (Some(0.0), Some(1.0)),
            _ => (None, None),
        };
        PortMetadata::scalar(
            direction,
            None,
            min,
            max,
            "control surface",
            "control owner",
            true,
        )
    }),
    read_output: |_world, _entity, _name| None,
    read_input: |world, entity, name| {
        world
            .get::<InputPorts>(entity)
            .and_then(|inputs| inputs.values.get(name).copied())
    },
    write_input: |world, entity, name, value| {
        let Some(mut inputs) = world.get_mut::<InputPorts>(entity) else {
            return false;
        };
        let Some(slot) = inputs.values.get_mut(name) else {
            return false;
        };
        *slot = value;
        true
    },
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

impl PortRegistry {
    /// Register a backend. Later registrations have lower precedence on name
    /// collisions. Call from a plugin `build`.
    ///
    /// Precedence follows plugin add-order, which no plugin controls, so a backend
    /// must claim a name only when it genuinely owns it. One that would otherwise
    /// have to guess needs an authoritative set to answer from instead.
    pub fn register(&mut self, backend: PortBackend) {
        self.backends.push(backend);
    }

    /// Enumerate every entity owned by at least one registered port backend.
    ///
    /// Entity discovery belongs to each backend because only the backend owner
    /// knows which component or authored surface makes an entity eligible. The
    /// registry only merges and deduplicates those authoritative candidate sets;
    /// it never infers ownership by probing the whole ECS world.
    pub fn port_entities(&self, world: &mut World) -> Vec<Entity> {
        self.port_entities_with_topology_keys(world)
            .into_iter()
            .map(|(entity, _)| entity)
            .collect()
    }

    /// Enumerate every backend-owned entity with its combined port-identity key.
    ///
    /// Each backend's key is evaluated alongside that backend's authoritative
    /// entity list. This avoids probing every registered backend for every
    /// candidate while retaining the same combined key for entities exposed by
    /// multiple owners.
    pub fn port_entities_with_topology_keys(&self, world: &mut World) -> Vec<(Entity, u64)> {
        let mut keys: HashMap<Entity, u64> = HashMap::new();
        for (backend_index, backend) in self.backends.iter().enumerate() {
            let mut owned = Vec::new();
            (backend.list_entities)(world, &mut owned);
            owned.sort_unstable_by_key(|entity| entity.to_bits());
            owned.dedup();
            for entity in owned {
                let owner_key = (backend.topology_key)(world, entity);
                let contribution = owner_key.rotate_left((backend_index % 63) as u32);
                keys.entry(entity)
                    .and_modify(|key| *key = key.wrapping_add(contribution))
                    .or_insert(contribution);
            }
        }
        let mut entities: Vec<_> = keys.into_iter().collect();
        entities.sort_unstable_by_key(|(entity, _)| entity.to_bits());
        entities
    }

    /// Return the combined port-identity key for one entity.
    ///
    /// Each backend owns the identity of its own surface; the registry only
    /// combines those owner keys for cache invalidation. Live port values are
    /// intentionally not part of this key.
    pub fn entity_port_topology_key(&self, world: &World, entity: Entity) -> u64 {
        self.backends
            .iter()
            .enumerate()
            .fold(0u64, |key, (index, backend)| {
                let owner_key = (backend.topology_key)(world, entity);
                key.wrapping_add(owner_key.rotate_left((index % 63) as u32))
            })
    }

    /// Enumerate every exposed port on `entity`, across all backends.
    /// The backbone of `ListPorts`.
    pub fn entity_ports(&self, world: &World, entity: Entity) -> Vec<PortRef> {
        let mut out = Vec::new();
        for backend in &self.backends {
            (backend.list)(world, entity, &mut out);
        }
        out
    }

    /// Enumerate every exposed port with owner-provided metadata.
    ///
    /// This is the native/API inspection surface. The older [`Self::entity_ports`]
    /// remains the compact value-only surface used by compatibility consumers;
    /// both are produced from the same backend list callbacks.
    pub fn entity_port_infos(&self, world: &World, entity: Entity) -> Vec<PortInfo> {
        self.entity_port_infos_with_handles(world, entity)
            .into_iter()
            .map(|(_, info)| info)
            .collect()
    }

    /// Enumerate every exposed port with its owning backend handle.
    ///
    /// This is the cache-friendly inspection surface. The handle lets a
    /// consumer refresh the value through the same owner without re-running
    /// registry precedence resolution on every sample.
    pub fn entity_port_infos_with_handles(
        &self,
        world: &World,
        entity: Entity,
    ) -> Vec<(PortHandle, PortInfo)> {
        let mut out = Vec::new();
        for (backend_index, backend) in self.backends.iter().enumerate() {
            let mut ports = Vec::new();
            (backend.list)(world, entity, &mut ports);
            out.extend(ports.into_iter().map(|port| {
                (
                    PortHandle {
                        backend: backend_index,
                        slot: match port.direction {
                            PortDirection::In => backend
                                .resolve_input
                                .and_then(|resolve| resolve(world, entity, &port.name)),
                            PortDirection::Out => backend
                                .resolve_output
                                .and_then(|resolve| resolve(world, entity, &port.name)),
                            PortDirection::InOut => backend
                                .resolve_output
                                .and_then(|resolve| resolve(world, entity, &port.name))
                                .or_else(|| {
                                    backend
                                        .resolve_input
                                        .and_then(|resolve| resolve(world, entity, &port.name))
                                }),
                        },
                        reader: backend.read_slot,
                    },
                    PortInfo {
                        metadata: backend
                            .metadata
                            .map(|describe| describe(world, entity, &port.name, port.direction))
                            .unwrap_or_else(|| PortMetadata::unknown(port.direction)),
                        name: port.name,
                        direction: port.direction,
                        value: port.value,
                    },
                )
            }));
        }
        out
    }

    /// Enumerate the distinct runtime owners of every public port on `entity`.
    ///
    /// A backend may expose one port through more than one inspection view. The
    /// registered backend is the owner identity, so repeated views from the
    /// same backend are collapsed while distinct backends remain visible.
    /// The returned precedence is the registry order consumed by the read/write
    /// methods; this is intentionally a diagnostic read and never changes
    /// routing.
    pub fn entity_port_owners(&self, world: &World, entity: Entity) -> Vec<PortOwnerInfo> {
        let mut out = Vec::new();
        for (precedence, backend) in self.backends.iter().enumerate() {
            let mut ports = Vec::new();
            (backend.list)(world, entity, &mut ports);
            let mut by_name = BTreeMap::new();
            for port in ports {
                by_name
                    .entry(port.name)
                    .and_modify(|direction| {
                        *direction = match (*direction, port.direction) {
                            (PortDirection::In, PortDirection::In)
                            | (PortDirection::Out, PortDirection::Out) => *direction,
                            _ => PortDirection::InOut,
                        };
                    })
                    .or_insert(port.direction);
            }
            for (name, direction) in by_name {
                let metadata = backend
                    .metadata
                    .map(|describe| describe(world, entity, &name, direction))
                    .unwrap_or_else(|| PortMetadata::unknown(direction));
                out.push(PortOwnerInfo {
                    name,
                    direction,
                    precedence,
                    metadata,
                });
            }
        }
        out.sort_by(|a, b| {
            a.precedence
                .cmp(&b.precedence)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.direction.cmp(&b.direction))
                .then_with(|| a.metadata.source.cmp(&b.metadata.source))
        });
        out
    }

    /// Find public names with more than one runtime owner on `entity`.
    ///
    /// `InOut` owners participate in both the input and output access sides
    /// when mixed with a one-way owner. Two or more `InOut` owners produce one
    /// `InOut` collision, avoiding duplicate warnings for the same ambiguity.
    pub fn entity_port_collisions(&self, world: &World, entity: Entity) -> Vec<PortCollision> {
        let mut by_name: BTreeMap<String, Vec<PortOwnerInfo>> = BTreeMap::new();
        for owner in self.entity_port_owners(world, entity) {
            by_name.entry(owner.name.clone()).or_default().push(owner);
        }

        let mut collisions = Vec::new();
        for (name, owners) in by_name {
            if owners.len() < 2 {
                continue;
            }
            let all_inout = owners
                .iter()
                .all(|owner| owner.direction == PortDirection::InOut);
            if all_inout {
                collisions.push(PortCollision {
                    name,
                    direction: PortCollisionDirection::InOut,
                    owners,
                });
                continue;
            }

            let inputs: Vec<_> = owners
                .iter()
                .filter(|owner| matches!(owner.direction, PortDirection::In | PortDirection::InOut))
                .cloned()
                .collect();
            if inputs.len() > 1 {
                collisions.push(PortCollision {
                    name: name.clone(),
                    direction: PortCollisionDirection::Input,
                    owners: inputs,
                });
            }

            let outputs: Vec<_> = owners
                .iter()
                .filter(|owner| {
                    matches!(owner.direction, PortDirection::Out | PortDirection::InOut)
                })
                .cloned()
                .collect();
            if outputs.len() > 1 {
                collisions.push(PortCollision {
                    name,
                    direction: PortCollisionDirection::Output,
                    owners: outputs,
                });
            }
        }
        collisions
    }

    /// Read the **output** named `name` on `entity` — the value a connection reads
    /// from its *source*. Searches outputs only (plus bidirectional single-value
    /// ports). Critical when a name exists as both input and output on one entity.
    pub fn read_output_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        self.backends
            .iter()
            .find_map(|b| (b.read_output)(world, entity, name))
    }

    /// Read the current value of port `name`, preferring an **output**, then
    /// falling back to an **input**. The backbone of `GetPort`.
    pub fn read_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        if let Some(v) = self.read_output_port(world, entity, name) {
            return Some(v);
        }
        self.backends
            .iter()
            .find_map(|b| (b.read_input)(world, entity, name))
    }

    /// Read the **input** value of port `name` — the commanded side, skipping
    /// outputs. Use where the input specifically is wanted (e.g. a joint's
    /// commanded motor setpoint vs its measured angle, both named `angle`).
    pub fn read_input_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        self.backends
            .iter()
            .find_map(|b| (b.read_input)(world, entity, name))
    }

    /// Read a port through the backend that produced its inspection row.
    ///
    /// This preserves per-owner rows when two backends expose the same public
    /// name and avoids a full registry fold in bounded-cadence inspectors.
    pub fn read_port_for_handle(
        &self,
        world: &World,
        handle: PortHandle,
        entity: Entity,
        name: &str,
        direction: PortDirection,
    ) -> Option<f64> {
        if let (Some(slot), Some(read)) = (handle.slot, handle.reader) {
            return read(world, entity, slot);
        }
        let backend = self.backends.get(handle.backend)?;
        match direction {
            PortDirection::In => (backend.read_input)(world, entity, name),
            PortDirection::Out => (backend.read_output)(world, entity, name),
            PortDirection::InOut => (backend.read_output)(world, entity, name)
                .or_else(|| (backend.read_input)(world, entity, name)),
        }
    }

    /// Whether an output port is declared by an owning backend, independently
    /// of whether it has produced a sample yet.
    ///
    /// Port identity and sample availability are different facts. A physics
    /// body owns its velocity port before the first writeback, and a Modelica
    /// participant owns a declared output while it is still compiling. Using
    /// `read_output_port` as an existence test turns both lifecycle states into
    /// a dangling-wire fault. Backends with a resolver (such as Avian) answer
    /// from their component contract; the ordinary list surface covers
    /// map-backed and authored-output participants.
    pub fn has_output_port(&self, world: &World, entity: Entity, name: &str) -> bool {
        self.backends.iter().any(|backend| {
            let mut ports = Vec::new();
            (backend.list)(world, entity, &mut ports);
            ports.iter().any(|port| {
                port.name == name
                    && matches!(port.direction, PortDirection::Out | PortDirection::InOut)
            }) || backend
                .resolve_output
                .is_some_and(|resolve| resolve(world, entity, name).is_some())
        })
    }

    /// Whether an input port is declared by an owning backend, independently of
    /// its current value.
    pub fn has_input_port(&self, world: &World, entity: Entity, name: &str) -> bool {
        self.backends.iter().any(|backend| {
            let mut ports = Vec::new();
            (backend.list)(world, entity, &mut ports);
            ports.iter().any(|port| {
                port.name == name
                    && matches!(port.direction, PortDirection::In | PortDirection::InOut)
            }) || backend
                .resolve_input
                .is_some_and(|resolve| resolve(world, entity, name).is_some())
        })
    }

    /// Write `value` to **input** port `name`. Returns `true` if such an input
    /// existed and was written. Strict: an undeclared name is rejected (never
    /// silently created) — what lets the API and propagation master report
    /// dangling wires. First backend that owns the port wins.
    pub fn write_port(&self, world: &mut World, entity: Entity, name: &str, value: f64) -> bool {
        for backend in &self.backends {
            if (backend.write_input)(world, entity, name, value) {
                return true;
            }
        }
        false
    }

    // ── Resolve→slot fast path ─────────────────────────────────────────────────

    /// Resolve an **output** endpoint `(entity, name)` to a [`ResolvedPort`] — a
    /// process-local handle a hot consumer caches once and reads by slot every
    /// tick. Returns `None` when the precedence-winning owner has no fast path
    /// (the caller then falls back to [`read_output_port`](Self::read_output_port),
    /// which honours the same precedence).
    ///
    /// **Precedence-correct:** walks backends in registration order and stops at
    /// the FIRST that owns `name` — so a lower-precedence fast-path backend can
    /// never shadow a higher-precedence name-only owner (e.g. an avian output
    /// can't win over a `SimComponent` output of the same name on one entity).
    /// The first owner is used whether via slot (it has a fast path) or via the
    /// name read (it doesn't → `None` here).
    pub fn resolve_output(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
    ) -> Option<ResolvedPort> {
        for (i, b) in self.backends.iter().enumerate() {
            // A readable output reveals ownership by name (outputs are readable).
            if (b.read_output)(world, entity, name).is_some() {
                let slot = (b.resolve_output?)(world, entity, name)?;
                return Some(ResolvedPort { backend: i, slot });
            }
        }
        None
    }

    /// Resolve an **input** endpoint to a [`ResolvedPort`] for writing. See
    /// [`resolve_output`](Self::resolve_output).
    ///
    /// Inputs may be **write-only** (an avian `force_y` reads `None`), so a
    /// readable-input probe can't detect every owner. We therefore stop at the
    /// first backend that owns the input *either* readably *or* via its own
    /// `resolve_input` (the authority for write ownership). Precedence holds for
    /// our registration order — the only readable-input backends (`SimComponent`,
    /// FSW) precede the write-only fast-path one (avian) — so a write-only port's
    /// name can't shadow an earlier readable input.
    pub fn resolve_input(&self, world: &World, entity: Entity, name: &str) -> Option<ResolvedPort> {
        for (i, b) in self.backends.iter().enumerate() {
            if (b.read_input)(world, entity, name).is_some() {
                // Earlier readable owner: use its slot if it has a fast path, else
                // `None` → the caller's name write hits it first (precedence held).
                let slot = (b.resolve_input?)(world, entity, name)?;
                return Some(ResolvedPort { backend: i, slot });
            }
            if let Some(resolve) = b.resolve_input {
                if let Some(slot) = resolve(world, entity, name) {
                    return Some(ResolvedPort { backend: i, slot });
                }
            }
        }
        None
    }

    /// Read the value at a resolved port. `None` if the slot no longer backs a
    /// live value (e.g. its component was removed) — the caller skips or
    /// re-resolves, exactly as an absent name read would contribute nothing.
    pub fn read_resolved(&self, world: &World, entity: Entity, r: ResolvedPort) -> Option<f64> {
        (self.backends[r.backend].read_slot?)(world, entity, r.slot)
    }

    /// Write to a resolved input port. `false` if the slot no longer backs a live
    /// input (component removed) — the caller reports the dangling target.
    pub fn write_resolved(
        &self,
        world: &mut World,
        entity: Entity,
        r: ResolvedPort,
        value: f64,
    ) -> bool {
        match self.backends[r.backend].write_slot {
            Some(write) => write(world, entity, r.slot, value),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PortBackend, PortCollisionDirection, PortDirection, PortMetadata, PortRef, PortRegistry,
    };
    use crate::InputPorts;
    use bevy::prelude::*;

    fn duplicate_input_list(_world: &World, _entity: Entity, out: &mut Vec<PortRef>) {
        out.push(PortRef {
            name: "release".into(),
            direction: PortDirection::In,
            value: 0.0,
        });
        out.push(PortRef {
            name: "release".into(),
            direction: PortDirection::In,
            value: 0.0,
        });
    }

    fn duplicate_inout_list(_world: &World, _entity: Entity, out: &mut Vec<PortRef>) {
        out.push(PortRef {
            name: "release".into(),
            direction: PortDirection::InOut,
            value: 0.0,
        });
        out.push(PortRef {
            name: "release".into(),
            direction: PortDirection::InOut,
            value: 0.0,
        });
    }

    fn owner_a_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(direction, None, None, None, "Modelica/OBC", "solver", true)
    }

    fn owner_b_metadata(
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
            "dock/runtime actuator",
            "actuator",
            true,
        )
    }

    fn write_input(_world: &mut World, _entity: Entity, _name: &str, _value: f64) -> bool {
        true
    }

    fn no_read(_world: &World, _entity: Entity, _name: &str) -> Option<f64> {
        None
    }

    const OWNER_A_INPUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        list: duplicate_input_list,
        metadata: Some(owner_a_metadata),
        read_output: no_read,
        read_input: no_read,
        write_input,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        write_slot: None,
    };

    const OWNER_B_INPUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        list: duplicate_input_list,
        metadata: Some(owner_b_metadata),
        read_output: no_read,
        read_input: no_read,
        write_input,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        write_slot: None,
    };

    const OWNER_A_INOUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        list: duplicate_inout_list,
        metadata: Some(owner_a_metadata),
        read_output: no_read,
        read_input: no_read,
        write_input,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        write_slot: None,
    };

    const OWNER_B_INOUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        list: duplicate_inout_list,
        metadata: Some(owner_b_metadata),
        read_output: no_read,
        read_input: no_read,
        write_input,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        write_slot: None,
    };

    #[test]
    fn generic_input_ports_are_listed_and_written_through_the_registry() {
        let mut world = World::new();
        let entity = world.spawn(InputPorts::new(&["throttle", "arm"])).id();
        let registry = PortRegistry::default();

        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.0)
        );
        assert!(registry.write_port(&mut world, entity, "throttle", 0.75));
        assert!(!registry.write_port(&mut world, entity, "undeclared", 1.0));
        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.75)
        );

        let ports = registry.entity_ports(&world, entity);
        assert!(ports
            .iter()
            .any(|port| port.name == "arm" && port.direction == super::PortDirection::In));
    }

    #[test]
    fn topology_key_ignores_live_values_but_tracks_port_identity() {
        let mut world = World::new();
        let entity = world.spawn(InputPorts::new(&["throttle"])).id();
        let registry = PortRegistry::default();
        let (handle, info) = registry
            .entity_port_infos_with_handles(&world, entity)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(info.name, "throttle");
        let initial = registry.entity_port_topology_key(&world, entity);

        assert!(registry.write_port(&mut world, entity, "throttle", 0.75));
        assert_eq!(registry.entity_port_topology_key(&world, entity), initial);
        assert_eq!(
            registry.read_port_for_handle(&world, handle, entity, "throttle", PortDirection::In,),
            Some(0.75)
        );

        world
            .get_mut::<InputPorts>(entity)
            .unwrap()
            .values
            .insert("steer".into(), 0.0);
        assert_ne!(registry.entity_port_topology_key(&world, entity), initial);
    }

    #[test]
    fn registry_discovers_backend_owned_entities_once() {
        let mut world = World::new();
        let first = world.spawn(InputPorts::new(&["first"])).id();
        let second = world.spawn(InputPorts::new(&["second"])).id();
        world.spawn_empty();

        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |world, out| {
                out.extend(
                    world
                        .query_filtered::<Entity, With<InputPorts>>()
                        .iter(world),
                );
            },
            topology_key: |_world, _entity| 0,
            list: duplicate_input_list,
            metadata: None,
            read_output: no_read,
            read_input: no_read,
            write_input,
            resolve_output: None,
            resolve_input: None,
            read_slot: None,
            write_slot: None,
        });

        let entities = registry.port_entities(&mut world);
        assert_eq!(entities.len(), 2);
        assert!(entities.contains(&first));
        assert!(entities.contains(&second));
    }

    #[test]
    fn generic_input_metadata_exposes_control_bounds_and_write_contract() {
        let mut world = World::new();
        let entity = world
            .spawn(InputPorts::new(&["throttle", "arm", "speed_boost"]))
            .id();
        let registry = PortRegistry::default();

        let infos = registry.entity_port_infos(&world, entity);
        let throttle = infos.iter().find(|port| port.name == "throttle").unwrap();
        assert_eq!(throttle.metadata.value_type, "scalar");
        assert_eq!(throttle.metadata.min, Some(-1.0));
        assert_eq!(throttle.metadata.max, Some(1.0));
        assert_eq!(throttle.metadata.source, "control surface");
        assert!(throttle.metadata.writable);
        assert!(throttle.metadata.validate(1.0).is_ok());
        assert!(throttle.metadata.validate(1.01).is_err());

        let speed_boost = infos
            .iter()
            .find(|port| port.name == "speed_boost")
            .unwrap();
        assert_eq!(speed_boost.metadata.min, Some(0.0));
        assert_eq!(speed_boost.metadata.max, Some(1.0));

        let arm = infos.iter().find(|port| port.name == "arm").unwrap();
        assert_eq!(arm.metadata.min, None);
        assert_eq!(arm.metadata.max, None);
        assert!(arm.metadata.writable);
    }

    #[test]
    fn registry_reports_distinct_input_owners_in_write_precedence_order() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut registry = PortRegistry::default();
        registry.register(OWNER_A_INPUT);
        registry.register(OWNER_B_INPUT);

        let collisions = registry.entity_port_collisions(&world, entity);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].direction, PortCollisionDirection::Input);
        assert_eq!(collisions[0].name, "release");
        assert_eq!(collisions[0].owners.len(), 2);
        assert_eq!(collisions[0].owners[0].metadata.source, "Modelica/OBC");
        assert_eq!(collisions[0].owners[0].precedence, 1);
        assert_eq!(
            collisions[0].owners[1].metadata.source,
            "dock/runtime actuator"
        );
        assert_eq!(collisions[0].owners[1].precedence, 2);
        assert!(registry.write_port(&mut world, entity, "release", 1.0));
    }

    #[test]
    fn registry_reports_inout_collision_once_for_both_access_sides() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut registry = PortRegistry::default();
        registry.register(OWNER_A_INOUT);
        registry.register(OWNER_B_INOUT);

        let collisions = registry.entity_port_collisions(&world, entity);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].direction, PortCollisionDirection::InOut);
        assert_eq!(collisions[0].owners.len(), 2);
    }
}
